//! Branch, exception and system instruction decoding.
//!
//! Every encoding here was produced by an assembler, not computed by hand.

use coracle_core::decode::instruction::Form;
use coracle_core::decode::operand::{Cond, RegWidth};
use coracle_core::decode::{decode, Op};
use coracle_core::reg::Gpr;

#[test]
fn branch_offsets_are_sign_extended_and_scaled_to_bytes() {
    // b .+8
    let forward = decode(0x1400_0002);
    assert_eq!(forward.op, Op::B);
    assert_eq!(forward.form, Form::Branch { offset: 8 });

    // bl .-4
    let backward = decode(0x97ff_ffff);
    assert_eq!(backward.op, Op::Bl);
    assert_eq!(backward.form, Form::Branch { offset: -4 });
}

#[test]
fn the_widest_immediate_branch_reaches_the_ends_of_its_26_bit_field() {
    // The offset is 26 bits scaled by 4, so it spans ±128 MiB. A decoder that
    // sign-extends at the wrong width still looks right for small offsets.
    assert_eq!(
        decode(0x15ff_ffff).form,
        Form::Branch {
            offset: 0x07ff_fffc
        },
        "largest forward"
    );
    assert_eq!(
        decode(0x1600_0000).form,
        Form::Branch {
            offset: -0x0800_0000
        },
        "largest backward"
    );
}

#[test]
fn conditional_branches_carry_their_condition_and_offset() {
    // b.eq .+8
    let eq = decode(0x5400_0040);
    assert_eq!(eq.op, Op::B);
    assert_eq!(
        eq.form,
        Form::BranchCond {
            offset: 8,
            cond: Cond::Eq,
        }
    );

    // b.ne .-4
    let ne = decode(0x54ff_ffe1);
    assert_eq!(
        ne.form,
        Form::BranchCond {
            offset: -4,
            cond: Cond::Ne,
        }
    );

    // b.al .-16
    let al = decode(0x54ff_ff8e);
    assert_eq!(
        al.form,
        Form::BranchCond {
            offset: -16,
            cond: Cond::Al,
        }
    );
}

#[test]
fn the_hinted_conditional_branch_is_unallocated_without_feat_hbc() {
    // BC.cond is bit 4 set — FEAT_HBC, outside the advertised mask.
    assert!(decode(0x5400_0050).op.is_unallocated(), "bc.eq");
    // o1, bit 24, is fixed at zero and names nothing when set.
    assert!(decode(0x5500_0040).op.is_unallocated(), "o1 = 1");
}

#[test]
fn compare_and_branch_reports_its_width_and_tests_no_bit() {
    // cbz x0, .-24
    let cbz = decode(0xb4ff_ffa0);
    assert_eq!(cbz.op, Op::Cbz);
    assert_eq!(cbz.width, RegWidth::X64);
    assert_eq!(
        cbz.form,
        Form::BranchReg {
            rt: Gpr::X(0),
            offset: -12,
            bit: 0,
        }
    );

    // cbnz w1, .-24
    let cbnz = decode(0x35ff_ff41);
    assert_eq!(cbnz.op, Op::Cbnz);
    assert_eq!(cbnz.width, RegWidth::W32);
    let Form::BranchReg { rt, bit, .. } = cbnz.form else {
        panic!("expected BranchReg, got {:?}", cbnz.form);
    };
    assert_eq!((rt, bit), (Gpr::X(1), 0));
}

#[test]
fn test_and_branch_splits_its_bit_position_across_two_fields() {
    // tbz w0, #5, .-20. b5 is bit 31 and b40 is bits 23..19, so a decoder
    // that reads only b40 gets every bit below 32 right and every bit above
    // it wrong.
    let low = decode(0x362f_ff60);
    assert_eq!(low.op, Op::Tbz);
    assert_eq!(low.width, RegWidth::W32);
    assert_eq!(
        low.form,
        Form::BranchReg {
            rt: Gpr::X(0),
            offset: -20,
            bit: 5,
        }
    );

    // tbnz x2, #40, .-32
    let high = decode(0xb747_ff02);
    assert_eq!(high.op, Op::Tbnz);
    assert_eq!(high.width, RegWidth::X64);
    let Form::BranchReg { rt, bit, .. } = high.form else {
        panic!("expected BranchReg, got {:?}", high.form);
    };
    assert_eq!((rt, bit), (Gpr::X(2), 40));

    // tbz x3, #63, .-28 — the highest bit the split field can name.
    let top = decode(0xb6ff_ff23);
    let Form::BranchReg { bit, .. } = top.form else {
        panic!("expected BranchReg, got {:?}", top.form);
    };
    assert_eq!(bit, 63);
}

#[test]
fn a_test_and_branch_offset_uses_its_own_narrower_field() {
    // The offset is 14 bits here, not the 19 of CBZ: the bit position takes
    // the rest. Reading 19 bits would fold b40 into the offset.
    let Form::BranchReg { offset, .. } = decode(0x362f_ff60).form else {
        panic!("expected BranchReg");
    };
    assert_eq!(offset, -20);
}

#[test]
fn indirect_branches_name_the_register_they_jump_through() {
    // br x0
    let br = decode(0xd61f_0000);
    assert_eq!(br.op, Op::Br);
    assert_eq!(br.form, Form::BranchIndirect { rn: Gpr::X(0) });

    // blr x1
    let blr = decode(0xd63f_0020);
    assert_eq!(blr.op, Op::Blr);
    assert_eq!(blr.form, Form::BranchIndirect { rn: Gpr::X(1) });

    // ret — implicitly through x30
    let ret = decode(0xd65f_03c0);
    assert_eq!(ret.op, Op::Ret);
    assert_eq!(ret.form, Form::BranchIndirect { rn: Gpr::X(30) });

    // ret x5 — the register is a real field, not always x30
    assert_eq!(
        decode(0xd65f_00a0).form,
        Form::BranchIndirect { rn: Gpr::X(5) }
    );
}

#[test]
fn eret_names_no_register() {
    let insn = decode(0xd69f_03e0);

    assert_eq!(insn.op, Op::Eret);
    assert_eq!(insn.form, Form::None);
}

#[test]
fn the_pointer_authenticating_branches_are_unallocated() {
    // BRAA, BLRAA, RETAA and friends set op3, which is FEAT_PAuth — outside
    // the advertised mask (docs/machine-spec.md §2).
    assert!(decode(0xd61f_0800).op.is_unallocated(), "braaz");
    assert!(decode(0xd65f_0bff).op.is_unallocated(), "retaa");
    // op2, bits 20..16, is fixed at all-ones.
    assert!(decode(0xd60f_0000).op.is_unallocated(), "op2 != 11111");
    // Rm must be zero for BR, BLR and RET.
    assert!(decode(0xd61f_0001).op.is_unallocated(), "br with Rm != 0");
}

#[test]
fn the_exception_group_keeps_its_16_bit_immediate() {
    // svc #0
    let svc = decode(0xd400_0001);
    assert_eq!(svc.op, Op::Svc);
    assert_eq!(svc.form, Form::Imm { imm: 0 });

    // svc #0x1234
    assert_eq!(decode(0xd402_4681).form, Form::Imm { imm: 0x1234 });

    // hvc #1 and smc #2
    assert_eq!(decode(0xd400_0022).op, Op::Hvc);
    assert_eq!(decode(0xd400_0043).op, Op::Smc);

    // brk #0xdead
    let brk = decode(0xd43b_d5a0);
    assert_eq!(brk.op, Op::Brk);
    assert_eq!(brk.form, Form::Imm { imm: 0xdead });

    // hlt #0
    assert_eq!(decode(0xd440_0000).op, Op::Hlt);
}

#[test]
fn an_exception_encoding_with_a_reserved_variant_field_is_unallocated() {
    // opc = 000 with LL = 00 names nothing; BRK is opc = 001, LL = 00.
    assert!(decode(0xd400_0000).op.is_unallocated(), "opc 000 ll 00");
    assert!(decode(0xd420_0001).op.is_unallocated(), "brk with ll = 01");
    // op2, bits 4..2, is fixed at zero.
    assert!(decode(0xd400_0005).op.is_unallocated(), "op2 != 0");
    // opc = 101 is the DCPS family, which exists only in debug state.
    assert!(decode(0xd4a0_0001).op.is_unallocated(), "dcps1");
}

#[test]
fn the_named_hints_decode_and_the_rest_of_the_hint_space_is_a_nop() {
    let cases = [
        (0xd503_201fu32, Op::Nop),
        (0xd503_203f, Op::Yield),
        (0xd503_205f, Op::Wfe),
        (0xd503_207f, Op::Wfi),
        (0xd503_209f, Op::Sev),
        (0xd503_20bf, Op::Sev),
    ];

    for (encoding, op) in cases {
        let insn = decode(encoding);
        assert_eq!(insn.op, op, "{encoding:#010x}");
        assert_eq!(insn.form, Form::None, "{encoding:#010x}");
    }

    // An unnamed point in the hint space is architecturally a NOP, not an
    // unallocated encoding: that is what lets a binary built against a later
    // revision run here.
    assert_eq!(decode(0xd503_22df).op, Op::Nop, "hint #22");
    assert_eq!(decode(0xd503_2fff).op, Op::Nop, "hint #127");
}

#[test]
fn the_barriers_decode_with_their_domain_field() {
    // dmb sy — CRm = 1111
    let dmb = decode(0xd503_3fbf);
    assert_eq!(dmb.op, Op::Dmb);
    assert_eq!(dmb.form, Form::Imm { imm: 0b1111 });

    // dmb ish — CRm = 1011
    assert_eq!(decode(0xd503_3bbf).form, Form::Imm { imm: 0b1011 });

    // dsb ishst — CRm = 1010
    let dsb = decode(0xd503_3a9f);
    assert_eq!(dsb.op, Op::Dsb);
    assert_eq!(dsb.form, Form::Imm { imm: 0b1010 });

    // isb sy
    assert_eq!(decode(0xd503_3fdf).op, Op::Isb);

    // clrex — musl's atomics emit it, so it must decode even before the
    // exclusive monitor exists.
    assert_eq!(decode(0xd503_3f5f).op, Op::Clrex);
    assert_eq!(decode(0xd503_355f).form, Form::Imm { imm: 5 }, "clrex #5");
}

#[test]
fn the_unadvertised_barrier_variants_are_unallocated() {
    // op2 = 111 is SB (FEAT_SB) and 011 is TCOMMIT (FEAT_TME).
    assert!(decode(0xd503_30ff).op.is_unallocated(), "sb");
    assert!(decode(0xd503_307f).op.is_unallocated(), "tcommit");
}

#[test]
fn system_register_moves_pack_their_five_encoding_fields() {
    // mrs x1, tpidr_el0 — op0=3 op1=3 CRn=13 CRm=0 op2=2
    let mrs = decode(0xd53b_d041);
    assert_eq!(mrs.op, Op::Mrs);
    let Form::System { rt, sysreg } = mrs.form else {
        panic!("expected System, got {:?}", mrs.form);
    };
    assert_eq!(rt, Gpr::X(1));
    // CRm is zero for this register, so it contributes nothing to the packing;
    // the other four fields are what must survive.
    assert_eq!(sysreg, (3 << 14) | (3 << 11) | (13 << 7) | 2);

    // msr tpidr_el0, x0 — the same register, written.
    let msr = decode(0xd51b_d040);
    assert_eq!(msr.op, Op::Msr);
    assert_eq!(
        msr.form,
        Form::System {
            rt: Gpr::X(0),
            sysreg,
        },
        "MSR and MRS of one register pack identically"
    );
}

#[test]
fn distinct_system_registers_pack_to_distinct_values() {
    // A packing that dropped a field would collide these, and the collision
    // would only surface in M2 as the wrong register being read.
    let packed = |encoding| {
        let Form::System { sysreg, .. } = decode(encoding).form else {
            panic!("expected System for {encoding:#010x}");
        };
        sysreg
    };

    // tpidr_el0, nzcv, midr_el1
    let ids = [
        packed(0xd53b_d041),
        packed(0xd51b_4202),
        packed(0xd538_0003),
    ];
    for (i, a) in ids.iter().enumerate() {
        for b in &ids[i + 1..] {
            assert_ne!(a, b);
        }
    }
}

#[test]
fn the_pstate_writing_msr_immediate_forms_decode_without_a_register() {
    // msr daifset, #3 — CRm carries the immediate, and Rt is 11111.
    let daifset = decode(0xd503_43df);
    assert_eq!(daifset.op, Op::Msr);
    let Form::System { rt, sysreg } = daifset.form else {
        panic!("expected System, got {:?}", daifset.form);
    };
    assert_eq!(rt, Gpr::ZR, "MSR (immediate) transfers no register");
    // op0 is zero for the immediate form, which is what separates it from a
    // system register write.
    assert_eq!(sysreg >> 14, 0);

    // msr daifclr, #15 and msr spsel, #1 — distinct fields, distinct packing.
    assert_ne!(sysreg, {
        let Form::System { sysreg, .. } = decode(0xd503_4fff).form else {
            panic!("expected System");
        };
        sysreg
    });
    assert_eq!(decode(0xd500_41bf).op, Op::Msr, "msr spsel, #1");
}

#[test]
fn the_cache_and_tlb_maintenance_instructions_are_left_for_m2() {
    // SYS/SYSL name a real register in Rt, so they are not hints. They are
    // EL1-only and land with the MMU in M2 rather than in this slice.
    // dc civac, x0
    assert!(decode(0xd50b_7e20).op.is_unallocated(), "dc civac");
    // tlbi vmalle1is
    assert!(decode(0xd508_831f).op.is_unallocated(), "tlbi");
}

#[test]
fn the_unallocated_rows_of_the_branch_table_stay_unallocated() {
    // op0 = 010 with op1 = 1xxx names nothing in the base architecture.
    assert!(decode(0x5600_0000).op.is_unallocated(), "op0=010 op1=1xxx");
    // op0 = 011 and 111 are the halves this group does not own.
    assert!(decode(0x7600_0000).op.is_unallocated(), "op0=011");
    // op0 = 110 with op1 = 0101 holds MRRS/MSRR, which is FEAT_D128 and not
    // advertised (docs/machine-spec.md §2).
    assert!(decode(0xd560_0000).op.is_unallocated(), "mrrs");
}

#[test]
fn the_system_registers_m1_and_m2_actually_touch_all_decode() {
    // Not an exhaustive list — the point is that the encodings a real guest
    // emits reach Form::System rather than falling into an unallocated hole.
    // TPIDR_EL0 is the M1 gate's TLS corpus; the CNT* and EL1 registers are
    // what M2's boot path reads first.
    let cases = [
        (0xd53b_0020u32, Op::Mrs, "ctr_el0"),
        (0xd53b_00e1, Op::Mrs, "dczid_el0"),
        (0xd53b_e042, Op::Mrs, "cntvct_el0"),
        (0xd53b_e003, Op::Mrs, "cntfrq_el0"),
        (0xd51b_d044, Op::Msr, "tpidr_el0"),
        (0xd53b_d065, Op::Mrs, "tpidrro_el0"),
        (0xd53b_4406, Op::Mrs, "fpcr"),
        (0xd51b_4427, Op::Msr, "fpsr"),
        (0xd538_0008, Op::Mrs, "midr_el1"),
        (0xd538_0609, Op::Mrs, "id_aa64isar0_el1"),
        (0xd538_410a, Op::Mrs, "sp_el0"),
        (0xd518_402b, Op::Msr, "elr_el1"),
        (0xd518_c00c, Op::Msr, "vbar_el1"),
    ];

    for (encoding, op, name) in cases {
        let insn = decode(encoding);
        assert_eq!(insn.op, op, "{name}");
        assert!(
            matches!(insn.form, Form::System { .. }),
            "{name}: got {:?}",
            insn.form
        );
    }
}
