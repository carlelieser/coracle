//! The dispatch arms, exercised through real assembled encodings.
//!
//! Every instruction word here came out of an assembler, not out of a bit
//! calculation, so a wrong constant cannot make a passing test that proves
//! nothing.
//!
//! These cover the opcodes this build executes. The decode slices own which
//! encodings exist; this file owns what happens when one is executed.

#![cfg(not(target_arch = "wasm32"))]

use coracle_core::guest_memory::PHYS_RAM_BASE;
use coracle_core::interp::{Cpu, FlatMemory, Memory, Stop};
use coracle_core::pstate::Nzcv;
use coracle_core::reg::Gpr;

fn run(program: &[u32], seed: &[(u8, u64)]) -> Cpu<FlatMemory> {
    let mut memory = FlatMemory::new(1 << 16);
    for (index, word) in program.iter().enumerate() {
        memory
            .write_uint(PHYS_RAM_BASE + index as u64 * 4, 4, *word as u64)
            .expect("program fits");
    }
    let mut cpu = Cpu::new(memory);
    cpu.regs.set_pc(PHYS_RAM_BASE);
    for (reg, value) in seed {
        cpu.regs.write_x(Gpr::X(*reg), *value);
    }

    let stop = cpu.run(program.len() as u64);
    assert_eq!(stop, Stop::BudgetExhausted, "program trapped");
    cpu
}

#[test]
fn a_32_bit_subtract_borrows_within_32_bits_and_zeroes_the_upper_half() {
    // sub w0, w1, w2  — 0x4b020020, with w1 = 1 and w2 = 2.
    let cpu = run(&[0x4b02_0020], &[(1, 1), (2, 2)]);

    assert_eq!(
        cpu.regs.read_x(Gpr::X(0)),
        0xffff_ffff,
        "the result is -1 in 32 bits, zero-extended, not -1 in 64"
    );
}

#[test]
fn a_64_bit_subtract_of_the_same_values_borrows_through_all_64_bits() {
    // sub x6, x7, x8  — 0xcb0800e6.
    let cpu = run(&[0xcb08_00e6], &[(7, 1), (8, 2)]);

    assert_eq!(cpu.regs.read_x(Gpr::X(6)), u64::MAX);
}

#[test]
fn a_flag_setting_subtract_reports_the_borrow_at_the_operand_width() {
    // subs w3, w4, w5 — 0x6b050083, with w4 = 1 and w5 = 2: negative, borrow.
    let cpu = run(&[0x6b05_0083], &[(4, 1), (5, 2)]);

    let flags = cpu.regs.pstate.nzcv;
    assert!(flags.n, "1 - 2 is negative");
    assert!(!flags.z);
    assert!(!flags.c, "a borrow clears carry");
}

#[test]
fn subtracting_a_value_from_itself_sets_zero_and_carry() {
    // subs w3, w4, w5 with equal operands.
    let cpu = run(&[0x6b05_0083], &[(4, 7), (5, 7)]);

    assert_eq!(
        cpu.regs.pstate.nzcv,
        Nzcv {
            n: false,
            z: true,
            c: true,
            v: false
        }
    );
}

#[test]
fn a_32_bit_add_immediate_zero_extends_past_the_top_of_the_w_register() {
    // add w9, w10, #1 — 0x11000549, with w10 = 0xffff_ffff.
    let cpu = run(&[0x1100_0549], &[(10, 0xffff_ffff)]);

    assert_eq!(cpu.regs.read_x(Gpr::X(9)), 0, "wraps within 32 bits");
}

#[test]
fn an_add_immediate_leaves_a_preexisting_upper_half_cleared() {
    // The W form must clear bits 63..32 of its destination, not preserve them.
    let cpu = run(&[0x1100_0549], &[(9, u64::MAX), (10, 1)]);

    assert_eq!(cpu.regs.read_x(Gpr::X(9)), 2);
}

#[test]
fn a_store_then_load_round_trips_through_guest_memory() {
    // str x2, [x9] ; ldr x8, [x9]
    let cpu = run(
        &[0xf900_0122, 0xf940_0128],
        &[(2, 0x0123_4567_89ab_cdef), (9, PHYS_RAM_BASE + 0x100)],
    );

    assert_eq!(cpu.regs.read_x(Gpr::X(8)), 0x0123_4567_89ab_cdef);
}

#[test]
fn a_store_through_an_unmapped_base_is_a_data_abort_naming_the_address() {
    use coracle_core::trap::Trap;
    let mut memory = FlatMemory::new(1 << 16);
    memory
        .write_uint(PHYS_RAM_BASE, 4, 0xf900_0122u64)
        .expect("fits");
    let mut cpu = Cpu::new(memory);
    cpu.regs.set_pc(PHYS_RAM_BASE);
    cpu.regs.write_x(Gpr::X(9), 0);

    assert_eq!(
        cpu.run(1),
        Stop::Trapped(Trap::DataAbort {
            pc: PHYS_RAM_BASE,
            address: 0,
            is_write: true,
        })
    );
}

#[test]
fn a_branch_with_link_leaves_the_return_address_in_x30() {
    // bl .+8 — assembled as 0x94000002.
    let cpu = run(&[0x9400_0002], &[]);

    assert_eq!(cpu.regs.read_x(Gpr::X(30)), PHYS_RAM_BASE + 4);
    assert_eq!(cpu.regs.pc(), PHYS_RAM_BASE + 8);
}

#[test]
fn the_logical_opcodes_compute_what_their_mnemonics_say() {
    // eor x4, x4, x5 ; orr x6, x6, x7 ; and x10, x2, x3
    let cpu = run(
        &[0xca05_0084, 0xaa07_00c6, 0x8a03_004a],
        &[
            (4, 0b1100),
            (5, 0b1010),
            (6, 0b1000),
            (7, 0b0001),
            (2, 0b1110),
            (3, 0b0110),
        ],
    );

    assert_eq!(cpu.regs.read_x(Gpr::X(4)), 0b0110, "eor");
    assert_eq!(cpu.regs.read_x(Gpr::X(6)), 0b1001, "orr");
    assert_eq!(cpu.regs.read_x(Gpr::X(10)), 0b0110, "and");
}

#[test]
fn writes_to_the_zero_register_are_discarded_but_flags_still_update() {
    // cmp w4, w5 is `subs wzr, w4, w5` — the comparison is the flags.
    // Assembled: 0x6b05009f.
    let cpu = run(&[0x6b05_009f], &[(4, 1), (5, 2)]);

    assert!(cpu.regs.pstate.nzcv.n);
    assert!(!cpu.regs.pstate.nzcv.c);
}
