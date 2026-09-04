//! The kernel structures the shim writes into guest memory.
//!
//! Only the fields musl actually reads are filled; the rest stay zero. Offsets
//! are AArch64's, which for these three structures is the `asm-generic` layout.

use super::host::{Errno, SysResult};
use crate::interp::Memory;

/// Size of `struct stat` on AArch64.
pub const STAT_BYTES: usize = 128;
/// Size of `struct utsname`: six 65-byte fields.
pub const UTSNAME_BYTES: usize = 390;
/// Size of one `struct iovec`: a pointer and a length.
pub const IOVEC_BYTES: u64 = 16;
/// Length of each `struct utsname` field, including its terminator.
const UTSNAME_FIELD_BYTES: u64 = 65;

/// `S_IFCHR` — the mode bits that make libc treat a descriptor as a terminal.
const MODE_CHARACTER_DEVICE: u64 = 0o020_666;
/// `S_IFREG`.
const MODE_REGULAR_FILE: u64 = 0o100_644;

/// Writes a `struct stat` describing a character device or a regular file.
///
/// musl reads `st_mode` to decide whether stdout is line-buffered and
/// `st_size` to size a read buffer; nothing else in the startup path is read.
pub fn write_stat(memory: &mut impl Memory, address: u64, file: (bool, u64)) -> Result<(), Errno> {
    let (is_terminal, size) = file;
    let mut buffer = [0u8; STAT_BYTES];

    let mode = if is_terminal {
        MODE_CHARACTER_DEVICE
    } else {
        MODE_REGULAR_FILE
    };
    // st_mode is a u32 at offset 16; st_size an i64 at offset 48.
    buffer[16..20].copy_from_slice(&(mode as u32).to_le_bytes());
    buffer[48..56].copy_from_slice(&size.to_le_bytes());
    // st_blksize at 56, st_blocks at 64: a plausible block size keeps libc
    // from choosing a zero-sized stdio buffer.
    buffer[56..60].copy_from_slice(&4096u32.to_le_bytes());
    buffer[64..72].copy_from_slice(&size.div_ceil(512).to_le_bytes());

    memory.write(address, &buffer).map_err(|_| Errno::Fault)
}

/// Writes a `struct utsname` naming this machine.
pub fn write_utsname(memory: &mut impl Memory, address: u64) -> Result<(), Errno> {
    // Zero first, so a field shorter than its slot has no stale bytes after
    // its terminator.
    memory
        .write(address, &[0u8; UTSNAME_BYTES])
        .map_err(|_| Errno::Fault)?;

    let fields = [
        &b"Linux"[..],
        &b"coracle"[..],
        // The release string gates feature detection in libc and in glibc's
        // loader, so it names a kernel new enough for the syscalls above.
        &b"6.12.0"[..],
        &b"#1 SMP coracle"[..],
        &b"aarch64"[..],
        &b"(none)"[..],
    ];

    for (index, field) in fields.iter().enumerate() {
        let mut padded = [0u8; UTSNAME_FIELD_BYTES as usize];
        padded[..field.len()].copy_from_slice(field);
        let offset = address + index as u64 * UTSNAME_FIELD_BYTES;
        memory.write(offset, &padded).map_err(|_| Errno::Fault)?;
    }
    Ok(())
}

/// One entry of an `iovec` array: where the data is and how much of it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IoVec {
    /// Guest address of the buffer.
    pub base: u64,
    /// Bytes in the buffer.
    pub len: u64,
}

/// Reads the `index`th `struct iovec` of an array at `address`.
pub fn read_iovec(memory: &impl Memory, address: u64, index: u64) -> Result<IoVec, Errno> {
    let entry = address + index * IOVEC_BYTES;
    let base = memory.read_uint(entry, 8).map_err(|_| Errno::Fault)?;
    let len = memory.read_uint(entry + 8, 8).map_err(|_| Errno::Fault)?;
    Ok(IoVec { base, len })
}

/// Writes a `struct timespec` of `nanos` since the epoch.
pub fn write_timespec(memory: &mut impl Memory, address: u64, nanos: u64) -> SysResult {
    memory
        .write_uint(address, 8, nanos / 1_000_000_000)
        .map_err(|_| Errno::Fault)?;
    memory
        .write_uint(address + 8, 8, nanos % 1_000_000_000)
        .map_err(|_| Errno::Fault)?;
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::guest_memory::PHYS_RAM_BASE;
    use crate::interp::FlatMemory;

    #[test]
    fn a_terminal_stat_reports_a_character_device() {
        let mut memory = FlatMemory::new(4096);

        write_stat(&mut memory, PHYS_RAM_BASE, (true, 0)).expect("in range");

        let mode = memory.read_uint(PHYS_RAM_BASE + 16, 4).expect("in range");
        assert_eq!(mode, MODE_CHARACTER_DEVICE);
    }

    #[test]
    fn a_file_stat_reports_its_size_and_a_regular_file_mode() {
        let mut memory = FlatMemory::new(4096);

        write_stat(&mut memory, PHYS_RAM_BASE, (false, 1234)).expect("in range");

        assert_eq!(
            memory.read_uint(PHYS_RAM_BASE + 16, 4),
            Ok(MODE_REGULAR_FILE)
        );
        assert_eq!(memory.read_uint(PHYS_RAM_BASE + 48, 8), Ok(1234));
        assert_eq!(
            memory.read_uint(PHYS_RAM_BASE + 64, 8),
            Ok(3),
            "512-byte blocks"
        );
    }

    #[test]
    fn stat_into_an_unmapped_address_is_a_fault_not_a_panic() {
        let mut memory = FlatMemory::new(4096);

        assert_eq!(write_stat(&mut memory, 0, (true, 0)), Err(Errno::Fault));
    }

    #[test]
    fn utsname_names_an_aarch64_machine_in_the_field_musl_reads() {
        let mut memory = FlatMemory::new(4096);

        write_utsname(&mut memory, PHYS_RAM_BASE).expect("in range");

        let machine_offset = PHYS_RAM_BASE + 4 * UTSNAME_FIELD_BYTES;
        let field = memory.slice(machine_offset, 8).expect("in range");
        assert_eq!(&field[..7], b"aarch64");
        assert_eq!(field[7], 0, "fields are NUL-terminated");
    }

    #[test]
    fn an_iovec_array_is_read_entry_by_entry() {
        let mut memory = FlatMemory::new(4096);
        let array = PHYS_RAM_BASE;
        memory.write_uint(array, 8, 0x1000).expect("in range");
        memory.write_uint(array + 8, 8, 4).expect("in range");
        memory.write_uint(array + 16, 8, 0x2000).expect("in range");
        memory.write_uint(array + 24, 8, 8).expect("in range");

        assert_eq!(
            read_iovec(&memory, array, 0),
            Ok(IoVec {
                base: 0x1000,
                len: 4
            })
        );
        assert_eq!(
            read_iovec(&memory, array, 1),
            Ok(IoVec {
                base: 0x2000,
                len: 8
            })
        );
    }

    #[test]
    fn a_timespec_splits_nanoseconds_into_whole_seconds_and_a_remainder() {
        let mut memory = FlatMemory::new(4096);

        write_timespec(&mut memory, PHYS_RAM_BASE, 2_500_000_000).expect("in range");

        assert_eq!(memory.read_uint(PHYS_RAM_BASE, 8), Ok(2));
        assert_eq!(memory.read_uint(PHYS_RAM_BASE + 8, 8), Ok(500_000_000));
    }
}
