//! Owned by the integer slice.

use super::super::instruction::{unallocated, Form, Instruction};
use super::super::op::Op;
use super::{bits, sign_extend};
use crate::reg::Gpr;

/// `op0 = 101x` — branches, exception generation and system instructions.
///
/// Owned by the integer slice. Unconditional immediate branches and `RET` are
/// decoded to prove the branch forms; conditional branches, compare-and-branch,
/// the exception-generation group, barriers and `MSR`/`MRS` remain.
pub fn branches_exceptions_system(encoding: u32) -> Instruction {
    // Unconditional branch (immediate): op0=x00 in bits 31..29, 00101 in 30..26.
    if bits(encoding, 30, 26) == 0b00101 {
        let op = if bits(encoding, 31, 31) == 1 {
            Op::Bl
        } else {
            Op::B
        };
        let offset = sign_extend(bits(encoding, 25, 0), 26) * 4;
        return Instruction::new(encoding, op, Form::Branch { offset });
    }

    // RET: 1101011 0 0 10 11111 000000 Rn 00000.
    if encoding & 0xffff_fc1f == 0xd65f_0000 {
        let rn = Gpr::from_index_zr(bits(encoding, 9, 5) as u8);
        return Instruction::new(encoding, Op::Ret, Form::BranchIndirect { rn });
    }

    // NOP: the hint space, encoded as a system instruction with CRm:op2 = 0.
    if encoding == 0xd503_201f {
        return Instruction::new(encoding, Op::Nop, Form::None);
    }

    unallocated(encoding)
}
