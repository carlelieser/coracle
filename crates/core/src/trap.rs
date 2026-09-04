//! Traps, and the log that turns an unimplemented opcode into a work item.
//!
//! `docs/plan.md` makes NEON coverage a lazy, trap-driven exercise and runs a
//! 10,000-binary fuzz corpus through the decoder at the M1 gate. Both depend on
//! the same property: reaching an encoding this build does not implement is a
//! recorded, recoverable event, never a panic.
//!
//! The log distinguishes nothing about *why* an encoding was unclaimed — an
//! architecturally unallocated word and a NEON opcode nobody has written yet
//! both land here. The guest cannot tell them apart either; both take an
//! undefined-instruction exception. The distinction that matters is for the
//! coverage report, and it is made by looking at the recorded encodings
//! afterwards.

use crate::decode::{EncodingGroup, Instruction, Op};

/// A trap the CPU raises to its host.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Trap {
    /// An encoding this build does not implement, or one the architecture
    /// leaves unallocated. The guest sees an undefined-instruction exception.
    UndefinedInstruction {
        /// PC of the offending instruction.
        pc: u64,
        /// The 32-bit encoding.
        encoding: u32,
    },
    /// `SVC` — a supervisor call. The M1 syscall shim handles these; from M2
    /// the kernel does.
    SupervisorCall {
        /// PC of the `SVC`.
        pc: u64,
        /// The 16-bit immediate.
        imm: u16,
    },
    /// A software breakpoint (`BRK`).
    Breakpoint {
        /// PC of the `BRK`.
        pc: u64,
        /// The 16-bit immediate.
        imm: u16,
    },
    /// A memory access that could not be completed.
    DataAbort {
        /// PC of the faulting instruction.
        pc: u64,
        /// Virtual address the access targeted.
        address: u64,
        /// Whether the access was a write.
        is_write: bool,
    },
    /// An instruction fetch that could not be completed.
    InstructionAbort {
        /// Virtual address that could not be fetched.
        pc: u64,
    },
}

impl Trap {
    /// The trap an unimplemented or unallocated instruction raises.
    pub const fn undefined(pc: u64, insn: &Instruction) -> Self {
        Trap::UndefinedInstruction {
            pc,
            encoding: insn.encoding,
        }
    }
}

/// Distinct encodings the log remembers before it stops recording new ones.
///
/// A bound rather than a growable buffer because this runs in a `no_std` hot
/// loop and a fuzz corpus of random words would otherwise grow the log without
/// limit. `docs/plan.md` wants a coverage report, and the encodings a real
/// guest actually reaches are far fewer than this.
pub const TRAP_LOG_CAPACITY: usize = 256;

/// One distinct unimplemented encoding and how often it was reached.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TrapLogEntry {
    /// The 32-bit encoding.
    pub encoding: u32,
    /// Top-level group it decodes into, which is what names the phase B slice
    /// that owes an implementation.
    pub group: EncodingGroup,
    /// Times this encoding has been reached.
    pub count: u64,
}

/// Records which encodings this build failed to implement.
///
/// Deduplicated by encoding: a `memcpy` loop hitting one unimplemented NEON
/// opcode a million times produces one entry with a count, not a million lines.
#[derive(Debug, Clone)]
pub struct TrapLog {
    entries: [Option<TrapLogEntry>; TRAP_LOG_CAPACITY],
    used: usize,
    dropped: u64,
    total: u64,
}

impl Default for TrapLog {
    fn default() -> Self {
        Self::new()
    }
}

impl TrapLog {
    /// An empty log.
    pub const fn new() -> Self {
        Self {
            entries: [None; TRAP_LOG_CAPACITY],
            used: 0,
            dropped: 0,
            total: 0,
        }
    }

    /// Records one encounter with an unimplemented encoding.
    ///
    /// Never fails and never allocates. Once capacity is exhausted a new
    /// encoding increments [`TrapLog::dropped_encodings`] instead, so the
    /// report can say the log was truncated rather than implying coverage it
    /// does not have.
    pub fn record(&mut self, encoding: u32) {
        self.total += 1;

        if let Some(entry) = self.find_mut(encoding) {
            entry.count += 1;
            return;
        }

        if self.used == TRAP_LOG_CAPACITY {
            self.dropped += 1;
            return;
        }

        self.entries[self.used] = Some(TrapLogEntry {
            encoding,
            group: EncodingGroup::of(encoding),
            count: 1,
        });
        self.used += 1;
    }

    /// Records the instruction behind a trap, when that trap is an undefined
    /// instruction. Other traps are not a coverage gap and are ignored.
    pub fn record_trap(&mut self, trap: Trap) {
        if let Trap::UndefinedInstruction { encoding, .. } = trap {
            self.record(encoding);
        }
    }

    /// The distinct encodings recorded, in first-seen order.
    pub fn entries(&self) -> impl Iterator<Item = &TrapLogEntry> {
        self.entries[..self.used].iter().flatten()
    }

    /// Total unimplemented-encoding encounters, including those past capacity.
    pub const fn total_encounters(&self) -> u64 {
        self.total
    }

    /// Encounters with distinct encodings the log had no room for.
    pub const fn dropped_encodings(&self) -> u64 {
        self.dropped
    }

    /// Whether anything was recorded.
    pub const fn is_empty(&self) -> bool {
        self.used == 0 && self.dropped == 0
    }

    fn find_mut(&mut self, encoding: u32) -> Option<&mut TrapLogEntry> {
        self.entries[..self.used]
            .iter_mut()
            .flatten()
            .find(|entry| entry.encoding == encoding)
    }
}

/// Whether reaching this instruction is an unimplemented-opcode trap.
pub const fn is_unimplemented(insn: &Instruction) -> bool {
    matches!(insn.op, Op::Unallocated)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decode::decode;

    #[test]
    fn repeated_encounters_with_one_encoding_collapse_to_a_single_entry() {
        let mut log = TrapLog::new();

        for _ in 0..1000 {
            log.record(0x4e20_8400);
        }

        let entries: alloc::vec::Vec<_> = log.entries().collect();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].encoding, 0x4e20_8400);
        assert_eq!(entries[0].count, 1000);
        assert_eq!(log.total_encounters(), 1000);
        assert_eq!(log.dropped_encodings(), 0);
    }

    #[test]
    fn an_entry_names_the_group_that_owes_the_implementation() {
        let mut log = TrapLog::new();

        log.record(0x4e20_8400);

        let entry = *log.entries().next().expect("one entry");
        assert_eq!(entry.group, EncodingGroup::DataProcessingSimdFp);
    }

    #[test]
    fn overflowing_the_log_is_counted_rather_than_silently_lost() {
        let mut log = TrapLog::new();

        // One past capacity, all distinct.
        for encoding in 0..TRAP_LOG_CAPACITY as u32 + 1 {
            log.record(encoding);
        }

        assert_eq!(log.entries().count(), TRAP_LOG_CAPACITY);
        assert_eq!(log.dropped_encodings(), 1);
        assert_eq!(log.total_encounters(), TRAP_LOG_CAPACITY as u64 + 1);
    }

    #[test]
    fn a_full_log_still_counts_encodings_it_already_holds() {
        let mut log = TrapLog::new();
        for encoding in 0..TRAP_LOG_CAPACITY as u32 {
            log.record(encoding);
        }

        log.record(0);

        assert_eq!(log.dropped_encodings(), 0);
        let first = *log.entries().next().expect("one entry");
        assert_eq!(first.count, 2);
    }

    #[test]
    fn only_undefined_instruction_traps_are_a_coverage_gap() {
        let mut log = TrapLog::new();

        log.record_trap(Trap::SupervisorCall { pc: 0x1000, imm: 0 });
        log.record_trap(Trap::DataAbort {
            pc: 0x1000,
            address: 0,
            is_write: true,
        });

        assert!(log.is_empty());

        log.record_trap(Trap::UndefinedInstruction {
            pc: 0x1000,
            encoding: 0xffff_ffff,
        });

        assert!(!log.is_empty());
    }

    #[test]
    fn an_unimplemented_encoding_becomes_a_trap_naming_its_own_pc() {
        let insn = decode(0x4e20_8400);
        assert!(is_unimplemented(&insn));

        let trap = Trap::undefined(0xdead_0000, &insn);

        assert_eq!(
            trap,
            Trap::UndefinedInstruction {
                pc: 0xdead_0000,
                encoding: 0x4e20_8400,
            }
        );
    }

    #[test]
    fn a_fuzz_sweep_of_random_words_never_panics_and_stays_bounded() {
        // The M1 gate runs 10,000 random binaries through this path. The log
        // must absorb whatever they produce without growing or aborting.
        let mut log = TrapLog::new();
        let mut state = 0x853c_49e6_748f_ea9bu64;

        for _ in 0..100_000 {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            let insn = decode(state as u32);
            if is_unimplemented(&insn) {
                log.record_trap(Trap::undefined(0x1000, &insn));
            }
        }

        assert!(log.entries().count() <= TRAP_LOG_CAPACITY);
        assert!(log.total_encounters() > 0);
    }
}
