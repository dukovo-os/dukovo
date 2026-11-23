// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! inotify (ADR 0615): Linux's calls on a watch file.
//!
//! A watch file is the native `watch_file` of the process in the lowest
//! free descriptor, and `inotify_add_watch` and `inotify_rm_watch` are the
//! native `watch_add` and `watch_remove` of the file in a descriptor. The
//! masks and the records a read gives are Linux's own, which the native
//! methods take and give as they are (`radix_linux::notify`).

use radix_abi::file;
use radix_abi::process;
use radix_abi::startup::SLOT_PROCESS;
use radix_cap::ObjectKind;
use radix_linux::errno::EINVAL;
use radix_linux::flags;
use radix_linux::notify::check_mask;

use super::files;
use super::paths::{self, AT_FDCWD};
use super::{Error, Result, fail, method};
use crate::sched::ThreadRef;
use crate::user::UserRef;

/// `inotify_init` (flags 0) and `inotify_init1`: the native `watch_file`
/// in the lowest free descriptor, which is that descriptor.
///
/// Linux's rule: EINVAL for any flag but `IN_CLOEXEC` and `IN_NONBLOCK`,
/// EMFILE if the process has no descriptor left, and ENOSPC or ENOMEM
/// where the native method's no_room and no_memory are.
pub fn inotify_init1(me: &ThreadRef, user: &UserRef, how: u64) -> Result<u64> {
    // IN_CLOEXEC and IN_NONBLOCK are O_CLOEXEC and O_NONBLOCK.
    let watch_flags = flags::pipe(how as u32).map_err(Error::Errno)?;
    let slot = files::into_free(user, 0, |slot| {
        let args = process::WatchFileArgs { to: slot, flags: watch_flags };
        method(me, user, SLOT_PROCESS, process::WATCH_FILE, args.encode()).map(|_| slot)
    })?;
    Ok(files::descriptor(slot))
}

/// `inotify_add_watch`: the native `watch_add` of the path beneath the
/// working directory, for the file in descriptor `fd`.
///
/// Linux's rule: EINVAL for a `mask` with none of the bits there are, or
/// with both `IN_MASK_ADD` and `IN_MASK_CREATE`, which comes before the
/// descriptor is looked at; EBADF if `fd` is open no more; EINVAL if it
/// is not a watch file; EFAULT, ENAMETOOLONG, ENOENT, ENOTDIR (also for
/// `IN_ONLYDIR`), ELOOP and EACCES as the path's walk fails them, and
/// EACCES if the caller cannot read what it leads to; EEXIST with
/// `IN_MASK_CREATE` for a file watched already; ENOSPC past the account's
/// watches, and ENOMEM.
pub fn inotify_add_watch(
    _me: &ThreadRef,
    user: &UserRef,
    fd: u64,
    path: u64,
    mask: u64,
) -> Result<u64> {
    let mask = mask as u32;
    check_mask(mask).map_err(Error::Errno)?;
    let slot = paths::slot_of(fd)?;
    if files::kind_of(user, slot)? != ObjectKind::File {
        return fail(EINVAL);
    }
    let watch = files::at(user, AT_FDCWD, path, false, |place| {
        files::on_object(user, slot, |found| {
            crate::user::files::watch_add_given(user, found, place.slot, &place.path, mask)
        })
    });
    Ok(u64::from(watch?))
}

/// `inotify_rm_watch`: the native `watch_remove` of watch `watch` of the
/// file in descriptor `fd`.
///
/// Linux's rule: EBADF if `fd` is open no more; EINVAL if it is not a
/// watch file or holds no such watch. The watch file then reads an
/// `IN_IGNORED` event for it.
pub fn inotify_rm_watch(me: &ThreadRef, user: &UserRef, fd: u64, watch: u64) -> Result<u64> {
    let slot = paths::slot_of(fd)?;
    // Linux's `wd` is an `int`, which no watch has the high bits of.
    let Ok(number) = u32::try_from(watch as i32) else { return fail(EINVAL) };
    let args = file::WatchRemoveArgs { watch: number };
    match method(me, user, slot, file::WATCH_REMOVE, args.encode()) {
        Ok(_) => Ok(0),
        Err(Error::Native(status)) if files::other_kind(status) => fail(EINVAL),
        Err(err) => Err(err),
    }
}
