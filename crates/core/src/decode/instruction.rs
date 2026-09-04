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
use super::op::Op;
use super::operand::{Cond, ExtendedReg, RegWidth, RoundMode, ShiftedReg, VecOperand};
use crate::reg::{Gpr, Vec};

/// A fully decoded A64 instruction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Instruction {
    /// What the instruction computes.
    pub op: Op,
    /// Where its operands are.
    pub form: Form,
    /// Width of the instruction's general-purpose operands.
    ///
    /// Load-bearing wherever a `Gpr` appears, which includes
    /// [`Form::VecGprMove`]: `SCVTF S0, W0` and `SCVTF S0, X0` differ only in
    /// `sf`. It is [`RegWidth::X64`] and unread for the forms with no `Gpr`
    /// operand, whose widths live in [`VecOperand::shape`].
    pub width: RegWidth,
    /// Rounding mode named by the encoding itself.
    ///
    /// [`RoundMode::Current`] — the default — means FPCR decides, which is the
    /// case for every instruction outside the `FCVTxS`/`FRINT` families.
    pub round: RoundMode,
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
            round: RoundMode::Current,
            sets_flags: false,
            encoding,
        }
    }

    /// Sets the integer operand width.
    pub const fn with_width(mut self, width: RegWidth) -> Self {
        self.width = width;
        self
    }

    /// Sets the rounding mode the encoding names.
    pub const fn with_round(mut self, round: RoundMode) -> Self {
        self.round = round;
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
    /// `Rd, #imm16, LSL #shift` — move wide: `MOVZ`, `MOVN`, `MOVK`.
    ///
    /// The halfword and its position stay separate rather than being folded
    /// into one `u64`. `MOVK` merges into only the selected halfword and
    /// preserves the other three, so a pre-shifted immediate of zero would not
    /// say which halfword to overwrite.
    MoveWide {
        /// Destination.
        rd: Gpr,
        /// The 16-bit `imm16` field, unshifted.
        imm16: u16,
        /// Halfword position: `hw * 16` is the left shift.
        hw: u8,
    },
    /// `Rd, #offset` — PC-relative address formation: `ADR`, `ADRP`.
    ///
    /// `ADRP` scales its offset by the 4 KiB page and forms the result from the
    /// page-aligned PC; the decoder applies the scaling, and the [`Op`] says
    /// whether the PC is aligned first.
    PcRelAddr {
        /// Destination.
        rd: Gpr,
        /// Signed byte offset from this instruction's own address.
        offset: i64,
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
    /// `Rt{, Rt2}, addr` — load or store of one or two registers.
    ///
    /// One variant rather than separate single and pair forms: `LDP`, `STP`,
    /// `LDXP` and `STXP` differ from their single-register counterparts only in
    /// whether `rt2` is present, and splitting them would duplicate the
    /// address-generation arm.
    LoadStore {
        /// First transferred register.
        rt: Gpr,
        /// Second transferred register, for the pair forms.
        rt2: Option<Gpr>,
        /// Status register written by `STXR`/`STLXR`/`STXP`.
        ///
        /// `None` for every non-exclusive access. It cannot be [`Gpr::ZR`] as a
        /// sentinel: `STP xzr, xzr, [sp]` makes `ZR` a legal operand.
        rs: Option<Gpr>,
        /// Address computation.
        addr: AddrMode,
        /// Access width of each transferred register.
        size: AccessSize,
        /// Ordering and exclusivity.
        ordering: Ordering,
    },
    /// `#prfop, addr` — `PRFM` and `PRFUM`.
    ///
    /// Separate from [`Form::LoadStore`] because the `Rt` slot holds a 5-bit
    /// prefetch operation, not a register: reading it as one would turn
    /// `PRFM #31` into [`Gpr::ZR`], and no data is transferred at all.
    Prefetch {
        /// The 5-bit `prfop` field: type, target and policy.
        prfop: u8,
        /// Address computation.
        addr: AddrMode,
    },
    /// `{Vt..}, addr` — SIMD/FP load or store.
    ///
    /// Covers the scalar transfers, `LDP`/`STP` of FP registers, and the
    /// `LD1`–`LD4` structure forms. The register list is a base plus a count
    /// rather than a set, because the architecture only ever names consecutive
    /// registers, wrapping modulo 32.
    LoadStoreVec {
        /// First transferred register, and the shape each one is accessed with.
        vt: VecOperand,
        /// Registers transferred, counting `vt`: 1–4.
        ///
        /// 2 also covers the FP pair forms; [`Op`] says whether the access
        /// de-interleaves.
        count: u8,
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
    /// `Vd, Vn, Vm, cond` — `FCSEL`.
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
    /// `Vn, Vm, #nzcv, cond` — `FCCMP`, `FCCMPE`.
    ///
    /// Distinct from [`Form::VecCond`] because it has no destination and needs
    /// the substituted flags, and from [`Form::CondCompare`] because its
    /// operands are SIMD/FP registers.
    VecCondCompare {
        /// First compared register.
        vn: VecOperand,
        /// Second compared register.
        vm: VecOperand,
        /// NZCV substituted when the condition fails.
        nzcv: u8,
        /// Condition tested.
        cond: Cond,
    },
    /// `Vn, Vm` or `Vn, #0.0` — `FCMP`, `FCMPE`.
    ///
    /// No destination: the result is NZCV. `vm` is `None` for the
    /// compare-with-zero encodings, which have no second register operand.
    VecCompare {
        /// First compared register.
        vn: VecOperand,
        /// Second compared register, absent for the zero forms.
        vm: Option<VecOperand>,
    },
    /// `Vd, {Vn..}, Vm` — `TBL`, `TBX`.
    ///
    /// The table is 1–4 consecutive registers, so it is a base plus a length
    /// rather than a source list; no other form needs more than three sources.
    TableLookup {
        /// Destination.
        vd: VecOperand,
        /// First table register.
        table: Vec,
        /// Table registers, counting `table`: 1–4.
        table_len: u8,
        /// Index register.
        vm: VecOperand,
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
