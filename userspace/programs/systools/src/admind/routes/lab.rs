// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The route that only the lab's build has (ADR 0514): `POST
//! /api/1/test/elevate`, which runs a command as an administrator for an
//! unlocked session, the way a page of the console's will, so that the
//! lab can test the unlock, `console-elevate`'s four descriptors and the
//! hang-up of the command when its relay closes, without a page that runs
//! commands. The shipped admind is built without the `lab` feature and
//! has no such route.
//!
//! The body is `{"command": ["/system/commands/echo", "hello"]}`, or the
//! command as one line of words apart by spaces; the
//! answer is `{"process", "output", "status"}`, the output being the
//! first 64 KiB the command wrote to its output and its error output, and
//! the status the exit code a shell would see. At most 32 commands run at
//! once.

use std::fs::File;
use std::io::pipe;
use std::os::fd::{AsRawFd, OwnedFd};
use std::time::Duration;

use accounts::console::{Console, Elevator};
use accounts::request::{self, Answer};
use serde::Deserialize;
use serde_json::json;
use tokio::io::AsyncReadExt;
use tokio::net::unix::pipe::Receiver;

use super::{Context, Live, Outcome, Reply, Text, refused, shown};
use crate::admind::authd;
use crate::admind::errors::{Attempt, Failure, Kind, refusal};
use crate::admind::shared::Shared;

/// The most commands that run at once.
const ELEVATED_MAX: usize = 32;

/// The most output of a command that is kept.
const OUTPUT_MAX: usize = 64 << 10;

/// How long a command has to write its output and end.
const RUN_TIME: Duration = Duration::from_secs(60);

/// A place among the commands running, given back when it drops.
struct Place<'a>(&'a Shared);

impl<'a> Place<'a> {
    /// A place, if there is one.
    fn take(shared: &'a Shared) -> Option<Place<'a>> {
        let running = shared.elevating.get();
        (running < ELEVATED_MAX).then(|| {
            shared.elevating.set(running + 1);
            Place(shared)
        })
    }
}

impl Drop for Place<'_> {
    fn drop(&mut self) {
        self.0.elevating.set(self.0.elevating.get().saturating_sub(1));
    }
}

/// A command, as the body gives it.
#[derive(Deserialize)]
#[serde(untagged)]
enum Command {
    /// Each word.
    Words(Vec<Text>),
    /// One line, whose words are apart by spaces.
    Line(Text),
}

/// The words of a command, as authd takes them.
fn words(command: &Command) -> Result<Vec<Vec<u8>>, Failure> {
    let words: Vec<Vec<u8>> = match command {
        Command::Words(words) => words.iter().map(|word| word.as_bytes().to_vec()).collect(),
        Command::Line(line) => line.split_whitespace().map(|word| word.as_bytes().to_vec()).collect(),
    };
    if words.is_empty() {
        return Err(Failure::new(Kind::BadRequest, "give a command to run"));
    }
    Ok(words)
}

/// What the kept output holds once `more` has been read: as much of it as
/// fits, so that what comes after the limit is read and dropped.
fn keep(kept: &mut Vec<u8>, more: &[u8]) {
    let room = OUTPUT_MAX.saturating_sub(kept.len());
    kept.extend_from_slice(&more[..more.len().min(room)]);
}

/// The exit code a shell gives for the wait status `status`.
fn exit_code(status: i32) -> i32 {
    if libc::WIFEXITED(status) {
        libc::WEXITSTATUS(status)
    } else if libc::WIFSIGNALED(status) {
        128 + libc::WTERMSIG(status)
    } else {
        1
    }
}

/// Reads what the command writes until it closes its output.
async fn read_output(mut output: Receiver) -> std::io::Result<Vec<u8>> {
    let (mut kept, mut buffer) = (Vec::new(), [0u8; 4096]);
    loop {
        match output.read(&mut buffer).await? {
            0 => return Ok(kept),
            n => keep(&mut kept, &buffer[..n]),
        }
    }
}

/// `POST /api/1/test/elevate`.
pub async fn elevate(cx: &Context<'_>, live: &Live) -> Outcome {
    #[derive(Deserialize)]
    struct Body {
        command: Command,
    }
    let body: Body = cx.json()?;
    let command = words(&body.command)?;
    let _place = Place::take(cx.shared)
        .ok_or_else(|| Failure::wait("too many elevated commands at once", 1))?;
    let internal = |err: std::io::Error| Failure::new(Kind::Internal, format!("no pipe: {err}"));
    // Nothing is typed to the command, so its input ends at once.
    let (input, typed) = pipe().map_err(internal)?;
    drop(typed);
    let (output, written) = pipe().map_err(internal)?;
    // authd takes the last of the four as the relay, whose closing hangs
    // the command up: ours stays open until the command has ended.
    let (relay, _hang_up) = pipe().map_err(internal)?;
    let fds = [
        input.as_raw_fd(),
        written.as_raw_fd(),
        written.as_raw_fd(),
        relay.as_raw_fd(),
    ];
    let console =
        Console::Elevate { by: Elevator::Session(live.authd.clone()), language: Vec::new(), command };
    let what = format!("to run a command for {}", shown(&live.name));
    let (answer, mut asked) = authd::ask(&cx.shared.authd, console, &fds)
        .await
        .map_err(|why| Failure::new(Kind::AuthdSilent, why))?;
    // authd has its own copies now; ours would keep the output open.
    drop((input, written, relay));
    let process = match answer {
        Answer::Done(text) => text,
        Answer::Refused(why) => return Err(refused(cx, &what, refusal(Attempt::Other, &why))),
    };
    let output = Receiver::from_file(File::from(OwnedFd::from(output))).map_err(internal)?;
    let run = async {
        let kept = read_output(output).await?;
        let status = loop {
            match asked.line().await? {
                Some(line) => {
                    if let Some(status) = request::ended_status(&line) {
                        break status;
                    }
                }
                None => break 1,
            }
        };
        Ok::<_, std::io::Error>((kept, status))
    };
    let (kept, status) = match tokio::time::timeout(RUN_TIME, run).await {
        Ok(Ok(done)) => done,
        Ok(Err(err)) => return Err(Failure::new(Kind::Internal, format!("the command's end: {err}"))),
        Err(_) => return Err(Failure::new(Kind::Internal, "the command did not end in a minute")),
    };
    Ok(Reply::json(json!({
        "process": process,
        "output": String::from_utf8_lossy(&kept),
        "status": exit_code(status),
    })))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_command_is_its_words_or_a_line_of_them_and_there_must_be_one() {
        let echo = [b"/system/commands/echo".to_vec(), b"hi".to_vec()];
        let given = |json: &str| words(&serde_json::from_str::<Command>(json).unwrap());
        assert_eq!(given(r#"["/system/commands/echo", "hi"]"#).unwrap(), echo);
        assert_eq!(given(r#""/system/commands/echo  hi ""#).unwrap(), echo);
        assert_eq!(given("[]").unwrap_err().kind, Kind::BadRequest);
        assert_eq!(given(r#""  ""#).unwrap_err().kind, Kind::BadRequest);
        // A word with a NUL would split a field of the request.
        assert!(serde_json::from_str::<Command>(r#"["a\u0000b"]"#).is_err());
    }

    #[test]
    fn only_the_first_64_kib_of_output_is_kept() {
        let mut kept = Vec::new();
        keep(&mut kept, &[b'a'; 40_000]);
        keep(&mut kept, &[b'b'; 40_000]);
        keep(&mut kept, b"more");
        assert_eq!(kept.len(), OUTPUT_MAX);
        assert_eq!(kept[39_999], b'a');
        assert_eq!(kept[40_000], b'b');
    }

    #[test]
    fn the_exit_code_is_a_shells() {
        assert_eq!(exit_code(0), 0);
        assert_eq!(exit_code(3 << 8), 3);
        assert_eq!(exit_code(libc::SIGKILL), 128 + libc::SIGKILL);
    }
}
