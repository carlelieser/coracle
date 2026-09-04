//! QEMU `virt` machine model: wiring, device tree, machine loop.
//!
//! Skeleton only. The machine spec (memory map, IRQ map, MMIO addresses, PSCI,
//! DTS) is a separate document; this crate implements it from M2 onward.

#![no_std]

extern crate alloc;

use coracle_core::guest_memory::{self, LayoutError};

/// Machine configuration settled before the guest starts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MachineConfig {
    /// Guest RAM in bytes. Must be 4 KiB aligned and fit the linear-memory
    /// window; see [`coracle_core::guest_memory`].
    pub ram_bytes: usize,
}

impl Default for MachineConfig {
    fn default() -> Self {
        Self {
            ram_bytes: guest_memory::DEFAULT_RAM_BYTES,
        }
    }
}

impl MachineConfig {
    /// Checks the configuration against the linear-memory layout.
    ///
    /// Returns the half-open linear-memory byte range guest RAM will occupy.
    pub fn validate(&self) -> Result<(usize, usize), LayoutError> {
        guest_memory::reserve(self.ram_bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_machine_is_a_valid_configuration() {
        let (start, end) = MachineConfig::default()
            .validate()
            .expect("the default machine must be constructible");

        assert_eq!(end - start, guest_memory::DEFAULT_RAM_BYTES);
    }

    #[test]
    fn a_machine_asking_for_more_ram_than_the_address_space_is_rejected() {
        let config = MachineConfig {
            ram_bytes: guest_memory::MAX_RAM_BYTES + guest_memory::PAGE_SIZE,
        };

        assert!(matches!(
            config.validate(),
            Err(LayoutError::TooLarge { .. })
        ));
    }
}
