//! AArch64 CPU and stage-1 MMU.
//!
//! Carries the build-configuration surface that the rest of the workspace and
//! the JS host agree on, the architectural register file, and the A64 decoder.
//! The interpreter and MMU land in M1 phase B and M2.

#![no_std]

extern crate alloc;

pub mod decode;
pub mod guest_memory;
pub mod pstate;
pub mod reg;
pub mod regfile;
pub mod threading;
pub mod trace;
pub mod trap;

/// Whether this build was compiled against shared linear memory.
///
/// The single-threaded degraded build (Safari, and any page that cannot be
/// cross-origin isolated) reports `false` and drives the machine loop on the
/// main thread instead of a worker.
pub const IS_THREADED_BUILD: bool = cfg!(feature = "threads");
