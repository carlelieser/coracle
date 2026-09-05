//! The hook the machine loop calls, and the events it reports.
//!
//! Shape fixed by `tests/EMULATOR_INTERFACE.md` §1. The required call ordering
//! is: one [`TraceSink::on_marker`] with [`MarkerKind::TraceStart`] before the
//! first block, [`TraceSink::on_block`] per retired block in execution order,
//! [`TraceSink::on_exception`] between the block that faulted and the block at
//! the vector, and exactly one [`TraceSink::finish`].

use super::format::{DisconType, EndReason, MarkerKind, MAX_DELTAS_PER_BLOCK};
use crate::reg::{trace_reg_id, Vec as VecReg, NUM_GPR};
use crate::regfile::RegFile;

/// One register's new value, identified by its stable trace id.
///
/// Emitted only when the value changed since the previous record, per
/// `tests/EMULATOR_INTERFACE.md` §2.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RegDelta {
    /// Stable id, `crate::reg::trace_reg_id`.
    pub reg_id: u16,
    /// New value. A V register emits two deltas, low half then high.
    pub value: u64,
}

/// Somewhere [`deltas_between`] can append deltas.
///
/// Exists so the machine loop can diff into a fixed-size array on the hot path
/// while tests keep using `alloc::vec::Vec`.
pub trait DeltaOut {
    /// Drops every delta collected so far.
    fn clear(&mut self);

    /// Appends one delta, discarding it if there is no room.
    fn push(&mut self, delta: RegDelta);
}

#[cfg(feature = "trace")]
impl DeltaOut for alloc::vec::Vec<RegDelta> {
    fn clear(&mut self) {
        self.clear();
    }

    fn push(&mut self, delta: RegDelta) {
        self.push(delta);
    }
}

/// A block's deltas, in an array sized by the format's own limit.
///
/// [`MAX_DELTAS_PER_BLOCK`] is what a record header's `flags` byte can count,
/// so no reachable diff overflows this and the machine loop never allocates.
#[derive(Debug, Clone)]
pub struct DeltaBuffer {
    deltas: [RegDelta; MAX_DELTAS_PER_BLOCK],
    len: usize,
}

impl Default for DeltaBuffer {
    fn default() -> Self {
        Self::new()
    }
}

impl DeltaBuffer {
    /// An empty buffer.
    pub const fn new() -> Self {
        Self {
            deltas: [RegDelta {
                reg_id: 0,
                value: 0,
            }; MAX_DELTAS_PER_BLOCK],
            len: 0,
        }
    }

    /// The deltas collected so far.
    pub fn as_slice(&self) -> &[RegDelta] {
        &self.deltas[..self.len]
    }
}

impl DeltaOut for DeltaBuffer {
    fn clear(&mut self) {
        self.len = 0;
    }

    fn push(&mut self, delta: RegDelta) {
        if self.len < MAX_DELTAS_PER_BLOCK {
            self.deltas[self.len] = delta;
            self.len += 1;
        }
    }
}

/// Full architectural state at an exception entry.
///
/// Carries a borrow of the register file rather than a copy of it: the record
/// is serialised immediately and these are rare enough that the indirection
/// costs nothing.
#[derive(Debug, Clone, Copy)]
pub struct ExceptionEvent<'a> {
    /// Instructions retired before the faulting instruction, which is itself
    /// **not** counted.
    pub icount: u64,
    /// Address of the faulting or interrupted instruction.
    pub from_pc: u64,
    /// Vector entry the CPU moved to.
    pub to_pc: u64,
    /// What kind of discontinuity this was.
    pub discon: DisconType,
    /// Architectural state after the redirect.
    pub regs: &'a RegFile,
}

/// Receives execution events in the CDT ordering.
///
/// Implemented by the CDT writer and by test doubles. The machine loop holds
/// one of these behind the `trace` feature, so a release build carries no
/// emission cost.
pub trait TraceSink {
    /// Whether this sink records anything.
    ///
    /// The machine loop guards its snapshot-and-diff work on this constant, so
    /// a sink that discards events costs nothing beyond the empty method
    /// bodies: the guard const-folds away at monomorphisation.
    const ENABLED: bool;

    /// A synchronisation point or annotation.
    fn on_marker(&mut self, kind: MarkerKind, icount: u64, value: u64);

    /// A basic block retired.
    ///
    /// `icount` is the count **before** this block; `n_insns` advances it. M1
    /// runs in the per-instruction mode of `tests/EMULATOR_INTERFACE.md` §4,
    /// where `n_insns` is always 1.
    fn on_block(&mut self, pc: u64, icount: u64, deltas: &[RegDelta]);

    /// An exception, interrupt or host call, after the CPU reached the vector.
    fn on_exception(&mut self, event: &ExceptionEvent<'_>);

    /// End of stream. Called exactly once.
    ///
    /// `icount` is the total retired, per `tests/TRACE_FORMAT.md` §4.4. It is
    /// passed rather than inferred from the last block: a block record is
    /// written on entry, so the last one may name an instruction that trapped
    /// and never retired.
    fn finish(&mut self, reason: EndReason, icount: u64);
}

/// A sink that discards everything.
///
/// What a build without tracing enabled uses, so the machine loop has one code
/// path rather than a `cfg` at every call site.
#[derive(Debug, Clone, Copy, Default)]
pub struct NullSink;

impl TraceSink for NullSink {
    const ENABLED: bool = false;

    fn on_marker(&mut self, _kind: MarkerKind, _icount: u64, _value: u64) {}
    fn on_block(&mut self, _pc: u64, _icount: u64, _deltas: &[RegDelta]) {}
    fn on_exception(&mut self, _event: &ExceptionEvent<'_>) {}
    fn finish(&mut self, _reason: EndReason, _icount: u64) {}
}

/// Collects the deltas between two register-file snapshots.
///
/// `tests/EMULATOR_INTERFACE.md` §2 requires a register be emitted only when it
/// changed. Passing `None` for `previous` emits the full set, which is what the
/// block after an exception must do so the two streams cannot silently drift.
pub fn deltas_between<O: DeltaOut + ?Sized>(
    previous: Option<&RegFile>,
    current: &RegFile,
    out: &mut O,
) {
    out.clear();
    let mut push = |reg_id, value, old: Option<u64>| {
        if old != Some(value) {
            out.push(RegDelta { reg_id, value });
        }
    };

    for index in 0..NUM_GPR as u8 {
        push(
            trace_reg_id::gpr(index),
            current.x(index),
            previous.map(|regs| regs.x(index)),
        );
    }
    push(trace_reg_id::SP, current.sp(), previous.map(RegFile::sp));
    push(trace_reg_id::PC, current.pc(), previous.map(RegFile::pc));
    push(
        trace_reg_id::PSTATE,
        current.pstate.to_trace_word(),
        previous.map(|regs| regs.pstate.to_trace_word()),
    );
    push(trace_reg_id::FPCR, current.fpcr, previous.map(|r| r.fpcr));
    push(trace_reg_id::FPSR, current.fpsr, previous.map(|r| r.fpsr));

    push_vec_deltas(previous, current, out);
}

fn push_vec_deltas<O: DeltaOut + ?Sized>(
    previous: Option<&RegFile>,
    current: &RegFile,
    out: &mut O,
) {
    for index in 0..VecReg::COUNT as u8 {
        let reg = VecReg::new(index);
        let value = current.read_v(reg);
        if previous.map(|regs| regs.read_v(reg)) == Some(value) {
            continue;
        }
        out.push(RegDelta {
            reg_id: trace_reg_id::vec_lo(index),
            value: value as u64,
        });
        out.push(RegDelta {
            reg_id: trace_reg_id::vec_hi(index),
            value: (value >> 64) as u64,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_null_sink_accepts_every_event_without_state() {
        let regs = RegFile::new();
        let mut sink = NullSink;

        sink.on_marker(MarkerKind::TraceStart, 0, 0);
        sink.on_block(
            0x1000,
            0,
            &[RegDelta {
                reg_id: 0,
                value: 1,
            }],
        );
        sink.on_exception(&ExceptionEvent {
            icount: 1,
            from_pc: 0x1000,
            to_pc: 0x200,
            discon: DisconType::Exception,
            regs: &regs,
        });
        sink.finish(EndReason::Normal, 1);
    }
}
