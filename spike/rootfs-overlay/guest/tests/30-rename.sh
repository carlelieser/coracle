# Rename within the upper layer and across the overlay boundary.
# Sourced by run-tests.sh; assert.sh is already loaded.

# --------------------------------------------------------------------------
# 5. Rename within the upper layer. Plain POSIX rename, must be atomic-ish
#    and preserve content.
echo "TEST rename-upper"
mkdir -p /work
echo rename-payload > /work/src.txt
mv /work/src.txt /work/dst.txt
if [ -e /work/src.txt ]; then
  fail rename-upper "source still exists after mv"
elif [ ! -f /work/dst.txt ]; then
  fail rename-upper "destination missing after mv"
else
  expect_eq rename-upper "renamed content" "$(cat /work/dst.txt)" "rename-payload"
fi
echo

# --------------------------------------------------------------------------
# 6. Rename across the overlay boundary: a file that lives only in the lower
#    layer, renamed. This is the interesting case -- overlayfs must copy it up
#    and whiteout the original. It is also where a lower filesystem lacking
#    features (no xattr / no stable inode) makes overlayfs return EXDEV.
echo "TEST rename-cross-layer"
RC_CONTENT_BEFORE=$(cat /fixtures/movable.txt 2>/dev/null || echo MISSING)
info "lower-only file content: '$RC_CONTENT_BEFORE'"
MV_ERR=$(mv /fixtures/movable.txt /work/moved.txt 2>&1)
MV_RC=$?
if [ "$MV_RC" -ne 0 ]; then
  fail rename-cross-layer "mv failed rc=$MV_RC: $MV_ERR"
elif [ -e /fixtures/movable.txt ]; then
  fail rename-cross-layer "source still visible in merged view after cross-layer mv"
elif [ ! -f "$LOWER/fixtures/movable.txt" ]; then
  fail rename-cross-layer "lower layer was mutated by the rename"
elif [ "$(cat /work/moved.txt)" != "$RC_CONTENT_BEFORE" ]; then
  fail rename-cross-layer "content changed: got '$(cat /work/moved.txt)'"
else
  pass rename-cross-layer "lower-only file copied up and moved, lower intact, content preserved"
fi
echo

# --------------------------------------------------------------------------
# 7. Directory rename. Without redirect_dir this returns EXDEV for a
#    lower-only directory; the plan needs to know which behavior we get.
echo "TEST rename-dir-cross-layer"
DR_ERR=$(mv /fixtures/movedir /work/moveddir 2>&1)
DR_RC=$?
if [ "$DR_RC" -ne 0 ]; then
  fail rename-dir-cross-layer "mv of lower-only dir failed rc=$DR_RC: $DR_ERR (redirect_dir off?)"
elif [ ! -f /work/moveddir/inside.txt ]; then
  fail rename-dir-cross-layer "directory moved but its contents are missing"
else
  REDIR=$(getfattr -n trusted.overlay.redirect -d "$UPPER/work/moveddir" 2>/dev/null | grep -c redirect || true)
  info "redirect xattr present on moved dir: $REDIR"
  pass rename-dir-cross-layer "lower-only directory renamed with contents intact"
fi
echo

