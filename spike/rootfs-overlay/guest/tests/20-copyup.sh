# Copy-up, whiteouts, and opaque directories.
# Sourced by run-tests.sh; assert.sh is already loaded.

# --------------------------------------------------------------------------
# 2. Copy-up. Modifying a lower file must materialize it in upper, leave the
#    lower byte-identical, and preserve the original content plus the change.
echo "TEST copy-up"
LOWER_BEFORE=$(cat "$LOWER/fixtures/hello.txt")
info "lower content before: '$LOWER_BEFORE'"
info "upper before copy-up: $(ls "$UPPER/fixtures/hello.txt" 2>&1 || echo 'absent (expected)')"

echo "appended-by-guest" >> /fixtures/hello.txt

MERGED_AFTER=$(cat /fixtures/hello.txt)
LOWER_AFTER=$(cat "$LOWER/fixtures/hello.txt")
info "merged content after: '$(echo "$MERGED_AFTER" | tr '\n' '|')'"
info "lower content after:  '$LOWER_AFTER'"

if [ ! -f "$UPPER/fixtures/hello.txt" ]; then
  fail copy-up "file was not materialized in the upper layer"
elif [ "$LOWER_AFTER" != "$LOWER_BEFORE" ]; then
  fail copy-up "lower layer was MUTATED (was '$LOWER_BEFORE', now '$LOWER_AFTER')"
elif ! echo "$MERGED_AFTER" | grep -q "appended-by-guest"; then
  fail copy-up "append not visible in merged view"
elif ! echo "$MERGED_AFTER" | grep -q "$LOWER_BEFORE"; then
  fail copy-up "original lower content lost during copy-up"
else
  pass copy-up "materialized in upper, lower untouched, content preserved + appended"
fi
echo

# --------------------------------------------------------------------------
# 3. Whiteout. Deleting a lower-only file must hide it in the merged view,
#    leave the lower intact, and record a whiteout in the upper.
echo "TEST whiteout"
rm -f /fixtures/deleteme.txt
WO_LOWER_STILL=$([ -f "$LOWER/fixtures/deleteme.txt" ] && echo yes || echo no)
WO_MERGED_GONE=$([ -e /fixtures/deleteme.txt ] && echo no || echo yes)
# A whiteout is a char device 0:0 in the upper layer.
WO_MARKER=$(find "$UPPER/fixtures" -maxdepth 1 -name deleteme.txt 2>/dev/null | head -1)
if [ -n "$WO_MARKER" ]; then
  info "upper whiteout marker: $(ls -l "$WO_MARKER")"
fi
info "still in lower: $WO_LOWER_STILL / hidden in merged: $WO_MERGED_GONE"
# Also prove it stays hidden through a readdir, not just a stat.
WO_IN_LISTING=$(ls /fixtures/ | grep -c '^deleteme.txt$' || true)
info "occurrences in merged readdir: $WO_IN_LISTING"

if [ "$WO_MERGED_GONE" != "yes" ]; then
  fail whiteout "file still visible in merged view after rm"
elif [ "$WO_LOWER_STILL" != "yes" ]; then
  fail whiteout "rm deleted the file from the LOWER layer"
elif [ "$WO_IN_LISTING" -ne 0 ]; then
  fail whiteout "file hidden from stat but still present in readdir"
elif [ -z "$WO_MARKER" ]; then
  fail whiteout "no whiteout marker recorded in upper"
else
  pass whiteout "hidden from stat and readdir, lower intact, marker present"
fi
echo

# --------------------------------------------------------------------------
# 4. Opaque directory. Removing a whole lower directory then recreating it
#    must not resurrect the lower entries.
echo "TEST opaque-dir"
LOWER_SUBDIR_COUNT=$(ls "$LOWER/fixtures/subdir" 2>/dev/null | wc -l)
info "lower subdir holds $LOWER_SUBDIR_COUNT entries"
rm -rf /fixtures/subdir
mkdir -p /fixtures/subdir
echo new > /fixtures/subdir/fresh.txt
MERGED_SUBDIR=$(ls /fixtures/subdir | tr '\n' ' ')
info "merged subdir after recreate: '$MERGED_SUBDIR'"
if [ "$LOWER_SUBDIR_COUNT" -lt 1 ]; then
  skip opaque-dir "lower subdir fixture was empty; nothing to shadow"
elif echo "$MERGED_SUBDIR" | grep -qE 'lower-a|lower-b'; then
  fail opaque-dir "lower entries resurfaced after rmdir+mkdir (directory not opaque)"
elif echo "$MERGED_SUBDIR" | grep -q fresh.txt; then
  pass opaque-dir "recreated directory is opaque; only new entry visible"
else
  fail opaque-dir "new entry missing from recreated directory"
fi
echo

