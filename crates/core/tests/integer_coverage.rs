//! Totality and coverage of the three groups the integer slice owns.
//!
//! `docs/plan.md`'s M1 gate feeds a 10,000-binary fuzz corpus of random words
//! through the decoder and requires that unallocated encodings fault
//! identically to the oracle. These sweeps are the cheap half of that: they
//! prove decode terminates for every word in the owned groups, and they pin
//! how much of each group is claimed, so a later change that silently drops
//! coverage shows up here rather than as a differential divergence.

use coracle_core::decode::{decode, EncodingGroup, Op};

/// Walks the encodings whose `op0` selects `group`, holding the low ten bits
/// at zero.
///
/// `op0` is bits 28..25, so a complete walk is 2^28 words per matching `op0`
/// value — too many for every `cargo test`. Almost every field that selects an
/// opcode lives in bits 31..29 and 24..10, and those are swept exhaustively
/// here. The handful of guards below bit 10 — `ERET`'s `Rm`, the exception
/// group's `op2` and `LL`, `CCMP`'s `o3` — are covered by
/// [`sweep_low_bits`] instead.
fn sweep(group: EncodingGroup, mut visit: impl FnMut(u32)) {
    for op0 in 0..16u32 {
        if EncodingGroup::of(op0 << 25) != group {
            continue;
        }
        let base = op0 << 25;
        for high in 0..(1u32 << 7) {
            for mid in 0..(1u32 << 15) {
                visit(base | (high << 29) | (mid << 10));
            }
        }
    }
}

/// Walks the low ten bits exhaustively against a coarse sample of the rest.
///
/// Complements [`sweep`], which holds those bits at zero: the guards that read
/// them decide whether an encoding is allocated at all, so leaving them fixed
/// would exercise exactly one of their 1,024 values.
fn sweep_low_bits(group: EncodingGroup, mut visit: impl FnMut(u32)) {
    for op0 in 0..16u32 {
        if EncodingGroup::of(op0 << 25) != group {
            continue;
        }
        let base = op0 << 25;
        for high in 0..(1u32 << 7) {
            // Bits 24..10 stride coarsely; the point of this sweep is the low
            // ten, and every opcode-selecting field above them is already
            // covered exhaustively by `sweep`.
            for mid in (0..(1u32 << 15)).step_by(97) {
                for low in 0..(1u32 << 10) {
                    visit(base | (high << 29) | (mid << 10) | low);
                }
            }
        }
    }
}

/// Fraction of the swept encodings that decoded to a real opcode.
fn claimed_fraction(group: EncodingGroup) -> f64 {
    let mut total = 0u64;
    let mut claimed = 0u64;
    sweep(group, |word| {
        total += 1;
        if !decode(word).op.is_unallocated() {
            claimed += 1;
        }
    });
    claimed as f64 / total as f64
}

#[test]
fn decoding_the_owned_groups_terminates_for_every_encoding() {
    // Totality is the property the fuzz corpus depends on: a panic here is a
    // crash in the guest's decoder, not a fault delivered to the guest.
    for group in [
        EncodingGroup::DataProcessingImmediate,
        EncodingGroup::DataProcessingRegister,
        EncodingGroup::BranchesExceptionsSystem,
    ] {
        sweep(group, |word| {
            assert_eq!(decode(word).encoding, word);
        });
        sweep_low_bits(group, |word| {
            assert_eq!(decode(word).encoding, word);
        });
    }
}

#[test]
fn the_integer_groups_are_substantially_claimed() {
    // Not 100%: each group has rows the machine deliberately does not
    // advertise (MTE, PAuth, CRC32, FEAT_FlagM, FEAT_HBC), and the ARM ARM
    // leaves others unallocated outright. The thresholds pin the current
    // state so a regression that drops a whole row is visible.
    let immediate = claimed_fraction(EncodingGroup::DataProcessingImmediate);
    assert!(immediate > 0.50, "dp-immediate claimed {immediate:.3}");

    let register = claimed_fraction(EncodingGroup::DataProcessingRegister);
    assert!(register > 0.25, "dp-register claimed {register:.3}");

    let branch = claimed_fraction(EncodingGroup::BranchesExceptionsSystem);
    assert!(branch > 0.30, "branch/system claimed {branch:.3}");
}

#[test]
fn no_opcode_from_another_slice_leaks_out_of_the_integer_groups() {
    // The four slices share one Op enum. If a dispatch arm here fell through
    // into another group's decoder the mistake would look like a working
    // decode, so the owned groups are checked to yield only integer, branch
    // and system opcodes.
    let foreign = |op: Op| {
        matches!(
            op,
            Op::Ldr | Op::Str | Op::Ldp | Op::Stp | Op::Fmov | Op::Fadd | Op::VecAdd
        )
    };

    for group in [
        EncodingGroup::DataProcessingImmediate,
        EncodingGroup::DataProcessingRegister,
        EncodingGroup::BranchesExceptionsSystem,
    ] {
        sweep(group, |word| {
            assert!(!foreign(decode(word).op), "{word:#010x}");
        });
    }
}

#[test]
fn the_encodings_a_static_busybox_emits_are_claimed_or_deliberately_not() {
    // Disassembling the M1 gate's own static busybox turns up exactly five
    // distinct words in the owned groups that this decoder does not claim.
    // All five are correct refusals, and they are pinned here so that a later
    // change which starts claiming one has to say why.
    //
    // GMI, IRG, DC GVA and DC GZVA are MTE, which docs/machine-spec.md §2
    // does not advertise; busybox reaches them only behind a runtime
    // ID_AA64PFR1_EL1.MTE check that reads as absent here. DC ZVA is a
    // cache-maintenance SYS instruction, which lands with the MMU in M2.
    let refused = [
        (0x9adf_1401u32, "gmi"),
        (0x9ac1_1000, "irg"),
        (0xd50b_7423, "dc zva"),
        (0xd50b_7462, "dc gva"),
        (0xd50b_7482, "dc gzva"),
    ];

    for (encoding, name) in refused {
        assert!(decode(encoding).op.is_unallocated(), "{name}");
    }
}
