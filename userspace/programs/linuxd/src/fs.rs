// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! linuxd's filesystem (ADR 0621): `proc` and `sys`, which the sandbox
//! attaches at `/proc` and `/sys`, in Linux's text, and `.netlink`, the
//! files that stand for netlink sockets.
//!
//! [`LinuxFs`] is the `fuser::Filesystem`; it answers the kernel's
//! requests from [`tree::Tree`], which makes each file's text from what
//! the kernel keeps under `/processes`, `/kernel` and `/system`:
//!
//! - [`node`]: every node, and the number the kernel knows it by;
//! - [`tree`]: what is in each directory and what each file holds;
//! - [`task`], [`system`], [`cpu`], [`mounts`]: the texts, made from
//!   the kernel's, as Linux's proc(5) shows them;
//! - [`records`], [`text`]: the kernel's files, read;
//! - [`handles`]: what each open handle reads;
//! - [`kernel`]: the machine, which the host's tests replace.
//!
//! The kernel's FUSE keeps one handle for each node it has open, so the
//! snapshot a program reads is made when the handle is opened and again
//! at each read of offset 0 after the first, which is what a new
//! `open(2)` of the file reads on Linux.
//!
//! When a file shows something Linux's does not, find its node in
//! [`node`], and follow it to the generator that makes its text.

mod cpu;
mod handles;
mod kernel;
mod mounts;
mod node;
mod records;
mod system;
mod task;
mod text;
mod tree;

use std::ffi::OsStr;
use std::os::fd::OwnedFd;
use std::sync::Arc;
use std::time::SystemTime;

use fuser::consts::FOPEN_DIRECT_IO;
use fuser::{
    FileAttr, FileType, Filesystem, ReplyAttr, ReplyData, ReplyDirectory, ReplyEmpty, ReplyEntry,
    ReplyOpen, ReplyStatfs, ReplyWrite, Request, Session, SessionACL, TimeOrNow,
};

use self::handles::Handles;
use self::kernel::Machine;
use self::node::{Kind, Node};
use self::tree::{Attr, Tree};
use crate::netlink::Netlinks;

/// The filesystem.
pub struct LinuxFs {
    tree: Tree<Machine>,
    netlinks: Arc<Netlinks>,
    /// When the kernel started, which every node's times say.
    boot: SystemTime,
    handles: Handles,
}

impl LinuxFs {
    /// The filesystem, whose `.netlink` files are `netlinks`' sockets.
    pub fn new(netlinks: Arc<Netlinks>) -> LinuxFs {
        let owner = (sysapi::process::uid(), sysapi::process::gid());
        let mounts = std::env::var(mounts::VARIABLE).ok().filter(|path| !path.is_empty());
        let tree = Tree::new(Machine, Arc::clone(&netlinks), mounts, owner);
        let boot = tree.boot_time();
        // The time in seconds differs between one linuxd and the next.
        let seconds = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map_or(1, |elapsed| elapsed.as_secs());
        LinuxFs {
            tree,
            netlinks,
            boot,
            handles: Handles::new(u32::try_from(seconds % (1 << 32)).unwrap_or(1)),
        }
    }

    /// The attributes of a node as the protocol shows them.
    fn file_attr(&self, attr: &Attr) -> FileAttr {
        let kind = match attr.kind {
            Kind::Directory => FileType::Directory,
            Kind::File => FileType::RegularFile,
            Kind::Link => FileType::Symlink,
        };
        FileAttr {
            ino: attr.node.number(),
            size: attr.size,
            blocks: 0,
            atime: self.boot,
            mtime: self.boot,
            ctime: self.boot,
            crtime: self.boot,
            kind,
            perm: attr.mode,
            nlink: if attr.kind == Kind::Directory { 2 } else { 1 },
            uid: attr.user,
            gid: attr.group,
            rdev: 0,
            blksize: 4096,
            flags: 0,
        }
    }

    /// The node with number `ino`, or ENOENT.
    fn node(ino: u64) -> Result<Node, i32> {
        Node::of_number(ino).ok_or(libc::ENOENT)
    }
}

impl Filesystem for LinuxFs {
    fn lookup(&mut self, req: &Request<'_>, parent: u64, name: &OsStr, reply: ReplyEntry) {
        let found = Self::node(parent).and_then(|parent| {
            let name = name.to_str().ok_or(libc::ENOENT)?;
            self.tree.lookup(parent, name, u64::from(req.pid()))
        });
        match found {
            Ok(attr) => reply.entry(&attr.node.timeout(), &self.file_attr(&attr), 0),
            Err(errno) => reply.error(errno),
        }
    }

    fn getattr(&mut self, req: &Request<'_>, ino: u64, _fh: Option<u64>, reply: ReplyAttr) {
        match Self::node(ino).and_then(|node| self.tree.attr(node, u64::from(req.pid()))) {
            Ok(attr) => reply.attr(&attr.node.timeout(), &self.file_attr(&attr)),
            Err(errno) => reply.error(errno),
        }
    }

    #[allow(clippy::too_many_arguments, reason = "the signature fuser's trait gives it")]
    fn setattr(
        &mut self,
        _req: &Request<'_>,
        _ino: u64,
        _mode: Option<u32>,
        _uid: Option<u32>,
        _gid: Option<u32>,
        _size: Option<u64>,
        _atime: Option<TimeOrNow>,
        _mtime: Option<TimeOrNow>,
        _ctime: Option<SystemTime>,
        _fh: Option<u64>,
        _crtime: Option<SystemTime>,
        _chgtime: Option<SystemTime>,
        _bkuptime: Option<SystemTime>,
        _flags: Option<u32>,
        reply: ReplyAttr,
    ) {
        reply.error(libc::EPERM);
    }

    fn readlink(&mut self, req: &Request<'_>, ino: u64, reply: ReplyData) {
        match Self::node(ino).and_then(|node| self.tree.readlink(node, u64::from(req.pid()))) {
            Ok(target) => reply.data(&target),
            Err(errno) => reply.error(errno),
        }
    }

    fn open(&mut self, _req: &Request<'_>, ino: u64, _flags: i32, reply: ReplyOpen) {
        let node = match Self::node(ino) {
            Ok(node) => node,
            Err(errno) => return reply.error(errno),
        };
        let snapshot = match node {
            Node::Socket(id) if self.netlinks.exists(id) => None,
            Node::Socket(_) => return reply.error(libc::ENOENT),
            node => match self.tree.open(node) {
                Ok(bytes) => Some(bytes),
                Err(errno) => return reply.error(errno),
            },
        };
        let fh = self.handles.open(node, snapshot);
        reply.opened(fh, FOPEN_DIRECT_IO);
    }

    #[allow(clippy::too_many_arguments, reason = "the signature fuser's trait gives it")]
    fn read(
        &mut self,
        _req: &Request<'_>,
        ino: u64,
        fh: u64,
        offset: i64,
        size: u32,
        _flags: i32,
        _lock_owner: Option<u64>,
        reply: ReplyData,
    ) {
        let Ok(offset) = u64::try_from(offset) else { return reply.error(libc::EINVAL) };
        if let Some(id) = crate::netlink::id_of(ino) {
            return match self.netlinks.read(id, size as usize) {
                Ok(bytes) => reply.data(&bytes),
                Err(errno) => reply.error(errno),
            };
        }
        let node = match Self::node(ino) {
            Ok(node) => node,
            Err(errno) => return reply.error(errno),
        };
        let tree = &self.tree;
        match self.handles.snapshot(fh, node, offset, || tree.open(node)) {
            Ok(bytes) => {
                let start =
                    usize::try_from(offset).map_or(bytes.len(), |start| start.min(bytes.len()));
                let end = start.saturating_add(size as usize).min(bytes.len());
                reply.data(&bytes[start..end]);
            }
            Err(errno) => reply.error(errno),
        }
    }

    #[allow(clippy::too_many_arguments, reason = "the signature fuser's trait gives it")]
    fn write(
        &mut self,
        _req: &Request<'_>,
        ino: u64,
        _fh: u64,
        _offset: i64,
        data: &[u8],
        _write_flags: u32,
        _flags: i32,
        _lock_owner: Option<u64>,
        reply: ReplyWrite,
    ) {
        match crate::netlink::id_of(ino) {
            Some(id) => match self.netlinks.write(id, data) {
                Ok(written) => reply.written(u32::try_from(written).unwrap_or(u32::MAX)),
                Err(errno) => reply.error(errno),
            },
            None => reply.error(libc::EACCES),
        }
    }

    fn flush(
        &mut self,
        _req: &Request<'_>,
        _ino: u64,
        _fh: u64,
        _lock_owner: u64,
        reply: ReplyEmpty,
    ) {
        reply.ok();
    }

    #[allow(clippy::too_many_arguments, reason = "the signature fuser's trait gives it")]
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
        // A handle an earlier linuxd gave is not in the table, and its
        // socket is gone with that linuxd.
        if let Some(id) = self.handles.release(fh) {
            self.netlinks.release(id);
        }
        reply.ok();
    }

    fn opendir(&mut self, _req: &Request<'_>, ino: u64, _flags: i32, reply: ReplyOpen) {
        match Self::node(ino) {
            Ok(node) if node.kind() == Kind::Directory => reply.opened(0, 0),
            Ok(_) => reply.error(libc::ENOTDIR),
            Err(errno) => reply.error(errno),
        }
    }

    fn readdir(
        &mut self,
        _req: &Request<'_>,
        ino: u64,
        _fh: u64,
        offset: i64,
        mut reply: ReplyDirectory,
    ) {
        let listed = Self::node(ino).and_then(|dir| {
            let mut entries = vec![(1, dir, ".".to_string()), (2, dir.parent(), "..".to_string())];
            let children = self.tree.children(dir)?;
            entries
                .extend(children.into_iter().map(|child| (child.cookie, child.node, child.name)));
            Ok(entries)
        });
        let entries = match listed {
            Ok(entries) => entries,
            Err(errno) => return reply.error(errno),
        };
        for (cookie, node, name) in entries.into_iter().filter(|entry| entry.0 > offset) {
            let kind = match node.kind() {
                Kind::Directory => FileType::Directory,
                Kind::File => FileType::RegularFile,
                Kind::Link => FileType::Symlink,
            };
            if reply.add(node.number(), cookie, kind, name) {
                break;
            }
        }
        reply.ok();
    }

    fn releasedir(
        &mut self,
        _req: &Request<'_>,
        _ino: u64,
        _fh: u64,
        _flags: i32,
        reply: ReplyEmpty,
    ) {
        reply.ok();
    }

    fn statfs(&mut self, _req: &Request<'_>, _ino: u64, reply: ReplyStatfs) {
        reply.statfs(0, 0, 0, 0, 0, 4096, 255, 4096);
    }
}

/// Serves `fs` on `connection`, a description of `/devices/fuse`, until
/// the kernel lets go of it; `again` when linuxd was started again, so
/// that the session takes up the connection without waiting for `INIT`.
pub fn serve(
    fs: LinuxFs,
    connection: OwnedFd,
    #[cfg_attr(
        not(target_os = "radix"),
        allow(unused_variables, reason = "only Radix's fuser can take up a connection")
    )]
    again: bool,
) -> std::io::Result<()> {
    #[cfg(target_os = "radix")]
    let mut session = if again {
        Session::from_fd_initialized(fs, connection, SessionACL::All)
    } else {
        Session::from_fd(fs, connection, SessionACL::All)
    };
    // The host's fuser, crates.io's, cannot start a session as
    // initialized; the host builds only to test the parts.
    #[cfg(not(target_os = "radix"))]
    let mut session = Session::from_fd(fs, connection, SessionACL::All);
    session.run()
}
