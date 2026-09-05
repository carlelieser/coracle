//! CDT v1 trace emission.
//!
//! Gated behind the `trace` feature (`tests/EMULATOR_INTERFACE.md` §1) so a
//! release build carries no emission cost. `format` and `sink` are always
//! compiled: the machine loop's call sites and the trap/exception vocabulary
//! reference them either way, and [`sink::NullSink`] keeps that one code path
//! rather than a `cfg` at every call.
//!
//! M1 emits one `REC_BLOCK` per instruction. `tests/EMULATOR_INTERFACE.md` §4
//! names this the fallback for block-boundary agreement: matching QEMU's TCG
//! block boundaries — the page-boundary rule in particular — is a separate
//! problem from getting the CPU right, and per-instruction records put every
//! divergence at exactly one instruction at the cost of roughly 3x the stream.

pub mod format;
pub mod sink;

#[cfg(feature = "trace")]
pub mod writer;

pub use format::{DisconType, EndReason, MarkerKind};
pub use sink::{
    deltas_between, DeltaBuffer, DeltaOut, ExceptionEvent, NullSink, RegDelta, TraceSink,
};
