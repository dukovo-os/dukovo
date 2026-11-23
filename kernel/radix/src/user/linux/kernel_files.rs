// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Files of the kernel's own (ADR 0613): eventfd, timerfd, signalfd and
//! pidfd.

use radix_linux::errno::ENOSYS;

use super::{Args, Result, fail};
use crate::sched::ThreadRef;
use crate::user::UserRef;

/// `eventfd` (flags 0) and `eventfd2`.
pub fn eventfd2(me: &ThreadRef, user: &UserRef, initial: u64, how: u64) -> Result<u64> {
    let _ = (me, user, initial, how);
    fail(ENOSYS)
}

/// `timerfd_create`.
pub fn timerfd_create(me: &ThreadRef, user: &UserRef, clock: u64, how: u64) -> Result<u64> {
    let _ = (me, user, clock, how);
    fail(ENOSYS)
}

/// `timerfd_settime`.
pub fn timerfd_settime(me: &ThreadRef, user: &UserRef, a: &Args) -> Result<u64> {
    let _ = (me, user, a);
    fail(ENOSYS)
}

/// `timerfd_gettime`.
pub fn timerfd_gettime(me: &ThreadRef, user: &UserRef, fd: u64, value: u64) -> Result<u64> {
    let _ = (me, user, fd, value);
    fail(ENOSYS)
}

/// `signalfd` (flags 0) and `signalfd4`.
pub fn signalfd4(me: &ThreadRef, user: &UserRef, a: &Args) -> Result<u64> {
    let _ = (me, user, a);
    fail(ENOSYS)
}

/// `pidfd_open`.
pub fn pidfd_open(me: &ThreadRef, user: &UserRef, pid: u64, how: u64) -> Result<u64> {
    let _ = (me, user, pid, how);
    fail(ENOSYS)
}

/// `pidfd_send_signal`.
pub fn pidfd_send_signal(me: &ThreadRef, user: &UserRef, a: &Args) -> Result<u64> {
    let _ = (me, user, a);
    fail(ENOSYS)
}
