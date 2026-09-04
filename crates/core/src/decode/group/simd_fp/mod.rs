//! Owned by the FP/NEON slice.
//!
//! Two entry points, because the architecture puts this slice's encodings in
//! two different top-level groups: [`data_processing_simd_fp`] answers for
//! `op0 = x111`, and [`loads_and_stores_vec`] for the `V = 1` half of the
//! loads-and-stores group, which the memory slice hands over.
//!
//! Coverage is deliberately partial. `docs/plan.md` §M1 calls for NEON to be
//! implemented lazily, driven by a trap-and-log on unimplemented opcodes, so an
//! encoding this slice has not claimed returns `unallocated` and faults rather
//! than decoding to something approximate.

mod loads_stores;
mod scalar;

pub use loads_stores::loads_and_stores_vec;

use super::super::instruction::{unallocated, Instruction};
use super::bits;

/// `op0 = x111` — data processing, scalar FP and advanced SIMD.
pub fn data_processing_simd_fp(encoding: u32) -> Instruction {
    // Bit 28 set is the scalar FP group; clear is advanced SIMD, which this
    // slice decodes lazily and so leaves unallocated for now.
    if bits(encoding, 28, 28) == 1 {
        return scalar::data_processing(encoding);
    }
    unallocated(encoding)
}
