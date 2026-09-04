//! A floor under interpreter throughput, so a regression fails CI.
//!
//! `benches/interp_mips.rs` is the report a human reads; this is the guard. It
//! asserts a deliberately loose floor — a fraction of what the benchmark
//! measures — because a test that tracked the benchmark closely would fail on
//! a loaded CI machine and get muted, which is worse than no test.
//!
//! The floor is in guest MIPS rather than in seconds so it means the same thing
//! on a faster or slower machine.

#![cfg(not(target_arch = "wasm32"))]

use std::time::Instant;

use coracle_core::guest_memory::PHYS_RAM_BASE;
use coracle_core::interp::{Cpu, FlatMemory, Memory, Stop};
use coracle_core::reg::Gpr;

/// The `coremark-like` kernel from `benches/kernels.rs`.
const KERNEL: [u32; 8] = [
    0xd100_0421,
    0x8b03_0042,
    0xca05_0084,
    0xaa07_00c6,
    0x8a03_004a,
    0x8b02_016b,
    0xcb01_018c,
    0x17ff_fff9,
];

/// Instructions the timed run retires.
const MEASURED_INSNS: u64 = 5_000_000;

/// The floor, well under the ~120 MIPS the benchmark reports on a developer
/// machine. Crossing it means something structural changed in the dispatch,
/// not that the machine was busy.
const FLOOR_MIPS: f64 = 20.0;

#[test]
fn the_dispatch_stays_fast_enough_to_reach_the_m1_gate() {
    let mut memory = FlatMemory::new(1 << 16);
    for (index, word) in KERNEL.iter().enumerate() {
        memory
            .write_uint(PHYS_RAM_BASE + index as u64 * 4, 4, *word as u64)
            .expect("kernel fits");
    }
    let mut cpu = Cpu::new(memory);
    cpu.regs.set_pc(PHYS_RAM_BASE);
    cpu.regs.write_x(Gpr::X(1), u64::MAX);
    cpu.regs.write_x(Gpr::X(3), 3);
    cpu.run(500_000);

    let started = Instant::now();
    let stop = cpu.run(MEASURED_INSNS);
    let mips = MEASURED_INSNS as f64 / started.elapsed().as_secs_f64() / 1e6;

    assert_eq!(stop, Stop::BudgetExhausted, "the kernel must not trap");
    assert!(
        mips >= FLOOR_MIPS,
        "interpreter fell to {mips:.1} MIPS, below the {FLOOR_MIPS} MIPS floor"
    );
}

#[test]
fn a_warm_loop_decodes_each_instruction_only_once() {
    // The decoded-instruction cache is what the M1 performance risk rests on.
    // If it stopped working the throughput test above would still pass on a
    // fast machine, so the cache behaviour is asserted directly.
    let mut memory = FlatMemory::new(1 << 16);
    for (index, word) in KERNEL.iter().enumerate() {
        memory
            .write_uint(PHYS_RAM_BASE + index as u64 * 4, 4, *word as u64)
            .expect("kernel fits");
    }
    let mut cpu = Cpu::new(memory);
    cpu.regs.set_pc(PHYS_RAM_BASE);

    cpu.run(10_000);

    // Eight distinct instructions, so eight decodes however long it runs.
    assert_eq!(cpu.icount(), 10_000);
    assert_eq!(cpu.decode_count(), KERNEL.len() as u64);
}
