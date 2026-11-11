// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Receiving a send stream (ADR 0462): its commands applied to a
//! [`Target`], the folder received into, which makes each subvolume the
//! stream names in it.
//!
//! A stream comes from elsewhere, so every path in it is checked before a
//! target sees it: relative to the subvolume being made, of names that
//! are neither empty, `.` nor `..`, and without a NUL byte. The target
//! reaches nothing but the folder received into, so even a path through a
//! symbolic link the stream made stays inside it.

use std::io::{self, Read, Write};
use std::vec::Vec;

use super::stream::{Command, Reader, Time};

/// What a node the stream makes is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Make<'a> {
    /// An empty file.
    File,
    /// An empty folder.
    Dir,
    /// A device.
    Device {
        /// Its mode, with its kind.
        mode: u32,
        /// The device number it stands for.
        rdev: u64,
    },
    /// A named pipe.
    Fifo {
        /// Its mode.
        mode: u32,
    },
    /// A socket's name.
    Socket {
        /// Its mode.
        mode: u32,
    },
    /// A symbolic link.
    Symlink {
        /// What it points to.
        target: &'a [u8],
    },
}

/// Where a clone's bytes come from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CloneFrom<'a> {
    /// The subvolume being made.
    Here {
        /// The file.
        path: &'a [u8],
    },
    /// Another subvolume in the folder received into.
    Other {
        /// The UUID it was received as, or has.
        uuid: &'a [u8; 16],
        /// Its transaction, as the sender holds it.
        ctransid: u64,
        /// The file, in it.
        path: &'a [u8],
    },
}

/// What a stream changes: the folder received into, and the subvolume
/// being made in it. Paths are checked, and relative to that subvolume;
/// the empty path is its top.
pub trait Target {
    /// Makes the empty subvolume `name` in the folder received into: the
    /// commands up to the next end are of it.
    fn subvolume(&mut self, name: &[u8]) -> io::Result<()>;

    /// Makes `name` a writable snapshot of the subvolume in the folder
    /// received into that was received as, or is, `uuid` at transaction
    /// `ctransid`, to change into the snapshot sent.
    fn snapshot(&mut self, name: &[u8], uuid: &[u8; 16], ctransid: u64) -> io::Result<()>;

    /// Makes `what` at `path`.
    fn make(&mut self, path: &[u8], what: Make<'_>) -> io::Result<()>;
    /// Moves `from` to `to`.
    fn rename(&mut self, from: &[u8], to: &[u8]) -> io::Result<()>;
    /// Makes `path` another name of the file at `target`.
    fn link(&mut self, path: &[u8], target: &[u8]) -> io::Result<()>;
    /// Takes the name `path` away.
    fn unlink(&mut self, path: &[u8]) -> io::Result<()>;
    /// Removes the empty folder `path`.
    fn rmdir(&mut self, path: &[u8]) -> io::Result<()>;
    /// Sets extended attribute `name` of `path` to `value`.
    fn set_xattr(&mut self, path: &[u8], name: &[u8], value: &[u8]) -> io::Result<()>;
    /// Removes extended attribute `name` of `path`.
    fn remove_xattr(&mut self, path: &[u8], name: &[u8]) -> io::Result<()>;
    /// Writes `data` to `path` at `offset`.
    fn write(&mut self, path: &[u8], offset: u64, data: &[u8]) -> io::Result<()>;
    /// Makes `len` bytes of `path` from `offset` share those of `from`
    /// from `from_offset`.
    fn clone_range(
        &mut self,
        path: &[u8],
        offset: u64,
        len: u64,
        from: CloneFrom<'_>,
        from_offset: u64,
    ) -> io::Result<()>;
    /// Makes `path` `size` bytes long.
    fn truncate(&mut self, path: &[u8], size: u64) -> io::Result<()>;
    /// Sets the permission bits of `path`.
    fn chmod(&mut self, path: &[u8], mode: u32) -> io::Result<()>;
    /// Sets the owner and group of `path`.
    fn chown(&mut self, path: &[u8], uid: u32, gid: u32) -> io::Result<()>;
    /// Sets when `path` was last read and when its contents last changed.
    fn utimes(&mut self, path: &[u8], atime: Time, mtime: Time) -> io::Result<()>;

    /// Marks the subvolume made a copy of `uuid` as it was at transaction
    /// `ctransid`, and read-only.
    fn finish(&mut self, uuid: &[u8; 16], ctransid: u64) -> io::Result<()>;
}

/// What [`receive`] does.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Options {
    /// Stop at the first end, as `btrfs receive -e` does, for a stream
    /// that more follows in the same input.
    pub stop_at_end: bool,
}

/// What a receive made.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Received {
    /// The subvolumes made, by name.
    pub subvolumes: Vec<Vec<u8>>,
    /// The commands read.
    pub commands: u64,
    /// The files' bytes written.
    pub data: u64,
}

fn invalid(what: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, std::format!("refused a send stream: {what}"))
}

/// Whether `path` is a path in the subvolume made: relative, of names
/// that are neither empty, `.` nor `..`, and without NUL. `top` lets the
/// empty path, the subvolume's top, through.
pub fn checked(path: &[u8], top: bool) -> io::Result<&[u8]> {
    if path.is_empty() {
        return if top { Ok(path) } else { Err(invalid("a command on the subvolume's top")) };
    }
    if path.contains(&0) {
        return Err(invalid("a path with a NUL byte"));
    }
    for name in path.split(|&b| b == b'/') {
        if name.is_empty() || name == b"." || name == b".." {
            return Err(invalid("a path out of the subvolume, or with an empty name"));
        }
    }
    Ok(path)
}

/// Whether `name` is one name, for a subvolume.
fn one_name(name: &[u8]) -> io::Result<&[u8]> {
    if name.contains(&b'/') {
        return Err(invalid("a subvolume's name with a slash"));
    }
    checked(name, false)
}

/// A number the stream gives that a 32-bit field takes.
fn u32_of(value: u64, what: &str) -> io::Result<u32> {
    u32::try_from(value).map_err(|_| invalid(what))
}

/// Applies the stream `input` to `target`: every subvolume in it, one
/// after another, or with `options.stop_at_end`, the first.
pub fn receive<R: Read, T: Target>(
    input: R,
    target: &mut T,
    options: Options,
) -> io::Result<Received> {
    let mut reader = Reader::new(input);
    let mut received = Received::default();
    // The subvolume being made: its UUID and transaction.
    let mut current: Option<([u8; 16], u64)> = None;
    while let Some(command) = reader.command()? {
        received.commands += 1;
        use Command as C;
        let here = |current: &Option<([u8; 16], u64)>| -> io::Result<()> {
            if current.is_none() { Err(invalid("a command before its subvolume")) } else { Ok(()) }
        };
        match &command {
            C::Subvol { .. } | C::Snapshot { .. } | C::End => {}
            _ => here(&current)?,
        }
        match command {
            C::Subvol { path, uuid, ctransid } => {
                if current.is_some() {
                    return Err(invalid("a subvolume inside another"));
                }
                target.subvolume(one_name(&path)?)?;
                received.subvolumes.push(path);
                current = Some((uuid, ctransid));
            }
            C::Snapshot { path, uuid, ctransid, clone_uuid, clone_ctransid } => {
                if current.is_some() {
                    return Err(invalid("a subvolume inside another"));
                }
                target.snapshot(one_name(&path)?, &clone_uuid, clone_ctransid)?;
                received.subvolumes.push(path);
                current = Some((uuid, ctransid));
            }
            C::MkFile { path, .. } => target.make(checked(&path, false)?, Make::File)?,
            C::MkDir { path, .. } => target.make(checked(&path, false)?, Make::Dir)?,
            C::MkNod { path, mode, rdev, .. } => {
                let mode = u32_of(mode, "a mode past 32 bits")?;
                target.make(checked(&path, false)?, Make::Device { mode, rdev })?;
            }
            C::MkFifo { path, mode, .. } => {
                let mode = u32_of(mode, "a mode past 32 bits")?;
                target.make(checked(&path, false)?, Make::Fifo { mode })?;
            }
            C::MkSock { path, mode, .. } => {
                let mode = u32_of(mode, "a mode past 32 bits")?;
                target.make(checked(&path, false)?, Make::Socket { mode })?;
            }
            C::Symlink { path, target: to, .. } => {
                if to.is_empty() || to.contains(&0) {
                    return Err(invalid("a symbolic link to nothing"));
                }
                target.make(checked(&path, false)?, Make::Symlink { target: &to })?;
            }
            C::Rename { path, to } => {
                target.rename(checked(&path, false)?, checked(&to, false)?)?;
            }
            C::Link { path, target: to } => {
                target.link(checked(&path, false)?, checked(&to, false)?)?;
            }
            C::Unlink { path } => target.unlink(checked(&path, false)?)?,
            C::Rmdir { path } => target.rmdir(checked(&path, false)?)?,
            C::SetXattr { path, name, data } => {
                if name.is_empty() || name.contains(&0) {
                    return Err(invalid("an extended attribute without a name"));
                }
                target.set_xattr(checked(&path, true)?, &name, &data)?;
            }
            C::RemoveXattr { path, name } => {
                if name.is_empty() || name.contains(&0) {
                    return Err(invalid("an extended attribute without a name"));
                }
                target.remove_xattr(checked(&path, true)?, &name)?;
            }
            C::Write { path, offset, data } => {
                offset
                    .checked_add(data.len() as u64)
                    .ok_or_else(|| invalid("a write past the end"))?;
                target.write(checked(&path, false)?, offset, &data)?;
                received.data += data.len() as u64;
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
                let from = checked(&clone_path, false)?;
                let from = match current {
                    Some((uuid, _)) if uuid == clone_uuid => CloneFrom::Here { path: from },
                    _ => {
                        CloneFrom::Other { uuid: &clone_uuid, ctransid: clone_ctransid, path: from }
                    }
                };
                target.clone_range(checked(&path, false)?, offset, len, from, clone_offset)?;
            }
            C::Truncate { path, size } => target.truncate(checked(&path, false)?, size)?,
            C::Chmod { path, mode } => {
                target
                    .chmod(checked(&path, true)?, u32_of(mode, "a mode past 32 bits")? & 0o7777)?;
            }
            C::Chown { path, uid, gid } => {
                let (uid, gid) =
                    (u32_of(uid, "a user past 32 bits")?, u32_of(gid, "a group past 32 bits")?);
                target.chown(checked(&path, true)?, uid, gid)?;
            }
            C::Utimes { path, atime, mtime, .. } => {
                target.utimes(checked(&path, true)?, atime, mtime)?;
            }
            // What a stream without data has for a write: nothing to do.
            C::UpdateExtent { path, .. } => {
                checked(&path, false)?;
            }
            C::End => {
                // An end with no subvolume ends a stream of none.
                if let Some((uuid, ctransid)) = current.take() {
                    target.finish(&uuid, ctransid)?;
                }
                if options.stop_at_end {
                    break;
                }
            }
        }
    }
    if current.is_some() {
        return Err(invalid("it ends in the middle of a subvolume"));
    }
    Ok(received)
}

/// A UUID as text.
fn uuid_text(uuid: &[u8; 16]) -> std::string::String {
    let mut text = std::string::String::new();
    for (i, byte) in uuid.iter().enumerate() {
        if matches!(i, 4 | 6 | 8 | 10) {
            text.push('-');
        }
        text.push_str(&std::format!("{byte:02x}"));
    }
    text
}

/// `bytes` as text, escaped as `btrfs receive --dump` escapes paths:
/// backslashes and what does not print as octal.
fn escaped(bytes: &[u8]) -> std::string::String {
    let mut text = std::string::String::new();
    for &b in bytes {
        match b {
            b'\\' => text.push_str("\\\\"),
            b' ' | b'\t' | b'\n' | b'\r' | 0x0b | 0x0c => text.push_str(&std::format!("\\{b:03o}")),
            0x21..=0x7e => text.push(char::from(b)),
            _ => text.push_str(&std::format!("\\{b:03o}")),
        }
    }
    text
}

/// A path as a dump shows it: under `./`.
fn shown(path: &[u8]) -> std::string::String {
    if path.is_empty() { "./".into() } else { std::format!("./{}", escaped(path)) }
}

fn time_text(time: &Time) -> std::string::String {
    std::format!("{}.{:09}", time.secs, time.nanos)
}

/// Writes `command` to `out` as `btrfs receive --dump` writes it, for a
/// stream read and not applied.
pub fn dump(command: &Command, subvolume: &[u8], out: &mut impl Write) -> io::Result<()> {
    use Command as C;
    let at = |path: &[u8]| -> std::string::String {
        if subvolume.is_empty() || path.is_empty() {
            shown(if path.is_empty() { subvolume } else { path })
        } else {
            let mut full = subvolume.to_vec();
            full.push(b'/');
            full.extend_from_slice(path);
            shown(&full)
        }
    };
    let (title, path, rest) = match command {
        C::Subvol { path, uuid, ctransid } => {
            ("subvol", shown(path), std::format!("uuid={} transid={ctransid}", uuid_text(uuid)))
        }
        C::Snapshot { path, uuid, ctransid, clone_uuid, clone_ctransid } => (
            "snapshot",
            shown(path),
            std::format!(
                "uuid={} transid={ctransid} parent_uuid={} parent_transid={clone_ctransid}",
                uuid_text(uuid),
                uuid_text(clone_uuid)
            ),
        ),
        C::MkFile { path, .. } => ("mkfile", at(path), std::string::String::new()),
        C::MkDir { path, .. } => ("mkdir", at(path), std::string::String::new()),
        C::MkNod { path, mode, rdev, .. } => {
            ("mknod", at(path), std::format!("mode={mode:o} dev=0x{rdev:x}"))
        }
        C::MkFifo { path, .. } => ("mkfifo", at(path), std::string::String::new()),
        C::MkSock { path, .. } => ("mksock", at(path), std::string::String::new()),
        C::Symlink { path, target, .. } => {
            ("symlink", at(path), std::format!("dest={}", escaped(target)))
        }
        C::Rename { path, to } => ("rename", at(path), std::format!("dest={}", at(to))),
        C::Link { path, target } => ("link", at(path), std::format!("dest={}", escaped(target))),
        C::Unlink { path } => ("unlink", at(path), std::string::String::new()),
        C::Rmdir { path } => ("rmdir", at(path), std::string::String::new()),
        C::SetXattr { path, name, data } => (
            "set_xattr",
            at(path),
            std::format!("name={} data={} len={}", escaped(name), escaped(data), data.len()),
        ),
        C::RemoveXattr { path, name } => {
            ("remove_xattr", at(path), std::format!("name={}", escaped(name)))
        }
        C::Write { path, offset, data } => {
            ("write", at(path), std::format!("offset={offset} len={}", data.len()))
        }
        C::Clone { path, offset, len, clone_path, clone_offset, .. } => (
            "clone",
            at(path),
            std::format!(
                "offset={offset} len={len} from={} clone_offset={clone_offset}",
                at(clone_path)
            ),
        ),
        C::Truncate { path, size } => ("truncate", at(path), std::format!("size={size}")),
        C::Chmod { path, mode } => ("chmod", at(path), std::format!("mode={mode:o}")),
        C::Chown { path, uid, gid } => ("chown", at(path), std::format!("gid={gid} uid={uid}")),
        C::Utimes { path, atime, mtime, ctime } => (
            "utimes",
            at(path),
            std::format!(
                "atime={} mtime={} ctime={}",
                time_text(atime),
                time_text(mtime),
                time_text(ctime)
            ),
        ),
        C::UpdateExtent { path, offset, size } => {
            ("update_extent", at(path), std::format!("offset={offset} len={size}"))
        }
        C::End => return Ok(()),
    };
    writeln!(out, "{title:<16}{path:<32} {rest}")
}

/// Reads the stream `input` and writes each command to `out`, as `btrfs
/// receive --dump` does, checking the stream as [`receive`] would.
pub fn dump_all<R: Read>(input: R, out: &mut impl Write) -> io::Result<Received> {
    let mut reader = Reader::new(input);
    let mut received = Received::default();
    let mut subvolume: Vec<u8> = Vec::new();
    while let Some(command) = reader.command()? {
        received.commands += 1;
        match &command {
            Command::Subvol { path, .. } | Command::Snapshot { path, .. } => {
                one_name(path)?;
                subvolume.clone_from(path);
                received.subvolumes.push(path.clone());
            }
            Command::Write { data, .. } => received.data += data.len() as u64,
            _ => {}
        }
        dump(&command, &subvolume, out)?;
    }
    Ok(received)
}
