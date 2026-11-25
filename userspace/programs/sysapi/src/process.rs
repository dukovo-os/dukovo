// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Processes, for what `std::process` has no call for: a fork, sessions and
//! a controlling terminal, waiting for and signalling a child by number,
//! the process's ids, a thread's name as the system keeps it, its
//! environment while it has one thread, and a fault on purpose.
//!
//! A fork, and a change to the environment, are sound only in a process
//! with one thread: a fork copies only the thread that calls it, so a lock
//! another thread held stays locked in the child for good, and a change to
//! the environment races any thread that reads it. Both count the
//! process's threads first, and refuse when there is more than one.

use std::ffi::OsStr;
use std::io;
use std::os::fd::{IntoRawFd, OwnedFd};
use std::os::unix::process::ExitStatusExt;
use std::process::ExitStatus;

use crate::{Pid, checked};

/// Which side of a fork a process is on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fork {
    /// The new process.
    Child,
    /// The process that forked, with its child's number.
    Parent(Pid),
}

/// Makes a copy of this process, as fork(2) does.
///
/// # Errors
///
/// EAGAIN when the system has no room for another process, and
/// `ErrorKind::Unsupported` when this process has more than one thread.
pub fn fork() -> io::Result<Fork> {
    single_threaded("fork")?;
    // SAFETY: this process has one thread, the caller, so the child is a
    // whole copy of it: no lock is held by a thread the child lacks.
    match checked(unsafe { libc::fork() })? {
        0 => Ok(Fork::Child),
        child => Ok(Fork::Parent(child)),
    }
}

/// Fails with `ErrorKind::Unsupported` unless this process has one thread;
/// `what` names the call that needs it. Only the calling thread can start
/// another, so the answer holds until it does.
fn single_threaded(what: &str) -> io::Result<()> {
    match thread_count()? {
        1 => Ok(()),
        threads => Err(io::Error::new(
            io::ErrorKind::Unsupported,
            format!("cannot {what}: the process has {threads} threads, and needs to have one"),
        )),
    }
}

/// How many of this process's threads have not ended.
#[cfg(target_os = "radix")]
fn thread_count() -> io::Result<u32> {
    use radix_abi::{process, startup};
    process::info(&crate::kernel::SYS, startup::SLOT_PROCESS)
        .map(|info| info.threads)
        .map_err(|status| io::Error::other(format!("the process's thread count: {status:?}")))
}

/// How many of this process's threads have not ended: Linux lists each in
/// /proc/self/task.
#[cfg(not(target_os = "radix"))]
fn thread_count() -> io::Result<u32> {
    let threads = std::fs::read_dir("/proc/self/task")?.count();
    u32::try_from(threads).map_err(io::Error::other)
}

/// Ends this process at once with exit status `code`, as _exit(2) does:
/// no destructor runs and no buffered output is written. A forked child
/// that cannot start its program ends this way, so that it does not write
/// out the copies of its parent's buffers.
pub fn exit_now(code: i32) -> ! {
    // SAFETY: _exit takes an integer and does not return.
    unsafe { libc::_exit(code) }
}

/// Makes this process the leader of a new session and of a new process
/// group, with no controlling terminal; returns the session's number.
///
/// # Errors
///
/// EPERM when this process leads a process group already.
pub fn new_session() -> io::Result<Pid> {
    // SAFETY: setsid takes no arguments.
    checked(unsafe { libc::setsid() })
}

/// Makes `terminal` this process's controlling terminal in a new session,
/// and its standard input, output and error, as login_tty(3) does for a
/// shell that is about to start: the call a forked child makes before
/// `exec`. On failure the descriptor may stay open, which a child that
/// then ends does not mind.
///
/// # Errors
///
/// EPERM when this process leads a process group, so cannot start a
/// session; ENOTTY when `terminal` is not a terminal.
pub fn login_terminal(terminal: OwnedFd) -> io::Result<()> {
    // SAFETY: login_tty takes over the descriptor, which `into_raw_fd`
    // gave up: it becomes 0, 1 and 2 and is closed under its own number.
    checked(unsafe { libc::login_tty(terminal.into_raw_fd()) })?;
    Ok(())
}

/// Waits for child `pid` to end, and returns how it ended. A signal that
/// interrupts the wait does not end it.
///
/// # Errors
///
/// ECHILD when `pid` is not a child of this process.
pub fn wait(pid: Pid) -> io::Result<ExitStatus> {
    let mut status = 0;
    loop {
        // SAFETY: waitpid writes the child's status into `status`.
        match checked(unsafe { libc::waitpid(pid, &raw mut status, 0) }) {
            Ok(_) => return Ok(ExitStatus::from_raw(status)),
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => return Err(error),
        }
    }
}

/// A child of this process that has ended, with how it ended, which it
/// takes out of the process table; none when no child has ended yet, or
/// this process has none.
#[must_use]
pub fn reap() -> Option<(Pid, ExitStatus)> {
    let mut status = 0;
    // SAFETY: waitpid writes the status of the child it reaps into
    // `status`; WNOHANG returns at once when none has ended.
    let pid = unsafe { libc::waitpid(-1, &raw mut status, libc::WNOHANG) };
    (pid > 0).then(|| (pid, ExitStatus::from_raw(status)))
}

/// Sends `signal` to process `pid`, or to process group `-pid`, as
/// kill(2) does.
///
/// # Errors
///
/// ESRCH when there is no such process, EPERM when this process may not
/// signal it, EINVAL for a number that is no signal.
pub fn kill(pid: Pid, signal: libc::c_int) -> io::Result<()> {
    // SAFETY: kill takes two integers.
    checked(unsafe { libc::kill(pid, signal) })?;
    Ok(())
}

/// The number of this process's process group.
#[must_use]
pub fn process_group() -> Pid {
    // SAFETY: getpgrp takes no arguments and cannot fail.
    unsafe { libc::getpgrp() }
}

/// The calling thread's name as the system keeps it, which
/// `std::thread::Builder::name` sets: its first 15 bytes, as
/// pthread_getname_np(3) gives them.
///
/// # Errors
///
/// What pthread_getname_np fails with, which it does not for the calling
/// thread.
pub fn thread_name() -> io::Result<Vec<u8>> {
    // The longest name and its NUL.
    let mut name = [0 as libc::c_char; 16];
    // SAFETY: pthread_getname_np writes at most the buffer's length into
    // it, ending the name with a NUL.
    match unsafe { libc::pthread_getname_np(libc::pthread_self(), name.as_mut_ptr(), name.len()) } {
        0 => Ok(crate::field_bytes(&name)),
        errno => Err(io::Error::from_raw_os_error(errno)),
    }
}

/// This process's real user id.
#[must_use]
pub fn uid() -> u32 {
    // SAFETY: getuid takes no arguments and cannot fail.
    unsafe { libc::getuid() }
}

/// This process's effective user id, which the kernel checks access with.
#[must_use]
pub fn euid() -> u32 {
    // SAFETY: geteuid takes no arguments and cannot fail.
    unsafe { libc::geteuid() }
}

/// This process's real group id.
#[must_use]
pub fn gid() -> u32 {
    // SAFETY: getgid takes no arguments and cannot fail.
    unsafe { libc::getgid() }
}

/// This process's effective group id.
#[must_use]
pub fn egid() -> u32 {
    // SAFETY: getegid takes no arguments and cannot fail.
    unsafe { libc::getegid() }
}

/// Sets this process's file mode creation mask to `mask`, and returns
/// the mask it had.
#[must_use = "the mask it had is how to put it back"]
pub fn umask(mask: libc::mode_t) -> libc::mode_t {
    // SAFETY: umask takes an integer and cannot fail.
    unsafe { libc::umask(mask) }
}

/// Sets environment variable `name` to `value` in this process's own
/// environment, which its children inherit.
///
/// # Errors
///
/// `ErrorKind::Unsupported` when this process has more than one thread,
/// and `ErrorKind::InvalidInput` for a name that is empty or holds `=` or
/// a NUL byte, or a value that holds a NUL byte.
pub fn set_env(name: &OsStr, value: &OsStr) -> io::Result<()> {
    valid_name(name)?;
    if value.as_encoded_bytes().contains(&0) {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "the value holds a NUL byte"));
    }
    single_threaded("set an environment variable")?;
    // SAFETY: this process has one thread, the caller, so no other reads
    // the environment while it changes; the name and value are valid.
    unsafe { std::env::set_var(name, value) };
    Ok(())
}

/// Takes environment variable `name` out of this process's environment.
///
/// # Errors
///
/// As [`set_env`]'s.
pub fn remove_env(name: &OsStr) -> io::Result<()> {
    valid_name(name)?;
    single_threaded("remove an environment variable")?;
    // SAFETY: this process has one thread, the caller, so no other reads
    // the environment while it changes; the name is valid.
    unsafe { std::env::remove_var(name) };
    Ok(())
}

/// Refuses a name the standard library would panic on.
fn valid_name(name: &OsStr) -> io::Result<()> {
    let bytes = name.as_encoded_bytes();
    if bytes.is_empty() || bytes.contains(&b'=') || bytes.contains(&0) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "an environment variable's name is empty, or holds = or a NUL byte",
        ));
    }
    Ok(())
}

/// Writes to address 0, which nothing maps, so that the kernel ends the
/// process with SIGSEGV where this is called; for the test programs of
/// crash reports. It is always inlined, so the fault is in the caller's
/// own frame.
#[inline(always)]
pub fn fault() -> ! {
    // SAFETY: nothing is mapped at address 0, so the store faults before
    // any memory changes, and the kernel ends the process; a handler that
    // returns from the fault runs the store again. It is volatile, so the
    // compiler keeps it.
    unsafe { std::ptr::null_mut::<u32>().write_volatile(42) };
    unreachable!("the store to address 0 ends the process")
}

#[cfg(test)]
mod tests {
    use super::{euid, fork, set_env, thread_count, thread_name, uid};
    use std::ffi::OsStr;

    #[test]
    fn a_thread_has_the_name_it_was_given() {
        let named = std::thread::Builder::new().name("sysapi-named".into()).spawn(thread_name);
        assert_eq!(named.unwrap().join().unwrap().unwrap(), b"sysapi-named");
    }

    #[test]
    fn the_ids_are_the_c_library_s() {
        let status = std::fs::read_to_string("/proc/self/status").unwrap();
        let line = status.lines().find(|line| line.starts_with("Uid:")).unwrap();
        let ids: Vec<u32> = line.split_whitespace().skip(1).map(|id| id.parse().unwrap()).collect();
        assert_eq!((uid(), euid()), (ids[0], ids[1]));
    }

    #[test]
    fn a_process_with_threads_is_not_forked() {
        // The test harness runs each test on a thread of its own.
        assert!(thread_count().unwrap() > 1);
        let error = fork().unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::Unsupported);
        let error = set_env(OsStr::new("SYSAPI_TEST"), OsStr::new("1")).unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::Unsupported);
    }

    #[test]
    fn a_bad_name_is_refused_before_the_threads_are_counted() {
        for name in ["", "A=B", "A\0B"] {
            let error = set_env(OsStr::new(name), OsStr::new("1")).unwrap_err();
            assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput, "{name:?}");
        }
    }
}
