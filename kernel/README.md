# Guest kernel and initramfs

Pinned mainline kernel, busybox initramfs and device tree for the arm64
emulator. Every milestone boots this exact kernel; M5 publishes a pre-booted
snapshot per `KERNEL_VERSION`.

## Build and boot

```sh
./build.sh              # kernel, initramfs and DTB into out/
./scripts/boot.sh       # interactive busybox shell under QEMU virt
./scripts/boot.sh --gate # non-interactive M0 gate checks
./build.sh clean
```

Everything compiles inside a `linux/arm64` container, so macOS and CI produce
identical artifacts. Sources are fetched by pinned version and verified by
SHA-256; no kernel source or build output is committed.

## Pins

| Component | Version | Why |
|-----------|---------|-----|
| Linux | 6.12.108 (LTS) | Supported across the whole milestone sequence, unlike a current stable |
| busybox | 1.37.0 | Static, no libc in the initramfs |
| QEMU | 11.1.1 | Reference oracle. The differential harness pins its own copy; the two must agree |

Change a pin in `versions.env` and nowhere else.

## Machine map

Derived from QEMU `virt` (`-machine dumpdtb`), not invented, so the stock
kernel boots unpatched and QEMU stays usable as the differential oracle.
`scripts/dump-reference-dtb.sh` regenerates the reference and diffs it.

| Region | Base | Size | IRQ (SPI) | GIC IRQ |
|--------|------|------|-----------|---------|
| GIC distributor | `0x0800_0000` | 64 KB | — | — |
| GIC CPU interface | `0x0801_0000` | 64 KB | — | — |
| PL011 UART (`ttyAMA0`) | `0x0900_0000` | 4 KB | 1 | 33 |
| PL031 RTC | `0x0901_0000` | 4 KB | 2 | 34 |
| virtio-mmio slot 0 | `0x0a00_0000` | 512 B | 16 | 48 |
| virtio-mmio slot 1 | `0x0a00_0200` | 512 B | 17 | 49 |
| virtio-mmio slot 2 | `0x0a00_0400` | 512 B | 18 | 50 |
| virtio-mmio slot 3 | `0x0a00_0600` | 512 B | 19 | 51 |
| virtio-mmio slot 4 | `0x0a00_0800` | 512 B | 20 | 52 |
| virtio-mmio slot 5 | `0x0a00_0a00` | 512 B | 21 | 53 |
| virtio-mmio slot 6 | `0x0a00_0c00` | 512 B | 22 | 54 |
| virtio-mmio slot 7 | `0x0a00_0e00` | 512 B | 23 | 55 |
| Guest RAM | `0x4000_0000` | 1 GB | — | — |

SPI *n* is GIC INTID *n* + 32. Timer PPIs are 13 (secure phys), 14 (non-secure
phys), 11 (virt) and 10 (hyp) — GIC INTID 29, 30, 27 and 26.

The counter runs at 62.5 MHz, from QEMU's `virt`.

### PSCI

`psci` node, `method = "smc"`, `compatible = "arm,psci-1.0"`. This is the one
place the DTS deliberately departs from QEMU's dump, which emits `"hvc"`
because its CPU has EL2. We have no EL2 or EL3, so `SMC` is trapped directly.

Only `CPU_SUSPEND` (`0xc4000001`) is declared. `PSCI_VERSION`, `SYSTEM_OFF` and
`SYSTEM_RESET` need no DT property. `CPU_ON`, `CPU_OFF` and `MIGRATE` are
deliberately absent: single vCPU is a v1 non-goal, and advertising a function
the emulator does not implement invites the kernel to call it.

### Devices in QEMU `virt` that this machine drops

`pcie@10000000`, `fw-cfg@9020000`, `platform-bus@c000000`, `flash@0`,
`pl061@9030000` with its `gpio-keys` poweroff button, `v2m@8020000`, the `pmu`
node, and virtio-mmio slots 8..31. The emulator implements none of them and the
kernel is built without the drivers.

## Feature mask

The plan advertises no LSE, SVE, PAuth, MTE, crypto or BTI. `build.sh` fails
the build if any of those reach the final `.config` — a kernel built expecting
LSE atomics does not run on our CPU model.

Two subtleties worth knowing:

- `ARM64_LSE_ATOMICS` has no prompt; it is `default ARM64_USE_LSE_ATOMICS`.
  Disabling the user-visible symbol is what actually turns it off.
- `KERNEL_MODE_NEON` is `def_bool y` on arm64 and cannot be disabled. That is
  fine: base AdvSIMD is part of ARMv8.0-A, which we do advertise. What must
  stay out is `ARM64_CRYPTO`, which needs FEAT_AES/FEAT_SHA.

`boot.sh` runs QEMU with `-cpu cortex-a53`, which is ARMv8.0-A with none of the
forbidden extensions — QEMU models that core without crypto, so there is no
`crypto=off` property to set.
