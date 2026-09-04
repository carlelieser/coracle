#!/usr/bin/env bash
# Runs inside the linux/arm64 builder. Not meant to be invoked directly;
# build.sh sets the mounts and environment this expects.
#
#   /src    kernel/ directory, read-only
#   /cache  verified source tarballs, read-only
#   /out    artifacts

set -euo pipefail

SRC=/src
CACHE=/cache
OUT=/out
WORK=/build

# Reproducibility: fixed build metadata so two builds of the same sources
# produce byte-identical images.
export KBUILD_BUILD_TIMESTAMP='Thu Jan  1 00:00:00 UTC 1970'
export KBUILD_BUILD_USER=coracle
export KBUILD_BUILD_HOST=builder
export ARCH=arm64
export SOURCE_DATE_EPOCH=0

JOBS="$(nproc)"

log() { printf '\n--- %s\n' "$*"; }

# Force a symbol in the .config of the current directory. `y` sets it, `n`
# comments it out in the form oldconfig expects.
set_symbol() {
	local symbol="$1" value="$2"

	sed -i "/^$symbol=/d;/^# $symbol is not set\$/d" .config
	if [ "$value" = "y" ]; then
		echo "$symbol=y" >> .config
	else
		echo "# $symbol is not set" >> .config
	fi
}

build_busybox() {
	log "Extracting busybox $BUSYBOX_VERSION"
	tar -xf "$CACHE/busybox-$BUSYBOX_VERSION.tar.bz2" -C "$WORK"

	cd "$WORK/busybox-$BUSYBOX_VERSION"
	make -s defconfig

	# busybox ships no scripts/config, so edit .config directly.
	#   STATIC   the initramfs has no dynamic loader and no /lib
	#   TC       fails to build against modern kernel headers; unused
	#   PAM/SELINUX/UTMP/WTMP  pull in libraries that do not exist here
	set_symbol CONFIG_STATIC y
	set_symbol CONFIG_TC n
	set_symbol CONFIG_PAM n
	set_symbol CONFIG_SELINUX n
	set_symbol CONFIG_FEATURE_UTMP n
	set_symbol CONFIG_FEATURE_WTMP n
	# Hardware-accelerated hashing uses the ARMv8 crypto extensions, which the
	# feature mask does not advertise. On 1.37.0 it also fails to compile on
	# aarch64 (sha1_process_block64_shaNI is declared for x86 only).
	set_symbol CONFIG_SHA1_HWACCEL n
	set_symbol CONFIG_SHA256_HWACCEL n
	# oldconfig takes the default for every new symbol on EOF. No pipe here:
	# `yes` would be SIGPIPEd on exit and trip `set -e`.
	make -s oldconfig </dev/null >/dev/null

	log "Building busybox"
	make -s -j"$JOBS"
	make -s CONFIG_PREFIX="$WORK/rootfs" install
}

assemble_initramfs() {
	log "Assembling initramfs"
	cd "$WORK/rootfs"
	mkdir -p proc sys dev dev/pts tmp run etc root mnt

	install -m 0755 "$SRC/initramfs/init" ./init
	# Selected with rdinit=/gate-init for the non-interactive M0 gate run.
	install -m 0755 "$SRC/initramfs/gate-init" ./gate-init

	# A minimal passwd/group keeps `id`, `ls -l` and ps(1) from printing raw
	# uids in the gate transcript.
	printf 'root:x:0:0:root:/root:/bin/sh\n' > etc/passwd
	printf 'root:x:0:\n'                     > etc/group

	find . -print0 \
		| sort -z \
		| cpio --null --create --format=newc --owner=root:root 2>/dev/null \
		| gzip -9n > "$OUT/initramfs.cpio.gz"
}

build_kernel() {
	log "Extracting linux $KERNEL_VERSION"
	tar -xf "$CACHE/linux-$KERNEL_VERSION.tar.xz" -C "$WORK"
	cd "$WORK/linux-$KERNEL_VERSION"

	log "Merging coracle.config onto defconfig"
	make -s defconfig
	./scripts/kconfig/merge_config.sh -m -O . .config "$SRC/config/coracle.config" >/dev/null
	make -s olddefconfig

	log "Building kernel"
	make -s -j"$JOBS" Image

	cp arch/arm64/boot/Image "$OUT/Image"
	cp .config "$OUT/kernel.config"
}

build_dtb() {
	log "Compiling device tree"
	dtc -I dts -O dtb -o "$OUT/coracle-virt.dtb" \
		-W no-unit_address_vs_reg \
		-W no-avoid_unnecessary_addr_size \
		"$SRC/dts/coracle-virt.dts"
}

# A kernel built with any of these does not run on our CPU model. Checking the
# final .config catches it here rather than as a boot-time undefined
# instruction.
#
# KERNEL_MODE_NEON is deliberately absent: it is def_bool y on arm64 and only
# gates kernel_neon_begin/end. Base AdvSIMD is ARMv8.0-A, which the feature mask
# does advertise; ARM64_CRYPTO is what must stay out.
FORBIDDEN_SYMBOLS=(
	CONFIG_ARM64_USE_LSE_ATOMICS
	CONFIG_ARM64_LSE_ATOMICS
	CONFIG_ARM64_SVE
	CONFIG_ARM64_SME
	CONFIG_ARM64_MTE
	CONFIG_ARM64_PTR_AUTH
	CONFIG_ARM64_PTR_AUTH_KERNEL
	CONFIG_ARM64_BTI
	CONFIG_ARM64_BTI_KERNEL
	CONFIG_ARM64_CRYPTO
	CONFIG_COMPAT
	CONFIG_RANDOMIZE_BASE
	CONFIG_MODULES
	CONFIG_PCI
)

# Without these the machine has no console, no root, and no way to power off.
REQUIRED_SYMBOLS=(
	CONFIG_ARM_PSCI_FW
	CONFIG_ARM_GIC
	CONFIG_SERIAL_AMBA_PL011_CONSOLE
	CONFIG_VIRTIO_MMIO
	CONFIG_VIRTIO_BLK
	CONFIG_VIRTIO_NET
	CONFIG_HW_RANDOM_VIRTIO
	CONFIG_VIRTIO_CONSOLE
	CONFIG_9P_FS
	CONFIG_NET_9P_VIRTIO
	CONFIG_OVERLAY_FS
	CONFIG_EXT4_FS
	CONFIG_BLK_DEV_INITRD
)

verify_config() {
	log "Verifying the feature mask is honoured"
	local config="$OUT/kernel.config"
	local failed=0

	for symbol in "${FORBIDDEN_SYMBOLS[@]}"; do
		if grep -q "^$symbol=y" "$config"; then
			echo "FAIL: $symbol is enabled but the feature mask forbids it" >&2
			failed=1
		fi
	done

	for symbol in "${REQUIRED_SYMBOLS[@]}"; do
		if ! grep -q "^$symbol=y" "$config"; then
			echo "FAIL: $symbol is required but not enabled" >&2
			failed=1
		fi
	done

	[ "$failed" -eq 0 ] || exit 1
	echo "config verification passed"
}

write_manifest() {
	log "Writing manifest"
	{
		echo "kernel        linux-$KERNEL_VERSION"
		echo "busybox       busybox-$BUSYBOX_VERSION"
		echo "built         reproducible (SOURCE_DATE_EPOCH=0)"
		echo
		echo "artifact                    size       sha256"
		for artifact in Image initramfs.cpio.gz coracle-virt.dtb kernel.config; do
			printf '%-26s  %-9s  %s\n' \
				"$artifact" \
				"$(stat -c %s "$OUT/$artifact")" \
				"$(sha256sum "$OUT/$artifact" | cut -d' ' -f1)"
		done
	} > "$OUT/manifest.txt"
}

build_busybox
assemble_initramfs
build_kernel
build_dtb
verify_config
write_manifest

log "Done"
