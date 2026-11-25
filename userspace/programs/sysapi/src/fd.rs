// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Descriptors: pipes, the status flags the standard library has no call
//! for, reads and writes on a descriptor held as an `OwnedFd`, the
//! descriptors a parent hands down, and the standard descriptors 0 to 2.

use std::fs::File;
use std::io;
use std::os::fd::{AsFd, AsRawFd, BorrowedFd, FromRawFd, OwnedFd, RawFd};

use crate::checked;

/// The device that reads as empty and takes every write.
#[cfg(target_os = "radix")]
const NULL_DEVICE: &str = "/devices/null";
#[cfg(not(target_os = "radix"))]
const NULL_DEVICE: &str = "/dev/null";

/// A pipe: its read end, then its write end, each closed on exec.
///
/// # Errors
///
/// EMFILE or ENFILE when no descriptor is free.
pub fn pipe() -> io::Result<(OwnedFd, OwnedFd)> {
    let mut ends = [-1; 2];
    // SAFETY: pipe2 writes two descriptors into the array of two it is
    // given.
    checked(unsafe { libc::pipe2(ends.as_mut_ptr(), libc::O_CLOEXEC) })?;
    // SAFETY: pipe2 made both descriptors, and nothing else holds them.
    Ok(unsafe { (OwnedFd::from_raw_fd(ends[0]), OwnedFd::from_raw_fd(ends[1])) })
}

/// Makes reads and writes on `fd` return at once with EAGAIN rather than
/// wait, or wait again. The flag belongs to the open file, so every
/// descriptor of it, in this process or another, sees the change.
///
/// # Errors
///
/// EBADF when `fd` is not open.
pub fn set_nonblocking(fd: impl AsFd, nonblocking: bool) -> io::Result<()> {
    let fd = fd.as_fd().as_raw_fd();
    // SAFETY: F_GETFL reads the status flags of a descriptor the caller
    // holds, and takes no further argument.
    let flags = checked(unsafe { libc::fcntl(fd, libc::F_GETFL) })?;
    let wanted = if nonblocking { flags | libc::O_NONBLOCK } else { flags & !libc::O_NONBLOCK };
    if wanted != flags {
        // SAFETY: F_SETFL sets the status flags of the same descriptor from
        // an integer.
        checked(unsafe { libc::fcntl(fd, libc::F_SETFL, wanted) })?;
    }
    Ok(())
}

/// Whether `fd` is closed on exec, as F_GETFD reads it: the cheapest
/// look at a descriptor there is.
///
/// # Errors
///
/// EBADF when `fd` is not open.
pub fn close_on_exec(fd: impl AsFd) -> io::Result<bool> {
    // SAFETY: F_GETFD reads the descriptor flags of a descriptor the caller
    // holds, and takes no further argument.
    let flags = checked(unsafe { libc::fcntl(fd.as_fd().as_raw_fd(), libc::F_GETFD) })?;
    Ok(flags & libc::FD_CLOEXEC != 0)
}

/// Has `fd` closed on exec, or kept open in a program this one starts,
/// as F_SETFD sets it: a shell keeps the descriptors a redirection names.
///
/// # Errors
///
/// EBADF when `fd` is not open.
pub fn set_close_on_exec(fd: impl AsFd, on: bool) -> io::Result<()> {
    let fd = fd.as_fd().as_raw_fd();
    // SAFETY: F_GETFD reads the descriptor flags of a descriptor the caller
    // holds.
    let flags = checked(unsafe { libc::fcntl(fd, libc::F_GETFD) })?;
    let wanted = if on { flags | libc::FD_CLOEXEC } else { flags & !libc::FD_CLOEXEC };
    // SAFETY: F_SETFD sets the same descriptor's flags from an integer.
    checked(unsafe { libc::fcntl(fd, libc::F_SETFD, wanted) })?;
    Ok(())
}

/// Reads from `fd` into `buffer`, as read(2) does: what is there, up to
/// the buffer's length; 0 at the end.
///
/// # Errors
///
/// EAGAIN on a descriptor that does not wait with nothing to read, EINTR
/// when a signal came first, EIO on a terminal whose other side closed.
pub fn read(fd: impl AsFd, buffer: &mut [u8]) -> io::Result<usize> {
    // SAFETY: read writes at most `buffer.len()` bytes into the buffer.
    let n = unsafe { libc::read(fd.as_fd().as_raw_fd(), buffer.as_mut_ptr().cast(), buffer.len()) };
    usize::try_from(n).map_err(|_| io::Error::last_os_error())
}

/// Writes from `bytes` to `fd`, as write(2) does; returns how many were
/// taken, which may be fewer.
///
/// # Errors
///
/// EAGAIN on a descriptor that does not wait and has no room, EINTR when a
/// signal came first, EPIPE when nothing reads the other end.
pub fn write(fd: impl AsFd, bytes: &[u8]) -> io::Result<usize> {
    // SAFETY: write reads at most `bytes.len()` bytes from the slice.
    let n = unsafe { libc::write(fd.as_fd().as_raw_fd(), bytes.as_ptr().cast(), bytes.len()) };
    usize::try_from(n).map_err(|_| io::Error::last_os_error())
}

/// Takes descriptor `fd`, which this program's parent left open across
/// `exec` for it, as authd hands a worker its pipes as 3 and 4.
///
/// Such a descriptor has close-on-exec clear, which no descriptor this
/// process's Rust code opened has: the standard library and this crate
/// open every one with it. The descriptor is given close-on-exec here, so
/// that it does not reach this program's own children and a second claim
/// of the same number fails.
///
/// # Errors
///
/// EBADF when nothing is open at `fd`, and EINVAL when `fd` has
/// close-on-exec (it is not a handed-down descriptor, or it was claimed
/// already) or is 0, 1 or 2, which [`take_standard`] takes.
pub fn handed_down(fd: RawFd) -> io::Result<OwnedFd> {
    if fd <= 2 {
        return Err(io::Error::from_raw_os_error(libc::EINVAL));
    }
    // SAFETY: F_GETFD reads the descriptor flags of the number `fd`, and
    // takes no further argument; it fails with EBADF for a number with no
    // descriptor, and changes nothing.
    let flags = checked(unsafe { libc::fcntl(fd, libc::F_GETFD) })?;
    if flags & libc::FD_CLOEXEC != 0 {
        return Err(io::Error::from_raw_os_error(libc::EINVAL));
    }
    // SAFETY: as above, F_SETFD with an integer; it sets the flag that
    // marks the descriptor as taken.
    checked(unsafe { libc::fcntl(fd, libc::F_SETFD, flags | libc::FD_CLOEXEC) })?;
    // SAFETY: the descriptor is open and was not opened by this process's
    // Rust code, which opens every descriptor with close-on-exec, so no
    // owner of it exists; the flag just set makes this the only claim.
    Ok(unsafe { OwnedFd::from_raw_fd(fd) })
}

/// One of the three standard descriptors.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Standard {
    /// Descriptor 0, the standard input.
    Input,
    /// Descriptor 1, the standard output.
    Output,
    /// Descriptor 2, the standard error.
    Error,
}

impl Standard {
    fn number(self) -> RawFd {
        match self {
            Standard::Input => 0,
            Standard::Output => 1,
            Standard::Error => 2,
        }
    }

    /// The descriptor, borrowed for as long as the program runs: what the
    /// standard library's `Stdin`, `Stdout` and `Stderr` borrow for as
    /// long as each handle lives, which ends with a temporary handle's
    /// statement.
    #[must_use]
    pub fn fd(self) -> BorrowedFd<'static> {
        // SAFETY: 0, 1 and 2 are open for the whole of the program's life:
        // the standard library opens the null device at each one that is
        // closed before `main`, and no Rust code may close them, as no
        // `OwnedFd` holds them; `redirect` and `take_standard` leave each
        // open, on another file.
        unsafe { BorrowedFd::borrow_raw(self.number()) }
    }
}

/// Makes standard descriptor `which` another descriptor of what `to` is
/// open on, as dup2(2) does: what `which` was open on before is closed
/// unless something else holds it. The standard library's handles keep
/// using the number, and now reach `to`'s file.
///
/// # Errors
///
/// EBADF when `to` is not open.
pub fn redirect(which: Standard, to: impl AsFd) -> io::Result<()> {
    // SAFETY: dup2 onto 0, 1 or 2, which no `OwnedFd` holds: the standard
    // library's handles use the numbers without owning them, and expect
    // them to stay open, which they do, now on `to`'s file.
    checked(unsafe { libc::dup2(to.as_fd().as_raw_fd(), which.number()) })?;
    Ok(())
}

/// Takes what standard descriptor `which` is open on as a descriptor of
/// the program's own, as a worker takes the connection its parent gave it
/// as its input, and leaves `which` open on the null device. The
/// connection then has this one descriptor, so that closing it closes the
/// connection; the standard library's handle reads nothing more from it.
///
/// # Errors
///
/// EBADF when `which` is not open, and what opening the null device fails
/// with.
pub fn take_standard(which: Standard) -> io::Result<OwnedFd> {
    let null = File::options().read(true).write(true).open(NULL_DEVICE)?;
    // SAFETY: F_DUPFD_CLOEXEC makes a new descriptor of what `which` is open
    // on, at the lowest free number from 3; it takes an integer and closes
    // nothing.
    let taken = checked(unsafe { libc::fcntl(which.number(), libc::F_DUPFD_CLOEXEC, 3) })?;
    // SAFETY: the descriptor was just made, and nothing else holds it.
    let taken = unsafe { OwnedFd::from_raw_fd(taken) };
    redirect(which, &null)?;
    Ok(taken)
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};
    use std::os::fd::AsRawFd;

    use super::{
        Standard, close_on_exec, handed_down, pipe, read, set_close_on_exec, set_nonblocking, write,
    };

    #[test]
    fn close_on_exec_is_read_and_set() {
        let (read_end, _write_end) = pipe().unwrap();
        assert!(close_on_exec(&read_end).unwrap());
        set_close_on_exec(&read_end, false).unwrap();
        assert!(!close_on_exec(&read_end).unwrap());
        set_close_on_exec(&read_end, true).unwrap();
        assert!(close_on_exec(&read_end).unwrap());
    }

    #[test]
    fn the_standard_descriptors_are_0_1_and_2() {
        let numbers = [Standard::Input, Standard::Output, Standard::Error]
            .map(|which| which.fd().as_raw_fd());
        assert_eq!(numbers, [0, 1, 2]);
    }

    #[test]
    fn a_pipe_carries_bytes_from_its_write_end_to_its_read_end() {
        let (from, to) = pipe().unwrap();
        assert_eq!(write(&to, b"abc").unwrap(), 3);
        let mut buffer = [0; 8];
        assert_eq!(read(&from, &mut buffer).unwrap(), 3);
        assert_eq!(&buffer[..3], b"abc");
    }

    #[test]
    fn an_empty_pipe_that_does_not_wait_says_eagain() {
        let (from, _to) = pipe().unwrap();
        set_nonblocking(&from, true).unwrap();
        let error = read(&from, &mut [0; 1]).unwrap_err();
        assert_eq!(error.raw_os_error(), Some(libc::EAGAIN));
        set_nonblocking(&from, false).unwrap();
    }

    #[test]
    fn a_descriptor_this_process_opened_is_not_handed_down() {
        let (from, to) = pipe().unwrap();
        let error = handed_down(from.as_raw_fd()).unwrap_err();
        assert_eq!(error.raw_os_error(), Some(libc::EINVAL));
        // The refused claim changed nothing: the pipe still works.
        let mut to = std::fs::File::from(to);
        to.write_all(b"x").unwrap();
        let mut byte = [0; 1];
        std::fs::File::from(from).read_exact(&mut byte).unwrap();
        assert_eq!(&byte, b"x");
    }

    #[test]
    fn a_handed_down_descriptor_is_taken_once() {
        let (from, _to) = pipe().unwrap();
        // SAFETY: dup makes a descriptor of the pipe without close-on-exec,
        // as a parent leaves one for its child; nothing else holds it.
        let left = unsafe { libc::dup(from.as_raw_fd()) };
        assert!(left > 2);
        let taken = handed_down(left).unwrap();
        assert_eq!(taken.as_raw_fd(), left);
        assert_eq!(handed_down(left).unwrap_err().raw_os_error(), Some(libc::EINVAL));
    }

    #[test]
    fn the_standard_descriptors_are_not_handed_down() {
        for fd in 0..=2 {
            assert_eq!(handed_down(fd).unwrap_err().raw_os_error(), Some(libc::EINVAL));
        }
    }

    #[test]
    fn a_number_with_no_descriptor_is_not_handed_down() {
        let error = handed_down(1 << 20).unwrap_err();
        assert_eq!(error.raw_os_error(), Some(libc::EBADF));
    }
}
