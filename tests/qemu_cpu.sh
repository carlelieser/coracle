#!/usr/bin/env bash
# Single source of truth for the QEMU pin and the advertised CPU feature mask.
# Sourced by every harness script so no run can drift from the plan's §2 mask.
#
# Feature mask (docs/plan.md §2): ARMv8.0-A, AArch64 only, EL0/EL1 only, and
# explicitly NO LSE, SVE, PAuth, MTE, crypto, or BTI.

# Pinned QEMU version. A different version is a hard error, not a warning:
# TCG behaviour and the plugin register list both move between releases.
CORACLE_QEMU_VERSION="11.1.1"

# cortex-a53 is an ARMv8.0-A implementation: no LSE, SVE, PAuth, MTE or BTI,
# all verified empirically by ./verify_feature_mask.sh.
#
# Why cortex-a53 and not `max`: `max` advertises the full modern feature set
# (LSE, SVE, PAuth, MTE, crypto, BTI, and every v8.1+ extension), so every one
# would have to be turned off by name, and any newly-added default in a later
# QEMU would silently re-enable ISA surface we do not implement. Starting from
# a v8.0 core makes the mask default-closed.
#
# CRYPTO CAVEAT: QEMU 11.1.1 has no property to disable FEAT_AES/SHA on any
# CPU model -- crypto is baked into every AArch64 model's ID registers. So the
# oracle advertises crypto that the emulator will not implement. This is
# contained rather than ignored:
#   - verify_feature_mask.sh asserts crypto is the ONLY excluded feature that
#     QEMU still advertises, so the gap cannot silently widen.
#   - The M1 fuzz corpus must not draw crypto encodings (they are excluded from
#     the mask by the plan, so they are out of the corpus by construction).
#   - Guest code that probes ID_AA64ISAR0_EL1 and takes a crypto path would
#     diverge. Nothing in the M0-M2 scope does: the kernel checks the same ID
#     register the emulator controls, so the emulator's own ID_AA64ISAR0_EL1
#     value -- not QEMU's -- determines what the guest attempts.
CORACLE_QEMU_CPU="cortex-a53,aarch64=on,pmu=off"

# Comma-free identity for the mask, hashed into the trace header. Both
# producers must agree on this string or the differ refuses to compare.
# Bump it whenever CORACLE_QEMU_CPU changes.
CORACLE_FEATURE_ID="armv8.0-a+el1+nolse+nosve+nopauth+nomte+nobti+qemucrypto"

# Machine model per plan §2: clone of QEMU `virt`, GICv2, single vCPU.
CORACLE_QEMU_MACHINE="virt,gic-version=2,virtualization=off,secure=off"

CORACLE_QEMU_BIN="${CORACLE_QEMU_BIN:-qemu-system-aarch64}"

coracle_check_qemu() {
    if ! command -v "$CORACLE_QEMU_BIN" >/dev/null 2>&1; then
        echo "error: $CORACLE_QEMU_BIN not found on PATH" >&2
        return 1
    fi
    local found
    found=$("$CORACLE_QEMU_BIN" --version | head -1 | sed 's/.*version \([0-9.]*\).*/\1/')
    if [ "$found" != "$CORACLE_QEMU_VERSION" ]; then
        echo "error: QEMU $found found, harness pins $CORACLE_QEMU_VERSION" >&2
        echo "       traces from a different build are not comparable" >&2
        return 1
    fi
    return 0
}
