//! Owned by the FP/NEON slice.

use super::super::instruction::{unallocated, Instruction};

/// `op0 = x111` — data processing, scalar FP and advanced SIMD.
///
/// Owned by the FP+NEON slice. Nothing is decoded yet: every encoding traps and
/// is logged, which is exactly the mechanism `docs/plan.md` names for driving
/// NEON coverage.
pub fn data_processing_simd_fp(encoding: u32) -> Instruction {
    unallocated(encoding)
}
