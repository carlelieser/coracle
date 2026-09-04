#!/usr/bin/env bash
# M0 gate: "Differential harness runs a 10-instruction ELF and reports
# identical state", plus the negative cases that make the differ trustworthy.
set -uo pipefail
cd "$(dirname "$0")"
. ./qemu_cpu.sh

PASSED=0
FAILED=0

check() {
    local name="$1" want="$2"
    shift 2
    local output status
    output=$("$@" 2>&1)
    status=$?
    if [ "$status" -eq "$want" ]; then
        printf "  ok    %s\n" "$name"
        PASSED=$((PASSED + 1))
    else
        printf "  FAIL  %s (exit %d, wanted %d)\n" "$name" "$status" "$want"
        printf "%s\n" "$output" | sed 's/^/          /'
        FAILED=$((FAILED + 1))
    fi
}

echo "== build =="
make -C qemu-plugin >/dev/null || { echo "plugin build failed"; exit 1; }
./build_corpus.sh >/dev/null || { echo "corpus build failed"; exit 1; }
echo "  ok    plugin and corpus built"
echo ""

echo "== feature mask =="
./verify_feature_mask.sh | sed -n '3,$p' | sed 's/^/  /' | grep -E "ok|FAIL|PASS" || true
./verify_feature_mask.sh >/dev/null 2>&1 || { echo "  FAIL feature mask"; FAILED=$((FAILED+1)); }
echo ""

echo "== trace generation =="
./run_qemu.sh build/m0_ten_insn.bin out/gate.cdt 40 >/dev/null 2>&1
check "10-instruction program traces" 0 test -s out/gate.cdt
check "trace is well-formed and complete" 0 node differ/cdt.mjs is-complete out/gate.cdt
./run_qemu.sh build/m0_exception.bin out/gate_exc.cdt 60 >/dev/null 2>&1
check "exception program traces" 0 test -s out/gate_exc.cdt
echo ""

echo "== M0 GATE: identical state =="
cp out/gate.cdt out/gate_copy.cdt
check "identical traces report MATCH" 0 \
    node differ/diff.mjs out/gate.cdt out/gate_copy.cdt
echo ""

echo "== negative cases: differ localises a fault =="
node differ/perturb.mjs out/gate.cdt out/bad_gpr.cdt --reg=x5 --at-step=2 --xor=0x40 >/dev/null
check "perturbed x5 is caught" 1 \
    node differ/diff.mjs out/gate.cdt out/bad_gpr.cdt

node differ/perturb.mjs out/gate.cdt out/bad_pstate.cdt --reg=pstate --at-step=2 --xor=0x40000000 >/dev/null
check "perturbed pstate is caught" 1 \
    node differ/diff.mjs out/gate.cdt out/bad_pstate.cdt

node differ/perturb.mjs out/gate.cdt out/bad_pc.cdt --reg=pc --at-step=2 --xor=0x10 >/dev/null
check "perturbed pc is caught" 1 \
    node differ/diff.mjs out/gate.cdt out/bad_pc.cdt

node differ/perturb.mjs out/gate_exc.cdt out/bad_esr.cdt --reg=ESR_EL1 --at-step=4 --xor=0x1000000 >/dev/null
check "perturbed ESR_EL1 in exception record is caught" 1 \
    node differ/diff.mjs out/gate_exc.cdt out/bad_esr.cdt
echo ""

echo "== differ localises the RIGHT register =="
REPORT=$(node differ/diff.mjs out/gate.cdt out/bad_gpr.cdt 2>&1)
if printf "%s" "$REPORT" | grep -q "register x5 differs" &&
   printf "%s" "$REPORT" | grep -q "0x0000000000000032" &&
   printf "%s" "$REPORT" | grep -q "0x0000000000000072"; then
    echo "  ok    report names x5 with expected and actual values"
    PASSED=$((PASSED + 1))
else
    echo "  FAIL  report did not localise x5 correctly"
    printf "%s\n" "$REPORT" | sed 's/^/          /'
    FAILED=$((FAILED + 1))
fi
echo ""

echo "== cross-implementation constants =="
check "feature id hash agrees between C and JS" 0 \
    node differ/feature_id.mjs "$CORACLE_FEATURE_ID" out/gate.cdt
echo ""

echo "== guards =="
node -e '
const fs = require("fs");
const bytes = fs.readFileSync("out/gate.cdt");
bytes.writeBigUInt64LE(0xdeadbeefn, 24);
fs.writeFileSync("out/othercpu.cdt", bytes);
'
check "mismatched feature mask is refused" 2 \
    node differ/diff.mjs out/gate.cdt out/othercpu.cdt
check "mismatched mask can be overridden explicitly" 0 \
    node differ/diff.mjs out/gate.cdt out/othercpu.cdt --allow-feature-mismatch
printf '\x00' > out/truncated.cdt
check "garbage trace is rejected, not misread" 2 \
    node differ/diff.mjs out/gate.cdt out/truncated.cdt
echo ""

echo "== FP comparison policy =="
node differ/fp_policy_test.mjs
if [ $? -eq 0 ]; then
    PASSED=$((PASSED + 1))
else
    FAILED=$((FAILED + 1))
fi
echo ""

echo "== scope equivalence =="
CORACLE_SCOPE=core ./run_qemu.sh build/m0_exception.bin out/scope_core.cdt 60 >/dev/null 2>&1
check "core scope still emits full state at exception entry" 0 \
    node differ/assert_exception_state.mjs out/scope_core.cdt
echo ""

echo "----------------------------------------"
echo "passed: $PASSED   failed: $FAILED"
[ "$FAILED" -eq 0 ] || exit 1
echo "M0 gate item satisfied."
