//! virtio-mmio devices, GICv2, generic timer, PL011, PL031.
//!
//! Skeleton only. Device implementations land in M2 (GIC, timer, PL011, PL031)
//! and M3 (virtio). This crate depends on `coracle-core` and on nothing else in
//! the workspace.

#![no_std]

extern crate alloc;

/// How a device signals the CPU that it needs attention.
///
/// The threaded build wakes a blocked worker; the degraded build sets a flag the
/// host polls when it re-enters the machine loop. Devices are written against
/// this enum rather than against `Atomics.notify` directly, so the degraded
/// build does not need a parallel device tree.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WakeMechanism {
    /// `Atomics.notify` on the shared interrupt word.
    NotifyAtomic,
    /// Set a pending bit; the host observes it on the next loop entry.
    SetPendingFlag,
}

/// The wake mechanism this build was compiled for.
pub const WAKE_MECHANISM: WakeMechanism = if cfg!(feature = "threads") {
    WakeMechanism::NotifyAtomic
} else {
    WakeMechanism::SetPendingFlag
};

#[cfg(test)]
mod tests {
    use super::*;
    use coracle_core::threading::{IdleStrategy, IDLE_STRATEGY};

    #[test]
    fn the_wake_mechanism_matches_how_the_cpu_idles() {
        // A device that notifies an atomic while the CPU yields to the host
        // would drop interrupts; a device that sets a flag while the CPU blocks
        // would hang. The two must be chosen by the same feature.
        let expected = match IDLE_STRATEGY {
            IdleStrategy::BlockOnAtomic => WakeMechanism::NotifyAtomic,
            IdleStrategy::YieldToHost => WakeMechanism::SetPendingFlag,
        };

        assert_eq!(WAKE_MECHANISM, expected);
    }
}
