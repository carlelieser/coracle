//! The storage the interpreter loads and stores against.
//!
//! [`crate::guest_memory`] fixes *where* guest RAM sits in linear memory but
//! owns no storage, so M1 needs a type that holds bytes. The trait exists
//! because M2 replaces this implementation with one that walks page tables:
//! the interpreter is written against the trait now so that swap is not a
//! rewrite of every load and store.
//!
//! Addresses here are guest *virtual*, which at EL0 with the MMU off — M1's
//! only configuration — equal guest physical.

use alloc::vec;
use alloc::vec::Vec;

use crate::guest_memory::PHYS_RAM_BASE;

/// Why an access could not be completed.
///
/// One variant: M1 has no permissions and no page tables, so the only way to
/// fail is to leave the mapped window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AccessFault {
    /// Virtual address the access targeted.
    pub address: u64,
    /// Whether the access was a write.
    pub is_write: bool,
}

/// Guest memory, as the interpreter sees it.
///
/// Widths are separate methods rather than one length-taking method so the hot
/// path compiles to a bounds check and a native load, with no loop over bytes.
pub trait Memory {
    /// Reads `LEN` bytes little-endian.
    fn read(&self, address: u64, bytes: &mut [u8]) -> Result<(), AccessFault>;

    /// Writes `bytes` little-endian.
    fn write(&mut self, address: u64, bytes: &[u8]) -> Result<(), AccessFault>;

    /// Reads an unsigned integer of `width` bytes (1, 2, 4 or 8).
    fn read_uint(&self, address: u64, width: u8) -> Result<u64, AccessFault> {
        let mut buffer = [0u8; 8];
        self.read(address, &mut buffer[..width as usize])?;
        Ok(u64::from_le_bytes(buffer))
    }

    /// Writes the low `width` bytes of `value`.
    fn write_uint(&mut self, address: u64, width: u8, value: u64) -> Result<(), AccessFault> {
        self.write(address, &value.to_le_bytes()[..width as usize])
    }
}

/// A flat span of guest RAM starting at [`PHYS_RAM_BASE`].
///
/// Backed by a `Vec` rather than the linear-memory window
/// [`crate::guest_memory::reserve`] describes: M1's shim harness runs natively
/// under `cargo test` and `cargo bench` as well as in wasm, and only the M2
/// machine loop is in a position to own the real window.
#[derive(Debug, Clone)]
pub struct FlatMemory {
    base: u64,
    bytes: Vec<u8>,
}

impl FlatMemory {
    /// Allocates `size` zeroed bytes mapped at [`PHYS_RAM_BASE`].
    pub fn new(size: usize) -> Self {
        Self::at(PHYS_RAM_BASE, size)
    }

    /// Allocates `size` zeroed bytes mapped at `base`.
    pub fn at(base: u64, size: usize) -> Self {
        Self {
            base,
            bytes: vec![0; size],
        }
    }

    /// Lowest mapped address.
    pub const fn base(&self) -> u64 {
        self.base
    }

    /// One past the highest mapped address.
    pub fn limit(&self) -> u64 {
        self.base + self.bytes.len() as u64
    }

    /// Copies `data` to `address`, for loading a program image or a stack.
    pub fn store_slice(&mut self, address: u64, data: &[u8]) -> Result<(), AccessFault> {
        self.write(address, data)
    }

    /// Borrows `len` bytes at `address`, for the shim's `write`-family calls.
    pub fn slice(&self, address: u64, len: usize) -> Result<&[u8], AccessFault> {
        let start = self.offset(address, len, false)?;
        Ok(&self.bytes[start..start + len])
    }

    fn offset(&self, address: u64, len: usize, is_write: bool) -> Result<usize, AccessFault> {
        let fault = AccessFault { address, is_write };
        let start = address.checked_sub(self.base).ok_or(fault)?;
        let end = start.checked_add(len as u64).ok_or(fault)?;
        if end > self.bytes.len() as u64 {
            return Err(fault);
        }
        Ok(start as usize)
    }
}

impl Memory for FlatMemory {
    fn read(&self, address: u64, bytes: &mut [u8]) -> Result<(), AccessFault> {
        let start = self.offset(address, bytes.len(), false)?;
        bytes.copy_from_slice(&self.bytes[start..start + bytes.len()]);
        Ok(())
    }

    fn write(&mut self, address: u64, bytes: &[u8]) -> Result<(), AccessFault> {
        let start = self.offset(address, bytes.len(), true)?;
        self.bytes[start..start + bytes.len()].copy_from_slice(bytes);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn memory() -> FlatMemory {
        FlatMemory::new(4096)
    }

    #[test]
    fn a_stored_value_reads_back_at_every_width() {
        let mut mem = memory();
        let address = PHYS_RAM_BASE + 8;

        mem.write_uint(address, 8, 0x8877_6655_4433_2211)
            .expect("in range");

        assert_eq!(mem.read_uint(address, 1), Ok(0x11));
        assert_eq!(mem.read_uint(address, 2), Ok(0x2211));
        assert_eq!(mem.read_uint(address, 4), Ok(0x4433_2211));
        assert_eq!(mem.read_uint(address, 8), Ok(0x8877_6655_4433_2211));
    }

    #[test]
    fn a_narrow_write_leaves_the_neighbouring_bytes_alone() {
        let mut mem = memory();
        let address = PHYS_RAM_BASE + 16;
        mem.write_uint(address, 8, u64::MAX).expect("in range");

        mem.write_uint(address, 1, 0).expect("in range");

        assert_eq!(mem.read_uint(address, 8), Ok(0xffff_ffff_ffff_ff00));
    }

    #[test]
    fn an_access_below_the_mapped_window_faults_rather_than_wrapping() {
        let mem = memory();

        assert_eq!(
            mem.read_uint(PHYS_RAM_BASE - 1, 1),
            Err(AccessFault {
                address: PHYS_RAM_BASE - 1,
                is_write: false,
            })
        );
    }

    #[test]
    fn an_access_straddling_the_end_of_the_window_faults() {
        let mut mem = memory();
        let last = mem.limit() - 1;

        assert_eq!(mem.read_uint(last, 1), Ok(0));
        assert_eq!(
            mem.write_uint(last, 2, 0),
            Err(AccessFault {
                address: last,
                is_write: true,
            })
        );
    }

    #[test]
    fn a_borrowed_slice_sees_what_was_stored_there() {
        let mut mem = memory();
        let address = PHYS_RAM_BASE + 32;

        mem.store_slice(address, b"hi").expect("in range");

        assert_eq!(mem.slice(address, 2), Ok(&b"hi"[..]));
        assert!(mem.slice(mem.limit() - 1, 2).is_err());
    }
}
