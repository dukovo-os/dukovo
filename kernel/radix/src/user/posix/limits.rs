// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Resource limits and the file mode mask (Phase 8, radix-process(2)'s
//! `limits` and `file_mask`): what `setrlimit` and `umask` set, kept in
//! each process's POSIX state, copied by `fork` and kept by `exec`.
//! `limits_of` is `prlimit`: the same limits of any process the caller
//! may signal.
//!
//! The kernel enforces RLIMIT_STACK (the size of a new program's stack),
//! RLIMIT_NICE and RLIMIT_RTPRIO (`priority`), and RLIMIT_FSIZE, for
//! writes and for growing a file (`crate::user::files`); the C library
//! enforces RLIMIT_NOFILE, which counts file descriptors, its own. The
//! others are kept for programs to read (radix-posix(7)). A hard limit is
//! only ever lowered: raising one needs privileges, which come with users
//! in Phase 13.

use radix_abi::errors::Kernel;
use radix_abi::process::{self, LimitKind, UNLIMITED};
use radix_posix::signal::QUEUE_MAX;

use super::{find, join, same_user, with_tree};
use crate::user::methods::{Answer, answer};
use crate::user::process::{ProcessObject, ProcessRef};
use crate::user::{Result, UserRef, error};

/// One resource limit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limit {
    /// The soft limit, which counts.
    pub current: u64,
    /// The hard limit, the most the soft one may be raised to.
    pub max: u64,
}

impl Limit {
    /// No limit.
    const NONE: Limit = Limit { current: UNLIMITED, max: UNLIMITED };

    /// `current` and `max`.
    const fn of(current: u64, max: u64) -> Limit {
        Limit { current, max }
    }
}

/// How many limits there are.
const COUNT: usize = LimitKind::RealtimeTime as usize + 1;

/// A process's resource limits, by LimitKind.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits([Limit; COUNT]);

impl Limits {
    /// The limits of a process that no other gave its own: Linux's
    /// defaults, with this system's for pending signals. Files have
    /// Linux's classic 1024 and 4096; the C library allows no more
    /// descriptors than a capability space has slots for, whatever the
    /// limit says (radix-posix(7)).
    pub const DEFAULT: Limits = Limits([
        Limit::NONE,                                   // cpu
        Limit::NONE,                                   // file_size
        Limit::NONE,                                   // data
        Limit::of(8 << 20, UNLIMITED),                 // stack
        Limit::of(0, UNLIMITED),                       // core
        Limit::NONE,                                   // resident
        Limit::NONE,                                   // processes
        Limit::of(1024, 4096),                         // files
        Limit::of(8 << 20, 8 << 20),                   // locked_memory
        Limit::NONE,                                   // address_space
        Limit::NONE,                                   // locks
        Limit::of(QUEUE_MAX as u64, QUEUE_MAX as u64), // pending_signals
        Limit::of(819_200, 819_200),                   // message_queues
        Limit::of(0, 0),                               // nice
        Limit::of(0, 0),                               // realtime_priority
        Limit::NONE,                                   // realtime_time
    ]);

    /// Limit `kind`.
    pub fn get(&self, kind: LimitKind) -> Limit {
        self.0[kind as usize]
    }
}

/// The smallest stack a new program gets, whatever RLIMIT_STACK says.
const STACK_MIN: u64 = 128 << 10;
/// The largest.
const STACK_MAX: u64 = 1 << 30;

/// The bytes of stack a new program gets under `limits`: RLIMIT_STACK's
/// soft limit, in whole pages, from STACK_MIN to STACK_MAX.
pub fn stack_bytes(limits: &Limits, page: u64) -> u64 {
    let wanted = limits.get(LimitKind::Stack).current.clamp(STACK_MIN, STACK_MAX);
    wanted / page * page
}

/// The bits of a file mode mask.
const MODE_BITS: u32 = 0o777;

/// Reads `process`'s limit `resource`, and with `new` sets it, as
/// `setrlimit` does; returns it as it was. `argument` is the position of
/// the soft limit among the call's arguments, which an invalid pair names.
///
/// The soft limit of RLIMIT_FSIZE is copied for writes, which read it
/// without the POSIX lock, before that lock is let go.
fn change(
    process: &ProcessObject,
    resource: LimitKind,
    new: Option<Limit>,
    argument: u32,
) -> Result<Limit> {
    let mut posix = process.posix.lock();
    let limit = &mut posix.limits.0[resource as usize];
    let old = *limit;
    if let Some(new) = new {
        if new.current > new.max {
            return Err(Kernel::InvalidArgument.status(argument));
        }
        if new.max > old.max {
            return Err(error(Kernel::NotPermitted));
        }
        *limit = new;
        if resource == LimitKind::FileSize {
            process.set_file_size_limit(new.current);
        }
    }
    Ok(old)
}

/// The live process `pid`, if `me` may signal it, which is what Linux's
/// `prlimit` asks to read or set another process's limits.
fn other_process(me: &ProcessRef, pid: u64) -> Result<ProcessRef> {
    with_tree(|tree| {
        join(tree, me)?;
        let target = find(tree, pid).ok_or(error(Kernel::NoSuchProcess))?;
        if !tree.may_signal(me.number(), pid, same_user) {
            return Err(error(Kernel::NotPermitted));
        }
        Ok(target)
    })
}

/// The limit and file mask methods of radix-process(2), called by
/// `user`'s thread.
#[inline(never)]
pub fn method(user: &UserRef, label: u32, args: &[u64; 8]) -> Result<Answer> {
    let process = &user.process;
    match label {
        process::LIMITS => {
            let a = process::LimitsArgs::decode(args)?;
            let new = a.set.then_some(Limit::of(a.current, a.max));
            let old = change(process, a.resource, new, 2)?;
            let results = process::LimitsResults { current: old.current, max: old.max };
            answer(label, process::LimitsResults::WORDS, results.encode())
        }
        process::LIMITS_OF => {
            let a = process::LimitsOfArgs::decode(args)?;
            let new = a.set.then_some(Limit::of(a.current, a.max));
            let old = if a.pid == 0 || a.pid == process.number() {
                change(process, a.resource, new, 3)?
            } else {
                let target = other_process(process, a.pid)?;
                // The C library keeps that limit in its own process, which
                // a change from outside would not reach.
                if new.is_some() && a.resource == LimitKind::Files {
                    return Err(error(Kernel::NotPermitted));
                }
                change(&target, a.resource, new, 3)?
            };
            let results = process::LimitsOfResults { current: old.current, max: old.max };
            answer(label, process::LimitsOfResults::WORDS, results.encode())
        }
        process::FILE_MASK => {
            let a = process::FileMaskArgs::decode(args)?;
            if a.mask & !MODE_BITS != 0 {
                return Err(Kernel::InvalidArgument.status(1));
            }
            let mut posix = process.posix.lock();
            let old = posix.file_mask;
            if a.set {
                posix.file_mask = a.mask;
            }
            drop(posix);
            answer(
                label,
                process::FileMaskResults::WORDS,
                process::FileMaskResults { mask: old }.encode(),
            )
        }
        _ => Err(error(Kernel::NoSuchMethod)),
    }
}
