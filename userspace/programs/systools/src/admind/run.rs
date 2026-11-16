// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Running an action's command, and what the API answers with what it
//! did (ADR 0515).
//!
//! An administrator's change runs through authd's `console-elevate`, for
//! an unlocked session or a token, with four descriptors from admind: its
//! input, which holds the secrets the action takes, if any, and then ends
//! (ADR 0561), or a document more than a pipe holds, which a writer of its
//! own gives while the command runs (ADR 0567, [`Typing`]); its output and
//! its error output, two pipes admind reads
//! apart; and the relay, whose closing hangs the command up, which admind
//! keeps open until the command has ended or ran out of time. Anything
//! else runs in the person's worker (`run`), as the person, whose input is
//! the same.
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
use std::io::{ErrorKind, PipeWriter, Write, pipe};
use std::os::fd::{AsRawFd, OwnedFd};
use std::task::Poll;
use std::time::Duration;

use accounts::console::{Console, Elevator};
use accounts::request::{self, Answer};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::unix::pipe::{Receiver, Sender};
use tokio::task::JoinHandle;

use super::actions::secret::INPUT_MAX;
use super::actions::{Prepared, Secret};
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

/// Runs `argv` as an administrator by `by`'s right, with `input` on its
/// standard input, waiting `limit` for it to end.
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
    input: Option<&Secret>,
    limit: Duration,
) -> Result<Ran, Failure> {
    let _place = Place::take(shared)
        .ok_or_else(|| Failure::wait("too many elevated commands at once", 1))?;
    start(shared, by, argv, input).await?.finish(limit).await
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
    /// The writer of its input, when the input is more than a pipe holds.
    pub typing: Typing,
}

/// Has authd run `argv` as an administrator by `by`'s right, with its four
/// descriptors from admind, and gives what admind holds of it. Its input
/// holds `input`, and then ends. The caller has a [`Place`] for it.
///
/// # Errors
///
/// authd's refusal (a locked session, a token that may not, a command that
/// is not one), ADM-111 when authd is silent.
pub async fn start(
    shared: &Shared,
    by: Elevator,
    argv: &[String],
    input: Option<&Secret>,
) -> Result<Started, Failure> {
    let internal = |err: std::io::Error| Failure::new(Kind::Internal, format!("no pipe: {err}"));
    let (input_end, typed) = pipe().map_err(internal)?;
    let typing = typed_and_ended(typed, input).map_err(internal)?;
    let (output, output_end) = pipe().map_err(internal)?;
    let (errors, errors_end) = pipe().map_err(internal)?;
    // authd takes the last of the four as the relay, whose closing hangs
    // the command up: ours stays open until the command has ended.
    let (relay, relay_end) = pipe().map_err(internal)?;
    let fds =
        [input_end.as_raw_fd(), output_end.as_raw_fd(), errors_end.as_raw_fd(), relay.as_raw_fd()];
    let command = argv.iter().map(|word| word.as_bytes().to_vec()).collect();
    let console = Console::Elevate { by, language: Vec::new(), command };
    let (answer, asked) = authd::ask(&shared.authd, console, &fds)
        .await
        .map_err(|why| Failure::new(Kind::AuthdSilent, why))?;
    // authd has its own copies now; ours would keep the pipes open.
    drop((input_end, output_end, errors_end, relay));
    if let Answer::Refused(why) = answer {
        return Err(refusal(Attempt::Other, &why));
    }
    let output = Receiver::from_file(File::from(OwnedFd::from(output))).map_err(internal)?;
    let errors = Receiver::from_file(File::from(OwnedFd::from(errors))).map_err(internal)?;
    Ok(Started { output, errors, asked, relay: relay_end, typing })
}

/// Writes `input` to `typed`, the write end of a command's new input, and
/// closes it, so that the command reads the input and then its end. Input
/// of at most [`INPUT_MAX`] bytes, which a new pipe holds before any is
/// read, is written now, and the write does not wait for the command. More
/// is written by a writer of its own while the command runs (ADR 0567),
/// from a copy that is zeroed when the writer ends: the [`Typing`] this
/// gives, which the caller ends once the command has ended.
///
/// # Errors
///
/// When the pipe does not take it, or cannot be watched.
pub fn typed_and_ended(mut typed: PipeWriter, input: Option<&Secret>) -> std::io::Result<Typing> {
    match input {
        Some(input) if input.bytes().len() > INPUT_MAX => {
            let typed = Sender::from_file(File::from(OwnedFd::from(typed)))?;
            Ok(Typing(Some(tokio::spawn(written(typed, input.copied())))))
        }
        input => {
            if let Some(input) = input {
                typed.write_all(input.bytes())?;
            }
            drop(typed);
            Ok(Typing(None))
        }
    }
}

/// Writes `input` to `typed` and closes it. A command that ends before it
/// has read it all leaves the pipe with no reader, which is no fault of
/// the action: its status says what it did. `input` is zeroed when this
/// ends, or when it is stopped.
async fn written(mut typed: Sender, input: Secret) {
    if let Err(err) = typed.write_all(input.bytes()).await
        && err.kind() != ErrorKind::BrokenPipe
    {
        super::log(4, &format!("a command's input was not all written: {err}"));
    }
}

/// The writer of a command's input that a pipe does not hold at once (ADR
/// 0567), or nothing when the input was written at once. Its copy of the
/// input is zeroed when the writer ends, is stopped or panics.
#[must_use]
pub struct Typing(Option<JoinHandle<()>>);

impl Typing {
    /// Ends the writer of a command that has ended: what it has not read,
    /// it never will. When this returns, the writer's copy of the input is
    /// gone, so a caller ends it before the action answers.
    pub async fn ended(mut self) {
        if let Some(writer) = self.0.take() {
            writer.abort();
            // Cancelled, or ended before: either way, it has been dropped.
            let _ = writer.await;
        }
    }
}

impl Drop for Typing {
    /// A writer left running, as on a panic of the caller's, is stopped:
    /// its copy goes the next time the runtime runs it.
    fn drop(&mut self) {
        if let Some(writer) = self.0.take() {
            writer.abort();
        }
    }
}

impl Started {
    /// Waits `limit` for the command to end, keeping its output and its
    /// error output as [`read_both`] does.
    ///
    /// # Errors
    ///
    /// ADM-121 when the command does not end in `limit`, and is hung up.
    pub async fn finish(self, limit: Duration) -> Result<Ran, Failure> {
        let Started { output, errors, mut asked, relay: _hang_up, typing } = self;
        let run = async {
            let (output, errors) = read_both(output, errors).await?;
            let status = ended_status(&mut asked).await?;
            Ok::<_, std::io::Error>(Ran {
                status: exit_code(status),
                output: String::from_utf8_lossy(&output).into_owned(),
                errors: String::from_utf8_lossy(&errors).into_owned(),
            })
        };
        let ran = tokio::time::timeout(limit, run).await;
        typing.ended().await;
        match ran {
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

/// Runs `argv` in the worker of `link`, as the person, with `input` on its
/// standard input.
///
/// # Errors
///
/// ADM-112 when the worker is silent or gone, the worker's own fault
/// when it cannot start the command.
pub async fn as_the_person(
    link: &Link,
    argv: &[String],
    input: Option<&Secret>,
) -> Result<Ran, Failure> {
    let mut params = json!({ "argv": argv });
    if let Some(input) = input {
        // The worker writes the text to the command's input. The link
        // carries it as JSON, which must be text: a secret is.
        let text = String::from_utf8_lossy(input.bytes()).into_owned();
        params["input"] = Value::String(text);
    }
    match link.call("run", params).await {
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
        let not_json = || {
            Failure::new(
                Kind::Internal,
                format!("{} printed something that is not JSON", prepared.argv.join(" ")),
            )
        };
        if action.lines() {
            return ran
                .output
                .lines()
                .filter(|line| !line.trim().is_empty())
                .map(serde_json::from_str::<Value>)
                .collect::<Result<Vec<_>, _>>()
                .map(Value::Array)
                .map_err(|_| not_json());
        }
        return serde_json::from_str(&ran.output).map_err(|_| not_json());
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
    use crate::admind::actions::secret::INPUT_LIMIT;
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
        // checks.run's too: it runs the same checks and keeps what they found.
        let run = prepared("checks.run", serde_json::json!({}));
        assert_eq!(run.action.answers(), [0, 1, 2]);
        // diskhealth exits with 2, 4 or 8 added for a disk it cannot read, one
        // that warns and one that is failing, and its answer is the output then too.
        let health = crate::admind::actions::REGISTRY
            .iter()
            .find(|action| action.id() == "storage.health")
            .unwrap();
        assert_eq!(health.answers(), [0, 2, 4, 6, 8, 10, 12, 14]);
        // scrub exits with 3 when it found errors it could not mend: the scrub ran.
        let scrub = crate::admind::actions::REGISTRY
            .iter()
            .find(|action| action.id() == "storage.scrub")
            .unwrap();
        assert_eq!(scrub.answers(), [0, 3]);
        // So does snapshots.list's, 1 being a place that failed and was left
        // out of the list, as it says on standard error.
        let list = crate::admind::actions::REGISTRY
            .iter()
            .find(|action| action.id() == "snapshots.list")
            .unwrap();
        assert_eq!(list.answers(), [0, 1]);
        // hardware last ends with the status of the check it shows, or with 3
        // for none kept; a hardware check that found a failure or a warning
        // ends with 1 or 2, and has said what it found. cert get --check ends
        // with 1 when a check of the names failed, and has printed each.
        // updates.last's 3 is no check kept yet, and updates.advisories' 1 is
        // an advisory that applies and its 3 a list that could not be read;
        // each prints its JSON then too. apps.update's 1, and apps.tonight's,
        // is the stop that says what an update asks for before it is told yes.
        // alerts.test's 1 is a channel that refused the test, which the JSON it
        // printed says.
        for (id, answers) in [
            ("hardware.last", [0, 1, 2, 3].as_slice()),
            ("hardware.check", [0, 1, 2].as_slice()),
            ("certificate.check", [0, 1].as_slice()),
            ("updates.last", [0, 3].as_slice()),
            ("updates.advisories", [0, 1, 3].as_slice()),
            ("apps.update", [0, 1].as_slice()),
            ("apps.tonight", [0, 1].as_slice()),
            ("alerts.test", [0, 1].as_slice()),
        ] {
            let action =
                crate::admind::actions::REGISTRY.iter().find(|action| action.id() == id).unwrap();
            assert_eq!(action.answers(), answers, "{id}");
        }
        // Every other action answers only after 0.
        let named = [
            "service.explain",
            "checks.last",
            "checks.run",
            "storage.health",
            "storage.scrub",
            "snapshots.list",
            "hardware.last",
            "hardware.check",
            "certificate.check",
            "updates.last",
            "updates.advisories",
            "apps.update",
            "apps.tonight",
            "alerts.test",
        ];
        for action in crate::admind::actions::REGISTRY.iter().filter(|a| !named.contains(&a.id())) {
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
    fn input_a_pipe_holds_is_written_at_once_and_more_while_its_reader_reads() {
        use std::io::Read as _;
        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        runtime.block_on(async {
            // A pipe holds it: written and closed before anything reads it.
            let (mut reading, typed) = pipe().unwrap();
            let small = Secret::from("s".repeat(INPUT_MAX));
            let typing = typed_and_ended(typed, Some(&small)).unwrap();
            assert!(typing.0.is_none());
            let mut read = Vec::new();
            reading.read_to_end(&mut read).unwrap();
            assert_eq!(read, small.bytes());
            // More than a pipe holds: written as it is read, and then ended.
            let (mut reading, typed) = pipe().unwrap();
            let large: String = (0..INPUT_LIMIT)
                .map(|n| char::from(b'a' + u8::try_from(n % 26).unwrap()))
                .collect();
            let large = Secret::from(large);
            let typing = typed_and_ended(typed, Some(&large)).unwrap();
            assert!(typing.0.is_some());
            let reader = std::thread::spawn(move || {
                let mut read = Vec::new();
                reading.read_to_end(&mut read).unwrap();
                read
            });
            let read = tokio::task::spawn_blocking(move || reader.join().unwrap()).await.unwrap();
            assert_eq!(read, large.bytes());
            typing.ended().await;
            // No input: the pipe ends at once.
            let (mut reading, typed) = pipe().unwrap();
            typed_and_ended(typed, None).unwrap().ended().await;
            assert_eq!(reading.read(&mut [0u8; 8]).unwrap(), 0);
        });
    }

    #[test]
    fn a_writer_whose_reader_goes_or_never_reads_ends_and_is_no_fault() {
        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        runtime.block_on(async {
            let large = Secret::from("x".repeat(INPUT_LIMIT));
            // The reader goes after a little: the writer meets no reader.
            let (mut reading, typed) = pipe().unwrap();
            let typing = typed_and_ended(typed, Some(&large)).unwrap();
            let gone = std::thread::spawn(move || {
                use std::io::Read as _;
                let _ = reading.read(&mut [0u8; 100]).unwrap();
            });
            tokio::task::spawn_blocking(move || gone.join().unwrap()).await.unwrap();
            tokio::time::timeout(Duration::from_secs(5), typing.ended()).await.unwrap();
            // The reader is there and never reads: the writer is stopped.
            let (reading, typed) = pipe().unwrap();
            let typing = typed_and_ended(typed, Some(&large)).unwrap();
            tokio::task::yield_now().await;
            tokio::time::timeout(Duration::from_secs(5), typing.ended()).await.unwrap();
            drop(reading);
        });
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
