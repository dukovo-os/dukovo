// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! A Linux program's files: its descriptors, which are its slots from
//! SLOT_FD_BASE, and the paths it names (ADR 0611).
//!
//! Each call is the one Dukovo's C library makes for the same POSIX
//! function (ports/mlibc/sysdeps/radix/generic/files.cpp), with what that
//! library keeps for a program kept by the kernel: a new descriptor is
//! the lowest free one below RLIMIT_NOFILE, a directory's listing cursor
//! is its description's offset, and paths are resolved as `paths` says.
//! A method that writes a Radix record where Linux wants another (a file's
//! status, a directory's entries) writes it into the program's own buffer,
//! which then gets the Linux record in its place.

use radix_abi::capability::{self, Rights};
use radix_abi::directory::{self, FileKind, FileStatus, OpenFlags, PathFlags};
use radix_abi::errors::Kernel;
use radix_abi::process::{self, LimitKind};
use radix_abi::startup::{SLOT_FD_BASE, SLOT_PROCESS};
use radix_abi::{Status, UserBuffer, file};
use radix_cap::ObjectKind;
use radix_hal::UserMode;
use radix_linux::errno::{EBADF, EFAULT, EINVAL, EISDIR, EMFILE, ENOTDIR, ERANGE};
use radix_linux::flags;
use radix_linux::records::{self, Stat, Time};

use super::paths::{self, Place};
use super::{Error, Result, fail, is, method, results};
use crate::Arch;
use crate::sched::ThreadRef;
use crate::user::UserRef;
use crate::user::files::{self as native, Buffers, Paths, directory_call, free_slot};
use crate::user::methods::Answer;
use crate::user::objects;

/// `creat`'s flags: O_CREAT | O_WRONLY | O_TRUNC.
pub const CREAT_FLAGS: u64 = 0o1101;

/// AT_SYMLINK_NOFOLLOW, which `lstat` is `newfstatat` with.
pub const NO_FOLLOW: u64 = flags::AT_SYMLINK_NOFOLLOW as u64;

/// How many times a new descriptor is looked for again when another
/// thread took the one found first.
pub(super) const TAKEN_TRIES: usize = 8;

/// `user`'s process's soft RLIMIT_NOFILE, as it was set.
pub(super) fn file_limit(user: &UserRef) -> u64 {
    user.process.posix.lock().limits.get(LimitKind::Files).current
}

/// How many descriptors `user`'s process may have: its soft
/// RLIMIT_NOFILE, as far as its capability space can number them.
pub(super) fn descriptor_limit(user: &UserRef) -> u32 {
    u32::try_from(file_limit(user)).unwrap_or(u32::MAX).min(process::MAX_SLOTS - SLOT_FD_BASE)
}

/// The lowest free descriptor of `user`'s process from `min`, below its
/// RLIMIT_NOFILE: its slot, or EMFILE.
pub fn free_descriptor(user: &UserRef, min: u32) -> Result<u32> {
    let end = SLOT_FD_BASE + descriptor_limit(user);
    match free_slot(user, SLOT_FD_BASE.saturating_add(min), end) {
        Some(slot) => Ok(slot),
        None => fail(EMFILE),
    }
}

/// Grows `user`'s capability space to reach `slot`, as the C library does
/// (`reach_slot`) and `free_slot` does for a new descriptor, with the
/// process's own `grow`: a power of two of slots that takes it in.
pub(super) fn reach_slot(me: &ThreadRef, user: &UserRef, slot: u32) -> Result<()> {
    let args = process::GrowArgs { slots: (slot + 1).next_power_of_two() };
    match results(me, user, SLOT_PROCESS, process::GROW, args.encode()) {
        Err(Error::Native(status)) if is(status, Kernel::NoMemory) => fail(EMFILE),
        done => done.map(drop),
    }
}

/// The rights of the capability in `slot`: EBADF if it holds none.
pub(super) fn rights_of(me: &ThreadRef, user: &UserRef, slot: u32) -> Result<Rights> {
    let words = results(me, user, slot, capability::INFO, [0; 8])?;
    match capability::InfoResults::decode(&words) {
        Ok(info) => Ok(info.rights),
        Err(_) => fail(radix_linux::errno::EIO),
    }
}

/// Whether a call failed with `status` because its capability is of
/// another kind than the call is for: a file's method on a directory, or
/// the other way round (`no_such_method`, radix-ipc(7)).
pub(super) fn other_kind(status: Status) -> bool {
    is(status, Kernel::NoSuchMethod) || is(status, Kernel::WrongKind)
}

/// The descriptor of slot `slot`.
pub(super) fn descriptor(slot: u32) -> u64 {
    u64::from(slot - SLOT_FD_BASE)
}

/// Runs `f` on a new descriptor's slot, again on another if a thread of
/// the process took the first in the meantime (the method fails with
/// `occupied`), as Linux hands out descriptors one at a time.
pub(super) fn into_free<T>(
    user: &UserRef,
    min: u32,
    mut f: impl FnMut(u32) -> Result<T>,
) -> Result<T> {
    let mut tries = 0;
    loop {
        let slot = free_descriptor(user, min)?;
        match f(slot) {
            Err(Error::Native(status)) if is(status, Kernel::Occupied) && tries < TAKEN_TRIES => {
                tries += 1;
            }
            done => return done,
        }
    }
}

/// The kind of object in `user`'s slot `slot`.
pub(super) fn kind_of(user: &UserRef, slot: u32) -> Result<ObjectKind> {
    match objects::capability(user.cspace, slot) {
        Ok(cap) => Ok(cap.kind()),
        Err(_) => fail(EBADF),
    }
}

/// Calls directory method `label` with `words` on the directory `place`
/// names: its path is the method's path argument 0, `others` the rest.
pub(super) fn directory(
    user: &UserRef,
    place: &Place,
    label: u32,
    words: [u64; 8],
    others: &[&[u8]],
) -> Result<Answer> {
    let mut given: [&[u8]; 3] = [&place.path, &[], &[]];
    for (to, other) in given[1..].iter_mut().zip(others) {
        *to = other;
    }
    on_directory(user, place.slot, |found| {
        directory_call(user, found, label, &words, Paths::Given(&given[..=others.len()]))
    })
}

/// The status of what `place` names, as the directory's `stat` method
/// gives it, in the kernel's memory (`status_given`), following a link
/// only if `follow`.
pub(super) fn status_at_place(
    user: &UserRef,
    place: &Place,
    follow: bool,
) -> Result<[u8; FileStatus::SIZE]> {
    on_directory(user, place.slot, |found| {
        crate::user::files::status_given(found, &place.path, follow)
    })
}

/// Runs `f` on the directory in `user`'s slot `slot`, found as a method
/// call finds its capability; anything else is not a directory.
fn on_directory<T>(
    user: &UserRef,
    slot: u32,
    f: impl FnOnce(&objects::Found) -> core::result::Result<T, radix_abi::Status>,
) -> Result<T> {
    on_object(user, slot, |found| {
        if found.cap.kind() != ObjectKind::Directory {
            return Err(radix_abi::errors::File::NotDirectory.status(0));
        }
        f(found)
    })
}

/// Runs `f` on the capability in `user`'s slot `slot`, found as a method
/// call finds its capability: for the methods' work with the records in
/// the kernel's memory (`crate::user::files`'s `socket_name_of` and the
/// others), each of which checks the capability's kind and rights as its
/// method does.
pub(super) fn on_object<T>(
    user: &UserRef,
    slot: u32,
    f: impl FnOnce(&objects::Found) -> core::result::Result<T, radix_abi::Status>,
) -> Result<T> {
    let mut lent = objects::Lent::default();
    let found = objects::find_holding(
        user.cspace,
        slot,
        crate::user::methods::takes_object,
        &user.process,
        &mut lent,
    );
    let done = found.and_then(f);
    lent.end();
    Ok(done?)
}

/// A path argument naming the path at `index` of those
/// [`directory`] gives the method, `len` bytes long.
pub(super) fn given(index: u64, len: usize) -> UserBuffer {
    UserBuffer { address: index, len: len as u64 }
}

/// The directory in `slot` as the place its own `.` names, which is how
/// a directory descriptor takes the calls a file's methods answer.
pub(super) fn itself(slot: u32) -> Result<Place> {
    Ok(Place { slot, path: paths::joined(b"", b".")?, beneath_descriptor: false })
}

/// Whether `place`, where an empty path (AT_EMPTY_PATH, `empty`) led,
/// names the descriptor `dirfd` itself, not a path beneath it.
pub(super) fn names_descriptor(place: &Place, empty: bool) -> bool {
    empty && place.beneath_descriptor && place.path == b"."
}

/// Runs `on_file` for what a descriptor in `slot` holds, and `on_directory`
/// for it as the place its own `.` names if it holds a directory instead,
/// which a file's methods fail with as the other kind.
pub(super) fn on_either<T>(
    slot: u32,
    on_file: impl FnOnce() -> Result<T>,
    on_directory: impl FnOnce(&Place) -> Result<T>,
) -> Result<T> {
    match on_file() {
        Err(Error::Native(status)) if other_kind(status) => on_directory(&itself(slot)?),
        done => done,
    }
}

/// Changes the owner, group and mode of what the descriptor in `slot`
/// holds, each that is `directory::KEEP` staying.
pub(super) fn set_access(
    me: &ThreadRef,
    user: &UserRef,
    slot: u32,
    owner: u32,
    group: u32,
    mode: u32,
) -> Result<()> {
    on_either(
        slot,
        || {
            let args = file::SetAccessArgs { owner, group, mode };
            method(me, user, slot, file::SET_ACCESS, args.encode()).map(drop)
        },
        |place| {
            let args = directory::SetAccessArgs {
                path: given(0, place.path.len()),
                flags: PathFlags::NONE,
                owner,
                group,
                mode,
            };
            directory(user, place, directory::SET_ACCESS, args.encode(), &[]).map(drop)
        },
    )
}

/// Runs `f` on where the path at `address` leads from `dirfd` (AT_FDCWD
/// or a descriptor), and once more from the root if the walk left a
/// directory descriptor's directory (`paths::from_root`). With `empty`
/// (AT_EMPTY_PATH), an empty path names `dirfd` itself.
pub(super) fn at<T>(
    user: &UserRef,
    dirfd: u64,
    address: u64,
    empty: bool,
    f: impl FnMut(&Place) -> Result<T>,
) -> Result<T> {
    at_path(user, dirfd, &paths::read(address)?, empty, f)
}

/// [`at`], for a path in the kernel's memory.
pub(super) fn at_path<T>(
    user: &UserRef,
    dirfd: u64,
    path: &[u8],
    empty: bool,
    mut f: impl FnMut(&Place) -> Result<T>,
) -> Result<T> {
    let place = paths::place(user, dirfd, path, empty)?;
    match f(&place) {
        Err(Error::Native(status)) if place.beneath_descriptor && outside(status) => {
            f(&paths::from_root(user, &place)?)
        }
        done => done,
    }
}

/// [`at`], for a call with two paths, the one at `old_address` from
/// `old_dirfd` and the one at `new_address` from `new_dirfd`, which `f`
/// takes the places of. When the walk left a directory descriptor's
/// directory, it is made again with each path beneath a descriptor, whose
/// path from the root is known, from the root.
pub(super) fn at2<T>(
    user: &UserRef,
    old_dirfd: u64,
    old_address: u64,
    new_dirfd: u64,
    new_address: u64,
    mut f: impl FnMut(&Place, &Place) -> Result<T>,
) -> Result<T> {
    let from = paths::place(user, old_dirfd, &paths::read(old_address)?, false)?;
    let to = paths::place(user, new_dirfd, &paths::read(new_address)?, false)?;
    match f(&from, &to) {
        Err(Error::Native(status)) if outside(status) => {
            let (again_from, again_to) = (from_root(user, &from), from_root(user, &to));
            if again_from.is_none() && again_to.is_none() {
                return Err(Error::Native(status));
            }
            f(again_from.as_ref().unwrap_or(&from), again_to.as_ref().unwrap_or(&to))
        }
        done => done,
    }
}

/// `place` from the root, if it is beneath a directory descriptor whose
/// path from the root is known.
fn from_root(user: &UserRef, place: &Place) -> Option<Place> {
    if place.beneath_descriptor { paths::from_root(user, place).ok() } else { None }
}

/// Whether `status` is a walk's that left the directory it started in.
fn outside(status: Status) -> bool {
    status == crate::fs::status(radix_vfs::Error::Outside)
}

/// `openat`, and `open` and `creat` with AT_FDCWD: as Dukovo's C library
/// opens (`open_path`), into the lowest free descriptor.
pub fn openat(
    me: &ThreadRef,
    user: &UserRef,
    dirfd: u64,
    address: u64,
    flags: u64,
    mode: u64,
) -> Result<u64> {
    let open = Opening::of(flags as u32)?;
    let mode = (mode as u32) & 0o7777;
    at(user, dirfd, address, false, |place| {
        into_free(user, 0, |slot| open_into(me, user, place, &open, mode, slot))
    })
    .map(descriptor)
}

/// Opens `path`, from `dirfd`, as `openat` with the Linux flags
/// `linux_flags` does, into the lowest free descriptor, and returns its
/// slot: how `execve` opens its program.
pub fn open_path(
    me: &ThreadRef,
    user: &UserRef,
    dirfd: u64,
    path: &[u8],
    linux_flags: u32,
) -> Result<u32> {
    let open = Opening::of(linux_flags)?;
    at_path(user, dirfd, path, false, |place| {
        into_free(user, 0, |slot| open_into(me, user, place, &open, 0, slot))
    })
}

/// A Linux open: the native open its flags ask for, and the two facts of
/// them that `open_into` decides by.
struct Opening {
    native: flags::Open,
    /// Opened only to read, or only to name the file (O_PATH): a
    /// directory may be opened so.
    read_only: bool,
    /// Opened only to name the file (O_PATH).
    path_only: bool,
}

impl Opening {
    fn of(linux_flags: u32) -> Result<Opening> {
        let native = flags::open(linux_flags).map_err(Error::Errno)?;
        let path_only = linux_flags & flags::O_PATH != 0;
        let read_only = path_only || linux_flags & flags::O_ACCMODE == 0;
        Ok(Opening { native, read_only, path_only })
    }
}

/// Opens what `place` names as `open` says, into `slot`, as Dukovo's C
/// library's `open_path` does: a directory opened to read gets the write
/// right too where it may have it, which lets a program make files
/// beneath its descriptor, as POSIX allows; one opened to write is EISDIR.
fn open_into(
    me: &ThreadRef,
    user: &UserRef,
    place: &Place,
    open: &Opening,
    mode: u32,
    slot: u32,
) -> Result<u32> {
    // A walk beneath a directory that this opens fails at a link to an
    // absolute path, which `at` follows from the root (ADR 0498).
    let flags = open.native.flags;
    let call = |rights: Rights| {
        let args =
            directory::OpenArgs { path: given(0, place.path.len()), flags, rights, to: slot, mode };
        match directory(user, place, directory::OPEN, args.encode(), &[])? {
            Answer::Reply(_, words) => Ok(directory::OpenResults::decode(&words)
                .map_or(FileKind::File, |results| results.kind)),
            _ => fail(EFAULT),
        }
    };
    let read_write = Rights::READ.union(Rights::WRITE);
    if open.read_only && flags.contains(OpenFlags::DIRECTORY) {
        match call(read_write) {
            Err(Error::Native(status)) if may_narrow(status) => {}
            Ok(_) => return readable(me, user, open, slot),
            Err(err) => return Err(err),
        }
    }
    let kind = call(open.native.rights)?;
    if kind != FileKind::Directory {
        return Ok(slot);
    }
    if !open.read_only {
        delete(me, user, slot);
        return fail(EISDIR);
    }
    readable(me, user, open, slot)?;
    if !flags.contains(OpenFlags::DIRECTORY) {
        // It turned out to be a directory: again with the write right if
        // it may have it, in its place.
        delete(me, user, slot);
        if call(read_write).is_err() {
            call(Rights::READ)?;
        }
    }
    Ok(slot)
}

/// Whether an open that asked for the write right may go on without it:
/// the filesystem is read-only, or the program may not write there.
fn may_narrow(status: radix_abi::Status) -> bool {
    status == crate::fs::status(radix_vfs::Error::ReadOnly)
        || status == crate::fs::status(radix_vfs::Error::Denied)
}

/// A directory opened in `slot` to read, not only to name (O_PATH), needs
/// its read bit, as on Linux (ADR 0214): checked here, so that the open
/// fails rather than the listing. Returns the slot.
fn readable(me: &ThreadRef, user: &UserRef, open: &Opening, slot: u32) -> Result<u32> {
    if open.path_only {
        return Ok(slot);
    }
    let args = directory::CheckAccessArgs {
        path: given(0, 1),
        flags: PathFlags::NONE,
        want: directory::READ_OK,
    };
    match directory(user, &itself(slot)?, directory::CHECK_ACCESS, args.encode(), &[]) {
        Ok(_) => Ok(slot),
        Err(err) => {
            delete(me, user, slot);
            Err(err)
        }
    }
}

/// Empties `slot`, as `close` does, whatever it held.
pub fn delete(me: &ThreadRef, user: &UserRef, slot: u32) {
    let _ = method(me, user, slot, radix_abi::capability::DELETE, [0; 8]);
}

/// `close`.
pub fn close(me: &ThreadRef, user: &UserRef, fd: u64) -> Result<u64> {
    let slot = paths::slot_of(fd)?;
    method(me, user, slot, radix_abi::capability::DELETE, [0; 8])?;
    Ok(0)
}

/// `read`: at the description's offset.
pub fn read(me: &ThreadRef, user: &UserRef, fd: u64, buffer: u64, count: u64) -> Result<u64> {
    transfer(me, user, fd, file::READ, file::CURRENT, buffer, count)
}

/// `write`: at the description's offset, or its end with O_APPEND.
pub fn write(me: &ThreadRef, user: &UserRef, fd: u64, buffer: u64, count: u64) -> Result<u64> {
    transfer(me, user, fd, file::WRITE, file::CURRENT, buffer, count)
}

/// `pread64`: at `offset`, which may not be negative.
pub fn pread(
    me: &ThreadRef,
    user: &UserRef,
    fd: u64,
    buffer: u64,
    count: u64,
    offset: u64,
) -> Result<u64> {
    if (offset as i64) < 0 {
        return fail(EINVAL);
    }
    transfer(me, user, fd, file::READ, offset, buffer, count)
}

/// `pwrite64`: at `offset`, which may not be negative.
pub fn pwrite(
    me: &ThreadRef,
    user: &UserRef,
    fd: u64,
    buffer: u64,
    count: u64,
    offset: u64,
) -> Result<u64> {
    if (offset as i64) < 0 {
        return fail(EINVAL);
    }
    transfer(me, user, fd, file::WRITE, offset, buffer, count)
}

/// The most one read or write moves, as Linux's MAX_RW_COUNT: a page
/// short of 2 GiB, so that the count it returns fits an `int`.
pub(super) const MAX_RW_COUNT: u64 = 0x7fff_f000;

/// A file's `read` or `write` (`label`, whose arguments have the same
/// form) of `count` bytes at `buffer`, at `offset`, on descriptor `fd`:
/// the count it moved.
fn transfer(
    me: &ThreadRef,
    user: &UserRef,
    fd: u64,
    label: u32,
    offset: u64,
    buffer: u64,
    count: u64,
) -> Result<u64> {
    move_bytes(me, user, paths::slot_of(fd)?, label, offset, buffer, count)
}

/// [`transfer`], on the descriptor in `slot`. A directory has neither a
/// read nor a write, which Linux calls EISDIR for a read and EBADF for a
/// write, as a directory is never open for writing; so is a descriptor not
/// open for what the call does.
pub(super) fn move_bytes(
    me: &ThreadRef,
    user: &UserRef,
    slot: u32,
    label: u32,
    offset: u64,
    buffer: u64,
    count: u64,
) -> Result<u64> {
    let len = count.min(MAX_RW_COUNT);
    let args = file::ReadArgs { offset, buffer: UserBuffer { address: buffer, len } };
    match results(me, user, slot, label, args.encode()) {
        Ok(words) => Ok(words[0]),
        Err(Error::Native(status)) if is(status, Kernel::MissingRights) => fail(EBADF),
        Err(_) if matches!(kind_of(user, slot), Ok(ObjectKind::Directory)) => {
            fail(if label == file::WRITE { EBADF } else { EISDIR })
        }
        Err(err) => Err(err),
    }
}

/// [`move_bytes`] for the buffers `parts` as one, a `readv` or `writev`
/// that is a single native read or write of all of them: `appends` makes
/// a write at CURRENT go to the end of the file for this call alone.
pub(super) fn move_buffers(
    user: &UserRef,
    slot: u32,
    label: u32,
    offset: u64,
    parts: &[UserBuffer],
    appends: bool,
) -> Result<u64> {
    let mut buffers = Buffers::new(parts)?;
    let moved = on_object(user, slot, |found| {
        if label == file::WRITE {
            native::write_buffers(user, found, offset, &buffers, appends)
        } else {
            native::read_buffers(user, found, offset, &mut buffers)
        }
    });
    match moved {
        Err(Error::Native(status)) if is(status, Kernel::MissingRights) => fail(EBADF),
        Err(_) if matches!(kind_of(user, slot), Ok(ObjectKind::Directory)) => {
            fail(if label == file::WRITE { EBADF } else { EISDIR })
        }
        moved => moved,
    }
}

/// `fstat`.
pub fn fstat(me: &ThreadRef, user: &UserRef, fd: u64, buffer: u64) -> Result<u64> {
    let slot = paths::slot_of(fd)?;
    status_of(me, user, slot, buffer)?;
    put_stat(buffer)
}

/// Writes the status of what `slot` holds into the program's `buffer`,
/// as Radix's FileStatus: a file's `stat`, or a directory's of `.`.
fn status_of(me: &ThreadRef, user: &UserRef, slot: u32, buffer: u64) -> Result<()> {
    let out = UserBuffer { address: buffer, len: FileStatus::SIZE as u64 };
    if kind_of(user, slot)? == ObjectKind::Directory {
        let args = directory::StatArgs { path: given(0, 1), flags: PathFlags::NONE, buffer: out };
        directory(user, &itself(slot)?, directory::STAT, args.encode(), &[])?;
    } else {
        results(me, user, slot, file::STAT, file::StatArgs { buffer: out }.encode())?;
    }
    Ok(())
}

/// `newfstatat`, and `stat` and `lstat` with AT_FDCWD.
pub fn stat_at(
    me: &ThreadRef,
    user: &UserRef,
    dirfd: u64,
    address: u64,
    buffer: u64,
    flags: u64,
) -> Result<u64> {
    let flags = flags as u32;
    if flags & !(flags::AT_SYMLINK_NOFOLLOW | flags::AT_EMPTY_PATH) != 0 {
        return fail(EINVAL);
    }
    status_at(me, user, dirfd, address, buffer, flags)?;
    put_stat(buffer)
}

/// Writes the status of what the path at `address` names from `dirfd`
/// into the program's `buffer`, as Radix's FileStatus. An empty path with
/// AT_EMPTY_PATH names `dirfd` itself, a file's descriptor too.
fn status_at(
    me: &ThreadRef,
    user: &UserRef,
    dirfd: u64,
    address: u64,
    buffer: u64,
    flags: u32,
) -> Result<()> {
    let empty = flags & flags::AT_EMPTY_PATH != 0;
    let follow = flags & flags::AT_SYMLINK_NOFOLLOW == 0;
    let path_flags = if follow { PathFlags::NONE } else { PathFlags::NO_FOLLOW };
    at(user, dirfd, address, empty, |place| {
        if names_descriptor(place, empty) {
            return status_of(me, user, place.slot, buffer);
        }
        let out = UserBuffer { address: buffer, len: FileStatus::SIZE as u64 };
        let args = directory::StatArgs {
            path: given(0, place.path.len()),
            flags: path_flags,
            buffer: out,
        };
        directory(user, place, directory::STAT, args.encode(), &[]).map(drop)
    })
}

/// Reads the FileStatus a method wrote at `buffer` in the program's
/// memory, as Linux's record of a file's status.
fn read_status(buffer: u64) -> Result<Stat> {
    let mut bytes = [0u8; FileStatus::SIZE];
    if !Arch::copy_from_user(&mut bytes, buffer) {
        return fail(EFAULT);
    }
    let u64_at = |at: usize| u64::from_le_bytes(bytes[at..at + 8].try_into().unwrap_or([0; 8]));
    let u32_at = |at: usize| u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap_or([0; 4]));
    let kind = FileKind::from_raw(bytes[FileStatus::OFFSET_KIND]).unwrap_or(FileKind::File);
    let mode =
        u16::from_le_bytes([bytes[FileStatus::OFFSET_MODE], bytes[FileStatus::OFFSET_MODE + 1]]);
    let device = u64_at(FileStatus::OFFSET_DEVICE);
    let rdev = if matches!(kind, FileKind::CharDevice | FileKind::BlockDevice) {
        records::makedev((device >> 32) as u32, device as u32)
    } else {
        0
    };
    let time =
        |ns: u64| Time { sec: (ns / 1_000_000_000) as i64, nsec: (ns % 1_000_000_000) as u32 };
    let created = u64_at(FileStatus::OFFSET_CREATED);
    Ok(Stat {
        dev: u64_at(FileStatus::OFFSET_MOUNT),
        ino: u64_at(FileStatus::OFFSET_ID),
        mode: type_bits(kind) | u32::from(mode & 0o7777),
        nlink: u64::from(u32_at(FileStatus::OFFSET_LINKS)),
        uid: u32_at(FileStatus::OFFSET_OWNER),
        gid: u32_at(FileStatus::OFFSET_GROUP),
        rdev,
        size: u64_at(FileStatus::OFFSET_SIZE),
        blksize: 4096,
        blocks: u64_at(FileStatus::OFFSET_ALLOCATED).div_ceil(512),
        atime: time(u64_at(FileStatus::OFFSET_ACCESSED)),
        mtime: time(u64_at(FileStatus::OFFSET_MODIFIED)),
        ctime: time(u64_at(FileStatus::OFFSET_CHANGED)),
        // A filesystem that keeps no birth time gives 0.
        btime: (created != 0).then(|| time(created)),
    })
}

/// The file type bits of a mode, for `kind`.
fn type_bits(kind: FileKind) -> u32 {
    match kind {
        FileKind::Directory => 0o040_000,
        FileKind::BlockDevice => 0o060_000,
        FileKind::CharDevice => 0o020_000,
        FileKind::Pipe => 0o010_000,
        FileKind::Symlink => 0o120_000,
        FileKind::Socket => 0o140_000,
        _ => 0o100_000,
    }
}

/// Puts Linux's `struct stat` in place of the FileStatus at `buffer`.
fn put_stat(buffer: u64) -> Result<u64> {
    let stat = read_status(buffer)?;
    let mut out = [0u8; 144];
    records::put_stat(&stat, &mut out);
    if !Arch::copy_to_user(buffer, &out) {
        return fail(EFAULT);
    }
    Ok(0)
}

/// `statx`: the basic statistics, and the birth time where the
/// filesystem keeps one. The FileStatus is written where the record goes,
/// which is longer.
pub fn statx(
    me: &ThreadRef,
    user: &UserRef,
    dirfd: u64,
    address: u64,
    flags: u64,
    mask: u64,
    buffer: u64,
) -> Result<u64> {
    let flags = flags as u32;
    // AT_STATX_SYNC_TYPE's values ask how fresh the answer is: a local
    // filesystem's always is.
    let known = flags::AT_SYMLINK_NOFOLLOW
        | flags::AT_EMPTY_PATH
        | flags::AT_NO_AUTOMOUNT
        | flags::AT_STATX_SYNC_TYPE;
    if flags & !known != 0 || flags & flags::AT_STATX_SYNC_TYPE == flags::AT_STATX_SYNC_TYPE {
        return fail(EINVAL);
    }
    status_at(me, user, dirfd, address, buffer, flags)?;
    let stat = read_status(buffer)?;
    let mut out = [0u8; 256];
    records::put_statx(&stat, mask as u32, &mut out);
    if !Arch::copy_to_user(buffer, &out) {
        return fail(EFAULT);
    }
    Ok(0)
}

/// `getdents64`: the directory's entries from its description's offset,
/// as Linux's `linux_dirent64` records, `.` and `..` first, as Dukovo's
/// C library lists them (`ReadEntries`); the offset moves past those
/// given. Each entry's number is its cursor, unique in the directory and
/// never 0, since listings carry no file numbers.
pub fn getdents64(user: &UserRef, fd: u64, buffer: u64, size: u64) -> Result<u64> {
    let slot = paths::slot_of(fd)?;
    let cap = objects::capability(user.cspace, slot).map_err(|_| Error::Errno(EBADF))?;
    let Ok(objects::Object::Directory(directory)) = objects::object_of(&cap) else {
        return fail(ENOTDIR);
    };
    // Held for the listing, so that two listings of the description
    // follow one another, as Linux's position lock makes them.
    let mut offset = directory.offset.lock();
    let mut out = Entries::new(size)?;
    if *offset == 0 {
        if !out.put(b".", DOTDOT_CURSOR, records::DT_DIR) {
            return fail(EINVAL);
        }
        *offset = DOTDOT_CURSOR;
    }
    if *offset == DOTDOT_CURSOR && out.put(b"..", LISTING_CURSOR, records::DT_DIR) {
        *offset = LISTING_CURSOR;
    }
    if *offset != DOTDOT_CURSOR {
        let from = if *offset == LISTING_CURSOR { 0 } else { *offset };
        let (mut next, mut full) = (None, false);
        let listed = crate::user::files::list_with(user, slot, from, &mut |after, name, kind| {
            if !out.put(name.as_bytes(), after, dirent_type(kind)) {
                full = true;
                return false;
            }
            next = Some(after);
            true
        });
        if let Some(next) = next {
            *offset = next;
        }
        match listed {
            Err(status) if out.used == 0 => return Err(status.into()),
            // Not even one entry fits: an empty answer would read as the
            // end.
            _ if full && out.used == 0 => return fail(EINVAL),
            _ => {}
        }
    }
    if !Arch::copy_to_user(buffer, &out.bytes[..out.used]) {
        return fail(EFAULT);
    }
    Ok(out.used as u64)
}

/// The type a directory record gives an entry of `kind`.
fn dirent_type(kind: radix_vfs::Kind) -> u8 {
    match kind {
        radix_vfs::Kind::Directory => records::DT_DIR,
        radix_vfs::Kind::BlockDevice => records::DT_BLK,
        radix_vfs::Kind::CharDevice => records::DT_CHR,
        radix_vfs::Kind::Pipe => records::DT_FIFO,
        radix_vfs::Kind::Symlink => records::DT_LNK,
        radix_vfs::Kind::Socket => records::DT_SOCK,
        _ => records::DT_REG,
    }
}

/// Runs `f` on the listing cursor of the directory in `slot`: its
/// description's offset, which `getdents64` moves and `lseek` sets, and
/// which copies of the descriptor share.
pub(super) fn with_cursor<T>(
    user: &UserRef,
    slot: u32,
    f: impl FnOnce(&mut u64) -> T,
) -> Result<T> {
    let cap = objects::capability(user.cspace, slot).map_err(|_| Error::Errno(EBADF))?;
    let Ok(objects::Object::Directory(directory)) = objects::object_of(&cap) else {
        return fail(EBADF);
    };
    let mut offset = directory.offset.lock();
    Ok(f(&mut offset))
}

/// The cursor after `.`, and after `..`: above any a filesystem gives.
const DOTDOT_CURSOR: u64 = (1 << 62) | 1;
const LISTING_CURSOR: u64 = (1 << 62) | 2;

/// Linux directory records being made, in the kernel's memory.
pub struct Entries {
    bytes: alloc::vec::Vec<u8, radix_mm::Kmalloc>,
    used: usize,
}

impl Entries {
    /// Room for `size` bytes of records, but no more than a listing gives
    /// at once.
    fn new(size: u64) -> Result<Entries> {
        let len = usize::try_from(size).unwrap_or(usize::MAX).min(LISTING_MAX);
        let bytes = radix_mm::bytes::zeroed(len, super::kmalloc())
            .map_err(|_| Error::Errno(radix_linux::errno::ENOMEM))?;
        Ok(Entries { bytes, used: 0 })
    }

    /// Adds an entry for `name`, whose cursor after it is `next`, of type
    /// `kind`; false if it does not fit.
    pub fn put(&mut self, name: &[u8], next: u64, kind: u8) -> bool {
        match records::put_dirent64(&mut self.bytes[self.used..], next, next as i64, kind, name) {
            Some(len) => {
                self.used += len;
                true
            }
            None => false,
        }
    }
}

/// The most bytes of records one `getdents64` makes: what one listing of
/// a directory gives at a time.
const LISTING_MAX: usize = 32768;

/// `getcwd`: the working directory, with its NUL; its length, the NUL
/// counted, as the raw call returns it.
pub fn getcwd(user: &UserRef, buffer: u64, size: u64) -> Result<u64> {
    let cwd = user.process.posix.lock().directory.clone();
    let cwd = cwd.as_deref().map_or(&b"/"[..], |cwd| &cwd[..]);
    let len = cwd.len() as u64 + 1;
    if size < len {
        return fail(ERANGE);
    }
    if !Arch::copy_to_user(buffer, cwd) || !Arch::copy_to_user(buffer + cwd.len() as u64, &[0]) {
        return fail(EFAULT);
    }
    Ok(len)
}

/// `chdir`: the working directory becomes where the path leads, which
/// must be a directory the program may search, by its path from the root
/// with links and `..` worked out.
pub fn chdir(user: &UserRef, address: u64) -> Result<u64> {
    let real = at(user, paths::AT_FDCWD, address, false, |place| {
        Ok(crate::user::files::directory_path(user, place.slot, &place.path)?)
    })?;
    set_directory(user, &real)
}

/// `fchdir`: the working directory becomes the directory `fd` holds, by
/// the path it was opened by.
pub fn fchdir(user: &UserRef, fd: u64) -> Result<u64> {
    let slot = paths::slot_of(fd)?;
    if kind_of(user, slot)? != ObjectKind::Directory {
        return fail(ENOTDIR);
    }
    let place = Place { slot, path: paths::joined(b"", b".")?, beneath_descriptor: true };
    let root = paths::from_root(user, &place)?;
    let real = crate::user::files::directory_path(user, root.slot, &root.path)?;
    set_directory(user, &real)
}

/// Makes `path` the working directory of `user`'s process.
fn set_directory(user: &UserRef, path: &[u8]) -> Result<u64> {
    if crate::user::posix::set_directory(&user.process, path) {
        Ok(0)
    } else {
        fail(radix_linux::errno::ENOMEM)
    }
}
