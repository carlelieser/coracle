#!/usr/bin/env bash
# Runs inside the linux/arm64 runner container. Builds the 9p lower tree and
# the ext4 upper image on the container's own filesystem (which, unlike a
# macOS bind mount, preserves uid/gid and user.* xattrs), assembles the
# initramfs, and boots QEMU.
#
# /out is the bind-mounted host build dir: read Image and busybox from it,
# write logs back to it. Nothing that needs Linux metadata lives there.
set -euo pipefail

OUT=/out
WORK=/work
mkdir -p "$WORK"

echo "=== runner: qemu $(qemu-system-aarch64 --version | head -1) ==="

# --------------------------------------------------------------------------
# Lower layer. Built on the container fs so ownership and xattrs are real.
# --------------------------------------------------------------------------
LOWER="$WORK/lower"
rm -rf "$LOWER"
mkdir -p "$LOWER/fixtures" "$LOWER/work"

# The lower layer carries the userspace, exactly as a container image layer
# would: after switch_root the initramfs is gone, so /bin/sh must come from
# the overlay root itself.
mkdir -p "$LOWER/bin" "$LOWER/proc" "$LOWER/sys" "$LOWER/dev" "$LOWER/layers"
cp "$OUT/busybox" "$LOWER/bin/busybox"
chmod +x "$LOWER/bin/busybox"
for applet in sh mount umount mkdir ls cat echo rm mv ln cp stat find awk grep \
              sed tr head tail wc chmod chown touch sync date poweroff \
              switch_root readlink sleep uname basename dmesg; do
  ln -sf busybox "$LOWER/bin/$applet"
done
for tool in /usr/bin/getfattr /usr/bin/setfattr; do
  cp "$tool" "$LOWER/bin/"
  ldd "$tool" 2>/dev/null | grep -oE '/[^ ]+\.so[^ ]*' | while read -r lib; do
    mkdir -p "$LOWER$(dirname "$lib")"
    cp -n "$lib" "$LOWER$lib" 2>/dev/null || true
  done
done
for loader in /lib/ld-linux-aarch64.so.1 /lib64/ld-linux-aarch64.so.1; do
  [ -e "$loader" ] && { mkdir -p "$LOWER$(dirname "$loader")"; cp -n "$loader" "$LOWER$loader"; }
done

echo "lower-original-content" > "$LOWER/fixtures/hello.txt"
setfattr -n user.spike -v lower-xattr-value "$LOWER/fixtures/hello.txt"

echo "delete me" > "$LOWER/fixtures/deleteme.txt"

mkdir -p "$LOWER/fixtures/subdir"
echo a > "$LOWER/fixtures/subdir/lower-a.txt"
echo b > "$LOWER/fixtures/subdir/lower-b.txt"

echo "movable-content" > "$LOWER/fixtures/movable.txt"
mkdir -p "$LOWER/fixtures/movedir"
echo "inside-movedir" > "$LOWER/fixtures/movedir/inside.txt"

echo "owned-content" > "$LOWER/fixtures/owned.txt"
chown 1234:5678 "$LOWER/fixtures/owned.txt"

echo "setuid-content" > "$LOWER/fixtures/setuid.bin"
chmod 4755 "$LOWER/fixtures/setuid.bin"

echo "linked-content" > "$LOWER/fixtures/link-a.txt"
ln "$LOWER/fixtures/link-a.txt" "$LOWER/fixtures/link-b.txt"

ln -s hello.txt "$LOWER/fixtures/link-to-hello"

# Fail loudly if the filesystem holding the lower tree cannot carry the
# metadata at all; downstream results would then be measuring the host rather
# than the guest. This deliberately checks *capability*, not the specific
# fixture values -- asserting the exact uid here would pre-empt the guest-side
# test and mask whether that test actually works.
FIX_OWNER=$(stat -c '%u:%g' "$LOWER/fixtures/owned.txt")
FIX_XATTR_COUNT=$(getfattr -d -m 'user\.' "$LOWER/fixtures/" -R 2>/dev/null | grep -c '^user\.' || true)
FIX_NLINK=$(stat -c '%h' "$LOWER/fixtures/link-a.txt")
echo "=== fixture self-check (host capability, not fixture values) ==="
echo "owned.txt owner: $FIX_OWNER (want any non-root)"
echo "user.* xattrs present under fixtures/: $FIX_XATTR_COUNT (want >= 1)"
echo "link-a.txt nlink: $FIX_NLINK (want 2)"
[ "$FIX_OWNER" != "0:0" ] || { echo "FIXTURE SELF-CHECK FAILED: chown did not take on the host fs"; exit 1; }
[ "$FIX_XATTR_COUNT" -ge 1 ] || { echo "FIXTURE SELF-CHECK FAILED: user.* xattrs not supported on the host fs"; exit 1; }
[ "$FIX_NLINK" = "2" ] || { echo "FIXTURE SELF-CHECK FAILED: hardlink not preserved on the host fs"; exit 1; }
echo "fixture self-check OK"
echo

# --------------------------------------------------------------------------
# Upper layer: a formatted, empty ext4 image.
# --------------------------------------------------------------------------
rm -f "$WORK/upper.img"
truncate -s 256M "$WORK/upper.img"
mkfs.ext4 -q -F -L spike-upper "$WORK/upper.img"

# --------------------------------------------------------------------------
# initramfs: busybox plus the spike scripts.
# --------------------------------------------------------------------------
IRD="$WORK/initramfs"
rm -rf "$IRD"
mkdir -p "$IRD/bin" "$IRD/spike/tests" "$IRD/proc" "$IRD/sys" "$IRD/dev" \
         "$IRD/mnt/lower" "$IRD/mnt/upper" "$IRD/mnt/root"

cp "$OUT/busybox" "$IRD/bin/busybox"
chmod +x "$IRD/bin/busybox"
for applet in sh mount umount mkdir ls cat echo rm mv ln cp stat find awk grep \
              sed tr head tail wc chmod chown touch sync date poweroff \
              switch_root readlink sleep uname basename dmesg; do
  ln -sf busybox "$IRD/bin/$applet"
done

# busybox has no getfattr/setfattr; the real attr binaries come from the
# runner image. They are dynamically linked, so bring their libraries too.
for tool in /usr/bin/getfattr /usr/bin/setfattr; do
  cp "$tool" "$IRD/bin/"
  ldd "$tool" 2>/dev/null | grep -oE '/[^ ]+\.so[^ ]*' | while read -r lib; do
    mkdir -p "$IRD$(dirname "$lib")"
    cp -n "$lib" "$IRD$lib" 2>/dev/null || true
  done
done
# The dynamic loader itself.
for loader in /lib/ld-linux-aarch64.so.1 /lib64/ld-linux-aarch64.so.1; do
  [ -e "$loader" ] && { mkdir -p "$IRD$(dirname "$loader")"; cp -n "$loader" "$IRD$loader"; }
done

cp "$OUT/guest/init" "$IRD/init"
cp "$OUT/guest/run-tests.sh" "$IRD/spike/run-tests.sh"
cp "$OUT/guest/stage2.sh" "$IRD/spike/stage2.sh"
cp "$OUT/guest/assert.sh" "$IRD/spike/assert.sh"
cp "$OUT/guest/tests/"*.sh "$IRD/spike/tests/"
chmod +x "$IRD/init" "$IRD/spike/run-tests.sh" "$IRD/spike/stage2.sh"

( cd "$IRD" && find . | cpio -o -H newc --quiet | gzip -9 > "$WORK/initramfs.cpio.gz" )
echo "initramfs: $(stat -c %s "$WORK/initramfs.cpio.gz") bytes"
echo

# --------------------------------------------------------------------------
# Boot. virtio-mmio only, matching the plan's machine model.
# --------------------------------------------------------------------------
echo "=== booting ==="
set +e
timeout 300 qemu-system-aarch64 \
  -machine virt \
  -cpu cortex-a72 \
  -m 1024 \
  -smp 1 \
  -nographic \
  -no-reboot \
  -net none \
  -kernel "$OUT/Image" \
  -initrd "$WORK/initramfs.cpio.gz" \
  -append "console=ttyAMA0 panic=1 loglevel=4" \
  -global virtio-mmio.force-legacy=false \
  -drive file="$WORK/upper.img",format=raw,if=none,id=upper \
  -device virtio-blk-device,drive=upper \
  -fsdev local,id=lowerfs,path="$LOWER",security_model=passthrough,readonly=on \
  -device virtio-9p-device,fsdev=lowerfs,mount_tag=lower \
  2>&1 | tee "$OUT/boot.log"
QEMU_RC=$?
set -e
echo
echo "=== qemu exit rc=$QEMU_RC ==="
