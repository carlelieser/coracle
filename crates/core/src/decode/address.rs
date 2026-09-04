//! Memory addressing modes, as one closed set.
//!
//! Every A64 load and store computes its address in one of these ways. Keeping
//! them in a single enum — rather than spreading base/offset/writeback fields
//! across the instruction — means the memory slice writes one address-generation
//! function and every load/store opcode reuses it.

use super::operand::ExtendedReg;
use crate::reg::Gpr;

/// When the base register is updated relative to the access.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteBack {
    /// The base register is not updated.
    None,
    /// The base is updated before the access; the access uses the new value.
    Pre,
    /// The base is updated after the access; the access uses the old value.
    Post,
}

/// How a load or store forms its effective address.
///
/// The base is [`Gpr::SP`] rather than [`Gpr::ZR`] for slot 31 in every one of
/// these forms — an SP-relative access is the common case, and the decoder has
/// already applied that rule.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AddrMode {
    /// `[base, #offset]` with optional write-back.
    ///
    /// Covers the unsigned scaled offset (`LDR Xt, [Xn, #imm12]`, offset
    /// already multiplied by the access size), the signed unscaled forms
    /// (`LDUR`), and pre/post-indexed forms, which differ only in
    /// [`WriteBack`].
    Immediate {
        /// Base register, `SP` for slot 31.
        base: Gpr,
        /// Byte offset, already scaled and sign-extended by the decoder.
        offset: i64,
        /// Whether and when the base is updated.
        writeback: WriteBack,
    },
    /// `[base, Xm, extend #amount]`.
    ///
    /// The shift amount is 0 or `log2(access size)`, selected by the `S` bit;
    /// the decoder has resolved it.
    Register {
        /// Base register, `SP` for slot 31.
        base: Gpr,
        /// Index register with its extension and scale.
        index: ExtendedReg,
    },
    /// `label` — PC-relative, as used by `LDR (literal)`.
    ///
    /// The offset is relative to the address of this instruction.
    PcRelative {
        /// Byte offset from the instruction's own address.
        offset: i64,
    },
    /// `[base]` with no offset, as used by the exclusive and acquire/release
    /// forms, which have no offset field at all.
    BaseOnly {
        /// Base register, `SP` for slot 31.
        base: Gpr,
    },
}

impl AddrMode {
    /// The base register, when the mode has one.
    ///
    /// `None` for [`AddrMode::PcRelative`], whose address does not read a
    /// register.
    pub const fn base(self) -> Option<Gpr> {
        match self {
            AddrMode::Immediate { base, .. }
            | AddrMode::Register { base, .. }
            | AddrMode::BaseOnly { base } => Some(base),
            AddrMode::PcRelative { .. } => None,
        }
    }

    /// Whether this access updates its base register.
    pub const fn has_writeback(self) -> bool {
        matches!(
            self,
            AddrMode::Immediate {
                writeback: WriteBack::Pre | WriteBack::Post,
                ..
            }
        )
    }
}

/// Width of a memory access, and how a narrower load fills the destination.
///
/// Sign-extension is part of the access rather than a separate opcode because
/// `LDRB`/`LDRSB`/`LDRSH` differ only here; folding it in keeps one `Ldr`
/// opcode instead of eight.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AccessSize {
    /// Bytes touched: 1, 2, 4, 8 or 16.
    pub bytes: u8,
    /// Whether a load sign-extends into the destination register.
    pub is_signed: bool,
}

impl AccessSize {
    /// Unsigned byte.
    pub const B: Self = Self {
        bytes: 1,
        is_signed: false,
    };
    /// Unsigned halfword.
    pub const H: Self = Self {
        bytes: 2,
        is_signed: false,
    };
    /// Unsigned word.
    pub const W: Self = Self {
        bytes: 4,
        is_signed: false,
    };
    /// Doubleword.
    pub const X: Self = Self {
        bytes: 8,
        is_signed: false,
    };
    /// 128-bit, for `LDP`/`STP` of `Q` registers and `LDXP`/`STXP`.
    pub const Q: Self = Self {
        bytes: 16,
        is_signed: false,
    };

    /// `log2` of the access size, which is also the register-offset scale.
    pub const fn scale(self) -> u32 {
        self.bytes.trailing_zeros()
    }
}

/// Ordering an access imposes, from the acquire/release and exclusive forms.
///
/// M1 runs a single vCPU so these carry no runtime effect, but they are decoded
/// rather than dropped: the exclusive monitor is part of the M1 gate, and M2's
/// kernel uses acquire/release throughout.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Ordering {
    /// `LDAR`-family acquire semantics.
    pub is_acquire: bool,
    /// `STLR`-family release semantics.
    pub is_release: bool,
    /// Participates in the exclusive monitor (`LDXR`/`STXR`).
    pub is_exclusive: bool,
}

impl Ordering {
    /// A plain access with no ordering requirement.
    pub const PLAIN: Self = Self {
        is_acquire: false,
        is_release: false,
        is_exclusive: false,
    };
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decode::operand::ExtendKind;

    #[test]
    fn every_register_based_mode_reports_its_base() {
        let base = Gpr::SP;

        assert_eq!(
            AddrMode::Immediate {
                base,
                offset: 8,
                writeback: WriteBack::None
            }
            .base(),
            Some(base)
        );
        assert_eq!(AddrMode::BaseOnly { base }.base(), Some(base));
        assert_eq!(
            AddrMode::Register {
                base,
                index: ExtendedReg {
                    reg: Gpr::X(1),
                    kind: ExtendKind::Uxtx,
                    amount: 3
                }
            }
            .base(),
            Some(base)
        );
        assert_eq!(AddrMode::PcRelative { offset: -4 }.base(), None);
    }

    #[test]
    fn only_pre_and_post_indexed_modes_write_back() {
        let indexed = |writeback| AddrMode::Immediate {
            base: Gpr::X(0),
            offset: 16,
            writeback,
        };

        assert!(!indexed(WriteBack::None).has_writeback());
        assert!(indexed(WriteBack::Pre).has_writeback());
        assert!(indexed(WriteBack::Post).has_writeback());
        assert!(!AddrMode::BaseOnly { base: Gpr::X(0) }.has_writeback());
    }

    #[test]
    fn the_access_scale_is_log2_of_its_width() {
        assert_eq!(AccessSize::B.scale(), 0);
        assert_eq!(AccessSize::H.scale(), 1);
        assert_eq!(AccessSize::W.scale(), 2);
        assert_eq!(AccessSize::X.scale(), 3);
        assert_eq!(AccessSize::Q.scale(), 4);
    }

    #[test]
    fn a_plain_access_imposes_no_ordering() {
        assert_eq!(Ordering::PLAIN, Ordering::default());
    }
}
