// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The Btrfs log of `ob fixture btrfs-log` (ADR 0151). As process 1 of a
//! Linux virtual machine whose one disk is a new Btrfs, `--btrfs-log
//! make` writes files, commits them, then changes them with an fsync
//! after each change and powers the machine off, so that the changes are
//! in Btrfs's log tree alone. `--btrfs-log list` lists the disk as its
//! last commit has it, then as Linux has it after replaying the log:
//!
//! ```text
//! LIST <path> <kind> <mode> <uid> <gid> <links> <size> <what> <xattrs>
//! ```
//!
//! `what` is a file's FNV-1a hash, a link's target, or `-`; `xattrs` the
//! extended attributes, `name=hex` in order, or `-`.

use std::fmt::Write as _;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{FileExt, MetadataExt};
use std::path::Path;

use sysapi::fs::{self, Link};
use sysapi::linux;

/// The disk, the virtual machine's only one, and where it is mounted.
const DISK: &str = "/dev/vda";
const AT: &str = "/mnt/log";

/// The inode flag of a file Btrfs compresses (`chattr +c`).
const FS_COMPR_FL: libc::c_long = 0x4;

/// `len` bytes that do not repeat, the xorshift run of `seed`.
fn noise(seed: u64, len: usize) -> Vec<u8> {
    let mut state = 0x9e37_79b9_7f4a_7c15 ^ seed.wrapping_mul(0xbf58_476d_1ce4_e5b9);
    (0..len)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            (state >> 32) as u8
        })
        .collect()
}

/// `len` bytes of text that compress.
fn text(len: usize) -> Vec<u8> {
    b"The log holds what an fsync made durable since the last commit. "
        .iter()
        .copied()
        .cycle()
        .take(len)
        .collect()
}

/// Mounts the disk on [`AT`] with `flags` and the options `options`.
fn mount(flags: libc::c_ulong, options: &str) {
    let _ = std::fs::create_dir_all(AT);
    if let Err(err) = linux::mount(DISK, Path::new(AT), "btrfs", flags, options) {
        fail(&format!("cannot mount the disk: {err}"));
    }
}

/// Unmounts [`AT`], which commits what is open.
fn unmount() {
    if let Err(err) = linux::unmount(Path::new(AT)) {
        fail(&format!("cannot unmount the disk: {err}"));
    }
}

/// Says why the log could not be made or listed; `ob fixture` waits for
/// the line that says it was.
fn fail(why: &str) -> ! {
    println!("linux-bench: {why}");
    std::process::exit(1);
}

/// `name` beneath [`AT`].
fn path(name: &str) -> String {
    format!("{AT}/{name}")
}

/// A new file `name` holding `bytes`.
fn put(name: &str, bytes: &[u8]) -> File {
    let mut file = File::create(path(name)).unwrap_or_else(|e| fail(&format!("{name}: {e}")));
    file.write_all(bytes).unwrap_or_else(|e| fail(&format!("writing {name}: {e}")));
    file
}

/// Makes what `file` (`name`) holds durable: with Btrfs's log, until the
/// next commit.
fn fsync(file: &File, name: &str) {
    file.sync_all().unwrap_or_else(|e| fail(&format!("fsync of {name}: {e}")));
}

/// Opens `name`, a file or a directory, and makes it durable.
fn fsync_path(name: &str) {
    let file = File::open(path(name)).unwrap_or_else(|e| fail(&format!("{name}: {e}")));
    fsync(&file, name);
}

/// Sets the extended attribute `key` of `name` to `value`, or removes it.
fn xattr(name: &str, key: &str, value: Option<&[u8]>) {
    let file = path(name);
    let file = Path::new(&file);
    let result = match value {
        Some(value) => fs::set_xattr(file, key, value),
        None => fs::remove_xattr(file, key),
    };
    if let Err(err) = result {
        fail(&format!("xattr of {name}: {err}"));
    }
}

/// `--btrfs-log make`.
pub fn make() {
    // No commit of its own before the machine goes off.
    mount(0, "commit=300");
    // What the last commit holds.
    put("kept", &noise(1, 64 << 10));
    put("shrinks", &noise(2, 40 << 10));
    put("renamed", &noise(3, 8 << 10));
    for dir in ["d", "into"] {
        std::fs::create_dir(path(dir)).unwrap_or_else(|e| fail(&format!("{dir}: {e}")));
    }
    put("d/gone", &noise(4, 3000));
    put("d/stays", &noise(5, 3000));
    put("attrs", b"a file with extended attributes\n");
    xattr("attrs", "user.kept", Some(b"one"));
    xattr("attrs", "user.changed", Some(b"old"));
    xattr("attrs", "user.removed", Some(b"gone"));
    linux::sync();

    // A file moved to another directory and made durable: Linux 6.8
    // commits the transaction for it, rather than log it, so it comes
    // first.
    std::fs::rename(path("renamed"), path("into/moved"))
        .unwrap_or_else(|e| fail(&format!("renaming: {e}")));
    fsync_path("into/moved");

    // What the log holds: each change, made durable by an fsync.
    let new = put("new", &noise(6, 40 << 10));
    fsync(&new, "new");
    let kept = OpenOptions::new()
        .write(true)
        .open(path("kept"))
        .unwrap_or_else(|e| fail(&format!("kept: {e}")));
    let writes = kept
        .write_all_at(&noise(7, 8192), 20480)
        .and(kept.write_all_at(&noise(8, 30 << 10), 64 << 10));
    writes.unwrap_or_else(|e| fail(&format!("writing kept: {e}")));
    fsync(&kept, "kept");
    let shrinks = OpenOptions::new()
        .write(true)
        .open(path("shrinks"))
        .unwrap_or_else(|e| fail(&format!("shrinks: {e}")));
    shrinks.set_len(5000).unwrap_or_else(|e| fail(&format!("cutting shrinks: {e}")));
    fsync(&shrinks, "shrinks");
    std::fs::remove_file(path("d/gone")).unwrap_or_else(|e| fail(&format!("d/gone: {e}")));
    fsync_path("d");
    std::fs::hard_link(path("kept"), path("d/link"))
        .unwrap_or_else(|e| fail(&format!("linking: {e}")));
    fsync_path("d/link");
    std::fs::create_dir(path("newdir")).unwrap_or_else(|e| fail(&format!("newdir: {e}")));
    let small = put("newdir/small", b"a file small enough to be inline\n");
    fsync(&small, "newdir/small");
    let other = put("newdir/other", &noise(9, 5000));
    fsync(&other, "newdir/other");
    fsync_path("newdir");
    xattr("attrs", "user.changed", Some(b"new"));
    xattr("attrs", "user.removed", None);
    xattr("attrs", "user.added", Some(b"added"));
    fsync_path("attrs");
    let zipped = put("zipped", b"");
    let flags = FS_COMPR_FL;
    if let Err(err) = linux::set_inode_flags(&zipped, flags) {
        fail(&format!("chattr +c: {err}"));
    }
    (&zipped).write_all(&text(200 << 10)).unwrap_or_else(|e| fail(&format!("zipped: {e}")));
    fsync(&zipped, "zipped");
    std::os::unix::fs::symlink("kept", path("pointer"))
        .unwrap_or_else(|e| fail(&format!("symlink: {e}")));
    fsync_path(".");
    // Written after the last fsync: gone after the replay.
    put("unsynced", &noise(10, 4096));
    println!("linux-bench: logged");
}

/// `--btrfs-log list`.
pub fn list() {
    mount(libc::MS_RDONLY, "rescue=nologreplay");
    println!("LIST committed");
    walk(Path::new(AT), "");
    unmount();
    mount(0, "");
    println!("LIST replayed");
    walk(Path::new(AT), "");
    unmount();
    println!("LIST end");
}

/// Prints `dir`, `rel` beneath the disk's top, and what is beneath it, in
/// the order of their names.
fn walk(dir: &Path, rel: &str) {
    line(dir, if rel.is_empty() { "/" } else { rel });
    let Ok(entries) = std::fs::read_dir(dir) else { fail(&format!("listing {rel}")) };
    let mut names: Vec<_> = entries.filter_map(|e| Some(e.ok()?.file_name())).collect();
    names.sort();
    for name in names {
        let child = dir.join(&name);
        let rel = format!("{rel}/{}", name.to_string_lossy());
        if std::fs::symlink_metadata(&child).is_ok_and(|m| m.is_dir()) {
            walk(&child, &rel);
        } else {
            line(&child, &rel);
        }
    }
}

/// FNV-1a, 64 bits.
fn fnv(bytes: &[u8]) -> u64 {
    bytes
        .iter()
        .fold(0xcbf2_9ce4_8422_2325, |h, &b| (h ^ u64::from(b)).wrapping_mul(0x100_0000_01b3))
}

/// The line of `at`, `rel` beneath the disk's top.
fn line(at: &Path, rel: &str) {
    let Ok(meta) = std::fs::symlink_metadata(at) else { fail(&format!("status of {rel}")) };
    let (kind, what) = if meta.is_dir() {
        ("dir", "-".to_string())
    } else if meta.is_symlink() {
        let target = std::fs::read_link(at).unwrap_or_default();
        ("link", target.to_string_lossy().into_owned())
    } else {
        let bytes = std::fs::read(at).unwrap_or_else(|e| fail(&format!("reading {rel}: {e}")));
        ("file", format!("{:016x}", fnv(&bytes)))
    };
    println!(
        "LIST {rel} {kind} {:o} {} {} {} {} {what} {}",
        meta.mode() & 0o7777,
        meta.uid(),
        meta.gid(),
        meta.nlink(),
        meta.size(),
        xattrs(at)
    );
}

/// The extended attributes of `at`, `name=hex` in the order of their
/// names, or `-`.
fn xattrs(at: &Path) -> String {
    let Ok(names) = fs::xattr_names(at, Link::Itself) else { return "-".into() };
    let mut out: Vec<String> = Vec::new();
    for name in names {
        let Ok(value) = fs::xattr(at, &name, Link::Itself) else { continue };
        let mut entry = format!("{}=", name.to_string_lossy());
        for byte in &value {
            let _ = write!(entry, "{byte:02x}");
        }
        out.push(entry);
    }
    out.sort();
    if out.is_empty() { "-".into() } else { out.join(",") }
}
