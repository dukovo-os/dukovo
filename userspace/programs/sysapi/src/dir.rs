// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Folders open as descriptors, whose names are looked up one part at a
//! time and never through a symbolic link (openat(2) and the calls of its
//! family): for a program that works in a folder someone else may change
//! while it does, as bundled(8) works in the folders of the people it
//! installs for, so that nothing there leads it elsewhere.

use std::ffi::{CStr, CString};
use std::io::{self, Read};
use std::os::fd::{AsFd, AsRawFd, BorrowedFd, FromRawFd, OwnedFd};

use crate::checked;

/// The deepest [`Dir::remove_tree`] goes.
pub const DEPTH_MAX: usize = 128;

/// A folder open as a descriptor. Each name is looked up beneath it, one
/// part only, never through a link.
#[derive(Debug)]
pub struct Dir(OwnedFd);

/// `name` as one part of a path, as a C string: not empty, `.` or `..`,
/// and without a `/` or a NUL.
///
/// # Errors
///
/// `ErrorKind::InvalidInput` for a name that is none of these.
pub fn c_name(name: &str) -> io::Result<CString> {
    if name.is_empty() || name.contains('/') || name == "." || name == ".." {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, format!("{name:?}: not a name")));
    }
    crate::c_string(name.as_bytes())
}

/// A descriptor the C library just made, owned.
fn owned(fd: libc::c_int) -> io::Result<OwnedFd> {
    let fd = checked(fd)?;
    // SAFETY: the call that returned `fd` made it, and nothing else holds
    // it.
    Ok(unsafe { OwnedFd::from_raw_fd(fd) })
}

impl Dir {
    /// Opens the folder at `path`, not through a link at its last part.
    ///
    /// # Errors
    ///
    /// ENOTDIR when `path` is no folder, ELOOP when it is a link.
    pub fn open(path: &str) -> io::Result<Dir> {
        let path = crate::c_string(path.as_bytes())?;
        let flags = libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC;
        // SAFETY: the path is a C string, which open only reads.
        owned(unsafe { libc::open(path.as_ptr(), flags) }).map(Dir)
    }

    /// Opens `name` beneath it, without following a link, and without
    /// waiting for a named pipe's writer: a descriptor to read, for a
    /// folder or a file.
    ///
    /// # Errors
    ///
    /// ELOOP for a link, ENOTDIR for a `folder` that is none.
    pub fn open_at(&self, name: &str, folder: bool) -> io::Result<OwnedFd> {
        let mut flags = libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK;
        if folder {
            flags |= libc::O_DIRECTORY;
        }
        self.open_with(name, flags, 0)
    }

    /// Opens the file `name` beneath it to read and write, without
    /// following a link: what changing its owner and mode needs.
    ///
    /// # Errors
    ///
    /// ELOOP for a link, EISDIR for a folder.
    pub fn open_writable(&self, name: &str) -> io::Result<OwnedFd> {
        let flags = libc::O_RDWR | libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK;
        self.open_with(name, flags, 0)
    }

    /// Makes the file `name` beneath it, which must not be there, with
    /// `mode` less the process's mask, and opens it to write: a link at
    /// the name is no more followed than any file is.
    ///
    /// # Errors
    ///
    /// EEXIST when something has the name.
    pub fn create_new(&self, name: &str, mode: libc::mode_t) -> io::Result<OwnedFd> {
        let flags =
            libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW | libc::O_CLOEXEC;
        self.open_with(name, flags, mode)
    }

    fn open_with(&self, name: &str, flags: libc::c_int, mode: libc::mode_t) -> io::Result<OwnedFd> {
        let name = c_name(name)?;
        // SAFETY: the name is a C string, which openat only reads; the mode
        // is read only with O_CREAT.
        owned(unsafe { libc::openat(self.0.as_raw_fd(), name.as_ptr(), flags, mode) })
    }

    /// Opens the folder `name` beneath it, not through a link.
    ///
    /// # Errors
    ///
    /// As [`Dir::open_at`]'s.
    pub fn dir_at(&self, name: &str) -> io::Result<Dir> {
        self.open_at(name, true).map(Dir)
    }

    /// The folder's own owner, mode and identity.
    ///
    /// # Errors
    ///
    /// None in practice for an open folder.
    pub fn stat(&self) -> io::Result<libc::stat> {
        stat(&self.0)
    }

    /// `name`'s, not through a link; none if there is no such name.
    ///
    /// # Errors
    ///
    /// EACCES when the folder may not be searched.
    pub fn stat_at(&self, name: &str) -> io::Result<Option<libc::stat>> {
        let name = c_name(name)?;
        // SAFETY: all zeros is a valid stat, which is numbers alone.
        let mut st: libc::stat = unsafe { std::mem::zeroed() };
        // SAFETY: the name is a C string, and fstatat fills the stat it is
        // given.
        let r = unsafe {
            libc::fstatat(self.0.as_raw_fd(), name.as_ptr(), &raw mut st, libc::AT_SYMLINK_NOFOLLOW)
        };
        match checked(r) {
            Ok(_) => Ok(Some(st)),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// Makes the folder `name` beneath it, with `mode` less the process's
    /// mask.
    ///
    /// # Errors
    ///
    /// EEXIST when something has the name.
    pub fn make(&self, name: &str, mode: libc::mode_t) -> io::Result<()> {
        let name = c_name(name)?;
        // SAFETY: the name is a C string, which mkdirat only reads.
        checked(unsafe { libc::mkdirat(self.0.as_raw_fd(), name.as_ptr(), mode) })?;
        Ok(())
    }

    /// Renames `from` beneath it to `to`, beneath it too, in one step:
    /// what was at `to` is gone.
    ///
    /// # Errors
    ///
    /// ENOENT when there is no `from`.
    pub fn rename(&self, from: &str, to: &str) -> io::Result<()> {
        self.move_to(from, self, to)
    }

    /// Renames `name` beneath it to `new` beneath `to`, in one step: what
    /// was at `new` is gone.
    ///
    /// # Errors
    ///
    /// ENOENT when there is no `name`; EXDEV when `to` is on another file
    /// system.
    pub fn move_to(&self, name: &str, to: &Dir, new: &str) -> io::Result<()> {
        let (name, new) = (c_name(name)?, c_name(new)?);
        // SAFETY: both names are C strings, which renameat only reads.
        checked(unsafe {
            libc::renameat(self.0.as_raw_fd(), name.as_ptr(), to.0.as_raw_fd(), new.as_ptr())
        })?;
        Ok(())
    }

    /// The names in it, but `.` and `..`; a name that is not UTF-8 is
    /// given with U+FFFD for its bytes that are not.
    ///
    /// # Errors
    ///
    /// EMFILE when the process has no descriptor to spare.
    pub fn names(&self) -> io::Result<Vec<String>> {
        // The stream takes a descriptor of its own, which closedir closes;
        // dup shares the folder's offset, so the stream rewinds first.
        let fd = self.0.try_clone()?;
        // SAFETY: fdopendir takes the descriptor, which this function
        // owns, and on success owns it.
        let dir = unsafe { libc::fdopendir(fd.as_raw_fd()) };
        if dir.is_null() {
            return Err(io::Error::last_os_error());
        }
        // The stream owns it now; closedir closes it.
        let _ = std::os::fd::IntoRawFd::into_raw_fd(fd);
        let mut names = Vec::new();
        // SAFETY: `dir` is open until closedir below; an entry readdir
        // gives lives until the next call on the stream, and its name is a
        // C string in it, copied out before the next.
        unsafe {
            libc::rewinddir(dir);
            loop {
                let entry = libc::readdir(dir);
                if entry.is_null() {
                    break;
                }
                let name = CStr::from_ptr((*entry).d_name.as_ptr()).to_string_lossy();
                if name != "." && name != ".." {
                    names.push(name.into_owned());
                }
            }
            libc::closedir(dir);
        }
        Ok(names)
    }

    /// Makes the symbolic link `name` beneath it, leading to `target`.
    ///
    /// # Errors
    ///
    /// EEXIST when something has the name.
    pub fn symlink(&self, target: &str, name: &str) -> io::Result<()> {
        let target = crate::c_string(target.as_bytes())?;
        let name = c_name(name)?;
        // SAFETY: both are C strings, which symlinkat only reads.
        checked(unsafe { libc::symlinkat(target.as_ptr(), self.0.as_raw_fd(), name.as_ptr()) })?;
        Ok(())
    }

    /// Where the link `name` leads; none if `name` is not a link.
    ///
    /// # Errors
    ///
    /// ENOENT when there is no such name.
    pub fn read_link(&self, name: &str) -> io::Result<Option<String>> {
        let name = c_name(name)?;
        let mut buffer = [0u8; 4096];
        // SAFETY: the name is a C string, and readlinkat writes at most
        // the buffer's length into it.
        let n = unsafe {
            libc::readlinkat(
                self.0.as_raw_fd(),
                name.as_ptr(),
                buffer.as_mut_ptr().cast(),
                buffer.len(),
            )
        };
        let Ok(n) = usize::try_from(n) else {
            let e = io::Error::last_os_error();
            return if e.raw_os_error() == Some(libc::EINVAL) { Ok(None) } else { Err(e) };
        };
        Ok(Some(String::from_utf8_lossy(&buffer[..n]).into_owned()))
    }

    /// Gives `name` beneath it, itself and not what a link leads to, the
    /// owner `uid` and group `gid` (fchownat(2) with AT_SYMLINK_NOFOLLOW).
    ///
    /// # Errors
    ///
    /// EPERM without the right to.
    pub fn set_owner_at(&self, name: &str, uid: u32, gid: u32) -> io::Result<()> {
        let name = c_name(name)?;
        // SAFETY: the name is a C string, which fchownat only reads.
        checked(unsafe {
            libc::fchownat(self.0.as_raw_fd(), name.as_ptr(), uid, gid, libc::AT_SYMLINK_NOFOLLOW)
        })?;
        Ok(())
    }

    /// Removes the file or link `name`.
    ///
    /// # Errors
    ///
    /// EISDIR (EPERM on some systems) for a folder.
    pub fn unlink(&self, name: &str) -> io::Result<()> {
        self.unlink_with(name, 0)
    }

    /// Removes the empty folder `name`.
    ///
    /// # Errors
    ///
    /// ENOTEMPTY when it holds anything.
    pub fn remove_dir(&self, name: &str) -> io::Result<()> {
        self.unlink_with(name, libc::AT_REMOVEDIR)
    }

    fn unlink_with(&self, name: &str, flags: libc::c_int) -> io::Result<()> {
        let name = c_name(name)?;
        // SAFETY: the name is a C string, which unlinkat only reads.
        checked(unsafe { libc::unlinkat(self.0.as_raw_fd(), name.as_ptr(), flags) })?;
        Ok(())
    }

    /// Reads the regular file `name` beneath it, not through a link, if it
    /// holds at most `max` bytes; with its owner and mode.
    ///
    /// # Errors
    ///
    /// ELOOP for a link; `ErrorKind::Other` for what is no file or holds
    /// more.
    pub fn read(&self, name: &str, max: u64) -> io::Result<(Vec<u8>, libc::stat)> {
        let fd = self.open_at(name, false)?;
        let st = stat(&fd)?;
        if st.st_mode & libc::S_IFMT != libc::S_IFREG {
            return Err(io::Error::other(format!("{name}: not a file")));
        }
        if u64::try_from(st.st_size).unwrap_or(u64::MAX) > max {
            return Err(io::Error::other(format!("{name}: too large")));
        }
        let mut bytes = Vec::new();
        std::fs::File::from(fd).take(max + 1).read_to_end(&mut bytes)?;
        if bytes.len() as u64 > max {
            return Err(io::Error::other(format!("{name}: too large")));
        }
        Ok((bytes, st))
    }

    /// Removes `name` and all beneath it, not through a link, to
    /// [`DEPTH_MAX`] folders below this one, of which `depth` are above.
    ///
    /// # Errors
    ///
    /// `ErrorKind::Other` beyond [`DEPTH_MAX`]; what a removal fails with.
    pub fn remove_tree(&self, name: &str, depth: usize) -> io::Result<()> {
        let Some(st) = self.stat_at(name)? else { return Ok(()) };
        if st.st_mode & libc::S_IFMT == libc::S_IFDIR {
            if depth > DEPTH_MAX {
                return Err(io::Error::other("too deep"));
            }
            let inner = self.dir_at(name)?;
            for entry in inner.names()? {
                inner.remove_tree(&entry, depth + 1)?;
            }
            return self.remove_dir(name);
        }
        self.unlink(name)
    }
}

impl AsFd for Dir {
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.0.as_fd()
    }
}

impl From<OwnedFd> for Dir {
    /// The folder `fd` holds, as opened with [`Dir::open_at`]: a
    /// descriptor that is no folder fails each call made through it.
    fn from(fd: OwnedFd) -> Dir {
        Dir(fd)
    }
}

impl From<Dir> for OwnedFd {
    fn from(dir: Dir) -> OwnedFd {
        dir.0
    }
}

/// The owner, mode and identity of what `fd` holds, as fstat(2) gives
/// them.
///
/// # Errors
///
/// None in practice for an open descriptor.
pub fn stat(fd: impl AsFd) -> io::Result<libc::stat> {
    // SAFETY: all zeros is a valid stat, which is numbers alone.
    let mut st: libc::stat = unsafe { std::mem::zeroed() };
    // SAFETY: fstat fills the stat it is given.
    checked(unsafe { libc::fstat(fd.as_fd().as_raw_fd(), &raw mut st) })?;
    Ok(st)
}

/// Whether `a` and `b` are of one file.
#[must_use]
pub fn same(a: &libc::stat, b: &libc::stat) -> bool {
    a.st_dev == b.st_dev && a.st_ino == b.st_ino
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use super::{Dir, c_name, same, stat};

    fn scratch(name: &str) -> (std::path::PathBuf, Dir) {
        let path = std::env::temp_dir().join(format!("sysapi-dir-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        let dir = Dir::open(path.to_str().unwrap()).unwrap();
        (path, dir)
    }

    #[test]
    fn a_name_is_one_part_of_a_path() {
        for bad in ["", ".", "..", "a/b", "a\0b"] {
            assert_eq!(
                c_name(bad).unwrap_err().kind(),
                std::io::ErrorKind::InvalidInput,
                "{bad:?}"
            );
        }
        assert!(c_name("a.b").is_ok());
    }

    #[test]
    fn files_and_folders_are_made_read_renamed_and_removed() {
        let (path, dir) = scratch("tree");
        let mut file = std::fs::File::from(dir.create_new("file", 0o600).unwrap());
        file.write_all(b"held").unwrap();
        assert!(dir.create_new("file", 0o600).is_err(), "it is there already");
        dir.make("inner", 0o700).unwrap();
        let inner = dir.dir_at("inner").unwrap();
        drop(inner.create_new("deeper", 0o600).unwrap());
        dir.rename("file", "renamed").unwrap();
        let (bytes, st) = dir.read("renamed", 16).unwrap();
        assert_eq!(bytes, b"held");
        assert!(same(&st, &stat(dir.open_at("renamed", false).unwrap()).unwrap()));
        assert!(dir.read("renamed", 3).is_err(), "more than the most");
        inner.move_to("deeper", &dir, "moved").unwrap();
        assert!(inner.stat_at("deeper").unwrap().is_none());
        dir.move_to("moved", &inner, "deeper").unwrap();
        let mut names = dir.names().unwrap();
        names.sort();
        assert_eq!(names, ["inner", "renamed"]);
        assert!(dir.remove_dir("inner").is_err(), "it holds a file");
        dir.remove_tree("inner", 0).unwrap();
        assert!(dir.stat_at("inner").unwrap().is_none());
        dir.unlink("renamed").unwrap();
        assert!(dir.names().unwrap().is_empty());
        std::fs::remove_dir_all(&path).unwrap();
    }

    #[test]
    fn a_link_is_read_but_never_followed() {
        let (path, dir) = scratch("link");
        dir.make("target", 0o700).unwrap();
        dir.symlink("target", "link").unwrap();
        assert_eq!(dir.read_link("link").unwrap().as_deref(), Some("target"));
        assert_eq!(dir.read_link("target").unwrap(), None, "a folder is no link");
        let followed = dir.dir_at("link").unwrap_err();
        assert!(matches!(followed.raw_os_error(), Some(libc::ELOOP | libc::ENOTDIR)), "{followed}");
        let st = dir.stat_at("link").unwrap().unwrap();
        assert_eq!(st.st_mode & libc::S_IFMT, libc::S_IFLNK);
        // Its own owner, as it is: what this process may give it.
        dir.set_owner_at("link", st.st_uid, st.st_gid).unwrap();
        assert!(Dir::open(path.join("link").to_str().unwrap()).is_err());
        dir.remove_tree("link", 0).unwrap();
        assert!(dir.stat_at("target").unwrap().is_some(), "only the link went");
        std::fs::remove_dir_all(&path).unwrap();
    }
}
