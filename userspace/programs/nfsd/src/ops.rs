// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The operations on files (RFC 8881 section 18, RFC 7862 section 15):
//! each on the COMPOUND's current file, done by the worker of the person
//! who calls, so the kernel decides what they may do.

use std::time::Instant;

use crate::attr::{self, Facts, Settings};
use crate::fh::Fh;
use crate::ipc::{Changes, Create, FsStat, Make, Reply, Request, Stat};
use crate::nfs::{self, Attrs, IO_MAX, err, kind, op};
use crate::rpc::AUTH_SYS;
use crate::server::{Backend, Cur, Done, Fail, Here, Server};
use crate::state::{FileKey, Kind, Locker, Open, Stateid};
use crate::worker::ATTACHED;
use crate::xdr::{Reader, Writer};

/// ACCESS's bits.
const ACCESS_READ: u32 = 0x01;
const ACCESS_LOOKUP: u32 = 0x02;
const ACCESS_MODIFY: u32 = 0x04;
const ACCESS_EXTEND: u32 = 0x08;
const ACCESS_DELETE: u32 = 0x10;
const ACCESS_EXECUTE: u32 = 0x20;

/// OPEN's share access bits: read and write.
const SHARE_READ: u32 = 1;
const SHARE_WRITE: u32 = 2;

/// OPEN4_RESULT_LOCKTYPE_POSIX: locks are POSIX's, split and merged.
const LOCKTYPE_POSIX: u32 = 4;

/// The most entries a worker looks through for a file whose handle
/// nfsd has not seen since it started.
const FIND_MAX: u32 = 1_000_000;

/// The longest name, and the longest opaque owner.
const NAME_MAX: usize = 255;
const OWNER_MAX: usize = 1024;

/// How an OPEN makes its file.
enum How {
    Unchecked(Settings),
    Guarded(Settings),
    Exclusive([u8; 8], Settings),
}

/// A file's stat, filesystem ID, handle, and its filesystem's counts.
type Facts4 = (Stat, (u64, u64), Vec<u8>, Option<FsStat>);

/// What LOCK's locker is: a new lock-owner, through an open, or one
/// that has a stateid.
enum LockerArg {
    New(Stateid, Vec<u8>),
    Existing(Stateid),
}

/// The NFS status for a worker's errno.
pub fn status_of(errno: i32) -> u32 {
    match errno {
        libc::EPERM => err::PERM,
        libc::ENOENT => err::NOENT,
        libc::ENXIO => err::NXIO,
        libc::EACCES => err::ACCESS,
        libc::EEXIST => err::EXIST,
        libc::EXDEV => err::XDEV,
        libc::ENOTDIR => err::NOTDIR,
        libc::EISDIR => err::ISDIR,
        libc::EINVAL => err::INVAL,
        libc::EFBIG => err::FBIG,
        libc::ENOSPC => err::NOSPC,
        libc::EROFS => err::ROFS,
        libc::EMLINK => err::MLINK,
        libc::ENAMETOOLONG => err::NAMETOOLONG,
        libc::ENOTEMPTY => err::NOTEMPTY,
        libc::EDQUOT => err::DQUOT,
        libc::ESTALE => err::STALE,
        libc::ELOOP => err::SYMLINK,
        libc::EOPNOTSUPP => err::NOTSUPP,
        libc::EAGAIN | libc::EINTR => err::DELAY,
        _ => err::IO,
    }
}

/// Fails unless `name` is a name: not empty, at most 255 bytes, neither
/// `.` nor `..`, and without `/` or NUL.
fn check_name(name: &[u8]) -> Result<(), Fail> {
    if name.is_empty() {
        return Err(err::INVAL.into());
    }
    if name.len() > NAME_MAX {
        return Err(err::NAMETOOLONG.into());
    }
    if name == b"." || name == b".." || name.iter().any(|&b| b == b'/' || b == 0) {
        return Err(err::BADNAME.into());
    }
    Ok(())
}

/// `rel` below `root`.
fn join(root: &[u8], rel: &[u8]) -> Vec<u8> {
    if rel.is_empty() {
        return root.to_vec();
    }
    let mut out = Vec::with_capacity(root.len() + 1 + rel.len());
    out.extend_from_slice(root);
    out.push(b'/');
    out.extend_from_slice(rel);
    out
}

/// The current file, or NOFILEHANDLE.
fn here(cur: &Cur) -> Result<&Here, Fail> {
    cur.here.as_ref().ok_or(Fail::Status(err::NOFILEHANDLE))
}

/// The folder a file in `fh` is learned in: none for a share's own.
fn folder_of(fh: Fh) -> Option<(u64, u64)> {
    match fh {
        Fh::File(_, dev, ino) => Some((dev, ino)),
        Fh::Root | Fh::Share(_) => None,
    }
}

/// The status for reading or writing a file of `kind` that is not
/// regular.
fn not_regular(kind: u32) -> Fail {
    Fail::Status(match kind {
        kind::DIR => err::ISDIR,
        kind::LNK => err::SYMLINK,
        _ => err::WRONG_TYPE,
    })
}

/// The status for a folder's operation on a file of `kind`.
fn not_folder(kind: u32) -> Fail {
    Fail::Status(if kind == kind::LNK { err::SYMLINK } else { err::NOTDIR })
}

/// Where a lock from `offset` for `length` bytes ends: all ones for the
/// end of any file.
fn range_end(offset: u64, length: u64) -> Result<u64, Fail> {
    if length == 0 {
        return Err(err::INVAL.into());
    }
    if length == u64::MAX {
        return Ok(u64::MAX);
    }
    offset.checked_add(length).ok_or(Fail::Status(err::INVAL))
}

/// change_info4: not atomic, from `before` to `after`.
fn cinfo(w: &mut Writer, before: u64, after: u64) {
    w.bool(false).u64(before).u64(after);
}

/// SECINFO's answer: AUTH_SYS alone, until Kerberos (ADR 0341).
fn secinfo() -> Writer {
    let mut w = Writer::new();
    w.u32(1).u32(AUTH_SYS);
    w
}

/// LOCK4denied: the lock in the way.
fn denied(range: &crate::state::Range) -> Fail {
    let mut w = Writer::new();
    let length = if range.end == u64::MAX { u64::MAX } else { range.end - range.start };
    w.u64(range.start).u64(length).u32(if range.write { 2 } else { 1 });
    w.u64(range.owner.0).opaque(&range.owner.1);
    Fail::With(err::DENIED, w)
}

/// Reads a stateid, the current one for the special value that names it.
fn stateid_arg(cur: &Cur, r: &mut Reader) -> Result<Stateid, Fail> {
    let stateid = Stateid::read(r)?;
    if stateid == Stateid::CURRENT {
        return cur.stateid.ok_or(Fail::Status(err::BAD_STATEID));
    }
    Ok(stateid)
}

impl<B: Backend> Server<B> {
    /// One operation on files.
    pub fn file_operation(&mut self, code: u32, cur: &mut Cur, r: &mut Reader) -> Done {
        match code {
            op::PUTROOTFH | op::PUTPUBFH => {
                cur.here = Some(Here::Root);
                cur.stateid = None;
                Ok(Writer::new())
            }
            op::PUTFH => {
                let fh = r.opaque(128)?.to_vec();
                cur.here = Some(self.put_fh(cur, &fh)?);
                cur.stateid = None;
                Ok(Writer::new())
            }
            op::GETFH => {
                let fh = match here(cur)? {
                    Here::Root => Fh::Root,
                    Here::File { fh, .. } => *fh,
                };
                let mut w = Writer::new();
                w.opaque(&fh.encode());
                Ok(w)
            }
            op::SAVEFH => {
                cur.saved = Some(here(cur)?.clone());
                cur.saved_stateid = cur.stateid;
                Ok(Writer::new())
            }
            op::RESTOREFH => {
                cur.here = Some(cur.saved.clone().ok_or(err::RESTOREFH)?);
                cur.stateid = cur.saved_stateid;
                Ok(Writer::new())
            }
            op::GETATTR => self.getattr(cur, r),
            op::ACCESS => self.access(cur, r),
            op::LOOKUP => self.lookup(cur, r),
            op::LOOKUPP => self.lookupp(cur),
            op::READDIR => self.readdir(cur, r),
            op::READLINK => self.readlink(cur),
            op::READ => self.read(cur, r),
            op::WRITE => self.write(cur, r),
            op::COMMIT => self.commit(cur, r),
            op::OPEN => self.open(cur, r),
            op::CLOSE => self.close(cur, r),
            op::OPEN_DOWNGRADE => self.open_downgrade(cur, r),
            op::SETATTR => self.setattr(cur, r).map_err(|fail| match fail {
                // SETATTR says which it set, none, when it fails.
                Fail::Status(s) => {
                    let mut w = Writer::new();
                    w.u32(0);
                    Fail::With(s, w)
                }
                with => with,
            }),
            op::CREATE => self.create(cur, r),
            op::REMOVE => self.remove(cur, r),
            op::RENAME => self.rename(cur, r),
            op::LINK => self.link(cur, r),
            op::LOCK => self.lock(cur, r),
            op::LOCKT => self.lockt(cur, r),
            op::LOCKU => self.locku(cur, r),
            op::FREE_STATEID => self.free_stateid(cur, r),
            op::TEST_STATEID => self.test_stateid(r),
            op::SECINFO => {
                let name = r.opaque(OWNER_MAX)?.to_vec();
                check_name(&name)?;
                here(cur)?;
                // SECINFO uses up the current filehandle.
                cur.here = None;
                Ok(secinfo())
            }
            op::SECINFO_NO_NAME => {
                let _style = r.u32()?;
                here(cur)?;
                cur.here = None;
                Ok(secinfo())
            }
            op::VERIFY => self.verify(cur, r, true),
            op::NVERIFY => self.verify(cur, r, false),
            op::SEEK => self.seek(cur, r),
            // nfsd gives no delegations: there are none to return.
            op::DELEGRETURN => {
                Stateid::read(r)?;
                Err(err::BAD_STATEID.into())
            }
            // NFS 4's other operations: 4.0's alone, pNFS's, delegations',
            // and 4.2's copies, allocations and extended attributes.
            _ if (op::ACCESS..=op::REMOVEXATTR).contains(&code) => Err(err::NOTSUPP.into()),
            _ => Err(err::OP_ILLEGAL.into()),
        }
    }

    /// The path the workers reach share `share`'s folder at.
    pub fn root_path(&self, share: usize) -> Vec<u8> {
        let share = &self.shares[share];
        if self.attached {
            format!("{ATTACHED}{:016x}", share.key).into_bytes()
        } else {
            share.folder.as_os_str().as_encoded_bytes().to_vec()
        }
    }

    /// The share keyed `key`, if the caller's address may reach it.
    fn share_of(&self, cur: &Cur, key: u64) -> Result<usize, Fail> {
        let at = self.shares.iter().position(|s| s.key == key).ok_or(err::STALE)?;
        if self.shares[at].allows(cur.peer) { Ok(at) } else { Err(err::ACCESS.into()) }
    }

    /// Fails if share `share` is read-only.
    fn writable(&self, share: usize) -> Result<(), Fail> {
        if self.shares[share].read_only { Err(err::ROFS.into()) } else { Ok(()) }
    }

    /// Asks the worker of the person numbered `uid`.
    fn ask(&mut self, uid: u32, request: Request) -> Result<Reply, Fail> {
        self.backend.call(uid, request).map_err(|errno| Fail::Status(status_of(errno)))
    }

    /// The stat of `path`.
    fn stat_path(&mut self, uid: u32, path: &[u8]) -> Result<Stat, Fail> {
        match self.ask(uid, Request::Stat(path.to_vec()))? {
            Reply::Stat(stat) => Ok(stat),
            _ => Err(err::SERVERFAULT.into()),
        }
    }

    /// The change attribute of the folder at `path` now, for change_info4.
    fn change_now(&mut self, uid: u32, path: &[u8]) -> u64 {
        self.stat_path(uid, path).map_or(0, |s| attr::change_of(&s))
    }

    /// The root's stat: a folder no one may change, whose times are when
    /// the shares last changed.
    fn root_stat(&self) -> Stat {
        let time = self.root_time();
        Stat {
            kind: kind::DIR,
            mode: 0o040_555,
            nlink: 2,
            size: 4096,
            ino: 1,
            atime: time,
            mtime: time,
            ctime: time,
            ..Stat::default()
        }
    }

    /// The current file's stat, read again if something changed it.
    fn stat_here(&mut self, cur: &mut Cur) -> Result<Stat, Fail> {
        let uid = cur.uid;
        match cur.here.as_mut().ok_or(err::NOFILEHANDLE)? {
            Here::Root => Ok(self.root_stat()),
            Here::File { path, stat, .. } => {
                if let Some(s) = stat {
                    return Ok(s.clone());
                }
                let s = self.stat_path(uid, path)?;
                *stat = Some(s.clone());
                Ok(s)
            }
        }
    }

    /// The current file, which must be in a share: its share, path and
    /// handle; `root` is the status for the root.
    fn file_here(cur: &Cur, root: u32) -> Result<(usize, Vec<u8>, Fh), Fail> {
        match here(cur)? {
            Here::Root => Err(root.into()),
            Here::File { share, path, fh, .. } => Ok((*share, path.clone(), *fh)),
        }
    }

    /// The current file's key, for opens and locks, which are on regular
    /// files alone.
    fn key_here(&mut self, cur: &mut Cur) -> Result<FileKey, Fail> {
        let (_, _, fh) = Self::file_here(cur, err::ISDIR)?;
        let s = self.stat_here(cur)?;
        match fh {
            Fh::File(key, dev, ino) if s.kind == kind::REG => Ok((key, dev, ino)),
            _ => Err(not_regular(s.kind)),
        }
    }

    /// The current file, which must be a folder: its share, path and
    /// handle.
    fn folder_here(&mut self, cur: &mut Cur, root: u32) -> Result<(usize, Vec<u8>, Fh), Fail> {
        let found = Self::file_here(cur, root)?;
        let s = self.stat_here(cur)?;
        if s.kind != kind::DIR {
            return Err(not_folder(s.kind));
        }
        Ok(found)
    }

    /// Forgets the current file's stat, after a change to it.
    fn changed(cur: &mut Cur) {
        if let Some(Here::File { stat, .. }) = cur.here.as_mut() {
            *stat = None;
        }
    }

    /// PUTFH: the file `bytes` name, found where it was seen or looked
    /// for below its share.
    fn put_fh(&mut self, cur: &Cur, bytes: &[u8]) -> Result<Here, Fail> {
        let fh = Fh::decode(bytes).ok_or(err::BADHANDLE)?;
        let (key, dev, ino) = match fh {
            Fh::Root => return Ok(Here::Root),
            Fh::Share(key) => {
                let share = self.share_of(cur, key)?;
                let path = self.root_path(share);
                let stat = self.stat_path(cur.uid, &path)?;
                return Ok(Here::File { share, path, fh, stat: Some(stat) });
            }
            Fh::File(key, dev, ino) => (key, dev, ino),
        };
        let share = self.share_of(cur, key)?;
        let root = self.root_path(share);
        if let Some(rel) = self.places.path(key, dev, ino) {
            let path = join(&root, &rel);
            match self.backend.call(cur.uid, Request::Stat(path.clone())) {
                Ok(Reply::Stat(s)) if s.dev == dev && s.ino == ino => {
                    return Ok(Here::File { share, path, fh, stat: Some(s) });
                }
                Err(libc::EACCES) => return Err(err::ACCESS.into()),
                // Somewhere else now, or gone.
                _ => self.places.forget(key, dev, ino),
            }
        }
        // Not seen since nfsd started, or moved by someone else; looked
        // for once in a while.
        let now = Instant::now();
        if self.places.missing(key, dev, ino, cur.uid, now) {
            return Err(err::STALE.into());
        }
        match self.ask(cur.uid, Request::Find(root.clone(), dev, ino, FIND_MAX))? {
            Reply::Found(Some(rel)) => {
                let path = join(&root, &rel);
                let stat = self.stat_path(cur.uid, &path)?;
                if (stat.dev, stat.ino) != (dev, ino) {
                    return Err(err::STALE.into());
                }
                self.places.learn(key, None, &rel, dev, ino);
                Ok(Here::File { share, path, fh, stat: Some(stat) })
            }
            _ => {
                self.places.not_found(key, dev, ino, cur.uid, now);
                Err(err::STALE.into())
            }
        }
    }

    /// The current file's stat, filesystem ID, handle, and its
    /// filesystem's counts if `wanted` has them.
    fn facts_here(&mut self, cur: &mut Cur, wanted: Attrs) -> Result<Facts4, Fail> {
        let stat = self.stat_here(cur)?;
        let counts = !wanted.and(attr::FS_ATTRS).is_empty();
        match here(cur)? {
            Here::Root => {
                let fs = counts.then(FsStat::default);
                Ok((stat, (0, 0), Fh::Root.encode(), fs))
            }
            Here::File { share, path, fh, .. } => {
                let (key, path, fh) = (self.shares[*share].key, path.clone(), *fh);
                let fs = if counts {
                    match self.ask(cur.uid, Request::FsStat(path))? {
                        Reply::FsStat(fs) => Some(fs),
                        _ => None,
                    }
                } else {
                    None
                };
                Ok((stat.clone(), (key, stat.dev), fh.encode(), fs))
            }
        }
    }

    /// GETATTR.
    fn getattr(&mut self, cur: &mut Cur, r: &mut Reader) -> Done {
        let wanted = Attrs::from_words(&r.bitmap()?);
        if !wanted.and(attr::WRITE_ONLY).is_empty() {
            return Err(err::INVAL.into());
        }
        let (stat, fsid, fh, fs) = self.facts_here(cur, wanted)?;
        let mut w = Writer::new();
        attr::encode(&mut w, wanted, &Facts { stat: &stat, fsid, fh: &fh, fs });
        Ok(w)
    }

    /// VERIFY and NVERIFY: whether the attributes are as the client says.
    fn verify(&mut self, cur: &mut Cur, r: &mut Reader, verify: bool) -> Done {
        let claim = attr::decode_claim(r)?;
        let (stat, fsid, fh, fs) = self.facts_here(cur, claim.attrs)?;
        let given = attr::given(claim.attrs, fs.is_some());
        let ours = attr::values(given, &Facts { stat: &stat, fsid, fh: &fh, fs });
        let same = given == claim.attrs && ours.bytes == claim.values;
        match (verify, same) {
            (true, false) => Err(err::NOT_SAME.into()),
            (false, true) => Err(err::SAME.into()),
            _ => Ok(Writer::new()),
        }
    }

    /// ACCESS: what the person may do with the current file.
    fn access(&mut self, cur: &mut Cur, r: &mut Reader) -> Done {
        let asked = r.u32()?;
        let supported = asked
            & (ACCESS_READ
                | ACCESS_LOOKUP
                | ACCESS_MODIFY
                | ACCESS_EXTEND
                | ACCESS_DELETE
                | ACCESS_EXECUTE);
        let granted = match here(cur)? {
            Here::Root => supported & (ACCESS_READ | ACCESS_LOOKUP | ACCESS_EXECUTE),
            Here::File { share, path, .. } => {
                let (share, path) = (*share, path.clone());
                let changes = ACCESS_MODIFY | ACCESS_EXTEND | ACCESS_DELETE;
                let mut want = 0;
                if supported & ACCESS_READ != 0 {
                    want |= 4;
                }
                if supported & changes != 0 {
                    want |= 2;
                }
                if supported & (ACCESS_LOOKUP | ACCESS_EXECUTE) != 0 {
                    want |= 1;
                }
                let rwx = match self.ask(cur.uid, Request::Access(path, want))? {
                    Reply::Number(n) => n as u32,
                    _ => return Err(err::SERVERFAULT.into()),
                };
                let mut granted = 0;
                if rwx & 4 != 0 {
                    granted |= supported & ACCESS_READ;
                }
                if rwx & 2 != 0 && !self.shares[share].read_only {
                    granted |= supported & changes;
                }
                if rwx & 1 != 0 {
                    granted |= supported & (ACCESS_LOOKUP | ACCESS_EXECUTE);
                }
                granted
            }
        };
        let mut w = Writer::new();
        w.u32(supported).u32(granted);
        Ok(w)
    }

    /// LOOKUP: a share in the root, or a name in a folder.
    fn lookup(&mut self, cur: &mut Cur, r: &mut Reader) -> Done {
        let name = r.opaque(OWNER_MAX)?.to_vec();
        check_name(&name)?;
        if let Here::Root = here(cur)? {
            let text = std::str::from_utf8(&name).map_err(|_| err::NOENT)?;
            let share = self
                .shares
                .iter()
                .position(|s| manifest::shares::same_name(&s.name, text) && s.allows(cur.peer))
                .ok_or(err::NOENT)?;
            let path = self.root_path(share);
            let stat = self.stat_path(cur.uid, &path)?;
            let fh = Fh::Share(self.shares[share].key);
            cur.here = Some(Here::File { share, path, fh, stat: Some(stat) });
            cur.stateid = None;
            return Ok(Writer::new());
        }
        let (share, folder, folder_fh) = self.folder_here(cur, err::NOENT)?;
        let path = join(&folder, &name);
        let stat = self.stat_path(cur.uid, &path)?;
        let key = self.shares[share].key;
        self.places.learn(key, folder_of(folder_fh), &name, stat.dev, stat.ino);
        let fh = Fh::File(key, stat.dev, stat.ino);
        cur.here = Some(Here::File { share, path, fh, stat: Some(stat) });
        cur.stateid = None;
        Ok(Writer::new())
    }

    /// LOOKUPP: the folder the current one is in; the root's for a
    /// share's.
    fn lookupp(&mut self, cur: &mut Cur) -> Done {
        let (share, path, fh) = self.folder_here(cur, err::NOENT)?;
        if let Fh::Share(_) = fh {
            cur.here = Some(Here::Root);
            cur.stateid = None;
            return Ok(Writer::new());
        }
        let root = self.root_path(share);
        let cut = path.iter().rposition(|&b| b == b'/').ok_or(err::SERVERFAULT)?;
        let parent = path[..cut].to_vec();
        let key = self.shares[share].key;
        let stat = self.stat_path(cur.uid, &parent)?;
        let fh = if parent == root {
            Fh::Share(key)
        } else {
            let rel = parent.get(root.len() + 1..).ok_or(err::SERVERFAULT)?;
            if self.places.path(key, stat.dev, stat.ino).as_deref() != Some(rel) {
                self.places.learn(key, None, rel, stat.dev, stat.ino);
            }
            Fh::File(key, stat.dev, stat.ino)
        };
        cur.here = Some(Here::File { share, path: parent, fh, stat: Some(stat) });
        cur.stateid = None;
        Ok(Writer::new())
    }

    /// READDIR: a folder's entries from a cookie, each with the
    /// attributes asked for, as many as fit.
    fn readdir(&mut self, cur: &mut Cur, r: &mut Reader) -> Done {
        let cookie = r.u64()?;
        let _verifier: [u8; 8] = r.array()?;
        let _dircount = r.u32()?;
        let maxcount = r.u32()?;
        let wanted = Attrs::from_words(&r.bitmap()?);
        if cookie == 1 || cookie == 2 {
            return Err(err::BAD_COOKIE.into());
        }
        // Entry k, from 0, has cookie k + 3: 0 starts, 1 and 2 are `.`
        // and `..`, which NFS does not list.
        let start = cookie.saturating_sub(2);
        // What fits: maxcount, less the status, verifier and end.
        let budget = (maxcount as usize).min(crate::rpc::RECORD_MAX).saturating_sub(24);
        let counts = !wanted.and(attr::FS_ATTRS).is_empty();
        let (entries, last, fs) = match here(cur)?.clone() {
            Here::Root => self.root_entries(cur, start),
            Here::File { .. } => {
                let (share, path, fh) = self.folder_here(cur, err::NOTDIR)?;
                let key = self.shares[share].key;
                let count = (budget / 96).clamp(1, 4096) as u32;
                let Reply::List(listed, last) =
                    self.ask(cur.uid, Request::List(path.clone(), start, count))?
                else {
                    return Err(err::SERVERFAULT.into());
                };
                let fs = if counts {
                    match self.ask(cur.uid, Request::FsStat(path))? {
                        Reply::FsStat(fs) => Some(fs),
                        _ => None,
                    }
                } else {
                    None
                };
                let entries = listed
                    .into_iter()
                    .map(|(name, stat)| {
                        self.places.learn(key, folder_of(fh), &name, stat.dev, stat.ino);
                        let fh = Fh::File(key, stat.dev, stat.ino);
                        let fsid = (key, stat.dev);
                        (name, stat, fh, fsid)
                    })
                    .collect();
                (entries, last, fs)
            }
        };
        let mut w = Writer::new();
        w.fixed(&[0; 8]);
        let mut all = true;
        let mut fit = 0;
        let mut index = start;
        for (name, stat, fh, fsid) in entries {
            index += 1;
            let mut e = Writer::new();
            e.u32(1).u64(index + 2).opaque(&name);
            let fh = fh.encode();
            attr::encode(&mut e, wanted, &Facts { stat: &stat, fsid, fh: &fh, fs });
            if w.len() + e.len() > budget {
                all = false;
                break;
            }
            w.bytes.extend_from_slice(&e.bytes);
            fit += 1;
        }
        if fit == 0 && !(all && last) {
            return Err(err::TOOSMALL.into());
        }
        w.u32(0).bool(all && last);
        Ok(w)
    }

    /// The root's entries from `start`: the shares the client may reach,
    /// by name, those whose folder the person cannot reach left out.
    #[allow(clippy::type_complexity)]
    fn root_entries(
        &mut self,
        cur: &Cur,
        start: u64,
    ) -> (Vec<(Vec<u8>, Stat, Fh, (u64, u64))>, bool, Option<FsStat>) {
        let mut shares: Vec<usize> =
            (0..self.shares.len()).filter(|&i| self.shares[i].allows(cur.peer)).collect();
        shares.sort_by(|&a, &b| self.shares[a].name.cmp(&self.shares[b].name));
        let mut entries = Vec::new();
        for &share in shares.iter().skip(usize::try_from(start).unwrap_or(usize::MAX)) {
            let path = self.root_path(share);
            let Ok(stat) = self.stat_path(cur.uid, &path) else { continue };
            let key = self.shares[share].key;
            entries.push((
                self.shares[share].name.clone().into_bytes(),
                stat.clone(),
                Fh::Share(key),
                (key, stat.dev),
            ));
        }
        (entries, true, Some(FsStat::default()))
    }

    /// READLINK.
    fn readlink(&mut self, cur: &mut Cur) -> Done {
        let (_, path, _) = Self::file_here(cur, err::ISDIR)?;
        let s = self.stat_here(cur)?;
        if s.kind != kind::LNK {
            return Err(if s.kind == kind::DIR { err::ISDIR } else { err::INVAL }.into());
        }
        let Reply::Bytes(target, _) = self.ask(cur.uid, Request::ReadLink(path))? else {
            return Err(err::SERVERFAULT.into());
        };
        let mut w = Writer::new();
        w.opaque(&target);
        Ok(w)
    }

    /// The worker's handle to read or write the current file with, under
    /// `stateid`: none to go by its path.
    fn io_handle(
        &self,
        cur: &Cur,
        stateid: &Stateid,
        file: FileKey,
        write: bool,
    ) -> Result<Option<u64>, Fail> {
        if stateid.is_special() {
            // Without an open, a writer is still kept out by another's
            // deny-write.
            if write && self.state.share_conflict(file, SHARE_WRITE, 0, None) {
                return Err(err::LOCKED.into());
            }
            return Ok(None);
        }
        let open = self.state.open_of(stateid)?;
        if open.file != file {
            return Err(err::BAD_STATEID.into());
        }
        if write && open.access & SHARE_WRITE == 0 {
            return Err(err::OPENMODE.into());
        }
        let bit = if write { SHARE_WRITE } else { SHARE_READ };
        let usable = open.uid == cur.uid && open.access & bit != 0;
        Ok(if usable { open.handle } else { None })
    }

    /// READ.
    fn read(&mut self, cur: &mut Cur, r: &mut Reader) -> Done {
        let stateid = stateid_arg(cur, r)?;
        let offset = r.u64()?;
        let count = r.u32()?;
        let file = self.key_here(cur)?;
        let (_, path, _) = Self::file_here(cur, err::ISDIR)?;
        let handle = self.io_handle(cur, &stateid, file, false)?;
        let request = Request::Read { handle, path, offset, count: count.min(IO_MAX) };
        let Reply::Bytes(data, eof) = self.ask(cur.uid, request)? else {
            return Err(err::SERVERFAULT.into());
        };
        let mut w = Writer::new();
        w.bool(eof).opaque(&data);
        Ok(w)
    }

    /// WRITE: unstable unless the client asks for it on the disk.
    fn write(&mut self, cur: &mut Cur, r: &mut Reader) -> Done {
        let stateid = stateid_arg(cur, r)?;
        let offset = r.u64()?;
        let stable = r.u32()?;
        let data = r.opaque(IO_MAX as usize)?.to_vec();
        let file = self.key_here(cur)?;
        let (share, path, _) = Self::file_here(cur, err::ISDIR)?;
        self.writable(share)?;
        let handle = self.io_handle(cur, &stateid, file, true)?;
        let sync = stable != 0;
        let Reply::Number(n) =
            self.ask(cur.uid, Request::Write { handle, path, offset, sync, data })?
        else {
            return Err(err::SERVERFAULT.into());
        };
        Self::changed(cur);
        let mut w = Writer::new();
        // FILE_SYNC4, or UNSTABLE4 for COMMIT to finish.
        w.u32(n as u32).u32(if sync { 2 } else { 0 }).fixed(&self.verifier);
        Ok(w)
    }

    /// COMMIT: what was written unstable, onto the disk, through any open
    /// of the file.
    fn commit(&mut self, cur: &mut Cur, r: &mut Reader) -> Done {
        let _offset = r.u64()?;
        let _count = r.u32()?;
        let file = self.key_here(cur)?;
        let (_, path, _) = Self::file_here(cur, err::ISDIR)?;
        let open = self.state.states.values().find_map(|kind| match kind {
            Kind::Open(o) if o.file == file && o.handle.is_some() => Some((o.uid, o.handle)),
            _ => None,
        });
        let (uid, handle) = open.unwrap_or((cur.uid, None));
        self.ask(uid, Request::Sync(handle, path))?;
        let mut w = Writer::new();
        w.fixed(&self.verifier);
        Ok(w)
    }

    /// SEEK: data runs to the end of the file, where the only hole is.
    fn seek(&mut self, cur: &mut Cur, r: &mut Reader) -> Done {
        let stateid = stateid_arg(cur, r)?;
        let offset = r.u64()?;
        let what = r.u32()?;
        let file = self.key_here(cur)?;
        self.io_handle(cur, &stateid, file, false)?;
        let s = self.stat_here(cur)?;
        if offset >= s.size {
            return Err(err::NXIO.into());
        }
        let mut w = Writer::new();
        match what {
            0 => w.bool(false).u64(offset),
            1 => w.bool(true).u64(s.size),
            _ => return Err(err::BADXDR.into()),
        };
        Ok(w)
    }

    /// OPEN: a file opened, perhaps made, by an open-owner.
    fn open(&mut self, cur: &mut Cur, r: &mut Reader) -> Done {
        let _seqid = r.u32()?;
        // The want flags above the access bits ask about delegations.
        let access = r.u32()? & 0xff;
        let deny = r.u32()?;
        let _client = r.u64()?;
        let owner = r.opaque(OWNER_MAX)?.to_vec();
        let how = match r.u32()? {
            0 => None,
            1 => Some(match r.u32()? {
                0 => How::Unchecked(attr::decode_settings(r, nfs::SETTABLE)?),
                1 => How::Guarded(attr::decode_settings(r, nfs::SETTABLE)?),
                2 => How::Exclusive(r.array()?, Settings::default()),
                3 => {
                    let verifier = r.array()?;
                    How::Exclusive(verifier, attr::decode_settings(r, nfs::EXCLUSIVE_SETTABLE)?)
                }
                _ => return Err(err::BADXDR.into()),
            }),
            _ => return Err(err::BADXDR.into()),
        };
        let name = match r.u32()? {
            // CLAIM_NULL and CLAIM_FH.
            0 => Some(r.opaque(OWNER_MAX)?.to_vec()),
            4 => None,
            // Reclaims after a restart: nfsd keeps nothing to check them
            // with, so the client opens afresh.
            1 => {
                r.u32()?;
                return Err(err::NO_GRACE.into());
            }
            3 => {
                r.opaque(OWNER_MAX)?;
                return Err(err::NO_GRACE.into());
            }
            5 => return Err(err::NO_GRACE.into()),
            // Delegations, which nfsd never gives.
            2 => {
                Stateid::read(r)?;
                r.opaque(OWNER_MAX)?;
                return Err(err::BAD_STATEID.into());
            }
            6 => {
                Stateid::read(r)?;
                return Err(err::BAD_STATEID.into());
            }
            _ => return Err(err::BADXDR.into()),
        };
        if access == 0 || access > 3 || deny > 3 {
            return Err(err::INVAL.into());
        }
        let client = cur.client.ok_or(err::OP_NOT_IN_SESSION)?;
        let root_status = if how.is_some() { err::ROFS } else { err::ISDIR };
        let (share, folder, folder_fh, path) = match &name {
            Some(name) => {
                check_name(name)?;
                let (share, folder, fh) = self.folder_here(cur, root_status)?;
                let path = join(&folder, name);
                (share, Some(folder), fh, path)
            }
            None => {
                if how.is_some() {
                    return Err(err::INVAL.into());
                }
                self.key_here(cur)?;
                let (share, path, fh) = Self::file_here(cur, root_status)?;
                (share, None, fh, path)
            }
        };
        if how.is_some() || access & SHARE_WRITE != 0 {
            self.writable(share)?;
        }
        let create = match &how {
            None => Create::No,
            Some(How::Unchecked(s)) => Create::Unchecked(attr::new_mode(s, 0).unwrap_or(0o644)),
            Some(How::Guarded(s)) => Create::Guarded(attr::new_mode(s, 0).unwrap_or(0o644)),
            Some(How::Exclusive(v, s)) => {
                Create::Exclusive(*v, attr::new_mode(s, 0).unwrap_or(0o644))
            }
        };
        let request = Request::Open {
            path: path.clone(),
            read: access & SHARE_READ != 0,
            write: access & SHARE_WRITE != 0,
            create,
        };
        let opened = self.backend.call(cur.uid, request).map_err(|errno| {
            // The worker opens regular files alone.
            Fail::Status(if errno == libc::EINVAL { err::WRONG_TYPE } else { status_of(errno) })
        })?;
        let Reply::Opened(handle, mut stat, made) = opened else {
            return Err(err::SERVERFAULT.into());
        };
        let key = self.shares[share].key;
        let file = (key, stat.dev, stat.ino);
        if let (Some(name), Some(_)) = (&name, &folder) {
            self.places.learn(key, folder_of(folder_fh), name, stat.dev, stat.ino);
        }
        let existing = self.state.open_by(client, &owner, file);
        if self.state.share_conflict(file, access, deny, existing) {
            let _ = self.backend.call(cur.uid, Request::Close(handle));
            return Err(err::SHARE_DENIED.into());
        }
        // The attributes the mode did not set: all of them for a file
        // made, the size alone for one that was there.
        let mut attrset = Attrs::default();
        if let Some(How::Unchecked(s) | How::Guarded(s) | How::Exclusive(_, s)) = &how {
            let changes = if made {
                attrset = s.given;
                Changes {
                    mode: None,
                    uid: s.uid,
                    gid: s.gid,
                    size: s.size,
                    atime: s.atime,
                    mtime: s.mtime,
                }
            } else if s.size.is_some() && matches!(how, Some(How::Unchecked(_))) {
                attrset.add(nfs::attr::SIZE);
                Changes { size: s.size, ..Changes::default() }
            } else {
                if matches!(how, Some(How::Exclusive(..))) {
                    attrset = s.given;
                }
                Changes::default()
            };
            if !changes.is_empty() {
                match self
                    .backend
                    .call(cur.uid, Request::SetAttr(path.clone(), Some(handle), changes))
                {
                    Ok(Reply::Stat(s)) => stat = s,
                    Ok(_) => {}
                    Err(errno) => {
                        let _ = self.backend.call(cur.uid, Request::Close(handle));
                        return Err(status_of(errno).into());
                    }
                }
            }
        }
        let stateid =
            self.keep_open(cur, client, owner, file, &path, handle, access, deny, existing);
        let (before, after) = match (&how, &folder) {
            (Some(_), Some(folder)) => {
                let after = self.change_now(cur.uid, folder);
                (if made { after.wrapping_sub(1) } else { after }, after)
            }
            _ => (0, 0),
        };
        let mut w = Writer::new();
        stateid.write(&mut w);
        cinfo(&mut w, before, after);
        w.u32(LOCKTYPE_POSIX).bitmap(&attrset.0);
        // OPEN_DELEGATE_NONE.
        w.u32(0);
        let fh = Fh::File(key, stat.dev, stat.ino);
        cur.here = Some(Here::File { share, path, fh, stat: Some(stat) });
        cur.stateid = Some(stateid);
        Ok(w)
    }

    /// Keeps an open: a new one, or the open-owner's `existing` one on
    /// the file widened. Its stateid.
    #[allow(clippy::too_many_arguments)]
    fn keep_open(
        &mut self,
        cur: &Cur,
        client: u64,
        owner: Vec<u8>,
        file: FileKey,
        path: &[u8],
        handle: u64,
        access: u32,
        deny: u32,
        existing: Option<[u8; 12]>,
    ) -> Stateid {
        let Some(other) = existing else {
            let other = self.state.new_other();
            let open = Open {
                client,
                owner,
                file,
                uid: cur.uid,
                handle: Some(handle),
                access,
                deny,
                seqid: 1,
            };
            self.state.states.insert(other, Kind::Open(open));
            return Stateid { seqid: 1, other };
        };
        let Some(Kind::Open(old)) = self.state.states.get(&other).cloned() else {
            return Stateid { seqid: 0, other };
        };
        let wider = old.access | access;
        // The new handle does all the open may, or one is opened that
        // does; the old one closes.
        let mut kept = Some(handle);
        if wider != access {
            let again = Request::Open {
                path: path.to_vec(),
                read: wider & SHARE_READ != 0,
                write: wider & SHARE_WRITE != 0,
                create: Create::No,
            };
            kept = match self.backend.call(cur.uid, again) {
                Ok(Reply::Opened(wide, ..)) => Some(wide),
                _ => None,
            };
            let _ = self.backend.call(cur.uid, Request::Close(handle));
        }
        if let Some(old_handle) = old.handle {
            let _ = self.backend.call(old.uid, Request::Close(old_handle));
        }
        let seqid = old.seqid.wrapping_add(1);
        let open =
            Open { uid: cur.uid, handle: kept, access: wider, deny: old.deny | deny, seqid, ..old };
        self.state.states.insert(other, Kind::Open(open));
        Stateid { seqid, other }
    }

    /// CLOSE: the open ends, and its lock-owners' locks with it.
    fn close(&mut self, cur: &mut Cur, r: &mut Reader) -> Done {
        let _seqid = r.u32()?;
        let stateid = stateid_arg(cur, r)?;
        let file = match self.state.find(&stateid)? {
            Kind::Open(open) => open.file,
            Kind::Lock(_) => return Err(err::BAD_STATEID.into()),
        };
        if self.key_here(cur)? != file {
            return Err(err::BAD_STATEID.into());
        }
        if let Some((uid, handle)) = self.state.remove(&stateid.other) {
            let _ = self.backend.call(uid, Request::Close(handle));
        }
        cur.stateid = None;
        let mut w = Writer::new();
        Stateid::INVALID.write(&mut w);
        Ok(w)
    }

    /// OPEN_DOWNGRADE: an open narrowed.
    fn open_downgrade(&mut self, cur: &mut Cur, r: &mut Reader) -> Done {
        let stateid = stateid_arg(cur, r)?;
        let _seqid = r.u32()?;
        let access = r.u32()? & 0xff;
        let deny = r.u32()?;
        let Kind::Open(_) = self.state.find(&stateid)? else {
            return Err(err::BAD_STATEID.into());
        };
        let Some(Kind::Open(open)) = self.state.states.get_mut(&stateid.other) else {
            return Err(err::BAD_STATEID.into());
        };
        if access == 0 || access & !open.access != 0 || deny & !open.deny != 0 {
            return Err(err::INVAL.into());
        }
        open.access = access;
        open.deny = deny;
        open.seqid = open.seqid.wrapping_add(1);
        let stateid = Stateid { seqid: open.seqid, other: stateid.other };
        cur.stateid = Some(stateid);
        let mut w = Writer::new();
        stateid.write(&mut w);
        Ok(w)
    }

    /// SETATTR: the attributes asked for, and which were set.
    fn setattr(&mut self, cur: &mut Cur, r: &mut Reader) -> Done {
        let stateid = stateid_arg(cur, r)?;
        let settings = attr::decode_settings(r, nfs::SETTABLE)?;
        let (share, path, fh) = Self::file_here(cur, err::ROFS)?;
        self.writable(share)?;
        let s = self.stat_here(cur)?;
        let mut handle = None;
        if settings.size.is_some() {
            let Fh::File(key, dev, ino) = fh else { return Err(err::ISDIR.into()) };
            if s.kind != kind::REG {
                return Err(if s.kind == kind::DIR { err::ISDIR } else { err::INVAL }.into());
            }
            handle = self.io_handle(cur, &stateid, (key, dev, ino), true)?;
        }
        let changes = Changes {
            mode: attr::new_mode(&settings, s.mode & 0o7777),
            uid: settings.uid,
            gid: settings.gid,
            size: settings.size,
            atime: settings.atime,
            mtime: settings.mtime,
        };
        if !changes.is_empty() {
            Self::changed(cur);
            if let Reply::Stat(new) = self.ask(cur.uid, Request::SetAttr(path, handle, changes))?
                && let Some(Here::File { stat, .. }) = cur.here.as_mut()
            {
                *stat = Some(new);
            }
        }
        let mut w = Writer::new();
        w.bitmap(&settings.given.0);
        Ok(w)
    }

    /// CREATE: a folder, link, device, FIFO or socket in the current
    /// folder, which becomes current.
    fn create(&mut self, cur: &mut Cur, r: &mut Reader) -> Done {
        let what = r.u32()?;
        let target = match what {
            kind::LNK => Some(r.opaque(4096)?.to_vec()),
            _ => None,
        };
        let device = match what {
            kind::BLK | kind::CHR => Some((r.u32()?, r.u32()?)),
            _ => None,
        };
        let name = r.opaque(OWNER_MAX)?.to_vec();
        let settings = attr::decode_settings(r, nfs::SETTABLE)?;
        check_name(&name)?;
        if settings.size.is_some() {
            return Err(err::INVAL.into());
        }
        let (share, folder, folder_fh) = self.folder_here(cur, err::ROFS)?;
        self.writable(share)?;
        let default = if what == kind::DIR { 0o755 } else { 0o644 };
        let mode = attr::new_mode(&settings, 0).unwrap_or(default);
        let make = match (what, target, device) {
            (kind::LNK, Some(target), _) => Make::Symlink(target),
            (kind::DIR, ..) => Make::Directory(mode),
            (kind::FIFO, ..) => Make::Fifo(mode),
            (kind::SOCK, ..) => Make::Socket(mode),
            (kind::BLK | kind::CHR, _, Some((major, minor))) => {
                Make::Device(what == kind::CHR, major, minor, mode)
            }
            _ => return Err(err::BADTYPE.into()),
        };
        let path = join(&folder, &name);
        let Reply::Stat(mut stat) = self.ask(cur.uid, Request::Make(path.clone(), make))? else {
            return Err(err::SERVERFAULT.into());
        };
        let changes = Changes {
            uid: settings.uid,
            gid: settings.gid,
            atime: settings.atime,
            mtime: settings.mtime,
            ..Changes::default()
        };
        if !changes.is_empty()
            && let Reply::Stat(s) =
                self.ask(cur.uid, Request::SetAttr(path.clone(), None, changes))?
        {
            stat = s;
        }
        let key = self.shares[share].key;
        self.places.learn(key, folder_of(folder_fh), &name, stat.dev, stat.ino);
        let after = self.change_now(cur.uid, &folder);
        let mut w = Writer::new();
        cinfo(&mut w, after.wrapping_sub(1), after);
        w.bitmap(&settings.given.0);
        let fh = Fh::File(key, stat.dev, stat.ino);
        cur.here = Some(Here::File { share, path, fh, stat: Some(stat) });
        cur.stateid = None;
        Ok(w)
    }

    /// REMOVE: a name in the current folder.
    fn remove(&mut self, cur: &mut Cur, r: &mut Reader) -> Done {
        let name = r.opaque(OWNER_MAX)?.to_vec();
        check_name(&name)?;
        let (share, folder, _) = self.folder_here(cur, err::ROFS)?;
        self.writable(share)?;
        self.ask(cur.uid, Request::Remove(join(&folder, &name)))?;
        Self::changed(cur);
        let after = self.change_now(cur.uid, &folder);
        let mut w = Writer::new();
        cinfo(&mut w, after.wrapping_sub(1), after);
        Ok(w)
    }

    /// RENAME: from the saved folder to the current one, in one share.
    fn rename(&mut self, cur: &mut Cur, r: &mut Reader) -> Done {
        let old = r.opaque(OWNER_MAX)?.to_vec();
        let new = r.opaque(OWNER_MAX)?.to_vec();
        check_name(&old)?;
        check_name(&new)?;
        let (share, to, to_fh) = self.folder_here(cur, err::ROFS)?;
        let Some(Here::File { share: from_share, path: from, .. }) = cur.saved.clone() else {
            return Err(if cur.saved.is_none() { err::NOFILEHANDLE } else { err::XDEV }.into());
        };
        if from_share != share {
            return Err(err::XDEV.into());
        }
        self.writable(share)?;
        let to_path = join(&to, &new);
        self.ask(cur.uid, Request::Rename(join(&from, &old), to_path.clone()))?;
        let key = self.shares[share].key;
        if let Ok(stat) = self.stat_path(cur.uid, &to_path) {
            self.places.learn(key, folder_of(to_fh), &new, stat.dev, stat.ino);
        }
        Self::changed(cur);
        if let Some(Here::File { stat, .. }) = cur.saved.as_mut() {
            *stat = None;
        }
        let from_after = self.change_now(cur.uid, &from);
        let to_after = self.change_now(cur.uid, &to);
        let mut w = Writer::new();
        cinfo(&mut w, from_after.wrapping_sub(1), from_after);
        cinfo(&mut w, to_after.wrapping_sub(1), to_after);
        Ok(w)
    }

    /// LINK: the saved file under a new name in the current folder.
    fn link(&mut self, cur: &mut Cur, r: &mut Reader) -> Done {
        let name = r.opaque(OWNER_MAX)?.to_vec();
        check_name(&name)?;
        let (share, folder, _) = self.folder_here(cur, err::ROFS)?;
        let Some(Here::File { share: from_share, path: from, .. }) = cur.saved.clone() else {
            return Err(if cur.saved.is_none() { err::NOFILEHANDLE } else { err::ISDIR }.into());
        };
        if from_share != share {
            return Err(err::XDEV.into());
        }
        self.writable(share)?;
        self.ask(cur.uid, Request::Link(from, join(&folder, &name)))?;
        Self::changed(cur);
        if let Some(Here::File { stat, .. }) = cur.saved.as_mut() {
            *stat = None;
        }
        let after = self.change_now(cur.uid, &folder);
        let mut w = Writer::new();
        cinfo(&mut w, after.wrapping_sub(1), after);
        Ok(w)
    }

    /// LOCK: a byte-range lock, for a new lock-owner through its open or
    /// for one that has a stateid.
    fn lock(&mut self, cur: &mut Cur, r: &mut Reader) -> Done {
        let kind_of = r.u32()?;
        let reclaim = r.bool()?;
        let offset = r.u64()?;
        let length = r.u64()?;
        let locker = if r.bool()? {
            let _open_seqid = r.u32()?;
            let open = stateid_arg(cur, r)?;
            let _lock_seqid = r.u32()?;
            let _client = r.u64()?;
            LockerArg::New(open, r.opaque(OWNER_MAX)?.to_vec())
        } else {
            let stateid = stateid_arg(cur, r)?;
            let _lock_seqid = r.u32()?;
            LockerArg::Existing(stateid)
        };
        if !(1..=4).contains(&kind_of) {
            return Err(err::INVAL.into());
        }
        if reclaim {
            return Err(err::NO_GRACE.into());
        }
        let end = range_end(offset, length)?;
        let write = kind_of == 2 || kind_of == 4;
        let file = self.key_here(cur)?;
        let client = cur.client.ok_or(err::OP_NOT_IN_SESSION)?;
        let (other, owner, open_other, access) = match locker {
            LockerArg::New(open_id, owner) => {
                let Kind::Open(open) = self.state.find(&open_id)? else {
                    return Err(err::BAD_STATEID.into());
                };
                if open.file != file || open.client != client {
                    return Err(err::BAD_STATEID.into());
                }
                let access = open.access;
                (self.state.locker_by(client, &owner, file), owner, open_id.other, access)
            }
            LockerArg::Existing(stateid) => {
                let Kind::Lock(locker) = self.state.find(&stateid)? else {
                    return Err(err::BAD_STATEID.into());
                };
                if locker.file != file {
                    return Err(err::BAD_STATEID.into());
                }
                let (owner, open_other) = (locker.owner.clone(), locker.open);
                let access = self.state.open_of(&stateid)?.access;
                (Some(stateid.other), owner, open_other, access)
            }
        };
        if write && access & SHARE_WRITE == 0 {
            return Err(err::OPENMODE.into());
        }
        let lock_owner = (client, owner);
        if let Some(range) = self.state.conflict(file, &lock_owner, offset, end, write) {
            return Err(denied(range));
        }
        let other = match other {
            Some(other) => other,
            None => {
                let other = self.state.new_other();
                let locker = Locker {
                    client,
                    owner: lock_owner.1.clone(),
                    open: open_other,
                    file,
                    seqid: 0,
                };
                self.state.states.insert(other, Kind::Lock(locker));
                other
            }
        };
        self.state.lock(file, lock_owner, offset, end, write);
        self.bump_locker(cur, other)
    }

    /// A locker's stateid after a change: its sequence number moved.
    fn bump_locker(&mut self, cur: &mut Cur, other: [u8; 12]) -> Done {
        let Some(Kind::Lock(locker)) = self.state.states.get_mut(&other) else {
            return Err(err::BAD_STATEID.into());
        };
        locker.seqid = locker.seqid.wrapping_add(1);
        let stateid = Stateid { seqid: locker.seqid, other };
        cur.stateid = Some(stateid);
        let mut w = Writer::new();
        stateid.write(&mut w);
        Ok(w)
    }

    /// LOCKT: whether a lock would be granted.
    fn lockt(&mut self, cur: &mut Cur, r: &mut Reader) -> Done {
        let kind_of = r.u32()?;
        let offset = r.u64()?;
        let length = r.u64()?;
        let _client = r.u64()?;
        let owner = r.opaque(OWNER_MAX)?.to_vec();
        if !(1..=4).contains(&kind_of) {
            return Err(err::INVAL.into());
        }
        let end = range_end(offset, length)?;
        let file = self.key_here(cur)?;
        let client = cur.client.ok_or(err::OP_NOT_IN_SESSION)?;
        let write = kind_of == 2 || kind_of == 4;
        if let Some(range) = self.state.conflict(file, &(client, owner), offset, end, write) {
            return Err(denied(range));
        }
        Ok(Writer::new())
    }

    /// LOCKU: a range unlocked.
    fn locku(&mut self, cur: &mut Cur, r: &mut Reader) -> Done {
        let _kind = r.u32()?;
        let _seqid = r.u32()?;
        let stateid = stateid_arg(cur, r)?;
        let offset = r.u64()?;
        let length = r.u64()?;
        let end = range_end(offset, length)?;
        let Kind::Lock(locker) = self.state.find(&stateid)? else {
            return Err(err::BAD_STATEID.into());
        };
        let (file, owner) = (locker.file, (locker.client, locker.owner.clone()));
        if self.key_here(cur)? != file {
            return Err(err::BAD_STATEID.into());
        }
        self.state.unlock(file, &owner, offset, end);
        self.bump_locker(cur, stateid.other)
    }

    /// FREE_STATEID: a locker that holds no locks goes.
    fn free_stateid(&mut self, cur: &Cur, r: &mut Reader) -> Done {
        let stateid = stateid_arg(cur, r)?;
        let (file, owner) = match self.state.find(&stateid)? {
            Kind::Lock(locker) => (locker.file, (locker.client, locker.owner.clone())),
            Kind::Open(_) => return Err(err::LOCKS_HELD.into()),
        };
        if self.state.holds(file, &owner) {
            return Err(err::LOCKS_HELD.into());
        }
        self.state.remove(&stateid.other);
        Ok(Writer::new())
    }

    /// TEST_STATEID: whether each stateid is good.
    fn test_stateid(&mut self, r: &mut Reader) -> Done {
        let n = r.count(1024)?;
        let mut w = Writer::new();
        w.u32(n as u32);
        for _ in 0..n {
            let stateid = Stateid::read(r)?;
            let status = if stateid.is_special() {
                err::BAD_STATEID
            } else {
                self.state.find(&stateid).err().unwrap_or(err::OK)
            };
            w.u32(status);
        }
        Ok(w)
    }
}
