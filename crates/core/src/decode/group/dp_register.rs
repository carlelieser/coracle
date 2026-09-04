//! Owned by the integer slice.

use super::super::instruction::{unallocated, Form, Instruction};
use super::super::op::Op;
use super::super::operand::{Cond, ExtendKind, ExtendedReg, RegWidth, ShiftKind, ShiftedReg};
use super::bits;
use crate::reg::Gpr;

/// `op0 = x101` — data processing (register).
///
/// Owned by the integer slice. Dispatch follows the ARM ARM's "Data Processing
/// -- Register" table: `op1` (bit 28) picks the half, then `op2` (bits 24..21)
/// the row.
pub fn data_processing_register(encoding: u32) -> Instruction {
    let op1 = bits(encoding, 28, 28);
    let op2 = bits(encoding, 24, 21);

    match (op1, op2) {
        // Logical (shifted register) — op2 = 0xxx with op1 = 0.
        (0, 0b0000..=0b0111) => logical_shifted(encoding),
        // Add/sub (shifted register) — op2 = 1xx0.
        (0, 0b1000 | 0b1010 | 0b1100 | 0b1110) => add_sub_shifted(encoding),
        // Add/sub (extended register) — op2 = 1xx1.
        (0, 0b1001 | 0b1011 | 0b1101 | 0b1111) => add_sub_extended(encoding),
        (1, 0b0000) => add_sub_with_carry(encoding),
        (1, 0b0010) => conditional_compare(encoding),
        (1, 0b0100) => conditional_select(encoding),
        (1, 0b0110) => two_source_or_one_source(encoding),
        (1, 0b1000..=0b1111) => three_source(encoding),
        // op2 = 0001 and 0011 are rotate-right-into-flags and evaluate-into-
        // flags, both FEAT_FlagM; 0101 is unallocated. None is advertised.
        _ => unallocated(encoding),
    }
}

/// Logical (shifted register): `AND`, `BIC`, `ORR`, `ORN`, `EOR`, `EON`.
///
/// `MOV` (register), `MVN` and `TST` are aliases of `ORR`, `ORN` and `ANDS`
/// with the zero register in one slot, so they need no decoding of their own.
fn logical_shifted(encoding: u32) -> Instruction {
    let width = RegWidth::from_sf(bits(encoding, 31, 31) == 1);
    let amount = bits(encoding, 15, 10) as u8;
    // The shift amount indexes a bit of the operand, so bit 5 of it is
    // unallocated in the 32-bit form.
    if width == RegWidth::W32 && amount & 0b10_0000 != 0 {
        return unallocated(encoding);
    }

    let opc = bits(encoding, 30, 29);
    let is_negated = bits(encoding, 21, 21) == 1;
    let op = match (opc, is_negated) {
        (0b00, false) | (0b11, false) => Op::And,
        (0b00, true) | (0b11, true) => Op::Bic,
        (0b01, false) => Op::Orr,
        (0b01, true) => Op::Orn,
        (0b10, false) => Op::Eor,
        _ => Op::Eon,
    };

    let form = Form::RegShifted {
        rd: Gpr::from_index_zr(bits(encoding, 4, 0) as u8),
        rn: Gpr::from_index_zr(bits(encoding, 9, 5) as u8),
        rm: ShiftedReg {
            reg: Gpr::from_index_zr(bits(encoding, 20, 16) as u8),
            // The logical group is the only one that allows `ROR`.
            kind: shift_kind(bits(encoding, 23, 22)),
            amount,
        },
    };

    let insn = Instruction::new(encoding, op, form).with_width(width);
    // Only `ANDS`, opc = 11, sets flags.
    if opc == 0b11 {
        insn.setting_flags()
    } else {
        insn
    }
}

/// Add/sub (shifted register).
fn add_sub_shifted(encoding: u32) -> Instruction {
    let width = RegWidth::from_sf(bits(encoding, 31, 31) == 1);
    let shift = bits(encoding, 23, 22);
    let amount = bits(encoding, 15, 10) as u8;
    // The arithmetic group has no `ROR`, and the shift amount cannot index
    // bit 32 or above at 32 bits.
    if shift == 0b11 || (width == RegWidth::W32 && amount & 0b10_0000 != 0) {
        return unallocated(encoding);
    }

    let form = Form::RegShifted {
        rd: Gpr::from_index_zr(bits(encoding, 4, 0) as u8),
        rn: Gpr::from_index_zr(bits(encoding, 9, 5) as u8),
        rm: ShiftedReg {
            reg: Gpr::from_index_zr(bits(encoding, 20, 16) as u8),
            kind: shift_kind(shift),
            amount,
        },
    };
    finish_arithmetic(encoding, form, width)
}

/// Add/sub (extended register).
///
/// This is the only add/sub form that can name `SP` as a source, which is why
/// `CMP SP, X0` assembles to it rather than to the shifted form.
fn add_sub_extended(encoding: u32) -> Instruction {
    let width = RegWidth::from_sf(bits(encoding, 31, 31) == 1);
    let amount = bits(encoding, 12, 10) as u8;
    // `opt` must be zero, and the post-extension left shift is limited to 0-4.
    if bits(encoding, 23, 22) != 0 || amount > 4 {
        return unallocated(encoding);
    }
    let sets_flags = bits(encoding, 29, 29) == 1;

    // Rn is always SP-capable here; Rd is SP-capable only when flags are not
    // set, which is what separates `ADD SP, ...` from `CMP SP, ...`.
    let rd = if sets_flags {
        Gpr::from_index_zr(bits(encoding, 4, 0) as u8)
    } else {
        Gpr::from_index_sp(bits(encoding, 4, 0) as u8)
    };
    let form = Form::RegExtended {
        rd,
        rn: Gpr::from_index_sp(bits(encoding, 9, 5) as u8),
        rm: ExtendedReg {
            // Rm is a data operand, never SP, whatever the extension.
            reg: Gpr::from_index_zr(bits(encoding, 20, 16) as u8),
            kind: ExtendKind::from_option(bits(encoding, 15, 13) as u8),
            amount,
        },
    };
    finish_arithmetic(encoding, form, width)
}

/// Add/sub with carry: `ADC`, `ADCS`, `SBC`, `SBCS`.
fn add_sub_with_carry(encoding: u32) -> Instruction {
    // The whole `Rm`-adjacent field is fixed at zero in this encoding.
    if bits(encoding, 15, 10) != 0 {
        return unallocated(encoding);
    }
    let width = RegWidth::from_sf(bits(encoding, 31, 31) == 1);
    let form = Form::RegShifted {
        rd: Gpr::from_index_zr(bits(encoding, 4, 0) as u8),
        rn: Gpr::from_index_zr(bits(encoding, 9, 5) as u8),
        rm: ShiftedReg {
            reg: Gpr::from_index_zr(bits(encoding, 20, 16) as u8),
            // There is no shift field; `LSL #0` is the identity.
            kind: ShiftKind::Lsl,
            amount: 0,
        },
    };

    let op = if bits(encoding, 30, 30) == 1 {
        Op::Sbc
    } else {
        Op::Adc
    };
    let insn = Instruction::new(encoding, op, form).with_width(width);
    if bits(encoding, 29, 29) == 1 {
        insn.setting_flags()
    } else {
        insn
    }
}

/// Conditional compare, register and immediate forms.
///
/// The register form is rewritten into [`Form::CondSelect`] rather than given a
/// form of its own: it has the same four operands, and `Rd` is unused because
/// the result is NZCV. That is what `Form::CondCompare`'s doc comment already
/// says the decoder does.
fn conditional_compare(encoding: u32) -> Instruction {
    // o2 (bit 10) and o3 (bit 4) are fixed at zero, and S (bit 29) at one:
    // both `CCMP` and `CCMN` always write flags.
    if bits(encoding, 29, 29) != 1 || bits(encoding, 10, 10) != 0 || bits(encoding, 4, 4) != 0 {
        return unallocated(encoding);
    }

    let width = RegWidth::from_sf(bits(encoding, 31, 31) == 1);
    let cond = Cond::from_bits(bits(encoding, 15, 12) as u8);
    let nzcv = bits(encoding, 3, 0) as u8;
    let rn = Gpr::from_index_zr(bits(encoding, 9, 5) as u8);
    let is_immediate = bits(encoding, 11, 11) == 1;

    let form = if is_immediate {
        Form::CondCompare {
            rn,
            imm: bits(encoding, 20, 16) as u64,
            nzcv,
            cond,
        }
    } else {
        Form::CondSelect {
            // The comparison has no destination; the zero register is the only
            // honest thing to put here, and a write to it is discarded.
            rd: Gpr::ZR,
            rn,
            rm: Gpr::from_index_zr(bits(encoding, 20, 16) as u8),
            cond,
        }
    };

    let op = if bits(encoding, 30, 30) == 1 {
        Op::Ccmp
    } else {
        Op::Ccmn
    };
    Instruction::new(encoding, op, form)
        .with_width(width)
        .setting_flags()
}

/// Conditional select: `CSEL`, `CSINC`, `CSINV`, `CSNEG`.
///
/// `CSET`, `CSETM`, `CINC`, `CINV` and `CNEG` are aliases of these with an
/// inverted condition or the zero register, so they decode as the base opcode.
fn conditional_select(encoding: u32) -> Instruction {
    // S (bit 29) is fixed at zero: none of these writes flags.
    if bits(encoding, 29, 29) != 0 {
        return unallocated(encoding);
    }
    let op = match (bits(encoding, 30, 30), bits(encoding, 11, 10)) {
        (0, 0b00) => Op::Csel,
        (0, 0b01) => Op::Csinc,
        (1, 0b00) => Op::Csinv,
        (1, 0b01) => Op::Csneg,
        _ => return unallocated(encoding),
    };

    let form = Form::CondSelect {
        rd: Gpr::from_index_zr(bits(encoding, 4, 0) as u8),
        rn: Gpr::from_index_zr(bits(encoding, 9, 5) as u8),
        rm: Gpr::from_index_zr(bits(encoding, 20, 16) as u8),
        cond: Cond::from_bits(bits(encoding, 15, 12) as u8),
    };
    Instruction::new(encoding, op, form).with_width(RegWidth::from_sf(bits(encoding, 31, 31) == 1))
}

/// The `op2 = 0110` row, which holds both the two-source and one-source groups.
///
/// Bit 30 splits them: the architecture reuses the row rather than spending an
/// `op2` value on a group with six members.
fn two_source_or_one_source(encoding: u32) -> Instruction {
    // S (bit 29) is fixed at zero throughout both groups.
    if bits(encoding, 29, 29) != 0 {
        return unallocated(encoding);
    }
    if bits(encoding, 30, 30) == 1 {
        one_source(encoding)
    } else {
        two_source(encoding)
    }
}

/// Data processing (2 source): the variable shifts and the divides.
///
/// `LSL`, `LSR`, `ASR` and `ROR` in their register-operand spelling are aliases
/// of `LSLV` and friends, so they carry no separate opcode.
fn two_source(encoding: u32) -> Instruction {
    let width = RegWidth::from_sf(bits(encoding, 31, 31) == 1);
    let op = match bits(encoding, 15, 10) {
        0b000010 => Op::Udiv,
        0b000011 => Op::Sdiv,
        0b001000 => Op::Lslv,
        0b001001 => Op::Lsrv,
        0b001010 => Op::Asrv,
        0b001011 => Op::Rorv,
        // The remaining opcodes are CRC32 (FEAT_CRC32), the pointer-
        // authentication `PACGA`, and the FEAT_MTE subtract-pointer forms.
        // None is advertised (docs/machine-spec.md §2).
        _ => return unallocated(encoding),
    };

    let form = Form::RegShifted {
        rd: Gpr::from_index_zr(bits(encoding, 4, 0) as u8),
        rn: Gpr::from_index_zr(bits(encoding, 9, 5) as u8),
        rm: ShiftedReg {
            reg: Gpr::from_index_zr(bits(encoding, 20, 16) as u8),
            // The shift amount is the register operand's *value*, not a
            // constant, so the encoding has no shift field of its own.
            kind: ShiftKind::Lsl,
            amount: 0,
        },
    };
    Instruction::new(encoding, op, form).with_width(width)
}

/// Data processing (1 source): `RBIT`, `REV16`, `REV32`, `REV`, `CLZ`, `CLS`.
///
/// These have one source, but no [`Form`] exists for that shape and adding one
/// would be a change to a file this slice does not own. They use
/// [`Form::RegShifted`] with `rm` repeating `rn`, which is what the
/// bitfield group already does for its unused second source.
fn one_source(encoding: u32) -> Instruction {
    let width = RegWidth::from_sf(bits(encoding, 31, 31) == 1);
    // `opcode2`, bits 20..16, selects the sub-group; only zero is allocated
    // outside pointer authentication, which is not advertised.
    if bits(encoding, 20, 16) != 0 {
        return unallocated(encoding);
    }

    let op = match bits(encoding, 15, 10) {
        0b000000 => Op::Rbit,
        0b000001 => Op::Rev16,
        // `REV32` at 64 bits and `REV` at 32 bits share opcode 000010: a W
        // register's whole-register reverse *is* a 32-bit reverse.
        0b000010 if width == RegWidth::X64 => Op::Rev32,
        0b000010 => Op::Rev,
        // Opcode 000011 is the 64-bit whole-register reverse; there is no
        // 32-bit encoding of it, because 000010 already covers that case.
        0b000011 if width == RegWidth::X64 => Op::Rev,
        0b000100 => Op::Clz,
        0b000101 => Op::Cls,
        _ => return unallocated(encoding),
    };

    let rn = Gpr::from_index_zr(bits(encoding, 9, 5) as u8);
    let form = Form::RegShifted {
        rd: Gpr::from_index_zr(bits(encoding, 4, 0) as u8),
        rn,
        rm: ShiftedReg {
            reg: rn,
            kind: ShiftKind::Lsl,
            amount: 0,
        },
    };
    Instruction::new(encoding, op, form).with_width(width)
}

/// Data processing (3 source): the multiply-accumulate family.
///
/// `MUL`, `MNEG`, `SMULL`, `UMULL`, `SMNEGL` and `UMNEGL` are aliases with the
/// zero register as the addend, so they decode as the accumulating opcode.
fn three_source(encoding: u32) -> Instruction {
    let width = RegWidth::from_sf(bits(encoding, 31, 31) == 1);
    // `op54`, bits 30..29, is zero for every allocated three-source encoding.
    if bits(encoding, 30, 29) != 0 {
        return unallocated(encoding);
    }
    let op31 = bits(encoding, 23, 21);
    let o0 = bits(encoding, 15, 15);

    let op = match (width, op31, o0) {
        (_, 0b000, 0) => Op::Madd,
        (_, 0b000, _) => Op::Msub,
        // The widening and high-half forms exist only at 64 bits: they read
        // W registers and write an X, which a 32-bit encoding cannot express.
        (RegWidth::X64, 0b001, 0) => Op::Smaddl,
        (RegWidth::X64, 0b001, _) => Op::Smsubl,
        (RegWidth::X64, 0b010, 0) => Op::Smulh,
        (RegWidth::X64, 0b101, 0) => Op::Umaddl,
        (RegWidth::X64, 0b101, _) => Op::Umsubl,
        (RegWidth::X64, 0b110, 0) => Op::Umulh,
        _ => return unallocated(encoding),
    };
    // `SMULH` and `UMULH` have no addend. Their `Ra` field is "should be one"
    // rather than reserved, so a non-31 value is constrained-unpredictable,
    // not unallocated: it decodes, and the interpreter ignores `ra`.
    let form = Form::ThreeSource {
        rd: Gpr::from_index_zr(bits(encoding, 4, 0) as u8),
        rn: Gpr::from_index_zr(bits(encoding, 9, 5) as u8),
        rm: Gpr::from_index_zr(bits(encoding, 20, 16) as u8),
        ra: Gpr::from_index_zr(bits(encoding, 14, 10) as u8),
    };
    // The widening forms name X operands even though Rn and Rm are read as W:
    // the multiplicand width is implied by the opcode, not by `sf`.
    Instruction::new(encoding, op, form).with_width(width)
}

/// Shared tail of the two add/sub register forms.
fn finish_arithmetic(encoding: u32, form: Form, width: RegWidth) -> Instruction {
    let op = if bits(encoding, 30, 30) == 1 {
        Op::Sub
    } else {
        Op::Add
    };
    let insn = Instruction::new(encoding, op, form).with_width(width);
    if bits(encoding, 29, 29) == 1 {
        insn.setting_flags()
    } else {
        insn
    }
}

const fn shift_kind(field: u32) -> ShiftKind {
    match field & 0b11 {
        0b00 => ShiftKind::Lsl,
        0b01 => ShiftKind::Lsr,
        0b10 => ShiftKind::Asr,
        _ => ShiftKind::Ror,
    }
}
