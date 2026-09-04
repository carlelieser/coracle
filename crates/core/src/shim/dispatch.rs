//! One match over the syscall number.
//!
//! Grouped by what the call touches — output, input, memory, identity — rather
//! than by number, because a reader looking for "how does the guest print"
//! wants the first group, not syscall 64.

use super::host::{Errno, HostIo, SysResult};
use super::number as sys;
use super::{structs, Shim};
use crate::interp::{Cpu, Memory};
use crate::trace::TraceSink;
use alloc::vec;

/// What servicing a syscall produced.
pub enum Outcome {
    /// A value or error to place in `x0`.
    Returned(SysResult),
    /// The guest asked to terminate.
    Exited(i32),
}

/// Largest single transfer the shim will stage through a buffer.
///
/// A guest asking for more gets a short read or write, which is a legal answer
/// to both calls and keeps one bad length from allocating without bound.
const MAX_TRANSFER: u64 = 1 << 20;

/// Services one syscall.
pub fn call<H: HostIo, M: Memory, S: TraceSink>(
    shim: &mut Shim<H>,
    cpu: &mut Cpu<M, S>,
    number: u64,
    args: &[u64; 6],
) -> Outcome {
    match number {
        sys::EXIT | sys::EXIT_GROUP => Outcome::Exited(args[0] as i32),
        sys::WRITE => Outcome::Returned(write(shim, cpu, args)),
        sys::WRITEV => Outcome::Returned(writev(shim, cpu, args)),
        sys::READ => Outcome::Returned(read(shim, cpu, args)),
        sys::READV => Outcome::Returned(readv(shim, cpu, args)),
        _ => other(shim, cpu, number, args),
    }
}

/// The calls that are a value, a memory poke, or a refusal.
fn other<H: HostIo, M: Memory, S: TraceSink>(
    shim: &mut Shim<H>,
    cpu: &mut Cpu<M, S>,
    number: u64,
    args: &[u64; 6],
) -> Outcome {
    let result = match number {
        sys::BRK => Ok(shim.map.set_brk(args[0])),
        sys::MMAP => shim.map.map(args[1]).ok_or(Errno::NoMem),
        // Nothing is reclaimed; see the module docs.
        sys::MUNMAP | sys::MPROTECT | sys::MADVISE => Ok(0),
        sys::OPENAT => open(shim, cpu, args),
        sys::CLOSE => shim.host.close(args[0] as i32),
        sys::FSTAT => fstat(shim, cpu, args[0] as i32, args[1]),
        sys::NEWFSTATAT => fstat(shim, cpu, args[0] as i32, args[2]),
        sys::UNAME => structs::write_utsname(&mut cpu.memory, args[0]).map(|()| 0),
        sys::CLOCK_GETTIME => {
            let nanos = shim.host.monotonic_nanos();
            structs::write_timespec(&mut cpu.memory, args[1], nanos)
        }
        sys::GETRANDOM => getrandom(shim, cpu, args),
        sys::IOCTL => ioctl(shim, args),
        // A terminal has no meaningful seek, and no M1 guest seeks a file.
        sys::LSEEK => Ok(0),
        sys::FACCESSAT | sys::READLINKAT | sys::GETCWD => Err(Errno::NoEnt),
        // musl calls these once at startup and ignores a benign answer.
        sys::SET_TID_ADDRESS | sys::GETTID | sys::GETPID => Ok(1),
        sys::SET_ROBUST_LIST | sys::RSEQ | sys::PRLIMIT64 => Ok(0),
        sys::RT_SIGACTION | sys::RT_SIGPROCMASK | sys::FCNTL => Ok(0),
        sys::GETPPID => Ok(0),
        sys::GETUID | sys::GETEUID | sys::GETGID | sys::GETEGID => Ok(0),
        sys::SCHED_GETAFFINITY => affinity(cpu, args),
        // Single-threaded by decision, not by omission (module docs).
        sys::CLONE | sys::FUTEX => Err(Errno::NoSys),
        // No process model: there is nothing to exec into or wait for.
        sys::EXECVE | sys::WAIT4 | sys::TGKILL => Err(Errno::NoSys),
        _ => {
            shim.record_unhandled(number);
            Err(Errno::NoSys)
        }
    };
    Outcome::Returned(result)
}

/// `write(fd, buf, count)`.
fn write<H: HostIo, M: Memory, S: TraceSink>(
    shim: &mut Shim<H>,
    cpu: &mut Cpu<M, S>,
    args: &[u64; 6],
) -> SysResult {
    transfer_out(shim, cpu, args[0] as i32, (args[1], args[2]))
}

/// `writev(fd, iov, iovcnt)` — musl's stdio writes through this, not `write`.
fn writev<H: HostIo, M: Memory, S: TraceSink>(
    shim: &mut Shim<H>,
    cpu: &mut Cpu<M, S>,
    args: &[u64; 6],
) -> SysResult {
    let mut written = 0;
    for index in 0..args[2] {
        let vector = structs::read_iovec(&cpu.memory, args[1], index)?;
        if vector.len == 0 {
            continue;
        }
        let count = transfer_out(shim, cpu, args[0] as i32, (vector.base, vector.len))?;
        written += count;
        // A short write ends the call, as it does on Linux.
        if count < vector.len {
            break;
        }
    }
    Ok(written)
}

/// Copies `len` bytes out of guest memory to a descriptor.
fn transfer_out<H: HostIo, M: Memory, S: TraceSink>(
    shim: &mut Shim<H>,
    cpu: &mut Cpu<M, S>,
    fd: i32,
    buffer: (u64, u64),
) -> SysResult {
    let (address, len) = buffer;
    let len = len.min(MAX_TRANSFER) as usize;
    let mut bytes = vec![0u8; len];
    cpu.memory
        .read(address, &mut bytes)
        .map_err(|_| Errno::Fault)?;
    shim.host.write(fd, &bytes)
}

/// `read(fd, buf, count)`.
fn read<H: HostIo, M: Memory, S: TraceSink>(
    shim: &mut Shim<H>,
    cpu: &mut Cpu<M, S>,
    args: &[u64; 6],
) -> SysResult {
    transfer_in(shim, cpu, args[0] as i32, (args[1], args[2]))
}

/// `readv(fd, iov, iovcnt)`.
fn readv<H: HostIo, M: Memory, S: TraceSink>(
    shim: &mut Shim<H>,
    cpu: &mut Cpu<M, S>,
    args: &[u64; 6],
) -> SysResult {
    let mut total = 0;
    for index in 0..args[2] {
        let vector = structs::read_iovec(&cpu.memory, args[1], index)?;
        let count = transfer_in(shim, cpu, args[0] as i32, (vector.base, vector.len))?;
        total += count;
        if count < vector.len {
            break;
        }
    }
    Ok(total)
}

/// Copies up to `len` bytes from a descriptor into guest memory.
fn transfer_in<H: HostIo, M: Memory, S: TraceSink>(
    shim: &mut Shim<H>,
    cpu: &mut Cpu<M, S>,
    fd: i32,
    buffer: (u64, u64),
) -> SysResult {
    let (address, len) = buffer;
    let mut bytes = vec![0u8; len.min(MAX_TRANSFER) as usize];
    let count = shim.host.read(fd, &mut bytes)? as usize;
    cpu.memory
        .write(address, &bytes[..count])
        .map_err(|_| Errno::Fault)?;
    Ok(count as u64)
}

/// `openat(dirfd, path, flags, mode)`.
fn open<H: HostIo, M: Memory, S: TraceSink>(
    shim: &mut Shim<H>,
    cpu: &mut Cpu<M, S>,
    args: &[u64; 6],
) -> SysResult {
    let path = read_c_string(&cpu.memory, args[1])?;
    shim.host.open(&path, args[2])
}

/// `fstat(fd, statbuf)` and the `newfstatat` form that names one.
fn fstat<H: HostIo, M: Memory, S: TraceSink>(
    shim: &mut Shim<H>,
    cpu: &mut Cpu<M, S>,
    fd: i32,
    address: u64,
) -> SysResult {
    let size = shim.host.size_of(fd)?;
    let is_terminal = shim.host.is_terminal(fd);
    structs::write_stat(&mut cpu.memory, address, (is_terminal, size))?;
    Ok(0)
}

/// `getrandom(buf, buflen, flags)`.
fn getrandom<H: HostIo, M: Memory, S: TraceSink>(
    shim: &mut Shim<H>,
    cpu: &mut Cpu<M, S>,
    args: &[u64; 6],
) -> SysResult {
    let len = args[1].min(MAX_TRANSFER) as usize;
    let mut bytes = vec![0u8; len];
    shim.host.random(&mut bytes);
    cpu.memory
        .write(args[0], &bytes)
        .map_err(|_| Errno::Fault)?;
    Ok(len as u64)
}

/// `ioctl(fd, request, ...)`.
///
/// Only the terminal query matters: libc issues `TCGETS` to decide whether
/// stdout is line-buffered, and expects `ENOTTY` when it is not a terminal.
fn ioctl<H: HostIo>(shim: &mut Shim<H>, args: &[u64; 6]) -> SysResult {
    if shim.host.is_terminal(args[0] as i32) {
        Ok(0)
    } else {
        Err(Errno::NoTty)
    }
}

/// `sched_getaffinity(pid, cpusetsize, mask)` — one vCPU, so one bit.
fn affinity<M: Memory, S: TraceSink>(cpu: &mut Cpu<M, S>, args: &[u64; 6]) -> SysResult {
    if args[1] < 8 {
        return Err(Errno::Inval);
    }
    cpu.memory
        .write_uint(args[2], 8, 1)
        .map_err(|_| Errno::Fault)?;
    Ok(8)
}

/// Longest path the shim will read out of guest memory.
const MAX_PATH: u64 = 4096;

/// Reads a NUL-terminated string from guest memory.
fn read_c_string(memory: &impl Memory, address: u64) -> Result<alloc::vec::Vec<u8>, Errno> {
    let mut bytes = alloc::vec::Vec::new();
    for offset in 0..MAX_PATH {
        let byte = memory
            .read_uint(address + offset, 1)
            .map_err(|_| Errno::Fault)? as u8;
        if byte == 0 {
            return Ok(bytes);
        }
        bytes.push(byte);
    }
    Err(Errno::Inval)
}
