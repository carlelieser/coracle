# Shared assertions and layer paths for the overlay test matrix. Sourced, not
# executed. Every check states what it observed; a check that cannot observe
# its precondition reports SKIP, never PASS -- a silently-inert test passing is
# the failure mode this spike exists to avoid.
#
# Output vocabulary, parsed by the host harness:
#   TEST <name>       start of a case
#   PASS/FAIL <name>  verdict, with a reason
#   INFO              evidence line

PASS_COUNT=0
FAIL_COUNT=0
SKIP_COUNT=0
KNOWN_COUNT=0

# The layers are also reachable directly, so a test can distinguish "the
# overlay changed" from "the lower layer changed" -- the latter would be a bug.
LOWER=/layers/lower
UPPER=/layers/upper/upper

pass() { PASS_COUNT=$((PASS_COUNT + 1)); echo "PASS $1: $2"; }
fail() { FAIL_COUNT=$((FAIL_COUNT + 1)); echo "FAIL $1: $2"; }
skip() { SKIP_COUNT=$((SKIP_COUNT + 1)); echo "SKIP $1: $2"; }
# A documented, upstream-known limitation that behaved exactly as predicted.
# Recorded as a design constraint, not counted as a spike failure -- but it
# must still be justified in RESULTS.md, never used to bury a surprise.
known() { KNOWN_COUNT=$((KNOWN_COUNT + 1)); echo "KNOWN $1: $2"; }
info() { echo "INFO   $*"; }

expect_eq() {
  _name=$1; _what=$2; _got=$3; _want=$4
  if [ "$_got" = "$_want" ]; then
    pass "$_name" "$_what is '$_got' as expected"
  else
    fail "$_name" "$_what is '$_got', expected '$_want'"
  fi
}
