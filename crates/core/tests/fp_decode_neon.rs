//! The advanced SIMD subset.
//!
//! `docs/plan.md` §M1 scopes this to what musl and glibc's `memcpy`, `memset`,
//! `strlen` and `memchr` use plus V8's baseline, implemented lazily off the
//! unimplemented-opcode trap. These tests pin what is claimed; the last few pin
//! that the rest — crypto especially — still faults.
//!
//! Encodings verified with `llvm-mc -arch=aarch64 -show-encoding`.

use coracle_core::decode::decode;
use coracle_core::decode::instruction::Form;
use coracle_core::decode::op::Op;
use coracle_core::decode::operand::{ElemSize, VecHalf, VecOperand, VecShape};
use coracle_core::reg::{Gpr, Vec};

/// A full-width vector operand.
fn full(index: u8, elem: ElemSize) -> VecOperand {
    let lanes = 128 / elem.bits();
    VecOperand {
        reg: Vec::new(index),
        shape: VecShape::Vector {
            elem,
            count: lanes as u8,
            half: VecHalf::Full,
        },
    }
}

/// A 64-bit vector operand.
fn half(index: u8, elem: ElemSize) -> VecOperand {
    let lanes = 64 / elem.bits();
    VecOperand {
        reg: Vec::new(index),
        shape: VecShape::Vector {
            elem,
            count: lanes as u8,
            half: VecHalf::Low,
        },
    }
}

/// A single addressed lane.
fn lane(index: u8, elem: ElemSize, at: u8) -> VecOperand {
    VecOperand {
        reg: Vec::new(index),
        shape: VecShape::Element { elem, index: at },
    }
}

#[test]
fn the_logical_group_reads_its_operation_from_the_size_field() {
    // These eight share one opcode and differ only in size and the U bit, so a
    // decoder reading `size` as an element width would collapse them.
    let expected = [
        (0x4e22_1c20u32, Op::VecAnd, 0u8, 1u8, 2u8),
        (0x4ea5_1c83, Op::VecOrr, 3, 4, 5),
        (0x6e28_1ce6, Op::VecEor, 6, 7, 8),
        (0x4e6b_1d49, Op::VecBic, 9, 10, 11),
        (0x4eee_1dac, Op::VecOrn, 12, 13, 14),
        (0x6e71_1e0f, Op::VecBsl, 15, 16, 17),
        (0x6eb4_1e72, Op::VecBit, 18, 19, 20),
        (0x6ef7_1ed5, Op::VecBif, 21, 22, 23),
    ];

    for (encoding, op, rd, rn, rm) in expected {
        let insn = decode(encoding);
        assert_eq!(insn.op, op, "{encoding:#010x}");
        assert_eq!(
            insn.form,
            Form::VecData {
                vd: full(rd, ElemSize::B8),
                vn: full(rn, ElemSize::B8),
                vm: Some(full(rm, ElemSize::B8)),
                va: None,
            },
            "{encoding:#010x}"
        );
    }
}

#[test]
fn the_bitwise_operations_are_always_byte_shaped() {
    // `size` selects the operation, so every logical form is 16 bytes wide
    // regardless of what the assembler's mnemonic suffix says.
    let insn = decode(0x4eee_1dac);
    let Form::VecData { vd, .. } = insn.form else {
        panic!("expected VecData, got {:?}", insn.form);
    };
    assert_eq!(
        vd.shape,
        VecShape::Vector {
            elem: ElemSize::B8,
            count: 16,
            half: VecHalf::Full,
        }
    );
}

#[test]
fn integer_add_and_subtract_carry_their_arrangement() {
    // add v0.4s, v1.4s, v2.4s
    let add = decode(0x4ea2_8420);
    assert_eq!(add.op, Op::VecAdd);
    assert_eq!(
        add.form,
        Form::VecData {
            vd: full(0, ElemSize::S32),
            vn: full(1, ElemSize::S32),
            vm: Some(full(2, ElemSize::S32)),
            va: None,
        }
    );

    // sub v3.8h, v4.8h, v5.8h — the U bit selects subtract.
    let sub = decode(0x6e65_8483);
    assert_eq!(sub.op, Op::VecSub);
    assert_eq!(
        sub.form,
        Form::VecData {
            vd: full(3, ElemSize::H16),
            vn: full(4, ElemSize::H16),
            vm: Some(full(5, ElemSize::H16)),
            va: None,
        }
    );
}

#[test]
fn a_64_bit_arrangement_reports_half_the_lanes() {
    // add v6.8b, v7.8b, v8.8b — Q clear.
    let insn = decode(0x0e28_84e6);

    assert_eq!(insn.op, Op::VecAdd);
    assert_eq!(
        insn.form,
        Form::VecData {
            vd: half(6, ElemSize::B8),
            vn: half(7, ElemSize::B8),
            vm: Some(half(8, ElemSize::B8)),
            va: None,
        }
    );
}

#[test]
fn the_comparisons_split_by_signedness() {
    // CMHI/CMHS are the unsigned forms of CMGT/CMGE, sharing their opcodes.
    let expected = [
        (0x6e2b_8d49u32, Op::VecCmeq),
        (0x6eae_35ac, Op::VecCmhi),
        (0x6e71_3e0f, Op::VecCmhs),
        (0x4eb4_3672, Op::VecCmgt),
        (0x4e37_3ed5, Op::VecCmge),
    ];

    for (encoding, op) in expected {
        assert_eq!(decode(encoding).op, op, "{encoding:#010x}");
    }
}

#[test]
fn the_across_lanes_reductions_write_a_scalar() {
    // uminv b0, v1.16b — the destination is one element, not a vector, so a
    // decoder producing a vector would write fifteen lanes too many.
    let insn = decode(0x6e31_a820);

    assert_eq!(insn.op, Op::Uminv);
    assert_eq!(
        insn.form,
        Form::VecData {
            vd: VecOperand {
                reg: Vec::new(0),
                shape: VecShape::Scalar(ElemSize::B8),
            },
            vn: full(1, ElemSize::B8),
            vm: None,
            va: None,
        }
    );
}

#[test]
fn each_reduction_decodes_to_its_own_opcode() {
    let expected = [
        (0x6e31_a820u32, Op::Uminv),
        (0x6e30_a862, Op::Umaxv),
        (0x4e31_b8a4, Op::Addv),
        (0x4eb1_b8e6, Op::Addv),
    ];

    for (encoding, op) in expected {
        assert_eq!(decode(encoding).op, op, "{encoding:#010x}");
    }
}

#[test]
fn dup_from_a_general_purpose_register_names_both_files() {
    // dup v4.16b, w5
    let insn = decode(0x4e01_0ca4);

    assert_eq!(insn.op, Op::Dup);
    assert_eq!(
        insn.form,
        Form::VecGprMove {
            gpr: Gpr::X(5),
            vec: full(4, ElemSize::B8),
        }
    );

    // dup v6.2d, x7 — imm5 selects the element width.
    let wide = decode(0x4e08_0ce6);
    assert_eq!(wide.op, Op::Dup);
    assert_eq!(
        wide.form,
        Form::VecGprMove {
            gpr: Gpr::X(7),
            vec: full(6, ElemSize::D64),
        }
    );
}

#[test]
fn dup_from_an_element_addresses_the_source_lane() {
    // dup v8.4s, v9.s[2] — the lane index comes from imm5's upper bits.
    let insn = decode(0x4e14_0528);

    assert_eq!(insn.op, Op::Dup);
    assert_eq!(
        insn.form,
        Form::VecData {
            vd: full(8, ElemSize::S32),
            vn: lane(9, ElemSize::S32, 2),
            vm: None,
            va: None,
        }
    );
}

#[test]
fn umov_and_smov_read_a_lane_into_a_general_purpose_register() {
    // umov w10, v11.b[5]
    let unsigned = decode(0x0e0b_3d6a);
    assert_eq!(unsigned.op, Op::Umov);
    assert_eq!(
        unsigned.form,
        Form::VecGprMove {
            gpr: Gpr::X(10),
            vec: lane(11, ElemSize::B8, 5),
        }
    );

    // smov w14, v15.h[3] — sign-extending, so a separate opcode.
    let signed = decode(0x0e0e_2dee);
    assert_eq!(signed.op, Op::Smov);
    assert_eq!(
        signed.form,
        Form::VecGprMove {
            gpr: Gpr::X(14),
            vec: lane(15, ElemSize::H16, 3),
        }
    );
}

#[test]
fn umov_of_a_64_bit_lane_reports_a_64_bit_destination() {
    // umov x12, v13.d[1]
    let insn = decode(0x4e18_3dac);

    assert_eq!(insn.op, Op::Umov);
    assert_eq!(
        insn.form,
        Form::VecGprMove {
            gpr: Gpr::X(12),
            vec: lane(13, ElemSize::D64, 1),
        }
    );
}

#[test]
fn the_two_insert_forms_are_separate_opcodes() {
    // Phase A flagged Op::Ins as spanning two layouts; these are they.
    // ins v16.b[7], w17 — from a general-purpose register.
    let from_gpr = decode(0x4e0f_1e30);
    assert_eq!(from_gpr.op, Op::InsGpr);
    assert_eq!(
        from_gpr.form,
        Form::VecGprMove {
            gpr: Gpr::X(17),
            vec: lane(16, ElemSize::B8, 7),
        }
    );

    // ins v18.d[1], v19.d[0] — element to element, which reads no GPR at all.
    let from_element = decode(0x6e18_0672);
    assert_eq!(from_element.op, Op::Ins);
    assert_eq!(
        from_element.form,
        Form::VecData {
            vd: lane(18, ElemSize::D64, 1),
            vn: lane(19, ElemSize::D64, 0),
            vm: None,
            va: None,
        }
    );
}

#[test]
fn both_insert_forms_read_their_destination() {
    // An insert replaces one lane and preserves the rest, so the interpreter
    // must load the destination first.
    assert!(Op::Ins.reads_destination());
    assert!(Op::InsGpr.reads_destination());
}

#[test]
fn the_table_lookup_forms_carry_their_table_length() {
    // tbl v3.16b, { v4.16b }, v5.16b — one table register.
    let single = decode(0x4e05_0083);
    assert_eq!(single.op, Op::Tbl);
    assert_eq!(
        single.form,
        Form::TableLookup {
            vd: full(3, ElemSize::B8),
            table: Vec::new(4),
            table_len: 1,
            vm: full(5, ElemSize::B8),
        }
    );

    // tbl v6.8b, { v7.16b, v8.16b }, v9.8b — two, and a 64-bit result.
    let pair = decode(0x0e09_20e6);
    assert_eq!(pair.op, Op::Tbl);
    assert_eq!(
        pair.form,
        Form::TableLookup {
            vd: half(6, ElemSize::B8),
            table: Vec::new(7),
            table_len: 2,
            vm: half(9, ElemSize::B8),
        }
    );
}

#[test]
fn tbx_is_distinct_from_tbl_and_reads_its_destination() {
    // tbx v10.16b, { v11.16b }, v12.16b — TBX preserves the destination where
    // an index is out of range; TBL zeroes it.
    let insn = decode(0x4e0c_116a);

    assert_eq!(insn.op, Op::Tbx);
    assert!(Op::Tbx.reads_destination());
    assert!(!Op::Tbl.reads_destination());
}

#[test]
fn the_permute_group_decodes_all_six_mnemonics() {
    let expected = [
        (0x4e0d_398bu32, Op::Zip1),
        (0x4e90_79ee, Op::Zip2),
        (0x4e53_1a51, Op::Uzp1),
        (0x4e16_5ab4, Op::Uzp2),
        (0x4e99_2b17, Op::Trn1),
        (0x4e1c_6b7a, Op::Trn2),
    ];

    for (encoding, op) in expected {
        assert_eq!(decode(encoding).op, op, "{encoding:#010x}");
    }
}

#[test]
fn zip1_carries_both_sources_and_its_arrangement() {
    // zip1 v11.16b, v12.16b, v13.16b
    let insn = decode(0x4e0d_398b);

    assert_eq!(
        insn.form,
        Form::VecData {
            vd: full(11, ElemSize::B8),
            vn: full(12, ElemSize::B8),
            vm: Some(full(13, ElemSize::B8)),
            va: None,
        }
    );
}

#[test]
fn the_immediate_moves_expand_their_eight_bit_field() {
    // movi v0.16b, #0 — the byte-replicating form.
    let zero = decode(0x4f00_e400);
    assert_eq!(zero.op, Op::Movi);
    let Form::VecImm { imm, .. } = zero.form else {
        panic!("expected VecImm, got {:?}", zero.form);
    };
    assert_eq!(imm, 0);

    // movi v1.4s, #7 — a 32-bit element with no shift.
    let seven = decode(0x4f00_04e1);
    assert_eq!(seven.op, Op::Movi);
    let Form::VecImm { imm, vd, .. } = seven.form else {
        panic!("expected VecImm, got {:?}", seven.form);
    };
    assert_eq!(imm, 7);
    assert_eq!(vd.reg, Vec::new(1));
}

#[test]
fn mvni_is_a_separate_opcode_from_movi() {
    // mvni v2.8h, #3
    let insn = decode(0x6f00_8462);

    assert_eq!(insn.op, Op::Mvni);
    let Form::VecImm { imm, .. } = insn.form else {
        panic!("expected VecImm, got {:?}", insn.form);
    };
    assert_eq!(imm, 3);
}

#[test]
fn the_64_bit_movi_expands_each_immediate_bit_to_a_whole_byte() {
    // movi v3.2d, #0 — cmode 1110 with op set is the byte-mask form, where
    // imm8 bit n sets byte n. A decoder treating it as a plain byte value
    // would produce 0 for every input, so this uses a non-zero one.
    let insn = decode(0x6f00_e403);
    assert_eq!(insn.op, Op::Movi);

    // imm8 is split: `abc` in bits 18..16 and `defgh` in bits 9..5. Setting
    // bits 18..16 to 011 makes imm8 = 0b0110_0000, so bytes 5 and 6 are set —
    // not bytes 0 and 1, which is the mistake a decoder reading imm8 from one
    // contiguous field would make.
    let with_imm = 0x6f00_e403u32 | (0b011 << 16);
    let Form::VecImm { imm, .. } = decode(with_imm).form else {
        panic!("expected VecImm");
    };
    assert_eq!(imm, 0x00ff_ff00_0000_0000);

    // And `defgh` really is the low half: setting bit 5 sets byte 0.
    let low = 0x6f00_e403u32 | (1 << 5);
    let Form::VecImm { imm, .. } = decode(low).form else {
        panic!("expected VecImm");
    };
    assert_eq!(imm, 0x0000_0000_0000_00ff);
}

#[test]
fn the_shift_immediates_are_resolved_against_their_element_width() {
    // shl v0.4s, v1.4s, #7 — a left shift counts up from the element width.
    let left = decode(0x4f27_5420);
    assert_eq!(left.op, Op::VecShl);
    let Form::VecImm { imm, vd, .. } = left.form else {
        panic!("expected VecImm, got {:?}", left.form);
    };
    assert_eq!(imm, 7);
    assert_eq!(vd, full(0, ElemSize::S32));

    // ushr v2.8h, v3.8h, #5 — a right shift counts down from twice it, so the
    // two directions cannot share one formula.
    let right = decode(0x6f1b_0462);
    assert_eq!(right.op, Op::VecUshr);
    let Form::VecImm { imm, vd, .. } = right.form else {
        panic!("expected VecImm, got {:?}", right.form);
    };
    assert_eq!(imm, 5);
    assert_eq!(vd, full(2, ElemSize::H16));

    // sshr v4.16b, v5.16b, #3
    let signed = decode(0x4f0d_04a4);
    assert_eq!(signed.op, Op::VecSshr);
    let Form::VecImm { imm, .. } = signed.form else {
        panic!("expected VecImm");
    };
    assert_eq!(imm, 3);
}

#[test]
fn the_two_register_misc_group_decodes_its_subset() {
    let expected = [
        (0x6e20_5b38u32, Op::VecNot),
        (0x6ea0_b8e6, Op::VecNeg),
        (0x4e60_b928, Op::VecAbs),
        (0x4e20_596a, Op::VecCnt),
        (0x4e20_09ac, Op::VecRev64),
        (0x4e20_19ee, Op::VecRev16),
    ];

    for (encoding, op) in expected {
        assert_eq!(decode(encoding).op, op, "{encoding:#010x}");
    }
}

#[test]
fn ext_carries_its_byte_index() {
    // ext v0.16b, v1.16b, v2.16b, #5
    let insn = decode(0x6e02_2820);

    assert_eq!(insn.op, Op::Ext);
    let Form::VecImm { imm, vd, vn } = insn.form else {
        panic!("expected VecImm, got {:?}", insn.form);
    };
    assert_eq!(imm, 5);
    assert_eq!(vd, full(0, ElemSize::B8));
    assert_eq!(vn, full(1, ElemSize::B8));
}

#[test]
fn the_crypto_encodings_stay_unallocated() {
    // docs/machine-spec.md §2 advertises no AES, SHA or PMULL, so these must
    // fault exactly as an unimplemented encoding does. QEMU cannot disable
    // them, which is why the fuzz corpus draws none and this is asserted here.
    let crypto = [
        0x4e28_4800u32, // aese v0.16b, v0.16b
        0x4e28_5800,    // aesmc v0.16b, v0.16b
        0x5e00_0000,    // sha1c q0, s0, v0.4s
        0x5e28_0800,    // sha1h s0, s0
        0x0e22_e020,    // pmull v0.8h, v1.8b, v2.8b
    ];

    for encoding in crypto {
        assert!(
            decode(encoding).op.is_unallocated(),
            "{encoding:#010x} must fault"
        );
    }
}

#[test]
fn unclaimed_neon_encodings_still_fault() {
    // The plan implements NEON lazily off a trap-and-log, so anything this
    // slice has not claimed must fault rather than decode approximately.
    let unclaimed = [
        0x4e21_f800u32, // fmla v0.4s, v0.4s, v1.4s — the FP vector group
        0x0e20_2800,    // saddl, a widening form
    ];

    for encoding in unclaimed {
        assert!(
            decode(encoding).op.is_unallocated(),
            "{encoding:#010x} is not claimed yet"
        );
    }
}

#[test]
fn decoding_the_advanced_simd_space_never_panics() {
    let mut state = 0x4e22_1c20u64;
    for _ in 0..200_000 {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        // Force the word into the advanced SIMD group: op0 = x111, bit 28
        // clear.
        let encoding = (((state as u32) & !(0b1111 << 25)) | (0b0111 << 25)) & !(1 << 28);
        assert_eq!(decode(encoding).encoding, encoding);
    }
}
