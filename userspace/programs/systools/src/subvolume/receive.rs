// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! `subvolume receive` (ADR 0462): send streams, Dukovo's or Linux's,
//! made into subvolumes in a folder, as Linux's `btrfs receive` makes
//! them. The Btrfs crate reads and checks the stream; every change goes
//! through the capability of the subvolume being made, by paths relative
//! to it that the crate has checked, and never follows a symbolic link
//! that is a path's last name, so a stream changes nothing outside that
//! subvolume.

use std::fs::File;
use std::io::{self, BufReader, Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use radix_abi::capability::{self, Rights};
use radix_abi::directory::{
    self, AttributeFlags, FileKind, LinkFlags, OpenFlags, PathFlags, SubvolumeInfo as S,
};
use radix_abi::file;
use radix_btrfs::send::items;
use radix_btrfs::send::receive::{self, CloneFrom, Make, Options, Target};
use radix_btrfs::send::stream::Time;

use super::{INODE_BITS, TOP_INODE};
use crate::sys::{self, SYS};
use crate::util::{self, Fail};

/// A status as an I/O error.
fn status(s: radix_abi::Status) -> io::Error {
    sys::io_error(s)
}

/// A path as the directory calls take it: `.` for the subvolume's top.
fn at(path: &[u8]) -> &[u8] {
    if path.is_empty() { b"." } else { path }
}

/// A time as the kernel takes it: nanoseconds since 1970, none before.
fn nanos(time: Time) -> u64 {
    u64::try_from(time.secs)
        .map_or(0, |secs| secs.saturating_mul(1_000_000_000).saturating_add(u64::from(time.nanos)))
}

/// A slot of the capability space, empty, for the target's own use: a
/// copy of `of`'s descriptor, kept open so nothing else takes the slot,
/// whose capability goes.
fn spare_slot(of: &File) -> io::Result<u32> {
    let spare = of.try_clone()?;
    let slot = sys::slot_of(spare.as_raw_fd());
    std::mem::forget(spare);
    capability::delete(&SYS, slot).map_err(status)?;
    Ok(slot)
}

/// The folder received into, through its capability.
struct CapTarget {
    /// The folder received into: its path, and its slot.
    dest: PathBuf,
    dest_slot: u32,
    /// The slot that holds the top of the subvolume being made, if one is.
    top: u32,
    /// The subvolume being made, by name.
    current: Option<Vec<u8>>,
    /// The slot that holds the file being written, and its path.
    file: u32,
    open: Option<Vec<u8>>,
    /// The slot that holds a clone's source while it is cloned.
    source: u32,
    quiet: bool,
}

impl CapTarget {
    fn new(dest: &Path, folder: &File, quiet: bool) -> io::Result<CapTarget> {
        Ok(CapTarget {
            dest: dest.to_path_buf(),
            dest_slot: sys::slot_of(folder.as_raw_fd()),
            top: spare_slot(folder)?,
            current: None,
            file: spare_slot(folder)?,
            open: None,
            source: spare_slot(folder)?,
            quiet,
        })
    }

    /// The subvolume being made.
    fn top(&self) -> io::Result<u32> {
        if self.current.is_some() {
            Ok(self.top)
        } else {
            Err(io::Error::other("no subvolume is being received"))
        }
    }

    /// Lets the file being written go.
    fn close(&mut self) {
        if self.open.take().is_some() {
            let _ = capability::delete(&SYS, self.file);
        }
    }

    /// The file `path` of the subvolume being made, open to write.
    fn file_at(&mut self, path: &[u8]) -> io::Result<u32> {
        if self.open.as_deref() != Some(path) {
            self.close();
            let rights = Rights::READ.union(Rights::WRITE);
            directory::open(&SYS, self.top()?, path, OpenFlags::NO_FOLLOW, rights, self.file, 0)
                .map_err(status)?;
            self.open = Some(path.to_vec());
        }
        Ok(self.file)
    }

    /// Opens the top of the subvolume `name`, just made, as the one being
    /// made.
    fn begin(&mut self, name: &[u8], what: &str) -> io::Result<()> {
        let flags = OpenFlags::DIRECTORY.union(OpenFlags::NO_FOLLOW);
        let rights = Rights::READ.union(Rights::WRITE);
        directory::open(&SYS, self.dest_slot, name, flags, rights, self.top, 0).map_err(status)?;
        self.current = Some(name.to_vec());
        if !self.quiet {
            let name = String::from_utf8_lossy(name);
            let _ = writeln!(io::stderr(), "At {what} {name}");
        }
        Ok(())
    }

    /// Lets the subvolume being made go.
    fn end(&mut self) {
        self.close();
        if self.current.take().is_some() {
            let _ = capability::delete(&SYS, self.top);
        }
    }

    /// The name, in the folder received into, of the subvolume received
    /// as `uuid` at its sender's transaction `ctransid`, or else the one
    /// that is `uuid` at its own transaction `ctransid`, as Linux's
    /// receive finds a snapshot's parent.
    fn find(&self, uuid: &[u8; 16], ctransid: u64) -> io::Result<Vec<u8>> {
        let mut by_uuid = None;
        for entry in std::fs::read_dir(&self.dest)? {
            let entry = entry?;
            let Ok(meta) = entry.metadata() else { continue };
            if !meta.is_dir() || meta.ino() & INODE_BITS != TOP_INODE {
                continue;
            }
            let name = entry.file_name();
            let mut buffer = [0u8; S::SIZE + 255];
            if directory::subvolume(&SYS, self.dest_slot, name.as_bytes(), &mut buffer).is_err() {
                continue;
            }
            let word =
                |at: usize| u64::from_le_bytes(buffer[at..at + 8].try_into().unwrap_or([0; 8]));
            let uuid_at =
                |at: usize| -> [u8; 16] { buffer[at..at + 16].try_into().unwrap_or([0; 16]) };
            if uuid_at(S::OFFSET_RECEIVED_UUID) == *uuid
                && word(S::OFFSET_SENT_TRANSACTION) == ctransid
            {
                return Ok(name.as_bytes().to_vec());
            }
            if uuid_at(S::OFFSET_UUID) == *uuid && word(S::OFFSET_CHANGED_TRANSACTION) == ctransid {
                by_uuid = Some(name.as_bytes().to_vec());
            }
        }
        by_uuid.ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                format!(
                    "no subvolume in {} is the stream's parent; receive its parent first",
                    self.dest.display()
                ),
            )
        })
    }

    /// Takes the subvolume that a failed stream left half made away.
    fn remove_partial(&mut self) -> Option<String> {
        let name = self.current.clone()?;
        self.end();
        directory::remove_subvolume(&SYS, self.dest_slot, &name).ok()?;
        Some(String::from_utf8_lossy(&name).into_owned())
    }
}

impl Target for CapTarget {
    fn subvolume(&mut self, name: &[u8]) -> io::Result<()> {
        self.end();
        directory::make_subvolume(&SYS, self.dest_slot, name).map_err(status)?;
        self.begin(name, "subvol")
    }

    fn snapshot(&mut self, name: &[u8], uuid: &[u8; 16], ctransid: u64) -> io::Result<()> {
        self.end();
        let parent = self.find(uuid, ctransid)?;
        directory::snapshot(&SYS, self.dest_slot, &parent, self.dest_slot, name, false)
            .map_err(status)?;
        self.begin(name, "snapshot")
    }

    fn make(&mut self, path: &[u8], what: Make<'_>) -> io::Result<()> {
        self.close();
        let top = self.top()?;
        let node = |kind, device, mode: u32| {
            directory::make_node(&SYS, top, path, kind, device, mode & 0o7777).map_err(status)
        };
        match what {
            Make::File => node(FileKind::File, 0, 0o600),
            Make::Dir => directory::make_directory(&SYS, top, path, 0o700).map_err(status),
            Make::Symlink { target } => {
                directory::make_link(&SYS, top, path, target).map_err(status)
            }
            Make::Fifo { mode } => node(FileKind::Pipe, 0, mode),
            Make::Socket { mode } => node(FileKind::Socket, 0, mode),
            Make::Device { mode, rdev } => {
                let kind = if mode & items::kind::MASK == items::kind::BLOCK {
                    FileKind::BlockDevice
                } else {
                    FileKind::CharDevice
                };
                let (major, minor) = items::device_parts(rdev);
                node(kind, (u64::from(major) << 32) | u64::from(minor), mode)
            }
        }
    }

    fn rename(&mut self, from: &[u8], to: &[u8]) -> io::Result<()> {
        self.close();
        let top = self.top()?;
        directory::rename(&SYS, top, from, top, to).map_err(status)
    }

    fn link(&mut self, path: &[u8], target: &[u8]) -> io::Result<()> {
        self.close();
        let top = self.top()?;
        directory::link(&SYS, top, target, LinkFlags::NONE, top, path).map_err(status)
    }

    fn unlink(&mut self, path: &[u8]) -> io::Result<()> {
        self.close();
        directory::remove(&SYS, self.top()?, path).map_err(status)
    }

    fn rmdir(&mut self, path: &[u8]) -> io::Result<()> {
        self.unlink(path)
    }

    fn set_xattr(&mut self, path: &[u8], name: &[u8], value: &[u8]) -> io::Result<()> {
        self.close();
        let flags = AttributeFlags::NO_FOLLOW;
        directory::set_attribute(&SYS, self.top()?, at(path), flags, name, value).map_err(status)
    }

    fn remove_xattr(&mut self, path: &[u8], name: &[u8]) -> io::Result<()> {
        self.close();
        directory::remove_attribute(&SYS, self.top()?, at(path), PathFlags::NO_FOLLOW, name)
            .map_err(status)
    }

    fn write(&mut self, path: &[u8], offset: u64, data: &[u8]) -> io::Result<()> {
        let file = self.file_at(path)?;
        let mut done = 0usize;
        while done < data.len() {
            let n =
                file::write(&SYS, file, offset + done as u64, &data[done..]).map_err(status)?.count;
            if n == 0 {
                return Err(io::Error::new(io::ErrorKind::WriteZero, "the disk took nothing"));
            }
            done += n as usize;
        }
        Ok(())
    }

    fn clone_range(
        &mut self,
        path: &[u8],
        offset: u64,
        len: u64,
        from: CloneFrom<'_>,
        from_offset: u64,
    ) -> io::Result<()> {
        let file = self.file_at(path)?;
        let flags = OpenFlags::NO_FOLLOW;
        match from {
            CloneFrom::Here { path } => {
                directory::open(&SYS, self.top()?, path, flags, Rights::READ, self.source, 0)
            }
            CloneFrom::Other { uuid, ctransid, path } => {
                let mut full = self.find(uuid, ctransid)?;
                full.push(b'/');
                full.extend_from_slice(path);
                directory::open(&SYS, self.dest_slot, &full, flags, Rights::READ, self.source, 0)
            }
        }
        .map_err(status)?;
        // Linux's struct file_clone_range: the source, its offset, the
        // length and the offset in the file.
        let mut range = [0u8; 32];
        let words = [u64::from(self.source), from_offset, len, offset];
        for (word, value) in range.as_chunks_mut::<8>().0.iter_mut().zip(words) {
            *word = value.to_le_bytes();
        }
        let address = range.as_ptr() as u64;
        let result = file::control(&SYS, file, file::CLONE_RANGE, address, address, 32);
        let _ = capability::delete(&SYS, self.source);
        result.map(drop).map_err(status)
    }

    fn truncate(&mut self, path: &[u8], size: u64) -> io::Result<()> {
        let file = self.file_at(path)?;
        file::set_size(&SYS, file, size).map_err(status)
    }

    fn chmod(&mut self, path: &[u8], mode: u32) -> io::Result<()> {
        self.close();
        let keep = directory::KEEP;
        directory::set_access(&SYS, self.top()?, at(path), PathFlags::NO_FOLLOW, keep, keep, mode)
            .map_err(status)
    }

    fn chown(&mut self, path: &[u8], uid: u32, gid: u32) -> io::Result<()> {
        self.close();
        let flags = PathFlags::NO_FOLLOW;
        directory::set_access(&SYS, self.top()?, at(path), flags, uid, gid, directory::KEEP)
            .map_err(status)
    }

    fn utimes(&mut self, path: &[u8], atime: Time, mtime: Time) -> io::Result<()> {
        self.close();
        let (atime, mtime) = (nanos(atime), nanos(mtime));
        directory::set_times(&SYS, self.top()?, at(path), atime, mtime, PathFlags::NO_FOLLOW)
            .map_err(status)
    }

    fn finish(&mut self, uuid: &[u8; 16], ctransid: u64) -> io::Result<()> {
        self.close();
        directory::mark_received(&SYS, self.top()?, b".", uuid, ctransid, 0).map_err(status)?;
        self.end();
        Ok(())
    }
}

/// The stream to read: `from`, or standard input.
fn input(from: Option<&Path>) -> Result<Box<dyn Read>, Fail> {
    Ok(match from {
        Some(path) => Box::new(File::open(path).map_err(|e| util::failed(&path.display(), e))?),
        None => Box::new(io::stdin().lock()),
    })
}

/// Writes the stream `from` as text, as `btrfs receive --dump` does.
pub(super) fn dump(from: Option<&Path>) -> Result<(), Fail> {
    let input = BufReader::with_capacity(1 << 20, input(from)?);
    let mut out = io::BufWriter::new(io::stdout().lock());
    receive::dump_all(input, &mut out).map_err(|e| util::failed(&"the stream", e))?;
    out.flush().map_err(|e| util::failed(&"standard output", e))
}

/// Receives the stream `from` into the folder `dest`: each subvolume it
/// holds, made there, read-only and marked received. Stops at the first
/// end with `stop_at_end`.
pub(super) fn run(
    dest: &Path,
    from: Option<&Path>,
    stop_at_end: bool,
    quiet: bool,
) -> Result<(), Fail> {
    let folder = File::open(dest).map_err(|e| util::failed(&dest.display(), e))?;
    if !folder.metadata().is_ok_and(|m| m.is_dir()) {
        return Err(util::fail(format!("{}: not a folder", dest.display())));
    }
    let input = BufReader::with_capacity(1 << 20, input(from)?);
    let mut target =
        CapTarget::new(dest, &folder, quiet).map_err(|e| util::failed(&dest.display(), e))?;
    match receive::receive(input, &mut target, Options { stop_at_end }) {
        Ok(_) => Ok(()),
        Err(error) => {
            let why = util::strip_os_error(&error);
            Err(util::fail(match target.remove_partial() {
                Some(name) => format!("{why}; removed the half-received subvolume {name}"),
                None => why,
            }))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn times_before_1970_are_1970() {
        assert_eq!(nanos(Time { secs: 2, nanos: 5 }), 2_000_000_005);
        assert_eq!(nanos(Time { secs: -1, nanos: 5 }), 0);
        assert_eq!(nanos(Time { secs: i64::MAX, nanos: 0 }), u64::MAX);
    }

    #[test]
    fn the_top_is_dot() {
        assert_eq!(at(b""), b".");
        assert_eq!(at(b"a/b"), b"a/b");
    }
}
