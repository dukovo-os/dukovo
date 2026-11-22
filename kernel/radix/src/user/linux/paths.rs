// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Where a Linux program's paths lead (ADR 0611): the directory capability
//! a directory method resolves a path beneath, and the path there.
//!
//! An absolute path is resolved beneath the process's root (SLOT_ROOT)
//! without its leading slashes; a relative one beneath the root too, as
//! the working directory and the path joined, since the working directory
//! the kernel keeps is a path from the root with no links or `..` in it
//! (`set_directory`), and a walk from the root takes `..` and an absolute
//! link's target as Linux does. A path relative to a directory descriptor
//! is resolved beneath that directory, whose capability ends a walk that
//! would leave it (a `..` above it, or a link to an absolute path) with
//! EXDEV; the call then goes again from the root, by the path the
//! directory was opened by ([`from_root`]), as Dukovo's C library does.

use alloc::vec::Vec;

use radix_abi::address_space::PAGE_SIZE;
use radix_abi::startup::{SLOT_FD_BASE, SLOT_ROOT};
use radix_hal::UserMode;
use radix_linux::errno::{EBADF, EFAULT, ENAMETOOLONG, ENOENT, ENOMEM, EXDEV};
use radix_mm::Kmalloc;
use radix_vfs::PATH_MAX;

use super::{Result, fail, kmalloc};
use crate::Arch;
use crate::user::UserRef;
use crate::user::objects::{self, Object};

/// The `dirfd` that means the working directory.
pub const AT_FDCWD: u64 = -100i64 as u64;

/// The slot of descriptor `fd`, a Linux `int`, or EBADF.
pub fn slot_of(fd: u64) -> Result<u32> {
    let fd = fd as i32;
    match u32::try_from(fd) {
        Ok(fd) if fd < radix_abi::process::MAX_SLOTS - SLOT_FD_BASE => Ok(SLOT_FD_BASE + fd),
        _ => fail(EBADF),
    }
}

/// Copies the NUL-terminated string at `address` in the caller's memory,
/// without its NUL: at most `max` bytes before it, or ENAMETOOLONG. Read
/// a page at a time, so that a string that ends just before memory the
/// program cannot read is read whole.
pub fn read_string(address: u64, max: usize) -> Result<Vec<u8, Kmalloc>> {
    let mut bytes = Vec::new_in(kmalloc());
    let mut at = address;
    loop {
        let room = (max + 1).saturating_sub(bytes.len());
        if room == 0 {
            return fail(ENAMETOOLONG);
        }
        let in_page = (PAGE_SIZE - at % PAGE_SIZE) as usize;
        let n = in_page.min(room).min(256);
        let start = bytes.len();
        let mut chunk = [0u8; 256];
        if !Arch::copy_from_user(&mut chunk[..n], at) {
            return fail(EFAULT);
        }
        radix_mm::bytes::extend(&mut bytes, &chunk[..n])
            .map_err(|_| super::Error::Errno(ENOMEM))?;
        if let Some(nul) = bytes[start..].iter().position(|&b| b == 0) {
            bytes.truncate(start + nul);
            return Ok(bytes);
        }
        at += n as u64;
    }
}

/// A path a Linux call named, as it named it, without the NUL.
pub fn read(address: u64) -> Result<Vec<u8, Kmalloc>> {
    // PATH_MAX counts the NUL.
    read_string(address, PATH_MAX - 1)
}

/// Where a directory method resolves a Linux path: beneath the
/// directory in `slot`, by `path`.
pub struct Place {
    pub slot: u32,
    pub path: Vec<u8, Kmalloc>,
    /// Whether it is beneath a directory descriptor, which [`from_root`]
    /// may go round.
    pub beneath_descriptor: bool,
}

/// Where `path`, relative to `dirfd` (a descriptor or [`AT_FDCWD`]),
/// leads for `user`'s process. An empty path is ENOENT unless `empty`
/// (AT_EMPTY_PATH), when it names the directory or file `dirfd` holds
/// itself, as `.`.
pub fn place(user: &UserRef, dirfd: u64, path: &[u8], empty: bool) -> Result<Place> {
    if path.is_empty() && !empty {
        return fail(ENOENT);
    }
    if let [b'/', ..] = path {
        let start = path.iter().position(|&b| b != b'/').unwrap_or(path.len());
        return Ok(Place {
            slot: SLOT_ROOT,
            path: own_or_dot(&path[start..])?,
            beneath_descriptor: false,
        });
    }
    if dirfd as i32 != AT_FDCWD as i32 {
        let slot = slot_of(dirfd)?;
        return Ok(Place { slot, path: own_or_dot(path)?, beneath_descriptor: true });
    }
    let cwd = user.process.posix.lock().directory.clone();
    let cwd = cwd.as_deref().map_or(&[][..], |cwd| &cwd[..]);
    Ok(Place { slot: SLOT_ROOT, path: joined(cwd, path)?, beneath_descriptor: false })
}

/// `path` beneath the root by the directory path `directory` (from the
/// root, with or without its leading slash): the two joined by a `/`.
/// `.` stands for the directory itself in either.
pub fn joined(directory: &[u8], path: &[u8]) -> Result<Vec<u8, Kmalloc>> {
    let start = directory.iter().position(|&b| b != b'/').unwrap_or(directory.len());
    let directory = match &directory[start..] {
        b"." => &[][..],
        directory => directory,
    };
    let path = if path == b"." { &[][..] } else { path };
    let need =
        directory.len() + usize::from(!directory.is_empty() && !path.is_empty()) + path.len();
    if need >= PATH_MAX {
        return fail(ENAMETOOLONG);
    }
    let mut joined =
        radix_mm::bytes::zeroed(need.max(1), kmalloc()).map_err(|_| super::Error::Errno(ENOMEM))?;
    if need == 0 {
        joined[0] = b'.';
        return Ok(joined);
    }
    joined[..directory.len()].copy_from_slice(directory);
    joined[need - path.len()..].copy_from_slice(path);
    if !directory.is_empty() && !path.is_empty() {
        joined[directory.len()] = b'/';
    }
    Ok(joined)
}

/// `path` in the kernel's memory, or `.` for an empty one.
fn own_or_dot(path: &[u8]) -> Result<Vec<u8, Kmalloc>> {
    joined(b"", path)
}

/// `place`, beneath a directory descriptor, whose walk left the
/// directory: the same path from the root, joined to the path the
/// directory was opened by, if the kernel knows all of it; EXDEV if not.
pub fn from_root(user: &UserRef, place: &Place) -> Result<Place> {
    let cap = objects::capability(user.cspace, place.slot).map_err(super::Error::from)?;
    let Ok(Object::Directory(directory)) = objects::object_of(&cap) else { return fail(EXDEV) };
    match directory.opened.path() {
        Some((opened, false)) if !opened.is_empty() => Ok(Place {
            slot: SLOT_ROOT,
            path: joined(opened, &place.path)?,
            beneath_descriptor: false,
        }),
        _ => fail(EXDEV),
    }
}
