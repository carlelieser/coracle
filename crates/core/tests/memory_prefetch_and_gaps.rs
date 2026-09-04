//! Prefetch hints, and the encodings the memory slice deliberately leaves for
//! someone else — or for nobody.
//!
//! The unallocated cases matter as much as the claimed ones: `docs/plan.md`
//! makes the M1 gate a differential fuzz corpus in which unallocated encodings
//! must fault exactly as the oracle faults them. An encoding decoded into the
//! wrong instruction is worse than one that traps.

use coracle_core::decode::address::{AddrMode, WriteBack};
use coracle_core::decode::instruction::Form;
use coracle_core::decode::{decode, Op};
use coracle_core::reg::Gpr;

/// The prefetch operation an encoding names, or a panic naming what came
/// instead.
fn prfop_of(encoding: u32) -> u8 {
    match decode(encoding).form {
        Form::Prefetch { prfop, .. } => prfop,
        other => panic!("expected a prefetch, got {other:?}"),
    }
}

#[test]
fn a_prefetch_keeps_its_operation_out_of_the_register_file() {
    // prfm pldl1keep, [x1, #8] — prfop 0.
    assert_eq!(decode(0xf980_0420).op, Op::Prfm);
    assert_eq!(prfop_of(0xf980_0420), 0);

    // prfm pstl2strm, [x1, #8] — prfop 19.
    assert_eq!(prfop_of(0xf980_0433), 19);
}

#[test]
fn prefetch_operation_31_is_a_hint_not_the_zero_register() {
    // prfm #31, [x1, #8]. Read as a register this would be Gpr::ZR and the
    // operation would be lost entirely.
    assert_eq!(prfop_of(0xf980_043f), 31);
}

#[test]
fn a_prefetch_scales_its_unsigned_offset_like_a_doubleword() {
    // prfm pldl1keep, [x1, #8] — imm12 = 1 at scale 3.
    assert_eq!(
        decode(0xf980_0420).form,
        Form::Prefetch {
            prfop: 0,
            addr: AddrMode::Immediate {
                base: Gpr::X(1),
                offset: 8,
                writeback: WriteBack::None,
            },
        }
    );
}

#[test]
fn the_unscaled_prefetch_signs_its_offset_and_does_not_scale_it() {
    // prfum #17, [x1, #-8]
    assert_eq!(
        decode(0xf89f_8031).form,
        Form::Prefetch {
            prfop: 17,
            addr: AddrMode::Immediate {
                base: Gpr::X(1),
                offset: -8,
                writeback: WriteBack::None,
            },
        }
    );
}

#[test]
fn a_prefetch_can_take_a_register_offset() {
    use coracle_core::decode::operand::{ExtendKind, ExtendedReg};

    // prfm #5, [x1, x2, lsl #3]
    assert_eq!(
        decode(0xf8a2_7825).form,
        Form::Prefetch {
            prfop: 5,
            addr: AddrMode::Register {
                base: Gpr::X(1),
                index: ExtendedReg {
                    reg: Gpr::X(2),
                    kind: ExtendKind::Uxtx,
                    amount: 3,
                },
                writeback: WriteBack::None,
            },
        }
    );
}

#[test]
fn a_literal_prefetch_is_pc_relative() {
    // prfm pldl1keep, back
    assert_eq!(
        decode(0xd8ff_ff60).form,
        Form::Prefetch {
            prfop: 0,
            addr: AddrMode::PcRelative { offset: -20 },
        }
    );
}

#[test]
fn the_lse_atomics_stay_unallocated() {
    // docs/machine-spec.md §2 advertises no LSE, so the whole atomic group
    // must fault. These sit inside the load/store space this slice owns and
    // would otherwise be easy to claim by accident: they share the
    // register-offset encoding's op2 and differ only in bits 11..10.
    let atomics = [
        (0xf820_0020u32, "ldadd x0, x0, [x1]"),
        (0xf820_1020, "ldclr x0, x0, [x1]"),
        (0xf820_2020, "ldeor x0, x0, [x1]"),
        (0xf820_3020, "ldset x0, x0, [x1]"),
        (0xf820_8020, "swp x0, x0, [x1]"),
        (0xc8a0_7c20, "cas x0, x0, [x1]"),
        (0x08a0_7c20, "casb w0, w0, [x1]"),
        (0x4820_7c22, "casp x0, x1, x2, x3, [x1]"),
    ];

    for (encoding, name) in atomics {
        assert!(
            decode(encoding).op.is_unallocated(),
            "{name} (0x{encoding:08x}) belongs to an unadvertised feature"
        );
    }
}

#[test]
fn the_simd_and_fp_transfers_are_left_to_the_fp_slice() {
    // V = 1 selects the SIMD/FP register file. Those encodings belong to the
    // FP/NEON slice; this slice must not answer for them.
    let vector_transfers = [
        (0x3dc0_0420u32, "ldr q0, [x1, #16]"),
        (0xad40_0440, "ldp q0, q1, [x2]"),
        (0xfd40_0420, "ldr d0, [x1, #8]"),
        (0xfc40_8420, "ldr d0, [x1], #8"),
        (0x4c40_7000, "ld1 {v0.16b}, [x0]"),
    ];

    for (encoding, name) in vector_transfers {
        let insn = decode(encoding);
        assert!(
            !matches!(insn.form, Form::LoadStore { .. } | Form::Prefetch { .. }),
            "{name} (0x{encoding:08x}) is not a general-purpose transfer"
        );
    }
}

#[test]
fn the_unallocated_holes_inside_the_claimed_groups_still_fault() {
    // Each of these sits next to an encoding this slice claims and differs
    // only in a field the architecture leaves unallocated.
    let holes = [
        // Unsigned-offset size = 11, opc = 11: no such load.
        (0xf9c0_0420u32, "size 11 with opc 11"),
        // LDUR-space opc = 11 at size = 11.
        (0xf8c0_0020, "unscaled size 11 with opc 11"),
        // Literal opc = 11 with V = 0 is PRFM; with V = 1 it is unallocated
        // rather than a wider vector literal. (opc = 10 with V = 1 is the
        // legal `ldr q0, literal`, which the FP/NEON slice claims.)
        (0xdc00_0000, "literal opc 11 with V = 1"),
        // Exclusive group with o2/o1/o0 combinations the architecture does
        // not allocate at size = 00.
        (0x0820_0020, "exclusive-space hole"),
    ];

    for (encoding, name) in holes {
        assert!(
            decode(encoding).op.is_unallocated(),
            "{name} (0x{encoding:08x}) has no allocated encoding"
        );
    }
}

#[test]
fn a_register_offset_requires_a_32_or_64_bit_index() {
    // The option field names the index's width, and only UXTW, LSL, SXTW and
    // SXTX are allocated: option<1> must be set. A byte or halfword index has
    // no encoding, so option = 00x and 10x are reserved rather than naming
    // ExtendKind::Uxtb and Uxtd.
    let base = 0xf862_6820u32; // ldr x0, [x1, x2] — option = 011.

    for option in [0b000u32, 0b001, 0b100, 0b101] {
        let encoding = (base & !(0b111 << 13)) | (option << 13);
        assert!(
            decode(encoding).op.is_unallocated(),
            "option = {option:03b} (0x{encoding:08x}) is reserved"
        );
    }

    for option in [0b010u32, 0b011, 0b110, 0b111] {
        let encoding = (base & !(0b111 << 13)) | (option << 13);
        assert_eq!(
            decode(encoding).op,
            Op::Ldr,
            "option = {option:03b} (0x{encoding:08x}) is allocated"
        );
    }
}

#[test]
fn a_literal_load_is_only_the_encoding_with_its_fixed_bit_clear() {
    // Bit 24 is 0 in every literal form. With it set the encoding is not a
    // wider literal, it is unallocated — and claiming it would decode a
    // quarter of the literal space into loads that should fault.
    for opc in 0..4u32 {
        let literal = 0x18ff_ffc0 | (opc << 30);
        assert!(
            !decode(literal).op.is_unallocated(),
            "opc = {opc:02b} is an allocated literal"
        );
        assert!(
            decode(literal | 1 << 24).op.is_unallocated(),
            "opc = {opc:02b} with bit 24 set (0x{:08x}) is unallocated",
            literal | 1 << 24
        );
    }
}

#[test]
fn the_signed_word_pair_has_no_non_temporal_form() {
    // LDNPSW does not exist: opc = 01 is allocated only for the offset,
    // pre- and post-indexed pairs. STPSW does not exist either.
    assert!(
        decode(0x6840_0440).op.is_unallocated(),
        "a non-temporal LDPSW has no encoding"
    );
    assert!(
        decode(0x6900_0440).op.is_unallocated(),
        "a signed-word pair store has no encoding"
    );

    // The three that do exist still decode.
    for (encoding, name) in [
        (0x6940_0440u32, "ldpsw x0, x1, [x2]"),
        (0x68c0_0440, "ldpsw x0, x1, [x2], #0"),
        (0x69c0_0440, "ldpsw x0, x1, [x2, #0]!"),
    ] {
        assert_eq!(decode(encoding).op, Op::Ldp, "{name}");
    }
}

#[test]
fn the_exclusive_forms_need_their_fixed_bit_clear_too() {
    // Like the literals, the whole exclusive and acquire/release group fixes
    // bit 24 at 0. Ignoring it claims a mirror copy of every LDXR, STXR,
    // LDAR and STLR that the architecture leaves unallocated.
    for (encoding, name) in [
        (0xc85f_7c20u32, "ldxr x0, [x1]"),
        (0xc802_7c20, "stxr w2, x0, [x1]"),
        (0xc8df_fc20, "ldar x0, [x1]"),
        (0xc87f_0440, "ldxp x0, x1, [x2]"),
    ] {
        assert!(!decode(encoding).op.is_unallocated(), "{name} is allocated");
        assert!(
            decode(encoding | 1 << 24).op.is_unallocated(),
            "{name} with bit 24 set (0x{:08x}) is unallocated",
            encoding | 1 << 24
        );
    }
}

#[test]
fn a_prefetch_has_no_pre_or_post_indexed_form() {
    // Write-back updates the base register, and a prefetch has no register
    // to write anything into. PRFUM occupies only the unscaled slot, so the
    // indexed encodings beside it are unallocated.
    let unscaled = 0xf880_8000u32; // prfum pldl1keep, [x0, #8]
    assert_eq!(decode(unscaled).op, Op::Prfm);

    for index_bits in [0b01u32, 0b10, 0b11] {
        let encoding = (unscaled & !(0b11 << 10)) | (index_bits << 10);
        assert!(
            decode(encoding).op.is_unallocated(),
            "prefetch with bits 11..10 = {index_bits:02b} (0x{encoding:08x}) is unallocated"
        );
    }
}

#[test]
fn decode_never_panics_across_the_whole_load_store_space() {
    // The M1 gate feeds random words through decode. Every encoding whose
    // op0 selects this group must produce an Instruction, claimed or not.
    let mut claimed = 0u32;
    let mut state = 0x1234_5678_9abc_def0u64;

    for _ in 0..300_000 {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        // Force op0 (bits 28..25) to one of the load/store patterns.
        let group = [0b0100u32, 0b0110, 0b1100, 0b1110][(state & 3) as usize];
        let encoding = (state as u32 & !(0b1111 << 25)) | (group << 25);

        let insn = decode(encoding);
        assert_eq!(insn.encoding, encoding);
        if !insn.op.is_unallocated() {
            claimed += 1;
        }
    }

    assert!(claimed > 0, "the slice should claim part of its own group");
}
