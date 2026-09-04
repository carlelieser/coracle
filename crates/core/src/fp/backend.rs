//! The one trait both FP backends implement.
//!
//! `docs/plan.md` §2 fixes two backends: native wasm ops while FPCR is in
//! default mode, softfloat otherwise and in precise mode. [`FpBackend`] is the
//! seam between them.
//!
//! Every method takes the operand format and the rounding mode explicitly and
//! returns an [`FpResult`]. Nothing in the signature says which backend is
//! answering, which is what keeps the choice out of the call sites: the
//! interpreter resolves it once per FPCR write into an [`FpEngine`] and then
//! calls through that.

use super::control::{FpControl, FpExceptions};
use super::operand::{FpComparison, FpFormat, FpResult, FpRounding};

/// The two-operand arithmetic an FP encoding can name.
///
/// A single [`FpBackend::binary`] entry point over this enum rather than one
/// method per mnemonic: the backends dispatch on it internally, and a new
/// mnemonic that is just another rounding of the same operation does not widen
/// the trait.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FpBinaryOp {
    /// `FADD`.
    Add,
    /// `FSUB`.
    Sub,
    /// `FMUL`.
    Mul,
    /// `FDIV`.
    Div,
    /// `FMAX` — propagates NaNs, and prefers the larger operand.
    Max,
    /// `FMIN` — propagates NaNs.
    Min,
    /// `FMAXNM` — returns the non-NaN operand when exactly one is a quiet NaN.
    MaxNum,
    /// `FMINNM` — the same for the minimum.
    MinNum,
}

/// The one-operand arithmetic an FP encoding can name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FpUnaryOp {
    /// `FSQRT`.
    Sqrt,
    /// `FRINT<mode>` — round to an integral value in the same format.
    RoundToIntegral,
}

/// Width and signedness of the integer side of a conversion.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IntFormat {
    /// 32 or 64.
    pub bits: u32,
    /// Whether the integer is two's-complement signed.
    pub is_signed: bool,
}

impl IntFormat {
    /// Signed 32-bit — the `W` forms of `FCVTZS` and `SCVTF`.
    pub const S32: Self = Self {
        bits: 32,
        is_signed: true,
    };
    /// Unsigned 32-bit.
    pub const U32: Self = Self {
        bits: 32,
        is_signed: false,
    };
    /// Signed 64-bit.
    pub const S64: Self = Self {
        bits: 64,
        is_signed: true,
    };
    /// Unsigned 64-bit.
    pub const U64: Self = Self {
        bits: 64,
        is_signed: false,
    };
}

/// One floating-point implementation.
///
/// Implementors are stateless: FPCR arrives as an argument rather than being
/// held, so a single backend instance serves every guest and the interpreter
/// can swap engines without carrying state across.
pub trait FpBackend {
    /// Whether this backend maintains FPSR cumulative exception flags.
    ///
    /// False for the native path. `docs/machine-spec.md` §6 records that as a
    /// deliberate divergence, and the differ reads this to pick its comparison
    /// policy rather than inferring it from the build.
    fn tracks_exceptions(&self) -> bool;

    /// Two-operand arithmetic.
    fn binary(&self, op: FpBinaryOp, operands: FpOperands, control: FpControl) -> FpResult;

    /// One-operand arithmetic.
    fn unary(
        &self,
        op: FpUnaryOp,
        operand: FpValue,
        rounding: FpRounding,
        control: FpControl,
    ) -> FpResult;

    /// Fused multiply-add: `addend + product`, rounded once.
    ///
    /// A single rounding is the whole point of the operation, so this cannot be
    /// composed from [`FpBackend::binary`] and is its own entry point.
    fn fused_multiply_add(&self, operands: FpFmaOperands, control: FpControl) -> FpResult;

    /// Negation, absolute value and the sign-manipulating moves.
    ///
    /// These are bit operations in the architecture — they do not raise
    /// exceptions even for signalling NaNs — so they have no rounding mode and
    /// both backends answer identically.
    fn copy_sign(&self, value: FpValue, op: FpSignOp) -> u64 {
        let format = value.format;
        let sign = 1u64 << format.sign_shift();
        match op {
            FpSignOp::Negate => value.bits ^ sign,
            FpSignOp::Absolute => value.bits & !sign,
        }
    }

    /// Ordered comparison. `FCMP` and `FCMPE` differ only in `is_signalling`.
    fn compare(&self, operands: FpOperands, is_signalling: bool) -> (FpComparison, FpExceptions);

    /// Conversion between two FP formats — `FCVT`.
    fn convert_format(&self, value: FpValue, target: FpFormat, control: FpControl) -> FpResult;

    /// FP to integer, in an explicit rounding mode.
    ///
    /// `fractional_bits` is non-zero for the fixed-point forms, which scale by
    /// `2^fbits` before rounding.
    fn to_integer(&self, value: FpValue, target: ToIntegerTarget, control: FpControl) -> FpResult;

    /// Integer to FP.
    fn from_integer(&self, value: u64, source: FromIntegerSource, control: FpControl) -> FpResult;
}

/// Which sign operation to apply.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FpSignOp {
    /// `FNEG` — flip the sign bit.
    Negate,
    /// `FABS` — clear the sign bit.
    Absolute,
}

/// A value together with the format its bits are in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FpValue {
    /// Raw bits, in `format`.
    pub bits: u64,
    /// How to read them.
    pub format: FpFormat,
}

impl FpValue {
    /// Pairs bits with their format.
    pub const fn new(bits: u64, format: FpFormat) -> Self {
        Self {
            bits: bits & format.mask(),
            format,
        }
    }
}

/// Both operands of a two-operand instruction, which always share a format.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FpOperands {
    /// First operand.
    pub lhs: u64,
    /// Second operand.
    pub rhs: u64,
    /// The format both are in.
    pub format: FpFormat,
    /// Rounding to apply to the result.
    pub rounding: FpRounding,
}

impl FpOperands {
    /// Two operands in `format`, rounded per `rounding`.
    pub const fn new(lhs: u64, rhs: u64, format: FpFormat, rounding: FpRounding) -> Self {
        Self {
            lhs,
            rhs,
            format,
            rounding,
        }
    }

    /// The first operand as a [`FpValue`].
    pub const fn lhs_value(self) -> FpValue {
        FpValue::new(self.lhs, self.format)
    }

    /// The second operand as a [`FpValue`].
    pub const fn rhs_value(self) -> FpValue {
        FpValue::new(self.rhs, self.format)
    }
}

/// The three operands of a fused multiply-add, plus the sign controls that
/// distinguish `FMADD`, `FMSUB`, `FNMADD` and `FNMSUB`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FpFmaOperands {
    /// First multiplicand.
    pub multiplicand: u64,
    /// Second multiplicand.
    pub multiplier: u64,
    /// Addend.
    pub addend: u64,
    /// The format all three are in.
    pub format: FpFormat,
    /// Rounding applied to the single rounding step.
    pub rounding: FpRounding,
    /// Whether the product is negated before the addition.
    pub is_product_negated: bool,
    /// Whether the addend is negated before the addition.
    pub is_addend_negated: bool,
}

/// Where an FP-to-integer conversion is going.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ToIntegerTarget {
    /// Width and signedness of the result.
    pub format: IntFormat,
    /// Rounding applied before truncation.
    pub rounding: FpRounding,
    /// Fixed-point fractional bits; zero for the integer forms.
    pub fractional_bits: u8,
}

/// Where an integer-to-FP conversion is coming from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FromIntegerSource {
    /// Width and signedness of the input.
    pub format: IntFormat,
    /// Format of the result.
    pub target: FpFormat,
    /// Rounding applied to the result.
    pub rounding: FpRounding,
    /// Fixed-point fractional bits; zero for the integer forms.
    pub fractional_bits: u8,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A backend that implements nothing but the provided `copy_sign`, so the
    /// default body is tested through the trait rather than on a concrete type.
    struct SignOnlyBackend;

    impl FpBackend for SignOnlyBackend {
        fn tracks_exceptions(&self) -> bool {
            false
        }
        fn binary(&self, _: FpBinaryOp, _: FpOperands, _: FpControl) -> FpResult {
            unimplemented!("not exercised")
        }
        fn unary(&self, _: FpUnaryOp, _: FpValue, _: FpRounding, _: FpControl) -> FpResult {
            unimplemented!("not exercised")
        }
        fn fused_multiply_add(&self, _: FpFmaOperands, _: FpControl) -> FpResult {
            unimplemented!("not exercised")
        }
        fn compare(&self, _: FpOperands, _: bool) -> (FpComparison, FpExceptions) {
            unimplemented!("not exercised")
        }
        fn convert_format(&self, _: FpValue, _: FpFormat, _: FpControl) -> FpResult {
            unimplemented!("not exercised")
        }
        fn to_integer(&self, _: FpValue, _: ToIntegerTarget, _: FpControl) -> FpResult {
            unimplemented!("not exercised")
        }
        fn from_integer(&self, _: u64, _: FromIntegerSource, _: FpControl) -> FpResult {
            unimplemented!("not exercised")
        }
    }

    #[test]
    fn negation_and_absolute_value_touch_only_the_sign_bit() {
        let backend = SignOnlyBackend;
        let negative = FpValue::new((-2.5f64).to_bits(), FpFormat::Double);
        let positive = FpValue::new(2.5f64.to_bits(), FpFormat::Double);

        assert_eq!(backend.copy_sign(negative, FpSignOp::Negate), positive.bits);
        assert_eq!(
            backend.copy_sign(negative, FpSignOp::Absolute),
            positive.bits
        );
        assert_eq!(
            backend.copy_sign(positive, FpSignOp::Absolute),
            positive.bits,
            "absolute value of a positive is a no-op"
        );
    }

    #[test]
    fn the_sign_operations_leave_a_signalling_nan_signalling() {
        // FNEG and FABS are bit manipulations: they must not quieten, and they
        // must not raise. Getting this wrong is invisible until a differential
        // run compares NaN payloads.
        let backend = SignOnlyBackend;
        let signalling = FpValue::new(0x7f80_0001, FpFormat::Single);

        assert_eq!(backend.copy_sign(signalling, FpSignOp::Negate), 0xff80_0001);
        assert_eq!(
            backend.copy_sign(signalling, FpSignOp::Absolute),
            0x7f80_0001
        );
    }

    #[test]
    fn an_fp_value_is_masked_to_its_format() {
        // A single-precision operand read out of a 128-bit register arrives
        // with the upper bits still set; the format decides what is in range.
        let value = FpValue::new(0xdead_beef_7f80_0001, FpFormat::Single);
        assert_eq!(value.bits, 0x7f80_0001);
    }
}
