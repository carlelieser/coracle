//! Linux `asm-generic` syscall numbers, which is what AArch64 uses.
//!
//! Only the calls a static musl binary reaches between `_start` and a simple
//! `main` are named. `docs/plan.md` deletes this whole module after M2, so the
//! list is deliberately not exhaustive: an unnamed number returns `ENOSYS` and
//! is counted, which is how the missing ones get found.

/// `getcwd`.
pub const GETCWD: u64 = 17;
/// `fcntl`.
pub const FCNTL: u64 = 25;
/// `ioctl`.
pub const IOCTL: u64 = 29;
/// `faccessat`.
pub const FACCESSAT: u64 = 48;
/// `openat`.
pub const OPENAT: u64 = 56;
/// `close`.
pub const CLOSE: u64 = 57;
/// `lseek`.
pub const LSEEK: u64 = 62;
/// `read`.
pub const READ: u64 = 63;
/// `write`.
pub const WRITE: u64 = 64;
/// `readv`.
pub const READV: u64 = 65;
/// `writev`.
pub const WRITEV: u64 = 66;
/// `pread64`.
pub const PREAD64: u64 = 67;
/// `readlinkat`.
pub const READLINKAT: u64 = 78;
/// `newfstatat`.
pub const NEWFSTATAT: u64 = 79;
/// `fstat`.
pub const FSTAT: u64 = 80;
/// `exit`.
pub const EXIT: u64 = 93;
/// `exit_group`.
pub const EXIT_GROUP: u64 = 94;
/// `set_tid_address`.
pub const SET_TID_ADDRESS: u64 = 96;
/// `futex`. Refused by name rather than by omission: `docs/plan.md` puts it
/// out of shim scope, and a silent `ENOSYS` would look like a gap.
pub const FUTEX: u64 = 98;
/// `set_robust_list`.
pub const SET_ROBUST_LIST: u64 = 99;
/// `clock_gettime`.
pub const CLOCK_GETTIME: u64 = 113;
/// `sched_getaffinity`.
pub const SCHED_GETAFFINITY: u64 = 123;
/// `tgkill`.
pub const TGKILL: u64 = 131;
/// `rt_sigaction`.
pub const RT_SIGACTION: u64 = 134;
/// `rt_sigprocmask`.
pub const RT_SIGPROCMASK: u64 = 135;
/// `uname`.
pub const UNAME: u64 = 160;
/// `getpid`.
pub const GETPID: u64 = 172;
/// `getppid`.
pub const GETPPID: u64 = 173;
/// `getuid`.
pub const GETUID: u64 = 174;
/// `geteuid`.
pub const GETEUID: u64 = 175;
/// `getgid`.
pub const GETGID: u64 = 176;
/// `getegid`.
pub const GETEGID: u64 = 177;
/// `gettid`.
pub const GETTID: u64 = 178;
/// `brk`.
pub const BRK: u64 = 214;
/// `munmap`.
pub const MUNMAP: u64 = 215;
/// `clone`. Refused by name, as [`FUTEX`] is.
pub const CLONE: u64 = 220;
/// `execve`.
pub const EXECVE: u64 = 221;
/// `mmap`.
pub const MMAP: u64 = 222;
/// `mprotect`.
pub const MPROTECT: u64 = 226;
/// `madvise`.
pub const MADVISE: u64 = 233;
/// `wait4`.
pub const WAIT4: u64 = 260;
/// `prlimit64`.
pub const PRLIMIT64: u64 = 261;
/// `getrandom`.
pub const GETRANDOM: u64 = 278;
/// `rseq`.
pub const RSEQ: u64 = 293;
