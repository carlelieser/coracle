//! Register identity, as the decoder and the trace layer both name it.
//!
//! Two vocabularies meet here and must not be confused:
//!
//! - [`Gpr`], [`Vec`] and friends are *decoded operands*. `x31` has already
//!   been resolved to either [`Gpr::ZR`] or [`Gpr::SP`] by the decoder, so the
//!   register file never has to guess which the encoding meant.
//! - [`TraceRegId`] is the wire contract in `tests/TRACE_FORMAT.md` §5. Those
//!   numbers are never renumbered.

/// A general-purpose register operand, after `x31` has been resolved.
///
/// The architecture spends encoding bits on a single field that means `xzr` in
/// most instructions and `sp` in a few. Resolving it at decode time is what
/// lets the register file be a plain array read: nothing downstream needs to
/// know which encoding it came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Gpr {
    /// `x0`–`x30`.
    X(u8),
    /// The stack pointer, where the encoding selects `SP` for slot 31.
    SP,
    /// The zero register, where the encoding selects `XZR`/`WZR` for slot 31.
    ZR,
}

impl Gpr {
    /// Resolves encoding slot `index` where slot 31 means the zero register.
    ///
    /// # Panics
    ///
    /// Panics if `index` exceeds 31. Callers pass a 5-bit field extracted from
    /// an instruction word, so this is unreachable from decoded input.
    pub const fn from_index_zr(index: u8) -> Self {
        assert!(index < 32, "register index out of range");
        if index == 31 {
            Gpr::ZR
        } else {
            Gpr::X(index)
        }
    }

    /// Resolves encoding slot `index` where slot 31 means the stack pointer.
    ///
    /// # Panics
    ///
    /// Panics if `index` exceeds 31.
    pub const fn from_index_sp(index: u8) -> Self {
        assert!(index < 32, "register index out of range");
        if index == 31 {
            Gpr::SP
        } else {
            Gpr::X(index)
        }
    }

    /// Whether writes to this operand are discarded.
    pub const fn is_zero(self) -> bool {
        matches!(self, Gpr::ZR)
    }
}

/// A SIMD/FP register operand, `v0`–`v31`.
///
/// The same physical file is addressed as `b`/`h`/`s`/`d`/`q` scalars and as
/// vectors; the width lives in the instruction's size fields, not here.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Vec(u8);

impl Vec {
    /// Number of SIMD/FP registers.
    pub const COUNT: usize = 32;

    /// Wraps an encoding slot.
    ///
    /// # Panics
    ///
    /// Panics if `index` exceeds 31.
    pub const fn new(index: u8) -> Self {
        assert!(index < 32, "vector register index out of range");
        Self(index)
    }

    /// The encoding slot this register occupies.
    pub const fn index(self) -> u8 {
        self.0
    }
}

/// Number of architectural general-purpose registers excluding `SP` and `XZR`.
pub const NUM_GPR: usize = 31;

/// Stable register ids for the CDT trace stream.
///
/// `tests/TRACE_FORMAT.md` §5 calls these a wire contract: the QEMU plugin, the
/// differ and this crate all hardcode the same numbers, so they are never
/// renumbered. `x31` is never emitted — id 31 is `sp`, and `xzr` reads as zero
/// by definition.
pub mod trace_reg_id {
    /// `x0`. `xn` is `X0 + n` for `n` in `0..31`.
    pub const X0: u16 = 0;
    /// `sp`.
    pub const SP: u16 = 31;
    /// `pc`.
    pub const PC: u16 = 32;
    /// `pstate`, normalised per TRACE_FORMAT §6.
    pub const PSTATE: u16 = 33;
    /// `fpcr`.
    pub const FPCR: u16 = 34;
    /// `fpsr`.
    pub const FPSR: u16 = 35;
    /// `v0` low half. `vn` low is `V_BASE + 2*n`, high is one past it.
    pub const V_BASE: u16 = 64;
    /// First EL1 system register id. M1 streams emit none of these.
    pub const SYS_BASE: u16 = 256;

    /// Trace id for `xn`.
    ///
    /// # Panics
    ///
    /// Panics if `n` is 31 or above: `x31` is never emitted.
    pub const fn gpr(n: u8) -> u16 {
        assert!((n as usize) < super::NUM_GPR, "x31 is never emitted");
        X0 + n as u16
    }

    /// Trace id for the low 64 bits of `vn`.
    ///
    /// # Panics
    ///
    /// Panics if `n` exceeds 31.
    pub const fn vec_lo(n: u8) -> u16 {
        assert!(
            (n as usize) < super::Vec::COUNT,
            "vector index out of range"
        );
        V_BASE + 2 * n as u16
    }

    /// Trace id for the high 64 bits of `vn`.
    ///
    /// # Panics
    ///
    /// Panics if `n` exceeds 31.
    pub const fn vec_hi(n: u8) -> u16 {
        vec_lo(n) + 1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slot_31_resolves_to_the_register_the_encoding_asked_for() {
        assert_eq!(Gpr::from_index_zr(31), Gpr::ZR);
        assert_eq!(Gpr::from_index_sp(31), Gpr::SP);
        assert_eq!(Gpr::from_index_zr(30), Gpr::X(30));
        assert_eq!(Gpr::from_index_sp(30), Gpr::X(30));
    }

    #[test]
    fn only_the_zero_register_discards_writes() {
        assert!(Gpr::ZR.is_zero());
        assert!(!Gpr::SP.is_zero());
        assert!(!Gpr::X(0).is_zero());
    }

    #[test]
    fn trace_ids_match_the_wire_contract() {
        // TRACE_FORMAT.md §5, quoted verbatim as numbers so a renumbering of
        // the constants cannot pass silently.
        assert_eq!(trace_reg_id::gpr(0), 0);
        assert_eq!(trace_reg_id::gpr(30), 30);
        assert_eq!(trace_reg_id::SP, 31);
        assert_eq!(trace_reg_id::PC, 32);
        assert_eq!(trace_reg_id::PSTATE, 33);
        assert_eq!(trace_reg_id::FPCR, 34);
        assert_eq!(trace_reg_id::FPSR, 35);
        assert_eq!(trace_reg_id::vec_lo(0), 64);
        assert_eq!(trace_reg_id::vec_hi(0), 65);
        assert_eq!(trace_reg_id::vec_lo(31), 126);
        assert_eq!(trace_reg_id::vec_hi(31), 127);
        assert_eq!(trace_reg_id::SYS_BASE, 256);
    }

    #[test]
    fn vector_registers_keep_the_slot_they_were_built_from() {
        for index in 0..Vec::COUNT as u8 {
            assert_eq!(Vec::new(index).index(), index);
        }
    }
}
