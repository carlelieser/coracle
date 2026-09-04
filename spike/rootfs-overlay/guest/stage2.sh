#!/bin/sh
# PID 1 after switch_root, running on the overlay root.
set -u

mount -t proc proc /proc 2>/dev/null
mount -t sysfs sysfs /sys 2>/dev/null
mount -t devtmpfs devtmpfs /dev 2>/dev/null

# Prove we are actually running on the overlay, not still on the initramfs.
# Every result below is meaningless if this is not true.
ROOT_FSTYPE=$(awk '$2 == "/" { print $3 }' /proc/mounts)
echo "stage2: / is fstype=$ROOT_FSTYPE"
if [ "$ROOT_FSTYPE" != "overlay" ]; then
  echo "SPIKE_RESULT: BOOT_FAILED (root is $ROOT_FSTYPE, expected overlay)"
  poweroff -f
  sleep 100
fi

echo "SPIKE_BOOT: OK root=overlay"

/spike/run-tests.sh
echo "stage2: tests finished"
poweroff -f
sleep 100
