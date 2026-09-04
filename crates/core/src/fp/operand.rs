//! The value vocabulary the FP backends share.
//!
//! Values cross the backend boundary as raw bit patterns rather than `f32` /
//! `f64`. A Rust float is not a faithful carrier for an architectural FP
//! register: moving a signalling NaN through one is permitted to quieten it,
//! and the softfloat path must see the exact input bits the guest wrote.

/// Rounding mode in force for an operation.
///
/// Distinct from [`crate::decode::operand::RoundMode`], which is what an
/// *encoding* names — including [`crate::decode::operand::RoundMode::Current`],
/// meaning "ask FPCR". By the time a backend is called the question is already
/// resolved, so this enum has no `Current`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FpRounding {
    /// To nearest, ties to even. `FPCR.RMode = 00`, and the reset state.
    #[default]
    Nearest,
    /// Toward positive infinity. `FPCR.RMode = 01`.
    Plus,
    /// Toward negative infinity. `FPCR.RMode = 10`.
    Minus,
    /// Toward zero. `FPCR.RMode = 11`.
    Zero,
    /// To nearest, ties away from zero. Named only by `FCVTAS`/`FCVTAU` and
    /// `FRINTA`; `FPCR.RMode` cannot select it.
    NearestAway,
    /// To nearest, ties to odd. `FCVTXN` only; `FPCR.RMode` cannot select it.
    Odd,
}

impl FpRounding {
    /// Decodes the 2-bit `FPCR.RMode` field.
    pub const fn from_rmode(rmode: u8) -> Self {
        match rmode & 0b11 {
            0b00 => FpRounding::Nearest,
            0b01 => FpRounding::Plus,
            0b10 => FpRounding::Minus,
            _ => FpRounding::Zero,
        }
    }
}

/// Which IEEE format a value is in.
///
/// The backends are generic over this rather than having one method per width:
/// `FADD S` and `FADD D` differ only in the format, and half precision joins
/// them for the conversion forms.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum FpFormat {
    /// IEEE binary16.
    Half,
    /// IEEE binary32.
    Single,
    /// IEEE binary64.
    Double,
}

impl FpFormat {
    /// Total width in bits.
    pub const fn bits(self) -> u32 {
        match self {
            FpFormat::Half => 16,
            FpFormat::Single => 32,
            FpFormat::Double => 64,
        }
    }

    /// Bits in the trailing significand field.
    pub const fn mantissa_bits(self) -> u32 {
        match self {
            FpFormat::Half => 10,
            FpFormat::Single => 23,
            FpFormat::Double => 52,
        }
    }

    /// Bits in the biased exponent field.
    pub const fn exponent_bits(self) -> u32 {
        self.bits() - self.mantissa_bits() - 1
    }

    /// The exponent field value shared by infinities and NaNs.
    pub const fn max_exponent(self) -> u32 {
        (1 << self.exponent_bits()) - 1
    }

    /// Amount added to an unbiased exponent to store it.
    pub const fn exponent_bias(self) -> i32 {
        (1 << (self.exponent_bits() - 1)) - 1
    }

    /// Mask covering every bit of a value in this format.
    pub const fn mask(self) -> u64 {
        match self {
            FpFormat::Half => 0xffff,
            FpFormat::Single => 0xffff_ffff,
            FpFormat::Double => u64::MAX,
        }
    }

    /// The sign bit's position.
    pub const fn sign_shift(self) -> u32 {
        self.bits() - 1
    }

    /// Mask covering the trailing significand.
    pub const fn mantissa_mask(self) -> u64 {
        (1u64 << self.mantissa_bits()) - 1
    }

    /// The quiet bit — the significand's most significant bit.
    pub const fn quiet_bit(self) -> u64 {
        1u64 << (self.mantissa_bits() - 1)
    }

    /// Positive infinity.
    pub const fn infinity(self) -> u64 {
        (self.max_exponent() as u64) << self.mantissa_bits()
    }

    /// The default quiet NaN, which is what `FPCR.DN` forces and what an
    /// invalid operation produces from non-NaN inputs.
    pub const fn default_nan(self) -> u64 {
        self.infinity() | self.quiet_bit()
    }

    /// The largest finite magnitude, with a clear sign bit.
    pub const fn max_finite(self) -> u64 {
        self.infinity() - 1
    }

    /// Whether `bits` has the exponent and significand of a NaN.
    pub const fn is_nan(self, bits: u64) -> bool {
        let exponent = (bits >> self.mantissa_bits()) & self.max_exponent() as u64;
        exponent == self.max_exponent() as u64 && bits & self.mantissa_mask() != 0
    }

    /// Whether `bits` is a NaN with its quiet bit clear.
    pub const fn is_signalling_nan(self, bits: u64) -> bool {
        self.is_nan(bits) && bits & self.quiet_bit() == 0
    }

    /// Whether `bits` is an infinity.
    pub const fn is_infinite(self, bits: u64) -> bool {
        let exponent = (bits >> self.mantissa_bits()) & self.max_exponent() as u64;
        exponent == self.max_exponent() as u64 && bits & self.mantissa_mask() == 0
    }

    /// Whether `bits` is a zero of either sign.
    pub const fn is_zero(self, bits: u64) -> bool {
        bits & !(1u64 << self.sign_shift()) == 0
    }

    /// Whether `bits` is subnormal — a zero exponent with a non-zero
    /// significand.
    pub const fn is_subnormal(self, bits: u64) -> bool {
        (bits >> self.mantissa_bits()) & self.max_exponent() as u64 == 0
            && bits & self.mantissa_mask() != 0
    }

    /// The sign bit of `bits`.
    pub const fn is_negative(self, bits: u64) -> bool {
        bits >> self.sign_shift() & 1 == 1
    }

    /// Turns a signalling NaN into its quiet counterpart, leaving the rest of
    /// the payload alone. Any other value passes through unchanged.
    pub const fn quieten(self, bits: u64) -> u64 {
        if self.is_signalling_nan(bits) {
            bits | self.quiet_bit()
        } else {
            bits
        }
    }

    /// A zero of the given sign.
    pub const fn zero(self, is_negative: bool) -> u64 {
        (is_negative as u64) << self.sign_shift()
    }

    /// An infinity of the given sign.
    pub const fn signed_infinity(self, is_negative: bool) -> u64 {
        self.infinity() | self.zero(is_negative)
    }
}

/// The outcome of an FP comparison, before it becomes NZCV.
///
/// `Unordered` is a fourth case rather than a flag on the other three: IEEE
/// comparison is a partial order, and collapsing unordered into "not equal"
/// is exactly the bug that makes `FCMP` set the wrong flags for NaN operands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FpComparison {
    /// The first operand is smaller.
    Less,
    /// The operands are equal, including `+0.0 == -0.0`.
    Equal,
    /// The first operand is larger.
    Greater,
    /// At least one operand is a NaN.
    Unordered,
}

impl FpComparison {
    /// The NZCV value `FCMP` writes for this outcome.
    ///
    /// The architecture's table: less is `0b1000`, equal `0b0110`, greater
    /// `0b0010`, unordered `0b0011`.
    pub const fn to_nzcv(self) -> u8 {
        match self {
            FpComparison::Less => 0b1000,
            FpComparison::Equal => 0b0110,
            FpComparison::Greater => 0b0010,
            FpComparison::Unordered => 0b0011,
        }
    }
}

/// A backend result: the value, and whatever exception flags producing it
/// raised.
///
/// One type rather than an out-parameter because every backend entry point
/// returns both, and the native backend returning [`FpExceptions::NONE`] is the
/// documented divergence rather than something a call site should have to
/// remember to skip.
///
/// [`FpExceptions::NONE`]: super::control::FpExceptions::NONE
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FpResult {
    /// The result, as raw bits in its format.
    pub bits: u64,
    /// Flags raised while computing it.
    pub raised: super::control::FpExceptions,
}

impl FpResult {
    /// A result that raised nothing.
    pub const fn exact(bits: u64) -> Self {
        Self {
            bits,
            raised: super::control::FpExceptions::NONE,
        }
    }

    /// A result that raised `raised`.
    pub const fn raising(bits: u64, raised: super::control::FpExceptions) -> Self {
        Self { bits, raised }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_geometry_matches_ieee_754() {
        let expected = [
            (FpFormat::Half, 16, 10, 5, 15),
            (FpFormat::Single, 32, 23, 8, 127),
            (FpFormat::Double, 64, 52, 11, 1023),
        ];

        for (format, bits, mantissa, exponent, bias) in expected {
            assert_eq!(format.bits(), bits, "{format:?} width");
            assert_eq!(format.mantissa_bits(), mantissa, "{format:?} mantissa");
            assert_eq!(format.exponent_bits(), exponent, "{format:?} exponent");
            assert_eq!(format.exponent_bias(), bias, "{format:?} bias");
        }
    }

    #[test]
    fn the_named_constants_match_the_hosts_own_float_encodings() {
        // Cross-checked against Rust's f32/f64 rather than hand-computed, so a
        // transposed shift cannot pass.
        assert_eq!(FpFormat::Single.infinity(), f32::INFINITY.to_bits() as u64);
        assert_eq!(FpFormat::Double.infinity(), f64::INFINITY.to_bits());
        assert_eq!(FpFormat::Single.max_finite(), f32::MAX.to_bits() as u64);
        assert_eq!(FpFormat::Double.max_finite(), f64::MAX.to_bits());
        assert_eq!(FpFormat::Single.default_nan(), 0x7fc0_0000);
        assert_eq!(FpFormat::Double.default_nan(), 0x7ff8_0000_0000_0000);
        assert_eq!(FpFormat::Half.default_nan(), 0x7e00);
    }

    #[test]
    fn classification_separates_nan_infinity_zero_and_subnormal() {
        let single = FpFormat::Single;

        assert!(single.is_nan(single.default_nan()));
        assert!(!single.is_nan(single.infinity()));
        assert!(single.is_infinite(single.infinity()));
        assert!(!single.is_infinite(single.default_nan()));
        assert!(single.is_zero(0));
        assert!(single.is_zero(0x8000_0000), "negative zero is a zero");
        assert!(!single.is_zero(1));
        assert!(single.is_subnormal(1), "the smallest subnormal");
        assert!(!single.is_subnormal(0), "zero is not subnormal");
        assert!(!single.is_subnormal(1.0f32.to_bits() as u64));
    }

    #[test]
    fn a_signalling_nan_is_a_nan_with_a_clear_quiet_bit() {
        let single = FpFormat::Single;
        let signalling = 0x7f80_0001;
        let quiet = 0x7fc0_0001;

        assert!(single.is_signalling_nan(signalling));
        assert!(!single.is_signalling_nan(quiet));
        assert!(single.is_nan(signalling) && single.is_nan(quiet));
        // Quietening keeps the payload, which is what NaN propagation requires.
        assert_eq!(single.quieten(signalling), quiet);
        assert_eq!(single.quieten(quiet), quiet, "already quiet");
        assert_eq!(single.quieten(0), 0, "not a NaN");
    }

    #[test]
    fn comparison_outcomes_map_to_the_architectures_nzcv_values() {
        assert_eq!(FpComparison::Less.to_nzcv(), 0b1000);
        assert_eq!(FpComparison::Equal.to_nzcv(), 0b0110);
        assert_eq!(FpComparison::Greater.to_nzcv(), 0b0010);
        assert_eq!(FpComparison::Unordered.to_nzcv(), 0b0011);
    }

    #[test]
    fn rmode_maps_to_rounding_in_encoding_order() {
        assert_eq!(FpRounding::from_rmode(0b00), FpRounding::Nearest);
        assert_eq!(FpRounding::from_rmode(0b01), FpRounding::Plus);
        assert_eq!(FpRounding::from_rmode(0b10), FpRounding::Minus);
        assert_eq!(FpRounding::from_rmode(0b11), FpRounding::Zero);
    }
}
