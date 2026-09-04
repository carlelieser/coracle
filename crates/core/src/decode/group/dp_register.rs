//! Owned by the integer slice.

use super::super::instruction::{unallocated, Form, Instruction};
use super::super::op::Op;
use super::super::operand::{RegWidth, ShiftKind, ShiftedReg};
use super::bits;
use crate::reg::Gpr;

/// `op0 = x101` — data processing (register).
///
/// Owned by the integer slice. Logical and add/sub shifted-register forms are
/// decoded to prove [`Form::RegShifted`]; extended-register, three-source,
/// conditional select/compare, variable shift and the bit-manipulation group
/// remain.
pub fn data_processing_register(encoding: u32) -> Instruction {
    let is_logical = bits(encoding, 28, 24) == 0b01010 && bits(encoding, 21, 21) == 0;
    let is_add_sub = bits(encoding, 28, 24) == 0b01011 && bits(encoding, 21, 21) == 0;
    if !is_logical && !is_add_sub {
        return unallocated(encoding);
    }

    let rm = ShiftedReg {
        reg: Gpr::from_index_zr(bits(encoding, 20, 16) as u8),
        kind: shift_kind(bits(encoding, 23, 22)),
        amount: bits(encoding, 15, 10) as u8,
    };
    let form = Form::RegShifted {
        rd: Gpr::from_index_zr(bits(encoding, 4, 0) as u8),
        rn: Gpr::from_index_zr(bits(encoding, 9, 5) as u8),
        rm,
    };

    let opc = bits(encoding, 30, 29);
    let is_negated = is_logical && bits(encoding, 21, 21) == 1;
    let op = if is_logical {
        logical_op(opc, is_negated)
    } else if opc & 0b10 != 0 {
        Op::Sub
    } else {
        Op::Add
    };

    let width = RegWidth::from_sf(bits(encoding, 31, 31) == 1);
    let insn = Instruction::new(encoding, op, form).with_width(width);
    // ADDS/SUBS set flags on opc<0>; the logical group sets them only for ANDS,
    // which is opc = 11.
    let sets_flags = if is_logical {
        opc == 0b11
    } else {
        opc & 1 == 1
    };
    if sets_flags {
        insn.setting_flags()
    } else {
        insn
    }
}

const fn shift_kind(field: u32) -> ShiftKind {
    match field & 0b11 {
        0b00 => ShiftKind::Lsl,
        0b01 => ShiftKind::Lsr,
        0b10 => ShiftKind::Asr,
        _ => ShiftKind::Ror,
    }
}

/// Selects the logical opcode from `opc` and the `N` (negate) bit.
const fn logical_op(opc: u32, is_negated: bool) -> Op {
    match (opc & 0b11, is_negated) {
        (0b00, false) => Op::And,
        (0b00, true) => Op::Bic,
        (0b01, false) => Op::Orr,
        (0b01, true) => Op::Orn,
        (0b10, false) => Op::Eor,
        (0b10, true) => Op::Eon,
        (_, false) => Op::And,
        (_, true) => Op::Bic,
    }
}
