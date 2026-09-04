//! Conversions: FP to integer, integer to FP, between FP formats, and
//! round-to-integral.
//!
//! An out-of-range FP-to-integer conversion saturates and raises invalid
//! rather than wrapping, which is the rule that separates this from a cast.

use super::decompose::{round_and_pack, unpack, RoundingContext, Unpacked, GUARD_BITS};
use super::nan;
use crate::fp::backend::IntFormat;
use crate::fp::control::{FpControl, FpExceptions};
use crate::fp::operand::{FpFormat, FpResult, FpRounding};

/// Converts between two FP formats — `FCVT`.
pub fn convert_format(
    bits: u64,
    source: FpFormat,
    target: FpFormat,
    control: FpControl,
    rounding: FpRounding,
) -> FpResult {
    if source.is_nan(bits) {
        return convert_nan(bits, source, target, control);
    }
    if source.is_infinite(bits) {
        return FpResult::exact(target.signed_infinity(source.is_negative(bits)));
    }
    if source.is_zero(bits) {
        return FpResult::exact(target.zero(source.is_negative(bits)));
    }

    let value = unpack(bits, source);
    // The significand is re-scaled to the target's own width; widening is
    // exact, narrowing rounds.
    let significand = rescale(value.significand, source, target);
    round_and_pack(
        Unpacked {
            significand: significand.0,
            ..value
        },
        target,
        RoundingContext {
            rounding,
            is_sticky: significand.1,
            control,
        },
    )
}

/// Moves a significand between two formats' scales, reporting lost bits.
fn rescale(significand: u64, source: FpFormat, target: FpFormat) -> (u64, bool) {
    let source_scale = source.mantissa_bits() + GUARD_BITS;
    let target_scale = target.mantissa_bits() + GUARD_BITS;

    if target_scale >= source_scale {
        return (significand << (target_scale - source_scale), false);
    }
    let shift = source_scale - target_scale;
    let discarded = significand & ((1u64 << shift) - 1);
    (significand >> shift, discarded != 0)
}

/// Re-encodes a NaN into the target format, keeping as much payload as fits.
fn convert_nan(bits: u64, source: FpFormat, target: FpFormat, control: FpControl) -> FpResult {
    let raised = if source.is_signalling_nan(bits) {
        FpExceptions::INVALID
    } else {
        FpExceptions::NONE
    };
    if control.is_default_nan {
        return FpResult::raising(target.default_nan(), raised);
    }

    // The payload is left-aligned in both formats, so it is shifted rather
    // than truncated from the bottom.
    let payload = bits & source.mantissa_mask();
    let source_bits = source.mantissa_bits();
    let target_bits = target.mantissa_bits();
    let moved = if target_bits >= source_bits {
        payload << (target_bits - source_bits)
    } else {
        payload >> (source_bits - target_bits)
    };

    let sign = (source.is_negative(bits) as u64) << target.sign_shift();
    FpResult::raising(
        sign | target.infinity() | target.quiet_bit() | (moved & (target.quiet_bit() - 1)),
        raised,
    )
}

/// Rounds to an integral value in the same format — the `FRINT` family.
pub fn round_to_integral(
    bits: u64,
    format: FpFormat,
    rounding: FpRounding,
    control: FpControl,
) -> FpResult {
    if let Some(result) = nan::propagate(&[bits], format, control) {
        return result;
    }
    if format.is_infinite(bits) || format.is_zero(bits) {
        return FpResult::exact(bits);
    }

    let value = unpack(bits, format);
    let scale = format.mantissa_bits() + GUARD_BITS;
    // A value whose exponent is at or above the significand width is already
    // integral, so there is nothing to round.
    if value.exponent >= scale as i32 {
        return FpResult::exact(bits);
    }

    let Some(integral) = round_significand_to_integer(value, scale, rounding) else {
        // Everything below the units place was discarded, so the result is a
        // zero — signed, because `rint(-0.4)` is `-0.0`.
        return FpResult::exact(format.zero(value.is_negative));
    };

    round_and_pack(
        Unpacked {
            is_negative: value.is_negative,
            exponent: integral.exponent,
            significand: integral.significand,
        },
        format,
        RoundingContext {
            rounding,
            is_sticky: false,
            control,
        },
    )
}

/// The value rounded to an integer, still in unpacked form.
///
/// `None` means the magnitude rounded away entirely, which the caller turns
/// into a signed zero. The returned value carries its own exponent so it cannot
/// drift out of step with the significand the shifting produced.
fn round_significand_to_integer(
    value: Unpacked,
    scale: u32,
    rounding: FpRounding,
) -> Option<Unpacked> {
    // Bits below this position are the fractional part.
    let shift = (scale as i32 - value.exponent) as u32;
    if shift >= 64 {
        // Smaller than one unit; only a directed mode pointing away from zero
        // reaches it, and then the result is exactly one.
        let is_away = matches!(
            (rounding, value.is_negative),
            (FpRounding::Plus, false) | (FpRounding::Minus, true)
        );
        return is_away.then(|| Unpacked {
            exponent: 0,
            significand: 1u64 << scale,
            ..value
        });
    }

    let truncated = value.significand >> shift;
    let remainder = value.significand & ((1u64 << shift) - 1);
    let is_away = remainder != 0 && rounds_away(truncated, remainder, shift, rounding, value);
    let integral = truncated + is_away as u64;
    if integral == 0 {
        return None;
    }

    // Shifting back up restores the original exponent's scale, so the exponent
    // is unchanged rather than replaced.
    Some(Unpacked {
        significand: integral << shift,
        ..value
    })
}

/// Whether a non-zero fractional remainder rounds the magnitude up.
fn rounds_away(
    truncated: u64,
    remainder: u64,
    shift: u32,
    rounding: FpRounding,
    value: Unpacked,
) -> bool {
    let half = 1u64 << (shift - 1);
    match rounding {
        FpRounding::Nearest => remainder > half || (remainder == half && truncated & 1 == 1),
        FpRounding::NearestAway => remainder >= half,
        FpRounding::Zero => false,
        FpRounding::Plus => !value.is_negative,
        FpRounding::Minus => value.is_negative,
        FpRounding::Odd => truncated & 1 == 0,
    }
}

/// FP to integer, saturating on overflow.
pub fn to_integer(
    bits: u64,
    format: FpFormat,
    target: IntFormat,
    rounding: FpRounding,
) -> FpResult {
    if format.is_nan(bits) {
        // A NaN converts to zero and raises invalid, not to a saturated value.
        return FpResult::raising(0, FpExceptions::INVALID);
    }

    let is_negative = format.is_negative(bits);
    if format.is_infinite(bits) {
        return FpResult::raising(saturate(is_negative, target), FpExceptions::INVALID);
    }
    if format.is_zero(bits) {
        return FpResult::exact(0);
    }

    let value = unpack(bits, format);
    let scale = format.mantissa_bits() + GUARD_BITS;
    let (magnitude, is_inexact) = integral_magnitude(value, scale, rounding);

    encode_integer(magnitude, is_negative, is_inexact, target)
}

/// The rounded absolute value, as an integer, plus whether rounding lost
/// anything.
fn integral_magnitude(value: Unpacked, scale: u32, rounding: FpRounding) -> (u128, bool) {
    let shift = value.exponent - scale as i32;
    if shift >= 0 {
        // Shifting past 127 means the value is far out of any integer's range;
        // the caller saturates, so a saturated placeholder is enough.
        if shift >= 128 {
            return (u128::MAX, false);
        }
        return ((value.significand as u128) << shift, false);
    }

    let shift = (-shift) as u32;
    if shift >= 64 {
        let is_away = matches!(
            (rounding, value.is_negative),
            (FpRounding::Plus, false) | (FpRounding::Minus, true)
        );
        return (is_away as u128, true);
    }

    let truncated = value.significand >> shift;
    let remainder = value.significand & ((1u64 << shift) - 1);
    if remainder == 0 {
        return (truncated as u128, false);
    }

    let half = 1u64 << (shift - 1);
    let is_away = match rounding {
        FpRounding::Nearest => remainder > half || (remainder == half && truncated & 1 == 1),
        FpRounding::NearestAway => remainder >= half,
        FpRounding::Zero => false,
        FpRounding::Plus => !value.is_negative,
        FpRounding::Minus => value.is_negative,
        FpRounding::Odd => truncated & 1 == 0,
    };
    ((truncated + is_away as u64) as u128, true)
}

/// Fits a magnitude into the target integer, saturating out of range.
fn encode_integer(
    magnitude: u128,
    is_negative: bool,
    is_inexact: bool,
    target: IntFormat,
) -> FpResult {
    let inexact = if is_inexact {
        FpExceptions::INEXACT
    } else {
        FpExceptions::NONE
    };

    if !target.is_signed && is_negative {
        // A negative value has no unsigned representation; only an exact zero
        // would, and that was handled before unpacking.
        return FpResult::raising(0, FpExceptions::INVALID);
    }

    let limit = magnitude_limit(is_negative, target);
    if magnitude > limit {
        return FpResult::raising(saturate(is_negative, target), FpExceptions::INVALID);
    }

    let value = if is_negative {
        (magnitude as u64).wrapping_neg()
    } else {
        magnitude as u64
    };
    FpResult::raising(mask_to_width(value, target), inexact)
}

/// The largest magnitude the target can hold with this sign.
fn magnitude_limit(is_negative: bool, target: IntFormat) -> u128 {
    let width = target.bits;
    if !target.is_signed {
        return (1u128 << width) - 1;
    }
    if is_negative {
        1u128 << (width - 1)
    } else {
        (1u128 << (width - 1)) - 1
    }
}

/// The saturated result for an out-of-range conversion.
fn saturate(is_negative: bool, target: IntFormat) -> u64 {
    let width = target.bits;
    let value = if !target.is_signed {
        if is_negative {
            0
        } else {
            u64::MAX
        }
    } else if is_negative {
        1u64 << (width - 1)
    } else {
        (1u64 << (width - 1)) - 1
    };
    mask_to_width(value, target)
}

/// Truncates to the target's width.
fn mask_to_width(value: u64, target: IntFormat) -> u64 {
    if target.bits >= 64 {
        value
    } else {
        value & ((1u64 << target.bits) - 1)
    }
}

/// Integer to FP.
pub fn from_integer(
    value: u64,
    source: IntFormat,
    target: FpFormat,
    rounding: FpRounding,
    control: FpControl,
) -> FpResult {
    let (magnitude, is_negative) = magnitude_of(value, source);
    if magnitude == 0 {
        return FpResult::exact(target.zero(false));
    }

    let scale = target.mantissa_bits() + GUARD_BITS;
    // The exponent is the position of the leading bit; the significand is then
    // aligned to the format's own scale.
    let leading = 63 - magnitude.leading_zeros();
    let (significand, is_sticky) = if leading <= scale {
        (magnitude << (scale - leading), false)
    } else {
        let shift = leading - scale;
        let discarded = magnitude & ((1u64 << shift) - 1);
        (magnitude >> shift, discarded != 0)
    };

    round_and_pack(
        Unpacked {
            is_negative,
            exponent: leading as i32,
            significand,
        },
        target,
        RoundingContext {
            rounding,
            is_sticky,
            control,
        },
    )
}

/// Splits a possibly-signed integer into magnitude and sign.
fn magnitude_of(value: u64, source: IntFormat) -> (u64, bool) {
    let value = mask_to_width(value, source);
    if !source.is_signed {
        return (value, false);
    }

    let sign_bit = 1u64 << (source.bits - 1);
    if value & sign_bit == 0 {
        return (value, false);
    }
    // Two's complement negation, within the source's width.
    (mask_to_width(value.wrapping_neg(), source), true)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SINGLE: FpFormat = FpFormat::Single;
    const DOUBLE: FpFormat = FpFormat::Double;
    const HALF: FpFormat = FpFormat::Half;

    fn to_signed_64(value: f64, rounding: FpRounding) -> u64 {
        to_integer(value.to_bits(), DOUBLE, IntFormat::S64, rounding).bits
    }

    #[test]
    fn widening_a_single_to_a_double_is_exact() {
        for value in [1.0f32, -2.5, 0.1, f32::MIN_POSITIVE, f32::MAX] {
            let result = convert_format(
                value.to_bits() as u64,
                SINGLE,
                DOUBLE,
                FpControl::DEFAULT,
                FpRounding::Nearest,
            );
            assert_eq!(
                f64::from_bits(result.bits).to_bits(),
                (value as f64).to_bits(),
                "{value}"
            );
            assert!(result.raised.is_empty(), "widening never rounds");
        }
    }

    #[test]
    fn narrowing_a_double_to_a_single_matches_the_hosts_cast() {
        for value in [1.0f64, -2.5, 0.1, 1.0 / 3.0, 1e-300, 1e300, 123456.789] {
            let result = convert_format(
                value.to_bits(),
                DOUBLE,
                SINGLE,
                FpControl::DEFAULT,
                FpRounding::Nearest,
            );
            assert_eq!(result.bits as u32, (value as f32).to_bits(), "{value}");
        }
    }

    #[test]
    fn narrowing_preserves_infinities_and_signed_zeroes() {
        let cases = [
            (f64::INFINITY.to_bits(), f32::INFINITY.to_bits()),
            (f64::NEG_INFINITY.to_bits(), f32::NEG_INFINITY.to_bits()),
            (0.0f64.to_bits(), 0.0f32.to_bits()),
            ((-0.0f64).to_bits(), (-0.0f32).to_bits()),
        ];

        for (source, expected) in cases {
            let result = convert_format(
                source,
                DOUBLE,
                SINGLE,
                FpControl::DEFAULT,
                FpRounding::Nearest,
            );
            assert_eq!(result.bits as u32, expected);
        }
    }

    #[test]
    fn a_signalling_nan_converts_to_a_quiet_one_and_raises() {
        let result = convert_format(
            0x7f80_0001,
            SINGLE,
            DOUBLE,
            FpControl::DEFAULT,
            FpRounding::Nearest,
        );

        assert!(DOUBLE.is_nan(result.bits));
        assert!(!DOUBLE.is_signalling_nan(result.bits), "quietened");
        assert!(result.raised.contains(FpExceptions::INVALID));
    }

    #[test]
    fn half_precision_round_trips_through_single() {
        // 1.0 and 2.0 are exact in both, so a scale error would show up.
        for value in [1.0f32, 2.0, -0.5, 0.0] {
            let narrowed = convert_format(
                value.to_bits() as u64,
                SINGLE,
                HALF,
                FpControl::DEFAULT,
                FpRounding::Nearest,
            );
            let widened = convert_format(
                narrowed.bits,
                HALF,
                SINGLE,
                FpControl::DEFAULT,
                FpRounding::Nearest,
            );
            assert_eq!(f32::from_bits(widened.bits as u32), value, "{value}");
        }
    }

    #[test]
    fn converting_to_a_signed_integer_truncates_toward_zero() {
        let cases = [
            (1.9f64, 1i64),
            (-1.9, -1),
            (0.5, 0),
            (-0.5, 0),
            (2.0, 2),
            (-1000.75, -1000),
            (1e15, 1_000_000_000_000_000),
        ];

        for (value, expected) in cases {
            assert_eq!(
                to_signed_64(value, FpRounding::Zero) as i64,
                expected,
                "trunc({value})"
            );
        }
    }

    #[test]
    fn each_rounding_mode_moves_the_expected_way() {
        let value = 2.5f64;
        let expected = [
            (FpRounding::Zero, 2i64),
            (FpRounding::Plus, 3),
            (FpRounding::Minus, 2),
            (FpRounding::Nearest, 2),
            (FpRounding::NearestAway, 3),
        ];

        for (rounding, want) in expected {
            assert_eq!(to_signed_64(value, rounding) as i64, want, "{rounding:?}");
        }

        // A negative operand flips which directed mode rounds away.
        assert_eq!(to_signed_64(-2.5, FpRounding::Minus) as i64, -3);
        assert_eq!(to_signed_64(-2.5, FpRounding::Plus) as i64, -2);
        // Ties to even rounds 3.5 up and 2.5 down.
        assert_eq!(to_signed_64(3.5, FpRounding::Nearest) as i64, 4);
    }

    #[test]
    fn an_out_of_range_conversion_saturates_and_raises_invalid() {
        let too_big = to_integer(1e300f64.to_bits(), DOUBLE, IntFormat::S32, FpRounding::Zero);
        assert_eq!(too_big.bits as u32, i32::MAX as u32);
        assert!(too_big.raised.contains(FpExceptions::INVALID));

        let too_small = to_integer(
            (-1e300f64).to_bits(),
            DOUBLE,
            IntFormat::S32,
            FpRounding::Zero,
        );
        assert_eq!(too_small.bits as u32, i32::MIN as u32);
        assert!(too_small.raised.contains(FpExceptions::INVALID));
    }

    #[test]
    fn a_negative_value_converts_to_zero_and_raises_for_an_unsigned_target() {
        let result = to_integer(
            (-1.5f64).to_bits(),
            DOUBLE,
            IntFormat::U64,
            FpRounding::Zero,
        );

        assert_eq!(result.bits, 0);
        assert!(result.raised.contains(FpExceptions::INVALID));
    }

    #[test]
    fn a_nan_converts_to_zero_rather_than_a_saturated_value() {
        // This is the case that separates a conversion from a saturating cast.
        let result = to_integer(f64::NAN.to_bits(), DOUBLE, IntFormat::S64, FpRounding::Zero);

        assert_eq!(result.bits, 0);
        assert!(result.raised.contains(FpExceptions::INVALID));
    }

    #[test]
    fn an_infinity_saturates_with_the_right_sign() {
        let positive = to_integer(
            f64::INFINITY.to_bits(),
            DOUBLE,
            IntFormat::S64,
            FpRounding::Zero,
        );
        assert_eq!(positive.bits as i64, i64::MAX);

        let negative = to_integer(
            f64::NEG_INFINITY.to_bits(),
            DOUBLE,
            IntFormat::S64,
            FpRounding::Zero,
        );
        assert_eq!(negative.bits as i64, i64::MIN);
    }

    #[test]
    fn integer_to_fp_matches_the_hosts_own_cast() {
        let signed = [
            0i64,
            1,
            -1,
            42,
            -42,
            1 << 40,
            -(1 << 40),
            i64::MAX,
            i64::MIN,
        ];

        for value in signed {
            let result = from_integer(
                value as u64,
                IntFormat::S64,
                DOUBLE,
                FpRounding::Nearest,
                FpControl::DEFAULT,
            );
            assert_eq!(
                f64::from_bits(result.bits).to_bits(),
                (value as f64).to_bits(),
                "{value} as f64"
            );
        }

        for value in [0u64, 1, u64::MAX, 1 << 63, 12345678901234567890] {
            let result = from_integer(
                value,
                IntFormat::U64,
                DOUBLE,
                FpRounding::Nearest,
                FpControl::DEFAULT,
            );
            assert_eq!(
                f64::from_bits(result.bits).to_bits(),
                (value as f64).to_bits(),
                "{value} as f64"
            );
        }
    }

    #[test]
    fn a_32_bit_source_reads_only_its_own_width() {
        // The upper half of the register is not part of a W-form operand, so
        // rubbish there must not change the result.
        let result = from_integer(
            0xdead_beef_0000_002a,
            IntFormat::S32,
            DOUBLE,
            FpRounding::Nearest,
            FpControl::DEFAULT,
        );

        assert_eq!(f64::from_bits(result.bits), 42.0);
    }

    #[test]
    fn a_negative_32_bit_integer_sign_extends_from_its_own_width() {
        let result = from_integer(
            0xffff_ffffu64,
            IntFormat::S32,
            DOUBLE,
            FpRounding::Nearest,
            FpControl::DEFAULT,
        );

        assert_eq!(f64::from_bits(result.bits), -1.0);

        // The same bits read as unsigned are a large positive number.
        let unsigned = from_integer(
            0xffff_ffffu64,
            IntFormat::U32,
            DOUBLE,
            FpRounding::Nearest,
            FpControl::DEFAULT,
        );
        assert_eq!(f64::from_bits(unsigned.bits), 4294967295.0);
    }

    #[test]
    fn round_to_integral_matches_the_hosts_own_rounding() {
        let cases = [
            (1.5f64, FpRounding::Nearest, 2.0),
            (2.5, FpRounding::Nearest, 2.0),
            (-1.5, FpRounding::Nearest, -2.0),
            (1.5, FpRounding::NearestAway, 2.0),
            (2.5, FpRounding::NearestAway, 3.0),
            (1.1, FpRounding::Plus, 2.0),
            (1.9, FpRounding::Minus, 1.0),
            (-1.1, FpRounding::Zero, -1.0),
            (0.4, FpRounding::Nearest, 0.0),
            (1e300, FpRounding::Nearest, 1e300),
        ];

        for (value, rounding, expected) in cases {
            let result = round_to_integral(value.to_bits(), DOUBLE, rounding, FpControl::DEFAULT);
            assert_eq!(
                f64::from_bits(result.bits),
                expected,
                "rint({value}, {rounding:?})"
            );
        }
    }

    #[test]
    fn round_to_integral_keeps_the_sign_of_a_zero_result() {
        // rint(-0.4) is -0.0, not +0.0. A sign lost here is invisible until a
        // differential run compares bit patterns.
        let result = round_to_integral(
            (-0.4f64).to_bits(),
            DOUBLE,
            FpRounding::Nearest,
            FpControl::DEFAULT,
        );

        assert_eq!(result.bits, DOUBLE.zero(true));
    }

    #[test]
    fn round_to_integral_leaves_infinities_and_nans_alone() {
        let infinity = round_to_integral(
            f64::INFINITY.to_bits(),
            DOUBLE,
            FpRounding::Nearest,
            FpControl::DEFAULT,
        );
        assert_eq!(infinity.bits, DOUBLE.infinity());

        let nan = round_to_integral(
            0x7ff0_0000_0000_0001,
            DOUBLE,
            FpRounding::Nearest,
            FpControl::DEFAULT,
        );
        assert!(DOUBLE.is_nan(nan.bits));
        assert!(nan.raised.contains(FpExceptions::INVALID));
    }

    #[test]
    fn conversions_agree_with_the_host_across_a_pseudorandom_sweep() {
        let mut state = 0xd1b5_4a32_d192_ed03u64;
        let mut next = || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };

        for _ in 0..2000 {
            let raw = next();
            let value = f64::from_bits((raw & 0x800f_ffff_ffff_ffff) | 0x40b0_0000_0000_0000);

            assert_eq!(
                to_signed_64(value, FpRounding::Zero) as i64,
                value as i64,
                "trunc({value})"
            );
            let narrowed = convert_format(
                value.to_bits(),
                DOUBLE,
                SINGLE,
                FpControl::DEFAULT,
                FpRounding::Nearest,
            );
            assert_eq!(narrowed.bits as u32, (value as f32).to_bits(), "{value}");

            let integer = raw as i64;
            let converted = from_integer(
                integer as u64,
                IntFormat::S64,
                DOUBLE,
                FpRounding::Nearest,
                FpControl::DEFAULT,
            );
            assert_eq!(
                f64::from_bits(converted.bits).to_bits(),
                (integer as f64).to_bits(),
                "{integer} as f64"
            );
        }
    }
}
