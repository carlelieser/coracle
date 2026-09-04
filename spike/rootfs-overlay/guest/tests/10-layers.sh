# Preconditions, 9p readdir type reporting, and the negative control.
# Sourced by run-tests.sh; assert.sh is already loaded.

# --------------------------------------------------------------------------
# 0. Preconditions. If these fail, later PASSes would be meaningless.
echo "TEST precondition"
if [ ! -d "$LOWER" ]; then
  fail precondition "lower layer not reachable at $LOWER; every result below is untrustworthy"
elif [ ! -d "$UPPER" ]; then
  fail precondition "upper layer not reachable at $UPPER"
elif [ ! -f /fixtures/hello.txt ]; then
  fail precondition "lower fixtures missing from overlay root"
else
  pass precondition "lower, upper, and merged view all reachable"
fi
echo

# --------------------------------------------------------------------------
# 1. readdir d_type. Overlayfs needs the lower layer to report file types in
#    readdir; a lower that returns DT_UNKNOWN for everything forces overlayfs
#    into extra lookups and historically broke readdir merging.
echo "TEST d_type"
# Shell cannot read d_type out of getdents directly, so this checks the two
# things that are observable and that d_type actually governs.
#
# (a) The merged readdir must classify a mixed directory correctly. The
#     fixtures dir holds regular files, a subdirectory and a symlink.
DT_DIRS=$(find "$LOWER/fixtures" -maxdepth 1 -type d | wc -l)
DT_FILES=$(find "$LOWER/fixtures" -maxdepth 1 -type f | wc -l)
DT_LINKS=$(find "$LOWER/fixtures" -maxdepth 1 -type l | wc -l)
info "9p lower /fixtures: dirs=$DT_DIRS files=$DT_FILES symlinks=$DT_LINKS"

# (b) The load-bearing one. fs/overlayfs/readdir.c:170 collects whiteout
#     candidates with `if (d_type == DT_CHR)`. A layer whose readdir returns
#     DT_UNKNOWN never enters that list, so whiteouts on it stop working.
#     The whiteout case in 20-copyup.sh is therefore the functional probe;
#     here we only record whether the char-device type survives readdir at all.
DT_WHITEOUT_SEEN=$(ls -l "$UPPER" 2>/dev/null | grep -c '^c' || true)
info "char-device entries visible in upper readdir: $DT_WHITEOUT_SEEN (whiteouts, if any yet)"

if [ "$DT_DIRS" -lt 2 ]; then
  fail d_type "no subdirectory found under /fixtures; fixture tree is wrong, not a d_type result"
elif [ "$DT_FILES" -lt 1 ]; then
  fail d_type "9p readdir surfaced no regular files under /fixtures"
elif [ "$DT_LINKS" -lt 1 ]; then
  fail d_type "9p readdir surfaced no symlink under /fixtures"
else
  pass d_type "9p lower distinguishes dirs, regular files and symlinks in readdir"
fi
echo

# --------------------------------------------------------------------------
# 13. Negative control. Proves the harness can actually observe a failure.
#    The lower layer is read-only; writing directly to it MUST fail. If this
#    "passes" (i.e. the write succeeds) every other result is suspect.
echo "TEST negative-control"
NC_ERR=$(touch "$LOWER/should-not-be-writable" 2>&1)
NC_RC=$?
info "direct write to 9p lower: rc=$NC_RC err='$NC_ERR'"
if [ "$NC_RC" -eq 0 ]; then
  fail negative-control "the 9p lower layer was WRITABLE; it must be read-only for this spike to mean anything"
else
  pass negative-control "lower layer correctly rejects direct writes"
fi
echo
