// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! inotify (ADR 0615): Linux's calls on a watch file.

use radix_linux::errno::ENOSYS;

use super::{Result, fail};
use crate::sched::ThreadRef;
use crate::user::UserRef;

/// `inotify_init` (flags 0) and `inotify_init1`.
pub fn inotify_init1(me: &ThreadRef, user: &UserRef, how: u64) -> Result<u64> {
    let _ = (me, user, how);
    fail(ENOSYS)
}

/// `inotify_add_watch`.
pub fn inotify_add_watch(
    me: &ThreadRef,
    user: &UserRef,
    fd: u64,
    path: u64,
    mask: u64,
) -> Result<u64> {
    let _ = (me, user, fd, path, mask);
    fail(ENOSYS)
}

/// `inotify_rm_watch`.
pub fn inotify_rm_watch(me: &ThreadRef, user: &UserRef, fd: u64, watch: u64) -> Result<u64> {
    let _ = (me, user, fd, watch);
    fail(ENOSYS)
}
