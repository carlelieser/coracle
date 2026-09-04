//! Operand vocabulary shared by every A64 instruction group.
//!
//! The A64 encoding reuses a small set of operand shapes across hundreds of
//! mnemonics: a shifted register, an extended register, a scaled or unscaled
//! memory offset, a condition code. Naming each shape once — rather than once
//! per instruction — is what keeps [`crate::decode::Instruction`] flat.

use crate::reg::{Gpr, Vec};

/// Operand register width for the integer datapath.
///
/// `sf` in the encoding. Selects both the operand width and, on a write,
/// whether the upper 32 bits are zeroed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegWidth {
    /// 32-bit `W` form.
    W32,
    /// 64-bit `X` form.
    X64,
}

impl RegWidth {
    /// Decodes the `sf` bit.
    pub const fn from_sf(sf: bool) -> Self {
        if sf {
            RegWidth::X64
        } else {
            RegWidth::W32
        }
    }

    /// Width in bits.
    pub const fn bits(self) -> u32 {
        match self {
            RegWidth::W32 => 32,
            RegWidth::X64 => 64,
        }
    }

    /// Mask covering the operand's significant bits.
    pub const fn mask(self) -> u64 {
        match self {
            RegWidth::W32 => u32::MAX as u64,
            RegWidth::X64 => u64::MAX,
        }
    }
}

/// How a register operand is shifted before use.
///
/// The `shift` field of data-processing (register) encodings. `ROR` is legal
/// only for the logical group; the arithmetic group's decoder rejects it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShiftKind {
    /// Logical shift left.
    Lsl,
    /// Logical shift right.
    Lsr,
    /// Arithmetic shift right.
    Asr,
    /// Rotate right.
    Ror,
}

/// A register operand with a constant shift applied.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShiftedReg {
    /// The register to read.
    pub reg: Gpr,
    /// Shift applied to the read value.
    pub kind: ShiftKind,
    /// Shift amount, already range-checked against the operand width.
    pub amount: u8,
}

/// How a register operand is extended and shifted before use.
///
/// The `option` field of the add/sub extended-register and load/store
/// register-offset encodings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExtendKind {
    /// Unsigned extend byte.
    Uxtb,
    /// Unsigned extend halfword.
    Uxth,
    /// Unsigned extend word.
    Uxtw,
    /// Unsigned extend doubleword (a no-op widening).
    Uxtx,
    /// Signed extend byte.
    Sxtb,
    /// Signed extend halfword.
    Sxth,
    /// Signed extend word.
    Sxtw,
    /// Signed extend doubleword (a no-op widening).
    Sxtx,
}

impl ExtendKind {
    /// Decodes the 3-bit `option` field.
    pub const fn from_option(option: u8) -> Self {
        match option & 0b111 {
            0b000 => ExtendKind::Uxtb,
            0b001 => ExtendKind::Uxth,
            0b010 => ExtendKind::Uxtw,
            0b011 => ExtendKind::Uxtx,
            0b100 => ExtendKind::Sxtb,
            0b101 => ExtendKind::Sxth,
            0b110 => ExtendKind::Sxtw,
            _ => ExtendKind::Sxtx,
        }
    }

    /// Number of source bits taken from the register before extension.
    pub const fn source_bits(self) -> u32 {
        match self {
            ExtendKind::Uxtb | ExtendKind::Sxtb => 8,
            ExtendKind::Uxth | ExtendKind::Sxth => 16,
            ExtendKind::Uxtw | ExtendKind::Sxtw => 32,
            ExtendKind::Uxtx | ExtendKind::Sxtx => 64,
        }
    }

    /// Whether the extension replicates the source's sign bit.
    pub const fn is_signed(self) -> bool {
        matches!(
            self,
            ExtendKind::Sxtb | ExtendKind::Sxth | ExtendKind::Sxtw | ExtendKind::Sxtx
        )
    }
}

/// A register operand extended from a narrower width, then shifted left.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExtendedReg {
    /// The register to read.
    pub reg: Gpr,
    /// Extension applied to the read value.
    pub kind: ExtendKind,
    /// Left shift applied after extension, 0–4.
    pub amount: u8,
}

/// AArch64 condition codes, in encoding order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cond {
    /// Equal: `Z == 1`.
    Eq,
    /// Not equal.
    Ne,
    /// Carry set / unsigned higher or same.
    Cs,
    /// Carry clear / unsigned lower.
    Cc,
    /// Minus / negative.
    Mi,
    /// Plus / positive or zero.
    Pl,
    /// Overflow set.
    Vs,
    /// Overflow clear.
    Vc,
    /// Unsigned higher.
    Hi,
    /// Unsigned lower or same.
    Ls,
    /// Signed greater than or equal.
    Ge,
    /// Signed less than.
    Lt,
    /// Signed greater than.
    Gt,
    /// Signed less than or equal.
    Le,
    /// Always. `AL` and `NV` both test true; the encodings differ, the
    /// behaviour does not.
    Al,
    /// Never — architecturally identical to [`Cond::Al`], kept distinct so an
    /// encode/decode round trip is lossless.
    Nv,
}

impl Cond {
    /// Decodes the 4-bit `cond` field.
    pub const fn from_bits(bits: u8) -> Self {
        match bits & 0b1111 {
            0b0000 => Cond::Eq,
            0b0001 => Cond::Ne,
            0b0010 => Cond::Cs,
            0b0011 => Cond::Cc,
            0b0100 => Cond::Mi,
            0b0101 => Cond::Pl,
            0b0110 => Cond::Vs,
            0b0111 => Cond::Vc,
            0b1000 => Cond::Hi,
            0b1001 => Cond::Ls,
            0b1010 => Cond::Ge,
            0b1011 => Cond::Lt,
            0b1100 => Cond::Gt,
            0b1101 => Cond::Le,
            0b1110 => Cond::Al,
            _ => Cond::Nv,
        }
    }

    /// The condition that is true exactly when this one is false.
    ///
    /// `AL` and `NV` are the exception: both test true, so neither has an
    /// inverse and this returns them unchanged.
    pub const fn invert(self) -> Self {
        match self {
            Cond::Eq => Cond::Ne,
            Cond::Ne => Cond::Eq,
            Cond::Cs => Cond::Cc,
            Cond::Cc => Cond::Cs,
            Cond::Mi => Cond::Pl,
            Cond::Pl => Cond::Mi,
            Cond::Vs => Cond::Vc,
            Cond::Vc => Cond::Vs,
            Cond::Hi => Cond::Ls,
            Cond::Ls => Cond::Hi,
            Cond::Ge => Cond::Lt,
            Cond::Lt => Cond::Ge,
            Cond::Gt => Cond::Le,
            Cond::Le => Cond::Gt,
            Cond::Al => Cond::Al,
            Cond::Nv => Cond::Nv,
        }
    }
}

/// Where a SIMD/FP operand's data sits inside its 128-bit register.
///
/// One type covers both the scalar forms (`b`/`h`/`s`/`d`/`q`) and the vector
/// arrangements, because the FP and NEON slices share the register file and the
/// same instruction encodings often differ only in `Q` and `size`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VecShape {
    /// A single element of the given width, in lane 0, upper bits zeroed on
    /// write.
    Scalar(ElemSize),
    /// `count` elements of `elem` width, filling the low 64 or full 128 bits.
    Vector {
        /// Element width.
        elem: ElemSize,
        /// Number of active lanes.
        count: u8,
    },
    /// A single addressed lane of a vector register, for `INS`, `DUP`, `MOV`
    /// element forms and the indexed-element FP/NEON encodings.
    Element {
        /// Element width.
        elem: ElemSize,
        /// Lane index.
        index: u8,
    },
}

/// SIMD/FP element width.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ElemSize {
    /// 8-bit.
    B8,
    /// 16-bit.
    H16,
    /// 32-bit.
    S32,
    /// 64-bit.
    D64,
    /// 128-bit. Scalar `Q` forms only; there is no 128-bit lane.
    Q128,
}

impl ElemSize {
    /// Width in bits.
    pub const fn bits(self) -> u32 {
        match self {
            ElemSize::B8 => 8,
            ElemSize::H16 => 16,
            ElemSize::S32 => 32,
            ElemSize::D64 => 64,
            ElemSize::Q128 => 128,
        }
    }

    /// Decodes a 2-bit `size` field, which never selects the `Q` form.
    pub const fn from_size(size: u8) -> Self {
        match size & 0b11 {
            0b00 => ElemSize::B8,
            0b01 => ElemSize::H16,
            0b10 => ElemSize::S32,
            _ => ElemSize::D64,
        }
    }
}

/// A SIMD/FP register operand together with how its data is laid out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VecOperand {
    /// The register to read or write.
    pub reg: Vec,
    /// Which bits of it participate.
    pub shape: VecShape,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_sf_bit_selects_operand_width_and_its_mask() {
        assert_eq!(RegWidth::from_sf(false), RegWidth::W32);
        assert_eq!(RegWidth::from_sf(true), RegWidth::X64);
        assert_eq!(RegWidth::W32.mask(), 0xffff_ffff);
        assert_eq!(RegWidth::X64.mask(), u64::MAX);
        assert_eq!(RegWidth::W32.bits(), 32);
        assert_eq!(RegWidth::X64.bits(), 64);
    }

    #[test]
    fn extend_options_decode_in_encoding_order() {
        let expected = [
            (ExtendKind::Uxtb, 8, false),
            (ExtendKind::Uxth, 16, false),
            (ExtendKind::Uxtw, 32, false),
            (ExtendKind::Uxtx, 64, false),
            (ExtendKind::Sxtb, 8, true),
            (ExtendKind::Sxth, 16, true),
            (ExtendKind::Sxtw, 32, true),
            (ExtendKind::Sxtx, 64, true),
        ];

        for (option, (kind, bits, is_signed)) in expected.into_iter().enumerate() {
            let decoded = ExtendKind::from_option(option as u8);
            assert_eq!(decoded, kind);
            assert_eq!(decoded.source_bits(), bits);
            assert_eq!(decoded.is_signed(), is_signed);
        }
    }

    #[test]
    fn condition_codes_decode_in_encoding_order() {
        assert_eq!(Cond::from_bits(0b0000), Cond::Eq);
        assert_eq!(Cond::from_bits(0b0111), Cond::Vc);
        assert_eq!(Cond::from_bits(0b1101), Cond::Le);
        assert_eq!(Cond::from_bits(0b1110), Cond::Al);
        assert_eq!(Cond::from_bits(0b1111), Cond::Nv);
    }

    #[test]
    fn inverting_a_condition_flips_the_low_encoding_bit() {
        // The architecture defines cond<0> as the inversion bit, so this is the
        // property the table must satisfy — except for AL/NV, which both pass.
        for bits in 0..14u8 {
            let cond = Cond::from_bits(bits);
            assert_eq!(cond.invert(), Cond::from_bits(bits ^ 1));
            assert_eq!(cond.invert().invert(), cond);
        }
        assert_eq!(Cond::Al.invert(), Cond::Al);
        assert_eq!(Cond::Nv.invert(), Cond::Nv);
    }

    #[test]
    fn element_sizes_decode_from_the_two_bit_size_field() {
        assert_eq!(ElemSize::from_size(0b00).bits(), 8);
        assert_eq!(ElemSize::from_size(0b01).bits(), 16);
        assert_eq!(ElemSize::from_size(0b10).bits(), 32);
        assert_eq!(ElemSize::from_size(0b11).bits(), 64);
        assert_eq!(ElemSize::Q128.bits(), 128);
    }
}
