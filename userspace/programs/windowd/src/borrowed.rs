// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The screens, borrowed from consoled (consoled(8), ADR 0713).
//!
//! consoled has the screens and lends them to the desktop, which then
//! stands in the console that showed. windowd keeps the connection it
//! asked on: through it consoled knows the desktop still wants its
//! console, and windowd says which console is to show when it gives the
//! screens back for one. When the person chooses the desktop's console
//! again, consoled lends the screens again and the display layer says
//! EVENT_FRONT.

use std::io::{self, Write};
use std::os::fd::{AsFd, BorrowedFd, OwnedFd};
use std::os::unix::net::UnixStream;

use display::loan::{Answer, Ask, LINE_MAX, SOCKET};
use sysapi::net;

/// The loan.
#[derive(Debug)]
pub struct Borrowed {
    /// The connection to consoled, until consoled ends.
    consoled: Option<UnixStream>,
    /// The console the desktop stands in.
    console: u32,
}

impl Borrowed {
    /// Asks consoled for the screens. Returns the loan and the holder
    /// of the screens, which is in front.
    ///
    /// # Errors
    ///
    /// Why the screens were not lent, for the person to read.
    pub fn ask() -> Result<(Borrowed, OwnedFd), String> {
        let consoled = UnixStream::connect(SOCKET).map_err(|err| format!("{SOCKET}: {err}"))?;
        let mut line = [0u8; LINE_MAX];
        let mut passed = Vec::new();
        let read = net::receive_with_fds(&consoled, &mut line, &mut passed, 1)
            .map_err(|err| format!("{SOCKET}: {err}"))?;
        let line = String::from_utf8_lossy(&line[..read]);
        match (Answer::parse(&line), passed.pop()) {
            (Some(Answer::Lent(console)), Some(holder)) => {
                // What consoled sends from here on is only its end.
                consoled.set_nonblocking(true).map_err(|err| format!("{SOCKET}: {err}"))?;
                Ok((Borrowed { consoled: Some(consoled), console }, holder))
            }
            (Some(Answer::Busy), _) => {
                Err("another program has the screens, or no screen shows the consoles".to_owned())
            }
            (Some(Answer::Refused), _) => Err("the desktop is started by root, or from the \
                 console that shows on the screens"
                .to_owned()),
            (Some(Answer::Failed), _) => {
                Err("consoled could not lend the screens; its log says why".to_owned())
            }
            _ => Err(format!("consoled answered `{}`", line.trim_end())),
        }
    }

    /// The connection, to wait on: it is readable when consoled ended.
    pub fn consoled(&self) -> Option<BorrowedFd<'_>> {
        self.consoled.as_ref().map(AsFd::as_fd)
    }

    /// consoled ended. The desktop keeps the screens, and the consoled
    /// that starts in its place stands under it (radix-display(2),
    /// STAND_UNDER).
    pub fn consoled_ended(&mut self) {
        self.consoled = None;
    }

    /// Whether the desktop is to give the screens back so that
    /// `console` shows, which this tells consoled. Not for the
    /// desktop's own console, and not once consoled ended: the consoled
    /// after it would not lend the screens again.
    ///
    /// # Errors
    ///
    /// What write(2) fails with.
    pub fn leaves_for(&mut self, console: u32) -> io::Result<bool> {
        let Some(consoled) = self.consoled.as_mut().filter(|_| console != self.console) else {
            return Ok(false);
        };
        consoled.write_all(Ask::Console(console).line().as_bytes())?;
        Ok(true)
    }
}
