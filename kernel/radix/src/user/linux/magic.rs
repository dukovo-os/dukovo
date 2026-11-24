// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The links in /proc that the kernel answers for a process's own (ADR
//! 0620): `/proc/self/exe`, `cwd`, `root` and `fd/<n>`, the same beneath
//! `/proc/thread-self` and `/proc/<its own number>`, and `/dev/fd/<n>`,
//! `/dev/stdin`, `/dev/stdout` and `/dev/stderr`.
//!
//! linuxd serves `/proc` in a Linux view, with these as symbolic links to
//! paths; but a descriptor's object need not have a path (a pipe, a
//! socket, a file removed since), and a path may lead elsewhere in the
//! view than the object the link stands for. So the table looks at a
//! path before any walk: one of these is the kernel's. `readlink` gives
//! the link's target as Linux does ([`target`]); a call that follows a
//! last link (`open`, `stat`, `access`, `execve`) goes where it leads
//! ([`lead`]): the program's file, the working directory or the root by
//! their paths, and a descriptor as the descriptor itself, so that
//! `/dev/stdin` and bash's `/dev/fd/63` reach a pipe. A link of another
//! process's, and one met part-way through a walk, are linuxd's.

use alloc::vec::Vec;

use radix_abi::startup::SLOT_FD_BASE;
use radix_linux::errno::{EFAULT, ENOENT, ENOMEM};
use radix_linux::magic::{Magic, parse};
use radix_mm::Kmalloc;

use super::{Error, Result, fail, kmalloc};
use crate::user::UserRef;
use crate::user::objects::{self, Object};

/// Where a call that follows a last link goes for one of [`Magic`]'s.
pub enum Lead {
    /// To the path from the root it is a link to.
    Path(Vec<u8, Kmalloc>),
    /// To the descriptor itself.
    Descriptor(u64),
}

/// Where `path` leads `user`'s call that follows a last link, if it names
/// one of [`Magic`]'s: ENOENT for a program whose path is unknown, as for
/// a descriptor that is not open, which the call finds out.
pub fn lead(user: &UserRef, path: &[u8]) -> Result<Option<Lead>> {
    let Some(magic) = parse(path, user.process.number()) else { return Ok(None) };
    Ok(Some(match magic {
        Magic::Fd(fd) => Lead::Descriptor(u64::from(fd)),
        Magic::Exe => Lead::Path(program(user)?),
        Magic::Cwd => Lead::Path(directory(user)?),
        Magic::Root => Lead::Path(copied(b"/")?),
    }))
}

/// What `readlink` gives for `path`, if it names one of [`Magic`]'s: the
/// program's path, the working directory, `/`, or for a descriptor the
/// path it was opened by (`/memfd:NAME (deleted)` for a memory file),
/// `pipe:[n]`, `socket:[n]` or `anon_inode:[kind]`.
pub fn target(user: &UserRef, path: &[u8]) -> Result<Option<Vec<u8, Kmalloc>>> {
    let Some(magic) = parse(path, user.process.number()) else { return Ok(None) };
    Ok(Some(match magic {
        Magic::Exe => program(user)?,
        Magic::Cwd => directory(user)?,
        Magic::Root => copied(b"/")?,
        Magic::Fd(fd) => descriptor(user, fd)?,
    }))
}

/// The path of the program `user`'s process runs (its interpreter, for a
/// script), as `exec` was given it, from the root.
fn program(user: &UserRef) -> Result<Vec<u8, Kmalloc>> {
    let image = user.process.posix.lock().image.clone();
    match image.as_deref().and_then(|opened| opened.path()) {
        Some((path, false)) if !path.is_empty() => absolute(path),
        _ => fail(ENOENT),
    }
}

/// The working directory of `user`'s process, from the root.
fn directory(user: &UserRef) -> Result<Vec<u8, Kmalloc>> {
    let cwd = user.process.posix.lock().directory.clone();
    match cwd.as_deref() {
        Some(path) => absolute(path),
        None => copied(b"/"),
    }
}

/// Descriptor `fd`'s link target, as Linux's /proc gives it.
fn descriptor(user: &UserRef, fd: u32) -> Result<Vec<u8, Kmalloc>> {
    let slot = SLOT_FD_BASE.checked_add(fd).ok_or(Error::Errno(ENOENT))?;
    let cap = objects::capability(user.cspace, slot).map_err(|_| Error::Errno(ENOENT))?;
    let file = match objects::object_of(&cap) {
        Ok(Object::File(file) | Object::Directory(file)) => file,
        Ok(Object::EventQueue(_)) => return copied(b"anon_inode:[eventpoll]"),
        _ => return anon(cap.kind().name().as_bytes()),
    };
    let node = &file.at.node;
    let id = node.stat().id;
    if crate::fs::pipe::end_of(node).is_some() {
        return bracketed(b"pipe:[", id);
    }
    if crate::fs::socket::socket_of(node).is_some() || crate::fs::inet::socket_of(node).is_some() {
        return bracketed(b"socket:[", id);
    }
    // The kernel's own files (ADR 0613), by Linux's names for them.
    if crate::fs::counter::counter_of(node).is_some() {
        return copied(b"anon_inode:[eventfd]");
    }
    if crate::fs::timer_file::timer_of(node).is_some() {
        return copied(b"anon_inode:[timerfd]");
    }
    if crate::fs::signal_file::signal_file_of(node).is_some() {
        return copied(b"anon_inode:[signalfd]");
    }
    if crate::fs::process_file::process_file_of(node).is_some() {
        return copied(b"anon_inode:[pidfd]");
    }
    if crate::fs::watch_file::watch_file_of(node).is_some() {
        return copied(b"anon_inode:inotify");
    }
    match file.opened.path() {
        Some((path, false)) if !path.is_empty() => absolute(path),
        _ => anon(file.at.mount.fs.kind().name().as_bytes()),
    }
}

/// `anon_inode:[name]`.
fn anon(name: &[u8]) -> Result<Vec<u8, Kmalloc>> {
    let mut out = copied(b"anon_inode:[")?;
    extend(&mut out, name)?;
    extend(&mut out, b"]")?;
    Ok(out)
}

/// `prefix`, then `id` in decimal and `]`: `pipe:[1234]`.
fn bracketed(prefix: &[u8], id: u64) -> Result<Vec<u8, Kmalloc>> {
    let mut digits = [0u8; 20];
    let mut at = digits.len();
    let mut n = id;
    loop {
        at -= 1;
        digits[at] = b'0' + (n % 10) as u8;
        n /= 10;
        if n == 0 {
            break;
        }
    }
    let mut out = copied(prefix)?;
    extend(&mut out, &digits[at..])?;
    extend(&mut out, b"]")?;
    Ok(out)
}

/// `path`, a path from the root with or without its leading slash, with
/// one.
fn absolute(path: &[u8]) -> Result<Vec<u8, Kmalloc>> {
    let start = path.iter().position(|&b| b != b'/').unwrap_or(path.len());
    let mut out = copied(b"/")?;
    extend(&mut out, &path[start..])?;
    Ok(out)
}

/// `bytes`, in the kernel's memory.
fn copied(bytes: &[u8]) -> Result<Vec<u8, Kmalloc>> {
    let mut out = Vec::new_in(kmalloc());
    extend(&mut out, bytes)?;
    Ok(out)
}

/// Adds `bytes` to `out`, or fails with ENOMEM.
fn extend(out: &mut Vec<u8, Kmalloc>, bytes: &[u8]) -> Result<()> {
    out.try_reserve(bytes.len()).map_err(|_| Error::Errno(ENOMEM))?;
    for &b in bytes {
        let _ = out.push_within_capacity(b);
    }
    Ok(())
}

/// Copies what `readlink` gives for a link, `target`, to `buffer` of
/// `size` bytes in the program's memory, cut to fit and with no NUL, as
/// Linux's `readlink`; how many bytes it put.
pub fn put_target(target: &[u8], buffer: u64, size: u64) -> Result<u64> {
    let len = target.len().min(usize::try_from(size).unwrap_or(usize::MAX));
    if !radix_ipc::range::user_bytes(buffer, len as u64) {
        return fail(EFAULT);
    }
    super::copy_out(buffer, &target[..len])?;
    Ok(len as u64)
}
