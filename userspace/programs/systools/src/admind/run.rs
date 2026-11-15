// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Running an action's command, and what the API answers with what it
//! did (ADR 0515).
//!
//! An administrator's change runs through authd's `console-elevate`, for
//! an unlocked session or a token, with four descriptors from admind: its
//! input, which ends at once (nothing is typed to a command the console
//! runs); its output and its error output, two pipes admind reads apart;
//! and the relay, whose closing hangs the command up, which admind keeps
//! open until the command has ended or ran out of time. Anything else
//! runs in the person's worker (`run`), as the person.
//!
//! A command's output is kept up to 1 MiB, and its error output up to 64
//! KiB; what comes after is read and dropped, so the command never blocks
//! on a full pipe. At most 32 elevated commands run at once, every session
//! and token together.
//!
//! A long action's command runs the same way, as a task
//! ([`tasks`](super::tasks), ADR 0516): [`start`] gives it its descriptors
//! and its place among the 32, and the task reads its output as it comes,
//! without the time limit of a short command.

use std::fs::File;
use std::io::{PipeWriter, pipe};
use std::os::fd::{AsRawFd, OwnedFd};
use std::task::Poll;
use std::time::Duration;

use accounts::console::{Console, Elevator};
use accounts::request::{self, Answer};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::io::AsyncReadExt;
use tokio::net::unix::pipe::Receiver;

use super::actions::Prepared;
use super::authd::{self, Asked};
use super::errors::{Attempt, Failure, Kind, refusal};
use super::link::{CallError, Link};
use super::shared::Shared;

/// The most elevated commands that run at once.
const ELEVATED_MAX: usize = 32;

/// The most of a command's output that is kept, and of its error output.
pub const OUTPUT_MAX: usize = 1 << 20;
pub const ERRORS_MAX: usize = 64 << 10;

/// How long a command that is not a task may run.
pub const RUN_TIME: Duration = Duration::from_secs(30);

/// The most lines of a change's output its answer carries.
const LINES_MAX: usize = 200;

/// What a command did.
#[derive(Debug, Default, Deserialize, PartialEq, Eq)]
pub struct Ran {
    /// The exit code a shell would see: 128 and the signal for one that a
    /// signal ended.
    pub status: i32,
    /// Its output, as far as it was kept.
    pub output: String,
    /// Its error output, as far as it was kept.
    pub errors: String,
}

/// A place among the elevated commands that run at once, given back when
/// it goes.
pub struct Place<'a>(&'a Shared);

impl<'a> Place<'a> {
    /// A place, if one is free.
    pub fn take(shared: &'a Shared) -> Option<Place<'a>> {
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
        self.0.tasks.place_given_back();
    }
}

/// Runs `argv` as an administrator by `by`'s right, waiting `limit` for it
/// to end; `what` is how a refusal's log line names it.
///
/// # Errors
///
/// ADM-105 when 32 run already, authd's refusal (a locked session, a
/// token that may not, a command that is not one), ADM-111 when authd is
/// silent, ADM-121 when the command does not end in `limit`.
pub async fn elevated(
    shared: &Shared,
    by: Elevator,
    argv: &[String],
    limit: Duration,
) -> Result<Ran, Failure> {
    let _place = Place::take(shared)
        .ok_or_else(|| Failure::wait("too many elevated commands at once", 1))?;
    start(shared, by, argv).await?.finish(limit).await
}

/// A command authd has started as an administrator: what admind holds of
/// it until it ends. The caller holds the [`Place`] it runs in.
pub struct Started {
    /// What the command writes to its standard output.
    pub output: Receiver,
    /// What it writes to its error output.
    pub errors: Receiver,
    /// authd's connection, on which the command's end comes.
    pub asked: Asked,
    /// The write end of the relay: a line written to it is a signal for
    /// the command ([`request::signal`]), and closing it hangs the command
    /// up.
    pub relay: PipeWriter,
}

/// Has authd run `argv` as an administrator by `by`'s right, with its four
/// descriptors from admind, and gives what admind holds of it. The caller
/// has a [`Place`] for it.
///
/// # Errors
///
/// authd's refusal (a locked session, a token that may not, a command that
/// is not one), ADM-111 when authd is silent.
pub async fn start(shared: &Shared, by: Elevator, argv: &[String]) -> Result<Started, Failure> {
    let internal = |err: std::io::Error| Failure::new(Kind::Internal, format!("no pipe: {err}"));
    let (input, typed) = pipe().map_err(internal)?;
    drop(typed);
    let (output, output_end) = pipe().map_err(internal)?;
    let (errors, errors_end) = pipe().map_err(internal)?;
    // authd takes the last of the four as the relay, whose closing hangs
    // the command up: ours stays open until the command has ended.
    let (relay, relay_end) = pipe().map_err(internal)?;
    let fds =
        [input.as_raw_fd(), output_end.as_raw_fd(), errors_end.as_raw_fd(), relay.as_raw_fd()];
    let command = argv.iter().map(|word| word.as_bytes().to_vec()).collect();
    let console = Console::Elevate { by, language: Vec::new(), command };
    let (answer, asked) = authd::ask(&shared.authd, console, &fds)
        .await
        .map_err(|why| Failure::new(Kind::AuthdSilent, why))?;
    // authd has its own copies now; ours would keep the pipes open.
    drop((input, output_end, errors_end, relay));
    if let Answer::Refused(why) = answer {
        return Err(refusal(Attempt::Other, &why));
    }
    let output = Receiver::from_file(File::from(OwnedFd::from(output))).map_err(internal)?;
    let errors = Receiver::from_file(File::from(OwnedFd::from(errors))).map_err(internal)?;
    Ok(Started { output, errors, asked, relay: relay_end })
}

impl Started {
    /// Waits `limit` for the command to end, keeping its output and its
    /// error output as [`read_both`] does.
    ///
    /// # Errors
    ///
    /// ADM-121 when the command does not end in `limit`, and is hung up.
    pub async fn finish(self, limit: Duration) -> Result<Ran, Failure> {
        let Started { output, errors, mut asked, relay: _hang_up } = self;
        let run = async {
            let (output, errors) = read_both(output, errors).await?;
            let status = ended_status(&mut asked).await?;
            Ok::<_, std::io::Error>(Ran {
                status: exit_code(status),
                output: String::from_utf8_lossy(&output).into_owned(),
                errors: String::from_utf8_lossy(&errors).into_owned(),
            })
        };
        match tokio::time::timeout(limit, run).await {
            Ok(Ok(ran)) => Ok(ran),
            Ok(Err(err)) => Err(Failure::new(Kind::Internal, format!("the command's end: {err}"))),
            Err(_) => Err(Failure::new(
                Kind::Internal,
                format!("the command did not end in {} seconds, and was hung up", limit.as_secs()),
            )),
        }
    }
}

/// Reads authd's lines until the one that says how the command ended, and
/// gives its wait status; a connection that closes first is a status of
/// 1.
pub async fn ended_status(asked: &mut Asked) -> std::io::Result<i32> {
    loop {
        match asked.line().await? {
            Some(line) => {
                if let Some(status) = request::ended_status(&line) {
                    return Ok(status);
                }
            }
            None => return Ok(1 << 8),
        }
    }
}

/// Runs `argv` in the worker of `link`, as the person.
///
/// # Errors
///
/// ADM-112 when the worker is silent or gone, the worker's own fault
/// when it cannot start the command.
pub async fn as_the_person(link: &Link, argv: &[String]) -> Result<Ran, Failure> {
    match link.call("run", json!({ "argv": argv })).await {
        Ok(value) => serde_json::from_value(value)
            .map_err(|_| Failure::new(Kind::WorkerSilent, "the worker's answer did not read")),
        Err(CallError::Worker(fault)) => {
            Err(Failure::new(Kind::of(&fault.id).unwrap_or(Kind::Internal), fault.detail))
        }
        Err(CallError::Silent | CallError::Gone) => {
            Err(Failure::new(Kind::WorkerSilent, "the session's worker did not answer"))
        }
    }
}

/// The exit code a shell gives for the wait status `status`.
pub fn exit_code(status: i32) -> i32 {
    if libc::WIFEXITED(status) {
        libc::WEXITSTATUS(status)
    } else if libc::WIFSIGNALED(status) {
        128 + libc::WTERMSIG(status)
    } else {
        1
    }
}

/// Reads a command's output and its error output to their ends at once,
/// so that a command that fills one pipe while the other is waited on
/// never stops, keeping [`OUTPUT_MAX`] and [`ERRORS_MAX`] of them.
pub async fn read_both(output: Receiver, errors: Receiver) -> std::io::Result<(Vec<u8>, Vec<u8>)> {
    let mut output = std::pin::pin!(read_up_to(output, OUTPUT_MAX));
    let mut errors = std::pin::pin!(read_up_to(errors, ERRORS_MAX));
    let (mut output_read, mut errors_read) = (None, None);
    std::future::poll_fn(|cx| {
        for (reading, read) in
            [(output.as_mut(), &mut output_read), (errors.as_mut(), &mut errors_read)]
        {
            if read.is_none()
                && let Poll::Ready(done) = reading.poll(cx)
            {
                *read = Some(done?);
            }
        }
        if output_read.is_some() && errors_read.is_some() {
            Poll::Ready(Ok::<_, std::io::Error>(()))
        } else {
            Poll::Pending
        }
    })
    .await?;
    Ok((output_read.unwrap_or_default(), errors_read.unwrap_or_default()))
}

/// Reads `pipe` to its end, keeping its first `max` bytes.
pub async fn read_up_to(mut pipe: Receiver, max: usize) -> std::io::Result<Vec<u8>> {
    let (mut kept, mut buffer) = (Vec::new(), [0u8; 8192]);
    loop {
        match pipe.read(&mut buffer).await? {
            0 => return Ok(kept),
            n => {
                let room = max.saturating_sub(kept.len());
                kept.extend_from_slice(&buffer[..n.min(room)]);
            }
        }
    }
}

/// The API's answer for `prepared`, which ran as `ran`: a read's JSON as
/// its command printed it, a change's `{done, changed, output}` with the
/// `changed` it is given (ADR 0515), or ADM-123 with the command's own
/// words.
///
/// # Errors
///
/// ADM-123, or the command's own error ID when the last line of its error
/// output ends with one its action names, when the command ended with a
/// status its action does not answer with
/// ([`Action::ANSWERS`](crate::admind::actions::Action::ANSWERS));
/// ADM-121 when a read's output is not JSON.
pub fn answer(prepared: &Prepared, ran: &Ran, changed: bool) -> Result<Value, Failure> {
    let action = prepared.action;
    if !action.answers().contains(&ran.status) {
        return Err(failed(prepared, ran));
    }
    if !action.changes() {
        return serde_json::from_str(&ran.output).map_err(|_| {
            Failure::new(
                Kind::Internal,
                format!("{} printed something that is not JSON", prepared.argv.join(" ")),
            )
        });
    }
    let lines: Vec<&str> = ran.output.lines().take(LINES_MAX).collect();
    Ok(json!({ "done": true, "changed": changed, "output": lines }))
}

/// What the journal says of how a command ended, when it was run: its
/// status, or why its end was not seen. ADM-121 and ADM-112 can come after
/// the command was started, which may have changed what it names, so they
/// have a row. `None` when it was not run, because the caller's rights or
/// authd refused it, or authd or the worker could not be asked: nothing
/// changed then.
pub fn ending(ran: &Result<Ran, Failure>) -> Option<(Option<i32>, Option<&str>)> {
    match ran {
        Ok(ran) => Some((Some(ran.status), None)),
        Err(failure) => ending_failure(failure),
    }
}

/// [`ending`] for a command that did not end as [`Ran`] says: the failure
/// of starting it or of waiting for it.
pub fn ending_failure(failure: &Failure) -> Option<(Option<i32>, Option<&str>)> {
    matches!(failure.kind, Kind::Internal | Kind::WorkerSilent)
        .then_some((None, Some(failure.detail.as_str())))
}

/// The failure of a command that ended with a status other than 0.
fn failed(prepared: &Prepared, ran: &Ran) -> Failure {
    command_failed(&prepared.argv, prepared.action.errors(), ran)
}

/// The failure of the command `argv`, which ended as `ran` says with a
/// status other than 0: its last line of error output, and its own error
/// ID when the line ends with one of `named`.
pub fn command_failed(argv: &[String], named: &[&str], ran: &Ran) -> Failure {
    let last = ran.errors.lines().rev().find(|line| !line.trim().is_empty()).unwrap_or_default();
    let detail = if last.is_empty() {
        format!("{} ended with status {}", argv[0], ran.status)
    } else {
        last.trim().to_string()
    };
    let id = error_id(last).filter(|id| named.contains(id));
    Failure::command(id, detail, argv, ran.status, &ran.errors)
}

/// The error ID a line ends with, as `(SVC-104)`: two to five capitals, a
/// hyphen and three digits.
pub fn error_id(line: &str) -> Option<&str> {
    let inside = line.trim_end().strip_suffix(')')?;
    let id = &inside[inside.rfind('(')? + 1..];
    let (letters, digits) = id.split_once('-')?;
    let ok = (2..=5).contains(&letters.len())
        && letters.bytes().all(|b| b.is_ascii_uppercase())
        && digits.len() == 3
        && digits.bytes().all(|b| b.is_ascii_digit());
    ok.then_some(id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::admind::fixture::Console;

    #[test]
    fn an_error_id_is_read_from_the_end_of_a_line() {
        assert_eq!(error_id("nginx could not load blog.conf (SVC-104)"), Some("SVC-104"));
        assert_eq!(error_id("the update did not start (UPD-101)  "), Some("UPD-101"));
        assert_eq!(error_id("(NET-122)"), Some("NET-122"));
        for line in [
            "",
            "no ID here",
            "an ID (SVC-104) in the middle",
            "(svc-104)",
            "(S-104)",
            "(SERVCE-104)",
            "(SVC-10)",
            "(SVC-1044)",
            "(SVC104)",
            "(SVC-1a4)",
            "a parenthesis (or two)",
        ] {
            assert_eq!(error_id(line), None, "{line:?}");
        }
    }

    #[test]
    fn at_most_32_elevated_commands_run_at_once_and_a_place_is_given_back() {
        let console = Console::new("run-places", std::sync::Arc::new(|_, _| Vec::new()));
        let shared = &*console.shared;
        let held: Vec<Place<'_>> =
            (0..ELEVATED_MAX).map(|_| Place::take(shared).unwrap()).collect();
        assert!(Place::take(shared).is_none());
        drop(held);
        assert_eq!(shared.elevating.get(), 0);
        assert!(Place::take(shared).is_some());
        assert_eq!(shared.elevating.get(), 0);
    }

    #[test]
    fn a_read_answers_with_its_output_after_each_status_it_names_and_fails_after_others() {
        let prepared = |id: &str, args: serde_json::Value| {
            let action = crate::admind::actions::REGISTRY
                .iter()
                .find(|action| action.id() == id)
                .unwrap_or_else(|| panic!("no {id}"));
            action.prepare(args).unwrap()
        };
        let ran = |status: i32| Ran {
            status,
            output: r#"{"state": "stopped"}"#.into(),
            errors: "nginx does not run".into(),
        };
        // service explain exits 3 for a service that does not run, and its
        // explanation is the answer then too.
        let explain = prepared("service.explain", serde_json::json!({ "name": "nginx" }));
        assert_eq!(explain.action.answers(), [0, 3]);
        for status in [0, 3] {
            let answered = answer(&explain, &ran(status), false).unwrap();
            assert_eq!(answered, serde_json::json!({ "state": "stopped" }), "status {status}");
        }
        for status in [1, 4] {
            let failed = answer(&explain, &ran(status), false).unwrap_err();
            assert_eq!(failed.kind, Kind::CommandFailed, "status {status}");
        }
        // So does checks.last's, 1 and 2 being a kept result with a failure
        // or a warning.
        let checks = prepared("checks.last", serde_json::json!({}));
        assert_eq!(checks.action.answers(), [0, 1, 2]);
        for status in [1, 2] {
            assert!(answer(&checks, &ran(status), false).is_ok(), "status {status}");
        }
        assert_eq!(answer(&checks, &ran(3), false).unwrap_err().kind, Kind::CommandFailed);
        // Every other action answers only after 0.
        for action in crate::admind::actions::REGISTRY
            .iter()
            .filter(|a| !["service.explain", "checks.last"].contains(&a.id()))
        {
            assert_eq!(action.answers(), [0], "{}", action.id());
        }
    }

    #[test]
    fn exit_codes_are_a_shell_s() {
        assert_eq!(exit_code(0), 0);
        assert_eq!(exit_code(3 << 8), 3);
        assert_eq!(exit_code(libc::SIGTERM), 128 + libc::SIGTERM);
    }

    #[test]
    fn a_pipe_is_read_to_its_end_and_kept_up_to_its_limit() {
        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        runtime.block_on(async {
            let (reading, mut writing) = pipe().unwrap();
            let writer = std::thread::spawn(move || {
                use std::io::Write as _;
                writing.write_all(&vec![7u8; 100_000]).unwrap();
            });
            let pipe = Receiver::from_file(File::from(OwnedFd::from(reading))).unwrap();
            let kept = read_up_to(pipe, 1000).await.unwrap();
            writer.join().unwrap();
            assert_eq!(kept, vec![7u8; 1000]);
        });
    }
}
