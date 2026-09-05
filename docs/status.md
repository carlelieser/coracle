# Status

Current state of the implementation against `plan.md`. `plan.md` states the
milestones and their gates; this file states what is built, what is deferred,
and which gates are genuinely met.

Updated at the M1 Phase B merge.

## M1 — CPU core, user-mode harness

In progress. Decode coverage leads execution coverage: the decoder resolves
roughly 31% of the 32-bit encoding space, and 8 opcodes execute.

### Built

- Two-axis decode model (`Op` × `Form`) covering data processing (immediate,
  register, 3-source), loads and stores, branches, system instructions,
  conditional select and compare, and bit manipulation.
- Scalar FP and the NEON subset, with native and softfloat backends behind one
  trait. FPSR cumulative flags on the softfloat path only.
- Syscall shim, ~45 calls behind a `HostIo` trait. `clone` and `futex` are
  matched and refused with `ENOSYS`.
- CDT v1 trace writer and register-delta encoder.
- Interpreter dispatch for `ADD`, `SUB`, the logical group, `B`/`BL`,
  `BR`/`BLR`/`RET`, `NOP`, `LDR`/`STR`, `SVC`, `BRK`.

### Gates

| Gate                                                  | State                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                     |
| ----------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Differential match against QEMU, 10,000-binary corpus | Not started. Requires execution coverage first: most of a random corpus traps as unimplemented, so there is no state to compare.                                                                                                                                                                                                                                                                                                                                                                          |
| Static `busybox` under the shim                       | Not started. Same dependency.                                                                                                                                                                                                                                                                                                                                                                                                                                                                             |
| Exclusive-monitor and TLS corpus                      | Not started.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                              |
| ≤ 60× native, ≥ 40 guest MIPS                         | **Not met.** The benchmark reports 140.4 and 121.9 guest MIPS, but on a synthetic kernel exercising the 8 implemented opcodes with a working set inside the decode cache and no FP, NEON, or fault traffic. The benchmark prints this caveat in its own output. The ratio reads 105× on the memory kernel against a native baseline of roughly 3.7 IPC on a two-word array, which is not a realistic comparison. Both numbers are re-measured against a real instruction mix before this gate is claimed. |

### Deferred

**Trace deltas are not emitted from the interpreter.** `interp/mod.rs` calls
`on_block` with an empty delta slice. `trace::writer::deltas_between` is
complete and correct, and the CDT tests drive it directly, but nothing wires it
into the run loop, so a trace captured from an actual run carries no register
state. Closing this needs a previous-`RegFile` snapshot owned by `Cpu`. This is
a prerequisite for debugging any differential failure.

**Guest memory has no linear-window backing.** `guest_memory.rs` computes the
layout — `reserve` and `linear_offset` — but owns no storage and defines no
access trait. The interpreter's `Memory` trait is the only access path, and its
implementation is backed by a `Vec`. M2's MMU is what determines the shape this
should take, so it is settled there rather than guessed at now.

**FP correctness is unvalidated in four places.** NaN payload propagation
through `FCVT` is implemented from the specification and never differentially
tested; `FpRounding::Odd` is a stub; subnormal-boundary rounding under directed
modes is thinly covered; FPSR `IDC` is unverified against QEMU. All four are
closed by the differential corpus.

## M0 — Foundation

Gates are unticked in `plan.md`. CI is green on the native and wasm targets;
the remaining three gates — the harness reporting identical state on a
10-instruction ELF, the kernel and initramfs booting under QEMU `virt`, and the
overlayfs-over-9p rootfs spike — are not recorded as verified here.
