//! Owned by the integer slice.

use super::super::instruction::{unallocated, Form, Instruction};
use super::super::op::Op;
use super::super::operand::RegWidth;
use super::bits;
use crate::reg::Gpr;

/// `op0 = 100x` — data processing (immediate).
///
/// Owned by the integer slice. Dispatch is on `op0` — bits 25..23 — exactly as
/// the ARM ARM's "Data Processing -- Immediate" table does.
pub fn data_processing_immediate(encoding: u32) -> Instruction {
    match bits(encoding, 25, 23) {
        0b000 | 0b001 => pc_relative(encoding),
        0b010 => add_sub_immediate(encoding),
        // 0b011 is add/sub (immediate, with tags), which needs MTE. The
        // machine advertises none (docs/machine-spec.md §2).
        0b100 => logical_immediate(encoding),
        0b101 => move_wide(encoding),
        0b110 => bitfield(encoding),
        0b111 => extract(encoding),
        _ => unallocated(encoding),
    }
}

/// `ADR` and `ADRP`.
///
/// The offset is split across two non-adjacent fields, `immhi` and `immlo`,
/// and `ADRP` scales it by the 4 KiB page. Both are resolved here so the
/// interpreter only ever adds a byte offset to a (possibly aligned) PC.
fn pc_relative(encoding: u32) -> Instruction {
    let imm = (bits(encoding, 23, 5) << 2) | bits(encoding, 30, 29);
    let is_page = bits(encoding, 31, 31) == 1;
    let offset = super::sign_extend(imm, 21) * if is_page { 4096 } else { 1 };

    let form = Form::PcRelAddr {
        rd: Gpr::from_index_zr(bits(encoding, 4, 0) as u8),
        offset,
    };
    let op = if is_page { Op::Adrp } else { Op::Adr };
    // Both forms write a full 64-bit address regardless of any other field.
    Instruction::new(encoding, op, form)
}

/// Add/sub (immediate).
fn add_sub_immediate(encoding: u32) -> Instruction {
    let shift = bits(encoding, 23, 22);
    // `sh` selects a 12-bit left shift of the immediate; the reserved values
    // of the field belong to add/sub (immediate, with tags), which needs MTE.
    if shift & 0b10 != 0 {
        return unallocated(encoding);
    }
    let imm = match shift & 1 {
        0 => bits(encoding, 21, 10) as u64,
        _ => (bits(encoding, 21, 10) as u64) << 12,
    };
    let is_sub = bits(encoding, 30, 30) == 1;
    let sets_flags = bits(encoding, 29, 29) == 1;

    // Rd is SP unless the instruction sets flags, in which case slot 31 is the
    // zero register — the difference between `ADD SP, ...` and `CMN`.
    let rd = if sets_flags {
        Gpr::from_index_zr(bits(encoding, 4, 0) as u8)
    } else {
        Gpr::from_index_sp(bits(encoding, 4, 0) as u8)
    };
    let form = Form::RegImm {
        rd,
        rn: Gpr::from_index_sp(bits(encoding, 9, 5) as u8),
        imm,
    };

    let op = if is_sub { Op::Sub } else { Op::Add };
    let insn = Instruction::new(encoding, op, form)
        .with_width(RegWidth::from_sf(bits(encoding, 31, 31) == 1));

    if sets_flags {
        insn.setting_flags()
    } else {
        insn
    }
}

/// Logical (immediate): `AND`, `ORR`, `EOR`, `ANDS`.
fn logical_immediate(encoding: u32) -> Instruction {
    let width = RegWidth::from_sf(bits(encoding, 31, 31) == 1);
    let n = bits(encoding, 22, 22);
    // `N` is part of the bitmask encoding and only the 64-bit form has a
    // 64-bit pattern to name, so `N = 1` with `sf = 0` is unallocated.
    if width == RegWidth::W32 && n == 1 {
        return unallocated(encoding);
    }
    let Some(imm) = decode_bit_masks(
        n as u8,
        bits(encoding, 21, 16) as u8,
        bits(encoding, 15, 10) as u8,
        width,
    ) else {
        return unallocated(encoding);
    };

    let opc = bits(encoding, 30, 29);
    let sets_flags = opc == 0b11;
    // ANDS writes the zero register for slot 31 (that is `TST`); the other
    // three write SP.
    let rd = if sets_flags {
        Gpr::from_index_zr(bits(encoding, 4, 0) as u8)
    } else {
        Gpr::from_index_sp(bits(encoding, 4, 0) as u8)
    };
    let form = Form::RegImm {
        rd,
        rn: Gpr::from_index_zr(bits(encoding, 9, 5) as u8),
        imm,
    };

    let op = match opc {
        0b00 | 0b11 => Op::And,
        0b01 => Op::Orr,
        _ => Op::Eor,
    };
    let insn = Instruction::new(encoding, op, form).with_width(width);
    if sets_flags {
        insn.setting_flags()
    } else {
        insn
    }
}

/// Move wide (immediate): `MOVN`, `MOVZ`, `MOVK`.
fn move_wide(encoding: u32) -> Instruction {
    let width = RegWidth::from_sf(bits(encoding, 31, 31) == 1);
    let hw = bits(encoding, 22, 21) as u8;
    // `hw` selects the halfword; the 32-bit form has only two, so its upper
    // two `hw` values are unallocated rather than aliases.
    if width == RegWidth::W32 && hw & 0b10 != 0 {
        return unallocated(encoding);
    }

    let op = match bits(encoding, 30, 29) {
        0b00 => Op::Movn,
        0b10 => Op::Movz,
        0b11 => Op::Movk,
        _ => return unallocated(encoding),
    };
    let form = Form::MoveWide {
        rd: Gpr::from_index_zr(bits(encoding, 4, 0) as u8),
        imm16: bits(encoding, 20, 5) as u16,
        hw,
    };
    Instruction::new(encoding, op, form).with_width(width)
}

/// Bitfield: `SBFM`, `BFM`, `UBFM`, and every alias built on them.
///
/// The aliases (`LSL`, `LSR`, `ASR`, `SBFX`, `UBFX`, `UXTB`, `SXTW`, …) are
/// all the same three opcodes with particular `immr`/`imms` pairs, so they
/// need no decoding of their own.
fn bitfield(encoding: u32) -> Instruction {
    let width = RegWidth::from_sf(bits(encoding, 31, 31) == 1);
    // `N` must equal `sf`: the field widths only make sense at the operand
    // width, and the architecture leaves the mismatched combinations
    // unallocated.
    if bits(encoding, 22, 22) != bits(encoding, 31, 31) {
        return unallocated(encoding);
    }
    let immr = bits(encoding, 21, 16) as u8;
    let imms = bits(encoding, 15, 10) as u8;
    // Both fields index a bit of the operand, so their top bit is unallocated
    // in the 32-bit form.
    if width == RegWidth::W32 && (immr | imms) & 0b10_0000 != 0 {
        return unallocated(encoding);
    }

    let op = match bits(encoding, 30, 29) {
        0b00 => Op::Sbfm,
        0b01 => Op::Bfm,
        0b10 => Op::Ubfm,
        _ => return unallocated(encoding),
    };
    let rn = Gpr::from_index_zr(bits(encoding, 9, 5) as u8);
    let form = Form::Bitfield {
        rd: Gpr::from_index_zr(bits(encoding, 4, 0) as u8),
        rn,
        // Bitfield has one source; `rm` exists for `EXTR`, so it repeats `rn`
        // rather than inventing a register the encoding does not name.
        rm: rn,
        immr,
        imms,
    };
    Instruction::new(encoding, op, form).with_width(width)
}

/// Extract: `EXTR`, and its `ROR` alias where `Rn` equals `Rm`.
fn extract(encoding: u32) -> Instruction {
    let width = RegWidth::from_sf(bits(encoding, 31, 31) == 1);
    // op21 = 00, o0 = 0 is the only allocated combination; `N` must equal `sf`.
    if bits(encoding, 30, 29) != 0
        || bits(encoding, 21, 21) != 0
        || bits(encoding, 22, 22) != bits(encoding, 31, 31)
    {
        return unallocated(encoding);
    }
    let imms = bits(encoding, 15, 10) as u8;
    // `imms` is the rotate amount, so its top bit is unallocated at 32 bits.
    if width == RegWidth::W32 && imms & 0b10_0000 != 0 {
        return unallocated(encoding);
    }

    let form = Form::Bitfield {
        rd: Gpr::from_index_zr(bits(encoding, 4, 0) as u8),
        rn: Gpr::from_index_zr(bits(encoding, 9, 5) as u8),
        rm: Gpr::from_index_zr(bits(encoding, 20, 16) as u8),
        // `EXTR` names its rotate in `imms`; `immr` is not part of the
        // encoding, so it stays zero rather than carrying a stale field.
        immr: 0,
        imms,
    };
    Instruction::new(encoding, Op::Extr, form).with_width(width)
}

/// Expands the `N:immr:imms` bitmask encoding into the value it names.
///
/// This is the architecture's `DecodeBitMasks` with `immediate = TRUE`,
/// resolved once at decode time so no consumer re-implements it. Returns
/// `None` for the encodings the architecture leaves unallocated: an all-ones
/// `imms` within the selected element, and an element size the `N:imms`
/// prefix does not name.
fn decode_bit_masks(n: u8, immr: u8, imms: u8, width: RegWidth) -> Option<u64> {
    // The element size is `1 << len`, where `len` is the position of the
    // highest set bit of `N:NOT(imms)`. An `imms` of all ones with `N` clear
    // leaves that value zero, which names no element size.
    let combined = ((n as u32) << 6) | (!(imms as u32) & 0b11_1111);
    if combined == 0 {
        return None;
    }
    let len = 31 - combined.leading_zeros();
    // A 32-bit operand has no 64-bit element to replicate.
    if width == RegWidth::W32 && len == 6 {
        return None;
    }

    let esize = 1u32 << len;
    let level = (esize - 1) as u8;
    let s = imms & level;
    // `S` is the number of set bits minus one, so all-ones is reserved.
    if s == level {
        return None;
    }
    let r = (immr & level) as u32;

    // The element is `s + 1` low ones rotated right within `esize` bits, not
    // within 64: `ROR` on the full register would pull in bits from outside
    // the element.
    let ones = (1u64 << (s as u32 + 1)) - 1;
    let element = if r == 0 {
        ones
    } else {
        ((ones >> r) | (ones << (esize - r))) & mask_of(esize)
    };

    // Replicate the element across the operand width.
    let mut value = element;
    let mut filled = esize;
    while filled < width.bits() {
        value |= value << filled;
        filled *= 2;
    }
    Some(value & width.mask())
}

/// Mask of the low `bits` bits, for `bits` in `1..=64`.
const fn mask_of(bits: u32) -> u64 {
    if bits >= 64 {
        u64::MAX
    } else {
        (1u64 << bits) - 1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The ARM ARM's `DecodeBitMasks` pseudocode, transcribed directly.
    ///
    /// Kept separate from the decoder so the exhaustive sweep below compares
    /// two independent readings of the same spec rather than one against
    /// itself.
    fn reference(n: u8, immr: u8, imms: u8, width: RegWidth) -> Option<u64> {
        let bits = width.bits();
        for len in (0..=6u32).rev() {
            if len == 6 && n != 1 {
                continue;
            }
            if len < 6 && (n != 0 || (!imms >> len) & 1 == 0) {
                continue;
            }
            if 1u32 << len > bits {
                return None;
            }
            let esize = 1u32 << len;
            let s = (imms as u32) & (esize - 1);
            let r = (immr as u32) & (esize - 1);
            if s == esize - 1 {
                return None;
            }
            let mut welem = 0u64;
            for i in 0..=s {
                welem |= 1 << i;
            }
            let mut rotated = 0u64;
            for i in 0..esize {
                if welem >> ((i + r) % esize) & 1 == 1 {
                    rotated |= 1 << i;
                }
            }
            let mut value = 0u64;
            let mut at = 0;
            while at < bits {
                value |= rotated << at;
                at += esize;
            }
            return Some(value & width.mask());
        }
        None
    }

    #[test]
    fn the_bitmask_decoder_matches_the_architecture_over_every_encoding() {
        // 2 * 64 * 64 * 2 combinations, so exhaustive is cheap and there is no
        // reason to sample.
        for n in 0..2u8 {
            for immr in 0..64u8 {
                for imms in 0..64u8 {
                    for width in [RegWidth::W32, RegWidth::X64] {
                        assert_eq!(
                            decode_bit_masks(n, immr, imms, width),
                            reference(n, immr, imms, width),
                            "N={n} immr={immr} imms={imms} width={width:?}"
                        );
                    }
                }
            }
        }
    }
}
