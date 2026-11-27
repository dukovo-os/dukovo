// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Descriptors kept by their numbers, as a shell keeps them. A
//! redirection names a number (`3>file`, `2>&1`, `<&-`), and the shell's
//! own commands and the programs it starts find their files at those
//! numbers, so a shell cannot hold its descriptors as `OwnedFd`s alone.
//!
//! The table here owns each number it holds, and its calls take numbers
//! of three kinds only: the table's own; one the program's parent left
//! open across exec, which no code of this program holds (it has
//! close-on-exec clear, as [`fd::handed_down`](crate::fd::handed_down)
//! tells them apart), and which the table takes the first time a call
//! names it; and the standard numbers 0, 1 and 2. A number other code of
//! the program holds, as a `File` or a socket, is refused (EBUSY to
//! replace, EBADF to use), so the table never closes or replaces what it
//! does not own. That rests on what the rest of this crate rests on:
//! every descriptor this program's code opens has close-on-exec, as the
//! standard library and this crate open them.
//!
//! Each number the table opens or copies is closed on exec, except where
//! [`duplicate_onto`] puts one, and where [`duplicate_above`] puts one
//! when asked: those a program the shell starts gets.
//!
//! 0, 1 and 2 stay open for as long as the program runs, as the standard
//! library expects of them ([`fd::Standard`](crate::fd::Standard)).
//! [`close`] of one leaves it open on the null device and marks it
//! closed: the calls here treat it as closed until [`duplicate_onto`]
//! gives it a file again, and [`process::exec`](crate::process::exec)
//! closes it for the program it starts, just before that program begins.
//! Meanwhile the standard library's handles write to the null device.
//!
//! [`reserve`] keeps the numbers a redirection names (3 to 9) from the
//! rest of the program, as a shell keeps its own descriptors out of their
//! way: a number there that nothing is open at holds a placeholder on the
//! null device, closed on exec, which the calls here treat as closed. So
//! the files the standard library and the shell's libraries open land
//! elsewhere, and `exec 3>file` finds 3 free.
//!
//! A number is lent with [`lend`], for the calls that take `AsFd`; while
//! the loan lives, the table neither closes nor replaces the number.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::io;
use std::ops::RangeInclusive;
use std::os::fd::{AsFd, AsRawFd, BorrowedFd, FromRawFd, OwnedFd, RawFd};
use std::path::Path;
use std::sync::{Mutex, MutexGuard, PoisonError};

use libc::c_int;

use crate::checked;

/// The lowest number the table's own null device sits at, out of the way
/// of the numbers 0 to 9 that a redirection names.
const OWN_AT_LEAST: RawFd = 10;

/// A number the table holds.
struct Entry {
    fd: OwnedFd,
    /// How many loans of it live.
    lent: usize,
    /// Whether it only keeps the number from the rest of the program: open
    /// on the null device, closed on exec, and closed as far as the calls
    /// here go.
    placeholder: bool,
}

struct Table {
    held: BTreeMap<RawFd, Entry>,
    /// The numbers [`reserve`] keeps.
    reserved: BTreeSet<RawFd>,
    /// Which of 0, 1 and 2 [`close`] closed.
    closed: [bool; 3],
    /// The null device, for a closed standard number and a placeholder.
    null: Option<OwnedFd>,
}

static TABLE: Mutex<Table> = Mutex::new(Table {
    held: BTreeMap::new(),
    reserved: BTreeSet::new(),
    closed: [false; 3],
    null: None,
});

/// The table. No call holds it while it runs code of the caller's, so a
/// poisoned lock only means a panic of this module's own, after which the
/// table is still whole.
fn table() -> MutexGuard<'static, Table> {
    TABLE.lock().unwrap_or_else(PoisonError::into_inner)
}

fn error(errno: c_int) -> io::Error {
    io::Error::from_raw_os_error(errno)
}

/// What a number is, as the table sees it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    /// 0, 1 or 2, open.
    Standard,
    /// 0, 1 or 2, which [`close`] closed.
    ClosedStandard,
    /// One of the table's, open on a file.
    Held,
    /// One of the table's placeholders.
    Placeholder,
    /// The table's null device.
    Null,
    /// Left open by the program's parent, and held by nothing here.
    HandedDown,
    /// Not open.
    Free,
    /// Held by other code of the program.
    Other,
}

/// The index of a standard number among 0, 1 and 2.
fn standard(n: RawFd) -> Option<usize> {
    usize::try_from(n).ok().filter(|&index| index < 3)
}

/// dup2(2): makes `to` a descriptor of what `from` is open on, with
/// close-on-exec clear, closing what `to` was open on in the same step.
///
/// # Safety
///
/// `from` is open, and `to` is a number the caller owns or that nothing
/// holds: one of the table's, a standard number, or one free while this
/// process has one thread.
unsafe fn dup2(from: RawFd, to: RawFd) -> io::Result<()> {
    // SAFETY: dup2 acts on two numbers only, which the caller vouches for.
    checked(unsafe { libc::dup2(from, to) })?;
    Ok(())
}

/// Sets or clears close-on-exec on `n`, which the caller holds.
fn set_close_on_exec_of(n: RawFd, on: bool) -> io::Result<()> {
    // SAFETY: F_GETFD reads the descriptor flags of `n`, which the caller
    // holds, and takes no further argument.
    let flags = checked(unsafe { libc::fcntl(n, libc::F_GETFD) })?;
    let wanted = if on { flags | libc::FD_CLOEXEC } else { flags & !libc::FD_CLOEXEC };
    // SAFETY: F_SETFD sets the same descriptor's flags from an integer.
    checked(unsafe { libc::fcntl(n, libc::F_SETFD, wanted) })?;
    Ok(())
}

/// A new descriptor of what `from` is open on, at the lowest free number
/// from `least`, as F_DUPFD makes it, closed on exec if asked.
fn copy_above(from: RawFd, least: RawFd, close_on_exec: bool) -> io::Result<OwnedFd> {
    let command = if close_on_exec { libc::F_DUPFD_CLOEXEC } else { libc::F_DUPFD };
    // SAFETY: F_DUPFD and F_DUPFD_CLOEXEC take an integer, and make a new
    // descriptor at a number that was free; they close nothing.
    let copy = checked(unsafe { libc::fcntl(from, command, least) })?;
    // SAFETY: the descriptor was just made, and nothing else holds it.
    Ok(unsafe { OwnedFd::from_raw_fd(copy) })
}

impl Table {
    fn kind(&self, n: RawFd) -> io::Result<Kind> {
        if n < 0 {
            return Err(error(libc::EBADF));
        }
        if let Some(index) = standard(n) {
            return Ok(if self.closed[index] { Kind::ClosedStandard } else { Kind::Standard });
        }
        if let Some(entry) = self.held.get(&n) {
            return Ok(if entry.placeholder { Kind::Placeholder } else { Kind::Held });
        }
        if self.null.as_ref().is_some_and(|null| null.as_raw_fd() == n) {
            return Ok(Kind::Null);
        }
        // SAFETY: F_GETFD reads the descriptor flags of the number `n` and
        // changes nothing; it fails with EBADF where nothing is open.
        match unsafe { libc::fcntl(n, libc::F_GETFD) } {
            -1 => match io::Error::last_os_error() {
                error if error.raw_os_error() == Some(libc::EBADF) => Ok(Kind::Free),
                error => Err(error),
            },
            flags if flags & libc::FD_CLOEXEC != 0 => Ok(Kind::Other),
            _ => Ok(Kind::HandedDown),
        }
    }

    /// Takes `n`, which [`Table::kind`] found handed down.
    fn claim(&mut self, n: RawFd) {
        // SAFETY: `n` is open with close-on-exec clear and is not the
        // table's, so the program's parent left it open across exec and no
        // code of this program holds it: every descriptor this program's
        // code opens has close-on-exec, and the table's own are in the
        // table. The lock keeps `fd::handed_down` from taking it too.
        let fd = unsafe { OwnedFd::from_raw_fd(n) };
        self.held.insert(n, Entry { fd, lent: 0, placeholder: false });
    }

    /// `n`, open, for a call to use: a standard number, one of the table's
    /// that is not a placeholder, or one handed down, which is taken.
    fn usable(&mut self, n: RawFd) -> io::Result<Kind> {
        match self.kind(n)? {
            Kind::HandedDown => {
                self.claim(n);
                Ok(Kind::Held)
            }
            kind @ (Kind::Standard | Kind::Held) => Ok(kind),
            _ => Err(error(libc::EBADF)),
        }
    }

    /// The null device's number, opened the first time it is needed.
    fn null(&mut self) -> io::Result<RawFd> {
        if let Some(null) = &self.null {
            return Ok(null.as_raw_fd());
        }
        let opened =
            OwnedFd::from(File::options().read(true).write(true).open(crate::fd::NULL_DEVICE)?);
        let null = match copy_above(opened.as_raw_fd(), OWN_AT_LEAST, true) {
            Ok(high) => high,
            // A limit of fewer than 11 descriptors: it stays where it is.
            Err(_) => opened,
        };
        let n = null.as_raw_fd();
        self.null = Some(null);
        Ok(n)
    }

    /// Moves the null device from its number, which becomes a placeholder
    /// of the table's, to replace as one.
    fn move_null(&mut self) -> io::Result<()> {
        let Some(old) = self.null.take() else { return Ok(()) };
        match copy_above(old.as_raw_fd(), 3, true) {
            Ok(new) => {
                self.null = Some(new);
                self.held.insert(old.as_raw_fd(), Entry { fd: old, lent: 0, placeholder: true });
                Ok(())
            }
            Err(error) => {
                self.null = Some(old);
                Err(error)
            }
        }
    }

    /// Makes `n`, one of the table's, a placeholder on the null device.
    fn hold_place(&mut self, n: RawFd) -> io::Result<()> {
        let null = self.null()?;
        // SAFETY: `n` is the table's, and the null device is open.
        unsafe { dup2(null, n) }?;
        set_close_on_exec_of(n, true)?;
        if let Some(entry) = self.held.get_mut(&n) {
            entry.placeholder = true;
        }
        Ok(())
    }

    /// Makes `to` a descriptor of what `from` is open on, `from` being
    /// usable and `to` any number but `from`.
    fn put(&mut self, from: RawFd, to: RawFd) -> io::Result<()> {
        match self.kind(to)? {
            Kind::Standard | Kind::ClosedStandard => {
                // SAFETY: no `OwnedFd` holds 0, 1 or 2, which stay open.
                unsafe { dup2(from, to) }?;
                if let Some(index) = standard(to) {
                    self.closed[index] = false;
                }
            }
            Kind::Held | Kind::Placeholder => {
                let entry = self.held.get_mut(&to).ok_or_else(|| error(libc::EBADF))?;
                if entry.lent > 0 {
                    return Err(error(libc::EBUSY));
                }
                // SAFETY: `to` is the table's, and not lent. Its `OwnedFd`
                // still owns the number, now open on `from`'s file.
                unsafe { dup2(from, to) }?;
                entry.placeholder = false;
            }
            Kind::HandedDown => {
                self.claim(to);
                return self.put(from, to);
            }
            Kind::Null => {
                self.move_null()?;
                return self.put(from, to);
            }
            Kind::Free => {
                crate::process::single_threaded("put a descriptor at a free number")?;
                // SAFETY: nothing is open at `to`, and with one thread, the
                // caller's, nothing opens a file there first.
                unsafe { dup2(from, to) }?;
                // SAFETY: dup2 just made the descriptor, and nothing else
                // holds it.
                let fd = unsafe { OwnedFd::from_raw_fd(to) };
                self.held.insert(to, Entry { fd, lent: 0, placeholder: false });
            }
            Kind::Other => return Err(error(libc::EBUSY)),
        }
        Ok(())
    }

    /// Gives the table `fd`.
    fn keep(&mut self, fd: OwnedFd) -> RawFd {
        let n = fd.as_raw_fd();
        // No other `OwnedFd` can hold a number the table holds.
        debug_assert!(!self.held.contains_key(&n), "{n} is the table's already");
        self.held.insert(n, Entry { fd, lent: 0, placeholder: false });
        n
    }
}

/// Keeps the numbers in `numbers`, from 3 up, out of the rest of the
/// program's way: each that nothing is open at gets a placeholder, and
/// each that [`close`] closes later gets one back. Call it as the shell
/// starts, before its libraries open anything.
///
/// # Errors
///
/// EINVAL for a range that takes in 0, 1 or 2; `ErrorKind::Unsupported`
/// when the process has more than one thread, which could open a file at
/// a free number first; what opening the null device fails with.
pub fn reserve(numbers: RangeInclusive<RawFd>) -> io::Result<()> {
    if *numbers.start() < 3 {
        return Err(error(libc::EINVAL));
    }
    let mut table = table();
    crate::process::single_threaded("keep descriptor numbers")?;
    let null = table.null()?;
    for n in numbers.clone() {
        if table.kind(n)? == Kind::Free {
            // SAFETY: nothing is open at `n`, and with one thread nothing
            // opens a file there first; the null device is open.
            unsafe { dup2(null, n) }?;
            // SAFETY: dup2 just made the descriptor, and nothing else holds
            // it.
            let fd = unsafe { OwnedFd::from_raw_fd(n) };
            table.held.insert(n, Entry { fd, lent: 0, placeholder: true });
            set_close_on_exec_of(n, true)?;
        }
    }
    table.reserved.extend(numbers);
    Ok(())
}

/// Opens `path` as open(2) does with `flags` and, for a file it makes,
/// `mode`, adding O_CLOEXEC; its number, which the table holds.
///
/// # Errors
///
/// What open(2) fails with; `ErrorKind::InvalidInput` for a path with a
/// NUL byte.
pub fn open(path: &Path, flags: c_int, mode: libc::mode_t) -> io::Result<RawFd> {
    let path = crate::c_path(path)?;
    // SAFETY: a C string, flags and a mode; open makes a new descriptor at
    // a number that was free.
    let n = checked(unsafe {
        libc::open(path.as_ptr(), flags | libc::O_CLOEXEC, libc::c_uint::from(mode))
    })?;
    // SAFETY: open just made the descriptor, and nothing else holds it.
    let fd = unsafe { OwnedFd::from_raw_fd(n) };
    Ok(table().keep(fd))
}

/// A pipe, both ends closed on exec: the numbers of its read end and its
/// write end, which the table holds.
///
/// # Errors
///
/// EMFILE or ENFILE when no descriptor is free.
pub fn pipe() -> io::Result<(RawFd, RawFd)> {
    let (read, write) = crate::fd::pipe()?;
    let mut table = table();
    Ok((table.keep(read), table.keep(write)))
}

/// Gives the table `fd`, which it then holds by its number.
pub fn keep(fd: OwnedFd) -> RawFd {
    table().keep(fd)
}

/// Makes `to` a descriptor of what `from` is open on, as dup2(2) does: a
/// redirection, which a program the shell starts gets, so its
/// close-on-exec is clear. What `to` was open on is closed in the same
/// step. With `from` and `to` the same number, it only clears
/// close-on-exec.
///
/// # Errors
///
/// EBADF when `from` is not open or another part of the program holds
/// it; EBUSY when another part of the program holds `to`, or it is lent;
/// `ErrorKind::Unsupported` for a `to` that nothing is open at while the
/// process has more than one thread, which could open a file there first.
pub fn duplicate_onto(from: RawFd, to: RawFd) -> io::Result<()> {
    let mut table = table();
    let kind = table.usable(from)?;
    if from == to {
        return match kind {
            Kind::Held => set_close_on_exec_of(to, false),
            _ => Ok(()),
        };
    }
    table.put(from, to)
}

/// A new descriptor of what `from` is open on, at the lowest free number
/// from `least`, as F_DUPFD makes it; closed on exec if asked. The table
/// holds it.
///
/// # Errors
///
/// EBADF when `from` is not open or another part of the program holds
/// it; EINVAL for a `least` beyond the process's limit; EMFILE when no
/// number from `least` is free.
pub fn duplicate_above(from: RawFd, least: RawFd, close_on_exec: bool) -> io::Result<RawFd> {
    let mut table = table();
    table.usable(from)?;
    let copy = copy_above(from, least, close_on_exec)?;
    Ok(table.keep(copy))
}

/// Closes `n`: one of the table's, or one handed down, as close(2) does;
/// one the table keeps for the shell gets a placeholder back. A standard
/// number is left open on the null device, and marked closed.
///
/// # Errors
///
/// EBADF when `n` is not open, or another part of the program holds it;
/// EBUSY when it is lent.
pub fn close(n: RawFd) -> io::Result<()> {
    let mut table = table();
    match table.usable(n)? {
        Kind::Standard => {
            let null = table.null()?;
            // SAFETY: no `OwnedFd` holds 0, 1 or 2, which stay open.
            unsafe { dup2(null, n) }?;
            if let Some(index) = standard(n) {
                table.closed[index] = true;
            }
        }
        _ => {
            let entry = table.held.get(&n).ok_or_else(|| error(libc::EBADF))?;
            if entry.lent > 0 {
                return Err(error(libc::EBUSY));
            }
            if table.reserved.contains(&n) {
                table.hold_place(n)?;
            } else {
                table.held.remove(&n);
            }
        }
    }
    Ok(())
}

/// Whether `n` is open, as far as the calls here go: a standard number
/// not closed, one of the table's that is not a placeholder, or one
/// handed down.
#[must_use]
pub fn is_open(n: RawFd) -> bool {
    let table = table();
    matches!(table.kind(n), Ok(Kind::Standard | Kind::Held | Kind::HandedDown))
}

/// Sets or clears close-on-exec on `n`, one of the table's or one handed
/// down.
///
/// # Errors
///
/// EBADF when `n` is not open, is a standard number, or another part of
/// the program holds it.
pub fn set_close_on_exec(n: RawFd, on: bool) -> io::Result<()> {
    let mut table = table();
    match table.usable(n)? {
        Kind::Held => set_close_on_exec_of(n, on),
        _ => Err(error(libc::EBADF)),
    }
}

/// Number `n`, lent for the calls that take `AsFd`.
///
/// # Errors
///
/// EBADF when `n` is not open, or another part of the program holds it.
pub fn lend(n: RawFd) -> io::Result<Lent> {
    let mut table = table();
    let held = table.usable(n)? == Kind::Held;
    if held && let Some(entry) = table.held.get_mut(&n) {
        entry.lent += 1;
    }
    Ok(Lent { number: n, held })
}

/// Takes `n` out of the table, for code that holds it as an `OwnedFd`
/// from then on; a number the table keeps for the shell is then free for
/// the rest of the program once that `OwnedFd` closes it.
///
/// # Errors
///
/// EBADF when `n` is not open, is a standard number, or another part of
/// the program holds it; EBUSY when it is lent.
pub fn take(n: RawFd) -> io::Result<OwnedFd> {
    let mut table = table();
    if table.usable(n)? != Kind::Held {
        return Err(error(libc::EBADF));
    }
    if table.held.get(&n).is_some_and(|entry| entry.lent > 0) {
        return Err(error(libc::EBUSY));
    }
    table.held.remove(&n).map(|entry| entry.fd).ok_or_else(|| error(libc::EBADF))
}

/// A number [`lend`] lent: while it lives, the table neither closes nor
/// replaces the number.
#[derive(Debug)]
pub struct Lent {
    number: RawFd,
    /// Whether it is one of the table's, whose loans the table counts.
    held: bool,
}

impl Lent {
    /// Its number.
    #[must_use]
    pub fn number(&self) -> RawFd {
        self.number
    }
}

impl AsFd for Lent {
    fn as_fd(&self) -> BorrowedFd<'_> {
        // SAFETY: the number is open for as long as this lives: a standard
        // number always is, and the table neither closes nor replaces one
        // of its own while it is lent.
        unsafe { BorrowedFd::borrow_raw(self.number) }
    }
}

impl Drop for Lent {
    fn drop(&mut self) {
        if self.held
            && let Some(entry) = table().held.get_mut(&self.number)
        {
            entry.lent -= 1;
        }
    }
}

/// Whether [`close`] closed standard number `n`.
pub(crate) fn standard_is_closed(n: RawFd) -> bool {
    standard(n).is_some_and(|index| table().closed[index])
}

/// Makes standard number `n` a descriptor of what `from` is open on, for
/// `fd::redirect`: no longer closed.
pub(crate) fn put_standard(from: BorrowedFd<'_>, n: RawFd) -> io::Result<()> {
    let index = standard(n).ok_or_else(|| error(libc::EINVAL))?;
    let mut table = table();
    // SAFETY: no `OwnedFd` holds 0, 1 or 2, which stay open; `from` is
    // borrowed, so open.
    unsafe { dup2(from.as_raw_fd(), n) }?;
    table.closed[index] = false;
    Ok(())
}

/// Whether the table holds `n`, for `fd::handed_down`, which runs `claim`
/// under the same lock so that the two never both take a number.
pub(crate) fn with_held<R>(n: RawFd, claim: impl FnOnce(bool) -> R) -> R {
    let table = table();
    claim(
        table.held.contains_key(&n)
            || table.null.as_ref().is_some_and(|null| null.as_raw_fd() == n),
    )
}

/// Runs `exec`, an execve(2), with each standard number [`close`] closed
/// closed for real, as the program it starts must find it; when `exec`
/// returns, it failed, and they are open on the null device again.
///
/// # Errors
///
/// `ErrorKind::Unsupported` when a standard number is to be closed while
/// the process has more than one thread, which could open a file there.
pub(crate) fn with_closed_standard_closed<R>(exec: impl FnOnce() -> R) -> io::Result<R> {
    let table = table();
    let closed: Vec<RawFd> =
        (0..3).filter(|&n| standard(n).is_some_and(|i| table.closed[i])).collect();
    let Some(null) = table.null.as_ref().map(AsRawFd::as_raw_fd).filter(|_| !closed.is_empty())
    else {
        return Ok(exec());
    };
    crate::process::single_threaded("start a program with a standard descriptor closed")?;
    for &n in &closed {
        // SAFETY: no `OwnedFd` holds 0, 1 or 2; with one thread and the
        // table locked, nothing opens a file at the number before `exec`
        // runs, and it is open again before any other code of this
        // program runs.
        unsafe { libc::close(n) };
    }
    let result = exec();
    for &n in &closed {
        // SAFETY: as above; the null device is open.
        unsafe { libc::dup2(null, n) };
    }
    Ok(result)
}
