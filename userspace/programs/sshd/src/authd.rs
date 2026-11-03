// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! What sshd asks authd(8) (ADR 0347): whether a key may sign someone
//! in, and to start their session, which it then waits on for its end.
//! authd may be busy with another's password for a second or two, so the
//! connection waits for its answer without holding the others up.

use std::io;
use std::net::Shutdown;
use std::os::fd::RawFd;
use std::time::Duration;

use accounts::request::{Answer, Request};
use tokio::io::{AsyncBufReadExt, BufReader};
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
