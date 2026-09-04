//! Data processing (immediate) decoding.
//!
//! Every encoding here was produced by an assembler, not computed by hand, so
//! a misread bit field shows up as a failing test rather than as a plausible
//! wrong register.

use coracle_core::decode::instruction::Form;
use coracle_core::decode::operand::RegWidth;
use coracle_core::decode::{decode, Op};
use coracle_core::reg::Gpr;

#[test]
fn adr_forms_a_byte_offset_and_adrp_a_page_offset() {
    // adr x0, .+8
    let adr = decode(0x1000_0040);
    assert_eq!(adr.op, Op::Adr);
    assert_eq!(
        adr.form,
        Form::PcRelAddr {
            rd: Gpr::X(0),
            offset: 8,
        }
    );

    // adrp x9, .-0x4000
    let adrp = decode(0x90ff_ffe9);
    assert_eq!(adrp.op, Op::Adrp);
    assert_eq!(
        adrp.form,
        Form::PcRelAddr {
            rd: Gpr::X(9),
            offset: -0x4000,
        }
    );
}

#[test]
fn a_pc_relative_offset_is_sign_extended_across_its_split_fields() {
    // The offset is immhi:immlo with immlo in bits 30..29, so a decoder that
    // reads only immhi still produces a plausible-looking small offset.
    // adr x5, .-4096
    let insn = decode(0x10ff_8005);
    assert_eq!(
        insn.form,
        Form::PcRelAddr {
            rd: Gpr::X(5),
            offset: -4096,
        }
    );
}

#[test]
fn pc_relative_addressing_always_writes_a_64_bit_register() {
    // Bit 31 selects ADRP, not the operand width; the result is an address.
    assert_eq!(decode(0x1000_0040).width, RegWidth::X64);
    assert_eq!(decode(0x9000_0000).width, RegWidth::X64);
}

#[test]
fn add_immediate_resolves_slot_31_as_sp_and_keeps_its_immediate() {
    // add sp, sp, #0x10
    let insn = decode(0x9100_43ff);

    assert_eq!(insn.op, Op::Add);
    assert!(!insn.sets_flags);
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
fn subtract_immediate_carries_its_shift_and_narrow_width() {
    // sub w1, w2, #3, lsl #12
    let insn = decode(0x5140_0c41);

    assert_eq!(insn.op, Op::Sub);
    assert!(!insn.sets_flags);
    assert_eq!(insn.width, RegWidth::W32);
    assert_eq!(
        insn.form,
        Form::RegImm {
            rd: Gpr::X(1),
            rn: Gpr::X(2),
            imm: 0x3000,
        }
    );

    // subs x3, x4, #0xfff
    let subs = decode(0xf13f_fc83);
    assert_eq!(subs.op, Op::Sub);
    assert!(subs.sets_flags);
    assert_eq!(
        subs.form,
        Form::RegImm {
            rd: Gpr::X(3),
            rn: Gpr::X(4),
            imm: 0xfff,
        }
    );
}

#[test]
fn tagged_add_sub_immediate_is_unallocated_because_the_machine_has_no_mte() {
    // ADDG/SUBG occupy sh = 1x of the add/sub (immediate) space. The feature
    // mask in docs/machine-spec.md §2 excludes MTE, so these must fault.
    assert!(decode(0x9180_0000).op.is_unallocated());
    assert!(decode(0xd180_0000).op.is_unallocated());
}

#[test]
fn logical_immediates_are_expanded_from_their_bitmask_encoding() {
    let cases = [
        // (encoding, op, rd, rn, imm, width, sets_flags)
        (0x9240_1c20u32, Op::And, Gpr::X(0), Gpr::X(1), 0xffu64, true),
        (
            0xb200_f0a4,
            Op::Orr,
            Gpr::X(4),
            Gpr::X(5),
            0x5555_5555_5555_5555,
            true,
        ),
        (0x5200_00e6, Op::Eor, Gpr::X(6), Gpr::X(7), 1, false),
        (
            0xd260_7d8b,
            Op::Eor,
            Gpr::X(11),
            Gpr::X(12),
            0xffff_ffff_0000_0000,
            true,
        ),
        (0x927f_05cd, Op::And, Gpr::X(13), Gpr::X(14), 6, true),
    ];

    for (encoding, op, rd, rn, imm, is_64) in cases {
        let insn = decode(encoding);
        assert_eq!(insn.op, op, "{encoding:#010x}");
        assert_eq!(
            insn.width,
            if is_64 { RegWidth::X64 } else { RegWidth::W32 },
            "{encoding:#010x}"
        );
        assert_eq!(insn.form, Form::RegImm { rd, rn, imm }, "{encoding:#010x}");
    }
}

#[test]
fn a_32_bit_logical_immediate_replicates_its_element_only_within_32_bits() {
    // ands w2, w3, #0xf0f0f0f0 — the element is 8 bits wide and must not
    // spill into the upper half of the 64-bit value.
    let insn = decode(0x7204_cc62);

    assert_eq!(insn.op, Op::And);
    assert!(insn.sets_flags);
    assert_eq!(insn.width, RegWidth::W32);
    assert_eq!(
        insn.form,
        Form::RegImm {
            rd: Gpr::X(2),
            rn: Gpr::X(3),
            imm: 0xf0f0_f0f0,
        }
    );
}

#[test]
fn ands_writes_the_zero_register_for_slot_31_but_and_writes_sp() {
    // tst x8, #0x8000000000000000 — `ands xzr, x8, #...`.
    let tst = decode(0xf241_011f);
    assert!(tst.sets_flags);
    let Form::RegImm { rd, .. } = tst.form else {
        panic!("expected RegImm, got {:?}", tst.form);
    };
    assert_eq!(rd, Gpr::ZR);

    // and sp, x0, #0xfffffffffff000
    let and_sp = decode(0x9274_ac1f);
    assert!(!and_sp.sets_flags);
    assert_eq!(
        and_sp.form,
        Form::RegImm {
            rd: Gpr::SP,
            rn: Gpr::X(0),
            imm: 0x00ff_ffff_ffff_f000,
        }
    );
}

#[test]
fn a_logical_immediate_that_names_no_element_size_is_unallocated() {
    // N = 0, imms = 0b111111 leaves N:NOT(imms) all zero, which names no
    // element size; and N = 1 has no 64-bit element to replicate at sf = 0.
    assert!(
        decode(0x1200_fc00).op.is_unallocated(),
        "sf=0 imms=all ones"
    );
    assert!(decode(0x1240_0000).op.is_unallocated(), "sf=0 N=1");
    // An `imms` that is all ones within its element leaves S = level, which
    // the architecture reserves: N=0, imms=0b111101 names a 2-bit element
    // whose S is already 1.
    assert!(decode(0x9200_f400).op.is_unallocated(), "S == level");
}

#[test]
fn move_wide_keeps_the_halfword_and_its_shift_apart() {
    // movz x0, #0x1234, lsl #16
    let movz = decode(0xd2a2_4680);
    assert_eq!(movz.op, Op::Movz);
    assert_eq!(
        movz.form,
        Form::MoveWide {
            rd: Gpr::X(0),
            imm16: 0x1234,
            hw: 1,
        }
    );

    // movk x2, #0xbeef, lsl #48
    let movk = decode(0xf2f7_dde2);
    assert_eq!(movk.op, Op::Movk);
    assert!(movk.op.reads_destination());
    assert_eq!(
        movk.form,
        Form::MoveWide {
            rd: Gpr::X(2),
            imm16: 0xbeef,
            hw: 3,
        }
    );

    // movn w1, #0 — the `mov w1, #-1` alias.
    let movn = decode(0x1280_0001);
    assert_eq!(movn.op, Op::Movn);
    assert_eq!(movn.width, RegWidth::W32);
    assert_eq!(
        movn.form,
        Form::MoveWide {
            rd: Gpr::X(1),
            imm16: 0,
            hw: 0,
        }
    );
}

#[test]
fn a_32_bit_move_wide_cannot_name_the_upper_halfwords() {
    // hw = 2 shifts by 32, which does not exist in a W register.
    assert!(
        decode(0x12c0_0000).op.is_unallocated(),
        "movz w0, #0, lsl #32"
    );
    // opc = 01 is unallocated in the move-wide space at every width.
    assert!(decode(0x3280_0000).op.is_unallocated(), "opc = 01");
}

#[test]
fn bitfield_carries_immr_and_imms_unmodified() {
    // sbfx x0, x1, #4, #5 — sbfm x0, x1, #4, #8
    let sbfm = decode(0x9344_2020);
    assert_eq!(sbfm.op, Op::Sbfm);
    assert_eq!(sbfm.width, RegWidth::X64);
    assert_eq!(
        sbfm.form,
        Form::Bitfield {
            rd: Gpr::X(0),
            rn: Gpr::X(1),
            rm: Gpr::X(1),
            immr: 4,
            imms: 8,
        }
    );

    // bfxil w2, w3, #1, #2 — bfm w2, w3, #1, #2
    let bfm = decode(0x3301_0862);
    assert_eq!(bfm.op, Op::Bfm);
    assert!(bfm.op.reads_destination());
    assert_eq!(bfm.width, RegWidth::W32);
    assert_eq!(
        bfm.form,
        Form::Bitfield {
            rd: Gpr::X(2),
            rn: Gpr::X(3),
            rm: Gpr::X(3),
            immr: 1,
            imms: 2,
        }
    );
}

#[test]
fn the_shift_and_extend_aliases_decode_to_the_bitfield_opcode_they_alias() {
    // These are the aliases the plan names; none of them is its own opcode.
    let cases = [
        (0xd37f_f8a4u32, Op::Ubfm, 63u8, 62u8), // lsl x4, x5, #1
        (0x5300_1c20, Op::Ubfm, 0, 7),          // uxtb w0, w1
        (0x9340_7c62, Op::Sbfm, 0, 31),         // sxtw x2, w3
    ];

    for (encoding, op, immr, imms) in cases {
        let insn = decode(encoding);
        assert_eq!(insn.op, op, "{encoding:#010x}");
        let Form::Bitfield {
            immr: got_immr,
            imms: got_imms,
            ..
        } = insn.form
        else {
            panic!("expected Bitfield, got {:?}", insn.form);
        };
        assert_eq!((got_immr, got_imms), (immr, imms), "{encoding:#010x}");
    }
}

#[test]
fn a_bitfield_whose_n_bit_disagrees_with_sf_is_unallocated() {
    // sf = 1 needs N = 1 and sf = 0 needs N = 0; the field widths only make
    // sense at the operand width.
    assert!(decode(0x9300_0400).op.is_unallocated(), "sf=1 N=0");
    assert!(decode(0x1340_a400).op.is_unallocated(), "sf=0 N=1");
    // opc = 11 has no bitfield opcode.
    assert!(decode(0xf340_0400).op.is_unallocated(), "opc = 11");
    // A 32-bit form cannot index bit 32 or above.
    assert!(
        decode(0x1300_c400).op.is_unallocated(),
        "sf=0 imms bit 5 set"
    );
}

#[test]
fn extract_names_two_sources_and_puts_its_rotate_in_imms() {
    // extr x0, x1, x2, #5
    let extr = decode(0x93c2_1420);
    assert_eq!(extr.op, Op::Extr);
    assert_eq!(extr.width, RegWidth::X64);
    assert_eq!(
        extr.form,
        Form::Bitfield {
            rd: Gpr::X(0),
            rn: Gpr::X(1),
            rm: Gpr::X(2),
            immr: 0,
            imms: 5,
        }
    );
}

#[test]
fn the_ror_alias_of_extract_repeats_its_source_register() {
    // ror w4, w5, #9 — extr w4, w5, w5, #9
    let insn = decode(0x1385_24a4);

    assert_eq!(insn.op, Op::Extr);
    assert_eq!(insn.width, RegWidth::W32);
    assert_eq!(
        insn.form,
        Form::Bitfield {
            rd: Gpr::X(4),
            rn: Gpr::X(5),
            rm: Gpr::X(5),
            immr: 0,
            imms: 9,
        }
    );
}

#[test]
fn an_extract_with_a_reserved_field_combination_is_unallocated() {
    // op21 must be 00 and o0 must be 0.
    assert!(decode(0xb3c0_0400).op.is_unallocated(), "opc != 00");
    assert!(decode(0x93e0_0400).op.is_unallocated(), "bit 21 set");
    // N must equal sf, and a 32-bit rotate cannot reach bit 32.
    assert!(decode(0x13c0_0400).op.is_unallocated(), "sf=0 N=1");
    assert!(decode(0x9380_0400).op.is_unallocated(), "sf=1 N=0");
    assert!(decode(0x1382_8461).op.is_unallocated(), "sf=0 imms bit 5");
}
