// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! `nfsd --worker`: the files of the shares NFS reaches, as one person.
//!
//! authd starts a worker as the person an NFS call names (or as no one,
//! for root and for numbers with no account), with its socket to the
//! listener as descriptor 3, in a view that holds only the folders of the
//! shares NFS serves, each at `/.nfs-` and its share's key. It does what
//! the listener asks, on paths in them and nowhere else, and the kernel
//! decides what each may do as that person: as Linux's server, the
//! folders above a share are not walked through.

use std::collections::{HashMap, VecDeque};
use std::ffi::{CString, OsStr};
use std::fs::{self, File, OpenOptions};
use std::io;
use std::os::fd::FromRawFd;
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::os::unix::fs::{DirBuilderExt, FileExt, FileTypeExt, MetadataExt, OpenOptionsExt};
use std::os::unix::net::UnixStream;
use std::path::{Component, Path, PathBuf};
use std::process::ExitCode;

use crate::ipc::{
    self, Answer, Changes, Create, FsStat, Make, Reply, Request, SetTime, Stat, Time,
};
use crate::nfs::kind;

/// The files a worker has open, at most.
const OPEN_MAX: usize = 4096;

/// A worker's state: the folders it may reach and the files it opened.
pub struct Worker {
    /// The shares' folders, as shares.toml names them and as their real
    /// paths.
    roots: Vec<(PathBuf, PathBuf)>,
    /// The files it opened, by handle.
    files: HashMap<u64, File>,
    /// The next handle.
    next: u64,
}

/// The errno of `err`.
fn errno(err: &io::Error) -> i32 {
    err.raw_os_error().unwrap_or(libc::EIO)
}

/// A path from the listener's bytes.
fn path_of(bytes: &[u8]) -> &Path {
    Path::new(OsStr::from_bytes(bytes))
}

/// A time from seconds and nanoseconds as std gives them.
fn time_of(seconds: i64, nanos: i64) -> Time {
    Time { seconds, nanos: u32::try_from(nanos).unwrap_or(0) }
}

/// What NFS says of a file of `meta`.
pub fn stat_of(meta: &fs::Metadata) -> Stat {
    let t = meta.file_type();
    let kind = if t.is_dir() {
        kind::DIR
    } else if t.is_symlink() {
        kind::LNK
    } else if t.is_block_device() {
        kind::BLK
    } else if t.is_char_device() {
        kind::CHR
    } else if t.is_fifo() {
        kind::FIFO
    } else if t.is_socket() {
        kind::SOCK
    } else {
        kind::REG
    };
    let rdev = meta.rdev();
    let major = ((rdev >> 8) & 0xfff) | ((rdev >> 32) & !0xfff);
    let minor = (rdev & 0xff) | ((rdev >> 12) & !0xff);
    Stat {
        kind,
        mode: meta.mode() & 0o7777,
        nlink: u32::try_from(meta.nlink()).unwrap_or(u32::MAX),
        uid: meta.uid(),
        gid: meta.gid(),
        size: meta.size(),
        used: meta.blocks().saturating_mul(512),
        rdev: (major as u32, minor as u32),
        dev: meta.dev(),
        ino: meta.ino(),
        atime: time_of(meta.atime(), meta.atime_nsec()),
        mtime: time_of(meta.mtime(), meta.mtime_nsec()),
        ctime: time_of(meta.ctime(), meta.ctime_nsec()),
    }
}

/// `path` as a C string.
fn c_path(path: &Path) -> io::Result<CString> {
    CString::new(path.as_os_str().as_bytes())
        .map_err(|_| io::Error::from_raw_os_error(libc::EINVAL))
}

/// A libc call's result as an io::Result.
fn checked(result: libc::c_int) -> io::Result<()> {
    if result == 0 { Ok(()) } else { Err(io::Error::last_os_error()) }
}

/// The timespec that sets a time as `set` says.
fn timespec(set: SetTime) -> libc::timespec {
    match set {
        SetTime::Keep => libc::timespec { tv_sec: 0, tv_nsec: libc::UTIME_OMIT as _ },
        SetTime::Now => libc::timespec { tv_sec: 0, tv_nsec: libc::UTIME_NOW as _ },
        SetTime::At(t) => libc::timespec { tv_sec: t.seconds as _, tv_nsec: t.nanos as _ },
    }
}

/// Sets `path`'s times, not through a symbolic link.
fn set_times(path: &Path, atime: SetTime, mtime: SetTime) -> io::Result<()> {
    let c = c_path(path)?;
    let times = [timespec(atime), timespec(mtime)];
    // SAFETY: the path is a C string, and `times` the two timespecs.
    checked(unsafe {
        libc::utimensat(libc::AT_FDCWD, c.as_ptr(), times.as_ptr(), libc::AT_SYMLINK_NOFOLLOW)
    })
}

/// The times an exclusive create keeps its verifier in, as Linux's nfsd
/// keeps it: its first half as the access time's seconds, its second as
/// the modification time's.
fn verifier_times(verifier: [u8; 8]) -> (Time, Time) {
    let half = |b: &[u8]| i64::from(u32::from_be_bytes([b[0], b[1], b[2], b[3]]));
    (
        Time { seconds: half(&verifier[..4]), nanos: 0 },
        Time { seconds: half(&verifier[4..]), nanos: 0 },
    )
}

impl Worker {
    /// A worker that reaches the folders `roots`.
    pub fn new(roots: &[PathBuf]) -> Worker {
        Worker { roots: real_roots(roots), files: HashMap::new(), next: 1 }
    }

    /// The path the listener named, if it is in a share: absolute, with
    /// no `.` or `..`, and its folder, followed through any links, in the
    /// folder of the share it names, so that no link leads from one share
    /// into another, which may be read-only or kept from the client's
    /// network. Its last part is not followed.
    fn place(&self, bytes: &[u8]) -> io::Result<PathBuf> {
        let refused = || io::Error::from_raw_os_error(libc::EACCES);
        let path = path_of(bytes);
        // A share's folder itself, as shares.toml names it.
        if let Some((_, real)) = self.roots.iter().find(|(named, _)| named == path) {
            return Ok(real.clone());
        }
        let mut parts = path.components();
        if parts.next() != Some(Component::RootDir)
            || !parts.all(|c| matches!(c, Component::Normal(_)))
        {
            return Err(refused());
        }
        let real = match (path.parent(), path.file_name()) {
            (Some(parent), Some(name)) => fs::canonicalize(parent)?.join(name),
            _ => return Err(refused()),
        };
        let inside =
            |(named, r): &(PathBuf, PathBuf)| path.starts_with(named) && real.starts_with(r);
        if self.roots.iter().any(inside) { Ok(real) } else { Err(refused()) }
    }

    /// The open file `handle`.
    fn file(&self, handle: u64) -> io::Result<&File> {
        self.files.get(&handle).ok_or_else(|| io::Error::from_raw_os_error(libc::EBADF))
    }

    /// Answers `request`.
    pub fn serve(&mut self, request: Request) -> Answer {
        self.act(request).map_err(|err| errno(&err))
    }

    fn act(&mut self, request: Request) -> io::Result<Reply> {
        match request {
            Request::Stat(p) => Ok(Reply::Stat(stat_of(&fs::symlink_metadata(self.place(&p)?)?))),
            Request::FsStat(p) => Ok(Reply::FsStat(fs_stat(&self.place(&p)?)?)),
            Request::Access(p, want) => {
                let path = c_path(&self.place(&p)?)?;
                let mut granted = 0;
                for bit in [4, 2, 1] {
                    let mode = match bit {
                        4 => libc::R_OK,
                        2 => libc::W_OK,
                        _ => libc::X_OK,
                    };
                    // SAFETY: the path is a C string.
                    let ok = unsafe {
                        libc::faccessat(libc::AT_FDCWD, path.as_ptr(), mode, libc::AT_EACCESS)
                    };
                    if want & bit != 0 && ok == 0 {
                        granted |= bit;
                    }
                }
                Ok(Reply::Number(u64::from(granted)))
            }
            Request::List(p, start, count) => Self::list(&self.place(&p)?, start, count),
            Request::ReadLink(p) => {
                Ok(Reply::Bytes(fs::read_link(self.place(&p)?)?.into_os_string().into_vec(), true))
            }
            Request::Open { path, read, write, create } => {
                self.open(&self.place(&path)?, read, write, create)
            }
            Request::Close(handle) => {
                self.files.remove(&handle);
                Ok(Reply::Done)
            }
            Request::Read { handle, path, offset, count } => {
                let opened;
                let file = match handle {
                    Some(h) => self.file(h)?,
                    None => {
                        opened = regular(OpenOptions::new().read(true), &self.place(&path)?)?;
                        &opened
                    }
                };
                read(file, offset, count)
            }
            Request::Write { handle, path, offset, sync, data } => {
                let opened;
                let file = match handle {
                    Some(h) => self.file(h)?,
                    None => {
                        opened = regular(OpenOptions::new().write(true), &self.place(&path)?)?;
                        &opened
                    }
                };
                file.write_all_at(&data, offset)?;
                if sync {
                    file.sync_all()?;
                }
                Ok(Reply::Number(data.len() as u64))
            }
            Request::Sync(handle, path) => {
                match handle {
                    Some(h) => self.file(h)?.sync_all()?,
                    None => {
                        // A file the person may only write is opened to write.
                        let path = self.place(&path)?;
                        regular(OpenOptions::new().read(true), &path)
                            .or_else(|_| regular(OpenOptions::new().write(true), &path))?
                            .sync_all()?;
                    }
                }
                Ok(Reply::Done)
            }
            Request::SetAttr(p, handle, changes) => {
                self.set_attr(&self.place(&p)?, handle, &changes)
            }
            Request::Make(p, make) => {
                let path = self.place(&p)?;
                make_node(&path, &make)?;
                Ok(Reply::Stat(stat_of(&fs::symlink_metadata(&path)?)))
            }
            Request::Remove(p) => {
                let path = self.place(&p)?;
                if fs::symlink_metadata(&path)?.is_dir() {
                    fs::remove_dir(&path)?;
                } else {
                    fs::remove_file(&path)?;
                }
                Ok(Reply::Done)
            }
            Request::Rename(from, to) => {
                fs::rename(self.place(&from)?, self.place(&to)?)?;
                Ok(Reply::Done)
            }
            Request::Link(from, to) => {
                fs::hard_link(self.place(&from)?, self.place(&to)?)?;
                Ok(Reply::Done)
            }
            Request::Find(root, dev, ino, max) => {
                Ok(Reply::Found(Self::find(&self.place(&root)?, dev, ino, max)))
            }
        }
    }

    /// The entries of `dir` from the `start`th, sorted by name.
    fn list(dir: &Path, start: u64, count: u32) -> io::Result<Reply> {
        let mut names: Vec<_> =
            fs::read_dir(dir)?.filter_map(|e| e.ok().map(|e| e.file_name())).collect();
        names.sort();
        let total = names.len() as u64;
        let mut out = Vec::new();
        let mut at = start;
        for name in names.into_iter().skip(usize::try_from(start).unwrap_or(usize::MAX)) {
            if out.len() >= count as usize {
                break;
            }
            at += 1;
            // One removed since it was listed is left out.
            if let Ok(meta) = fs::symlink_metadata(dir.join(&name)) {
                out.push((name.into_vec(), stat_of(&meta)));
            }
        }
        Ok(Reply::List(out, at >= total))
    }

    /// Opens `path`, making it as `create` says.
    fn open(&mut self, path: &Path, read: bool, write: bool, create: Create) -> io::Result<Reply> {
        if self.files.len() >= OPEN_MAX {
            return Err(io::Error::from_raw_os_error(libc::EMFILE));
        }
        let mut options = OpenOptions::new();
        // Opening to change nothing still needs a mode: read. A FIFO
        // opened by mistake does not wait for a writer.
        options.read(read || !write).write(write).custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
        let mut made = false;
        let file = match create {
            Create::No => options.open(path)?,
            Create::Unchecked(mode) => {
                let existed = fs::symlink_metadata(path).is_ok();
                let file = options.clone().create(true).mode(mode).open(path)?;
                made = !existed;
                file
            }
            Create::Guarded(mode) => {
                made = true;
                options.create_new(true).mode(mode).open(path)?
            }
            Create::Exclusive(verifier, mode) => {
                let (atime, mtime) = verifier_times(verifier);
                match options.clone().create_new(true).mode(mode).open(path) {
                    Ok(file) => {
                        set_times(path, SetTime::At(atime), SetTime::At(mtime))?;
                        made = true;
                        file
                    }
                    Err(err) if err.kind() == io::ErrorKind::AlreadyExists => {
                        // The same create again finds its own file.
                        let s = stat_of(&fs::symlink_metadata(path)?);
                        if s.atime.seconds != atime.seconds || s.mtime.seconds != mtime.seconds {
                            return Err(err);
                        }
                        options.open(path)?
                    }
                    Err(err) => return Err(err),
                }
            }
        };
        let stat = stat_of(&file.metadata()?);
        if stat.kind != kind::REG {
            return Err(not_regular(stat.kind));
        }
        let handle = self.next;
        self.next += 1;
        self.files.insert(handle, file);
        Ok(Reply::Opened(handle, stat, made))
    }

    /// Changes what `changes` names of `path`.
    fn set_attr(&self, path: &Path, handle: Option<u64>, changes: &Changes) -> io::Result<Reply> {
        if let Some(size) = changes.size {
            match handle {
                Some(h) => self.file(h)?.set_len(size)?,
                None => OpenOptions::new()
                    .write(true)
                    .custom_flags(libc::O_NOFOLLOW)
                    .open(path)?
                    .set_len(size)?,
            }
        }
        if changes.uid.is_some() || changes.gid.is_some() {
            std::os::unix::fs::lchown(path, changes.uid, changes.gid)?;
        }
        if let Some(mode) = changes.mode {
            if fs::symlink_metadata(path)?.file_type().is_symlink() {
                return Err(io::Error::from_raw_os_error(libc::EOPNOTSUPP));
            }
            let c = c_path(path)?;
            // SAFETY: the path is a C string.
            checked(unsafe { libc::chmod(c.as_ptr(), (mode & 0o7777) as libc::mode_t) })?;
        }
        if changes.atime != SetTime::Keep || changes.mtime != SetTime::Keep {
            set_times(path, changes.atime, changes.mtime)?;
        }
        Ok(Reply::Stat(stat_of(&fs::symlink_metadata(path)?)))
    }

    /// The path of the file numbered `ino` on `dev` below `root`, looked
    /// for through at most `max` entries, breadth first.
    fn find(root: &Path, dev: u64, ino: u64, max: u32) -> Option<Vec<u8>> {
        let mut queue = VecDeque::from([root.to_path_buf()]);
        let mut seen = 0u32;
        while let Some(dir) = queue.pop_front() {
            // A folder the worker may not read is passed over.
            let Ok(entries) = fs::read_dir(&dir) else { continue };
            for entry in entries.flatten() {
                seen += 1;
                if seen > max {
                    return None;
                }
                let Ok(meta) = entry.metadata() else { continue };
                let path = entry.path();
                if meta.dev() == dev && meta.ino() == ino {
                    let below = path.strip_prefix(root).ok()?;
                    return Some(below.as_os_str().as_bytes().to_vec());
                }
                if meta.is_dir() && meta.dev() == dev {
                    queue.push_back(path);
                }
            }
        }
        None
    }
}

/// Each of `roots` with its real path, those that are not there left out.
fn real_roots(roots: &[PathBuf]) -> Vec<(PathBuf, PathBuf)> {
    roots.iter().filter_map(|r| Some((r.clone(), fs::canonicalize(r).ok()?))).collect()
}

/// Reads at most `count` bytes of `file` from `offset`.
fn read(file: &File, offset: u64, count: u32) -> io::Result<Reply> {
    let mut data = vec![0; count.min(crate::nfs::IO_MAX) as usize];
    let mut got = 0;
    while got < data.len() {
        match file.read_at(&mut data[got..], offset + got as u64) {
            Ok(0) => break,
            Ok(n) => got += n,
            Err(err) if err.kind() == io::ErrorKind::Interrupted => {}
            Err(err) => return Err(err),
        }
    }
    data.truncate(got);
    let size = file.metadata()?.len();
    Ok(Reply::Bytes(data, offset + got as u64 >= size))
}

/// Makes the node `make` at `path`.
fn make_node(path: &Path, make: &Make) -> io::Result<()> {
    let node = |mode: libc::mode_t, dev: libc::dev_t| {
        let c = c_path(path)?;
        // SAFETY: the path is a C string.
        checked(unsafe { libc::mknod(c.as_ptr(), mode, dev) })
    };
    match make {
        Make::Directory(mode) => fs::DirBuilder::new().mode(*mode & 0o7777).create(path),
        Make::Symlink(target) => std::os::unix::fs::symlink(path_of(target), path),
        Make::Fifo(mode) => {
            let c = c_path(path)?;
            // SAFETY: the path is a C string.
            checked(unsafe { libc::mkfifo(c.as_ptr(), (*mode & 0o7777) as libc::mode_t) })
        }
        Make::Socket(mode) => node(libc::S_IFSOCK | (*mode & 0o7777) as libc::mode_t, 0),
        Make::Device(chr, major, minor, mode) => {
            let kind = if *chr { libc::S_IFCHR } else { libc::S_IFBLK };
            let (major, minor) = (u64::from(*major), u64::from(*minor));
            let dev = ((major & 0xfff) << 8)
                | ((major & !0xfff) << 32)
                | (minor & 0xff)
                | ((minor & !0xff) << 12);
            node(kind | (*mode & 0o7777) as libc::mode_t, dev as libc::dev_t)
        }
    }
}

/// What the filesystem holding `path` has room for.
fn fs_stat(path: &Path) -> io::Result<FsStat> {
    let c = c_path(path)?;
    // SAFETY: all zeros is a valid statvfs: numbers.
    let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
    // SAFETY: the path is a C string and `st` a statvfs for the call.
    checked(unsafe { libc::statvfs(c.as_ptr(), &raw mut st) })?;
    let unit = if st.f_frsize > 0 { st.f_frsize as u64 } else { st.f_bsize as u64 };
    Ok(FsStat {
        files_avail: st.f_favail as u64,
        files_free: st.f_ffree as u64,
        files_total: st.f_files as u64,
        space_avail: (st.f_bavail as u64).saturating_mul(unit),
        space_free: (st.f_bfree as u64).saturating_mul(unit),
        space_total: (st.f_blocks as u64).saturating_mul(unit),
    })
}

/// Opens `path` with `options`, if it is a regular file: a FIFO does
/// not wait for its other end, and a link is not followed.
fn regular(options: &mut OpenOptions, path: &Path) -> io::Result<File> {
    let file = options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK).open(path)?;
    let kind = stat_of(&file.metadata()?).kind;
    if kind == kind::REG { Ok(file) } else { Err(not_regular(kind)) }
}

/// The error for reading or writing a file of `kind`, which is not
/// regular: EISDIR for a directory, EINVAL for the rest.
fn not_regular(kind: u32) -> io::Error {
    io::Error::from_raw_os_error(if kind == kind::DIR { libc::EISDIR } else { libc::EINVAL })
}

/// The prefix of the names authd attaches the shares' folders at in a
/// worker's view: `/.nfs-` and the share's key, in hexadecimal.
pub const ATTACHED: &str = "/.nfs-";

/// The shares' folders in the worker's view, as authd attached them: the
/// worker reaches nothing else. The listener starts its workers again
/// when the shares change.
fn attached() -> Vec<PathBuf> {
    let prefix = &ATTACHED[1..];
    let Ok(entries) = fs::read_dir("/") else { return Vec::new() };
    entries
        .flatten()
        .filter(|e| e.file_name().as_bytes().starts_with(prefix.as_bytes()))
        .map(|e| e.path())
        .collect()
}

/// Runs as `nfsd --worker`: serves the listener on descriptor 3 until it
/// closes.
pub fn run() -> ExitCode {
    // The modes NFS asks for are what files get: the client applied its
    // own mask already.
    // SAFETY: umask only sets the mask.
    unsafe { libc::umask(0) };
    // SAFETY: authd gives the worker the listener's socket as descriptor
    // 3, and nothing else owns it.
    let mut socket = unsafe { UnixStream::from_raw_fd(3) };
    let mut worker = Worker::new(&attached());
    loop {
        let message = match ipc::receive(&mut socket) {
            Ok(Some(message)) => message,
            Ok(None) => return ExitCode::SUCCESS,
            Err(err) => {
                eprintln!("nfsd worker: the listener's socket: {err}");
                return ExitCode::FAILURE;
            }
        };
        let answer = match Request::decode(&message) {
            Ok(request) => worker.serve(request),
            Err(_) => Err(libc::EINVAL),
        };
        if let Err(err) = ipc::send(&mut socket, &ipc::encode_answer(&answer)) {
            eprintln!("nfsd worker: the listener's socket: {err}");
            return ExitCode::FAILURE;
        }
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;

    fn bytes(path: &Path) -> Vec<u8> {
        path.as_os_str().as_bytes().to_vec()
    }

    #[test]
    fn paths_outside_the_shares_are_refused() {
        let dir = tempdir("place");
        let share = dir.join("share");
        fs::create_dir_all(share.join("sub")).unwrap();
        fs::create_dir_all(dir.join("secret")).unwrap();
        std::os::unix::fs::symlink(dir.join("secret"), share.join("out")).unwrap();
        let mut w = Worker::new(std::slice::from_ref(&share));
        assert!(w.serve(Request::Stat(bytes(&share.join("sub")))).is_ok());
        assert!(w.serve(Request::Stat(bytes(&share))).is_ok());
        // The link itself is in the share; what is through it is not.
        assert!(w.serve(Request::Stat(bytes(&share.join("out")))).is_ok());
        assert_eq!(w.serve(Request::Stat(bytes(&share.join("out/x")))), Err(libc::EACCES));
        assert_eq!(w.serve(Request::Stat(bytes(&dir.join("secret")))), Err(libc::EACCES));
        assert_eq!(w.serve(Request::Stat(b"share/sub".to_vec())), Err(libc::EACCES));
        let dotdot = [bytes(&share), b"/sub/../../secret".to_vec()].concat();
        assert_eq!(w.serve(Request::Stat(dotdot)), Err(libc::EACCES));
    }

    #[test]
    fn links_lead_from_no_share_into_another() {
        let dir = tempdir("between");
        let (open, kept) = (dir.join("open"), dir.join("kept"));
        fs::create_dir_all(&open).unwrap();
        fs::create_dir_all(kept.join("in")).unwrap();
        fs::write(kept.join("in/file"), b"kept").unwrap();
        std::os::unix::fs::symlink(&kept, open.join("to-kept")).unwrap();
        let mut w = Worker::new(&[open.clone(), kept.clone()]);
        // Each share reaches its own, and not the other's through a link.
        assert!(w.serve(Request::Stat(bytes(&kept.join("in/file")))).is_ok());
        assert!(w.serve(Request::Stat(bytes(&open.join("to-kept")))).is_ok());
        let through = bytes(&open.join("to-kept/in/file"));
        assert_eq!(w.serve(Request::Stat(through.clone())), Err(libc::EACCES));
        let write = Request::Write {
            handle: None,
            path: through,
            offset: 0,
            sync: false,
            data: b"x".to_vec(),
        };
        assert_eq!(w.serve(write), Err(libc::EACCES));
        assert_eq!(fs::read(kept.join("in/file")).unwrap(), b"kept");
    }

    #[test]
    fn files_are_made_written_read_and_listed() {
        let dir = tempdir("files");
        let mut w = Worker::new(std::slice::from_ref(&dir));
        let file = bytes(&dir.join("b.txt"));
        let Ok(Reply::Opened(h, _, true)) = w.serve(Request::Open {
            path: file.clone(),
            read: true,
            write: true,
            create: Create::Guarded(0o640),
        }) else {
            panic!("not made");
        };
        let wrote = w.serve(Request::Write {
            handle: Some(h),
            path: file.clone(),
            offset: 3,
            sync: false,
            data: b"xyz".to_vec(),
        });
        assert_eq!(wrote, Ok(Reply::Number(3)));
        let read =
            w.serve(Request::Read { handle: None, path: file.clone(), offset: 0, count: 100 });
        assert_eq!(read, Ok(Reply::Bytes(b"\0\0\0xyz".to_vec(), true)));
        assert_eq!(w.serve(Request::Close(h)), Ok(Reply::Done));
        w.serve(Request::Make(bytes(&dir.join("a")), Make::Directory(0o755))).unwrap();
        let Ok(Reply::List(entries, true)) = w.serve(Request::List(bytes(&dir), 0, 10)) else {
            panic!()
        };
        let names: Vec<_> = entries.iter().map(|(n, _)| n.as_slice()).collect();
        assert_eq!(names, [&b"a"[..], b"b.txt"]);
        assert_eq!(entries[1].1.mode, 0o640);
        let Ok(Reply::List(entries, true)) = w.serve(Request::List(bytes(&dir), 1, 10)) else {
            panic!()
        };
        assert_eq!(entries.len(), 1);
        // An exclusive create sent again finds its file; another's fails.
        let excl = || Request::Open {
            path: bytes(&dir.join("e")),
            read: false,
            write: true,
            create: Create::Exclusive([1, 2, 3, 4, 5, 6, 7, 8], 0o600),
        };
        assert!(matches!(w.serve(excl()), Ok(Reply::Opened(_, _, true))));
        assert!(matches!(w.serve(excl()), Ok(Reply::Opened(_, _, false))));
        let other = Request::Open {
            path: bytes(&dir.join("e")),
            read: false,
            write: true,
            create: Create::Exclusive([9; 8], 0o600),
        };
        assert_eq!(w.serve(other), Err(libc::EEXIST));
        let found = w.serve(Request::Find(bytes(&dir), entries[0].1.dev, entries[0].1.ino, 100));
        assert_eq!(found, Ok(Reply::Found(Some(b"b.txt".to_vec()))));
    }

    /// A fresh directory for a test.
    pub fn tempdir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("nfsd-test-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::canonicalize(dir).unwrap()
    }
}
