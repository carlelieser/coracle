//! Scalar floating-point: `op0 = x111` with bit 28 clear and bit 30 clear.
//!
//! The family is chosen by bit 21 and then by bits 11..10, exactly as the ARM
//! ARM's "Floating-point data-processing" tables do: `11` is one source, `10`
//! two sources, `01` the conditional compares, `00` the compares and the
//! conversions.

use super::super::super::instruction::{unallocated, Form, Instruction};
use super::super::super::op::Op;
use super::super::super::operand::{Cond, ElemSize, RegWidth, RoundMode, VecOperand, VecShape};
use super::super::bits;
use crate::reg::{Gpr, Vec};

/// Decodes the 2-bit `ftype` field, which is not [`ElemSize::from_size`]'s
/// encoding: `10` is double and `11` is half.
fn float_size(ftype: u32) -> Option<ElemSize> {
    match ftype {
        0b00 => Some(ElemSize::S32),
        0b01 => Some(ElemSize::D64),
        0b11 => Some(ElemSize::H16),
        _ => None,
    }
}

/// A scalar operand of the given width.
fn scalar(index: u32, size: ElemSize) -> VecOperand {
    VecOperand {
        reg: Vec::new(index as u8),
        shape: VecShape::Scalar(size),
    }
}

/// The `Rd`, `Rn`, `Rm` and `Ra` register fields.
fn fields(encoding: u32) -> (u32, u32, u32, u32) {
    (
        bits(encoding, 4, 0),
        bits(encoding, 9, 5),
        bits(encoding, 20, 16),
        bits(encoding, 14, 10),
    )
}

/// `op0 = x111`, bit 28 clear — scalar FP, including the conversions to and
/// from the general-purpose registers.
pub fn data_processing(encoding: u32) -> Instruction {
    // S = 1 is unallocated across the whole group. Bit 31 is not reserved: it
    // is `sf` for the conversions to and from a general-purpose register, and
    // is checked there. For every other family it must be clear.
    if bits(encoding, 29, 29) == 1 {
        return unallocated(encoding);
    }
    // Bit 24 separates the three-source group (FMADD and friends) from the
    // rest, and it is the only family that uses `Ra`.
    let is_three_source = bits(encoding, 24, 24) == 1;
    // With bit 24 clear, bit 21 clear is the fixed-point conversion family and
    // bits 15..10 all clear alongside bit 21 set is the integer one. Both read
    // bit 31 as `sf`; every other family requires it clear.
    let is_fixed_point = !is_three_source && bits(encoding, 21, 21) == 0;
    let is_integer_conversion =
        !is_three_source && bits(encoding, 21, 21) == 1 && bits(encoding, 15, 10) == 0;

    if bits(encoding, 31, 31) == 1 && !(is_fixed_point || is_integer_conversion) {
        return unallocated(encoding);
    }
    // Both conversion families are decoded before `ftype` is resolved: the
    // high-half FMOV uses ftype = 10, which names no format and so would
    // otherwise be rejected as an unallocated size.
    if is_fixed_point {
        return fixed_point_conversion(encoding);
    }
    if is_integer_conversion {
        return integer_conversion(encoding);
    }

    let Some(size) = float_size(bits(encoding, 23, 22)) else {
        return unallocated(encoding);
    };
    if is_three_source {
        return three_source(encoding, size);
    }

    match bits(encoding, 11, 10) {
        0b00 => compare_or_one_source(encoding, size),
        0b01 => conditional_compare(encoding, size),
        0b10 => two_source(encoding, size),
        _ => conditional_select(encoding, size),
    }
}

/// `FMADD`, `FMSUB`, `FNMADD`, `FNMSUB` — the only forms with four registers.
fn three_source(encoding: u32, size: ElemSize) -> Instruction {
    let (rd, rn, rm, ra) = fields(encoding);
    // o1 selects the negated pair, o0 the subtracting one.
    let op = match (bits(encoding, 21, 21), bits(encoding, 15, 15)) {
        (0, 0) => Op::Fmadd,
        (0, 1) => Op::Fmsub,
        (1, 0) => Op::Fnmadd,
        _ => Op::Fnmsub,
    };

    let form = Form::VecData {
        vd: scalar(rd, size),
        vn: scalar(rn, size),
        vm: Some(scalar(rm, size)),
        va: Some(scalar(ra, size)),
    };
    Instruction::new(encoding, op, form)
}

/// Two-source arithmetic: `FMUL`, `FDIV`, `FADD`, `FSUB`, the extrema, and
/// `FNMUL`.
fn two_source(encoding: u32, size: ElemSize) -> Instruction {
    let (rd, rn, rm, _) = fields(encoding);
    let op = match bits(encoding, 15, 12) {
        0b0000 => Op::Fmul,
        0b0001 => Op::Fdiv,
        0b0010 => Op::Fadd,
        0b0011 => Op::Fsub,
        0b0100 => Op::Fmax,
        0b0101 => Op::Fmin,
        0b0110 => Op::Fmaxnm,
        0b0111 => Op::Fminnm,
        0b1000 => Op::Fnmul,
        _ => return unallocated(encoding),
    };

    let form = Form::VecData {
        vd: scalar(rd, size),
        vn: scalar(rn, size),
        vm: Some(scalar(rm, size)),
        va: None,
    };
    Instruction::new(encoding, op, form)
}

/// `FCSEL`.
fn conditional_select(encoding: u32, size: ElemSize) -> Instruction {
    let (rd, rn, rm, _) = fields(encoding);
    let form = Form::VecCond {
        vd: scalar(rd, size),
        vn: scalar(rn, size),
        vm: scalar(rm, size),
        cond: Cond::from_bits(bits(encoding, 15, 12) as u8),
    };
    Instruction::new(encoding, Op::Fcsel, form)
}

/// `FCCMP` and `FCCMPE`.
fn conditional_compare(encoding: u32, size: ElemSize) -> Instruction {
    let (nzcv, rn, rm, _) = fields(encoding);
    // Bit 4 selects the signalling form; the other four bits are the NZCV to
    // substitute, so they are not a register.
    let op = if bits(encoding, 4, 4) == 1 {
        Op::Fccmpe
    } else {
        Op::Fccmp
    };

    let form = Form::VecCondCompare {
        vn: scalar(rn, size),
        vm: scalar(rm, size),
        nzcv: (nzcv & 0b1111) as u8,
        cond: Cond::from_bits(bits(encoding, 15, 12) as u8),
    };
    Instruction::new(encoding, op, form)
}

/// Bits 11..10 = `00`: a compare, a one-source operation, or the immediate
/// move, split by bits 14..10.
///
/// The three families sit at `10000`, `01000` and `00100` — a one-hot field, so
/// the bit that is set names the family.
fn compare_or_one_source(encoding: u32, size: ElemSize) -> Instruction {
    match bits(encoding, 14, 10) {
        0b10000 => one_source(encoding, size),
        0b01000 => compare(encoding, size),
        0b00100 => move_immediate(encoding, size),
        _ => unallocated(encoding),
    }
}

/// `FCMP`, `FCMPE`, and their compare-with-zero forms.
fn compare(encoding: u32, size: ElemSize) -> Instruction {
    let (opcode2, rn, rm, _) = fields(encoding);
    // `opcode2` occupies the Rd slot: bit 4 selects the compare-with-zero form
    // and bit 3 the signalling one, so neither is a destination register.
    // Reading those five bits as an Rd would invent a write.
    let is_zero_form = opcode2 & 0b1000 != 0;
    let op = if opcode2 & 0b1_0000 != 0 {
        Op::Fcmpe
    } else {
        Op::Fcmp
    };
    if opcode2 & 0b111 != 0 {
        return unallocated(encoding);
    }
    // The zero forms name no second register, so a non-zero Rm is unallocated.
    if is_zero_form && rm != 0 {
        return unallocated(encoding);
    }

    let form = Form::VecCompare {
        vn: scalar(rn, size),
        vm: (!is_zero_form).then(|| scalar(rm, size)),
    };
    Instruction::new(encoding, op, form)
}

/// `FMOV #imm` — the 8-bit immediate expands to a full float.
fn move_immediate(encoding: u32, size: ElemSize) -> Instruction {
    let rd = bits(encoding, 4, 0);
    // Bits 9..5 are RES0 in this encoding, below the immediate.
    if bits(encoding, 9, 5) != 0 {
        return unallocated(encoding);
    }

    let imm8 = bits(encoding, 20, 13) as u8;
    let vd = scalar(rd, size);
    let form = Form::VecImm {
        vd,
        vn: vd,
        imm: expand_float_immediate(imm8, size),
    };
    Instruction::new(encoding, Op::FmovImm, form)
}

/// `VFPExpandImm`: an 8-bit `abcdefgh` becomes a float with a 4-bit mantissa
/// and an exponent formed from `b` repeated.
fn expand_float_immediate(imm8: u8, size: ElemSize) -> u64 {
    let sign = (imm8 >> 7) as u64;
    let exponent_high = ((imm8 >> 6) & 1) as u64;
    let exponent_low = ((imm8 >> 4) & 0b11) as u64;
    let mantissa = (imm8 & 0b1111) as u64;

    let (exponent_bits, mantissa_bits) = match size {
        ElemSize::H16 => (5, 10),
        ElemSize::D64 => (11, 52),
        _ => (8, 23),
    };
    // The exponent is `NOT(b) : b repeated : cd`, which is what makes the
    // immediate span a small range around 1.0 rather than the whole exponent.
    let repeated = if exponent_high == 1 {
        (1u64 << (exponent_bits - 3)) - 1
    } else {
        0
    };
    let exponent = ((1 - exponent_high) << (exponent_bits - 1)) | (repeated << 2) | exponent_low;

    (sign << (exponent_bits + mantissa_bits))
        | (exponent << mantissa_bits)
        | (mantissa << (mantissa_bits - 4))
}

/// One-source: `FMOV`, `FABS`, `FNEG`, `FSQRT`, `FCVT` and the `FRINT` family.
fn one_source(encoding: u32, size: ElemSize) -> Instruction {
    let (rd, rn, _, _) = fields(encoding);
    let opcode = bits(encoding, 20, 15);

    let (op, round, target) = match opcode {
        0b000000 => (Op::Fmov, RoundMode::Current, None),
        0b000001 => (Op::Fabs, RoundMode::Current, None),
        0b000010 => (Op::Fneg, RoundMode::Current, None),
        0b000011 => (Op::Fsqrt, RoundMode::Current, None),
        // The convert forms name their target in the low two bits of the
        // opcode, and converting to the source's own size is unallocated.
        0b000100 | 0b000101 | 0b000111 => {
            let Some(target) = float_size(opcode & 0b11) else {
                return unallocated(encoding);
            };
            if target == size {
                return unallocated(encoding);
            }
            (Op::Fcvt, RoundMode::Current, Some(target))
        }
        0b001000 => (Op::Frint, RoundMode::Nearest, None),
        0b001001 => (Op::Frint, RoundMode::Plus, None),
        0b001010 => (Op::Frint, RoundMode::Minus, None),
        0b001011 => (Op::Frint, RoundMode::Zero, None),
        0b001100 => (Op::Frint, RoundMode::NearestAway, None),
        // FRINTX and FRINTI both round in FPCR's mode; they differ only in
        // whether an inexact result raises, which is not a decode question.
        0b001110 | 0b001111 => (Op::Frint, RoundMode::Current, None),
        _ => return unallocated(encoding),
    };

    let form = Form::VecData {
        vd: scalar(rd, target.unwrap_or(size)),
        vn: scalar(rn, size),
        vm: None,
        va: None,
    };
    Instruction::new(encoding, op, form).with_round(round)
}

/// What a conversion's `rmode:opcode` names.
struct ConversionKind {
    op: Op,
    round: RoundMode,
    /// Whether the general-purpose register is the destination.
    is_to_gpr: bool,
}

/// Decodes `rmode` and `opcode` together.
///
/// `opcode` 000/001 are the FP-to-integer conversions and take their rounding
/// from `rmode` — so `FCVTZS` is `rmode = 11` rather than an opcode of its own,
/// and `FCVTNS`, `FCVTPS`, `FCVTMS` are the other three values. `FCVTAS` and
/// `FCVTAU` round to nearest-with-ties-away, which `rmode` cannot express, so
/// those get opcodes 100/101 instead. Everything else ignores `rmode`.
fn conversion_kind(rmode: u32, opcode: u32) -> Option<ConversionKind> {
    let signed = |is_signed| if is_signed { Op::Fcvts } else { Op::Fcvtu };
    // The round-toward-zero conversions have their own opcodes because the
    // interpreter reaches them from the fixed-point forms too.
    let toward_zero = |is_signed| if is_signed { Op::Fcvtzs } else { Op::Fcvtzu };

    match (rmode, opcode) {
        // Round toward zero is named often enough to warrant its own opcode
        // rather than an Fcvts carrying RoundMode::Zero.
        (0b11, 0b000 | 0b001) => Some(ConversionKind {
            op: toward_zero(opcode == 0b000),
            round: RoundMode::Zero,
            is_to_gpr: true,
        }),
        (_, 0b000 | 0b001) => Some(ConversionKind {
            op: signed(opcode == 0b000),
            round: match rmode {
                0b00 => RoundMode::Nearest,
                0b01 => RoundMode::Plus,
                _ => RoundMode::Minus,
            },
            is_to_gpr: true,
        }),
        (0b00, 0b010 | 0b011) => Some(ConversionKind {
            op: if opcode == 0b010 {
                Op::Scvtf
            } else {
                Op::Ucvtf
            },
            round: RoundMode::Current,
            is_to_gpr: false,
        }),
        (0b00, 0b100 | 0b101) => Some(ConversionKind {
            op: signed(opcode == 0b100),
            round: RoundMode::NearestAway,
            is_to_gpr: true,
        }),
        // rmode 00 is the scalar transfer; rmode 01 the high-half one, which
        // only exists for the 128-bit form.
        (0b00 | 0b01, 0b110 | 0b111) => Some(ConversionKind {
            op: if opcode == 0b110 {
                Op::FmovFromVec
            } else {
                Op::FmovToVec
            },
            round: RoundMode::Current,
            is_to_gpr: opcode == 0b110,
        }),
        _ => None,
    }
}

/// `FCVTZS`/`SCVTF` and friends between a scalar and a general-purpose
/// register.
fn integer_conversion(encoding: u32) -> Instruction {
    let (rd, rn, _, _) = fields(encoding);
    let width = RegWidth::from_sf(bits(encoding, 31, 31) == 1);
    let rmode = bits(encoding, 20, 19);
    let opcode = bits(encoding, 18, 16);

    let Some(kind) = conversion_kind(rmode, opcode) else {
        return unallocated(encoding);
    };
    // The FMOV transfers are shaped differently and accept the ftype the other
    // conversions reject, so they resolve their own operand size.
    if matches!(kind.op, Op::FmovFromVec | Op::FmovToVec) {
        return move_between_files(encoding, width, kind);
    }

    let Some(size) = float_size(bits(encoding, 23, 22)) else {
        return unallocated(encoding);
    };

    let (gpr, vec) = if kind.is_to_gpr {
        (Gpr::from_index_zr(rd as u8), scalar(rn, size))
    } else {
        (Gpr::from_index_zr(rn as u8), scalar(rd, size))
    };
    let form = Form::VecGprMove { gpr, vec };
    Instruction::new(encoding, kind.op, form)
        .with_width(width)
        .with_round(kind.round)
}

/// `FMOV` between a general-purpose register and a scalar or a vector's high
/// half.
fn move_between_files(encoding: u32, width: RegWidth, kind: ConversionKind) -> Instruction {
    let (rd, rn, _, _) = fields(encoding);
    // rmode = 01 names the high half of a Q register, which exists only in the
    // 64-bit form and only with ftype = 10. Every other transfer is the plain
    // scalar one, whose two widths must agree: there is no encoding moving a
    // W register to or from a D scalar.
    let is_high_half = bits(encoding, 20, 19) == 0b01;
    if is_high_half && (width != RegWidth::X64 || bits(encoding, 23, 22) != 0b10) {
        return unallocated(encoding);
    }

    let size = match float_size(bits(encoding, 23, 22)) {
        Some(size) if is_high_half || (size == ElemSize::D64) == (width == RegWidth::X64) => size,
        // ftype = 10 is legal only for the high-half form, and a W register
        // never pairs with a D scalar.
        _ if !is_high_half => return unallocated(encoding),
        _ => ElemSize::D64,
    };

    let shape = if is_high_half {
        VecShape::Element {
            elem: ElemSize::D64,
            index: 1,
        }
    } else {
        VecShape::Scalar(size)
    };
    let (gpr_index, vec_index) = if kind.is_to_gpr { (rd, rn) } else { (rn, rd) };

    let form = Form::VecGprMove {
        gpr: Gpr::from_index_zr(gpr_index as u8),
        vec: VecOperand {
            reg: Vec::new(vec_index as u8),
            shape,
        },
    };
    Instruction::new(encoding, kind.op, form).with_width(width)
}

/// The fixed-point conversions, which scale by `2^fbits` around the
/// conversion.
fn fixed_point_conversion(encoding: u32) -> Instruction {
    let Some(size) = float_size(bits(encoding, 23, 22)) else {
        return unallocated(encoding);
    };
    let (rd, rn, _, _) = fields(encoding);
    let width = RegWidth::from_sf(bits(encoding, 31, 31) == 1);
    let scale = bits(encoding, 15, 10);
    // `scale` counts down from 64, and a 32-bit form cannot name more than 31
    // fractional bits.
    let fractional_bits = 64 - scale;
    if width == RegWidth::W32 && fractional_bits > 32 {
        return unallocated(encoding);
    }

    let rmode = bits(encoding, 20, 19);
    let opcode = bits(encoding, 18, 16);
    // Only the round-toward-zero conversions and the integer-to-FP pair have
    // fixed-point forms.
    let (op, is_to_gpr) = match (rmode, opcode) {
        (0b11, 0b000) => (Op::Fcvtzs, true),
        (0b11, 0b001) => (Op::Fcvtzu, true),
        (0b00, 0b010) => (Op::Scvtf, false),
        (0b00, 0b011) => (Op::Ucvtf, false),
        _ => return unallocated(encoding),
    };

    let (gpr, vec) = if is_to_gpr {
        (Gpr::from_index_zr(rd as u8), scalar(rn, size))
    } else {
        (Gpr::from_index_zr(rn as u8), scalar(rd, size))
    };
    let form = Form::VecGprMove { gpr, vec };
    Instruction::new(encoding, op, form)
        .with_width(width)
        .with_round(RoundMode::Zero)
}
