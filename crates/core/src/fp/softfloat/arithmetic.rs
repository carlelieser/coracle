//! Add, subtract, multiply, divide and square root over unpacked significands.
//!
//! Each operation resolves NaNs and infinities first, then computes an exact
//! significand with a sticky bit, and hands the pair to
//! [`super::decompose::round_and_pack`] for the single rounding step.

use super::decompose::{round_and_pack, unpack, RoundingContext, Unpacked, GUARD_BITS};
use super::nan;
use crate::fp::control::{FpControl, FpExceptions};
use crate::fp::operand::{FpFormat, FpResult, FpRounding};

/// Everything an operation needs beyond its operands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Context {
    /// Format of the operands and the result.
    pub format: FpFormat,
    /// Rounding applied to the result.
    pub rounding: FpRounding,
    /// FPCR, for NaN and flush-to-zero behaviour.
    pub control: FpControl,
}

impl Context {
    /// The rounding context for a result with the given sticky bit.
    fn rounding_context(self, is_sticky: bool) -> RoundingContext {
        RoundingContext {
            rounding: self.rounding,
            is_sticky,
            control: self.control,
        }
    }

    /// Rounds and packs a computed value.
    fn pack(self, value: Unpacked, is_sticky: bool) -> FpResult {
        round_and_pack(value, self.format, self.rounding_context(is_sticky))
    }
}

/// Flushes a subnormal input to zero when `FPCR.FZ` demands it.
///
/// Returns the possibly-replaced bits and whether the input-denormal flag was
/// raised. Applied before unpacking, so the arithmetic never sees a value the
/// guest's FPCR said to treat as zero.
fn flush_input(bits: u64, context: Context) -> (u64, FpExceptions) {
    let is_flushing = match context.format {
        FpFormat::Half => context.control.is_flush_to_zero_fp16,
        FpFormat::Single | FpFormat::Double => context.control.is_flush_to_zero,
    };
    if !is_flushing || !context.format.is_subnormal(bits) {
        return (bits, FpExceptions::NONE);
    }
    (
        context.format.zero(context.format.is_negative(bits)),
        FpExceptions::INPUT_DENORMAL,
    )
}

/// Applies input flushing to both operands.
fn flush_pair(lhs: u64, rhs: u64, context: Context) -> (u64, u64, FpExceptions) {
    let (lhs, from_lhs) = flush_input(lhs, context);
    let (rhs, from_rhs) = flush_input(rhs, context);
    (lhs, rhs, from_lhs.union(from_rhs))
}

/// Adds `raised` to a result's flags.
fn with_flags(result: FpResult, raised: FpExceptions) -> FpResult {
    FpResult::raising(result.bits, result.raised.union(raised))
}

/// `lhs + rhs`, or `lhs - rhs` when `is_subtracting`.
pub fn add(lhs: u64, rhs: u64, is_subtracting: bool, context: Context) -> FpResult {
    let (lhs, rhs, denormal) = flush_pair(lhs, rhs, context);
    let format = context.format;
    // Subtraction is addition with the second operand's sign flipped, which
    // keeps one implementation of the hard part.
    let rhs = if is_subtracting {
        rhs ^ (1u64 << format.sign_shift())
    } else {
        rhs
    };

    if let Some(result) = nan::propagate(&[lhs, rhs], format, context.control) {
        return with_flags(result, denormal);
    }
    if let Some(result) = add_infinities(lhs, rhs, context) {
        return with_flags(result, denormal);
    }

    let (value, is_sticky) = add_finite(unpack(lhs, format), unpack(rhs, format), context);
    with_flags(context.pack(value, is_sticky), denormal)
}

/// The infinity cases of addition, including `inf + -inf`, which is invalid.
fn add_infinities(lhs: u64, rhs: u64, context: Context) -> Option<FpResult> {
    let format = context.format;
    let (is_lhs_infinite, is_rhs_infinite) = (format.is_infinite(lhs), format.is_infinite(rhs));
    if !is_lhs_infinite && !is_rhs_infinite {
        return None;
    }

    if is_lhs_infinite && is_rhs_infinite {
        // Opposite signs have no defined sum.
        if format.is_negative(lhs) != format.is_negative(rhs) {
            return Some(nan::invalid_operation(format));
        }
        return Some(FpResult::exact(lhs));
    }
    Some(FpResult::exact(if is_lhs_infinite { lhs } else { rhs }))
}

/// Adds two finite unpacked values, returning the exact result and its sticky
/// bit.
fn add_finite(lhs: Unpacked, rhs: Unpacked, context: Context) -> (Unpacked, bool) {
    if lhs.is_zero() && rhs.is_zero() {
        // The sign of a zero sum: negative only when both are negative, except
        // that rounding toward -inf makes -0.0 the identity.
        let is_negative = if lhs.is_negative == rhs.is_negative {
            lhs.is_negative
        } else {
            matches!(context.rounding, FpRounding::Minus)
        };
        return (Unpacked::zero(is_negative), false);
    }
    if lhs.is_zero() {
        return (rhs, false);
    }
    if rhs.is_zero() {
        return (lhs, false);
    }

    let (larger, smaller) = if is_larger(lhs, rhs) {
        (lhs, rhs)
    } else {
        (rhs, lhs)
    };
    let shift = (larger.exponent - smaller.exponent) as u32;
    let (aligned, is_sticky) = align(smaller.significand, shift);

    if larger.is_negative == smaller.is_negative {
        return (
            Unpacked {
                significand: larger.significand + aligned,
                ..larger
            },
            is_sticky,
        );
    }
    subtract_aligned(larger, aligned, is_sticky, context)
}

/// Subtracts an aligned significand from the larger operand.
fn subtract_aligned(
    larger: Unpacked,
    aligned: u64,
    is_sticky: bool,
    context: Context,
) -> (Unpacked, bool) {
    // Borrowing one unit when the shifted-out bits were non-zero is what makes
    // the sticky bit correct for subtraction.
    let (significand, is_sticky) = if is_sticky {
        (larger.significand - aligned - 1, true)
    } else {
        (larger.significand - aligned, false)
    };

    if significand == 0 && !is_sticky {
        // An exact cancellation is +0.0 in every mode but round-toward-minus.
        return (
            Unpacked::zero(matches!(context.rounding, FpRounding::Minus)),
            false,
        );
    }
    (
        Unpacked {
            significand,
            ..larger
        },
        is_sticky,
    )
}

/// Whether `lhs` has the larger magnitude.
fn is_larger(lhs: Unpacked, rhs: Unpacked) -> bool {
    (lhs.exponent, lhs.significand) >= (rhs.exponent, rhs.significand)
}

/// Shifts a significand right by `shift`, reporting whether any set bit was
/// discarded.
fn align(significand: u64, shift: u32) -> (u64, bool) {
    if shift >= 64 {
        return (0, significand != 0);
    }
    let discarded = significand & ((1u64 << shift) - 1);
    (significand >> shift, discarded != 0)
}

/// `lhs * rhs`.
pub fn multiply(lhs: u64, rhs: u64, context: Context) -> FpResult {
    let (lhs, rhs, denormal) = flush_pair(lhs, rhs, context);
    let format = context.format;

    if let Some(result) = nan::propagate(&[lhs, rhs], format, context.control) {
        return with_flags(result, denormal);
    }
    let is_negative = format.is_negative(lhs) != format.is_negative(rhs);
    if let Some(result) = multiply_specials(lhs, rhs, is_negative, context) {
        return with_flags(result, denormal);
    }

    let (left, right) = (unpack(lhs, format), unpack(rhs, format));
    // The exact product of two (mantissa + 1 + guard)-bit significands needs
    // twice the width, so it is computed in u128 and folded back down.
    let product = (left.significand as u128) * (right.significand as u128);
    // Both significands carry an implicit bit at `mantissa + guard`, so the
    // product carries it at twice that; shifting by one copy returns the
    // result to the operands' own scale.
    let (significand, is_sticky) = narrow(product, significand_scale(format));

    let value = Unpacked {
        is_negative,
        exponent: left.exponent + right.exponent,
        significand,
    };
    with_flags(context.pack(value, is_sticky), denormal)
}

/// The zero and infinity cases of multiplication.
fn multiply_specials(lhs: u64, rhs: u64, is_negative: bool, context: Context) -> Option<FpResult> {
    let format = context.format;
    let (is_lhs_infinite, is_rhs_infinite) = (format.is_infinite(lhs), format.is_infinite(rhs));
    let (is_lhs_zero, is_rhs_zero) = (format.is_zero(lhs), format.is_zero(rhs));

    if (is_lhs_infinite && is_rhs_zero) || (is_lhs_zero && is_rhs_infinite) {
        return Some(nan::invalid_operation(format));
    }
    if is_lhs_infinite || is_rhs_infinite {
        return Some(FpResult::exact(format.signed_infinity(is_negative)));
    }
    if is_lhs_zero || is_rhs_zero {
        return Some(FpResult::exact(format.zero(is_negative)));
    }
    None
}

/// The bit position the implicit bit occupies in an unpacked significand.
///
/// Multiplication and division work at twice this scale and shift back by it;
/// square root works at twice it and halves.
const fn significand_scale(format: FpFormat) -> u32 {
    format.mantissa_bits() + GUARD_BITS
}

/// Shifts a 128-bit exact product down to 64 bits, keeping a sticky bit.
fn narrow(product: u128, shift: u32) -> (u64, bool) {
    let discarded = product & ((1u128 << shift) - 1);
    ((product >> shift) as u64, discarded != 0)
}

/// `lhs / rhs`.
pub fn divide(lhs: u64, rhs: u64, context: Context) -> FpResult {
    let (lhs, rhs, denormal) = flush_pair(lhs, rhs, context);
    let format = context.format;

    if let Some(result) = nan::propagate(&[lhs, rhs], format, context.control) {
        return with_flags(result, denormal);
    }
    let is_negative = format.is_negative(lhs) != format.is_negative(rhs);
    if let Some(result) = divide_specials(lhs, rhs, is_negative, context) {
        return with_flags(result, denormal);
    }

    let (left, right) = (unpack(lhs, format), unpack(rhs, format));
    // Scaling the numerator up by the significand width before dividing keeps
    // the quotient's precision; the remainder becomes the sticky bit.
    let numerator = (left.significand as u128) << significand_scale(format);
    let quotient = numerator / right.significand as u128;
    let is_sticky = !numerator.is_multiple_of(right.significand as u128);

    let value = Unpacked {
        is_negative,
        exponent: left.exponent - right.exponent,
        significand: quotient as u64,
    };
    with_flags(context.pack(value, is_sticky), denormal)
}

/// The zero and infinity cases of division, including division by zero.
fn divide_specials(lhs: u64, rhs: u64, is_negative: bool, context: Context) -> Option<FpResult> {
    let format = context.format;
    let (is_lhs_infinite, is_rhs_infinite) = (format.is_infinite(lhs), format.is_infinite(rhs));
    let (is_lhs_zero, is_rhs_zero) = (format.is_zero(lhs), format.is_zero(rhs));

    if (is_lhs_infinite && is_rhs_infinite) || (is_lhs_zero && is_rhs_zero) {
        return Some(nan::invalid_operation(format));
    }
    if is_lhs_infinite {
        return Some(FpResult::exact(format.signed_infinity(is_negative)));
    }
    if is_rhs_infinite {
        return Some(FpResult::exact(format.zero(is_negative)));
    }
    if is_rhs_zero {
        // A finite non-zero over zero is the one case that raises DZC.
        return Some(FpResult::raising(
            format.signed_infinity(is_negative),
            FpExceptions::DIVIDE_BY_ZERO,
        ));
    }
    if is_lhs_zero {
        return Some(FpResult::exact(format.zero(is_negative)));
    }
    None
}

/// Square root.
pub fn square_root(value: u64, context: Context) -> FpResult {
    let (value, denormal) = flush_input(value, context);
    let format = context.format;

    if let Some(result) = nan::propagate(&[value], format, context.control) {
        return with_flags(result, denormal);
    }
    if format.is_zero(value) {
        // sqrt(-0.0) is -0.0, not a NaN.
        return with_flags(FpResult::exact(value), denormal);
    }
    if format.is_negative(value) {
        return with_flags(nan::invalid_operation(format), denormal);
    }
    if format.is_infinite(value) {
        return with_flags(FpResult::exact(value), denormal);
    }

    let unpacked = unpack(value, format);
    let (significand, is_sticky) = integer_square_root(unpacked, format);
    let value = Unpacked {
        is_negative: false,
        exponent: unpacked.exponent.div_euclid(2),
        significand,
    };
    with_flags(context.pack(value, is_sticky), denormal)
}

/// The significand of the square root, computed as an integer square root.
///
/// An odd exponent is made even by doubling the significand first, because
/// halving the exponent must be exact.
fn integer_square_root(value: Unpacked, format: FpFormat) -> (u64, bool) {
    let mut scaled = value.significand as u128;
    if value.exponent.rem_euclid(2) != 0 {
        scaled <<= 1;
    }
    // sqrt halves the scale, so squaring the input's scale first leaves the
    // root back at the significand's own.
    scaled <<= significand_scale(format);

    let root = isqrt_u128(scaled);
    (root as u64, root * root != scaled)
}

/// Integer square root by bit-by-bit restoration.
///
/// Written out rather than using `u128::isqrt` because that is not available on
/// this crate's pinned toolchain in a `no_std` build.
fn isqrt_u128(value: u128) -> u128 {
    if value == 0 {
        return 0;
    }
    let mut remainder = value;
    let mut root = 0u128;
    // The highest even bit position at or below the value's magnitude.
    let mut bit = 1u128 << ((127 - value.leading_zeros()) & !1);

    while bit != 0 {
        if remainder >= root + bit {
            remainder -= root + bit;
            root = (root >> 1) + bit;
        } else {
            root >>= 1;
        }
        bit >>= 2;
    }
    root
}

#[cfg(test)]
mod tests {
    use super::*;

    const SINGLE: FpFormat = FpFormat::Single;
    const DOUBLE: FpFormat = FpFormat::Double;

    fn context(format: FpFormat) -> Context {
        Context {
            format,
            rounding: FpRounding::Nearest,
            control: FpControl::DEFAULT,
        }
    }

    /// Runs an operation on `f64` inputs and returns the result as an `f64`.
    fn double_op(operation: fn(u64, u64, Context) -> FpResult, lhs: f64, rhs: f64) -> f64 {
        f64::from_bits(operation(lhs.to_bits(), rhs.to_bits(), context(DOUBLE)).bits)
    }

    fn add_double(lhs: f64, rhs: f64) -> f64 {
        f64::from_bits(add(lhs.to_bits(), rhs.to_bits(), false, context(DOUBLE)).bits)
    }

    #[test]
    fn addition_matches_the_hosts_own_double_arithmetic() {
        // The host FPU is the oracle here: it is IEEE-754 in the same default
        // mode, so any disagreement is a softfloat bug.
        let cases = [
            (1.0, 2.0),
            (0.1, 0.2),
            (1.0, -1.0),
            (1e300, 1e300),
            (1.0, 1e-300),
            (-5.5, 2.25),
            (f64::MAX, f64::MAX),
            (f64::MIN_POSITIVE, f64::MIN_POSITIVE),
            (1.0, f64::MIN_POSITIVE),
            (123456.789, -0.000123),
        ];

        for (lhs, rhs) in cases {
            assert_eq!(
                add_double(lhs, rhs).to_bits(),
                (lhs + rhs).to_bits(),
                "{lhs} + {rhs}"
            );
        }
    }

    #[test]
    fn subtraction_matches_the_host() {
        let cases = [(1.0, 2.0), (0.3, 0.1), (1e300, -1e300), (1.0, 1.0)];

        for (lhs, rhs) in cases {
            let result = subtract(lhs, rhs);
            assert_eq!(result.to_bits(), (lhs - rhs).to_bits(), "{lhs} - {rhs}");
        }
    }

    fn subtract(lhs: f64, rhs: f64) -> f64 {
        f64::from_bits(add(lhs.to_bits(), rhs.to_bits(), true, context(DOUBLE)).bits)
    }

    #[test]
    fn multiplication_matches_the_host() {
        let cases = [
            (1.5, 2.0),
            (0.1, 0.3),
            (1e200, 1e200),
            (1e-200, 1e-200),
            (-3.0, 7.5),
            (f64::MAX, 0.5),
            (f64::MIN_POSITIVE, 0.5),
            (1.0 / 3.0, 3.0),
        ];

        for (lhs, rhs) in cases {
            assert_eq!(
                double_op(multiply, lhs, rhs).to_bits(),
                (lhs * rhs).to_bits(),
                "{lhs} * {rhs}"
            );
        }
    }

    #[test]
    fn division_matches_the_host() {
        let cases = [
            (1.0, 3.0),
            (10.0, 4.0),
            (1e300, 1e-10),
            (-7.0, 2.0),
            (1.0, f64::MAX),
            (f64::MIN_POSITIVE, 2.0),
        ];

        for (lhs, rhs) in cases {
            assert_eq!(
                double_op(divide, lhs, rhs).to_bits(),
                (lhs / rhs).to_bits(),
                "{lhs} / {rhs}"
            );
        }
    }

    #[test]
    fn square_root_matches_the_host() {
        for value in [
            0.0f64, 1.0, 2.0, 4.0, 9.0, 0.25, 1e300, 1e-300, 123456.789, 3.0,
        ] {
            let result = f64::from_bits(square_root(value.to_bits(), context(DOUBLE)).bits);
            assert_eq!(result.to_bits(), value.sqrt().to_bits(), "sqrt({value})");
        }
    }

    #[test]
    fn single_precision_arithmetic_matches_the_host() {
        let single = context(SINGLE);
        let cases = [(1.0f32, 3.0f32), (0.1, 0.2), (1e30, 1e30), (7.5, -2.25)];

        for (lhs, rhs) in cases {
            let sum = add(lhs.to_bits() as u64, rhs.to_bits() as u64, false, single);
            assert_eq!(
                f32::from_bits(sum.bits as u32).to_bits(),
                (lhs + rhs).to_bits()
            );

            let product = multiply(lhs.to_bits() as u64, rhs.to_bits() as u64, single);
            assert_eq!(
                f32::from_bits(product.bits as u32).to_bits(),
                (lhs * rhs).to_bits()
            );

            let quotient = divide(lhs.to_bits() as u64, rhs.to_bits() as u64, single);
            assert_eq!(
                f32::from_bits(quotient.bits as u32).to_bits(),
                (lhs / rhs).to_bits()
            );
        }
    }

    #[test]
    fn adding_opposite_infinities_is_invalid() {
        let result = add(
            DOUBLE.infinity(),
            DOUBLE.signed_infinity(true),
            false,
            context(DOUBLE),
        );

        assert_eq!(result.bits, DOUBLE.default_nan());
        assert!(result.raised.contains(FpExceptions::INVALID));
    }

    #[test]
    fn multiplying_zero_by_infinity_is_invalid() {
        let result = multiply(0, DOUBLE.infinity(), context(DOUBLE));

        assert_eq!(result.bits, DOUBLE.default_nan());
        assert!(result.raised.contains(FpExceptions::INVALID));
    }

    #[test]
    fn dividing_a_finite_by_zero_raises_divide_by_zero_not_invalid() {
        let result = divide(1.0f64.to_bits(), 0, context(DOUBLE));

        assert_eq!(result.bits, DOUBLE.infinity());
        assert!(result.raised.contains(FpExceptions::DIVIDE_BY_ZERO));
        assert!(!result.raised.contains(FpExceptions::INVALID));

        // The sign follows the operands, and 0/0 is the invalid case instead.
        let negative = divide((-1.0f64).to_bits(), 0, context(DOUBLE));
        assert_eq!(negative.bits, DOUBLE.signed_infinity(true));

        let zero_over_zero = divide(0, 0, context(DOUBLE));
        assert!(zero_over_zero.raised.contains(FpExceptions::INVALID));
        assert!(!zero_over_zero.raised.contains(FpExceptions::DIVIDE_BY_ZERO));
    }

    #[test]
    fn the_square_root_of_a_negative_is_invalid_but_of_negative_zero_is_not() {
        let negative = square_root((-1.0f64).to_bits(), context(DOUBLE));
        assert_eq!(negative.bits, DOUBLE.default_nan());
        assert!(negative.raised.contains(FpExceptions::INVALID));

        // sqrt(-0.0) = -0.0 is the exception the architecture makes.
        let negative_zero = square_root(DOUBLE.zero(true), context(DOUBLE));
        assert_eq!(negative_zero.bits, DOUBLE.zero(true));
        assert!(negative_zero.raised.is_empty());
    }

    #[test]
    fn the_sign_of_a_zero_sum_follows_the_rounding_mode() {
        // x + (-x) is +0.0 in every mode but round-toward-minus-infinity.
        let mut toward_minus = context(DOUBLE);
        toward_minus.rounding = FpRounding::Minus;

        assert_eq!(
            add(
                1.0f64.to_bits(),
                (-1.0f64).to_bits(),
                false,
                context(DOUBLE)
            )
            .bits,
            0
        );
        assert_eq!(
            add(1.0f64.to_bits(), (-1.0f64).to_bits(), false, toward_minus).bits,
            DOUBLE.zero(true)
        );
    }

    #[test]
    fn inexact_is_raised_exactly_when_the_result_was_rounded() {
        // 1 + 1 is exact; 0.1 + 0.2 is not.
        let exact = add(1.0f64.to_bits(), 1.0f64.to_bits(), false, context(DOUBLE));
        assert!(!exact.raised.contains(FpExceptions::INEXACT));

        let rounded = add(0.1f64.to_bits(), 0.2f64.to_bits(), false, context(DOUBLE));
        assert!(rounded.raised.contains(FpExceptions::INEXACT));
    }

    #[test]
    fn overflow_and_underflow_are_flagged() {
        let overflowed = multiply(f64::MAX.to_bits(), 2.0f64.to_bits(), context(DOUBLE));
        assert_eq!(overflowed.bits, DOUBLE.infinity());
        assert!(overflowed.raised.contains(FpExceptions::OVERFLOW));

        let underflowed = multiply(
            f64::MIN_POSITIVE.to_bits(),
            f64::MIN_POSITIVE.to_bits(),
            context(DOUBLE),
        );
        assert_eq!(underflowed.bits, 0);
        assert!(underflowed.raised.contains(FpExceptions::UNDERFLOW));
    }

    #[test]
    fn a_flushed_subnormal_input_raises_input_denormal_and_reads_as_zero() {
        let mut flushing = context(DOUBLE);
        flushing.control.is_flush_to_zero = true;

        // The smallest subnormal plus one is 1.0 exactly once the subnormal is
        // flushed; without flushing the sum is inexact.
        let result = add(1.0f64.to_bits(), 1, false, flushing);

        assert_eq!(result.bits, 1.0f64.to_bits());
        assert!(result.raised.contains(FpExceptions::INPUT_DENORMAL));
    }

    #[test]
    fn a_signalling_nan_operand_propagates_quietened_from_every_operation() {
        let signalling = 0x7ff0_0000_0000_0001;
        let quietened = 0x7ff8_0000_0000_0001;
        let one = 1.0f64.to_bits();

        for result in [
            add(signalling, one, false, context(DOUBLE)),
            multiply(signalling, one, context(DOUBLE)),
            divide(signalling, one, context(DOUBLE)),
            square_root(signalling, context(DOUBLE)),
        ] {
            assert_eq!(result.bits, quietened);
            assert!(result.raised.contains(FpExceptions::INVALID));
        }
    }

    #[test]
    fn arithmetic_agrees_with_the_host_across_a_pseudorandom_sweep() {
        // Fixed-seed sweep over normal-range values: a rounding bug that the
        // hand-picked cases miss shows up here.
        let mut state = 0x853c_49e6_748f_ea9bu64;
        let mut next = || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            // Keep the exponent in a range where nothing overflows, so the
            // comparison is about rounding rather than about limits.
            f64::from_bits((state & 0x800f_ffff_ffff_ffff) | 0x3ff0_0000_0000_0000)
        };

        for _ in 0..2000 {
            let (lhs, rhs) = (next(), next());
            assert_eq!(add_double(lhs, rhs).to_bits(), (lhs + rhs).to_bits());
            assert_eq!(subtract(lhs, rhs).to_bits(), (lhs - rhs).to_bits());
            assert_eq!(
                double_op(multiply, lhs, rhs).to_bits(),
                (lhs * rhs).to_bits()
            );
            assert_eq!(double_op(divide, lhs, rhs).to_bits(), (lhs / rhs).to_bits());
            let root = f64::from_bits(square_root(lhs.abs().to_bits(), context(DOUBLE)).bits);
            assert_eq!(root.to_bits(), lhs.abs().sqrt().to_bits());
        }
    }
}
