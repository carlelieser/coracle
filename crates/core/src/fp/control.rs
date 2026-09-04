//! FPCR and FPSR, interpreted.
//!
//! [`crate::regfile::RegFile`] stores both as raw `u64`, because the trace
//! stream carries them verbatim. The fields the FP path actually reads are
//! named here so the backend-selection rule in `docs/machine-spec.md` §6 is
//! stated once rather than rediscovered per instruction.

use super::operand::FpRounding;

/// FPCR bit positions, as the architecture numbers them.
mod fpcr_bit {
    /// Flush denormalised results to zero.
    pub const FZ: u32 = 24;
    /// Default NaN: quiet NaN results carry no propagated payload.
    pub const DN: u32 = 25;
    /// Alternative half-precision format.
    pub const AHP: u32 = 26;
    /// Flush denormalised FP16 inputs and results to zero.
    pub const FZ16: u32 = 19;
    /// Least significant bit of `RMode`.
    pub const RMODE_LSB: u32 = 22;
}

/// Every FPCR bit this build gives meaning to.
///
/// Anything outside this mask is stored and read back but does not change
/// behaviour, which includes the exception-trap enables: the machine advertises
/// untrapped FP only, so `IDE`/`IXE`/`UFE`/`OFE`/`DZE`/`IOE` being set does not
/// make an exception trap.
const MEANINGFUL_BITS: u64 = (0b11 << fpcr_bit::RMODE_LSB)
    | (1 << fpcr_bit::FZ)
    | (1 << fpcr_bit::DN)
    | (1 << fpcr_bit::AHP)
    | (1 << fpcr_bit::FZ16);

/// The interpreted contents of FPCR.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct FpControl {
    /// Rounding mode selected by `FPCR.RMode`.
    pub rounding: FpRounding,
    /// `FPCR.FZ` — flush denormalised single and double results to zero.
    pub is_flush_to_zero: bool,
    /// `FPCR.FZ16` — the same for half precision.
    pub is_flush_to_zero_fp16: bool,
    /// `FPCR.DN` — every NaN result is the default NaN.
    pub is_default_nan: bool,
    /// `FPCR.AHP` — alternative half-precision format, which has no infinities.
    pub is_alternative_fp16: bool,
}

impl FpControl {
    /// The reset state: round to nearest, no flushing, IEEE NaN propagation.
    pub const DEFAULT: Self = Self {
        rounding: FpRounding::Nearest,
        is_flush_to_zero: false,
        is_flush_to_zero_fp16: false,
        is_default_nan: false,
        is_alternative_fp16: false,
    };

    /// Interprets a raw FPCR value.
    pub const fn from_bits(fpcr: u64) -> Self {
        Self {
            rounding: FpRounding::from_rmode(((fpcr >> fpcr_bit::RMODE_LSB) & 0b11) as u8),
            is_flush_to_zero: fpcr & (1 << fpcr_bit::FZ) != 0,
            is_flush_to_zero_fp16: fpcr & (1 << fpcr_bit::FZ16) != 0,
            is_default_nan: fpcr & (1 << fpcr_bit::DN) != 0,
            is_alternative_fp16: fpcr & (1 << fpcr_bit::AHP) != 0,
        }
    }

    /// Whether this state is the one the native backend is allowed to serve.
    ///
    /// `docs/machine-spec.md` §6: wasm offers no rounding-mode control and no
    /// flush-to-zero, so the native path is correct only in default mode.
    pub const fn is_default_mode(self) -> bool {
        matches!(self.rounding, FpRounding::Nearest)
            && !self.is_flush_to_zero
            && !self.is_flush_to_zero_fp16
            && !self.is_default_nan
            && !self.is_alternative_fp16
    }
}

/// Whether a raw FPCR value leaves the FP path in default mode.
///
/// Cheaper than [`FpControl::from_bits`] followed by
/// [`FpControl::is_default_mode`], and this is the check the interpreter makes
/// on every FPCR write.
pub const fn is_default_mode_bits(fpcr: u64) -> bool {
    fpcr & MEANINGFUL_BITS == 0
}

/// FPSR cumulative exception flags, as bit positions.
mod fpsr_bit {
    /// Invalid operation.
    pub const IOC: u32 = 0;
    /// Divide by zero.
    pub const DZC: u32 = 1;
    /// Overflow.
    pub const OFC: u32 = 2;
    /// Underflow.
    pub const UFC: u32 = 3;
    /// Inexact.
    pub const IXC: u32 = 4;
    /// Input denormal.
    pub const IDC: u32 = 7;
}

/// Cumulative exception flags raised by one operation.
///
/// Accumulated into FPSR by the softfloat backend only — the native backend
/// cannot observe them, which `docs/machine-spec.md` §6 records as a known
/// divergence rather than a defect.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct FpExceptions(u8);

impl FpExceptions {
    /// No exception raised.
    pub const NONE: Self = Self(0);
    /// Invalid operation — `IOC`.
    pub const INVALID: Self = Self(1 << fpsr_bit::IOC);
    /// Divide by zero — `DZC`.
    pub const DIVIDE_BY_ZERO: Self = Self(1 << fpsr_bit::DZC);
    /// Overflow — `OFC`.
    pub const OVERFLOW: Self = Self(1 << fpsr_bit::OFC);
    /// Underflow — `UFC`.
    pub const UNDERFLOW: Self = Self(1 << fpsr_bit::UFC);
    /// Inexact — `IXC`.
    pub const INEXACT: Self = Self(1 << fpsr_bit::IXC);
    /// Input denormal — `IDC`.
    pub const INPUT_DENORMAL: Self = Self(1 << fpsr_bit::IDC);

    /// Whether no flag is set.
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    /// The union of two flag sets.
    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    /// Whether every flag in `other` is present here.
    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    /// The FPSR bits these flags occupy.
    pub const fn to_bits(self) -> u64 {
        self.0 as u64
    }
}

/// Accumulates `raised` into a raw FPSR value.
///
/// The flags are cumulative and sticky: an operation never clears one another
/// operation set. Only a write to FPSR does.
pub const fn accumulate(fpsr: u64, raised: FpExceptions) -> u64 {
    fpsr | raised.to_bits()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_zeroed_fpcr_is_default_mode() {
        assert!(is_default_mode_bits(0));
        assert_eq!(FpControl::from_bits(0), FpControl::DEFAULT);
        assert!(FpControl::from_bits(0).is_default_mode());
    }

    #[test]
    fn each_meaningful_fpcr_bit_leaves_default_mode() {
        // Every one of these forces the softfloat backend. A regression that
        // dropped any single bit from the check would silently run wasm ops
        // under a rounding mode or flush setting they cannot honour.
        let leaves_default = [
            fpcr_bit::FZ,
            fpcr_bit::DN,
            fpcr_bit::AHP,
            fpcr_bit::FZ16,
            fpcr_bit::RMODE_LSB,
            fpcr_bit::RMODE_LSB + 1,
        ];

        for bit in leaves_default {
            let fpcr = 1u64 << bit;
            assert!(
                !is_default_mode_bits(fpcr),
                "bit {bit} must leave default mode"
            );
            assert!(!FpControl::from_bits(fpcr).is_default_mode(), "bit {bit}");
        }
    }

    #[test]
    fn the_trap_enable_bits_do_not_leave_default_mode() {
        // The machine advertises untrapped FP only, so setting a trap enable
        // changes nothing observable and must not push the FP path onto
        // softfloat.
        for bit in [8u32, 9, 10, 11, 12, 15] {
            assert!(is_default_mode_bits(1 << bit), "trap enable bit {bit}");
        }
    }

    #[test]
    fn rmode_decodes_in_encoding_order() {
        let expected = [
            (0b00, FpRounding::Nearest),
            (0b01, FpRounding::Plus),
            (0b10, FpRounding::Minus),
            (0b11, FpRounding::Zero),
        ];

        for (rmode, rounding) in expected {
            let fpcr = (rmode as u64) << fpcr_bit::RMODE_LSB;
            assert_eq!(FpControl::from_bits(fpcr).rounding, rounding);
        }
    }

    #[test]
    fn exception_flags_land_on_the_fpsr_bits_the_architecture_names() {
        assert_eq!(FpExceptions::INVALID.to_bits(), 0b1);
        assert_eq!(FpExceptions::DIVIDE_BY_ZERO.to_bits(), 0b10);
        assert_eq!(FpExceptions::OVERFLOW.to_bits(), 0b100);
        assert_eq!(FpExceptions::UNDERFLOW.to_bits(), 0b1000);
        assert_eq!(FpExceptions::INEXACT.to_bits(), 0b1_0000);
        assert_eq!(FpExceptions::INPUT_DENORMAL.to_bits(), 0b1000_0000);
    }

    #[test]
    fn accumulating_flags_is_sticky() {
        let after_first = accumulate(0, FpExceptions::INEXACT);
        let after_second = accumulate(after_first, FpExceptions::OVERFLOW);

        assert_eq!(
            after_second,
            FpExceptions::INEXACT
                .union(FpExceptions::OVERFLOW)
                .to_bits()
        );
        // The second operation raised no inexact flag, but the first one's
        // survives — that is what cumulative means.
        assert_eq!(accumulate(after_second, FpExceptions::NONE), after_second);
    }

    #[test]
    fn unrelated_fpsr_bits_survive_accumulation() {
        // QC (bit 27) and the NZCV copy in the top nibble belong to other
        // paths; accumulating must not disturb them.
        let existing = (1 << 27) | (0b1010 << 28);
        assert_eq!(accumulate(existing, FpExceptions::INVALID), existing | 1,);
    }
}
