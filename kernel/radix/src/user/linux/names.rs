// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The calls a Linux program makes on a name (ADR 0611): checking access,
//! making, removing, renaming and linking files and directories, reading
//! links, and changing a file's mode, owner and times.
//!
//! Each call is the one Dukovo's C library makes for the same POSIX
//! function (ports/mlibc/sysdeps/radix/generic/files.cpp): the path goes
//! through `at` (`at2` for the two of `rename` and `link`), which resolves
//! it as `paths` says and goes again from the root when a walk beneath a
//! directory descriptor left it, and ends in a method of the directory the
//! path is beneath, with the path in the kernel's memory
//! (`Paths::Given`). The old calls (`rename`, `mkdir`, `chmod`) are the
//! `at` ones with AT_FDCWD.

use radix_abi::UserBuffer;
use radix_abi::directory::{self, FileKind, FileStatus, LinkFlags, PathFlags};
use radix_abi::file;
use radix_linux::errno::{self, EFAULT, EINVAL, EISDIR, ENOENT, ENOTDIR, EPERM};
use radix_linux::flags;
use radix_linux::records::{self, SetTime, TIMESPEC_SIZE};

use super::files::{self, at, at2, directory, given};
use super::paths;
use super::{Error, Result, copy_in, fail, method};
use crate::sched::ThreadRef;
use crate::user::UserRef;
use crate::user::methods::Answer;

/// AT_REMOVEDIR, which `rmdir` is `unlinkat` with.
pub const REMOVE_DIRECTORY: u64 = flags::AT_REMOVEDIR as u64;

/// `access`, `faccessat` and `faccessat2`: the native `check_access` of the
/// directory the path is beneath, for what `mode` asks (R_OK, W_OK and
/// X_OK). `how` is the `faccessat2` flags: AT_SYMLINK_NOFOLLOW, and
/// AT_EMPTY_PATH for the directory `dirfd` itself; AT_EACCESS changes
/// nothing, as a process's real and effective IDs are the same.
pub fn faccessat(user: &UserRef, dirfd: u64, address: u64, mode: u64, how: u64) -> Result<u64> {
    let how = how as u32;
    let known = flags::AT_EACCESS | flags::AT_SYMLINK_NOFOLLOW | flags::AT_EMPTY_PATH;
    if how & !known != 0 {
        return fail(EINVAL);
    }
    let want = flags::access(mode as u32).map_err(Error::Errno)?;
    let path_flags = flags::path_flags(how);
    at(user, dirfd, address, how & flags::AT_EMPTY_PATH != 0, |place| {
        let args = directory::CheckAccessArgs {
            path: given(0, place.path.len()),
            flags: path_flags,
            want,
        };
        directory(user, place, directory::CHECK_ACCESS, args.encode(), &[]).map(drop)
    })?;
    Ok(0)
}

/// `truncate`: opens the file to write, as Dukovo's C library does, makes
/// it `length` bytes long (the native `set_size`) and closes it. `length`
/// may not be negative.
pub fn truncate(me: &ThreadRef, user: &UserRef, address: u64, length: u64) -> Result<u64> {
    if length > i64::MAX as u64 {
        return fail(EINVAL);
    }
    let path = paths::read(address)?;
    let open = flags::O_WRONLY | flags::O_CLOEXEC;
    let slot = files::open_path(me, user, paths::AT_FDCWD, &path, open)?;
    let resized = super::descriptors::resize(me, user, slot, length);
    files::delete(me, user, slot);
    resized?;
    Ok(0)
}

/// `rename`, `renameat` and `renameat2`: the native `rename` of the
/// directory the old path is beneath, to the one the new is. `how` is the
/// `renameat2` flags, none of which Dukovo has.
pub fn rename(
    user: &UserRef,
    old_dirfd: u64,
    old_address: u64,
    new_dirfd: u64,
    new_address: u64,
    how: u64,
) -> Result<u64> {
    if how as u32 != 0 {
        return fail(EINVAL);
    }
    at2(user, old_dirfd, old_address, new_dirfd, new_address, |from, to| {
        let args = directory::RenameArgs {
            from: given(0, from.path.len()),
            to_dir: to.slot,
            to: given(1, to.path.len()),
        };
        directory(user, from, directory::RENAME, args.encode(), &[to.path.as_slice()]).map(drop)
    })?;
    Ok(0)
}

/// `mkdir` and `mkdirat`: the native `make_directory`, with the permission
/// bits of `mode` less the process's file mode mask, which the kernel
/// applies.
pub fn mkdirat(user: &UserRef, dirfd: u64, address: u64, mode: u64) -> Result<u64> {
    at(user, dirfd, address, false, |place| {
        let args = directory::MakeDirectoryArgs {
            path: given(0, place.path.len()),
            mode: mode as u32 & 0o7777,
        };
        directory(user, place, directory::MAKE_DIRECTORY, args.encode(), &[]).map(drop)
    })?;
    Ok(0)
}

/// `rmdir`, `unlink` and `unlinkat`: the native `remove`, which removes
/// anything, of what the path names if it is what the call is for: a
/// directory for AT_REMOVEDIR (ENOTDIR if not), and anything else, a
/// symbolic link itself too, without (EISDIR for a directory), as Linux
/// has it.
pub fn unlinkat(user: &UserRef, dirfd: u64, address: u64, how: u64) -> Result<u64> {
    let how = how as u32;
    if how & !flags::AT_REMOVEDIR != 0 {
        return fail(EINVAL);
    }
    let want_directory = how & flags::AT_REMOVEDIR != 0;
    at(user, dirfd, address, false, |place| {
        match (names_directory(user, place)?, want_directory) {
            (false, true) => return fail(ENOTDIR),
            (true, false) => return fail(EISDIR),
            _ => {}
        }
        let args = directory::RemoveArgs { path: given(0, place.path.len()) };
        directory(user, place, directory::REMOVE, args.encode(), &[]).map(drop)
    })?;
    Ok(0)
}

/// Whether `place` names a directory, not by way of a link, which is not
/// one: its status, as `stat` would give it with AT_SYMLINK_NOFOLLOW.
fn names_directory(user: &UserRef, place: &paths::Place) -> Result<bool> {
    let status = files::status_at_place(user, place, false)?;
    Ok(status[FileStatus::OFFSET_KIND] == FileKind::Directory.raw())
}

/// `link` and `linkat`: the native `link` of the directory the old path is
/// beneath, to the new path, following a symbolic link only with
/// AT_SYMLINK_FOLLOW in `how`. A directory is EPERM, as Linux has it.
pub fn linkat(
    user: &UserRef,
    old_dirfd: u64,
    old_address: u64,
    new_dirfd: u64,
    new_address: u64,
    how: u64,
) -> Result<u64> {
    let how = how as u32;
    if how & !flags::AT_SYMLINK_FOLLOW != 0 {
        return fail(EINVAL);
    }
    let follow =
        if how & flags::AT_SYMLINK_FOLLOW != 0 { LinkFlags::FOLLOW } else { LinkFlags::NONE };
    at2(user, old_dirfd, old_address, new_dirfd, new_address, |from, to| {
        let args = directory::LinkArgs {
            path: given(0, from.path.len()),
            flags: follow,
            to_dir: to.slot,
            to: given(1, to.path.len()),
        };
        match directory(user, from, directory::LINK, args.encode(), &[to.path.as_slice()]) {
            Err(Error::Native(status)) if errno::from_status(status) == EISDIR => fail(EPERM),
            done => done.map(drop),
        }
    })?;
    Ok(0)
}

/// `symlink` and `symlinkat`: the native `make_link` of the directory the
/// link's path is beneath, to the target at `target_address`, which may
/// name anything and is not looked at.
pub fn symlinkat(user: &UserRef, target_address: u64, dirfd: u64, address: u64) -> Result<u64> {
    let target = paths::read(target_address)?;
    if target.is_empty() {
        return fail(ENOENT);
    }
    at(user, dirfd, address, false, |place| {
        let args = directory::MakeLinkArgs {
            path: given(0, place.path.len()),
            target: given(1, target.len()),
        };
        directory(user, place, directory::MAKE_LINK, args.encode(), &[target.as_slice()]).map(drop)
    })?;
    Ok(0)
}

/// `readlink` and `readlinkat`: the native `read_link` into the `size`
/// bytes at `buffer`, which cuts what does not fit, as POSIX has it. The
/// bytes of the target that fit, with no NUL.
pub fn readlinkat(user: &UserRef, dirfd: u64, address: u64, buffer: u64, size: u64) -> Result<u64> {
    // The size is an `int`.
    let size = size as i32;
    let Ok(size) = u64::try_from(size) else { return fail(EINVAL) };
    if size == 0 {
        return fail(EINVAL);
    }
    let len = at(user, dirfd, address, false, |place| {
        let args = directory::ReadLinkArgs {
            path: given(0, place.path.len()),
            buffer: UserBuffer { address: buffer, len: size },
        };
        match directory(user, place, directory::READ_LINK, args.encode(), &[])? {
            Answer::Reply(_, words) => match directory::ReadLinkResults::decode(&words) {
                Ok(read) => Ok(read.len),
                Err(_) => fail(EFAULT),
            },
            _ => fail(EFAULT),
        }
    })?;
    Ok(len.min(size))
}

/// `chmod` and `fchmodat`: the native `set_access` of the directory the
/// path is beneath, keeping the owner and group, with the permission bits
/// of `mode`.
pub fn fchmodat(user: &UserRef, dirfd: u64, address: u64, mode: u64) -> Result<u64> {
    let keep = directory::KEEP;
    let mode = mode as u32 & 0o7777;
    at(user, dirfd, address, false, |place| {
        set_access(user, place, PathFlags::NONE, keep, keep, mode)
    })?;
    Ok(0)
}

/// `chown`, `lchown` and `fchownat`: the native `set_access`, keeping the
/// mode; an owner or group of -1 stays, as the native `KEEP` is the same
/// number. `how` is AT_SYMLINK_NOFOLLOW, for a link itself, and
/// AT_EMPTY_PATH, for the file or directory `dirfd` holds.
pub fn fchownat(
    me: &ThreadRef,
    user: &UserRef,
    dirfd: u64,
    address: u64,
    owner: u64,
    group: u64,
    how: u64,
) -> Result<u64> {
    let how = how as u32;
    if how & !(flags::AT_EMPTY_PATH | flags::AT_SYMLINK_NOFOLLOW) != 0 {
        return fail(EINVAL);
    }
    let empty = how & flags::AT_EMPTY_PATH != 0;
    let (owner, group, keep) = (owner as u32, group as u32, directory::KEEP);
    at(user, dirfd, address, empty, |place| {
        if files::names_descriptor(place, empty) {
            return files::set_access(me, user, place.slot, owner, group, keep);
        }
        set_access(user, place, flags::path_flags(how), owner, group, keep)
    })?;
    Ok(0)
}

/// The native `set_access` of the directory `place` is beneath, for the
/// file at its path.
fn set_access(
    user: &UserRef,
    place: &paths::Place,
    path_flags: PathFlags,
    owner: u32,
    group: u32,
    mode: u32,
) -> Result<()> {
    let args = directory::SetAccessArgs {
        path: given(0, place.path.len()),
        flags: path_flags,
        owner,
        group,
        mode,
    };
    directory(user, place, directory::SET_ACCESS, args.encode(), &[]).map(drop)
}

/// `utimensat`: sets the times of the file the path names, or with no path
/// (`futimens`) of the file or directory `dirfd` holds. `times` is the two
/// `struct timespec` of the access and the modification, which may be
/// UTIME_NOW or UTIME_OMIT, or none for now. `how` is AT_SYMLINK_NOFOLLOW
/// and AT_EMPTY_PATH.
pub fn utimensat(
    me: &ThreadRef,
    user: &UserRef,
    dirfd: u64,
    address: u64,
    times: u64,
    how: u64,
) -> Result<u64> {
    let how = how as u32;
    if how & !(flags::AT_SYMLINK_NOFOLLOW | flags::AT_EMPTY_PATH) != 0 {
        return fail(EINVAL);
    }
    let times = read_times(times, records::utimespec)?;
    set_times(me, user, dirfd, address, times, how)
}

/// `utimes` and `futimesat`: `utimensat`, with `struct timeval`s, which
/// have no UTIME_NOW or UTIME_OMIT.
pub fn futimesat(
    me: &ThreadRef,
    user: &UserRef,
    dirfd: u64,
    address: u64,
    times: u64,
) -> Result<u64> {
    let times = read_times(times, |bytes| records::timeval(bytes).map(SetTime::At))?;
    set_times(me, user, dirfd, address, times, 0)
}

/// The two times at `address`, each read by `one`, or both now if there is
/// no address.
fn read_times(
    address: u64,
    one: impl Fn(&[u8; TIMESPEC_SIZE]) -> core::result::Result<SetTime, i32>,
) -> Result<[SetTime; 2]> {
    let mut times = [SetTime::Now; 2];
    if address == 0 {
        return Ok(times);
    }
    let mut bytes = [0u8; 2 * TIMESPEC_SIZE];
    copy_in(address, &mut bytes)?;
    for (time, chunk) in times.iter_mut().zip(bytes.as_chunks::<TIMESPEC_SIZE>().0) {
        *time = one(chunk).map_err(Error::Errno)?;
    }
    Ok(times)
}

/// Sets the access and modification `times` of the file at the path at
/// `address` from `dirfd` (the native `set_times` of the directory it is
/// beneath), or without a path (a null `address`) of the file or directory
/// `dirfd` holds, as `how`'s AT_EMPTY_PATH also makes of an empty one.
fn set_times(
    me: &ThreadRef,
    user: &UserRef,
    dirfd: u64,
    address: u64,
    times: [SetTime; 2],
    how: u32,
) -> Result<u64> {
    let accessed = times[0].native().map_err(Error::Errno)?;
    let modified = times[1].native().map_err(Error::Errno)?;
    if address == 0 {
        set_times_of(me, user, paths::slot_of(dirfd)?, accessed, modified)?;
        return Ok(0);
    }
    let empty = how & flags::AT_EMPTY_PATH != 0;
    at(user, dirfd, address, empty, |place| {
        if files::names_descriptor(place, empty) {
            return set_times_of(me, user, place.slot, accessed, modified);
        }
        let args = directory::SetTimesArgs {
            path: given(0, place.path.len()),
            accessed,
            modified,
            flags: flags::path_flags(how),
        };
        directory(user, place, directory::SET_TIMES, args.encode(), &[]).map(drop)
    })?;
    Ok(0)
}

/// The native `set_times` of the file in `slot`, or of a directory there
/// as its own `.`.
fn set_times_of(
    me: &ThreadRef,
    user: &UserRef,
    slot: u32,
    accessed: u64,
    modified: u64,
) -> Result<()> {
    files::on_either(
        slot,
        || {
            let args = file::SetTimesArgs { accessed, modified };
            method(me, user, slot, file::SET_TIMES, args.encode()).map(drop)
        },
        |place| {
            let args = directory::SetTimesArgs {
                path: given(0, place.path.len()),
                accessed,
                modified,
                flags: PathFlags::NONE,
            };
            directory(user, place, directory::SET_TIMES, args.encode(), &[]).map(drop)
        },
    )
}

/// `mknod` and `mknodat`: the native `make_node` of a regular file, a
/// named pipe or a socket's name, with the permission bits of `mode` less
/// the file mode mask. A device, which only a caller with the privilege may
/// make, is EPERM, as is a directory.
pub fn mknodat(user: &UserRef, dirfd: u64, address: u64, mode: u64) -> Result<u64> {
    let kind = flags::node_kind(mode as u32).map_err(Error::Errno)?;
    at(user, dirfd, address, false, |place| {
        let args = directory::MakeNodeArgs {
            path: given(0, place.path.len()),
            kind,
            device: 0,
            mode: mode as u32 & 0o7777,
        };
        directory(user, place, directory::MAKE_NODE, args.encode(), &[]).map(drop)
    })?;
    Ok(0)
}
