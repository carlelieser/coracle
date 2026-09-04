//! The interpreter loop.
//!
//! Structure, in the order the hot path runs:
//!
//! 1. Fetch a 32-bit word at PC.
//! 2. Look it up in the decoded-instruction cache, decoding on a miss. The
//!    cache is what `docs/plan.md` asks for "from day one", and it is what
//!    keeps decode off the hot path once a loop is warm.
//! 3. Dispatch on [`crate::decode::Op`] in one `match`, destructuring the
//!    [`crate::decode::Form`] the opcode expects inside its arm.
//! 4. Advance PC, unless the arm set it.
//!
//! Dispatch is keyed on `Op` rather than on `Form` because `Op` is a fieldless
//! enum that rustc lowers to a jump table, whereas `Form` carries payloads and
//! a match on it becomes a compare chain. Fetching operands before knowing the
//! opcode would also mean fetching operands for instructions that do not read
//! them.
//!
//! An opcode this build does not implement raises
//! [`crate::trap::Trap::UndefinedInstruction`] and is recorded in the
//! [`crate::trap::TrapLog`]. Reaching one is never a panic: the M1 gate runs a
//! fuzz corpus of random words through exactly this path.

mod cache;
mod dispatch;
pub mod fetch;
pub mod flags;
pub mod memory;

pub use cache::{DecodeCache, DECODE_CACHE_ENTRIES};
pub use memory::{AccessFault, FlatMemory, Memory};

use crate::decode::INSN_BYTES;
use crate::regfile::RegFile;
use crate::trace::{NullSink, TraceSink};
use crate::trap::{Trap, TrapLog};

/// Why [`Cpu::run`] stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stop {
    /// The instruction budget was exhausted. Re-entering resumes.
    BudgetExhausted,
    /// A trap the host must service. PC still points at the trapping
    /// instruction, so a handler that services it advances PC itself.
    Trapped(Trap),
}

/// A single vCPU: architectural state, its memory, and the loop over both.
///
/// Generic over [`Memory`] rather than holding a trait object: the load and
/// store arms are the hot path and an indirect call per access is measurable.
/// The trace sink is a type parameter for the same reason — a build without
/// tracing monomorphises [`NullSink`]'s empty bodies away entirely.
#[derive(Debug)]
pub struct Cpu<M: Memory, S: TraceSink = NullSink> {
    /// Architectural register file.
    pub regs: RegFile,
    /// Guest memory.
    pub memory: M,
    /// Unimplemented encodings reached so far.
    pub trap_log: TrapLog,
    /// Trace sink, per `tests/EMULATOR_INTERFACE.md`.
    pub sink: S,
    cache: DecodeCache,
    icount: u64,
}

impl<M: Memory> Cpu<M, NullSink> {
    /// A CPU over `memory` with every register zeroed and tracing discarded.
    pub fn new(memory: M) -> Self {
        Self::with_sink(memory, NullSink)
    }
}

impl<M: Memory, S: TraceSink> Cpu<M, S> {
    /// A CPU over `memory` reporting execution to `sink`.
    pub fn with_sink(memory: M, sink: S) -> Self {
        Self {
            regs: RegFile::new(),
            memory,
            trap_log: TrapLog::new(),
            sink,
            cache: DecodeCache::new(),
            icount: 0,
        }
    }

    /// Instructions retired since this CPU was created.
    pub const fn icount(&self) -> u64 {
        self.icount
    }

    /// Instructions that had to be decoded rather than served from the cache.
    ///
    /// Exposed because the decoded-instruction cache is what the M1
    /// performance risk rests on: throughput alone would not notice the cache
    /// silently missing on a fast enough machine.
    pub const fn decode_count(&self) -> u64 {
        self.cache.misses()
    }

    /// Runs until a trap or until `budget` instructions have retired.
    ///
    /// `budget` bounds the loop so the single-threaded build can return to the
    /// host, and so a guest that never traps cannot wedge the tab.
    pub fn run(&mut self, budget: u64) -> Stop {
        for _ in 0..budget {
            if let Err(trap) = self.step() {
                return Stop::Trapped(trap);
            }
        }
        Stop::BudgetExhausted
    }

    /// Executes exactly one instruction.
    pub fn step(&mut self) -> Result<(), Trap> {
        let pc = self.regs.pc();
        let encoding = self.fetch(pc)?;
        let insn = self.cache.decoded(pc, encoding);

        let outcome = dispatch::execute(self, &insn, pc);
        match outcome {
            Ok(Flow::Next) => self.regs.set_pc(pc.wrapping_add(INSN_BYTES)),
            Ok(Flow::Branched) => {}
            Err(trap) => {
                self.trap_log.record_trap(trap);
                return Err(trap);
            }
        }

        self.sink.on_block(pc, self.icount, &[]);
        self.icount += 1;
        Ok(())
    }

    fn fetch(&self, pc: u64) -> Result<u32, Trap> {
        let mut word = [0u8; 4];
        self.memory
            .read(pc, &mut word)
            .map_err(|_| Trap::InstructionAbort { pc })?;
        Ok(u32::from_le_bytes(word))
    }
}

/// Whether an instruction set PC itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Flow {
    /// PC advances to the next instruction.
    Next,
    /// The arm already wrote PC.
    Branched,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::guest_memory::PHYS_RAM_BASE;
    use crate::reg::Gpr;

    fn cpu_running(program: &[u32]) -> Cpu<FlatMemory> {
        let mut memory = FlatMemory::new(4096);
        for (index, word) in program.iter().enumerate() {
            memory
                .write_uint(PHYS_RAM_BASE + index as u64 * INSN_BYTES, 4, *word as u64)
                .expect("program fits");
        }
        let mut cpu = Cpu::new(memory);
        cpu.regs.set_pc(PHYS_RAM_BASE);
        cpu
    }

    #[test]
    fn a_straight_line_program_retires_one_instruction_per_step() {
        // add x0, x0, #1  ×3
        let mut cpu = cpu_running(&[0x9100_0400, 0x9100_0400, 0x9100_0400]);

        assert_eq!(cpu.run(3), Stop::BudgetExhausted);

        assert_eq!(cpu.regs.read_x(Gpr::X(0)), 3);
        assert_eq!(cpu.icount(), 3);
        assert_eq!(cpu.regs.pc(), PHYS_RAM_BASE + 12);
    }

    #[test]
    fn an_unimplemented_opcode_traps_and_is_logged_rather_than_panicking() {
        // A NEON encoding no slice has claimed.
        let mut cpu = cpu_running(&[0x4e20_8400]);

        let stop = cpu.run(16);

        assert_eq!(
            stop,
            Stop::Trapped(Trap::UndefinedInstruction {
                pc: PHYS_RAM_BASE,
                encoding: 0x4e20_8400,
            })
        );
        assert_eq!(cpu.regs.pc(), PHYS_RAM_BASE, "PC stays at the trap");
        assert_eq!(cpu.trap_log.entries().count(), 1);
    }

    #[test]
    fn fetching_outside_mapped_memory_is_an_instruction_abort() {
        let mut cpu = cpu_running(&[]);
        cpu.regs.set_pc(0);

        assert_eq!(cpu.run(1), Stop::Trapped(Trap::InstructionAbort { pc: 0 }));
    }

    #[test]
    fn the_budget_bounds_the_loop_even_when_the_guest_never_traps() {
        // b .  — an infinite loop.
        let mut cpu = cpu_running(&[0x1400_0000]);

        assert_eq!(cpu.run(1000), Stop::BudgetExhausted);

        assert_eq!(cpu.icount(), 1000);
        assert_eq!(cpu.regs.pc(), PHYS_RAM_BASE);
    }

    #[test]
    fn a_trapped_run_resumes_where_it_stopped() {
        // add; <unimplemented>; add
        let mut cpu = cpu_running(&[0x9100_0400, 0x4e20_8400, 0x9100_0400]);

        assert!(matches!(cpu.run(16), Stop::Trapped(_)));
        assert_eq!(cpu.icount(), 1);

        // The host services the trap by stepping over it.
        cpu.regs.set_pc(cpu.regs.pc() + INSN_BYTES);

        assert_eq!(cpu.run(1), Stop::BudgetExhausted);
        assert_eq!(cpu.regs.read_x(Gpr::X(0)), 2);
    }
}
