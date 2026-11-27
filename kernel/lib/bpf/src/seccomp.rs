// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! What a seccomp filter reads and returns (linux/seccomp.h): the call's
//! [`SeccompData`], and the action in the high half of the value a
//! program returns, with its data in the low half.
//!
//! Of the values a chain of filters returns, the strictest wins
//! ([`stricter`]): Linux compares the actions as signed numbers, so that
//! `KILL_PROCESS`, whose top bit is set, is stricter than all, and `ALLOW`
//! the least strict. A value whose action Linux does not know kills the
//! process ([`Action::of`]).

use crate::machine::Input;

/// The size of `struct seccomp_data`.
pub const SECCOMP_DATA_SIZE: usize = 64;

/// The arch an x86-64 call is made with: Linux's `AUDIT_ARCH_X86_64`.
pub const AUDIT_ARCH_X86_64: u32 = 0xc000_003e;

/// The actions, in the high half of a program's value.
pub const RET_KILL_PROCESS: u32 = 0x8000_0000;
/// The thread ends with SIGSYS.
pub const RET_KILL_THREAD: u32 = 0x0000_0000;
/// The call is not made, and SIGSYS reports it.
pub const RET_TRAP: u32 = 0x0003_0000;
/// The call is not made, and fails with the data as its errno.
pub const RET_ERRNO: u32 = 0x0005_0000;
/// The call goes to the filter's listener.
pub const RET_USER_NOTIF: u32 = 0x7fc0_0000;
/// The call goes to the process's tracer.
pub const RET_TRACE: u32 = 0x7ff0_0000;
/// The call is made, and logged.
pub const RET_LOG: u32 = 0x7ffc_0000;
/// The call is made.
pub const RET_ALLOW: u32 = 0x7fff_0000;

/// The bits of the action, and of its data.
const ACTION_FULL: u32 = 0xffff_0000;
const DATA: u32 = 0x0000_ffff;

/// The largest errno a filter's `ERRNO` can give: Linux's `MAX_ERRNO`.
pub const ERRNO_MAX: u16 = 4095;

/// The call a filter judges: Linux's `struct seccomp_data`, which a
/// program reads as sixteen 32-bit words.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SeccompData {
    /// The call's number.
    pub nr: i32,
    /// The arch it was made with ([`AUDIT_ARCH_X86_64`]).
    pub arch: u32,
    /// The address of the instruction after the `syscall`.
    pub instruction_pointer: u64,
    /// The six arguments.
    pub args: [u64; 6],
}

impl SeccompData {
    /// The 32-bit word at index `index` (an offset of 4 times it), in the
    /// order x86-64 lays the structure out: the low half of a 64-bit field
    /// first. None past the end.
    #[allow(clippy::cast_possible_truncation, reason = "a cast keeps the low half, as meant")]
    pub fn word(&self, index: usize) -> Option<u32> {
        let half = |value: u64, high: bool| if high { (value >> 32) as u32 } else { value as u32 };
        Some(match index {
            0 => self.nr.cast_unsigned(),
            1 => self.arch,
            2 | 3 => half(self.instruction_pointer, index == 3),
            4..=15 => half(self.args[(index - 4) / 2], index % 2 == 1),
            _ => return None,
        })
    }
}

/// A filter reads the data a whole word at a time, at offsets that are
/// multiples of 4 (`filter`'s check refuses any other load), in the
/// machine's order, not the network's.
impl Input for SeccompData {
    fn length(&self) -> u32 {
        SECCOMP_DATA_SIZE as u32
    }

    fn load(&self, offset: u64, size: usize) -> Option<u32> {
        if size != 4 || !offset.is_multiple_of(4) {
            return None;
        }
        self.word(usize::try_from(offset / 4).ok()?)
    }
}

/// What a filter's value tells the kernel to do with the call.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    /// End the process with SIGSYS, and a core.
    KillProcess,
    /// End the thread with SIGSYS.
    KillThread,
    /// Skip the call and send SIGSYS, whose `si_errno` is the data.
    Trap(u16),
    /// Skip the call and fail it with this errno, at most [`ERRNO_MAX`].
    Errno(u16),
    /// Hand the call to the filter's listener.
    UserNotif,
    /// Hand the call to the tracer, with the data.
    Trace(u16),
    /// Make the call, and log it.
    Log,
    /// Make the call.
    Allow,
}

impl Action {
    /// The action of `value`, as Linux's `__seccomp_filter` reads it: an
    /// action it does not know kills the process.
    #[allow(clippy::cast_possible_truncation, reason = "the data is the low 16 bits")]
    pub fn of(value: u32) -> Action {
        let data = (value & DATA) as u16;
        match value & ACTION_FULL {
            RET_KILL_THREAD => Action::KillThread,
            RET_TRAP => Action::Trap(data),
            RET_ERRNO => Action::Errno(data.min(ERRNO_MAX)),
            RET_USER_NOTIF => Action::UserNotif,
            RET_TRACE => Action::Trace(data),
            RET_LOG => Action::Log,
            RET_ALLOW => Action::Allow,
            _ => Action::KillProcess,
        }
    }

    /// Whether `seccomp(SECCOMP_GET_ACTION_AVAIL)` names `action` (a value
    /// with its data bits clear) as one the kernel knows.
    pub fn is_known(action: u32) -> bool {
        [
            RET_KILL_PROCESS,
            RET_KILL_THREAD,
            RET_TRAP,
            RET_ERRNO,
            RET_USER_NOTIF,
            RET_TRACE,
            RET_LOG,
            RET_ALLOW,
        ]
        .contains(&action)
    }
}

impl Action {
    /// The name Linux's `actions_avail` gives the action, for the log.
    pub fn name(self) -> &'static str {
        match self {
            Action::KillProcess => "kill_process",
            Action::KillThread => "kill_thread",
            Action::Trap(_) => "trap",
            Action::Errno(_) => "errno",
            Action::UserNotif => "user_notif",
            Action::Trace(_) => "trace",
            Action::Log => "log",
            Action::Allow => "allow",
        }
    }

    /// Whether the kernel logs a call that met `action`, as Linux's
    /// `seccomp_log` does with its default `actions_logged`: a kill and a
    /// `LOG` always, `ALLOW` never, the others only when the filter whose
    /// value won was installed with SECCOMP_FILTER_FLAG_LOG (`requested`).
    pub fn is_logged(self, requested: bool) -> bool {
        match self {
            Action::KillProcess | Action::KillThread | Action::Log => true,
            Action::Allow => false,
            Action::Trap(_) | Action::Errno(_) | Action::UserNotif | Action::Trace(_) => requested,
        }
    }
}

/// The action bits of `value` as Linux's `ACTION_ONLY` compares them:
/// signed, so that the top bit makes the strictest.
pub(crate) fn action_only(value: u32) -> i32 {
    (value & ACTION_FULL).cast_signed()
}

/// Of `kept`, the value of the filters run so far, and `next`, the value
/// of an older filter, the one the chain gives: `next` only when its
/// action is stricter, so that of two filters with the same action the
/// newer one's data counts, as in Linux's `seccomp_run_filters`.
pub fn stricter(kept: u32, next: u32) -> u32 {
    if is_stricter(next, kept) { next } else { kept }
}

/// Whether the action of `value` is stricter than that of `than`: what
/// makes a filter's value replace the one a chain kept so far.
pub fn is_stricter(value: u32, than: u32) -> bool {
    action_only(value) < action_only(than)
}
