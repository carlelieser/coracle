//! The syscall shim, driven the way the guest drives it.
//!
//! Each test places arguments in the registers the AArch64 ABI names, services
//! one call, and asserts on what the guest would observe: `x0`, guest memory,
//! and what reached the host.
//!
//! `service` is called directly rather than through an `SVC` instruction
//! because the shim must be provable independently of which encodings the
//! decode slices have landed.

use coracle_core::guest_memory::PHYS_RAM_BASE;
use coracle_core::interp::{Cpu, FlatMemory, Memory};
use coracle_core::reg::Gpr;
use coracle_core::shim::{number as sys, ArenaLayout, CapturingHost, Errno, Shim};

const RAM_BYTES: usize = 1 << 20;

/// The window the guest's data lives in, below the shim's arenas.
const SCRATCH: u64 = PHYS_RAM_BASE + 0x1000;

struct Guest {
    cpu: Cpu<FlatMemory>,
    shim: Shim<CapturingHost>,
}

impl Guest {
    fn new() -> Self {
        let layout = ArenaLayout {
            heap_base: PHYS_RAM_BASE + (RAM_BYTES / 4) as u64,
            heap_limit: PHYS_RAM_BASE + (RAM_BYTES / 2) as u64,
            mmap_base: PHYS_RAM_BASE + (RAM_BYTES / 2) as u64,
            mmap_limit: PHYS_RAM_BASE + RAM_BYTES as u64,
        };
        Self {
            cpu: Cpu::new(FlatMemory::new(RAM_BYTES)),
            shim: Shim::new(CapturingHost::new(), layout),
        }
    }

    /// Issues one syscall and returns what the guest finds in `x0`.
    fn call(&mut self, number: u64, args: &[u64]) -> u64 {
        self.cpu.regs.write_x(Gpr::X(8), number);
        for (index, value) in args.iter().enumerate() {
            self.cpu.regs.write_x(Gpr::X(index as u8), *value);
        }
        self.shim.service(&mut self.cpu);
        self.cpu.regs.read_x(Gpr::X(0))
    }

    /// Issues a call the guest expects to terminate it.
    fn call_expecting_exit(&mut self, number: u64, status: u64) -> Option<i32> {
        self.cpu.regs.write_x(Gpr::X(8), number);
        self.cpu.regs.write_x(Gpr::X(0), status);
        self.shim.service(&mut self.cpu)
    }

    fn store(&mut self, address: u64, bytes: &[u8]) {
        self.cpu.memory.write(address, bytes).expect("in range");
    }
}

#[test]
fn write_delivers_the_guests_bytes_to_the_host_and_reports_the_count() {
    let mut guest = Guest::new();
    guest.store(SCRATCH, b"hello\n");

    let written = guest.call(sys::WRITE, &[1, SCRATCH, 6]);

    assert_eq!(written, 6);
    assert_eq!(guest.shim.host.stdout_text(), "hello\n");
}

#[test]
fn writev_concatenates_its_vectors_in_order() {
    // musl's stdio writes through writev, not write, so this is the path a
    // real `printf` takes.
    let mut guest = Guest::new();
    guest.store(SCRATCH, b"hi");
    guest.store(SCRATCH + 16, b" there");

    let array = SCRATCH + 64;
    for (index, (base, len)) in [(SCRATCH, 2u64), (SCRATCH + 16, 6)].iter().enumerate() {
        let entry = array + index as u64 * 16;
        guest.store(entry, &base.to_le_bytes());
        guest.store(entry + 8, &len.to_le_bytes());
    }

    let written = guest.call(sys::WRITEV, &[1, array, 2]);

    assert_eq!(written, 8);
    assert_eq!(guest.shim.host.stdout_text(), "hi there");
}

#[test]
fn writing_to_a_descriptor_the_host_refuses_returns_negative_errno() {
    let mut guest = Guest::new();
    guest.store(SCRATCH, b"x");

    let result = guest.call(sys::WRITE, &[9, SCRATCH, 1]);

    assert_eq!(result, Errno::BadFd.to_return_value());
}

#[test]
fn writing_from_an_unmapped_address_faults_without_reaching_the_host() {
    let mut guest = Guest::new();

    let result = guest.call(sys::WRITE, &[1, 0, 8]);

    assert_eq!(result, Errno::Fault.to_return_value());
    assert!(guest.shim.host.stdout.is_empty());
}

#[test]
fn reading_stdin_reports_end_of_file() {
    let mut guest = Guest::new();

    assert_eq!(guest.call(sys::READ, &[0, SCRATCH, 16]), 0);
}

#[test]
fn exit_and_exit_group_both_stop_the_guest_with_its_status() {
    for number in [sys::EXIT, sys::EXIT_GROUP] {
        let mut guest = Guest::new();

        let status = guest.call_expecting_exit(number, 3);

        assert_eq!(status, Some(3));
        assert_eq!(guest.shim.host.exit_status, Some(3));
    }
}

#[test]
fn brk_reports_the_current_break_when_asked_for_an_impossible_one() {
    let mut guest = Guest::new();

    let initial = guest.call(sys::BRK, &[0]);
    let grown = guest.call(sys::BRK, &[initial + 0x1000]);
    let refused = guest.call(sys::BRK, &[u64::MAX]);

    assert_eq!(grown, initial + 0x1000);
    assert_eq!(refused, grown, "a refusal reports the break, not an error");
}

#[test]
fn mmap_returns_distinct_page_aligned_addresses_inside_guest_ram() {
    let mut guest = Guest::new();

    let first = guest.call(sys::MMAP, &[0, 4096, 3, 0x22, u64::MAX, 0]);
    let second = guest.call(sys::MMAP, &[0, 4096, 3, 0x22, u64::MAX, 0]);

    assert_ne!(first, second);
    assert_eq!(first % 4096, 0);
    // The address the guest gets back must be one it can actually write to.
    guest.store(first, b"usable");
    assert_eq!(guest.cpu.memory.slice(first, 6), Ok(&b"usable"[..]));
}

#[test]
fn munmap_succeeds_so_a_guest_that_frees_its_mappings_keeps_running() {
    let mut guest = Guest::new();
    let mapping = guest.call(sys::MMAP, &[0, 4096, 3, 0x22, u64::MAX, 0]);

    assert_eq!(guest.call(sys::MUNMAP, &[mapping, 4096]), 0);
}

#[test]
fn uname_reports_an_aarch64_linux_machine() {
    let mut guest = Guest::new();

    assert_eq!(guest.call(sys::UNAME, &[SCRATCH]), 0);

    let sysname = guest.cpu.memory.slice(SCRATCH, 5).expect("in range");
    let machine = guest
        .cpu
        .memory
        .slice(SCRATCH + 4 * 65, 7)
        .expect("in range");
    assert_eq!(sysname, b"Linux");
    assert_eq!(machine, b"aarch64");
}

#[test]
fn fstat_of_stdout_tells_libc_it_is_a_terminal() {
    let mut guest = Guest::new();

    assert_eq!(guest.call(sys::FSTAT, &[1, SCRATCH]), 0);

    let mode = guest
        .cpu
        .memory
        .read_uint(SCRATCH + 16, 4)
        .expect("in range");
    assert_eq!(mode & 0o170_000, 0o020_000, "S_IFCHR");
}

#[test]
fn getrandom_fills_the_guests_buffer() {
    let mut guest = Guest::new();

    let count = guest.call(sys::GETRANDOM, &[SCRATCH, 32, 0]);

    assert_eq!(count, 32);
    let bytes = guest.cpu.memory.slice(SCRATCH, 32).expect("in range");
    assert!(bytes.iter().any(|&byte| byte != 0));
}

#[test]
fn clock_gettime_writes_a_timespec_that_advances() {
    let mut guest = Guest::new();

    guest.call(sys::CLOCK_GETTIME, &[0, SCRATCH]);
    let first = guest
        .cpu
        .memory
        .read_uint(SCRATCH + 8, 8)
        .expect("in range");
    guest.call(sys::CLOCK_GETTIME, &[0, SCRATCH + 16]);
    let second = guest
        .cpu
        .memory
        .read_uint(SCRATCH + 24, 8)
        .expect("in range");

    assert!(second > first);
}

#[test]
fn the_startup_calls_musl_makes_all_succeed() {
    // Every one of these is issued by a static musl binary before `main`, and
    // any of them returning ENOSYS aborts startup.
    let mut guest = Guest::new();

    for number in [
        sys::SET_TID_ADDRESS,
        sys::SET_ROBUST_LIST,
        sys::RSEQ,
        sys::PRLIMIT64,
        sys::RT_SIGACTION,
        sys::RT_SIGPROCMASK,
    ] {
        let result = guest.call(number, &[0, 0, 0, 0, 0, 0]) as i64;
        assert!(result >= 0, "syscall {number} failed with {result}");
    }
}

#[test]
fn the_identity_calls_report_root_of_a_single_process() {
    let mut guest = Guest::new();

    assert_eq!(guest.call(sys::GETUID, &[]), 0);
    assert_eq!(guest.call(sys::GETEUID, &[]), 0);
    assert_eq!(guest.call(sys::GETGID, &[]), 0);
    assert_eq!(guest.call(sys::GETPID, &[]), 1);
}

#[test]
fn sched_getaffinity_reports_the_single_vcpu() {
    let mut guest = Guest::new();

    let size = guest.call(sys::SCHED_GETAFFINITY, &[0, 8, SCRATCH]);

    assert_eq!(size, 8);
    assert_eq!(guest.cpu.memory.read_uint(SCRATCH, 8), Ok(1));
}

#[test]
fn clone_and_futex_are_refused_rather_than_silently_missing() {
    // docs/plan.md puts both out of shim scope. They must fail as unimplemented
    // syscalls, and must not appear in the unhandled log as a coverage gap.
    let mut guest = Guest::new();

    assert_eq!(
        guest.call(sys::CLONE, &[0; 6]),
        Errno::NoSys.to_return_value()
    );
    assert_eq!(
        guest.call(sys::FUTEX, &[0; 6]),
        Errno::NoSys.to_return_value()
    );
    assert_eq!(guest.shim.unhandled().count(), 0);
}

#[test]
fn an_unknown_syscall_returns_enosys_and_is_recorded_once() {
    let mut guest = Guest::new();

    for _ in 0..10 {
        assert_eq!(guest.call(4242, &[0; 6]), Errno::NoSys.to_return_value());
    }

    let unhandled: Vec<u64> = guest.shim.unhandled().collect();
    assert_eq!(unhandled, vec![4242]);
}

#[test]
fn opening_a_file_the_host_has_no_filesystem_for_reports_no_such_file() {
    let mut guest = Guest::new();
    guest.store(SCRATCH, b"/etc/passwd\0");

    let result = guest.call(sys::OPENAT, &[0, SCRATCH, 0, 0]);

    assert_eq!(result, Errno::NoEnt.to_return_value());
}
