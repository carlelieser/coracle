//! Processor state: condition flags, interrupt masks, exception level.
//!
//! The in-memory layout is deliberately not the architectural `SPSR` packing.
//! It is split into named fields because the interpreter touches NZCV on almost
//! every data-processing instruction and packing/unpacking bitfields there
//! would be the hot path. [`Pstate::to_trace_word`] produces the normalised
//! word the trace stream requires (`tests/TRACE_FORMAT.md` §6).

/// Bit position of `N` within the trace word's NZCV nibble.
const TRACE_NZCV_SHIFT: u32 = 28;
const TRACE_DAIF_SHIFT: u32 = 6;
const TRACE_EL_SHIFT: u32 = 2;

/// Exception level. EL2 and EL3 do not exist on this machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ExceptionLevel {
    /// Application level.
    #[default]
    El0 = 0,
    /// Kernel level. Reached in M2.
    El1 = 1,
}

/// Which stack pointer `SP` refers to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum StackPointerSelect {
    /// `SP_EL0`.
    #[default]
    El0 = 0,
    /// `SP_ELx` for the current level.
    Elx = 1,
}

/// The NZCV condition flags.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Nzcv {
    /// Negative.
    pub n: bool,
    /// Zero.
    pub z: bool,
    /// Carry / unsigned overflow.
    pub c: bool,
    /// Signed overflow.
    pub v: bool,
}

impl Nzcv {
    /// Unpacks the four flags from a 4-bit `NZCV` field.
    pub const fn from_bits(bits: u8) -> Self {
        Self {
            n: bits & 0b1000 != 0,
            z: bits & 0b0100 != 0,
            c: bits & 0b0010 != 0,
            v: bits & 0b0001 != 0,
        }
    }

    /// Packs the four flags into a 4-bit `NZCV` field.
    pub const fn to_bits(self) -> u8 {
        (self.n as u8) << 3 | (self.z as u8) << 2 | (self.c as u8) << 1 | self.v as u8
    }
}

/// The `DAIF` interrupt masks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Daif {
    /// Debug exception mask.
    pub d: bool,
    /// SError mask.
    pub a: bool,
    /// IRQ mask.
    pub i: bool,
    /// FIQ mask.
    pub f: bool,
}

impl Daif {
    /// Unpacks the four masks from a 4-bit `DAIF` field.
    pub const fn from_bits(bits: u8) -> Self {
        Self {
            d: bits & 0b1000 != 0,
            a: bits & 0b0100 != 0,
            i: bits & 0b0010 != 0,
            f: bits & 0b0001 != 0,
        }
    }

    /// Packs the four masks into a 4-bit `DAIF` field.
    pub const fn to_bits(self) -> u8 {
        (self.d as u8) << 3 | (self.a as u8) << 2 | (self.i as u8) << 1 | self.f as u8
    }
}

/// Processor state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Pstate {
    /// Condition flags.
    pub nzcv: Nzcv,
    /// Interrupt masks.
    pub daif: Daif,
    /// Current exception level.
    pub el: ExceptionLevel,
    /// Which stack pointer `SP` names.
    pub sp_sel: StackPointerSelect,
}

impl Pstate {
    /// Packs into the normalised trace word: NZCV at 31..28, DAIF at 9..6,
    /// `CurrentEL` at 3..2, `SPSel` at 0, every other bit zero.
    pub const fn to_trace_word(self) -> u64 {
        (self.nzcv.to_bits() as u64) << TRACE_NZCV_SHIFT
            | (self.daif.to_bits() as u64) << TRACE_DAIF_SHIFT
            | (self.el as u64) << TRACE_EL_SHIFT
            | self.sp_sel as u64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn condition_flags_round_trip_through_their_packed_field() {
        for bits in 0..16u8 {
            assert_eq!(Nzcv::from_bits(bits).to_bits(), bits);
            assert_eq!(Daif::from_bits(bits).to_bits(), bits);
        }
    }

    #[test]
    fn the_trace_word_places_every_field_where_the_format_says() {
        let state = Pstate {
            nzcv: Nzcv::from_bits(0b1010),
            daif: Daif::from_bits(0b0110),
            el: ExceptionLevel::El1,
            sp_sel: StackPointerSelect::Elx,
        };

        let word = state.to_trace_word();

        assert_eq!((word >> 28) & 0xf, 0b1010);
        assert_eq!((word >> 6) & 0xf, 0b0110);
        assert_eq!((word >> 2) & 0x3, 1);
        assert_eq!(word & 1, 1);
    }

    #[test]
    fn the_trace_word_leaves_every_other_bit_zero() {
        // TRACE_FORMAT §6 makes this a hard requirement: stray bits are what
        // produce false divergences against QEMU's `cpsr`.
        let all_fields_set = Pstate {
            nzcv: Nzcv::from_bits(0b1111),
            daif: Daif::from_bits(0b1111),
            el: ExceptionLevel::El1,
            sp_sel: StackPointerSelect::Elx,
        };

        let occupied = 0xf << 28 | 0xf << 6 | 0x3 << 2 | 1;
        assert_eq!(all_fields_set.to_trace_word() & !occupied, 0);
        assert_eq!(Pstate::default().to_trace_word(), 0);
    }
}
