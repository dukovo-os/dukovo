// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The terminal a client asks for (RFC 4254 6.2): a pseudo-terminal of
//! the size it says, with the modes it sends (RFC 4254 8), as Linux's
//! sshd sets them. sshd keeps the master; the session gets the terminal.

use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};

use russh::Pty;

/// What a client asked for in its `pty-req`.
#[derive(Clone, Debug, Default)]
pub struct Asked {
    /// Its type, for `TERM`.
    pub term: String,
    /// Its size, in characters and in pixels.
    pub size: Size,
    /// Its modes.
    pub modes: Vec<(Pty, u32)>,
}

/// A terminal's size.
#[derive(Clone, Copy, Debug, Default)]
pub struct Size {
    pub columns: u32,
    pub rows: u32,
    pub width: u32,
    pub height: u32,
}

impl Size {
    fn winsize(self) -> libc::winsize {
        let fit = |n: u32| u16::try_from(n).unwrap_or(u16::MAX);
        libc::winsize {
            ws_row: fit(self.rows),
            ws_col: fit(self.columns),
            ws_xpixel: fit(self.width),
            ws_ypixel: fit(self.height),
        }
    }
}

/// Makes a pseudo-terminal as `asked` says: its master, then the
/// terminal, both closed on exec.
pub fn open(asked: &Asked) -> io::Result<(OwnedFd, OwnedFd)> {
    let (mut master, mut terminal) = (-1, -1);
    let mut size = asked.size.winsize();
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
    set_modes(&terminal, &asked.modes)?;
    Ok((master, terminal))
}

/// Gives the terminal of `master` a new size: its foreground process
/// group gets SIGWINCH.
pub fn resize(master: &OwnedFd, size: Size) {
    let size = size.winsize();
    // SAFETY: TIOCSWINSZ reads the winsize, which lives for the call.
    unsafe { libc::ioctl(master.as_raw_fd(), libc::TIOCSWINSZ, &raw const size) };
}

/// Sets the modes a client sent on `terminal`: the characters it edits
/// lines with, and how input and output are treated. One the terminal
/// does not have is passed over, as Linux's sshd passes it over.
fn set_modes(terminal: &OwnedFd, modes: &[(Pty, u32)]) -> io::Result<()> {
    // SAFETY: all zeros is a valid termios, which tcgetattr fills.
    let mut t: libc::termios = unsafe { std::mem::zeroed() };
    // SAFETY: tcgetattr writes the settings of a terminal we hold.
    if unsafe { libc::tcgetattr(terminal.as_raw_fd(), &raw mut t) } != 0 {
        return Err(io::Error::last_os_error());
    }
    for &(mode, value) in modes {
        apply(&mut t, mode, value);
    }
    // SAFETY: tcsetattr reads the settings, which live for the call.
    if unsafe { libc::tcsetattr(terminal.as_raw_fd(), libc::TCSANOW, &raw const t) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// Sets one mode on `t`.
fn apply(t: &mut libc::termios, mode: Pty, value: u32) {
    let flag = |flags: &mut libc::tcflag_t, bit: libc::tcflag_t| {
        if value != 0 {
            *flags |= bit;
        } else {
            *flags &= !bit;
        }
    };
    // A character: 255 is none, as _POSIX_VDISABLE is on Linux.
    let character = |t: &mut libc::termios, index: usize| {
        if let Some(slot) = t.c_cc.get_mut(index) {
            *slot = if value == 255 { 0 } else { value as libc::cc_t };
        }
    };
    match mode {
        Pty::VINTR => character(t, libc::VINTR),
        Pty::VQUIT => character(t, libc::VQUIT),
        Pty::VERASE => character(t, libc::VERASE),
        Pty::VKILL => character(t, libc::VKILL),
        Pty::VEOF => character(t, libc::VEOF),
        Pty::VSTART => character(t, libc::VSTART),
        Pty::VSTOP => character(t, libc::VSTOP),
        Pty::VSUSP => character(t, libc::VSUSP),
        Pty::VREPRINT => character(t, libc::VREPRINT),
        Pty::VWERASE => character(t, libc::VWERASE),
        Pty::VLNEXT => character(t, libc::VLNEXT),
        Pty::VDISCARD => character(t, libc::VDISCARD),
        Pty::IGNPAR => flag(&mut t.c_iflag, libc::IGNPAR),
        Pty::PARMRK => flag(&mut t.c_iflag, libc::PARMRK),
        Pty::INPCK => flag(&mut t.c_iflag, libc::INPCK),
        Pty::ISTRIP => flag(&mut t.c_iflag, libc::ISTRIP),
        Pty::INLCR => flag(&mut t.c_iflag, libc::INLCR),
        Pty::IGNCR => flag(&mut t.c_iflag, libc::IGNCR),
        Pty::ICRNL => flag(&mut t.c_iflag, libc::ICRNL),
        Pty::IUCLC => flag(&mut t.c_iflag, libc::IUCLC),
        Pty::IXON => flag(&mut t.c_iflag, libc::IXON),
        Pty::IXANY => flag(&mut t.c_iflag, libc::IXANY),
        Pty::IXOFF => flag(&mut t.c_iflag, libc::IXOFF),
        Pty::IMAXBEL => flag(&mut t.c_iflag, libc::IMAXBEL),
        Pty::IUTF8 => flag(&mut t.c_iflag, libc::IUTF8),
        Pty::ISIG => flag(&mut t.c_lflag, libc::ISIG),
        Pty::ICANON => flag(&mut t.c_lflag, libc::ICANON),
        Pty::XCASE => flag(&mut t.c_lflag, libc::XCASE),
        Pty::ECHO => flag(&mut t.c_lflag, libc::ECHO),
        Pty::ECHOE => flag(&mut t.c_lflag, libc::ECHOE),
        Pty::ECHOK => flag(&mut t.c_lflag, libc::ECHOK),
        Pty::ECHONL => flag(&mut t.c_lflag, libc::ECHONL),
        Pty::NOFLSH => flag(&mut t.c_lflag, libc::NOFLSH),
        Pty::TOSTOP => flag(&mut t.c_lflag, libc::TOSTOP),
        Pty::IEXTEN => flag(&mut t.c_lflag, libc::IEXTEN),
        Pty::ECHOCTL => flag(&mut t.c_lflag, libc::ECHOCTL),
        Pty::ECHOKE => flag(&mut t.c_lflag, libc::ECHOKE),
        Pty::PENDIN => flag(&mut t.c_lflag, libc::PENDIN),
        Pty::OPOST => flag(&mut t.c_oflag, libc::OPOST),
        Pty::OLCUC => flag(&mut t.c_oflag, libc::OLCUC),
        Pty::ONLCR => flag(&mut t.c_oflag, libc::ONLCR),
        Pty::OCRNL => flag(&mut t.c_oflag, libc::OCRNL),
        Pty::ONOCR => flag(&mut t.c_oflag, libc::ONOCR),
        Pty::ONLRET => flag(&mut t.c_oflag, libc::ONLRET),
        Pty::CS7 if value != 0 => t.c_cflag = (t.c_cflag & !libc::CSIZE) | libc::CS7,
        Pty::CS8 if value != 0 => t.c_cflag = (t.c_cflag & !libc::CSIZE) | libc::CS8,
        Pty::PARENB => flag(&mut t.c_cflag, libc::PARENB),
        Pty::PARODD => flag(&mut t.c_cflag, libc::PARODD),
        // Speeds mean nothing to a pseudo-terminal, and the rest are
        // characters Linux has not.
        _ => {}
    }
}
