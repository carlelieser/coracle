//! SIMD/FP loads and stores — the `V = 1` half of the loads-and-stores group.
//!
//! Encodings verified with `llvm-mc -arch=aarch64 -show-encoding`. The general
//! -purpose half of this group belongs to the memory slice; these tests also
//! pin that the two halves stay apart, because both decoders answer for the
//! same `op0`.

use coracle_core::decode::address::{AddrMode, Ordering, WriteBack};
use coracle_core::decode::decode;
use coracle_core::decode::instruction::Form;
use coracle_core::decode::op::Op;
use coracle_core::decode::operand::{ElemSize, ExtendKind, VecHalf, VecOperand, VecShape};
use coracle_core::reg::{Gpr, Vec};

/// The scalar operand a single-register transfer names.
fn scalar(index: u8, size: ElemSize) -> VecOperand {
    VecOperand {
        reg: Vec::new(index),
        shape: VecShape::Scalar(size),
    }
}

/// Unwraps a `LoadStoreVec` form, failing loudly on anything else.
fn vec_form(encoding: u32) -> (VecOperand, u8, AddrMode) {
    let insn = decode(encoding);
    match insn.form {
        Form::LoadStoreVec {
            vt, count, addr, ..
        } => (vt, count, addr),
        other => panic!("expected LoadStoreVec for {encoding:#010x}, got {other:?}"),
    }
}

#[test]
fn an_unsigned_offset_load_scales_its_immediate_by_the_access_width() {
    // ldr q3, [x4, #32] — imm12 = 2, scaled by 16.
    let insn = decode(0x3dc0_0883);

    assert_eq!(insn.op, Op::Ldr);
    assert_eq!(
        insn.form,
        Form::LoadStoreVec {
            vt: scalar(3, ElemSize::Q128),
            count: 1,
            addr: AddrMode::Immediate {
                base: Gpr::X(4),
                offset: 32,
                writeback: WriteBack::None,
            },
            ordering: Ordering::PLAIN,
        }
    );
}

#[test]
fn each_access_width_scales_its_own_immediate() {
    // The scale is the access width, so the same imm12 means a different byte
    // offset per width. A decoder using one fixed scale fails here.
    let expected = [
        (0xfd00_08c5u32, Op::Str, 5, ElemSize::D64, 6, 16i64),
        (0x3d40_0107, Op::Ldr, 7, ElemSize::B8, 8, 0),
        (0x7d40_0949, Op::Ldr, 9, ElemSize::H16, 10, 4),
        (0xbd00_098b, Op::Str, 11, ElemSize::S32, 12, 8),
    ];

    for (encoding, op, vt_index, size, base, offset) in expected {
        let insn = decode(encoding);
        assert_eq!(insn.op, op, "{encoding:#010x}");
        assert_eq!(
            insn.form,
            Form::LoadStoreVec {
                vt: scalar(vt_index, size),
                count: 1,
                addr: AddrMode::Immediate {
                    base: Gpr::X(base),
                    offset,
                    writeback: WriteBack::None,
                },
                ordering: Ordering::PLAIN,
            },
            "{encoding:#010x}"
        );
    }
}

#[test]
fn the_q_form_is_size_00_with_the_wide_bit_rather_than_size_11() {
    // A 128-bit access encodes size = 00 and opc<1> = 1; reading `size` alone
    // would call it a byte.
    let (vt, _, _) = vec_form(0x3dc0_0883);
    assert_eq!(vt.shape, VecShape::Scalar(ElemSize::Q128));

    // And size = 11 with the wide bit set is unallocated, not a wider access.
    assert!(decode(0x3dc0_0883 | (0b11 << 30)).op.is_unallocated());
}

#[test]
fn the_unscaled_form_sign_extends_its_offset() {
    // ldur d13, [x14, #-8]
    let (vt, count, addr) = vec_form(0xfc5f_81cd);

    assert_eq!(vt, scalar(13, ElemSize::D64));
    assert_eq!(count, 1);
    assert_eq!(
        addr,
        AddrMode::Immediate {
            base: Gpr::X(14),
            offset: -8,
            writeback: WriteBack::None,
        }
    );
}

#[test]
fn the_indexed_forms_report_when_the_base_is_updated() {
    // ldr q15, [x16], #16 — post-indexed.
    let (_, _, post) = vec_form(0x3cc1_060f);
    assert_eq!(
        post,
        AddrMode::Immediate {
            base: Gpr::X(16),
            offset: 16,
            writeback: WriteBack::Post,
        }
    );

    // str q17, [x18, #32]! — pre-indexed, so the access uses the new value.
    let (_, _, pre) = vec_form(0x3c82_0e51);
    assert_eq!(
        pre,
        AddrMode::Immediate {
            base: Gpr::X(18),
            offset: 32,
            writeback: WriteBack::Pre,
        }
    );
}

#[test]
fn the_register_offset_form_carries_its_extension_and_scale() {
    // ldr s19, [x20, x21, lsl #2] — the S bit scales by the access width.
    let (vt, _, addr) = vec_form(0xbc75_7a93);

    assert_eq!(vt, scalar(19, ElemSize::S32));
    assert_eq!(
        addr,
        AddrMode::Register {
            base: Gpr::X(20),
            index: coracle_core::decode::operand::ExtendedReg {
                reg: Gpr::X(21),
                kind: ExtendKind::Uxtx,
                amount: 2,
            },
            writeback: WriteBack::None,
        }
    );
}

#[test]
fn a_sign_extending_index_keeps_its_extension_kind() {
    // ldr q22, [x23, w24, sxtw #4] — a 32-bit index, sign-extended, scaled by
    // 16 for a Q access.
    let (_, _, addr) = vec_form(0x3cf8_daf6);

    assert_eq!(
        addr,
        AddrMode::Register {
            base: Gpr::X(23),
            index: coracle_core::decode::operand::ExtendedReg {
                reg: Gpr::X(24),
                kind: ExtendKind::Sxtw,
                amount: 4,
            },
            writeback: WriteBack::None,
        }
    );
}

#[test]
fn the_pair_forms_transfer_two_registers() {
    // ldp q25, q26, [x27, #64]
    let insn = decode(0xad42_6b79);

    assert_eq!(insn.op, Op::Ldp);
    let Form::LoadStoreVec {
        vt, count, addr, ..
    } = insn.form
    else {
        panic!("expected LoadStoreVec, got {:?}", insn.form);
    };
    assert_eq!(vt, scalar(25, ElemSize::Q128));
    assert_eq!(count, 2);
    assert_eq!(
        addr,
        AddrMode::Immediate {
            base: Gpr::X(27),
            offset: 64,
            writeback: WriteBack::None,
        }
    );
}

#[test]
fn each_pair_addressing_mode_decodes_with_its_writeback() {
    // stp d28, d29, [x30, #16] — plain, scaled by 8.
    let (vt, count, plain) = vec_form(0x6d01_77dc);
    assert_eq!(vt, scalar(28, ElemSize::D64));
    assert_eq!(count, 2);
    assert_eq!(
        plain,
        AddrMode::Immediate {
            base: Gpr::X(30),
            offset: 16,
            writeback: WriteBack::None,
        }
    );

    // ldp s1, s2, [x3], #8 — post-indexed, scaled by 4.
    let (_, _, post) = vec_form(0x2cc1_0861);
    assert_eq!(
        post,
        AddrMode::Immediate {
            base: Gpr::X(3),
            offset: 8,
            writeback: WriteBack::Post,
        }
    );

    // stp q4, q5, [x6, #32]! — pre-indexed.
    let (_, _, pre) = vec_form(0xad81_14c4);
    assert_eq!(
        pre,
        AddrMode::Immediate {
            base: Gpr::X(6),
            offset: 32,
            writeback: WriteBack::Pre,
        }
    );
}

#[test]
fn the_structure_forms_report_their_register_count_and_arrangement() {
    // ld1 { v0.16b }, [x1] — one register, sixteen byte lanes.
    let insn = decode(0x4c40_7020);

    assert_eq!(insn.op, Op::Ld1);
    assert_eq!(
        insn.form,
        Form::LoadStoreVec {
            vt: VecOperand {
                reg: Vec::new(0),
                shape: VecShape::Vector {
                    elem: ElemSize::B8,
                    count: 16,
                    half: VecHalf::Full,
                },
            },
            count: 1,
            addr: AddrMode::BaseOnly { base: Gpr::X(1) },
            ordering: Ordering::PLAIN,
        }
    );
}

#[test]
fn ld1_transfers_between_one_and_four_registers() {
    // The count comes from the opcode, not the mnemonic: LD1 spans all four.
    let expected = [
        (0x4c40_7020u32, 1u8, 0u8),
        (0x0c40_a082, 2, 2),
        (0x4c40_6905, 3, 5),
        (0x4c40_2da9, 4, 9),
    ];

    for (encoding, count, first) in expected {
        let insn = decode(encoding);
        assert_eq!(insn.op, Op::Ld1, "{encoding:#010x}");
        let Form::LoadStoreVec { vt, count: got, .. } = insn.form else {
            panic!("expected LoadStoreVec for {encoding:#010x}");
        };
        assert_eq!(got, count, "{encoding:#010x}");
        assert_eq!(vt.reg, Vec::new(first), "{encoding:#010x}");
    }
}

#[test]
fn the_de_interleaving_forms_have_a_fixed_register_count() {
    // LD2, LD3 and LD4 always transfer two, three and four.
    let expected = [
        (0x4c40_8a0e_u32, Op::Ld2, 2u8, 14u8),
        (0x4c40_4691, Op::Ld3, 3, 17),
        (0x4c40_0335, Op::Ld4, 4, 21),
    ];

    for (encoding, op, count, first) in expected {
        let insn = decode(encoding);
        assert_eq!(insn.op, op, "{encoding:#010x}");
        let Form::LoadStoreVec { vt, count: got, .. } = insn.form else {
            panic!("expected LoadStoreVec for {encoding:#010x}");
        };
        assert_eq!(got, count, "{encoding:#010x}");
        assert_eq!(vt.reg, Vec::new(first), "{encoding:#010x}");
    }
}

#[test]
fn the_store_forms_share_the_load_encodings_but_a_different_opcode() {
    // st1 { v0.16b }, [x1] — the same shape as its load, bit 22 clear.
    assert_eq!(decode(0x4c00_7020).op, Op::St1);
    assert_eq!(decode(0x4c00_8882).op, Op::St2);
}

#[test]
fn a_half_width_arrangement_reports_its_lanes_and_half() {
    // ld1 { v2.8b, v3.8b }, [x4] — Q clear, so eight lanes in the low half.
    let (vt, count, _) = vec_form(0x0c40_a082);

    assert_eq!(count, 2);
    assert_eq!(
        vt.shape,
        VecShape::Vector {
            elem: ElemSize::B8,
            count: 8,
            half: VecHalf::Low,
        }
    );
}

#[test]
fn the_immediate_post_indexed_structure_form_advances_by_its_transfer_size() {
    // ld1 { v0.16b }, [x1], #16 — Rm = 31 means "advance by what was
    // transferred" rather than naming register 31.
    let (_, count, addr) = vec_form(0x4cdf_7020);

    assert_eq!(count, 1);
    assert_eq!(
        addr,
        AddrMode::Immediate {
            base: Gpr::X(1),
            offset: 16,
            writeback: WriteBack::Post,
        }
    );

    // ld2 { v5.4s, v6.4s }, [x7], #32 — two registers, so twice as far.
    let (_, pair_count, pair_addr) = vec_form(0x4cdf_88e5);
    assert_eq!(pair_count, 2);
    assert_eq!(
        pair_addr,
        AddrMode::Immediate {
            base: Gpr::X(7),
            offset: 32,
            writeback: WriteBack::Post,
        }
    );
}

#[test]
fn a_register_post_indexed_structure_form_names_its_index() {
    // ld1 { v2.8b }, [x3], x4 — a real register, so not the immediate form.
    let (_, _, addr) = vec_form(0x0cc4_7062);

    assert_eq!(
        addr,
        AddrMode::Register {
            base: Gpr::X(3),
            index: coracle_core::decode::operand::ExtendedReg {
                reg: Gpr::X(4),
                kind: ExtendKind::Uxtx,
                amount: 0,
            },
            writeback: WriteBack::Post,
        }
    );
}

#[test]
fn a_64_bit_arrangement_of_64_bit_elements_is_unallocated() {
    // size = 11 with Q clear would be a single lane, which has no encoding.
    let single_lane = (0x4c40_7020u32 & !(1 << 30)) | (0b11 << 10);
    assert!(decode(single_lane).op.is_unallocated());
}

#[test]
fn the_general_purpose_half_of_the_group_still_belongs_to_the_memory_slice() {
    // Same addressing mode, V = 0: this must stay a `LoadStore`, not become a
    // `LoadStoreVec`. Both decoders answer for the same op0, so the split is
    // worth pinning from this side too.
    let insn = decode(0xf940_0820);

    assert_eq!(insn.op, Op::Ldr);
    assert!(
        matches!(insn.form, Form::LoadStore { .. }),
        "expected a general-purpose LoadStore, got {:?}",
        insn.form
    );
}

#[test]
fn decoding_the_vector_load_store_space_never_panics() {
    // The fuzz corpus at the M1 gate depends on totality here.
    let mut state = 0x3dc0_0883u64;
    for _ in 0..100_000 {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        // Force V = 1 inside the loads-and-stores group.
        let encoding = ((state as u32) & !(0b1111 << 25)) | (0b0110 << 25) | (1 << 26);
        assert_eq!(decode(encoding).encoding, encoding);
    }
}
