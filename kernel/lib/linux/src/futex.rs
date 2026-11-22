// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The `op` argument of Linux's `futex` call.
//!
//! `futex(uaddr, op, val, timeout, uaddr2, val3)` packs a command and two
//! flags into `op`. This module splits them so that the table can answer
//! the commands the kernel's futex has (wait and wake, with a bit set that
//! matches every waiter) and refuse the rest with ENOSYS.
//!
//! The timeout is relative for `FUTEX_WAIT` and absolute for
//! `FUTEX_WAIT_BITSET`, on the monotonic clock, or on the real-time clock
//! with `FUTEX_CLOCK_REALTIME`. Linux gives ENOSYS for that flag with any
//! command but the bit-set wait, and the table does the same.

/// The futex is private to the process: waiters and wakers are found by
/// address in the caller's address space alone.
pub const FUTEX_PRIVATE_FLAG: u32 = 128;
/// The absolute timeout of `FUTEX_WAIT_BITSET` is on the real-time clock.
pub const FUTEX_CLOCK_REALTIME: u32 = 256;

/// Wait while the word equals a value, for a relative timeout.
pub const FUTEX_WAIT: u32 = 0;
/// Wake up to a number of waiters.
pub const FUTEX_WAKE: u32 = 1;
/// Wait while the word equals a value, for an absolute timeout, and wake
/// only for a waker whose bit set shares a bit with the waiter's.
pub const FUTEX_WAIT_BITSET: u32 = 9;
/// Wake up to a number of waiters whose bit set shares a bit with the
/// waker's.
pub const FUTEX_WAKE_BITSET: u32 = 10;

/// The bit set that matches every other: what `FUTEX_WAIT` and `FUTEX_WAKE`
/// act as having.
pub const FUTEX_BITSET_MATCH_ANY: u32 = u32::MAX;

/// What a futex call does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command {
    /// `FUTEX_WAIT`.
    Wait,
    /// `FUTEX_WAKE`.
    Wake,
    /// `FUTEX_WAIT_BITSET`.
    WaitBitset,
    /// `FUTEX_WAKE_BITSET`.
    WakeBitset,
    /// Another command, by its number: requeue, priority inheritance, wake
    /// with an operation. The table answers ENOSYS.
    Other(u32),
}

/// An `op` argument, taken apart.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Op {
    /// The command, which is `op` without the two flags.
    pub command: Command,
    /// Whether `FUTEX_PRIVATE_FLAG` is set.
    pub private: bool,
    /// Whether `FUTEX_CLOCK_REALTIME` is set.
    pub realtime: bool,
}

/// The command and flags of a raw `op`.
pub fn op(raw: u32) -> Op {
    let number = raw & !(FUTEX_PRIVATE_FLAG | FUTEX_CLOCK_REALTIME);
    let command = match number {
        FUTEX_WAIT => Command::Wait,
        FUTEX_WAKE => Command::Wake,
        FUTEX_WAIT_BITSET => Command::WaitBitset,
        FUTEX_WAKE_BITSET => Command::WakeBitset,
        other => Command::Other(other),
    };
    Op {
        command,
        private: raw & FUTEX_PRIVATE_FLAG != 0,
        realtime: raw & FUTEX_CLOCK_REALTIME != 0,
    }
}
