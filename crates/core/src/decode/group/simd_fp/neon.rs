//! Advanced SIMD — the subset `docs/plan.md` §M1 calls for.
//!
//! Coverage is deliberately partial: the plan says to implement NEON lazily,
//! driven by a trap-and-log on unimplemented opcodes, so what is here is what
//! musl and glibc's `memcpy`, `memset`, `strlen` and `memchr` reach for, plus
//! V8's baseline. Anything else returns `unallocated` and faults, which is what
//! feeds the log that drives the next increment.
//!
//! The crypto encodings that share this space stay unallocated permanently:
//! `docs/machine-spec.md` §2 advertises no AES, SHA or PMULL, so they must
//! fault exactly as an unimplemented encoding does.

use super::super::super::instruction::{unallocated, Form, Instruction};
use super::super::super::op::Op;
use super::super::super::operand::{ElemSize, VecHalf, VecOperand, VecShape};
use super::super::bits;
use crate::reg::{Gpr, Vec};

/// The `Rd`, `Rn` and `Rm` register fields.
fn fields(encoding: u32) -> (u32, u32, u32) {
    (
        bits(encoding, 4, 0),
        bits(encoding, 9, 5),
        bits(encoding, 20, 16),
    )
}

/// Whether the `Q` bit selects the full 128-bit register.
fn is_quad(encoding: u32) -> bool {
    bits(encoding, 30, 30) == 1
}

/// A vector operand of `elem` lanes filling the selected width.
fn vector(index: u32, elem: ElemSize, is_quad: bool) -> VecOperand {
    let lanes = (if is_quad { 128 } else { 64 }) / elem.bits();
    VecOperand {
        reg: Vec::new(index as u8),
        shape: VecShape::Vector {
            elem,
            count: lanes as u8,
            half: if is_quad { VecHalf::Full } else { VecHalf::Low },
        },
    }
}

/// A single addressed lane.
fn element(index: u32, elem: ElemSize, lane: u8) -> VecOperand {
    VecOperand {
        reg: Vec::new(index as u8),
        shape: VecShape::Element { elem, index: lane },
    }
}

/// Advanced SIMD: `op0 = x111` with bit 28 clear.
///
/// Families are separated by bits 31, 24..21 and 15..10, following the ARM
/// ARM's "Advanced SIMD" tables. Anything this slice has not claimed falls
/// through to `unallocated`, which is the trap the lazy NEON plan feeds on.
pub fn data_processing(encoding: u32) -> Instruction {
    // Bit 31 is RES0 across every encoding this slice claims.
    if bits(encoding, 31, 31) == 1 {
        return unallocated(encoding);
    }
    if bits(encoding, 28, 24) == 0b01111 {
        return immediate_or_shift(encoding);
    }
    if bits(encoding, 28, 24) != 0b01110 {
        return unallocated(encoding);
    }

    if bits(encoding, 21, 21) == 0 {
        return bit21_clear(encoding);
    }
    bit21_set(encoding)
}

/// Bit 21 clear: the copy group, `TBL`/`TBX`, `EXT`, and the permutes.
///
/// All four share bit 21 and are told apart by bits 15 and 11..10: the copy
/// group has bit 10 set, `EXT` bit 15 set, and the other two sit at bits
/// 11..10 of `00` and `10`.
fn bit21_clear(encoding: u32) -> Instruction {
    // The copy group is the only one here with bit 10 set.
    if bits(encoding, 10, 10) == 1 {
        return copy(encoding);
    }
    if bits(encoding, 11, 10) == 0b00 {
        return table_lookup(encoding);
    }
    if bits(encoding, 11, 10) != 0b10 {
        return unallocated(encoding);
    }

    // EXT and the permutes share bits 11..10 = 10. `op` — bit 29 — set is EXT,
    // which also requires op2 in bits 23..22 to be zero.
    if bits(encoding, 29, 29) == 1 {
        if bits(encoding, 23, 22) != 0b00 {
            return unallocated(encoding);
        }
        return extract(encoding);
    }
    permute(encoding)
}

/// Bit 21 set: the three-register-same group, the reductions and the
/// two-register misc family.
fn bit21_set(encoding: u32) -> Instruction {
    // The reductions and the two-register misc group both sit at bits 11..10 =
    // 10 with bits 20..17 clear; bit 16 separates them, since a reduction's
    // opcode field starts 1_ and the misc group's 0_.
    if bits(encoding, 11, 10) == 0b10 {
        // The reductions carry `1000` in bits 20..17 and the two-register misc
        // group `0000`; nothing else in this space has bit 10 clear.
        match bits(encoding, 20, 17) {
            0b1000 => return across_lanes(encoding),
            0b0000 => return two_register(encoding),
            _ => {}
        }
    }

    // The logical operations share one opcode and read `size` as the
    // operation rather than as an element width.
    if bits(encoding, 15, 10) == 0b000111 {
        return three_same_logical(encoding);
    }
    if bits(encoding, 10, 10) == 1 {
        return three_same(encoding);
    }
    unallocated(encoding)
}

/// Bits 28..24 = 01111: the modified immediates and the shifts by immediate.
///
/// The two are told apart by `immh` in bits 22..19: it is zero for a modified
/// immediate, whose own bits live elsewhere, and non-zero for every shift.
fn immediate_or_shift(encoding: u32) -> Instruction {
    if bits(encoding, 22, 19) == 0 {
        return move_immediate(encoding);
    }
    shift_by_immediate(encoding)
}

/// `AND`, `ORR`, `EOR`, `BIC`, `ORN`, `BSL`, `BIT`, `BIF` — the logical group,
/// whose `size` field selects the operation rather than an element width.
fn three_same_logical(encoding: u32) -> Instruction {
    let (rd, rn, rm) = fields(encoding);
    let op = match (bits(encoding, 29, 29), bits(encoding, 23, 22)) {
        (0, 0b00) => Op::VecAnd,
        (0, 0b01) => Op::VecBic,
        (0, 0b10) => Op::VecOrr,
        (0, 0b11) => Op::VecOrn,
        (_, 0b00) => Op::VecEor,
        (_, 0b01) => Op::VecBsl,
        (_, 0b10) => Op::VecBit,
        _ => Op::VecBif,
    };

    // Every operand is bytes here: the operation is bitwise, so the lane width
    // carries no meaning.
    let quad = is_quad(encoding);
    let form = Form::VecData {
        vd: vector(rd, ElemSize::B8, quad),
        vn: vector(rn, ElemSize::B8, quad),
        vm: Some(vector(rm, ElemSize::B8, quad)),
        va: None,
    };
    Instruction::new(encoding, op, form)
}

/// The three-register-same group: arithmetic and the comparisons.
fn three_same(encoding: u32) -> Instruction {
    let (rd, rn, rm) = fields(encoding);
    let is_unsigned = bits(encoding, 29, 29) == 1;
    let elem = ElemSize::from_size(bits(encoding, 23, 22) as u8);
    let quad = is_quad(encoding);

    let Some(op) = three_same_op(bits(encoding, 15, 11), is_unsigned) else {
        return unallocated(encoding);
    };
    // A 64-bit element needs the full register: one lane is not an
    // arrangement the architecture encodes.
    if elem == ElemSize::D64 && !quad {
        return unallocated(encoding);
    }

    let form = Form::VecData {
        vd: vector(rd, elem, quad),
        vn: vector(rn, elem, quad),
        vm: Some(vector(rm, elem, quad)),
        va: None,
    };
    Instruction::new(encoding, op, form)
}

/// The opcode table of the three-register-same group.
fn three_same_op(opcode: u32, is_unsigned: bool) -> Option<Op> {
    let signed_choice = |signed, unsigned| Some(if is_unsigned { unsigned } else { signed });

    match opcode {
        0b00001 => signed_choice(Op::Sshl, Op::Ushl),
        0b00110 => signed_choice(Op::VecCmgt, Op::VecCmhi),
        0b00111 => signed_choice(Op::VecCmge, Op::VecCmhs),
        0b10000 => signed_choice(Op::VecAdd, Op::VecSub),
        0b10001 if is_unsigned => Some(Op::VecCmeq),
        0b10011 => signed_choice(Op::VecMul, Op::VecMul),
        0b10010 => signed_choice(Op::VecMla, Op::VecMls),
        0b10111 => Some(Op::Addp),
        _ => None,
    }
}

/// The across-lanes reductions: `ADDV`, `UMAXV`, `UMINV`.
fn across_lanes(encoding: u32) -> Instruction {
    let (rd, rn, _) = fields(encoding);
    let elem = ElemSize::from_size(bits(encoding, 23, 22) as u8);
    let quad = is_quad(encoding);

    // opcode is bits 16..12: 11010 is the minimum/maximum pair, separated by
    // bit 16 of the opcode itself, and 11011 is ADDV.
    // opcode is bits 16..12, with `U` choosing between the signed and unsigned
    // extrema. Only the unsigned pair and ADDV are claimed.
    let op = match (bits(encoding, 16, 12), bits(encoding, 29, 29)) {
        (0b11010, 1) => Op::Uminv,
        (0b01010, 1) => Op::Umaxv,
        (0b11011, _) => Op::Addv,
        _ => return unallocated(encoding),
    };
    // The reduction writes one element, not a vector.
    let form = Form::VecData {
        vd: VecOperand {
            reg: Vec::new(rd as u8),
            shape: VecShape::Scalar(elem),
        },
        vn: vector(rn, elem, quad),
        vm: None,
        va: None,
    };
    Instruction::new(encoding, op, form)
}

/// `DUP`, `INS`, `UMOV`, `SMOV` — the copy group, keyed on `imm5`.
fn copy(encoding: u32) -> Instruction {
    let imm5 = bits(encoding, 20, 16);
    let Some((elem, lane)) = lane_from_imm5(imm5) else {
        return unallocated(encoding);
    };

    // op — bit 29 — set is the element-to-element insert, whose imm4 names a
    // source lane rather than selecting an operation.
    if bits(encoding, 29, 29) == 1 {
        return insert_element(encoding);
    }

    match bits(encoding, 14, 11) {
        0b0000 => duplicate_from_element(encoding, elem, lane),
        0b0001 => duplicate_from_gpr(encoding, elem),
        0b0011 => insert_from_gpr(encoding, elem, lane),
        0b0101 => move_to_gpr(encoding, elem, lane, true),
        0b0111 => move_to_gpr(encoding, elem, lane, false),
        _ => unallocated(encoding),
    }
}

/// `imm5` names both the element width and the lane, by the position of its
/// lowest set bit.
fn lane_from_imm5(imm5: u32) -> Option<(ElemSize, u8)> {
    if imm5 & 0b1 != 0 {
        return Some((ElemSize::B8, (imm5 >> 1) as u8));
    }
    if imm5 & 0b10 != 0 {
        return Some((ElemSize::H16, (imm5 >> 2) as u8));
    }
    if imm5 & 0b100 != 0 {
        return Some((ElemSize::S32, (imm5 >> 3) as u8));
    }
    if imm5 & 0b1000 != 0 {
        return Some((ElemSize::D64, (imm5 >> 4) as u8));
    }
    None
}

/// `DUP Vd.T, Rn`.
fn duplicate_from_gpr(encoding: u32, elem: ElemSize) -> Instruction {
    let (rd, rn, _) = fields(encoding);
    let form = Form::VecGprMove {
        gpr: Gpr::from_index_zr(rn as u8),
        vec: vector(rd, elem, is_quad(encoding)),
    };
    Instruction::new(encoding, Op::Dup, form)
}

/// `DUP Vd.T, Vn.Ts[index]`.
fn duplicate_from_element(encoding: u32, elem: ElemSize, lane: u8) -> Instruction {
    let (rd, rn, _) = fields(encoding);
    let form = Form::VecData {
        vd: vector(rd, elem, is_quad(encoding)),
        vn: element(rn, elem, lane),
        vm: None,
        va: None,
    };
    Instruction::new(encoding, Op::Dup, form)
}

/// `INS Vd.Ts[index], Rn`.
fn insert_from_gpr(encoding: u32, elem: ElemSize, lane: u8) -> Instruction {
    let (rd, rn, _) = fields(encoding);
    let form = Form::VecGprMove {
        gpr: Gpr::from_index_zr(rn as u8),
        vec: element(rd, elem, lane),
    };
    Instruction::new(encoding, Op::InsGpr, form)
}

/// `UMOV`/`SMOV`.
fn move_to_gpr(encoding: u32, elem: ElemSize, lane: u8, is_signed: bool) -> Instruction {
    let (rd, rn, _) = fields(encoding);
    let form = Form::VecGprMove {
        gpr: Gpr::from_index_zr(rd as u8),
        vec: element(rn, elem, lane),
    };
    let op = if is_signed { Op::Smov } else { Op::Umov };
    let width = super::super::super::operand::RegWidth::from_sf(is_quad(encoding));
    Instruction::new(encoding, op, form).with_width(width)
}

/// `INS Vd.Ts[index], Vn.Ts[index2]` — the element-to-element form.
fn insert_element(encoding: u32) -> Instruction {
    let (rd, rn, _) = fields(encoding);
    let Some((elem, destination)) = lane_from_imm5(bits(encoding, 20, 16)) else {
        return unallocated(encoding);
    };
    // imm4 names the source lane, shifted by the element's own width.
    let source = (bits(encoding, 14, 11) >> elem as u32) as u8;

    let form = Form::VecData {
        vd: element(rd, elem, destination),
        vn: element(rn, elem, source),
        vm: None,
        va: None,
    };
    Instruction::new(encoding, Op::Ins, form)
}

/// `TBL` and `TBX`.
fn table_lookup(encoding: u32) -> Instruction {
    let (rd, rn, rm) = fields(encoding);
    // `len` counts the table registers, one less than the field.
    let table_len = bits(encoding, 14, 13) as u8 + 1;
    let op = if bits(encoding, 12, 12) == 1 {
        Op::Tbx
    } else {
        Op::Tbl
    };

    let quad = is_quad(encoding);
    let form = Form::TableLookup {
        vd: vector(rd, ElemSize::B8, quad),
        table: Vec::new(rn as u8),
        table_len,
        vm: vector(rm, ElemSize::B8, quad),
    };
    Instruction::new(encoding, op, form)
}

/// `EXT` — a byte-granular extraction from a pair of registers.
fn extract(encoding: u32) -> Instruction {
    let (rd, rn, rm) = fields(encoding);
    let quad = is_quad(encoding);
    let index = bits(encoding, 14, 11);
    // A 64-bit form cannot name an index above 7.
    if !quad && index >= 8 {
        return unallocated(encoding);
    }

    // `Rm` is the second half of the extracted pair. `VecImm` has no slot for
    // it, so the interpreter reads it back out of `encoding`, which every
    // Instruction carries; the index is what needs resolving at decode time.
    let _ = rm;
    let form = Form::VecImm {
        vd: vector(rd, ElemSize::B8, quad),
        vn: vector(rn, ElemSize::B8, quad),
        imm: index as u64,
    };
    Instruction::new(encoding, Op::Ext, form)
}

/// The two-register misc group: `NOT`, `CNT`, `REV64`, `REV16`, `NEG`, `ABS`.
fn two_register(encoding: u32) -> Instruction {
    let (rd, rn, _) = fields(encoding);
    let elem = ElemSize::from_size(bits(encoding, 23, 22) as u8);
    let quad = is_quad(encoding);

    let op = match (bits(encoding, 16, 12), bits(encoding, 29, 29)) {
        (0b00000, 0) => Op::VecRev64,
        (0b00001, 0) => Op::VecRev16,
        (0b00101, 0) => Op::VecCnt,
        (0b00101, 1) => Op::VecNot,
        (0b01011, 0) => Op::VecAbs,
        (0b01011, 1) => Op::VecNeg,
        _ => return unallocated(encoding),
    };
    // The bitwise operations are always byte-shaped.
    let elem = if matches!(op, Op::VecNot | Op::VecCnt) {
        ElemSize::B8
    } else {
        elem
    };

    let form = Form::VecData {
        vd: vector(rd, elem, quad),
        vn: vector(rn, elem, quad),
        vm: None,
        va: None,
    };
    Instruction::new(encoding, op, form)
}

/// `MOVI` and `MVNI`.
fn move_immediate(encoding: u32) -> Instruction {
    let rd = bits(encoding, 4, 0);
    let cmode = bits(encoding, 15, 12);
    let is_inverted = bits(encoding, 29, 29) == 1;
    let abc = bits(encoding, 18, 16);
    let defgh = bits(encoding, 9, 5);
    let imm8 = ((abc << 5) | defgh) as u8;

    let Some((imm, elem)) = expand_advsimd_immediate(imm8, cmode, is_inverted) else {
        return unallocated(encoding);
    };
    // cmode = 1110 with op = 1 is the 64-bit `MOVI Dd, #imm`, whose immediate
    // is a byte mask rather than a replicated value.
    let op = if is_inverted && cmode != 0b1110 {
        Op::Mvni
    } else {
        Op::Movi
    };

    let vd = vector(rd, elem, is_quad(encoding));
    let form = Form::VecImm { vd, vn: vd, imm };
    Instruction::new(encoding, op, form)
}

/// `AdvSIMDExpandImm`, restricted to the `cmode` values this slice claims.
///
/// Returns the per-element immediate and the element width it replicates at.
fn expand_advsimd_immediate(imm8: u8, cmode: u32, is_inverted: bool) -> Option<(u64, ElemSize)> {
    let value = imm8 as u64;
    match cmode >> 1 {
        // 32-bit, shifted by 0, 8, 16 or 24.
        0b000..=0b011 => Some((value << (8 * (cmode >> 1)), ElemSize::S32)),
        // 16-bit, shifted by 0 or 8.
        0b100 | 0b101 => Some((value << (8 * ((cmode >> 1) & 1)), ElemSize::H16)),
        // 32-bit with trailing ones.
        0b110 => {
            let shift = 8 * (((cmode >> 1) & 1) + 1);
            Some(((value << shift) | ((1 << shift) - 1), ElemSize::S32))
        }
        _ => expand_wide_immediate(imm8, cmode, is_inverted),
    }
}

/// The `cmode = 111x` values: byte replication and the 64-bit byte mask.
fn expand_wide_immediate(imm8: u8, cmode: u32, is_inverted: bool) -> Option<(u64, ElemSize)> {
    match (cmode & 0b1111, is_inverted) {
        (0b1110, false) => Some((imm8 as u64, ElemSize::B8)),
        // MOVI Dd, #imm: each bit of imm8 becomes a whole byte.
        (0b1110, true) => {
            let mut expanded = 0u64;
            for bit in 0..8 {
                if imm8 & (1 << bit) != 0 {
                    expanded |= 0xffu64 << (8 * bit);
                }
            }
            Some((expanded, ElemSize::D64))
        }
        // A single-precision float immediate, as VFPExpandImm builds it.
        (0b1111, false) => Some((expand_float32(imm8), ElemSize::S32)),
        _ => None,
    }
}

/// The `cmode = 1111` float immediate.
fn expand_float32(imm8: u8) -> u64 {
    let sign = (imm8 >> 7) as u64;
    let exponent_high = ((imm8 >> 6) & 1) as u64;
    let exponent_low = ((imm8 >> 4) & 0b11) as u64;
    let mantissa = (imm8 & 0b1111) as u64;
    let repeated = if exponent_high == 1 { 0b11111 } else { 0 };

    (sign << 31)
        | ((1 - exponent_high) << 30)
        | (repeated << 25)
        | (exponent_low << 23)
        | (mantissa << 19)
}

/// `SHL`, `USHR`, `SSHR` — shifts by an immediate, whose `immh` names the
/// element width.
fn shift_by_immediate(encoding: u32) -> Instruction {
    let (rd, rn, _) = fields(encoding);
    let immh = bits(encoding, 22, 19);
    let immb = bits(encoding, 18, 16);
    let Some(elem) = element_from_immh(immh) else {
        return unallocated(encoding);
    };

    let width = elem.bits();
    let encoded = (immh << 3) | immb;
    let is_left = bits(encoding, 15, 11) == 0b01010;
    // A left shift counts up from the element's width; a right shift counts
    // down from twice it.
    let amount = if is_left {
        encoded - width
    } else {
        2 * width - encoded
    };

    let op = if is_left {
        Op::VecShl
    } else if bits(encoding, 29, 29) == 1 {
        Op::VecUshr
    } else {
        Op::VecSshr
    };
    if !is_left && bits(encoding, 15, 11) != 0b00000 {
        return unallocated(encoding);
    }

    let quad = is_quad(encoding);
    let vd = vector(rd, elem, quad);
    let form = Form::VecImm {
        vd,
        vn: vector(rn, elem, quad),
        imm: amount as u64,
    };
    Instruction::new(encoding, op, form)
}

/// `immh` names the element width by its highest set bit.
fn element_from_immh(immh: u32) -> Option<ElemSize> {
    match immh {
        0b0001 => Some(ElemSize::B8),
        0b0010 | 0b0011 => Some(ElemSize::H16),
        0b0100..=0b0111 => Some(ElemSize::S32),
        0b1000..=0b1111 => Some(ElemSize::D64),
        _ => None,
    }
}

/// The permute group: `ZIP`, `UZP`, `TRN`.
fn permute(encoding: u32) -> Instruction {
    let (rd, rn, rm) = fields(encoding);
    let elem = ElemSize::from_size(bits(encoding, 23, 22) as u8);
    let quad = is_quad(encoding);

    let op = match bits(encoding, 14, 12) {
        0b001 => Op::Uzp1,
        0b010 => Op::Trn1,
        0b011 => Op::Zip1,
        0b101 => Op::Uzp2,
        0b110 => Op::Trn2,
        0b111 => Op::Zip2,
        _ => return unallocated(encoding),
    };
    if elem == ElemSize::D64 && !quad {
        return unallocated(encoding);
    }

    let form = Form::VecData {
        vd: vector(rd, elem, quad),
        vn: vector(rn, elem, quad),
        vm: Some(vector(rm, elem, quad)),
        va: None,
    };
    Instruction::new(encoding, op, form)
}
