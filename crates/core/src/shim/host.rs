//! What the shim needs from outside the guest, and the errors it reports back.
//!
//! A trait rather than direct `std::fs` calls because this crate is `no_std`
//! and links to `wasm32-unknown-unknown`, where there are no files at all. The
//! native test and benchmark harnesses supply one implementation; a browser
//! host would supply another. `docs/plan.md` deletes the shim after M2, so the
//! surface is the narrowest one that runs a static musl binary, not a
//! filesystem abstraction.

use alloc::vec::Vec;

/// Linux error numbers the shim returns.
///
/// A syscall reports failure as `-errno` in `x0`, which is what musl's syscall
/// wrappers expect.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i64)]
pub enum Errno {
    /// Operation not permitted.
    Perm = 1,
    /// No such file or directory.
    NoEnt = 2,
    /// Bad file descriptor.
    BadFd = 9,
    /// Try again.
    Again = 11,
    /// Out of memory.
    NoMem = 12,
    /// Bad address.
    Fault = 14,
    /// Invalid argument.
    Inval = 22,
    /// Too many open files.
    MFile = 24,
    /// Inappropriate ioctl for device.
    NoTty = 25,
    /// Function not implemented.
    NoSys = 38,
}

impl Errno {
    /// The value a failing syscall leaves in `x0`.
    pub const fn to_return_value(self) -> u64 {
        (-(self as i64)) as u64
    }
}

/// A syscall's result: a value, or an error number.
pub type SysResult = Result<u64, Errno>;

/// The world outside the guest.
///
/// Every method may fail with an [`Errno`], which the shim passes straight
/// through to the guest rather than interpreting.
pub trait HostIo {
    /// Writes `bytes` to an open descriptor, returning the count written.
    fn write(&mut self, fd: i32, bytes: &[u8]) -> SysResult;

    /// Reads into `buffer`, returning the count read. Zero means end of file.
    fn read(&mut self, fd: i32, buffer: &mut [u8]) -> SysResult;

    /// Opens `path` relative to `dirfd`, returning a descriptor.
    fn open(&mut self, path: &[u8], flags: u64) -> SysResult;

    /// Closes a descriptor.
    fn close(&mut self, fd: i32) -> SysResult;

    /// Size in bytes of what `fd` refers to, for `fstat`.
    fn size_of(&mut self, fd: i32) -> SysResult;

    /// Whether `fd` is a terminal, which decides libc's buffering mode.
    fn is_terminal(&mut self, fd: i32) -> bool;

    /// Fills `buffer` with random bytes.
    fn random(&mut self, buffer: &mut [u8]);

    /// Nanoseconds since an unspecified fixed epoch.
    fn monotonic_nanos(&mut self) -> u64;

    /// Records that the guest called `exit` with `status`.
    fn exit(&mut self, status: i32);
}

/// A host with no filesystem, capturing everything written to it.
///
/// The default for wasm, where there are no files, and what the interpreter
/// tests assert against: a test can read back exactly what the guest printed
/// without touching the real stdout.
#[derive(Debug, Default)]
pub struct CapturingHost {
    /// Bytes the guest wrote to fd 1.
    pub stdout: Vec<u8>,
    /// Bytes the guest wrote to fd 2.
    pub stderr: Vec<u8>,
    /// Status passed to `exit`, once the guest has called it.
    pub exit_status: Option<i32>,
    seed: u64,
    nanos: u64,
}

impl CapturingHost {
    /// A host with empty capture buffers.
    pub fn new() -> Self {
        Self {
            seed: 0x2545_f491_4f6c_dd1d,
            ..Self::default()
        }
    }

    /// What the guest wrote to fd 1, as UTF-8 where it is valid.
    pub fn stdout_text(&self) -> &str {
        core::str::from_utf8(&self.stdout).unwrap_or("")
    }

    fn next_random(&mut self) -> u64 {
        self.seed ^= self.seed << 13;
        self.seed ^= self.seed >> 7;
        self.seed ^= self.seed << 17;
        self.seed
    }
}

impl HostIo for CapturingHost {
    fn write(&mut self, fd: i32, bytes: &[u8]) -> SysResult {
        let sink = match fd {
            1 => &mut self.stdout,
            2 => &mut self.stderr,
            _ => return Err(Errno::BadFd),
        };
        sink.extend_from_slice(bytes);
        Ok(bytes.len() as u64)
    }

    fn read(&mut self, fd: i32, _buffer: &mut [u8]) -> SysResult {
        // Stdin is always at end of file; there is nothing to read from.
        if fd == 0 {
            Ok(0)
        } else {
            Err(Errno::BadFd)
        }
    }

    fn open(&mut self, _path: &[u8], _flags: u64) -> SysResult {
        Err(Errno::NoEnt)
    }

    fn close(&mut self, fd: i32) -> SysResult {
        if (0..=2).contains(&fd) {
            Ok(0)
        } else {
            Err(Errno::BadFd)
        }
    }

    fn size_of(&mut self, fd: i32) -> SysResult {
        if (0..=2).contains(&fd) {
            Ok(0)
        } else {
            Err(Errno::BadFd)
        }
    }

    fn is_terminal(&mut self, fd: i32) -> bool {
        (0..=2).contains(&fd)
    }

    fn random(&mut self, buffer: &mut [u8]) {
        for byte in buffer.iter_mut() {
            *byte = self.next_random() as u8;
        }
    }

    fn monotonic_nanos(&mut self) -> u64 {
        // Monotonic and cheap. A guest that measures elapsed time sees it
        // advance; nothing in M1 needs it to track wall clock.
        self.nanos += 1000;
        self.nanos
    }

    fn exit(&mut self, status: i32) {
        self.exit_status = Some(status);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_error_number_becomes_the_negative_value_musl_expects() {
        assert_eq!(Errno::NoSys.to_return_value(), (-38i64) as u64);
        assert_eq!(Errno::BadFd.to_return_value(), (-9i64) as u64);
    }

    #[test]
    fn the_capturing_host_keeps_the_two_output_streams_apart() {
        let mut host = CapturingHost::new();

        assert_eq!(host.write(1, b"out"), Ok(3));
        assert_eq!(host.write(2, b"err"), Ok(3));

        assert_eq!(host.stdout_text(), "out");
        assert_eq!(host.stderr, b"err");
    }

    #[test]
    fn writing_to_a_descriptor_that_was_never_opened_is_a_bad_descriptor() {
        let mut host = CapturingHost::new();

        assert_eq!(host.write(7, b"x"), Err(Errno::BadFd));
    }

    #[test]
    fn random_bytes_are_not_all_the_same_value() {
        let mut host = CapturingHost::new();
        let mut buffer = [0u8; 32];

        host.random(&mut buffer);

        assert!(buffer.iter().any(|&byte| byte != buffer[0]));
    }

    #[test]
    fn the_monotonic_clock_never_goes_backwards() {
        let mut host = CapturingHost::new();

        let first = host.monotonic_nanos();
        let second = host.monotonic_nanos();

        assert!(second > first);
    }
}
