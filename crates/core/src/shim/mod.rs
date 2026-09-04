//! The throwaway Linux syscall shim.
//!
//! `docs/plan.md` M1 asks for roughly 40 syscalls so a static binary runs
//! without a kernel, and deletes the whole thing after M2. It exists to
//! exercise the CPU, so it favours directness over structure worth keeping:
//! the fd table is the host's, `munmap` reclaims nothing, and `fstat` fills in
//! the handful of fields musl actually reads.
//!
//! `clone` and `futex` are refused by name rather than merely unimplemented.
//! The shim is single-threaded by decision, not by omission, and an `ENOSYS`
//! that looked like a gap would send someone to implement them.
//!
//! Conventions, from the AArch64 Linux ABI: the number is in `x8`, arguments
//! in `x0`–`x5`, the result in `x0`, and a failure is `-errno`.

mod dispatch;
mod host;
mod memory_map;
pub mod number;
mod structs;

pub use host::{CapturingHost, Errno, HostIo, SysResult};
pub use memory_map::{ArenaLayout, MemoryMap};

use crate::decode::INSN_BYTES;
use crate::interp::{Cpu, Memory, Stop};
use crate::reg::Gpr;
use crate::trace::TraceSink;
use crate::trap::Trap;

/// Syscall numbers reached that the shim does not implement.
///
/// Bounded and deduplicated for the same reason [`crate::trap::TrapLog`] is:
/// a guest in a retry loop must not grow this without limit.
pub const UNHANDLED_CAPACITY: usize = 32;

/// Why the guest stopped running under the shim.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShimExit {
    /// The guest called `exit` or `exit_group`.
    Exited(i32),
    /// The guest took a trap the shim cannot service.
    Trapped(Trap),
    /// The instruction budget ran out before the guest exited.
    BudgetExhausted,
}

/// A Linux personality for one guest process.
#[derive(Debug)]
pub struct Shim<H: HostIo> {
    /// The world outside the guest.
    pub host: H,
    /// Heap and anonymous-mapping arena.
    pub map: MemoryMap,
    unhandled: [Option<u64>; UNHANDLED_CAPACITY],
    unhandled_count: usize,
}

impl<H: HostIo> Shim<H> {
    /// A shim over `host` handing out memory from `layout`.
    pub const fn new(host: H, layout: ArenaLayout) -> Self {
        Self {
            host,
            map: MemoryMap::new(layout),
            unhandled: [None; UNHANDLED_CAPACITY],
            unhandled_count: 0,
        }
    }

    /// Syscall numbers the guest asked for that this build does not implement.
    pub fn unhandled(&self) -> impl Iterator<Item = u64> + '_ {
        self.unhandled[..self.unhandled_count]
            .iter()
            .flatten()
            .copied()
    }

    /// Runs the guest, servicing supervisor calls, until it exits.
    ///
    /// The budget bounds total instructions across all resumptions, so a guest
    /// that never exits still returns control.
    pub fn run<M: Memory, S: TraceSink>(&mut self, cpu: &mut Cpu<M, S>, budget: u64) -> ShimExit {
        // `icount` is the CPU's lifetime total, so the budget is tracked
        // against where it started rather than by subtracting it each pass.
        let deadline = cpu.icount().saturating_add(budget);
        while cpu.icount() < deadline {
            match cpu.run(deadline - cpu.icount()) {
                Stop::BudgetExhausted => return ShimExit::BudgetExhausted,
                Stop::Trapped(Trap::SupervisorCall { pc, .. }) => {
                    if let Some(status) = self.service(cpu) {
                        return ShimExit::Exited(status);
                    }
                    cpu.regs.set_pc(pc.wrapping_add(INSN_BYTES));
                }
                Stop::Trapped(trap) => return ShimExit::Trapped(trap),
            }
        }
        ShimExit::BudgetExhausted
    }

    /// Services one supervisor call, returning an exit status if it was `exit`.
    ///
    /// PC is left on the `SVC`; advancing past it is the caller's job, because
    /// only the caller knows whether the guest is resuming.
    pub fn service<M: Memory, S: TraceSink>(&mut self, cpu: &mut Cpu<M, S>) -> Option<i32> {
        let number = cpu.regs.read_x(Gpr::X(8));
        let args = [
            cpu.regs.read_x(Gpr::X(0)),
            cpu.regs.read_x(Gpr::X(1)),
            cpu.regs.read_x(Gpr::X(2)),
            cpu.regs.read_x(Gpr::X(3)),
            cpu.regs.read_x(Gpr::X(4)),
            cpu.regs.read_x(Gpr::X(5)),
        ];

        match dispatch::call(self, cpu, number, &args) {
            dispatch::Outcome::Returned(result) => {
                let value = result.unwrap_or_else(Errno::to_return_value);
                cpu.regs.write_x(Gpr::X(0), value);
                None
            }
            dispatch::Outcome::Exited(status) => {
                self.host.exit(status);
                Some(status)
            }
        }
    }

    /// Records a syscall number this build does not implement.
    fn record_unhandled(&mut self, number: u64) {
        if self.unhandled().any(|seen| seen == number) {
            return;
        }
        if self.unhandled_count < UNHANDLED_CAPACITY {
            self.unhandled[self.unhandled_count] = Some(number);
            self.unhandled_count += 1;
        }
    }
}

/// The arena layout the M1 harness uses: heap then mmap, above the program.
///
/// Named here rather than in the harness because both the shim tests and the
/// benchmarks need the same one, and a mismatch would show up as an unmapped
/// address rather than as a configuration error.
pub const fn default_layout(ram_base: u64, ram_bytes: u64) -> ArenaLayout {
    let heap_base = ram_base + ram_bytes / 4;
    let mmap_base = ram_base + ram_bytes / 2;
    ArenaLayout {
        heap_base,
        heap_limit: mmap_base,
        mmap_base,
        mmap_limit: ram_base + ram_bytes,
    }
}
