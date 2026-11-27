// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Lending the screens to a program that draws them itself, such as a
//! desktop (ADR 0702, ADR 0713).
//!
//! consoled has the screens from the kernel's log, and the display layer
//! lets only the program in front pass them on (radix-display(2)). So a
//! program that wants them asks consoled, on [`SOCKET`]. Connecting is
//! the request, and consoled answers with one line (`display::loan`):
//!
//! ```text
//! lent 2
//! busy
//! failed
//! refused
//! ```
//!
//! Who asks is what the kernel says of the connection, never what the
//! program says: the screens are lent to root's programs, and to a
//! program of the session whose terminal is the console that shows,
//! which is a program the person at the screens started there. Any
//! other is `refused`.
//!
//! With `lent` comes a descriptor (SCM_RIGHTS): a holder of
//! _/devices/display/control_ of the borrower's own, which is in front.
//! The number is the console the borrower stands in, the one that
//! showed. The screens show the consoles until the borrower's first
//! change. `busy` says consoled does not have the screens to lend:
//! another program has borrowed them or stands in a console, or no
//! screen shows the consoles. `failed` says the loan could not be made,
//! and consoled's log says why.
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
//! A borrower that keeps the connection open stands in its console for
//! as long as it does. Before it gives the screens back for another
//! console it says which:
//!
//! ```text
//! console 3
//! ```
//!
//! and when the person chooses the borrower's console again, consoled
//! lends it the screens again, with what it last showed: it reads
//! EVENT_FRONT. For that consoled keeps the borrower's description open
//! too, until the borrower closes the connection, which a borrower that
//! ends does. One that says anything else is not heard again.
//!
//! While the screens are lent, and through those five seconds, what is
//! typed on the keyboard is not typed into a console.

use std::fs::Permissions;
use std::io::{self, ErrorKind, Read};
use std::os::fd::{AsFd, BorrowedFd};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};

pub use display::loan::SOCKET;
use display::loan::{Answer, Ask, LINE_MAX};
use sysapi::{Pid, kernel, net};

use crate::braille;
use crate::screen::{Device, Screen};

/// A borrower that kept its connection: it stands in a console.
#[derive(Debug)]
struct Loan {
    /// The connection, which the borrower writes its lines on.
    stream: UnixStream,
    /// consoled's own descriptor of the borrower's holder, to lend the
    /// screens to again. While it is open the layer does not know of a
    /// borrower that ended, so it is closed as soon as the connection
    /// is.
    holder: Device,
    /// The index of the console the borrower stands in.
    console: usize,
    /// What the borrower wrote of a line that is not whole yet.
    line: Vec<u8>,
}

/// What a borrower's connection had to say.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Heard {
    /// The index of the console it last asked for.
    pub console: Option<usize>,
    /// It closed the connection, or wrote what is no line of its: it is
    /// heard no more.
    pub left: bool,
}

/// The socket programs ask for the screens on, and the borrower that
/// stands in a console.
#[derive(Debug, Default)]
pub struct Lender {
    listener: Option<UnixListener>,
    loan: Option<Loan>,
}

impl Lender {
    /// Listens on `path`, which any program may open: who may have the
    /// screens is decided when one asks. If it cannot listen, it says
    /// why, and the screens stay the consoles'.
    pub fn listen(path: &str) -> Lender {
        let listening = braille::listener(path).and_then(|listener| {
            std::fs::set_permissions(path, Permissions::from_mode(0o666))?;
            Ok(listener)
        });
        match listening {
            Ok(listener) => Lender { listener: Some(listener), loan: None },
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

    /// The connection of the borrower that stands in a console, if one
    /// does, to wait on.
    pub fn borrower_fd(&self) -> Option<BorrowedFd<'_>> {
        self.loan.as_ref().map(|loan| loan.stream.as_fd())
    }

    /// The index of the console a borrower stands in, if one does.
    pub fn stands_in(&self) -> Option<usize> {
        self.loan.as_ref().map(|loan| loan.console)
    }

    /// Answers the program that connected: lends it `screen` if it may
    /// have the screens and consoled has them to lend. `shown` is the
    /// index of the console that shows, which the borrower then stands
    /// in, and `session` the session whose terminal that console is.
    pub fn accept(&mut self, screen: &mut Screen, shown: usize, session: Option<Pid>) {
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
        if !may_borrow(asker(&client), session, session_of) {
            let _ = send(&client, Answer::Refused, None);
            return;
        }
        // A borrower that ended a moment ago closed its connection, and
        // is no reason to refuse the one started in its place.
        self.hear();
        if self.loan.is_some() {
            let _ = send(&client, Answer::Busy, None);
            return;
        }
        let borrower = match screen.lend() {
            Ok(borrower) => borrower,
            Err(err) => {
                let busy = err.raw_os_error() == Some(libc::EBUSY);
                if !busy {
                    eprintln!("consoled: cannot lend the screens: {err}");
                }
                let _ = send(&client, if busy { Answer::Busy } else { Answer::Failed }, None);
                return;
            }
        };
        let sent = send(&client, Answer::Lent(shown as u32 + 1), Some(borrower.as_fd()))
            .and_then(|()| client.set_nonblocking(true));
        match sent {
            Ok(()) => {
                eprintln!("consoled: the screens are lent to another program");
                self.loan =
                    Some(Loan { stream: client, holder: borrower, console: shown, line: vec![] });
            }
            // consoled's copy of the borrower's description closes
            // here: the layer then knows the borrower by its own alone.
            Err(err) => {
                eprintln!("consoled: the program that asked for the screens left: {err}");
                screen.unlent();
            }
        }
    }

    /// Reads what the borrower that stands in a console wrote. When it
    /// has closed the connection, consoled's descriptor of its holder is
    /// closed too, and no borrower stands in a console.
    pub fn hear(&mut self) -> Heard {
        let Some(loan) = &mut self.loan else { return Heard::default() };
        let mut heard = Heard::default();
        let mut bytes = [0u8; 4 * LINE_MAX];
        loop {
            match (&loan.stream).read(&mut bytes) {
                Ok(0) => heard.left = true,
                Ok(read) => match asks(&mut loan.line, &bytes[..read]) {
                    Some(asks) => {
                        for Ask::Console(console) in asks {
                            heard.console = Some(console as usize - 1);
                        }
                        continue;
                    }
                    None => heard.left = true,
                },
                Err(err) if err.kind() == ErrorKind::Interrupted => continue,
                Err(err) if err.kind() == ErrorKind::WouldBlock => {}
                Err(_) => heard.left = true,
            }
            break;
        }
        if heard.left {
            self.loan = None;
        }
        heard
    }

    /// Lends `screen` again to the borrower that stands in a console:
    /// the person chose its console. Returns whether it has the screens;
    /// if not, the console itself shows.
    pub fn lend_again(&mut self, screen: &mut Screen) -> bool {
        let Some(loan) = &self.loan else { return false };
        match screen.lend_to(&loan.holder) {
            Ok(()) => true,
            // It has them, or consoled has them not to lend.
            Err(err) if err.raw_os_error() == Some(libc::EBUSY) => false,
            Err(err) => {
                eprintln!("consoled: cannot lend the screens again: {err}");
                false
            }
        }
    }
}

/// Takes `bytes`, which a borrower wrote, after what `line` holds of a
/// line that was not whole, and returns what its whole lines ask: `None`
/// if one of them is no line of a borrower's, or is longer than a line
/// is.
fn asks(line: &mut Vec<u8>, bytes: &[u8]) -> Option<Vec<Ask>> {
    let mut asks = Vec::new();
    for &byte in bytes {
        line.push(byte);
        if byte == b'\n' {
            asks.push(Ask::parse(str::from_utf8(line).ok()?)?);
            line.clear();
        } else if line.len() >= LINE_MAX {
            return None;
        }
    }
    Some(asks)
}

/// The process at the other end of a connection, as the kernel has it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Asker {
    /// The user it acted for when it connected.
    user: u32,
    /// Its number.
    process: u64,
}

/// Who connected `client` (radix-file(2)'s `socket_state`).
fn asker(client: &UnixStream) -> Option<Asker> {
    let state = radix_abi::file::socket_state(&kernel::SYS, kernel::slot_of(client)).ok()?;
    Some(Asker { user: state.user, process: state.peer })
}

/// The session process `number` is of, as _/processes_ has it.
fn session_of(number: u64) -> Option<Pid> {
    session_in(&std::fs::read_to_string(format!("/processes/{number}/status")).ok()?)
}

/// The session a process's `status` names.
fn session_in(status: &str) -> Option<Pid> {
    status.lines().find_map(|line| line.strip_prefix("session: "))?.trim().parse().ok()
}

/// Whether `asker` may have the screens: root's program, or one of
/// `session`, the session whose terminal the console that shows is.
/// `session_of` gives a process's session.
fn may_borrow(
    asker: Option<Asker>,
    session: Option<Pid>,
    session_of: impl FnOnce(u64) -> Option<Pid>,
) -> bool {
    let Some(asker) = asker else { return false };
    asker.user == 0
        || session.is_some_and(|session| session > 0 && session_of(asker.process) == Some(session))
}

/// Sends `answer` on `client`, with the descriptor `passed` if there is
/// one (SCM_RIGHTS). An answer is a few bytes, which a socket nobody has
/// written to takes whole or not at all.
fn send(client: &UnixStream, answer: Answer, passed: Option<BorrowedFd<'_>>) -> io::Result<()> {
    net::send_with_fds(client, answer.line().as_bytes(), passed.as_slice())
}

#[cfg(test)]
mod tests {
    use std::fs::File;
    use std::io::{Read, Seek, SeekFrom, Write};
    use std::os::fd::{AsRawFd, OwnedFd};

    use super::*;

    /// Reads what `stream` has, and the descriptor that came with it.
    fn receive(stream: &UnixStream) -> (String, Option<OwnedFd>) {
        let mut bytes = [0u8; LINE_MAX];
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
        send(&ours, Answer::Lent(2), Some(file.as_fd())).unwrap();
        let (answer, passed) = receive(&theirs);
        assert_eq!(Answer::parse(&answer), Some(Answer::Lent(2)));
        let mut passed = File::from(passed.expect("a descriptor comes with a loan"));
        assert_ne!(passed.as_raw_fd(), file.as_raw_fd());
        passed.seek(SeekFrom::Start(0)).unwrap();
        let mut read = String::new();
        passed.read_to_string(&mut read).unwrap();
        assert_eq!(read, "the borrower's");

        send(&ours, Answer::Busy, None).unwrap();
        let (answer, passed) = receive(&theirs);
        assert_eq!((answer.as_str(), passed.is_none()), ("busy\n", true));
        // A program that asked and left is an error to the sender, which
        // then knows the loan never arrived.
        drop(theirs);
        assert!(send(&ours, Answer::Lent(1), Some(file.as_fd())).is_err());
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
        let mut lender = Lender::default();
        assert!(lender.fd().is_none() && lender.borrower_fd().is_none());
        assert_eq!((lender.stands_in(), lender.hear()), (None, Heard::default()));
    }

    #[test]
    fn the_screens_are_lent_to_root_and_to_the_session_of_the_console_that_shows() {
        let root = Some(Asker { user: 0, process: 40 });
        let person = Some(Asker { user: 1000, process: 77 });
        let nobody = |_| None;
        let in_session = |session| move |process| (process == 77).then_some(session);
        assert!(may_borrow(root, None, nobody));
        assert!(may_borrow(root, Some(9), in_session(12)));
        assert!(may_borrow(person, Some(12), in_session(12)));
        // Started at another console, or with nobody signed in at the
        // one that shows, or by a process that is gone.
        assert!(!may_borrow(person, Some(9), in_session(12)));
        assert!(!may_borrow(person, None, in_session(12)));
        assert!(!may_borrow(person, Some(0), in_session(0)));
        assert!(!may_borrow(person, Some(12), nobody));
        // And a connection the kernel says nothing of.
        assert!(!may_borrow(None, Some(12), in_session(12)));
    }

    #[test]
    fn a_process_s_session_is_read_from_its_status() {
        let status = "number: 77\nname: windowd\nparent: 70\ngroup: 77\nsession: 61\n";
        assert_eq!(session_in(status), Some(61));
        assert_eq!(session_in("number: 77\nterminal_session: 4\n"), None);
        assert_eq!(session_in("session: none\n"), None);
    }

    #[test]
    fn a_borrower_s_lines_are_put_together_however_the_socket_cuts_them() {
        let mut line = Vec::new();
        assert_eq!(asks(&mut line, b"cons"), Some(vec![]));
        assert_eq!(
            asks(&mut line, b"ole 3\nconsole 1\ncon"),
            Some(vec![Ask::Console(3), Ask::Console(1)])
        );
        assert_eq!(asks(&mut line, b"sole 12\n"), Some(vec![Ask::Console(12)]));
        assert!(line.is_empty());
        // What is no line of a borrower's, and a line with no end.
        assert_eq!(asks(&mut Vec::new(), b"console 0\n"), None);
        assert_eq!(asks(&mut Vec::new(), b"lent 2\n"), None);
        assert_eq!(asks(&mut Vec::new(), &[0xff, b'\n']), None);
        assert_eq!(asks(&mut Vec::new(), &[b'c'; LINE_MAX]), None);
        assert_eq!(asks(&mut Vec::new(), &[b'c'; LINE_MAX - 1]), Some(vec![]));
    }
}
