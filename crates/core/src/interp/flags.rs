//! NZCV production and consumption.
//!
//! Split out of the dispatch because three groups need it — the flag-setting
//! data-processing forms produce it, the conditional forms consume it, and M2's
//! exception entry saves it — and because it is the one piece of the datapath
//! where a width-dependent detail (carry out of bit 31 versus bit 63) is easy
//! to get wrong in each place separately.

use crate::decode::operand::{Cond, RegWidth};
use crate::pstate::Nzcv;

/// The result of an add or subtract, with the flags it would set.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FlaggedResult {
    /// The result, already masked to the operand width.
    pub value: u64,
    /// Flags this computation produces.
    pub flags: Nzcv,
}

/// Adds `left`, `right` and `carry_in` at `width`, reporting NZCV.
///
/// Subtraction is this with `right` inverted and `carry_in` set, which is what
/// the architecture itself specifies, so there is no separate subtract here.
pub fn add_with_carry(operands: (u64, u64), carry_in: bool, width: RegWidth) -> FlaggedResult {
    let (left, right) = (operands.0 & width.mask(), operands.1 & width.mask());
    let carry = carry_in as u64;

    let unsigned = (left as u128) + (right as u128) + (carry as u128);
    let value = (unsigned as u64) & width.mask();

    let sign_bit = 1u64 << (width.bits() - 1);
    let signed_overflow = (left ^ value) & (right ^ value) & sign_bit != 0;

    FlaggedResult {
        value,
        flags: Nzcv {
            n: value & sign_bit != 0,
            z: value == 0,
            c: unsigned >> width.bits() != 0,
            v: signed_overflow,
        },
    }
}

/// Flags a logical operation sets: N and Z from the result, C and V cleared.
pub fn logical_flags(value: u64, width: RegWidth) -> Nzcv {
    let value = value & width.mask();
    Nzcv {
        n: value & (1u64 << (width.bits() - 1)) != 0,
        z: value == 0,
        c: false,
        v: false,
    }
}

/// Whether `cond` holds under `flags`.
pub const fn is_condition_met(cond: Cond, flags: Nzcv) -> bool {
    match cond {
        Cond::Eq => flags.z,
        Cond::Ne => !flags.z,
        Cond::Cs => flags.c,
        Cond::Cc => !flags.c,
        Cond::Mi => flags.n,
        Cond::Pl => !flags.n,
        Cond::Vs => flags.v,
        Cond::Vc => !flags.v,
        Cond::Hi => flags.c && !flags.z,
        Cond::Ls => !(flags.c && !flags.z),
        Cond::Ge => flags.n == flags.v,
        Cond::Lt => flags.n != flags.v,
        Cond::Gt => !flags.z && flags.n == flags.v,
        Cond::Le => !(!flags.z && flags.n == flags.v),
        Cond::Al | Cond::Nv => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adding_zero_to_zero_sets_only_the_zero_flag() {
        let result = add_with_carry((0, 0), false, RegWidth::X64);

        assert_eq!(result.value, 0);
        assert_eq!(
            result.flags,
            Nzcv {
                n: false,
                z: true,
                c: false,
                v: false
            }
        );
    }

    #[test]
    fn carry_out_is_taken_from_the_top_of_the_operand_width_not_the_register() {
        // 0xffff_ffff + 1 carries out of bit 31 at W but not at X.
        let narrow = add_with_carry((0xffff_ffff, 1), false, RegWidth::W32);
        assert!(narrow.flags.c);
        assert_eq!(narrow.value, 0);

        let wide = add_with_carry((0xffff_ffff, 1), false, RegWidth::X64);
        assert!(!wide.flags.c);
        assert_eq!(wide.value, 0x1_0000_0000);
    }

    #[test]
    fn signed_overflow_is_distinct_from_unsigned_carry() {
        // 0x7fff_ffff + 1 overflows signed 32-bit without carrying out.
        let result = add_with_carry((0x7fff_ffff, 1), false, RegWidth::W32);

        assert!(result.flags.v);
        assert!(!result.flags.c);
        assert!(result.flags.n);
    }

    #[test]
    fn subtracting_equal_values_sets_zero_and_carry() {
        // SUBS is add_with_carry(left, !right, 1).
        let result = add_with_carry((7, !7), true, RegWidth::X64);

        assert_eq!(result.value, 0);
        assert!(result.flags.z);
        assert!(result.flags.c, "no borrow means carry set");
        assert!(!result.flags.v);
    }

    #[test]
    fn carry_in_participates_in_the_result_and_the_flags() {
        let result = add_with_carry((u64::MAX, 0), true, RegWidth::X64);

        assert_eq!(result.value, 0);
        assert!(result.flags.c);
        assert!(result.flags.z);
    }

    #[test]
    fn logical_operations_clear_carry_and_overflow() {
        let flags = logical_flags(0xffff_ffff_ffff_ffff, RegWidth::X64);

        assert_eq!(
            flags,
            Nzcv {
                n: true,
                z: false,
                c: false,
                v: false
            }
        );
        assert!(logical_flags(0, RegWidth::W32).z);
        assert!(logical_flags(0x8000_0000, RegWidth::W32).n);
        assert!(!logical_flags(0x8000_0000, RegWidth::X64).n);
    }

    #[test]
    fn each_condition_is_the_negation_of_its_inverse() {
        let all_flag_combinations = (0..16u8).map(Nzcv::from_bits);

        for flags in all_flag_combinations {
            for bits in 0..14u8 {
                let cond = Cond::from_bits(bits);
                assert_ne!(
                    is_condition_met(cond, flags),
                    is_condition_met(cond.invert(), flags),
                    "{cond:?} under {flags:?}"
                );
            }
        }
    }

    #[test]
    fn always_and_never_both_test_true() {
        let flags = Nzcv::from_bits(0);

        assert!(is_condition_met(Cond::Al, flags));
        assert!(is_condition_met(Cond::Nv, flags));
    }

    #[test]
    fn the_signed_and_unsigned_orderings_disagree_where_they_should() {
        // CMP w0, w1 with w0 = -1, w1 = 1: signed less-than, unsigned higher.
        let result = add_with_carry((0xffff_ffffu64, !1u64), true, RegWidth::W32);

        assert!(is_condition_met(Cond::Lt, result.flags));
        assert!(is_condition_met(Cond::Hi, result.flags));
    }
}
