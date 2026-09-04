//! Reading operands out of the register file.
//!
//! The decoder resolved *which* register and *how much* to shift; these apply
//! that to a value. They are separate from the dispatch so that the shifted-,
//! extended- and plain-register arms share one implementation rather than
//! three near-copies.

use crate::decode::address::{AddrMode, WriteBack};
use crate::decode::operand::{ExtendedReg, RegWidth, ShiftKind, ShiftedReg};
use crate::regfile::RegFile;

/// Reads a general-purpose operand at `width`, zero-extended to 64 bits.
pub fn read_gpr(regs: &RegFile, reg: crate::reg::Gpr, width: RegWidth) -> u64 {
    regs.read_x(reg) & width.mask()
}

/// Reads a register operand and applies its constant shift.
///
/// The shift is modulo the operand width, which the decoder has already
/// range-checked, so this does not re-check it.
pub fn read_shifted(regs: &RegFile, operand: ShiftedReg, width: RegWidth) -> u64 {
    let value = read_gpr(regs, operand.reg, width);
    let amount = operand.amount as u32;
    let bits = width.bits();

    let shifted = match operand.kind {
        ShiftKind::Lsl => value << amount,
        ShiftKind::Lsr => value >> amount,
        ShiftKind::Asr => ((sign_extend_to_64(value, bits) as i64) >> amount) as u64,
        ShiftKind::Ror => value.rotate_right(amount) | rotate_carry(value, amount, bits),
    };
    shifted & width.mask()
}

/// Reads a register operand, extends it from its source width, and shifts left.
pub fn read_extended(regs: &RegFile, operand: ExtendedReg) -> u64 {
    let raw = regs.read_x(operand.reg);
    let source_bits = operand.kind.source_bits();
    let extended = if operand.kind.is_signed() {
        sign_extend_to_64(raw, source_bits)
    } else {
        mask_to(raw, source_bits)
    };
    extended << operand.amount
}

/// The address an access forms, and the base write-back it owes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EffectiveAddress {
    /// Address the access uses.
    pub address: u64,
    /// Value to store back into the base register, if any.
    pub writeback: Option<(crate::reg::Gpr, u64)>,
}

/// Computes an access's effective address from the register file and `pc`.
///
/// Write-back is reported rather than applied: a faulting access must leave the
/// base register untouched, and only the caller knows whether the access
/// succeeded.
pub fn effective_address(regs: &RegFile, addr: AddrMode, pc: u64) -> EffectiveAddress {
    let (base_reg, offset, writeback) = match addr {
        AddrMode::Immediate {
            base,
            offset,
            writeback,
        } => (base, offset, writeback),
        AddrMode::Register {
            base,
            index,
            writeback,
        } => (base, read_extended(regs, index) as i64, writeback),
        AddrMode::BaseOnly { base } => (base, 0, WriteBack::None),
        AddrMode::PcRelative { offset } => {
            return EffectiveAddress {
                address: pc.wrapping_add(offset as u64),
                writeback: None,
            }
        }
    };

    let base_value = regs.read_x(base_reg);
    let updated = base_value.wrapping_add(offset as u64);
    let address = match writeback {
        WriteBack::Post => base_value,
        WriteBack::Pre | WriteBack::None => updated,
    };
    EffectiveAddress {
        address,
        writeback: match writeback {
            WriteBack::None => None,
            WriteBack::Pre | WriteBack::Post => Some((base_reg, updated)),
        },
    }
}

/// Keeps the low `bits` of `value`, clearing the rest.
const fn mask_to(value: u64, bits: u32) -> u64 {
    if bits >= 64 {
        value
    } else {
        value & ((1u64 << bits) - 1)
    }
}

/// Replicates bit `bits - 1` of `value` through the upper bits.
const fn sign_extend_to_64(value: u64, bits: u32) -> u64 {
    if bits >= 64 {
        value
    } else {
        let shift = 64 - bits;
        (((value << shift) as i64) >> shift) as u64
    }
}

/// The bits a 32-bit rotate must pull back down from the 64-bit rotation.
const fn rotate_carry(value: u64, amount: u32, bits: u32) -> u64 {
    if bits >= 64 || amount == 0 {
        0
    } else {
        value << (bits - amount % bits)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decode::operand::ExtendKind;
    use crate::reg::Gpr;

    fn regs_with(reg: u8, value: u64) -> RegFile {
        let mut regs = RegFile::new();
        regs.write_x(Gpr::X(reg), value);
        regs
    }

    fn shifted(kind: ShiftKind, amount: u8) -> ShiftedReg {
        ShiftedReg {
            reg: Gpr::X(0),
            kind,
            amount,
        }
    }

    #[test]
    fn an_arithmetic_shift_right_replicates_the_sign_of_the_operand_width() {
        let regs = regs_with(0, 0xffff_ffff_8000_0000);

        assert_eq!(
            read_shifted(&regs, shifted(ShiftKind::Asr, 4), RegWidth::W32),
            0xf800_0000,
            "W form sees only the low 32 bits and their sign"
        );
        assert_eq!(
            read_shifted(&regs, shifted(ShiftKind::Asr, 4), RegWidth::X64),
            0xffff_ffff_f800_0000,
        );
    }

    #[test]
    fn a_32_bit_rotate_wraps_within_32_bits() {
        let regs = regs_with(0, 0x0000_0000_0000_00ff);

        assert_eq!(
            read_shifted(&regs, shifted(ShiftKind::Ror, 4), RegWidth::W32),
            0xf000_000f,
        );
    }

    #[test]
    fn a_logical_shift_left_drops_bits_above_the_operand_width() {
        let regs = regs_with(0, 0xffff_ffff);

        assert_eq!(
            read_shifted(&regs, shifted(ShiftKind::Lsl, 4), RegWidth::W32),
            0xffff_fff0,
        );
    }

    #[test]
    fn a_signed_extension_widens_from_its_source_width_then_scales() {
        let regs = regs_with(1, 0xffff_ffff_ffff_ff80);

        let value = read_extended(
            &regs,
            ExtendedReg {
                reg: Gpr::X(1),
                kind: ExtendKind::Sxtb,
                amount: 2,
            },
        );

        assert_eq!(value, (-128i64 * 4) as u64);
    }

    #[test]
    fn an_unsigned_extension_discards_the_bits_above_its_source_width() {
        let regs = regs_with(1, 0xdead_beef);

        let value = read_extended(
            &regs,
            ExtendedReg {
                reg: Gpr::X(1),
                kind: ExtendKind::Uxth,
                amount: 0,
            },
        );

        assert_eq!(value, 0xbeef);
    }

    #[test]
    fn a_post_indexed_access_uses_the_old_base_and_writes_back_the_new_one() {
        let regs = regs_with(2, 0x1000);
        let addr = AddrMode::Immediate {
            base: Gpr::X(2),
            offset: 8,
            writeback: WriteBack::Post,
        };

        let computed = effective_address(&regs, addr, 0);

        assert_eq!(computed.address, 0x1000);
        assert_eq!(computed.writeback, Some((Gpr::X(2), 0x1008)));
    }

    #[test]
    fn a_pre_indexed_access_uses_the_new_base_and_writes_it_back() {
        let regs = regs_with(2, 0x1000);
        let addr = AddrMode::Immediate {
            base: Gpr::X(2),
            offset: -8,
            writeback: WriteBack::Pre,
        };

        let computed = effective_address(&regs, addr, 0);

        assert_eq!(computed.address, 0x0ff8);
        assert_eq!(computed.writeback, Some((Gpr::X(2), 0x0ff8)));
    }

    #[test]
    fn a_plain_offset_access_owes_no_write_back() {
        let regs = regs_with(2, 0x1000);
        let addr = AddrMode::Immediate {
            base: Gpr::X(2),
            offset: 8,
            writeback: WriteBack::None,
        };

        assert_eq!(
            effective_address(&regs, addr, 0),
            EffectiveAddress {
                address: 0x1008,
                writeback: None,
            }
        );
    }

    #[test]
    fn a_literal_access_is_relative_to_the_instructions_own_address() {
        let regs = RegFile::new();

        let computed = effective_address(&regs, AddrMode::PcRelative { offset: -16 }, 0x8000);

        assert_eq!(computed.address, 0x7ff0);
        assert_eq!(computed.writeback, None);
    }
}
