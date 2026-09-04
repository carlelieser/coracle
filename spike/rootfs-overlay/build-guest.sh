#!/usr/bin/env bash
# Builds the pinned kernel and a static busybox inside a linux/arm64 container.
# Outputs build/Image (kernel) and build/busybox. Both are cached: re-running
# is a no-op once they exist.
set -euo pipefail

SPIKE_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# Artifacts are only accepted if the container exits clean; a partial build
# must not be cached and silently reused.
BUILD_DIR="$SPIKE_DIR/build"
# shellcheck source=versions.env
set -a; source "$SPIKE_DIR/versions.env"; set +a

mkdir -p "$BUILD_DIR"

# Cached per artifact: the kernel is the slow one and must not be rebuilt
# just because busybox is missing.
if [[ -s "$BUILD_DIR/Image" ]]; then
  echo "build-guest: kernel Image already present, skipping kernel build."
else
echo "build-guest: building kernel $KERNEL_VERSION (15-40 minutes)"
docker run --rm --platform linux/arm64 \
  -v "$BUILD_DIR:/out" \
  -v "$SPIKE_DIR/kernel.fragment:/kernel.fragment:ro" \
  -e KERNEL_VERSION -e KERNEL_SHA256 -e KERNEL_URL \
  "$BUILDER_IMAGE" bash -euxo pipefail -c '
    export DEBIAN_FRONTEND=noninteractive
    apt-get update -qq
    apt-get install -y -qq --no-install-recommends \
      build-essential bc bison flex libssl-dev libelf-dev \
      curl xz-utils bzip2 ca-certificates kmod cpio python3 >/dev/null

    cd /tmp

    # --- kernel ---
    curl -fsSL -o linux.tar.xz "$KERNEL_URL"
    echo "$KERNEL_SHA256  linux.tar.xz" | sha256sum -c -
    tar xf linux.tar.xz
    cd "linux-$KERNEL_VERSION"
    make -s defconfig
    ./scripts/kconfig/merge_config.sh -m .config /kernel.fragment
    make -s olddefconfig
    # merge_config + olddefconfig can silently drop an option (unmet
    # dependency, or a later default winning). Verify every =y we asked for
    # actually landed, so the spike cannot pass on a kernel missing a feature.
    fail=0
    while read -r line; do
      case "$line" in \#*|"") continue ;; esac
      opt="${line%%=*}"
      want="${line#*=}"
      if [ "$want" = "y" ]; then
        if ! grep -qx "${opt}=y" .config; then
          echo "MISSING: ${opt}=y"
          fail=1
        fi
      elif [ "$want" = "n" ]; then
        if grep -qx "${opt}=y" .config; then
          echo "UNWANTED: ${opt} is =y"
          fail=1
        fi
      fi
    done < /kernel.fragment
    [ "$fail" -eq 0 ] || { echo "kernel config verification FAILED"; exit 1; }
    echo "kernel config verification OK"

    make -s -j"$(nproc)" Image
    cp arch/arm64/boot/Image /out/Image
    cp .config /out/kernel.config

  '
fi

# --- busybox: prebuilt static, see versions.env for why not from source ---
if [[ -s "$BUILD_DIR/busybox" ]]; then
  echo "build-guest: busybox already present."
else
  docker run --rm --platform linux/arm64 -v "$BUILD_DIR:/out" \
    "$BUSYBOX_IMAGE" sh -euc '
      apk add --no-cache -q busybox-static
      cp /bin/busybox.static /out/busybox
      /out/busybox | head -1
    '
fi

# A partial build must never be cached and silently reused.
for artifact in Image busybox; do
  [ -s "$BUILD_DIR/$artifact" ] || { echo "build-guest: $artifact missing or empty"; exit 1; }
done

echo "build-guest: done."
ls -la "$BUILD_DIR/Image" "$BUILD_DIR/busybox"
