// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The calls a Linux program makes on a descriptor it holds (ADR 0611):
//! vectored reads and writes, seeking, copying a descriptor, its flags and
//! locks, pipes, `ioctl`, and the changes `fsync` and `fchmod` make to what
//! it holds.
//!
//! Each call is the one Dukovo's C library makes for the same POSIX
//! function (ports/mlibc/sysdeps/radix/generic/files.cpp and
//! terminal.cpp), on the descriptor's slot, with what raw Linux does
//! differently from the library's wrapper: `readv` and its relatives take
//! their buffers from the program's memory, `fcntl` and `ioctl` take their
//! arguments in registers and in records of the kernel's own layout, and
//! `pipe` writes the two descriptors where the program says.

use alloc::vec::Vec;

use radix_abi::capability::{self, Rights, SlotFlags};
use radix_abi::directory;
use radix_abi::errors::Kernel;
use radix_abi::file::{self, DescriptionFlags, LockOwner, Whence};
use radix_abi::process;
use radix_abi::startup::{SLOT_FD_BASE, SLOT_PROCESS};
use radix_linux::errno::{
    EBADF, EBUSY, EINVAL, EIO, ENOLCK, ENOMEM, ENOTTY, ENXIO, EOPNOTSUPP, EPERM,
};
use radix_linux::flags::{self, Seek};
use radix_linux::memfd;
use radix_linux::records::{self, FLOCK_SIZE, Flock, IOVEC_SIZE, Iovec};
use radix_mm::Kmalloc;

use super::files::{self, MAX_RW_COUNT, TAKEN_TRIES};
use super::paths;
use super::{Error, Result, copy_in, copy_out, fail, is, kmalloc, method, results};
use crate::sched::ThreadRef;
use crate::user::UserRef;

/// The most buffers one `readv` or `writev` takes: Linux's UIO_MAXIOV.
const IOV_MAX: u64 = 1024;

/// How many `struct iovec`s are read from the program at a time.
const IOVECS_AT_ONCE: usize = 16;

/// A buffer of the program's memory a vectored call moves bytes to or
/// from: its address and its length.
type Segment = (u64, u64);

/// `readv`: reads into the buffers in turn, at the description's offset.
pub fn readv(me: &ThreadRef, user: &UserRef, fd: u64, vector: u64, count: u64) -> Result<u64> {
    vectored(me, user, fd, file::READ, None, vector, count)
}

/// `writev`: writes the buffers in turn, at the description's offset. Each
/// buffer is one native write, so the buffers of one call are not kept
/// together in a pipe against other writers as Linux keeps up to PIPE_BUF
/// bytes of them.
pub fn writev(me: &ThreadRef, user: &UserRef, fd: u64, vector: u64, count: u64) -> Result<u64> {
    vectored(me, user, fd, file::WRITE, None, vector, count)
}

/// `preadv`: `readv` at `offset`, which may not be negative. The raw call
/// takes the offset whole in its fourth argument; its fifth is the high
/// half that only a 32-bit program needs.
pub fn preadv(
    me: &ThreadRef,
    user: &UserRef,
    fd: u64,
    vector: u64,
    count: u64,
    offset: u64,
) -> Result<u64> {
    vectored(me, user, fd, file::READ, Some(offset), vector, count)
}

/// `pwritev`: `writev` at `offset`, which may not be negative.
pub fn pwritev(
    me: &ThreadRef,
    user: &UserRef,
    fd: u64,
    vector: u64,
    count: u64,
    offset: u64,
) -> Result<u64> {
    vectored(me, user, fd, file::WRITE, Some(offset), vector, count)
}

/// The `readv` or `writev` (`label`) of the `count` buffers at `vector`,
/// at `offset` or the description's, as Dukovo's C library does: one
/// native read or write each, stopping at the first that moves fewer bytes
/// than its buffer holds, and returning the bytes moved if an error comes
/// after some.
fn vectored(
    me: &ThreadRef,
    user: &UserRef,
    fd: u64,
    label: u32,
    offset: Option<u64>,
    vector: u64,
    count: u64,
) -> Result<u64> {
    if offset.is_some_and(|offset| offset > i64::MAX as u64) {
        return fail(EINVAL);
    }
    let slot = paths::slot_of(fd)?;
    let segments = segments(vector, count)?;
    if segments.is_empty() {
        // Nothing to move: a call of none, which checks the descriptor as
        // the call does.
        return files::move_bytes(me, user, slot, label, offset.unwrap_or(file::CURRENT), 0, 0);
    }
    let mut total = 0u64;
    for &(base, len) in &segments {
        let at = match offset {
            None => Some(file::CURRENT),
            Some(offset) => offset.checked_add(total).filter(|at| *at <= i64::MAX as u64),
        };
        let moved = match at {
            Some(at) => files::move_bytes(me, user, slot, label, at, base, len),
            None => fail(EINVAL),
        };
        match moved {
            Ok(moved) => {
                total += moved;
                if moved < len {
                    break;
                }
            }
            Err(_) if total > 0 => break,
            Err(err) => return Err(err),
        }
    }
    Ok(total)
}

/// The buffers of the `count` `struct iovec`s at `vector`, as Linux
/// reads them: a length that is negative as a signed number is EINVAL,
/// and what lies past MAX_RW_COUNT bytes in all is cut off. Buffers with
/// no bytes are left out.
fn segments(vector: u64, count: u64) -> Result<Vec<Segment, Kmalloc>> {
    if count > IOV_MAX {
        return fail(EINVAL);
    }
    let count = count as usize;
    let mut segments = Vec::new_in(kmalloc());
    segments.try_reserve(count).map_err(|_| Error::Errno(ENOMEM))?;
    let mut left = MAX_RW_COUNT;
    let mut chunk = [0u8; IOVECS_AT_ONCE * IOVEC_SIZE];
    let mut done = 0;
    while done < count {
        let n = (count - done).min(IOVECS_AT_ONCE);
        let bytes = &mut chunk[..n * IOVEC_SIZE];
        copy_in(vector.saturating_add((done * IOVEC_SIZE) as u64), bytes)?;
        for entry in bytes.as_chunks::<IOVEC_SIZE>().0 {
            let iovec = Iovec::read(entry);
            if (iovec.len as i64) < 0 {
                return fail(EINVAL);
            }
            let len = iovec.len.min(left);
            left -= len;
            if len > 0 {
                // Room for every one was reserved above.
                let _ = segments.push_within_capacity((iovec.base, len));
            }
        }
        done += n;
    }
    Ok(segments)
}

/// `close_range`: closes the descriptors from `first` to `last`, or with
/// CLOSE_RANGE_CLOEXEC makes them close-on-exec, as Dukovo's C library
/// does: a descriptor that fails to close is no error. CLOSE_RANGE_UNSHARE
/// is EINVAL, as no process here shares its table.
pub fn close_range(me: &ThreadRef, user: &UserRef, first: u64, last: u64, how: u64) -> Result<u64> {
    let (first, last, how) = (first as u32, last as u32, how as u32);
    if how & !flags::CLOSE_RANGE_CLOEXEC != 0 || first > last {
        return fail(EINVAL);
    }
    // No descriptor is numbered past the table's last.
    let last = last.min(process::MAX_SLOTS - SLOT_FD_BASE - 1);
    let mut next = first;
    while next <= last {
        let slot = SLOT_FD_BASE + next;
        let args = process::SlotsUsedArgs { first: slot };
        let words = results(me, user, SLOT_PROCESS, process::SLOTS_USED, args.encode())?;
        let Ok(used) = process::SlotsUsedResults::decode(&words) else { return fail(EIO) };
        for bit in 0..(last - next + 1).min(64) {
            if (used.used >> bit) & 1 == 0 {
                continue;
            }
            if how & flags::CLOSE_RANGE_CLOEXEC != 0 {
                let _ = close_on_exec(me, user, slot + bit, true);
            } else {
                files::delete(me, user, slot + bit);
            }
        }
        // Past the space's last slot there is nothing more.
        if slot + 64 >= used.slots {
            break;
        }
        next += 64;
    }
    Ok(0)
}

/// `lseek`: the native `seek`, with what Linux has that it has not: the
/// listing cursor of a directory, which can be set (`rewinddir`, `seekdir`)
/// and read (`telldir`), and SEEK_DATA and SEEK_HOLE, which see a file as
/// one piece of data with a hole at its end, as Linux does of a file whose
/// filesystem keeps no holes. A pipe, a socket and a terminal are ESPIPE.
pub fn lseek(me: &ThreadRef, user: &UserRef, fd: u64, offset: u64, whence: u64) -> Result<u64> {
    let slot = paths::slot_of(fd)?;
    let offset = offset as i64;
    let Some(whence) = Seek::from_raw(whence as u32) else { return fail(EINVAL) };
    match whence {
        Seek::Set => seek(me, user, slot, offset, Whence::Start),
        Seek::Current => seek(me, user, slot, offset, Whence::Current),
        Seek::End => seek(me, user, slot, offset, Whence::End),
        Seek::Data => seek_data_or_hole(me, user, slot, offset, false),
        Seek::Hole => seek_data_or_hole(me, user, slot, offset, true),
    }
}

/// The native `seek` on the descriptor in `slot`: where the offset is
/// then.
fn native_seek(
    me: &ThreadRef,
    user: &UserRef,
    slot: u32,
    offset: i64,
    whence: Whence,
) -> Result<u64> {
    let args = file::SeekArgs { offset, whence };
    results(me, user, slot, file::SEEK, args.encode()).map(|words| words[0])
}

/// [`native_seek`], or for a directory, which has no seek, its listing
/// cursor: set by SEEK_SET and read by SEEK_CUR of 0, which is all a
/// listing is seeked by; anything else is EINVAL.
fn seek(me: &ThreadRef, user: &UserRef, slot: u32, offset: i64, whence: Whence) -> Result<u64> {
    match native_seek(me, user, slot, offset, whence) {
        Err(Error::Native(status)) if files::other_kind(status) => {
            match (whence, u64::try_from(offset)) {
                (Whence::Start, Ok(cursor)) => files::with_cursor(user, slot, |at| {
                    *at = cursor;
                    cursor
                }),
                (Whence::Current, Ok(0)) => files::with_cursor(user, slot, |at| *at),
                _ => fail(EINVAL),
            }
        }
        done => done,
    }
}

/// SEEK_DATA (or with `hole` SEEK_HOLE): the first offset from `offset`
/// that is data (a hole), which is `offset` itself (the end of the file)
/// anywhere before the end, and ENXIO from the end on. The file is seen
/// by moving the offset to its end, and back if there is no such offset.
fn seek_data_or_hole(
    me: &ThreadRef,
    user: &UserRef,
    slot: u32,
    offset: i64,
    hole: bool,
) -> Result<u64> {
    let here = native_seek(me, user, slot, 0, Whence::Current).map_err(not_a_file)?;
    let end = native_seek(me, user, slot, 0, Whence::End)?;
    if !u64::try_from(offset).is_ok_and(|at| at < end) {
        let back = i64::try_from(here).unwrap_or(i64::MAX);
        native_seek(me, user, slot, back, Whence::Start)?;
        return fail(ENXIO);
    }
    // The offset is at the end already, where a hole starts.
    if hole { Ok(end) } else { native_seek(me, user, slot, offset, Whence::Start) }
}

/// A directory, or anything else that is not a file, where a file was
/// needed, is EINVAL.
fn not_a_file(err: Error) -> Error {
    match err {
        Error::Native(status) if files::other_kind(status) => Error::Errno(EINVAL),
        err => err,
    }
}

/// `dup`: a copy of the descriptor in the lowest free one, which shares
/// its description: its offset, its status flags and its locks.
pub fn dup(me: &ThreadRef, user: &UserRef, fd: u64) -> Result<u64> {
    duplicate(me, user, fd, 0, false)
}

/// `dup2`: a copy in descriptor `new`, which a call that is not atomic
/// closes first, as Dukovo's C library does; `old` itself if they are the
/// same.
pub fn dup2(me: &ThreadRef, user: &UserRef, old: u64, new: u64) -> Result<u64> {
    duplicate_onto(me, user, old, new, false)
}

/// `dup3`: `dup2`, with O_CLOEXEC if `how` has it, and EINVAL for the same
/// descriptor twice.
pub fn dup3(me: &ThreadRef, user: &UserRef, old: u64, new: u64, how: u64) -> Result<u64> {
    let how = how as u32;
    if how & !flags::O_CLOEXEC != 0 || old as i32 == new as i32 {
        return fail(EINVAL);
    }
    duplicate_onto(me, user, old, new, how & flags::O_CLOEXEC != 0)
}

/// The native `derive` of the descriptor `fd` into the lowest free one
/// from descriptor `min`: a copy with the same rights, which shares its
/// description. The new descriptor.
fn duplicate(
    me: &ThreadRef,
    user: &UserRef,
    fd: u64,
    min: u32,
    close_on_exec_too: bool,
) -> Result<u64> {
    let from = paths::slot_of(fd)?;
    let rights = files::rights_of(me, user, from)?;
    let slot = files::into_free(user, min, |slot| {
        let args = capability::DeriveArgs { to: slot, rights, badge: 0 };
        method(me, user, from, capability::DERIVE, args.encode()).map(|_| slot)
    })?;
    if close_on_exec_too {
        close_on_exec(me, user, slot, true)?;
    }
    Ok(files::descriptor(slot))
}

/// [`duplicate`] into the descriptor `new`, which is closed first, again
/// if another thread fills it before the copy goes in (EBUSY, as Linux has
/// it, if that goes on), and which the space grows to reach.
fn duplicate_onto(
    me: &ThreadRef,
    user: &UserRef,
    old: u64,
    new: u64,
    close_on_exec_too: bool,
) -> Result<u64> {
    let from = paths::slot_of(old)?;
    let to = paths::slot_of(new)?;
    if to - SLOT_FD_BASE >= files::descriptor_limit(user) {
        return fail(EBADF);
    }
    let rights = files::rights_of(me, user, from)?;
    if from == to {
        return Ok(files::descriptor(to));
    }
    for _ in 0..TAKEN_TRIES {
        files::delete(me, user, to);
        let args = capability::DeriveArgs { to, rights, badge: 0 };
        match method(me, user, from, capability::DERIVE, args.encode()) {
            Ok(_) => {
                if close_on_exec_too {
                    close_on_exec(me, user, to, true)?;
                }
                return Ok(files::descriptor(to));
            }
            Err(Error::Native(status)) if is(status, Kernel::Occupied) => {}
            // Past the space's end.
            Err(Error::Native(status)) if is(status, Kernel::NoSuchSlot) => {
                files::reach_slot(me, user, to)?;
            }
            Err(err) => return Err(err),
        }
    }
    fail(EBUSY)
}

/// Makes the slot `slot` close-on-exec, or not.
fn close_on_exec(me: &ThreadRef, user: &UserRef, slot: u32, on: bool) -> Result<()> {
    let slot_flags = if on { SlotFlags::CLOSE_ON_EXEC } else { SlotFlags::NONE };
    let args = capability::FlagsArgs { set: true, flags: slot_flags };
    method(me, user, slot, capability::FLAGS, args.encode()).map(drop)
}

/// `fcntl`: the commands Dukovo's C library has (`Fcntl`), as Linux's
/// raw call takes them; `argument` is the command's `int`, or the
/// address of its `struct flock`. Any other is EINVAL.
pub fn fcntl(me: &ThreadRef, user: &UserRef, fd: u64, command: u64, argument: u64) -> Result<u64> {
    let slot = paths::slot_of(fd)?;
    let command = command as u32;
    match command {
        flags::F_DUPFD | flags::F_DUPFD_CLOEXEC => {
            let min = u32::try_from(argument as i32).map_err(|_| Error::Errno(EINVAL))?;
            if min >= files::descriptor_limit(user) {
                return fail(EINVAL);
            }
            duplicate(me, user, fd, min, command == flags::F_DUPFD_CLOEXEC)
        }
        flags::F_GETFD => {
            let args = capability::FlagsArgs { set: false, flags: SlotFlags::NONE };
            let words = results(me, user, slot, capability::FLAGS, args.encode())?;
            Ok(u64::from(words[0] & u64::from(SlotFlags::CLOSE_ON_EXEC.bits()) != 0))
        }
        flags::F_SETFD => {
            close_on_exec(me, user, slot, argument as u32 & flags::FD_CLOEXEC != 0)?;
            Ok(0)
        }
        flags::F_GETFL => get_status(me, user, slot),
        flags::F_SETFL => set_status(me, user, slot, argument as u32),
        flags::F_GETLK
        | flags::F_SETLK
        | flags::F_SETLKW
        | flags::F_OFD_GETLK
        | flags::F_OFD_SETLK
        | flags::F_OFD_SETLKW => record_lock(me, user, slot, command, argument),
        flags::F_GETPIPE_SZ => pipe_size(me, user, slot, false, 0),
        flags::F_SETPIPE_SZ => pipe_size(me, user, slot, true, argument as u32),
        memfd::F_ADD_SEALS => seals(me, user, slot, Some(argument as u32)),
        memfd::F_GET_SEALS => seals(me, user, slot, None),
        _ => fail(EINVAL),
    }
}

/// `F_ADD_SEALS` of `add`, or with none `F_GET_SEALS`: the native `seal`,
/// which gives the seals the memory file has. As Linux has it, in this
/// order for adding: EPERM if the descriptor is not open for writing,
/// EINVAL for a bit that is no seal, EINVAL for any file but a memory
/// file (for reading too), EPERM if the file is sealed with `F_SEAL_SEAL`
/// (also for `add` of 0), and EBUSY for `F_SEAL_WRITE` with a writable
/// shared mapping.
fn seals(me: &ThreadRef, user: &UserRef, slot: u32, add: Option<u32>) -> Result<u64> {
    if add.is_some() && !files::rights_of(me, user, slot)?.contains(Rights::WRITE) {
        return fail(EPERM);
    }
    let adding = add.map(memfd::seals).transpose().map_err(Error::Errno)?;
    let args = file::SealArgs { add: adding.unwrap_or(file::Seals::NONE) };
    let words = match results(me, user, slot, file::SEAL, args.encode()) {
        Ok(words) => words,
        Err(Error::Native(status)) if files::other_kind(status) => return fail(EINVAL),
        Err(err) => return Err(err),
    };
    let Ok(done) = file::SealResults::decode(&words) else { return fail(EIO) };
    if adding == Some(file::Seals::NONE) && done.seals.contains(file::Seals::SEAL) {
        return fail(EPERM);
    }
    Ok(u64::from(done.seals.bits()))
}

/// The native `status_flags` of the descriptor in `slot`: its flags as
/// they were, and with `set` its flags now those.
fn status_flags(
    me: &ThreadRef,
    user: &UserRef,
    slot: u32,
    set: Option<DescriptionFlags>,
) -> Result<DescriptionFlags> {
    let args =
        file::StatusFlagsArgs { set: set.is_some(), flags: set.unwrap_or(DescriptionFlags::NONE) };
    let words = results(me, user, slot, file::STATUS_FLAGS, args.encode())?;
    match file::StatusFlagsResults::decode(&words) {
        Ok(old) => Ok(old.flags),
        Err(_) => fail(EIO),
    }
}

/// `F_GETFL`: the access mode the descriptor's rights make, with O_APPEND
/// and O_NONBLOCK of its description. A directory is open for reading
/// only, though its capability may have the write right, for making files
/// beneath it.
fn get_status(me: &ThreadRef, user: &UserRef, slot: u32) -> Result<u64> {
    let rights = files::rights_of(me, user, slot)?;
    let mut mode = match (rights.contains(Rights::READ), rights.contains(Rights::WRITE)) {
        (true, true) => flags::O_RDWR,
        (_, true) => flags::O_WRONLY,
        _ => 0,
    };
    match status_flags(me, user, slot, None) {
        Ok(status) => {
            if status.contains(DescriptionFlags::APPEND) {
                mode |= flags::O_APPEND;
            }
            if status.contains(DescriptionFlags::NON_BLOCKING) {
                mode |= flags::O_NONBLOCK;
            }
        }
        Err(Error::Native(status)) if files::other_kind(status) => mode = flags::O_DIRECTORY,
        Err(err) => return Err(err),
    }
    Ok(u64::from(mode))
}

/// `F_SETFL`: the description's O_APPEND and O_NONBLOCK become those of
/// `value`. The other flags Linux can change have no meaning here, and a
/// directory has no status flags to change.
fn set_status(me: &ThreadRef, user: &UserRef, slot: u32, value: u32) -> Result<u64> {
    let mut status = DescriptionFlags::NONE;
    if value & flags::O_APPEND != 0 {
        status = status.union(DescriptionFlags::APPEND);
    }
    if value & flags::O_NONBLOCK != 0 {
        status = status.union(DescriptionFlags::NON_BLOCKING);
    }
    match status_flags(me, user, slot, Some(status)) {
        Ok(_) => Ok(0),
        Err(Error::Native(status)) if files::other_kind(status) => Ok(0),
        Err(err) => Err(err),
    }
}

/// `F_GETPIPE_SZ`, and with `set` `F_SETPIPE_SZ` of `size`: the native
/// `pipe_size`, which gives the size the pipe has. Anything but an end of
/// a pipe is EBADF.
fn pipe_size(me: &ThreadRef, user: &UserRef, slot: u32, set: bool, size: u32) -> Result<u64> {
    let args = file::PipeSizeArgs { set, size: u64::from(size) };
    match results(me, user, slot, file::PIPE_SIZE, args.encode()) {
        Ok(words) => Ok(words[0]),
        Err(Error::Native(status)) if files::other_kind(status) => fail(EBADF),
        Err(err) => Err(err),
    }
}

/// The errno of a lock that failed, as POSIX has it: a descriptor not open
/// for what the lock guards is a bad one for it, and no memory for it is no
/// locks.
fn lock_error(err: Error) -> Error {
    match err {
        Error::Native(status) if is(status, Kernel::MissingRights) => Error::Errno(EBADF),
        Error::Native(status) if is(status, Kernel::NoMemory) => Error::Errno(ENOLCK),
        err => err,
    }
}

/// `fcntl`'s record lock commands (F_GETLK, F_SETLK and F_SETLKW) and their
/// open file description forms (F_OFD_...) with the `struct flock` at
/// `address`: the native `lock` and `lock_holder`, as Dukovo's C library
/// calls them (`record_lock`). F_GETLK writes the lock in the way, or
/// F_UNLCK, back.
fn record_lock(
    me: &ThreadRef,
    user: &UserRef,
    slot: u32,
    command: u32,
    address: u64,
) -> Result<u64> {
    let mut bytes = [0u8; FLOCK_SIZE];
    copy_in(address, &mut bytes)?;
    let mut lock = Flock::read(&bytes);
    let description =
        matches!(command, flags::F_OFD_GETLK | flags::F_OFD_SETLK | flags::F_OFD_SETLKW);
    let testing = matches!(command, flags::F_GETLK | flags::F_OFD_GETLK);
    let kind = flags::lock_kind(lock.kind, testing).map_err(Error::Errno)?;
    // Linux asks open file description locks for a process number of 0.
    if description && lock.pid != 0 {
        return fail(EINVAL);
    }
    let base = lock_base(me, user, slot, lock.whence)?;
    let (start, length) = records::lock_range(base, lock.start, lock.len).map_err(Error::Errno)?;
    let owner = if description { LockOwner::DescriptionRange } else { LockOwner::Process };
    if !testing {
        let wait = matches!(command, flags::F_SETLKW | flags::F_OFD_SETLKW);
        let args = file::LockArgs { owner, kind, start, length, wait };
        method(me, user, slot, file::LOCK, args.encode()).map_err(lock_error)?;
        return Ok(0);
    }
    let args = file::LockHolderArgs { owner, kind, start, length };
    let words = results(me, user, slot, file::LOCK_HOLDER, args.encode()).map_err(lock_error)?;
    let Ok(holder) = file::LockHolderResults::decode(&words) else { return fail(EIO) };
    lock.kind = flags::lock_type(holder.kind);
    if holder.kind != file::LockKind::None {
        lock.whence = 0;
        lock.start = i64::try_from(holder.start).unwrap_or(i64::MAX);
        lock.len = i64::try_from(holder.length).unwrap_or(i64::MAX);
        // An open file description's lock has no process: -1, as on Linux.
        lock.pid = if holder.process == u64::MAX { -1 } else { holder.process as i32 };
    }
    lock.put(&mut bytes);
    copy_out(address, &bytes)?;
    Ok(0)
}

/// Where a lock's range is counted from: the start of the file, the
/// description's offset or the end of the file, by `whence`. The offset
/// is as it was after the call, though the end is found by moving it.
fn lock_base(me: &ThreadRef, user: &UserRef, slot: u32, whence: i16) -> Result<i64> {
    let at = |offset: u64| i64::try_from(offset).unwrap_or(i64::MAX);
    match whence {
        0 => Ok(0),
        1 => native_seek(me, user, slot, 0, Whence::Current).map(at).map_err(not_a_file),
        2 => {
            let here = native_seek(me, user, slot, 0, Whence::Current).map_err(not_a_file)?;
            let end = native_seek(me, user, slot, 0, Whence::End)?;
            native_seek(me, user, slot, at(here), Whence::Start)?;
            Ok(at(end))
        }
        _ => fail(EINVAL),
    }
}

/// `flock`: the description's lock on the whole file, as BSD's `flock`
/// (the native `lock` of owner `description`): shared, exclusive or none,
/// and with LOCK_NB not waited for.
pub fn flock(me: &ThreadRef, user: &UserRef, fd: u64, operation: u64) -> Result<u64> {
    let slot = paths::slot_of(fd)?;
    let (kind, wait) = flags::flock_operation(operation as u32).map_err(Error::Errno)?;
    let args = file::LockArgs { owner: LockOwner::Description, kind, start: 0, length: 0, wait };
    method(me, user, slot, file::LOCK, args.encode()).map_err(lock_error)?;
    Ok(0)
}

/// `pipe`: `pipe2` with no flags.
pub fn pipe(me: &ThreadRef, user: &UserRef, fds: u64) -> Result<u64> {
    pipe2(me, user, fds, 0)
}

/// `pipe2`: the process's native `pipe` into the two lowest free
/// descriptors, which are written as two `int`s at `fds`: the end to
/// read, then the end to write. O_DIRECT, which asks for a pipe of packets
/// that a native pipe is not, is EINVAL. If `fds` cannot be written, the
/// pipe is closed again.
pub fn pipe2(me: &ThreadRef, user: &UserRef, fds: u64, how: u64) -> Result<u64> {
    let pipe_flags = flags::pipe(how as u32).map_err(Error::Errno)?;
    let (read, write) = new_pipe(me, user, pipe_flags)?;
    let mut bytes = [0u8; 8];
    bytes[..4].copy_from_slice(&(files::descriptor(read) as i32).to_le_bytes());
    bytes[4..].copy_from_slice(&(files::descriptor(write) as i32).to_le_bytes());
    if let Err(err) = copy_out(fds, &bytes) {
        files::delete(me, user, read);
        files::delete(me, user, write);
        return Err(err);
    }
    Ok(0)
}

/// Makes a pipe in the two lowest free descriptors, the read end first,
/// again if another thread fills one in the meantime: their slots.
fn new_pipe(me: &ThreadRef, user: &UserRef, pipe_flags: process::PipeFlags) -> Result<(u32, u32)> {
    for _ in 0..=TAKEN_TRIES {
        let read = files::free_descriptor(user, 0)?;
        let write = files::free_descriptor(user, files::descriptor(read) as u32 + 1)?;
        let args = process::PipeArgs { read, write, flags: pipe_flags };
        match method(me, user, SLOT_PROCESS, process::PIPE, args.encode()) {
            Ok(_) => return Ok((read, write)),
            Err(Error::Native(status)) if is(status, Kernel::Occupied) => {}
            Err(err) => return Err(err),
        }
    }
    fail(EBUSY)
}

/// `ioctl`: the requests Dukovo's C library takes (`Ioctl`): the native
/// `control` for a terminal's, which are Linux's numbers, with the size
/// of the record each moves; FIONBIO, FIOCLEX and FIONCLEX, which are the
/// descriptor's flags; and FICLONE, whose argument is a descriptor, which
/// the native request wants as a slot. Any other request is the native
/// `control`'s to answer, and a descriptor that is not a terminal answers
/// ENOTTY. FICLONERANGE, which has a descriptor in a record, is ENOTTY too.
pub fn ioctl(me: &ThreadRef, user: &UserRef, fd: u64, request: u64, argument: u64) -> Result<u64> {
    let slot = paths::slot_of(fd)?;
    let request = request as u32;
    match request {
        flags::FIONBIO => non_blocking(me, user, slot, argument),
        flags::FIOCLEX | flags::FIONCLEX => {
            close_on_exec(me, user, slot, request == flags::FIOCLEX)?;
            Ok(0)
        }
        // A terminal is told by a request that asks it to do nothing.
        flags::TCSBRK => control(me, user, slot, radix_abi::terminal::FLOW, 3, 0, 0),
        file::CLONE => {
            let source = paths::slot_of(argument)?;
            control(me, user, slot, request, u64::from(source), 0, 0)
        }
        file::CLONE_RANGE => fail(ENOTTY),
        _ => control(me, user, slot, request, argument, argument, flags::ioctl_size(request)),
    }
}

/// The native `control` of the descriptor in `slot`: `request` with its
/// `argument`, and the `size` bytes at `buffer`. What is not a file is not
/// a terminal either.
fn control(
    me: &ThreadRef,
    user: &UserRef,
    slot: u32,
    request: u32,
    argument: u64,
    buffer: u64,
    size: usize,
) -> Result<u64> {
    let args = file::ControlArgs { request, argument, buffer, size: size as u64 };
    match results(me, user, slot, file::CONTROL, args.encode()) {
        Ok(words) => Ok(words[0]),
        Err(Error::Native(status)) if files::other_kind(status) => fail(ENOTTY),
        Err(err) => Err(err),
    }
}

/// FIONBIO: the `int` at `address` says whether the description does not
/// wait. A directory has no status flags to change.
fn non_blocking(me: &ThreadRef, user: &UserRef, slot: u32, address: u64) -> Result<u64> {
    let mut bytes = [0u8; 4];
    copy_in(address, &mut bytes)?;
    let old = match status_flags(me, user, slot, None) {
        Ok(old) => old,
        Err(Error::Native(status)) if files::other_kind(status) => return Ok(0),
        Err(err) => return Err(err),
    };
    let wait_not = DescriptionFlags::NON_BLOCKING;
    let new = if i32::from_le_bytes(bytes) != 0 {
        old.union(wait_not)
    } else {
        DescriptionFlags(old.bits() & !wait_not.bits())
    };
    status_flags(me, user, slot, Some(new))?;
    Ok(0)
}

/// `umask`: the process's file mode mask (the native `file_mask`), which
/// the kernel applies to what a program makes, becomes `mask`'s permission
/// bits. The old mask.
pub fn umask(me: &ThreadRef, user: &UserRef, mask: u64) -> Result<u64> {
    let args = process::FileMaskArgs { set: true, mask: mask as u32 & 0o777 };
    results(me, user, SLOT_PROCESS, process::FILE_MASK, args.encode()).map(|words| words[0])
}

/// `fsync`: the native `sync` of the file in the descriptor, or of the
/// filesystem of the directory, as Dukovo's C library calls it.
pub fn fsync(me: &ThreadRef, user: &UserRef, fd: u64) -> Result<u64> {
    let slot = paths::slot_of(fd)?;
    files::on_either(
        slot,
        || method(me, user, slot, file::SYNC, [0; 8]).map(drop),
        |_| method(me, user, slot, directory::SYNC, [0; 8]).map(drop),
    )?;
    Ok(0)
}

/// `ftruncate`: the native `set_size` of the file in the descriptor.
/// `length` may not be negative.
pub fn ftruncate(me: &ThreadRef, user: &UserRef, fd: u64, length: u64) -> Result<u64> {
    if length > i64::MAX as u64 {
        return fail(EINVAL);
    }
    resize(me, user, paths::slot_of(fd)?, length)?;
    Ok(0)
}

/// The native `set_size` of the file in `slot`. As on Linux, a descriptor
/// not open for writing, one that is no file, and a file whose filesystem
/// cannot be cut or grown are EINVAL.
pub(super) fn resize(me: &ThreadRef, user: &UserRef, slot: u32, length: u64) -> Result<()> {
    let args = file::SetSizeArgs { size: length };
    match method(me, user, slot, file::SET_SIZE, args.encode()) {
        Err(Error::Native(status))
            if files::other_kind(status)
                || is(status, Kernel::MissingRights)
                || is(status, Kernel::NotSupported) =>
        {
            fail(EINVAL)
        }
        done => done.map(drop),
    }
}

/// `fallocate`: EOPNOTSUPP for a descriptor that is open, which makes
/// `posix_fallocate` of the C library write the space instead; EINVAL for
/// a range that is not one, and EBADF for no descriptor, as Linux's order
/// has them. No native filesystem keeps space for a file it does not hold.
pub fn fallocate(
    me: &ThreadRef,
    user: &UserRef,
    fd: u64,
    _mode: u64,
    offset: u64,
    length: u64,
) -> Result<u64> {
    files::rights_of(me, user, paths::slot_of(fd)?)?;
    if (offset as i64) < 0 || (length as i64) <= 0 {
        return fail(EINVAL);
    }
    fail(EOPNOTSUPP)
}

/// `fchmod`: the native `set_access` of the file or directory in the
/// descriptor, keeping its owner and group, with the permission bits of
/// `mode`.
pub fn fchmod(me: &ThreadRef, user: &UserRef, fd: u64, mode: u64) -> Result<u64> {
    let slot = paths::slot_of(fd)?;
    let keep = directory::KEEP;
    files::set_access(me, user, slot, keep, keep, mode as u32 & 0o7777)?;
    Ok(0)
}

/// `fchown`: the native `set_access`, keeping the mode. An owner or group
/// of -1 stays, as the native `KEEP` is the same number.
pub fn fchown(me: &ThreadRef, user: &UserRef, fd: u64, owner: u64, group: u64) -> Result<u64> {
    let slot = paths::slot_of(fd)?;
    files::set_access(me, user, slot, owner as u32, group as u32, directory::KEEP)?;
    Ok(0)
}

/// `preadv2` (Linux's flags RWF_*).
pub fn preadv2(me: &ThreadRef, user: &UserRef, a: &super::Args) -> Result<u64> {
    let _ = (me, user, a);
    fail(radix_linux::errno::ENOSYS)
}

/// `pwritev2` (Linux's flags RWF_*).
pub fn pwritev2(me: &ThreadRef, user: &UserRef, a: &super::Args) -> Result<u64> {
    let _ = (me, user, a);
    fail(radix_linux::errno::ENOSYS)
}
