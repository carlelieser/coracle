#!/usr/bin/env bash
# Adversarial check on the test matrix itself.
#
# A suite that only ever passes proves nothing: it might be passing because it
# silently did nothing. This script breaks the setup in specific ways and
# requires the matrix to NOTICE. If a mutation does not turn the run red, the
# corresponding test is inert and its PASS in RESULTS.md is worthless.
#
#   ./verify-harness.sh
set -euo pipefail

SPIKE_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BUILD_DIR="$SPIKE_DIR/build"
set -a; source "$SPIKE_DIR/versions.env"; set +a

# Each mutation: a name, a sed program applied to the in-container runner, and
# the test that must go red as a result.
run_mutation() {
  local name=$1 sed_program=$2 must_fail=$3
  local mutated="$BUILD_DIR/mutated-run.sh"

  sed "$sed_program" "$SPIKE_DIR/in-container-run.sh" > "$mutated"
  if cmp -s "$mutated" "$SPIKE_DIR/in-container-run.sh"; then
    echo "  INCONCLUSIVE: mutation '$name' changed nothing; the sed no longer matches"
    return 1
  fi

  rm -rf "$BUILD_DIR/guest"
  cp -R "$SPIKE_DIR/guest" "$BUILD_DIR/guest"

  # Remove the previous log first. Without this, a mutation that aborts before
  # QEMU starts leaves the last clean log in place and reads as a PASS -- which
  # would let a broken setup masquerade as a working one.
  rm -f "$BUILD_DIR/boot.log"

  docker run --rm --platform linux/arm64 \
    -v "$BUILD_DIR:/out" -v "$mutated:/run.sh:ro" \
    "$RUNNER_IMAGE" bash -euo pipefail -c '
      export DEBIAN_FRONTEND=noninteractive
      apt-get update -qq
      apt-get install -y -qq --no-install-recommends \
        qemu-system-arm attr e2fsprogs cpio gzip file >/dev/null
      bash /run.sh
    ' >/dev/null 2>&1 || true

  local log="$BUILD_DIR/boot.log"
  if [ ! -f "$log" ]; then
    echo "  DETECTED: '$name' was caught by the fixture self-check before boot"
    return 0
  fi
  if grep -qE "^FAIL ${must_fail}:" "$log"; then
    echo "  DETECTED: '$name' correctly failed test '$must_fail'"
    grep -E "^FAIL ${must_fail}:" "$log" | sed 's/^/    /'
    return 0
  fi
  if grep -q "SPIKE_RESULT: BOOT_FAILED" "$log"; then
    echo "  DETECTED: '$name' broke the boot outright (also an honest failure)"
    return 0
  fi
  echo "  NOT DETECTED: '$name' did not fail test '$must_fail' -- that test is inert"
  grep -E "^(PASS|FAIL) ${must_fail}:" "$log" | sed 's/^/    /' || true
  return 1
}

echo "=== harness verification: breaking the setup on purpose ==="
echo

failures=0

# Mutations 1 and 2 change the fixture to a value that still satisfies the
# pre-boot self-check, so the failure has to be caught by the test in the
# guest rather than by the fixture check on the way in.
echo "[1] Name the lower xattr something the test does not read."
echo "    Expect: the xattr test notices user.spike is unreadable."
run_mutation "xattr-wrong-name" \
  's|^setfattr -n user.spike -v lower-xattr-value|setfattr -n user.decoy -v lower-xattr-value|' \
  "xattr" || failures=$((failures + 1))
echo

echo "[2] chown the fixture to the wrong uid/gid (still not root, so the"
echo "    self-check's 'ownership survived' premise is not what catches it)."
echo "    Expect: the uid-gid test notices it is not 1234:5678."
run_mutation "wrong-owner" \
  's|^chown 1234:5678 |chown 4321:8765 |' \
  "uid-gid" || failures=$((failures + 1))
echo

echo "[3] Serve the 9p lower read-WRITE instead of read-only."
echo "    Expect: the negative control notices the lower is writable."
run_mutation "writable-lower" \
  's|,readonly=on||' \
  "negative-control" || failures=$((failures + 1))
echo

echo "[4] Drop the setuid bit from the lower fixture."
echo "    Expect: the setuid-mode test notices the bit never arrived."
run_mutation "no-setuid" \
  's|^chmod 4755 |chmod 0755 |' \
  "setuid-mode" || failures=$((failures + 1))
echo

# Restore a clean staging dir so a later run-spike.sh is not left mutated.
rm -f "$BUILD_DIR/mutated-run.sh"
rm -rf "$BUILD_DIR/guest"

echo "=============================================="
if [ "$failures" -eq 0 ]; then
  echo "harness verification PASSED: every mutation was detected"
  exit 0
fi
echo "harness verification FAILED: $failures mutation(s) went unnoticed"
echo "the corresponding tests are inert and must not be trusted"
exit 1
