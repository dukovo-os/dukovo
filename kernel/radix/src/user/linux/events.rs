// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! epoll (ADR 0612): Linux's calls on the event queue's `poll` filter.

use radix_linux::errno::ENOSYS;

use super::{Args, Result, fail};
use crate::sched::ThreadRef;
use crate::user::UserRef;

/// `epoll_create1`.
pub fn epoll_create1(me: &ThreadRef, user: &UserRef, how: u64) -> Result<u64> {
    let _ = (me, user, how);
    fail(ENOSYS)
}

/// `epoll_create`.
pub fn epoll_create(me: &ThreadRef, user: &UserRef, size: u64) -> Result<u64> {
    let _ = (me, user, size);
    fail(ENOSYS)
}

/// `epoll_ctl`.
pub fn epoll_ctl(me: &ThreadRef, user: &UserRef, a: &Args) -> Result<u64> {
    let _ = (me, user, a);
    fail(ENOSYS)
}

/// `epoll_wait`, `epoll_pwait` and `epoll_pwait2`.
pub fn epoll_wait(me: &ThreadRef, user: &UserRef, a: &Args, number: u64) -> Result<u64> {
    let _ = (me, user, a, number);
    fail(ENOSYS)
}
