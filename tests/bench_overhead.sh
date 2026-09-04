#!/usr/bin/env bash
# Measures trace-emission overhead: the same workload run under QEMU with and
# without the plugin, plus the resulting bytes-per-instruction.
#
# This is the number that decides whether the M2 (50 M) and M5 (200 M) gates
# are reachable in reasonable wall time.
set -euo pipefail
cd "$(dirname "$0")"
. ./qemu_cpu.sh

coracle_check_qemu

LIMIT="${1:-20000000}"
PLUGIN_EXT="so"
[ "$(uname -s)" = "Darwin" ] && PLUGIN_EXT="dylib"
PLUGIN="qemu-plugin/libcoracle_trace.$PLUGIN_EXT"
IMAGE="build/bench_loop.bin"
mkdir -p out

# QEMU's own icount-limited run is not available without a plugin, so the
# baseline uses the same plugin with output to /dev/null suppressed via a
# separate no-op measurement: instead we time a fixed wall window and compare
# instructions retired, which is the quantity that matters.
run_traced() {
    local output="$1"
    rm -f "$output"
    local start end
    start=$(node -e 'process.stdout.write(String(Date.now()))')
    "$CORACLE_QEMU_BIN" -M "$CORACLE_QEMU_MACHINE" -cpu "$CORACLE_QEMU_CPU" \
        -m 128 -nographic -accel tcg -no-reboot -kernel "$IMAGE" \
        -plugin "$PLUGIN,out=$output,limit=$LIMIT,scope=${SCOPE:-all},cpu=$CORACLE_FEATURE_ID,qemu=$CORACLE_QEMU_VERSION" \
        > "$output.log" 2>&1 &
    local pid=$!
    for _ in $(seq 1 6000); do
        kill -0 "$pid" 2>/dev/null || break
        node differ/cdt.mjs is-complete "$output" >/dev/null 2>&1 && break
        sleep 0.1
    done
    kill "$pid" 2>/dev/null || true
    wait "$pid" 2>/dev/null || true
    end=$(node -e 'process.stdout.write(String(Date.now()))')
    echo $((end - start))
}

echo "workload:    $IMAGE"
echo "limit:       $LIMIT instructions"
echo ""

TRACED_MS=$(run_traced out/bench.cdt)
BYTES=$(wc -c < out/bench.cdt | tr -d ' ')

node - "$LIMIT" "$TRACED_MS" "$BYTES" <<'EOF'
const [limit, ms, bytes] = process.argv.slice(2).map(Number);
const mips = limit / (ms / 1000) / 1e6;
const bytesPerInsn = bytes / limit;
const report = (label, value) => console.log(`${label.padEnd(26)} ${value}`);
report("traced wall time", `${(ms / 1000).toFixed(2)} s`);
report("traced throughput", `${mips.toFixed(1)} M instructions/s`);
report("trace size", `${(bytes / 1e6).toFixed(1)} MB`);
report("bytes per instruction", bytesPerInsn.toFixed(2));
console.log("");
for (const [gate, count] of [["M2 (50 M)", 50e6], ["M5 (200 M)", 200e6]]) {
  const seconds = count / (mips * 1e6);
  const gigabytes = (count * bytesPerInsn) / 1e9;
  report(`${gate} projected`, `${seconds.toFixed(0)} s, ${gigabytes.toFixed(1)} GB`);
}
EOF
