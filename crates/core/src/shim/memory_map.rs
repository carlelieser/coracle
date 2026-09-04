//! The guest's `brk` heap and `mmap` arena.
//!
//! A bump allocator over two fixed windows, not a page table. `docs/plan.md`
//! deletes the shim after M2 and the real MMU arrives with the kernel, so the
//! only property that matters is that a static musl binary's allocator gets
//! back distinct, aligned, writable addresses that stay inside guest RAM.
//!
//! `munmap` therefore returns success without reclaiming anything. A process
//! that maps and unmaps in a loop would exhaust the arena; nothing in the M1
//! corpus does.

use crate::guest_memory::PAGE_SIZE;

/// Layout of the two windows the shim hands out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ArenaLayout {
    /// First address `brk` can return.
    pub heap_base: u64,
    /// One past the last address `brk` can return.
    pub heap_limit: u64,
    /// First address `mmap` can return.
    pub mmap_base: u64,
    /// One past the last address `mmap` can return.
    pub mmap_limit: u64,
}

/// The guest's heap and anonymous-mapping arena.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MemoryMap {
    layout: ArenaLayout,
    brk: u64,
    mmap_next: u64,
}

impl MemoryMap {
    /// A map over `layout`, with `brk` at the base of the heap window.
    pub const fn new(layout: ArenaLayout) -> Self {
        Self {
            layout,
            brk: layout.heap_base,
            mmap_next: layout.mmap_base,
        }
    }

    /// The current program break.
    pub const fn brk(&self) -> u64 {
        self.brk
    }

    /// Moves the program break, returning where it ended up.
    ///
    /// Linux's `brk` reports the *current* break on failure rather than an
    /// error, and musl relies on that to detect a refusal, so a request outside
    /// the window leaves the break where it was.
    pub const fn set_brk(&mut self, requested: u64) -> u64 {
        let is_in_window =
            requested >= self.layout.heap_base && requested <= self.layout.heap_limit;
        if is_in_window {
            self.brk = requested;
        }
        self.brk
    }

    /// Reserves `length` bytes, page-aligned, returning the base address.
    ///
    /// `None` when the arena is exhausted, which the caller reports as
    /// `ENOMEM`.
    pub const fn map(&mut self, length: u64) -> Option<u64> {
        if length == 0 {
            return None;
        }
        let pages = length.div_ceil(PAGE_SIZE as u64);
        let Some(size) = pages.checked_mul(PAGE_SIZE as u64) else {
            return None;
        };
        let Some(end) = self.mmap_next.checked_add(size) else {
            return None;
        };
        if end > self.layout.mmap_limit {
            return None;
        }

        let base = self.mmap_next;
        self.mmap_next = end;
        Some(base)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LAYOUT: ArenaLayout = ArenaLayout {
        heap_base: 0x1_0000,
        heap_limit: 0x2_0000,
        mmap_base: 0x10_0000,
        mmap_limit: 0x13_0000,
    };

    #[test]
    fn brk_starts_at_the_base_of_its_window_and_moves_where_asked() {
        let mut map = MemoryMap::new(LAYOUT);

        assert_eq!(map.brk(), LAYOUT.heap_base);
        assert_eq!(
            map.set_brk(LAYOUT.heap_base + 0x1000),
            LAYOUT.heap_base + 0x1000
        );
        assert_eq!(map.brk(), LAYOUT.heap_base + 0x1000);
    }

    #[test]
    fn querying_the_break_with_a_low_address_reports_it_without_moving_it() {
        // musl calls brk(0) to learn the current break.
        let mut map = MemoryMap::new(LAYOUT);
        map.set_brk(LAYOUT.heap_base + 0x800);

        assert_eq!(map.set_brk(0), LAYOUT.heap_base + 0x800);
    }

    #[test]
    fn a_break_beyond_the_window_is_refused_by_reporting_the_old_one() {
        let mut map = MemoryMap::new(LAYOUT);

        let result = map.set_brk(LAYOUT.heap_limit + 1);

        assert_eq!(
            result, LAYOUT.heap_base,
            "refusal reports the current break"
        );
        assert_eq!(map.brk(), LAYOUT.heap_base);
    }

    #[test]
    fn successive_mappings_do_not_overlap_and_are_page_aligned() {
        let mut map = MemoryMap::new(LAYOUT);

        let first = map.map(1).expect("arena has room");
        let second = map.map(PAGE_SIZE as u64 + 1).expect("arena has room");

        assert_eq!(first % PAGE_SIZE as u64, 0);
        assert_eq!(second, first + PAGE_SIZE as u64);
        assert!(map.map(1).expect("still room") >= second + 2 * PAGE_SIZE as u64);
    }

    #[test]
    fn exhausting_the_arena_is_reported_rather_than_wrapping() {
        let mut map = MemoryMap::new(LAYOUT);
        let arena = LAYOUT.mmap_limit - LAYOUT.mmap_base;

        assert!(map.map(arena).is_some());
        assert_eq!(map.map(1), None);
    }

    #[test]
    fn a_zero_length_mapping_is_rejected() {
        let mut map = MemoryMap::new(LAYOUT);

        assert_eq!(map.map(0), None);
    }
}
