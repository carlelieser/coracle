# xattrs, ownership, hardlinks, symlinks, persistence.
# Sourced by run-tests.sh; assert.sh is already loaded.

# --------------------------------------------------------------------------
# 8. xattrs. User xattrs must survive on upper files, and be readable from
#    lower files through the merged view.
echo "TEST xattr"
# 8a: read an xattr set on the lower layer at image build time.
XA_LOWER=$(getfattr -n user.spike --only-values /fixtures/hello.txt 2>/dev/null || echo UNREADABLE)
info "xattr read from lower via merged view: '$XA_LOWER'"

# 8b: set and read back on the upper layer.
echo xattr-target > /work/xattr.txt
XA_SET_ERR=$(setfattr -n user.testkey -v testvalue /work/xattr.txt 2>&1)
XA_SET_RC=$?
XA_READ=$(getfattr -n user.testkey --only-values /work/xattr.txt 2>/dev/null || echo UNREADABLE)
info "xattr set rc=$XA_SET_RC err='$XA_SET_ERR' readback='$XA_READ'"

if [ "$XA_SET_RC" -ne 0 ]; then
  fail xattr "setfattr on upper failed: $XA_SET_ERR"
elif [ "$XA_READ" != "testvalue" ]; then
  fail xattr "xattr readback on upper was '$XA_READ', expected 'testvalue'"
elif [ "$XA_LOWER" = "UNREADABLE" ]; then
  fail xattr "upper xattrs work but lower xattr was not readable through the overlay"
else
  pass xattr "set/get on upper works and lower xattr '$XA_LOWER' readable through overlay"
fi
echo

# --------------------------------------------------------------------------
# 9. uid/gid preservation. Both on the lower layer as served by 9p, and
#    across a copy-up, which must not silently reset ownership to root.
echo "TEST uid-gid"
UG_LOWER=$(stat -c '%u:%g' /fixtures/owned.txt 2>/dev/null || echo ERR)
info "lower-only file ownership through overlay: $UG_LOWER"
# Touching it forces a copy-up; ownership must survive.
echo more >> /fixtures/owned.txt
UG_AFTER=$(stat -c '%u:%g' /fixtures/owned.txt 2>/dev/null || echo ERR)
UG_UPPER=$(stat -c '%u:%g' "$UPPER/fixtures/owned.txt" 2>/dev/null || echo ABSENT)
info "after copy-up, ownership through overlay: $UG_AFTER (upper file: $UG_UPPER)"

if [ "$UG_LOWER" != "1234:5678" ]; then
  fail uid-gid "9p did not preserve ownership on the lower layer: got $UG_LOWER, expected 1234:5678"
elif [ "$UG_AFTER" != "1234:5678" ]; then
  fail uid-gid "copy-up reset ownership: was $UG_LOWER, now $UG_AFTER"
elif [ "$UG_UPPER" != "1234:5678" ]; then
  fail uid-gid "copy-up wrote the wrong owner to the upper layer: $UG_UPPER"
else
  pass uid-gid "ownership 1234:5678 preserved through 9p and across copy-up"
fi
echo

# The setuid bit is a separate case from ownership, and it needs both halves
# asserted: that 9p delivered it at all, and that copy-up did not drop it.
# Checking only "before == after" passes on a layer that never had it.
echo "TEST setuid-mode"
MODE_BEFORE=$(stat -c '%a' /fixtures/setuid.bin 2>/dev/null || echo ERR)
echo x >> /fixtures/setuid.bin 2>/dev/null
MODE_AFTER=$(stat -c '%a' /fixtures/setuid.bin 2>/dev/null || echo ERR)
info "setuid file mode: before=$MODE_BEFORE after copy-up=$MODE_AFTER (want 4755 both)"
if [ "$MODE_BEFORE" != "4755" ]; then
  fail setuid-mode "9p did not deliver the setuid bit: lower mode reads $MODE_BEFORE, expected 4755"
elif [ "$MODE_AFTER" != "4755" ]; then
  fail setuid-mode "copy-up dropped mode bits: $MODE_BEFORE -> $MODE_AFTER"
else
  pass setuid-mode "setuid bit survives 9p transport and copy-up (mode 4755)"
fi
echo

# --------------------------------------------------------------------------
# 10. Hardlinks. Two cases, and they behave differently.
#     (a) links created in the upper layer must share an inode and content.
#     (b) a hardlink pair in the LOWER layer is broken by copy-up unless
#         overlayfs index=on. The plan needs the real answer here.
echo "TEST hardlink-upper"
echo hl > /work/hl-a.txt
ln /work/hl-a.txt /work/hl-b.txt 2>/dev/null
HL_RC=$?
if [ "$HL_RC" -ne 0 ]; then
  fail hardlink-upper "ln failed on the upper layer (rc=$HL_RC)"
else
  HL_INO_A=$(stat -c '%i' /work/hl-a.txt)
  HL_INO_B=$(stat -c '%i' /work/hl-b.txt)
  HL_NLINK=$(stat -c '%h' /work/hl-a.txt)
  info "inodes a=$HL_INO_A b=$HL_INO_B nlink=$HL_NLINK"
  echo changed > /work/hl-a.txt
  HL_B_SEES=$(cat /work/hl-b.txt)
  info "wrote through a, b reads: '$HL_B_SEES'"
  if [ "$HL_INO_A" != "$HL_INO_B" ]; then
    fail hardlink-upper "links have different inodes ($HL_INO_A vs $HL_INO_B)"
  elif [ "$HL_NLINK" -ne 2 ]; then
    fail hardlink-upper "nlink is $HL_NLINK, expected 2"
  elif [ "$HL_B_SEES" != "changed" ]; then
    fail hardlink-upper "write through one link not visible via the other"
  else
    pass hardlink-upper "shared inode, nlink=2, writes visible through both links"
  fi
fi
echo

echo "TEST hardlink-lower-copyup"
# The fixture has link-a.txt and link-b.txt as one inode in the lower layer.
HLL_INO_A=$(stat -c '%i' /fixtures/link-a.txt 2>/dev/null || echo ERR)
HLL_INO_B=$(stat -c '%i' /fixtures/link-b.txt 2>/dev/null || echo ERR)
HLL_NLINK=$(stat -c '%h' /fixtures/link-a.txt 2>/dev/null || echo ERR)
info "before copy-up: inode a=$HLL_INO_A b=$HLL_INO_B nlink=$HLL_NLINK"
if [ "$HLL_INO_A" != "$HLL_INO_B" ]; then
  info "NOTE: 9p lower does not present the pair as one inode"
fi
echo copyup >> /fixtures/link-a.txt
HLL_A_AFTER=$(stat -c '%i' /fixtures/link-a.txt)
HLL_B_AFTER=$(stat -c '%i' /fixtures/link-b.txt)
HLL_B_CONTENT=$(cat /fixtures/link-b.txt)
info "after copy-up of a: inode a=$HLL_A_AFTER b=$HLL_B_AFTER"
info "b content after writing through a: '$(echo "$HLL_B_CONTENT" | tr '\n' '|')'"
if [ "$(cat "$LOWER/fixtures/link-b.txt")" != "linked-content" ]; then
  fail hardlink-lower-copyup "copy-up mutated the LOWER layer"
elif [ "$HLL_A_AFTER" = "$HLL_B_AFTER" ]; then
  pass hardlink-lower-copyup "link preserved across copy-up (overlay index working)"
else
  # Predicted from source: 9p exposes no export_op, so ovl_can_decode_fh()
  # returns 0 and overlayfs forces index=off; without the index, copy-up
  # cannot rejoin the pair. Only accept this if the kernel actually said so --
  # otherwise it is an unexplained break and must fail.
  if dmesg 2>/dev/null | grep -q "does not support file handles"; then
    known hardlink-lower-copyup \
      "copy-up broke the lower hardlink pair (a=$HLL_A_AFTER b=$HLL_B_AFTER); expected, kernel forced index=off over 9p"
  else
    fail hardlink-lower-copyup \
      "copy-up broke the lower hardlink pair (a=$HLL_A_AFTER b=$HLL_B_AFTER) with no index=off warning in dmesg"
  fi
fi
echo

# --------------------------------------------------------------------------
# 11. Symlinks through the lower layer.
echo "TEST symlink"
SL_TARGET=$(readlink /fixtures/link-to-hello 2>/dev/null || echo ERR)
SL_CONTENT=$(cat /fixtures/link-to-hello 2>/dev/null || echo ERR)
info "symlink target='$SL_TARGET' resolves to content '$(echo "$SL_CONTENT" | tr '\n' '|')'"
if [ "$SL_TARGET" = "ERR" ]; then
  fail symlink "readlink failed through the overlay"
elif [ "$SL_CONTENT" = "ERR" ]; then
  fail symlink "symlink did not resolve to readable content"
else
  pass symlink "symlink readable and resolvable through overlay on 9p lower"
fi
echo

# --------------------------------------------------------------------------
# 12. Persistence of the upper layer. Writes must be on ext4, not tmpfs, so
#    they survive. Verified here by confirming the data is on the block
#    device; the host harness re-boots and checks it is still there.
echo "TEST persistence-marker"
echo "persist-$(date +%s 2>/dev/null || echo fixed)" > /work/persist.txt
sync
if [ -f "$UPPER/work/persist.txt" ]; then
  pass persistence-marker "written through overlay and present on the ext4 upper: $(cat "$UPPER/work/persist.txt")"
else
  fail persistence-marker "write did not land on the ext4 upper layer"
fi
echo

