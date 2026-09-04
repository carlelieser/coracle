//! The reference floating-point implementation.
//!
//! Bit-exact and independent of the host's FP unit: every operation is integer
//! arithmetic over unpacked significands. This is the backend for non-default
//! FPCR and for the build-time precise mode that `docs/plan.md` §2 requires of
//! every differential FP leg.

mod arithmetic;
mod compare;
mod convert;
mod decompose;
mod fused;
mod nan;

use arithmetic::Context;
use compare::Extremum;
use fused::FusedOperands;

use super::backend::{
    FpBackend, FpBinaryOp, FpFmaOperands, FpOperands, FpUnaryOp, FpValue, FromIntegerSource,
    ToIntegerTarget,
};
use super::control::{FpControl, FpExceptions};
use super::operand::{FpComparison, FpFormat, FpResult, FpRounding};

/// The bit-exact backend.
///
/// Stateless: FPCR arrives with each call, so one instance serves the whole
/// machine.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Softfloat;

impl Softfloat {
    /// A new backend.
    pub const fn new() -> Self {
        Self
    }

    /// Bundles the per-operation context.
    const fn context(format: FpFormat, rounding: FpRounding, control: FpControl) -> Context {
        Context {
            format,
            rounding,
            control,
        }
    }
}

impl FpBackend for Softfloat {
    fn tracks_exceptions(&self) -> bool {
        true
    }

    fn binary(&self, op: FpBinaryOp, operands: FpOperands, control: FpControl) -> FpResult {
        let context = Self::context(operands.format, operands.rounding, control);
        let (lhs, rhs) = (operands.lhs, operands.rhs);

        match op {
            FpBinaryOp::Add => arithmetic::add(lhs, rhs, false, context),
            FpBinaryOp::Sub => arithmetic::add(lhs, rhs, true, context),
            FpBinaryOp::Mul => arithmetic::multiply(lhs, rhs, context),
            FpBinaryOp::Div => arithmetic::divide(lhs, rhs, context),
            FpBinaryOp::Max => compare::extremum(lhs, rhs, Extremum::Max, operands.format, control),
            FpBinaryOp::Min => compare::extremum(lhs, rhs, Extremum::Min, operands.format, control),
            FpBinaryOp::MaxNum => {
                compare::extremum(lhs, rhs, Extremum::MaxNum, operands.format, control)
            }
            FpBinaryOp::MinNum => {
                compare::extremum(lhs, rhs, Extremum::MinNum, operands.format, control)
            }
        }
    }

    fn unary(
        &self,
        op: FpUnaryOp,
        operand: FpValue,
        rounding: FpRounding,
        control: FpControl,
    ) -> FpResult {
        match op {
            FpUnaryOp::Sqrt => arithmetic::square_root(
                operand.bits,
                Self::context(operand.format, rounding, control),
            ),
            FpUnaryOp::RoundToIntegral => {
                convert::round_to_integral(operand.bits, operand.format, rounding, control)
            }
        }
    }

    fn fused_multiply_add(&self, operands: FpFmaOperands, control: FpControl) -> FpResult {
        fused::multiply_add(
            FusedOperands {
                multiplicand: operands.multiplicand,
                multiplier: operands.multiplier,
                addend: operands.addend,
                is_product_negated: operands.is_product_negated,
                is_addend_negated: operands.is_addend_negated,
            },
            Self::context(operands.format, operands.rounding, control),
        )
    }

    fn compare(&self, operands: FpOperands, is_signalling: bool) -> (FpComparison, FpExceptions) {
        compare::compare(operands.lhs, operands.rhs, operands.format, is_signalling)
    }

    fn convert_format(&self, value: FpValue, target: FpFormat, control: FpControl) -> FpResult {
        convert::convert_format(value.bits, value.format, target, control, control.rounding)
    }

    fn to_integer(&self, value: FpValue, target: ToIntegerTarget, _: FpControl) -> FpResult {
        convert::to_integer(value.bits, value.format, target.format, target.rounding)
    }

    fn convert_from_integer(
        &self,
        value: u64,
        source: FromIntegerSource,
        control: FpControl,
    ) -> FpResult {
        convert::from_integer(
            value,
            source.format,
            source.target,
            source.rounding,
            control,
        )
    }
}
