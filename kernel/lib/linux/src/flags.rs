// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Linux's `open` flags and `AT_` flags, and what they ask of the native
//! directory methods; the commands and flags of `fcntl`, `flock`, `lseek`,
//! `close_range` and `ioctl`, and what they ask of the native file
//! methods; and the flags of `pipe2` and the types of `mknod`.
//!
//! [`open`] follows `open_flags` and `open_path` in the C library's Radix
//! layer (`ports/mlibc/sysdeps/radix/generic/files.cpp`), which the C
//! library's own tests exercise: the same flag gives the same native flag
//! and the same rights word here. Where the raw Linux call differs from the
//! C library's wrapper, the raw call's rule is the one written, and the
//! difference is said at the function.
//!
//! The values are those of Linux's x86-64 `asm-generic/fcntl.h`,
//! `linux/fcntl.h` and `unistd.h`.

use radix_abi::capability::Rights;
use radix_abi::directory::{EXECUTE_OK, FileKind, OpenFlags, PathFlags, READ_OK, WRITE_OK};
use radix_abi::file::LockKind;
use radix_abi::process::PipeFlags;
use radix_abi::terminal;

use crate::errno::{EINVAL, EOPNOTSUPP, EPERM};
use crate::records::{S_IFBLK, S_IFCHR, S_IFDIR, S_IFIFO, S_IFMT, S_IFREG, S_IFSOCK};

/// Access mode: write only. Read only is 0.
pub const O_WRONLY: u32 = 0o1;
/// Access mode: read and write.
pub const O_RDWR: u32 = 0o2;
/// The bits that hold the access mode.
pub const O_ACCMODE: u32 = 0o3;
/// Make the file if it is not there.
pub const O_CREAT: u32 = 0o100;
/// With `O_CREAT`: fail if the file is there.
pub const O_EXCL: u32 = 0o200;
/// Opening a terminal does not make it the controlling terminal.
pub const O_NOCTTY: u32 = 0o400;
/// Cut the file to 0 bytes.
pub const O_TRUNC: u32 = 0o1000;
/// Every write goes to the end of the file.
pub const O_APPEND: u32 = 0o2000;
/// Reads and writes that would wait fail with EAGAIN instead.
pub const O_NONBLOCK: u32 = 0o4000;
/// Writes wait for the data to reach the disk.
pub const O_DSYNC: u32 = 0o10000;
/// Signal the owner when the file is ready.
pub const O_ASYNC: u32 = 0o20000;
/// Transfer without the page cache.
pub const O_DIRECT: u32 = 0o40000;
/// Allow files over 2 GiB. The kernel adds it itself on x86-64.
pub const O_LARGEFILE: u32 = 0o100000;
/// Fail with ENOTDIR unless the file is a directory.
pub const O_DIRECTORY: u32 = 0o200000;
/// Fail with ELOOP if the last name is a symbolic link.
pub const O_NOFOLLOW: u32 = 0o400000;
/// Do not update the access time.
pub const O_NOATIME: u32 = 0o1000000;
/// Close the descriptor at `exec`.
pub const O_CLOEXEC: u32 = 0o2000000;
/// Writes wait for the data and the file's metadata to reach the disk.
pub const O_SYNC: u32 = 0o4010000;
/// Open a descriptor that only names the file.
pub const O_PATH: u32 = 0o10000000;
/// Make an unnamed file in a directory. It is `O_DIRECTORY` and one more
/// bit, so an old kernel that does not know it fails with EISDIR.
pub const O_TMPFILE: u32 = 0o20200000;

/// The bit of `O_TMPFILE` that `O_DIRECTORY` does not have.
const TMPFILE_BIT: u32 = O_TMPFILE & !O_DIRECTORY;

/// The flags `open` keeps with `O_PATH`: the rest are dropped.
const PATH_KEEPS: u32 = O_PATH | O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC;

/// `dirfd` of the `...at` calls that means the working directory.
pub const AT_FDCWD: i32 = -100;
/// `newfstatat`, `statx`, `utimensat`, `fchownat` and `faccessat2`: do not
/// follow a symbolic link that is the last name.
pub const AT_SYMLINK_NOFOLLOW: u32 = 0x100;
/// `unlinkat`: remove a directory, as `rmdir` does.
pub const AT_REMOVEDIR: u32 = 0x200;
/// `linkat`: follow a symbolic link that is the old name.
pub const AT_SYMLINK_FOLLOW: u32 = 0x400;
/// `statx`: do not mount an automounted directory the path ends at.
pub const AT_NO_AUTOMOUNT: u32 = 0x800;
/// The path may be empty, and then the descriptor itself is meant.
pub const AT_EMPTY_PATH: u32 = 0x1000;
/// `faccessat2`: check with the effective IDs, not the real ones. It has
/// `AT_REMOVEDIR`'s value, for a different call.
pub const AT_EACCESS: u32 = 0x200;
/// The bits of `statx`'s flags (`AT_STATX_FORCE_SYNC` and
/// `AT_STATX_DONT_SYNC`) that say how far to refresh a remote file's
/// attributes. They change nothing for the files Radix has.
pub const AT_STATX_SYNC_TYPE: u32 = 0x6000;

/// `access`: the file exists.
pub const F_OK: u32 = 0;
/// `access`: the caller may read the file.
pub const R_OK: u32 = 4;
/// `access`: the caller may write the file.
pub const W_OK: u32 = 2;
/// `access`: the caller may run the file, or search the directory.
pub const X_OK: u32 = 1;

/// What a Linux `open` asks of the native `directory.open`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Open {
    /// The native flags, as the C library passes them: every open asks
    /// for `ABSOLUTE_LINKS_LEAVE`, and `O_CLOEXEC` is `CLOSE_ON_EXEC`.
    pub flags: OpenFlags,
    /// The rights the capability is made with.
    pub rights: Rights,
    /// Whether the descriptor closes at `exec`: the same fact as
    /// `CLOSE_ON_EXEC` in `flags`, for a caller that sets the slot's flag
    /// itself.
    pub close_on_exec: bool,
}

/// The native open that Linux `flags` ask for, or the errno of the refusal.
///
/// The access mode picks the rights: read only gives `READ`, `O_WRONLY`
/// gives `WRITE` and `O_RDWR` gives both. `O_PATH` opens for reading, as
/// the C library does, since Radix has no narrower right to name a file by,
/// and (as Linux does) drops every flag but `O_DIRECTORY`, `O_NOFOLLOW` and
/// `O_CLOEXEC`. `O_DSYNC`, `O_SYNC`, `O_ASYNC`, `O_DIRECT`, `O_LARGEFILE`
/// and `O_NOATIME` change nothing here, and a bit Linux does not define is
/// ignored, as the raw `open` and `openat` ignore it.
///
/// # Errors
///
/// - EINVAL for the access mode 3, which names no access, and for
///   `O_CREAT` with `O_DIRECTORY`, which Linux 6.4 and later refuse as the
///   C library does. `O_TMPFILE` without `O_DIRECTORY`, with `O_CREAT`, or
///   without write access is EINVAL as in Linux.
/// - EOPNOTSUPP for a valid `O_TMPFILE`: no native filesystem makes an
///   unnamed file.
pub fn open(linux_flags: u32) -> Result<Open, i32> {
    // O_PATH beats everything else, as in Linux's `build_open_how`: what it
    // keeps, it keeps whole, and the access mode is not read.
    let linux_flags =
        if linux_flags & O_PATH != 0 { linux_flags & PATH_KEEPS } else { linux_flags };
    let rights = rights(linux_flags)?;
    if linux_flags & TMPFILE_BIT != 0 {
        return Err(tmpfile_errno(linux_flags));
    }
    if linux_flags & O_CREAT != 0 && linux_flags & O_DIRECTORY != 0 {
        return Err(EINVAL);
    }
    let close_on_exec = linux_flags & O_CLOEXEC != 0;
    Ok(Open { flags: native_flags(linux_flags), rights, close_on_exec })
}

/// The errno of a flag word with `O_TMPFILE`'s own bit set. Linux checks
/// that the whole of `O_TMPFILE` is there, `O_CREAT` is not, and the file
/// may be written, before a filesystem is asked whether it can make one.
fn tmpfile_errno(linux_flags: u32) -> i32 {
    let whole = linux_flags & (O_TMPFILE | O_CREAT) == O_TMPFILE;
    let writable = linux_flags & O_ACCMODE != 0;
    if whole && writable { EOPNOTSUPP } else { EINVAL }
}

/// The rights of an open with `linux_flags`, with `O_PATH` already
/// handled: a path-only open has no access bits left and reads.
fn rights(linux_flags: u32) -> Result<Rights, i32> {
    match linux_flags & O_ACCMODE {
        0 => Ok(Rights::READ),
        O_WRONLY => Ok(Rights::WRITE),
        O_RDWR => Ok(Rights::READ.union(Rights::WRITE)),
        _ => Err(EINVAL),
    }
}

/// The native flags for `linux_flags`, one for each Linux flag that has a
/// native twin.
fn native_flags(linux_flags: u32) -> OpenFlags {
    const TWINS: [(u32, OpenFlags); 8] = [
        (O_CREAT, OpenFlags::CREATE),
        (O_EXCL, OpenFlags::EXCLUSIVE),
        (O_TRUNC, OpenFlags::TRUNCATE),
        (O_DIRECTORY, OpenFlags::DIRECTORY),
        (O_APPEND, OpenFlags::APPEND),
        (O_NONBLOCK, OpenFlags::NON_BLOCKING),
        (O_NOCTTY, OpenFlags::NO_TERMINAL),
        (O_NOFOLLOW, OpenFlags::NO_FOLLOW),
    ];
    // A path beneath a directory capability that meets a symbolic link
    // with an absolute target fails with EXDEV, and the caller follows the
    // link from the root, as the C library does (ADR 0498).
    let mut native = OpenFlags::ABSOLUTE_LINKS_LEAVE;
    for (linux, twin) in TWINS {
        if linux_flags & linux != 0 {
            native = native.union(twin);
        }
    }
    if linux_flags & O_CLOEXEC != 0 {
        native = native.union(OpenFlags::CLOSE_ON_EXEC);
    }
    native
}

/// The native path flags for the `AT_` flags of a call that finds a file
/// by path: `AT_SYMLINK_NOFOLLOW` takes a symbolic link itself.
pub fn path_flags(at_flags: u32) -> PathFlags {
    if at_flags & AT_SYMLINK_NOFOLLOW != 0 { PathFlags::NO_FOLLOW } else { PathFlags::NONE }
}

/// The native access mask for the `mode` of `access` and `faccessat`, which
/// the native `check_access` takes, or EINVAL for a bit that is none of
/// `R_OK`, `W_OK` and `X_OK`.
///
/// The raw `faccessat` call (269) takes no flags; `faccessat2` (439) does,
/// and a C library's `faccessat` wrapper picks between them. The kernel
/// asks the second gate for the name the directory acts for, which a
/// process's real and effective IDs make the same, so `AT_EACCESS` changes
/// nothing.
///
/// # Errors
///
/// EINVAL, as Linux gives it, when `mode` has a bit outside the three.
pub fn access(mode: u32) -> Result<u32, i32> {
    if mode & !(R_OK | W_OK | X_OK) != 0 {
        return Err(EINVAL);
    }
    let mut native = 0;
    for (linux, twin) in [(R_OK, READ_OK), (W_OK, WRITE_OK), (X_OK, EXECUTE_OK)] {
        if mode & linux != 0 {
            native |= twin;
        }
    }
    Ok(native)
}

/// `fcntl`'s descriptor flag: close the descriptor at `exec`.
pub const FD_CLOEXEC: u32 = 1;

/// `fcntl`: copy the descriptor to the lowest free one from the argument.
pub const F_DUPFD: u32 = 0;
/// `fcntl`: read the descriptor's flags.
pub const F_GETFD: u32 = 1;
/// `fcntl`: set the descriptor's flags.
pub const F_SETFD: u32 = 2;
/// `fcntl`: read the description's status flags.
pub const F_GETFL: u32 = 3;
/// `fcntl`: set the description's status flags.
pub const F_SETFL: u32 = 4;
/// `fcntl`: find a record lock that would stand in the way.
pub const F_GETLK: u32 = 5;
/// `fcntl`: take or let go of a record lock without waiting.
pub const F_SETLK: u32 = 6;
/// `fcntl`: take or let go of a record lock, waiting for it.
pub const F_SETLKW: u32 = 7;
/// `fcntl`: [`F_GETLK`] for an open file description lock.
pub const F_OFD_GETLK: u32 = 36;
/// `fcntl`: [`F_SETLK`] for an open file description lock.
pub const F_OFD_SETLK: u32 = 37;
/// `fcntl`: [`F_SETLKW`] for an open file description lock.
pub const F_OFD_SETLKW: u32 = 38;
/// `fcntl`: [`F_DUPFD`], and close the copy at `exec`.
pub const F_DUPFD_CLOEXEC: u32 = 1030;
/// `fcntl`: set the size of a pipe.
pub const F_SETPIPE_SZ: u32 = 1031;
/// `fcntl`: read the size of a pipe.
pub const F_GETPIPE_SZ: u32 = 1032;

/// A record lock's `l_type`: shared.
pub const F_RDLCK: i16 = 0;
/// A record lock's `l_type`: exclusive.
pub const F_WRLCK: i16 = 1;
/// A record lock's `l_type`: none.
pub const F_UNLCK: i16 = 2;

/// `flock`: a shared lock.
pub const LOCK_SH: u32 = 1;
/// `flock`: an exclusive lock.
pub const LOCK_EX: u32 = 2;
/// `flock`: fail instead of waiting.
pub const LOCK_NB: u32 = 4;
/// `flock`: let go of the lock.
pub const LOCK_UN: u32 = 8;

/// `close_range`: make the descriptors of a process of their own first.
pub const CLOSE_RANGE_UNSHARE: u32 = 1 << 1;
/// `close_range`: close the descriptors at `exec` instead of now.
pub const CLOSE_RANGE_CLOEXEC: u32 = 1 << 2;

/// The `ioctl` request that sets or clears non-blocking mode, with an `int`
/// that says which.
pub const FIONBIO: u32 = 0x5421;
/// The `ioctl` request that sets the descriptor's close-on-exec flag.
pub const FIOCLEX: u32 = 0x5451;
/// The `ioctl` request that clears the descriptor's close-on-exec flag.
pub const FIONCLEX: u32 = 0x5450;
/// `TCSBRK`: wait for the output to drain, or send a break.
pub const TCSBRK: u32 = 0x5409;

/// The size of the kernel's `struct termios`, which `TCGETS` writes and
/// `TCSETS` reads: four flag words, the line and 19 control characters.
/// The C libraries' `struct termios` is longer; the kernel's is what the
/// request moves, and a native terminal takes it (radix-terminal(2)).
pub const TERMIOS_SIZE: usize = terminal::LINUX_TERMIOS_SIZE as usize;

/// Where `lseek` counts from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Seek {
    /// `SEEK_SET`: the start of the file.
    Set,
    /// `SEEK_CUR`: the description's offset.
    Current,
    /// `SEEK_END`: the end of the file.
    End,
    /// `SEEK_DATA`: the next offset at or after the given one that holds
    /// data.
    Data,
    /// `SEEK_HOLE`: the next offset at or after the given one that starts
    /// a hole, which is the end of the file where none does.
    Hole,
}

impl Seek {
    /// The `whence` of `lseek` numbered `raw`, or `None` for another.
    pub fn from_raw(raw: u32) -> Option<Seek> {
        match raw {
            0 => Some(Seek::Set),
            1 => Some(Seek::Current),
            2 => Some(Seek::End),
            3 => Some(Seek::Data),
            4 => Some(Seek::Hole),
            _ => None,
        }
    }
}

/// The native lock a record lock's `l_type` asks for. With `getting`, as
/// `F_GETLK` asks, the type names what to look for, and `F_UNLCK` is not
/// one.
///
/// # Errors
///
/// EINVAL for a type that is none of the three, and for `F_UNLCK` when
/// `getting`.
pub fn lock_kind(l_type: i16, getting: bool) -> Result<LockKind, i32> {
    match l_type {
        F_RDLCK => Ok(LockKind::Shared),
        F_WRLCK => Ok(LockKind::Exclusive),
        F_UNLCK if !getting => Ok(LockKind::None),
        _ => Err(EINVAL),
    }
}

/// The `l_type` of a lock of native kind `kind`.
pub fn lock_type(kind: LockKind) -> i16 {
    match kind {
        LockKind::None => F_UNLCK,
        LockKind::Shared => F_RDLCK,
        LockKind::Exclusive => F_WRLCK,
    }
}

/// The native lock a `flock` `operation` asks for, and whether it waits
/// for it: `LOCK_SH`, `LOCK_EX` or `LOCK_UN`, with `LOCK_NB` to not wait.
///
/// # Errors
///
/// EINVAL for any other operation.
pub fn flock_operation(operation: u32) -> Result<(LockKind, bool), i32> {
    let wait = operation & LOCK_NB == 0;
    match operation & !LOCK_NB {
        LOCK_SH => Ok((LockKind::Shared, wait)),
        LOCK_EX => Ok((LockKind::Exclusive, wait)),
        LOCK_UN => Ok((LockKind::None, wait)),
        _ => Err(EINVAL),
    }
}

/// The native flags of the pipe `pipe2` makes with `linux_flags`.
///
/// # Errors
///
/// EINVAL for a flag other than `O_NONBLOCK` and `O_CLOEXEC`, `O_DIRECT`
/// included: Linux's packet mode needs a pipe that keeps the sizes of what
/// was written, which a native pipe does not.
pub fn pipe(linux_flags: u32) -> Result<PipeFlags, i32> {
    if linux_flags & !(O_NONBLOCK | O_CLOEXEC) != 0 {
        return Err(EINVAL);
    }
    let mut native = PipeFlags::NONE;
    if linux_flags & O_NONBLOCK != 0 {
        native = native.union(PipeFlags::NON_BLOCKING);
    }
    if linux_flags & O_CLOEXEC != 0 {
        native = native.union(PipeFlags::CLOSE_ON_EXEC);
    }
    Ok(native)
}

/// The kind of file `mknod` makes for the type bits of `mode`: a regular
/// file (type 0 too), a named pipe or a socket's name.
///
/// # Errors
///
/// EPERM for a device, which only a caller with the privilege makes, and
/// for a directory, which `mkdir` makes; EINVAL for any other type.
pub fn node_kind(mode: u32) -> Result<FileKind, i32> {
    match mode & S_IFMT {
        0 | S_IFREG => Ok(FileKind::File),
        S_IFIFO => Ok(FileKind::Pipe),
        S_IFSOCK => Ok(FileKind::Socket),
        S_IFCHR | S_IFBLK | S_IFDIR => Err(EPERM),
        _ => Err(EINVAL),
    }
}

/// How many bytes of the program's memory the terminal `ioctl` `request`
/// reads or writes, for the requests a native terminal takes
/// (`radix_abi::terminal`, whose numbers are Linux's), and `None` for
/// every other request. The termios requests move the kernel's
/// [`TERMIOS_SIZE`] bytes.
pub fn terminal_request_size(request: u32) -> Option<usize> {
    match request {
        terminal::GET_ATTRIBUTES
        | terminal::SET_ATTRIBUTES
        | terminal::SET_ATTRIBUTES_DRAIN
        | terminal::SET_ATTRIBUTES_FLUSH => Some(TERMIOS_SIZE),
        // A `struct winsize`: four 16-bit numbers.
        terminal::GET_WINDOW_SIZE | terminal::SET_WINDOW_SIZE => Some(8),
        terminal::GET_FOREGROUND
        | terminal::SET_FOREGROUND
        | terminal::GET_SESSION
        | terminal::READABLE
        | terminal::GET_PTY_NUMBER
        | terminal::SET_PTY_LOCK => Some(4),
        // These take their argument by value.
        terminal::FLOW | terminal::FLUSH | terminal::SET_CONTROLLING | terminal::DETACH => Some(0),
        _ => None,
    }
}

/// How many bytes of the program's memory the `ioctl` `request` reads or
/// writes: the terminal requests' ([`terminal_request_size`]), and for any
/// other the size its number carries (`_IOC_SIZE`, bits 16 to 29), which is
/// what Linux's own drivers move for a request made with `_IOR`, `_IOW` or
/// `_IOWR`, and 0 for an old request that carries none.
pub fn ioctl_size(request: u32) -> usize {
    terminal_request_size(request).unwrap_or(((request >> 16) & 0x3fff) as usize)
}
