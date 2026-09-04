//! NaN propagation, as `FPProcessNaNs` defines it.
//!
//! Isolated from the arithmetic because every operation shares it and because
//! it is the part a differential run against QEMU exercises hardest. The rules,
//! in order: a signalling NaN operand raises invalid and is quietened; the
//! first NaN operand in operand order wins; `FPCR.DN` replaces any NaN result
//! with the default NaN.

use crate::fp::control::{FpControl, FpExceptions};
use crate::fp::operand::{FpFormat, FpResult};

/// The NaN result an operation with these operands must produce, if any.
///
/// `None` means no operand was a NaN and the caller proceeds with arithmetic.
/// Operands are inspected in the order given, which is the architectural
/// operand order.
pub fn propagate(operands: &[u64], format: FpFormat, control: FpControl) -> Option<FpResult> {
    let has_signalling = operands.iter().any(|&bits| format.is_signalling_nan(bits));
    let first_nan = operands.iter().copied().find(|&bits| format.is_nan(bits))?;

    let raised = if has_signalling {
        FpExceptions::INVALID
    } else {
        FpExceptions::NONE
    };
    Some(FpResult::raising(
        quiet_result(first_nan, format, control),
        raised,
    ))
}

/// The NaN an invalid operation produces when no operand is a NaN — `0/0`,
/// `inf - inf`, and their kind.
pub fn invalid_operation(format: FpFormat) -> FpResult {
    FpResult::raising(format.default_nan(), FpExceptions::INVALID)
}

/// Quietens a NaN, honouring `FPCR.DN`.
fn quiet_result(bits: u64, format: FpFormat, control: FpControl) -> u64 {
    if control.is_default_nan {
        format.default_nan()
    } else {
        format.quieten(bits)
    }
}

/// Whether a signalling NaN among the operands should raise invalid, for the
/// operations that inspect NaNs without producing one — the comparisons.
pub fn signalling_among(operands: &[u64], format: FpFormat) -> bool {
    operands.iter().any(|&bits| format.is_signalling_nan(bits))
}

/// Whether any operand is a NaN at all.
pub fn any_nan(operands: &[u64], format: FpFormat) -> bool {
    operands.iter().any(|&bits| format.is_nan(bits))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SINGLE: FpFormat = FpFormat::Single;
    const QUIET_A: u64 = 0x7fc0_0001;
    const QUIET_B: u64 = 0x7fc0_0002;
    const SIGNALLING: u64 = 0x7f80_0003;
    const ONE: u64 = 0x3f80_0000;

    #[test]
    fn no_nan_operand_yields_no_nan_result() {
        assert!(propagate(&[ONE, ONE], SINGLE, FpControl::DEFAULT).is_none());
    }

    #[test]
    fn the_first_nan_in_operand_order_wins() {
        // Distinct payloads, so a implementation returning "some NaN" rather
        // than "the first one" fails here.
        let result = propagate(&[QUIET_A, QUIET_B], SINGLE, FpControl::DEFAULT).unwrap();
        assert_eq!(result.bits, QUIET_A);

        let reversed = propagate(&[QUIET_B, QUIET_A], SINGLE, FpControl::DEFAULT).unwrap();
        assert_eq!(reversed.bits, QUIET_B);
    }

    #[test]
    fn a_non_nan_first_operand_does_not_shadow_a_later_nan() {
        let result = propagate(&[ONE, QUIET_B], SINGLE, FpControl::DEFAULT).unwrap();
        assert_eq!(result.bits, QUIET_B);
    }

    #[test]
    fn a_signalling_nan_is_quietened_with_its_payload_intact_and_raises_invalid() {
        let result = propagate(&[SIGNALLING, ONE], SINGLE, FpControl::DEFAULT).unwrap();

        assert_eq!(result.bits, 0x7fc0_0003, "payload survives quietening");
        assert!(result.raised.contains(FpExceptions::INVALID));
    }

    #[test]
    fn a_signalling_nan_raises_invalid_even_when_a_quiet_nan_is_returned() {
        // Operand order returns the quiet NaN, but the signalling one still
        // raised. Collapsing these two questions into one is the classic bug.
        let result = propagate(&[QUIET_A, SIGNALLING], SINGLE, FpControl::DEFAULT).unwrap();

        assert_eq!(result.bits, QUIET_A);
        assert!(result.raised.contains(FpExceptions::INVALID));
    }

    #[test]
    fn a_quiet_nan_alone_raises_nothing() {
        let result = propagate(&[QUIET_A, ONE], SINGLE, FpControl::DEFAULT).unwrap();
        assert!(result.raised.is_empty());
    }

    #[test]
    fn default_nan_mode_replaces_the_payload_but_keeps_the_exception() {
        let control = FpControl {
            is_default_nan: true,
            ..FpControl::DEFAULT
        };

        let quiet = propagate(&[QUIET_A, ONE], SINGLE, control).unwrap();
        assert_eq!(quiet.bits, SINGLE.default_nan());
        assert!(quiet.raised.is_empty());

        let signalling = propagate(&[SIGNALLING, ONE], SINGLE, control).unwrap();
        assert_eq!(signalling.bits, SINGLE.default_nan());
        assert!(signalling.raised.contains(FpExceptions::INVALID));
    }

    #[test]
    fn an_invalid_operation_without_nan_operands_produces_the_default_nan() {
        let result = invalid_operation(SINGLE);
        assert_eq!(result.bits, 0x7fc0_0000);
        assert!(result.raised.contains(FpExceptions::INVALID));
    }

    #[test]
    fn nan_predicates_scan_every_operand() {
        assert!(signalling_among(&[ONE, ONE, SIGNALLING], SINGLE));
        assert!(!signalling_among(&[ONE, QUIET_A], SINGLE));
        assert!(any_nan(&[ONE, QUIET_A], SINGLE));
        assert!(!any_nan(&[ONE, ONE], SINGLE));
    }
}
