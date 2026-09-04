//! The reference floating-point implementation.
//!
//! Bit-exact and independent of the host's FP unit: every operation is integer
//! arithmetic over unpacked significands. This is the backend for non-default
//! FPCR and for the build-time precise mode that `docs/plan.md` §2 requires of
//! every differential FP leg.

mod arithmetic;
mod decompose;
mod fused;
mod nan;
