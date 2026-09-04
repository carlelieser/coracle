//! Comparison, and the minimum and maximum families.
//!
//! Comparison is a total function over bit patterns rather than a subtraction:
//! `+0.0` and `-0.0` compare equal despite differing bits, and any NaN operand
//! makes the result unordered rather than any of the three orderings.

use super::nan;
use crate::fp::control::{FpControl, FpExceptions};
use crate::fp::operand::{FpComparison, FpFormat, FpResult};

/// Compares two values.
///
/// `is_signalling` selects `FCMPE` over `FCMP`: both raise invalid for a
/// signalling NaN, but only `FCMPE` raises it for a quiet one.
pub fn compare(
    lhs: u64,
    rhs: u64,
    format: FpFormat,
    is_signalling: bool,
) -> (FpComparison, FpExceptions) {
    let operands = [lhs, rhs];
    if nan::any_nan(&operands, format) {
        let is_invalid = is_signalling || nan::signalling_among(&operands, format);
        let raised = if is_invalid {
            FpExceptions::INVALID
        } else {
            FpExceptions::NONE
        };
        return (FpComparison::Unordered, raised);
    }

    (ordering(lhs, rhs, format), FpExceptions::NONE)
}

/// The ordering of two non-NaN values.
fn ordering(lhs: u64, rhs: u64, format: FpFormat) -> FpComparison {
    // The two zeroes are equal despite their differing sign bits, so this case
    // cannot be folded into the magnitude comparison below.
    if format.is_zero(lhs) && format.is_zero(rhs) {
        return FpComparison::Equal;
    }
    if lhs == rhs {
        return FpComparison::Equal;
    }

    let (is_lhs_negative, is_rhs_negative) = (format.is_negative(lhs), format.is_negative(rhs));
    if is_lhs_negative != is_rhs_negative {
        return if is_lhs_negative {
            FpComparison::Less
        } else {
            FpComparison::Greater
        };
    }

    // Within one sign, IEEE's ordering matches the magnitude ordering of the
    // bit patterns, reversed for negatives.
    let magnitude = |bits: u64| bits & !(1u64 << format.sign_shift());
    let is_less = if is_lhs_negative {
        magnitude(lhs) > magnitude(rhs)
    } else {
        magnitude(lhs) < magnitude(rhs)
    };
    if is_less {
        FpComparison::Less
    } else {
        FpComparison::Greater
    }
}

/// Which extremum to take, and how it treats NaNs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Extremum {
    /// `FMAX` — a NaN operand propagates.
    Max,
    /// `FMIN` — a NaN operand propagates.
    Min,
    /// `FMAXNM` — a single quiet NaN operand is ignored.
    MaxNum,
    /// `FMINNM` — a single quiet NaN operand is ignored.
    MinNum,
}

impl Extremum {
    /// Whether this picks the larger operand.
    const fn is_maximum(self) -> bool {
        matches!(self, Extremum::Max | Extremum::MaxNum)
    }

    /// Whether a lone quiet NaN is ignored rather than propagated.
    const fn ignores_quiet_nan(self) -> bool {
        matches!(self, Extremum::MaxNum | Extremum::MinNum)
    }
}

/// `FMAX`, `FMIN`, `FMAXNM` and `FMINNM`.
pub fn extremum(
    lhs: u64,
    rhs: u64,
    which: Extremum,
    format: FpFormat,
    control: FpControl,
) -> FpResult {
    if let Some(result) = numeric_nan_case(lhs, rhs, which, format) {
        return result;
    }
    if let Some(result) = nan::propagate(&[lhs, rhs], format, control) {
        return result;
    }

    // Zeroes of opposite sign are equal, so the comparison cannot pick between
    // them; the architecture takes the one the operation prefers.
    if format.is_zero(lhs) && format.is_zero(rhs) {
        let is_negative = if which.is_maximum() {
            format.is_negative(lhs) && format.is_negative(rhs)
        } else {
            format.is_negative(lhs) || format.is_negative(rhs)
        };
        return FpResult::exact(format.zero(is_negative));
    }

    let is_lhs_greater = matches!(ordering(lhs, rhs, format), FpComparison::Greater);
    let takes_lhs = is_lhs_greater == which.is_maximum();
    FpResult::exact(if takes_lhs { lhs } else { rhs })
}

/// The `FMAXNM`/`FMINNM` rule: exactly one quiet NaN operand returns the other.
///
/// A signalling NaN still propagates and still raises, and two NaNs fall
/// through to ordinary propagation.
fn numeric_nan_case(lhs: u64, rhs: u64, which: Extremum, format: FpFormat) -> Option<FpResult> {
    if !which.ignores_quiet_nan() {
        return None;
    }
    if nan::signalling_among(&[lhs, rhs], format) {
        return None;
    }

    let (is_lhs_nan, is_rhs_nan) = (format.is_nan(lhs), format.is_nan(rhs));
    let surviving = match (is_lhs_nan, is_rhs_nan) {
        (true, false) => rhs,
        (false, true) => lhs,
        _ => return None,
    };
    Some(FpResult::exact(surviving))
}

#[cfg(test)]
mod tests {
    use super::*;

    const DOUBLE: FpFormat = FpFormat::Double;
    const QUIET: u64 = 0x7ff8_0000_0000_0001;
    const SIGNALLING: u64 = 0x7ff0_0000_0000_0001;

    fn compare_double(lhs: f64, rhs: f64) -> FpComparison {
        compare(lhs.to_bits(), rhs.to_bits(), DOUBLE, false).0
    }

    #[test]
    fn ordering_matches_the_hosts_own_partial_order() {
        let values = [
            f64::NEG_INFINITY,
            -1e300,
            -1.0,
            -f64::MIN_POSITIVE,
            -0.0,
            0.0,
            f64::MIN_POSITIVE,
            1.0,
            1e300,
            f64::INFINITY,
        ];

        for &lhs in &values {
            for &rhs in &values {
                let expected = if lhs < rhs {
                    FpComparison::Less
                } else if lhs > rhs {
                    FpComparison::Greater
                } else {
                    FpComparison::Equal
                };
                assert_eq!(compare_double(lhs, rhs), expected, "{lhs} vs {rhs}");
            }
        }
    }

    #[test]
    fn the_two_zeroes_compare_equal_despite_differing_bits() {
        assert_eq!(compare_double(0.0, -0.0), FpComparison::Equal);
        assert_eq!(compare_double(-0.0, 0.0), FpComparison::Equal);
        // And the bits really are different, so this is not a trivial pass.
        assert_ne!(0.0f64.to_bits(), (-0.0f64).to_bits());
    }

    #[test]
    fn any_nan_operand_makes_the_result_unordered() {
        for (lhs, rhs) in [
            (QUIET, 1.0f64.to_bits()),
            (1.0f64.to_bits(), QUIET),
            (QUIET, QUIET),
        ] {
            assert_eq!(compare(lhs, rhs, DOUBLE, false).0, FpComparison::Unordered);
        }
    }

    #[test]
    fn a_quiet_nan_raises_invalid_only_for_the_signalling_compare() {
        let one = 1.0f64.to_bits();

        let quiet_fcmp = compare(QUIET, one, DOUBLE, false).1;
        assert!(!quiet_fcmp.contains(FpExceptions::INVALID), "FCMP is quiet");

        let quiet_fcmpe = compare(QUIET, one, DOUBLE, true).1;
        assert!(quiet_fcmpe.contains(FpExceptions::INVALID), "FCMPE raises");

        // A signalling NaN raises for both.
        assert!(compare(SIGNALLING, one, DOUBLE, false)
            .1
            .contains(FpExceptions::INVALID));
        assert!(compare(SIGNALLING, one, DOUBLE, true)
            .1
            .contains(FpExceptions::INVALID));
    }

    #[test]
    fn unordered_maps_to_the_nzcv_the_architecture_names() {
        assert_eq!(FpComparison::Unordered.to_nzcv(), 0b0011);
        assert_eq!(compare_double(f64::NAN, 1.0).to_nzcv(), 0b0011);
    }

    fn extremum_double(lhs: f64, rhs: f64, which: Extremum) -> f64 {
        f64::from_bits(
            extremum(
                lhs.to_bits(),
                rhs.to_bits(),
                which,
                DOUBLE,
                FpControl::DEFAULT,
            )
            .bits,
        )
    }

    #[test]
    fn maximum_and_minimum_pick_the_expected_operand() {
        let cases = [(1.0, 2.0), (-1.0, 1.0), (-5.0, -2.0), (1e300, 1e-300)];

        for (lhs, rhs) in cases {
            assert_eq!(extremum_double(lhs, rhs, Extremum::Max), lhs.max(rhs));
            assert_eq!(extremum_double(lhs, rhs, Extremum::Min), lhs.min(rhs));
            // Order of operands must not matter for finite values.
            assert_eq!(extremum_double(rhs, lhs, Extremum::Max), lhs.max(rhs));
            assert_eq!(extremum_double(rhs, lhs, Extremum::Min), lhs.min(rhs));
        }
    }

    #[test]
    fn the_extrema_distinguish_the_two_zeroes() {
        // max(+0, -0) is +0 and min(+0, -0) is -0, in either operand order.
        assert_eq!(
            extremum_double(0.0, -0.0, Extremum::Max).to_bits(),
            0.0f64.to_bits()
        );
        assert_eq!(
            extremum_double(-0.0, 0.0, Extremum::Max).to_bits(),
            0.0f64.to_bits()
        );
        assert_eq!(
            extremum_double(0.0, -0.0, Extremum::Min).to_bits(),
            (-0.0f64).to_bits()
        );
        assert_eq!(
            extremum_double(-0.0, 0.0, Extremum::Min).to_bits(),
            (-0.0f64).to_bits()
        );
    }

    #[test]
    fn fmax_propagates_a_nan_but_fmaxnm_ignores_a_lone_quiet_one() {
        let one = 1.0f64.to_bits();
        let control = FpControl::DEFAULT;

        // FMAX propagates.
        assert_eq!(
            extremum(QUIET, one, Extremum::Max, DOUBLE, control).bits,
            QUIET
        );
        assert_eq!(
            extremum(QUIET, one, Extremum::Min, DOUBLE, control).bits,
            QUIET
        );

        // FMAXNM returns the number, whichever side the NaN is on.
        assert_eq!(
            extremum(QUIET, one, Extremum::MaxNum, DOUBLE, control).bits,
            one
        );
        assert_eq!(
            extremum(one, QUIET, Extremum::MaxNum, DOUBLE, control).bits,
            one
        );
        assert_eq!(
            extremum(QUIET, one, Extremum::MinNum, DOUBLE, control).bits,
            one
        );
    }

    #[test]
    fn fmaxnm_still_propagates_a_signalling_nan_and_raises() {
        let one = 1.0f64.to_bits();
        let result = extremum(
            SIGNALLING,
            one,
            Extremum::MaxNum,
            DOUBLE,
            FpControl::DEFAULT,
        );

        assert_eq!(result.bits, 0x7ff8_0000_0000_0001, "quietened, not ignored");
        assert!(result.raised.contains(FpExceptions::INVALID));
    }

    #[test]
    fn fmaxnm_with_two_quiet_nans_propagates_the_first() {
        let second = 0x7ff8_0000_0000_0002;
        let result = extremum(QUIET, second, Extremum::MaxNum, DOUBLE, FpControl::DEFAULT);

        assert_eq!(result.bits, QUIET);
    }
}
