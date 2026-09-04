//! Per-group decoders reached from the top-level `op0` switch.
//!
//! Each function owns one row of the ARM ARM's top-level encoding table and is
//! the seam a phase B slice fills in. They are stubs: every one returns
//! [`unallocated`] for the encodings it has not yet claimed, which is a working
//! state rather than a hole — an unclaimed encoding traps and is logged, and
//! that log is what drives the lazy NEON implementation the plan calls for.
//!
//! The handful of encodings decoded here exist to prove the dispatch, operand
//! model and trace path end to end; they are not a coverage claim.

use super::address::{AccessSize, AddrMode, Ordering, WriteBack};
use super::instruction::{unallocated, Form, Instruction};
use super::op::Op;
use super::operand::{RegWidth, ShiftKind, ShiftedReg};
use crate::reg::Gpr;

/// Extracts the `[hi:lo]` bit field of an encoding, inclusive at both ends.
const fn bits(encoding: u32, hi: u32, lo: u32) -> u32 {
    (encoding >> lo) & ((1 << (hi - lo + 1)) - 1)
}

/// Sign-extends the low `width` bits of `value`.
const fn sign_extend(value: u32, width: u32) -> i64 {
    let shift = 64 - width;
    ((value as i64) << shift) >> shift
}

/// `op0 = 100x` — data processing (immediate).
///
/// Owned by the integer slice. Add/sub (immediate) is decoded to prove the
/// [`Form::RegImm`] path; move-wide, bitfield, extract, logical (immediate) and
/// PC-relative addressing remain.
pub fn data_processing_immediate(encoding: u32) -> Instruction {
    // Add/sub (immediate): op0=100, op1=010 in bits 25..23.
    if bits(encoding, 25, 23) != 0b010 {
        return unallocated(encoding);
    }

    let shift = bits(encoding, 23, 22);
    // `sh` selects a 12-bit left shift of the immediate; other values of the
    // field belong to add/sub (immediate, with tags), which needs MTE.
    let imm = match shift & 1 {
        0 => bits(encoding, 21, 10) as u64,
        _ => (bits(encoding, 21, 10) as u64) << 12,
    };
    let is_sub = bits(encoding, 30, 30) == 1;
    let sets_flags = bits(encoding, 29, 29) == 1;

    // Rd is SP unless the instruction sets flags, in which case slot 31 is the
    // zero register — the difference between `ADD SP, ...` and `CMN`.
    let rd = if sets_flags {
        Gpr::from_index_zr(bits(encoding, 4, 0) as u8)
    } else {
        Gpr::from_index_sp(bits(encoding, 4, 0) as u8)
    };
    let form = Form::RegImm {
        rd,
        rn: Gpr::from_index_sp(bits(encoding, 9, 5) as u8),
        imm,
    };

    let op = if is_sub { Op::Sub } else { Op::Add };
    let insn = Instruction::new(encoding, op, form)
        .with_width(RegWidth::from_sf(bits(encoding, 31, 31) == 1));

    if sets_flags {
        insn.setting_flags()
    } else {
        insn
    }
}

/// `op0 = 101x` — branches, exception generation and system instructions.
///
/// Owned by the integer slice. Unconditional immediate branches and `RET` are
/// decoded to prove the branch forms; conditional branches, compare-and-branch,
/// the exception-generation group, barriers and `MSR`/`MRS` remain.
pub fn branches_exceptions_system(encoding: u32) -> Instruction {
    // Unconditional branch (immediate): op0=x00 in bits 31..29, 00101 in 30..26.
    if bits(encoding, 30, 26) == 0b00101 {
        let op = if bits(encoding, 31, 31) == 1 {
            Op::Bl
        } else {
            Op::B
        };
        let offset = sign_extend(bits(encoding, 25, 0), 26) * 4;
        return Instruction::new(encoding, op, Form::Branch { offset });
    }

    // RET: 1101011 0 0 10 11111 000000 Rn 00000.
    if encoding & 0xffff_fc1f == 0xd65f_0000 {
        let rn = Gpr::from_index_zr(bits(encoding, 9, 5) as u8);
        return Instruction::new(encoding, Op::Ret, Form::BranchIndirect { rn });
    }

    // NOP: the hint space, encoded as a system instruction with CRm:op2 = 0.
    if encoding == 0xd503_201f {
        return Instruction::new(encoding, Op::Nop, Form::None);
    }

    unallocated(encoding)
}

/// `op0 = x1x0` — loads and stores.
///
/// Owned by the memory slice. The unsigned-offset single-register forms are
/// decoded to prove [`AddrMode::Immediate`]; pre/post-indexed, register-offset,
/// literal, pair, exclusive and acquire/release forms remain, as do all the
/// SIMD/FP transfers.
pub fn loads_and_stores(encoding: u32) -> Instruction {
    // Load/store register (unsigned immediate): bits 29..27 = 111, 25..24 = 01,
    // and V = 0 for the general-purpose forms.
    let is_unsigned_offset = bits(encoding, 29, 27) == 0b111 && bits(encoding, 25, 24) == 0b01;
    if !is_unsigned_offset || bits(encoding, 26, 26) == 1 {
        return unallocated(encoding);
    }

    let size_field = bits(encoding, 31, 30);
    let opc = bits(encoding, 23, 22);
    // opc<0> selects load over store for the unsigned forms; opc<1> with a load
    // selects sign-extension, which also narrows the destination to `W`.
    let is_load = opc & 1 == 1;
    let is_signed = is_load && opc & 0b10 != 0;

    let size = AccessSize {
        bytes: 1 << size_field,
        is_signed,
    };
    let addr = AddrMode::Immediate {
        base: Gpr::from_index_sp(bits(encoding, 9, 5) as u8),
        offset: ((bits(encoding, 21, 10) as u64) << size.scale()) as i64,
        writeback: WriteBack::None,
    };
    let form = Form::LoadStore {
        rt: Gpr::from_index_zr(bits(encoding, 4, 0) as u8),
        rt2: None,
        rs: None,
        addr,
        size,
        ordering: Ordering::PLAIN,
    };

    let op = if is_load { Op::Ldr } else { Op::Str };
    // A sign-extending load writes a 32-bit destination when opc<0> is set.
    let width = RegWidth::from_sf(if is_signed {
        opc & 1 == 0
    } else {
        size_field == 0b11
    });
    Instruction::new(encoding, op, form).with_width(width)
}

/// `op0 = x101` — data processing (register).
///
/// Owned by the integer slice. Logical and add/sub shifted-register forms are
/// decoded to prove [`Form::RegShifted`]; extended-register, three-source,
/// conditional select/compare, variable shift and the bit-manipulation group
/// remain.
pub fn data_processing_register(encoding: u32) -> Instruction {
    let is_logical = bits(encoding, 28, 24) == 0b01010 && bits(encoding, 21, 21) == 0;
    let is_add_sub = bits(encoding, 28, 24) == 0b01011 && bits(encoding, 21, 21) == 0;
    if !is_logical && !is_add_sub {
        return unallocated(encoding);
    }

    let rm = ShiftedReg {
        reg: Gpr::from_index_zr(bits(encoding, 20, 16) as u8),
        kind: shift_kind(bits(encoding, 23, 22)),
        amount: bits(encoding, 15, 10) as u8,
    };
    let form = Form::RegShifted {
        rd: Gpr::from_index_zr(bits(encoding, 4, 0) as u8),
        rn: Gpr::from_index_zr(bits(encoding, 9, 5) as u8),
        rm,
    };

    let opc = bits(encoding, 30, 29);
    let is_negated = is_logical && bits(encoding, 21, 21) == 1;
    let op = if is_logical {
        logical_op(opc, is_negated)
    } else if opc & 0b10 != 0 {
        Op::Sub
    } else {
        Op::Add
    };

    let width = RegWidth::from_sf(bits(encoding, 31, 31) == 1);
    let insn = Instruction::new(encoding, op, form).with_width(width);
    // ADDS/SUBS set flags on opc<0>; the logical group sets them only for ANDS,
    // which is opc = 11.
    let sets_flags = if is_logical {
        opc == 0b11
    } else {
        opc & 1 == 1
    };
    if sets_flags {
        insn.setting_flags()
    } else {
        insn
    }
}

/// `op0 = x111` — data processing, scalar FP and advanced SIMD.
///
/// Owned by the FP+NEON slice. Nothing is decoded yet: every encoding traps and
/// is logged, which is exactly the mechanism `docs/plan.md` names for driving
/// NEON coverage.
pub fn data_processing_simd_fp(encoding: u32) -> Instruction {
    unallocated(encoding)
}

/// Decodes the 2-bit `shift` field of a data-processing (register) encoding.
const fn shift_kind(field: u32) -> ShiftKind {
    match field & 0b11 {
        0b00 => ShiftKind::Lsl,
        0b01 => ShiftKind::Lsr,
        0b10 => ShiftKind::Asr,
        _ => ShiftKind::Ror,
    }
}

/// Selects the logical opcode from `opc` and the `N` (negate) bit.
const fn logical_op(opc: u32, is_negated: bool) -> Op {
    match (opc & 0b11, is_negated) {
        (0b00, false) => Op::And,
        (0b00, true) => Op::Bic,
        (0b01, false) => Op::Orr,
        (0b01, true) => Op::Orn,
        (0b10, false) => Op::Eor,
        (0b10, true) => Op::Eon,
        (_, false) => Op::And,
        (_, true) => Op::Bic,
    }
}

#[cfg(test)]
mod tests {
    use super::super::decode;
    use super::*;

    #[test]
    fn add_immediate_resolves_slot_31_as_sp_and_keeps_its_immediate() {
        // add sp, sp, #0x10
        let insn = decode(0x9100_43ff);

        assert_eq!(insn.op, Op::Add);
        assert!(!insn.sets_flags);
        assert_eq!(insn.width, RegWidth::X64);
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
    fn a_shifted_immediate_is_expanded_at_decode_time() {
        // add x0, x0, #0x1000  (sh = 1, imm12 = 1)
        let insn = decode(0x9140_0400);

        assert_eq!(
            insn.form,
            Form::RegImm {
                rd: Gpr::X(0),
                rn: Gpr::X(0),
                imm: 0x1000,
            }
        );
    }

    #[test]
    fn a_32_bit_add_immediate_reports_the_narrow_width() {
        // add w1, w2, #3
        let insn = decode(0x1100_0c41);

        assert_eq!(insn.width, RegWidth::W32);
        assert_eq!(insn.op, Op::Add);
    }

    #[test]
    fn branch_offsets_are_sign_extended_and_scaled_to_bytes() {
        // b .+8
        assert_eq!(
            decode(0x1400_0002).form,
            Form::Branch { offset: 8 },
            "forward branch"
        );
        // bl .-8
        let backward = decode(0x97ff_fffe);
        assert_eq!(backward.op, Op::Bl);
        assert_eq!(backward.form, Form::Branch { offset: -8 });
    }

    #[test]
    fn ret_decodes_to_an_indirect_branch_through_its_link_register() {
        // ret  (implicitly x30)
        let insn = decode(0xd65f_03c0);

        assert_eq!(insn.op, Op::Ret);
        assert_eq!(insn.form, Form::BranchIndirect { rn: Gpr::X(30) });
    }

    #[test]
    fn nop_decodes_with_no_operands() {
        let insn = decode(0xd503_201f);

        assert_eq!(insn.op, Op::Nop);
        assert_eq!(insn.form, Form::None);
    }

    #[test]
    fn an_unsigned_offset_load_scales_its_immediate_by_the_access_size() {
        // ldr x0, [x1, #16]  — imm12 = 2, scaled by 8
        let insn = decode(0xf940_0820);

        assert_eq!(insn.op, Op::Ldr);
        assert_eq!(
            insn.form,
            Form::LoadStore {
                rt: Gpr::X(0),
                rt2: None,
                rs: None,
                addr: AddrMode::Immediate {
                    base: Gpr::X(1),
                    offset: 16,
                    writeback: WriteBack::None,
                },
                size: AccessSize::X,
                ordering: Ordering::PLAIN,
            }
        );
    }

    #[test]
    fn a_byte_store_reports_a_one_byte_unsigned_access() {
        // strb w0, [x1, #3]
        let insn = decode(0x3900_0c20);

        assert_eq!(insn.op, Op::Str);
        let Form::LoadStore { size, addr, .. } = insn.form else {
            panic!("expected a single-register load/store, got {:?}", insn.form);
        };
        assert_eq!(size, AccessSize::B);
        assert_eq!(
            addr,
            AddrMode::Immediate {
                base: Gpr::X(1),
                offset: 3,
                writeback: WriteBack::None,
            }
        );
    }

    #[test]
    fn a_load_through_slot_31_uses_the_stack_pointer_as_its_base() {
        // ldr x0, [sp]
        let insn = decode(0xf940_03e0);

        let Form::LoadStore { addr, .. } = insn.form else {
            panic!("expected a single-register load/store, got {:?}", insn.form);
        };
        assert_eq!(addr.base(), Some(Gpr::SP));
    }

    #[test]
    fn shifted_register_forms_carry_their_shift_kind_and_amount() {
        // add x0, x1, x2, lsl #4
        let insn = decode(0x8b02_1020);

        assert_eq!(insn.op, Op::Add);
        assert_eq!(
            insn.form,
            Form::RegShifted {
                rd: Gpr::X(0),
                rn: Gpr::X(1),
                rm: ShiftedReg {
                    reg: Gpr::X(2),
                    kind: ShiftKind::Lsl,
                    amount: 4,
                },
            }
        );
    }

    #[test]
    fn the_logical_group_sets_flags_only_for_ands() {
        // and x0, x1, x2
        assert!(!decode(0x8a02_0020).sets_flags);
        // ands x0, x1, x2
        let ands = decode(0xea02_0020);
        assert_eq!(ands.op, Op::And);
        assert!(ands.sets_flags);
        // orr x0, x1, x2
        let orr = decode(0xaa02_0020);
        assert_eq!(orr.op, Op::Orr);
        assert!(!orr.sets_flags);
    }

    #[test]
    fn every_simd_fp_encoding_is_still_unclaimed() {
        // The FP+NEON slice has not landed. Until it does the whole group must
        // trap rather than decode to something wrong.
        assert!(decode(0x1e20_2800).op.is_unallocated());
        assert!(decode(0x4e20_8400).op.is_unallocated());
    }
}
