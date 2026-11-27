// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! A pane's pseudo-terminal: admind opens the pair with the size the page
//! asked for, keeps the master, and passes the terminal side to authd,
//! which starts the shell on it. The master is read and written through
//! tokio's pipes over copies of it, as sshd's is: tokio's AsyncFd waits on
//! Radix for one event only.

use std::io;
use std::os::fd::OwnedFd;

use sysapi::terminal::{self, When, WindowSize};
use tokio::net::unix::pipe;

/// A size of `cols` columns and `rows` rows.
fn size(cols: u16, rows: u16) -> WindowSize {
    WindowSize { rows, cols, ..WindowSize::default() }
}

/// Makes a pseudo-terminal of `cols` columns and `rows` rows: its master,
/// then the terminal, both closed on exec. Line editing takes UTF-8, and
/// erases with DEL, which the page's Backspace sends.
pub fn open(cols: u16, rows: u16) -> io::Result<(OwnedFd, OwnedFd)> {
    let pty = terminal::open_pty(Some(size(cols, rows)))?;
    let mut settings = terminal::attributes(&pty.terminal)?;
    settings.c_iflag |= libc::IUTF8;
    settings.c_cc[libc::VERASE] = 0x7f;
    terminal::set_attributes(&pty.terminal, When::Now, &settings)?;
    Ok((pty.master, pty.terminal))
}

/// Gives the terminal of `master` a new size: its foreground process
/// group gets SIGWINCH.
pub fn resize(master: &OwnedFd, cols: u16, rows: u16) {
    let _ = terminal::set_window_size(master, size(cols, rows));
}

/// The terminal's foreground process group, or `None` when the kernel
/// does not say.
pub fn foreground(master: &OwnedFd) -> Option<i32> {
    terminal::foreground_group(master).ok().filter(|&group| group > 0)
}

/// What reads the output of `master` and what writes typing to it,
/// through copies of it: the terminal hangs up once all of them close.
pub fn ends(master: &OwnedFd) -> io::Result<(pipe::Receiver, pipe::Sender)> {
    let reader = master.try_clone()?;
    let writer = master.try_clone()?;
    sysapi::fd::set_nonblocking(&reader, true)?;
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
        assert!(std::io::IsTerminal::is_terminal(&terminal));
        let size = |fd: &OwnedFd| {
            let size = terminal::window_size(fd).unwrap();
            (size.cols, size.rows)
        };
        assert_eq!(size(&terminal), (120, 40));
        resize(&master, 80, 24);
        assert_eq!(size(&terminal), (80, 24));
        // No session has it as its terminal, so no group is in front.
        assert_eq!(foreground(&master), None);
    }
}
