//! Backend selection, and the FPSR bookkeeping that goes with it.
//!
//! This is the type the interpreter holds. It resolves "native or softfloat"
//! once, when FPCR changes, rather than at every call site: an instruction
//! implementation calls [`FpEngine::binary`] and never learns which backend
//! answered.

use super::backend::{
    FpBackend, FpBinaryOp, FpFmaOperands, FpOperands, FpSignOp, FpUnaryOp, FpValue,
    FromIntegerSource, ToIntegerTarget,
};
use super::control::{accumulate, is_default_mode_bits, FpControl};
use super::native::Native;
use super::operand::{FpComparison, FpFormat, FpResult, FpRounding};
use super::softfloat::Softfloat;

/// Whether this build routes everything through the reference implementation.
///
/// `docs/plan.md` §2 calls this "precise mode" and requires every differential
/// FP leg to run in it. It is a compile-time feature rather than a runtime flag
/// so the native path costs nothing when it is off.
pub const IS_PRECISE_BUILD: bool = cfg!(feature = "fp-precise");

/// Which backend is serving the current FPCR.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Selected {
    /// Host float operations.
    Native(Native),
    /// The bit-exact reference.
    Reference(Softfloat),
}

/// The floating-point unit the interpreter drives.
///
/// Holds the selected backend, the interpreted FPCR, and the FPSR bits raised
/// so far. Callers reach the backend only through this type's methods, which is
/// what keeps the selection from leaking: adding a backend changes this file
/// and nothing else.
#[derive(Debug, Clone, Copy)]
pub struct FpEngine {
    selected: Selected,
    control: FpControl,
    /// Cumulative exception flags, in their FPSR bit positions.
    ///
    /// Only ever written by a backend that reports flags, so in a default-mode
    /// build this stays zero — the divergence `docs/machine-spec.md` §6 names.
    exceptions: u64,
}

impl Default for FpEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl FpEngine {
    /// An engine in the reset state: default FPCR, no flags raised.
    pub fn new() -> Self {
        Self::with_fpcr(0)
    }

    /// An engine serving `fpcr`.
    pub fn with_fpcr(fpcr: u64) -> Self {
        Self {
            selected: select(fpcr),
            control: FpControl::from_bits(fpcr),
            exceptions: 0,
        }
    }

    /// Re-selects the backend for a new FPCR value.
    ///
    /// The interpreter calls this from its `MSR FPCR` implementation and
    /// nowhere else; every other path reads the cached selection.
    pub fn set_fpcr(&mut self, fpcr: u64) {
        self.selected = select(fpcr);
        self.control = FpControl::from_bits(fpcr);
    }

    /// The interpreted FPCR.
    pub const fn control(&self) -> FpControl {
        self.control
    }

    /// The cumulative exception flags, for `MRS ..., FPSR`.
    pub const fn exceptions(&self) -> u64 {
        self.exceptions
    }

    /// Replaces the cumulative flags, for `MSR FPSR, ...`.
    pub const fn set_exceptions(&mut self, fpsr: u64) {
        self.exceptions = fpsr;
    }

    /// Whether the selected backend maintains FPSR.
    pub fn tracks_exceptions(&self) -> bool {
        self.backend().tracks_exceptions()
    }

    /// The rounding mode an instruction gets when its encoding names none.
    pub const fn default_rounding(&self) -> FpRounding {
        self.control.rounding
    }

    /// The selected backend, as a trait object-free borrow.
    fn backend(&self) -> &dyn FpBackend {
        match &self.selected {
            Selected::Native(backend) => backend,
            Selected::Reference(backend) => backend,
        }
    }

    /// Records a result's flags and hands back its bits.
    fn record(&mut self, result: FpResult) -> u64 {
        self.exceptions = accumulate(self.exceptions, result.raised);
        result.bits
    }

    /// Two-operand arithmetic.
    pub fn binary(&mut self, op: FpBinaryOp, operands: FpOperands) -> u64 {
        let result = self.backend().binary(op, operands, self.control);
        self.record(result)
    }

    /// One-operand arithmetic.
    pub fn unary(&mut self, op: FpUnaryOp, operand: FpValue, rounding: FpRounding) -> u64 {
        let result = self.backend().unary(op, operand, rounding, self.control);
        self.record(result)
    }

    /// Fused multiply-add.
    pub fn fused_multiply_add(&mut self, operands: FpFmaOperands) -> u64 {
        let result = self.backend().fused_multiply_add(operands, self.control);
        self.record(result)
    }

    /// Negation and absolute value, which raise nothing.
    pub fn copy_sign(&self, value: FpValue, op: FpSignOp) -> u64 {
        self.backend().copy_sign(value, op)
    }

    /// Ordered comparison, returning the NZCV the architecture writes.
    pub fn compare(&mut self, operands: FpOperands, is_signalling: bool) -> u8 {
        let (ordering, raised) = self.backend().compare(operands, is_signalling);
        self.exceptions = accumulate(self.exceptions, raised);
        ordering.to_nzcv()
    }

    /// Comparison as an ordering rather than as flags, for the vector forms.
    pub fn compare_ordering(&mut self, operands: FpOperands) -> FpComparison {
        let (ordering, raised) = self.backend().compare(operands, false);
        self.exceptions = accumulate(self.exceptions, raised);
        ordering
    }

    /// Conversion between FP formats.
    pub fn convert_format(&mut self, value: FpValue, target: FpFormat) -> u64 {
        let result = self.backend().convert_format(value, target, self.control);
        self.record(result)
    }

    /// FP to integer.
    pub fn to_integer(&mut self, value: FpValue, target: ToIntegerTarget) -> u64 {
        let result = self.backend().to_integer(value, target, self.control);
        self.record(result)
    }

    /// Integer to FP.
    pub fn from_integer(&mut self, value: u64, source: FromIntegerSource) -> u64 {
        let result = self.backend().from_integer(value, source, self.control);
        self.record(result)
    }
}

/// Picks the backend for an FPCR value.
///
/// Precise mode overrides the choice entirely, which is what lets a
/// differential run compare against QEMU without the native path's NaN-payload
/// and FPSR divergences in the way.
fn select(fpcr: u64) -> Selected {
    if IS_PRECISE_BUILD || !is_default_mode_bits(fpcr) {
        return Selected::Reference(Softfloat::new());
    }
    Selected::Native(Native::new())
}

/// Whether an FPCR value selects the native backend in this build.
pub fn selects_native_backend(fpcr: u64) -> bool {
    matches!(select(fpcr), Selected::Native(_))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fp::control::FpExceptions;

    const DOUBLE: FpFormat = FpFormat::Double;

    /// FPCR with RMode set to round-toward-zero.
    const TOWARD_ZERO: u64 = 0b11 << 22;
    /// FPCR with flush-to-zero set.
    const FLUSH_TO_ZERO: u64 = 1 << 24;

    fn operands(lhs: f64, rhs: f64) -> FpOperands {
        FpOperands::new(lhs.to_bits(), rhs.to_bits(), DOUBLE, FpRounding::Nearest)
    }

    #[test]
    fn a_default_fpcr_selects_the_native_backend_outside_precise_builds() {
        let engine = FpEngine::new();

        assert_eq!(engine.tracks_exceptions(), IS_PRECISE_BUILD);
        assert_eq!(selects_native_backend(0), !IS_PRECISE_BUILD);
    }

    #[test]
    fn a_non_default_fpcr_always_selects_the_reference() {
        for fpcr in [TOWARD_ZERO, FLUSH_TO_ZERO, 1 << 25, 1 << 19] {
            assert!(!selects_native_backend(fpcr), "fpcr {fpcr:#x}");
            assert!(FpEngine::with_fpcr(fpcr).tracks_exceptions());
        }
    }

    #[test]
    fn writing_fpcr_reselects_the_backend() {
        let mut engine = FpEngine::new();
        assert_eq!(engine.tracks_exceptions(), IS_PRECISE_BUILD);

        engine.set_fpcr(TOWARD_ZERO);
        assert!(engine.tracks_exceptions(), "now on the reference path");
        assert_eq!(engine.default_rounding(), FpRounding::Zero);

        engine.set_fpcr(0);
        assert_eq!(engine.tracks_exceptions(), IS_PRECISE_BUILD);
        assert_eq!(engine.default_rounding(), FpRounding::Nearest);
    }

    #[test]
    fn the_engine_computes_the_same_answer_whichever_backend_is_selected() {
        // The selection must be invisible in the result for ordinary values.
        // This is the property that lets the interpreter ignore it.
        let mut native = FpEngine::with_fpcr(0);
        let mut reference = FpEngine::with_fpcr(TOWARD_ZERO);
        reference.set_fpcr(0);
        // Force the reference by asking for a mode the host cannot serve, then
        // compare against a directly-computed reference result.
        let pair = operands(0.1, 0.2);

        let from_engine = native.binary(FpBinaryOp::Add, pair);
        let from_reference = Softfloat::new()
            .binary(FpBinaryOp::Add, pair, FpControl::DEFAULT)
            .bits;

        assert_eq!(from_engine, from_reference);
    }

    #[test]
    fn exception_flags_accumulate_only_on_the_reference_path() {
        // 1/0 raises DZC. Under a non-default FPCR the reference is selected
        // and the flag lands; in default mode outside a precise build it does
        // not. That asymmetry is the documented divergence.
        let mut reference = FpEngine::with_fpcr(TOWARD_ZERO);
        let pair = FpOperands::new(1.0f64.to_bits(), 0.0f64.to_bits(), DOUBLE, FpRounding::Zero);

        reference.binary(FpBinaryOp::Div, pair);
        assert!(
            reference.exceptions() & FpExceptions::DIVIDE_BY_ZERO.to_bits() != 0,
            "the reference path records DZC"
        );

        let mut native = FpEngine::new();
        native.binary(
            FpBinaryOp::Div,
            FpOperands::new(
                1.0f64.to_bits(),
                0.0f64.to_bits(),
                DOUBLE,
                FpRounding::Nearest,
            ),
        );
        assert_eq!(
            native.exceptions() != 0,
            IS_PRECISE_BUILD,
            "flags appear only when the reference is serving"
        );
    }

    #[test]
    fn flags_are_cumulative_across_operations_and_cleared_only_by_a_write() {
        let mut engine = FpEngine::with_fpcr(TOWARD_ZERO);
        let rounding = FpRounding::Zero;

        engine.binary(
            FpBinaryOp::Div,
            FpOperands::new(1.0f64.to_bits(), 0.0f64.to_bits(), DOUBLE, rounding),
        );
        let after_first = engine.exceptions();
        assert_ne!(after_first, 0);

        // An exact operation raises nothing but must not clear what is set.
        engine.binary(
            FpBinaryOp::Add,
            FpOperands::new(1.0f64.to_bits(), 1.0f64.to_bits(), DOUBLE, rounding),
        );
        assert!(engine.exceptions() & after_first == after_first);

        engine.set_exceptions(0);
        assert_eq!(engine.exceptions(), 0, "only a write clears them");
    }

    #[test]
    fn the_sign_operations_never_touch_the_flags() {
        let mut engine = FpEngine::with_fpcr(TOWARD_ZERO);
        let signalling = FpValue::new(0x7ff0_0000_0000_0001, DOUBLE);

        let negated = engine.copy_sign(signalling, FpSignOp::Negate);

        assert_eq!(negated, 0xfff0_0000_0000_0001);
        assert_eq!(engine.exceptions(), 0, "FNEG raises nothing");
    }

    #[test]
    fn a_comparison_returns_the_architectures_nzcv() {
        let mut engine = FpEngine::new();

        assert_eq!(engine.compare(operands(1.0, 2.0), false), 0b1000, "less");
        assert_eq!(engine.compare(operands(2.0, 1.0), false), 0b0010, "greater");
        assert_eq!(engine.compare(operands(1.0, 1.0), false), 0b0110, "equal");
        assert_eq!(
            engine.compare(operands(f64::NAN, 1.0), false),
            0b0011,
            "unordered"
        );
    }
}
