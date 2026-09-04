//! How the machine loop yields, and what the host must provide for it.
//!
//! Two shipping configurations, selected by the `threads` feature:
//!
//! - threaded (default): CPU runs in a Web Worker against shared linear memory.
//!   Blocking on an idle guest is a real block (`Atomics.wait`), so the loop
//!   sleeps until a device or timer wakes it.
//! - single-threaded (`--no-default-features`): Safari, or any page that cannot
//!   be cross-origin isolated. `Atomics.wait` is unavailable on the main thread,
//!   so the loop must return to the host and be re-entered.
//!
//! Keeping this a compiled difference rather than a runtime branch is what stops
//! the degraded build from rotting: it fails to compile if a later milestone
//! assumes blocking.

/// What the machine loop does when the guest has nothing to run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdleStrategy {
    /// Block the calling thread until a device signals. Worker only.
    BlockOnAtomic,
    /// Return to the host, which re-enters the loop from a task or timer.
    YieldToHost,
}

/// The idle strategy this build was compiled for.
pub const IDLE_STRATEGY: IdleStrategy = if cfg!(feature = "threads") {
    IdleStrategy::BlockOnAtomic
} else {
    IdleStrategy::YieldToHost
};

/// Whether the host page must be cross-origin isolated for this build to run.
///
/// A threaded build needs `SharedArrayBuffer`, which the browser only exposes
/// under COOP/COEP. The degraded build carries no such requirement, which is the
/// entire reason it exists.
pub const REQUIRES_CROSS_ORIGIN_ISOLATION: bool = cfg!(feature = "threads");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_threaded_build_blocks_and_only_it_needs_isolation() {
        // Both constants derive from the same feature, so this holds by
        // construction; the test exists to fail loudly if a later change gives
        // either one an independent definition. A build that blocked without
        // shared memory would hang, and one that demanded isolation without
        // needing it would lock out Safari for no reason.
        let does_block = IDLE_STRATEGY == IdleStrategy::BlockOnAtomic;

        assert_eq!(does_block, crate::IS_THREADED_BUILD);
        assert_eq!(REQUIRES_CROSS_ORIGIN_ISOLATION, crate::IS_THREADED_BUILD);
    }

    #[test]
    fn the_idle_strategy_is_one_of_the_two_supported_modes() {
        assert!(matches!(
            IDLE_STRATEGY,
            IdleStrategy::BlockOnAtomic | IdleStrategy::YieldToHost
        ));
    }
}
