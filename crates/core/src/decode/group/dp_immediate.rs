//! Owned by the integer slice.

use super::super::instruction::{unallocated, Form, Instruction};
use super::super::op::Op;
use super::super::operand::RegWidth;
use super::bits;
use crate::reg::Gpr;

/// `op0 = 100x` — data processing (immediate).
///
/// Owned by the integer slice. Add/sub (immediate) is decoded to prove the
/// [`Form::RegImm`] path; move-wide, bitfield, extract, logical (immediate) and
/// PC-relative addressing remain.
pub fn data_processing_immediate(encoding: u32) -> Instruction {
    // Add/sub (immediate): op0=100, op1=010 in bits 25..23.
    if bits(encoding, 25, 23) != 0b010 {
        return unallocated(encoding);
    }

    let shift = bits(encoding, 23, 22);
    // `sh` selects a 12-bit left shift of the immediate; other values of the
    // field belong to add/sub (immediate, with tags), which needs MTE.
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
