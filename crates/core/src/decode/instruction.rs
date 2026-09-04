//! The decoded-instruction model.
//!
//! Two axes, deliberately kept orthogonal:
//!
//! - [`Op`] names *what* the instruction computes. It is flat and cheap to
//!   match on; the interpreter's dispatch is a jump table over it.
//! - [`Form`] names *where the operands are*. There are far fewer operand
//!   shapes than mnemonics, because A64 reuses the same shapes across whole
//!   encoding groups.
//!
//! The alternative — one enum variant per instruction, carrying its own
//! operands — needs several hundred variants and forces the interpreter to
//! re-extract identical operand patterns in every arm. Splitting the two lets
//! `ADD`, `SUB`, `AND` and `ORR` share a single [`Form::RegShifted`] arm for
//! operand fetch and differ only in the [`Op`] they dispatch on.
//!
//! [`Form`] is `Copy` and fixed-size; the whole [`Instruction`] fits in a
//! decoded-instruction cache entry without an allocation, which is what
//! `docs/plan.md` requires from day one.

use super::address::{AccessSize, AddrMode, Ordering};
use super::operand::{Cond, ExtendedReg, RegWidth, ShiftedReg, VecOperand};
use crate::reg::{Gpr, Vec};

/// A fully decoded A64 instruction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Instruction {
    /// What the instruction computes.
    pub op: Op,
    /// Where its operands are.
    pub form: Form,
    /// Integer operand width. Meaningless for the SIMD/FP forms, which carry
    /// their width in [`VecOperand::shape`]; it is set to
    /// [`RegWidth::X64`] there.
    pub width: RegWidth,
    /// Whether the instruction updates NZCV.
    ///
    /// A field rather than separate `Adds`/`Add` opcodes: the flag-setting and
    /// non-flag-setting encodings are the same computation, and the `S` bit is
    /// literally one bit of the encoding.
    pub sets_flags: bool,
    /// The 32-bit encoding this was decoded from.
    ///
    /// Kept so an unimplemented-opcode trap and the coverage report can name
    /// the exact word without re-reading guest memory, which may since have
    /// been unmapped.
    pub encoding: u32,
}

impl Instruction {
    /// Builds an instruction with the common defaults: 64-bit operands, no flag
    /// update.
    pub const fn new(encoding: u32, op: Op, form: Form) -> Self {
        Self {
            op,
            form,
            width: RegWidth::X64,
            sets_flags: false,
            encoding,
        }
    }

    /// Sets the integer operand width.
    pub const fn with_width(mut self, width: RegWidth) -> Self {
        self.width = width;
        self
    }

    /// Marks the instruction as updating NZCV.
    pub const fn setting_flags(mut self) -> Self {
        self.sets_flags = true;
        self
    }
}

/// Operand shapes. One variant per distinct operand layout in A64, not per
/// mnemonic.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Form {
    /// No operands: `NOP`, `WFI`, `ERET`.
    None,
    /// `Rd, Rn, #imm` — data processing (immediate).
    ///
    /// The immediate is fully resolved: the `shift` field of add/sub-immediate
    /// and the `N:immr:imms` bitmask encoding are both expanded here, so no
    /// consumer re-implements `DecodeBitMasks`.
    RegImm {
        /// Destination.
        rd: Gpr,
        /// First source.
        rn: Gpr,
        /// Resolved immediate.
        imm: u64,
    },
    /// `Rd, Rn, Rm{, shift #amount}` — data processing (register).
    RegShifted {
        /// Destination.
        rd: Gpr,
        /// First source.
        rn: Gpr,
        /// Second source with its shift.
        rm: ShiftedReg,
    },
    /// `Rd, Rn, Rm{, extend #amount}` — add/sub (extended register).
    RegExtended {
        /// Destination.
        rd: Gpr,
        /// First source.
        rn: Gpr,
        /// Second source with its extension.
        rm: ExtendedReg,
    },
    /// `Rd, Rn, Rm, Ra` — three-source: `MADD`, `MSUB`, `SMADDL`, `UMULH`.
    ThreeSource {
        /// Destination.
        rd: Gpr,
        /// First multiplicand.
        rn: Gpr,
        /// Second multiplicand.
        rm: Gpr,
        /// Addend.
        ra: Gpr,
    },
    /// `Rd, Rn{, Rm}, #immr, #imms` — bitfield and extract.
    Bitfield {
        /// Destination.
        rd: Gpr,
        /// Source.
        rn: Gpr,
        /// Second source; equals `rn` for the `EXTR` alias `ROR`.
        rm: Gpr,
        /// Rotate / LSB field.
        immr: u8,
        /// Width / MSB field.
        imms: u8,
    },
    /// `Rd, Rn, Rm, cond` — conditional select and `CCMP` register form.
    CondSelect {
        /// Destination.
        rd: Gpr,
        /// Value when the condition holds.
        rn: Gpr,
        /// Value when it does not.
        rm: Gpr,
        /// Condition tested.
        cond: Cond,
    },
    /// `Rn, #imm, #nzcv, cond` — conditional compare (immediate).
    CondCompare {
        /// Compared register.
        rn: Gpr,
        /// Compared immediate, or the second register for the register form,
        /// which the decoder rewrites into [`Form::CondSelect`].
        imm: u64,
        /// NZCV substituted when the condition fails.
        nzcv: u8,
        /// Condition tested.
        cond: Cond,
    },
    /// `#offset` — unconditional branch and `BL`, relative to this
    /// instruction.
    Branch {
        /// Byte offset from the instruction's own address.
        offset: i64,
    },
    /// `#offset` with a condition — `B.cond`.
    BranchCond {
        /// Byte offset from the instruction's own address.
        offset: i64,
        /// Condition tested.
        cond: Cond,
    },
    /// `Rt, #offset` — compare-and-branch and test-and-branch.
    BranchReg {
        /// Register tested.
        rt: Gpr,
        /// Byte offset from the instruction's own address.
        offset: i64,
        /// Bit position tested by `TBZ`/`TBNZ`; zero for `CBZ`/`CBNZ`.
        bit: u8,
    },
    /// `Rn` — indirect branch: `BR`, `BLR`, `RET`.
    BranchIndirect {
        /// Branch target register.
        rn: Gpr,
    },
    /// `Rt, addr` — single-register load or store.
    LoadStore {
        /// Transferred register.
        rt: Gpr,
        /// Address computation.
        addr: AddrMode,
        /// Access width and sign-extension.
        size: AccessSize,
        /// Ordering and exclusivity.
        ordering: Ordering,
    },
    /// `Rt, Rt2, addr` — load/store pair, and the `STXP` status form.
    LoadStorePair {
        /// First transferred register.
        rt: Gpr,
        /// Second transferred register.
        rt2: Gpr,
        /// Status register for `STXR`/`STXP`; [`Gpr::ZR`] otherwise.
        rs: Gpr,
        /// Address computation.
        addr: AddrMode,
        /// Access width of each element.
        size: AccessSize,
        /// Ordering and exclusivity.
        ordering: Ordering,
    },
    /// `Vt, addr` — SIMD/FP load or store.
    LoadStoreVec {
        /// Transferred SIMD/FP register.
        vt: VecOperand,
        /// Second transferred register for the pair forms.
        vt2: Option<Vec>,
        /// Address computation.
        addr: AddrMode,
        /// Ordering and exclusivity.
        ordering: Ordering,
    },
    /// `Vd, Vn{, Vm{, Va}}` — SIMD/FP data processing, one to three sources.
    ///
    /// One variant covers the one-, two- and three-source encodings rather than
    /// three, because the FP slice's operand fetch is `for each Some(source)`
    /// either way and three arms would triple the match without adding
    /// information.
    VecData {
        /// Destination.
        vd: VecOperand,
        /// First source.
        vn: VecOperand,
        /// Second source, where the encoding has one.
        vm: Option<VecOperand>,
        /// Third source, for `FMADD` and friends.
        va: Option<VecOperand>,
    },
    /// `Vd, Vn, #imm` — SIMD/FP with an immediate: `FMOV`, `MOVI`, shifts by
    /// immediate.
    VecImm {
        /// Destination.
        vd: VecOperand,
        /// Source; equals `vd` for the `MOVI` family, which has no source.
        vn: VecOperand,
        /// Resolved immediate, expanded from the encoding.
        imm: u64,
    },
    /// `Vd, Vn, Vm, cond` — `FCSEL`, and `FCCMP` after the decoder rewrites it.
    VecCond {
        /// Destination.
        vd: VecOperand,
        /// Value when the condition holds.
        vn: VecOperand,
        /// Value when it does not.
        vm: VecOperand,
        /// Condition tested.
        cond: Cond,
    },
    /// `Rd, Vn` or `Vd, Rn` — transfers between the two register files:
    /// `FMOV`, `SCVTF`, `FCVTZS`, `UMOV`, `INS`.
    ///
    /// The direction is implied by the [`Op`], so it is not a field here.
    VecGprMove {
        /// The general-purpose side of the transfer.
        gpr: Gpr,
        /// The SIMD/FP side.
        vec: VecOperand,
    },
    /// `#imm` — exception generation (`SVC`, `BRK`, `HLT`) and the barriers,
    /// whose `CRm` field is carried here.
    Imm {
        /// Raw immediate field.
        imm: u64,
    },
    /// `Rt, sysreg` — `MRS`/`MSR` register form. The system register is left as
    /// its raw encoding; naming it is M2's job.
    System {
        /// Transferred register.
        rt: Gpr,
        /// Packed `op0:op1:CRn:CRm:op2`.
        sysreg: u16,
    },
}

/// What an instruction computes.
///
/// Flat and semantic. `ADD` appears once even though the architecture has four
/// `ADD` encodings, because they differ only in [`Form`]. Aliases (`MOV`,
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
    /// leaves unallocated. Carried as an [`Op`] rather than a decode error so
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
    /// Load-acquire / load-exclusive, per [`Ordering`].
    Ldar,
    /// Store-release / store-exclusive, per [`Ordering`].
    Stlr,
    /// Prefetch memory.
    Prfm,

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
    /// Table lookup.
    Tbl,
    /// Extract vector from pair.
    Ext,
}

impl Op {
    /// Whether this opcode denotes an encoding no decoder claimed.
    pub const fn is_unallocated(self) -> bool {
        matches!(self, Op::Unallocated)
    }
}

/// The instruction the decoder yields for an encoding it does not implement.
///
/// A decode never fails and never panics: it returns this. The trap fires when
/// the interpreter reaches the instruction, so the reported PC is the guest's
/// and the decoded-instruction cache holds one entry like any other.
pub const fn unallocated(encoding: u32) -> Instruction {
    Instruction::new(encoding, Op::Unallocated, Form::None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_instruction_defaults_to_64_bit_operands_without_flags() {
        let insn = Instruction::new(
            0x8b01_0020,
            Op::Add,
            Form::RegShifted {
                rd: Gpr::X(0),
                rn: Gpr::X(1),
                rm: ShiftedReg {
                    reg: Gpr::X(1),
                    kind: super::super::operand::ShiftKind::Lsl,
                    amount: 0,
                },
            },
        );

        assert_eq!(insn.width, RegWidth::X64);
        assert!(!insn.sets_flags);
        assert_eq!(insn.encoding, 0x8b01_0020);
    }

    #[test]
    fn width_and_flag_setting_are_independent_of_the_opcode() {
        let insn = Instruction::new(0, Op::Add, Form::None)
            .with_width(RegWidth::W32)
            .setting_flags();

        assert_eq!(insn.width, RegWidth::W32);
        assert!(insn.sets_flags);
        assert_eq!(insn.op, Op::Add);
    }

    #[test]
    fn an_unallocated_encoding_is_an_instruction_not_an_error() {
        let insn = unallocated(0xffff_ffff);

        assert!(insn.op.is_unallocated());
        assert_eq!(insn.encoding, 0xffff_ffff);
        assert!(!Op::Add.is_unallocated());
    }

    #[test]
    fn a_decoded_instruction_stays_small_enough_to_cache_by_value() {
        // The decoded-instruction cache stores these inline. A regression here
        // would show up as cache pressure in the interpreter, not a test
        // failure, so the size is asserted rather than assumed.
        assert!(
            core::mem::size_of::<Instruction>() <= 48,
            "Instruction grew to {} bytes",
            core::mem::size_of::<Instruction>()
        );
    }
}
