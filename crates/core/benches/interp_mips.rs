//! Interpreter throughput, in guest MIPS and as a ratio to native.
//!
//! The M1 gate has two numbers: ≤ 60× slower than native, and ≥ 40 guest MIPS
//! absolute. The absolute one is what predicts M2's 60-second boot bound, so it
//! is the one this reports first.
//!
//! **What this number is not.** Only a handful of opcodes decode today, so the
//! kernels here are short loops over that handful. That overstates throughput
//! in three ways, all of which get worse as the real instruction set lands:
//!
//! - the whole working set sits in the decoded-instruction cache, so the
//!   dispatch never pays a decode after the first pass;
//! - eight distinct opcodes keep the dispatch's indirect branch perfectly
//!   predicted, which a realistic mix will not;
//! - there is no FP, no NEON, and no unaligned or faulting access.
//!
//! Treat it as a ceiling on the dispatch design, not as a measurement of the
//! finished interpreter.
//!
//! Native-only: it measures wall-clock time and prints a report, neither of
//! which `wasm32-unknown-unknown` has.

#![cfg(not(target_arch = "wasm32"))]

mod kernels;

use std::hint::black_box;
use std::time::{Duration, Instant};

use coracle_core::guest_memory::PHYS_RAM_BASE;
use coracle_core::interp::{Cpu, FlatMemory, Memory, Stop};
use coracle_core::reg::Gpr;
use kernels::Kernel;

/// Guest RAM the benchmark runs in. Large enough for the kernels' working set
/// and small enough to stay in the host's cache, which keeps the measurement
/// about the interpreter rather than about the host's memory system.
const RAM_BYTES: usize = 1 << 16;

/// Instructions each measured run retires.
///
/// Large enough that timer resolution and the run-up cost nothing, small enough
/// that the whole suite finishes in a few seconds.
const MEASURED_INSNS: u64 = 20_000_000;

/// Passes before measurement, so the decoded-instruction cache is warm and the
/// host has settled on a branch prediction.
const WARMUP_INSNS: u64 = 2_000_000;

fn main() {
    println!("interpreter throughput — M1 gate: >= 40 guest MIPS, <= 60x native\n");

    let coremark = measure_guest(&kernels::COREMARK_LIKE);
    let coremark_native = measure_coremark_native();
    report(&kernels::COREMARK_LIKE, coremark, coremark_native);

    let dhrystone = measure_guest(&kernels::DHRYSTONE_LIKE);
    let dhrystone_native = measure_dhrystone_native();
    report(&kernels::DHRYSTONE_LIKE, dhrystone, dhrystone_native);

    println!(
        "\nCEILING, not a measurement: only {} opcodes decode today, the whole\n\
         working set fits the decode cache, and there is no FP, NEON or fault\n\
         traffic. A realistic instruction mix will be materially slower.",
        8
    );
}

/// Wall-clock time to retire `MEASURED_INSNS` guest instructions.
fn measure_guest(kernel: &Kernel) -> Duration {
    let mut cpu = load(kernel);
    cpu.run(WARMUP_INSNS);

    fastest(|| {
        let started = Instant::now();
        let stop = cpu.run(MEASURED_INSNS);
        let elapsed = started.elapsed();

        assert_eq!(stop, Stop::BudgetExhausted, "{} trapped", kernel.name);
        black_box(cpu.regs.read_x(Gpr::X(11)));
        elapsed
    })
}

/// Times measured before taking the fastest.
///
/// The fastest rather than the mean: every source of noise on a laptop —
/// scheduling, thermal throttling, another core waking — makes a run slower and
/// none makes it faster, so the minimum is the closest estimate of the
/// interpreter's own cost.
const REPEATS: usize = 5;

/// Runs `measure` [`REPEATS`] times and returns its shortest time.
fn fastest(mut measure: impl FnMut() -> Duration) -> Duration {
    (0..REPEATS).map(|_| measure()).min().expect("REPEATS > 0")
}

/// A CPU with `kernel` loaded at the base of RAM and its registers seeded.
fn load(kernel: &Kernel) -> Cpu<FlatMemory> {
    let mut memory = FlatMemory::new(RAM_BYTES);
    for (index, word) in kernel.code.iter().enumerate() {
        memory
            .write_uint(PHYS_RAM_BASE + index as u64 * 4, 4, *word as u64)
            .expect("kernel fits in RAM");
    }

    let mut cpu = Cpu::new(memory);
    cpu.regs.set_pc(PHYS_RAM_BASE);
    // Non-zero seeds, so the arithmetic is not trivially constant.
    cpu.regs.write_x(Gpr::X(1), u64::MAX);
    cpu.regs.write_x(Gpr::X(3), 3);
    cpu.regs.write_x(Gpr::X(5), 5);
    cpu.regs.write_x(Gpr::X(7), 7);
    // The data window the memory kernel addresses, clear of the code.
    cpu.regs.write_x(Gpr::X(9), PHYS_RAM_BASE + 0x100);
    cpu
}

/// Native time for the same instruction count of CoreMark-shaped work.
fn measure_coremark_native() -> Duration {
    let iterations = MEASURED_INSNS / kernels::COREMARK_LIKE.insns_per_iteration;
    kernels::coremark_native(WARMUP_INSNS / 8);

    fastest(|| {
        let started = Instant::now();
        let result = kernels::coremark_native(iterations);
        let elapsed = started.elapsed();

        black_box(result);
        elapsed
    })
}

/// Native time for the same instruction count of Dhrystone-shaped work.
fn measure_dhrystone_native() -> Duration {
    let iterations = MEASURED_INSNS / kernels::DHRYSTONE_LIKE.insns_per_iteration;
    let mut memory = [0u64; 2];
    kernels::dhrystone_native(WARMUP_INSNS / 8, &mut memory);

    fastest(|| {
        let started = Instant::now();
        let result = kernels::dhrystone_native(iterations, &mut memory);
        let elapsed = started.elapsed();

        black_box(result);
        black_box(memory);
        elapsed
    })
}

/// Prints one kernel's throughput, ratio, and whether it clears the gate.
fn report(kernel: &Kernel, guest: Duration, native: Duration) {
    let mips = MEASURED_INSNS as f64 / guest.as_secs_f64() / 1e6;
    let ratio = guest.as_secs_f64() / native.as_secs_f64();

    println!("{}", kernel.name);
    println!(
        "  guest      {:>10.1} MIPS  ({:.3} s)",
        mips,
        guest.as_secs_f64()
    );
    println!(
        "  native     {:>10.1} MIPS  ({:.3} s)",
        MEASURED_INSNS as f64 / native.as_secs_f64() / 1e6,
        native.as_secs_f64()
    );
    println!(
        "  ratio      {:>10.1}x native   [{}]",
        ratio,
        verdict(ratio <= 60.0)
    );
    println!(
        "  absolute   {:>10.1} MIPS     [{}]",
        mips,
        verdict(mips >= 40.0)
    );

    // A host that retires the loop at more than one instruction per cycle is
    // not a fair stand-in for the real benchmark: an eight-instruction kernel
    // over a two-word array has no cache misses, no branch misprediction and
    // perfect store forwarding, none of which Dhrystone proper enjoys. Say so
    // rather than let the ratio read as an interpreter result.
    let native_ipc = MEASURED_INSNS as f64 / native.as_secs_f64() / 3.5e9;
    if native_ipc > 1.5 {
        println!(
            "  note       native baseline retires ~{native_ipc:.1} IPC — the kernel is\n\
             \x20            too small to stand in for the real benchmark, so this\n\
             \x20            ratio understates the interpreter. Trust the absolute."
        );
    }
}

const fn verdict(passes: bool) -> &'static str {
    if passes {
        "pass"
    } else {
        "FAIL"
    }
}
