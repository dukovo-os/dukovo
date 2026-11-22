// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The records of Linux's x86-64 system call interface that the kernel
//! writes to, or reads from, a program's memory: `struct stat` and `struct
//! statx`, `linux_dirent64`, the kernel's `struct sigaction`, `struct
//! utsname`, `timespec` and `timeval`, `struct itimerval`, `struct rusage`
//! and `struct sysinfo`, `struct flock` and `struct iovec`, and
//! `utimensat`'s times.
//!
//! Each is built in, or read from, a byte array or slice of the record's
//! exact size, little-endian as the CPU is, so the kernel fills a local
//! buffer and copies it out in one go. The layouts are those of the x86-64
//! uapi (`asm/stat.h`, `linux/stat.h`, `linux/dirent.h`, `asm/signal.h`,
//! `linux/utsname.h`, `linux/time_types.h`, `linux/time.h`,
//! `linux/resource.h` and `linux/sysinfo.h`), and the host tests check every field's offset. A
//! writer sets every byte of its record, padding and fields it has no value
//! for included, so no kernel memory leaks through one.

use radix_abi::file::{TIME_NOW, TIME_OMIT};

use crate::errno::{EINVAL, EOVERFLOW};

/// The size of `struct stat`.
pub const STAT_SIZE: usize = 144;
/// The size of `struct statx`.
pub const STATX_SIZE: usize = 256;
/// The size of the kernel's `struct sigaction`.
pub const SIGACTION_SIZE: usize = 32;
/// The size of `struct utsname`: six fields of 65 bytes.
pub const UTSNAME_SIZE: usize = 390;
/// The size of `struct timespec` and `struct timeval`.
pub const TIMESPEC_SIZE: usize = 16;
/// The size of `struct rusage`.
pub const RUSAGE_SIZE: usize = 144;
/// The size of `struct sysinfo`.
pub const SYSINFO_SIZE: usize = 112;

/// The file type bits of a mode, `S_IFMT`.
pub const S_IFMT: u32 = 0o170_000;
/// A socket.
pub const S_IFSOCK: u32 = 0o140_000;
/// A symbolic link.
pub const S_IFLNK: u32 = 0o120_000;
/// A regular file.
pub const S_IFREG: u32 = 0o100_000;
/// A block device.
pub const S_IFBLK: u32 = 0o060_000;
/// A directory.
pub const S_IFDIR: u32 = 0o040_000;
/// A character device.
pub const S_IFCHR: u32 = 0o020_000;
/// A pipe.
pub const S_IFIFO: u32 = 0o010_000;

/// `linux_dirent64`'s `d_type` when the file system does not say.
pub const DT_UNKNOWN: u8 = 0;
/// A pipe.
pub const DT_FIFO: u8 = 1;
/// A character device.
pub const DT_CHR: u8 = 2;
/// A directory.
pub const DT_DIR: u8 = 4;
/// A block device.
pub const DT_BLK: u8 = 6;
/// A regular file.
pub const DT_REG: u8 = 8;
/// A symbolic link.
pub const DT_LNK: u8 = 10;
/// A socket.
pub const DT_SOCK: u8 = 12;

/// `statx`'s `stx_mask` bits for the fields `struct stat` has: the type,
/// mode, link count, owners, times, inode, size and blocks.
pub const STATX_BASIC_STATS: u32 = 0x7ff;
/// `statx`'s `stx_mask` bit for the birth time.
pub const STATX_BTIME: u32 = 0x800;

/// The bytes of a `linux_dirent64` before its name: the inode, the offset,
/// the record's length and the type.
const DIRENT64_HEADER: usize = 19;
/// The length of each field of `struct utsname`, with its NUL.
const UTS_FIELD: usize = 65;
/// Nanoseconds in a second.
const NANOS: u32 = 1_000_000_000;
/// Microseconds in a second.
const MICROS: u32 = 1_000_000;

/// An instant or a span as Linux's `timespec` and `statx_timestamp` hold
/// it: whole seconds from 1970 and the nanoseconds past them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Time {
    /// Whole seconds. Negative before 1970.
    pub sec: i64,
    /// Nanoseconds past `sec`, 0 to 999,999,999.
    pub nsec: u32,
}

impl Time {
    /// The time `ns` nanoseconds after 1970, which is how the native
    /// interface counts.
    pub fn from_ns(ns: u64) -> Time {
        // At most u64::MAX / 10^9, which is about 1.8 * 10^10 and fits in
        // an i64; the remainder is below 10^9 and fits in a u32.
        Time { sec: (ns / u64::from(NANOS)) as i64, nsec: (ns % u64::from(NANOS)) as u32 }
    }

    /// The nanoseconds since 1970 this time is, which is how the native
    /// interface counts. A time before 1970 is 0 and one past what a `u64`
    /// holds is `u64::MAX`.
    pub fn as_ns(&self) -> u64 {
        let Ok(sec) = u64::try_from(self.sec) else {
            return 0;
        };
        sec.saturating_mul(u64::from(NANOS)).saturating_add(u64::from(self.nsec))
    }
}

/// What `stat`, `fstat` and `statx` tell of a file. The kernel fills it
/// from the native status and this module writes it in either record.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Stat {
    /// The device the file is on. A device number as [`makedev`] builds it.
    pub dev: u64,
    /// The file's number on its device.
    pub ino: u64,
    /// The file type bits and the permission bits, `st_mode`.
    pub mode: u32,
    /// How many names the file has.
    pub nlink: u64,
    /// The owner's number.
    pub uid: u32,
    /// The group's number.
    pub gid: u32,
    /// The device a device node stands for, as [`makedev`] builds it.
    pub rdev: u64,
    /// The size in bytes.
    pub size: u64,
    /// The best size for I/O, in bytes.
    pub blksize: u32,
    /// The size on the disk, in 512-byte blocks.
    pub blocks: u64,
    /// When the file was last read.
    pub atime: Time,
    /// When its contents last changed.
    pub mtime: Time,
    /// When it last changed in any way.
    pub ctime: Time,
    /// When it was made, if the filesystem keeps that. `stat` has no field
    /// for it.
    pub btime: Option<Time>,
}

/// The device number of `major` and `minor`, as Linux's C libraries build
/// the `st_dev` and `st_rdev` of x86-64 programs: 12 bits of major and 8
/// of minor in the low 20 bits, and the rest of each above 32 and 12 bits
/// up.
pub fn makedev(major: u32, minor: u32) -> u64 {
    let (major, minor) = (u64::from(major), u64::from(minor));
    ((major & 0xfff) << 8) | ((major & !0xfff) << 32) | (minor & 0xff) | ((minor & !0xff) << 12)
}

/// The major number of a device number.
pub fn major(dev: u64) -> u32 {
    // The first part is 12 bits and the second the high 32 bits of a u64
    // moved down, so the whole is below 2^32.
    (((dev >> 8) & 0xfff) | ((dev >> 32) & !0xfff)) as u32
}

/// The minor number of a device number.
pub fn minor(dev: u64) -> u32 {
    // The high part of a minor number that does not fit in 32 bits is cut,
    // as the C library's `minor` cuts it in its `unsigned int`.
    ((dev & 0xff) | ((dev >> 12) & !0xff)) as u32
}

/// Writes `bytes` at `at` in `out`. Every offset is a constant of a
/// record's layout, which the record's size covers, so the range exists.
fn put<const N: usize>(out: &mut [u8], at: usize, bytes: [u8; N]) {
    if let Some(field) = out.get_mut(at..at + N) {
        field.copy_from_slice(&bytes);
    }
}

/// The `N` bytes at `at` in `bytes`, zero where `bytes` is short.
fn get<const N: usize>(bytes: &[u8], at: usize) -> [u8; N] {
    let mut field = [0u8; N];
    if let Some(source) = bytes.get(at..at + N) {
        field.copy_from_slice(source);
    }
    field
}

/// `value` as a signed field, for a size or a count that no file reaches
/// `i64::MAX` of.
fn signed(value: u64) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}

/// Writes `stat` as x86-64's `struct stat`.
pub fn put_stat(stat: &Stat, out: &mut [u8; STAT_SIZE]) {
    out.fill(0);
    put(out, 0, stat.dev.to_le_bytes());
    put(out, 8, stat.ino.to_le_bytes());
    put(out, 16, stat.nlink.to_le_bytes());
    put(out, 24, stat.mode.to_le_bytes());
    put(out, 28, stat.uid.to_le_bytes());
    put(out, 32, stat.gid.to_le_bytes());
    put(out, 40, stat.rdev.to_le_bytes());
    put(out, 48, signed(stat.size).to_le_bytes());
    put(out, 56, i64::from(stat.blksize).to_le_bytes());
    put(out, 64, signed(stat.blocks).to_le_bytes());
    for (at, time) in [(72, stat.atime), (88, stat.mtime), (104, stat.ctime)] {
        put(out, at, time.sec.to_le_bytes());
        put(out, at + 8, i64::from(time.nsec).to_le_bytes());
    }
}

/// Writes `stat` as `struct statx`, with `stx_mask` naming the fields that
/// are valid.
///
/// Linux fills the basic fields whatever `wanted_mask` asks for, so they
/// are always valid here. The birth time is written only when the caller
/// asked for it (`STATX_BTIME` in `wanted_mask`) and the file has one, as
/// the filesystems that keep it do. Fields past the device numbers, which
/// newer kernels use, are zero, and so are `stx_attributes` and
/// `stx_attributes_mask`: no attribute is reported.
pub fn put_statx(stat: &Stat, wanted_mask: u32, out: &mut [u8; STATX_SIZE]) {
    out.fill(0);
    let btime = stat.btime.filter(|_| wanted_mask & STATX_BTIME != 0);
    let mask = if btime.is_some() { STATX_BASIC_STATS | STATX_BTIME } else { STATX_BASIC_STATS };
    put(out, 0, mask.to_le_bytes());
    put(out, 4, stat.blksize.to_le_bytes());
    put(out, 16, u32::try_from(stat.nlink).unwrap_or(u32::MAX).to_le_bytes());
    put(out, 20, stat.uid.to_le_bytes());
    put(out, 24, stat.gid.to_le_bytes());
    // `stx_mode` holds the 16 bits of the type and permissions.
    let [low, high, ..] = stat.mode.to_le_bytes();
    put(out, 28, [low, high]);
    put(out, 32, stat.ino.to_le_bytes());
    put(out, 40, stat.size.to_le_bytes());
    put(out, 48, stat.blocks.to_le_bytes());
    put_timestamp(out, 64, stat.atime);
    put_timestamp(out, 80, btime.unwrap_or_default());
    put_timestamp(out, 96, stat.ctime);
    put_timestamp(out, 112, stat.mtime);
    put(out, 128, major(stat.rdev).to_le_bytes());
    put(out, 132, minor(stat.rdev).to_le_bytes());
    put(out, 136, major(stat.dev).to_le_bytes());
    put(out, 140, minor(stat.dev).to_le_bytes());
}

/// Writes a `statx_timestamp` at `at`: seconds, nanoseconds and a pad.
fn put_timestamp(out: &mut [u8], at: usize, time: Time) {
    put(out, at, time.sec.to_le_bytes());
    put(out, at + 8, time.nsec.to_le_bytes());
}

/// Writes a `linux_dirent64` at the start of `out`, and returns the bytes
/// it took, or `None` if the record does not fit.
///
/// `ino` is the file's number, `next_offset` the offset that a `lseek` or
/// the next `getdents64` continues from, `kind` a `DT_` value and `name` the
/// entry's name without a NUL. The record is the 19-byte header, the name,
/// a NUL, and zeroes up to a multiple of 8, so that the next record is
/// aligned. A record too long for its 16-bit length is `None`.
pub fn put_dirent64(
    out: &mut [u8],
    ino: u64,
    next_offset: i64,
    kind: u8,
    name: &[u8],
) -> Option<usize> {
    let used = (DIRENT64_HEADER + name.len() + 1).next_multiple_of(8);
    let length = u16::try_from(used).ok()?;
    let record = out.get_mut(..used)?;
    record.fill(0);
    let (header, tail) = record.split_at_mut(DIRENT64_HEADER);
    put(header, 0, ino.to_le_bytes());
    put(header, 8, next_offset.to_le_bytes());
    put(header, 16, length.to_le_bytes());
    put(header, 18, [kind]);
    // The record has room for the name and a NUL after it.
    tail[..name.len()].copy_from_slice(name);
    Some(used)
}

/// The kernel's `struct sigaction`, as `rt_sigaction` reads and writes it.
/// It is not the C library's `struct sigaction`, which has the same fields
/// in another order.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SigAction {
    /// The handler's address, or `SIG_DFL` or `SIG_IGN`.
    pub handler: u64,
    /// The `SA_` flags.
    pub flags: u64,
    /// The address of the function that returns from the handler.
    pub restorer: u64,
    /// The signals blocked while the handler runs, bit `n - 1` for signal
    /// `n`.
    pub mask: u64,
}

impl SigAction {
    /// Reads the record at `bytes`.
    pub fn read(bytes: &[u8; SIGACTION_SIZE]) -> SigAction {
        SigAction {
            handler: u64::from_le_bytes(get(bytes, 0)),
            flags: u64::from_le_bytes(get(bytes, 8)),
            restorer: u64::from_le_bytes(get(bytes, 16)),
            mask: u64::from_le_bytes(get(bytes, 24)),
        }
    }

    /// Writes the record to `out`.
    pub fn put(&self, out: &mut [u8; SIGACTION_SIZE]) {
        put(out, 0, self.handler.to_le_bytes());
        put(out, 8, self.flags.to_le_bytes());
        put(out, 16, self.restorer.to_le_bytes());
        put(out, 24, self.mask.to_le_bytes());
    }
}

/// Writes `struct utsname`: the system name "Linux", the three names the
/// caller gives, the machine "x86_64" and the domain name "(none)".
///
/// Each field is 65 bytes, padded with NULs. A name longer than 64 bytes is
/// cut there, so that its NUL stays.
pub fn utsname(nodename: &[u8], release: &[u8], version: &[u8], out: &mut [u8; UTSNAME_SIZE]) {
    out.fill(0);
    let fields: [&[u8]; 6] = [b"Linux", nodename, release, version, b"x86_64", b"(none)"];
    for (index, text) in fields.into_iter().enumerate() {
        let kept = text.len().min(UTS_FIELD - 1);
        let start = index * UTS_FIELD;
        out[start..start + kept].copy_from_slice(&text[..kept]);
    }
}

/// Reads a `struct timespec`: seconds and nanoseconds, each 64 bits.
///
/// # Errors
///
/// EINVAL, as Linux gives it for a time that is not valid for a wait or a
/// clock: negative seconds, or nanoseconds negative or a second or more.
/// `utimensat`'s `UTIME_NOW` and `UTIME_OMIT`, which lie in the nanoseconds,
/// are not valid here and need reading before this.
pub fn timespec(bytes: &[u8; TIMESPEC_SIZE]) -> Result<Time, i32> {
    let sec = i64::from_le_bytes(get(bytes, 0));
    let nsec = i64::from_le_bytes(get(bytes, 8));
    match (u64::try_from(sec), u32::try_from(nsec)) {
        (Ok(_), Ok(nsec)) if nsec < NANOS => Ok(Time { sec, nsec }),
        _ => Err(EINVAL),
    }
}

/// Writes `time` as a `struct timespec`.
pub fn put_timespec(time: Time, out: &mut [u8; TIMESPEC_SIZE]) {
    put(out, 0, time.sec.to_le_bytes());
    put(out, 8, i64::from(time.nsec).to_le_bytes());
}

/// Reads a `struct timeval`: seconds and microseconds, each 64 bits.
///
/// # Errors
///
/// EINVAL for negative seconds, or microseconds negative or a second or
/// more.
pub fn timeval(bytes: &[u8; TIMESPEC_SIZE]) -> Result<Time, i32> {
    let sec = i64::from_le_bytes(get(bytes, 0));
    let usec = i64::from_le_bytes(get(bytes, 8));
    match (u64::try_from(sec), u32::try_from(usec)) {
        (Ok(_), Ok(usec)) if usec < MICROS => Ok(Time { sec, nsec: usec * 1000 }),
        _ => Err(EINVAL),
    }
}

/// Writes `time` as a `struct timeval`, cutting the nanoseconds to
/// microseconds.
pub fn put_timeval(time: Time, out: &mut [u8; TIMESPEC_SIZE]) {
    put_timeval_at(out, 0, time);
}

/// Writes a `struct timeval` at `at`.
fn put_timeval_at(out: &mut [u8], at: usize, time: Time) {
    put(out, at, time.sec.to_le_bytes());
    put(out, at + 8, i64::from(time.nsec / 1000).to_le_bytes());
}

/// The size of `struct itimerval`: the interval, then the value, each a
/// `struct timeval`.
pub const ITIMERVAL_SIZE: usize = 32;

/// Reads a `struct itimerval` as `setitimer` takes it: the value and the
/// interval, in nanoseconds.
///
/// # Errors
///
/// EINVAL if either `struct timeval` is not valid ([`timeval`]).
pub fn itimerval(bytes: &[u8; ITIMERVAL_SIZE]) -> Result<(u64, u64), i32> {
    let interval = timeval(&get(bytes, 0))?;
    let value = timeval(&get(bytes, 16))?;
    Ok((value.as_ns(), interval.as_ns()))
}

/// Writes a `struct itimerval` for a timer with `value_ns` left and an
/// interval of `interval_ns`. A timer under a microsecond from expiring
/// reads as one microsecond, never 0, so that a program setting it again
/// with what it read does not turn it off.
pub fn put_itimerval(value_ns: u64, interval_ns: u64, out: &mut [u8; ITIMERVAL_SIZE]) {
    let value = if value_ns > 0 && value_ns < 1000 { 1000 } else { value_ns };
    put_timeval_at(out, 0, Time::from_ns(interval_ns));
    put_timeval_at(out, 16, Time::from_ns(value));
}

/// What `alarm` returns for a timer with `value_ns` left: the whole
/// seconds rounded to the nearest, and never 0 for a timer still set, as
/// Linux rounds it; cut to an `unsigned int`, as Linux returns it.
pub fn alarm_left(value_ns: u64) -> u32 {
    let time = Time::from_ns(value_ns);
    let rounds_up = (time.sec == 0 && time.nsec != 0) || time.nsec >= NANOS / 2;
    (time.sec as u64).wrapping_add(u64::from(rounds_up)) as u32
}

/// Writes `struct rusage` for a process that ran `user_ns` nanoseconds in
/// user mode and `system_ns` in the kernel, and whose largest resident set
/// was `max_rss_kib` KiB. The counts Radix does not keep are 0.
pub fn put_rusage(user_ns: u64, system_ns: u64, max_rss_kib: u64, out: &mut [u8; RUSAGE_SIZE]) {
    out.fill(0);
    put_timeval_at(out, 0, Time::from_ns(user_ns));
    put_timeval_at(out, 16, Time::from_ns(system_ns));
    put(out, 32, signed(max_rss_kib).to_le_bytes());
}

/// Writes `struct sysinfo` for a machine up for `uptime_s` seconds, with
/// `total_ram` bytes of memory of which `free_ram` are free, and `procs`
/// processes. The load averages, shared and buffer memory and swap are 0,
/// and the memory unit is one byte.
pub fn put_sysinfo(
    uptime_s: i64,
    total_ram: u64,
    free_ram: u64,
    procs: u16,
    out: &mut [u8; SYSINFO_SIZE],
) {
    out.fill(0);
    put(out, 0, uptime_s.to_le_bytes());
    put(out, 32, total_ram.to_le_bytes());
    put(out, 40, free_ram.to_le_bytes());
    put(out, 80, procs.to_le_bytes());
    put(out, 104, 1u32.to_le_bytes());
}

/// The size of `struct flock`.
pub const FLOCK_SIZE: usize = 32;
/// The size of `struct iovec`.
pub const IOVEC_SIZE: usize = 16;

/// `struct flock`, which `fcntl`'s record lock commands read and, for
/// `F_GETLK`, write.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Flock {
    /// `l_type`: `F_RDLCK`, `F_WRLCK` or `F_UNLCK`.
    pub kind: i16,
    /// `l_whence`: what `l_start` counts from.
    pub whence: i16,
    /// `l_start`: where the range starts, from `whence`.
    pub start: i64,
    /// `l_len`: how long the range is, 0 for to the end of the file,
    /// however it grows, and negative for the bytes before `start`.
    pub len: i64,
    /// `l_pid`: the process holding the lock, which `F_GETLK` fills in.
    pub pid: i32,
}

impl Flock {
    /// Reads the record at `bytes`.
    pub fn read(bytes: &[u8; FLOCK_SIZE]) -> Flock {
        Flock {
            kind: i16::from_le_bytes(get(bytes, 0)),
            whence: i16::from_le_bytes(get(bytes, 2)),
            start: i64::from_le_bytes(get(bytes, 8)),
            len: i64::from_le_bytes(get(bytes, 16)),
            pid: i32::from_le_bytes(get(bytes, 24)),
        }
    }

    /// Writes the record to `out`, its padding zero.
    pub fn put(&self, out: &mut [u8; FLOCK_SIZE]) {
        out.fill(0);
        put(out, 0, self.kind.to_le_bytes());
        put(out, 2, self.whence.to_le_bytes());
        put(out, 8, self.start.to_le_bytes());
        put(out, 16, self.len.to_le_bytes());
        put(out, 24, self.pid.to_le_bytes());
    }
}

/// The range a record lock covers, as the native lock takes it: where it
/// starts and how long it is (0: to the end of the file). `base` is where
/// `l_whence` counts from, and `start` and `len` are the record's.
///
/// A negative `len` is the bytes before `start`, as Linux has it.
///
/// # Errors
///
/// EOVERFLOW for a start or an end past what an offset holds, and EINVAL
/// for a range that starts before the file or has a length that cannot be
/// negated.
pub fn lock_range(base: i64, start: i64, len: i64) -> Result<(u64, u64), i32> {
    let mut first = base.checked_add(start).ok_or(EOVERFLOW)?;
    let mut length = len;
    if length < 0 {
        first = first.checked_add(length).ok_or(EINVAL)?;
        length = length.checked_neg().ok_or(EINVAL)?;
    }
    if first < 0 {
        return Err(EINVAL);
    }
    if length != 0 {
        first.checked_add(length - 1).ok_or(EOVERFLOW)?;
    }
    // Neither is negative here.
    Ok((first.unsigned_abs(), length.unsigned_abs()))
}

/// One `struct iovec` of a vectored read or write.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Iovec {
    /// `iov_base`: where the buffer is.
    pub base: u64,
    /// `iov_len`: how long it is. A length that is negative as a signed
    /// number, which no buffer has, is EINVAL.
    pub len: u64,
}

impl Iovec {
    /// Reads the record at the start of `bytes`, with zero for what a
    /// short slice does not hold.
    pub fn read(bytes: &[u8]) -> Iovec {
        Iovec { base: u64::from_le_bytes(get(bytes, 0)), len: u64::from_le_bytes(get(bytes, 8)) }
    }
}

/// `utimensat`'s `tv_nsec` that sets a time to now.
pub const UTIME_NOW: i64 = (1 << 30) - 1;
/// `utimensat`'s `tv_nsec` that leaves a time as it is.
pub const UTIME_OMIT: i64 = (1 << 30) - 2;

/// What `utimensat` and its relatives say to do with one of a file's two
/// times.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SetTime {
    /// Set it to the time of the call.
    Now,
    /// Leave it.
    Omit,
    /// Set it to this time.
    At(Time),
}

impl SetTime {
    /// The native `set_times` argument for this: a time in nanoseconds
    /// since 1970, or the native "now" or "leave it".
    ///
    /// # Errors
    ///
    /// EINVAL for a time the native interface cannot hold, which is the
    /// two numbers it keeps for "now" and "leave it" and every one past
    /// them.
    pub fn native(self) -> Result<u64, i32> {
        match self {
            SetTime::Now => Ok(TIME_NOW),
            SetTime::Omit => Ok(TIME_OMIT),
            SetTime::At(time) => match time.as_ns() {
                ns if ns >= TIME_NOW => Err(EINVAL),
                ns => Ok(ns),
            },
        }
    }
}

/// Reads one of `utimensat`'s two `struct timespec`s: `UTIME_NOW` and
/// `UTIME_OMIT` in the nanoseconds, or a time, which [`timespec`] reads.
///
/// # Errors
///
/// EINVAL for a time that is not valid. A time before 1970 is not valid
/// here, as the native interface counts from then.
pub fn utimespec(bytes: &[u8; TIMESPEC_SIZE]) -> Result<SetTime, i32> {
    match i64::from_le_bytes(get(bytes, 8)) {
        UTIME_NOW => Ok(SetTime::Now),
        UTIME_OMIT => Ok(SetTime::Omit),
        _ => timespec(bytes).map(SetTime::At),
    }
}
