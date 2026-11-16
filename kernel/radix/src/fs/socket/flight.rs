// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Unix sockets on their way (ADR 0525): the rule that keeps a socket
//! sent among a send's files from coming to hold itself open, which no
//! process could then close.
//!
//! A socket keeps the queues it receives from (`Socket::each_kept`): a
//! connected stream socket the pipe it reads, a listening one the pipes
//! of the connections that wait for `accept`, a datagram socket its own
//! queue. Say that S holds T while T rides in a queue S keeps. A send of
//! a Unix socket T fails with too_many when
//!
//! - (a) a queue T keeps holds a Unix socket already, or
//! - (b) the queue the files go into is one that T keeps.
//!
//! Then no cycle of sockets holding each other can form. A send of T adds
//! only "holds T" for the keeper of the queue it goes into. By (a) T
//! holds nothing then, and by (b) the new link does not start at T, so
//! no cycle closes through T. Taking files out of a queue only takes
//! links away. Every check and the insertion that follows it happen
//! while [`SENDING`] is held, so two sends never each pass the check
//! against the state before the other. A queue changes keeper only when
//! `accept` gives a waiting connection its socket, which nothing can send
//! until it has it, and which nothing holds, so the move adds no cycle.
//!
//! [`SENDING`] is taken inside the target queue's lock, just before the
//! files go in, and held only while the kernel looks at its own state:
//! never across a wait, and never while a copy from or to a program's
//! memory, which can wait on a disk or a filesystem's server, is under
//! way. So the data that rides with the files is copied into the kernel
//! first, and a send that finds no room lets go, waits, and starts again
//! from the check.

use core::sync::atomic::Ordering;

use radix_abi::Status;
use radix_abi::errors::Kernel;
use radix_sync::{Mutex, MutexGuard};
use radix_vfs::{In, Slice};

use super::End;
use super::datagram::Inbox;
use crate::fs::pipe::{self, ATOMIC, Files, Pipe};

/// Held while a send of Unix sockets checks the queues and puts them in.
static SENDING: Mutex<()> = Mutex::new(());

/// Holds [`SENDING`], for a check and the insertion after it. The caller
/// holds the target queue's lock, and waits for nothing until it lets
/// go.
pub(super) fn hold() -> MutexGuard<'static, ()> {
    SENDING.lock()
}

/// A queue, by identity: a pipe or a datagram socket's inbox.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) struct Queue(usize);

impl Queue {
    pub(super) fn pipe(pipe: &Pipe) -> Queue {
        Queue(core::ptr::from_ref(pipe) as usize)
    }

    pub(super) fn inbox(inbox: &Inbox) -> Queue {
        Queue(core::ptr::from_ref(inbox) as usize)
    }
}

/// Checks the rule for each Unix socket among `files`, on their way into
/// `into`; called holding [`SENDING`]. A socket that cannot say which
/// queues it keeps yet, as `accept` is giving it its connection, fails
/// too.
pub(super) fn check(files: &Files, into: Queue) -> Result<(), Status> {
    for passed in files {
        let Some(socket) = passed.file.socket() else { continue };
        let mut refused = false;
        let said = socket.each_kept(&mut |queue, sockets| {
            refused |= queue == into || sockets > 0;
        });
        if !said || refused {
            return Err(Kernel::TooMany.status(0));
        }
    }
    Ok(())
}

/// `data` from byte `skip` on.
struct After<'a> {
    data: &'a dyn In,
    skip: usize,
}

impl In for After<'_> {
    fn len(&self) -> usize {
        self.data.len() - self.skip
    }

    fn get(&self, at: usize, out: &mut [u8]) -> radix_vfs::Result<()> {
        self.data.get(self.skip + at, out)
    }
}

/// Sends `data`, which holds a byte at least, on `end`'s connection with
/// `files`, among which a Unix socket rides, as `End::send` sends: its
/// first [`ATOMIC`] bytes go whole with the files once the rule lets them
/// in, and the rest after, as a write does. Waits for room unless
/// `non_blocking`.
pub(super) fn send_stream(
    end: &End,
    data: &dyn In,
    files: Files,
    non_blocking: bool,
    signal: bool,
) -> Result<usize, Status> {
    if end.shut_write.load(Ordering::Acquire) {
        return pipe::broken(0, signal);
    }
    debug_assert!(!data.is_empty(), "a stream's files ride on a byte");
    let first = data.len().min(ATOMIC);
    let mut copied = pipe::storage(first)?;
    data.get(0, &mut copied).map_err(crate::fs::status)?;
    let mut files = Some(files);
    loop {
        // Let go once the call returns, after the pipe's lock.
        let mut held = None;
        let wrote = end.tx.write_admitted(&Slice(&copied), &mut files, signal, &mut |sent| {
            let sending = hold();
            check(sent, Queue::pipe(&end.tx))?;
            held = Some(sending);
            Ok(())
        });
        drop(held);
        match wrote {
            Ok(_) => break,
            Err(err) if err == Kernel::WouldBlock.status(0) && !non_blocking => {
                end.tx.wait_writable(first)?;
            }
            Err(err) => return Err(err),
        }
    }
    if first == data.len() {
        return Ok(first);
    }
    // The rest as any write, which gives a short count rather than an
    // error once some has gone, and so raises no SIGPIPE, as on Linux.
    let rest = After { data, skip: first };
    Ok(first + end.tx.write_files(&rest, None, non_blocking, false).unwrap_or(0))
}
