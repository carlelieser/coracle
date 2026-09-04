//! The decode model can express the instructions that shape it.
//!
//! Each case here is an A64 encoding whose operands do not fit an obvious
//! model, and which therefore fixes a decision the four phase B slices build
//! against. They assert representability, not decoding: the group decoders that
//! produce these forms land in phase B.

use coracle_core::decode::address::{AccessSize, AddrMode, Ordering};
use coracle_core::decode::instruction::Form;
use coracle_core::decode::operand::{Cond, ElemSize, RoundMode, VecHalf, VecOperand, VecShape};
use coracle_core::decode::{Instruction, Op};
use coracle_core::reg::{Gpr, Vec};

#[test]
fn move_wide_keeps_the_halfword_and_its_position_apart() {
    // MOVK X0, #0, LSL #16 and MOVK X0, #0, LSL #48 both merge a zero
    // halfword; a pre-shifted immediate would make them identical.
    let low = Form::MoveWide {
        rd: Gpr::X(0),
        imm16: 0,
        hw: 1,
    };
    let high = Form::MoveWide {
        rd: Gpr::X(0),
        imm16: 0,
        hw: 3,
    };

    assert_ne!(low, high);
}

#[test]
fn an_exclusive_store_names_a_status_register_distinct_from_the_zero_one() {
    // STXR W2, X0, [X1] writes its status to W2. STP XZR, XZR, [SP] makes
    // ZR a legal transferred register, so ZR cannot double as "no status".
    let exclusive = Form::LoadStore {
        rt: Gpr::X(0),
        rt2: None,
        rs: Some(Gpr::X(2)),
        addr: AddrMode::BaseOnly { base: Gpr::X(1) },
        size: AccessSize::X,
        ordering: Ordering {
            is_exclusive: true,
            ..Ordering::PLAIN
        },
    };
    let pair_of_zero = Form::LoadStore {
        rt: Gpr::ZR,
        rt2: Some(Gpr::ZR),
        rs: None,
        addr: AddrMode::BaseOnly { base: Gpr::SP },
        size: AccessSize::X,
        ordering: Ordering::PLAIN,
    };

    assert_ne!(exclusive, pair_of_zero);
}

#[test]
fn a_prefetch_keeps_its_operation_field_out_of_the_register_file() {
    // PRFM #31, [X0] would decode to Gpr::ZR if prfop were read as a
    // register, losing the operation.
    let prefetch = Form::Prefetch {
        prfop: 31,
        addr: AddrMode::BaseOnly { base: Gpr::X(0) },
    };

    assert!(matches!(prefetch, Form::Prefetch { prfop: 31, .. }));
}

#[test]
fn a_four_register_structure_load_is_a_base_and_a_count() {
    // LD4 {v30.16b, v31.16b, v0.16b, v1.16b}, [x0] wraps modulo 32, which
    // a base-plus-count representation expresses and a set would not.
    let load = Form::LoadStoreVec {
        vt: VecOperand {
            reg: Vec::new(30),
            shape: VecShape::Vector {
                elem: ElemSize::B8,
                count: 16,
                half: VecHalf::Full,
            },
        },
        count: 4,
        addr: AddrMode::BaseOnly { base: Gpr::X(0) },
        ordering: Ordering::PLAIN,
    };

    let Form::LoadStoreVec { vt, count, .. } = load else {
        panic!("expected a vector load");
    };
    assert_eq!(vt.reg.index(), 30);
    assert_eq!(count, 4);
}

#[test]
fn the_two_suffix_mnemonics_select_the_high_half_of_their_operand() {
    // UADDL reads lanes 0-3 of its sources; UADDL2 reads lanes 4-7 of the
    // same register at the same element width.
    let low = VecOperand {
        reg: Vec::new(1),
        shape: VecShape::Vector {
            elem: ElemSize::H16,
            count: 4,
            half: VecHalf::Low,
        },
    };
    let high = VecOperand {
        reg: Vec::new(1),
        shape: VecShape::Vector {
            elem: ElemSize::H16,
            count: 4,
            half: VecHalf::High,
        },
    };

    assert_ne!(low, high);
}

#[test]
fn a_four_register_table_lookup_fits_without_four_source_operands() {
    // TBL v0.16b, {v1.16b..v4.16b}, v5.16b needs five source registers,
    // which no three-source form could hold.
    let lookup = Form::TableLookup {
        vd: byte_vector(0),
        table: Vec::new(1),
        table_len: 4,
        vm: byte_vector(5),
    };

    assert!(matches!(lookup, Form::TableLookup { table_len: 4, .. }));
}

#[test]
fn an_fp_conditional_compare_keeps_the_flags_it_substitutes() {
    // FCCMP S0, S1, #4, EQ writes NZCV = 0100 when the condition fails.
    let compare = Form::VecCondCompare {
        vn: byte_vector(0),
        vm: byte_vector(1),
        nzcv: 0b0100,
        cond: Cond::Eq,
    };

    assert!(matches!(compare, Form::VecCondCompare { nzcv: 0b0100, .. }));
}

#[test]
fn comparing_against_zero_needs_no_second_register() {
    // FCMP S0, #0.0 has one register operand and writes only NZCV.
    let against_zero = Form::VecCompare {
        vn: byte_vector(0),
        vm: None,
    };

    assert!(matches!(against_zero, Form::VecCompare { vm: None, .. }));
}

#[test]
fn a_rounding_mode_named_by_the_encoding_survives_decode() {
    // FCVTMS rounds toward minus infinity regardless of FPCR.
    let insn = Instruction::new(0, Op::Fcvts, Form::None).with_round(RoundMode::Minus);

    assert_eq!(insn.round, RoundMode::Minus);
    assert_eq!(
        Instruction::new(0, Op::Fadd, Form::None).round,
        RoundMode::Current,
        "FPCR decides unless the encoding says otherwise"
    );
}

/// A full-width byte vector operand, the shape most of these cases only
/// need a placeholder for.
fn byte_vector(index: u8) -> VecOperand {
    VecOperand {
        reg: Vec::new(index),
        shape: VecShape::Vector {
            elem: ElemSize::B8,
            count: 16,
            half: VecHalf::Full,
        },
    }
}
