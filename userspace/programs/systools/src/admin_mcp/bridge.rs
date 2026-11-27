// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The bridge of `admin mcp NAME` (ADR 0690): it carries an MCP client's
//! messages from its standard input to the console's socket, and the
//! console's answers back to its standard output, a line each.
//!
//! It is what a client's MCP settings run over SSH, so it runs as the
//! client's owner, who signed in to the machine to run it. It asks authd
//! for a pass for the client ([`connect`]), which authd gives to the
//! owner alone, and gives the pass to the console
//! ([`local`](crate::admind::mcp::local)). The messages it reads only to
//! learn which requests are on their way, and it changes none.
//!
//! It is one thread that waits, with poll(2), for the client's bytes and
//! for the console's. What goes to the console is written without
//! waiting and the rest kept, so that the console's answers are read
//! while it is busy; while more than a few messages are kept, the client
//! is not read. An answer to the client is written to the end: a client
//! that does not read its answers holds only its own bridge.
//!
//! The console ends the connection when it stops or when the pass is
//! taken no longer. The requests then on their way are each answered
//! with error -32603, in words that say to ask again, and nothing is
//! sent a second time: a request that waits for an administrator stands
//! in the console, and `request` finds it. The next message connects
//! again, with a new pass. A pass that ends within a minute is replaced
//! before the next message while nothing is on its way, so that a
//! client that stays for days loses no request to a pass that ends.

use std::collections::VecDeque;
use std::io::{self, Read, Write};
use std::os::fd::{AsFd, BorrowedFd};
use std::os::unix::net::UnixStream;

use sysapi::poll::{self, PollFd};

use crate::admind::mcp::protocol;
use crate::admind::mcp::rpc::{self, Error, Id, Incoming, MESSAGE_MAX, Message, Refused};

/// The longest line the console's answers may have: a batch of the
/// longest answers there are, and room for what is around them.
const ANSWER_LINE_MAX: usize = 8 * 1024 * 1024;
/// The bytes kept for the console past which the client is not read.
const KEPT_MAX: usize = 4 * MESSAGE_MAX;
/// The most requests remembered as on their way: the oldest is
/// forgotten, and is then told nothing if the console is lost.
const FLIGHT_MAX: usize = 256;
/// A pass that ends within this many seconds is replaced before the
/// next message.
const RENEW_BEFORE: i64 = 60;

/// A connection to the console that was greeted.
pub struct Link {
    /// The socket, which does not wait.
    pub stream: UnixStream,
    /// The Unix time the pass ends.
    pub until: i64,
}

/// A line of bytes that came in pieces.
#[derive(Debug, PartialEq, Eq)]
enum Line {
    /// The line, without its end.
    Whole(Vec<u8>),
    /// A line longer than may be, whose bytes were dropped.
    TooLong,
}

/// Bytes that come in pieces, given out a line at a time.
struct Lines {
    held: Vec<u8>,
    limit: usize,
    /// Whether the line being read is longer than `limit` already.
    over: bool,
}

impl Lines {
    fn new(limit: usize) -> Lines {
        Lines { held: Vec::new(), limit, over: false }
    }

    /// Takes in `bytes`, and gives each line that ended in them.
    fn take(&mut self, bytes: &[u8]) -> Vec<Line> {
        let mut lines = Vec::new();
        let mut rest = bytes;
        while let Some(end) = rest.iter().position(|&byte| byte == b'\n') {
            let (part, after) = rest.split_at(end);
            rest = &after[1..];
            if self.over || self.held.len() + part.len() > self.limit {
                lines.push(Line::TooLong);
                self.held.clear();
            } else {
                self.held.extend_from_slice(part);
                lines.push(Line::Whole(std::mem::take(&mut self.held)));
            }
            self.over = false;
        }
        if self.over || self.held.len() + rest.len() > self.limit {
            self.over = true;
            self.held.clear();
        } else {
            self.held.extend_from_slice(rest);
        }
        lines
    }
}

/// The IDs of the requests in the client's `line` that the console
/// answers each by its ID, read as the console reads the line
/// (`server::handle`): a request, or the requests of a batch that is
/// taken. What the console answers with no ID, it answers once and at
/// once, and nothing is to be remembered of it.
fn asked(line: &[u8]) -> Vec<Id> {
    match rpc::parse(line) {
        Ok(Incoming::One(Message::Request { id, .. })) => vec![id],
        Ok(Incoming::One(Message::Notification { .. })) => Vec::new(),
        Ok(Incoming::Batch(messages)) if protocol::batch(&messages, None).is_err() => Vec::new(),
        Ok(Incoming::Batch(messages)) => messages
            .into_iter()
            .filter_map(|message| match message {
                Ok(Message::Request { id, .. }) | Err(Refused { id: Some(id), .. }) => Some(id),
                _ => None,
            })
            .collect(),
        Err(Refused { id, .. }) => id.into_iter().collect(),
    }
}

/// The IDs that the console's `line` answers: one answer's, or a
/// batch's.
fn answered(line: &[u8]) -> Vec<Id> {
    match serde_json::from_slice::<serde_json::Value>(line) {
        Ok(serde_json::Value::Array(answers)) => {
            answers.iter().filter_map(|answer| Id::of(&answer["id"])).collect()
        }
        Ok(answer) => Id::of(&answer["id"]).into_iter().collect(),
        Err(_) => Vec::new(),
    }
}

/// The requests passed on to the console and not yet answered, oldest
/// first.
#[derive(Default)]
struct Flight(VecDeque<Id>);

impl Flight {
    /// Remembers the requests `ids`.
    fn sent(&mut self, ids: Vec<Id>) {
        for id in ids {
            if self.0.len() == FLIGHT_MAX {
                self.0.pop_front();
            }
            self.0.push_back(id);
        }
    }

    /// Forgets the requests that the console's `line` answers.
    fn answered(&mut self, line: &[u8]) {
        for id in answered(line) {
            if let Some(at) = self.0.iter().position(|sent| *sent == id) {
                self.0.remove(at);
            }
        }
    }

    /// The answers that tell the client each request on its way failed,
    /// for `why`. None is on its way after.
    fn failed(&mut self, why: &str) -> Vec<String> {
        self.0.drain(..).map(|id| failure(Some(&id), Error::internal(why))).collect()
    }
}

/// The line that answers the request `id`, or bytes with no request in
/// them, with `error`.
fn failure(id: Option<&Id>, error: Error) -> String {
    rpc::answer(id, Err(error)).to_string()
}

/// Reads what `fd` has into `buffer`: how many bytes, 0 at its end.
fn read_from(fd: BorrowedFd<'_>, buffer: &mut [u8]) -> io::Result<usize> {
    loop {
        match sysapi::fd::read(fd, buffer) {
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            read => return read,
        }
    }
}

/// The bridge between one client and the console.
struct Bridge<'a, W: Write> {
    /// Gets a pass and connects.
    connect: &'a mut dyn FnMut() -> Result<Link, String>,
    /// The Unix time now.
    clock: &'a dyn Fn() -> i64,
    /// Where the client reads.
    out: W,
    link: Option<Link>,
    flight: Flight,
    /// What is still to be written to the console.
    kept: VecDeque<u8>,
    from_console: Lines,
}

impl<W: Write> Bridge<'_, W> {
    /// Writes `line` to the client.
    fn tell(&mut self, line: &[u8]) -> io::Result<()> {
        self.out.write_all(line)?;
        self.out.write_all(b"\n")?;
        self.out.flush()
    }

    /// Lets the console go, for `why`, and tells the client that each
    /// request on its way failed.
    fn lost(&mut self, why: &str) -> io::Result<()> {
        self.link = None;
        self.kept.clear();
        self.from_console = Lines::new(ANSWER_LINE_MAX);
        let why = format!("{why}: nothing is sent again, so ask again");
        for line in self.flight.failed(&why) {
            self.tell(line.as_bytes())?;
        }
        Ok(())
    }

    /// Takes a line of the client's: it goes to the console, connected
    /// first if it is not, or is answered here when it cannot.
    fn carry_line(&mut self, line: Line) -> io::Result<()> {
        let line = match line {
            Line::Whole(line) if line.iter().all(u8::is_ascii_whitespace) => return Ok(()),
            Line::Whole(line) => line,
            Line::TooLong => {
                let why = format!("a message is one line of at most {MESSAGE_MAX} bytes");
                return self.tell(failure(None, Error::invalid(why)).as_bytes());
            }
        };
        let idle = self.flight.0.is_empty() && self.kept.is_empty();
        let ends =
            self.link.as_ref().is_some_and(|link| link.until - (self.clock)() < RENEW_BEFORE);
        if idle && ends {
            self.link = None;
        }
        let ids = asked(&line);
        if self.link.is_none() {
            match (self.connect)() {
                Ok(link) => self.link = Some(link),
                Err(why) => {
                    let why = format!("cannot connect to the console: {why}");
                    for id in &ids {
                        self.tell(failure(Some(id), Error::internal(why.as_str())).as_bytes())?;
                    }
                    return Ok(());
                }
            }
        }
        self.flight.sent(ids);
        self.kept.extend(line);
        self.kept.push_back(b'\n');
        self.write_kept()
    }

    /// Writes what is kept for the console, as much as it takes now.
    fn write_kept(&mut self) -> io::Result<()> {
        while !self.kept.is_empty() {
            let Some(link) = &self.link else { return Ok(()) };
            let (front, _) = self.kept.as_slices();
            match (&link.stream).write(front) {
                Ok(0) => return self.lost("the console closed the connection"),
                Ok(written) => drop(self.kept.drain(..written)),
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => return Ok(()),
                Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                Err(_) => return self.lost("the connection to the console broke"),
            }
        }
        Ok(())
    }

    /// Reads what the console has sent, and passes each answer on.
    fn read_console(&mut self) -> io::Result<()> {
        let mut buffer = [0u8; 16 * 1024];
        loop {
            let Some(link) = &self.link else { return Ok(()) };
            let read = match (&link.stream).read(&mut buffer) {
                Ok(0) => return self.lost("the console closed the connection"),
                Ok(read) => read,
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => return Ok(()),
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(_) => return self.lost("the connection to the console broke"),
            };
            for line in self.from_console.take(&buffer[..read]) {
                let Line::Whole(line) = line else {
                    return self.lost("the console sent an answer that is too long");
                };
                self.flight.answered(&line);
                self.tell(&line)?;
            }
            // Not waited for to its end: nothing after it is to be read.
            if self.from_console.over {
                return self.lost("the console sent an answer that is too long");
            }
        }
    }

    /// Waits until the client's `input` has a message, the console has
    /// an answer, or the console takes what is kept for it. The client is
    /// not read while too much is kept. Returns what each has: the
    /// client, then the console.
    fn wait(&self, input: BorrowedFd<'_>) -> io::Result<(i16, i16)> {
        let reads_client = self.kept.len() < KEPT_MAX;
        let console = self.link.as_ref().map(|link| link.stream.as_fd());
        let writes = if self.kept.is_empty() { 0 } else { libc::POLLOUT };
        let mut fds = [
            PollFd::optional(reads_client.then_some(input), libc::POLLIN),
            PollFd::optional(console, libc::POLLIN | writes),
        ];
        poll::poll(&mut fds, None)?;
        Ok((fds[0].revents(), fds[1].revents()))
    }

    /// Carries messages until the client's `input` ends.
    fn run(&mut self, input: BorrowedFd<'_>) -> io::Result<()> {
        let mut from_client = Lines::new(MESSAGE_MAX);
        let mut buffer = [0u8; 16 * 1024];
        loop {
            let (client, console) = match self.wait(input) {
                Ok(ready) => ready,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) => return Err(error),
            };
            // Its answers first, so that a request that is answered is
            // not told it failed.
            if console & !libc::POLLOUT != 0 {
                self.read_console()?;
            }
            if console & libc::POLLOUT != 0 {
                self.write_kept()?;
            }
            if client != 0 {
                let read = read_from(input, &mut buffer)?;
                if read == 0 {
                    return Ok(());
                }
                for line in from_client.take(&buffer[..read]) {
                    self.carry_line(line)?;
                }
            }
        }
    }
}

/// Carries the messages of the client at `input` to the console and the
/// console's answers to `out`, until `input` ends. `link` is the
/// connection made already, if one was; `connect` gets a pass and
/// connects, for the first message when there is no connection and
/// again after the console is lost; `clock` gives the Unix time.
///
/// # Errors
///
/// When the client cannot be read or written to. The console's failures
/// are answered to the client, and are not the bridge's.
pub fn carry(
    input: BorrowedFd<'_>,
    out: impl Write,
    link: Option<Link>,
    connect: &mut dyn FnMut() -> Result<Link, String>,
    clock: &dyn Fn() -> i64,
) -> io::Result<()> {
    let mut bridge = Bridge {
        connect,
        clock,
        out,
        link,
        flight: Flight::default(),
        kept: VecDeque::new(),
        from_console: Lines::new(ANSWER_LINE_MAX),
    };
    bridge.run(input)
}

#[cfg(test)]
mod tests;
