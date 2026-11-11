// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The send stream (ADR 0462), version 1, as Linux's `btrfs send` writes
//! it and `btrfs receive` reads it (btrfs.readthedocs.io, "Send stream
//! format").
//!
//! A stream is a header, the 13 bytes `btrfs-stream\0` and the version as
//! a little-endian `u32`, then commands. A command is its length (of what
//! follows its header), its number and a CRC-32C of the whole command
//! with that field zero, then attributes: each a type, a length and that
//! many bytes, every number little-endian. Streams may follow one another,
//! each with its own header, as `btrfs send` writes one per subvolume.
//!
//! [`Writer`] makes commands and [`Reader`] reads them back as
//! [`Command`]s, checking each one's length, sum and attributes: a stream
//! comes from another machine, a disk or a pipe, and nothing in it is
//! trusted.

use std::io::{self, Read, Write};
use std::vec::Vec;

use crate::csum::crc32c_update;

/// The bytes a stream starts with.
pub const MAGIC: &[u8; 13] = b"btrfs-stream\0";

/// The version Dukovo writes and reads.
pub const VERSION: u32 = 1;

/// A stream header's size: the magic and the version.
pub const HEADER_SIZE: usize = 17;

/// A command header's size: its length, number and sum.
pub const COMMAND_HEADER: usize = 10;

/// An attribute header's size: its type and length.
pub const ATTRIBUTE_HEADER: usize = 4;

/// The largest command of a version 1 stream, header included: what
/// btrfs-progs reads (`BTRFS_SEND_BUF_SIZE_V1`).
pub const MAX_COMMAND: usize = 64 << 10;

/// The most data one write carries, as Linux sends it
/// (`BTRFS_SEND_READ_SIZE`): room is left for the path.
pub const MAX_WRITE: usize = 48 << 10;

/// The commands, by number.
pub mod cmd {
    /// A new subvolume.
    pub const SUBVOL: u16 = 1;
    /// A snapshot of a subvolume the receiver holds.
    pub const SNAPSHOT: u16 = 2;
    /// A new file.
    pub const MKFILE: u16 = 3;
    /// A new folder.
    pub const MKDIR: u16 = 4;
    /// A new device.
    pub const MKNOD: u16 = 5;
    /// A new named pipe.
    pub const MKFIFO: u16 = 6;
    /// A new socket's name.
    pub const MKSOCK: u16 = 7;
    /// A new symbolic link.
    pub const SYMLINK: u16 = 8;
    /// A move.
    pub const RENAME: u16 = 9;
    /// A new name for a file.
    pub const LINK: u16 = 10;
    /// A name taken away.
    pub const UNLINK: u16 = 11;
    /// A folder removed.
    pub const RMDIR: u16 = 12;
    /// An extended attribute set.
    pub const SET_XATTR: u16 = 13;
    /// An extended attribute removed.
    pub const REMOVE_XATTR: u16 = 14;
    /// Bytes written.
    pub const WRITE: u16 = 15;
    /// Bytes shared with another file.
    pub const CLONE: u16 = 16;
    /// A new size.
    pub const TRUNCATE: u16 = 17;
    /// New permission bits.
    pub const CHMOD: u16 = 18;
    /// A new owner and group.
    pub const CHOWN: u16 = 19;
    /// New times.
    pub const UTIMES: u16 = 20;
    /// The end of a subvolume.
    pub const END: u16 = 21;
    /// Bytes changed, for a stream without data.
    pub const UPDATE_EXTENT: u16 = 22;
}

/// The attributes, by type.
pub mod attr {
    /// A subvolume's UUID.
    pub const UUID: u16 = 1;
    /// A subvolume's transaction.
    pub const CTRANSID: u16 = 2;
    /// An inode number.
    pub const INO: u16 = 3;
    /// A size.
    pub const SIZE: u16 = 4;
    /// A mode.
    pub const MODE: u16 = 5;
    /// An owner.
    pub const UID: u16 = 6;
    /// A group.
    pub const GID: u16 = 7;
    /// A device number.
    pub const RDEV: u16 = 8;
    /// A change time.
    pub const CTIME: u16 = 9;
    /// A modification time.
    pub const MTIME: u16 = 10;
    /// An access time.
    pub const ATIME: u16 = 11;
    /// A creation time.
    pub const OTIME: u16 = 12;
    /// An extended attribute's name.
    pub const XATTR_NAME: u16 = 13;
    /// An extended attribute's value.
    pub const XATTR_DATA: u16 = 14;
    /// A path.
    pub const PATH: u16 = 15;
    /// A path moved to.
    pub const PATH_TO: u16 = 16;
    /// A link's target.
    pub const PATH_LINK: u16 = 17;
    /// A place in a file.
    pub const FILE_OFFSET: u16 = 18;
    /// Bytes.
    pub const DATA: u16 = 19;
    /// The UUID of the subvolume bytes are shared from.
    pub const CLONE_UUID: u16 = 20;
    /// That subvolume's transaction.
    pub const CLONE_CTRANSID: u16 = 21;
    /// The file bytes are shared from.
    pub const CLONE_PATH: u16 = 22;
    /// The place in that file.
    pub const CLONE_OFFSET: u16 = 23;
    /// How many bytes are shared.
    pub const CLONE_LEN: u16 = 24;
    /// The last type of version 1.
    pub const MAX: u16 = 24;
}

/// A time as a stream carries it: seconds since 1970 and nanoseconds.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Time {
    /// Seconds since 1970, UTC.
    pub secs: i64,
    /// Nanoseconds past them.
    pub nanos: u32,
}

impl Time {
    /// The time `ns` nanoseconds after 1970.
    pub fn from_nanos(ns: u64) -> Time {
        Time { secs: (ns / 1_000_000_000) as i64, nanos: (ns % 1_000_000_000) as u32 }
    }
}

/// A command of a stream, read. Paths are relative to the subvolume the
/// stream makes, as bytes: Btrfs's names are bytes, not text. The empty
/// path is the subvolume's top directory.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Command {
    /// Make the subvolume `path`, empty, in the directory received into.
    Subvol {
        /// The new subvolume's name.
        path: Vec<u8>,
        /// The UUID of the subvolume sent.
        uuid: [u8; 16],
        /// The transaction that last changed a file in the subvolume sent.
        ctransid: u64,
    },
    /// Make the subvolume `path` a snapshot of the one received as
    /// `clone_uuid` at `clone_ctransid`, which the commands then change.
    Snapshot {
        /// The new subvolume's name.
        path: Vec<u8>,
        /// The UUID of the subvolume sent.
        uuid: [u8; 16],
        /// The transaction that last changed a file in the subvolume sent.
        ctransid: u64,
        /// The UUID of the parent snapshot the receiver holds.
        clone_uuid: [u8; 16],
        /// The parent's transaction, as the sender holds it.
        clone_ctransid: u64,
    },
    /// Make an empty file.
    MkFile {
        /// Its path in the subvolume.
        path: Vec<u8>,
        /// Its inode number on the sender.
        ino: u64,
    },
    /// Make a directory.
    MkDir {
        /// Its path in the subvolume.
        path: Vec<u8>,
        /// Its inode number on the sender.
        ino: u64,
    },
    /// Make a device.
    MkNod {
        /// Its path in the subvolume.
        path: Vec<u8>,
        /// Its inode number on the sender.
        ino: u64,
        /// Its mode.
        mode: u64,
        /// The device number it stands for.
        rdev: u64,
    },
    /// Make a named pipe.
    MkFifo {
        /// Its path in the subvolume.
        path: Vec<u8>,
        /// Its inode number on the sender.
        ino: u64,
        /// Its mode.
        mode: u64,
        /// The device number it stands for.
        rdev: u64,
    },
    /// Make a socket's file.
    MkSock {
        /// Its path in the subvolume.
        path: Vec<u8>,
        /// Its inode number on the sender.
        ino: u64,
        /// Its mode.
        mode: u64,
        /// The device number it stands for.
        rdev: u64,
    },
    /// Make a symbolic link to `target`.
    Symlink {
        /// Its path in the subvolume.
        path: Vec<u8>,
        /// Its inode number on the sender.
        ino: u64,
        /// What it points to.
        target: Vec<u8>,
    },
    /// Move `path` to `to`.
    Rename {
        /// Its path in the subvolume.
        path: Vec<u8>,
        /// The new path.
        to: Vec<u8>,
    },
    /// Give the file at `target` another name, `path`.
    Link {
        /// Its path in the subvolume.
        path: Vec<u8>,
        /// The path of the file.
        target: Vec<u8>,
    },
    /// Take a name away.
    Unlink {
        /// Its path in the subvolume.
        path: Vec<u8>,
    },
    /// Remove an empty directory.
    Rmdir {
        /// Its path in the subvolume.
        path: Vec<u8>,
    },
    /// Set an extended attribute.
    SetXattr {
        /// Its path in the subvolume.
        path: Vec<u8>,
        /// The attribute's name.
        name: Vec<u8>,
        /// The bytes.
        data: Vec<u8>,
    },
    /// Remove an extended attribute.
    RemoveXattr {
        /// Its path in the subvolume.
        path: Vec<u8>,
        /// The attribute's name.
        name: Vec<u8>,
    },
    /// Write `data` at `offset`.
    Write {
        /// Its path in the subvolume.
        path: Vec<u8>,
        /// Where in the file, in bytes.
        offset: u64,
        /// The bytes.
        data: Vec<u8>,
    },
    /// Make `len` bytes at `offset` share those at `clone_offset` of
    /// `clone_path`, in the subvolume received as `clone_uuid`.
    Clone {
        /// Its path in the subvolume.
        path: Vec<u8>,
        /// Where in the file, in bytes.
        offset: u64,
        /// How many bytes.
        len: u64,
        /// The UUID of the subvolume the bytes come from.
        clone_uuid: [u8; 16],
        /// That subvolume's transaction, as the sender holds it.
        clone_ctransid: u64,
        /// The file the bytes come from.
        clone_path: Vec<u8>,
        /// Where in that file.
        clone_offset: u64,
    },
    /// Make a file `size` bytes long.
    Truncate {
        /// Its path in the subvolume.
        path: Vec<u8>,
        /// The new size, in bytes.
        size: u64,
    },
    /// Set the permissions.
    Chmod {
        /// Its path in the subvolume.
        path: Vec<u8>,
        /// The permission bits.
        mode: u64,
    },
    /// Set the owner and group.
    Chown {
        /// Its path in the subvolume.
        path: Vec<u8>,
        /// The owner.
        uid: u64,
        /// The group.
        gid: u64,
    },
    /// Set the times; the change time no one can set.
    Utimes {
        /// Its path in the subvolume.
        path: Vec<u8>,
        /// When it was last read.
        atime: Time,
        /// When its contents last changed.
        mtime: Time,
        /// When it last changed: sent, and set by no one.
        ctime: Time,
    },
    /// What a stream without data (`btrfs send --no-data`) has in place of
    /// a write: that `size` bytes at `offset` changed.
    UpdateExtent {
        /// Its path in the subvolume.
        path: Vec<u8>,
        /// Where in the file, in bytes.
        offset: u64,
        /// How many bytes.
        size: u64,
    },
    /// The subvolume is whole.
    End,
}

/// Writes commands to a stream.
pub struct Writer<W: Write> {
    out: W,
    /// The command being made, header first.
    command: Vec<u8>,
    /// The bytes written so far.
    written: u64,
}

impl<W: Write> Writer<W> {
    /// A stream to `out`: writes its header.
    pub fn new(mut out: W) -> io::Result<Writer<W>> {
        out.write_all(MAGIC)?;
        out.write_all(&VERSION.to_le_bytes())?;
        Ok(Writer { out, command: Vec::with_capacity(MAX_COMMAND), written: HEADER_SIZE as u64 })
    }

    /// The bytes written so far.
    pub fn written(&self) -> u64 {
        self.written
    }

    /// Starts command `number`.
    fn begin(&mut self, number: u16) {
        self.command.clear();
        self.command.extend_from_slice(&[0; 4]);
        self.command.extend_from_slice(&number.to_le_bytes());
        self.command.extend_from_slice(&[0; 4]);
    }

    /// Adds attribute `kind` with `bytes`.
    fn bytes(&mut self, kind: u16, bytes: &[u8]) -> io::Result<()> {
        let Ok(len) = u16::try_from(bytes.len()) else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "an attribute too long to send",
            ));
        };
        self.command.extend_from_slice(&kind.to_le_bytes());
        self.command.extend_from_slice(&len.to_le_bytes());
        self.command.extend_from_slice(bytes);
        Ok(())
    }

    fn u64(&mut self, kind: u16, value: u64) -> io::Result<()> {
        self.bytes(kind, &value.to_le_bytes())
    }

    fn time(&mut self, kind: u16, time: Time) -> io::Result<()> {
        let mut bytes = [0u8; 12];
        bytes[..8].copy_from_slice(&time.secs.to_le_bytes());
        bytes[8..].copy_from_slice(&time.nanos.to_le_bytes());
        self.bytes(kind, &bytes)
    }

    /// Ends the command: its length and sum, then out.
    fn finish(&mut self) -> io::Result<()> {
        if self.command.len() > MAX_COMMAND {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "a command too long to send"));
        }
        let len = (self.command.len() - COMMAND_HEADER) as u32;
        self.command[..4].copy_from_slice(&len.to_le_bytes());
        let crc = crc32c_update(0, &self.command);
        self.command[6..10].copy_from_slice(&crc.to_le_bytes());
        self.out.write_all(&self.command)?;
        self.written += self.command.len() as u64;
        Ok(())
    }

    /// Writes `command`.
    pub fn put(&mut self, command: &Command) -> io::Result<()> {
        use Command as C;
        match command {
            C::Subvol { path, uuid, ctransid } => {
                self.begin(cmd::SUBVOL);
                self.bytes(attr::PATH, path)?;
                self.bytes(attr::UUID, uuid)?;
                self.u64(attr::CTRANSID, *ctransid)?;
            }
            C::Snapshot { path, uuid, ctransid, clone_uuid, clone_ctransid } => {
                self.begin(cmd::SNAPSHOT);
                self.bytes(attr::PATH, path)?;
                self.bytes(attr::UUID, uuid)?;
                self.u64(attr::CTRANSID, *ctransid)?;
                self.bytes(attr::CLONE_UUID, clone_uuid)?;
                self.u64(attr::CLONE_CTRANSID, *clone_ctransid)?;
            }
            C::MkFile { path, ino } | C::MkDir { path, ino } => {
                let number =
                    if matches!(command, C::MkFile { .. }) { cmd::MKFILE } else { cmd::MKDIR };
                self.begin(number);
                self.bytes(attr::PATH, path)?;
                self.u64(attr::INO, *ino)?;
            }
            C::MkNod { path, ino, mode, rdev }
            | C::MkFifo { path, ino, mode, rdev }
            | C::MkSock { path, ino, mode, rdev } => {
                self.begin(match command {
                    C::MkNod { .. } => cmd::MKNOD,
                    C::MkFifo { .. } => cmd::MKFIFO,
                    _ => cmd::MKSOCK,
                });
                self.bytes(attr::PATH, path)?;
                self.u64(attr::INO, *ino)?;
                self.u64(attr::RDEV, *rdev)?;
                self.u64(attr::MODE, *mode)?;
            }
            C::Symlink { path, ino, target } => {
                self.begin(cmd::SYMLINK);
                self.bytes(attr::PATH, path)?;
                self.u64(attr::INO, *ino)?;
                self.bytes(attr::PATH_LINK, target)?;
            }
            C::Rename { path, to } => {
                self.begin(cmd::RENAME);
                self.bytes(attr::PATH, path)?;
                self.bytes(attr::PATH_TO, to)?;
            }
            C::Link { path, target } => {
                self.begin(cmd::LINK);
                self.bytes(attr::PATH, path)?;
                self.bytes(attr::PATH_LINK, target)?;
            }
            C::Unlink { path } => {
                self.begin(cmd::UNLINK);
                self.bytes(attr::PATH, path)?;
            }
            C::Rmdir { path } => {
                self.begin(cmd::RMDIR);
                self.bytes(attr::PATH, path)?;
            }
            C::SetXattr { path, name, data } => {
                self.begin(cmd::SET_XATTR);
                self.bytes(attr::PATH, path)?;
                self.bytes(attr::XATTR_NAME, name)?;
                self.bytes(attr::XATTR_DATA, data)?;
            }
            C::RemoveXattr { path, name } => {
                self.begin(cmd::REMOVE_XATTR);
                self.bytes(attr::PATH, path)?;
                self.bytes(attr::XATTR_NAME, name)?;
            }
            C::Write { path, offset, data } => {
                self.begin(cmd::WRITE);
                self.bytes(attr::PATH, path)?;
                self.u64(attr::FILE_OFFSET, *offset)?;
                self.bytes(attr::DATA, data)?;
            }
            C::Clone {
                path,
                offset,
                len,
                clone_uuid,
                clone_ctransid,
                clone_path,
                clone_offset,
            } => {
                self.begin(cmd::CLONE);
                self.bytes(attr::PATH, path)?;
                self.u64(attr::FILE_OFFSET, *offset)?;
                self.u64(attr::CLONE_LEN, *len)?;
                self.bytes(attr::CLONE_UUID, clone_uuid)?;
                self.u64(attr::CLONE_CTRANSID, *clone_ctransid)?;
                self.bytes(attr::CLONE_PATH, clone_path)?;
                self.u64(attr::CLONE_OFFSET, *clone_offset)?;
            }
            C::Truncate { path, size } => {
                self.begin(cmd::TRUNCATE);
                self.bytes(attr::PATH, path)?;
                self.u64(attr::SIZE, *size)?;
            }
            C::Chmod { path, mode } => {
                self.begin(cmd::CHMOD);
                self.bytes(attr::PATH, path)?;
                self.u64(attr::MODE, *mode)?;
            }
            C::Chown { path, uid, gid } => {
                self.begin(cmd::CHOWN);
                self.bytes(attr::PATH, path)?;
                self.u64(attr::UID, *uid)?;
                self.u64(attr::GID, *gid)?;
            }
            C::Utimes { path, atime, mtime, ctime } => {
                self.begin(cmd::UTIMES);
                self.bytes(attr::PATH, path)?;
                self.time(attr::ATIME, *atime)?;
                self.time(attr::MTIME, *mtime)?;
                self.time(attr::CTIME, *ctime)?;
            }
            C::UpdateExtent { path, offset, size } => {
                self.begin(cmd::UPDATE_EXTENT);
                self.bytes(attr::PATH, path)?;
                self.u64(attr::FILE_OFFSET, *offset)?;
                self.u64(attr::SIZE, *size)?;
            }
            C::End => self.begin(cmd::END),
        }
        self.finish()
    }

    /// Flushes what is written, and gives the output back.
    pub fn into_inner(mut self) -> io::Result<W> {
        self.out.flush()?;
        Ok(self.out)
    }
}

/// Why a stream could not be read.
fn bad(what: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, std::format!("not a send stream: {what}"))
}

/// The attributes of one command, by type.
struct Attributes<'a> {
    found: [Option<&'a [u8]>; attr::MAX as usize + 1],
}

impl<'a> Attributes<'a> {
    /// The attributes in `body`, each type once.
    fn of(mut body: &'a [u8]) -> io::Result<Attributes<'a>> {
        let mut found = [None; attr::MAX as usize + 1];
        while !body.is_empty() {
            if body.len() < ATTRIBUTE_HEADER {
                return Err(bad("an attribute cut short"));
            }
            let kind = u16::from_le_bytes([body[0], body[1]]);
            let len = usize::from(u16::from_le_bytes([body[2], body[3]]));
            let Some(data) = body.get(ATTRIBUTE_HEADER..ATTRIBUTE_HEADER + len) else {
                return Err(bad("an attribute longer than its command"));
            };
            let slot = found.get_mut(usize::from(kind)).filter(|_| kind != 0);
            let Some(slot) = slot else { return Err(bad("an attribute of an unknown type")) };
            if slot.is_some() {
                return Err(bad("an attribute given twice"));
            }
            *slot = Some(data);
            body = &body[ATTRIBUTE_HEADER + len..];
        }
        Ok(Attributes { found })
    }

    fn bytes(&self, kind: u16) -> io::Result<Vec<u8>> {
        self.found[usize::from(kind)].map(<[u8]>::to_vec).ok_or_else(|| bad("an attribute missing"))
    }

    fn u64(&self, kind: u16) -> io::Result<u64> {
        let bytes = self.found[usize::from(kind)].ok_or_else(|| bad("an attribute missing"))?;
        let word: [u8; 8] = bytes.try_into().map_err(|_| bad("a number of the wrong size"))?;
        Ok(u64::from_le_bytes(word))
    }

    fn uuid(&self, kind: u16) -> io::Result<[u8; 16]> {
        let bytes = self.found[usize::from(kind)].ok_or_else(|| bad("an attribute missing"))?;
        bytes.try_into().map_err(|_| bad("a UUID of the wrong size"))
    }

    fn time(&self, kind: u16) -> io::Result<Time> {
        let bytes = self.found[usize::from(kind)].ok_or_else(|| bad("an attribute missing"))?;
        if bytes.len() != 12 {
            return Err(bad("a time of the wrong size"));
        }
        let mut secs = [0u8; 8];
        secs.copy_from_slice(&bytes[..8]);
        let mut nanos = [0u8; 4];
        nanos.copy_from_slice(&bytes[8..]);
        Ok(Time { secs: i64::from_le_bytes(secs), nanos: u32::from_le_bytes(nanos) })
    }
}

/// Reads the commands of a stream, or of streams one after another.
pub struct Reader<R: Read> {
    input: R,
    buffer: Vec<u8>,
    /// Whether a header is due: at the start, and after an END.
    header_due: bool,
    /// The bytes read so far.
    read: u64,
}

impl<R: Read> Reader<R> {
    /// The stream `input`, its header not read yet.
    pub fn new(input: R) -> Reader<R> {
        Reader { input, buffer: Vec::with_capacity(MAX_COMMAND), header_due: true, read: 0 }
    }

    /// The bytes read so far.
    pub fn read_so_far(&self) -> u64 {
        self.read
    }

    /// Fills `buf` from the input; false if it ended before the first byte.
    fn fill(&mut self, buf_len: usize) -> io::Result<bool> {
        self.buffer.clear();
        self.buffer.resize(buf_len, 0);
        let mut got = 0;
        while got < buf_len {
            match self.input.read(&mut self.buffer[got..]) {
                Ok(0) if got == 0 => return Ok(false),
                Ok(0) => return Err(bad("it ends in the middle of a command")),
                Ok(n) => got += n,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(e) => return Err(e),
            }
        }
        self.read += buf_len as u64;
        Ok(true)
    }

    /// The next command; `None` where the input ends after a whole one.
    pub fn command(&mut self) -> io::Result<Option<Command>> {
        if self.header_due {
            if !self.fill(HEADER_SIZE)? {
                return if self.read == 0 { Err(bad("it is empty")) } else { Ok(None) };
            }
            if &self.buffer[..13] != MAGIC {
                return Err(bad("it does not start as one"));
            }
            let version = u32::from_le_bytes([
                self.buffer[13],
                self.buffer[14],
                self.buffer[15],
                self.buffer[16],
            ]);
            if version != VERSION {
                return Err(io::Error::new(
                    io::ErrorKind::Unsupported,
                    std::format!(
                        "a send stream of version {version}: Dukovo reads version 1, which \
                         `btrfs send --proto 1` makes"
                    ),
                ));
            }
            self.header_due = false;
        }
        if !self.fill(COMMAND_HEADER)? {
            return Err(bad("it ends before an END command"));
        }
        let len =
            u32::from_le_bytes([self.buffer[0], self.buffer[1], self.buffer[2], self.buffer[3]])
                as usize;
        let number = u16::from_le_bytes([self.buffer[4], self.buffer[5]]);
        let crc =
            u32::from_le_bytes([self.buffer[6], self.buffer[7], self.buffer[8], self.buffer[9]]);
        if len > MAX_COMMAND - COMMAND_HEADER {
            return Err(bad("a command longer than version 1 allows"));
        }
        let mut header = [0u8; COMMAND_HEADER];
        header.copy_from_slice(&self.buffer[..COMMAND_HEADER]);
        header[6..10].fill(0);
        if len > 0 && !self.fill(len)? {
            return Err(bad("it ends in the middle of a command"));
        }
        if len == 0 {
            self.buffer.clear();
        }
        let body = &self.buffer[..len];
        if crc32c_update(crc32c_update(0, &header), body) != crc {
            return Err(bad("a command's checksum is wrong"));
        }
        let a = Attributes::of(body)?;
        use Command as C;
        let command = match number {
            cmd::SUBVOL => C::Subvol {
                path: a.bytes(attr::PATH)?,
                uuid: a.uuid(attr::UUID)?,
                ctransid: a.u64(attr::CTRANSID)?,
            },
            cmd::SNAPSHOT => C::Snapshot {
                path: a.bytes(attr::PATH)?,
                uuid: a.uuid(attr::UUID)?,
                ctransid: a.u64(attr::CTRANSID)?,
                clone_uuid: a.uuid(attr::CLONE_UUID)?,
                clone_ctransid: a.u64(attr::CLONE_CTRANSID)?,
            },
            cmd::MKFILE => C::MkFile { path: a.bytes(attr::PATH)?, ino: a.u64(attr::INO)? },
            cmd::MKDIR => C::MkDir { path: a.bytes(attr::PATH)?, ino: a.u64(attr::INO)? },
            cmd::MKNOD | cmd::MKFIFO | cmd::MKSOCK => {
                let (path, ino) = (a.bytes(attr::PATH)?, a.u64(attr::INO)?);
                let (mode, rdev) = (a.u64(attr::MODE)?, a.u64(attr::RDEV)?);
                match number {
                    cmd::MKNOD => C::MkNod { path, ino, mode, rdev },
                    cmd::MKFIFO => C::MkFifo { path, ino, mode, rdev },
                    _ => C::MkSock { path, ino, mode, rdev },
                }
            }
            cmd::SYMLINK => C::Symlink {
                path: a.bytes(attr::PATH)?,
                ino: a.u64(attr::INO)?,
                target: a.bytes(attr::PATH_LINK)?,
            },
            cmd::RENAME => C::Rename { path: a.bytes(attr::PATH)?, to: a.bytes(attr::PATH_TO)? },
            cmd::LINK => C::Link { path: a.bytes(attr::PATH)?, target: a.bytes(attr::PATH_LINK)? },
            cmd::UNLINK => C::Unlink { path: a.bytes(attr::PATH)? },
            cmd::RMDIR => C::Rmdir { path: a.bytes(attr::PATH)? },
            cmd::SET_XATTR => C::SetXattr {
                path: a.bytes(attr::PATH)?,
                name: a.bytes(attr::XATTR_NAME)?,
                data: a.bytes(attr::XATTR_DATA)?,
            },
            cmd::REMOVE_XATTR => {
                C::RemoveXattr { path: a.bytes(attr::PATH)?, name: a.bytes(attr::XATTR_NAME)? }
            }
            cmd::WRITE => C::Write {
                path: a.bytes(attr::PATH)?,
                offset: a.u64(attr::FILE_OFFSET)?,
                data: a.bytes(attr::DATA)?,
            },
            cmd::CLONE => C::Clone {
                path: a.bytes(attr::PATH)?,
                offset: a.u64(attr::FILE_OFFSET)?,
                len: a.u64(attr::CLONE_LEN)?,
                clone_uuid: a.uuid(attr::CLONE_UUID)?,
                clone_ctransid: a.u64(attr::CLONE_CTRANSID)?,
                clone_path: a.bytes(attr::CLONE_PATH)?,
                clone_offset: a.u64(attr::CLONE_OFFSET)?,
            },
            cmd::TRUNCATE => C::Truncate { path: a.bytes(attr::PATH)?, size: a.u64(attr::SIZE)? },
            cmd::CHMOD => C::Chmod { path: a.bytes(attr::PATH)?, mode: a.u64(attr::MODE)? },
            cmd::CHOWN => C::Chown {
                path: a.bytes(attr::PATH)?,
                uid: a.u64(attr::UID)?,
                gid: a.u64(attr::GID)?,
            },
            cmd::UTIMES => C::Utimes {
                path: a.bytes(attr::PATH)?,
                atime: a.time(attr::ATIME)?,
                mtime: a.time(attr::MTIME)?,
                ctime: a.time(attr::CTIME)?,
            },
            cmd::UPDATE_EXTENT => C::UpdateExtent {
                path: a.bytes(attr::PATH)?,
                offset: a.u64(attr::FILE_OFFSET)?,
                size: a.u64(attr::SIZE)?,
            },
            cmd::END => {
                self.header_due = true;
                C::End
            }
            number => {
                return Err(io::Error::new(
                    io::ErrorKind::Unsupported,
                    std::format!("a send stream command Dukovo does not know ({number})"),
                ));
            }
        };
        Ok(Some(command))
    }
}
