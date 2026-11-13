// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! What admind asks authd(8) (ADR 0514): the console's requests, each on a
//! connection of its own, as `accounts::console` words them. authd may be
//! busy with another's password for a second or two, or hold a wrong
//! proof's answer back as long as its wait for the account lasts, so the
//! connection waits for its answer without holding the others up. The
//! worker asks the same way, from the person's own account, which is how
//! authd tells what it is asked by whom.

use std::io;
use std::net::Shutdown;
use std::os::fd::RawFd;
use std::time::Duration;

use accounts::console::Console;
use accounts::request::{Answer, Request};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};
use tokio::net::UnixStream;

use crate::signin;

/// How long authd may take to answer. A wrong proof's wait grows to a
/// minute or so, which this is longer than.
const ANSWER_TIME: Duration = Duration::from_secs(90);

/// The longest line authd writes, past which it is not read.
const LINE_MAX: u64 = 4096;

/// A connection to authd, its request sent.
pub struct Asked(BufReader<UnixStream>);

impl Asked {
    /// The next line authd writes, without its newline; `None` when it
    /// has closed the connection. An elevated command's `ended STATUS`
    /// comes this way.
    ///
    /// # Errors
    ///
    /// If the socket fails.
    pub async fn line(&mut self) -> io::Result<Option<String>> {
        let mut line = String::new();
        let read = (&mut self.0).take(LINE_MAX).read_line(&mut line).await?;
        if read == 0 {
            return Ok(None);
        }
        Ok(Some(line.trim_end_matches('\n').to_string()))
    }
}

/// Sends authd `console`, with the descriptors `fds`: its answer, and the
/// connection, on which an elevated command's end comes later.
///
/// # Errors
///
/// In plain words, if authd cannot be reached or does not answer in time.
pub async fn ask(console: Console, fds: &[RawFd]) -> Result<(Answer, Asked), String> {
    ask_at(accounts::SOCKET, ANSWER_TIME, console, fds).await
}

/// [`ask`] of the authd at `socket`, waiting `limit` for the answer.
async fn ask_at(
    socket: &str,
    limit: Duration,
    console: Console,
    fds: &[RawFd],
) -> Result<(Answer, Asked), String> {
    let unreachable = |err: io::Error| format!("cannot ask authd at {socket}: {err}");
    let stream = std::os::unix::net::UnixStream::connect(socket).map_err(unreachable)?;
    let mut bytes = Request::Console(console).encode();
    let sent = signin::send_with_fds(&stream, &bytes, fds);
    accounts::wipe(&mut bytes);
    sent.map_err(unreachable)?;
    stream.shutdown(Shutdown::Write).map_err(unreachable)?;
    stream.set_nonblocking(true).map_err(unreachable)?;
    let mut asked = Asked(BufReader::new(UnixStream::from_std(stream).map_err(unreachable)?));
    let line = match tokio::time::timeout(limit, asked.line()).await {
        Ok(Ok(Some(line))) => line,
        Ok(Ok(None)) => return Err("authd closed the connection without an answer".into()),
        Ok(Err(err)) => return Err(unreachable(err)),
        Err(_) => return Err(format!("authd did not answer in {} seconds", limit.as_secs())),
    };
    Ok((Answer::decode(&line), asked))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::os::fd::AsRawFd;
    use std::os::unix::net::UnixListener;

    /// A socket path of the test's own.
    fn socket(what: &str) -> String {
        let path = std::env::temp_dir().join(format!("admind-authd-{}-{what}", std::process::id()));
        let _ = std::fs::remove_file(&path);
        path.to_string_lossy().into_owned()
    }

    fn run<F: std::future::Future>(future: F) -> F::Output {
        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        runtime.block_on(future)
    }

    /// An authd that reads one request, answers `lines`, and closes the
    /// connection `hold` later.
    fn fake(
        path: &str,
        lines: &'static [&'static str],
        hold: Duration,
    ) -> std::thread::JoinHandle<Vec<u8>> {
        let listener = UnixListener::bind(path).unwrap();
        std::thread::spawn(move || {
            let (mut client, _) = listener.accept().unwrap();
            let mut request = Vec::new();
            client.read_to_end(&mut request).unwrap();
            for line in lines {
                client.write_all(line.as_bytes()).unwrap();
                std::thread::sleep(Duration::from_millis(20));
            }
            std::thread::sleep(hold);
            request
        })
    }

    #[test]
    fn a_request_goes_as_the_console_words_it_and_the_answer_comes_back() {
        let path = socket("answer");
        let authd = fake(&path, &["ok done 0a0b\n", "ended 0\n"], Duration::ZERO);
        let console = Console::Begin {
            from: "192.0.2.7".into(),
            name: "chase".into(),
            password: b"secret".to_vec(),
        };
        run(async {
            let (answer, mut asked) =
                ask_at(&path, Duration::from_secs(5), console, &[]).await.unwrap();
            assert_eq!(answer, Answer::Done("done 0a0b".into()));
            assert_eq!(asked.line().await.unwrap().as_deref(), Some("ended 0"));
            assert_eq!(asked.line().await.unwrap(), None);
        });
        assert_eq!(authd.join().unwrap(), b"console\0begin\0192.0.2.7\0chase\0secret\0");
    }

    #[test]
    fn a_refusal_is_an_answer_and_a_descriptor_goes_with_the_request() {
        let path = socket("refusal");
        let authd = fake(&path, &["no the name or the password is wrong\n"], Duration::ZERO);
        let (ours, _theirs) = std::os::unix::net::UnixStream::pair().unwrap();
        run(async {
            let console = Console::Lock { session: "3f9a".into() };
            let (answer, _) =
                ask_at(&path, Duration::from_secs(5), console, &[ours.as_raw_fd()]).await.unwrap();
            assert_eq!(answer, Answer::Refused("the name or the password is wrong".into()));
        });
        authd.join().unwrap();
    }

    #[test]
    fn an_authd_that_is_not_there_or_does_not_answer_is_said_in_words() {
        run(async {
            let console = Console::Lock { session: "3f9a".into() };
            let gone = ask_at("/nonexistent/authd", Duration::from_secs(5), console, &[]).await;
            assert!(gone.err().unwrap().starts_with("cannot ask authd at /nonexistent/authd"));
        });
        let path = socket("silent");
        let authd = fake(&path, &[], Duration::from_millis(300));
        run(async {
            let console = Console::Lock { session: "3f9a".into() };
            let silent = ask_at(&path, Duration::from_millis(50), console, &[]).await;
            let why = silent.err().unwrap();
            assert!(why == "authd did not answer in 0 seconds", "{why}");
        });
        authd.join().unwrap();
    }
}
