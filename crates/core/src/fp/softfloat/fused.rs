//! Fused multiply-add.
//!
//! Separate from [`super::arithmetic`] because the whole point is that the
//! product is not rounded before the addition: the exact 2N-bit product is
//! aligned against the addend and rounded once, which is what makes `FMADD`
//! differ from `FMUL` followed by `FADD`.

use super::arithmetic::Context;
use super::decompose::{round_and_pack, unpack, RoundingContext, GUARD_BITS};
use super::nan;
use crate::fp::operand::{FpFormat, FpResult, FpRounding};

/// The three operands and the sign controls that separate the four mnemonics.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FusedOperands {
    /// First multiplicand.
    pub multiplicand: u64,
    /// Second multiplicand.
    pub multiplier: u64,
    /// Addend.
    pub addend: u64,
    /// Whether the product is negated before the addition.
    pub is_product_negated: bool,
    /// Whether the addend is negated before the addition.
    pub is_addend_negated: bool,
}

/// The exact product, held at twice an unpacked significand's scale.
struct ExactProduct {
    is_negative: bool,
    exponent: i32,
    significand: u128,
}

/// `addend + multiplicand * multiplier`, with a single rounding.
pub fn multiply_add(operands: FusedOperands, context: Context) -> FpResult {
    let format = context.format;
    let inputs = [operands.multiplicand, operands.multiplier, operands.addend];
    if let Some(result) = nan::propagate(&inputs, format, context.control) {
        return result;
    }
    if let Some(result) = special_cases(&operands, context) {
        return result;
    }

    let product = exact_product(&operands, context);
    let addend = signed_addend(&operands, format);
    accumulate(product, addend, context)
}

/// The addend after `is_addend_negated` is applied.
fn signed_addend(operands: &FusedOperands, format: FpFormat) -> u64 {
    if operands.is_addend_negated {
        operands.addend ^ (1u64 << format.sign_shift())
    } else {
        operands.addend
    }
}

/// The infinity and zero cases, which never reach the significand arithmetic.
fn special_cases(operands: &FusedOperands, context: Context) -> Option<FpResult> {
    let format = context.format;
    let (multiplicand, multiplier) = (operands.multiplicand, operands.multiplier);
    let addend = signed_addend(operands, format);

    let is_product_infinite = format.is_infinite(multiplicand) || format.is_infinite(multiplier);
    let is_product_invalid = (format.is_infinite(multiplicand) && format.is_zero(multiplier))
        || (format.is_zero(multiplicand) && format.is_infinite(multiplier));
    if is_product_invalid {
        return Some(nan::invalid_operation(format));
    }

    let mut is_product_negative =
        format.is_negative(multiplicand) != format.is_negative(multiplier);
    if operands.is_product_negated {
        is_product_negative = !is_product_negative;
    }

    if is_product_infinite {
        // An infinite product plus the opposite infinity has no value.
        if format.is_infinite(addend) && format.is_negative(addend) != is_product_negative {
            return Some(nan::invalid_operation(format));
        }
        return Some(FpResult::exact(format.signed_infinity(is_product_negative)));
    }
    if format.is_infinite(addend) {
        return Some(FpResult::exact(addend));
    }

    if format.is_zero(multiplicand) || format.is_zero(multiplier) {
        return Some(zero_product(addend, is_product_negative, context));
    }
    None
}

/// The result when the product is zero, which reduces to returning the addend
/// except that two zeroes must agree on a sign.
fn zero_product(addend: u64, is_product_negative: bool, context: Context) -> FpResult {
    let format = context.format;
    if !format.is_zero(addend) {
        return FpResult::exact(addend);
    }

    let is_negative = if format.is_negative(addend) == is_product_negative {
        is_product_negative
    } else {
        matches!(context.rounding, FpRounding::Minus)
    };
    FpResult::exact(format.zero(is_negative))
}

/// The exact product of the two multiplicands, unrounded.
fn exact_product(operands: &FusedOperands, context: Context) -> ExactProduct {
    let format = context.format;
    let left = unpack(operands.multiplicand, format);
    let right = unpack(operands.multiplier, format);

    let mut is_negative = left.is_negative != right.is_negative;
    if operands.is_product_negated {
        is_negative = !is_negative;
    }

    ExactProduct {
        is_negative,
        exponent: left.exponent + right.exponent,
        significand: (left.significand as u128) * (right.significand as u128),
    }
}

/// Adds the addend to the exact product and rounds once.
fn accumulate(product: ExactProduct, addend: u64, context: Context) -> FpResult {
    let format = context.format;
    let scale = format.mantissa_bits() + GUARD_BITS;

    if format.is_zero(addend) {
        return round_wide(product, false, context);
    }

    let unpacked = unpack(addend, format);
    // Raise the addend to the product's scale so both are exact integers on
    // one axis; the product is already there by construction.
    let addend_significand = (unpacked.significand as u128) << scale;
    let addend_exponent = unpacked.exponent;

    let (aligned_product, aligned_addend, exponent, mut is_sticky) = align(
        product.significand,
        product.exponent,
        addend_significand,
        addend_exponent,
    );

    let (significand, is_negative) = if product.is_negative == unpacked.is_negative {
        (aligned_product + aligned_addend, product.is_negative)
    } else if aligned_product >= aligned_addend {
        (aligned_product - aligned_addend, product.is_negative)
    } else {
        (aligned_addend - aligned_product, unpacked.is_negative)
    };

    if significand == 0 && !is_sticky {
        let is_negative = matches!(context.rounding, FpRounding::Minus);
        return FpResult::exact(format.zero(is_negative));
    }
    // A cancellation that borrowed leaves the sticky bit meaningful only if
    // something survived above it.
    is_sticky &= significand != 0;

    round_wide(
        ExactProduct {
            is_negative,
            exponent,
            significand,
        },
        is_sticky,
        context,
    )
}

/// Brings two wide significands onto a common exponent, folding discarded bits
/// into a sticky flag.
fn align(
    product: u128,
    product_exponent: i32,
    addend: u128,
    addend_exponent: i32,
) -> (u128, u128, i32, bool) {
    if product_exponent >= addend_exponent {
        let shift = (product_exponent - addend_exponent) as u32;
        let (shifted, is_sticky) = shift_right_sticky(addend, shift);
        (product, shifted, product_exponent, is_sticky)
    } else {
        let shift = (addend_exponent - product_exponent) as u32;
        let (shifted, is_sticky) = shift_right_sticky(product, shift);
        (shifted, addend, addend_exponent, is_sticky)
    }
}

/// Shifts right, reporting whether a set bit was discarded.
fn shift_right_sticky(value: u128, shift: u32) -> (u128, bool) {
    if shift >= 128 {
        return (0, value != 0);
    }
    let discarded = value & ((1u128 << shift) - 1);
    (value >> shift, discarded != 0)
}

/// Narrows a wide significand back to 64 bits and rounds it.
///
/// The wide value carries its implicit bit at twice an unpacked significand's
/// scale. It is normalised *before* narrowing rather than after: a cancellation
/// can leave the leading bit far below that position, and shifting down by a
/// fixed amount first would discard the very bits the cancellation exposed.
fn round_wide(value: ExactProduct, is_sticky: bool, context: Context) -> FpResult {
    let format = context.format;
    if value.significand == 0 {
        return FpResult::exact(format.zero(value.is_negative));
    }

    // Place the leading bit at the wide scale's implicit position — twice an
    // unpacked significand's — so that narrowing by exactly one scale leaves
    // the value at the scale `round_and_pack` expects, with no exponent
    // correction beyond the normalisation itself.
    let scale = format.mantissa_bits() + GUARD_BITS;
    let target = 2 * scale;
    let leading = 127 - value.significand.leading_zeros();
    let mut exponent = value.exponent;
    let mut significand = value.significand;
    let mut is_sticky = is_sticky;

    if leading > target {
        let shift = leading - target;
        let (shifted, discarded) = shift_right_sticky(significand, shift);
        significand = shifted;
        is_sticky |= discarded;
        exponent += shift as i32;
    } else {
        let shift = target - leading;
        significand <<= shift;
        exponent -= shift as i32;
    }

    let (narrowed, discarded) = shift_right_sticky(significand, scale);
    round_and_pack(
        super::decompose::Unpacked {
            is_negative: value.is_negative,
            exponent,
            significand: narrowed as u64,
        },
        format,
        RoundingContext {
            rounding: context.rounding,
            is_sticky: is_sticky || discarded,
            control: context.control,
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fp::control::{FpControl, FpExceptions};

    const DOUBLE: FpFormat = FpFormat::Double;
    const SINGLE: FpFormat = FpFormat::Single;

    fn context(format: FpFormat) -> Context {
        Context {
            format,
            rounding: FpRounding::Nearest,
            control: FpControl::DEFAULT,
        }
    }

    fn plain(multiplicand: u64, multiplier: u64, addend: u64) -> FusedOperands {
        FusedOperands {
            multiplicand,
            multiplier,
            addend,
            is_product_negated: false,
            is_addend_negated: false,
        }
    }

    fn fma_double(multiplicand: f64, multiplier: f64, addend: f64) -> f64 {
        let operands = plain(
            multiplicand.to_bits(),
            multiplier.to_bits(),
            addend.to_bits(),
        );
        f64::from_bits(multiply_add(operands, context(DOUBLE)).bits)
    }

    #[test]
    fn fused_multiply_add_matches_the_hosts_own_fma() {
        // `f64::mul_add` is a genuine fused operation, so it is the right
        // oracle: it rounds once, exactly as this must.
        let cases = [
            (1.0, 2.0, 3.0),
            (0.1, 0.2, 0.3),
            (1.5, 2.5, -3.75),
            (1e300, 1e-300, 1.0),
            (123.456, 789.012, -0.5),
            (1.0 / 3.0, 3.0, -1.0),
            (f64::MIN_POSITIVE, 0.5, 1.0),
            (-2.5, 4.25, 10.0),
        ];

        for (multiplicand, multiplier, addend) in cases {
            assert_eq!(
                fma_double(multiplicand, multiplier, addend).to_bits(),
                multiplicand.mul_add(multiplier, addend).to_bits(),
                "fma({multiplicand}, {multiplier}, {addend})"
            );
        }
    }

    #[test]
    fn the_product_is_not_rounded_before_the_addition() {
        // This is the case that separates FMA from FMUL+FADD. The exact
        // product needs more bits than a double holds, and the addend cancels
        // the leading ones, exposing bits a rounded product would have lost.
        let multiplicand = 1.0 + f64::EPSILON;
        let multiplier = 1.0 - f64::EPSILON;
        let addend = -1.0;

        let fused = fma_double(multiplicand, multiplier, addend);
        let unfused = multiplicand * multiplier + addend;

        assert_eq!(
            fused.to_bits(),
            multiplicand.mul_add(multiplier, addend).to_bits()
        );
        assert_ne!(
            fused.to_bits(),
            unfused.to_bits(),
            "a separately-rounded product would not expose these bits"
        );
    }

    #[test]
    fn negating_the_product_and_the_addend_selects_the_four_mnemonics() {
        let (multiplicand, multiplier, addend) = (3.0f64, 5.0f64, 2.0f64);
        let build = |is_product_negated, is_addend_negated| {
            let operands = FusedOperands {
                multiplicand: multiplicand.to_bits(),
                multiplier: multiplier.to_bits(),
                addend: addend.to_bits(),
                is_product_negated,
                is_addend_negated,
            };
            f64::from_bits(multiply_add(operands, context(DOUBLE)).bits)
        };

        // FMADD, FMSUB, FNMSUB, FNMADD in the architecture's own terms.
        assert_eq!(build(false, false), 17.0, "addend + product");
        assert_eq!(build(true, false), -13.0, "addend - product");
        assert_eq!(build(false, true), 13.0, "product - addend");
        assert_eq!(build(true, true), -17.0, "-(addend + product)");
    }

    #[test]
    fn an_infinite_product_plus_the_opposite_infinity_is_invalid() {
        let operands = plain(
            DOUBLE.infinity(),
            1.0f64.to_bits(),
            DOUBLE.signed_infinity(true),
        );
        let result = multiply_add(operands, context(DOUBLE));

        assert_eq!(result.bits, DOUBLE.default_nan());
        assert!(result.raised.contains(FpExceptions::INVALID));
    }

    #[test]
    fn a_zero_times_infinity_product_is_invalid_whatever_the_addend() {
        let operands = plain(0, DOUBLE.infinity(), 1.0f64.to_bits());
        let result = multiply_add(operands, context(DOUBLE));

        assert_eq!(result.bits, DOUBLE.default_nan());
        assert!(result.raised.contains(FpExceptions::INVALID));
    }

    #[test]
    fn a_signalling_nan_in_any_of_the_three_operands_propagates() {
        let signalling = 0x7ff0_0000_0000_0001;
        let one = 1.0f64.to_bits();

        for operands in [
            plain(signalling, one, one),
            plain(one, signalling, one),
            plain(one, one, signalling),
        ] {
            let result = multiply_add(operands, context(DOUBLE));
            assert_eq!(result.bits, 0x7ff8_0000_0000_0001);
            assert!(result.raised.contains(FpExceptions::INVALID));
        }
    }

    #[test]
    fn single_precision_fma_matches_the_host() {
        for (multiplicand, multiplier, addend) in [
            (1.0f32, 2.0f32, 3.0f32),
            (0.1, 0.2, 0.3),
            (1.5, -2.5, 0.125),
        ] {
            let operands = plain(
                multiplicand.to_bits() as u64,
                multiplier.to_bits() as u64,
                addend.to_bits() as u64,
            );
            let result = multiply_add(operands, context(SINGLE));
            assert_eq!(
                (result.bits as u32),
                multiplicand.mul_add(multiplier, addend).to_bits(),
                "fma({multiplicand}, {multiplier}, {addend})"
            );
        }
    }

    #[test]
    fn fused_multiply_add_agrees_with_the_host_across_a_pseudorandom_sweep() {
        let mut state = 0x9e37_79b9_7f4a_7c15u64;
        let mut next = || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            f64::from_bits((state & 0x800f_ffff_ffff_ffff) | 0x3ff0_0000_0000_0000)
        };

        for _ in 0..2000 {
            let (multiplicand, multiplier, addend) = (next(), next(), next());
            assert_eq!(
                fma_double(multiplicand, multiplier, addend).to_bits(),
                multiplicand.mul_add(multiplier, addend).to_bits(),
                "fma({multiplicand}, {multiplier}, {addend})"
            );
        }
    }
}
