//! Conversions between the SIMD/FP and general-purpose register files.
//!
//! Encodings verified with `llvm-mc -arch=aarch64 -show-encoding`. Every case
//! names distinct registers on the two sides, so a decoder that swapped the
//! source and destination fields — the easy mistake in a family where the
//! direction is implied by the opcode — fails rather than passing by symmetry.

use coracle_core::decode::decode;
use coracle_core::decode::instruction::Form;
use coracle_core::decode::op::Op;
use coracle_core::decode::operand::{ElemSize, RegWidth, RoundMode, VecOperand, VecShape};
use coracle_core::reg::{Gpr, Vec};

/// The `Form::VecGprMove` a conversion should produce.
fn transfer(gpr: u8, vec_index: u8, size: ElemSize) -> Form {
    Form::VecGprMove {
        gpr: Gpr::X(gpr),
        vec: VecOperand {
            reg: Vec::new(vec_index),
            shape: VecShape::Scalar(size),
        },
    }
}

#[test]
fn converting_to_a_signed_integer_names_the_gpr_as_its_destination() {
    // fcvtzs w3, s4
    let insn = decode(0x1e38_0083);

    assert_eq!(insn.op, Op::Fcvtzs);
    assert_eq!(insn.width, RegWidth::W32);
    assert_eq!(insn.form, transfer(3, 4, ElemSize::S32));
}

#[test]
fn the_sf_bit_selects_a_64_bit_general_purpose_operand() {
    // fcvtzs x5, d6 — the same opcode with sf set.
    let insn = decode(0x9e78_00c5);

    assert_eq!(insn.op, Op::Fcvtzs);
    assert_eq!(insn.width, RegWidth::X64);
    assert_eq!(insn.form, transfer(5, 6, ElemSize::D64));
}

#[test]
fn the_gpr_and_fp_widths_are_independent() {
    // fcvtzu w7, d8 — a 32-bit destination from a 64-bit source, which is the
    // combination that catches a decoder deriving one width from the other.
    let insn = decode(0x1e79_0107);

    assert_eq!(insn.op, Op::Fcvtzu);
    assert_eq!(insn.width, RegWidth::W32);
    assert_eq!(insn.form, transfer(7, 8, ElemSize::D64));
}

#[test]
fn the_rounding_mode_family_shares_one_opcode_pair() {
    // FCVTAS/MS/NS/PS differ from FCVTZS only in the mode they name.
    let expected = [
        (0x9e25_0149u32, Op::Fcvtu, RoundMode::NearestAway),
        (0x1e70_018b, Op::Fcvts, RoundMode::Minus),
        (0x9e20_01cd, Op::Fcvts, RoundMode::Nearest),
        (0x1e68_020f, Op::Fcvts, RoundMode::Plus),
        (0x9e24_0251, Op::Fcvts, RoundMode::NearestAway),
    ];

    for (encoding, op, round) in expected {
        let insn = decode(encoding);
        assert_eq!(insn.op, op, "{encoding:#010x}");
        assert_eq!(insn.round, round, "{encoding:#010x}");
    }
}

#[test]
fn converting_from_an_integer_names_the_gpr_as_its_source() {
    // scvtf s19, w20 — the direction is the opposite of FCVTZS, and the fields
    // swap with it.
    let insn = decode(0x1e22_0293);

    assert_eq!(insn.op, Op::Scvtf);
    assert_eq!(insn.width, RegWidth::W32);
    assert_eq!(insn.form, transfer(20, 19, ElemSize::S32));
}

#[test]
fn each_integer_to_fp_conversion_keeps_its_signedness_and_widths() {
    let expected = [
        (
            0x9e62_02d5u32,
            Op::Scvtf,
            RegWidth::X64,
            22,
            21,
            ElemSize::D64,
        ),
        (0x1e23_0317, Op::Ucvtf, RegWidth::W32, 24, 23, ElemSize::S32),
        (0x9e63_0359, Op::Ucvtf, RegWidth::X64, 26, 25, ElemSize::D64),
    ];

    for (encoding, op, width, gpr, vec, size) in expected {
        let insn = decode(encoding);
        assert_eq!(insn.op, op, "{encoding:#010x}");
        assert_eq!(insn.width, width, "{encoding:#010x}");
        assert_eq!(insn.form, transfer(gpr, vec, size), "{encoding:#010x}");
    }
}

#[test]
fn fmov_to_and_from_a_gpr_are_separate_opcodes() {
    // Phase A flagged that one Fmov opcode spans four operand layouts, so the
    // interpreter cannot dispatch on it alone. These two are the transfers.
    // fmov w27, s28 — FP to GPR.
    let from_vec = decode(0x1e26_039b);
    assert_eq!(from_vec.op, Op::FmovFromVec);
    assert_eq!(from_vec.width, RegWidth::W32);
    assert_eq!(from_vec.form, transfer(27, 28, ElemSize::S32));

    // fmov s29, w30 — GPR to FP, the opposite direction.
    let to_vec = decode(0x1e27_03dd);
    assert_eq!(to_vec.op, Op::FmovToVec);
    assert_eq!(to_vec.width, RegWidth::W32);
    assert_eq!(to_vec.form, transfer(30, 29, ElemSize::S32));
}

#[test]
fn the_64_bit_fmov_transfers_use_double_precision_scalars() {
    // fmov x1, d2
    let from_vec = decode(0x9e66_0041);
    assert_eq!(from_vec.op, Op::FmovFromVec);
    assert_eq!(from_vec.width, RegWidth::X64);
    assert_eq!(from_vec.form, transfer(1, 2, ElemSize::D64));

    // fmov d3, x4
    let to_vec = decode(0x9e67_0083);
    assert_eq!(to_vec.op, Op::FmovToVec);
    assert_eq!(to_vec.form, transfer(4, 3, ElemSize::D64));
}

#[test]
fn the_high_half_fmov_transfers_address_a_lane_rather_than_a_scalar() {
    // fmov v5.d[1], x6 — this moves into the *upper* 64 bits, so decoding it
    // as a plain scalar would write the wrong half of the register.
    let to_lane = decode(0x9eaf_00c5);
    assert_eq!(to_lane.op, Op::FmovToVec);
    assert_eq!(
        to_lane.form,
        Form::VecGprMove {
            gpr: Gpr::X(6),
            vec: VecOperand {
                reg: Vec::new(5),
                shape: VecShape::Element {
                    elem: ElemSize::D64,
                    index: 1,
                },
            },
        }
    );

    // fmov x7, v8.d[1] — the same lane, read back.
    let from_lane = decode(0x9eae_0107);
    assert_eq!(from_lane.op, Op::FmovFromVec);
    assert_eq!(
        from_lane.form,
        Form::VecGprMove {
            gpr: Gpr::X(7),
            vec: VecOperand {
                reg: Vec::new(8),
                shape: VecShape::Element {
                    elem: ElemSize::D64,
                    index: 1,
                },
            },
        }
    );
}

#[test]
fn a_32_bit_fmov_between_files_cannot_name_a_double() {
    // The GPR and FP widths must agree for the plain transfers; `sf` clear with
    // ftype naming a double has no encoding.
    let mismatched = 0x1e26_039b | (0b01 << 22);
    assert!(decode(mismatched).op.is_unallocated());
}

#[test]
fn the_fixed_point_conversions_report_round_toward_zero() {
    // fcvtzs w9, s10, #7 — the fixed-point forms exist only for the
    // round-toward-zero conversions and the integer-to-FP pair.
    let insn = decode(0x1e18_e549);

    assert_eq!(insn.op, Op::Fcvtzs);
    assert_eq!(insn.width, RegWidth::W32);
    assert_eq!(insn.round, RoundMode::Zero);
    assert_eq!(insn.form, transfer(9, 10, ElemSize::S32));
}

#[test]
fn each_fixed_point_conversion_decodes_with_its_direction() {
    let expected = [
        (
            0x9e42_cd8b_u32,
            Op::Scvtf,
            RegWidth::X64,
            12,
            11,
            ElemSize::D64,
        ),
        (0x1e03_edcd, Op::Ucvtf, RegWidth::W32, 14, 13, ElemSize::S32),
        (
            0x9e59_b20f,
            Op::Fcvtzu,
            RegWidth::X64,
            15,
            16,
            ElemSize::D64,
        ),
    ];

    for (encoding, op, width, gpr, vec, size) in expected {
        let insn = decode(encoding);
        assert_eq!(insn.op, op, "{encoding:#010x}");
        assert_eq!(insn.width, width, "{encoding:#010x}");
        assert_eq!(insn.form, transfer(gpr, vec, size), "{encoding:#010x}");
    }
}

#[test]
fn an_unallocated_conversion_opcode_faults() {
    // rmode = 01 with opcode = 110 is not the FMOV transfer, which requires
    // rmode = 00, and nothing else is allocated there.
    let bad = 0x1e26_039b | (0b01 << 19);
    assert!(decode(bad).op.is_unallocated());
}
