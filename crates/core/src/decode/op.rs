//! What an instruction computes.
//!
//! Flat and semantic, split from the operand model in
//! [`super::instruction`] so the two grow independently: a phase B slice adds
//! opcodes here without touching [`super::instruction::Form`], and the operand
//! shapes stay a short list even as this one grows long.

/// What an instruction computes.
///
/// Flat and semantic. `ADD` appears once even though the architecture has four
/// `ADD` encodings, because they differ only in [`super::instruction::Form`]. Aliases (`MOV`,
/// `CMP`, `LSL`, `TST`) do not appear at all: the decoder rewrites them into the
/// instruction they alias, so the interpreter never implements the same
/// operation twice.
///
/// Grouped by the Phase B slice that owns each range. The groups are additive —
/// a slice appends to its own range without touching another's.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum Op {
    /// An encoding this build does not implement, or one the architecture
    /// leaves unallocated. Carried as an `Op` rather than a decode error so
    /// the decoded-instruction cache can hold it and the trap fires at execute
    /// time, at the right PC.
    Unallocated,

    // ---- integer ----
    /// Add.
    Add,
    /// Subtract.
    Sub,
    /// Add with carry.
    Adc,
    /// Subtract with carry.
    Sbc,
    /// Bitwise AND.
    And,
    /// Bitwise OR.
    Orr,
    /// Bitwise exclusive OR.
    Eor,
    /// Bitwise AND NOT.
    Bic,
    /// Bitwise OR NOT.
    Orn,
    /// Bitwise exclusive OR NOT.
    Eon,
    /// Move wide with zero.
    Movz,
    /// Move wide with NOT.
    Movn,
    /// Move wide keeping other halfwords.
    Movk,
    /// Form PC-relative address.
    Adr,
    /// Form PC-relative page address.
    Adrp,
    /// Signed bitfield move.
    Sbfm,
    /// Bitfield move.
    Bfm,
    /// Unsigned bitfield move.
    Ubfm,
    /// Extract register.
    Extr,
    /// Variable logical shift left.
    Lslv,
    /// Variable logical shift right.
    Lsrv,
    /// Variable arithmetic shift right.
    Asrv,
    /// Variable rotate right.
    Rorv,
    /// Conditional select.
    Csel,
    /// Conditional select increment.
    Csinc,
    /// Conditional select invert.
    Csinv,
    /// Conditional select negate.
    Csneg,
    /// Conditional compare.
    Ccmp,
    /// Conditional compare negative.
    Ccmn,
    /// Multiply-add.
    Madd,
    /// Multiply-subtract.
    Msub,
    /// Signed multiply-add long.
    Smaddl,
    /// Signed multiply-subtract long.
    Smsubl,
    /// Unsigned multiply-add long.
    Umaddl,
    /// Unsigned multiply-subtract long.
    Umsubl,
    /// Signed multiply high.
    Smulh,
    /// Unsigned multiply high.
    Umulh,
    /// Signed divide.
    Sdiv,
    /// Unsigned divide.
    Udiv,
    /// Reverse bits.
    Rbit,
    /// Reverse bytes in 16-bit halfwords.
    Rev16,
    /// Reverse bytes in 32-bit words.
    Rev32,
    /// Reverse bytes.
    Rev,
    /// Count leading zeros.
    Clz,
    /// Count leading sign bits.
    Cls,

    // ---- branches and system ----
    /// Branch.
    B,
    /// Branch with link.
    Bl,
    /// Branch to register.
    Br,
    /// Branch with link to register.
    Blr,
    /// Return from subroutine.
    Ret,
    /// Compare and branch on zero.
    Cbz,
    /// Compare and branch on non-zero.
    Cbnz,
    /// Test bit and branch on zero.
    Tbz,
    /// Test bit and branch on non-zero.
    Tbnz,
    /// Supervisor call.
    Svc,
    /// Hypervisor call.
    Hvc,
    /// Secure monitor call.
    Smc,
    /// Breakpoint.
    Brk,
    /// Halt.
    Hlt,
    /// No operation.
    Nop,
    /// Wait for interrupt.
    Wfi,
    /// Wait for event.
    Wfe,
    /// Yield.
    Yield,
    /// Data memory barrier.
    Dmb,
    /// Data synchronisation barrier.
    Dsb,
    /// Instruction synchronisation barrier.
    Isb,
    /// Move to system register.
    Msr,
    /// Move from system register.
    Mrs,
    /// Exception return.
    Eret,

    // ---- memory ----
    /// Load register.
    Ldr,
    /// Store register.
    Str,
    /// Load pair.
    Ldp,
    /// Store pair.
    Stp,
    /// Load-acquire / load-exclusive, per [`super::address::Ordering`].
    Ldar,
    /// Store-release / store-exclusive, per [`super::address::Ordering`].
    Stlr,
    /// Prefetch memory.
    Prfm,
    /// Load multiple single-element structures.
    Ld1,
    /// Load and de-interleave two-element structures.
    Ld2,
    /// Load and de-interleave three-element structures.
    Ld3,
    /// Load and de-interleave four-element structures.
    Ld4,
    /// Load one element and replicate it across every lane.
    Ld1r,
    /// Store multiple single-element structures.
    St1,
    /// Store and interleave two-element structures.
    St2,
    /// Store and interleave three-element structures.
    St3,
    /// Store and interleave four-element structures.
    St4,

    // ---- FP and NEON ----
    /// FP move, including the immediate and register-file transfer forms.
    Fmov,
    /// FP add.
    Fadd,
    /// FP subtract.
    Fsub,
    /// FP multiply.
    Fmul,
    /// FP divide.
    Fdiv,
    /// FP negate.
    Fneg,
    /// FP absolute value.
    Fabs,
    /// FP square root.
    Fsqrt,
    /// FP fused multiply-add.
    Fmadd,
    /// FP fused multiply-subtract.
    Fmsub,
    /// FP compare.
    Fcmp,
    /// FP conditional compare.
    Fccmp,
    /// FP conditional select.
    Fcsel,
    /// FP convert precision.
    Fcvt,
    /// FP convert and narrow.
    Fcvtn,
    /// FP convert and widen.
    Fcvtl,
    /// FP round to integral, in the mode named by [`super::operand::RoundMode`].
    Frint,
    /// FP convert to signed integer, in a named rounding mode.
    Fcvts,
    /// FP convert to unsigned integer, in a named rounding mode.
    Fcvtu,
    /// FP fused multiply-add to accumulator.
    Fmla,
    /// FP fused multiply-subtract from accumulator.
    Fmls,
    /// FP maximum.
    Fmax,
    /// FP minimum.
    Fmin,
    /// FP convert to signed integer, round toward zero.
    Fcvtzs,
    /// FP convert to unsigned integer, round toward zero.
    Fcvtzu,
    /// Signed integer convert to FP.
    Scvtf,
    /// Unsigned integer convert to FP.
    Ucvtf,
    /// Vector integer add.
    VecAdd,
    /// Vector integer subtract.
    VecSub,
    /// Vector bitwise AND.
    VecAnd,
    /// Vector bitwise OR.
    VecOrr,
    /// Vector bitwise exclusive OR.
    VecEor,
    /// Vector compare equal.
    VecCmeq,
    /// Vector multiply.
    VecMul,
    /// Vector multiply-add to accumulator.
    VecMla,
    /// Vector multiply-subtract from accumulator.
    VecMls,
    /// Vector bitwise select.
    VecBsl,
    /// Vector bitwise insert if true.
    VecBit,
    /// Vector bitwise insert if false.
    VecBif,
    /// Unsigned add long.
    Uaddl,
    /// Signed add long.
    Saddl,
    /// Unsigned multiply long.
    Umull,
    /// Signed multiply long.
    Smull,
    /// Extract narrow.
    Xtn,
    /// Shift right narrow.
    Shrn,
    /// Unsigned shift left.
    Ushl,
    /// Signed shift left.
    Sshl,
    /// Move immediate to vector.
    Movi,
    /// Move inverted immediate to vector.
    Mvni,
    /// Duplicate scalar or element across lanes.
    Dup,
    /// Insert element.
    Ins,
    /// Unsigned move vector element to general-purpose register.
    Umov,
    /// Signed move vector element to general-purpose register.
    Smov,
    /// Table lookup, zeroing out-of-range indices.
    Tbl,
    /// Table lookup, preserving the destination for out-of-range indices.
    Tbx,
    /// Extract vector from pair.
    Ext,
}

impl Op {
    /// Whether this opcode denotes an encoding no decoder claimed.
    pub const fn is_unallocated(self) -> bool {
        matches!(self, Op::Unallocated)
    }
}
