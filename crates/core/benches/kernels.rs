//! The guest programs the benchmarks run, and the native code they are
//! measured against.
//!
//! Both kernels are hand-assembled A64. `docs/plan.md` asks for Dhrystone and a
//! CoreMark-like loop; what is here is the *shape* of each — Dhrystone's
//! pointer-chasing and field copying, CoreMark's arithmetic mix — restricted to
//! the opcodes the decode slices have landed. It is deliberately not the real
//! benchmark: the real one needs the loads, stores and control flow that arrive
//! with the rest of phase B.
//!
//! The native counterpart of each kernel does the same arithmetic on the same
//! values, so the ratio between them is the plan's "×  slower than native".

/// One guest program: its instruction words and what one iteration costs.
#[derive(Debug)]
pub struct Kernel {
    /// The name the report prints.
    pub name: &'static str,
    /// Instruction words, executed from the first.
    pub code: &'static [u32],
    /// Instructions retired per pass through the loop body.
    pub insns_per_iteration: u64,
}

/// A CoreMark-shaped integer loop: dependent arithmetic over eight registers.
///
/// Every instruction's result feeds the next, which is what a real integer
/// benchmark's inner loop looks like and what defeats a superscalar host's
/// ability to hide the interpreter's overhead.
///
/// Assembled from:
/// ```text
/// loop:  sub x1, x1, #1      ; d1000421
///        add x2, x2, x3      ; 8b030042
///        eor x4, x4, x5      ; ca050084
///        orr x6, x6, x7      ; aa0700c6
///        and x10, x2, x3     ; 8a03004a
///        add x11, x11, x2    ; 8b02016b
///        sub x12, x12, x1    ; cb01018c
///        b loop              ; 17fffff9
/// ```
pub const COREMARK_LIKE: Kernel = Kernel {
    name: "coremark-like (integer ALU)",
    code: &[
        0xd100_0421,
        0x8b03_0042,
        0xca05_0084,
        0xaa07_00c6,
        0x8a03_004a,
        0x8b02_016b,
        0xcb01_018c,
        0x17ff_fff9,
    ],
    insns_per_iteration: 8,
};

/// A Dhrystone-shaped loop: memory traffic mixed with arithmetic.
///
/// Dhrystone spends its time copying structure fields and chasing pointers, so
/// this alternates loads and stores against the same cache line with the
/// address arithmetic around them. The load-to-use dependency is the point:
/// it is what makes the interpreter's bounds check land on the critical path.
///
/// Assembled from:
/// ```text
/// loop:  str x2, [x9]        ; f9000122
///        ldr x8, [x9]        ; f9400128
///        add x2, x2, x3      ; 8b030042
///        str x8, [x9, #8]    ; f9000528
///        ldr x10, [x9, #8]   ; f9400528  (ldr x8 -> x10: f940052a)
///        add x11, x11, x10   ; 8b0a016b
///        sub x1, x1, #1      ; d1000421
///        b loop              ; 17fffff9
/// ```
pub const DHRYSTONE_LIKE: Kernel = Kernel {
    name: "dhrystone-like (memory + ALU)",
    code: &[
        0xf900_0122,
        0xf940_0128,
        0x8b03_0042,
        0xf900_0528,
        0xf940_052a,
        0x8b0a_016b,
        0xd100_0421,
        0x17ff_fff9,
    ],
    insns_per_iteration: 8,
};

/// The CoreMark-shaped kernel's arithmetic, in Rust.
///
/// Each accumulator is laundered through `black_box` by value, which keeps the
/// dependency chain intact without forcing the array to memory. Passing the
/// array by reference instead would let the optimiser hoist the whole loop.
pub fn coremark_native(iterations: u64) -> u64 {
    let mut regs: [u64; 13] = [0; 13];
    regs[1] = iterations;
    regs[3] = 3;
    regs[5] = 5;
    regs[7] = 7;

    for _ in 0..iterations {
        regs[1] = core::hint::black_box(regs[1].wrapping_sub(1));
        regs[2] = core::hint::black_box(regs[2].wrapping_add(regs[3]));
        regs[4] = core::hint::black_box(regs[4] ^ regs[5]);
        regs[6] = core::hint::black_box(regs[6] | regs[7]);
        regs[10] = core::hint::black_box(regs[2] & regs[3]);
        regs[11] = core::hint::black_box(regs[11].wrapping_add(regs[2]));
        regs[12] = core::hint::black_box(regs[12].wrapping_sub(regs[1]));
    }
    regs[11]
}

/// The Dhrystone-shaped kernel's arithmetic and memory traffic, in Rust.
///
/// `black_box` is applied to the *address* before each access rather than to
/// the array afterwards. Passing the array by reference still lets the
/// optimiser keep both elements in registers and elide the round trip, which
/// is how a two-element array reaches 15 GIPS and makes the ratio meaningless.
/// Laundering the pointer forces a real store and a real load, which is what
/// the guest kernel is doing.
pub fn dhrystone_native(iterations: u64, memory: &mut [u64; 2]) -> u64 {
    let mut regs: [u64; 12] = [0; 12];
    regs[1] = iterations;
    regs[3] = 3;

    for _ in 0..iterations {
        let slot = core::hint::black_box(memory.as_mut_ptr());
        // SAFETY: `slot` is `memory`'s own pointer, laundered through
        // `black_box`, and both indices are within its two elements.
        unsafe {
            slot.write(regs[2]);
            regs[8] = slot.read();
            regs[2] = regs[2].wrapping_add(regs[3]);
            slot.add(1).write(regs[8]);
            regs[10] = slot.add(1).read();
        }
        regs[11] = regs[11].wrapping_add(regs[10]);
        regs[1] = regs[1].wrapping_sub(1);
    }
    regs[11]
}
