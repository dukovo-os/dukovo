// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Files, for what `std::fs` has no call for: extended attributes, device
//! nodes and named pipes, times set by path without opening the file, the
//! kernel's access check, what a filesystem has room for, a file's lock,
//! and a copy that shares its data.

use std::ffi::{CStr, OsStr, OsString, c_void};
use std::io;
use std::os::fd::{AsFd, AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::path::Path;

use libc::{c_char, c_int};

use crate::{c_path, c_string, checked};

// The C library's extended attribute calls (sys/xattr.h), which the libc
// crate does not declare for Radix.
unsafe extern "C" {
    fn getxattr(path: *const c_char, name: *const c_char, value: *mut c_void, size: usize)
    -> isize;
    fn lgetxattr(
        path: *const c_char,
        name: *const c_char,
        value: *mut c_void,
        size: usize,
    ) -> isize;
    fn setxattr(
        path: *const c_char,
        name: *const c_char,
        value: *const c_void,
        size: usize,
        flags: c_int,
    ) -> c_int;
    fn removexattr(path: *const c_char, name: *const c_char) -> c_int;
    fn listxattr(path: *const c_char, list: *mut c_char, size: usize) -> isize;
    fn llistxattr(path: *const c_char, list: *mut c_char, size: usize) -> isize;
}

/// The largest value of an extended attribute, and the longest list of
/// a file's attributes' names, as on Linux (XATTR_SIZE_MAX and
/// XATTR_LIST_MAX).
const XATTR_SIZE_MAX: usize = 65536;

/// What a call on a path that ends in a symbolic link acts on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Link {
    /// The file the link names.
    Follow,
    /// The link itself.
    Itself,
}

/// The value of `path`'s extended attribute `name` (`system.posix_acl_access`).
///
/// # Errors
///
/// ENODATA when the file has no such attribute, EOPNOTSUPP when its
/// filesystem keeps none, and what finding the file fails with.
pub fn xattr(path: &Path, name: impl AsRef<OsStr>, link: Link) -> io::Result<Vec<u8>> {
    let path = c_path(path)?;
    let name = c_string(name.as_ref().as_bytes())?;
    let get = match link {
        Link::Follow => getxattr,
        Link::Itself => lgetxattr,
    };
    // SAFETY: the path and name are C strings, and the call writes at most
    // `value.len()` bytes into `value`.
    filled(|value| unsafe {
        get(path.as_ptr(), name.as_ptr(), value.as_mut_ptr().cast(), value.len())
    })
}

/// The names of `path`'s extended attributes, in the order the
/// filesystem keeps them.
///
/// # Errors
///
/// EOPNOTSUPP when its filesystem keeps none, and what finding the file
/// fails with.
pub fn xattr_names(path: &Path, link: Link) -> io::Result<Vec<OsString>> {
    let path = c_path(path)?;
    let list = match link {
        Link::Follow => listxattr,
        Link::Itself => llistxattr,
    };
    // SAFETY: the path is a C string, and the call writes at most
    // `names.len()` bytes into `names`.
    let names =
        filled(|names| unsafe { list(path.as_ptr(), names.as_mut_ptr().cast(), names.len()) })?;
    // The names follow one another, each ended by a NUL.
    Ok(names
        .split(|&byte| byte == 0)
        .filter(|name| !name.is_empty())
        .map(|name| OsString::from_vec(name.to_vec()))
        .collect())
}

/// The bytes `call` writes into a buffer it is given, growing the buffer
/// while it fails with ERANGE, up to the largest a value or a list of
/// names can be: what it gives may have grown between two tries.
fn filled(mut call: impl FnMut(&mut [u8]) -> isize) -> io::Result<Vec<u8>> {
    let mut buffer = vec![0u8; 256];
    loop {
        if let Ok(n) = usize::try_from(call(&mut buffer)) {
            buffer.truncate(n);
            return Ok(buffer);
        }
        let error = io::Error::last_os_error();
        if error.raw_os_error() != Some(libc::ERANGE) || buffer.len() >= XATTR_SIZE_MAX {
            return Err(error);
        }
        buffer.resize((buffer.len() * 4).min(XATTR_SIZE_MAX), 0);
    }
}

/// Sets `path`'s extended attribute `name` to `value`, following a
/// symbolic link, making the attribute if the file has none.
///
/// # Errors
///
/// EOPNOTSUPP when the file's filesystem keeps no extended attributes,
/// EPERM or EACCES when the caller may not set it.
pub fn set_xattr(path: &Path, name: impl AsRef<OsStr>, value: &[u8]) -> io::Result<()> {
    let path = c_path(path)?;
    let name = c_string(name.as_ref().as_bytes())?;
    // SAFETY: C strings, and the value's bytes with their length, which
    // the call only reads.
    checked(unsafe {
        setxattr(path.as_ptr(), name.as_ptr(), value.as_ptr().cast(), value.len(), 0)
    })?;
    Ok(())
}

/// Removes `path`'s extended attribute `name`, following a symbolic link.
///
/// # Errors
///
/// ENODATA when the file has no such attribute, EOPNOTSUPP when its
/// filesystem keeps none.
pub fn remove_xattr(path: &Path, name: impl AsRef<OsStr>) -> io::Result<()> {
    let path = c_path(path)?;
    let name = c_string(name.as_ref().as_bytes())?;
    // SAFETY: two C strings, which the call only reads.
    checked(unsafe { removexattr(path.as_ptr(), name.as_ptr()) })?;
    Ok(())
}

/// Makes a node at `path` of the kind and permissions `mode` gives
/// (`S_IFCHR | 0o666`), for device `device` where it is a device's, as
/// mknod(2) does.
///
/// # Errors
///
/// EEXIST when something is at `path`, EPERM when the caller may not make
/// a device.
pub fn make_node(path: &Path, mode: libc::mode_t, device: libc::dev_t) -> io::Result<()> {
    let path = c_path(path)?;
    // SAFETY: the path is a C string, which mknod only reads.
    checked(unsafe { libc::mknod(path.as_ptr(), mode, device) })?;
    Ok(())
}

/// Makes a named pipe at `path` with permissions `mode`, as mkfifo(3)
/// does.
///
/// # Errors
///
/// EEXIST when something is at `path`.
pub fn make_fifo(path: &Path, mode: libc::mode_t) -> io::Result<()> {
    let path = c_path(path)?;
    // SAFETY: the path is a C string, which mkfifo only reads.
    checked(unsafe { libc::mkfifo(path.as_ptr(), mode) })?;
    Ok(())
}

/// What [`set_times`] makes one of a file's times.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Time {
    /// The time it has (UTIME_OMIT).
    Keep,
    /// The time of the call (UTIME_NOW).
    Now,
    /// This time, from the epoch; seconds may be negative.
    At {
        /// Whole seconds.
        seconds: i64,
        /// Nanoseconds past them, below 1,000,000,000.
        nanoseconds: u32,
    },
}

impl Time {
    fn timespec(self) -> libc::timespec {
        match self {
            Time::Keep => libc::timespec { tv_sec: 0, tv_nsec: libc::UTIME_OMIT },
            Time::Now => libc::timespec { tv_sec: 0, tv_nsec: libc::UTIME_NOW },
            Time::At { seconds, nanoseconds } => {
                libc::timespec { tv_sec: seconds, tv_nsec: i64::from(nanoseconds) }
            }
        }
    }
}

/// Sets `path`'s access and modification times, as utimensat(2) does.
///
/// # Errors
///
/// EINVAL for nanoseconds of a second or more, EPERM or EACCES when the
/// caller may not set them.
pub fn set_times(path: &Path, access: Time, modification: Time, link: Link) -> io::Result<()> {
    let path = c_path(path)?;
    let times = [access.timespec(), modification.timespec()];
    let flags = match link {
        Link::Follow => 0,
        Link::Itself => libc::AT_SYMLINK_NOFOLLOW,
    };
    // SAFETY: the path is a C string, and `times` the array of two the
    // call reads.
    checked(unsafe { libc::utimensat(libc::AT_FDCWD, path.as_ptr(), times.as_ptr(), flags) })?;
    Ok(())
}

/// Sets the permissions of what `fd` holds to `mode` (fchmod(2)), as
/// `File::set_permissions` does for a file, for a folder or a descriptor
/// that is no `File`.
///
/// # Errors
///
/// EPERM when this process does not own it.
pub fn set_mode(fd: impl AsFd, mode: libc::mode_t) -> io::Result<()> {
    // SAFETY: fchmod takes a descriptor held here and a number.
    checked(unsafe { libc::fchmod(fd.as_fd().as_raw_fd(), mode) })?;
    Ok(())
}

/// Whether this process's real user and groups may `mode` `path` (`R_OK`,
/// `W_OK`, `X_OK` or several), as access(2) checks it: as which(1) asks
/// whether a file runs. A path the caller cannot reach is false.
#[must_use]
pub fn access(path: &Path, mode: c_int) -> bool {
    check_access(path, mode).is_ok()
}

/// What access(2) says of `mode` on `path` for this process's real user
/// and groups, with its reason when it refuses: `EROFS` when the
/// filesystem refuses every writer, before it looks at who asks.
///
/// # Errors
///
/// The refusal, or what finding `path` fails with.
pub fn check_access(path: &Path, mode: c_int) -> io::Result<()> {
    let path = c_path(path)?;
    // SAFETY: the path is a C string, which the call only reads.
    checked(unsafe { libc::access(path.as_ptr(), mode) })?;
    Ok(())
}

/// Whether the kernel lets this process `mode` `path` (`R_OK`, `W_OK`,
/// `X_OK` or several), for its effective user and groups, as a server that
/// runs as its client checks before it acts (faccessat with AT_EACCESS).
/// A path the caller cannot reach is false.
#[must_use]
pub fn may_access(path: &Path, mode: c_int) -> bool {
    let Ok(path) = c_path(path) else { return false };
    // SAFETY: the path is a C string, which the call only reads.
    unsafe { libc::faccessat(libc::AT_FDCWD, path.as_ptr(), mode, libc::AT_EACCESS) == 0 }
}

/// Opens `name` beneath the folder `dir` with the open(2) flags `flags`
/// and close-on-exec, as openat(2) does, following a link unless
/// `O_NOFOLLOW` is among them. The name is a C string made once, so that
/// a benchmark that opens it again and again times the call alone;
/// `sysapi::dir::Dir` is for names that must not lead elsewhere.
///
/// # Errors
///
/// ENOENT when there is no such file, and what opening it fails with.
pub fn open_at(dir: impl AsFd, name: &CStr, flags: c_int) -> io::Result<OwnedFd> {
    // SAFETY: the name is a C string, which openat only reads; no mode is
    // read without O_CREAT, which an open of an existing file does not
    // need, so 0 is passed.
    let fd = checked(unsafe {
        libc::openat(dir.as_fd().as_raw_fd(), name.as_ptr(), flags | libc::O_CLOEXEC, 0)
    })?;
    // SAFETY: openat just made the descriptor, and nothing else holds it.
    Ok(unsafe { OwnedFd::from_raw_fd(fd) })
}

/// The status of `name` beneath the folder `dir`, following a link, as
/// fstatat(2) gives it; the name a C string made once, as for [`open_at`].
///
/// # Errors
///
/// ENOENT when there is no such file.
pub fn stat_at(dir: impl AsFd, name: &CStr) -> io::Result<libc::stat> {
    let mut status = std::mem::MaybeUninit::<libc::stat>::uninit();
    // SAFETY: the name is a C string, which fstatat only reads, and
    // `status` is room for the status it writes.
    checked(unsafe {
        libc::fstatat(dir.as_fd().as_raw_fd(), name.as_ptr(), status.as_mut_ptr(), 0)
    })?;
    // SAFETY: fstatat succeeded, so it wrote the whole status.
    Ok(unsafe { status.assume_init() })
}

/// What the filesystem holding `path` has: its sizes, room and longest
/// name, as statvfs(3) gives them.
///
/// # Errors
///
/// What finding `path` fails with.
pub fn statvfs(path: &Path) -> io::Result<libc::statvfs> {
    let path = c_path(path)?;
    // SAFETY: all zeros is a valid statvfs, which is numbers alone.
    let mut stats: libc::statvfs = unsafe { std::mem::zeroed() };
    // SAFETY: the path is a C string, and the call fills the statvfs it is
    // given.
    checked(unsafe { libc::statvfs(path.as_ptr(), &raw mut stats) })?;
    Ok(stats)
}

/// How a lock is held: by one holder, or by any number of holders at
/// once, none of whom holds it alone.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lock {
    /// Held by any number at once (LOCK_SH), to read.
    Shared,
    /// Held by one alone (LOCK_EX), to change.
    Exclusive,
}

/// Takes a lock on `file` as flock(2) does: one that the open file holds,
/// with every descriptor that shares it, until it is unlocked or the last
/// of them closes. With `wait`, waits while another holds a lock that
/// conflicts with it; without, fails at once.
///
/// # Errors
///
/// EWOULDBLOCK (`ErrorKind::WouldBlock`) without `wait` when another
/// holds it; EINTR when a signal ends the wait.
pub fn lock(file: impl AsFd, lock: Lock, wait: bool) -> io::Result<()> {
    let operation = match lock {
        Lock::Shared => libc::LOCK_SH,
        Lock::Exclusive => libc::LOCK_EX,
    } | if wait { 0 } else { libc::LOCK_NB };
    // SAFETY: flock takes a descriptor held here and a number.
    checked(unsafe { libc::flock(file.as_fd().as_raw_fd(), operation) })?;
    Ok(())
}

/// Lets go of the lock on `file` that [`lock`] took.
///
/// # Errors
///
/// None in practice for an open file.
pub fn unlock(file: impl AsFd) -> io::Result<()> {
    // SAFETY: flock takes a descriptor held here and a number.
    checked(unsafe { libc::flock(file.as_fd().as_raw_fd(), libc::LOCK_UN) })?;
    Ok(())
}

/// Linux's FICLONE, which Radix takes too (ADR 0139).
const FICLONE: libc::Ioctl = 0x4004_9409;

/// Gives `to`, open to write, the data of `from`, open to read, without
/// copying it: the two share it until one changes, as `cp --reflink`
/// does (FICLONE).
///
/// # Errors
///
/// EOPNOTSUPP or EINVAL when the filesystem makes no such copies, or not
/// of these two; EXDEV when they are on different filesystems. The
/// caller copies the bytes then.
pub fn clone_data(to: impl AsFd, from: impl AsFd) -> io::Result<()> {
    // SAFETY: FICLONE takes the source's descriptor as its argument, an
    // integer, and reads no memory of the caller's.
    checked(unsafe { libc::ioctl(to.as_fd().as_raw_fd(), FICLONE, from.as_fd().as_raw_fd()) })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        Link, Lock, Time, access, check_access, lock, make_fifo, may_access, open_at, set_mode,
        set_times, stat_at, statvfs, unlock, xattr, xattr_names,
    };
    use std::os::unix::fs::{FileTypeExt, MetadataExt};

    #[test]
    fn a_name_beneath_a_folder_opens_and_has_a_status() {
        let dir = scratch("open-at");
        std::fs::write(dir.join("file"), b"four").unwrap();
        let folder = std::fs::File::open(&dir).unwrap();
        let file = std::fs::File::from(open_at(&folder, c"file", libc::O_RDONLY).unwrap());
        assert!(crate::fd::close_on_exec(&file).unwrap());
        assert_eq!(std::io::read_to_string(&file).unwrap(), "four");
        assert_eq!(stat_at(&folder, c"file").unwrap().st_size, 4);
        let missing = open_at(&folder, c"missing", libc::O_RDONLY).unwrap_err();
        assert_eq!(missing.raw_os_error(), Some(libc::ENOENT));
        assert_eq!(stat_at(&folder, c"missing").unwrap_err().raw_os_error(), Some(libc::ENOENT));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    fn scratch(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("sysapi-fs-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_fifo_is_made_where_nothing_is() {
        let dir = scratch("fifo");
        make_fifo(&dir.join("pipe"), 0o600).unwrap();
        assert!(std::fs::metadata(dir.join("pipe")).unwrap().file_type().is_fifo());
        let again = make_fifo(&dir.join("pipe"), 0o600).unwrap_err();
        assert_eq!(again.raw_os_error(), Some(libc::EEXIST));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn times_are_set_to_the_nanosecond_and_kept_where_asked() {
        let dir = scratch("times");
        let file = dir.join("file");
        std::fs::write(&file, b"").unwrap();
        let at = Time::At { seconds: 1_000_000_000, nanoseconds: 5 };
        set_times(&file, at, at, Link::Follow).unwrap();
        set_times(&file, Time::Keep, Time::At { seconds: 7, nanoseconds: 0 }, Link::Follow)
            .unwrap();
        let meta = std::fs::metadata(&file).unwrap();
        assert_eq!((meta.atime(), meta.atime_nsec()), (1_000_000_000, 5));
        assert_eq!((meta.mtime(), meta.mtime_nsec()), (7, 0));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_mode_set_on_a_descriptor_is_what_access_sees() {
        let dir = scratch("mode");
        let path = dir.join("program");
        let file = std::fs::File::create(&path).unwrap();
        set_mode(&file, 0o700).unwrap();
        assert_eq!(std::fs::metadata(&path).unwrap().mode() & 0o777, 0o700);
        assert!(access(&path, libc::X_OK) || crate::process::uid() == 0);
        set_mode(&file, 0o600).unwrap();
        // Root may run a file only when someone may.
        assert!(!access(&path, libc::X_OK));
        assert!(!access(std::path::Path::new("a\0b"), libc::R_OK));
        let refused = check_access(&path, libc::X_OK).map_err(|e| e.raw_os_error());
        assert_eq!(refused, Err(Some(libc::EACCES)));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_path_with_a_nul_byte_is_refused() {
        let error = make_fifo(std::path::Path::new("a\0b"), 0o600).unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
        assert!(!may_access(std::path::Path::new("a\0b"), libc::R_OK));
    }

    #[test]
    fn a_lock_held_alone_keeps_out_another_until_it_is_let_go() {
        let dir = scratch("lock");
        let path = dir.join("file");
        std::fs::write(&path, b"").unwrap();
        let (first, second) =
            (std::fs::File::open(&path).unwrap(), std::fs::File::open(&path).unwrap());
        lock(&first, Lock::Exclusive, false).unwrap();
        let held = lock(&second, Lock::Shared, false).unwrap_err();
        assert_eq!(held.kind(), std::io::ErrorKind::WouldBlock);
        unlock(&first).unwrap();
        lock(&second, Lock::Shared, false).unwrap();
        lock(&first, Lock::Shared, false).unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_file_without_attributes_lists_none() {
        let dir = scratch("xattr");
        let path = dir.join("file");
        std::fs::write(&path, b"").unwrap();
        // The build machine's temporary folder may keep attributes or not.
        match xattr_names(&path, Link::Itself) {
            Ok(names) => {
                assert!(names.iter().all(|name| !name.as_encoded_bytes().starts_with(b"user.")));
            }
            Err(error) => assert_eq!(error.raw_os_error(), Some(libc::EOPNOTSUPP)),
        }
        let missing = xattr(&path, "user.sysapi-none", Link::Itself).unwrap_err();
        assert!(matches!(missing.raw_os_error(), Some(libc::ENODATA | libc::EOPNOTSUPP)));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_root_has_a_filesystem_with_a_longest_name() {
        assert!(statvfs(std::path::Path::new("/")).unwrap().f_namemax > 0);
    }
}
