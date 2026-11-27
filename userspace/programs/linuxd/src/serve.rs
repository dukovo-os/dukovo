// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The calls the kernel forwards (ADR 0618): workers that receive them on
//! the endpoint in SLOT_LINUX and answer each, and the thread that serves
//! the filesystem.
//!
//! Each worker receives a call, takes the call's own capability and
//! answers through it ([`Call`]), lets go of the capability, and replies with
//! Linux's result in the reply's first word, receiving the next call in
//! the same system call. A handler never waits: a call that would wait
//! answers as a non-blocking one does, so that a worker is never held by
//! one program. linuxd ends when the filesystem does: when the sandbox's
//! last program has gone and the kernel lets go of the mount.

use std::sync::Arc;

use radix_abi::linux_call::FORWARDED;
use radix_abi::{startup, syscalls};

use crate::call::{Answer, Call, Process};
use crate::fs;
use crate::netlink::{self, Netlinks};
use crate::sys::{SYS, errno_of};

/// The workers that answer calls: one program's threads seldom wait on
/// linuxd at once, and the calls it answers are short.
const WORKERS: u32 = 3;
/// The first of the workers' slots, two each (the call's capability, and
/// one for the call's own use), empty between calls: the slots after
/// SLOT_LINUX, which the C library never takes (linuxd is never given a
/// jit capability, so SLOT_JIT is free too).
const SLOTS_FIRST: u32 = startup::SLOT_LINUX + 1;
const _: () = assert!(SLOTS_FIRST + 2 * WORKERS <= startup::SLOT_FD_BASE);

/// The descriptor of the `/devices/fuse` connection, from `linux run`.
const CONNECTION: i32 = 3;
/// The descriptor of the filesystem's root, from `linux run`.
const ROOT: i32 = 4;

/// Linux's x86-64 call numbers that linuxd answers, from Linux's call
/// table (`arch/x86/entry/syscalls/syscall_64.tbl`).
pub mod nr {
    /// `socket`.
    pub const SOCKET: u64 = 41;
    /// `connect`.
    pub const CONNECT: u64 = 42;
    /// `sendto`.
    pub const SENDTO: u64 = 44;
    /// `recvfrom`.
    pub const RECVFROM: u64 = 45;
    /// `sendmsg`.
    pub const SENDMSG: u64 = 46;
    /// `recvmsg`.
    pub const RECVMSG: u64 = 47;
    /// `shutdown`.
    pub const SHUTDOWN: u64 = 48;
    /// `bind`.
    pub const BIND: u64 = 49;
    /// `getsockname`.
    pub const GETSOCKNAME: u64 = 51;
    /// `getpeername`.
    pub const GETPEERNAME: u64 = 52;
    /// `setsockopt`.
    pub const SETSOCKOPT: u64 = 54;
    /// `getsockopt`.
    pub const GETSOCKOPT: u64 = 55;
    /// `personality`.
    pub const PERSONALITY: u64 = 135;
}

/// Linux's `AF_NETLINK`.
const AF_NETLINK: u64 = 16;
/// `personality(0xffffffff)` asks for the persona without changing it.
const PERSONALITY_QUERY: u64 = 0xffff_ffff;

/// Serves until the filesystem ends; returns the exit status.
pub fn run(again: bool) -> i32 {
    let handed = sysapi::fd::handed_down(CONNECTION)
        .and_then(|connection| Ok((connection, sysapi::fd::handed_down(ROOT)?)));
    let (connection, root) = match handed {
        Ok(both) => both,
        Err(error) => {
            eprintln!("linuxd: descriptors 3 and 4 are not what linux run hands linuxd: {error}");
            return 2;
        }
    };
    let netlinks = Arc::new(Netlinks::new(Box::new(netlink::RootFiles::new(root))));
    for index in 0..WORKERS {
        let netlinks = Arc::clone(&netlinks);
        let spawned = std::thread::Builder::new()
            .name(format!("worker {index}"))
            .spawn(move || work(SLOTS_FIRST + 2 * index, &netlinks));
        if let Err(error) = spawned {
            eprintln!("linuxd: cannot start a worker: {error}");
            return 1;
        }
    }
    match fs::serve(fs::LinuxFs::new(netlinks), connection, again) {
        Ok(()) => 0,
        Err(error) => {
            eprintln!("linuxd: the filesystem stopped: {error}");
            1
        }
    }
}

/// The info word of a reply with one word and no capabilities.
const ANSWER_INFO: u64 = 1 << 32;

/// A worker, whose slots are `slots` and the one after: receives calls
/// and answers them, for as long as linuxd runs.
fn work(slots: u32, netlinks: &Netlinks) -> ! {
    let mut answer: Option<i64> = None;
    loop {
        let received = match answer.take() {
            None => syscalls::recv(&SYS, startup::SLOT_LINUX, 0).map(|m| (m.info, m.words)),
            Some(value) => {
                let mut words = [0u64; 8];
                words[0] = value as u64;
                syscalls::reply_recv(&SYS, startup::SLOT_LINUX, ANSWER_INFO, words)
                    .map(|m| (m.info, m.words))
            }
        };
        let (info, words) = match received {
            Ok(message) => message,
            Err(status) => {
                // An interrupted wait, or a caller gone before its answer:
                // wait for the next call.
                if errno_of(status) != libc::EINTR {
                    std::thread::sleep(std::time::Duration::from_millis(10));
                }
                continue;
            }
        };
        if info & 0xffff_ffff != u64::from(FORWARDED) {
            // Not a call the kernel forwarded: nothing to act on.
            answer = Some(-i64::from(libc::ENOSYS));
            continue;
        }
        let args: [u64; 6] = core::array::from_fn(|i| words[1 + i]);
        answer = Some(match Call::received(slots, slots + 1) {
            Ok((mut call, number)) => result(handle(&mut call, number, &args, netlinks)),
            Err(errno) => -i64::from(errno),
        });
    }
}

/// rax for an answer.
fn result(answer: Answer) -> i64 {
    answer.unwrap_or_else(|errno| -i64::from(errno))
}

/// Answers call `number` with `args`, made by `process`.
pub fn handle(
    process: &mut dyn Process,
    number: u64,
    args: &[u64; 6],
    netlinks: &Netlinks,
) -> Answer {
    match number {
        nr::SOCKET if args[0] == AF_NETLINK => netlinks.socket(process, args),
        // Families the kernel does not know.
        nr::SOCKET => Err(libc::EAFNOSUPPORT),
        nr::CONNECT
        | nr::SENDTO
        | nr::RECVFROM
        | nr::SENDMSG
        | nr::RECVMSG
        | nr::SHUTDOWN
        | nr::BIND
        | nr::GETSOCKNAME
        | nr::GETPEERNAME
        | nr::SETSOCKOPT
        | nr::GETSOCKOPT => netlinks.call(process, number, args),
        nr::PERSONALITY => personality(args[0]),
        _ => Err(libc::ENOSYS),
    }
}

/// personality(2): every program is PER_LINUX, the persona Linux gives
/// unless asked for another; asking which, or for PER_LINUX, answers it.
/// Another persona, or PER_LINUX with flags such as ADDR_NO_RANDOMIZE
/// that Dukovo does not carry out, is refused with EINVAL, so that
/// setarch(8) says it could not rather than run the program as it was.
fn personality(persona: u64) -> Answer {
    match persona {
        PERSONALITY_QUERY | 0 => Ok(0),
        _ => Err(libc::EINVAL),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::call::{Fake, Who};

    #[test]
    fn personality_is_linux_and_nothing_else() {
        assert_eq!(personality(PERSONALITY_QUERY), Ok(0));
        assert_eq!(personality(0), Ok(0));
        assert_eq!(personality(0x0040000), Err(libc::EINVAL));
    }

    #[test]
    fn an_unknown_call_is_enosys() {
        let netlinks = Netlinks::new(Box::new(netlink::NoFiles));
        let mut process = Fake::new(Who::default(), 0x1000, 64);
        assert_eq!(handle(&mut process, 999, &[0; 6], &netlinks), Err(libc::ENOSYS));
        assert_eq!(
            handle(&mut process, nr::SOCKET, &[2, 1, 0, 0, 0, 0], &netlinks),
            Err(libc::EAFNOSUPPORT)
        );
    }
}
