#!/usr/bin/env bash
# M0 rootfs spike entrypoint. One command, one verdict.
#
#   ./run-spike.sh
#
# Builds the pinned kernel and busybox if absent (slow, cached), then boots
# QEMU inside a linux/arm64 container and runs the overlay-on-9p test matrix.
# Prints a per-behavior table and exits non-zero if any case failed.
set -euo pipefail

SPIKE_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BUILD_DIR="$SPIKE_DIR/build"
set -a; source "$SPIKE_DIR/versions.env"; set +a

"$SPIKE_DIR/build-guest.sh"

# The guest scripts ride into the container through the build dir.
rm -rf "$BUILD_DIR/guest"
cp -R "$SPIKE_DIR/guest" "$BUILD_DIR/guest"

echo
echo "=== running spike under QEMU $QEMU_VERSION in $RUNNER_IMAGE ==="
docker run --rm --platform linux/arm64 \
  -v "$BUILD_DIR:/out" \
  -v "$SPIKE_DIR/in-container-run.sh:/run.sh:ro" \
  "$RUNNER_IMAGE" bash -euo pipefail -c '
    export DEBIAN_FRONTEND=noninteractive
    apt-get update -qq
    apt-get install -y -qq --no-install-recommends \
      qemu-system-arm attr e2fsprogs cpio gzip file >/dev/null
    bash /run.sh
  '

LOG="$BUILD_DIR/boot.log"
[ -f "$LOG" ] || { echo "spike: no boot log produced"; exit 1; }

echo
echo "=============================================="
echo " VERDICT"
echo "=============================================="
grep -E '^(PASS|FAIL|SKIP) ' "$LOG" | sed 's/^/  /' || true
echo
grep -E '^SPIKE_(BOOT|SUMMARY|RESULT)' "$LOG" | sed 's/^/  /' || true
echo "=============================================="
echo "full log: $LOG"

if grep -q '^SPIKE_RESULT: ALL_PASS' "$LOG"; then
  exit 0
fi
exit 1
