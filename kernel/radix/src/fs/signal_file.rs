// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Signal files (ADR 0613, radix-process(2)'s `signal_file`): a set of
//! signals that reads take as records, as Linux's signalfd(2).
//!
//! A read takes the signals of the file's mask that are pending for the
//! thread that reads, its own before its process's (`signal::take_pending`,
//! which `signal_wait` takes them with too), and writes each as Linux's
//! 128-byte `struct signalfd_siginfo`, as many as the buffer holds whole.
//! While none is pending it waits, unless the description is non-blocking,
//! as `signal_wait` does: on the process's signal events. A signal taken
//! is gone, also if the buffer then cannot be written, as on Linux.
//!
//! The signals are the reading thread's, not the file's: a poll finds the
//! file readable while a signal of the mask is pending for the thread that
//! polls, and what its pollers count themselves in is that thread's
//! process's `polled`, which `signal::signal_sent` changes. The
//! signals should be blocked, or their handlers run first.
//!
//! When it misbehaves, the boot test `posix` runs `kernelfiles.c`.

use core::any::Any;
use core::sync::atomic::{AtomicU64, Ordering};

use radix_abi::Status;
use radix_abi::errors::Kernel;
use radix_abi::process::PipeFlags;
use radix_linux::kernel_files::{SIGNALFD_SIGINFO_SIZE, put_signalfd_siginfo};
use radix_posix::signal::UNBLOCKABLE;
use radix_vfs::{Access, Node, NodeRef, Out, Stat, mem, next_id};

use super::own_file;
use crate::sched::{self, wait};
use crate::user::posix::poll::{self, Polled};
use crate::user::posix::signal::{self, Restart, Waiting, interrupted};
use crate::user::posix::signal_key;
use crate::user::{UserRef, UserThread};

/// A signal file's node.
pub struct SignalFile {
    id: u64,
    /// The signals it reads: a bit for each, bit 0 for signal 1.
    mask: AtomicU64,
}

impl Node for SignalFile {
    fn stat(&self) -> Stat {
        own_file::stat(self.id)
    }

    fn access(&self) -> Access {
        own_file::access()
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// The signal file `node` is, if it is one.
pub fn signal_file_of(node: &NodeRef) -> Option<&SignalFile> {
    radix_vfs::downcast::<SignalFile>(&**node)
}

/// Makes a signal file of `mask` for `user`'s process, in slot `to` with
/// `flags` (radix-process(2)'s `signal_file`). SIGKILL and SIGSTOP are left
/// out of the mask, as they are of a thread's blocked signals.
///
/// # Errors
///
/// no_memory, and the slot's status if the file cannot go there.
pub fn make(user: &UserRef, to: u32, mask: u64, flags: PipeFlags) -> Result<(), Status> {
    let file = SignalFile { id: next_id(), mask: AtomicU64::new(mask & !UNBLOCKABLE) };
    let node: NodeRef = mem::arc(file).map_err(|_| Kernel::NoMemory.status(0))?;
    own_file::install(user, node, to, flags)
}

/// Changes the signals the signal file of `node` reads to `mask`
/// (radix-file(2)'s `set_signal_mask`), and has the reads and polls of
/// `user`'s process look again, as a changed mask may make a signal
/// readable that was not.
///
/// # Errors
///
/// invalid_argument if `node` is no signal file.
pub fn set_mask(node: &NodeRef, user: &UserThread, mask: u64) -> Result<(), Status> {
    let file = signal_file_of(node).ok_or(Kernel::InvalidArgument.status(0))?;
    file.mask.store(mask & !UNBLOCKABLE, Ordering::Release);
    signal::signal_sent(&user.process);
    Ok(())
}

impl SignalFile {
    /// Calls `each` with what polls of it count themselves in: the
    /// `polled` of the running thread's process.
    pub fn polled(each: &mut dyn FnMut(&Polled)) {
        let running = sched::running();
        if let Some(user) = running.as_deref().and_then(|thread| thread.user()) {
            each(&user.process.polled);
        }
    }

    /// What `poll` finds: readable while a signal of the mask is pending
    /// for the running thread, blocked or not.
    pub fn events(&self) -> u16 {
        let running = sched::running();
        let Some(user) = running.as_deref().and_then(|thread| thread.user()) else { return 0 };
        if signal::pending_set(user) & self.mask.load(Ordering::Acquire) != 0 {
            poll::READABLE
        } else {
            0
        }
    }

    /// Reads the signals of the mask that are pending for `user`'s thread
    /// into `out`, one 128-byte record each, as many as fit whole, waiting
    /// while there are none unless `non_blocking`.
    ///
    /// # Errors
    ///
    /// invalid_argument for an `out` of fewer than 128 bytes, would_block
    /// when none is pending and `non_blocking`, bad_address for a buffer
    /// that cannot be written, if no record went in first, and interrupted
    /// (restarting) for a wait a signal cut short.
    pub fn read(
        &self,
        user: &UserThread,
        out: &mut dyn Out,
        non_blocking: bool,
    ) -> Result<usize, Status> {
        if out.len() < SIGNALFD_SIGINFO_SIZE {
            return Err(Kernel::InvalidArgument.status(0));
        }
        let process = &user.process;
        let _waiting = Waiting::new(process);
        loop {
            let seen = process.signal_events.load(Ordering::SeqCst);
            let mask = self.mask.load(Ordering::Acquire);
            let mut done = 0;
            while done + SIGNALFD_SIGINFO_SIZE <= out.len() {
                let Some((number, info)) = signal::take_pending(user, mask) else { break };
                let record = put_signalfd_siginfo(number, info.code, info.pid, info.status);
                if let Err(err) = out.put(done, &record) {
                    return if done > 0 { Ok(done) } else { Err(super::status(err)) };
                }
                done += SIGNALFD_SIGINFO_SIZE;
            }
            if done > 0 {
                return Ok(done);
            }
            if non_blocking {
                return Err(Kernel::WouldBlock.status(0));
            }
            if sched::interrupted() {
                return Err(interrupted(Restart::Always));
            }
            wait::wait_until(signal_key(process), "signal file", || {
                process.signal_events.load(Ordering::Acquire) != seen || sched::interrupted()
            });
        }
    }
}
