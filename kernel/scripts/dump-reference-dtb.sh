#!/usr/bin/env bash
# Dump QEMU `virt`'s own device tree and diff it against ours.
#
# coracle-virt.dts is derived from this dump; addresses and IRQs are QEMU's so
# the stock kernel boots unpatched. Run this after a QEMU bump to see whether
# the machine moved underneath us.

set -euo pipefail

KERNEL_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
OUT_DIR="$KERNEL_DIR/out/reference"

# shellcheck source=../versions.env
source "$KERNEL_DIR/versions.env"

mkdir -p "$OUT_DIR"

# Same machine shape the emulator clones: GICv2, one vCPU, 1 GB, no graphics.
# dtb-randomness=off drops the kaslr-seed and rng-seed properties so the dump is
# byte-stable across runs.
qemu-system-aarch64 \
	-machine "virt,gic-version=2,accel=tcg,graphics=off,usb=off,dtb-randomness=off,dumpdtb=$OUT_DIR/virt-reference.dtb" \
	-cpu cortex-a53 -smp 1 -m 1024 -nographic

dtc -I dtb -O dts -o "$OUT_DIR/virt-reference.dts" "$OUT_DIR/virt-reference.dtb" 2>/dev/null

echo "reference: $OUT_DIR/virt-reference.dts (qemu $QEMU_VERSION)"
echo

# Reconciliation map: one "node<TAB>reg<TAB>interrupts" line per device, sorted
# by unit address. Property order inside a node is not meaningful, so folding
# each node onto a single sorted line keeps the diff to real differences.
extract_map() {
	dtc -I dts -O dts "$1" 2>/dev/null | awk '
		/^[[:space:]]*[a-zA-Z0-9_-]+@[0-9a-f]+ \{/ {
			if (node != "") print node "\t" reg "\t" irq
			node = $1; reg = "-"; irq = "-"
			next
		}
		/^[[:space:]]*reg = / && node != ""        { sub(/^[^=]*= /, ""); sub(/;$/, ""); reg = $0 }
		/^[[:space:]]*interrupts = / && node != "" { sub(/^[^=]*= /, ""); sub(/;$/, ""); irq = $0 }
		END { if (node != "") print node "\t" reg "\t" irq }
	' | sort
}

extract_map "$OUT_DIR/virt-reference.dts" > "$OUT_DIR/reference.map"
extract_map "$KERNEL_DIR/dts/coracle-virt.dts" > "$OUT_DIR/coracle.map"

echo "=== devices: in QEMU virt only (<) vs in coracle only (>) ==="
diff "$OUT_DIR/reference.map" "$OUT_DIR/coracle.map" || true

echo
echo "=== coracle machine map (node / reg / interrupts) ==="
column -t -s "$(printf '\t')" "$OUT_DIR/coracle.map"
