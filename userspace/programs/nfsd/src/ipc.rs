// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! What the listener asks a worker, and its answers, in XDR on the
//! pipes between them: each a record of a u32 length and its bytes.
//!
//! A worker acts on paths the listener names, as the person it runs as;
//! the kernel decides what each may do. The listener keeps NFS's state;
//! the worker keeps only the files it opened, by number.

use std::io::{self, Read, Write};

use crate::xdr::{self, Reader, Writer};

/// The longest message, either way: a READ's or a WRITE's megabyte and
/// a little.
pub const MESSAGE_MAX: usize = (1 << 20) + 64 * 1024;

/// A time: seconds since 1970 and nanoseconds.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Time {
    /// The seconds.
    pub seconds: i64,
    /// The nanoseconds, below a billion.
    pub nanos: u32,
}

/// What a file is, as lstat(2) says.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Stat {
    /// Its kind, as NFS numbers them (nfs::kind).
    pub kind: u32,
    /// Its permission bits, set-user-ID, set-group-ID and sticky with
    /// them.
    pub mode: u32,
    /// Its links.
    pub nlink: u32,
    /// Its owner.
    pub uid: u32,
    /// Its group.
    pub gid: u32,
    /// Its size in bytes.
    pub size: u64,
    /// The bytes it takes on the disk.
    pub used: u64,
    /// A device's major and minor numbers.
    pub rdev: (u32, u32),
    /// The filesystem it is on.
    pub dev: u64,
    /// Its number there.
    pub ino: u64,
    /// When it was last read.
    pub atime: Time,
    /// When its data last changed.
    pub mtime: Time,
    /// When anything of it last changed.
    pub ctime: Time,
}

/// What the filesystem holding a file has room for.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FsStat {
    /// Files that may still be made, by anyone but root.
    pub files_avail: u64,
    /// Files that may still be made.
    pub files_free: u64,
    /// Files it holds at most.
    pub files_total: u64,
    /// Bytes free, for anyone but root.
    pub space_avail: u64,
    /// Bytes free.
    pub space_free: u64,
    /// Its size in bytes.
    pub space_total: u64,
}

/// How a time is set.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SetTime {
    /// It stays.
    #[default]
    Keep,
    /// To the server's time now.
    Now,
    /// To this.
    At(Time),
}

/// What a SETATTR changes.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Changes {
    /// The permission bits.
    pub mode: Option<u32>,
    /// The owner.
    pub uid: Option<u32>,
    /// The group.
    pub gid: Option<u32>,
    /// The size.
    pub size: Option<u64>,
    /// When it was read.
    pub atime: SetTime,
    /// When it was written.
    pub mtime: SetTime,
}

impl Changes {
    /// Whether it changes nothing.
    pub fn is_empty(&self) -> bool {
        *self == Changes::default()
    }
}

/// How OPEN makes a file it does not find.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Create {
    /// It does not: the file must be there.
    No,
    /// With these permission bits, or opens the one there.
    Unchecked(u32),
    /// With these, and not if one is there.
    Guarded(u32),
    /// Exclusively, with this verifier kept in its times so that the same
    /// create sent again finds it, and these bits.
    Exclusive([u8; 8], u32),
}

/// What CREATE makes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Make {
    /// A directory, with these permission bits.
    Directory(u32),
    /// A symbolic link to this.
    Symlink(Vec<u8>),
    /// A named pipe.
    Fifo(u32),
    /// A socket's name.
    Socket(u32),
    /// A device: whether a character one, its major and minor numbers,
    /// and its bits.
    Device(bool, u32, u32, u32),
}

/// A request to a worker. Paths are absolute, in bytes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Request {
    /// lstat(2).
    Stat(Vec<u8>),
    /// statvfs(3).
    FsStat(Vec<u8>),
    /// Which of read (4), write (2) and execute (1) the worker may do.
    Access(Vec<u8>, u32),
    /// A directory's entries, sorted by name, from the `start`th, at most
    /// `count`, without `.` and `..`.
    List(Vec<u8>, u64, u32),
    /// What a symbolic link holds.
    ReadLink(Vec<u8>),
    /// Opens a file to read, write or both, perhaps making it.
    Open {
        /// The file.
        path: Vec<u8>,
        /// Whether to read it.
        read: bool,
        /// Whether to write it.
        write: bool,
        /// How to make it.
        create: Create,
    },
    /// Closes an open file.
    Close(u64),
    /// Reads from an open file, or from the path if `handle` is none.
    Read {
        /// The open file.
        handle: Option<u64>,
        /// The file's path.
        path: Vec<u8>,
        /// Where from.
        offset: u64,
        /// At most how much.
        count: u32,
    },
    /// Writes to an open file, or to the path if `handle` is none.
    Write {
        /// The open file.
        handle: Option<u64>,
        /// The file's path.
        path: Vec<u8>,
        /// Where to.
        offset: u64,
        /// Whether it is on the disk before the answer.
        sync: bool,
        /// What.
        data: Vec<u8>,
    },
    /// Puts a file's data on the disk.
    Sync(Option<u64>, Vec<u8>),
    /// Changes what is said of a file.
    SetAttr(Vec<u8>, Option<u64>, Changes),
    /// Makes a file that is not a regular one.
    Make(Vec<u8>, Make),
    /// Removes a file, or an empty directory.
    Remove(Vec<u8>),
    /// Renames a file.
    Rename(Vec<u8>, Vec<u8>),
    /// Links a file under another name.
    Link(Vec<u8>, Vec<u8>),
    /// Looks below a folder for the file numbered `ino` on filesystem
    /// `dev`, through at most `max` entries: its path.
    Find(Vec<u8>, u64, u64, u32),
}

/// What a worker answers: an errno, or what was asked for.
pub type Answer = Result<Reply, i32>;

/// What a worker answers a request with.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Reply {
    /// Done.
    Done,
    /// A file's stat.
    Stat(Stat),
    /// The filesystem's.
    FsStat(FsStat),
    /// The rights granted, or a count written.
    Number(u64),
    /// Entries, and whether they were the last.
    List(Vec<(Vec<u8>, Stat)>, bool),
    /// Bytes: a link's target, or data read and whether at its end.
    Bytes(Vec<u8>, bool),
    /// An open file's handle, its stat, and whether OPEN made it.
    Opened(u64, Stat, bool),
    /// A path found, if any.
    Found(Option<Vec<u8>>),
}

fn time(w: &mut Writer, t: Time) {
    w.i64(t.seconds).u32(t.nanos);
}

fn read_time(r: &mut Reader) -> xdr::Result<Time> {
    Ok(Time { seconds: r.i64()?, nanos: r.u32()? })
}

/// Writes `stat`.
pub fn write_stat(w: &mut Writer, s: &Stat) {
    w.u32(s.kind).u32(s.mode).u32(s.nlink).u32(s.uid).u32(s.gid);
    w.u64(s.size).u64(s.used).u32(s.rdev.0).u32(s.rdev.1).u64(s.dev).u64(s.ino);
    time(w, s.atime);
    time(w, s.mtime);
    time(w, s.ctime);
}

/// Reads a stat.
pub fn read_stat(r: &mut Reader) -> xdr::Result<Stat> {
    Ok(Stat {
        kind: r.u32()?,
        mode: r.u32()?,
        nlink: r.u32()?,
        uid: r.u32()?,
        gid: r.u32()?,
        size: r.u64()?,
        used: r.u64()?,
        rdev: (r.u32()?, r.u32()?),
        dev: r.u64()?,
        ino: r.u64()?,
        atime: read_time(r)?,
        mtime: read_time(r)?,
        ctime: read_time(r)?,
    })
}

fn option(w: &mut Writer, value: Option<u64>) {
    match value {
        Some(v) => w.bool(true).u64(v),
        None => w.bool(false),
    };
}

fn read_option(r: &mut Reader) -> xdr::Result<Option<u64>> {
    Ok(if r.bool()? { Some(r.u64()?) } else { None })
}

fn set_time(w: &mut Writer, t: SetTime) {
    match t {
        SetTime::Keep => w.u32(0),
        SetTime::Now => w.u32(1),
        SetTime::At(at) => {
            w.u32(2);
            time(w, at);
            w
        }
    };
}

fn read_set_time(r: &mut Reader) -> xdr::Result<SetTime> {
    Ok(match r.u32()? {
        0 => SetTime::Keep,
        1 => SetTime::Now,
        2 => SetTime::At(read_time(r)?),
        _ => return Err(xdr::Bad),
    })
}

/// The longest path.
const PATH_MAX: usize = 4096;

impl Request {
    /// Its bytes.
    pub fn encode(&self) -> Vec<u8> {
        let mut w = Writer::new();
        match self {
            Request::Stat(p) => w.u32(0).opaque(p),
            Request::FsStat(p) => w.u32(1).opaque(p),
            Request::Access(p, want) => w.u32(2).opaque(p).u32(*want),
            Request::List(p, start, count) => w.u32(3).opaque(p).u64(*start).u32(*count),
            Request::ReadLink(p) => w.u32(4).opaque(p),
            Request::Open { path, read, write, create } => {
                w.u32(5).opaque(path).bool(*read).bool(*write);
                match create {
                    Create::No => w.u32(0),
                    Create::Unchecked(mode) => w.u32(1).u32(*mode),
                    Create::Guarded(mode) => w.u32(2).u32(*mode),
                    Create::Exclusive(verifier, mode) => w.u32(3).fixed(verifier).u32(*mode),
                }
            }
            Request::Close(handle) => w.u32(6).u64(*handle),
            Request::Read { handle, path, offset, count } => {
                w.u32(7);
                option(&mut w, *handle);
                w.opaque(path).u64(*offset).u32(*count)
            }
            Request::Write { handle, path, offset, sync, data } => {
                w.u32(8);
                option(&mut w, *handle);
                w.opaque(path).u64(*offset).bool(*sync).opaque(data)
            }
            Request::Sync(handle, path) => {
                w.u32(9);
                option(&mut w, *handle);
                w.opaque(path)
            }
            Request::SetAttr(path, handle, c) => {
                w.u32(10).opaque(path);
                option(&mut w, *handle);
                option(&mut w, c.mode.map(u64::from));
                option(&mut w, c.uid.map(u64::from));
                option(&mut w, c.gid.map(u64::from));
                option(&mut w, c.size);
                set_time(&mut w, c.atime);
                set_time(&mut w, c.mtime);
                &mut w
            }
            Request::Make(path, make) => {
                w.u32(11).opaque(path);
                match make {
                    Make::Directory(mode) => w.u32(0).u32(*mode),
                    Make::Symlink(target) => w.u32(1).opaque(target),
                    Make::Fifo(mode) => w.u32(2).u32(*mode),
                    Make::Socket(mode) => w.u32(3).u32(*mode),
                    Make::Device(chr, major, minor, mode) => {
                        w.u32(4).bool(*chr).u32(*major).u32(*minor).u32(*mode)
                    }
                }
            }
            Request::Remove(p) => w.u32(12).opaque(p),
            Request::Rename(a, b) => w.u32(13).opaque(a).opaque(b),
            Request::Link(a, b) => w.u32(14).opaque(a).opaque(b),
            Request::Find(root, dev, ino, max) => {
                w.u32(15).opaque(root).u64(*dev).u64(*ino).u32(*max)
            }
        };
        w.bytes
    }

    /// The request in `bytes`.
    ///
    /// # Errors
    ///
    /// Bytes that are no request.
    pub fn decode(bytes: &[u8]) -> xdr::Result<Request> {
        let mut r = Reader::new(bytes);
        let path = |r: &mut Reader| r.opaque(PATH_MAX).map(<[u8]>::to_vec);
        let request = match r.u32()? {
            0 => Request::Stat(path(&mut r)?),
            1 => Request::FsStat(path(&mut r)?),
            2 => Request::Access(path(&mut r)?, r.u32()?),
            3 => Request::List(path(&mut r)?, r.u64()?, r.u32()?),
            4 => Request::ReadLink(path(&mut r)?),
            5 => {
                let (path, read, write) = (path(&mut r)?, r.bool()?, r.bool()?);
                let create = match r.u32()? {
                    0 => Create::No,
                    1 => Create::Unchecked(r.u32()?),
                    2 => Create::Guarded(r.u32()?),
                    3 => Create::Exclusive(r.array()?, r.u32()?),
                    _ => return Err(xdr::Bad),
                };
                Request::Open { path, read, write, create }
            }
            6 => Request::Close(r.u64()?),
            7 => Request::Read {
                handle: read_option(&mut r)?,
                path: path(&mut r)?,
                offset: r.u64()?,
                count: r.u32()?,
            },
            8 => Request::Write {
                handle: read_option(&mut r)?,
                path: path(&mut r)?,
                offset: r.u64()?,
                sync: r.bool()?,
                data: r.opaque(MESSAGE_MAX)?.to_vec(),
            },
            9 => Request::Sync(read_option(&mut r)?, path(&mut r)?),
            10 => {
                let (p, handle) = (path(&mut r)?, read_option(&mut r)?);
                let small =
                    |v: Option<u64>| v.map(|v| u32::try_from(v).map_err(|_| xdr::Bad)).transpose();
                let changes = Changes {
                    mode: small(read_option(&mut r)?)?,
                    uid: small(read_option(&mut r)?)?,
                    gid: small(read_option(&mut r)?)?,
                    size: read_option(&mut r)?,
                    atime: read_set_time(&mut r)?,
                    mtime: read_set_time(&mut r)?,
                };
                Request::SetAttr(p, handle, changes)
            }
            11 => {
                let p = path(&mut r)?;
                let make = match r.u32()? {
                    0 => Make::Directory(r.u32()?),
                    1 => Make::Symlink(r.opaque(PATH_MAX)?.to_vec()),
                    2 => Make::Fifo(r.u32()?),
                    3 => Make::Socket(r.u32()?),
                    4 => Make::Device(r.bool()?, r.u32()?, r.u32()?, r.u32()?),
                    _ => return Err(xdr::Bad),
                };
                Request::Make(p, make)
            }
            12 => Request::Remove(path(&mut r)?),
            13 => Request::Rename(path(&mut r)?, path(&mut r)?),
            14 => Request::Link(path(&mut r)?, path(&mut r)?),
            15 => Request::Find(path(&mut r)?, r.u64()?, r.u64()?, r.u32()?),
            _ => return Err(xdr::Bad),
        };
        if r.left() != 0 {
            return Err(xdr::Bad);
        }
        Ok(request)
    }
}

/// `answer`'s bytes.
pub fn encode_answer(answer: &Answer) -> Vec<u8> {
    let mut w = Writer::new();
    match answer {
        Err(errno) => {
            w.u32(1).i32(*errno);
        }
        Ok(reply) => {
            w.u32(0);
            match reply {
                Reply::Done => {
                    w.u32(0);
                }
                Reply::Stat(s) => {
                    w.u32(1);
                    write_stat(&mut w, s);
                }
                Reply::FsStat(f) => {
                    w.u32(2).u64(f.files_avail).u64(f.files_free).u64(f.files_total);
                    w.u64(f.space_avail).u64(f.space_free).u64(f.space_total);
                }
                Reply::Number(n) => {
                    w.u32(3).u64(*n);
                }
                Reply::List(entries, eof) => {
                    w.u32(4).u32(entries.len() as u32);
                    for (name, stat) in entries {
                        w.opaque(name);
                        write_stat(&mut w, stat);
                    }
                    w.bool(*eof);
                }
                Reply::Bytes(data, eof) => {
                    w.u32(5).opaque(data).bool(*eof);
                }
                Reply::Opened(handle, stat, made) => {
                    w.u32(6).u64(*handle);
                    write_stat(&mut w, stat);
                    w.bool(*made);
                }
                Reply::Found(path) => {
                    w.u32(7);
                    match path {
                        Some(p) => w.bool(true).opaque(p),
                        None => w.bool(false),
                    };
                }
            }
        }
    }
    w.bytes
}

/// The answer in `bytes`.
///
/// # Errors
///
/// Bytes that are no answer.
pub fn decode_answer(bytes: &[u8]) -> xdr::Result<Answer> {
    let mut r = Reader::new(bytes);
    if r.u32()? == 1 {
        return Ok(Err(r.i32()?));
    }
    let reply = match r.u32()? {
        0 => Reply::Done,
        1 => Reply::Stat(read_stat(&mut r)?),
        2 => Reply::FsStat(FsStat {
            files_avail: r.u64()?,
            files_free: r.u64()?,
            files_total: r.u64()?,
            space_avail: r.u64()?,
            space_free: r.u64()?,
            space_total: r.u64()?,
        }),
        3 => Reply::Number(r.u64()?),
        4 => {
            let n = r.count(1 << 16)?;
            let mut entries = Vec::with_capacity(n);
            for _ in 0..n {
                entries.push((r.opaque(PATH_MAX)?.to_vec(), read_stat(&mut r)?));
            }
            Reply::List(entries, r.bool()?)
        }
        5 => Reply::Bytes(r.opaque(MESSAGE_MAX)?.to_vec(), r.bool()?),
        6 => Reply::Opened(r.u64()?, read_stat(&mut r)?, r.bool()?),
        7 => Reply::Found(if r.bool()? { Some(r.opaque(PATH_MAX)?.to_vec()) } else { None }),
        _ => return Err(xdr::Bad),
    };
    Ok(Ok(reply))
}

/// Sends one message.
///
/// # Errors
///
/// The pipe's.
pub fn send(output: &mut impl Write, bytes: &[u8]) -> io::Result<()> {
    let mut framed = Vec::with_capacity(bytes.len() + 4);
    framed.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
    framed.extend_from_slice(bytes);
    output.write_all(&framed)
}

/// Receives one message: `None` when the other end closed.
///
/// # Errors
///
/// The pipe's, and `InvalidData` for one past [`MESSAGE_MAX`].
pub fn receive(input: &mut impl Read) -> io::Result<Option<Vec<u8>>> {
    let mut length = [0u8; 4];
    match input.read_exact(&mut length) {
        Ok(()) => {}
        Err(err) if err.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(err) => return Err(err),
    }
    let n = u32::from_be_bytes(length) as usize;
    if n > MESSAGE_MAX + 1024 {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "a message past its limit"));
    }
    let mut bytes = vec![0; n];
    input.read_exact(&mut bytes)?;
    Ok(Some(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_and_answers_go_both_ways() {
        let requests = [
            Request::Stat(b"/a".to_vec()),
            Request::Open {
                path: b"/a/b".to_vec(),
                read: true,
                write: false,
                create: Create::Exclusive([7; 8], 0o644),
            },
            Request::Write {
                handle: Some(3),
                path: b"/a".to_vec(),
                offset: 9,
                sync: true,
                data: vec![1, 2, 3],
            },
            Request::SetAttr(
                b"/a".to_vec(),
                None,
                Changes {
                    mode: Some(0o600),
                    size: Some(0),
                    mtime: SetTime::Now,
                    ..Changes::default()
                },
            ),
            Request::Make(b"/l".to_vec(), Make::Symlink(b"target".to_vec())),
            Request::Find(b"/r".to_vec(), 1, 2, 3),
        ];
        for request in requests {
            assert_eq!(Request::decode(&request.encode()), Ok(request));
        }
        let stat = Stat {
            kind: 1,
            mode: 0o644,
            ino: 42,
            mtime: Time { seconds: -1, nanos: 5 },
            ..Stat::default()
        };
        let answers = [
            Ok(Reply::List(vec![(b"x".to_vec(), stat.clone())], true)),
            Ok(Reply::Opened(4, stat, false)),
            Ok(Reply::Found(None)),
            Err(libc::ENOENT),
        ];
        for answer in answers {
            assert_eq!(decode_answer(&encode_answer(&answer)), Ok(answer));
        }
    }
}
