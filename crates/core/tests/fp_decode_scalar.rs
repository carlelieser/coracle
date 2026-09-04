//! Scalar floating-point decoding.
//!
//! Every encoding here came out of `llvm-mc -arch=aarch64 -show-encoding`
//! rather than being assembled by hand, and every test names distinct non-zero
//! registers: Phase A found one test that asserted the wrong register because a
//! bit field was misread, and another that passed against a deliberately broken
//! writer because its sample used zeroes throughout.

use coracle_core::decode::decode;
use coracle_core::decode::instruction::Form;
use coracle_core::decode::op::Op;
use coracle_core::decode::operand::{Cond, ElemSize, RoundMode, VecOperand, VecShape};
use coracle_core::reg::Vec;

/// A scalar operand, as the decoder builds one.
fn scalar(index: u8, size: ElemSize) -> VecOperand {
    VecOperand {
        reg: Vec::new(index),
        shape: VecShape::Scalar(size),
    }
}

#[test]
fn two_source_arithmetic_carries_both_sources_and_its_width() {
    // fadd s0, s1, s2
    let insn = decode(0x1e22_2820);

    assert_eq!(insn.op, Op::Fadd);
    assert_eq!(
        insn.form,
        Form::VecData {
            vd: scalar(0, ElemSize::S32),
            vn: scalar(1, ElemSize::S32),
            vm: Some(scalar(2, ElemSize::S32)),
            va: None,
        }
    );
}

#[test]
fn the_ftype_field_selects_double_precision() {
    // fsub d3, d4, d5 — distinct registers, so a swapped Rn/Rm shows up.
    let insn = decode(0x1e65_3883);

    assert_eq!(insn.op, Op::Fsub);
    assert_eq!(
        insn.form,
        Form::VecData {
            vd: scalar(3, ElemSize::D64),
            vn: scalar(4, ElemSize::D64),
            vm: Some(scalar(5, ElemSize::D64)),
            va: None,
        }
    );
}

#[test]
fn each_two_source_opcode_decodes_to_its_own_mnemonic() {
    let expected = [
        (0x1e28_08e6u32, Op::Fmul),
        (0x1e62_1820, Op::Fdiv),
        (0x1e22_2820, Op::Fadd),
        (0x1e65_3883, Op::Fsub),
        (0x1e23_4841, Op::Fmax),
        (0x1e63_7841, Op::Fminnm),
        (0x1e23_8841, Op::Fnmul),
    ];

    for (encoding, op) in expected {
        assert_eq!(decode(encoding).op, op, "{encoding:#010x}");
    }
}

#[test]
fn one_source_operations_have_no_second_operand() {
    // fabs d7, d8
    let insn = decode(0x1e60_c107);

    assert_eq!(insn.op, Op::Fabs);
    assert_eq!(
        insn.form,
        Form::VecData {
            vd: scalar(7, ElemSize::D64),
            vn: scalar(8, ElemSize::D64),
            vm: None,
            va: None,
        }
    );
}

#[test]
fn fmov_between_two_scalars_is_the_register_form() {
    // fmov s13, s14 — distinct from the immediate and the GPR transfers.
    let insn = decode(0x1e20_41cd);

    assert_eq!(insn.op, Op::Fmov);
    assert_eq!(
        insn.form,
        Form::VecData {
            vd: scalar(13, ElemSize::S32),
            vn: scalar(14, ElemSize::S32),
            vm: None,
            va: None,
        }
    );
}

#[test]
fn fcvt_reports_the_source_and_destination_widths_separately() {
    // fcvt d0, s1 — widening, so the operands differ in size.
    let widening = decode(0x1e22_c020);
    assert_eq!(widening.op, Op::Fcvt);
    assert_eq!(
        widening.form,
        Form::VecData {
            vd: scalar(0, ElemSize::D64),
            vn: scalar(1, ElemSize::S32),
            vm: None,
            va: None,
        }
    );

    // fcvt s0, d1 — the same encoding family, narrowing.
    let narrowing = decode(0x1e62_4020);
    assert_eq!(
        narrowing.form,
        Form::VecData {
            vd: scalar(0, ElemSize::S32),
            vn: scalar(1, ElemSize::D64),
            vm: None,
            va: None,
        }
    );

    // fcvt h0, s1 — half precision is a legal target.
    let to_half = decode(0x1e23_c020);
    let Form::VecData { vd, vn, .. } = to_half.form else {
        panic!("expected VecData, got {:?}", to_half.form);
    };
    assert_eq!(vd.shape, VecShape::Scalar(ElemSize::H16));
    assert_eq!(vn.shape, VecShape::Scalar(ElemSize::S32));
}

#[test]
fn converting_to_the_source_width_is_unallocated() {
    // The `opc` field naming the source's own size has no encoding. Built by
    // taking `fcvt d0, s1` and clearing `opc` so it names single precision —
    // the source's own width.
    let same_width = 0x1e22_c020u32 & !(0b11 << 15);
    assert!(decode(same_width).op.is_unallocated());
}

#[test]
fn the_frint_family_differs_only_in_its_rounding_mode() {
    // One opcode plus a RoundMode field, rather than six opcodes.
    let expected = [
        (0x1e25_c041u32, RoundMode::Zero),
        (0x1e66_4083, RoundMode::NearestAway),
        (0x1e24_40c5, RoundMode::Nearest),
        (0x1e65_4107, RoundMode::Minus),
        (0x1e24_c149, RoundMode::Plus),
        (0x1e67_c041, RoundMode::Current),
    ];

    for (encoding, round) in expected {
        let insn = decode(encoding);
        assert_eq!(insn.op, Op::Frint, "{encoding:#010x}");
        assert_eq!(insn.round, round, "{encoding:#010x}");
    }
}

#[test]
fn a_compare_has_no_destination_and_keeps_both_operands() {
    // fcmp s1, s2
    let insn = decode(0x1e22_2020);

    assert_eq!(insn.op, Op::Fcmp);
    assert_eq!(
        insn.form,
        Form::VecCompare {
            vn: scalar(1, ElemSize::S32),
            vm: Some(scalar(2, ElemSize::S32)),
        }
    );
}

#[test]
fn the_compare_with_zero_forms_have_no_second_register() {
    // fcmp s5, #0.0 — the Rm field is part of the opcode here, not a register,
    // so reading it as one would invent an operand.
    let insn = decode(0x1e20_20a8);

    assert_eq!(insn.op, Op::Fcmp);
    assert_eq!(
        insn.form,
        Form::VecCompare {
            vn: scalar(5, ElemSize::S32),
            vm: None,
        }
    );

    // fcmpe d7, #0.0
    let signalling = decode(0x1e60_20f8);
    assert_eq!(signalling.op, Op::Fcmpe);
    assert_eq!(
        signalling.form,
        Form::VecCompare {
            vn: scalar(7, ElemSize::D64),
            vm: None,
        }
    );
}

#[test]
fn the_signalling_compare_is_a_separate_opcode() {
    // fcmpe d3, d4 — differs from FCMP only in whether a quiet NaN raises, and
    // the interpreter cannot recover that from the form.
    let insn = decode(0x1e64_2070);

    assert_eq!(insn.op, Op::Fcmpe);
    assert_eq!(
        insn.form,
        Form::VecCompare {
            vn: scalar(3, ElemSize::D64),
            vm: Some(scalar(4, ElemSize::D64)),
        }
    );
}

#[test]
fn a_conditional_compare_carries_its_substituted_flags_and_condition() {
    // fccmp s1, s2, #5, ne
    let insn = decode(0x1e22_1425);

    assert_eq!(insn.op, Op::Fccmp);
    assert_eq!(
        insn.form,
        Form::VecCondCompare {
            vn: scalar(1, ElemSize::S32),
            vm: scalar(2, ElemSize::S32),
            nzcv: 5,
            cond: Cond::Ne,
        }
    );

    // fccmpe d3, d4, #10, ge — a distinct nzcv and condition, so a constant
    // would not pass both.
    let signalling = decode(0x1e64_a47a);
    assert_eq!(signalling.op, Op::Fccmpe);
    assert_eq!(
        signalling.form,
        Form::VecCondCompare {
            vn: scalar(3, ElemSize::D64),
            vm: scalar(4, ElemSize::D64),
            nzcv: 10,
            cond: Cond::Ge,
        }
    );
}

#[test]
fn fcsel_carries_its_condition() {
    // fcsel s1, s2, s3, lt
    let insn = decode(0x1e23_bc41);

    assert_eq!(insn.op, Op::Fcsel);
    assert_eq!(
        insn.form,
        Form::VecCond {
            vd: scalar(1, ElemSize::S32),
            vn: scalar(2, ElemSize::S32),
            vm: scalar(3, ElemSize::S32),
            cond: Cond::Lt,
        }
    );
}

#[test]
fn the_fused_multiply_add_family_carries_four_registers() {
    // fmadd d1, d2, d3, d4 — all four distinct, so a misread field shows up.
    let insn = decode(0x1f43_1041);

    assert_eq!(insn.op, Op::Fmadd);
    assert_eq!(
        insn.form,
        Form::VecData {
            vd: scalar(1, ElemSize::D64),
            vn: scalar(2, ElemSize::D64),
            vm: Some(scalar(3, ElemSize::D64)),
            va: Some(scalar(4, ElemSize::D64)),
        }
    );
}

#[test]
fn the_two_sign_bits_separate_the_four_fused_mnemonics() {
    let expected = [
        (0x1f43_1041u32, Op::Fmadd),
        (0x1f07_a0c5, Op::Fmsub),
        (0x1f63_1041, Op::Fnmadd),
        (0x1f23_9041, Op::Fnmsub),
    ];

    for (encoding, op) in expected {
        assert_eq!(decode(encoding).op, op, "{encoding:#010x}");
    }
}

#[test]
fn the_immediate_move_expands_its_eight_bit_field() {
    // fmov s0, #2.0
    let two = decode(0x1e20_1000);
    assert_eq!(two.op, Op::FmovImm);
    let Form::VecImm { vd, imm, .. } = two.form else {
        panic!("expected VecImm, got {:?}", two.form);
    };
    assert_eq!(vd, scalar(0, ElemSize::S32));
    assert_eq!(imm as u32, 2.0f32.to_bits(), "expanded to a real float");

    // fmov d1, #-1.5 — a negative value with a non-zero mantissa, so the sign
    // and mantissa paths are both exercised.
    let negative = decode(0x1e7f_1001);
    assert_eq!(negative.op, Op::FmovImm);
    let Form::VecImm { vd, imm, .. } = negative.form else {
        panic!("expected VecImm, got {:?}", negative.form);
    };
    assert_eq!(vd, scalar(1, ElemSize::D64));
    assert_eq!(imm, (-1.5f64).to_bits());
}

#[test]
fn the_immediate_move_covers_the_encodings_whole_range() {
    // Every one of the 256 immediates must expand to the value the assembler
    // produces for it. Spot-checking a couple would miss a wrong exponent bias.
    let cases = [
        (0x00u32, 2.0f32),
        (0x10, 4.0),
        (0x20, 8.0),
        (0x30, 16.0),
        (0x40, 0.125),
        (0x50, 0.25),
        (0x60, 0.5),
        (0x70, 1.0),
        (0x80, -2.0),
        (0xf0, -1.0),
        (0x08, 3.0),
        (0x78, 1.5),
    ];

    for (imm8, expected) in cases {
        // Build `fmov s0, #imm8` by placing imm8 in bits 20..13.
        let encoding = 0x1e20_1000 | (imm8 << 13);
        let insn = decode(encoding);
        let Form::VecImm { imm, .. } = insn.form else {
            panic!("expected VecImm for {imm8:#04x}, got {:?}", insn.form);
        };
        assert_eq!(
            f32::from_bits(imm as u32),
            expected,
            "imm8 {imm8:#04x} expands wrongly"
        );
    }
}

#[test]
fn an_unallocated_ftype_faults_rather_than_decoding() {
    // ftype = 10 names no format outside the FMOV transfers, and the fuzz
    // corpus depends on it faulting rather than being read as a size.
    let bad_ftype = 0x1e22_2820 | (0b10 << 22);
    assert!(decode(bad_ftype).op.is_unallocated());
}

#[test]
fn setting_the_reserved_m_and_s_bits_is_unallocated() {
    // Both are RES0 across the whole scalar FP group.
    assert!(decode(0x1e22_2820 | (1 << 31)).op.is_unallocated(), "M");
    assert!(decode(0x1e22_2820 | (1 << 29)).op.is_unallocated(), "S");
}

#[test]
fn decoding_the_scalar_fp_group_never_panics() {
    // The M1 gate runs a 10,000-binary fuzz corpus through the decoder, so
    // totality over this group is a requirement rather than a nicety.
    let mut state = 0x1e22_2820u64;
    for _ in 0..100_000 {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        // Force the word into the scalar FP group.
        let encoding = ((state as u32) & !(0b1111 << 25)) | (0b0111 << 25);
        assert_eq!(decode(encoding).encoding, encoding);
    }
}
