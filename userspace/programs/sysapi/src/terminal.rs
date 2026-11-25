// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Terminals: their settings (termios), pseudo-terminals, the window
//! size, input not yet read, which terminal and session a descriptor is,
//! and the controlling terminal and its foreground.

use std::ffi::OsStr;
use std::io;
use std::os::fd::{AsFd, AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::ffi::OsStrExt;
use std::path::PathBuf;

use crate::{Pid, checked};

/// A terminal's settings, as tcgetattr(3) gives them: its modes, its
/// special characters and its speeds.
pub type Attributes = libc::termios;

/// When [`set_attributes`] changes the settings.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum When {
    /// At once (TCSANOW).
    Now,
    /// Once what was written has gone out (TCSADRAIN).
    Drained,
    /// Once what was written has gone out, dropping what came in and was
    /// not read (TCSAFLUSH).
    Flushed,
}

/// The settings of terminal `fd`.
///
/// # Errors
///
/// ENOTTY when `fd` is not a terminal.
pub fn attributes(fd: impl AsFd) -> io::Result<Attributes> {
    // SAFETY: all zeros is a valid termios: numbers and an array of them.
    let mut attributes: Attributes = unsafe { std::mem::zeroed() };
    // SAFETY: tcgetattr writes the termios it is given.
    checked(unsafe { libc::tcgetattr(fd.as_fd().as_raw_fd(), &raw mut attributes) })?;
    Ok(attributes)
}

/// Sets the settings of terminal `fd` to `attributes`, `when` it says.
///
/// # Errors
///
/// ENOTTY when `fd` is not a terminal, EINTR when a signal came while it
/// waited for the output to drain.
pub fn set_attributes(fd: impl AsFd, when: When, attributes: &Attributes) -> io::Result<()> {
    let when = match when {
        When::Now => libc::TCSANOW,
        When::Drained => libc::TCSADRAIN,
        When::Flushed => libc::TCSAFLUSH,
    };
    // SAFETY: tcsetattr reads the termios it is given.
    checked(unsafe { libc::tcsetattr(fd.as_fd().as_raw_fd(), when, attributes) })?;
    Ok(())
}

/// Changes `attributes` to a raw terminal's, as cfmakeraw(3) does: no echo,
/// no line editing, no signals from keys, and 8-bit characters.
pub fn make_raw(attributes: &mut Attributes) {
    // SAFETY: cfmakeraw changes the termios it is given, and nothing else.
    unsafe { libc::cfmakeraw(attributes) };
}

/// The output speed `attributes` hold, as a `B` constant (`B115200`).
#[must_use]
pub fn output_speed(attributes: &Attributes) -> libc::speed_t {
    // SAFETY: cfgetospeed reads the termios it is given.
    unsafe { libc::cfgetospeed(attributes) }
}

/// A terminal's size, which a program reads to lay out its screen: in
/// characters, and in pixels where the terminal says them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WindowSize {
    /// Lines.
    pub rows: u16,
    /// Characters on a line.
    pub cols: u16,
    /// Its width in pixels, or 0.
    pub width: u16,
    /// Its height in pixels, or 0.
    pub height: u16,
}

impl WindowSize {
    fn winsize(self) -> libc::winsize {
        libc::winsize {
            ws_row: self.rows,
            ws_col: self.cols,
            ws_xpixel: self.width,
            ws_ypixel: self.height,
        }
    }
}

/// The size of terminal `fd`.
///
/// # Errors
///
/// ENOTTY when `fd` is not a terminal.
pub fn window_size(fd: impl AsFd) -> io::Result<WindowSize> {
    let mut size = WindowSize::default().winsize();
    // SAFETY: TIOCGWINSZ writes the winsize it is given.
    checked(unsafe { libc::ioctl(fd.as_fd().as_raw_fd(), libc::TIOCGWINSZ, &raw mut size) })?;
    Ok(WindowSize {
        rows: size.ws_row,
        cols: size.ws_col,
        width: size.ws_xpixel,
        height: size.ws_ypixel,
    })
}

/// Sets the size of terminal `fd`, which sends the foreground process
/// group SIGWINCH when it changes.
///
/// # Errors
///
/// ENOTTY when `fd` is not a terminal.
pub fn set_window_size(fd: impl AsFd, size: WindowSize) -> io::Result<()> {
    let size = size.winsize();
    // SAFETY: TIOCSWINSZ reads the winsize it is given.
    checked(unsafe { libc::ioctl(fd.as_fd().as_raw_fd(), libc::TIOCSWINSZ, &raw const size) })?;
    Ok(())
}

/// A pseudo-terminal: the side a terminal emulator or a server reads and
/// writes, and the terminal a program runs on.
pub struct Pty {
    /// The emulator's side (the master).
    pub master: OwnedFd,
    /// The terminal itself, which a program takes as 0 to 2.
    pub terminal: OwnedFd,
    /// The terminal's path, such as `/devices/pts/3`.
    pub path: PathBuf,
}

/// Opens a pseudo-terminal of `size` (the system's default without one),
/// as openpty(3) does, each side closed on exec.
///
/// # Errors
///
/// EAGAIN or ENOSPC when none is free.
pub fn open_pty(size: Option<WindowSize>) -> io::Result<Pty> {
    let (mut master, mut terminal) = (-1, -1);
    // The C library writes the terminal's path, which is far shorter.
    let mut path = [0u8; 64];
    let size = size.map(WindowSize::winsize);
    let size = size.as_ref().map_or(std::ptr::null(), std::ptr::from_ref);
    // SAFETY: openpty writes the two descriptors and the path, which fits
    // the buffer, and reads the winsize when it is given one.
    checked(unsafe {
        libc::openpty(
            &raw mut master,
            &raw mut terminal,
            path.as_mut_ptr().cast(),
            std::ptr::null(),
            size,
        )
    })?;
    // SAFETY: openpty made both descriptors, and nothing else holds them.
    let (master, terminal) =
        unsafe { (OwnedFd::from_raw_fd(master), OwnedFd::from_raw_fd(terminal)) };
    for fd in [&master, &terminal] {
        // SAFETY: F_SETFD sets the descriptor flags of a descriptor held
        // here from an integer.
        checked(unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_SETFD, libc::FD_CLOEXEC) })?;
    }
    let end = path.iter().position(|&b| b == 0).unwrap_or(path.len());
    Ok(Pty { master, terminal, path: PathBuf::from(OsStr::from_bytes(&path[..end])) })
}

/// The path of terminal `fd`, as ttyname(3) gives it.
///
/// # Errors
///
/// ENOTTY when `fd` is not a terminal.
pub fn path(fd: impl AsFd) -> io::Result<PathBuf> {
    let mut path = [0u8; 256];
    // SAFETY: ttyname_r writes at most `path.len()` bytes into `path`,
    // ending them with a NUL, and returns an errno.
    match unsafe { libc::ttyname_r(fd.as_fd().as_raw_fd(), path.as_mut_ptr().cast(), path.len()) } {
        0 => {
            let end = path.iter().position(|&b| b == 0).unwrap_or(path.len());
            Ok(PathBuf::from(OsStr::from_bytes(&path[..end])))
        }
        errno => Err(io::Error::from_raw_os_error(errno)),
    }
}

/// Drops what terminal `fd` received and nothing has read yet
/// (tcflush(3) with TCIFLUSH): keys pressed before a prompt asked.
///
/// # Errors
///
/// ENOTTY when `fd` is not a terminal.
pub fn drop_input(fd: impl AsFd) -> io::Result<()> {
    // SAFETY: tcflush takes a descriptor held here and a number.
    checked(unsafe { libc::tcflush(fd.as_fd().as_raw_fd(), libc::TCIFLUSH) })?;
    Ok(())
}

/// Makes terminal `fd` the controlling terminal of this process's
/// session (TIOCSCTTY), which this process leads and which has none: as
/// a program that started a session does before it runs a shell there.
///
/// # Errors
///
/// EPERM when this process leads no session, or the session has a
/// terminal already; ENOTTY when `fd` is not a terminal.
pub fn take_control(fd: impl AsFd) -> io::Result<()> {
    // SAFETY: TIOCSCTTY takes an integer argument, 0: do not take the
    // terminal from another session.
    checked(unsafe { libc::ioctl(fd.as_fd().as_raw_fd(), libc::TIOCSCTTY, 0) })?;
    Ok(())
}

/// The foreground process group of terminal `fd` (tcgetpgrp(3)): the one
/// its keys' signals go to.
///
/// # Errors
///
/// ENOTTY when `fd` is not this process's controlling terminal, nor the
/// master of a pseudo-terminal.
pub fn foreground_group(fd: impl AsFd) -> io::Result<Pid> {
    // SAFETY: tcgetpgrp takes a descriptor held here.
    checked(unsafe { libc::tcgetpgrp(fd.as_fd().as_raw_fd()) })
}

/// The session whose controlling terminal `fd` is, as tcgetsid(3) gives
/// it.
///
/// # Errors
///
/// ENOTTY when `fd` is not a terminal, or no session's controlling one.
pub fn session(fd: impl AsFd) -> io::Result<Pid> {
    // SAFETY: tcgetsid takes a descriptor held here.
    checked(unsafe { libc::tcgetsid(fd.as_fd().as_raw_fd()) })
}

#[cfg(test)]
mod tests {
    use super::{
        When, WindowSize, attributes, drop_input, foreground_group, make_raw, open_pty, path,
        set_attributes, set_window_size, window_size,
    };

    #[test]
    fn a_pty_has_the_size_it_was_opened_with_and_takes_a_new_one() {
        let small = WindowSize { rows: 24, cols: 80, ..WindowSize::default() };
        let pty = open_pty(Some(small)).unwrap();
        assert_eq!(window_size(&pty.terminal).unwrap(), small);
        let large = WindowSize { rows: 50, cols: 132, width: 1320, height: 1000 };
        set_window_size(&pty.master, large).unwrap();
        assert_eq!(window_size(&pty.terminal).unwrap(), large);
        assert_eq!(path(&pty.terminal).unwrap(), pty.path);
    }

    #[test]
    fn a_raw_terminal_does_not_echo() {
        let pty = open_pty(None).unwrap();
        let mut settings = attributes(&pty.terminal).unwrap();
        assert_ne!(settings.c_lflag & libc::ECHO, 0);
        make_raw(&mut settings);
        set_attributes(&pty.terminal, When::Now, &settings).unwrap();
        assert_eq!(attributes(&pty.terminal).unwrap().c_lflag & libc::ECHO, 0);
    }

    #[test]
    fn input_not_read_is_dropped() {
        let pty = open_pty(None).unwrap();
        let mut settings = attributes(&pty.terminal).unwrap();
        make_raw(&mut settings);
        set_attributes(&pty.terminal, When::Now, &settings).unwrap();
        crate::fd::write(&pty.master, b"typed ahead").unwrap();
        assert!(
            crate::poll::wait_readable(&pty.terminal, std::time::Duration::from_secs(5)).unwrap()
        );
        drop_input(&pty.terminal).unwrap();
        let waiting = crate::poll::wait_readable(&pty.terminal, std::time::Duration::ZERO);
        assert!(!waiting.unwrap(), "nothing is left to read");
    }

    #[test]
    fn a_terminal_this_process_does_not_control_has_no_foreground_for_it() {
        // tests/one_thread.rs takes control of one in a new session.
        let pty = open_pty(None).unwrap();
        assert_eq!(foreground_group(&pty.terminal).unwrap_err().raw_os_error(), Some(libc::ENOTTY));
    }

    #[test]
    fn a_pipe_is_no_terminal() {
        let (from, _to) = crate::fd::pipe().unwrap();
        assert_eq!(attributes(&from).unwrap_err().raw_os_error(), Some(libc::ENOTTY));
        assert_eq!(path(&from).unwrap_err().raw_os_error(), Some(libc::ENOTTY));
        assert_eq!(drop_input(&from).unwrap_err().raw_os_error(), Some(libc::ENOTTY));
    }
}
