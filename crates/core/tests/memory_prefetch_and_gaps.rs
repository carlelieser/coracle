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
        // Literal opc = 11 with V = 0 is PRFM; opc = 11 with V = 1 is
        // unallocated rather than a wider vector literal.
        (0x9c00_0000, "literal opc 10 with V = 1"),
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
