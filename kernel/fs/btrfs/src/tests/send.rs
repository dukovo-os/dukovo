// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Host tests of send and receive (ADR 0462): two snapshots' trees
//! compared against a full read of both; streams written and read back,
//! damaged and refused; snapshots sent whole and as changes after rounds
//! of random changes, received on a second filesystem and compared file
//! by file; Linux's `btrfs receive --dump` reading the streams; and
//! streams Linux's `btrfs send` made, received.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::format;
use std::io;
use std::string::{String, ToString};
use std::vec;
use std::vec::Vec;

use radix_vfs::{
    Access, AccessChange, Error, FileSystem, Kind, NodeRef, Setting, Slice, SliceMut, TreeKey,
    TreeRange,
};

use super::XorShift;
use super::files::noise;
use super::rw::mount_rw;
use super::write::{btrfs_check, image_of, mkfs_empty, writable};
use super::{DIR, FILE, LINK, scratch, tool};
use crate::format::Key;
use crate::fs::BtrfsFs;
use crate::send::items;
use crate::send::receive::{self, CloneFrom, Make, Options, Target};
use crate::send::sender::{self, Item, Source, Subvolume, Trees};
use crate::send::stream::{Command, MAX_COMMAND, Reader, Time, Writer};

fn io_error(err: Error) -> io::Error {
    io::Error::other(format!("{err:?}"))
}

fn text(bytes: &[u8]) -> io::Result<&str> {
    core::str::from_utf8(bytes).map_err(|_| io::Error::other("a name that is not UTF-8"))
}

/// The node at `path` ("a/b", "" for `top`) under `top`, links not
/// followed.
fn node_at(top: &NodeRef, path: &[u8]) -> io::Result<NodeRef> {
    let mut node = top.clone();
    if path.is_empty() {
        return Ok(node);
    }
    for name in path.split(|&b| b == b'/') {
        node = node.lookup(text(name)?).map_err(io_error)?;
    }
    Ok(node)
}

/// The folder that holds `path`, and its last name.
fn parent_of<'p>(top: &NodeRef, path: &'p [u8]) -> io::Result<(NodeRef, &'p str)> {
    match path.iter().rposition(|&b| b == b'/') {
        Some(at) => Ok((node_at(top, &path[..at])?, text(&path[at + 1..])?)),
        None => Ok((top.clone(), text(path)?)),
    }
}

/// A snapshot and its parent, read through the VFS, a few items at a
/// time so that every walk goes on from where the last call stopped.
struct VfsSource {
    snapshot: NodeRef,
    parent: Option<NodeRef>,
    batch: usize,
}

impl Source for VfsSource {
    fn compare(
        &mut self,
        trees: Trees,
        from: &Key,
        to: &Key,
        out: &mut Vec<Item>,
    ) -> io::Result<bool> {
        let key = |k: &Key| TreeKey { objectid: k.objectid, kind: k.kind, offset: k.offset };
        let range = TreeRange { start: key(from), end: key(to) };
        let (node, parent) = match trees {
            Trees::Both => (&self.snapshot, self.parent.as_ref()),
            Trees::Old => {
                (self.parent.as_ref().ok_or_else(|| io::Error::other("no parent"))?, None)
            }
            Trees::New => (&self.snapshot, None),
        };
        let (first, batch) = (out.len(), self.batch);
        let done = node
            .compare(parent, &range, &mut |item| {
                if out.len() - first == batch {
                    return false;
                }
                let k = Key::new(item.key.objectid, item.key.kind, item.key.offset);
                out.push(Item {
                    key: k,
                    old: item.old.map(<[u8]>::to_vec),
                    new: item.new.map(<[u8]>::to_vec),
                });
                true
            })
            .map_err(io_error)?;
        if trees == Trees::Old {
            for item in &mut out[first..] {
                item.old = item.new.take();
            }
        }
        Ok(done)
    }

    fn read(&mut self, path: &[u8], offset: u64, out: &mut [u8]) -> io::Result<usize> {
        let node = node_at(&self.snapshot, path)?;
        let mut done = 0;
        while done < out.len() {
            let n = node
                .read(offset + done as u64, &mut SliceMut(&mut out[done..]))
                .map_err(io_error)?;
            if n == 0 {
                break;
            }
            done += n;
        }
        Ok(done)
    }

    fn read_link(&mut self, path: &[u8]) -> io::Result<Vec<u8>> {
        let node = node_at(&self.snapshot, path)?;
        let mut buf = vec![0; 4096];
        let n = node.read_link(&mut SliceMut(&mut buf)).map_err(io_error)?;
        buf.truncate(n);
        Ok(buf)
    }
}

/// What a send says of the subvolume `top` is the top of.
fn subvolume_of(top: &NodeRef) -> Subvolume {
    let info = top.subvolume().unwrap();
    Subvolume {
        uuid: info.uuid,
        received_uuid: info.received_uuid,
        changed_transaction: info.changed_transaction,
    }
}

/// A folder received into, through the VFS, as subvolume(8) receives
/// through a folder's capability.
struct VfsTarget {
    dest: NodeRef,
    top: Option<NodeRef>,
}

impl VfsTarget {
    fn top(&self) -> io::Result<&NodeRef> {
        self.top.as_ref().ok_or_else(|| io::Error::other("no subvolume"))
    }

    fn at(&self, path: &[u8]) -> io::Result<NodeRef> {
        node_at(self.top()?, path)
    }

    /// The subvolume in the folder received into that was received as,
    /// or is, `uuid` at `ctransid`.
    fn find(&self, uuid: &[u8; 16], ctransid: u64) -> io::Result<NodeRef> {
        let mut names = Vec::new();
        self.dest
            .list(0, &mut |_, name, kind| {
                if kind == Kind::Directory {
                    names.push(name.to_string());
                }
                true
            })
            .map_err(io_error)?;
        let mut by_uuid = None;
        for name in names {
            let node = self.dest.lookup(&name).map_err(io_error)?;
            let Ok(info) = node.subvolume() else { continue };
            if info.name.as_slice() != name.as_bytes() {
                continue;
            }
            if info.received_uuid == *uuid && info.sent_transaction == ctransid {
                return Ok(node);
            }
            if info.uuid == *uuid && info.changed_transaction == ctransid {
                by_uuid = Some(node);
            }
        }
        by_uuid.ok_or_else(|| io::Error::other("no parent subvolume here"))
    }
}

impl Target for VfsTarget {
    fn subvolume(&mut self, name: &[u8]) -> io::Result<()> {
        self.dest.make_subvolume(text(name)?).map_err(io_error)?;
        self.top = Some(self.dest.lookup(text(name)?).map_err(io_error)?);
        Ok(())
    }

    fn snapshot(&mut self, name: &[u8], uuid: &[u8; 16], ctransid: u64) -> io::Result<()> {
        let parent = self.find(uuid, ctransid)?;
        self.dest.snapshot(text(name)?, &parent, false).map_err(io_error)?;
        self.top = Some(self.dest.lookup(text(name)?).map_err(io_error)?);
        Ok(())
    }

    fn make(&mut self, path: &[u8], what: Make<'_>) -> io::Result<()> {
        let (dir, name) = parent_of(self.top()?, path)?;
        let access =
            |mode: u32, kind| Access { mode: (mode & 0o7777) as u16, ..Access::default_for(kind) };
        match what {
            Make::File => dir.create(name, Kind::File, FILE).map(drop),
            Make::Dir => dir.create(name, Kind::Directory, DIR).map(drop),
            Make::Symlink { target } => dir.make_link(name, text(target)?, LINK).map(drop),
            Make::Fifo { mode } => {
                dir.make_node(name, Kind::Pipe, 0, access(mode, Kind::Pipe)).map(drop)
            }
            Make::Socket { mode } => {
                dir.make_node(name, Kind::Socket, 0, access(mode, Kind::Socket)).map(drop)
            }
            Make::Device { mode, rdev } => {
                let kind = if mode & items::kind::MASK == items::kind::BLOCK {
                    Kind::BlockDevice
                } else {
                    Kind::CharDevice
                };
                let (major, minor) = items::device_parts(rdev);
                let device = radix_vfs::device_number(major, minor);
                dir.make_node(name, kind, device, access(mode, kind)).map(drop)
            }
        }
        .map_err(io_error)
    }

    fn rename(&mut self, from: &[u8], to: &[u8]) -> io::Result<()> {
        let (from_dir, from_name) = parent_of(self.top()?, from)?;
        let (to_dir, to_name) = parent_of(self.top()?, to)?;
        from_dir.rename(from_name, &*to_dir, to_name).map_err(io_error)
    }

    fn link(&mut self, path: &[u8], target: &[u8]) -> io::Result<()> {
        let (dir, name) = parent_of(self.top()?, path)?;
        let node = self.at(target)?;
        dir.link(name, &node).map_err(io_error)
    }

    fn unlink(&mut self, path: &[u8]) -> io::Result<()> {
        let (dir, name) = parent_of(self.top()?, path)?;
        dir.remove(name).map_err(io_error)
    }

    fn rmdir(&mut self, path: &[u8]) -> io::Result<()> {
        self.unlink(path)
    }

    fn set_xattr(&mut self, path: &[u8], name: &[u8], value: &[u8]) -> io::Result<()> {
        self.at(path)?.set_attribute(name, value, Setting::Either).map_err(io_error)
    }

    fn remove_xattr(&mut self, path: &[u8], name: &[u8]) -> io::Result<()> {
        self.at(path)?.remove_attribute(name).map_err(io_error)
    }

    fn write(&mut self, path: &[u8], offset: u64, data: &[u8]) -> io::Result<()> {
        let node = self.at(path)?;
        let n = node.write(offset, &Slice(data)).map_err(io_error)?;
        if n == data.len() { Ok(()) } else { Err(io::Error::other("a short write")) }
    }

    fn clone_range(
        &mut self,
        path: &[u8],
        offset: u64,
        len: u64,
        from: CloneFrom<'_>,
        from_offset: u64,
    ) -> io::Result<()> {
        let source = match from {
            CloneFrom::Here { path } => self.at(path)?,
            CloneFrom::Other { uuid, ctransid, path } => {
                node_at(&self.find(uuid, ctransid)?, path)?
            }
        };
        self.at(path)?.clone_range(&source, from_offset, len, offset).map_err(io_error)
    }

    fn truncate(&mut self, path: &[u8], size: u64) -> io::Result<()> {
        self.at(path)?.set_size(size).map_err(io_error)
    }

    fn chmod(&mut self, path: &[u8], mode: u32) -> io::Result<()> {
        let change = AccessChange { mode: Some(mode as u16), ..AccessChange::default() };
        self.at(path)?.set_access(&change).map_err(io_error)
    }

    fn chown(&mut self, path: &[u8], uid: u32, gid: u32) -> io::Result<()> {
        // As Linux's lchown does: the set-ID bits of what is not a folder go.
        let node = self.at(path)?;
        node.set_access(&AccessChange::chown(node.kind(), Some(uid), Some(gid))).map_err(io_error)
    }

    fn utimes(&mut self, path: &[u8], atime: Time, mtime: Time) -> io::Result<()> {
        let ns = |t: Time| t.secs as u64 * 1_000_000_000 + u64::from(t.nanos);
        self.at(path)?.set_times(Some(ns(atime)), Some(ns(mtime))).map_err(io_error)
    }

    fn finish(&mut self, uuid: &[u8; 16], ctransid: u64) -> io::Result<()> {
        self.top()?.mark_received(uuid, ctransid, 0).map_err(io_error)?;
        self.top = None;
        Ok(())
    }
}

/// What a file holds, as a send carries it.
#[derive(Clone, Debug, PartialEq, Eq)]
enum What {
    Dir,
    File(Vec<u8>),
    Link(String),
    Node(Kind, u64),
}

/// A file as a send carries it: what it holds, its owner, permissions,
/// times and extended attributes, and its first path, for files with
/// more than one name.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Entry {
    what: What,
    access: Access,
    modified: u64,
    accessed: u64,
    xattrs: BTreeMap<Vec<u8>, Vec<u8>>,
    first: String,
    /// Its inode number, on its own filesystem.
    id: u64,
}

/// Every file under `top`, by path ("" for `top` itself).
fn tree_of(top: &NodeRef) -> BTreeMap<String, Entry> {
    let mut out = BTreeMap::new();
    let mut seen: BTreeMap<u64, String> = BTreeMap::new();
    walk(top, "", &mut out, &mut seen);
    // A file's names, by the first of them in the order of paths, as
    // folders list their entries in any order.
    let ids: BTreeMap<String, u64> = out.iter().map(|(path, e)| (path.clone(), e.id)).collect();
    let mut first: BTreeMap<u64, String> = BTreeMap::new();
    for (path, id) in &ids {
        first.entry(*id).or_insert_with(|| path.clone());
    }
    for entry in out.values_mut() {
        entry.first.clone_from(&first[&entry.id]);
    }
    out
}

fn walk(
    node: &NodeRef,
    path: &str,
    out: &mut BTreeMap<String, Entry>,
    seen: &mut BTreeMap<u64, String>,
) {
    let stat = node.stat();
    let first = seen.entry(stat.id).or_insert_with(|| path.to_string()).clone();
    let id = stat.id;
    let what = match stat.kind {
        Kind::Directory => What::Dir,
        Kind::File => What::File(super::files::contents(&**node, 1 << 16).unwrap()),
        Kind::Symlink => {
            let mut buf = vec![0; 4096];
            let n = node.read_link(&mut SliceMut(&mut buf)).unwrap();
            What::Link(String::from_utf8(buf[..n].to_vec()).unwrap())
        }
        kind => What::Node(kind, stat.device),
    };
    let mut xattrs = BTreeMap::new();
    if let Ok(names) = node.list_attributes() {
        for name in names.split(|&b| b == 0).filter(|n| !n.is_empty()) {
            xattrs.insert(name.to_vec(), node.get_attribute(name).unwrap().clone());
        }
    }
    let entry = Entry {
        what,
        access: node.access(),
        modified: stat.modified,
        accessed: stat.accessed,
        xattrs,
        first,
        id,
    };
    out.insert(path.to_string(), entry);
    if stat.kind == Kind::Directory {
        let mut names = Vec::new();
        node.list(0, &mut |_, name, _| {
            names.push(name.to_string());
            true
        })
        .unwrap();
        for name in names {
            let child = node.lookup(&name).unwrap();
            let path = if path.is_empty() { name } else { format!("{path}/{name}") };
            walk(&child, &path, out, seen);
        }
    }
}

/// Checks that `got` holds what `want` does.
fn assert_same(want: &BTreeMap<String, Entry>, got: &BTreeMap<String, Entry>, what: &str) {
    let (w, g): (Vec<_>, Vec<_>) = (want.keys().collect(), got.keys().collect());
    assert_eq!(w, g, "{what}: the paths differ");
    for (path, entry) in want {
        let other = &got[path];
        assert!(entry.what == other.what, "{what}: /{path} holds other data");
        assert_eq!(entry.access, other.access, "{what}: /{path}");
        assert_eq!(entry.modified, other.modified, "{what}: /{path} mtime");
        assert_eq!(entry.accessed, other.accessed, "{what}: /{path} atime");
        assert_eq!(entry.xattrs, other.xattrs, "{what}: /{path} xattrs");
        assert_eq!(entry.first, other.first, "{what}: /{path} links");
    }
}

/// Sends the read-only subvolume `snapshot`, from `parent` if there is
/// one, as `name`; returns the stream.
fn send(snapshot: &NodeRef, parent: Option<&NodeRef>, name: &str, batch: usize) -> Vec<u8> {
    let mut source = VfsSource { snapshot: snapshot.clone(), parent: parent.cloned(), batch };
    let mut out = Writer::new(Vec::new()).unwrap();
    let parent = parent.map(subvolume_of);
    sender::send(&mut source, name.as_bytes(), &subvolume_of(snapshot), parent.as_ref(), &mut out)
        .unwrap_or_else(|e| panic!("send {name}: {e}"));
    out.into_inner().unwrap()
}

/// Receives `stream` into `dest`.
fn receive_into(dest: &NodeRef, stream: &[u8]) -> io::Result<receive::Received> {
    let mut target = VfsTarget { dest: dest.clone(), top: None };
    receive::receive(stream, &mut target, Options::default())
}

/// Linux's `btrfs receive --dump` reads `stream` as a whole stream, and
/// says as many commands as there are.
fn linux_dumps(stream: &[u8]) {
    // Linux dumps the first subvolume's stream: one line a command but
    // its end.
    let mut reader = Reader::new(stream);
    let mut commands = 0u64;
    while let Some(command) = reader.command().unwrap() {
        if command == Command::End {
            break;
        }
        commands += 1;
    }
    let path = scratch("stream");
    std::fs::write(&path, stream).unwrap();
    let out = std::process::Command::new(tool("btrfs"))
        .args(["receive", "--dump", "-f"])
        .arg(&path)
        .output()
        .unwrap();
    let _ = std::fs::remove_file(&path);
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "btrfs receive --dump: {}", String::from_utf8_lossy(&out.stderr));
    // A line a command, but for the lines in extended attributes' values,
    // which it writes as they are.
    let titles = [
        "subvol",
        "snapshot",
        "mkfile",
        "mkdir",
        "mknod",
        "mkfifo",
        "mksock",
        "symlink",
        "rename",
        "link",
        "unlink",
        "rmdir",
        "set_xattr",
        "remove_xattr",
        "write",
        "clone",
        "truncate",
        "chmod",
        "chown",
        "utimes",
        "update_extent",
    ];
    let lines = text
        .lines()
        .filter(|line| {
            line.split_once(' ').is_some_and(|(title, rest)| {
                titles.contains(&title) && rest.trim_start().starts_with("./")
            })
        })
        .count();
    assert_eq!(lines as u64, commands, "btrfs receive --dump:\n{text}");
}

/// Every kind of command, as a stream holds it.
fn every_command() -> Vec<Command> {
    let p = |s: &str| s.as_bytes().to_vec();
    let time = Time { secs: 1_790_000_000, nanos: 123_456_789 };
    vec![
        Command::Subvol { path: p("v"), uuid: [7; 16], ctransid: 42 },
        Command::MkFile { path: p("f"), ino: 257 },
        Command::MkDir { path: p("d"), ino: 258 },
        Command::MkNod { path: p("d/n"), ino: 259, mode: 0o20644, rdev: 0x103 },
        Command::MkFifo { path: p("d/p"), ino: 260, mode: 0o10600, rdev: 0 },
        Command::MkSock { path: p("d/s"), ino: 261, mode: 0o140755, rdev: 0 },
        Command::Symlink { path: p("l"), ino: 262, target: p("../outside/target") },
        Command::Rename { path: p("f"), to: p("d/f") },
        Command::Link { path: p("g"), target: p("d/f") },
        Command::Unlink { path: p("g") },
        Command::Rmdir { path: p("e") },
        Command::SetXattr { path: p("d/f"), name: p("user.a"), data: vec![0, 1, 2, 255] },
        Command::RemoveXattr { path: p("d/f"), name: p("user.a") },
        Command::Write { path: p("d/f"), offset: 4096, data: noise(3, 49152) },
        Command::Clone {
            path: p("d/f"),
            offset: 0,
            len: 4096,
            clone_uuid: [9; 16],
            clone_ctransid: 40,
            clone_path: p("x"),
            clone_offset: 8192,
        },
        Command::Truncate { path: p("d/f"), size: 1 << 40 },
        Command::Chmod { path: p("d/f"), mode: 0o4755 },
        Command::Chown { path: p(""), uid: 1000, gid: 100 },
        Command::Utimes { path: p("d"), atime: time, mtime: time, ctime: time },
        Command::UpdateExtent { path: p("d/f"), offset: 0, size: 100 },
        Command::End,
        Command::Snapshot {
            path: p("w"),
            uuid: [8; 16],
            ctransid: 50,
            clone_uuid: [7; 16],
            clone_ctransid: 42,
        },
        Command::End,
    ]
}

fn stream_of(commands: &[Command]) -> Vec<u8> {
    let mut writer = Writer::new(Vec::new()).unwrap();
    for (i, command) in commands.iter().enumerate() {
        writer.put(command).unwrap();
        // The next subvolume's stream after an end, as `btrfs send` makes
        // one per subvolume.
        if *command == Command::End && i + 1 < commands.len() {
            let inner = writer.into_inner().unwrap();
            writer = Writer::new(inner).unwrap();
        }
    }
    writer.into_inner().unwrap()
}

fn read_all(stream: &[u8]) -> io::Result<Vec<Command>> {
    let mut reader = Reader::new(stream);
    let mut commands = Vec::new();
    while let Some(command) = reader.command()? {
        commands.push(command);
    }
    Ok(commands)
}

#[test]
fn streams_read_back_every_command_written() {
    let commands = every_command();
    let stream = stream_of(&commands);
    assert_eq!(read_all(&stream).unwrap(), commands);
    // Linux reads them too: two streams, one after the other.
    linux_dumps(&stream);
    // A command that cannot fit is refused, not cut.
    let mut writer = Writer::new(Vec::new()).unwrap();
    let huge = Command::Write { path: b"f".to_vec(), offset: 0, data: vec![0; MAX_COMMAND] };
    assert!(writer.put(&huge).is_err());
}

#[test]
fn damaged_streams_are_refused() {
    let stream = stream_of(&every_command()[..3]);
    let refused = |bytes: &[u8], what: &str| {
        assert!(read_all(bytes).is_err(), "{what}: read");
    };
    refused(b"", "empty");
    refused(&stream[..10], "a header cut short");
    let mut bad = stream.clone();
    bad[0] = b'B';
    refused(&bad, "another magic");
    let mut v2 = stream.clone();
    v2[13] = 2;
    let err = read_all(&v2).unwrap_err();
    assert_eq!(err.kind(), io::ErrorKind::Unsupported, "version 2");
    // Every byte of the commands is under their sums.
    for at in 17..stream.len() {
        let mut bad = stream.clone();
        bad[at] ^= 0x10;
        refused(&bad, &format!("byte {at} changed"));
    }
    for len in 18..stream.len() {
        // Cut anywhere but after a whole command: refused, or a stream
        // with no end, which receive refuses.
        let _ = read_all(&stream[..len]);
    }
}

/// A target that only counts, for streams that are not applied.
#[derive(Default)]
struct Counting(u64);

impl Target for Counting {
    fn subvolume(&mut self, _: &[u8]) -> io::Result<()> {
        Ok(())
    }
    fn snapshot(&mut self, _: &[u8], _: &[u8; 16], _: u64) -> io::Result<()> {
        Ok(())
    }
    fn make(&mut self, _: &[u8], _: Make<'_>) -> io::Result<()> {
        self.0 += 1;
        Ok(())
    }
    fn rename(&mut self, _: &[u8], _: &[u8]) -> io::Result<()> {
        Ok(())
    }
    fn link(&mut self, _: &[u8], _: &[u8]) -> io::Result<()> {
        Ok(())
    }
    fn unlink(&mut self, _: &[u8]) -> io::Result<()> {
        Ok(())
    }
    fn rmdir(&mut self, _: &[u8]) -> io::Result<()> {
        Ok(())
    }
    fn set_xattr(&mut self, _: &[u8], _: &[u8], _: &[u8]) -> io::Result<()> {
        Ok(())
    }
    fn remove_xattr(&mut self, _: &[u8], _: &[u8]) -> io::Result<()> {
        Ok(())
    }
    fn write(&mut self, _: &[u8], _: u64, _: &[u8]) -> io::Result<()> {
        Ok(())
    }
    fn clone_range(
        &mut self,
        _: &[u8],
        _: u64,
        _: u64,
        _: CloneFrom<'_>,
        _: u64,
    ) -> io::Result<()> {
        Ok(())
    }
    fn truncate(&mut self, _: &[u8], _: u64) -> io::Result<()> {
        Ok(())
    }
    fn chmod(&mut self, _: &[u8], _: u32) -> io::Result<()> {
        Ok(())
    }
    fn chown(&mut self, _: &[u8], _: u32, _: u32) -> io::Result<()> {
        Ok(())
    }
    fn utimes(&mut self, _: &[u8], _: Time, _: Time) -> io::Result<()> {
        Ok(())
    }
    fn finish(&mut self, _: &[u8; 16], _: u64) -> io::Result<()> {
        Ok(())
    }
}

#[test]
fn paths_out_of_the_subvolume_are_refused() {
    let p = |s: &str| s.as_bytes().to_vec();
    let start = Command::Subvol { path: p("v"), uuid: [1; 16], ctransid: 1 };
    let bad = [
        Command::MkFile { path: p("../escape"), ino: 300 },
        Command::MkFile { path: p("/etc/passwd"), ino: 300 },
        Command::MkFile { path: p("a//b"), ino: 300 },
        Command::MkFile { path: p("a/./b"), ino: 300 },
        Command::MkFile { path: p("a/.."), ino: 300 },
        Command::MkFile { path: p(""), ino: 300 },
        Command::MkFile { path: b"a\0b".to_vec(), ino: 300 },
        Command::Rename { path: p("a"), to: p("../a") },
        Command::Link { path: p("a"), target: p("/etc/shadow") },
        Command::Write { path: p("a/../../b"), offset: 0, data: p("x") },
        Command::Clone {
            path: p("a"),
            offset: 0,
            len: 1,
            clone_uuid: [1; 16],
            clone_ctransid: 1,
            clone_path: p("../b"),
            clone_offset: 0,
        },
        Command::Write { path: p("a"), offset: u64::MAX, data: p("x") },
    ];
    for command in bad {
        let stream = stream_of(&[start.clone(), command.clone(), Command::End]);
        let result = receive::receive(&stream[..], &mut Counting::default(), Options::default());
        assert!(result.is_err(), "{command:?} was taken");
    }
    for name in ["", ".", "..", "a/b"] {
        let stream = stream_of(&[
            Command::Subvol { path: p(name), uuid: [1; 16], ctransid: 1 },
            Command::End,
        ]);
        let result = receive::receive(&stream[..], &mut Counting::default(), Options::default());
        assert!(result.is_err(), "subvolume {name:?} was taken");
    }
    // A command before its subvolume, and a subvolume left unfinished.
    let stream = stream_of(&[Command::MkFile { path: p("a"), ino: 300 }, Command::End]);
    assert!(receive::receive(&stream[..], &mut Counting::default(), Options::default()).is_err());
    let stream = stream_of(&[start.clone(), Command::MkFile { path: p("a"), ino: 300 }]);
    assert!(receive::receive(&stream[..], &mut Counting::default(), Options::default()).is_err());
    // A symbolic link may point anywhere: what it points to is followed
    // by no one but the programs that open it.
    let link = Command::Symlink { path: p("l"), ino: 300, target: p("../../etc") };
    let stream = stream_of(&[start, link, Command::End]);
    assert!(receive::receive(&stream[..], &mut Counting::default(), Options::default()).is_ok());
}

/// Damaged streams are read to an answer or an error, never a panic.
#[test]
fn damaged_streams_never_panic() {
    let stream = stream_of(&every_command());
    let mut random = XorShift(0x5eed);
    for round in 0..4000 {
        let mut bad = stream.clone();
        for _ in 0..=random.below(8) {
            let at = random.below(bad.len());
            bad[at] = random.next() as u8;
        }
        if round % 3 == 0 {
            bad.truncate(random.below(bad.len()));
        }
        let _ = read_all(&bad);
        let _ = receive::receive(&bad[..], &mut Counting::default(), Options::default());
        let _ = receive::dump_all(&bad[..], &mut io::sink());
    }
    // Commands with their sums right after damage: only the checks of
    // what is in them stand between the damage and the code that reads it.
    let mut random = XorShift(0xfeed);
    for _ in 0..4000 {
        let mut bad = stream.clone();
        let at = 17 + random.below(bad.len() - 17);
        bad[at] = random.next() as u8;
        reseal(&mut bad);
        let _ = read_all(&bad);
        let _ = receive::receive(&bad[..], &mut Counting::default(), Options::default());
    }
}

/// Puts every command's sum right, as far as the lengths still lead.
fn reseal(stream: &mut [u8]) {
    let mut at = 17;
    while at + 10 <= stream.len() {
        if stream[at..].starts_with(b"btrfs-stream\0") {
            at += 17;
            continue;
        }
        let len = u32::from_le_bytes(stream[at..at + 4].try_into().unwrap()) as usize;
        let Some(end) = (at + 10).checked_add(len).filter(|&end| end <= stream.len()) else {
            return;
        };
        stream[at + 6..at + 10].fill(0);
        let crc = crate::csum::crc32c_update(0, &stream[at..end]);
        stream[at + 6..at + 10].copy_from_slice(&crc.to_le_bytes());
        at = end;
    }
}

/// Gives files the times of the host's clock, so that a time a send
/// leaves out shows: with none, every time is 0.
fn set_clock() {
    radix_vfs::set_clock(|| {
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH);
        now.map_or(1, |now| now.as_nanos() as u64)
    });
}

/// A filesystem to change at random, and its subvolume `src`.
struct Changing {
    disk: radix_vfs::mem::KArc<radix_block::ram::RamDisk>,
    fs: radix_vfs::mem::KArc<BtrfsFs>,
    src: NodeRef,
    random: XorShift,
    count: u64,
}

impl Changing {
    fn new(seed: u64) -> Changing {
        set_clock();
        let disk = writable(&mkfs_empty(&["--nodesize", "4096"]));
        let fs = mount_rw(&disk);
        fs.root().make_subvolume("src").unwrap();
        let src = fs.root().lookup("src").unwrap();
        Changing { disk, fs, src, random: XorShift(seed), count: 0 }
    }

    /// Fills `src` with `n` files in a few folders, of every size.
    fn fill(&mut self, n: usize) {
        for d in 0..8 {
            self.src.create(&format!("d{d}"), Kind::Directory, DIR).unwrap();
        }
        for i in 0..n {
            let dir = self.src.lookup(&format!("d{}", i % 8)).unwrap();
            let node = dir.create(&format!("f{i}"), Kind::File, FILE).unwrap();
            let data = self.data();
            node.write(0, &Slice(&data)).unwrap();
            if i % 5 == 0 {
                let value = noise(i as u64, 40);
                node.set_attribute(b"user.mark", &value, Setting::Either).unwrap();
            }
        }
    }

    /// Every folder and file under `src`, by path, folders first.
    fn paths(&self) -> (Vec<String>, Vec<String>) {
        let tree = tree_of(&self.src);
        let mut dirs = Vec::new();
        let mut files = Vec::new();
        for (path, entry) in tree {
            if entry.what == What::Dir {
                dirs.push(path);
            } else {
                files.push(path);
            }
        }
        (dirs, files)
    }

    fn pick<'a>(&mut self, of: &'a [String]) -> Option<&'a String> {
        if of.is_empty() { None } else { Some(&of[self.random.below(of.len())]) }
    }

    fn name(&mut self) -> String {
        self.count += 1;
        // Short names, so that names come back: a new file where one was.
        match self.random.below(4) {
            0 => format!("n{}", self.random.below(6)),
            1 => format!("file {}", self.count),
            2 => format!("d{}", self.random.below(4)),
            _ => format!("é{}", self.count),
        }
    }

    fn data(&mut self) -> Vec<u8> {
        let len = match self.random.below(6) {
            0 => 0,
            1 => self.random.below(100),
            2 => self.random.below(3000),
            3 => 4096 * (1 + self.random.below(4)),
            4 => self.random.below(200_000),
            _ => 70_000 + self.random.below(9000),
        };
        noise(self.random.next(), len)
    }

    /// One change at random.
    fn change(&mut self) {
        let (dirs, files) = self.paths();
        let dir = self.pick(&dirs).cloned().unwrap_or_default();
        let at = node_at(&self.src, dir.as_bytes()).unwrap();
        let name = self.name();
        let file = self.pick(&files).cloned();
        let ns = 1_700_000_000_000_000_000 + self.random.next() % 100_000_000_000_000_000;
        match self.random.below(17) {
            0 | 1 => {
                if let Ok(node) = at.create(&name, Kind::File, FILE) {
                    let data = self.data();
                    node.write(0, &Slice(&data)).unwrap();
                }
            }
            2 => {
                let _ = at.create(&name, Kind::Directory, DIR);
            }
            3 => {
                let target = format!("../{}", self.name());
                let _ = at.make_link(&name, &target, LINK);
            }
            4 => {
                let kind = [Kind::Pipe, Kind::CharDevice, Kind::BlockDevice, Kind::Socket]
                    [self.random.below(4)];
                let device = radix_vfs::device_number(
                    self.random.below(300) as u32,
                    self.random.below(70_000) as u32,
                );
                let _ = at.make_node(
                    &name,
                    kind,
                    if matches!(kind, Kind::CharDevice | Kind::BlockDevice) { device } else { 0 },
                    Access::default_for(kind),
                );
            }
            5 => {
                // Another name for a file.
                if let Some(file) = &file {
                    let node = node_at(&self.src, file.as_bytes()).unwrap();
                    if node.kind() != Kind::Directory {
                        let _ = at.link(&name, &node);
                    }
                }
            }
            6 | 7 => {
                // A move, of a file or a folder, over whatever has the name.
                let all: Vec<String> =
                    dirs.iter().chain(files.iter()).filter(|p| !p.is_empty()).cloned().collect();
                if let Some(from) = self.pick(&all).cloned() {
                    let into_itself = dir == from || dir.starts_with(&format!("{from}/"));
                    if !into_itself {
                        let (from_dir, from_name) = parent_of(&self.src, from.as_bytes()).unwrap();
                        let _ = from_dir.rename(from_name, &*at, &name);
                    }
                }
            }
            8 => {
                // Two folders swap their names.
                let all: Vec<String> = dirs.iter().filter(|p| !p.is_empty()).cloned().collect();
                if let (Some(a), Some(b)) = (self.pick(&all).cloned(), self.pick(&all).cloned()) {
                    let nested = a.starts_with(&format!("{b}/")) || b.starts_with(&format!("{a}/"));
                    if a != b && !nested {
                        let (da, na) = parent_of(&self.src, a.as_bytes()).unwrap();
                        let (db, nb) = parent_of(&self.src, b.as_bytes()).unwrap();
                        if da.rename(na, &*self.src, "swap").is_ok() {
                            db.rename(nb, &*da, na).unwrap();
                            self.src.rename("swap", &*db, nb).unwrap();
                        }
                    }
                }
            }
            9 | 10 => {
                // Gone: a file, or a folder and all it holds.
                let all: Vec<String> =
                    dirs.iter().chain(files.iter()).filter(|p| !p.is_empty()).cloned().collect();
                if let Some(path) = self.pick(&all).cloned() {
                    remove_all(&self.src, &path);
                }
            }
            11 | 12 => {
                // Bytes written over, past the end, or cut.
                if let Some(file) = &file {
                    let node = node_at(&self.src, file.as_bytes()).unwrap();
                    if node.kind() == Kind::File {
                        let size = node.stat().size;
                        match self.random.below(4) {
                            0 => node.set_size(self.random.next() % (size + 1)).unwrap(),
                            1 => node.set_size(size + self.random.next() % 300_000).unwrap(),
                            _ => {
                                let data = self.data();
                                let offset = self.random.next() % (size + 5000);
                                node.write(offset, &Slice(&data)).unwrap();
                            }
                        }
                    }
                }
            }
            13 => {
                if let Some(file) = &file {
                    let node = node_at(&self.src, file.as_bytes()).unwrap();
                    let change = if self.random.below(2) == 0 {
                        AccessChange {
                            mode: Some((self.random.next() % 0o7777) as u16),
                            ..AccessChange::default()
                        }
                    } else {
                        AccessChange {
                            owner: Some(self.random.below(3) as u32 * 1000),
                            group: Some(self.random.below(3) as u32 * 100),
                            ..AccessChange::default()
                        }
                    };
                    let _ = node.set_access(&change);
                }
            }
            14 => {
                // Extended attributes, set and removed: of files and
                // folders, the only ones Linux gives user attributes.
                let path = file.clone().unwrap_or(dir.clone());
                let mut node = node_at(&self.src, path.as_bytes()).unwrap();
                if !matches!(node.kind(), Kind::File | Kind::Directory) {
                    node = at.clone();
                }
                let name = format!("user.{}", self.random.below(4));
                if self.random.below(3) == 0 {
                    let _ = node.remove_attribute(name.as_bytes());
                } else {
                    let value = noise(self.random.next(), self.random.below(300));
                    let _ = node.set_attribute(name.as_bytes(), &value, Setting::Either);
                }
            }
            15 => {
                let path = file.clone().unwrap_or(dir.clone());
                let node = node_at(&self.src, path.as_bytes()).unwrap();
                let _ = node.set_times(Some(ns), Some(ns + 7));
            }
            _ => {
                // A file made again where one was: often its inode number
                // used again, after a sync.
                if let Some(file) = &file {
                    let (parent, name) = parent_of(&self.src, file.as_bytes()).unwrap();
                    let name = name.to_string();
                    parent.remove(&name).unwrap();
                    self.fs.sync().unwrap();
                    if let Ok(node) = parent.create(&name, Kind::File, FILE) {
                        let data = self.data();
                        node.write(0, &Slice(&data)).unwrap();
                    }
                }
            }
        }
    }

    /// A read-only snapshot of `src`, as `name` at the top.
    fn snapshot(&self, name: &str) -> NodeRef {
        let root = self.fs.root();
        root.snapshot(name, &self.src, true).unwrap();
        root.lookup(name).unwrap()
    }
}

/// Removes `path` under `top`, and all it holds.
fn remove_all(top: &NodeRef, path: &str) {
    let node = node_at(top, path.as_bytes()).unwrap();
    if node.kind() == Kind::Directory {
        let mut names = Vec::new();
        node.list(0, &mut |_, name, _| {
            names.push(name.to_string());
            true
        })
        .unwrap();
        for name in names {
            remove_all(top, &format!("{path}/{name}"));
        }
    }
    let (parent, name) = parent_of(top, path.as_bytes()).unwrap();
    parent.remove(name).unwrap();
}

/// Checks that the image of `disk` passes `btrfs check`.
fn checks(fs: &BtrfsFs, disk: &radix_block::ram::RamDisk, what: &str) {
    fs.sync().unwrap();
    let image = image_of(
        disk,
        (radix_block::BlockDevice::info(disk).sectors * radix_block::SECTOR) as usize,
    );
    let (whole, text) = btrfs_check(&image);
    assert!(whole, "{what}: btrfs check failed:\n{text}");
}

#[test]
fn comparing_snapshots_finds_what_reading_both_whole_does() {
    let mut fs = Changing::new(0xc0ffee);
    fs.fill(600);
    for _ in 0..100 {
        fs.change();
    }
    let a = fs.snapshot("a");
    for _ in 0..150 {
        fs.change();
    }
    let b = fs.snapshot("b");
    // Both trees whole.
    let all = |node: &NodeRef| -> BTreeMap<Key, Vec<u8>> {
        let mut source = VfsSource { snapshot: node.clone(), parent: None, batch: usize::MAX };
        let mut out = Vec::new();
        let (from, to) = (Key::new(0, 0, 0), Key::new(u64::MAX, u8::MAX, u64::MAX));
        assert!(source.compare(Trees::New, &from, &to, &mut out).unwrap());
        out.into_iter().map(|item| (item.key, item.new.unwrap())).collect()
    };
    let (old, new) = (all(&a), all(&b));
    assert!(old.len() > 1000, "a tree of a few levels: {} items", old.len());
    let mut want = Vec::new();
    for (key, data) in &new {
        if old.get(key) != Some(data) {
            want.push(Item { key: *key, old: old.get(key).cloned(), new: Some(data.clone()) });
        }
    }
    for (key, data) in &old {
        if !new.contains_key(key) {
            want.push(Item { key: *key, old: Some(data.clone()), new: None });
        }
    }
    want.sort_by_key(|item| item.key);
    // Compared, a few at a time and all at once, and over a part.
    for batch in [1, 7, usize::MAX] {
        let mut source = VfsSource { snapshot: b.clone(), parent: Some(a.clone()), batch };
        let mut got = Vec::new();
        let mut from = Key::new(0, 0, 0);
        let to = Key::new(u64::MAX, u8::MAX, u64::MAX);
        loop {
            let first = got.len();
            let done = source.compare(Trees::Both, &from, &to, &mut got).unwrap();
            if done {
                break;
            }
            let last = got[first..].last().unwrap().key;
            from = Key::new(last.objectid, last.kind, last.offset + 1);
        }
        assert!(got == want, "batches of {batch}: {} items, {} wanted", got.len(), want.len());
    }
    let (from, to) = (Key::new(300, 0, 0), Key::new(320, 108, 1 << 20));
    let mut source = VfsSource { snapshot: b.clone(), parent: Some(a.clone()), batch: usize::MAX };
    let mut got = Vec::new();
    assert!(source.compare(Trees::Both, &from, &to, &mut got).unwrap());
    let part: Vec<Item> = want.iter().filter(|i| i.key >= from && i.key <= to).cloned().collect();
    assert!(got == part, "a part");
    // A snapshot that may change is refused.
    let src = fs.src.clone();
    let range = TreeRange {
        start: TreeKey { objectid: 0, kind: 0, offset: 0 },
        end: TreeKey { objectid: 1, kind: 0, offset: 0 },
    };
    assert_eq!(src.compare(None, &range, &mut |_| true), Err(Error::NotPermitted));
    assert_eq!(b.compare(Some(&src), &range, &mut |_| true), Err(Error::NotPermitted));
}

/// Sends `count` rounds of random changes from one filesystem to another,
/// whole and then each from the last, and checks every copy.
fn round_trips(seed: u64, rounds: usize, changes: usize, batch: usize) {
    let mut source = Changing::new(seed);
    source.fill(changes);
    let disk = writable(&mkfs_empty(&["--nodesize", "4096"]));
    let dest = mount_rw(&disk);
    let mut last: Option<NodeRef> = None;
    for round in 0..rounds {
        for _ in 0..if round == 0 { changes * 2 } else { changes } {
            source.change();
        }
        let name = format!("s{round}");
        let snapshot = source.snapshot(&name);
        let stream = send(&snapshot, last.as_ref(), &name, batch);
        receive_into(&dest.root(), &stream).unwrap_or_else(|e| {
            let mut dump = Vec::new();
            let _ = receive::dump_all(&stream[..], &mut dump);
            panic!("round {round}: receive: {e}\n{}", String::from_utf8_lossy(&dump));
        });
        linux_dumps(&stream);
        if let Ok(dir) = std::env::var("SEND_STREAMS_TO") {
            std::fs::write(format!("{dir}/{seed:x}-{round}.stream"), &stream).unwrap();
            std::fs::write(format!("{dir}/{seed:x}-{round}.txt"), listing(&snapshot)).unwrap();
        }
        let copy = dest.root().lookup(&name).unwrap();
        assert_same(&tree_of(&snapshot), &tree_of(&copy), &format!("seed {seed:#x} round {round}"));
        // The copy is the snapshot's, received, and read-only.
        let (info, sent) = (copy.subvolume().unwrap(), snapshot.subvolume().unwrap());
        assert_eq!(info.received_uuid, sent.uuid);
        assert_eq!(info.sent_transaction, sent.changed_transaction);
        assert_ne!(info.flags & radix_vfs::SUBVOLUME_READ_ONLY, 0);
        assert_eq!(copy.create("more", Kind::File, FILE).err(), Some(Error::ReadOnly));
        last = Some(snapshot);
    }
    checks(&source.fs, &source.disk, "source");
    checks(&dest, &disk, "copy");
}

#[test]
fn snapshots_sent_whole_and_as_changes_make_the_same_files() {
    round_trips(0x5e4d, 6, 40, 5);
}

#[test]
fn many_changes_at_once_make_the_same_files() {
    round_trips(0xbac1c, 3, 300, usize::MAX);
}

#[test]
fn copies_of_copies_have_their_parents() {
    // Sent from the source to a copy, and from the copy on: each copy
    // finds its parent by what it was received as.
    let mut source = Changing::new(0xcafe);
    let disks = [writable(&mkfs_empty(&[])), writable(&mkfs_empty(&[]))];
    let copies = [mount_rw(&disks[0]), mount_rw(&disks[1])];
    let mut last: Option<(NodeRef, NodeRef)> = None;
    for round in 0..3 {
        for _ in 0..60 {
            source.change();
        }
        let name = format!("s{round}");
        let snapshot = source.snapshot(&name);
        let stream = send(&snapshot, last.as_ref().map(|l| &l.0), &name, 64);
        receive_into(&copies[0].root(), &stream).unwrap();
        let first = copies[0].root().lookup(&name).unwrap();
        let stream = send(&first, last.as_ref().map(|l| &l.1), &name, 64);
        receive_into(&copies[1].root(), &stream).unwrap();
        let second = copies[1].root().lookup(&name).unwrap();
        assert_same(&tree_of(&snapshot), &tree_of(&second), &format!("round {round}"));
        assert_eq!(second.subvolume().unwrap().received_uuid, snapshot.subvolume().unwrap().uuid);
        last = Some((snapshot, first));
    }
}

#[test]
fn a_stream_from_a_parent_the_receiver_lacks_is_refused() {
    let mut source = Changing::new(0xdead);
    for _ in 0..50 {
        source.change();
    }
    let a = source.snapshot("a");
    source.change();
    let b = source.snapshot("b");
    let disk = writable(&mkfs_empty(&[]));
    let dest = mount_rw(&disk);
    let stream = send(&b, Some(&a), "b", 64);
    assert!(receive_into(&dest.root(), &stream).is_err());
    assert_eq!(dest.root().lookup("b").err(), Some(Error::NotFound));
}

#[test]
fn a_subvolume_made_after_a_received_one_is_removed_may_change() {
    // The removed subvolume's number is the next one's, as Linux numbers
    // them; nothing of the received one's being read-only passes to it.
    let mut source = Changing::new(0xbeef);
    for _ in 0..20 {
        source.change();
    }
    let snapshot = source.snapshot("s");
    let disk = writable(&mkfs_empty(&[]));
    let dest = mount_rw(&disk);
    receive_into(&dest.root(), &send(&snapshot, None, "s", 64)).unwrap();
    let received = dest.root().lookup("s").unwrap();
    let id = received.subvolume().unwrap().id;
    assert_eq!(received.create("new", Kind::File, FILE).err(), Some(Error::ReadOnly));
    drop(received);
    dest.root().remove_subvolume("s").unwrap();
    dest.root().make_subvolume("t").unwrap();
    let made = dest.root().lookup("t").unwrap();
    assert_eq!(made.subvolume().unwrap().id, id);
    made.create("new", Kind::File, FILE).unwrap();
}

/// Sends `snapshot` from `parent` into `dest`, and checks the copy.
fn sent_and_checked(
    dest: &NodeRef,
    snapshot: &NodeRef,
    parent: Option<&NodeRef>,
    name: &str,
) -> Vec<Command> {
    let stream = send(snapshot, parent, name, 2);
    receive_into(dest, &stream).unwrap();
    linux_dumps(&stream);
    assert_same(&tree_of(snapshot), &tree_of(&dest.lookup(name).unwrap()), name);
    read_all(&stream).unwrap()
}

/// `disk` synced and mounted again, from a copy of its image: a mount
/// looks its inode numbers up afresh, and uses a removed file's number
/// again if it was the highest.
fn remounted(
    fs: &BtrfsFs,
    disk: &radix_block::ram::RamDisk,
) -> (radix_vfs::mem::KArc<radix_block::ram::RamDisk>, radix_vfs::mem::KArc<BtrfsFs>) {
    fs.sync().unwrap();
    let image = image_of(
        disk,
        (radix_block::BlockDevice::info(disk).sectors * radix_block::SECTOR) as usize,
    );
    let disk = writable(&image);
    let fs = mount_rw(&disk);
    (disk, fs)
}

#[test]
fn numbers_used_again_are_other_files() {
    let source = Changing::new(1);
    let dest_disk = writable(&mkfs_empty(&[]));
    let dest = mount_rw(&dest_disk);
    let put = |dir: &NodeRef, name: &str, kind: Kind, data: &[u8]| -> NodeRef {
        let node = dir.create(name, kind, if kind == Kind::File { FILE } else { DIR }).unwrap();
        if kind == Kind::File {
            node.write(0, &Slice(data)).unwrap();
        }
        node
    };
    let number = |node: &NodeRef| node.stat().id;
    let snapshot = |fs: &BtrfsFs, name: &str| {
        let root = fs.root();
        root.snapshot(name, &root.lookup("src").unwrap(), true).unwrap();
        root.lookup(name).unwrap()
    };
    let src = source.src.clone();
    put(&src, "q", Kind::Directory, b"");
    put(&src, "c", Kind::File, b"the file that stays");
    let p = put(&src, "p", Kind::Directory, b"");
    src.rename("c", &*p, "c").unwrap();
    let old = number(&p);
    snapshot(&source.fs, "s0");
    // The folder goes and another takes its number, with the same file
    // under the same name: the file's reference to it is the same item.
    p.rename("c", &*src, "c").unwrap();
    drop(p);
    src.remove("p").unwrap();
    drop(src);
    let (disk, fs) = remounted(&source.fs, &source.disk);
    let s0 = fs.root().lookup("s0").unwrap();
    sent_and_checked(&dest.root(), &s0, None, "s0");
    let src = fs.root().lookup("src").unwrap();
    let p = put(&src, "p", Kind::Directory, b"");
    assert_eq!(number(&p), old, "the number is used again");
    src.rename("c", &*p, "c").unwrap();
    let s1 = snapshot(&fs, "s1");
    let commands = sent_and_checked(&dest.root(), &s1, Some(&s0), "s1");
    assert!(commands.iter().any(|c| matches!(c, Command::Rmdir { .. })), "{commands:?}");
    assert!(
        commands.iter().any(|c| matches!(c, Command::MkDir { path, .. } if path == b"p")),
        "{commands:?}"
    );
    // A folder's number taken by a file.
    p.rename("c", &*src, "c").unwrap();
    drop(p);
    src.remove("p").unwrap();
    drop((src, s0));
    let (disk, fs) = remounted(&fs, &disk);
    let src = fs.root().lookup("src").unwrap();
    let f = put(&src, "p", Kind::File, b"a file now");
    assert_eq!(number(&f), old);
    let s1 = fs.root().lookup("s1").unwrap();
    let s2 = snapshot(&fs, "s2");
    sent_and_checked(&dest.root(), &s2, Some(&s1), "s2");
    // And a file's by a folder that holds a file that was elsewhere.
    drop(f);
    src.remove("p").unwrap();
    drop((src, s1, s2));
    let (_disk, fs) = remounted(&fs, &disk);
    let src = fs.root().lookup("src").unwrap();
    let d = put(&src, "x", Kind::Directory, b"");
    assert_eq!(number(&d), old);
    put(&d, "inside", Kind::File, b"new");
    src.rename("c", &*d, "c").unwrap();
    let s2 = fs.root().lookup("s2").unwrap();
    let s3 = snapshot(&fs, "s3");
    sent_and_checked(&dest.root(), &s3, Some(&s2), "s3");
}

#[test]
fn names_that_change_places_all_at_once() {
    // Folders that swap names, a folder moved under what was its own
    // child, files that swap names, and a name a folder leaves taken by a
    // file.
    let source = Changing::new(2);
    let src = &source.src;
    let disk = writable(&mkfs_empty(&[]));
    let dest = mount_rw(&disk);
    let mkdir = |dir: &NodeRef, name: &str| dir.create(name, Kind::Directory, DIR).unwrap();
    let file = |dir: &NodeRef, name: &str, data: &str| {
        dir.create(name, Kind::File, FILE).unwrap().write(0, &Slice(data.as_bytes())).unwrap();
    };
    let a = mkdir(src, "a");
    let b = mkdir(&a, "b");
    let c = mkdir(&b, "c");
    file(&c, "deep", "deep");
    let x = mkdir(src, "x");
    let y = mkdir(src, "y");
    file(&x, "in x", "x");
    file(&y, "in y", "y");
    file(src, "one", "1");
    file(src, "two", "2");
    mkdir(src, "dir to file");
    let s0 = source.snapshot("s0");
    sent_and_checked(&dest.root(), &s0, None, "s0");
    // a/b/c becomes c/b/a.
    b.rename("c", &**src, "c").unwrap();
    a.rename("b", &*c, "b").unwrap();
    src.rename("a", &*b, "a").unwrap();
    // x and y swap.
    src.rename("x", &**src, "t").unwrap();
    src.rename("y", &**src, "x").unwrap();
    src.rename("t", &**src, "y").unwrap();
    // one and two swap.
    src.rename("one", &**src, "t").unwrap();
    src.rename("two", &**src, "one").unwrap();
    src.rename("t", &**src, "two").unwrap();
    src.remove("dir to file").unwrap();
    file(src, "dir to file", "a file");
    // A file whose names all change, and one with another name.
    src.rename("one", &*x, "one").unwrap();
    let two = src.lookup("two").unwrap();
    y.link("also two", &two).unwrap();
    let s1 = source.snapshot("s1");
    sent_and_checked(&dest.root(), &s1, Some(&s0), "s1");
    drop((a, b, c, x, y, two));
}

/// The list of every file under `top`, as `ob fixture btrfs-send` writes
/// it of Linux's snapshots.
fn listing(top: &NodeRef) -> String {
    let mut out = String::new();
    for (path, entry) in tree_of(top) {
        let hex = |bytes: &[u8]| {
            bytes.iter().fold(String::new(), |mut text, b| {
                let _ = write!(text, "{b:02x}");
                text
            })
        };
        let (kind, what) = match &entry.what {
            What::Dir => ("dir", "-".to_string()),
            What::File(data) => ("file", hex(&crate::sha256::hash(data))),
            What::Link(target) => ("link", target.clone()),
            What::Node(kind, device) => {
                let (major, minor) = radix_vfs::device_parts(*device);
                match kind {
                    Kind::Pipe => ("fifo", "-".to_string()),
                    Kind::Socket => ("sock", "-".to_string()),
                    Kind::BlockDevice => ("blk", format!("{major}:{minor}")),
                    _ => ("chr", format!("{major}:{minor}")),
                }
            }
        };
        let xattrs: Vec<String> = entry
            .xattrs
            .iter()
            .map(|(n, v)| format!("{}={}", String::from_utf8_lossy(n), hex(v)))
            .collect();
        let xattrs = if xattrs.is_empty() { "-".to_string() } else { xattrs.join(",") };
        let shown = |p: &str| if p.is_empty() { ".".to_string() } else { p.to_string() };
        out.push_str(&format!(
            "{}\t{kind}\t{:o}\t{}\t{}\t{}\t{}\t{what}\t{xattrs}\t{}\n",
            shown(&path),
            entry.access.mode,
            entry.access.owner,
            entry.access.group,
            entry.accessed,
            entry.modified,
            shown(&entry.first),
        ));
    }
    out
}

/// Checks the copy at `top` against the list of the snapshot Linux sent.
fn matches_list(top: &NodeRef, list: &str, what: &str) {
    let got = listing(top);
    for (want, got) in list.lines().zip(got.lines()) {
        assert_eq!(got, want, "{what}");
    }
    assert_eq!(got.lines().count(), list.lines().count(), "{what}:\n{got}");
}

#[test]
fn streams_linux_sent_make_the_files_linux_had() {
    let full = include_bytes!("fixtures/btrfs-send-full.stream");
    let inc = include_bytes!("fixtures/btrfs-send-inc.stream");
    let disk = writable(&mkfs_empty(&[]));
    let dest = mount_rw(&disk);
    // Linux sends a file cloned from another as a clone.
    let commands = read_all(full).unwrap();
    assert!(commands.iter().any(|c| matches!(c, Command::Clone { .. })));
    receive_into(&dest.root(), full).unwrap();
    let s1 = dest.root().lookup("s1").unwrap();
    matches_list(&s1, include_str!("fixtures/btrfs-send-s1.txt"), "s1");
    receive_into(&dest.root(), inc).unwrap();
    let s2 = dest.root().lookup("s2").unwrap();
    matches_list(&s2, include_str!("fixtures/btrfs-send-s2.txt"), "s2");
    // A stream with the first end only, as `btrfs receive -e` takes it.
    let both: Vec<u8> = full.iter().chain(inc.iter()).copied().collect();
    let other = writable(&mkfs_empty(&[]));
    let other = mount_rw(&other);
    let options = Options { stop_at_end: true };
    let mut target = VfsTarget { dest: other.root(), top: None };
    let received = receive::receive(&both[..], &mut target, options).unwrap();
    assert_eq!(received.subvolumes, vec![b"s1".to_vec()]);
    // Sent on from the copies: what Linux sent, Dukovo sends the same.
    let third = writable(&mkfs_empty(&[]));
    let third = mount_rw(&third);
    receive_into(&third.root(), &send(&s1, None, "s1", 16)).unwrap();
    receive_into(&third.root(), &send(&s2, Some(&s1), "s2", 16)).unwrap();
    matches_list(
        &third.root().lookup("s2").unwrap(),
        include_str!("fixtures/btrfs-send-s2.txt"),
        "s2 again",
    );
    checks(&dest, &disk, "Linux's streams received");
}

/// Rounds of changes from many seeds, a few changes a round: `SEND_SEEDS`
/// says how many seeds (8 by default), `SEND_STREAMS_TO` a folder to
/// keep the streams and the lists of their snapshots in, for a Linux
/// receiver to check (ADR 0462 says how).
#[test]
fn rounds_from_many_seeds_make_the_same_files() {
    let seeds: u64 = std::env::var("SEND_SEEDS").ok().and_then(|s| s.parse().ok()).unwrap_or(8);
    for seed in 1..=seeds {
        round_trips(seed * 7919, 6, 30, 3);
    }
}
