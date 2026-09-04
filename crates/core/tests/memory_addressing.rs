//! Every addressing mode a general-purpose load or store can name.
//!
//! One test per mode rather than per mnemonic: the mode is what the decoder
//! computes, and the mnemonics within a mode differ only in their opcode and
//! access size. Every encoding here was produced by `llvm-mc`, not derived by
//! hand.

use coracle_core::decode::address::{AddrMode, WriteBack};
use coracle_core::decode::instruction::Form;
use coracle_core::decode::{decode, Op};
use coracle_core::reg::Gpr;

/// The address a load or store computes, or a panic naming what came instead.
fn addr_of(encoding: u32) -> AddrMode {
    match decode(encoding).form {
        Form::LoadStore { addr, .. } | Form::Prefetch { addr, .. } => addr,
        other => panic!("expected a memory access, got {other:?}"),
    }
}

#[test]
fn an_unsigned_offset_is_scaled_by_the_access_size() {
    // ldr x0, [x1, #16] — imm12 = 2, scaled by 8.
    assert_eq!(
        addr_of(0xf940_0820),
        AddrMode::Immediate {
            base: Gpr::X(1),
            offset: 16,
            writeback: WriteBack::None,
        }
    );
    // ldrh w0, [x1, #4094] — the largest halfword offset, imm12 = 2047.
    assert_eq!(
        addr_of(0x795f_fc20),
        AddrMode::Immediate {
            base: Gpr::X(1),
            offset: 4094,
            writeback: WriteBack::None,
        }
    );
}

#[test]
fn an_unsigned_offset_is_never_negative() {
    // ldrsw x0, [x1, #16380] — imm12 = 4095 at scale 2. The field is
    // unsigned, so the widest offset must not sign-extend into a negative.
    assert_eq!(
        addr_of(0xb9bf_fc20),
        AddrMode::Immediate {
            base: Gpr::X(1),
            offset: 16380,
            writeback: WriteBack::None,
        }
    );
}

#[test]
fn an_unscaled_offset_is_signed_and_left_unscaled() {
    // ldur x0, [x1, #-8] — imm9 is signed and not multiplied by the size.
    assert_eq!(
        addr_of(0xf85f_8020),
        AddrMode::Immediate {
            base: Gpr::X(1),
            offset: -8,
            writeback: WriteBack::None,
        }
    );
    // stur w2, [x3, #255] — the largest positive imm9.
    assert_eq!(
        addr_of(0xb80f_f062),
        AddrMode::Immediate {
            base: Gpr::X(3),
            offset: 255,
            writeback: WriteBack::None,
        }
    );
    // ldurb w4, [x5, #-256] — the most negative imm9.
    assert_eq!(
        addr_of(0x3850_00a4),
        AddrMode::Immediate {
            base: Gpr::X(5),
            offset: -256,
            writeback: WriteBack::None,
        }
    );
}

#[test]
fn a_post_indexed_offset_updates_the_base_after_the_access() {
    // ldr x0, [x1], #8
    assert_eq!(
        addr_of(0xf840_8420),
        AddrMode::Immediate {
            base: Gpr::X(1),
            offset: 8,
            writeback: WriteBack::Post,
        }
    );
    // str x2, [x3], #-256
    assert_eq!(
        addr_of(0xf810_0462),
        AddrMode::Immediate {
            base: Gpr::X(3),
            offset: -256,
            writeback: WriteBack::Post,
        }
    );
}

#[test]
fn a_pre_indexed_offset_updates_the_base_before_the_access() {
    // ldr x0, [x1, #8]!
    assert_eq!(
        addr_of(0xf840_8c20),
        AddrMode::Immediate {
            base: Gpr::X(1),
            offset: 8,
            writeback: WriteBack::Pre,
        }
    );
    // ldrsb w4, [x5, #1]!
    assert_eq!(
        addr_of(0x38c0_1ca4),
        AddrMode::Immediate {
            base: Gpr::X(5),
            offset: 1,
            writeback: WriteBack::Pre,
        }
    );
}

#[test]
fn the_indexed_immediate_forms_are_never_scaled_by_the_access_size() {
    // ldr x0, [x1], #8 and ldur x0, [x1, #8] share one imm9 field. Scaling
    // either would make the doubleword post-index step 64 bytes.
    let post = addr_of(0xf840_8420);
    let unscaled = addr_of(0xf840_8020);

    let (
        AddrMode::Immediate { offset: post, .. },
        AddrMode::Immediate {
            offset: unscaled, ..
        },
    ) = (post, unscaled)
    else {
        panic!("both forms compute an immediate address");
    };
    assert_eq!(post, 8);
    assert_eq!(unscaled, 8);
}

#[test]
fn a_register_offset_carries_its_extension_and_scale() {
    use coracle_core::decode::operand::{ExtendKind, ExtendedReg};

    // ldr x0, [x1, x2, lsl #3] — S = 1 scales by log2(8).
    assert_eq!(
        addr_of(0xf862_7820),
        AddrMode::Register {
            base: Gpr::X(1),
            index: ExtendedReg {
                reg: Gpr::X(2),
                kind: ExtendKind::Uxtx,
                amount: 3,
            },
            writeback: WriteBack::None,
        }
    );
    // ldr x0, [x1, x2] — S = 0, so no scaling despite the 8-byte access.
    assert_eq!(
        addr_of(0xf862_6820),
        AddrMode::Register {
            base: Gpr::X(1),
            index: ExtendedReg {
                reg: Gpr::X(2),
                kind: ExtendKind::Uxtx,
                amount: 0,
            },
            writeback: WriteBack::None,
        }
    );
}

#[test]
fn a_register_offset_can_extend_a_32_bit_index() {
    use coracle_core::decode::operand::{ExtendKind, ExtendedReg};

    // ldr w0, [x1, w2, uxtw #2]
    assert_eq!(
        addr_of(0xb862_5820),
        AddrMode::Register {
            base: Gpr::X(1),
            index: ExtendedReg {
                reg: Gpr::X(2),
                kind: ExtendKind::Uxtw,
                amount: 2,
            },
            writeback: WriteBack::None,
        }
    );
    // ldr w0, [x1, w2, sxtw] — signed, and S = 0 leaves the scale at zero.
    assert_eq!(
        addr_of(0xb862_c820),
        AddrMode::Register {
            base: Gpr::X(1),
            index: ExtendedReg {
                reg: Gpr::X(2),
                kind: ExtendKind::Sxtw,
                amount: 0,
            },
            writeback: WriteBack::None,
        }
    );
}

#[test]
fn a_byte_register_offset_scales_by_zero_even_when_s_is_set() {
    use coracle_core::decode::operand::ExtendedReg;

    // ldrb w0, [x1, x2, lsl #0] — log2(1) is 0, so S selects a zero shift.
    let AddrMode::Register {
        index: ExtendedReg { amount, .. },
        ..
    } = addr_of(0x3862_7820)
    else {
        panic!("expected a register offset");
    };
    assert_eq!(amount, 0);
}

#[test]
fn a_literal_load_is_pc_relative_and_signed() {
    // ldr w0, back — 19-bit offset of -8 words from this instruction.
    assert_eq!(addr_of(0x18ff_ffc0), AddrMode::PcRelative { offset: -8 });
    // ldr w5, fwd — a forward reference, +12 bytes.
    assert_eq!(addr_of(0x1800_0065), AddrMode::PcRelative { offset: 12 });
}

#[test]
fn a_literal_offset_is_scaled_to_bytes_by_four() {
    // ldr x0, back at -12 bytes: imm19 counts words, not bytes.
    assert_eq!(addr_of(0x58ff_ffa0), AddrMode::PcRelative { offset: -12 });
}

#[test]
fn the_exclusive_and_ordered_forms_have_no_offset_at_all() {
    // ldxr x0, [x1]
    assert_eq!(addr_of(0xc85f_7c20), AddrMode::BaseOnly { base: Gpr::X(1) });
    // ldar x0, [x1]
    assert_eq!(addr_of(0xc8df_fc20), AddrMode::BaseOnly { base: Gpr::X(1) });
    // stxp w3, x0, x1, [x2]
    assert_eq!(addr_of(0xc823_0440), AddrMode::BaseOnly { base: Gpr::X(2) });
}

#[test]
fn slot_31_is_the_stack_pointer_in_every_base_position() {
    // The base is SP-form and the transferred register ZR-form. Reading
    // either with the wrong rule is silent, so each mode is pinned.
    let bases = [
        0xf940_03e0u32, // ldr x0, [sp]
        0xf85f_83e0,    // ldur x0, [sp, #-8]
        0xf840_87e0,    // ldr x0, [sp], #8
        0xf840_8fe0,    // ldr x0, [sp, #8]!
        0xf862_6be0,    // ldr x0, [sp, x2]
        0xc85f_7fe0,    // ldxr x0, [sp]
        0x88df_ffe0,    // ldar w0, [sp]
        0xa940_07e0,    // ldp x0, x1, [sp]
    ];

    for encoding in bases {
        assert_eq!(
            addr_of(encoding).base(),
            Some(Gpr::SP),
            "0x{encoding:08x} should address through SP"
        );
    }
}

#[test]
fn slot_31_is_the_zero_register_in_every_transfer_position() {
    // str xzr, [x0] — the transferred register uses the ZR rule, so a
    // store of slot 31 writes zero rather than the stack pointer.
    let Form::LoadStore { rt, .. } = decode(0xf900_001f).form else {
        panic!("expected a store");
    };
    assert_eq!(rt, Gpr::ZR);

    // stp xzr, xzr, [sp] — both transferred registers, and the base is SP.
    let Form::LoadStore { rt, rt2, addr, .. } = decode(0xa900_7fff).form else {
        panic!("expected a pair store");
    };
    assert_eq!(rt, Gpr::ZR);
    assert_eq!(rt2, Some(Gpr::ZR));
    assert_eq!(addr.base(), Some(Gpr::SP));
}

#[test]
fn a_literal_load_reads_no_base_register() {
    // ldr x0, back forms its address from the PC alone.
    assert_eq!(addr_of(0x58ff_ffa0).base(), None);
    assert_eq!(decode(0x58ff_ffa0).op, Op::Ldr);
}
