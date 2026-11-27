// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Lending the screens to a program that draws them itself, such as a
//! desktop (ADR 0702).
//!
//! consoled has the screens from the kernel's log, and the display layer
//! lets only the program in front pass them on (radix-display(2)). So a
//! program that wants them asks consoled, on [`SOCKET`], which only root
//! may open. Connecting is the request. consoled answers with one line
//! and closes the connection:
//!
//! ```text
//! lent
//! busy
//! failed
//! ```
//!
//! With `lent` comes a descriptor (SCM_RIGHTS): a holder of
//! _/devices/display/control_ of the borrower's own, which is in front.
//! The screens show the consoles until the borrower's first change.
//! `busy` says consoled does not have the screens to lend: another
//! program has borrowed them, or no screen shows the consoles. `failed`
//! says the loan could not be made, and consoled's log says why.
//!
//! The borrower gives the screens back with RETURN, and the consoles
//! show again within a frame. Or it ends, by closing the descriptor or
//! by ending itself, as one that crashes or is started again does. Its
//! last picture then stays on the screens for five seconds
//! ([`crate::screen::BORROWER_WAIT`]), and a program that borrows the
//! screens within them draws its first frame over that picture: a
//! desktop that starts again shows no console between. After the five
//! seconds the consoles show.
//!
//! While the screens are lent, and through those five seconds, what is
//! typed on the keyboard is not typed into a console.

use std::io::{self, ErrorKind};
use std::os::fd::{AsFd, BorrowedFd};
use std::os::unix::net::{UnixListener, UnixStream};

use sysapi::net;

use crate::braille;
use crate::screen::Screen;

/// Where consoled listens for a program that asks for the screens.
pub const SOCKET: &str = "/system/live/consoled/display";

/// The answers.
const LENT: &str = "lent\n";
const BUSY: &str = "busy\n";
const FAILED: &str = "failed\n";

/// The socket programs ask for the screens on.
#[derive(Debug, Default)]
pub struct Lender {
    listener: Option<UnixListener>,
}

impl Lender {
    /// Listens on `path`, which only root may open. If it cannot, it
    /// says why, and the screens stay the consoles'.
    pub fn listen(path: &str) -> Lender {
        match braille::listener(path) {
            Ok(listener) => Lender { listener: Some(listener) },
            Err(err) => {
                eprintln!("consoled: the screens cannot be lent: {path}: {err}");
                Lender::default()
            }
        }
    }

    /// The listening socket, if there is one, to wait on.
    pub fn fd(&self) -> Option<BorrowedFd<'_>> {
        self.listener.as_ref().map(AsFd::as_fd)
    }

    /// Answers the program that connected: lends it `screen` if consoled
    /// has the screens to lend.
    pub fn accept(&self, screen: &mut Screen) {
        let Some(listener) = &self.listener else { return };
        let client = match listener.accept() {
            Ok((client, _)) => client,
            Err(err) if err.kind() == ErrorKind::WouldBlock => return,
            Err(err) => {
                eprintln!(
                    "consoled: a program that asked for the screens could not connect: {err}"
                );
                return;
            }
        };
        let borrower = match screen.lend() {
            Ok(borrower) => borrower,
            Err(err) => {
                let busy = err.raw_os_error() == Some(libc::EBUSY);
                if !busy {
                    eprintln!("consoled: cannot lend the screens: {err}");
                }
                let _ = send(&client, if busy { BUSY } else { FAILED }, None);
                return;
            }
        };
        match send(&client, LENT, Some(borrower.as_fd())) {
            Ok(()) => eprintln!("consoled: the screens are lent to another program"),
            Err(err) => {
                eprintln!("consoled: the program that asked for the screens left: {err}");
                screen.unlent();
            }
        }
        // consoled's copy of the borrower's description closes here, sent
        // or not. The borrower's is then the only one, and the layer
        // knows the borrower has ended when that one closes.
    }
}

/// Sends `answer` on `client`, with the descriptor `passed` if there is
/// one (SCM_RIGHTS). An answer is a few bytes, which a socket nobody has
/// written to takes whole or not at all.
fn send(client: &UnixStream, answer: &str, passed: Option<BorrowedFd<'_>>) -> io::Result<()> {
    net::send_with_fds(client, answer.as_bytes(), passed.as_slice())
}

#[cfg(test)]
mod tests {
    use std::fs::File;
    use std::io::{Read, Seek, SeekFrom, Write};
    use std::os::fd::{AsRawFd, OwnedFd};

    use super::*;

    /// Reads what `stream` has, and the descriptor that came with it.
    fn receive(stream: &UnixStream) -> (String, Option<OwnedFd>) {
        let mut bytes = [0u8; 32];
        let mut passed = Vec::new();
        let n = net::receive_with_fds(stream, &mut bytes, &mut passed, 1).unwrap();
        (String::from_utf8_lossy(&bytes[..n]).into_owned(), passed.pop())
    }

    #[test]
    fn a_loan_is_answered_with_a_descriptor_and_a_refusal_with_none() {
        let (ours, theirs) = UnixStream::pair().unwrap();
        // Any file stands in for the layer's: what is passed is the same
        // open file, which the borrower reads and writes as its own.
        let mut file = tempfile();
        file.write_all(b"the borrower's").unwrap();
        send(&ours, LENT, Some(file.as_fd())).unwrap();
        let (answer, passed) = receive(&theirs);
        assert_eq!(answer, "lent\n");
        let mut passed = File::from(passed.expect("a descriptor comes with a loan"));
        assert_ne!(passed.as_raw_fd(), file.as_raw_fd());
        passed.seek(SeekFrom::Start(0)).unwrap();
        let mut read = String::new();
        passed.read_to_string(&mut read).unwrap();
        assert_eq!(read, "the borrower's");

        send(&ours, BUSY, None).unwrap();
        let (answer, passed) = receive(&theirs);
        assert_eq!((answer.as_str(), passed.is_none()), ("busy\n", true));
        // A program that asked and left is an error to the sender, which
        // then knows the loan never arrived.
        drop(theirs);
        assert!(send(&ours, LENT, Some(file.as_fd())).is_err());
    }

    /// A file of the test's own that is gone when it is closed.
    fn tempfile() -> File {
        let path = std::env::temp_dir().join(format!("consoled-lend-{}", std::process::id()));
        let file = File::options().read(true).write(true).create(true).truncate(true).open(&path);
        let _ = std::fs::remove_file(&path);
        file.unwrap()
    }

    #[test]
    fn without_its_socket_nothing_is_lent() {
        let lender = Lender::default();
        assert!(lender.fd().is_none());
    }
}
