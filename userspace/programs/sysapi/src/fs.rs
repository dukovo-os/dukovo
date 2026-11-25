// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Files, for what `std::fs` has no call for: extended attributes, device
//! nodes and named pipes, times set by path without opening the file, the
//! kernel's access check, and what a filesystem has room for.

use std::ffi::c_void;
use std::io;
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
}

/// The largest value of an extended attribute, as on Linux
/// (XATTR_SIZE_MAX).
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
pub fn xattr(path: &Path, name: &str, link: Link) -> io::Result<Vec<u8>> {
    let path = c_path(path)?;
    let name = c_string(name.as_bytes())?;
    let mut value = vec![0u8; 256];
    loop {
        // SAFETY: the path and name are C strings, and the call writes at
        // most `value.len()` bytes into `value`.
        let n = unsafe {
            let get = match link {
                Link::Follow => getxattr,
                Link::Itself => lgetxattr,
            };
            get(path.as_ptr(), name.as_ptr(), value.as_mut_ptr().cast(), value.len())
        };
        if let Ok(n) = usize::try_from(n) {
            value.truncate(n);
            return Ok(value);
        }
        let error = io::Error::last_os_error();
        // ERANGE: the value is longer than the buffer, and may have grown
        // since the last try, so the buffer grows to the largest a value
        // can be.
        if error.raw_os_error() != Some(libc::ERANGE) || value.len() >= XATTR_SIZE_MAX {
            return Err(error);
        }
        value.resize((value.len() * 4).min(XATTR_SIZE_MAX), 0);
    }
}

/// Sets `path`'s extended attribute `name` to `value`, following a
/// symbolic link, making the attribute if the file has none.
///
/// # Errors
///
/// EOPNOTSUPP when the file's filesystem keeps no extended attributes,
/// EPERM or EACCES when the caller may not set it.
pub fn set_xattr(path: &Path, name: &str, value: &[u8]) -> io::Result<()> {
    let path = c_path(path)?;
    let name = c_string(name.as_bytes())?;
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
pub fn remove_xattr(path: &Path, name: &str) -> io::Result<()> {
    let path = c_path(path)?;
    let name = c_string(name.as_bytes())?;
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

#[cfg(test)]
mod tests {
    use super::{Link, Time, make_fifo, may_access, set_times, statvfs};
    use std::os::unix::fs::{FileTypeExt, MetadataExt};

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
    fn a_path_with_a_nul_byte_is_refused() {
        let error = make_fifo(std::path::Path::new("a\0b"), 0o600).unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
        assert!(!may_access(std::path::Path::new("a\0b"), libc::R_OK));
    }

    #[test]
    fn the_root_has_a_filesystem_with_a_longest_name() {
        assert!(statvfs(std::path::Path::new("/")).unwrap().f_namemax > 0);
    }
}
