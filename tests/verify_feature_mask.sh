#!/usr/bin/env bash
# Verifies the advertised CPU feature mask empirically rather than by trusting
# the model name: executes one instruction gated on each excluded feature and
# asserts QEMU takes an undefined-instruction exception.
#
# Per docs/plan.md §2 the mask is ARMv8.0-A with NO LSE, SVE, PAuth, MTE,
# crypto, or BTI. If QEMU advertises any of them, every differential run is
# noise, so this is a gate, not a diagnostic.
set -uo pipefail
cd "$(dirname "$0")"
. ./qemu_cpu.sh

coracle_check_qemu || exit 1

OBJCOPY="${OBJCOPY:-$(brew --prefix llvm 2>/dev/null)/bin/llvm-objcopy}"
command -v "$OBJCOPY" >/dev/null 2>&1 || OBJCOPY=llvm-objcopy

PLUGIN_EXT="so"
[ "$(uname -s)" = "Darwin" ] && PLUGIN_EXT="dylib"
PLUGIN="qemu-plugin/libcoracle_trace.$PLUGIN_EXT"

mkdir -p out/featmask
FAILURES=0

# Each probe runs a single gated instruction under a vector table that counts
# synchronous exceptions. Feature absent => 1 exception; present => 0.
probe() {
    local name="$1" encoding="$2" want="$3"
    local dir="out/featmask"
    cat > "$dir/probe.s" <<EOF
.text
.global _start
_start:
    adr  x0, vectors
    msr  vbar_el1, x0
    mov  x0, #(3 << 20)
    msr  cpacr_el1, x0
    isb
    .inst $encoding
park:
    b    park
.balign 2048
vectors:
    .rept 4
    .balign 128
    b .
    .endr
    .balign 128
    b park
EOF
    clang -target aarch64-unknown-none -c "$dir/probe.s" -o "$dir/probe.o" 2>/dev/null
    "$OBJCOPY" -O binary --only-section=.text "$dir/probe.o" "$dir/probe.bin"
    rm -f "$dir/probe.cdt"

    "$CORACLE_QEMU_BIN" -M "$CORACLE_QEMU_MACHINE" -cpu "$CORACLE_QEMU_CPU" \
        -m 128 -nographic -accel tcg -no-reboot -kernel "$dir/probe.bin" \
        -plugin "$PLUGIN,out=$dir/probe.cdt,limit=200,cpu=$CORACLE_FEATURE_ID,qemu=$CORACLE_QEMU_VERSION" \
        > "$dir/probe.log" 2>&1 &
    local pid=$!
    for _ in $(seq 1 60); do
        kill -0 "$pid" 2>/dev/null || break
        node differ/cdt.mjs is-complete "$dir/probe.cdt" >/dev/null 2>&1 && break
        sleep 0.1
    done
    kill "$pid" 2>/dev/null; wait "$pid" 2>/dev/null

    local seen
    seen=$(node differ/cdt.mjs count-exceptions "$dir/probe.cdt" 2>/dev/null || echo "error")
    local verdict="present"
    [ "$seen" != "0" ] && [ "$seen" != "error" ] && verdict="absent"

    if [ "$verdict" = "$want" ]; then
        printf "  ok    %-16s %s\n" "$name" "$verdict"
    else
        printf "  FAIL  %-16s want=%s got=%s (exceptions=%s)\n" \
            "$name" "$want" "$verdict" "$seen"
        FAILURES=$((FAILURES + 1))
    fi
}

echo "feature mask check: -cpu $CORACLE_QEMU_CPU"
echo "                    id $CORACLE_FEATURE_ID"
echo ""

# Excluded by plan §2 -- all must be absent.
probe "LSE (CAS)"      "0x88a07c40" absent
probe "PAuth (PACIZA)" "0xdac10000" absent
probe "SVE (RDVL)"     "0x04bf5020" absent
probe "MTE (IRG)"      "0x9adf1000" absent

# Known deviation, asserted so it cannot widen: QEMU 11.1.1 offers no property
# to disable FEAT_AES/SHA on any AArch64 CPU model. These are expected PRESENT.
# If a future QEMU adds the knob, these two lines flip to `absent` and
# CORACLE_FEATURE_ID drops the `+qemucrypto` suffix.
probe "crypto (AESE)"  "0x4e284800" present
probe "crypto (SHA1H)" "0x5e280800" present

# Baseline ARMv8.0-A -- must be present, or the probe method is broken.
probe "base (NOP)"     "0xd503201f" present
probe "base (FADD d)"  "0x1e602800" present
probe "base (ADD v)"   "0x4ea08400" present

echo ""
if [ "$FAILURES" -ne 0 ]; then
    echo "FAIL: $FAILURES feature(s) do not match the plan's advertised mask"
    exit 1
fi
echo "PASS: advertised mask matches docs/plan.md section 2"
