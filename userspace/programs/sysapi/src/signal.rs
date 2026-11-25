// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Signals: a handler that records a signal's arrival for the program to
//! take when it next looks, and wakes a loop waiting in `poll` if asked;
//! ignoring a signal or giving it back its default; and a thread that
//! blocks signals and waits for one.
//!
//! A handler runs between any two instructions of the thread it lands on,
//! so it may only do what is async-signal-safe. The one handler here
//! stores to an atomic and writes a byte to a pipe, and the program reads
//! the atomic in its own time: no program writes a handler of its own.

use std::io;
use std::os::fd::{AsRawFd, OwnedFd};
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicPtr, Ordering};

use libc::c_int;

use crate::checked;

/// One more than the highest signal number, as Linux's and Radix's
/// `_NSIG`: signals 1 to 64.
const SIGNALS: usize = 65;

/// The flags [`catch`] takes: what a caller may change of how the handler
/// runs, none of which changes what it does.
const FLAGS: c_int = libc::SA_RESTART | libc::SA_NOCLDSTOP;

/// The arrival of a signal [`catch`] took, kept in a `static` the program
/// owns: `static RELOAD: Caught = Caught::new();`.
pub struct Caught {
    caught: AtomicBool,
    /// The write end of the pipe the handler wakes a loop through, or -1.
    wake: AtomicI32,
}

impl Caught {
    /// A signal that has not come.
    #[must_use]
    pub const fn new() -> Self {
        Caught { caught: AtomicBool::new(false), wake: AtomicI32::new(-1) }
    }

    /// Whether the signal came since this was last asked, which it clears.
    pub fn take(&self) -> bool {
        self.caught.swap(false, Ordering::SeqCst)
    }

    /// Whether the signal came, which it leaves set.
    pub fn is_set(&self) -> bool {
        self.caught.load(Ordering::SeqCst)
    }

    /// Has the handler also write a byte to `write_end`, the write end of a
    /// pipe whose read end a loop waits on in `poll`, so that the loop
    /// wakes when the signal comes. A full pipe already wakes it, so make
    /// the pipe non-blocking: the handler does not wait. The descriptor
    /// stays open for as long as the process runs, since a handler may be
    /// writing to it at any time.
    ///
    /// # Errors
    ///
    /// `ErrorKind::AlreadyExists` when this was given a pipe already;
    /// `write_end` is then closed.
    pub fn wake_through(&self, write_end: OwnedFd) -> io::Result<()> {
        let fd = write_end.as_raw_fd();
        self.wake.compare_exchange(-1, fd, Ordering::SeqCst, Ordering::SeqCst).map_err(|_| {
            io::Error::new(io::ErrorKind::AlreadyExists, "the signal wakes through a pipe already")
        })?;
        // The handler may write to the descriptor at any time from here on,
        // so it is never closed.
        std::mem::forget(write_end);
        Ok(())
    }
}

impl Default for Caught {
    fn default() -> Self {
        Caught::new()
    }
}

/// The record each caught signal's handler keeps: a pointer from a
/// `&'static Caught`, or null.
static CAUGHT: [AtomicPtr<Caught>; SIGNALS] =
    [const { AtomicPtr::new(std::ptr::null_mut()) }; SIGNALS];

/// The handler [`catch`] installs.
extern "C" fn record(signal: c_int) {
    let Some(slot) = usize::try_from(signal).ok().and_then(|index| CAUGHT.get(index)) else {
        return;
    };
    let caught = slot.load(Ordering::SeqCst);
    if caught.is_null() {
        return;
    }
    // SAFETY: a pointer in CAUGHT is from a `&'static Caught`, which lives
    // for as long as the process.
    let caught = unsafe { &*caught };
    caught.caught.store(true, Ordering::SeqCst);
    let fd = caught.wake.load(Ordering::SeqCst);
    if fd >= 0 {
        // The handler may land between a call that failed and the
        // program's read of errno, which write must not change.
        // SAFETY: __errno_location gives this thread's errno, and write
        // reads one byte from a local; both are async-signal-safe, and the
        // descriptor stays open for the process's life (`wake_through`).
        unsafe {
            let errno = *libc::__errno_location();
            libc::write(fd, [0u8].as_ptr().cast(), 1);
            *libc::__errno_location() = errno;
        }
    }
}

/// Has `signal` set `caught` when it comes, instead of what it did before;
/// `flags` may hold SA_RESTART, so that a call it interrupts goes on
/// rather than fail with EINTR, and SA_NOCLDSTOP, so that a child that
/// stops is no SIGCHLD.
///
/// # Errors
///
/// EINVAL for a number that is no signal, for SIGKILL or SIGSTOP, which
/// cannot be caught, or for another flag.
pub fn catch(signal: c_int, caught: &'static Caught, flags: c_int) -> io::Result<()> {
    let slot = usize::try_from(signal).ok().and_then(|index| CAUGHT.get(index));
    let Some(slot) = slot.filter(|_| signal > 0 && flags & !FLAGS == 0) else {
        return Err(io::Error::from_raw_os_error(libc::EINVAL));
    };
    slot.store(std::ptr::from_ref(caught).cast_mut(), Ordering::SeqCst);
    set_action(signal, record as extern "C" fn(c_int) as libc::sighandler_t, flags)
}

/// Has `signal` do nothing when it comes, as SIG_IGN; an ignored signal
/// stays ignored in a program this one starts.
///
/// # Errors
///
/// EINVAL for a number that is no signal, and for SIGKILL or SIGSTOP.
pub fn ignore(signal: c_int) -> io::Result<()> {
    set_action(signal, libc::SIG_IGN, 0)
}

/// Gives `signal` back the action it has by default (SIG_DFL), as a child
/// does before it starts a program that expects it.
///
/// # Errors
///
/// EINVAL for a number that is no signal.
pub fn restore_default(signal: c_int) -> io::Result<()> {
    set_action(signal, libc::SIG_DFL, 0)
}

fn set_action(signal: c_int, handler: libc::sighandler_t, flags: c_int) -> io::Result<()> {
    // SAFETY: all zeros is a valid sigaction: numbers, a signal set and a
    // function address, which is set next.
    let mut action: libc::sigaction = unsafe { std::mem::zeroed() };
    action.sa_sigaction = handler;
    action.sa_flags = flags;
    // SAFETY: the handler is SIG_IGN, SIG_DFL or `record`, which does only
    // what is async-signal-safe; sigaction reads the action it is given
    // and writes no old one.
    checked(unsafe { libc::sigaction(signal, &raw const action, std::ptr::null_mut()) })?;
    Ok(())
}

/// The set of `signals`.
fn set_of(signals: &[c_int]) -> io::Result<libc::sigset_t> {
    // SAFETY: all zeros is a valid sigset_t, which sigemptyset then sets.
    let mut set: libc::sigset_t = unsafe { std::mem::zeroed() };
    // SAFETY: sigemptyset writes the set it is given.
    checked(unsafe { libc::sigemptyset(&raw mut set) })?;
    for &signal in signals {
        // SAFETY: sigaddset writes the set it is given; it fails with
        // EINVAL for a number that is no signal.
        checked(unsafe { libc::sigaddset(&raw mut set, signal) })?;
    }
    Ok(set)
}

/// Blocks `signals` for the calling thread, and the threads it starts
/// after: one that comes waits until a thread takes it with [`wait`].
///
/// # Errors
///
/// EINVAL for a number that is no signal.
pub fn block(signals: &[c_int]) -> io::Result<()> {
    let set = set_of(signals)?;
    // SAFETY: pthread_sigmask reads the set and writes no old mask.
    match unsafe { libc::pthread_sigmask(libc::SIG_BLOCK, &raw const set, std::ptr::null_mut()) } {
        0 => Ok(()),
        errno => Err(io::Error::from_raw_os_error(errno)),
    }
}

/// Waits for one of `signals`, which every thread blocks ([`block`]),
/// and returns which came.
///
/// # Errors
///
/// EINVAL for a number that is no signal.
pub fn wait(signals: &[c_int]) -> io::Result<c_int> {
    let set = set_of(signals)?;
    let mut signal = 0;
    // SAFETY: sigwait reads the set and writes the signal that came.
    match unsafe { libc::sigwait(&raw const set, &raw mut signal) } {
        0 => Ok(signal),
        errno => Err(io::Error::from_raw_os_error(errno)),
    }
}

#[cfg(test)]
mod tests {
    use std::os::fd::AsFd;

    use super::{Caught, catch, ignore, restore_default};
    use crate::{fd, process};

    #[test]
    fn a_caught_signal_is_taken_once_and_wakes_the_pipe() {
        static CAUGHT: Caught = Caught::new();
        let (from, to) = fd::pipe().unwrap();
        fd::set_nonblocking(&to, true).unwrap();
        CAUGHT.wake_through(to).unwrap();
        catch(libc::SIGUSR1, &CAUGHT, libc::SA_RESTART).unwrap();
        assert!(!CAUGHT.take());
        process::kill(std::process::id() as i32, libc::SIGUSR1).unwrap();
        // The signal is handled before kill returns to a process's own
        // thread, or on another thread a moment later.
        let mut byte = [0; 1];
        crate::poll::wait_readable(from.as_fd(), std::time::Duration::from_secs(5)).unwrap();
        assert_eq!(fd::read(&from, &mut byte).unwrap(), 1);
        assert!(CAUGHT.is_set());
        assert!(CAUGHT.take());
        assert!(!CAUGHT.take());
        restore_default(libc::SIGUSR1).unwrap();
    }

    #[test]
    fn a_second_pipe_is_refused() {
        static CAUGHT: Caught = Caught::new();
        CAUGHT.wake_through(fd::pipe().unwrap().1).unwrap();
        let error = CAUGHT.wake_through(fd::pipe().unwrap().1).unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::AlreadyExists);
    }

    #[test]
    fn what_cannot_be_caught_is_refused() {
        static CAUGHT: Caught = Caught::new();
        for (signal, flags) in
            [(0, 0), (65, 0), (-1, 0), (libc::SIGKILL, 0), (libc::SIGUSR2, libc::SA_SIGINFO)]
        {
            let error = catch(signal, &CAUGHT, flags).unwrap_err();
            assert_eq!(error.raw_os_error(), Some(libc::EINVAL), "{signal} {flags}");
        }
        assert_eq!(ignore(libc::SIGKILL).unwrap_err().raw_os_error(), Some(libc::EINVAL));
    }
}
