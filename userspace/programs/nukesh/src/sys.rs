// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The system calls the shell makes, through sysapi, with errors as
//! [`std::io::Error`].
//!
//! The shell keeps its descriptors by number, as redirections name them,
//! in sysapi's table of descriptor numbers ([`sysapi::numbered`]), rather
//! than as `std::fs::File`s, whose buffers and drops would follow a forked
//! child. A number other code of the shell holds (a file the line editor
//! keeps open, say) is never closed or replaced from here: sysapi refuses
//! it.

use std::ffi::{CStr, CString};
use std::io::{Error, IsTerminal, Result, SeekFrom};
use std::os::fd::AsFd;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::process::ExitStatusExt;
use std::path::Path;
use std::time::Duration;

use sysapi::numbered;
use sysapi::poll::PollFd;
use sysapi::process::Fork;
use sysapi::terminal::When;

/// Descriptors the shell keeps for itself (saved descriptors, the script
/// it reads) are at or above this, out of the way of the 0 to 9 that
/// redirections name.
pub const HIGH_FD: i32 = 10;

/// Keeps 3 to 9 for redirections, so that the files the shell's libraries
/// open (the line editor's, say) land elsewhere; for the shell's start,
/// while it has one thread.
pub fn keep_redirection_numbers() {
    let _ = numbered::reserve(3..=HIGH_FD - 1);
}

/// The text of an error, without Rust's "(os error N)".
pub fn describe(error: &Error) -> String {
    let text = error.to_string();
    match error.raw_os_error() {
        Some(code) => match text.strip_suffix(&format!(" (os error {code})")) {
            Some(bare) => bare.to_string(),
            None => text,
        },
        None => text,
    }
}

/// Forks: the child's number in the parent, 0 in the child. sysapi
/// refuses while the shell has a second thread, whose locks the child
/// would find held for good.
pub fn fork() -> Result<i32> {
    Ok(match sysapi::process::fork()? {
        Fork::Child => 0,
        Fork::Parent(child) => child,
    })
}

/// Runs `path` in place of this process; it returns only on failure. The
/// program keeps the signals the shell ignores, its signal mask, and the
/// descriptors redirections put at their numbers.
pub fn execve(path: &CStr, argv: &[CString], envp: &[CString]) -> Error {
    sysapi::process::exec(path, argv, envp)
}

/// A pipe, both ends closed on exec: (read end, write end).
pub fn pipe() -> Result<(i32, i32)> {
    numbered::pipe()
}

/// Makes `to` a copy of `from`, kept across exec; with the two the same,
/// only keeps it across exec, as a redirection wants.
pub fn dup2(from: i32, to: i32) -> Result<()> {
    numbered::duplicate_onto(from, to)
}

/// A copy of `fd` at or above `min`, closed on exec.
pub fn dup_at_least(fd: i32, min: i32) -> Result<i32> {
    numbered::duplicate_above(fd, min, true)
}

/// A copy of `fd` at or above [`HIGH_FD`], closed on exec.
pub fn dup_high(fd: i32) -> Result<i32> {
    numbered::duplicate_above(fd, HIGH_FD, true)
}

/// A copy of `fd` at or above `min`, kept across exec: for `{name}>file`
/// and a process substitution, whose number the command is given.
pub fn dup_kept(fd: i32, min: i32) -> Result<i32> {
    numbered::duplicate_above(fd, min, false)
}

pub fn close(fd: i32) {
    let _ = numbered::close(fd);
}

pub fn is_open(fd: i32) -> bool {
    numbered::is_open(fd)
}

pub fn set_cloexec(fd: i32, on: bool) -> Result<()> {
    numbered::set_close_on_exec(fd, on)
}

pub fn open(path: &str, flags: i32, mode: u32) -> Result<i32> {
    numbered::open(Path::new(&crate::text::to_os(path)), flags, mode as libc::mode_t)
}

/// One read, as read(2) makes it: EINTR when a signal came first.
pub fn read_once(fd: i32, buf: &mut [u8]) -> Result<usize> {
    sysapi::fd::read(numbered::lend(fd)?, buf)
}

pub fn read(fd: i32, buf: &mut [u8]) -> Result<usize> {
    let lent = numbered::lend(fd)?;
    loop {
        match sysapi::fd::read(&lent, buf) {
            Ok(n) => return Ok(n),
            Err(error) if error.raw_os_error() != Some(libc::EINTR) || crate::trap::pending() => {
                return Err(error);
            }
            Err(_) => {}
        }
    }
}

/// Reads to the end of `fd`.
pub fn read_all(fd: i32) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    let mut buf = [0u8; 8192];
    loop {
        match read(fd, &mut buf) {
            Ok(0) => return Ok(out),
            Ok(n) => out.extend_from_slice(&buf[..n]),
            Err(error) if error.raw_os_error() == Some(libc::EINTR) => {}
            Err(error) => return Err(error),
        }
    }
}

pub fn write_all(fd: i32, mut bytes: &[u8]) -> Result<()> {
    let lent = numbered::lend(fd)?;
    while !bytes.is_empty() {
        match sysapi::fd::write(&lent, bytes) {
            Ok(n) => bytes = &bytes[n..],
            Err(error) if error.raw_os_error() == Some(libc::EINTR) => {}
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

/// Waits for a child: its number and status, or `None` with `WNOHANG`
/// when none has changed.
pub fn waitpid(pid: i32, flags: i32) -> Result<Option<(i32, i32)>> {
    Ok(sysapi::process::wait_with(pid, flags)?.map(|(pid, status)| (pid, status.into_raw())))
}

pub fn getpid() -> i32 {
    std::process::id().cast_signed()
}

pub fn getppid() -> i32 {
    std::os::unix::process::parent_id().cast_signed()
}

pub fn getuid() -> u32 {
    sysapi::process::uid()
}

pub fn geteuid() -> u32 {
    sysapi::process::euid()
}

pub fn getgid() -> u32 {
    sysapi::process::gid()
}

pub fn setpgid(pid: i32, pgid: i32) -> Result<()> {
    sysapi::process::set_process_group(pid, pgid)
}

pub fn getpgrp() -> i32 {
    sysapi::process::process_group()
}

pub fn tcgetpgrp(fd: i32) -> Result<i32> {
    sysapi::terminal::foreground_group(numbered::lend(fd)?)
}

/// Gives the terminal on `fd` to process group `pgid`. From a group in
/// the background the change raises SIGTTOU, which would stop a script's
/// shell taking the terminal back, or a job about to have it; sysapi
/// blocks the signal meanwhile, so the change goes ahead, as in bash.
pub fn tcsetpgrp(fd: i32, pgid: i32) -> Result<()> {
    sysapi::terminal::set_foreground_group(numbered::lend(fd)?, pgid)
}

pub fn isatty(fd: i32) -> bool {
    numbered::lend(fd).is_ok_and(|lent| lent.as_fd().is_terminal())
}

/// The path of the terminal on `fd`, as `/dev/pts/3`.
pub fn tty_path(fd: i32) -> Option<String> {
    let path = sysapi::terminal::path(numbered::lend(fd).ok()?).ok()?;
    Some(crate::text::decode(path.as_os_str().as_bytes()))
}

pub fn kill(pid: i32, signal: i32) -> Result<()> {
    sysapi::process::kill(pid, signal)
}

pub fn umask(mask: u32) -> u32 {
    sysapi::process::umask(mask as libc::mode_t) as u32
}

pub fn tcgetattr(fd: i32) -> Option<libc::termios> {
    sysapi::terminal::attributes(numbered::lend(fd).ok()?).ok()
}

pub fn tcsetattr(fd: i32, termios: &libc::termios) {
    if let Ok(lent) = numbered::lend(fd) {
        let _ = sysapi::terminal::set_attributes(lent, When::Drained, termios);
    }
}

/// The terminal's size in columns and rows.
pub fn window_size(fd: i32) -> Option<(u16, u16)> {
    let size = sysapi::terminal::window_size(numbered::lend(fd).ok()?).ok()?;
    (size.cols > 0).then_some((size.cols, size.rows))
}

pub fn hostname() -> String {
    sysapi::system::host_name().unwrap_or_else(|_| "localhost".into())
}

/// The home directory of user `name` (`None` for the current user).
pub fn home_of(name: Option<&str>) -> Option<String> {
    let user = match name {
        Some(name) => sysapi::users::user_by_name(name),
        None => sysapi::users::user_by_id(getuid()),
    };
    let home = user.ok()??.home;
    (!home.as_os_str().is_empty()).then(|| crate::text::decode(home.as_os_str().as_bytes()))
}

/// The login name of the current user.
pub fn user_name() -> Option<String> {
    let user = sysapi::users::user_by_id(getuid()).ok()??;
    (!user.name.is_empty()).then_some(user.name)
}

/// Every user's login name, for completion.
pub fn user_names() -> Vec<String> {
    sysapi::users::user_names()
}

pub type Rlimit = libc::rlimit;

pub fn getrlimit(resource: i32) -> Result<Rlimit> {
    let (soft, hard) = sysapi::process::limit(resource)?;
    Ok(libc::rlimit { rlim_cur: soft, rlim_max: hard })
}

pub fn setrlimit(resource: i32, limit: &Rlimit) -> Result<()> {
    sysapi::process::set_limit(resource, limit.rlim_cur, limit.rlim_max)
}

/// The CPU time used by the shell and by its waited-for children, user
/// and system, in seconds.
pub fn times() -> (f64, f64, f64, f64) {
    let usage = |of| {
        sysapi::process::usage(of)
            .map_or((0.0, 0.0), |usage| (usage.user.as_secs_f64(), usage.system.as_secs_f64()))
    };
    let (user, system) = usage(sysapi::process::UsageOf::Process);
    let (child_user, child_system) = usage(sysapi::process::UsageOf::Children);
    (user, system, child_user, child_system)
}

/// Whether `path` can be run by this user.
pub fn executable(path: &str) -> bool {
    access(path, libc::X_OK)
}

pub fn access(path: &str, mode: i32) -> bool {
    sysapi::fs::access(Path::new(&crate::text::to_os(path)), mode)
}

/// Random bytes from the kernel.
pub fn random_u32() -> u32 {
    match sysapi::random::bytes::<4>() {
        Ok(bytes) => u32::from_ne_bytes(bytes),
        Err(_) => {
            let (seconds, micros) = now();
            (seconds as u32 ^ micros) ^ getpid().cast_unsigned().rotate_left(16)
        }
    }
}

/// Exits at once, without running Rust's exit handlers or flushing
/// buffers the child copied from its parent.
pub fn exit_now(status: i32) -> ! {
    sysapi::process::exit_now(status)
}

/// The time of day, formatted with strftime's `format`.
pub fn strftime(format: &str, time: i64) -> String {
    sysapi::time::format(time, format, false).unwrap_or_default()
}

pub fn now() -> (i64, u32) {
    let since =
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
    (since.as_secs() as i64, since.subsec_micros())
}

/// Moves `fd`'s offset; the new offset.
pub fn lseek(fd: i32, offset: i64, whence: i32) -> Result<i64> {
    let to = match whence {
        libc::SEEK_SET => SeekFrom::Start(
            u64::try_from(offset).map_err(|_| Error::from_raw_os_error(libc::EINVAL))?,
        ),
        libc::SEEK_END => SeekFrom::End(offset),
        _ => SeekFrom::Current(offset),
    };
    let at = sysapi::fd::seek(numbered::lend(fd)?, to)?;
    i64::try_from(at).map_err(|_| Error::from_raw_os_error(libc::EOVERFLOW))
}

/// Whether `fd` is a regular file, whose offset can be moved back over
/// what was read but not used.
pub fn is_regular(fd: i32) -> bool {
    identity(fd).is_some_and(|(_, _, regular)| regular)
}

/// `fd`'s device, inode and whether it is a regular file.
pub fn identity(fd: i32) -> Option<(u64, u64, bool)> {
    let st = sysapi::dir::stat(numbered::lend(fd).ok()?).ok()?;
    Some((st.st_dev as u64, st.st_ino as u64, st.st_mode & libc::S_IFMT == libc::S_IFREG))
}

/// Waits up to `millis` (forever if negative) for `fd` to have input:
/// whether it has.
pub fn poll_in(fd: i32, millis: i32) -> Result<bool> {
    let lent = numbered::lend(fd)?;
    let wait = u64::try_from(millis).ok().map(Duration::from_millis);
    let ready = sysapi::poll::poll(&mut [PollFd::new(lent.as_fd(), libc::POLLIN)], wait)?;
    Ok(ready > 0)
}
