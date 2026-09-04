//! AArch64 CPU and stage-1 MMU.
//!
//! Skeleton only: this crate carries the build-configuration surface that the
//! rest of the workspace and the JS host agree on. The decoder, interpreter,
//! register file and MMU land in M1/M2.

#![no_std]

extern crate alloc;

pub mod guest_memory;
pub mod threading;

/// Whether this build was compiled against shared linear memory.
///
/// The single-threaded degraded build (Safari, and any page that cannot be
/// cross-origin isolated) reports `false` and drives the machine loop on the
/// main thread instead of a worker.
pub const IS_THREADED_BUILD: bool = cfg!(feature = "threads");
