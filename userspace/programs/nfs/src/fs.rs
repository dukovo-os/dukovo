// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The server's folder as a filesystem, served with fuser through
//! /devices/fuse (ADR 0226).
//!
//! A node stands for a filehandle, which a rename leaves as it is; its
//! number is the server's file number, as Linux's client shows it, unless
//! another node has it already (a file of another of the server's
//! filesystems). Owners and modes are the server's: the kernel checks
//! them, as Linux's `default_permissions`, and each call carries its
//! caller's numbers and groups, for the server to check them again.
//!
//! Each file the kernel opens is opened on the server, by an open-owner
//! of its own, and closed when the kernel releases it; an open the server
//! lost, after it restarted or the lease ran out, is opened again.
//! Writes are UNSTABLE, as Linux's client sends them: the data is kept
//! until a COMMIT, on fsync and release or past 16 MiB, finds it written,
//! and is written again, FILE_SYNC, if the server restarted before it
//! was. A folder is listed once, when it is opened.

use std::collections::HashMap;
use std::ffi::{OsStr, OsString};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use fuser::{
    FileAttr, FileType, Filesystem, KernelConfig, ReplyAttr, ReplyCreate, ReplyData,
    ReplyDirectory, ReplyEmpty, ReplyEntry, ReplyOpen, ReplyStatfs, ReplyWrite, Request, TimeOrNow,
};
use libc::c_int;
use nfsd::nfs::{Attrs, kind, op};
use nfsd::xdr::{self, Reader, Writer};

use crate::attrs::{self, Attr, Set, When};
use crate::client::{Client, Fail, Ops, Results, lost_state};
use crate::ids::Ids;
use crate::rpc::Cred;

/// How long the kernel keeps what it is told: the server's files change
/// under other clients too.
const TTL: Duration = Duration::from_secs(1);

/// The root's node number, as the protocol fixes it.
pub const ROOT: u64 = 1;

/// Where node numbers of files whose own are taken start.
const OTHERS: u64 = 1 << 63;

/// OPEN's share_access: to read, to write; and the want of no
/// delegation, which this client cannot be called back about.
const READ: u32 = 1;
const WRITE: u32 = 2;
const NO_DELEGATION: u32 = 0x0400;

/// OPEN's createmode4s.
const UNCHECKED: u32 = 0;
const GUARDED: u32 = 1;

/// stable_how4: UNSTABLE, and FILE_SYNC.
const UNSTABLE: u32 = 0;
const FILE_SYNC: u32 = 2;

/// How much unstable data a file keeps before it is committed.
const KEEP_MAX: usize = 16 << 20;

/// The most names a folder's listing holds.
const LISTED_MAX: usize = 1 << 20;

/// The size of a block as statfs gives it.
const BLOCK: u64 = 4096;

/// A file the kernel holds: its filehandle, and how many times the
/// kernel was given it.
struct Node {
    fh: Vec<u8>,
    lookups: u64,
}

/// The nodes the kernel holds, by number and by filehandle.
pub struct Nodes {
    nodes: HashMap<u64, Node>,
    by_fh: HashMap<Vec<u8>, u64>,
    next: u64,
}

impl Nodes {
    /// The root's alone, the folder of `fh`.
    pub fn new(fh: &[u8]) -> Nodes {
        let mut nodes = HashMap::new();
        nodes.insert(ROOT, Node { fh: fh.to_vec(), lookups: 1 });
        let mut by_fh = HashMap::new();
        by_fh.insert(fh.to_vec(), ROOT);
        Nodes { nodes, by_fh, next: OTHERS }
    }

    /// The filehandle of node `ino`.
    fn fh(&self, ino: u64) -> Result<Vec<u8>, c_int> {
        self.nodes.get(&ino).map(|node| node.fh.clone()).ok_or(libc::ESTALE)
    }

    /// The node number of `fh`, given to the kernel once more: the
    /// server's number for it, `fileid`, if no other node has it.
    fn give(&mut self, fh: &[u8], fileid: u64) -> u64 {
        if let Some(&ino) = self.by_fh.get(fh) {
            if let Some(node) = self.nodes.get_mut(&ino) {
                node.lookups += 1;
            }
            return ino;
        }
        let mut ino = fileid;
        if ino <= ROOT || ino >= OTHERS || self.nodes.contains_key(&ino) {
            while self.nodes.contains_key(&self.next) {
                self.next = self.next.wrapping_add(1).max(OTHERS);
            }
            ino = self.next;
            self.next = self.next.wrapping_add(1).max(OTHERS);
        }
        self.nodes.insert(ino, Node { fh: fh.to_vec(), lookups: 1 });
        self.by_fh.insert(fh.to_vec(), ino);
        ino
    }

    /// The kernel lets go of `count` of node `ino`'s lookups; the node
    /// goes with its last.
    fn forget(&mut self, ino: u64, count: u64) {
        let Some(node) = self.nodes.get_mut(&ino) else { return };
        node.lookups = node.lookups.saturating_sub(count);
        if node.lookups == 0
            && ino != ROOT
            && let Some(node) = self.nodes.remove(&ino)
        {
            self.by_fh.remove(&node.fh);
        }
    }
}

/// Unstable data a file was given and the server has not committed.
#[derive(Default)]
struct Unstable {
    /// The server's write verifier when it took the data.
    verifier: Option<[u8; 8]>,
    kept: Vec<(u64, Vec<u8>)>,
    bytes: usize,
}

/// A file opened on the server.
struct Open {
    fh: Vec<u8>,
    /// OPEN's share_access.
    access: u32,
    /// Who opened it, who its reads and writes are sent as.
    cred: Cred,
    owner: Vec<u8>,
    stateid: [u8; 16],
    /// The client ID's generation it was opened in.
    generation: u64,
    unstable: Unstable,
}

/// A name of a listed folder.
struct Listed {
    name: OsString,
    kind: FileType,
    fileid: u64,
}

/// What CREATE makes.
#[derive(Clone, Copy)]
pub enum Made<'a> {
    Folder,
    Link(&'a [u8]),
    Fifo,
    Socket,
}

/// Room and names, for statfs.
#[derive(Debug, PartialEq, Eq)]
pub struct Space {
    pub blocks: u64,
    pub free: u64,
    pub avail: u64,
    pub files: u64,
    pub files_free: u64,
    pub max_name: u32,
}

/// The filesystem.
pub struct NfsFs {
    client: Arc<Mutex<Client>>,
    ids: Ids,
    nodes: Nodes,
    files: HashMap<u64, Open>,
    listings: HashMap<u64, Vec<Listed>>,
    next_handle: u64,
    read_only: bool,
}

/// The node kind of an nfs_ftype4.
fn file_type(kind: u32) -> FileType {
    match kind {
        kind::DIR => FileType::Directory,
        kind::LNK => FileType::Symlink,
        kind::BLK => FileType::BlockDevice,
        kind::CHR => FileType::CharDevice,
        kind::SOCK => FileType::Socket,
        kind::FIFO => FileType::NamedPipe,
        _ => FileType::RegularFile,
    }
}

/// OPEN's share_access for open(2)'s `flags`.
fn access_of(flags: i32) -> u32 {
    match flags & libc::O_ACCMODE {
        libc::O_WRONLY => WRITE,
        libc::O_RDWR => READ | WRITE,
        _ => READ,
    }
}

/// OPEN's arguments, after its number: `access` asked for by `owner`;
/// made if `how` says so; the current filehandle's file, or its folder's
/// `name`.
fn open_args(
    w: &mut Writer,
    access: u32,
    client: u64,
    owner: &[u8],
    how: Option<(u32, &Set)>,
    name: Option<&[u8]>,
) {
    w.u32(0).u32(access | NO_DELEGATION).u32(0).u64(client).opaque(owner);
    match how {
        None => {
            w.u32(0);
        }
        Some((mode, set)) => {
            w.u32(1).u32(mode);
            set.write(w);
        }
    }
    match name {
        Some(name) => {
            w.u32(0).opaque(name);
        }
        None => {
            w.u32(4);
        }
    }
}

/// OPEN's result: its stateid, and a delegation's if the server gave one
/// though none was wanted.
fn open_result(r: &mut Reader) -> xdr::Result<([u8; 16], Option<[u8; 16]>)> {
    let stateid = r.array()?;
    // change_info4, rflags, attrset.
    r.bool()?;
    r.u64()?;
    r.u64()?;
    r.u32()?;
    r.bitmap()?;
    let delegation = match r.u32()? {
        0 => None,
        kind @ (1 | 2) => {
            let stateid = r.array()?;
            r.bool()?;
            if kind == 2 {
                match r.u32()? {
                    1 => {
                        r.u64()?;
                    }
                    2 => {
                        r.u32()?;
                        r.u32()?;
                    }
                    _ => return Err(xdr::Bad),
                }
            }
            // nfsace4: type, flag, mask, who.
            r.u32()?;
            r.u32()?;
            r.u32()?;
            r.opaque(1024)?;
            Some(stateid)
        }
        3 => {
            // Why none: contention and resources say one thing more.
            if matches!(r.u32()?, 1 | 2) {
                r.bool()?;
            }
            None
        }
        _ => return Err(xdr::Bad),
    };
    Ok((stateid, delegation))
}

/// A GETATTR of `attrs`.
fn getattr(ops: &mut Ops, attrs: Attrs) {
    attrs::ask(ops.op(op::GETATTR), attrs);
}

/// Linux's dev_t of a device's numbers.
fn device(major: u32, minor: u32) -> u32 {
    ((major & 0xfff) << 8) | (minor & 0xff) | ((minor & !0xff) << 12)
}

impl NfsFs {
    /// The files of the folder `root`, on `client`'s server.
    pub fn new(client: Arc<Mutex<Client>>, root: &[u8], read_only: bool) -> NfsFs {
        NfsFs {
            client,
            ids: Ids::new(),
            nodes: Nodes::new(root),
            files: HashMap::new(),
            listings: HashMap::new(),
            next_handle: 1,
            read_only,
        }
    }

    fn client(&self) -> MutexGuard<'_, Client> {
        self.client.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn run(&self, cred: &Cred, ops: &Ops) -> Result<Results, c_int> {
        self.client().run(cred, ops).map_err(|fail| fail.errno())
    }

    fn handle(&mut self) -> u64 {
        self.next_handle += 1;
        self.next_handle - 1
    }

    fn writable(&self) -> Result<(), c_int> {
        if self.read_only { Err(libc::EROFS) } else { Ok(()) }
    }

    /// The credential of `uid` in `gid`, with their other groups.
    pub fn cred(&mut self, uid: u32, gid: u32) -> Cred {
        self.ids.cred(uid, gid)
    }

    /// Node `ino`'s attributes, as the kernel is given them.
    fn attr(&mut self, ino: u64, a: &Attr) -> FileAttr {
        let ctime = a.ctime.unwrap_or(UNIX_EPOCH);
        let blksize = self.client().max_write;
        FileAttr {
            ino,
            size: a.size,
            blocks: a.space_used.div_ceil(512),
            atime: a.atime.unwrap_or(UNIX_EPOCH),
            mtime: a.mtime.unwrap_or(UNIX_EPOCH),
            ctime,
            crtime: ctime,
            kind: file_type(a.kind),
            perm: (a.mode & 0o7777) as u16,
            nlink: a.nlink,
            uid: self.ids.uid(&a.owner),
            gid: self.ids.gid(&a.group),
            rdev: device(a.rdev.0, a.rdev.1),
            blksize,
            flags: 0,
        }
    }

    /// The file of `fh`, found with attributes `a`: its node, given to
    /// the kernel, and attributes.
    fn found(&mut self, fh: &[u8], a: &Attr) -> FileAttr {
        let ino = self.nodes.give(fh, a.fileid);
        self.attr(ino, a)
    }

    /// Looks up `name` in folder `parent`.
    pub fn look_up(&mut self, cred: &Cred, parent: u64, name: &OsStr) -> Result<FileAttr, c_int> {
        let dir = self.nodes.fh(parent)?;
        let mut ops = Ops::new();
        ops.putfh(&dir);
        if name == ".." {
            ops.op(op::LOOKUPP);
        } else {
            ops.op(op::LOOKUP).opaque(name.as_bytes());
        }
        ops.op(op::GETFH);
        getattr(&mut ops, attrs::FILE);
        let results = self.run(cred, &ops)?;
        let (fh, a) = (|| -> Result<_, Fail> {
            let mut walk = results.walk();
            walk.next(op::PUTFH)?;
            walk.next(if name == ".." { op::LOOKUPP } else { op::LOOKUP })?;
            let fh = walk.next(op::GETFH)?.opaque(128)?.to_vec();
            Ok((fh, attrs::decode(walk.next(op::GETATTR)?)?))
        })()
        .map_err(|fail| fail.errno())?;
        Ok(self.found(&fh, &a))
    }

    /// The kernel lets go of `count` of node `ino`'s lookups.
    pub fn let_go(&mut self, ino: u64, count: u64) {
        self.nodes.forget(ino, count);
    }

    /// Node `ino`'s attributes.
    pub fn attributes(&mut self, cred: &Cred, ino: u64) -> Result<FileAttr, c_int> {
        let fh = self.nodes.fh(ino)?;
        let mut ops = Ops::new();
        ops.putfh(&fh);
        getattr(&mut ops, attrs::FILE);
        let results = self.run(cred, &ops)?;
        let a = (|| -> Result<_, Fail> {
            let mut walk = results.walk();
            walk.next(op::PUTFH)?;
            Ok(attrs::decode(walk.next(op::GETATTR)?)?)
        })()
        .map_err(|fail| fail.errno())?;
        Ok(self.attr(ino, &a))
    }

    /// Sets `set` on node `ino`, through open file `handle` if the kernel
    /// names one: its attributes after.
    pub fn set(
        &mut self,
        cred: &Cred,
        ino: u64,
        set: &Set,
        handle: Option<u64>,
    ) -> Result<FileAttr, c_int> {
        self.writable()?;
        let fh = self.nodes.fh(ino)?;
        // A size is set through the file's open, if it is open to write;
        // else with the anonymous stateid, all zeros.
        let stateid = handle
            .and_then(|handle| self.files.get(&handle))
            .filter(|open| open.access & WRITE != 0 && set.size.is_some())
            .filter(|open| open.generation == self.client().generation)
            .map_or([0; 16], |open| open.stateid);
        let mut ops = Ops::kept();
        ops.putfh(&fh);
        set.write(ops.op(op::SETATTR).fixed(&stateid));
        getattr(&mut ops, attrs::FILE);
        let results = self.run(cred, &ops)?;
        let a = (|| -> Result<_, Fail> {
            let mut walk = results.walk();
            walk.next(op::PUTFH)?;
            walk.next(op::SETATTR)?.bitmap()?;
            Ok(attrs::decode(walk.next(op::GETATTR)?)?)
        })()
        .map_err(|fail| fail.errno())?;
        Ok(self.attr(ino, &a))
    }

    /// Makes `name` in folder `parent` with CREATE: a folder, a symbolic
    /// link, a FIFO or a socket, with `mode`.
    pub fn make(
        &mut self,
        cred: &Cred,
        parent: u64,
        name: &OsStr,
        made: Made<'_>,
        mode: u32,
    ) -> Result<FileAttr, c_int> {
        self.writable()?;
        let dir = self.nodes.fh(parent)?;
        let mut ops = Ops::kept();
        ops.putfh(&dir);
        let w = ops.op(op::CREATE);
        let set = match made {
            Made::Folder => {
                w.u32(kind::DIR);
                Set::mode(mode)
            }
            Made::Link(target) => {
                w.u32(kind::LNK).opaque(target);
                Set::default()
            }
            Made::Fifo => {
                w.u32(kind::FIFO);
                Set::mode(mode)
            }
            Made::Socket => {
                w.u32(kind::SOCK);
                Set::mode(mode)
            }
        };
        w.opaque(name.as_bytes());
        set.write(w);
        ops.op(op::GETFH);
        getattr(&mut ops, attrs::FILE);
        let results = self.run(cred, &ops)?;
        let (fh, a) = (|| -> Result<_, Fail> {
            let mut walk = results.walk();
            walk.next(op::PUTFH)?;
            let r = walk.next(op::CREATE)?;
            r.bool()?;
            r.u64()?;
            r.u64()?;
            r.bitmap()?;
            let fh = walk.next(op::GETFH)?.opaque(128)?.to_vec();
            Ok((fh, attrs::decode(walk.next(op::GETATTR)?)?))
        })()
        .map_err(|fail| fail.errno())?;
        Ok(self.found(&fh, &a))
    }

    /// Removes `name` from folder `parent`: a file or an empty folder.
    pub fn remove(&mut self, cred: &Cred, parent: u64, name: &OsStr) -> Result<(), c_int> {
        self.writable()?;
        let dir = self.nodes.fh(parent)?;
        let mut ops = Ops::kept();
        ops.putfh(&dir).op(op::REMOVE).opaque(name.as_bytes());
        let results = self.run(cred, &ops)?;
        let mut walk = results.walk();
        walk.next(op::PUTFH).map_err(|fail| fail.errno())?;
        walk.next(op::REMOVE).map(drop).map_err(|fail| fail.errno())
    }

    /// Renames `name` in `parent` to `new_name` in `new_parent`.
    pub fn rename(
        &mut self,
        cred: &Cred,
        parent: u64,
        name: &OsStr,
        new_parent: u64,
        new_name: &OsStr,
    ) -> Result<(), c_int> {
        self.writable()?;
        let (from, to) = (self.nodes.fh(parent)?, self.nodes.fh(new_parent)?);
        let mut ops = Ops::kept();
        ops.putfh(&from).op(op::SAVEFH);
        ops.putfh(&to).op(op::RENAME).opaque(name.as_bytes()).opaque(new_name.as_bytes());
        let results = self.run(cred, &ops)?;
        let mut walk = results.walk();
        let done = (|| -> Result<(), Fail> {
            walk.next(op::PUTFH)?;
            walk.next(op::SAVEFH)?;
            walk.next(op::PUTFH)?;
            walk.next(op::RENAME)?;
            Ok(())
        })();
        done.map_err(|fail| fail.errno())
    }

    /// Links node `ino` into folder `new_parent` as `new_name`.
    pub fn link(
        &mut self,
        cred: &Cred,
        ino: u64,
        new_parent: u64,
        new_name: &OsStr,
    ) -> Result<FileAttr, c_int> {
        self.writable()?;
        let (fh, dir) = (self.nodes.fh(ino)?, self.nodes.fh(new_parent)?);
        let mut ops = Ops::kept();
        ops.putfh(&fh).op(op::SAVEFH);
        ops.putfh(&dir).op(op::LINK).opaque(new_name.as_bytes());
        ops.op(op::RESTOREFH);
        getattr(&mut ops, attrs::FILE);
        let results = self.run(cred, &ops)?;
        let a = (|| -> Result<_, Fail> {
            let mut walk = results.walk();
            walk.next(op::PUTFH)?;
            walk.next(op::SAVEFH)?;
            walk.next(op::PUTFH)?;
            // change_info4.
            let r = walk.next(op::LINK)?;
            r.bool()?;
            r.u64()?;
            r.u64()?;
            walk.next(op::RESTOREFH)?;
            Ok(attrs::decode(walk.next(op::GETATTR)?)?)
        })()
        .map_err(|fail| fail.errno())?;
        Ok(self.found(&fh, &a))
    }

    /// What symbolic link `ino` holds.
    pub fn read_link(&mut self, cred: &Cred, ino: u64) -> Result<Vec<u8>, c_int> {
        let fh = self.nodes.fh(ino)?;
        let mut ops = Ops::new();
        ops.putfh(&fh).op(op::READLINK);
        let results = self.run(cred, &ops)?;
        let mut walk = results.walk();
        let target = (|| -> Result<_, Fail> {
            walk.next(op::PUTFH)?;
            Ok(walk.next(op::READLINK)?.opaque(4096)?.to_vec())
        })();
        target.map_err(|fail| fail.errno())
    }

    /// The open-owner of open file `handle`, unique to it.
    fn owner(handle: u64) -> Vec<u8> {
        let mut owner = b"dukovo-nfs-".to_vec();
        owner.extend_from_slice(&handle.to_be_bytes());
        owner
    }

    /// Opens node `ino` as `cred`, as open(2)'s `flags` ask: its handle.
    pub fn open(&mut self, cred: &Cred, ino: u64, flags: i32) -> Result<u64, c_int> {
        let access = access_of(flags);
        if access & WRITE != 0 {
            self.writable()?;
        }
        let fh = self.nodes.fh(ino)?;
        let handle = self.handle();
        let owner = Self::owner(handle);
        let mut ops = Ops::kept();
        ops.putfh(&fh);
        let (client_id, generation) = {
            let client = self.client();
            (client.client_id(), client.generation)
        };
        open_args(ops.op(op::OPEN), access, client_id, &owner, None, None);
        let results = self.run(cred, &ops)?;
        let (stateid, delegation) = (|| -> Result<_, Fail> {
            let mut walk = results.walk();
            walk.next(op::PUTFH)?;
            Ok(open_result(walk.next(op::OPEN)?)?)
        })()
        .map_err(|fail| fail.errno())?;
        self.give_back(cred, &fh, delegation);
        let generation = generation.max(self.client().generation);
        let open = Open {
            fh,
            access,
            cred: cred.clone(),
            owner,
            stateid,
            generation,
            unstable: Unstable::default(),
        };
        self.files.insert(handle, open);
        Ok(handle)
    }

    /// Makes and opens `name` in folder `parent`, with `mode`, as open(2)'s
    /// `flags` ask: its attributes and handle.
    pub fn create(
        &mut self,
        cred: &Cred,
        parent: u64,
        name: &OsStr,
        mode: u32,
        flags: i32,
    ) -> Result<(FileAttr, u64), c_int> {
        self.writable()?;
        let dir = self.nodes.fh(parent)?;
        let access = access_of(flags) | WRITE;
        let handle = self.handle();
        let owner = Self::owner(handle);
        let exclusive = flags & libc::O_EXCL != 0;
        let mut set = Set::mode(mode);
        if flags & libc::O_TRUNC != 0 && !exclusive {
            set.size = Some(0);
        }
        let how = if exclusive { GUARDED } else { UNCHECKED };
        let client_id = self.client().client_id();
        let mut ops = Ops::kept();
        ops.putfh(&dir);
        open_args(
            ops.op(op::OPEN),
            access,
            client_id,
            &owner,
            Some((how, &set)),
            Some(name.as_bytes()),
        );
        ops.op(op::GETFH);
        getattr(&mut ops, attrs::FILE);
        let results = self.run(cred, &ops)?;
        let (stateid, delegation, fh, a) = (|| -> Result<_, Fail> {
            let mut walk = results.walk();
            walk.next(op::PUTFH)?;
            let (stateid, delegation) = open_result(walk.next(op::OPEN)?)?;
            let fh = walk.next(op::GETFH)?.opaque(128)?.to_vec();
            Ok((stateid, delegation, fh, attrs::decode(walk.next(op::GETATTR)?)?))
        })()
        .map_err(|fail| fail.errno())?;
        self.give_back(cred, &fh, delegation);
        let generation = self.client().generation;
        let open = Open {
            fh: fh.clone(),
            access,
            cred: cred.clone(),
            owner,
            stateid,
            generation,
            unstable: Unstable::default(),
        };
        self.files.insert(handle, open);
        Ok((self.found(&fh, &a), handle))
    }

    /// Returns a delegation given though none was wanted.
    fn give_back(&self, cred: &Cred, fh: &[u8], delegation: Option<[u8; 16]>) {
        let Some(stateid) = delegation else { return };
        let mut ops = Ops::kept();
        ops.putfh(fh).op(op::DELEGRETURN).fixed(&stateid);
        let _ = self.run(cred, &ops);
    }

    /// Opens file `handle` again, after the server lost its open.
    fn reopen(&mut self, handle: u64) -> Result<(), c_int> {
        let open = self.files.get(&handle).ok_or(libc::EBADF)?;
        let client_id = self.client().client_id();
        let mut ops = Ops::kept();
        ops.putfh(&open.fh);
        open_args(ops.op(op::OPEN), open.access, client_id, &open.owner, None, None);
        let (cred, fh) = (open.cred.clone(), open.fh.clone());
        let results = self.run(&cred, &ops)?;
        let (stateid, delegation) = (|| -> Result<_, Fail> {
            let mut walk = results.walk();
            walk.next(op::PUTFH)?;
            Ok(open_result(walk.next(op::OPEN)?)?)
        })()
        .map_err(|fail| fail.errno())?;
        self.give_back(&cred, &fh, delegation);
        let generation = self.client().generation;
        let open = self.files.get_mut(&handle).ok_or(libc::EBADF)?;
        open.stateid = stateid;
        open.generation = generation;
        Ok(())
    }

    /// Runs `call` with open file `handle` and the client; once more after
    /// opening the file again if the server lost its open.
    fn with_open<T>(
        &mut self,
        handle: u64,
        mut call: impl FnMut(&mut Client, &Open) -> Result<T, Fail>,
    ) -> Result<T, c_int> {
        for again in [false, true] {
            let stale = {
                let open = self.files.get(&handle).ok_or(libc::EBADF)?;
                open.generation != self.client().generation
            };
            if stale {
                self.reopen(handle)?;
            }
            let done = {
                let open = self.files.get(&handle).ok_or(libc::EBADF)?;
                let mut client = self.client.lock().unwrap_or_else(PoisonError::into_inner);
                call(&mut client, open)
            };
            match done {
                Err(Fail::Status(status)) if lost_state(status) && !again => {
                    self.reopen(handle)?;
                }
                done => return done.map_err(|fail| fail.errno()),
            }
        }
        Err(libc::EIO)
    }

    /// Reads at most `size` bytes of file `handle` from `offset`.
    pub fn read(&mut self, handle: u64, offset: u64, size: u32) -> Result<Vec<u8>, c_int> {
        let mut data = Vec::with_capacity(size as usize);
        while data.len() < size as usize {
            let at = offset + data.len() as u64;
            let want = (size as usize - data.len()) as u32;
            let (eof, got) = self.with_open(handle, |client, open| {
                let want = want.min(client.max_read);
                let mut ops = Ops::new();
                ops.putfh(&open.fh).op(op::READ).fixed(&open.stateid).u64(at).u32(want);
                let results = client.run(&open.cred, &ops)?;
                let mut walk = results.walk();
                walk.next(op::PUTFH)?;
                let r = walk.next(op::READ)?;
                let eof = r.bool()?;
                Ok((eof, r.opaque(want as usize)?.to_vec()))
            })?;
            let short = got.is_empty();
            data.extend_from_slice(&got);
            if eof || short {
                break;
            }
        }
        Ok(data)
    }

    /// Writes `data` to file `handle` at `offset`, `stable` as WRITE says:
    /// how many bytes the server took, its verifier, and whether it holds
    /// them only unstably.
    fn write_once(
        &mut self,
        handle: u64,
        offset: u64,
        data: &[u8],
        stable: u32,
    ) -> Result<(usize, [u8; 8], bool), c_int> {
        self.with_open(handle, |client, open| {
            let n = data.len().min(client.max_write as usize);
            let mut ops = Ops::new();
            let w = ops.putfh(&open.fh).op(op::WRITE);
            w.fixed(&open.stateid).u64(offset).u32(stable).opaque(&data[..n]);
            let results = client.run(&open.cred, &ops)?;
            let mut walk = results.walk();
            walk.next(op::PUTFH)?;
            let r = walk.next(op::WRITE)?;
            let count = r.u32()? as usize;
            let committed = r.u32()?;
            let verifier = r.array()?;
            Ok((count.min(n), verifier, committed == UNSTABLE))
        })
    }

    /// Writes `data` to file `handle` at `offset`: how many bytes were
    /// written.
    pub fn write(&mut self, handle: u64, offset: u64, data: &[u8]) -> Result<u32, c_int> {
        let mut done = 0;
        while done < data.len() {
            let at = offset + done as u64;
            let (n, verifier, unstable) = self.write_once(handle, at, &data[done..], UNSTABLE)?;
            if n == 0 {
                return Err(libc::EIO);
            }
            if unstable {
                let open = self.files.get(&handle).ok_or(libc::EBADF)?;
                if open.unstable.verifier.is_some_and(|kept| kept != verifier) {
                    // The server restarted after it took the data kept,
                    // which it may have lost: it is written again, to stay.
                    self.rewrite(handle)?;
                }
                let open = self.files.get_mut(&handle).ok_or(libc::EBADF)?;
                open.unstable.verifier = Some(verifier);
                open.unstable.kept.push((at, data[done..done + n].to_vec()));
                open.unstable.bytes += n;
            }
            done += n;
        }
        if self.files.get(&handle).is_some_and(|open| open.unstable.bytes > KEEP_MAX) {
            self.commit(handle)?;
        }
        u32::try_from(done).map_err(|_| libc::EIO)
    }

    /// Writes the unstable data file `handle` keeps again, FILE_SYNC.
    fn rewrite(&mut self, handle: u64) -> Result<(), c_int> {
        let open = self.files.get_mut(&handle).ok_or(libc::EBADF)?;
        let kept = std::mem::take(&mut open.unstable).kept;
        for (offset, data) in kept {
            let mut done = 0;
            while done < data.len() {
                let at = offset + done as u64;
                let (n, _, _) = self.write_once(handle, at, &data[done..], FILE_SYNC)?;
                if n == 0 {
                    return Err(libc::EIO);
                }
                done += n;
            }
        }
        Ok(())
    }

    /// Has the server commit what file `handle` wrote unstably.
    pub fn commit(&mut self, handle: u64) -> Result<(), c_int> {
        if self.files.get(&handle).ok_or(libc::EBADF)?.unstable.kept.is_empty() {
            return Ok(());
        }
        let verifier = self.with_open(handle, |client, open| {
            let mut ops = Ops::new();
            ops.putfh(&open.fh).op(op::COMMIT).u64(0).u32(0);
            let results = client.run(&open.cred, &ops)?;
            let mut walk = results.walk();
            walk.next(op::PUTFH)?;
            Ok(walk.next(op::COMMIT)?.array::<8>()?)
        })?;
        let open = self.files.get_mut(&handle).ok_or(libc::EBADF)?;
        if open.unstable.verifier == Some(verifier) {
            open.unstable = Unstable::default();
            return Ok(());
        }
        self.rewrite(handle)
    }

    /// Closes file `handle`, after committing what it wrote.
    pub fn release(&mut self, handle: u64) -> Result<(), c_int> {
        let committed = self.commit(handle);
        let open = self.files.remove(&handle).ok_or(libc::EBADF)?;
        if open.generation == self.client().generation {
            let mut ops = Ops::kept();
            ops.putfh(&open.fh).op(op::CLOSE).u32(0).fixed(&open.stateid);
            let _ = self.run(&open.cred, &ops);
        }
        committed
    }

    /// Lists folder `ino`: the listing's handle.
    pub fn list(&mut self, cred: &Cred, ino: u64) -> Result<u64, c_int> {
        let fh = self.nodes.fh(ino)?;
        let most = self.client().max_read.min(1 << 17);
        let mut listed = Vec::new();
        let (mut cookie, mut verifier) = (0u64, [0u8; 8]);
        loop {
            let mut ops = Ops::new();
            let w = ops.putfh(&fh).op(op::READDIR);
            w.u64(cookie).fixed(&verifier).u32(most / 4).u32(most);
            attrs::ask(w, attrs::LISTED);
            let results = self.run(cred, &ops)?;
            let (eof, any) = (|| -> Result<_, Fail> {
                let mut walk = results.walk();
                walk.next(op::PUTFH)?;
                let r = walk.next(op::READDIR)?;
                verifier = r.array()?;
                let mut any = false;
                while r.bool()? {
                    cookie = r.u64()?;
                    let name = r.opaque(4096)?.to_vec();
                    let a = attrs::decode(r)?;
                    any = true;
                    if name == b"." || name == b".." {
                        continue;
                    }
                    let kind = if a.error == 0 { file_type(a.kind) } else { FileType::RegularFile };
                    listed.push(Listed { name: OsString::from_vec(name), kind, fileid: a.fileid });
                }
                Ok((r.bool()?, any))
            })()
            .map_err(|fail| fail.errno())?;
            if eof || !any {
                break;
            }
            if listed.len() > LISTED_MAX {
                return Err(libc::EFBIG);
            }
        }
        let handle = self.handle();
        self.listings.insert(handle, listed);
        Ok(handle)
    }

    /// The names of listing `handle`, with their kinds.
    pub fn names(&self, handle: u64) -> Result<Vec<(OsString, FileType)>, c_int> {
        let listing = self.listings.get(&handle).ok_or(libc::EBADF)?;
        Ok(listing.iter().map(|listed| (listed.name.clone(), listed.kind)).collect())
    }

    /// The names of listing `handle` of folder `ino` from the `from`th
    /// on, `.` and `..` first, each with its place, number and kind: a
    /// page of a large folder starts where the last ended, without
    /// going through those before it.
    pub(crate) fn listed(
        &self,
        ino: u64,
        handle: u64,
        from: usize,
    ) -> Result<impl Iterator<Item = (usize, u64, FileType, &OsStr)> + '_, c_int> {
        let listing = self.listings.get(&handle).ok_or(libc::EBADF)?;
        let dots = [
            (ino, FileType::Directory, OsStr::new(".")),
            (ROOT, FileType::Directory, OsStr::new("..")),
        ];
        let start = from.saturating_sub(dots.len());
        let names =
            listing.get(start..).unwrap_or_default().iter().enumerate().map(move |(j, listed)| {
                let i = start + j;
                let number = if listed.fileid > ROOT { listed.fileid } else { u64::MAX - i as u64 };
                (dots.len() + i, number, listed.kind, listed.name.as_os_str())
            });
        let dots = dots
            .into_iter()
            .enumerate()
            .skip(from)
            .map(|(i, (number, kind, name))| (i, number, kind, name));
        Ok(dots.chain(names))
    }

    /// The room and names left on the server's filesystem.
    pub fn space(&mut self, cred: &Cred) -> Result<Space, c_int> {
        let fh = self.nodes.fh(ROOT)?;
        let mut ops = Ops::new();
        ops.putfh(&fh);
        getattr(&mut ops, attrs::SPACE);
        let results = self.run(cred, &ops)?;
        let a = (|| -> Result<_, Fail> {
            let mut walk = results.walk();
            walk.next(op::PUTFH)?;
            Ok(attrs::decode(walk.next(op::GETATTR)?)?)
        })()
        .map_err(|fail| fail.errno())?;
        Ok(Space {
            blocks: a.space_total / BLOCK,
            free: a.space_free / BLOCK,
            avail: a.space_avail / BLOCK,
            files: a.files_total,
            files_free: a.files_avail,
            max_name: if a.max_name == 0 { 255 } else { a.max_name },
        })
    }

    /// Has the client forget its client ID, as one that restarted with
    /// `verifier`: for the tests.
    #[cfg(test)]
    pub fn forget_client(&mut self, verifier: [u8; 8]) {
        self.client().forget(verifier);
    }

    /// Unmounted: what was written is committed, the files closed and
    /// the session ended.
    pub fn close_all(&mut self) {
        let handles: Vec<u64> = self.files.keys().copied().collect();
        for handle in handles {
            let _ = self.release(handle);
        }
        self.client().goodbye();
    }
}

/// A time to set, from the kernel's.
fn when(time: TimeOrNow) -> When {
    match time {
        TimeOrNow::SpecificTime(at) => When::At(at),
        TimeOrNow::Now => When::Now,
    }
}

/// Answers with `attr`, or the error.
fn entry(reply: ReplyEntry, done: Result<FileAttr, c_int>) {
    match done {
        Ok(attr) => reply.entry(&TTL, &attr, 0),
        Err(err) => reply.error(err),
    }
}

fn empty(reply: ReplyEmpty, done: Result<(), c_int>) {
    match done {
        Ok(()) => reply.ok(),
        Err(err) => reply.error(err),
    }
}

impl Filesystem for NfsFs {
    fn init(&mut self, _req: &Request<'_>, config: &mut KernelConfig) -> Result<(), c_int> {
        let most = self.client().max_write;
        let _ = config.set_max_write(most);
        Ok(())
    }

    fn destroy(&mut self) {
        self.close_all();
    }

    fn lookup(&mut self, req: &Request<'_>, parent: u64, name: &OsStr, reply: ReplyEntry) {
        let cred = self.cred(req.uid(), req.gid());
        entry(reply, self.look_up(&cred, parent, name));
    }

    fn forget(&mut self, _req: &Request<'_>, ino: u64, nlookup: u64) {
        self.let_go(ino, nlookup);
    }

    fn getattr(&mut self, req: &Request<'_>, ino: u64, _fh: Option<u64>, reply: ReplyAttr) {
        let cred = self.cred(req.uid(), req.gid());
        match self.attributes(&cred, ino) {
            Ok(attr) => reply.attr(&TTL, &attr),
            Err(err) => reply.error(err),
        }
    }

    fn setattr(
        &mut self,
        req: &Request<'_>,
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
        let cred = self.cred(req.uid(), req.gid());
        let set = Set {
            size,
            mode: mode.map(|mode| mode & 0o7777),
            uid,
            gid,
            atime: atime.map(when),
            mtime: mtime.map(when),
        };
        let done = if set == Set::default() {
            self.attributes(&cred, ino)
        } else {
            self.set(&cred, ino, &set, fh)
        };
        match done {
            Ok(attr) => reply.attr(&TTL, &attr),
            Err(err) => reply.error(err),
        }
    }

    fn readlink(&mut self, req: &Request<'_>, ino: u64, reply: ReplyData) {
        let cred = self.cred(req.uid(), req.gid());
        match self.read_link(&cred, ino) {
            Ok(target) => reply.data(&target),
            Err(err) => reply.error(err),
        }
    }

    fn mknod(
        &mut self,
        req: &Request<'_>,
        parent: u64,
        name: &OsStr,
        mode: u32,
        umask: u32,
        _rdev: u32,
        reply: ReplyEntry,
    ) {
        let cred = self.cred(req.uid(), req.gid());
        let perm = mode & !umask & 0o7777;
        let done = match mode & libc::S_IFMT {
            libc::S_IFIFO => self.make(&cred, parent, name, Made::Fifo, perm),
            libc::S_IFSOCK => self.make(&cred, parent, name, Made::Socket, perm),
            libc::S_IFREG => {
                let flags = libc::O_WRONLY | libc::O_EXCL;
                self.create(&cred, parent, name, perm, flags).and_then(|(attr, handle)| {
                    self.release(handle)?;
                    Ok(attr)
                })
            }
            // Devices: a mount of a program's opens none (ADR 0226).
            _ => Err(libc::EPERM),
        };
        entry(reply, done);
    }

    fn mkdir(
        &mut self,
        req: &Request<'_>,
        parent: u64,
        name: &OsStr,
        mode: u32,
        umask: u32,
        reply: ReplyEntry,
    ) {
        let cred = self.cred(req.uid(), req.gid());
        entry(reply, self.make(&cred, parent, name, Made::Folder, mode & !umask & 0o7777));
    }

    fn unlink(&mut self, req: &Request<'_>, parent: u64, name: &OsStr, reply: ReplyEmpty) {
        let cred = self.cred(req.uid(), req.gid());
        empty(reply, self.remove(&cred, parent, name));
    }

    fn rmdir(&mut self, req: &Request<'_>, parent: u64, name: &OsStr, reply: ReplyEmpty) {
        let cred = self.cred(req.uid(), req.gid());
        empty(reply, self.remove(&cred, parent, name));
    }

    fn symlink(
        &mut self,
        req: &Request<'_>,
        parent: u64,
        link_name: &OsStr,
        target: &Path,
        reply: ReplyEntry,
    ) {
        let cred = self.cred(req.uid(), req.gid());
        let made = Made::Link(target.as_os_str().as_bytes());
        entry(reply, self.make(&cred, parent, link_name, made, 0o777));
    }

    fn rename(
        &mut self,
        req: &Request<'_>,
        parent: u64,
        name: &OsStr,
        newparent: u64,
        newname: &OsStr,
        flags: u32,
        reply: ReplyEmpty,
    ) {
        if flags != 0 {
            // RENAME_NOREPLACE and RENAME_EXCHANGE: NFS has neither.
            return reply.error(libc::EINVAL);
        }
        let cred = self.cred(req.uid(), req.gid());
        empty(reply, self.rename(&cred, parent, name, newparent, newname));
    }

    fn link(
        &mut self,
        req: &Request<'_>,
        ino: u64,
        newparent: u64,
        newname: &OsStr,
        reply: ReplyEntry,
    ) {
        let cred = self.cred(req.uid(), req.gid());
        entry(reply, NfsFs::link(self, &cred, ino, newparent, newname));
    }

    fn open(&mut self, req: &Request<'_>, ino: u64, flags: i32, reply: ReplyOpen) {
        let cred = self.cred(req.uid(), req.gid());
        match NfsFs::open(self, &cred, ino, flags) {
            Ok(handle) => reply.opened(handle, 0),
            Err(err) => reply.error(err),
        }
    }

    fn create(
        &mut self,
        req: &Request<'_>,
        parent: u64,
        name: &OsStr,
        mode: u32,
        umask: u32,
        flags: i32,
        reply: ReplyCreate,
    ) {
        let cred = self.cred(req.uid(), req.gid());
        match NfsFs::create(self, &cred, parent, name, mode & !umask & 0o7777, flags) {
            Ok((attr, handle)) => reply.created(&TTL, &attr, 0, handle, 0),
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
        let Ok(offset) = u64::try_from(offset) else { return reply.error(libc::EINVAL) };
        match NfsFs::read(self, fh, offset, size) {
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
        let Ok(offset) = u64::try_from(offset) else { return reply.error(libc::EINVAL) };
        match NfsFs::write(self, fh, offset, data) {
            Ok(n) => reply.written(n),
            Err(err) => reply.error(err),
        }
    }

    fn flush(&mut self, _req: &Request<'_>, _ino: u64, fh: u64, _owner: u64, reply: ReplyEmpty) {
        // A close: what was written reaches the server's disk, for the
        // next to open the file anywhere.
        empty(reply, self.commit(fh));
    }

    fn fsync(&mut self, _req: &Request<'_>, _ino: u64, fh: u64, _data: bool, reply: ReplyEmpty) {
        empty(reply, self.commit(fh));
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
        empty(reply, NfsFs::release(self, fh));
    }

    fn opendir(&mut self, req: &Request<'_>, ino: u64, _flags: i32, reply: ReplyOpen) {
        let cred = self.cred(req.uid(), req.gid());
        match self.list(&cred, ino) {
            Ok(handle) => reply.opened(handle, 0),
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
        let from = usize::try_from(offset).unwrap_or(0);
        let entries = match self.listed(ino, fh, from) {
            Ok(entries) => entries,
            Err(err) => return reply.error(err),
        };
        for (i, number, kind, name) in entries {
            if reply.add(number, i as i64 + 1, kind, name) {
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

    fn statfs(&mut self, req: &Request<'_>, _ino: u64, reply: ReplyStatfs) {
        let cred = self.cred(req.uid(), req.gid());
        match self.space(&cred) {
            Ok(s) => {
                let block = BLOCK as u32;
                reply.statfs(
                    s.blocks,
                    s.free,
                    s.avail,
                    s.files,
                    s.files_free,
                    block,
                    s.max_name,
                    block,
                );
            }
            Err(err) => reply.error(err),
        }
    }
}
