// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! A share's files, as the worker opens them for a PC: names within a
//! share, the opens a session holds, and the locks on byte ranges.
//!
//! The worker runs as the person who signed in, so the kernel decides
//! what they may read and change, by the owners, groups and modes of the
//! files, as at their own shell. A share stops at its folder: a name with
//! `..`, and a symbolic link that leads out of the folder, are refused,
//! as Samba's `wide links = no` refuses them. Windows' names are found in
//! any case: a name that is not there as given is looked for among its
//! folder's names without regard to case.

use std::collections::HashMap;
use std::fs::{self, File};
use std::io;
use std::path::{Path, PathBuf};

use crate::rpc::Pipe;
use crate::wire::{Status, status, status_of};

/// The access rights (MS-SMB2 2.2.13.1).
pub mod access {
    pub const READ_DATA: u32 = 0x0000_0001;
    pub const WRITE_DATA: u32 = 0x0000_0002;
    pub const APPEND_DATA: u32 = 0x0000_0004;
    pub const READ_EA: u32 = 0x0000_0008;
    pub const WRITE_EA: u32 = 0x0000_0010;
    pub const EXECUTE: u32 = 0x0000_0020;
    pub const DELETE_CHILD: u32 = 0x0000_0040;
    pub const READ_ATTRIBUTES: u32 = 0x0000_0080;
    pub const WRITE_ATTRIBUTES: u32 = 0x0000_0100;
    pub const DELETE: u32 = 0x0001_0000;
    pub const READ_CONTROL: u32 = 0x0002_0000;
    pub const WRITE_DAC: u32 = 0x0004_0000;
    pub const WRITE_OWNER: u32 = 0x0008_0000;
    pub const SYNCHRONIZE: u32 = 0x0010_0000;
    pub const MAXIMUM_ALLOWED: u32 = 0x0200_0000;
    pub const GENERIC_ALL: u32 = 0x1000_0000;
    pub const GENERIC_EXECUTE: u32 = 0x2000_0000;
    pub const GENERIC_WRITE: u32 = 0x4000_0000;
    pub const GENERIC_READ: u32 = 0x8000_0000;

    /// Everything a file has: FILE_ALL_ACCESS.
    pub const ALL: u32 = 0x001f_01ff;
    /// What reads: FILE_GENERIC_READ.
    pub const READ: u32 = 0x0012_0089;
    /// What writes: FILE_GENERIC_WRITE.
    pub const WRITE: u32 = 0x0012_0116;
    /// What runs: FILE_GENERIC_EXECUTE.
    pub const RUN: u32 = 0x0012_00a0;
    /// What changes anything, which a read-only share never grants.
    pub const CHANGES: u32 = WRITE_DATA
        | APPEND_DATA
        | WRITE_EA
        | DELETE_CHILD
        | WRITE_ATTRIBUTES
        | DELETE
        | WRITE_DAC
        | WRITE_OWNER;

    /// `mask` with its generic rights spelled out.
    pub fn specific(mask: u32) -> u32 {
        let mut out = mask & !(GENERIC_ALL | GENERIC_EXECUTE | GENERIC_WRITE | GENERIC_READ);
        if mask & GENERIC_ALL != 0 {
            out |= ALL;
        }
        if mask & GENERIC_READ != 0 {
            out |= READ;
        }
        if mask & GENERIC_WRITE != 0 {
            out |= WRITE;
        }
        if mask & GENERIC_EXECUTE != 0 {
            out |= RUN;
        }
        out
    }
}

/// A tree: a share a session connected to.
#[derive(Clone, Debug)]
pub struct Tree {
    /// The share's name.
    pub name: String,
    /// Its folder, its links followed: `None` for IPC$.
    pub root: Option<PathBuf>,
    /// Whether nothing may change through it.
    pub read_only: bool,
}

/// What an open is of.
#[derive(Debug)]
pub enum Kind {
    /// A file.
    File,
    /// A folder.
    Directory,
    /// The srvsvc pipe.
    Pipe(Box<Pipe>),
}

/// A folder's names, as QUERY_DIRECTORY goes through them.
#[derive(Debug, Default)]
pub struct Listing {
    /// The names that match, `.` and `..` first.
    pub names: Vec<String>,
    /// How many have been given.
    pub at: usize,
}

/// An open file, folder or pipe.
#[derive(Debug)]
pub struct Open {
    /// The tree it is in.
    pub tree: u32,
    /// Its path.
    pub path: PathBuf,
    /// Its name within the share, with backslashes and one first.
    pub name: String,
    /// What it is.
    pub kind: Kind,
    /// The file's data, when access to it was asked for.
    pub file: Option<File>,
    /// The access it was granted.
    pub access: u32,
    /// Whether it is to be deleted when closed.
    pub delete_on_close: bool,
    /// Its position, which FilePositionInformation sets and gives.
    pub position: u64,
    /// A folder's listing.
    pub listing: Option<Listing>,
}

/// Reads a name a PC gives in a share: backslashes between its parts, no
/// part empty, `.` or `..`, and none with a character Windows keeps out
/// of names. A trailing `::$DATA`, the file's own stream, is dropped; any
/// other stream is not found, as smbd keeps none.
pub fn parts(name: &str) -> Result<Vec<&str>, Status> {
    let name = name.strip_suffix("::$DATA").unwrap_or(name);
    let name = name.trim_start_matches('\\');
    if name.is_empty() {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    for part in name.split('\\') {
        if part.is_empty() || part == "." || part == ".." {
            return Err(status::OBJECT_NAME_INVALID);
        }
        if part.contains(':') {
            return Err(status::OBJECT_NAME_NOT_FOUND);
        }
        if part.chars().any(|c| c.is_control() || "/*?<>\"|".contains(c)) {
            return Err(status::OBJECT_NAME_INVALID);
        }
        out.push(part);
    }
    Ok(out)
}

/// The one name in `folder` that is `part` in another case, if there is
/// one and only one.
fn other_case(folder: &Path, part: &str) -> Option<String> {
    let wanted: Vec<char> = part.chars().flat_map(char::to_lowercase).collect();
    let mut found = None;
    for entry in fs::read_dir(folder).ok()?.flatten() {
        let Ok(name) = entry.file_name().into_string() else { continue };
        if name.chars().flat_map(char::to_lowercase).eq(wanted.iter().copied()) {
            if found.is_some() {
                return None;
            }
            found = Some(name);
        }
    }
    found
}

/// The path of `name` within the share at `root`, its parts found in any
/// case. A part before the last that is not there is
/// `STATUS_OBJECT_PATH_NOT_FOUND`; the last may not be there, for a new
/// file. With `exact_last`, the last is taken as given, as a rename's
/// new name is.
pub fn resolve(root: &Path, name: &str, exact_last: bool) -> Result<PathBuf, Status> {
    let parts = parts(name)?;
    let mut path = root.to_path_buf();
    let count = parts.len();
    for (i, part) in parts.into_iter().enumerate() {
        let last = i + 1 == count;
        let given = path.join(part);
        if last && exact_last || fs::symlink_metadata(&given).is_ok() {
            path = given;
        } else if let Some(found) = other_case(&path, part) {
            path.push(found);
        } else if last {
            path = given;
        } else {
            return Err(status::OBJECT_PATH_NOT_FOUND);
        }
    }
    within(root, &path)?;
    Ok(path)
}

/// Checks that `path`, its links followed, is within `root`: the path
/// itself when it is there, or its folder for a new name.
pub fn within(root: &Path, path: &Path) -> Result<(), Status> {
    let real = match fs::canonicalize(path) {
        Ok(real) => real,
        Err(err) if err.kind() == io::ErrorKind::NotFound => {
            let Some(parent) = path.parent() else { return Ok(()) };
            match fs::canonicalize(parent) {
                Ok(real) => real,
                Err(err) if err.kind() == io::ErrorKind::NotFound => {
                    return Err(status::OBJECT_PATH_NOT_FOUND);
                }
                Err(err) => return Err(status_of(&err)),
            }
        }
        Err(err) => return Err(status_of(&err)),
    };
    if real.starts_with(root) {
        Ok(())
    } else {
        // A link out of the share.
        Err(status::ACCESS_DENIED)
    }
}

/// The name of `path` within the share at `root`, as Windows writes it:
/// a backslash first and between the parts.
pub fn share_name(root: &Path, path: &Path) -> String {
    let inner = path.strip_prefix(root).unwrap_or(path);
    let mut name = String::from("\\");
    let parts: Vec<String> = inner.iter().map(|p| p.to_string_lossy().into_owned()).collect();
    name.push_str(&parts.join("\\"));
    name
}

/// A lock on a range of a file's bytes (MS-SMB2 3.3.5.14), within the
/// session: Windows' locks are kept from reads and writes by other opens.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Lock {
    /// The file: its device and number.
    pub file: (u64, u64),
    /// The open that holds it.
    pub open: u64,
    /// Its first byte.
    pub offset: u64,
    /// Its length.
    pub length: u64,
    /// Whether no other open may read it, or lock it at all.
    pub exclusive: bool,
}

impl Lock {
    /// Whether the two ranges share a byte.
    fn overlaps(&self, offset: u64, length: u64) -> bool {
        length != 0
            && self.length != 0
            && offset < self.offset.saturating_add(self.length)
            && self.offset < offset.saturating_add(length)
    }
}

/// The locks of a session's opens.
#[derive(Debug, Default)]
pub struct Locks(pub Vec<Lock>);

impl Locks {
    /// Takes `lock`, unless another holds a range of it in a way that
    /// keeps it out.
    pub fn take(&mut self, lock: Lock) -> Result<(), Status> {
        let conflict = self.0.iter().any(|held| {
            held.file == lock.file
                && held.overlaps(lock.offset, lock.length)
                && (held.exclusive || lock.exclusive)
        });
        if conflict {
            return Err(status::LOCK_NOT_GRANTED);
        }
        self.0.push(lock);
        Ok(())
    }

    /// Lets go of the lock `open` holds on exactly this range.
    pub fn release(
        &mut self,
        file: (u64, u64),
        open: u64,
        offset: u64,
        length: u64,
    ) -> Result<(), Status> {
        let at = self
            .0
            .iter()
            .position(|l| {
                l.file == file && l.open == open && l.offset == offset && l.length == length
            })
            .ok_or(status::RANGE_NOT_LOCKED)?;
        self.0.remove(at);
        Ok(())
    }

    /// Lets go of every lock of `open`.
    pub fn drop_open(&mut self, open: u64) {
        self.0.retain(|l| l.open != open);
    }

    /// Whether `open` may read (or, with `write`, write) the range: no
    /// other open holds an exclusive lock on it, or for a write any lock.
    pub fn allows(
        &self,
        file: (u64, u64),
        open: u64,
        offset: u64,
        length: u64,
        write: bool,
    ) -> Result<(), Status> {
        let blocked = self.0.iter().any(|held| {
            held.file == file
                && held.open != open
                && held.overlaps(offset, length)
                && (held.exclusive || write)
        });
        if blocked { Err(status::FILE_LOCK_CONFLICT) } else { Ok(()) }
    }
}

/// The opens of a session, by their numbers.
#[derive(Debug, Default)]
pub struct Opens {
    /// Each, by its number, which is both halves of its FileId.
    pub all: HashMap<u64, Open>,
    next: u64,
}

impl Opens {
    /// Adds `open`: its number.
    pub fn add(&mut self, open: Open) -> u64 {
        self.next += 1;
        self.all.insert(self.next, open);
        self.next
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_read_as_windows_gives_them() {
        assert_eq!(parts(""), Ok(vec![]));
        assert_eq!(parts("\\"), Ok(vec![]));
        assert_eq!(parts("a\\b c.txt"), Ok(vec!["a", "b c.txt"]));
        assert_eq!(parts("a::$DATA"), Ok(vec!["a"]));
        assert_eq!(parts("a:Zone.Identifier"), Err(status::OBJECT_NAME_NOT_FOUND));
        for bad in ["a\\..\\b", "..", "a\\\\b", "a/b", "a*", "a\0b", ".\\a"] {
            assert_eq!(parts(bad), Err(status::OBJECT_NAME_INVALID), "{bad:?}");
        }
    }

    #[test]
    fn names_are_found_in_any_case_and_stay_in_the_share() {
        let dir = std::env::temp_dir().join(format!("smbd-fs-{}", std::process::id()));
        let root = dir.join("share");
        fs::create_dir_all(root.join("Folder")).unwrap();
        fs::write(root.join("Folder/Notes.TXT"), b"x").unwrap();
        fs::write(dir.join("secret"), b"x").unwrap();
        std::os::unix::fs::symlink(dir.join("secret"), root.join("out")).unwrap();
        let root = fs::canonicalize(&root).unwrap();
        assert_eq!(resolve(&root, "folder\\notes.txt", false), Ok(root.join("Folder/Notes.TXT")));
        assert_eq!(resolve(&root, "FOLDER\\new.txt", false), Ok(root.join("Folder/new.txt")));
        assert_eq!(resolve(&root, "Folder\\notes.txt", true), Ok(root.join("Folder/notes.txt")));
        assert_eq!(resolve(&root, "missing\\new.txt", false), Err(status::OBJECT_PATH_NOT_FOUND));
        assert_eq!(resolve(&root, "out", false), Err(status::ACCESS_DENIED));
        assert_eq!(share_name(&root, &root.join("Folder/Notes.TXT")), "\\Folder\\Notes.TXT");
        assert_eq!(share_name(&root, &root), "\\");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn locks_keep_others_out() {
        let file = (1, 2);
        let mut locks = Locks::default();
        let lock = |open, offset, length, exclusive| Lock { file, open, offset, length, exclusive };
        locks.take(lock(1, 0, 10, true)).unwrap();
        assert_eq!(locks.take(lock(2, 5, 10, false)), Err(status::LOCK_NOT_GRANTED));
        locks.take(lock(2, 10, 10, false)).unwrap();
        locks.take(lock(3, 10, 10, false)).unwrap();
        assert_eq!(locks.allows(file, 2, 0, 1, false), Err(status::FILE_LOCK_CONFLICT));
        assert_eq!(locks.allows(file, 1, 0, 1, true), Ok(()));
        assert_eq!(locks.allows(file, 1, 12, 1, true), Err(status::FILE_LOCK_CONFLICT));
        assert_eq!(locks.allows(file, 1, 12, 1, false), Ok(()));
        assert_eq!(locks.release(file, 2, 0, 10), Err(status::RANGE_NOT_LOCKED));
        locks.release(file, 1, 0, 10).unwrap();
        locks.drop_open(2);
        locks.drop_open(3);
        assert!(locks.0.is_empty());
    }
}
