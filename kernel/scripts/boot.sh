#!/usr/bin/env bash
# Boot the built kernel under QEMU `virt` with our own DTB.
#
# Passing -dtb is the point: the M0 gate is that the guest boots on the device
# tree the emulator will use, not on QEMU's auto-generated one.
#
#   ./boot.sh            interactive shell on the terminal
#   ./boot.sh --gate     non-interactive; runs the gate checks and exits

set -euo pipefail

KERNEL_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
OUT_DIR="$KERNEL_DIR/out"

# shellcheck source=../versions.env
source "$KERNEL_DIR/versions.env"

for artifact in Image initramfs.cpio.gz coracle-virt.dtb; do
	if [ ! -f "$OUT_DIR/$artifact" ]; then
		echo "boot.sh: $OUT_DIR/$artifact is missing — run ./build.sh first" >&2
		exit 1
	fi
done

installed_qemu="$(qemu-system-aarch64 --version | head -1 | grep -oE '[0-9]+\.[0-9]+\.[0-9]+')"
if [ "$installed_qemu" != "$QEMU_VERSION" ]; then
	echo "boot.sh: warning: QEMU $installed_qemu installed, $QEMU_VERSION pinned" >&2
fi

# -cpu cortex-a53 is ARMv8.0-A with no LSE, SVE, PAuth, MTE or BTI, so it
# matches the advertised feature mask. It also has no `crypto` property to set:
# QEMU models this core without the crypto extensions, which is what we want.
# -machine virt: only the CPU, RAM and the SMC/PSCI trap come from QEMU; every
# device the guest sees is described by our own DTB. No -bios: with -kernel on
# `virt`, QEMU boots the Image directly and loads no firmware.
# The gate runs a non-interactive init that self-checks and powers off; the
# default run hands over a shell.
if [ "${1:-}" = "--gate" ]; then
	rdinit=/gate-init
else
	rdinit=/init
fi

qemu_args=(
	-machine virt,gic-version=2,accel=tcg,graphics=off,usb=off
	-cpu cortex-a53
	-smp 1
	-m 1024
	-kernel "$OUT_DIR/Image"
	-initrd "$OUT_DIR/initramfs.cpio.gz"
	-dtb "$OUT_DIR/coracle-virt.dtb"
	-append "console=ttyAMA0 earlycon=pl011,0x9000000 rdinit=$rdinit panic=-1"
	-nographic
	-no-reboot
)

if [ "${1:-}" = "--gate" ]; then
	exec "$KERNEL_DIR/scripts/gate.sh" "${qemu_args[@]}"
fi

exec qemu-system-aarch64 "${qemu_args[@]}"
