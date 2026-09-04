//! `Shim::run` — the loop that services supervisor calls and resumes.
//!
//! The guest programs here trap rather than issue `SVC`, because the `SVC`
//! encoding is not decoded on this branch yet: the branch-and-system decode
//! slice owns it. What that leaves testable is everything except the
//! `Op::Svc` dispatch arm itself — the budget arithmetic across resumptions,
//! the trap pass-through, and the exit path — which is where the loop's
//! actual complexity is.
//!
//! When `SVC` lands, the missing case is one test: assemble `mov x8, #93;
//! svc #0` and assert `run` reports `Exited`.

#![cfg(not(target_arch = "wasm32"))]

use coracle_core::guest_memory::PHYS_RAM_BASE;
use coracle_core::interp::{Cpu, FlatMemory, Memory};
use coracle_core::reg::Gpr;
use coracle_core::shim::{default_layout, CapturingHost, Shim, ShimExit};
use coracle_core::trap::Trap;

const RAM_BYTES: u64 = 1 << 16;

fn guest(program: &[u32]) -> (Cpu<FlatMemory>, Shim<CapturingHost>) {
    let mut memory = FlatMemory::new(RAM_BYTES as usize);
    for (index, word) in program.iter().enumerate() {
        memory
            .write_uint(PHYS_RAM_BASE + index as u64 * 4, 4, *word as u64)
            .expect("program fits");
    }
    let mut cpu = Cpu::new(memory);
    cpu.regs.set_pc(PHYS_RAM_BASE);

    let shim = Shim::new(
        CapturingHost::new(),
        default_layout(PHYS_RAM_BASE, RAM_BYTES),
    );
    (cpu, shim)
}

#[test]
fn a_guest_that_never_exits_gives_control_back_when_its_budget_runs_out() {
    // b .  — an infinite loop.
    let (mut cpu, mut shim) = guest(&[0x1400_0000]);

    assert_eq!(shim.run(&mut cpu, 5_000), ShimExit::BudgetExhausted);
    assert_eq!(cpu.icount(), 5_000);
}

#[test]
fn the_budget_is_counted_from_where_this_call_started_not_from_zero() {
    // A run after an earlier one must get its full budget. Subtracting the
    // CPU's lifetime icount instead collapses the second budget to nothing.
    let (mut cpu, mut shim) = guest(&[0x1400_0000]);

    shim.run(&mut cpu, 1_000);
    assert_eq!(cpu.icount(), 1_000);

    shim.run(&mut cpu, 1_000);

    assert_eq!(cpu.icount(), 2_000, "the second run got its own budget");
}

#[test]
fn a_trap_the_shim_cannot_service_is_reported_rather_than_swallowed() {
    // An unimplemented NEON encoding.
    let (mut cpu, mut shim) = guest(&[0x4e20_8400]);

    let exit = shim.run(&mut cpu, 1_000);

    assert_eq!(
        exit,
        ShimExit::Trapped(Trap::UndefinedInstruction {
            pc: PHYS_RAM_BASE,
            encoding: 0x4e20_8400,
        })
    );
}

#[test]
fn a_fetch_outside_guest_ram_stops_the_run_as_an_instruction_abort() {
    let (mut cpu, mut shim) = guest(&[]);
    cpu.regs.set_pc(0);

    assert_eq!(
        shim.run(&mut cpu, 1_000),
        ShimExit::Trapped(Trap::InstructionAbort { pc: 0 })
    );
}

#[test]
fn servicing_a_call_leaves_the_guest_ready_to_resume_after_it() {
    // `service` is what the loop calls once SVC decodes. The guest must find
    // its result in x0 and nothing else disturbed.
    let (mut cpu, mut shim) = guest(&[]);
    cpu.regs
        .write_x(Gpr::X(8), coracle_core::shim::number::GETPID);
    cpu.regs.write_x(Gpr::X(5), 0xabcd);

    let exited = shim.service(&mut cpu);

    assert_eq!(exited, None, "getpid does not terminate the guest");
    assert_eq!(cpu.regs.read_x(Gpr::X(0)), 1);
    assert_eq!(cpu.regs.read_x(Gpr::X(5)), 0xabcd, "other registers intact");
}

#[test]
fn the_default_layout_hands_out_addresses_inside_guest_ram() {
    let layout = default_layout(PHYS_RAM_BASE, RAM_BYTES);

    assert!(layout.heap_base >= PHYS_RAM_BASE);
    assert!(layout.heap_limit <= layout.mmap_base);
    assert!(layout.mmap_limit <= PHYS_RAM_BASE + RAM_BYTES);
}
