// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! What sshd asks authd(8) (ADRs 0347 and 0348): whether a key may sign
//! someone in, whether a password is right, and to start their session,
//! which it then waits on for its end; and to sign a helper in to a help
//! session (ADR 0540), whose answer brings the help screen's socket and
//! transcript.
//! authd may be busy with another's password for a second or two, so the
//! connection waits for its answer without holding the others up.

use std::io;
use std::net::Shutdown;
use std::os::fd::{AsRawFd, OwnedFd, RawFd};
use std::time::Duration;

use accounts::request::{Answer, Request};
use tokio::io::{AsyncBufReadExt, BufReader, Interest};
use tokio::net::UnixStream;

use crate::sys;

/// How long authd may take to answer.
const ANSWER_TIME: Duration = Duration::from_secs(90);

/// The longest line authd writes, past which it is not read.
const LINE_MAX: u64 = 4096;

/// A connection to authd, its request sent.
pub struct Asked(BufReader<UnixStream>);

impl Asked {
    /// The next line authd writes, without its newline; `None` when it
    /// has closed the connection.
    pub async fn line(&mut self) -> io::Result<Option<String>> {
        let mut line = String::new();
        let read =
            tokio::io::AsyncReadExt::take(&mut self.0, LINE_MAX).read_line(&mut line).await?;
        if read == 0 {
            return Ok(None);
        }
        Ok(Some(line.trim_end_matches('\n').to_string()))
    }
}

/// Tells authd `request`, waiting a moment for its answer, which is not
/// looked at: for what a connection says as it ends, outside its
/// runtime.
pub fn tell(request: &Request) {
    let told = std::os::unix::net::UnixStream::connect(accounts::SOCKET).and_then(|stream| {
        sys::send_with_fds(&stream, &request.encode(), &[])?;
        stream.shutdown(Shutdown::Write)?;
        // Radix's sockets take no SO_RCVTIMEO: the answer is waited for
        // without blocking.
        stream.set_nonblocking(true)?;
        let until = std::time::Instant::now() + TELL_TIME;
        let mut answer = [0u8; 64];
        loop {
            match io::Read::read(&mut &stream, &mut answer) {
                Err(err) if err.kind() == io::ErrorKind::WouldBlock => {
                    if std::time::Instant::now() >= until {
                        return Err(io::Error::new(io::ErrorKind::TimedOut, "no answer"));
                    }
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(err) if err.kind() == io::ErrorKind::Interrupted => {}
                read => return read.map(drop),
            }
        }
    });
    if let Err(err) = told {
        eprintln!("sshd: cannot tell authd at {}: {err}", accounts::SOCKET);
    }
}

/// How long [`tell`] waits for authd.
const TELL_TIME: Duration = Duration::from_secs(5);

/// Sends authd `request`, with the descriptors `fds`: its answer, and
/// the connection, on which a session's end comes later.
pub async fn ask(request: &Request, fds: &[RawFd]) -> Result<(Answer, Asked), String> {
    let unreachable = |err: io::Error| format!("cannot ask authd at {}: {err}", accounts::SOCKET);
    let stream = std::os::unix::net::UnixStream::connect(accounts::SOCKET).map_err(unreachable)?;
    sys::send_with_fds(&stream, &request.encode(), fds).map_err(unreachable)?;
    stream.shutdown(Shutdown::Write).map_err(unreachable)?;
    stream.set_nonblocking(true).map_err(unreachable)?;
    let mut asked = Asked(BufReader::new(UnixStream::from_std(stream).map_err(unreachable)?));
    let line = match tokio::time::timeout(ANSWER_TIME, asked.line()).await {
        Ok(Ok(Some(line))) => line,
        Ok(Ok(None)) => return Err("authd closed the connection without an answer".into()),
        Ok(Err(err)) => return Err(unreachable(err)),
        Err(_) => return Err(format!("authd did not answer in {} seconds", ANSWER_TIME.as_secs())),
    };
    Ok((Answer::decode(&line), asked))
}

/// Sends authd `request` and reads its answer, with the descriptors that
/// come with its first bytes: for `ssh-help`, whose right answer brings
/// the help session's socket and transcript. Nothing more is read.
pub async fn ask_for_fds(request: &Request) -> Result<(Answer, Vec<OwnedFd>), String> {
    let unreachable = |err: io::Error| format!("cannot ask authd at {}: {err}", accounts::SOCKET);
    let stream = std::os::unix::net::UnixStream::connect(accounts::SOCKET).map_err(unreachable)?;
    sys::send_with_fds(&stream, &request.encode(), &[]).map_err(unreachable)?;
    stream.shutdown(Shutdown::Write).map_err(unreachable)?;
    stream.set_nonblocking(true).map_err(unreachable)?;
    let stream = UnixStream::from_std(stream).map_err(unreachable)?;
    let read = async {
        let (mut line, mut fds) = (Vec::new(), Vec::new());
        let mut buffer = [0u8; 512];
        while !line.contains(&b'\n') && (line.len() as u64) < LINE_MAX {
            stream.readable().await?;
            let fd = stream.as_raw_fd();
            match stream.try_io(Interest::READABLE, || sys::receive(fd, &mut buffer, &mut fds)) {
                Ok(0) => break,
                Ok(n) => line.extend_from_slice(&buffer[..n]),
                Err(err)
                    if matches!(
                        err.kind(),
                        io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
                    ) => {}
                Err(err) => return Err(err),
            }
        }
        Ok((line, fds))
    };
    let (line, fds) = match tokio::time::timeout(ANSWER_TIME, read).await {
        Ok(Ok(read)) => read,
        Ok(Err(err)) => return Err(unreachable(err)),
        Err(_) => return Err(format!("authd did not answer in {} seconds", ANSWER_TIME.as_secs())),
    };
    let Some(end) = line.iter().position(|&b| b == b'\n') else {
        return Err("authd closed the connection without an answer".into());
    };
    Ok((Answer::decode(&String::from_utf8_lossy(&line[..end])), fds))
}
