#!/usr/bin/env bash
# Runs one flat image under the pinned QEMU with the pinned feature mask and
# writes a CDT trace.
#
#   ./run_qemu.sh build/m0_ten_insn.bin out/qemu.cdt [instruction-limit]
set -euo pipefail
cd "$(dirname "$0")"
. ./qemu_cpu.sh

IMAGE="${1:?usage: run_qemu.sh <image.bin> <out.cdt> [limit]}"
OUTPUT="${2:?usage: run_qemu.sh <image.bin> <out.cdt> [limit]}"
LIMIT="${3:-200}"

coracle_check_qemu

PLUGIN_EXT="so"
[ "$(uname -s)" = "Darwin" ] && PLUGIN_EXT="dylib"
PLUGIN="qemu-plugin/libcoracle_trace.$PLUGIN_EXT"
if [ ! -f "$PLUGIN" ]; then
    echo "error: $PLUGIN not built; run 'make -C qemu-plugin'" >&2
    exit 1
fi

mkdir -p "$(dirname "$OUTPUT")"
rm -f "$OUTPUT"

# The guest parks in a self-branch once its work is done, so QEMU is stopped by
# the trace limit and then killed. `limit=` makes the trace deterministic.
"$CORACLE_QEMU_BIN" \
    -M "$CORACLE_QEMU_MACHINE" \
    -cpu "$CORACLE_QEMU_CPU" \
    -m 128 -nographic -accel tcg -no-reboot \
    -kernel "$IMAGE" \
    -plugin "$PLUGIN,out=$OUTPUT,limit=$LIMIT,scope=${CORACLE_SCOPE:-all},cpu=$CORACLE_FEATURE_ID,qemu=$CORACLE_QEMU_VERSION" \
    > "$OUTPUT.log" 2>&1 &
QEMU_PID=$!

# The plugin closes the trace as soon as `limit` instructions retire; the guest
# itself never exits (it parks in a self-branch), so QEMU is killed afterwards.
TIMEOUT_SECONDS="${CORACLE_QEMU_TIMEOUT:-10}"
for _ in $(seq 1 "$((TIMEOUT_SECONDS * 10))"); do
    kill -0 "$QEMU_PID" 2>/dev/null || break
    if node differ/cdt.mjs is-complete "$OUTPUT" >/dev/null 2>&1; then break; fi
    sleep 0.1
done

kill "$QEMU_PID" 2>/dev/null || true
wait "$QEMU_PID" 2>/dev/null || true

if [ ! -s "$OUTPUT" ]; then
    echo "error: no trace written to $OUTPUT" >&2
    sed -n '1,20p' "$OUTPUT.log" >&2
    exit 1
fi
echo "wrote $OUTPUT ($(wc -c < "$OUTPUT" | tr -d ' ') bytes)"
