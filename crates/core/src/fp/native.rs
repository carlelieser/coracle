//! The backend that uses the host's own float operations.
//!
//! On wasm these lower to `f32.add`, `f64.sqrt` and friends — one instruction
//! each, against softfloat's hundreds. It is correct only while FPCR is in
//! default mode, because wasm exposes neither a rounding-mode control nor
//! flush-to-zero, and it reports no exception flags at all: FPSR's cumulative
//! bits are maintained on the softfloat path only, which
//! `docs/machine-spec.md` §6 records as a deliberate divergence.
//!
//! Half precision has no native operations, so those calls fall through to
//! softfloat rather than being emulated here.

use super::backend::{
    FpBackend, FpBinaryOp, FpFmaOperands, FpOperands, FpUnaryOp, FpValue, FromIntegerSource,
    ToIntegerTarget,
};
use super::control::{FpControl, FpExceptions};
use super::operand::{FpComparison, FpFormat, FpResult, FpRounding};
use super::softfloat::Softfloat;

/// The host-float backend.
///
/// Half-precision work and the non-default rounding modes that reach it
/// through an instruction's own encoding are delegated to [`Softfloat`], so
/// the type is correct for every input rather than only the ones it can
/// accelerate.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Native {
    fallback: Softfloat,
}

impl Native {
    /// A new backend.
    pub const fn new() -> Self {
        Self {
            fallback: Softfloat::new(),
        }
    }

    /// Whether the host's operations can serve this format and rounding mode.
    ///
    /// The host rounds to nearest and nothing else, and it has no `f16`, so
    /// anything else is the fallback's.
    const fn can_serve(format: FpFormat, rounding: FpRounding) -> bool {
        matches!(rounding, FpRounding::Nearest)
            && matches!(format, FpFormat::Single | FpFormat::Double)
    }
}

/// Applies a single- and a double-precision operation to raw bits.
///
/// The two arms are identical but for the width, and writing them out per
/// operation is what makes this backend long; this keeps each operation to its
/// one interesting line.
fn dispatch(
    format: FpFormat,
    bits: u64,
    single: impl Fn(f32) -> f32,
    double: impl Fn(f64) -> f64,
) -> u64 {
    match format {
        FpFormat::Single => single(f32::from_bits(bits as u32)).to_bits() as u64,
        _ => double(f64::from_bits(bits)).to_bits(),
    }
}

/// The same for a two-operand computation.
fn dispatch_binary(
    format: FpFormat,
    operands: (u64, u64),
    single: impl Fn(f32, f32) -> f32,
    double: impl Fn(f64, f64) -> f64,
) -> u64 {
    match format {
        FpFormat::Single => single(
            f32::from_bits(operands.0 as u32),
            f32::from_bits(operands.1 as u32),
        )
        .to_bits() as u64,
        _ => double(f64::from_bits(operands.0), f64::from_bits(operands.1)).to_bits(),
    }
}

impl FpBackend for Native {
    fn tracks_exceptions(&self) -> bool {
        false
    }

    fn binary(&self, op: FpBinaryOp, operands: FpOperands, control: FpControl) -> FpResult {
        if !Self::can_serve(operands.format, operands.rounding) {
            return self.fallback.binary(op, operands, control);
        }

        let pair = (operands.lhs, operands.rhs);
        let bits = match op {
            FpBinaryOp::Add => dispatch_binary(operands.format, pair, |a, b| a + b, |a, b| a + b),
            FpBinaryOp::Sub => dispatch_binary(operands.format, pair, |a, b| a - b, |a, b| a - b),
            FpBinaryOp::Mul => dispatch_binary(operands.format, pair, |a, b| a * b, |a, b| a * b),
            FpBinaryOp::Div => dispatch_binary(operands.format, pair, |a, b| a / b, |a, b| a / b),
            // The extrema have NaN rules the host's own min/max do not share,
            // so they stay on the reference path rather than being approximated.
            FpBinaryOp::Max | FpBinaryOp::Min | FpBinaryOp::MaxNum | FpBinaryOp::MinNum => {
                return self.fallback.binary(op, operands, control)
            }
        };
        FpResult::exact(bits)
    }

    fn unary(
        &self,
        op: FpUnaryOp,
        operand: FpValue,
        rounding: FpRounding,
        control: FpControl,
    ) -> FpResult {
        match op {
            FpUnaryOp::Sqrt if Self::can_serve(operand.format, rounding) => FpResult::exact(
                dispatch(operand.format, operand.bits, |x| x.sqrt(), |x| x.sqrt()),
            ),
            // `FRINT` names its own rounding mode, which the host cannot
            // select, so every mode but the default one belongs to softfloat.
            _ => self.fallback.unary(op, operand, rounding, control),
        }
    }

    fn fused_multiply_add(&self, operands: FpFmaOperands, control: FpControl) -> FpResult {
        // A genuinely fused multiply-add needs one rounding; `mul_add` provides
        // it natively, but only in `std`. This crate is `no_std`, so the
        // reference path is the only correct option.
        self.fallback.fused_multiply_add(operands, control)
    }

    fn compare(&self, operands: FpOperands, is_signalling: bool) -> (FpComparison, FpExceptions) {
        if !matches!(operands.format, FpFormat::Single | FpFormat::Double) {
            return self.fallback.compare(operands, is_signalling);
        }

        let ordering = match operands.format {
            FpFormat::Single => partial_order(
                f32::from_bits(operands.lhs as u32),
                f32::from_bits(operands.rhs as u32),
            ),
            _ => partial_order(f64::from_bits(operands.lhs), f64::from_bits(operands.rhs)),
        };
        // Exception flags are the softfloat path's alone, so the comparison
        // reports the ordering and nothing else.
        (ordering, FpExceptions::NONE)
    }

    fn convert_format(&self, value: FpValue, target: FpFormat, control: FpControl) -> FpResult {
        match (value.format, target) {
            (FpFormat::Single, FpFormat::Double) => {
                FpResult::exact((f32::from_bits(value.bits as u32) as f64).to_bits())
            }
            (FpFormat::Double, FpFormat::Single) => {
                FpResult::exact((f64::from_bits(value.bits) as f32).to_bits() as u64)
            }
            // Half precision has no host operation.
            _ => self.fallback.convert_format(value, target, control),
        }
    }

    fn to_integer(&self, value: FpValue, target: ToIntegerTarget, control: FpControl) -> FpResult {
        // Rust's `as` saturates like the architecture only for
        // round-toward-zero, and it cannot express the fixed-point forms or
        // the other four rounding modes at all. Routing the whole family to the
        // reference keeps one implementation of the saturation boundaries
        // rather than two that must agree.
        self.fallback.to_integer(value, target, control)
    }

    fn from_integer(&self, value: u64, source: FromIntegerSource, control: FpControl) -> FpResult {
        // An integer wider than the significand rounds, and the mode is the
        // encoding's rather than the host's, so this is the reference's too.
        self.fallback.from_integer(value, source, control)
    }
}

/// The IEEE ordering of two host floats.
fn partial_order<T: PartialOrd>(lhs: T, rhs: T) -> FpComparison {
    if lhs < rhs {
        FpComparison::Less
    } else if lhs > rhs {
        FpComparison::Greater
    } else if lhs == rhs {
        FpComparison::Equal
    } else {
        // The only way all three fail is a NaN operand.
        FpComparison::Unordered
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fp::backend::FpSignOp;

    const DOUBLE: FpFormat = FpFormat::Double;
    const SINGLE: FpFormat = FpFormat::Single;

    fn operands(lhs: u64, rhs: u64, format: FpFormat) -> FpOperands {
        FpOperands::new(lhs, rhs, format, FpRounding::Nearest)
    }

    #[test]
    fn the_native_backend_reports_that_it_tracks_no_exceptions() {
        // The differ reads this to pick its comparison policy, so it is part
        // of the contract rather than an implementation detail.
        assert!(!Native::new().tracks_exceptions());
        assert!(Softfloat::new().tracks_exceptions());
    }

    #[test]
    fn native_arithmetic_agrees_with_softfloat_on_ordinary_values() {
        // The two backends must be indistinguishable in default mode for
        // everything but NaN payloads and FPSR, which is the equivalence the
        // M1 gate asks for.
        let native = Native::new();
        let reference = Softfloat::new();
        let mut state = 0x243f_6a88_85a3_08d3u64;
        let mut next = || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            f64::from_bits((state & 0x800f_ffff_ffff_ffff) | 0x3ff0_0000_0000_0000)
        };

        for _ in 0..1000 {
            let pair = operands(next().to_bits(), next().to_bits(), DOUBLE);
            for op in [
                FpBinaryOp::Add,
                FpBinaryOp::Sub,
                FpBinaryOp::Mul,
                FpBinaryOp::Div,
            ] {
                assert_eq!(
                    native.binary(op, pair, FpControl::DEFAULT).bits,
                    reference.binary(op, pair, FpControl::DEFAULT).bits,
                    "{op:?}"
                );
            }
        }
    }

    #[test]
    fn native_single_precision_agrees_with_softfloat() {
        let native = Native::new();
        let reference = Softfloat::new();

        for (lhs, rhs) in [(1.0f32, 3.0f32), (0.1, 0.2), (-7.5, 2.25), (1e30, 1e-30)] {
            let pair = operands(lhs.to_bits() as u64, rhs.to_bits() as u64, SINGLE);
            for op in [FpBinaryOp::Add, FpBinaryOp::Mul, FpBinaryOp::Div] {
                assert_eq!(
                    native.binary(op, pair, FpControl::DEFAULT).bits,
                    reference.binary(op, pair, FpControl::DEFAULT).bits,
                    "{op:?} on {lhs}, {rhs}"
                );
            }
        }
    }

    #[test]
    fn the_native_backend_raises_no_exception_flags() {
        // 1/0 raises DZC on the reference path and nothing here. This is the
        // documented divergence; a test pins it so it stays deliberate.
        let native = Native::new();
        let pair = operands(1.0f64.to_bits(), 0.0f64.to_bits(), DOUBLE);

        let result = native.binary(FpBinaryOp::Div, pair, FpControl::DEFAULT);
        assert_eq!(result.bits, DOUBLE.infinity());
        assert!(result.raised.is_empty(), "no flags on the native path");

        let reference = Softfloat::new().binary(FpBinaryOp::Div, pair, FpControl::DEFAULT);
        assert!(reference.raised.contains(FpExceptions::DIVIDE_BY_ZERO));
    }

    #[test]
    fn a_non_default_rounding_mode_falls_through_to_the_reference() {
        // The host cannot round any way but to-nearest, so a directed mode
        // must not silently use it.
        let native = Native::new();
        let reference = Softfloat::new();
        // 10/3 is the case that separates the modes: rounding to nearest goes
        // up, so truncating gives a different last bit. 1/3 would not — it
        // rounds down either way, and the test would pass vacuously.
        let (lhs, rhs) = (10.0f64.to_bits(), 3.0f64.to_bits());
        let toward_zero = FpOperands::new(lhs, rhs, DOUBLE, FpRounding::Zero);

        let native_result = native.binary(FpBinaryOp::Div, toward_zero, FpControl::DEFAULT);
        let reference_result = reference.binary(FpBinaryOp::Div, toward_zero, FpControl::DEFAULT);

        assert_eq!(native_result.bits, reference_result.bits);
        // And it genuinely differs from the round-to-nearest answer, so the
        // fallthrough is doing something rather than quietly using the host.
        let nearest = native.binary(
            FpBinaryOp::Div,
            operands(lhs, rhs, DOUBLE),
            FpControl::DEFAULT,
        );
        assert_ne!(native_result.bits, nearest.bits);
    }

    #[test]
    fn half_precision_falls_through_to_the_reference() {
        let native = Native::new();
        let reference = Softfloat::new();
        let half = FpOperands::new(0x3c00, 0x4000, FpFormat::Half, FpRounding::Nearest);

        assert_eq!(
            native
                .binary(FpBinaryOp::Add, half, FpControl::DEFAULT)
                .bits,
            reference
                .binary(FpBinaryOp::Add, half, FpControl::DEFAULT)
                .bits
        );
    }

    #[test]
    fn comparison_agrees_with_the_reference_including_the_unordered_case() {
        let native = Native::new();
        let reference = Softfloat::new();
        let values = [
            f64::NEG_INFINITY.to_bits(),
            (-1.0f64).to_bits(),
            (-0.0f64).to_bits(),
            0.0f64.to_bits(),
            1.0f64.to_bits(),
            f64::INFINITY.to_bits(),
            f64::NAN.to_bits(),
        ];

        for &lhs in &values {
            for &rhs in &values {
                let pair = operands(lhs, rhs, DOUBLE);
                assert_eq!(
                    native.compare(pair, false).0,
                    reference.compare(pair, false).0,
                    "{lhs:#x} vs {rhs:#x}"
                );
            }
        }
    }

    #[test]
    fn the_extrema_stay_on_the_reference_path() {
        // The host's own min/max disagree with the architecture about NaNs, so
        // the native backend must not use them.
        let native = Native::new();
        let quiet = 0x7ff8_0000_0000_0001;
        let pair = operands(quiet, 1.0f64.to_bits(), DOUBLE);

        assert_eq!(
            native
                .binary(FpBinaryOp::Max, pair, FpControl::DEFAULT)
                .bits,
            quiet,
            "FMAX propagates the NaN"
        );
        assert_eq!(
            native
                .binary(FpBinaryOp::MaxNum, pair, FpControl::DEFAULT)
                .bits,
            1.0f64.to_bits(),
            "FMAXNM ignores it"
        );
    }

    #[test]
    fn the_sign_operations_are_shared_by_both_backends() {
        // These come from the trait's default body, so both backends must give
        // the same answer for a signalling NaN, whose bits must not be touched.
        let signalling = FpValue::new(0x7ff0_0000_0000_0001, DOUBLE);

        for op in [FpSignOp::Negate, FpSignOp::Absolute] {
            assert_eq!(
                Native::new().copy_sign(signalling, op),
                Softfloat::new().copy_sign(signalling, op),
                "{op:?}"
            );
        }
    }

    #[test]
    fn format_conversion_agrees_with_the_reference() {
        let native = Native::new();
        let reference = Softfloat::new();

        for value in [1.0f64, -2.5, 0.1, 1.0 / 3.0, 1e300, 1e-300] {
            let source = FpValue::new(value.to_bits(), DOUBLE);
            assert_eq!(
                native
                    .convert_format(source, SINGLE, FpControl::DEFAULT)
                    .bits,
                reference
                    .convert_format(source, SINGLE, FpControl::DEFAULT)
                    .bits,
                "narrowing {value}"
            );
        }

        for value in [1.0f32, -2.5, 0.1, f32::MIN_POSITIVE] {
            let source = FpValue::new(value.to_bits() as u64, SINGLE);
            assert_eq!(
                native
                    .convert_format(source, DOUBLE, FpControl::DEFAULT)
                    .bits,
                reference
                    .convert_format(source, DOUBLE, FpControl::DEFAULT)
                    .bits,
                "widening {value}"
            );
        }
    }
}
