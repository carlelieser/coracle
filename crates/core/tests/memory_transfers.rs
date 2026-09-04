//! What a general-purpose load or store transfers: which registers, how wide,
//! and under what ordering.
//!
//! Companion to `memory_addressing.rs`, which covers where the address comes
//! from. Every encoding here was produced by `llvm-mc`.

use coracle_core::decode::address::{AccessSize, AddrMode, Ordering, WriteBack};
use coracle_core::decode::instruction::Form;
use coracle_core::decode::operand::RegWidth;
use coracle_core::decode::{decode, Op};
use coracle_core::reg::Gpr;

/// The transfer a load or store performs, or a panic naming what came instead.
fn transfer_of(encoding: u32) -> (Op, RegWidth, Gpr, Option<Gpr>, AccessSize, Ordering) {
    let insn = decode(encoding);
    let Form::LoadStore {
        rt,
        rt2,
        size,
        ordering,
        ..
    } = insn.form
    else {
        panic!("expected a load or store, got {:?}", insn.form);
    };
    (insn.op, insn.width, rt, rt2, size, ordering)
}

/// The access width a load or store uses.
fn size_of(encoding: u32) -> AccessSize {
    transfer_of(encoding).4
}

#[test]
fn the_size_field_selects_the_bytes_touched() {
    // strb, strh, str w, str x — one encoding per size field.
    assert_eq!(size_of(0x3900_0420).bytes, 1);
    assert_eq!(size_of(0x7900_0420).bytes, 2);
    assert_eq!(size_of(0xb900_0420).bytes, 4);
    assert_eq!(size_of(0xf900_0420).bytes, 8);
}

#[test]
fn a_sign_extending_load_is_a_load_not_a_store() {
    // opc = 10 is a load for every size, even though its low bit is clear.
    // Reading opc<0> as the load bit turns LDRSB into a store, silently.
    for (encoding, name) in [
        (0x3980_0420u32, "ldrsb x0, [x1, #1]"),
        (0x7980_0420, "ldrsh x0, [x1, #2]"),
        (0xb980_0420, "ldrsw x0, [x1, #4]"),
    ] {
        assert_eq!(decode(encoding).op, Op::Ldr, "{name}");
        assert!(size_of(encoding).is_signed, "{name} sign-extends");
    }
}

#[test]
fn a_sign_extending_load_takes_its_width_from_the_low_opc_bit() {
    // opc<0> picks the destination width, inverted: 0 is the 64-bit form.
    // ldrsb x0, [x1, #1]
    assert_eq!(decode(0x3980_0420).width, RegWidth::X64);
    // ldrsb w0, [x1, #1]
    assert_eq!(decode(0x39c0_0420).width, RegWidth::W32);
    // ldrsh x0 / ldrsh w0
    assert_eq!(decode(0x7980_0420).width, RegWidth::X64);
    assert_eq!(decode(0x79c0_0420).width, RegWidth::W32);
    // ldrsw x0 — the only signed word form; there is no 32-bit counterpart.
    assert_eq!(decode(0xb980_0420).width, RegWidth::X64);
}

#[test]
fn an_unsigned_load_takes_its_width_from_the_size_field() {
    // Only the doubleword access writes a 64-bit destination.
    assert_eq!(decode(0x3940_0420).width, RegWidth::W32, "ldrb w0");
    assert_eq!(decode(0x7940_0420).width, RegWidth::W32, "ldrh w0");
    assert_eq!(decode(0xb940_0420).width, RegWidth::W32, "ldr w0");
    assert_eq!(decode(0xf940_0420).width, RegWidth::X64, "ldr x0");
    assert!(!size_of(0xf940_0420).is_signed);
}

#[test]
fn a_store_is_never_sign_extending() {
    // Sign-extension describes how a load fills its destination; a store
    // has none to fill.
    for encoding in [0x3900_0420u32, 0x7900_0420, 0xb900_0420, 0xf900_0420] {
        assert_eq!(decode(encoding).op, Op::Str);
        assert!(!size_of(encoding).is_signed, "0x{encoding:08x}");
    }
}

#[test]
fn a_pair_names_both_transferred_registers() {
    // ldp x29, x30, [sp], #16
    let (op, width, rt, rt2, size, _) = transfer_of(0xa8c1_7bfd);

    assert_eq!(op, Op::Ldp);
    assert_eq!(width, RegWidth::X64);
    assert_eq!(rt, Gpr::X(29));
    assert_eq!(rt2, Some(Gpr::X(30)));
    assert_eq!(size, AccessSize::X);
}

#[test]
fn a_pair_offset_is_signed_and_scaled_by_one_register() {
    // stp x29, x30, [sp, #-16]! — imm7 counts registers, so -1 is -16 bytes.
    let Form::LoadStore { addr, .. } = decode(0xa9bf_7bfd).form else {
        panic!("expected a pair store");
    };
    assert_eq!(
        addr,
        AddrMode::Immediate {
            base: Gpr::SP,
            offset: -16,
            writeback: WriteBack::Pre,
        }
    );

    // stp w0, w1, [x2, #4] — the same imm7 scaled by 4 for a word pair.
    let Form::LoadStore { addr, .. } = decode(0x2900_8440).form else {
        panic!("expected a pair store");
    };
    assert_eq!(
        addr,
        AddrMode::Immediate {
            base: Gpr::X(2),
            offset: 4,
            writeback: WriteBack::None,
        }
    );
}

#[test]
fn a_pair_offset_reaches_both_ends_of_its_signed_field() {
    // ldp x0, x1, [sp, #504] — imm7 = 63, the largest doubleword pair offset.
    let Form::LoadStore { addr, .. } = decode(0xa95f_87e0).form else {
        panic!("expected a pair load");
    };
    assert_eq!(addr.base(), Some(Gpr::SP));
    let AddrMode::Immediate { offset, .. } = addr else {
        panic!("expected an immediate address");
    };
    assert_eq!(offset, 504);

    // stp x0, x1, [sp, #-512] — imm7 = -64.
    let Form::LoadStore { addr, .. } = decode(0xa920_07e0).form else {
        panic!("expected a pair store");
    };
    let AddrMode::Immediate { offset, .. } = addr else {
        panic!("expected an immediate address");
    };
    assert_eq!(offset, -512);
}

#[test]
fn every_pair_addressing_mode_decodes_to_its_writeback() {
    let modes = [
        (0xa940_0440u32, WriteBack::None, "ldp x0, x1, [x2]"),
        (0xa9c1_0440, WriteBack::Pre, "ldp x0, x1, [x2, #16]!"),
        (0xa8bf_0440, WriteBack::Post, "stp x0, x1, [x2], #-16"),
    ];

    for (encoding, expected, name) in modes {
        let Form::LoadStore {
            addr: AddrMode::Immediate { writeback, .. },
            ..
        } = decode(encoding).form
        else {
            panic!("{name} should compute an immediate address");
        };
        assert_eq!(writeback, expected, "{name}");
    }
}

#[test]
fn a_non_temporal_pair_decodes_as_an_ordinary_pair() {
    // STNP is a hint about cache allocation, not a different transfer, and
    // this machine has no cache to hint about.
    // stnp x0, x1, [x2, #16]
    let (op, _, rt, rt2, size, ordering) = transfer_of(0xa801_0440);

    assert_eq!(op, Op::Stp);
    assert_eq!(rt, Gpr::X(0));
    assert_eq!(rt2, Some(Gpr::X(1)));
    assert_eq!(size, AccessSize::X);
    assert_eq!(ordering, Ordering::PLAIN);

    // ldnp w0, w1, [x2, #-8]
    let (op, width, _, _, size, _) = transfer_of(0x287f_0440);
    assert_eq!(op, Op::Ldp);
    assert_eq!(width, RegWidth::W32);
    assert_eq!(size, AccessSize::W);
}

#[test]
fn a_non_temporal_pair_never_writes_back() {
    // The encoding has no writeback form; nothing should invent one.
    for encoding in [0xa801_0440u32, 0x287f_0440] {
        let Form::LoadStore { addr, .. } = decode(encoding).form else {
            panic!("expected a pair access");
        };
        assert!(!addr.has_writeback(), "0x{encoding:08x}");
    }
}

#[test]
fn a_signed_word_pair_loads_four_bytes_into_a_64_bit_register() {
    // ldpsw x0, x1, [x2, #8] — a 4-byte access sign-extended to 64 bits.
    let (op, width, rt, rt2, size, _) = transfer_of(0x6941_0440);

    assert_eq!(op, Op::Ldp);
    assert_eq!(width, RegWidth::X64);
    assert_eq!(rt, Gpr::X(0));
    assert_eq!(rt2, Some(Gpr::X(1)));
    assert_eq!(
        size,
        AccessSize {
            bytes: 4,
            is_signed: true
        }
    );
}

#[test]
fn a_signed_word_pair_scales_its_offset_by_four_not_eight() {
    // ldpsw x0, x1, [x2, #-256] — imm7 = -64 at a 4-byte element size. The
    // 64-bit destination must not pull the scale up to 8.
    let Form::LoadStore { addr, .. } = decode(0x6960_0440).form else {
        panic!("expected a pair load");
    };
    assert_eq!(
        addr,
        AddrMode::Immediate {
            base: Gpr::X(2),
            offset: -256,
            writeback: WriteBack::None,
        }
    );
}

#[test]
fn an_exclusive_load_takes_no_status_register() {
    // ldxr x0, [x1] — the status register belongs to the store half.
    let (op, _, rt, rt2, size, ordering) = transfer_of(0xc85f_7c20);

    assert_eq!(op, Op::Ldar);
    assert_eq!(rt, Gpr::X(0));
    assert_eq!(rt2, None);
    assert_eq!(size, AccessSize::X);
    assert!(ordering.is_exclusive);
    assert!(!ordering.is_acquire, "ldxr does not acquire");
    assert!(!ordering.is_release);

    let Form::LoadStore { rs, .. } = decode(0xc85f_7c20).form else {
        panic!("expected an exclusive load");
    };
    assert_eq!(rs, None);
}

#[test]
fn an_exclusive_store_names_its_status_register() {
    // stxr w2, x0, [x1] — w2 receives the success flag.
    let Form::LoadStore {
        rt, rs, ordering, ..
    } = decode(0xc802_7c20).form
    else {
        panic!("expected an exclusive store");
    };

    assert_eq!(decode(0xc802_7c20).op, Op::Stlr);
    assert_eq!(rt, Gpr::X(0));
    assert_eq!(rs, Some(Gpr::X(2)));
    assert!(ordering.is_exclusive);
    assert!(!ordering.is_release, "stxr does not release");
}

#[test]
fn the_status_register_is_read_with_the_zero_register_rule() {
    // stxr wzr, x0, [x1] — slot 31 in the Rs field is WZR, not SP.
    let Form::LoadStore { rs, .. } = decode(0xc81f_7c20).form else {
        panic!("expected an exclusive store");
    };
    assert_eq!(rs, Some(Gpr::ZR));
}

#[test]
fn an_exclusive_pair_names_two_registers_and_a_status_register() {
    // stxp w3, x0, x1, [x2]
    let Form::LoadStore {
        rt,
        rt2,
        rs,
        size,
        ordering,
        ..
    } = decode(0xc823_0440).form
    else {
        panic!("expected an exclusive pair store");
    };

    assert_eq!(rt, Gpr::X(0));
    assert_eq!(rt2, Some(Gpr::X(1)));
    assert_eq!(rs, Some(Gpr::X(3)));
    assert_eq!(size, AccessSize::X);
    assert!(ordering.is_exclusive);

    // ldxp x0, x1, [x2] — the load half has no status register.
    let Form::LoadStore { rt, rt2, rs, .. } = decode(0xc87f_0440).form else {
        panic!("expected an exclusive pair load");
    };
    assert_eq!(rt, Gpr::X(0));
    assert_eq!(rt2, Some(Gpr::X(1)));
    assert_eq!(rs, None);
}

#[test]
fn an_exclusive_pair_of_words_accesses_four_bytes_per_register() {
    // stxp w3, w0, w1, [x2] — the size field is 0 for the 32-bit pair,
    // where the single-register forms would read that as a byte access.
    let (_, width, _, _, size, _) = transfer_of(0x8823_0440);

    assert_eq!(width, RegWidth::W32);
    assert_eq!(size, AccessSize::W);

    // ldxp x0, x1, [x2] — size field 1 is the 64-bit pair.
    let (_, width, _, _, size, _) = transfer_of(0xc87f_0440);
    assert_eq!(width, RegWidth::X64);
    assert_eq!(size, AccessSize::X);
}

#[test]
fn the_exclusive_sizes_cover_byte_through_doubleword() {
    // ldxrb, ldxrh, ldxr w, ldxr x
    assert_eq!(size_of(0x085f_7c20).bytes, 1);
    assert_eq!(size_of(0x485f_7c20).bytes, 2);
    assert_eq!(size_of(0x885f_7c20).bytes, 4);
    assert_eq!(size_of(0xc85f_7c20).bytes, 8);
}

#[test]
fn the_acquire_release_exclusives_carry_both_flags() {
    // ldaxr x0, [x1] — exclusive and acquiring.
    let (_, _, _, _, _, ordering) = transfer_of(0xc85f_fc20);
    assert!(ordering.is_exclusive);
    assert!(ordering.is_acquire);
    assert!(!ordering.is_release);

    // stlxr w2, x0, [x1] — exclusive and releasing.
    let (_, _, _, _, _, ordering) = transfer_of(0xc802_fc20);
    assert!(ordering.is_exclusive);
    assert!(ordering.is_release);
    assert!(!ordering.is_acquire);

    // ldaxp x0, x1, [x2] and stlxp w3, x0, x1, [x2]
    let (_, _, _, _, _, ordering) = transfer_of(0xc87f_8440);
    assert!(ordering.is_exclusive && ordering.is_acquire);
    let (_, _, _, _, _, ordering) = transfer_of(0xc823_8440);
    assert!(ordering.is_exclusive && ordering.is_release);
}

#[test]
fn the_plain_ordered_forms_are_not_exclusive() {
    // ldar x0, [x1] acquires without taking the monitor; treating it as
    // exclusive would make every LDAR clobber an outstanding reservation.
    let (op, _, rt, _, size, ordering) = transfer_of(0xc8df_fc20);
    assert_eq!(op, Op::Ldar);
    assert_eq!(rt, Gpr::X(0));
    assert_eq!(size, AccessSize::X);
    assert!(ordering.is_acquire);
    assert!(!ordering.is_exclusive);
    assert!(!ordering.is_release);

    // stlr x0, [x1]
    let (op, _, _, _, _, ordering) = transfer_of(0xc89f_fc20);
    assert_eq!(op, Op::Stlr);
    assert!(ordering.is_release);
    assert!(!ordering.is_exclusive);
    assert!(!ordering.is_acquire);
}

#[test]
fn a_store_release_takes_no_status_register() {
    // STLR reuses the exclusive encoding's Rs field, which reads as 31
    // there. It is not a status register, so it must not be reported.
    let Form::LoadStore { rs, .. } = decode(0xc89f_fc20).form else {
        panic!("expected a store-release");
    };
    assert_eq!(rs, None);
}

#[test]
fn the_ordered_forms_cover_every_access_size() {
    // ldarb, ldarh, ldar w, ldar x
    assert_eq!(size_of(0x08df_fc20).bytes, 1);
    assert_eq!(size_of(0x48df_fc20).bytes, 2);
    assert_eq!(size_of(0x88df_fc20).bytes, 4);
    assert_eq!(size_of(0xc8df_fc20).bytes, 8);
    // stlrb w0, [x1] and stlrh w0, [x1]
    assert_eq!(decode(0x089f_fc20).op, Op::Stlr);
    assert_eq!(size_of(0x489f_fc20).bytes, 2);
}

#[test]
fn a_literal_load_reports_the_width_its_opc_names() {
    // ldr w0, back — a 4-byte access into a 32-bit register.
    let (op, width, rt, _, size, _) = transfer_of(0x18ff_ffc0);
    assert_eq!(op, Op::Ldr);
    assert_eq!(width, RegWidth::W32);
    assert_eq!(rt, Gpr::X(0));
    assert_eq!(size, AccessSize::W);

    // ldr x0, back
    let (_, width, _, _, size, _) = transfer_of(0x58ff_ffa0);
    assert_eq!(width, RegWidth::X64);
    assert_eq!(size, AccessSize::X);
}

#[test]
fn a_literal_signed_word_load_sign_extends_into_64_bits() {
    // ldrsw x0, back — 4 bytes read, 64-bit destination.
    let (op, width, _, _, size, _) = transfer_of(0x98ff_ff80);

    assert_eq!(op, Op::Ldr);
    assert_eq!(width, RegWidth::X64);
    assert_eq!(
        size,
        AccessSize {
            bytes: 4,
            is_signed: true
        }
    );
}
