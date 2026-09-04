#!/bin/sh
# Overlay-on-9p behavior matrix. Runs as PID 1's child inside the pivoted root.
set -u

. /spike/assert.sh

echo "=============================================="
echo "overlay-on-9p behavior matrix"
echo "=============================================="
info "root mount: $(awk '$2 == "/" { print $1, $3, $4 }' /proc/mounts)"
info "lower (direct): $(awk '$2 ~ /lower/ { print $1, $3 }' /proc/mounts | head -1)"
info "upper (direct): $(awk '$2 ~ /upper/ { print $3 }' /proc/mounts | head -1)"
info "kernel: $(uname -r)"
echo

for suite in /spike/tests/*.sh; do
  echo "########## $(basename "$suite") ##########"
  . "$suite"
  echo
done

echo "=============================================="
echo "SPIKE_SUMMARY pass=$PASS_COUNT fail=$FAIL_COUNT skip=$SKIP_COUNT known=$KNOWN_COUNT"
if [ "$FAIL_COUNT" -eq 0 ] && [ "$PASS_COUNT" -gt 0 ]; then
  echo "SPIKE_RESULT: ALL_PASS"
else
  echo "SPIKE_RESULT: FAILURES"
fi
echo "=============================================="
