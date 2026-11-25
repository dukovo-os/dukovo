// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Processes, for what `std::process` has no call for: a fork, sessions and
//! a controlling terminal, waiting for and signalling a child by number,
//! the process's ids, its scheduling priority and limits, a thread's name
//! as the system keeps it, its environment while it has one thread, and a
//! fault on purpose.
//!
//! A fork, and a change to the environment, are sound only in a process
//! with one thread: a fork copies only the thread that calls it, so a lock
//! another thread held stays locked in the child for good, and a change to
//! the environment races any thread that reads it. Both count the
//! process's threads first, and refuse when there is more than one. A
//! thread the caller has joined is still counted for a moment while the
//! kernel finishes ending it, so the count is read again, for up to
//! [`ENDING_THREADS_WAIT`], before a call is refused.

use std::ffi::{CStr, CString, OsStr};
use std::io;
use std::os::fd::{IntoRawFd, OwnedFd};
use std::os::unix::process::ExitStatusExt;
use std::process::ExitStatus;
use std::time::{Duration, Instant};

use crate::{Pid, checked};

/// How long a call that needs one thread waits for the process's other
/// threads to finish ending, before it is refused: a joined thread is
/// listed for some microseconds after the join returns, and one that is
/// still running is not waited for long.
pub const ENDING_THREADS_WAIT: Duration = Duration::from_millis(100);

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
/// `ErrorKind::Unsupported` when this process has more than one thread
/// once [`ENDING_THREADS_WAIT`] has passed.
pub fn fork() -> io::Result<Fork> {
    single_threaded("fork")?;
    // SAFETY: this process has one thread, the caller, so the child is a
    // whole copy of it: no lock is held by a thread the child lacks.
    match checked(unsafe { libc::fork() })? {
        0 => Ok(Fork::Child),
        child => Ok(Fork::Parent(child)),
    }
}

/// Fails with `ErrorKind::Unsupported` unless this process has one thread
/// within [`ENDING_THREADS_WAIT`]; `what` names the call that needs it.
/// Only the calling thread can start another, so the answer holds until
/// it does.
pub(crate) fn single_threaded(what: &str) -> io::Result<()> {
    let started = Instant::now();
    loop {
        let threads = thread_count()?;
        if threads == 1 {
            return Ok(());
        }
        if started.elapsed() >= ENDING_THREADS_WAIT {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                format!("cannot {what}: the process has {threads} threads, and needs to have one"),
            ));
        }
        std::thread::yield_now();
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

/// Starts the program at `path` in place of this process, as execve(2)
/// does, with `args` as its arguments (the first its name) and `env` as
/// its environment; it returns only when the program could not start.
///
/// Unlike `std::process::Command`, it keeps the signal mask and the
/// signals this process ignores, as a shell's children keep them
/// (XCU 2.11), and every descriptor without close-on-exec: those a shell
/// put at its numbers ([`numbered`](crate::numbered)), with a standard
/// one it closed closed.
pub fn exec(path: &CStr, args: &[CString], env: &[CString]) -> io::Error {
    let argv: Vec<*const libc::c_char> =
        args.iter().map(|arg| arg.as_ptr()).chain(std::iter::once(std::ptr::null())).collect();
    let envp: Vec<*const libc::c_char> =
        env.iter().map(|entry| entry.as_ptr()).chain(std::iter::once(std::ptr::null())).collect();
    let started = crate::numbered::with_closed_standard_closed(|| {
        // SAFETY: both arrays end with a null pointer and point at C
        // strings that live through the call; execve returns only when it
        // fails, and errno is read before anything else runs.
        unsafe { libc::execve(path.as_ptr(), argv.as_ptr(), envp.as_ptr()) };
        io::Error::last_os_error()
    });
    match started {
        Ok(error) | Err(error) => error,
    }
}

/// Puts process `pid` (0 for the caller) in process group `group` (0 for
/// a new one numbered as `pid`), as setpgid(2) does: a shell puts each
/// job's processes in a group of their own, from the parent and from the
/// child both, so that neither waits for the other.
///
/// # Errors
///
/// EACCES for a child that has started its program already, EPERM for a
/// group in another session, ESRCH for a process that is not the caller
/// or its child.
pub fn set_process_group(pid: Pid, group: Pid) -> io::Result<()> {
    // SAFETY: setpgid takes two integers and touches no memory.
    checked(unsafe { libc::setpgid(pid, group) })?;
    Ok(())
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

/// How child `pid` ended, if it has, which takes it out of the process
/// table; none while it runs.
///
/// # Errors
///
/// ECHILD when `pid` is not a child of this process.
pub fn try_wait(pid: Pid) -> io::Result<Option<ExitStatus>> {
    let mut status = 0;
    // SAFETY: waitpid writes the child's status into `status`; WNOHANG
    // returns at once while it runs.
    let reaped = checked(unsafe { libc::waitpid(pid, &raw mut status, libc::WNOHANG) })?;
    Ok((reaped > 0).then(|| ExitStatus::from_raw(status)))
}

/// Waits for any child of this process to end, and returns which and
/// how. A signal that interrupts the wait does not end it.
///
/// # Errors
///
/// ECHILD when this process has no child.
pub fn wait_any() -> io::Result<(Pid, ExitStatus)> {
    let mut status = 0;
    loop {
        // SAFETY: waitpid writes the status of the child it reaps into
        // `status`.
        match checked(unsafe { libc::waitpid(-1, &raw mut status, 0) }) {
            Ok(pid) => return Ok((pid, ExitStatus::from_raw(status))),
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
    reap_with(libc::WNOHANG)
}

/// As [`reap`], and a child that a signal stopped too, which stays in the
/// table: its status's `stopped_signal` (`ExitStatusExt`) says which
/// signal, as a shell sees Ctrl+Z stop a job.
#[must_use]
pub fn reap_or_stopped() -> Option<(Pid, ExitStatus)> {
    reap_with(libc::WNOHANG | libc::WUNTRACED)
}

/// Waits as waitpid(2) does with `flags` (WNOHANG, WUNTRACED,
/// WCONTINUED), for child `pid`, any child (-1), or one in process group
/// `-pid`: the child and how it changed, or `None` with WNOHANG when none
/// has. A signal that comes meanwhile ends the wait with EINTR, for a
/// shell to run its trap.
///
/// # Errors
///
/// ECHILD when there is no such child, EINTR when a signal came first.
pub fn wait_with(pid: Pid, flags: libc::c_int) -> io::Result<Option<(Pid, ExitStatus)>> {
    if flags & !(libc::WNOHANG | libc::WUNTRACED | libc::WCONTINUED) != 0 {
        return Err(io::Error::from_raw_os_error(libc::EINVAL));
    }
    let mut status = 0;
    // SAFETY: waitpid writes the status of the child it reports into
    // `status`.
    let got = checked(unsafe { libc::waitpid(pid, &raw mut status, flags) })?;
    Ok((got > 0).then(|| (got, ExitStatus::from_raw(status))))
}

fn reap_with(options: libc::c_int) -> Option<(Pid, ExitStatus)> {
    let mut status = 0;
    // SAFETY: waitpid writes the status of the child it reports into
    // `status`; WNOHANG returns at once when there is none.
    let pid = unsafe { libc::waitpid(-1, &raw mut status, options) };
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

/// Names the calling thread, as `std::thread::Builder::name` names a
/// thread it starts: the system keeps the first 15 bytes, which ps(1) and
/// a crash report show.
///
/// # Errors
///
/// `ErrorKind::InvalidInput` for a name with a NUL byte.
pub fn set_thread_name(name: &str) -> io::Result<()> {
    let mut end = name.len().min(15);
    while !name.is_char_boundary(end) {
        end -= 1;
    }
    let name = crate::c_string(&name.as_bytes()[..end])?;
    // SAFETY: the name is a C string of at most 15 bytes, which
    // pthread_setname_np copies.
    match unsafe { libc::pthread_setname_np(libc::pthread_self(), name.as_ptr()) } {
        0 => Ok(()),
        errno => Err(io::Error::from_raw_os_error(errno)),
    }
}

/// Which processes [`priority`] and [`set_priority`] are of.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Who {
    /// Process `n`, or this one for 0 (PRIO_PROCESS).
    Process(u32),
    /// The processes of process group `n`, or of this one's for 0
    /// (PRIO_PGRP).
    Group(u32),
    /// The processes of user `n`, or of this one's real user for 0
    /// (PRIO_USER).
    User(u32),
}

impl Who {
    fn which(self) -> (libc::c_int, libc::id_t) {
        match self {
            Who::Process(n) => (libc::PRIO_PROCESS as libc::c_int, n),
            Who::Group(n) => (libc::PRIO_PGRP as libc::c_int, n),
            Who::User(n) => (libc::PRIO_USER as libc::c_int, n),
        }
    }
}

/// The nice value of `who`, from -20 to 19, as getpriority(2) gives it:
/// the highest of their priorities, the lowest number, for several.
///
/// # Errors
///
/// ESRCH when there is no such process.
pub fn priority(who: Who) -> io::Result<i32> {
    let (which, id) = who.which();
    // getpriority may return -1 as a value, so errno tells a failure.
    // SAFETY: __errno_location gives this thread's errno.
    unsafe { *libc::__errno_location() = 0 };
    // SAFETY: getpriority takes two integers.
    let value = unsafe { libc::getpriority(which as _, id) };
    match io::Error::last_os_error() {
        error if value == -1 && error.raw_os_error() != Some(0) => Err(error),
        _ => Ok(value),
    }
}

/// Sets the nice value of `who` to `value`, as setpriority(2) does: from
/// -20, the most CPU time, to 19, the least.
///
/// # Errors
///
/// ESRCH when there is no such process; EACCES or EPERM to raise a
/// priority without the right, or to change another user's.
pub fn set_priority(who: Who, value: i32) -> io::Result<()> {
    let (which, id) = who.which();
    // SAFETY: setpriority takes three integers.
    checked(unsafe { libc::setpriority(which as _, id, value) })?;
    Ok(())
}

/// A limit of this process's (getrlimit(2)): its soft limit, which the
/// kernel holds it to, and the hard limit it may raise the soft one to;
/// `u64::MAX` is no limit.
///
/// # Errors
///
/// EINVAL for a number that is no resource.
pub fn limit(resource: libc::c_int) -> io::Result<(u64, u64)> {
    let mut limit = libc::rlimit { rlim_cur: 0, rlim_max: 0 };
    // SAFETY: getrlimit writes the rlimit it is given.
    checked(unsafe { libc::getrlimit(resource as _, &raw mut limit) })?;
    Ok((limit.rlim_cur, limit.rlim_max))
}

/// Sets a limit of this process's, `soft` and `hard`, as setrlimit(2)
/// does; its children inherit it.
///
/// # Errors
///
/// EINVAL for a number that is no resource or a soft limit above the hard
/// one, and EPERM to raise the hard limit without the right.
pub fn set_limit(resource: libc::c_int, soft: u64, hard: u64) -> io::Result<()> {
    let limit = libc::rlimit { rlim_cur: soft, rlim_max: hard };
    // SAFETY: setrlimit reads the rlimit it is given.
    checked(unsafe { libc::setrlimit(resource as _, &raw const limit) })?;
    Ok(())
}

/// Whose use of the machine [`usage`] counts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UsageOf {
    /// This process's own.
    Process,
    /// Its children's that it waited for.
    Children,
}

/// What a process used of the machine, as getrusage(2) counts it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Usage {
    /// Time on a CPU in user mode.
    pub user: Duration,
    /// Time on a CPU in the kernel.
    pub system: Duration,
    /// Times it gave up the CPU to wait.
    pub voluntary_switches: u64,
    /// Times another process took the CPU from it.
    pub involuntary_switches: u64,
}

/// What `of` used of the machine so far.
///
/// # Errors
///
/// None that the kernel gives for these two.
pub fn usage(of: UsageOf) -> io::Result<Usage> {
    let who = match of {
        UsageOf::Process => libc::RUSAGE_SELF,
        UsageOf::Children => libc::RUSAGE_CHILDREN,
    };
    // SAFETY: all zeros is a valid rusage: numbers and timevals.
    let mut usage: libc::rusage = unsafe { std::mem::zeroed() };
    // SAFETY: getrusage writes the rusage it is given.
    checked(unsafe { libc::getrusage(who, &raw mut usage) })?;
    let time = |t: libc::timeval| {
        Duration::new(
            u64::try_from(t.tv_sec).unwrap_or(0),
            u32::try_from(t.tv_usec).unwrap_or(0) * 1000,
        )
    };
    Ok(Usage {
        user: time(usage.ru_utime),
        system: time(usage.ru_stime),
        voluntary_switches: u64::try_from(usage.ru_nvcsw).unwrap_or(0),
        involuntary_switches: u64::try_from(usage.ru_nivcsw).unwrap_or(0),
    })
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
/// `ErrorKind::Unsupported` when this process has more than one thread
/// once [`ENDING_THREADS_WAIT`] has passed, and `ErrorKind::InvalidInput`
/// for a name that is empty or holds `=` or
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

/// The registers that place the calling function's frame: where it runs,
/// its stack pointer and its frame pointer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Registers {
    /// The instruction pointer (rip).
    pub pc: u64,
    /// The stack pointer (rsp).
    pub sp: u64,
    /// The frame pointer (rbp).
    pub fp: u64,
}

/// The calling function's [`Registers`], for a test of an unwinder that
/// starts from a real frame, as a crash report starts from a fault's. It
/// is always inlined, so they are of the caller's own frame.
#[cfg(target_arch = "x86_64")]
#[inline(always)]
#[must_use]
pub fn registers() -> Registers {
    let (pc, sp, fp): (u64, u64, u64);
    // SAFETY: reads three registers into three outputs, and writes no
    // memory and no other register.
    unsafe {
        std::arch::asm!(
            "lea {pc}, [rip]",
            "mov {sp}, rsp",
            "mov {fp}, rbp",
            pc = out(reg) pc,
            sp = out(reg) sp,
            fp = out(reg) fp,
            options(nomem, nostack, preserves_flags),
        );
    }
    Registers { pc, sp, fp }
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
    use super::{
        UsageOf, Who, euid, fork, kill, limit, priority, set_env, set_limit, set_priority,
        set_thread_name, thread_count, thread_name, try_wait, uid, usage, wait_with,
    };
    use std::ffi::OsStr;
    use std::os::unix::process::ExitStatusExt;
    use std::process::Command;

    #[test]
    fn time_on_a_cpu_counts_in_the_usage() {
        let before = usage(UsageOf::Process).unwrap();
        let spent = |now: super::Usage| (now.user + now.system) - (before.user + before.system);
        let started = std::time::Instant::now();
        while spent(usage(UsageOf::Process).unwrap()) < std::time::Duration::from_millis(10) {
            assert!(started.elapsed().as_secs() < 30, "no time on a CPU was counted");
            std::hint::black_box((0..1000u64).sum::<u64>());
        }
        assert!(usage(UsageOf::Children).is_ok());
    }

    #[test]
    fn a_thread_has_the_name_it_was_given() {
        let named = std::thread::Builder::new().name("sysapi-named".into()).spawn(thread_name);
        assert_eq!(named.unwrap().join().unwrap().unwrap(), b"sysapi-named");
    }

    #[test]
    fn a_thread_names_itself_with_at_most_15_bytes() {
        let named = std::thread::spawn(|| {
            set_thread_name("sysapi-a-long-name").unwrap();
            thread_name().unwrap()
        });
        assert_eq!(named.join().unwrap(), b"sysapi-a-long-n");
        assert!(set_thread_name("a\0b").is_err());
    }

    #[test]
    #[expect(clippy::zombie_processes, reason = "try_wait reaps it by its number")]
    fn a_child_is_waited_for_by_number_without_blocking() {
        let child = Command::new("sleep").arg("10").spawn().unwrap();
        let pid = child.id() as i32;
        assert_eq!(try_wait(pid).unwrap(), None, "it still runs");
        set_priority(Who::Process(child.id()), 10).unwrap();
        assert_eq!(priority(Who::Process(child.id())).unwrap(), 10);
        kill(pid, libc::SIGKILL).unwrap();
        let status = loop {
            if let Some(status) = try_wait(pid).unwrap() {
                break status;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        };
        assert_eq!(status.signal(), Some(libc::SIGKILL));
        assert_eq!(try_wait(pid).unwrap_err().raw_os_error(), Some(libc::ECHILD));
    }

    #[test]
    #[expect(clippy::zombie_processes, reason = "wait_with reaps it by its number")]
    fn a_child_s_stop_continuing_and_end_are_each_reported() {
        let child = Command::new("sleep").arg("10").spawn().unwrap();
        let pid = child.id() as i32;
        assert_eq!(wait_with(pid, libc::WNOHANG).unwrap(), None, "it still runs");
        kill(pid, libc::SIGSTOP).unwrap();
        let (got, status) = wait_with(pid, libc::WUNTRACED).unwrap().unwrap();
        assert_eq!((got, status.stopped_signal()), (pid, Some(libc::SIGSTOP)));
        kill(pid, libc::SIGCONT).unwrap();
        let (_, status) = wait_with(pid, libc::WCONTINUED).unwrap().unwrap();
        assert!(status.continued());
        kill(pid, libc::SIGKILL).unwrap();
        let (_, status) = wait_with(pid, 0).unwrap().unwrap();
        assert_eq!(status.signal(), Some(libc::SIGKILL));
        assert_eq!(wait_with(pid, 0).unwrap_err().raw_os_error(), Some(libc::ECHILD));
        // A flag waitpid(2) does not take.
        assert_eq!(wait_with(pid, 1 << 30).unwrap_err().raw_os_error(), Some(libc::EINVAL));
    }

    #[test]
    fn a_limit_reads_back_as_it_was_set() {
        let (soft, hard) = limit(libc::RLIMIT_NOFILE as libc::c_int).unwrap();
        assert!(soft <= hard);
        set_limit(libc::RLIMIT_NOFILE as libc::c_int, soft, hard).unwrap();
        assert_eq!(limit(libc::RLIMIT_NOFILE as libc::c_int).unwrap(), (soft, hard));
        let error = set_limit(libc::RLIMIT_NOFILE as libc::c_int, hard, soft.min(hard));
        assert!(soft == hard || error.is_err(), "a soft limit above the hard one");
    }

    #[cfg(target_arch = "x86_64")]
    #[test]
    fn the_registers_are_of_the_caller_s_frame() {
        let local = 0u8;
        let at = std::ptr::from_ref(&local) as u64;
        let here = super::registers();
        // The local is in this frame, above its stack pointer and near it.
        assert!(here.sp <= at && at - here.sp < 4096, "{here:x?} {at:#x}");
        let this = the_registers_are_of_the_caller_s_frame as fn() as usize as u64;
        assert!(here.pc > this && here.pc - this < 4096, "{here:x?} {this:#x}");
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
