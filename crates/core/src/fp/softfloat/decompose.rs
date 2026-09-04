//! Unpacking an IEEE value into sign, exponent and significand, and packing a
//! rounded result back.
//!
//! The intermediate form is deliberately wider than any supported format: the
//! arithmetic computes an exact significand and a "sticky" bit recording
//! whether anything was discarded below it, and rounding consults both. That is
//! what makes a fused multiply-add round once rather than twice.

use crate::fp::control::{FpControl, FpExceptions};
use crate::fp::operand::{FpFormat, FpResult, FpRounding};

/// Extra bits kept below the significand while computing.
///
/// Three is the classic guard/round/sticky triple, which is sufficient for
/// correctly rounded add, multiply and divide.
pub const GUARD_BITS: u32 = 3;

/// A finite value pulled apart, with the significand left-aligned to include
/// [`GUARD_BITS`] of headroom.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Unpacked {
    /// Sign bit.
    pub is_negative: bool,
    /// Unbiased exponent of the significand's most significant bit.
    pub exponent: i32,
    /// Significand, with the implicit bit made explicit and `GUARD_BITS` of
    /// zeroes appended.
    pub significand: u64,
}

impl Unpacked {
    /// An exact zero of the given sign.
    pub const fn zero(is_negative: bool) -> Self {
        Self {
            is_negative,
            exponent: 0,
            significand: 0,
        }
    }

    /// Whether the significand is zero.
    pub const fn is_zero(self) -> bool {
        self.significand == 0
    }
}

/// Splits a finite value into [`Unpacked`] form.
///
/// Subnormals are normalised: the exponent is reduced until the leading bit
/// sits where a normal value's would, so the arithmetic never special-cases
/// them again.
pub fn unpack(bits: u64, format: FpFormat) -> Unpacked {
    let is_negative = format.is_negative(bits);
    let raw_exponent = ((bits >> format.mantissa_bits()) & format.max_exponent() as u64) as i32;
    let mantissa = bits & format.mantissa_mask();

    if raw_exponent == 0 {
        if mantissa == 0 {
            return Unpacked::zero(is_negative);
        }
        // Subnormal: no implicit leading bit, and the exponent is fixed at the
        // minimum rather than being `raw_exponent - bias`. The shift moves the
        // leading significand bit up to the implicit-bit position, one above
        // the mantissa field, which is why it is measured against
        // `mantissa_bits + 1`.
        let shift = mantissa.leading_zeros() - (64 - (format.mantissa_bits() + 1));
        return Unpacked {
            is_negative,
            exponent: 1 - format.exponent_bias() - shift as i32,
            significand: (mantissa << (shift + GUARD_BITS)) & significand_mask(format),
        };
    }

    Unpacked {
        is_negative,
        exponent: raw_exponent - format.exponent_bias(),
        significand: (mantissa | (1u64 << format.mantissa_bits())) << GUARD_BITS,
    }
}

/// Mask covering a normalised significand plus its guard bits.
const fn significand_mask(format: FpFormat) -> u64 {
    (1u64 << (format.mantissa_bits() + 1 + GUARD_BITS)) - 1
}

/// The significand value with only the implicit bit set.
const fn implicit_bit(format: FpFormat) -> u64 {
    1u64 << (format.mantissa_bits() + GUARD_BITS)
}

/// Rounds an [`Unpacked`] result and packs it into `format`.
///
/// `is_sticky` says whether any non-zero bit was discarded below the
/// significand's guard bits — division and the wide multiply set it, exact
/// addition does not.
pub fn round_and_pack(value: Unpacked, format: FpFormat, context: RoundingContext) -> FpResult {
    if value.is_zero() {
        return FpResult::exact(format.zero(value.is_negative));
    }

    let normalised = normalise(value, context.is_sticky, format);
    let overflowed = pack_normalised(normalised, format, context);
    apply_flush_to_zero(overflowed, format, context.control)
}

/// What rounding needs to know beyond the value itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RoundingContext {
    /// Rounding mode in force.
    pub rounding: FpRounding,
    /// Whether bits were discarded below the guard bits.
    pub is_sticky: bool,
    /// FPCR, for flush-to-zero.
    pub control: FpControl,
}

/// Shifts the significand so its leading bit sits at the implicit-bit
/// position, folding anything shifted out into the sticky bit.
fn normalise(value: Unpacked, is_sticky: bool, format: FpFormat) -> (Unpacked, bool) {
    let mut value = value;
    let mut is_sticky = is_sticky;
    let target = implicit_bit(format);

    while value.significand >= target << 1 {
        is_sticky |= value.significand & 1 != 0;
        value.significand >>= 1;
        value.exponent += 1;
    }
    while value.significand < target && value.significand != 0 {
        value.significand <<= 1;
        value.exponent -= 1;
    }
    (value, is_sticky)
}

/// Applies the rounding mode and encodes the result, handling subnormal and
/// overflow ranges.
fn pack_normalised(
    (value, is_sticky): (Unpacked, bool),
    format: FpFormat,
    context: RoundingContext,
) -> FpResult {
    let minimum_exponent = 1 - format.exponent_bias();
    if value.exponent < minimum_exponent {
        return pack_subnormal(value, is_sticky, format, context);
    }

    let (significand, exponent, raised) = round_significand(
        value.significand,
        value.exponent,
        is_sticky,
        context.rounding,
        value.is_negative,
    );

    if exponent > format.exponent_bias() {
        return overflow(value.is_negative, format, context.rounding);
    }

    let biased = (exponent + format.exponent_bias()) as u64;
    let bits = ((value.is_negative as u64) << format.sign_shift())
        | (biased << format.mantissa_bits())
        | ((significand >> GUARD_BITS) & format.mantissa_mask());
    FpResult::raising(bits, raised)
}

/// Rounds a normalised significand, returning it renormalised if rounding
/// carried out of the implicit bit.
fn round_significand(
    significand: u64,
    exponent: i32,
    is_sticky: bool,
    rounding: FpRounding,
    is_negative: bool,
) -> (u64, i32, FpExceptions) {
    let discarded = significand & ((1 << GUARD_BITS) - 1);
    let is_inexact = discarded != 0 || is_sticky;
    if !is_inexact {
        return (significand, exponent, FpExceptions::NONE);
    }

    let increment = should_round_up(discarded, is_sticky, significand, rounding, is_negative);
    let mut rounded = significand + if increment { 1 << GUARD_BITS } else { 0 };
    rounded &= !((1u64 << GUARD_BITS) - 1);

    let mut exponent = exponent;
    // Rounding up can carry into the next binade.
    if rounded.leading_zeros() < significand.leading_zeros() {
        rounded >>= 1;
        exponent += 1;
    }
    (rounded, exponent, FpExceptions::INEXACT)
}

/// The rounding decision, given the discarded bits.
fn should_round_up(
    discarded: u64,
    is_sticky: bool,
    significand: u64,
    rounding: FpRounding,
    is_negative: bool,
) -> bool {
    let half = 1u64 << (GUARD_BITS - 1);
    let is_above_half = discarded > half || (discarded == half && is_sticky);
    let is_exactly_half = discarded == half && !is_sticky;

    match rounding {
        FpRounding::Nearest => {
            is_above_half || (is_exactly_half && significand & (1 << GUARD_BITS) != 0)
        }
        FpRounding::NearestAway => discarded >= half,
        FpRounding::Zero => false,
        FpRounding::Plus => !is_negative,
        FpRounding::Minus => is_negative,
        // FCVTXN rounds ties to odd; it never reaches the generic packer,
        // which is why this arm mirrors truncation rather than guessing.
        FpRounding::Odd => false,
    }
}

/// Encodes a value whose exponent is below the format's minimum.
fn pack_subnormal(
    value: Unpacked,
    is_sticky: bool,
    format: FpFormat,
    context: RoundingContext,
) -> FpResult {
    let minimum_exponent = 1 - format.exponent_bias();
    let shift = (minimum_exponent - value.exponent) as u32;
    let width = format.mantissa_bits() + 1 + GUARD_BITS;
    if shift >= width {
        // Everything is shifted out; the result is a zero, but a non-zero input
        // still underflowed inexactly.
        let raised = FpExceptions::UNDERFLOW.union(FpExceptions::INEXACT);
        return FpResult::raising(
            round_to_zero_or_minimum(value.is_negative, format, context.rounding),
            raised,
        );
    }

    let is_sticky = is_sticky || value.significand & ((1u64 << shift) - 1) != 0;
    let shifted = value.significand >> shift;
    let (significand, _, inexact) = round_significand(
        shifted,
        minimum_exponent,
        is_sticky,
        context.rounding,
        value.is_negative,
    );

    let mantissa = significand >> GUARD_BITS;
    // Rounding a subnormal up can reach the smallest normal, which encodes
    // naturally: the implicit bit lands in the exponent field.
    let bits = ((value.is_negative as u64) << format.sign_shift()) | mantissa;
    let raised = if inexact.is_empty() {
        FpExceptions::NONE
    } else {
        inexact.union(FpExceptions::UNDERFLOW)
    };
    FpResult::raising(bits, raised)
}

/// What a total underflow rounds to, which is not always zero: rounding away
/// from zero produces the smallest subnormal.
fn round_to_zero_or_minimum(is_negative: bool, format: FpFormat, rounding: FpRounding) -> u64 {
    let is_away = match rounding {
        FpRounding::Plus => !is_negative,
        FpRounding::Minus => is_negative,
        _ => false,
    };
    format.zero(is_negative) | is_away as u64
}

/// What an overflow rounds to, which depends on the mode and the sign.
fn overflow(is_negative: bool, format: FpFormat, rounding: FpRounding) -> FpResult {
    let goes_to_infinity = match rounding {
        FpRounding::Nearest | FpRounding::NearestAway => true,
        FpRounding::Zero => false,
        FpRounding::Plus => !is_negative,
        FpRounding::Minus => is_negative,
        FpRounding::Odd => false,
    };

    let bits = if goes_to_infinity {
        format.signed_infinity(is_negative)
    } else {
        format.max_finite() | format.zero(is_negative)
    };
    FpResult::raising(bits, FpExceptions::OVERFLOW.union(FpExceptions::INEXACT))
}

/// Replaces a subnormal result with a zero when `FPCR.FZ` is set.
fn apply_flush_to_zero(result: FpResult, format: FpFormat, control: FpControl) -> FpResult {
    let is_flushing = match format {
        FpFormat::Half => control.is_flush_to_zero_fp16,
        FpFormat::Single | FpFormat::Double => control.is_flush_to_zero,
    };
    if !is_flushing || !format.is_subnormal(result.bits) {
        return result;
    }

    FpResult::raising(
        format.zero(format.is_negative(result.bits)),
        result
            .raised
            .union(FpExceptions::UNDERFLOW)
            .union(FpExceptions::INEXACT),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const SINGLE: FpFormat = FpFormat::Single;
    const DOUBLE: FpFormat = FpFormat::Double;

    fn context(rounding: FpRounding) -> RoundingContext {
        RoundingContext {
            rounding,
            is_sticky: false,
            control: FpControl::DEFAULT,
        }
    }

    /// Unpack then repack must be the identity for every finite value.
    fn round_trip(bits: u64, format: FpFormat) -> u64 {
        round_and_pack(unpack(bits, format), format, context(FpRounding::Nearest)).bits
    }

    #[test]
    fn unpacking_a_normal_value_exposes_the_implicit_bit() {
        let one = unpack(1.0f32.to_bits() as u64, SINGLE);

        assert!(!one.is_negative);
        assert_eq!(one.exponent, 0, "1.0 is 1.significand x 2^0");
        assert_eq!(one.significand, implicit_bit(SINGLE));
    }

    #[test]
    fn unpacking_normalises_a_subnormal() {
        // The smallest single subnormal is 2^-149.
        let smallest = unpack(1, SINGLE);

        assert_eq!(smallest.exponent, -149);
        assert_eq!(smallest.significand, implicit_bit(SINGLE));
    }

    #[test]
    fn unpacking_distinguishes_the_two_zeroes() {
        assert!(unpack(0, SINGLE).is_zero());
        assert!(!unpack(0, SINGLE).is_negative);
        assert!(unpack(0x8000_0000, SINGLE).is_zero());
        assert!(unpack(0x8000_0000, SINGLE).is_negative);
    }

    #[test]
    fn packing_an_unpacked_value_returns_the_original_bits() {
        let values = [
            0u64,
            0x8000_0000,
            1.0f32.to_bits() as u64,
            (-2.5f32).to_bits() as u64,
            f32::MIN_POSITIVE.to_bits() as u64,
            f32::MAX.to_bits() as u64,
            1,
            0x007f_ffff,
            (1.0f32 / 3.0).to_bits() as u64,
        ];

        for bits in values {
            assert_eq!(round_trip(bits, SINGLE), bits, "single {bits:#x}");
        }

        for bits in [
            1.0f64.to_bits(),
            (-1.0e-300f64).to_bits(),
            f64::MAX.to_bits(),
            f64::MIN_POSITIVE.to_bits(),
            1,
        ] {
            assert_eq!(round_trip(bits, DOUBLE), bits, "double {bits:#x}");
        }
    }

    #[test]
    fn rounding_to_nearest_breaks_ties_toward_an_even_significand() {
        // A significand of ...1 with exactly half discarded rounds up; ...0
        // stays. Build both directly in unpacked form.
        let half = 1u64 << (GUARD_BITS - 1);
        let odd = Unpacked {
            is_negative: false,
            exponent: 0,
            significand: implicit_bit(SINGLE) | (1 << GUARD_BITS) | half,
        };
        let even = Unpacked {
            is_negative: false,
            exponent: 0,
            significand: implicit_bit(SINGLE) | half,
        };

        let rounded_odd = round_and_pack(odd, SINGLE, context(FpRounding::Nearest));
        let rounded_even = round_and_pack(even, SINGLE, context(FpRounding::Nearest));

        assert_eq!(rounded_odd.bits & 0b11, 0b10, "rounded up to even");
        assert_eq!(rounded_even.bits & 0b1, 0, "stayed even");
        assert!(rounded_odd.raised.contains(FpExceptions::INEXACT));
    }

    #[test]
    fn directed_rounding_moves_the_expected_way() {
        let half = 1u64 << (GUARD_BITS - 1);
        let build = |is_negative| Unpacked {
            is_negative,
            exponent: 0,
            significand: implicit_bit(SINGLE) | half,
        };

        let up = round_and_pack(build(false), SINGLE, context(FpRounding::Plus));
        let down = round_and_pack(build(false), SINGLE, context(FpRounding::Minus));
        assert_eq!(up.bits, down.bits + 1, "toward +inf exceeds toward -inf");

        // The signs swap which mode rounds away from zero.
        let negative_up = round_and_pack(build(true), SINGLE, context(FpRounding::Plus));
        let negative_down = round_and_pack(build(true), SINGLE, context(FpRounding::Minus));
        assert_eq!(negative_down.bits, negative_up.bits + 1);
    }

    #[test]
    fn overflow_goes_to_infinity_or_the_largest_finite_by_mode() {
        let huge = Unpacked {
            is_negative: false,
            exponent: 200,
            significand: implicit_bit(SINGLE),
        };

        assert_eq!(
            round_and_pack(huge, SINGLE, context(FpRounding::Nearest)).bits,
            SINGLE.infinity()
        );
        assert_eq!(
            round_and_pack(huge, SINGLE, context(FpRounding::Zero)).bits,
            SINGLE.max_finite(),
            "toward zero cannot reach infinity"
        );
        assert_eq!(
            round_and_pack(huge, SINGLE, context(FpRounding::Minus)).bits,
            SINGLE.max_finite(),
            "a positive overflow toward -inf stops at the largest finite"
        );

        let raised = round_and_pack(huge, SINGLE, context(FpRounding::Nearest)).raised;
        assert!(raised.contains(FpExceptions::OVERFLOW));
        assert!(raised.contains(FpExceptions::INEXACT));
    }

    #[test]
    fn a_result_below_the_smallest_subnormal_underflows_to_zero() {
        let tiny = Unpacked {
            is_negative: false,
            exponent: -200,
            significand: implicit_bit(SINGLE),
        };

        let result = round_and_pack(tiny, SINGLE, context(FpRounding::Nearest));
        assert_eq!(result.bits, 0);
        assert!(result.raised.contains(FpExceptions::UNDERFLOW));
        assert!(result.raised.contains(FpExceptions::INEXACT));
    }

    #[test]
    fn flush_to_zero_replaces_a_subnormal_result_and_flags_it() {
        let control = FpControl {
            is_flush_to_zero: true,
            ..FpControl::DEFAULT
        };
        let subnormal = unpack(1, SINGLE);

        let result = round_and_pack(
            subnormal,
            SINGLE,
            RoundingContext {
                rounding: FpRounding::Nearest,
                is_sticky: false,
                control,
            },
        );

        assert_eq!(result.bits, 0);
        assert!(result.raised.contains(FpExceptions::UNDERFLOW));
        // Without FZ the same value is exact, so this pins that FZ is what
        // introduced the flags.
        assert_eq!(round_trip(1, SINGLE), 1);
    }

    #[test]
    fn flush_to_zero_keeps_the_sign_of_what_it_flushed() {
        let control = FpControl {
            is_flush_to_zero: true,
            ..FpControl::DEFAULT
        };
        let result = round_and_pack(
            unpack(0x8000_0001, SINGLE),
            SINGLE,
            RoundingContext {
                rounding: FpRounding::Nearest,
                is_sticky: false,
                control,
            },
        );

        assert_eq!(result.bits, 0x8000_0000, "negative zero, not positive");
    }

    #[test]
    fn fp16_flushing_is_keyed_on_its_own_control_bit() {
        let fz_only = FpControl {
            is_flush_to_zero: true,
            ..FpControl::DEFAULT
        };
        let context_for = |control| RoundingContext {
            rounding: FpRounding::Nearest,
            is_sticky: false,
            control,
        };

        // FZ alone must not flush a half-precision subnormal; FZ16 must.
        assert_eq!(
            round_and_pack(
                unpack(1, FpFormat::Half),
                FpFormat::Half,
                context_for(fz_only)
            )
            .bits,
            1
        );
        let fz16 = FpControl {
            is_flush_to_zero_fp16: true,
            ..FpControl::DEFAULT
        };
        assert_eq!(
            round_and_pack(unpack(1, FpFormat::Half), FpFormat::Half, context_for(fz16)).bits,
            0
        );
    }
}
