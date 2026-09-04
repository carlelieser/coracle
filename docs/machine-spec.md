# Machine Specification

The authoritative description of the virtual machine the emulator implements.
Every crate codes against this document. Where an implementation and this
document disagree, this document is the defect report.

Status: **draft**. Address tables are pending reconciliation with the M0 kernel
lane (see §3). Everything else is decided.

## 1. Scope

A clone of QEMU's `virt` machine, narrowed to what a mainline arm64 kernel
requires to boot unpatched.

| Property | Value |
|---|---|
| Architecture | ARMv8.0-A, AArch64 only |
| Exception levels | EL0, EL1 only |
| CPUs | 1 (single vCPU; SMP is a v1 non-goal) |
| Endianness | Little-endian only |
| Interrupt controller | GICv2 |
| Timer | ARM generic timer (virtual and physical) |
| Console | PL011 UART |
| RTC | PL031 |
| Device transport | virtio-mmio |
| Firmware interface | PSCI subset over trapped `SMC` |
| Boot protocol | Linux arm64 boot protocol; DTB pointer in x0 |

AArch32 is not implemented at any exception level. EL2 and EL3 do not exist:
`ID_AA64PFR0_EL1.EL2` and `.EL3` read as unimplemented, and the kernel must
never observe an EL2-capable machine.

## 2. Feature mask

The emulator advertises ARMv8.0-A baseline and nothing beyond it. These features
are **not** implemented and must read as absent in their `ID_AA64*` fields:

| Feature | ID register field |
|---|---|
| LSE atomics | `ID_AA64ISAR0_EL1.Atomic` |
| SVE | `ID_AA64PFR0_EL1.SVE` |
| Pointer authentication | `ID_AA64ISAR1_EL1.{APA,API,GPA,GPI}` |
| MTE | `ID_AA64PFR1_EL1.MTE` |
| Crypto (AES/SHA/PMULL) | `ID_AA64ISAR0_EL1.{AES,SHA1,SHA2,SHA3,SM3,SM4}` |
| BTI | `ID_AA64PFR1_EL1.BT` |

Rationale: a small ISA surface, and libc/JIT feature detection picks portable
code paths. Two consequences bind other work:

- The QEMU oracle must be launched with a `-cpu` mask matching this table
  exactly. An oracle advertising features we do not implement makes every
  differential run noise.
- The guest kernel must be configured not to require or opportunistically use
  these. A kernel built expecting LSE atomics will not run here.

An instruction encoding belonging to an unadvertised feature raises the same
exception the architecture requires of an unimplemented encoding, and must do so
identically to the oracle. Faulting *identically* is a gate condition, not a
best effort.

### The oracle advertises crypto; we do not

QEMU 11.1.1 exposes no property to disable FEAT_AES/SHA on any AArch64 model —
crypto is baked into every model's ID registers, `max` included. The pinned
oracle therefore advertises AES and SHA that this machine does not implement.

This is benign only because guests select code paths by reading
`ID_AA64ISAR0_EL1`, which the emulator controls, so no guest takes a crypto path
during a differential run. It does make one thing load-bearing that would
otherwise be incidental: **the M1 fuzz corpus must not draw crypto encodings.**
Drawing them would compare against an oracle that executes what we fault on.

`tests/verify_feature_mask.sh` asserts the deviation rather than tolerating it,
so the gap cannot widen unnoticed if a future QEMU changes its defaults.

## 3. Memory map, IRQ map, MMIO

Derived from QEMU `virt`'s own generated device tree (`-machine dumpdtb`), not
invented, so the stock kernel binds its drivers without patches and QEMU stays
usable as the differential oracle. Verified against the built DTB at
`kernel/out/coracle-virt.dtb`; `kernel/scripts/dump-reference-dtb.sh`
regenerates the reference and diffs it.

| Region | Base | Size | SPI | GIC INTID |
|--------|------|------|-----|-----------|
| GIC distributor | `0x0800_0000` | 64 KB | — | — |
| GIC CPU interface | `0x0801_0000` | 64 KB | — | — |
| PL011 UART (`ttyAMA0`) | `0x0900_0000` | 4 KB | 1 | 33 |
| PL031 RTC | `0x0901_0000` | 4 KB | 2 | 34 |
| virtio-mmio slots 0–7 | `0x0a00_0000` | 512 B each | 16–23 | 48–55 |
| Guest RAM | `0x4000_0000` | 1 GB | — | — |

SPI *n* is GIC INTID *n* + 32. virtio-mmio slots are strided by `0x200`, so
slot *n* is at `0x0a00_0000 + n * 0x200` with SPI *16 + n*. QEMU's `virt`
provides 32 slots; we declare 8 — enough for blk, net, 9p, rng and console with
headroom, without 32 probe cycles at boot.

Timer PPIs: 13 secure physical, 14 non-secure physical, 11 virtual, 10
hypervisor — GIC INTIDs 29, 30, 27, 26. The counter runs at 62.5 MHz.

Consumers: the `machine` crate (device placement), the `devices` crate (MMIO
decode), the DTS, and the emulator's DTB generator.

## 4. Guest RAM

Guest RAM is a fixed-offset region inside the wasm module's exported linear
memory. The module is built with atomics and its memory is shared, so the CPU
worker and the device threads address the same bytes.

| Property | Value |
|---|---|
| Default size | 1 GB |
| Maximum size | 2 GB |
| Configurable | Yes, at machine construction |

Full-system mode means the MMU translates guest virtual addresses to physical
offsets within this region, so no 64-bit pointer masking is required on the
memory access path.

The 2 GB ceiling leaves tab-budget headroom for the layer cache, JIT modules,
and the JS heap. Two later consumers depend on this layout: M5's JIT modules
import this same memory rather than copying, and M5's snapshots serialize dirty
pages out of it.

## 5. Firmware: PSCI

There is no EL3 and no secure world. `SMC` and `HVC` are trapped and serviced by
the emulator directly. PSCI is declared in the DTB via a `psci` node with the
`smc` conduit, which is what the kernel on `virt` expects for boot, reboot, and
poweroff.

Implemented calls:

| Call | Behavior |
|---|---|
| `PSCI_VERSION` | Returns the implemented version |
| `SYSTEM_OFF` | Clean machine-loop exit; the machine is terminated |
| `SYSTEM_RESET` | Clean machine-loop exit and restart |
| `CPU_SUSPEND` | Idles until the next timer interrupt or IRQ (WFI semantics) |

Any other PSCI function returns `NOT_SUPPORTED`. Because the machine is single
vCPU, `CPU_ON` and the rest of the multiprocessor calls are deliberately absent.

`SYSTEM_OFF` and `SYSTEM_RESET` must unwind the machine loop cleanly enough that
the host observes a normal termination, not a trap or a hang — `poweroff` and
`reboot` working from a guest shell is an M2 gate item.

## 6. Floating point

Two backends sit behind one trait, selected on a cached FPCR flag:

| FPCR state | Backend |
|---|---|
| Default mode (round-to-nearest, no flush-to-zero, untrapped) | Native wasm FP ops |
| Any non-default mode | Softfloat |

wasm exposes no rounding-mode control and no FP exception flags, so native ops
are correct only while FPCR is in its default mode — which covers effectively
all userspace. FPSR cumulative exception flags are maintained **only** on the
softfloat path; this is a known, documented divergence, not a defect.

A build-time **precise mode** routes everything through softfloat. All
differential FP legs run in precise mode. The native backend is validated
separately under a NaN-payload-insensitive comparison policy, because wasm does
not guarantee NaN payload propagation.

The differ therefore cannot assume bitwise equality on FP state; the comparison
policy is per-leg.

## 7. Execution model

A basic-block interpreter with a decoded-instruction cache, structured so the
M5 translator slots in without a rewrite. The CPU runs in a Web Worker; devices
communicate over SharedArrayBuffer, which requires the embedding page to be
cross-origin isolated (COOP/COEP).

A single-threaded degraded mode is a first-class, feature-detected build kept
green in CI, for browsers where SharedArrayBuffer is unavailable or quirky.

### Page tracking

The MMU maintains **dirty** and **executed** page bitmaps from M2 onward. Two
later consumers depend on them, and retrofitting either is expensive:

- Snapshots serialize only dirty RAM pages.
- The JIT evicts translations using the executed-page bitmap plus
  write-protection traps, which is what makes a guest JIT (V8 writing its own
  code) safe.

## 8. Root filesystem

Validated under pure QEMU by the M0 spike (`spike/rootfs-overlay/`), which
settles the kernel-side question only: it uses QEMU's 9p server, not ours.

The spike ran kernel 6.12.107 and QEMU 10.0.11 in a container, while the
project pins 6.12.108 and QEMU 11.1.1 (`kernel/versions.env`). Both gaps are
deliberate. The spike containerises QEMU because a macOS bind mount reports
`chown` success without applying it and rejects xattrs, which would fail the
uid/gid and xattr cases for a host reason and read as an overlayfs limitation.
The kernel patch gap is inert: the 6.12.108 changelog touches neither 9p nor
overlayfs, so the findings below carry unchanged.

The guest mounts `overlayfs(lowerdir=9p merged layers, upperdir=ext4 on
virtio-blk)` and pivots to it. The kernel owns copy-up, whiteouts, and rename.

### Constraints our 9p server must satisfy

`spike/rootfs-overlay/9p-checklist.md` is the full list, consumed by the M3
gates. Two items are easy to implement wrongly and invisible until they corrupt
something:

- **`qid.path` is guest inode identity.** It is the key into `iget5_locked` via
  `QID2INO`, so it governs hardlink detection outright. An id derived from the
  path breaks under rename and collapses distinct files into one inode. Stable,
  unique, rename-invariant.
- **`d_type` degrades silently.** Overlayfs checks it on the workdir, not a
  lower, and only warns. A lower reporting `DT_UNKNOWN` does not fail the mount
  — whiteouts simply stop working, because whiteout candidates are collected by
  testing `d_type == DT_CHR`. M3 must assert readdir types directly; no mount
  failure will catch this.

### Permanent limitation: lower-layer hardlinks

Hardlinks that exist in a lower layer are severed by copy-up: the copied-up file
becomes an independent inode. Rejoining them requires overlayfs `index=on`,
which requires the lower to encode file handles, and `fs/9p` defines no
`export_operations` (verified against upstream v6.12). So `index=off` and
`xino=off` are forced for any 9p lower.

This is a property of the kernel's 9p **client**, not of QEMU's server, so our
own server cannot fix it, and the JS-side union fallback would have to solve it
independently. Docker's `overlay2` severs lower hardlinks the same way, so
published images tolerate it. M3 and M4 note the behavior; neither works around
it.

## 9. Non-goals

Not implemented in v1, and changes require written justification: AArch32,
big-endian, GPU or framebuffer, USB, multi-core SMP, Windows or macOS guests,
x86 images.
