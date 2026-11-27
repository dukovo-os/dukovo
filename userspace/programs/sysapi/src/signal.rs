// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Signals: a handler that records a signal's arrival for the program to
//! take when it next looks, and wakes a loop waiting in `poll` if asked;
//! ignoring a signal or giving it back its default, and putting back what
//! it did before; a thread that blocks signals and waits for one; and a
//! signal sent to the process itself.
//!
//! A handler runs between any two instructions of the thread it lands on,
//! so it may only do what is async-signal-safe. The one handler here
//! stores to an atomic and writes a byte to a pipe, and the program reads
//! the atomic in its own time: no program writes a handler of its own.
//! For a program that cannot get back to look, as a shell whose line
//! editor waits on a terminal that hung up, the handler can instead carry
//! out a [`Farewell`] made ready beforehand, and end the process.
//!
//! What each signal did when the program started is read before the
//! standard library's start-up changes it ([`ignored_at_start`]): Rust's
//! runtime ignores SIGPIPE before `main`, and a shell must know whether
//! its parent did.

use std::ffi::CString;
use std::io;
use std::os::fd::{AsRawFd, OwnedFd};
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicPtr, AtomicU64, AtomicUsize, Ordering};

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

    /// The flag the handler sets, for code that looks at a stop flag of
    /// its own while it works.
    pub fn flag(&self) -> &AtomicBool {
        &self.caught
    }

    /// Has the handler also write a byte to `write_end`, the write end of a
    /// pipe whose read end a loop waits on in `poll`, so that the loop
    /// wakes when the signal comes. The byte is the signal's number, so
    /// that `Caught`s of several signals may share one pipe and the loop
    /// still tell them apart. A full pipe already wakes it, so make the
    /// pipe non-blocking: the handler does not wait. The descriptor stays
    /// open for as long as the process runs, since a handler may be
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
    if signal == FAREWELL_SIGNAL.load(Ordering::SeqCst) {
        HANDLERS_IN_FAREWELL.fetch_add(1, Ordering::SeqCst);
        let plan = FAREWELL.load(Ordering::SeqCst);
        if !plan.is_null() {
            // SAFETY: a plan in FAREWELL is from `Box::into_raw`, and is not
            // freed while a handler counted in HANDLERS_IN_FAREWELL may read
            // it (`retire`).
            farewell(unsafe { &*plan });
        }
        HANDLERS_IN_FAREWELL.fetch_sub(1, Ordering::SeqCst);
    }
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
        // Every signal number is below SIGNALS, so it fits a byte.
        let byte = signal as u8;
        // SAFETY: __errno_location gives this thread's errno, and write
        // reads one byte from a local; both are async-signal-safe, and the
        // descriptor stays open for the process's life (`wake_through`).
        unsafe {
            let errno = *libc::__errno_location();
            libc::write(fd, (&raw const byte).cast(), 1);
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

/// What a [`Farewell`] does, made ready for the handler: the path as a C
/// string, so that the handler allocates nothing.
struct Plan {
    signals: Vec<(crate::Pid, c_int)>,
    append: Option<(CString, Vec<u8>)>,
    status: i32,
}

/// What the handler does at once when a signal comes, in place of
/// recording it, for a program that could not get back to look: each
/// signal sent, the bytes appended to the file, and the process ended
/// with the status. A shell readies one before its prompt, for SIGHUP,
/// with its jobs and its history (ADR 0267).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Farewell {
    /// Signals to send first, each to a process, or to a process group as
    /// a negative number, as kill(2) takes them.
    pub signals: Vec<(crate::Pid, c_int)>,
    /// A file to append to, made with mode 0600 if it is not there, and
    /// the bytes.
    pub append: Option<(std::path::PathBuf, Vec<u8>)>,
    /// The exit status.
    pub status: i32,
}

/// The signal the farewell in FAREWELL is for, or 0.
static FAREWELL_SIGNAL: AtomicI32 = AtomicI32::new(0);
/// The farewell made ready, from `Box::into_raw`, or null.
static FAREWELL: AtomicPtr<Plan> = AtomicPtr::new(std::ptr::null_mut());
/// How many handlers may be reading the farewell.
static HANDLERS_IN_FAREWELL: AtomicUsize = AtomicUsize::new(0);

/// Carries out `plan`, from the handler, with async-signal-safe calls
/// only, and ends the process.
fn farewell(plan: &Plan) -> ! {
    for &(target, signal) in &plan.signals {
        // SAFETY: kill takes integers and is async-signal-safe.
        unsafe { libc::kill(target, signal) };
    }
    if let Some((path, bytes)) = &plan.append {
        // SAFETY: open, write and close are async-signal-safe; the path is
        // a C string and the bytes are read for their length, both the
        // plan's, which lives while the handler reads it.
        unsafe {
            let fd = libc::open(
                path.as_ptr(),
                libc::O_WRONLY | libc::O_APPEND | libc::O_CREAT | libc::O_CLOEXEC,
                0o600 as libc::c_uint,
            );
            if fd >= 0 {
                libc::write(fd, bytes.as_ptr().cast(), bytes.len());
                libc::close(fd);
            }
        }
    }
    // SAFETY: _exit is async-signal-safe, and ends the process.
    unsafe { libc::_exit(plan.status) }
}

/// Frees `old`, a plan taken out of FAREWELL, once no handler reads it.
fn retire(old: *mut Plan) {
    if old.is_null() {
        return;
    }
    // A handler on another thread that counted itself before the plan was
    // taken out may still be reading it; one that counts itself after
    // finds the new one. A handler on this thread ran to its end already.
    while HANDLERS_IN_FAREWELL.load(Ordering::SeqCst) != 0 {
        std::hint::spin_loop();
    }
    // SAFETY: from `Box::into_raw`, out of FAREWELL, and no handler reads
    // it any more.
    drop(unsafe { Box::from_raw(old) });
}

/// Makes `farewell` ready for `signal`, which must be caught ([`catch`]):
/// from now on the handler carries it out when the signal comes, until
/// [`disarm_farewell`]. One farewell is ready at a time; this replaces an
/// earlier one.
///
/// # Errors
///
/// EINVAL for a number that is no signal; `ErrorKind::InvalidInput` for
/// a path with a NUL byte.
pub fn arm_farewell(signal: c_int, farewell: Farewell) -> io::Result<()> {
    if usize::try_from(signal).map_or(true, |index| index == 0 || index >= SIGNALS) {
        return Err(io::Error::from_raw_os_error(libc::EINVAL));
    }
    let append = match farewell.append {
        Some((path, bytes)) => Some((crate::c_path(&path)?, bytes)),
        None => None,
    };
    let plan = Box::new(Plan { signals: farewell.signals, append, status: farewell.status });
    retire(FAREWELL.swap(std::ptr::null_mut(), Ordering::SeqCst));
    FAREWELL_SIGNAL.store(signal, Ordering::SeqCst);
    FAREWELL.store(Box::into_raw(plan), Ordering::SeqCst);
    Ok(())
}

/// Puts the farewell away: the handler records its signal again.
pub fn disarm_farewell() {
    retire(FAREWELL.swap(std::ptr::null_mut(), Ordering::SeqCst));
}

/// What a signal does now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Disposition {
    /// Its default action (SIG_DFL).
    Default,
    /// Nothing (SIG_IGN).
    Ignored,
    /// [`catch`]'s handler records it.
    Caught,
    /// A handler of another part of the program, as a terminal library
    /// sets for SIGWINCH.
    Other,
}

/// What `signal` does now.
///
/// # Errors
///
/// EINVAL for a number that is no signal.
pub fn disposition(signal: c_int) -> io::Result<Disposition> {
    let handler = save(signal)?.action.sa_sigaction;
    Ok(match handler {
        libc::SIG_DFL => Disposition::Default,
        libc::SIG_IGN => Disposition::Ignored,
        h if h == record as extern "C" fn(c_int) as libc::sighandler_t => Disposition::Caught,
        _ => Disposition::Other,
    })
}

/// The signals that were ignored when the program started, a bit each.
static IGNORED_AT_START: AtomicU64 = AtomicU64::new(0);

/// Reads which signals are ignored, from `.init_array`, which the C
/// library runs before the standard library's start-up and `main`.
extern "C" fn read_ignored_at_start() {
    let mut ignored = 0u64;
    for signal in 1..SIGNALS as c_int {
        if save(signal).is_ok_and(|saved| saved.action.sa_sigaction == libc::SIG_IGN) {
            ignored |= 1 << (signal - 1);
        }
    }
    IGNORED_AT_START.store(ignored, Ordering::Relaxed);
}

#[used]
#[unsafe(link_section = ".init_array")]
static READ_IGNORED_AT_START: extern "C" fn() = read_ignored_at_start;

/// Whether `signal` was ignored when the program started, before Rust's
/// runtime ignored SIGPIPE: a non-interactive shell leaves such a signal
/// ignored (XCU 2.11), and gives SIGPIPE back its default otherwise, so
/// that a writer to a closed pipe ends.
#[must_use]
pub fn ignored_at_start(signal: c_int) -> bool {
    // The read above is in this crate's object only if something refers to
    // it, which this does.
    std::hint::black_box(&READ_IGNORED_AT_START);
    (1..SIGNALS as c_int).contains(&signal)
        && IGNORED_AT_START.load(Ordering::Relaxed) & (1 << (signal - 1)) != 0
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

/// What a signal did before a change, for [`Saved::restore`] to put
/// back: as a prompt catches Ctrl+C while it asks, and the program's own
/// action is back once it has its answer.
pub struct Saved {
    signal: c_int,
    action: libc::sigaction,
}

/// What `signal` does now, to put back later with [`Saved::restore`].
///
/// # Errors
///
/// EINVAL for a number that is no signal.
pub fn save(signal: c_int) -> io::Result<Saved> {
    // SAFETY: all zeros is a valid sigaction, which sigaction fills.
    let mut action: libc::sigaction = unsafe { std::mem::zeroed() };
    // SAFETY: with no new action sigaction changes nothing, and writes the
    // current one into `action`.
    checked(unsafe { libc::sigaction(signal, std::ptr::null(), &raw mut action) })?;
    Ok(Saved { signal, action })
}

impl Saved {
    /// The signal this is what of.
    #[must_use]
    pub fn signal(&self) -> c_int {
        self.signal
    }

    /// Has the signal do again what it did when this was saved.
    ///
    /// # Errors
    ///
    /// None in practice: the signal and its action were valid when they
    /// were saved.
    pub fn restore(&self) -> io::Result<()> {
        // SAFETY: the action is one sigaction gave for this signal, so its
        // handler is one the process had set: SIG_DFL, SIG_IGN, `record`,
        // or one the standard library or the C library set for itself.
        checked(unsafe {
            libc::sigaction(self.signal, &raw const self.action, std::ptr::null_mut())
        })?;
        Ok(())
    }
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

/// Unblocks `signals` for the calling thread, so that one that waits, or
/// comes, is delivered.
///
/// # Errors
///
/// EINVAL for a number that is no signal.
pub fn unblock(signals: &[c_int]) -> io::Result<()> {
    let set = set_of(signals)?;
    // SAFETY: pthread_sigmask reads the set and writes no old mask.
    match unsafe { libc::pthread_sigmask(libc::SIG_UNBLOCK, &raw const set, std::ptr::null_mut()) }
    {
        0 => Ok(()),
        errno => Err(io::Error::from_raw_os_error(errno)),
    }
}

/// Unblocks every signal for the calling thread, as a shell's child does
/// before it starts a program, which should not find any blocked that the
/// shell blocked for itself.
///
/// # Errors
///
/// None in practice: the empty set is valid.
pub fn unblock_all() -> io::Result<()> {
    let set = set_of(&[])?;
    // SAFETY: pthread_sigmask reads the set and writes no old mask.
    match unsafe { libc::pthread_sigmask(libc::SIG_SETMASK, &raw const set, std::ptr::null_mut()) }
    {
        0 => Ok(()),
        errno => Err(io::Error::from_raw_os_error(errno)),
    }
}

/// Sends `signal` to the calling thread, as raise(3) does: with its
/// default action, a program ends as the signal would end it, so that
/// what started it sees the same as for the program it ran.
///
/// # Errors
///
/// EINVAL for a number that is no signal.
pub fn raise(signal: c_int) -> io::Result<()> {
    // SAFETY: raise takes a number and touches no memory of the program's;
    // the handler it may run is one `set_action` or `Saved` set.
    match unsafe { libc::raise(signal) } {
        0 => Ok(()),
        _ => Err(io::Error::last_os_error()),
    }
}

/// Waits until a signal comes that runs a handler or ends the process,
/// as pause(2) does.
pub fn pause() {
    // SAFETY: pause takes nothing and returns when a handler has run.
    unsafe { libc::pause() };
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
    use std::sync::atomic::Ordering;

    use super::{Caught, catch, ignore, restore_default, save};
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
        assert_eq!(i32::from(byte[0]), libc::SIGUSR1, "the byte names the signal");
        assert!(CAUGHT.is_set());
        assert!(CAUGHT.flag().load(Ordering::SeqCst), "the flag is what the handler set");
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

    #[test]
    fn a_saved_action_comes_back() {
        static CAUGHT: Caught = Caught::new();
        // SIGURG's default is to do nothing, so a stray one harms no test.
        let before = save(libc::SIGURG).unwrap();
        assert_eq!(before.signal(), libc::SIGURG);
        catch(libc::SIGURG, &CAUGHT, 0).unwrap();
        let caught = save(libc::SIGURG).unwrap();
        before.restore().unwrap();
        process::kill(std::process::id() as i32, libc::SIGURG).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(50));
        assert!(!CAUGHT.take(), "the default action is back");
        caught.restore().unwrap();
        process::kill(std::process::id() as i32, libc::SIGURG).unwrap();
        for _ in 0..100 {
            if CAUGHT.is_set() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(CAUGHT.take(), "the handler is back");
        before.restore().unwrap();
    }
}
