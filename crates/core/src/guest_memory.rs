//! Placement of guest RAM inside the module's exported linear memory.
//!
//! Guest RAM is a single contiguous region at a fixed offset, not a `Vec`. M5
//! JIT modules import the same `WebAssembly.Memory` and inline
//! `guest_physical + RAM_BASE` as a constant displacement, so the base must be
//! knowable without consulting a runtime allocator.

/// Byte offset of guest physical address 0 within the wasm linear memory.
///
/// Everything below this belongs to the Rust allocator and static data. The
/// linker is told to keep the module well under this via `--max-memory` and the
/// stack/heap layout; `reserve` performs the check that turns a layout mistake
/// into a startup error rather than silent guest corruption.
pub const RAM_BASE: usize = 1 << 30;

/// Largest guest RAM the 32-bit address space leaves room for.
///
/// wasm32 caps linear memory at 4 GiB, and `usize::MAX` there is one byte short
/// of it, so the 4 GiB total is computed in `u64` and only the remainder — which
/// does fit — is narrowed.
pub const MAX_RAM_BYTES: usize = ((4u64 << 30) - RAM_BASE as u64) as usize;

/// Default guest RAM, per the plan's fixed design decisions.
pub const DEFAULT_RAM_BYTES: usize = 1 << 30;

/// Guest physical base address of RAM on the QEMU `virt` machine.
///
/// Guest physical `PHYS_RAM_BASE + n` lives at linear-memory byte
/// `RAM_BASE + n`. The machine spec owns this value; it is repeated here only
/// so the offset arithmetic has one definition.
pub const PHYS_RAM_BASE: u64 = 0x4000_0000;

/// Why a requested guest RAM size cannot be placed in linear memory.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LayoutError {
    /// Requested size exceeds what the wasm32 address space leaves after
    /// `RAM_BASE`.
    TooLarge { requested: usize, max: usize },
    /// Requested size is not a whole number of 4 KiB guest pages.
    Unaligned { requested: usize },
    /// Requested size is zero.
    Empty,
}

/// Guest page size. 4 KiB granule only, per the plan's fixed decisions.
pub const PAGE_SIZE: usize = 4096;

/// Validates a guest RAM size and reports the linear-memory range it occupies.
///
/// Returns the half-open byte range `[start, end)` within linear memory.
pub fn reserve(ram_bytes: usize) -> Result<(usize, usize), LayoutError> {
    if ram_bytes == 0 {
        return Err(LayoutError::Empty);
    }
    if !ram_bytes.is_multiple_of(PAGE_SIZE) {
        return Err(LayoutError::Unaligned {
            requested: ram_bytes,
        });
    }
    if ram_bytes > MAX_RAM_BYTES {
        return Err(LayoutError::TooLarge {
            requested: ram_bytes,
            max: MAX_RAM_BYTES,
        });
    }
    Ok((RAM_BASE, RAM_BASE + ram_bytes))
}

/// Translates a guest physical address to a linear-memory byte offset.
///
/// Returns `None` when the address falls outside the configured RAM window;
/// MMIO addresses are below `PHYS_RAM_BASE` and are dispatched elsewhere.
pub fn linear_offset(guest_physical: u64, ram_bytes: usize) -> Option<usize> {
    let offset_in_ram = guest_physical.checked_sub(PHYS_RAM_BASE)?;
    if offset_in_ram >= ram_bytes as u64 {
        return None;
    }
    Some(RAM_BASE + offset_in_ram as usize)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_ram_reserves_a_page_aligned_window_above_the_rust_heap() {
        let (start, end) = reserve(DEFAULT_RAM_BYTES).expect("1 GiB must fit");

        assert_eq!(start, RAM_BASE);
        assert_eq!(end - start, DEFAULT_RAM_BYTES);
        assert_eq!(end, 2 << 30);
    }

    #[test]
    fn ram_beyond_the_wasm32_address_space_is_rejected() {
        let too_big = MAX_RAM_BYTES + PAGE_SIZE;

        assert_eq!(
            reserve(too_big),
            Err(LayoutError::TooLarge {
                requested: too_big,
                max: MAX_RAM_BYTES,
            })
        );
    }

    #[test]
    fn unaligned_and_empty_sizes_are_rejected() {
        assert_eq!(
            reserve(PAGE_SIZE + 1),
            Err(LayoutError::Unaligned {
                requested: PAGE_SIZE + 1
            })
        );
        assert_eq!(reserve(0), Err(LayoutError::Empty));
    }

    #[test]
    fn guest_physical_maps_to_a_fixed_displacement_from_ram_base() {
        assert_eq!(
            linear_offset(PHYS_RAM_BASE, DEFAULT_RAM_BYTES),
            Some(RAM_BASE)
        );
        assert_eq!(
            linear_offset(PHYS_RAM_BASE + 0x1234, DEFAULT_RAM_BYTES),
            Some(RAM_BASE + 0x1234)
        );
    }

    #[test]
    fn addresses_outside_the_ram_window_do_not_translate() {
        // MMIO region on the `virt` machine sits below RAM.
        assert_eq!(linear_offset(0x0900_0000, DEFAULT_RAM_BYTES), None);
        // One byte past the end of configured RAM.
        assert_eq!(
            linear_offset(PHYS_RAM_BASE + DEFAULT_RAM_BYTES as u64, DEFAULT_RAM_BYTES),
            None
        );
    }
}
