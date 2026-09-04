//! One `match` over [`Op`], and the arms it dispatches to.
//!
//! Arms are grouped by the phase B slice that owns their opcodes so that a
//! slice landing new decodings extends one group here rather than threading
//! through the whole file. Every opcode the decoder can produce but this build
//! cannot execute falls through to the undefined-instruction trap; that is the
//! working state the plan expects, not a gap.

use super::flags::{add_with_carry, logical_flags};
use super::memory::Memory;
use super::{fetch, Cpu, Flow};
use crate::decode::operand::RegWidth;
use crate::decode::{Form, Instruction, Op, INSN_BYTES};
use crate::reg::Gpr;
use crate::trace::TraceSink;
use crate::trap::Trap;

/// Executes one decoded instruction.
///
/// `pc` is the instruction's own address, which the PC-relative and branch arms
/// need and which the register file no longer holds once an arm writes to it.
pub fn execute<M: Memory, S: TraceSink>(
    cpu: &mut Cpu<M, S>,
    insn: &Instruction,
    pc: u64,
) -> Result<Flow, Trap> {
    match insn.op {
        Op::Add | Op::Sub => arithmetic(cpu, insn),
        Op::And | Op::Orr | Op::Eor | Op::Bic | Op::Orn | Op::Eon => logical(cpu, insn),
        Op::B | Op::Bl => branch_immediate(cpu, insn, pc),
        Op::Br | Op::Blr | Op::Ret => branch_register(cpu, insn, pc),
        Op::Nop => Ok(Flow::Next),
        Op::Ldr | Op::Str => load_store(cpu, insn, pc),
        Op::Svc => Err(supervisor_call(insn, pc)),
        Op::Brk => Err(breakpoint(insn, pc)),
        _ => Err(Trap::undefined(pc, insn)),
    }
}

/// `ADD`, `SUB` and their flag-setting forms, across every operand shape.
///
/// One arm for all three shapes because they differ only in how the second
/// operand is produced; the datapath after that is identical.
fn arithmetic<M: Memory, S: TraceSink>(
    cpu: &mut Cpu<M, S>,
    insn: &Instruction,
) -> Result<Flow, Trap> {
    let width = insn.width;
    let (rd, rn, operand) = match insn.form {
        Form::RegImm { rd, rn, imm } => (rd, rn, imm),
        Form::RegShifted { rd, rn, rm } => (rd, rn, fetch::read_shifted(&cpu.regs, rm, width)),
        Form::RegExtended { rd, rn, rm } => (rd, rn, fetch::read_extended(&cpu.regs, rm)),
        _ => return Err(Trap::undefined(cpu.regs.pc(), insn)),
    };

    let left = fetch::read_gpr(&cpu.regs, rn, width);
    let is_sub = insn.op == Op::Sub;
    let right = if is_sub { !operand } else { operand };
    let result = add_with_carry((left, right), is_sub, width);

    write_result(cpu, rd, result.value, width);
    if insn.sets_flags {
        cpu.regs.pstate.nzcv = result.flags;
    }
    Ok(Flow::Next)
}

/// The six bitwise opcodes, immediate and shifted-register forms.
fn logical<M: Memory, S: TraceSink>(cpu: &mut Cpu<M, S>, insn: &Instruction) -> Result<Flow, Trap> {
    let width = insn.width;
    let (rd, rn, operand) = match insn.form {
        Form::RegImm { rd, rn, imm } => (rd, rn, imm),
        Form::RegShifted { rd, rn, rm } => (rd, rn, fetch::read_shifted(&cpu.regs, rm, width)),
        _ => return Err(Trap::undefined(cpu.regs.pc(), insn)),
    };

    let left = fetch::read_gpr(&cpu.regs, rn, width);
    let value = match insn.op {
        Op::And => left & operand,
        Op::Bic => left & !operand,
        Op::Orr => left | operand,
        Op::Orn => left | !operand,
        Op::Eor => left ^ operand,
        _ => left ^ !operand,
    };

    write_result(cpu, rd, value, width);
    if insn.sets_flags {
        cpu.regs.pstate.nzcv = logical_flags(value, width);
    }
    Ok(Flow::Next)
}

/// `B` and `BL`.
fn branch_immediate<M: Memory, S: TraceSink>(
    cpu: &mut Cpu<M, S>,
    insn: &Instruction,
    pc: u64,
) -> Result<Flow, Trap> {
    let Form::Branch { offset } = insn.form else {
        return Err(Trap::undefined(pc, insn));
    };

    if insn.op == Op::Bl {
        cpu.regs.write_x(Gpr::X(30), pc.wrapping_add(INSN_BYTES));
    }
    cpu.regs.set_pc(pc.wrapping_add(offset as u64));
    Ok(Flow::Branched)
}

/// `BR`, `BLR` and `RET`.
fn branch_register<M: Memory, S: TraceSink>(
    cpu: &mut Cpu<M, S>,
    insn: &Instruction,
    pc: u64,
) -> Result<Flow, Trap> {
    let Form::BranchIndirect { rn } = insn.form else {
        return Err(Trap::undefined(pc, insn));
    };

    let target = cpu.regs.read_x(rn);
    if insn.op == Op::Blr {
        cpu.regs.write_x(Gpr::X(30), pc.wrapping_add(INSN_BYTES));
    }
    cpu.regs.set_pc(target);
    Ok(Flow::Branched)
}

/// `LDR` and `STR`, single-register forms.
///
/// Write-back is applied only after the access succeeds: a faulting access
/// must leave the base register as the guest left it.
fn load_store<M: Memory, S: TraceSink>(
    cpu: &mut Cpu<M, S>,
    insn: &Instruction,
    pc: u64,
) -> Result<Flow, Trap> {
    let Form::LoadStore {
        rt,
        rt2: None,
        addr,
        size,
        ..
    } = insn.form
    else {
        return Err(Trap::undefined(pc, insn));
    };

    let computed = fetch::effective_address(&cpu.regs, addr, pc);
    let is_write = insn.op == Op::Str;
    transfer(cpu, (rt, computed.address, is_write), insn, size)
        .map_err(|_| data_abort(pc, computed.address, is_write))?;

    if let Some((base, value)) = computed.writeback {
        cpu.regs.write_x(base, value);
    }
    Ok(Flow::Next)
}

/// Moves one register's worth of data in the named direction.
fn transfer<M: Memory, S: TraceSink>(
    cpu: &mut Cpu<M, S>,
    access: (Gpr, u64, bool),
    insn: &Instruction,
    size: crate::decode::address::AccessSize,
) -> Result<(), super::AccessFault> {
    let (rt, address, is_write) = access;
    if is_write {
        let value = fetch::read_gpr(&cpu.regs, rt, insn.width);
        return cpu.memory.write_uint(address, size.bytes, value);
    }

    let raw = cpu.memory.read_uint(address, size.bytes)?;
    let value = if size.is_signed {
        sign_extend(raw, size.bytes as u32 * 8)
    } else {
        raw
    };
    write_result(cpu, rt, value, insn.width);
    Ok(())
}

/// `SVC` — the shim's entry point.
fn supervisor_call(insn: &Instruction, pc: u64) -> Trap {
    match insn.form {
        Form::Imm { imm } => Trap::SupervisorCall {
            pc,
            imm: imm as u16,
        },
        _ => Trap::undefined(pc, insn),
    }
}

/// `BRK`.
fn breakpoint(insn: &Instruction, pc: u64) -> Trap {
    match insn.form {
        Form::Imm { imm } => Trap::Breakpoint {
            pc,
            imm: imm as u16,
        },
        _ => Trap::undefined(pc, insn),
    }
}

/// Writes a result at the instruction's operand width.
///
/// A `W`-form write zero-extends, which the register file already does; this
/// exists so the width decision is made in one place rather than in each arm.
fn write_result<M: Memory, S: TraceSink>(
    cpu: &mut Cpu<M, S>,
    rd: Gpr,
    value: u64,
    width: RegWidth,
) {
    match width {
        RegWidth::W32 => cpu.regs.write_w(rd, value as u32),
        RegWidth::X64 => cpu.regs.write_x(rd, value),
    }
}

const fn data_abort(pc: u64, address: u64, is_write: bool) -> Trap {
    Trap::DataAbort {
        pc,
        address,
        is_write,
    }
}

const fn sign_extend(value: u64, bits: u32) -> u64 {
    if bits >= 64 {
        value
    } else {
        let shift = 64 - bits;
        (((value << shift) as i64) >> shift) as u64
    }
}
