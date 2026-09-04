//! Per-group decoders reached from the top-level `op0` switch.
//!
//! One module per row of the ARM ARM's top-level encoding table, and one owner
//! per module: the phase B slices extend these files independently, so a slice
//! never edits a file another slice is editing.
//!
//! Every module starts as a stub returning [`unallocated`] for the encodings it
//! has not yet claimed. That is a working state rather than a hole — an
//! unclaimed encoding traps and is logged, and that log drives the lazy NEON
//! implementation the plan calls for.

mod branch_system;
mod dp_immediate;
mod dp_register;
mod loads_stores;
mod simd_fp;

pub use branch_system::branches_exceptions_system;
pub use dp_immediate::data_processing_immediate;
pub use dp_register::data_processing_register;
pub use loads_stores::loads_and_stores;
pub use simd_fp::{data_processing_simd_fp, loads_and_stores_vec};

/// Extracts the `[hi:lo]` bit field of an encoding, inclusive at both ends.
const fn bits(encoding: u32, hi: u32, lo: u32) -> u32 {
    (encoding >> lo) & ((1 << (hi - lo + 1)) - 1)
}

/// Sign-extends the low `width` bits of `value`.
const fn sign_extend(value: u32, width: u32) -> i64 {
    let shift = 64 - width;
    ((value as i64) << shift) >> shift
}

#[cfg(test)]
mod tests {
    use super::super::address::{AccessSize, AddrMode, Ordering, WriteBack};
    use super::super::decode;
    use super::super::instruction::Form;
    use super::super::op::Op;
    use super::super::operand::{RegWidth, ShiftKind, ShiftedReg};
    use crate::reg::Gpr;

    #[test]
    fn add_immediate_resolves_slot_31_as_sp_and_keeps_its_immediate() {
        // add sp, sp, #0x10
        let insn = decode(0x9100_43ff);

        assert_eq!(insn.op, Op::Add);
        assert!(!insn.sets_flags);
        assert_eq!(insn.width, RegWidth::X64);
        assert_eq!(
            insn.form,
            Form::RegImm {
                rd: Gpr::SP,
                rn: Gpr::SP,
                imm: 0x10,
            }
        );
    }

    #[test]
    fn a_flag_setting_add_immediate_resolves_slot_31_as_the_zero_register() {
        // cmn x0, #1 — an alias of `adds xzr, x0, #1`.
        let insn = decode(0xb100_041f);

        assert_eq!(insn.op, Op::Add);
        assert!(insn.sets_flags);
        assert_eq!(
            insn.form,
            Form::RegImm {
                rd: Gpr::ZR,
                rn: Gpr::X(0),
                imm: 1,
            }
        );
    }

    #[test]
    fn a_shifted_immediate_is_expanded_at_decode_time() {
        // add x0, x0, #0x1000  (sh = 1, imm12 = 1)
        let insn = decode(0x9140_0400);

        assert_eq!(
            insn.form,
            Form::RegImm {
                rd: Gpr::X(0),
                rn: Gpr::X(0),
                imm: 0x1000,
            }
        );
    }

    #[test]
    fn a_32_bit_add_immediate_reports_the_narrow_width() {
        // add w1, w2, #3
        let insn = decode(0x1100_0c41);

        assert_eq!(insn.width, RegWidth::W32);
        assert_eq!(insn.op, Op::Add);
    }

    #[test]
    fn branch_offsets_are_sign_extended_and_scaled_to_bytes() {
        // b .+8
        assert_eq!(
            decode(0x1400_0002).form,
            Form::Branch { offset: 8 },
            "forward branch"
        );
        // bl .-8
        let backward = decode(0x97ff_fffe);
        assert_eq!(backward.op, Op::Bl);
        assert_eq!(backward.form, Form::Branch { offset: -8 });
    }

    #[test]
    fn ret_decodes_to_an_indirect_branch_through_its_link_register() {
        // ret  (implicitly x30)
        let insn = decode(0xd65f_03c0);

        assert_eq!(insn.op, Op::Ret);
        assert_eq!(insn.form, Form::BranchIndirect { rn: Gpr::X(30) });
    }

    #[test]
    fn nop_decodes_with_no_operands() {
        let insn = decode(0xd503_201f);

        assert_eq!(insn.op, Op::Nop);
        assert_eq!(insn.form, Form::None);
    }

    #[test]
    fn an_unsigned_offset_load_scales_its_immediate_by_the_access_size() {
        // ldr x0, [x1, #16]  — imm12 = 2, scaled by 8
        let insn = decode(0xf940_0820);

        assert_eq!(insn.op, Op::Ldr);
        assert_eq!(
            insn.form,
            Form::LoadStore {
                rt: Gpr::X(0),
                rt2: None,
                rs: None,
                addr: AddrMode::Immediate {
                    base: Gpr::X(1),
                    offset: 16,
                    writeback: WriteBack::None,
                },
                size: AccessSize::X,
                ordering: Ordering::PLAIN,
            }
        );
    }

    #[test]
    fn a_byte_store_reports_a_one_byte_unsigned_access() {
        // strb w0, [x1, #3]
        let insn = decode(0x3900_0c20);

        assert_eq!(insn.op, Op::Str);
        let Form::LoadStore { size, addr, .. } = insn.form else {
            panic!("expected a single-register load/store, got {:?}", insn.form);
        };
        assert_eq!(size, AccessSize::B);
        assert_eq!(
            addr,
            AddrMode::Immediate {
                base: Gpr::X(1),
                offset: 3,
                writeback: WriteBack::None,
            }
        );
    }

    #[test]
    fn a_load_through_slot_31_uses_the_stack_pointer_as_its_base() {
        // ldr x0, [sp]
        let insn = decode(0xf940_03e0);

        let Form::LoadStore { addr, .. } = insn.form else {
            panic!("expected a single-register load/store, got {:?}", insn.form);
        };
        assert_eq!(addr.base(), Some(Gpr::SP));
    }

    #[test]
    fn shifted_register_forms_carry_their_shift_kind_and_amount() {
        // add x0, x1, x2, lsl #4
        let insn = decode(0x8b02_1020);

        assert_eq!(insn.op, Op::Add);
        assert_eq!(
            insn.form,
            Form::RegShifted {
                rd: Gpr::X(0),
                rn: Gpr::X(1),
                rm: ShiftedReg {
                    reg: Gpr::X(2),
                    kind: ShiftKind::Lsl,
                    amount: 4,
                },
            }
        );
    }

    #[test]
    fn the_logical_group_sets_flags_only_for_ands() {
        // and x0, x1, x2
        assert!(!decode(0x8a02_0020).sets_flags);
        // ands x0, x1, x2
        let ands = decode(0xea02_0020);
        assert_eq!(ands.op, Op::And);
        assert!(ands.sets_flags);
        // orr x0, x1, x2
        let orr = decode(0xaa02_0020);
        assert_eq!(orr.op, Op::Orr);
        assert!(!orr.sets_flags);
    }

    #[test]
    fn the_advanced_simd_half_of_the_fp_group_is_still_unclaimed() {
        // Scalar FP has landed, so its encodings decode. Advanced SIMD is
        // implemented lazily off the unimplemented-opcode trap
        // (`docs/plan.md` §M1), so what it has not claimed must still fault
        // rather than decode to something approximate.
        assert_eq!(decode(0x1e20_2800).op, Op::Fadd);
        assert!(decode(0x4e20_8400).op.is_unallocated());
    }
}
