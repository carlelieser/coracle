//! Data processing (register) decoding.
//!
//! Every encoding here was produced by an assembler, not computed by hand.

use coracle_core::decode::instruction::Form;
use coracle_core::decode::operand::{
    Cond, ExtendKind, ExtendedReg, RegWidth, ShiftKind, ShiftedReg,
};
use coracle_core::decode::{decode, Op};
use coracle_core::reg::Gpr;

fn shifted(reg: Gpr, kind: ShiftKind, amount: u8) -> ShiftedReg {
    ShiftedReg { reg, kind, amount }
}

/// The identity shift the encodings without a shift field imply.
fn plain(reg: Gpr) -> ShiftedReg {
    shifted(reg, ShiftKind::Lsl, 0)
}

#[test]
fn shifted_register_arithmetic_carries_its_shift_kind_and_amount() {
    // add x0, x1, x2, lsl #4
    let add = decode(0x8b02_1020);
    assert_eq!(add.op, Op::Add);
    assert!(!add.sets_flags);
    assert_eq!(
        add.form,
        Form::RegShifted {
            rd: Gpr::X(0),
            rn: Gpr::X(1),
            rm: shifted(Gpr::X(2), ShiftKind::Lsl, 4),
        }
    );

    // adds w3, w4, w5, lsr #7
    let adds = decode(0x2b45_1c83);
    assert_eq!(adds.op, Op::Add);
    assert!(adds.sets_flags);
    assert_eq!(adds.width, RegWidth::W32);
    assert_eq!(
        adds.form,
        Form::RegShifted {
            rd: Gpr::X(3),
            rn: Gpr::X(4),
            rm: shifted(Gpr::X(5), ShiftKind::Lsr, 7),
        }
    );

    // sub x6, x7, x8, asr #20
    let sub = decode(0xcb88_50e6);
    assert_eq!(sub.op, Op::Sub);
    assert_eq!(
        sub.form,
        Form::RegShifted {
            rd: Gpr::X(6),
            rn: Gpr::X(7),
            rm: shifted(Gpr::X(8), ShiftKind::Asr, 20),
        }
    );
}

#[test]
fn the_cmp_and_neg_aliases_decode_to_the_opcode_they_alias() {
    // cmp x0, x1 — `subs xzr, x0, x1`.
    let cmp = decode(0xeb01_001f);
    assert_eq!(cmp.op, Op::Sub);
    assert!(cmp.sets_flags);
    assert_eq!(
        cmp.form,
        Form::RegShifted {
            rd: Gpr::ZR,
            rn: Gpr::X(0),
            rm: plain(Gpr::X(1)),
        }
    );

    // neg x2, x3 — `sub x2, xzr, x3`.
    let neg = decode(0xcb03_03e2);
    assert_eq!(neg.op, Op::Sub);
    assert!(!neg.sets_flags);
    assert_eq!(
        neg.form,
        Form::RegShifted {
            rd: Gpr::X(2),
            rn: Gpr::ZR,
            rm: plain(Gpr::X(3)),
        }
    );
}

#[test]
fn arithmetic_shifted_register_rejects_the_rotate_the_logical_group_allows() {
    // shift = 11 is ROR, which add/sub cannot name.
    assert!(decode(0x8bc2_0020).op.is_unallocated(), "add with ROR");
    // bic w4, w5, w6, ror #3 — the same field value in the logical group.
    let bic = decode(0x0ae6_0ca4);
    assert_eq!(bic.op, Op::Bic);
    assert_eq!(
        bic.form,
        Form::RegShifted {
            rd: Gpr::X(4),
            rn: Gpr::X(5),
            rm: shifted(Gpr::X(6), ShiftKind::Ror, 3),
        }
    );
}

#[test]
fn every_logical_shifted_register_opcode_decodes_with_its_negate_bit() {
    let cases = [
        (0x8a02_0020u32, Op::And, false),
        (0xea02_0020, Op::And, true),
        (0xaa02_0020, Op::Orr, false),
        (0xca0f_01cd, Op::Eor, false),
        (0xaa29_0107, Op::Orn, false),
        (0xca2c_056a, Op::Eon, false),
    ];

    for (encoding, op, sets_flags) in cases {
        let insn = decode(encoding);
        assert_eq!(insn.op, op, "{encoding:#010x}");
        assert_eq!(insn.sets_flags, sets_flags, "{encoding:#010x}");
    }
}

#[test]
fn the_mov_mvn_and_tst_aliases_decode_to_the_opcode_they_alias() {
    // mov x0, x1 — `orr x0, xzr, x1`.
    let mov = decode(0xaa01_03e0);
    assert_eq!(mov.op, Op::Orr);
    assert_eq!(
        mov.form,
        Form::RegShifted {
            rd: Gpr::X(0),
            rn: Gpr::ZR,
            rm: plain(Gpr::X(1)),
        }
    );

    // mvn x0, x1 — `orn x0, xzr, x1`.
    assert_eq!(decode(0xaa21_03e0).op, Op::Orn);

    // tst x2, x3 — `ands xzr, x2, x3`.
    let tst = decode(0xea03_005f);
    assert_eq!(tst.op, Op::And);
    assert!(tst.sets_flags);
    let Form::RegShifted { rd, .. } = tst.form else {
        panic!("expected RegShifted, got {:?}", tst.form);
    };
    assert_eq!(rd, Gpr::ZR);
}

#[test]
fn a_32_bit_shift_amount_cannot_index_bit_32_or_above() {
    // amount = 32 in a W-register form: no bit of the operand is there.
    assert!(decode(0x0b02_8020).op.is_unallocated(), "add w, amount 32");
    assert!(decode(0x0a02_8020).op.is_unallocated(), "and w, amount 32");
    // The same amount is fine at 64 bits.
    assert_eq!(decode(0x8b02_8020).op, Op::Add);
}

#[test]
fn extended_register_arithmetic_names_sp_where_the_shifted_form_cannot() {
    // add sp, x0, x1, lsl #3 — assembled as the extended form with UXTX,
    // because only this form can write SP.
    let add = decode(0x8b21_6c1f);
    assert_eq!(add.op, Op::Add);
    assert_eq!(
        add.form,
        Form::RegExtended {
            rd: Gpr::SP,
            rn: Gpr::X(0),
            rm: ExtendedReg {
                reg: Gpr::X(1),
                kind: ExtendKind::Uxtx,
                amount: 3,
            },
        }
    );

    // cmp sp, x0 — `subs xzr, sp, x0, uxtx #0`. Rd is the zero register here
    // even though Rn is SP.
    let cmp = decode(0xeb20_63ff);
    assert_eq!(cmp.op, Op::Sub);
    assert!(cmp.sets_flags);
    assert_eq!(
        cmp.form,
        Form::RegExtended {
            rd: Gpr::ZR,
            rn: Gpr::SP,
            rm: ExtendedReg {
                reg: Gpr::X(0),
                kind: ExtendKind::Uxtx,
                amount: 0,
            },
        }
    );
}

#[test]
fn the_extension_option_and_its_shift_are_decoded_separately() {
    // add x0, x1, w2, uxtb
    let uxtb = decode(0x8b22_0020);
    assert_eq!(
        uxtb.form,
        Form::RegExtended {
            rd: Gpr::X(0),
            rn: Gpr::X(1),
            rm: ExtendedReg {
                reg: Gpr::X(2),
                kind: ExtendKind::Uxtb,
                amount: 0,
            },
        }
    );

    // sub x0, sp, w1, sxth #2
    let sxth = decode(0xcb21_abe0);
    assert_eq!(sxth.op, Op::Sub);
    assert_eq!(
        sxth.form,
        Form::RegExtended {
            rd: Gpr::X(0),
            rn: Gpr::SP,
            rm: ExtendedReg {
                reg: Gpr::X(1),
                kind: ExtendKind::Sxth,
                amount: 2,
            },
        }
    );
}

#[test]
fn an_extended_register_shift_above_four_is_unallocated() {
    // The post-extension left shift is architecturally 0-4. add x0, x1, x2,
    // uxtx #5 has no encoding, but the field can still hold 5.
    assert!(decode(0x8b22_7420).op.is_unallocated(), "amount 5");
    assert!(decode(0x8b22_7c20).op.is_unallocated(), "amount 7");
    // `opt`, bits 23..22, is fixed at zero.
    assert!(decode(0x8b62_0020).op.is_unallocated(), "opt != 0");
}

#[test]
fn add_sub_with_carry_has_no_shift_field_to_read() {
    let cases = [
        (0x9a02_0020u32, Op::Adc, false, RegWidth::X64),
        (0x3a05_0083, Op::Adc, true, RegWidth::W32),
        (0xda08_00e6, Op::Sbc, false, RegWidth::X64),
        (0xfa0b_0149, Op::Sbc, true, RegWidth::X64),
    ];

    for (encoding, op, sets_flags, width) in cases {
        let insn = decode(encoding);
        assert_eq!(insn.op, op, "{encoding:#010x}");
        assert_eq!(insn.sets_flags, sets_flags, "{encoding:#010x}");
        assert_eq!(insn.width, width, "{encoding:#010x}");
        let Form::RegShifted { rm, .. } = insn.form else {
            panic!("expected RegShifted, got {:?}", insn.form);
        };
        assert_eq!(
            (rm.kind, rm.amount),
            (ShiftKind::Lsl, 0),
            "{encoding:#010x}"
        );
    }

    // ngc x0, x1 — `sbc x0, xzr, x1`.
    let ngc = decode(0xda01_03e0);
    assert_eq!(ngc.op, Op::Sbc);
    assert_eq!(
        ngc.form,
        Form::RegShifted {
            rd: Gpr::X(0),
            rn: Gpr::ZR,
            rm: plain(Gpr::X(1)),
        }
    );

    // The field between Rm and Rn is fixed at zero.
    assert!(decode(0x9a02_0420).op.is_unallocated());
}

#[test]
fn conditional_select_decodes_its_four_opcodes_and_condition() {
    let cases = [
        // csel x1, x2, x3, ne
        (
            0x9a83_1041u32,
            Op::Csel,
            Gpr::X(1),
            Gpr::X(2),
            Gpr::X(3),
            Cond::Ne,
        ),
        // csinc x4, x5, x6, eq
        (
            0x9a86_04a4,
            Op::Csinc,
            Gpr::X(4),
            Gpr::X(5),
            Gpr::X(6),
            Cond::Eq,
        ),
        // csinv w7, w8, w9, lt
        (
            0x5a89_b107,
            Op::Csinv,
            Gpr::X(7),
            Gpr::X(8),
            Gpr::X(9),
            Cond::Lt,
        ),
        // csneg x10, x11, x12, gt
        (
            0xda8c_c56a,
            Op::Csneg,
            Gpr::X(10),
            Gpr::X(11),
            Gpr::X(12),
            Cond::Gt,
        ),
    ];

    for (encoding, op, rd, rn, rm, cond) in cases {
        let insn = decode(encoding);
        assert_eq!(insn.op, op, "{encoding:#010x}");
        assert!(!insn.sets_flags, "{encoding:#010x}");
        assert_eq!(
            insn.form,
            Form::CondSelect { rd, rn, rm, cond },
            "{encoding:#010x}"
        );
    }
}

#[test]
fn the_cset_and_cinc_aliases_decode_to_the_conditional_select_they_alias() {
    // cset x0, eq — `csinc x0, xzr, xzr, ne`. The alias inverts the condition
    // and names the zero register twice; neither is a new opcode.
    let cset = decode(0x9a9f_17e0);
    assert_eq!(cset.op, Op::Csinc);
    assert_eq!(
        cset.form,
        Form::CondSelect {
            rd: Gpr::X(0),
            rn: Gpr::ZR,
            rm: Gpr::ZR,
            cond: Cond::Ne,
        }
    );

    // csetm w1, ne — `csinv w1, wzr, wzr, eq`.
    let csetm = decode(0x5a9f_03e1);
    assert_eq!(csetm.op, Op::Csinv);
    assert_eq!(csetm.width, RegWidth::W32);

    // cinc x2, x3, mi — `csinc x2, x3, x3, pl`.
    let cinc = decode(0x9a83_5462);
    assert_eq!(cinc.op, Op::Csinc);
    assert_eq!(
        cinc.form,
        Form::CondSelect {
            rd: Gpr::X(2),
            rn: Gpr::X(3),
            rm: Gpr::X(3),
            cond: Cond::Pl,
        }
    );
}

#[test]
fn a_conditional_select_that_sets_flags_or_names_no_opcode_is_unallocated() {
    // S (bit 29) is fixed at zero across the whole group.
    assert!(decode(0xba83_1041).op.is_unallocated(), "S = 1");
    // op2<1> (bit 11) is fixed at zero.
    assert!(decode(0x9a83_1841).op.is_unallocated(), "op2 = 1x");
}

#[test]
fn the_conditional_compare_immediate_form_keeps_its_immediate_and_nzcv() {
    // ccmn w2, #5, #3, ne
    let ccmn = decode(0x3a45_1843);
    assert_eq!(ccmn.op, Op::Ccmn);
    assert!(ccmn.sets_flags);
    assert_eq!(ccmn.width, RegWidth::W32);
    assert_eq!(
        ccmn.form,
        Form::CondCompare {
            rn: Gpr::X(2),
            imm: 5,
            nzcv: 3,
            cond: Cond::Ne,
        }
    );

    // ccmp x4, #31, #0, al — the widest immediate the 5-bit field holds.
    let ccmp = decode(0xfa5f_e880);
    assert_eq!(ccmp.op, Op::Ccmp);
    assert_eq!(
        ccmp.form,
        Form::CondCompare {
            rn: Gpr::X(4),
            imm: 31,
            nzcv: 0,
            cond: Cond::Al,
        }
    );
}

#[test]
fn the_conditional_compare_register_form_is_rewritten_into_cond_select() {
    // ccmp x0, x1, #15, eq. The register form has the same four operands as a
    // conditional select and no destination, so it reuses that shape with the
    // zero register standing in for Rd.
    let insn = decode(0xfa41_000f);

    assert_eq!(insn.op, Op::Ccmp);
    assert!(insn.sets_flags);
    assert_eq!(
        insn.form,
        Form::CondSelect {
            rd: Gpr::ZR,
            rn: Gpr::X(0),
            rm: Gpr::X(1),
            cond: Cond::Eq,
        }
    );
}

#[test]
fn a_conditional_compare_with_a_reserved_bit_set_is_unallocated() {
    // o2 (bit 10) and o3 (bit 4) are fixed at zero; S (bit 29) at one.
    assert!(decode(0xfa41_040f).op.is_unallocated(), "o2 = 1");
    assert!(decode(0xfa41_001f).op.is_unallocated(), "o3 = 1");
    assert!(decode(0xda41_000f).op.is_unallocated(), "S = 0");
}

#[test]
fn the_three_source_group_decodes_its_widening_and_high_half_forms() {
    let cases = [
        (0x9b02_0c20u32, Op::Madd, RegWidth::X64),
        (0x1b06_9ca4, Op::Msub, RegWidth::W32),
        (0x9b22_0c20, Op::Smaddl, RegWidth::X64),
        (0x9b26_9ca4, Op::Smsubl, RegWidth::X64),
        (0x9baa_2d28, Op::Umaddl, RegWidth::X64),
        (0x9bae_bdac, Op::Umsubl, RegWidth::X64),
        (0x9b42_7c20, Op::Smulh, RegWidth::X64),
        (0x9bc5_7c83, Op::Umulh, RegWidth::X64),
    ];

    for (encoding, op, width) in cases {
        let insn = decode(encoding);
        assert_eq!(insn.op, op, "{encoding:#010x}");
        assert_eq!(insn.width, width, "{encoding:#010x}");
        assert!(!insn.sets_flags, "{encoding:#010x}");
        assert!(
            matches!(insn.form, Form::ThreeSource { .. }),
            "{encoding:#010x} got {:?}",
            insn.form
        );
    }

    // madd x0, x1, x2, x3 — the operand order the form documents.
    assert_eq!(
        decode(0x9b02_0c20).form,
        Form::ThreeSource {
            rd: Gpr::X(0),
            rn: Gpr::X(1),
            rm: Gpr::X(2),
            ra: Gpr::X(3),
        }
    );
}

#[test]
fn the_mul_and_smull_aliases_name_the_zero_register_as_their_addend() {
    // mul x8, x9, x10 — `madd x8, x9, x10, xzr`.
    let mul = decode(0x9b0a_7d28);
    assert_eq!(mul.op, Op::Madd);
    assert_eq!(
        mul.form,
        Form::ThreeSource {
            rd: Gpr::X(8),
            rn: Gpr::X(9),
            rm: Gpr::X(10),
            ra: Gpr::ZR,
        }
    );

    // mneg x11, x12, x13 — `msub ..., xzr`.
    assert_eq!(decode(0x9b0d_fd8b).op, Op::Msub);
    // smull x6, w7, w8 — `smaddl x6, w7, w8, xzr`.
    assert_eq!(decode(0x9b28_7ce6).op, Op::Smaddl);
}

#[test]
fn the_widening_three_source_forms_have_no_32_bit_encoding() {
    // SMADDL reads two W registers and writes an X, so sf = 0 is unallocated
    // rather than a narrow variant of it.
    assert!(decode(0x1b22_0c20).op.is_unallocated(), "smaddl with sf=0");
    assert!(decode(0x1b42_7c20).op.is_unallocated(), "smulh with sf=0");
    assert!(decode(0x1baa_2d28).op.is_unallocated(), "umaddl with sf=0");
    // op54, bits 30..29, is zero throughout the group.
    assert!(decode(0xdb02_0c20).op.is_unallocated(), "op54 != 0");
    // op31 = 011 and 111 name nothing.
    assert!(decode(0x9b62_0c20).op.is_unallocated(), "op31 = 011");
    assert!(decode(0x9be2_0c20).op.is_unallocated(), "op31 = 111");
}

#[test]
fn the_high_half_multiplies_still_decode_when_their_ra_field_is_not_all_ones() {
    // SMULH and UMULH have no addend, but their Ra field is "should be one"
    // rather than reserved. A non-31 value is constrained-unpredictable, so it
    // must decode rather than fault — the oracle decodes it too.
    assert_eq!(decode(0x9b42_0c20).op, Op::Smulh);
    assert_eq!(decode(0x9bc5_0c83).op, Op::Umulh);
}

#[test]
fn the_variable_shift_aliases_decode_to_their_v_suffixed_opcodes() {
    let cases = [
        (0x9ac2_2020u32, Op::Lslv, RegWidth::X64), // lsl x0, x1, x2
        (0x1ac5_2483, Op::Lsrv, RegWidth::W32),    // lsr w3, w4, w5
        (0x9ac8_28e6, Op::Asrv, RegWidth::X64),    // asr x6, x7, x8
        (0x9acb_2d49, Op::Rorv, RegWidth::X64),    // ror x9, x10, x11
    ];

    for (encoding, op, width) in cases {
        let insn = decode(encoding);
        assert_eq!(insn.op, op, "{encoding:#010x}");
        assert_eq!(insn.width, width, "{encoding:#010x}");
    }

    // The shift amount is the register's value, so the form carries the
    // identity shift rather than a constant read out of the encoding.
    assert_eq!(
        decode(0x9ac2_2020).form,
        Form::RegShifted {
            rd: Gpr::X(0),
            rn: Gpr::X(1),
            rm: plain(Gpr::X(2)),
        }
    );
}

#[test]
fn the_divides_decode_at_both_widths() {
    // udiv x0, x1, x2
    let udiv = decode(0x9ac2_0820);
    assert_eq!(udiv.op, Op::Udiv);
    assert_eq!(udiv.width, RegWidth::X64);

    // sdiv w3, w4, w5
    let sdiv = decode(0x1ac5_0c83);
    assert_eq!(sdiv.op, Op::Sdiv);
    assert_eq!(sdiv.width, RegWidth::W32);
    assert_eq!(
        sdiv.form,
        Form::RegShifted {
            rd: Gpr::X(3),
            rn: Gpr::X(4),
            rm: plain(Gpr::X(5)),
        }
    );
}

#[test]
fn crc32_and_the_other_unadvertised_two_source_opcodes_are_unallocated() {
    // CRC32 is FEAT_CRC32, which docs/machine-spec.md §2 does not advertise.
    // crc32b w0, w1, w2 is opcode 010000.
    assert!(decode(0x1ac2_4020).op.is_unallocated(), "crc32b");
    assert!(decode(0x1ac2_4420).op.is_unallocated(), "crc32h");
    // S (bit 29) is fixed at zero across the two- and one-source groups.
    assert!(decode(0x9ae2_2020).op.is_unallocated(), "S = 1");
}

#[test]
fn the_one_source_group_reverses_and_counts_at_the_right_width() {
    let cases = [
        (0xdac0_0020u32, Op::Rbit, RegWidth::X64),
        (0x5ac0_0462, Op::Rev16, RegWidth::W32),
        (0xdac0_08a4, Op::Rev32, RegWidth::X64),
        (0xdac0_0ce6, Op::Rev, RegWidth::X64),
        (0xdac0_116a, Op::Clz, RegWidth::X64),
        (0x5ac0_15ac, Op::Cls, RegWidth::W32),
    ];

    for (encoding, op, width) in cases {
        let insn = decode(encoding);
        assert_eq!(insn.op, op, "{encoding:#010x}");
        assert_eq!(insn.width, width, "{encoding:#010x}");
    }

    // rbit x0, x1 — one source, so the form repeats it rather than reading a
    // second register out of a field the encoding does not have.
    assert_eq!(
        decode(0xdac0_0020).form,
        Form::RegShifted {
            rd: Gpr::X(0),
            rn: Gpr::X(1),
            rm: plain(Gpr::X(1)),
        }
    );
}

#[test]
fn a_32_bit_whole_register_reverse_shares_an_opcode_with_rev32() {
    // Opcode 000010 is REV32 at 64 bits and REV at 32: reversing a W
    // register's four bytes is the same operation either way.
    assert_eq!(decode(0xdac0_08a4).op, Op::Rev32, "rev32 x4, x5");
    assert_eq!(decode(0x5ac0_0928).op, Op::Rev, "rev w8, w9");
    // Opcode 000011 is the 64-bit-only whole-register reverse.
    assert_eq!(decode(0xdac0_0ce6).op, Op::Rev, "rev x6, x7");
    assert!(decode(0x5ac0_0ce6).op.is_unallocated(), "rev w with opc 3");
}

#[test]
fn a_one_source_encoding_with_a_nonzero_opcode2_is_unallocated() {
    // opcode2, bits 20..16, selects pointer authentication outside zero, and
    // PAuth is not advertised (docs/machine-spec.md §2).
    assert!(decode(0xdac1_0020).op.is_unallocated(), "opcode2 = 1");
    // Opcodes above 000101 name nothing in the base architecture.
    assert!(decode(0xdac0_1820).op.is_unallocated(), "opcode 000110");
}

#[test]
fn the_unallocated_rows_of_the_register_table_stay_unallocated() {
    // op2 = 0001 and 0011 are FEAT_FlagM (RMIF, SETF), 0101 names nothing.
    assert!(decode(0xba00_0400).op.is_unallocated(), "op2 = 0001");
    assert!(decode(0x9a60_0000).op.is_unallocated(), "op2 = 0011");
    assert!(decode(0xbaa0_0000).op.is_unallocated(), "op2 = 0101");
}
