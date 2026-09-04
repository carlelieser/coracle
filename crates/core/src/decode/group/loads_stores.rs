//! Owned by the memory slice.
//!
//! Every general-purpose load and store: the immediate, register-offset,
//! literal and pair addressing modes, the exclusives, and the acquire/release
//! forms. `V = 1` selects the SIMD/FP register file and belongs to the FP/NEON
//! slice, so this module answers only for `V = 0`.
//!
//! The LSE atomic group (`CAS`, `SWP`, `LDADD` and the rest) shares this
//! encoding space but is not advertised by this machine — `docs/machine-spec.md`
//! §2 — so it is left unallocated and faults like any unimplemented encoding.

use super::super::address::{AccessSize, AddrMode, Ordering, WriteBack};
use super::super::instruction::{unallocated, Form, Instruction};
use super::super::op::Op;
use super::super::operand::{ExtendKind, ExtendedReg, RegWidth};
use super::{bits, sign_extend};
use crate::reg::Gpr;

/// `op0 = x1x0` — loads and stores.
///
/// Bits 29..28 pick the family, exactly as the ARM ARM's "Loads and Stores"
/// table does: `00` the exclusives, `01` the literals, `10` the pairs, and `11`
/// the single-register forms, whose addressing mode is then chosen by bits
/// 25..24, 21 and 11..10.
pub fn loads_and_stores(encoding: u32) -> Instruction {
    // V = 1 is the SIMD/FP register file, owned by the FP/NEON slice.
    if bits(encoding, 26, 26) == 1 {
        return unallocated(encoding);
    }

    // The exclusive and literal families both fix bit 24 at 0. Only the pair
    // and single-register forms give it a meaning — the index mode and the
    // unsigned-offset selector respectively.
    let has_reserved_bit_set = bits(encoding, 24, 24) == 1;

    match bits(encoding, 29, 28) {
        0b00 if has_reserved_bit_set => unallocated(encoding),
        0b01 if has_reserved_bit_set => unallocated(encoding),
        0b00 => exclusive(encoding),
        0b01 => literal(encoding),
        0b10 => pair(encoding),
        _ => single_register(encoding),
    }
}

/// The `Rt` field, which names a transferred register and so uses the `ZR`
/// rule.
fn transferred(encoding: u32) -> Gpr {
    Gpr::from_index_zr(bits(encoding, 4, 0) as u8)
}

/// The `Rn` field, which names a base register and so uses the `SP` rule.
fn base(encoding: u32) -> Gpr {
    Gpr::from_index_sp(bits(encoding, 9, 5) as u8)
}

/// Builds a single-register transfer around an address.
fn transfer(encoding: u32, addr: AddrMode, size: AccessSize) -> Form {
    Form::LoadStore {
        rt: transferred(encoding),
        rt2: None,
        rs: None,
        addr,
        size,
        ordering: Ordering::PLAIN,
    }
}

/// What the `opc` field of a single-register form asks for.
///
/// `opc<1>` selects sign-extension, and when it is set the encoding is always a
/// load whose *destination* width comes from `opc<0>`, inverted. When it is
/// clear, `opc<0>` is the load bit and the width follows the access size. The
/// two rules are genuinely different, and conflating them decodes `LDRSB X0` —
/// whose `opc<0>` is clear — as a store.
struct SingleRegister {
    is_load: bool,
    size: AccessSize,
    width: RegWidth,
}

impl SingleRegister {
    /// Decodes `size` and `opc` together, or `None` when the pair names a
    /// prefetch or an unallocated hole.
    ///
    /// `size = 11` with `opc = 1x` is `PRFM`, which transfers nothing; every
    /// other `size`/`opc` pair without a sign-extended form is unallocated.
    fn decode(size_field: u32, opc: u32) -> Option<Self> {
        let is_sign_extending = opc & 0b10 != 0;
        // Only sizes narrower than a doubleword have a sign-extended form.
        // 32-bit LDRSW has no W-destination counterpart.
        let has_signed_form = size_field < 0b11 && !(size_field == 0b10 && opc == 0b11);
        if is_sign_extending && !has_signed_form {
            return None;
        }

        let is_load = if is_sign_extending {
            true
        } else {
            opc & 1 == 1
        };
        let width = if is_sign_extending {
            RegWidth::from_sf(opc & 1 == 0)
        } else {
            RegWidth::from_sf(size_field == 0b11)
        };

        Some(Self {
            is_load,
            size: AccessSize {
                bytes: 1 << size_field,
                is_signed: is_sign_extending,
            },
            width,
        })
    }

    /// The opcode this transfer dispatches on.
    fn op(&self) -> Op {
        if self.is_load {
            Op::Ldr
        } else {
            Op::Str
        }
    }
}

/// Whether `size = 11` with this `opc` names a prefetch rather than a transfer.
///
/// `PRFM` and `PRFUM` occupy the doubleword slot the sign-extended loads would
/// otherwise use, which is why they need no opcode of their own.
const fn is_prefetch(size_field: u32, opc: u32) -> bool {
    size_field == 0b11 && opc == 0b10
}

/// Load/store register: the unsigned-offset, unscaled, indexed and
/// register-offset forms, which share `size` and `opc` and differ only in how
/// the address is formed.
fn single_register(encoding: u32) -> Instruction {
    let size_field = bits(encoding, 31, 30);
    let opc = bits(encoding, 23, 22);
    let Some(addr) = single_register_address(encoding, size_field) else {
        return unallocated(encoding);
    };

    if is_prefetch(size_field, opc) {
        // Write-back updates the base register, and a prefetch transfers
        // nothing, so only the non-indexed forms are allocated.
        if addr.has_writeback() {
            return unallocated(encoding);
        }
        let form = Form::Prefetch {
            prfop: bits(encoding, 4, 0) as u8,
            addr,
        };
        return Instruction::new(encoding, Op::Prfm, form);
    }

    let Some(transferred) = SingleRegister::decode(size_field, opc) else {
        return unallocated(encoding);
    };
    let form = transfer(encoding, addr, transferred.size);
    Instruction::new(encoding, transferred.op(), form).with_width(transferred.width)
}

/// The address of a single-register form, or `None` for the encodings this
/// slice does not claim.
///
/// Bit 24 selects the unsigned scaled offset. Below it, bit 21 with `11` in
/// bits 11..10 is the register offset, and bit 21 clear selects the unscaled
/// and indexed forms by those same two bits. Everything else in the space is
/// LSE, which this machine does not advertise.
fn single_register_address(encoding: u32, size_field: u32) -> Option<AddrMode> {
    if bits(encoding, 24, 24) == 1 {
        let scale = size_field;
        return Some(AddrMode::Immediate {
            base: base(encoding),
            offset: ((bits(encoding, 21, 10) as u64) << scale) as i64,
            writeback: WriteBack::None,
        });
    }

    if bits(encoding, 21, 21) == 1 {
        // Only option = 11 in bits 11..10 is the register offset; the other
        // three are the LSE atomic group.
        if bits(encoding, 11, 10) != 0b10 {
            return None;
        }
        return register_offset(encoding, size_field);
    }

    let offset = sign_extend(bits(encoding, 20, 12), 9);
    let writeback = match bits(encoding, 11, 10) {
        0b00 => WriteBack::None,
        0b01 => WriteBack::Post,
        0b11 => WriteBack::Pre,
        // 10 is the unprivileged LDTR/STTR family, which needs EL1 and
        // arrives with the MMU in M2.
        _ => return None,
    };

    Some(AddrMode::Immediate {
        base: base(encoding),
        offset,
        writeback,
    })
}

/// `[base, Xm, extend #amount]`, or `None` when `option` names no index width.
///
/// The `S` bit selects a shift of `log2(access size)` rather than a fixed
/// amount, so a byte access scales by zero even with `S` set.
///
/// `option<1>` must be set: an index register is read as a word or a
/// doubleword, and the byte and halfword extensions the field could otherwise
/// name have no encoding here.
fn register_offset(encoding: u32, size_field: u32) -> Option<AddrMode> {
    let option = bits(encoding, 15, 13) as u8;
    if option & 0b010 == 0 {
        return None;
    }

    let is_scaled = bits(encoding, 12, 12) == 1;
    let amount = if is_scaled { size_field as u8 } else { 0 };

    Some(AddrMode::Register {
        base: base(encoding),
        index: ExtendedReg {
            reg: Gpr::from_index_zr(bits(encoding, 20, 16) as u8),
            kind: ExtendKind::from_option(option),
            amount,
        },
        writeback: WriteBack::None,
    })
}

/// Load register (literal): `LDR`, `LDRSW` and `PRFM` against a PC-relative
/// label.
///
/// `imm19` counts instructions, so the decoder scales it to bytes here and no
/// consumer repeats the multiply.
fn literal(encoding: u32) -> Instruction {
    let addr = AddrMode::PcRelative {
        offset: sign_extend(bits(encoding, 23, 5), 19) * 4,
    };

    // opc: 00 is a 32-bit load, 01 a 64-bit load, 10 a sign-extended word,
    // and 11 a prefetch.
    let (size, width) = match bits(encoding, 31, 30) {
        0b00 => (AccessSize::W, RegWidth::W32),
        0b01 => (AccessSize::X, RegWidth::X64),
        0b10 => (
            AccessSize {
                bytes: 4,
                is_signed: true,
            },
            RegWidth::X64,
        ),
        _ => {
            let form = Form::Prefetch {
                prfop: bits(encoding, 4, 0) as u8,
                addr,
            };
            return Instruction::new(encoding, Op::Prfm, form);
        }
    };

    let form = transfer(encoding, addr, size);
    Instruction::new(encoding, Op::Ldr, form).with_width(width)
}

/// Load/store pair, including the non-temporal forms.
///
/// `imm7` counts registers rather than bytes, so it scales by the access size.
/// `LDPSW` is the one form whose access is narrower than its destination.
fn pair(encoding: u32) -> Instruction {
    let opc = bits(encoding, 31, 30);
    let is_load = bits(encoding, 22, 22) == 1;
    // opc = 01 is LDPSW, a load only; the store half of that slot has no
    // encoding, and opc = 11 is unallocated for the general-purpose file.
    let (size, width) = match (opc, is_load) {
        (0b00, _) => (AccessSize::W, RegWidth::W32),
        (0b01, true) => (
            AccessSize {
                bytes: 4,
                is_signed: true,
            },
            RegWidth::X64,
        ),
        (0b10, _) => (AccessSize::X, RegWidth::X64),
        _ => return unallocated(encoding),
    };

    let is_non_temporal = bits(encoding, 24, 23) == 0b00;
    // LDPSW is allocated only for the offset and indexed forms; there is no
    // LDNPSW, so the signed-word pair has no non-temporal encoding.
    if is_non_temporal && size.is_signed {
        return unallocated(encoding);
    }

    let form = Form::LoadStore {
        rt: transferred(encoding),
        rt2: Some(Gpr::from_index_zr(bits(encoding, 14, 10) as u8)),
        rs: None,
        addr: AddrMode::Immediate {
            base: base(encoding),
            offset: sign_extend(bits(encoding, 21, 15), 7) * size.bytes as i64,
            writeback: pair_writeback(encoding),
        },
        size,
        ordering: Ordering::PLAIN,
    };

    let op = if is_load { Op::Ldp } else { Op::Stp };
    Instruction::new(encoding, op, form).with_width(width)
}

/// How a pair updates its base, from bits 24..23.
///
/// `00` is the non-temporal form, which has no write-back at all — `STNP` is a
/// cache-allocation hint, and this machine has no cache to hint about, so it
/// decodes as an ordinary pair.
const fn pair_writeback(encoding: u32) -> WriteBack {
    match bits(encoding, 24, 23) {
        0b00 | 0b10 => WriteBack::None,
        0b01 => WriteBack::Post,
        _ => WriteBack::Pre,
    }
}

/// Load/store exclusive, and the plain acquire/release forms that share the
/// encoding.
///
/// Bit 23 (`o2`) distinguishes them: clear takes the exclusive monitor, set is
/// a plain `LDAR`/`STLR`. Bit 21 (`o1`) selects the pair forms, and bit 15
/// (`o0`) adds acquire or release ordering.
fn exclusive(encoding: u32) -> Instruction {
    let size_field = bits(encoding, 31, 30);
    let is_load = bits(encoding, 22, 22) == 1;
    let is_exclusive = bits(encoding, 23, 23) == 0;
    let is_pair = bits(encoding, 21, 21) == 1;
    let is_ordered = bits(encoding, 15, 15) == 1;

    let Some(size) = exclusive_size(size_field, is_pair) else {
        return unallocated(encoding);
    };
    // The plain acquire/release forms have no pair encoding, and every form
    // here fixes bits 14..10 to the second transferred register or to 31.
    let is_unallocated_shape = is_pair && !is_exclusive;
    if is_unallocated_shape || (!is_exclusive && !is_ordered) {
        return unallocated(encoding);
    }

    let ordering = Ordering {
        is_acquire: is_load && is_ordered,
        is_release: !is_load && is_ordered,
        is_exclusive,
    };
    // Rs is a status register only on an exclusive store; elsewhere the field
    // reads as 31 and means nothing. ZR cannot stand in for "absent": the
    // status register may legitimately be WZR.
    let rs = (is_exclusive && !is_load).then(|| Gpr::from_index_zr(bits(encoding, 20, 16) as u8));
    let form = Form::LoadStore {
        rt: transferred(encoding),
        rt2: is_pair.then(|| Gpr::from_index_zr(bits(encoding, 14, 10) as u8)),
        rs,
        addr: AddrMode::BaseOnly {
            base: base(encoding),
        },
        size,
        ordering,
    };

    let op = if is_load { Op::Ldar } else { Op::Stlr };
    Instruction::new(encoding, op, form).with_width(RegWidth::from_sf(size_field == 0b11))
}

/// The access width of an exclusive form.
///
/// The pair forms reuse the size field to mean word or doubleword rather than
/// byte or halfword, so `LDXP W0, W1` and `LDXRB W0` share a size field of 00
/// with different widths.
const fn exclusive_size(size_field: u32, is_pair: bool) -> Option<AccessSize> {
    if !is_pair {
        return Some(AccessSize {
            bytes: 1 << size_field,
            is_signed: false,
        });
    }
    match size_field {
        0b10 => Some(AccessSize::W),
        0b11 => Some(AccessSize::X),
        _ => None,
    }
}
