//! SIMD/FP loads and stores — the `V = 1` half of the loads-and-stores group.
//!
//! The memory slice owns `V = 0` and hands this half over, because these
//! transfer the SIMD/FP register file and use [`Form::LoadStoreVec`].
//!
//! Two families live here: the single-register and pair forms, which mirror
//! their general-purpose counterparts but name a `VecOperand`, and the
//! `LD1`–`LD4` structure forms, which have no general-purpose equivalent at
//! all.

use super::super::super::address::{AddrMode, Ordering, WriteBack};
use super::super::super::instruction::{unallocated, Form, Instruction};
use super::super::super::op::Op;
use super::super::super::operand::{
    ElemSize, ExtendKind, ExtendedReg, VecHalf, VecOperand, VecShape,
};
use super::super::{bits, sign_extend};
use crate::reg::{Gpr, Vec};

/// The base register, which uses the `SP` rule for slot 31.
fn base(encoding: u32) -> Gpr {
    Gpr::from_index_sp(bits(encoding, 9, 5) as u8)
}

/// The first transferred SIMD/FP register.
fn transferred(encoding: u32) -> Vec {
    Vec::new(bits(encoding, 4, 0) as u8)
}

/// A scalar operand of the given width.
fn scalar(reg: Vec, size: ElemSize) -> VecOperand {
    VecOperand {
        reg,
        shape: VecShape::Scalar(size),
    }
}

/// `V = 1` in the loads-and-stores group.
pub fn loads_and_stores_vec(encoding: u32) -> Instruction {
    // Bits 29..28 pick the family, as they do for the general-purpose half:
    // `00` is the structure forms, `01` the literal, `10` the pairs, `11` the
    // single-register addressing modes.
    match bits(encoding, 29, 28) {
        0b00 => structure(encoding),
        0b01 => literal(encoding),
        0b10 => pair(encoding),
        _ => single_register(encoding),
    }
}

/// The access width of a single-register form.
///
/// `size` and `opc<1>` together give four bits of width: `opc<1>` is the top
/// bit, so a `Q` access is `size = 00` with `opc<1>` set, not `size = 11`.
fn single_size(encoding: u32) -> Option<ElemSize> {
    let size = bits(encoding, 31, 30);
    let is_wide = bits(encoding, 23, 23) == 1;
    match (is_wide, size) {
        (false, 0b00) => Some(ElemSize::B8),
        (false, 0b01) => Some(ElemSize::H16),
        (false, 0b10) => Some(ElemSize::S32),
        (false, 0b11) => Some(ElemSize::D64),
        (true, 0b00) => Some(ElemSize::Q128),
        _ => None,
    }
}

/// `log2` of an element's width in bytes, which is the immediate's scale.
fn scale_of(size: ElemSize) -> u32 {
    size.bits().trailing_zeros() - 3
}

/// The single-register forms: unsigned offset, unscaled, indexed, and
/// register offset.
fn single_register(encoding: u32) -> Instruction {
    let Some(size) = single_size(encoding) else {
        return unallocated(encoding);
    };
    let Some(addr) = single_register_address(encoding, size) else {
        return unallocated(encoding);
    };

    // opc<0> selects a load over a store.
    let op = if bits(encoding, 22, 22) == 1 {
        Op::Ldr
    } else {
        Op::Str
    };
    let form = Form::LoadStoreVec {
        vt: scalar(transferred(encoding), size),
        count: 1,
        addr,
        ordering: Ordering::PLAIN,
    };
    Instruction::new(encoding, op, form)
}

/// The address of a single-register form, or `None` for the unclaimed
/// encodings.
fn single_register_address(encoding: u32, size: ElemSize) -> Option<AddrMode> {
    if bits(encoding, 24, 24) == 1 {
        return Some(AddrMode::Immediate {
            base: base(encoding),
            offset: ((bits(encoding, 21, 10) as u64) << scale_of(size)) as i64,
            writeback: WriteBack::None,
        });
    }

    if bits(encoding, 21, 21) == 1 {
        // Only option = 10 in bits 11..10 is the register offset.
        if bits(encoding, 11, 10) != 0b10 {
            return None;
        }
        return Some(register_offset(encoding, size));
    }

    let offset = sign_extend(bits(encoding, 20, 12), 9);
    let writeback = match bits(encoding, 11, 10) {
        0b00 => WriteBack::None,
        0b01 => WriteBack::Post,
        0b11 => WriteBack::Pre,
        _ => return None,
    };
    Some(AddrMode::Immediate {
        base: base(encoding),
        offset,
        writeback,
    })
}

/// `[base, Xm, extend #amount]`.
fn register_offset(encoding: u32, size: ElemSize) -> AddrMode {
    let is_scaled = bits(encoding, 12, 12) == 1;
    AddrMode::Register {
        base: base(encoding),
        index: ExtendedReg {
            reg: Gpr::from_index_zr(bits(encoding, 20, 16) as u8),
            kind: ExtendKind::from_option(bits(encoding, 15, 13) as u8),
            amount: if is_scaled { scale_of(size) as u8 } else { 0 },
        },
        writeback: WriteBack::None,
    }
}

/// `LDR (literal, SIMD&FP)`.
///
/// `opc` names the width directly here rather than combining with `size`,
/// because a literal load has no `size` field.
fn literal(encoding: u32) -> Instruction {
    let size = match bits(encoding, 31, 30) {
        0b00 => ElemSize::S32,
        0b01 => ElemSize::D64,
        0b10 => ElemSize::Q128,
        _ => return unallocated(encoding),
    };

    let form = Form::LoadStoreVec {
        vt: scalar(transferred(encoding), size),
        count: 1,
        addr: AddrMode::PcRelative {
            offset: sign_extend(bits(encoding, 23, 5), 19) * 4,
        },
        ordering: Ordering::PLAIN,
    };
    Instruction::new(encoding, Op::Ldr, form)
}

/// `LDP`/`STP` of two SIMD/FP registers.
fn pair(encoding: u32) -> Instruction {
    let size = match bits(encoding, 31, 30) {
        0b00 => ElemSize::S32,
        0b01 => ElemSize::D64,
        0b10 => ElemSize::Q128,
        _ => return unallocated(encoding),
    };
    let Some(writeback) = pair_writeback(encoding) else {
        return unallocated(encoding);
    };

    let form = Form::LoadStoreVec {
        vt: scalar(transferred(encoding), size),
        // The pair forms name their second register in `Rt2`, which is always
        // the one after `Rt` only by convention — but `LoadStoreVec` counts
        // consecutive registers, so a non-consecutive pair cannot be expressed.
        count: 2,
        addr: AddrMode::Immediate {
            base: base(encoding),
            offset: sign_extend(bits(encoding, 21, 15), 7) << scale_of(size),
            writeback,
        },
        ordering: Ordering::PLAIN,
    };

    let op = if bits(encoding, 22, 22) == 1 {
        Op::Ldp
    } else {
        Op::Stp
    };
    Instruction::new(encoding, op, form)
}

/// The pair forms' addressing mode, from bits 24..23.
fn pair_writeback(encoding: u32) -> Option<WriteBack> {
    match bits(encoding, 24, 23) {
        0b01 => Some(WriteBack::Post),
        0b10 => Some(WriteBack::None),
        0b11 => Some(WriteBack::Pre),
        // `00` is the non-temporal pair, which this slice has not claimed.
        _ => None,
    }
}

/// What an `LD1`–`LD4` opcode names: the mnemonic and how many registers it
/// transfers.
struct Structure {
    op_load: Op,
    op_store: Op,
    count: u8,
}

/// Decodes the 4-bit `opcode` of a structure form.
///
/// The register count is not the mnemonic's number: `LD1` transfers one, two,
/// three or four registers depending on the opcode, while `LD3` always
/// transfers three.
fn structure_kind(opcode: u32) -> Option<Structure> {
    let kind = |op_load, op_store, count| {
        Some(Structure {
            op_load,
            op_store,
            count,
        })
    };
    match opcode {
        0b0000 => kind(Op::Ld4, Op::St4, 4),
        0b0010 => kind(Op::Ld1, Op::St1, 4),
        0b0100 => kind(Op::Ld3, Op::St3, 3),
        0b0110 => kind(Op::Ld1, Op::St1, 3),
        0b0111 => kind(Op::Ld1, Op::St1, 1),
        0b1000 => kind(Op::Ld2, Op::St2, 2),
        0b1010 => kind(Op::Ld1, Op::St1, 2),
        _ => None,
    }
}

/// `LD1`–`LD4` and `ST1`–`ST4`, the multiple-structure forms.
fn structure(encoding: u32) -> Instruction {
    // Bit 23 selects the post-indexed forms; bits 21..16 must be zero for the
    // non-indexed ones, and hold `Rm` otherwise.
    let is_post_indexed = bits(encoding, 23, 23) == 1;
    if !is_post_indexed && bits(encoding, 21, 16) != 0 {
        return unallocated(encoding);
    }
    let Some(kind) = structure_kind(bits(encoding, 15, 12)) else {
        return unallocated(encoding);
    };

    let is_quad = bits(encoding, 30, 30) == 1;
    let elem = ElemSize::from_size(bits(encoding, 11, 10) as u8);
    // A 64-bit vector of 64-bit elements is one lane, which the architecture
    // does not encode: `size = 11` requires `Q`.
    if elem == ElemSize::D64 && !is_quad {
        return unallocated(encoding);
    }

    let lanes = (if is_quad { 128 } else { 64 }) / elem.bits();
    let vt = VecOperand {
        reg: transferred(encoding),
        shape: VecShape::Vector {
            elem,
            count: lanes as u8,
            half: if is_quad { VecHalf::Full } else { VecHalf::Low },
        },
    };

    let is_load = bits(encoding, 22, 22) == 1;
    let op = if is_load { kind.op_load } else { kind.op_store };
    let form = Form::LoadStoreVec {
        vt,
        count: kind.count,
        addr: structure_address(
            encoding,
            is_post_indexed,
            kind.count,
            lanes * elem.bits() / 8,
        ),
        ordering: Ordering::PLAIN,
    };
    Instruction::new(encoding, op, form)
}

/// The address of a structure form.
///
/// The immediate post-indexed forms encode `Rm = 31`, meaning "advance by the
/// number of bytes transferred" rather than naming a register.
fn structure_address(
    encoding: u32,
    is_post_indexed: bool,
    count: u8,
    bytes_per_register: u32,
) -> AddrMode {
    if !is_post_indexed {
        return AddrMode::BaseOnly {
            base: base(encoding),
        };
    }

    let rm = bits(encoding, 20, 16);
    if rm == 31 {
        return AddrMode::Immediate {
            base: base(encoding),
            offset: (count as u32 * bytes_per_register) as i64,
            writeback: WriteBack::Post,
        };
    }
    AddrMode::Register {
        base: base(encoding),
        index: ExtendedReg {
            reg: Gpr::from_index_zr(rm as u8),
            kind: ExtendKind::Uxtx,
            amount: 0,
        },
        writeback: WriteBack::Post,
    }
}
