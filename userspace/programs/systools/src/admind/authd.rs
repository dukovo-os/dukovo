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

use crate::authd::console::clients::{CLIENTS_MAX, SCOPES_MAX};
use crate::authd::console::{PASSKEYS_MAX, TOKENS_MAX};
use crate::signin;

/// How long authd may take to answer. A wrong proof's wait grows to a
/// minute or so, which this is longer than.
const ANSWER_TIME: Duration = Duration::from_secs(90);

/// The most bytes a passkey takes in the JSON that `steps` answers: its
/// credential ID in hexadecimal (an ID is at most 1023 bytes), its
/// relying party (a DNS name of at most 253 bytes), its name (64
/// characters of at most 4 bytes), and the keys and numbers around them.
const PASSKEY_JSON: usize = 2 * 1023 + 253 + 4 * 64 + 160;

/// The most bytes a token takes there: its name (at most 32), its scopes
/// (at most 16, each an area of 32 characters and `:read`, in quotes and
/// with a comma), and the keys and numbers around them.
const TOKEN_JSON: usize = 32 + 16 * 40 + 160;

/// The longest line authd writes, past which it is not read: the `steps`
/// answer of an account that keeps as many passkeys and tokens as authd
/// lets it, which is longer than the lists of passkey IDs that the
/// answers to a challenge hold, and room for the rest of the JSON.
const LINE_MAX: u64 = (PASSKEYS_MAX * PASSKEY_JSON + TOKENS_MAX * TOKEN_JSON + 1024) as u64;

/// The most bytes an MCP client takes in the JSON that `clients`
/// answers: its owner and its name (at most 32 each), its scopes (each
/// an area of 32 characters and `:read`, in quotes and with a comma),
/// the address it last came from, and the keys and numbers around them.
const CLIENT_JSON: usize = 2 * 32 + SCOPES_MAX * 40 + 46 + 200;

// The `clients` answer of an account that keeps as many clients as authd
// lets it is a line that is read whole.
const _: () = assert!((CLIENTS_MAX * CLIENT_JSON + 1024) as u64 <= LINE_MAX);

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
        // authd ends each line it writes, so one that does not end was cut
        // short by the limit or by authd going.
        if !line.ends_with('\n') {
            let why = if read as u64 >= LINE_MAX {
                format!("authd wrote a line of more than {LINE_MAX} bytes")
            } else {
                "authd closed the connection in the middle of a line".to_string()
            };
            return Err(io::Error::new(io::ErrorKind::InvalidData, why));
        }
        Ok(Some(line.trim_end_matches('\n').to_string()))
    }
}

/// Sends the authd at `socket` `console`, with the descriptors `fds`: its
/// answer, and the connection, on which an elevated command's end comes
/// later.
///
/// # Errors
///
/// In plain words, if authd cannot be reached or does not answer in time.
pub async fn ask(socket: &str, console: Console, fds: &[RawFd]) -> Result<(Answer, Asked), String> {
    ask_within(socket, ANSWER_TIME, console, fds).await
}

/// [`ask`], waiting `limit` for the answer.
async fn ask_within(
    socket: &str,
    limit: Duration,
    console: Console,
    fds: &[RawFd],
) -> Result<(Answer, Asked), String> {
    let unreachable = |err: io::Error| format!("cannot ask authd at {socket}: {err}");
    let stream = std::os::unix::net::UnixStream::connect(socket).map_err(unreachable)?;
    let mut request = Request::Console(console);
    let mut bytes = request.encode();
    let sent = signin::send_with_fds(&stream, &bytes, fds);
    accounts::wipe(&mut bytes);
    request.wipe();
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
    use serde_json::json;
    use std::io::{Read, Write};
    use std::os::fd::AsRawFd;
    use std::os::unix::net::UnixListener;

    /// A socket path of the test's own.
    fn socket(what: &str) -> String {
        let path = std::env::temp_dir().join(format!("admind-authd-{}-{what}", std::process::id()));
        // A socket left by an earlier run of the test would be in the way.
        let _ = std::fs::remove_file(&path);
        path.to_string_lossy().into_owned()
    }

    fn run<F: std::future::Future>(future: F) -> F::Output {
        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        runtime.block_on(future)
    }

    /// An authd that reads one request, answers `lines`, and closes the
    /// connection `hold` later.
    fn fake(path: &str, lines: Vec<String>, hold: Duration) -> std::thread::JoinHandle<Vec<u8>> {
        let listener = UnixListener::bind(path).unwrap();
        std::thread::spawn(move || {
            let (mut client, _) = listener.accept().unwrap();
            let mut request = Vec::new();
            client.read_to_end(&mut request).unwrap();
            for line in lines {
                // A client that refused the answer has gone.
                if client.write_all(line.as_bytes()).is_err() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            std::thread::sleep(hold);
            request
        })
    }

    #[test]
    fn a_request_goes_as_the_console_words_it_and_the_answer_comes_back() {
        let path = socket("answer");
        let lines = vec!["ok done 0a0b\n".to_string(), "ended 0\n".to_string()];
        let authd = fake(&path, lines, Duration::ZERO);
        let console = Console::Begin {
            from: "192.0.2.7".into(),
            name: "chase".into(),
            password: b"secret".to_vec(),
        };
        run(async {
            let (answer, mut asked) =
                ask_within(&path, Duration::from_secs(5), console, &[]).await.unwrap();
            assert_eq!(answer, Answer::Done("done 0a0b".into()));
            assert_eq!(asked.line().await.unwrap().as_deref(), Some("ended 0"));
            assert_eq!(asked.line().await.unwrap(), None);
        });
        assert_eq!(authd.join().unwrap(), b"console\x00begin\x00192.0.2.7\x00chase\x00secret\x00");
    }

    #[test]
    fn a_refusal_is_an_answer_and_a_descriptor_goes_with_the_request() {
        let path = socket("refusal");
        let authd =
            fake(&path, vec!["no the name or the password is wrong\n".into()], Duration::ZERO);
        let (ours, _theirs) = std::os::unix::net::UnixStream::pair().unwrap();
        run(async {
            let console = Console::Lock { session: "3f9a".into() };
            let (answer, _) =
                ask_within(&path, Duration::from_secs(5), console, &[ours.as_raw_fd()])
                    .await
                    .unwrap();
            assert_eq!(answer, Answer::Refused("the name or the password is wrong".into()));
        });
        authd.join().unwrap();
    }

    #[test]
    fn an_authd_that_is_not_there_or_does_not_answer_is_said_in_words() {
        run(async {
            let console = Console::Lock { session: "3f9a".into() };
            let gone = ask_within("/nonexistent/authd", Duration::from_secs(5), console, &[]).await;
            assert!(gone.err().unwrap().starts_with("cannot ask authd at /nonexistent/authd"));
        });
        let path = socket("silent");
        let authd = fake(&path, Vec::new(), Duration::from_millis(300));
        run(async {
            let console = Console::Lock { session: "3f9a".into() };
            let silent = ask_within(&path, Duration::from_millis(50), console, &[]).await;
            let why = silent.err().unwrap();
            assert!(why == "authd did not answer in 0 seconds", "{why}");
        });
        authd.join().unwrap();
    }

    /// What `steps` answers for an account that keeps as many passkeys and
    /// tokens as authd lets it, each with the longest of everything.
    fn fullest_steps() -> String {
        let name = "a".repeat(63);
        let party = format!("{name}.{name}.{name}.{}", "a".repeat(61));
        assert_eq!(party.len(), 253);
        let passkey = json!({
            "id": "ab".repeat(1023),
            "relying-party": party,
            "name": "\u{1f511}".repeat(64),
            "added": i64::MIN,
            "used": i64::MIN,
        });
        let scope = format!("{}:read", "a".repeat(32));
        let token = json!({
            "name": "a".repeat(32),
            "scopes": vec![scope; 16],
            "until": i64::MIN,
            "made": i64::MIN,
            "used": i64::MIN,
            "expired": false,
        });
        json!({
            "app": { "added": i64::MIN },
            "passkeys": vec![passkey; PASSKEYS_MAX],
            "recovery": { "left": 10, "made": i64::MIN },
            "trusted": 16,
            "tokens": vec![token; TOKENS_MAX],
            "needed": true,
        })
        .to_string()
    }

    #[test]
    fn the_longest_answer_authd_gives_fits_the_limit_and_is_read_whole() {
        let steps = fullest_steps();
        // The lists of passkey IDs that a challenge's answer holds are
        // shorter.
        let ids = 64 + 1 + 32 + PASSKEYS_MAX * (2 * 1023 + 1);
        assert!(steps.len() as u64 <= LINE_MAX, "{} bytes", steps.len());
        assert!(steps.len() > 60_000 && (ids as u64) < LINE_MAX, "{} bytes", steps.len());
        let path = socket("steps");
        let authd = fake(&path, vec![format!("ok {steps}\n")], Duration::ZERO);
        run(async {
            let console = Console::Steps;
            let (answer, mut asked) =
                ask_within(&path, Duration::from_secs(5), console, &[]).await.unwrap();
            assert_eq!(answer, Answer::Done(steps));
            assert_eq!(asked.line().await.unwrap(), None);
        });
        authd.join().unwrap();
    }

    #[test]
    fn a_line_that_is_too_long_or_cut_short_is_an_error_and_not_a_shorter_answer() {
        let too_long = format!("ok {}\n", "x".repeat(usize::try_from(LINE_MAX).unwrap()));
        for (what, line, said) in [
            ("long", too_long.as_str(), "more than"),
            ("cut", "ok {\"app\":nu", "in the middle of a line"),
        ] {
            let path = socket(what);
            let authd = fake(&path, vec![line.to_string()], Duration::ZERO);
            run(async {
                let console = Console::Steps;
                let asked = ask_within(&path, Duration::from_secs(5), console, &[]).await;
                let why = asked.err().unwrap();
                assert!(why.contains(said), "{why}");
            });
            authd.join().unwrap();
        }
    }
}
