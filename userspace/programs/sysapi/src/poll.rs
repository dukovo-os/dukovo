// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Waiting for descriptors, as poll(2) does: for any of several to be
//! ready, or for one to be readable within a time.

use std::io;
use std::marker::PhantomData;
use std::os::fd::{AsFd, AsRawFd, BorrowedFd};
use std::time::Duration;

use crate::checked;

/// A descriptor to wait for, and what it is waited for: `libc::pollfd`,
/// holding a borrow of the descriptor so that it stays open while it is
/// waited for.
#[repr(transparent)]
pub struct PollFd<'fd> {
    raw: libc::pollfd,
    fd: PhantomData<BorrowedFd<'fd>>,
}

impl<'fd> PollFd<'fd> {
    /// Waits for `fd` to have `events` (`POLLIN`, `POLLOUT` or both).
    #[must_use]
    pub fn new(fd: BorrowedFd<'fd>, events: i16) -> Self {
        PollFd { raw: libc::pollfd { fd: fd.as_raw_fd(), events, revents: 0 }, fd: PhantomData }
    }

    /// Waits for `fd`, if there is one, to have `events`; with none, keeps
    /// its place in the list, which poll passes over, its
    /// [`revents`](PollFd::revents) left 0.
    #[must_use]
    pub fn optional(fd: Option<BorrowedFd<'fd>>, events: i16) -> Self {
        let raw = fd.map_or(-1, |fd| fd.as_raw_fd());
        PollFd { raw: libc::pollfd { fd: raw, events, revents: 0 }, fd: PhantomData }
    }

    /// What the descriptor had when [`poll`] returned: the events it was
    /// waited for that came, and `POLLHUP`, `POLLERR` or `POLLNVAL`,
    /// which come unasked.
    #[must_use]
    pub fn revents(&self) -> i16 {
        self.raw.revents
    }
}

/// Waits until any of `fds` is ready or `timeout` passes (none waits for
/// as long as it takes), and returns how many are ready, each with its
/// [`PollFd::revents`] set. The timeout is rounded up to a whole
/// millisecond, so that a wait for less than one waits rather than
/// returning at once.
///
/// # Errors
///
/// EINTR when a signal came first, which a caller that waits for one
/// checks for; EINVAL for more descriptors than the process may hold.
pub fn poll(fds: &mut [PollFd<'_>], timeout: Option<Duration>) -> io::Result<usize> {
    let timeout = timeout.map_or(-1, |timeout| {
        let ms = timeout.as_nanos().div_ceil(1_000_000);
        libc::c_int::try_from(ms).unwrap_or(libc::c_int::MAX)
    });
    let count = libc::nfds_t::try_from(fds.len())
        .map_err(|_| io::Error::from_raw_os_error(libc::EINVAL))?;
    // SAFETY: PollFd is a transparent pollfd, so the slice is `count`
    // pollfds, whose `revents` poll writes; each descriptor is borrowed
    // for the slice's life, so it is open.
    let ready = checked(unsafe { libc::poll(fds.as_mut_ptr().cast(), count, timeout) })?;
    // The count of a call that did not fail is never negative.
    Ok(ready as usize)
}

/// Whether `fd` has something to read, or its other end closed, within
/// `timeout`. A signal that interrupts the wait does not end it early.
///
/// # Errors
///
/// What [`poll`] fails with, but EINTR.
pub fn wait_readable(fd: impl AsFd, timeout: Duration) -> io::Result<bool> {
    let deadline = std::time::Instant::now() + timeout;
    loop {
        let left = deadline.saturating_duration_since(std::time::Instant::now());
        let mut fds = [PollFd::new(fd.as_fd(), libc::POLLIN)];
        match poll(&mut fds, Some(left)) {
            Ok(ready) => return Ok(ready > 0),
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => return Err(error),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::os::fd::AsFd;
    use std::time::{Duration, Instant};

    use super::{PollFd, poll, wait_readable};
    use crate::fd;

    #[test]
    fn a_pipe_with_bytes_is_readable_and_an_empty_one_is_not() {
        let (from, to) = fd::pipe().unwrap();
        let started = Instant::now();
        assert!(!wait_readable(&from, Duration::from_millis(20)).unwrap());
        assert!(started.elapsed() >= Duration::from_millis(20));
        fd::write(&to, b"x").unwrap();
        assert!(wait_readable(&from, Duration::from_secs(5)).unwrap());
    }

    #[test]
    fn each_descriptor_says_what_it_had() {
        let (from, to) = fd::pipe().unwrap();
        let (quiet, _quiet_to) = fd::pipe().unwrap();
        drop(to);
        let mut fds = [
            PollFd::new(quiet.as_fd(), libc::POLLIN),
            PollFd::optional(None, libc::POLLIN),
            PollFd::optional(Some(from.as_fd()), libc::POLLIN),
        ];
        assert_eq!(poll(&mut fds, Some(Duration::ZERO)).unwrap(), 1);
        assert_eq!(fds[0].revents(), 0);
        assert_eq!(fds[1].revents(), 0, "an entry with no descriptor is passed over");
        assert_ne!(fds[2].revents() & libc::POLLHUP, 0, "the write end is closed");
    }
}
