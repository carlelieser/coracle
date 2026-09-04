//! Owned by the memory slice.

use super::super::address::{AccessSize, AddrMode, Ordering, WriteBack};
use super::super::instruction::{unallocated, Form, Instruction};
use super::super::op::Op;
use super::super::operand::RegWidth;
use super::bits;
use crate::reg::Gpr;

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
