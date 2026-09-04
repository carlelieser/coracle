#!/usr/bin/env bash
# M0 gate: kernel + initramfs boot to a shell under QEMU `virt` on our own DTB,
# with a clean dmesg. Driven non-interactively so CI can run it.
#
# Invoked by `boot.sh --gate`, which supplies the QEMU argument list.

set -uo pipefail

KERNEL_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
OUT_DIR="$KERNEL_DIR/out"
TRANSCRIPT="$OUT_DIR/gate-transcript.txt"

# The guest boots rdinit=/gate-init, which self-checks and powers off, so this
# needs no interaction — only a guard against a boot that never terminates.
BOOT_TIMEOUT_SECONDS=180

qemu-system-aarch64 "$@" </dev/null >"$TRANSCRIPT" 2>&1 &
qemu_pid=$!

# Polls rather than sleeping the full timeout, so a guest that dies immediately
# (a bad QEMU invocation) reports in a second instead of blocking for minutes.
(
	waited=0
	while [ "$waited" -lt "$BOOT_TIMEOUT_SECONDS" ]; do
		kill -0 "$qemu_pid" 2>/dev/null || exit 0
		sleep 1
		waited=$((waited + 1))
	done
	if kill -0 "$qemu_pid" 2>/dev/null; then
		echo "gate.sh: guest did not power off within ${BOOT_TIMEOUT_SECONDS}s" >&2
		kill -9 "$qemu_pid" 2>/dev/null
	fi
) &
watchdog_pid=$!

wait "$qemu_pid"
qemu_status=$?
kill "$watchdog_pid" 2>/dev/null
wait "$watchdog_pid" 2>/dev/null

echo "transcript: $TRANSCRIPT"

# A guest that never ran produces an empty transcript, and every absence-based
# check below ("no oops", "no call trace") would then pass on nothing. Refuse
# to grade a run that did not happen.
if [ "$qemu_status" -ne 0 ] || ! grep -q "Booting Linux on physical CPU" "$TRANSCRIPT"; then
	echo
	echo "  QEMU did not boot the guest (exit $qemu_status). Transcript:"
	sed 's/^/    /' "$TRANSCRIPT"
	echo
	echo "M0 GATE: FAIL (no boot to grade)"
	exit 1
fi

fail=0
check() {
	local label="$1" pattern="$2"
	if grep -qE "$pattern" "$TRANSCRIPT"; then
		echo "  PASS  $label"
	else
		echo "  FAIL  $label"
		fail=1
	fi
}

refute() {
	local label="$1" pattern="$2"
	local hits
	hits="$(grep -cE "$pattern" "$TRANSCRIPT")"
	if [ "$hits" -eq 0 ]; then
		echo "  PASS  $label"
	else
		echo "  FAIL  $label ($hits occurrence(s))"
		grep -nE "$pattern" "$TRANSCRIPT" | head -10 | sed 's/^/          /'
		fail=1
	fi
}

echo
echo "M0 gate: kernel + initramfs boot to shell on the emulator's DTB"
echo

check  "kernel booted"                '^\[ *[0-9.]+\] Booting Linux on physical CPU'
check  "our DTB is in use"            'Machine model: coracle,virt'
check  "PSCI over SMC"                'psci: PSCIv[0-9.]+ detected in firmware|psci: Using standard PSCI v0.2'
check  "PL011 console claimed"        'ttyAMA0.*(PL011|pl011)'
check  "initramfs unpacked"           'Freeing initrd memory|Unpacking initramfs'
check  "reached userspace"            'Run /gate-init as init process'
check  "shell is alive"               'GATE:shell-alive 2'
check  "running as root"              'GATE:root-uid 0'
check  "fork+exec through a pipe"     'GATE:pipeline 3'
check  "proc mounted"                 'GATE:mount proc on /proc'
check  "sysfs mounted"                'GATE:mount sysfs on /sys'
check  "devtmpfs mounted"             'GATE:mount devtmpfs on /dev'
check  "gate script ran to the end"   'GATE:done'
check  "PSCI poweroff worked"         'reboot: Power down|System halted'

echo
refute "no oops or panic"             'Oops|kernel BUG|Unable to handle kernel|Internal error|Kernel panic'
refute "no call trace"                'Call trace:|BUG: |WARNING: '
refute "no undefined instruction"     'undefined instruction|Illegal instruction|SIGILL'
refute "no unhandled fault"           'Unhandled fault|Synchronous Abort|serror'

# dmesg lines the kernel itself tags as a problem. The initcall/deferred-probe
# noise QEMU's own DTB produces is exactly what our trimmed DTB should avoid.
refute "no driver probe failures"      'probe.*failed|failed to probe|Failed to register'

echo
if [ "$fail" -eq 0 ]; then
	echo "M0 GATE: PASS"
else
	echo "M0 GATE: FAIL"
fi
exit "$fail"
