//! A64 instruction decoding.
//!
//! [`decode`] is total: every 32-bit word maps to an [`Instruction`], and an
//! encoding no group claims becomes [`Op::Unallocated`] rather than a panic or
//! an error. `docs/plan.md` makes that a requirement rather than a nicety —
//! NEON is implemented lazily off the unimplemented-opcode trap, and the M1
//! gate runs a 10,000-binary fuzz corpus of random words through this function.

pub mod address;
pub mod group;
pub mod instruction;
pub mod operand;

pub use instruction::{unallocated, Form, Instruction, Op};

/// Bytes occupied by one A64 instruction. Fixed by the architecture.
pub const INSN_BYTES: u64 = 4;

/// The top-level encoding group an instruction word belongs to.
///
/// Selected by `op0` — bits 28..25 of the encoding — exactly as the ARM ARM's
/// "A64 instruction set encoding" table does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum EncodingGroup {
    /// `op0 = 0000`: reserved, including the `UDF` permanently-undefined space.
    Reserved,
    /// `op0 = 0010`: SVE. Not advertised by this machine, so always
    /// unallocated.
    Sve,
    /// `op0 = 100x`: data processing — immediate.
    DataProcessingImmediate,
    /// `op0 = 101x`: branches, exception generation and system instructions.
    BranchesExceptionsSystem,
    /// `op0 = x1x0`: loads and stores.
    LoadsAndStores,
    /// `op0 = x101`: data processing — register.
    DataProcessingRegister,
    /// `op0 = x111`: data processing — scalar FP and advanced SIMD.
    DataProcessingSimdFp,
    /// `op0 = 0001` or `0011`: no encoding is allocated here.
    Unallocated,
}

impl EncodingGroup {
    /// Classifies an instruction word by its `op0` field.
    pub const fn of(encoding: u32) -> Self {
        let op0 = (encoding >> 25) & 0b1111;
        match op0 {
            0b0000 => EncodingGroup::Reserved,
            0b0001 | 0b0011 => EncodingGroup::Unallocated,
            0b0010 => EncodingGroup::Sve,
            0b1000 | 0b1001 => EncodingGroup::DataProcessingImmediate,
            0b1010 | 0b1011 => EncodingGroup::BranchesExceptionsSystem,
            0b0100 | 0b0110 | 0b1100 | 0b1110 => EncodingGroup::LoadsAndStores,
            0b0101 | 0b1101 => EncodingGroup::DataProcessingRegister,
            _ => EncodingGroup::DataProcessingSimdFp,
        }
    }
}

/// Decodes one A64 instruction word.
///
/// Total by construction: an unclaimed encoding yields [`Op::Unallocated`],
/// which the interpreter turns into an undefined-instruction trap at the right
/// PC.
pub fn decode(encoding: u32) -> Instruction {
    match EncodingGroup::of(encoding) {
        EncodingGroup::DataProcessingImmediate => group::data_processing_immediate(encoding),
        EncodingGroup::BranchesExceptionsSystem => group::branches_exceptions_system(encoding),
        EncodingGroup::LoadsAndStores => group::loads_and_stores(encoding),
        EncodingGroup::DataProcessingRegister => group::data_processing_register(encoding),
        EncodingGroup::DataProcessingSimdFp => group::data_processing_simd_fp(encoding),
        // SVE is not advertised (docs/machine-spec.md §2) and the reserved and
        // unallocated spaces have no encodings, so all three fault identically.
        EncodingGroup::Sve | EncodingGroup::Reserved | EncodingGroup::Unallocated => {
            unallocated(encoding)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `op0` is bits 28..25, so this places `op0` and leaves the rest zero.
    const fn word_with_op0(op0: u32) -> u32 {
        op0 << 25
    }

    #[test]
    fn op0_routes_each_encoding_to_the_group_the_arm_arm_names() {
        let expected = [
            (0b0000, EncodingGroup::Reserved),
            (0b0001, EncodingGroup::Unallocated),
            (0b0010, EncodingGroup::Sve),
            (0b0011, EncodingGroup::Unallocated),
            (0b0100, EncodingGroup::LoadsAndStores),
            (0b0101, EncodingGroup::DataProcessingRegister),
            (0b0110, EncodingGroup::LoadsAndStores),
            (0b0111, EncodingGroup::DataProcessingSimdFp),
            (0b1000, EncodingGroup::DataProcessingImmediate),
            (0b1001, EncodingGroup::DataProcessingImmediate),
            (0b1010, EncodingGroup::BranchesExceptionsSystem),
            (0b1011, EncodingGroup::BranchesExceptionsSystem),
            (0b1100, EncodingGroup::LoadsAndStores),
            (0b1101, EncodingGroup::DataProcessingRegister),
            (0b1110, EncodingGroup::LoadsAndStores),
            (0b1111, EncodingGroup::DataProcessingSimdFp),
        ];

        for (op0, group) in expected {
            assert_eq!(
                EncodingGroup::of(word_with_op0(op0)),
                group,
                "op0 = {op0:04b}"
            );
        }
    }

    #[test]
    fn bits_outside_op0_do_not_change_the_group() {
        let op0_mask = 0b1111 << 25;

        for op0 in 0..16u32 {
            let group = EncodingGroup::of(word_with_op0(op0));
            assert_eq!(EncodingGroup::of(word_with_op0(op0) | !op0_mask), group);
        }
    }

    #[test]
    fn sve_and_the_reserved_spaces_decode_as_unallocated() {
        // The machine advertises no SVE (docs/machine-spec.md §2), so its whole
        // encoding space must fault exactly as the unallocated space does.
        for op0 in [0b0000, 0b0001, 0b0010, 0b0011] {
            assert!(decode(word_with_op0(op0)).op.is_unallocated());
        }
    }

    #[test]
    fn decode_is_total_over_a_wide_sweep_of_encodings() {
        // The M1 gate feeds 10,000 random binaries through this. Decoding must
        // terminate with an Instruction for every word, never panic.
        let mut state = 0x2545_f491_4f6c_dd1du64;
        for _ in 0..200_000 {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            let insn = decode(state as u32);
            assert_eq!(insn.encoding, state as u32);
        }
    }
}
