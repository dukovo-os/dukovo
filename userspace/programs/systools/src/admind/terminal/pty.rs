// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! A pane's pseudo-terminal: admind opens the pair with the size the page
//! asked for, keeps the master, and passes the terminal side to authd,
//! which starts the shell on it. The master is read and written through
//! tokio's pipes over copies of it, as sshd's is: tokio's AsyncFd waits on
//! Radix for one event only.

use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};

use tokio::net::unix::pipe;

/// Makes a pseudo-terminal of `cols` columns and `rows` rows: its master,
/// then the terminal, both closed on exec. Line editing takes UTF-8, and
/// erases with DEL, which the page's Backspace sends.
pub fn open(cols: u16, rows: u16) -> io::Result<(OwnedFd, OwnedFd)> {
    let (mut master, mut terminal) = (-1, -1);
    let mut size = libc::winsize { ws_row: rows, ws_col: cols, ws_xpixel: 0, ws_ypixel: 0 };
    // SAFETY: openpty writes the two descriptors; no name is asked for,
    // and the settings are left as the kernel makes them.
    let made = unsafe {
        libc::openpty(
            &raw mut master,
            &raw mut terminal,
            std::ptr::null_mut(),
            std::ptr::null(),
            &raw mut size,
        )
    };
    if made != 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: openpty made both descriptors, which are ours alone.
    let (master, terminal) =
        unsafe { (OwnedFd::from_raw_fd(master), OwnedFd::from_raw_fd(terminal)) };
    for fd in [&master, &terminal] {
        // SAFETY: fcntl sets a flag of a descriptor we hold.
        unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_SETFD, libc::FD_CLOEXEC) };
    }
    // SAFETY: all zeros is a valid termios, which tcgetattr fills.
    let mut settings: libc::termios = unsafe { std::mem::zeroed() };
    // SAFETY: tcgetattr writes the settings of a terminal we hold.
    if unsafe { libc::tcgetattr(terminal.as_raw_fd(), &raw mut settings) } != 0 {
        return Err(io::Error::last_os_error());
    }
    settings.c_iflag |= libc::IUTF8;
    settings.c_cc[libc::VERASE] = 0x7f;
    // SAFETY: tcsetattr reads the settings, which live for the call.
    if unsafe { libc::tcsetattr(terminal.as_raw_fd(), libc::TCSANOW, &raw const settings) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok((master, terminal))
}

/// Gives the terminal of `master` a new size: its foreground process
/// group gets SIGWINCH.
pub fn resize(master: &OwnedFd, cols: u16, rows: u16) {
    let size = libc::winsize { ws_row: rows, ws_col: cols, ws_xpixel: 0, ws_ypixel: 0 };
    // SAFETY: TIOCSWINSZ reads the winsize, which lives for the call.
    unsafe { libc::ioctl(master.as_raw_fd(), libc::TIOCSWINSZ, &raw const size) };
}

/// The terminal's foreground process group, or `None` when the kernel
/// does not say.
pub fn foreground(master: &OwnedFd) -> Option<i32> {
    // SAFETY: tcgetpgrp only reads the terminal of a descriptor we hold.
    let group = unsafe { libc::tcgetpgrp(master.as_raw_fd()) };
    (group > 0).then_some(group)
}

/// Makes `fd` not wait.
fn set_nonblocking(fd: &OwnedFd) -> io::Result<()> {
    // SAFETY: fcntl reads and sets the flags of a descriptor we hold.
    unsafe {
        let flags = libc::fcntl(fd.as_raw_fd(), libc::F_GETFL);
        if flags < 0 || libc::fcntl(fd.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK) < 0 {
            return Err(io::Error::last_os_error());
        }
    }
    Ok(())
}

/// What reads the output of `master` and what writes typing to it,
/// through copies of it: the terminal hangs up once all of them close.
pub fn ends(master: &OwnedFd) -> io::Result<(pipe::Receiver, pipe::Sender)> {
    let reader = master.try_clone()?;
    let writer = master.try_clone()?;
    set_nonblocking(&reader)?;
    Ok((
        pipe::Receiver::from_owned_fd_unchecked(reader)?,
        pipe::Sender::from_owned_fd_unchecked(writer)?,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_terminal_has_the_size_asked_and_takes_another() {
        let (master, terminal) = open(120, 40).unwrap();
        assert!(crate::signin::is_terminal(&terminal));
        let size = |fd: &OwnedFd| {
            let mut size = libc::winsize { ws_row: 0, ws_col: 0, ws_xpixel: 0, ws_ypixel: 0 };
            // SAFETY: TIOCGWINSZ writes the winsize, which lives for the call.
            unsafe { libc::ioctl(fd.as_raw_fd(), libc::TIOCGWINSZ, &raw mut size) };
            (size.ws_col, size.ws_row)
        };
        assert_eq!(size(&terminal), (120, 40));
        resize(&master, 80, 24);
        assert_eq!(size(&terminal), (80, 24));
        // No session has it as its terminal, so no group is in front.
        assert_eq!(foreground(&master), None);
    }
}
