//! The hook the machine loop calls, and the events it reports.
//!
//! Shape fixed by `tests/EMULATOR_INTERFACE.md` §1. The required call ordering
//! is: one [`TraceSink::on_marker`] with [`MarkerKind::TraceStart`] before the
//! first block, [`TraceSink::on_block`] per retired block in execution order,
//! [`TraceSink::on_exception`] between the block that faulted and the block at
//! the vector, and exactly one [`TraceSink::finish`].

use super::format::{DisconType, EndReason, MarkerKind};
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
    fn finish(&mut self, reason: EndReason);
}

/// A sink that discards everything.
///
/// What a build without tracing enabled uses, so the machine loop has one code
/// path rather than a `cfg` at every call site.
#[derive(Debug, Clone, Copy, Default)]
pub struct NullSink;

impl TraceSink for NullSink {
    fn on_marker(&mut self, _kind: MarkerKind, _icount: u64, _value: u64) {}
    fn on_block(&mut self, _pc: u64, _icount: u64, _deltas: &[RegDelta]) {}
    fn on_exception(&mut self, _event: &ExceptionEvent<'_>) {}
    fn finish(&mut self, _reason: EndReason) {}
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
        sink.finish(EndReason::Normal);
    }
}
