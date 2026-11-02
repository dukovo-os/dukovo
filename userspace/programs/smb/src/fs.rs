// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The share as a filesystem, served with fuser through /devices/fuse
//! (ADR 0226).
//!
//! A node number stands for a path of the share, from the first time the
//! kernel looks it up until it forgets it; a rename moves the paths
//! beneath. SMB has no owners and modes as POSIX has: every file is the
//! mounting user's, or the `uid` and `gid` given, with `file_mode` or
//! `dir_mode`, less the write bits when the server marks it read-only, as
//! Linux's cifs shows a share without the POSIX extensions. A chmod that
//! takes the write bits away marks the file read-only, and one that gives
//! them back unmarks it; owners do not change. Symbolic and hard links
//! and device nodes are not made (EPERM).
//!
//! A directory is listed once, when it is opened, so that a listing goes
//! on rightly while its names are removed. Files are opened on the server
//! when the kernel opens them, and their handles closed when it releases
//! them.

use std::collections::HashMap;
use std::ffi::OsStr;
use std::time::{Duration, SystemTime};

use fuser::{
    FileAttr, FileType, Filesystem, KernelConfig, ReplyAttr, ReplyCreate, ReplyData,
    ReplyDirectory, ReplyEmpty, ReplyEntry, ReplyOpen, ReplyStatfs, ReplyWrite, Request, TimeOrNow,
};
use libc::c_int;
use smb2::msg::create::CreateDisposition;
use smb2::types::FileId;
use smb2::types::flags::FileAttributes;

use crate::share::{self, MAX_IO, Share};

/// How long the kernel keeps what it is told: the server's files change
/// under other clients too.
const TTL: Duration = Duration::from_secs(1);

/// The root's node number, as the protocol fixes it.
const ROOT: u64 = 1;

/// Who the files are shown as: owner, group and modes.
#[derive(Clone, Copy)]
pub struct Shown {
    pub uid: u32,
    pub gid: u32,
    pub file_mode: u16,
    pub dir_mode: u16,
}

/// A node: its path, and how many times the kernel was given it.
struct Node {
    path: String,
    lookups: u64,
}

/// The nodes the kernel holds, by number and by path.
pub struct Nodes {
    nodes: HashMap<u64, Node>,
    by_path: HashMap<String, u64>,
    next: u64,
}

impl Nodes {
    /// The root's alone, at `root`, a folder of the share or its top ("").
    pub fn new(root: &str) -> Nodes {
        let mut nodes = HashMap::new();
        nodes.insert(ROOT, Node { path: root.to_string(), lookups: 0 });
        let mut by_path = HashMap::new();
        by_path.insert(root.to_string(), ROOT);
        Nodes { nodes, by_path, next: ROOT + 1 }
    }

    /// The path of node `ino`: ENOENT if the kernel never had it, or if
    /// what it named is gone.
    pub fn path(&self, ino: u64) -> Result<String, c_int> {
        match self.nodes.get(&ino) {
            Some(node) if ino == ROOT || !node.path.is_empty() => Ok(node.path.clone()),
            _ => Err(libc::ENOENT),
        }
    }

    /// The node number of `path`, given to the kernel once more.
    pub fn give(&mut self, path: &str) -> u64 {
        if let Some(&ino) = self.by_path.get(path) {
            if let Some(node) = self.nodes.get_mut(&ino) {
                node.lookups += 1;
            }
            return ino;
        }
        let ino = self.next;
        self.next += 1;
        self.nodes.insert(ino, Node { path: path.to_string(), lookups: 1 });
        self.by_path.insert(path.to_string(), ino);
        ino
    }

    /// The node number `path` has, if the kernel holds one.
    pub fn known(&self, path: &str) -> Option<u64> {
        self.by_path.get(path).copied()
    }

    /// The kernel lets go of `count` of node `ino`'s lookups; the node goes
    /// with its last.
    pub fn forget(&mut self, ino: u64, count: u64) {
        let Some(node) = self.nodes.get_mut(&ino) else { return };
        node.lookups = node.lookups.saturating_sub(count);
        if node.lookups == 0 && ino != ROOT {
            let path = std::mem::take(&mut node.path);
            self.nodes.remove(&ino);
            if self.by_path.get(&path) == Some(&ino) {
                self.by_path.remove(&path);
            }
        }
    }

    /// Moves the paths of `from` and those beneath it to `to`; what was at
    /// `to` is gone, replaced.
    pub fn moved(&mut self, from: &str, to: &str) {
        self.removed(to);
        let prefix = format!("{from}/");
        let moving: Vec<(String, u64)> = self
            .by_path
            .iter()
            .filter(|(path, _)| *path == from || path.starts_with(&prefix))
            .map(|(path, &ino)| (path.clone(), ino))
            .collect();
        for (path, _) in &moving {
            self.by_path.remove(path);
        }
        for (path, ino) in moving {
            let new = format!("{to}{}", &path[from.len()..]);
            if let Some(node) = self.nodes.get_mut(&ino) {
                node.path.clone_from(&new);
            }
            self.by_path.insert(new, ino);
        }
    }

    /// `path` is gone: its node stays until forgotten, but names nothing.
    pub fn removed(&mut self, path: &str) {
        if let Some(ino) = self.by_path.remove(path)
            && let Some(node) = self.nodes.get_mut(&ino)
        {
            node.path = String::new();
        }
    }
}

/// An open directory: what it held when opened.
struct Listing {
    entries: Vec<(String, FileType)>,
}

/// The filesystem.
pub struct SmbFs {
    share: Share,
    shown: Shown,
    read_only: bool,
    nodes: Nodes,
    files: HashMap<u64, FileId>,
    listings: HashMap<u64, Listing>,
    next_handle: u64,
}

/// `parent`'s path joined with `name`.
fn join(parent: &str, name: &str) -> String {
    if parent.is_empty() { name.to_string() } else { format!("{parent}/{name}") }
}

/// A name the kernel gives, as text: SMB names are Unicode.
fn text(name: &OsStr) -> Result<&str, c_int> {
    name.to_str().ok_or(libc::EINVAL)
}

/// Whether open(2)'s `flags` ask to write.
fn writes(flags: i32) -> bool {
    flags & libc::O_ACCMODE != libc::O_RDONLY
}

impl SmbFs {
    /// The share's files, from its folder `root` ("" for its top).
    pub fn new(share: Share, root: &str, shown: Shown, read_only: bool) -> SmbFs {
        SmbFs {
            share,
            shown,
            read_only,
            nodes: Nodes::new(root),
            files: HashMap::new(),
            listings: HashMap::new(),
            next_handle: 1,
        }
    }

    fn path(&self, ino: u64) -> Result<String, c_int> {
        self.nodes.path(ino)
    }

    fn child(&self, parent: u64, name: &OsStr) -> Result<String, c_int> {
        Ok(join(&self.path(parent)?, text(name)?))
    }

    /// A node's attributes from what the server says of it.
    fn attr(&self, ino: u64, info: &smb2::FileInfo) -> FileAttr {
        self.attributes(
            ino,
            info.is_directory,
            info.size,
            info.attributes.0,
            [info.accessed, info.modified, info.changed, info.created],
        )
    }

    fn attributes(
        &self,
        ino: u64,
        directory: bool,
        size: u64,
        attributes: u32,
        [accessed, modified, changed, created]: [smb2::pack::FileTime; 4],
    ) -> FileAttr {
        let (kind, mut perm) = if directory {
            (FileType::Directory, self.shown.dir_mode)
        } else {
            (FileType::RegularFile, self.shown.file_mode)
        };
        if (attributes & FileAttributes::READONLY != 0 && !directory) || self.read_only {
            perm &= !0o222;
        }
        FileAttr {
            ino,
            size,
            blocks: size.div_ceil(512),
            atime: share::system_time(accessed),
            mtime: share::system_time(modified),
            ctime: share::system_time(changed),
            crtime: share::system_time(created),
            kind,
            perm,
            nlink: if directory { 2 } else { 1 },
            uid: self.shown.uid,
            gid: self.shown.gid,
            rdev: 0,
            blksize: MAX_IO,
            flags: 0,
        }
    }

    /// Looks `path` up: its node, given to the kernel, and attributes.
    fn entry(&mut self, path: &str) -> Result<FileAttr, c_int> {
        let info = self.share.stat(path).map_err(|e| share::errno(&e))?;
        let ino = self.nodes.give(path);
        Ok(self.attr(ino, &info))
    }

    fn handle(&mut self) -> u64 {
        self.next_handle += 1;
        self.next_handle - 1
    }

    fn file(&self, fh: u64) -> Result<FileId, c_int> {
        self.files.get(&fh).copied().ok_or(libc::EBADF)
    }

    fn writable(&self) -> Result<(), c_int> {
        if self.read_only { Err(libc::EROFS) } else { Ok(()) }
    }

    fn set_attributes(
        &mut self,
        path: &str,
        mode: Option<u32>,
        times: Option<(Option<SystemTime>, Option<SystemTime>)>,
    ) -> Result<(), c_int> {
        let mut set = share::times(None, None);
        if let Some((accessed, modified)) = times {
            set = share::times(accessed, modified);
        }
        if let Some(mode) = mode {
            let info = self.share.stat(path).map_err(|e| share::errno(&e))?;
            let mut attributes = info.attributes.0;
            if mode & 0o200 == 0 {
                attributes |= FileAttributes::READONLY;
            } else {
                attributes &= !FileAttributes::READONLY;
            }
            if attributes != info.attributes.0 {
                // NORMAL stands alone: with others, it is left out; and
                // none at all, which SET_INFO takes as no change, is it.
                if attributes & !FileAttributes::NORMAL != 0 {
                    attributes &= !FileAttributes::NORMAL;
                } else {
                    attributes = FileAttributes::NORMAL;
                }
                set.attributes = Some(FileAttributes(attributes));
            }
        }
        if set.accessed.is_none() && set.modified.is_none() && set.attributes.is_none() {
            return Ok(());
        }
        self.share.set_times(path, set).map_err(|e| share::errno(&e))
    }

    fn set_size(&mut self, path: &str, fh: Option<u64>, size: u64) -> Result<(), c_int> {
        if let Some(file) = fh.and_then(|fh| self.files.get(&fh).copied()) {
            return self.share.set_size(file, size).map_err(|e| share::errno(&e));
        }
        let (file, _, _) = self
            .share
            .open(path, true, CreateDisposition::FileOpen, false)
            .map_err(|e| share::errno(&e))?;
        let set = self.share.set_size(file, size);
        let _ = self.share.close(file);
        set.map_err(|e| share::errno(&e))
    }
}

/// A time to set, now if `Now`.
fn when(time: TimeOrNow) -> SystemTime {
    match time {
        TimeOrNow::SpecificTime(time) => time,
        TimeOrNow::Now => SystemTime::now(),
    }
}

impl Filesystem for SmbFs {
    fn init(&mut self, _req: &Request<'_>, config: &mut KernelConfig) -> Result<(), c_int> {
        // One credit's worth to each READ and WRITE (see share.rs).
        let _ = config.set_max_write(MAX_IO);
        Ok(())
    }

    fn destroy(&mut self) {
        // Unmounted: the handles still open are closed, and the share left.
        for (_, file) in self.files.drain() {
            let _ = self.share.close(file);
        }
        self.share.disconnect();
    }

    fn lookup(&mut self, _req: &Request<'_>, parent: u64, name: &OsStr, reply: ReplyEntry) {
        match self.child(parent, name).and_then(|path| self.entry(&path)) {
            Ok(attr) => reply.entry(&TTL, &attr, 0),
            Err(err) => reply.error(err),
        }
    }

    fn forget(&mut self, _req: &Request<'_>, ino: u64, nlookup: u64) {
        self.nodes.forget(ino, nlookup);
    }

    fn getattr(&mut self, _req: &Request<'_>, ino: u64, _fh: Option<u64>, reply: ReplyAttr) {
        let stat =
            self.path(ino).and_then(|path| self.share.stat(&path).map_err(|e| share::errno(&e)));
        match stat {
            Ok(info) => reply.attr(&TTL, &self.attr(ino, &info)),
            Err(err) => reply.error(err),
        }
    }

    fn setattr(
        &mut self,
        _req: &Request<'_>,
        ino: u64,
        mode: Option<u32>,
        uid: Option<u32>,
        gid: Option<u32>,
        size: Option<u64>,
        atime: Option<TimeOrNow>,
        mtime: Option<TimeOrNow>,
        _ctime: Option<SystemTime>,
        fh: Option<u64>,
        _crtime: Option<SystemTime>,
        _chgtime: Option<SystemTime>,
        _bkuptime: Option<SystemTime>,
        _flags: Option<u32>,
        reply: ReplyAttr,
    ) {
        let done = (|| -> Result<FileAttr, c_int> {
            self.writable()?;
            let path = self.path(ino)?;
            // SMB keeps no owners: the files stay the ones shown.
            if uid.is_some_and(|u| u != self.shown.uid) || gid.is_some_and(|g| g != self.shown.gid)
            {
                return Err(libc::EPERM);
            }
            if let Some(size) = size {
                self.set_size(&path, fh, size)?;
            }
            let times =
                (atime.is_some() || mtime.is_some()).then(|| (atime.map(when), mtime.map(when)));
            self.set_attributes(&path, mode, times)?;
            let info = self.share.stat(&path).map_err(|e| share::errno(&e))?;
            Ok(self.attr(ino, &info))
        })();
        match done {
            Ok(attr) => reply.attr(&TTL, &attr),
            Err(err) => reply.error(err),
        }
    }

    fn mknod(
        &mut self,
        _req: &Request<'_>,
        _parent: u64,
        _name: &OsStr,
        _mode: u32,
        _umask: u32,
        _rdev: u32,
        reply: ReplyEntry,
    ) {
        // Files are made with CREATE; device nodes, FIFOs and sockets
        // have no place on a share.
        reply.error(libc::EPERM);
    }

    fn mkdir(
        &mut self,
        _req: &Request<'_>,
        parent: u64,
        name: &OsStr,
        _mode: u32,
        _umask: u32,
        reply: ReplyEntry,
    ) {
        let made = (|| {
            self.writable()?;
            let path = self.child(parent, name)?;
            self.share.make_directory(&path).map_err(|e| share::errno(&e))?;
            self.entry(&path)
        })();
        match made {
            Ok(attr) => reply.entry(&TTL, &attr, 0),
            Err(err) => reply.error(err),
        }
    }

    fn unlink(&mut self, _req: &Request<'_>, parent: u64, name: &OsStr, reply: ReplyEmpty) {
        let done = (|| {
            self.writable()?;
            let path = self.child(parent, name)?;
            self.share.remove_file(&path).map_err(|e| share::errno(&e))?;
            self.nodes.removed(&path);
            Ok(())
        })();
        match done {
            Ok(()) => reply.ok(),
            Err(err) => reply.error(err),
        }
    }

    fn rmdir(&mut self, _req: &Request<'_>, parent: u64, name: &OsStr, reply: ReplyEmpty) {
        let done = (|| {
            self.writable()?;
            let path = self.child(parent, name)?;
            self.share.remove_directory(&path).map_err(|e| share::errno(&e))?;
            self.nodes.removed(&path);
            Ok(())
        })();
        match done {
            Ok(()) => reply.ok(),
            Err(err) => reply.error(err),
        }
    }

    fn symlink(
        &mut self,
        _req: &Request<'_>,
        _parent: u64,
        _link_name: &OsStr,
        _target: &std::path::Path,
        reply: ReplyEntry,
    ) {
        reply.error(libc::EPERM);
    }

    fn link(
        &mut self,
        _req: &Request<'_>,
        _ino: u64,
        _newparent: u64,
        _newname: &OsStr,
        reply: ReplyEntry,
    ) {
        reply.error(libc::EPERM);
    }

    fn rename(
        &mut self,
        _req: &Request<'_>,
        parent: u64,
        name: &OsStr,
        newparent: u64,
        newname: &OsStr,
        flags: u32,
        reply: ReplyEmpty,
    ) {
        let done = (|| {
            self.writable()?;
            if flags != 0 {
                return Err(libc::EINVAL);
            }
            let from = self.child(parent, name)?;
            let to = self.child(newparent, newname)?;
            if from == to {
                return Ok(());
            }
            self.share.rename(&from, &to).map_err(|e| share::errno(&e))?;
            self.nodes.moved(&from, &to);
            Ok(())
        })();
        match done {
            Ok(()) => reply.ok(),
            Err(err) => reply.error(err),
        }
    }

    fn open(&mut self, _req: &Request<'_>, ino: u64, flags: i32, reply: ReplyOpen) {
        let opened = (|| {
            let write = writes(flags);
            if write {
                self.writable()?;
            }
            let path = self.path(ino)?;
            let disposition = if flags & libc::O_TRUNC != 0 && write {
                CreateDisposition::FileOverwrite
            } else {
                CreateDisposition::FileOpen
            };
            let (file, _, _) =
                self.share.open(&path, write, disposition, false).map_err(|e| share::errno(&e))?;
            let fh = self.handle();
            self.files.insert(fh, file);
            Ok(fh)
        })();
        match opened {
            Ok(fh) => reply.opened(fh, 0),
            Err(err) => reply.error(err),
        }
    }

    fn create(
        &mut self,
        _req: &Request<'_>,
        parent: u64,
        name: &OsStr,
        mode: u32,
        umask: u32,
        flags: i32,
        reply: ReplyCreate,
    ) {
        let made = (|| {
            self.writable()?;
            let path = self.child(parent, name)?;
            let disposition = if flags & libc::O_EXCL != 0 {
                CreateDisposition::FileCreate
            } else if flags & libc::O_TRUNC != 0 {
                CreateDisposition::FileOverwriteIf
            } else {
                CreateDisposition::FileOpenIf
            };
            // Made without write bits, as `install -m 444` makes one: the
            // server marks it read-only.
            let read_only = mode & !umask & 0o222 == 0;
            let (file, _, _) = self
                .share
                .open(&path, true, disposition, read_only)
                .map_err(|e| share::errno(&e))?;
            let attr = match self.entry(&path) {
                Ok(attr) => attr,
                Err(err) => {
                    let _ = self.share.close(file);
                    return Err(err);
                }
            };
            let fh = self.handle();
            self.files.insert(fh, file);
            Ok((attr, fh))
        })();
        match made {
            Ok((attr, fh)) => reply.created(&TTL, &attr, 0, fh, 0),
            Err(err) => reply.error(err),
        }
    }

    fn read(
        &mut self,
        _req: &Request<'_>,
        _ino: u64,
        fh: u64,
        offset: i64,
        size: u32,
        _flags: i32,
        _lock_owner: Option<u64>,
        reply: ReplyData,
    ) {
        let read = (|| {
            let file = self.file(fh)?;
            let offset = u64::try_from(offset).map_err(|_| libc::EINVAL)?;
            let mut data = Vec::new();
            // The kernel asks for one credit's worth at most; a bigger
            // request goes in pieces.
            while data.len() < size as usize {
                let want = (size as usize - data.len()).min(MAX_IO as usize) as u32;
                let got = self
                    .share
                    .read(file, offset + data.len() as u64, want)
                    .map_err(|e| share::errno(&e))?;
                let short = got.len() < want as usize;
                data.extend_from_slice(&got);
                if short {
                    break;
                }
            }
            Ok(data)
        })();
        match read {
            Ok(data) => reply.data(&data),
            Err(err) => reply.error(err),
        }
    }

    fn write(
        &mut self,
        _req: &Request<'_>,
        _ino: u64,
        fh: u64,
        offset: i64,
        data: &[u8],
        _write_flags: u32,
        _flags: i32,
        _lock_owner: Option<u64>,
        reply: ReplyWrite,
    ) {
        let written = (|| {
            let file = self.file(fh)?;
            let offset = u64::try_from(offset).map_err(|_| libc::EINVAL)?;
            let mut done = 0;
            while done < data.len() {
                let n = self
                    .share
                    .write(file, offset + done as u64, &data[done..])
                    .map_err(|e| share::errno(&e))?;
                if n == 0 {
                    break;
                }
                done += n as usize;
            }
            u32::try_from(done).map_err(|_| libc::EIO)
        })();
        match written {
            Ok(n) => reply.written(n),
            Err(err) => reply.error(err),
        }
    }

    fn flush(&mut self, _req: &Request<'_>, _ino: u64, _fh: u64, _owner: u64, reply: ReplyEmpty) {
        reply.ok();
    }

    fn fsync(&mut self, _req: &Request<'_>, _ino: u64, fh: u64, _data: bool, reply: ReplyEmpty) {
        match self.file(fh).and_then(|file| self.share.flush(file).map_err(|e| share::errno(&e))) {
            Ok(()) => reply.ok(),
            Err(err) => reply.error(err),
        }
    }

    fn release(
        &mut self,
        _req: &Request<'_>,
        _ino: u64,
        fh: u64,
        _flags: i32,
        _lock_owner: Option<u64>,
        _flush: bool,
        reply: ReplyEmpty,
    ) {
        match self.files.remove(&fh) {
            Some(file) => match self.share.close(file) {
                Ok(()) => reply.ok(),
                Err(err) => reply.error(share::errno(&err)),
            },
            None => reply.error(libc::EBADF),
        }
    }

    fn opendir(&mut self, _req: &Request<'_>, ino: u64, _flags: i32, reply: ReplyOpen) {
        let listed = (|| {
            let path = self.path(ino)?;
            let entries = self.share.list(&path).map_err(|e| share::errno(&e))?;
            let entries = entries
                .into_iter()
                .filter(|e| e.name != "." && e.name != "..")
                .map(|e| {
                    let kind =
                        if e.is_directory { FileType::Directory } else { FileType::RegularFile };
                    (e.name, kind)
                })
                .collect();
            let fh = self.handle();
            self.listings.insert(fh, Listing { entries });
            Ok(fh)
        })();
        match listed {
            Ok(fh) => reply.opened(fh, 0),
            Err(err) => reply.error(err),
        }
    }

    fn readdir(
        &mut self,
        _req: &Request<'_>,
        ino: u64,
        fh: u64,
        offset: i64,
        mut reply: ReplyDirectory,
    ) {
        let Ok(path) = self.path(ino) else { return reply.error(libc::ENOENT) };
        let Some(listing) = self.listings.get(&fh) else { return reply.error(libc::EBADF) };
        // `.` is 1, `..` 2, and each entry its place after them.
        let dots =
            [(".".to_string(), FileType::Directory), ("..".to_string(), FileType::Directory)];
        let all = dots.iter().chain(&listing.entries).enumerate();
        for (i, (name, kind)) in all.skip(usize::try_from(offset).unwrap_or(0)) {
            // A number for the entry: its node's, if the kernel holds one.
            let entry_ino = match name.as_str() {
                "." => ino,
                ".." => ROOT,
                _ => self.nodes.known(&join(&path, name)).unwrap_or(u64::MAX - i as u64),
            };
            if reply.add(entry_ino, i as i64 + 1, *kind, name) {
                break;
            }
        }
        reply.ok();
    }

    fn releasedir(
        &mut self,
        _req: &Request<'_>,
        _ino: u64,
        fh: u64,
        _flags: i32,
        reply: ReplyEmpty,
    ) {
        self.listings.remove(&fh);
        reply.ok();
    }

    fn statfs(&mut self, _req: &Request<'_>, _ino: u64, reply: ReplyStatfs) {
        match self.share.space() {
            Ok((total, free, block)) => {
                let block64 = u64::from(block);
                reply.statfs(
                    total / block64,
                    free / block64,
                    free / block64,
                    0,
                    0,
                    block,
                    255,
                    block,
                );
            }
            Err(err) => reply.error(share::errno(&err)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nodes_are_numbered_by_path_until_forgotten() {
        let mut nodes = Nodes::new("");
        assert_eq!(nodes.path(ROOT).unwrap(), "");
        let a = nodes.give("a");
        assert_eq!(nodes.give("a"), a);
        let b = nodes.give("a/b");
        assert_ne!(a, b);
        assert_eq!(nodes.path(b).unwrap(), "a/b");
        nodes.forget(a, 1);
        assert_eq!(nodes.known("a"), Some(a));
        nodes.forget(a, 1);
        assert_eq!((nodes.known("a"), nodes.path(a)), (None, Err(libc::ENOENT)));
        assert_ne!(nodes.give("a"), a, "a forgotten number is not given again");
        nodes.forget(ROOT, 5);
        assert_eq!(nodes.path(ROOT).unwrap(), "");
    }

    #[test]
    fn a_rename_moves_the_paths_beneath() {
        let mut nodes = Nodes::new("top");
        let dir = nodes.give("top/d");
        let file = nodes.give("top/d/f");
        let deep = nodes.give("top/d/e/g");
        let near = nodes.give("top/dd");
        let replaced = nodes.give("top/x");
        nodes.moved("top/d", "top/x");
        assert_eq!(nodes.path(dir).unwrap(), "top/x");
        assert_eq!(nodes.path(file).unwrap(), "top/x/f");
        assert_eq!(nodes.path(deep).unwrap(), "top/x/e/g");
        assert_eq!(nodes.path(near).unwrap(), "top/dd", "a name that only starts the same stays");
        assert_eq!(nodes.path(replaced), Err(libc::ENOENT));
        assert_eq!(nodes.known("top/x/f"), Some(file));
        assert_eq!(nodes.known("top/d/f"), None);
        // The replaced node's last FORGET leaves the new one's path alone.
        nodes.forget(replaced, 1);
        assert_eq!(nodes.known("top/x"), Some(dir));
    }

    #[test]
    fn a_removed_path_names_nothing() {
        let mut nodes = Nodes::new("");
        let f = nodes.give("f");
        nodes.removed("f");
        assert_eq!((nodes.path(f), nodes.known("f")), (Err(libc::ENOENT), None));
        let again = nodes.give("f");
        assert_ne!(again, f);
        nodes.forget(f, 1);
        assert_eq!(nodes.known("f"), Some(again));
    }

    #[test]
    fn names_join() {
        assert_eq!(join("", "a"), "a");
        assert_eq!(join("a/b", "c"), "a/b/c");
        assert!(writes(libc::O_WRONLY) && writes(libc::O_RDWR) && !writes(libc::O_RDONLY));
    }
}
