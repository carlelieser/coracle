//! The architectural register file.
//!
//! `x31` never reaches this type. The decoder has already resolved it to
//! [`Gpr::ZR`] or [`Gpr::SP`], so reads and writes here are a bounds-checked
//! array access plus one branch for the two special forms — no per-instruction
//! "did this encoding mean SP?" question.

use crate::pstate::Pstate;
use crate::reg::{Gpr, Vec, NUM_GPR};

/// X0–X30, SP, PC, V0–V31 and PSTATE.
///
/// FPCR/FPSR live here too: the trace stream carries them alongside the
/// architectural core, and the FP backend selection in `docs/plan.md` §2 is
/// keyed on a cached FPCR flag.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegFile {
    x: [u64; NUM_GPR],
    sp: u64,
    pc: u64,
    v: [u128; Vec::COUNT],
    /// Processor state.
    pub pstate: Pstate,
    /// Floating-point control register.
    pub fpcr: u64,
    /// Floating-point status register.
    pub fpsr: u64,
}

impl Default for RegFile {
    fn default() -> Self {
        Self::new()
    }
}

impl RegFile {
    /// A register file with every architectural register zeroed.
    pub const fn new() -> Self {
        Self {
            x: [0; NUM_GPR],
            sp: 0,
            pc: 0,
            v: [0; Vec::COUNT],
            pstate: Pstate {
                nzcv: crate::pstate::Nzcv {
                    n: false,
                    z: false,
                    c: false,
                    v: false,
                },
                daif: crate::pstate::Daif {
                    d: false,
                    a: false,
                    i: false,
                    f: false,
                },
                el: crate::pstate::ExceptionLevel::El0,
                sp_sel: crate::pstate::StackPointerSelect::El0,
            },
            fpcr: 0,
            fpsr: 0,
        }
    }

    /// Reads a 64-bit general-purpose operand. `ZR` reads as zero.
    pub fn read_x(&self, reg: Gpr) -> u64 {
        match reg {
            Gpr::X(index) => self.x[index as usize],
            Gpr::SP => self.sp,
            Gpr::ZR => 0,
        }
    }

    /// Reads the low 32 bits of a general-purpose operand.
    pub fn read_w(&self, reg: Gpr) -> u32 {
        self.read_x(reg) as u32
    }

    /// Writes a 64-bit general-purpose operand. Writes to `ZR` are discarded.
    pub fn write_x(&mut self, reg: Gpr, value: u64) {
        match reg {
            Gpr::X(index) => self.x[index as usize] = value,
            Gpr::SP => self.sp = value,
            Gpr::ZR => {}
        }
    }

    /// Writes a 32-bit result, zero-extending to the full 64-bit register.
    ///
    /// Zero-extension is architectural, not a convenience: a `W`-form write
    /// clears the upper half rather than preserving it.
    pub fn write_w(&mut self, reg: Gpr, value: u32) {
        self.write_x(reg, value as u64);
    }

    /// Reads the full 128 bits of a SIMD/FP register.
    pub fn read_v(&self, reg: Vec) -> u128 {
        self.v[reg.index() as usize]
    }

    /// Writes the full 128 bits of a SIMD/FP register.
    pub fn write_v(&mut self, reg: Vec, value: u128) {
        self.v[reg.index() as usize] = value;
    }

    /// The program counter.
    pub const fn pc(&self) -> u64 {
        self.pc
    }

    /// Sets the program counter.
    pub const fn set_pc(&mut self, value: u64) {
        self.pc = value;
    }

    /// The stack pointer, independent of whether any operand named it.
    pub const fn sp(&self) -> u64 {
        self.sp
    }

    /// Sets the stack pointer.
    pub const fn set_sp(&mut self, value: u64) {
        self.sp = value;
    }

    /// Reads `xn` by architectural index, for the trace and exception paths
    /// that walk the whole file rather than a decoded operand.
    ///
    /// # Panics
    ///
    /// Panics if `index` is 31 or above; `x31` is not a storage location.
    pub const fn x(&self, index: u8) -> u64 {
        self.x[index as usize]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_zero_register_reads_zero_and_swallows_writes() {
        let mut regs = RegFile::new();

        regs.write_x(Gpr::ZR, 0xdead_beef_dead_beef);

        assert_eq!(regs.read_x(Gpr::ZR), 0);
        // Nothing else moved either.
        assert_eq!(regs, RegFile::new());
    }

    #[test]
    fn sp_and_x_slot_31_are_different_storage() {
        let mut regs = RegFile::new();

        regs.write_x(Gpr::SP, 0x1000);
        regs.write_x(Gpr::X(30), 0x2000);

        assert_eq!(regs.read_x(Gpr::SP), 0x1000);
        assert_eq!(regs.sp(), 0x1000);
        assert_eq!(regs.read_x(Gpr::X(30)), 0x2000);
    }

    #[test]
    fn a_w_form_write_clears_the_upper_half() {
        let mut regs = RegFile::new();
        regs.write_x(Gpr::X(5), 0xffff_ffff_ffff_ffff);

        regs.write_w(Gpr::X(5), 0x1234_5678);

        assert_eq!(regs.read_x(Gpr::X(5)), 0x1234_5678);
        assert_eq!(regs.read_w(Gpr::X(5)), 0x1234_5678);
    }

    #[test]
    fn vector_registers_hold_all_128_bits() {
        let mut regs = RegFile::new();
        let value = 0x0f0e_0d0c_0b0a_0908_0706_0504_0302_0100u128;

        regs.write_v(Vec::new(31), value);

        assert_eq!(regs.read_v(Vec::new(31)), value);
        assert_eq!(regs.read_v(Vec::new(30)), 0);
    }

    #[test]
    fn every_general_purpose_register_is_independent_storage() {
        let mut regs = RegFile::new();

        for index in 0..NUM_GPR as u8 {
            regs.write_x(Gpr::X(index), 0x100 + index as u64);
        }

        for index in 0..NUM_GPR as u8 {
            assert_eq!(regs.x(index), 0x100 + index as u64);
        }
    }
}
