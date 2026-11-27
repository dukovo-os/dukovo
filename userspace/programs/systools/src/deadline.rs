// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Reads and writes on a Unix socket that end at a time. Radix's Unix
//! sockets take no SO_RCVTIMEO or SO_SNDTIMEO, so the stream is made
//! non-blocking and each read or write that would block waits by poll
//! for what is left of the time: of a time for them all, or of a time
//! for each, as SO_RCVTIMEO and SO_SNDTIMEO would give.

use std::io::{self, Read, Write};
use std::os::fd::AsFd;
use std::os::unix::net::UnixStream;
use std::time::{Duration, Instant};

use sysapi::poll::{self, PollFd};

/// A Unix stream whose reads and writes fail with TimedOut once `until`
/// has passed.
pub struct Deadline<'a> {
    stream: &'a UnixStream,
    until: Instant,
    /// The time each read or write has, when it is not one for them all.
    each: Option<Duration>,
}

impl<'a> Deadline<'a> {
    /// `stream`, made non-blocking, with reads and writes that end `wait`
    /// from now.
    pub fn new(stream: &'a UnixStream, wait: Duration) -> io::Result<Self> {
        stream.set_nonblocking(true)?;
        Ok(Self { stream, until: Instant::now() + wait, each: None })
    }

    /// `stream`, made non-blocking, with reads and writes that each wait
    /// at most `wait`.
    pub fn each(stream: &'a UnixStream, wait: Duration) -> io::Result<Self> {
        stream.set_nonblocking(true)?;
        Ok(Self { stream, until: Instant::now() + wait, each: Some(wait) })
    }

    /// Starts the time of one read or write, when each has its own.
    fn start(&mut self) {
        if let Some(wait) = self.each {
            self.until = Instant::now() + wait;
        }
    }

    /// Waits until `stream` is ready for `events` or the time is up.
    fn wait_for(&self, events: libc::c_short) -> io::Result<()> {
        loop {
            let left = self.until.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Err(io::ErrorKind::TimedOut.into());
            }
            let mut fds = [PollFd::new(self.stream.as_fd(), events)];
            match poll::poll(&mut fds, Some(left)) {
                Ok(0) => {}
                Ok(_) => return Ok(()),
                Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                Err(error) => return Err(error),
            }
        }
    }
}

/// A Unix stream of its own whose reads and writes each wait at most a
/// time, as [`Deadline::each`] gives one it borrows: for a client that
/// keeps its stream for as long as it talks, as a database session does.
pub struct Quiet {
    stream: UnixStream,
    wait: Duration,
}

impl Quiet {
    /// `stream`, made non-blocking, with reads and writes that each wait
    /// at most `wait`.
    ///
    /// # Errors
    ///
    /// When the stream cannot be made non-blocking.
    pub fn new(stream: UnixStream, wait: Duration) -> io::Result<Self> {
        stream.set_nonblocking(true)?;
        Ok(Self { stream, wait })
    }

    /// One read or write's deadline.
    fn one(&self) -> Deadline<'_> {
        Deadline { stream: &self.stream, until: Instant::now() + self.wait, each: None }
    }
}

impl Read for Quiet {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        self.one().read(buffer)
    }
}

impl Write for Quiet {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.one().write(bytes)
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Read for Deadline<'_> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        self.start();
        loop {
            match (&*self.stream).read(buffer) {
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    self.wait_for(libc::POLLIN)?;
                }
                Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                done => return done,
            }
        }
    }
}

impl Write for Deadline<'_> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.start();
        loop {
            match (&*self.stream).write(bytes) {
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    self.wait_for(libc::POLLOUT)?;
                }
                Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                done => return done,
            }
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader};

    #[test]
    fn a_quiet_streams_read_ends_when_nothing_comes_and_its_next_waits_again() {
        let (ours, theirs) = UnixStream::pair().unwrap();
        let mut quiet = Quiet::new(ours, Duration::from_millis(50)).unwrap();
        let mut byte = [0u8; 1];
        let error = quiet.read(&mut byte).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
        (&theirs).write_all(b"x").unwrap();
        assert_eq!(quiet.read(&mut byte).unwrap(), 1);
        assert_eq!(&byte, b"x");
        quiet.write_all(b"y").unwrap();
        let mut back = [0u8; 1];
        (&theirs).read_exact(&mut back).unwrap();
        assert_eq!(&back, b"y");
    }

    #[test]
    fn a_read_ends_at_the_deadline_when_nothing_comes() {
        let (ours, _theirs) = UnixStream::pair().unwrap();
        let started = Instant::now();
        let mut buffer = [0u8; 8];
        let error = Deadline::new(&ours, Duration::from_millis(200))
            .unwrap()
            .read(&mut buffer)
            .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
        assert!(started.elapsed() >= Duration::from_millis(200));
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn lines_that_come_in_time_are_read_whole() {
        let (ours, theirs) = UnixStream::pair().unwrap();
        let sender = std::thread::spawn(move || {
            (&theirs).write_all(b"one\n").unwrap();
            std::thread::sleep(Duration::from_millis(50));
            (&theirs).write_all(b"two\n").unwrap();
        });
        let deadline = Deadline::new(&ours, Duration::from_secs(5)).unwrap();
        let lines: Vec<String> = BufReader::new(deadline).lines().map(Result::unwrap).collect();
        sender.join().unwrap();
        assert_eq!(lines, ["one", "two"]);
    }

    #[test]
    fn each_read_has_its_own_time() {
        let (ours, theirs) = UnixStream::pair().unwrap();
        let sender = std::thread::spawn(move || {
            for line in [b"one\n", b"two\n"] {
                std::thread::sleep(Duration::from_millis(150));
                (&theirs).write_all(line).unwrap();
            }
        });
        // 250 ms in all would end before the second line.
        let deadline = Deadline::each(&ours, Duration::from_millis(250)).unwrap();
        let lines: Vec<String> = BufReader::new(deadline).lines().map(Result::unwrap).collect();
        sender.join().unwrap();
        assert_eq!(lines, ["one", "two"]);
    }

    #[test]
    fn a_write_the_other_end_never_reads_ends_at_the_deadline() {
        let (ours, _theirs) = UnixStream::pair().unwrap();
        let mut deadline = Deadline::new(&ours, Duration::from_millis(200)).unwrap();
        let chunk = vec![0u8; 1 << 16];
        let error = loop {
            if let Err(error) = deadline.write_all(&chunk) {
                break error;
            }
        };
        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
    }
}
