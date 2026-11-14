// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Running one job of a person's (ADR 0478): authd(8) starts it as the
//! person ([`crate::jobs::start`]) with a pipe for its output, timerd
//! reads the pipe to its end, and the job's end comes from authd.
//!
//! The output is read on a thread of its own, so that a job that fills
//! the pipe is never held up, and only its last 64 KiB is kept. A job
//! that leaves a process behind with the pipe open does not hold timerd
//! either: two seconds after the job itself ends, what was read is what
//! the run has.
//!
//! At most [`JOBS_MAX`] jobs run at once, as many as authd runs: one
//! whose time comes while that many run waits for one of them to end.

use std::io::Read;
use std::os::fd::OwnedFd;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use super::state::{OUTPUT_KEPT, Run, tail};
use accounts::request::JOBS_MAX;

use crate::jobs::{self, Job, NotRun};

/// How many jobs run now, and what a job waiting for its turn waits on.
static RUNNING: (Mutex<usize>, Condvar) = (Mutex::new(0), Condvar::new());

/// A job's turn to run, of the [`JOBS_MAX`] at once: given back when it
/// is dropped.
struct Turn;

impl Turn {
    /// Waits until fewer than [`JOBS_MAX`] jobs run, and takes a turn.
    fn take() -> Turn {
        let (count, ended) = &RUNNING;
        let mut running = count.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        while *running >= JOBS_MAX {
            running = ended.wait(running).unwrap_or_else(std::sync::PoisonError::into_inner);
        }
        *running += 1;
        Turn
    }
}

impl Drop for Turn {
    fn drop(&mut self) {
        let (count, ended) = &RUNNING;
        let mut running = count.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        *running -= 1;
        ended.notify_one();
    }
}

/// How long the output has to end once the job has.
const OUTPUT_GRACE: Duration = Duration::from_secs(2);

/// Why authd did not run a job, in words for the log and `crontab
/// --status`.
#[must_use]
pub fn not_run_words(why: &NotRun) -> String {
    match why {
        NotRun::SignedOut => {
            "its owner is signed out and does not keep their jobs running: crontab --keep-running \
             on turns that on"
                .into()
        }
        NotRun::Barred => "an administrator barred its owner's jobs from running while they are \
                           signed out"
            .into(),
        NotRun::Disabled => "its owner's account is disabled".into(),
        NotRun::NoUser => "its owner is no user".into(),
        NotRun::Refused(why) => why.clone(),
    }
}

/// Runs `job`, and gives the record of the run: when it started and
/// ended, how it ended and what it wrote, or why it did not run.
/// `now` gives the time in seconds since 1970.
pub fn execute(job: &Job, now: impl Fn() -> i64) -> (Run, Option<NotRun>) {
    let _turn = Turn::take();
    let start = now();
    let (reader, writer) = match std::io::pipe() {
        Ok(pipe) => pipe,
        Err(err) => return (stopped(start, now(), format!("no pipe for its output: {err}")), None),
    };
    let started = match jobs::start(job, OwnedFd::from(writer)) {
        Ok(Ok(started)) => started,
        Ok(Err(why)) => {
            let run = Run { not_run: Some(not_run_words(&why)), ..Run::default() };
            return (run, Some(why));
        }
        Err(err) => {
            return (stopped(start, now(), format!("authd could not be asked: {err}")), None);
        }
    };
    let output = Arc::new(Mutex::new(Vec::new()));
    let done = Arc::new(AtomicBool::new(false));
    let (kept, ended) = (Arc::clone(&output), Arc::clone(&done));
    thread::spawn(move || read_output(reader, &kept, &ended));
    let status = jobs::wait(started);
    let until = Instant::now() + OUTPUT_GRACE;
    while !done.load(Ordering::Acquire) && Instant::now() < until {
        thread::sleep(Duration::from_millis(20));
    }
    let bytes = output.lock().map_or_else(|_| String::new(), |bytes| tail(&bytes));
    let run = match status {
        Ok(status) => Run {
            start: Some(start),
            end: Some(now()),
            status: Some(status),
            not_run: None,
            output: bytes,
        },
        Err(err) => Run {
            start: Some(start),
            end: Some(now()),
            status: None,
            not_run: Some(format!("its end was not told: {err}")),
            output: bytes,
        },
    };
    (run, None)
}

/// A run that stopped before authd started it, for a reason of timerd's
/// own.
fn stopped(start: i64, end: i64, why: String) -> Run {
    Run {
        start: Some(start),
        end: Some(end),
        status: None,
        not_run: Some(why),
        output: String::new(),
    }
}

/// Reads `reader` to its end into `kept`, which never holds more than
/// twice what a run keeps.
fn read_output(mut reader: impl Read, kept: &Mutex<Vec<u8>>, done: &AtomicBool) {
    let mut chunk = [0u8; 8192];
    while let Ok(n) = reader.read(&mut chunk) {
        if n == 0 {
            break;
        }
        let Ok(mut bytes) = kept.lock() else { break };
        bytes.extend_from_slice(&chunk[..n]);
        if bytes.len() > 2 * OUTPUT_KEPT {
            let drop = bytes.len() - OUTPUT_KEPT;
            bytes.drain(..drop);
        }
    }
    done.store(true, Ordering::Release);
}

/// The command for `/bin/sh -c`: `command`, reading `input` as its
/// standard input when a crontab line has a `%`. The input is a here
/// document of a word that no line of it is.
#[must_use]
pub fn with_input(command: &str, input: Option<&str>) -> String {
    let Some(input) = input else { return command.to_string() };
    let mut word = String::from("DUKOVO_INPUT");
    while input.lines().any(|line| line == word) {
        word.push('_');
    }
    format!("{{ {command}\n}} <<'{word}'\n{input}{word}\n")
}

/// The message that mails a job's `output` to `to`, as cron does: from
/// the job's owner on this machine, with the job's command in the
/// subject. Lines end in CR LF.
#[must_use]
pub fn mail_message(owner: &str, host: &str, command: &str, to: &str, output: &str) -> Vec<u8> {
    let subject: String = command.chars().filter(|c| !c.is_control()).take(60).collect();
    let mut text = format!(
        "From: {owner}@{host}\r\nTo: {to}\r\nSubject: Cron <{owner}@{host}> {subject}\r\n\
         Content-Type: text/plain; charset=utf-8\r\nAuto-Submitted: auto-generated\r\n\r\n"
    );
    for line in output.lines() {
        text.push_str(line);
        text.push_str("\r\n");
    }
    text.into_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_percent_input_becomes_a_here_document() {
        assert_eq!(with_input("cat", None), "cat");
        assert_eq!(
            with_input("cat", Some("a\nb\n")),
            "{ cat\n} <<'DUKOVO_INPUT'\na\nb\nDUKOVO_INPUT\n"
        );
        let tricky = with_input("cat", Some("DUKOVO_INPUT\n"));
        assert!(tricky.contains("<<'DUKOVO_INPUT_'") && tricky.ends_with("DUKOVO_INPUT_\n"));
    }

    #[test]
    fn mail_has_the_owner_the_command_and_the_output() {
        let text =
            String::from_utf8(mail_message("alice", "box", "backup\tnow", "a@x.org", "one\ntwo\n"))
                .unwrap();
        assert!(text.starts_with(
            "From: alice@box\r\nTo: a@x.org\r\nSubject: Cron <alice@box> backupnow\r\n"
        ));
        assert!(text.ends_with("\r\n\r\none\r\ntwo\r\n"));
    }

    #[test]
    fn output_keeps_its_last_bytes() {
        let kept = Mutex::new(Vec::new());
        let done = AtomicBool::new(false);
        let data = vec![b'z'; 3 * OUTPUT_KEPT];
        read_output(&data[..], &kept, &done);
        assert!(done.load(Ordering::Acquire));
        let len = kept.lock().unwrap().len();
        assert!((OUTPUT_KEPT..=2 * OUTPUT_KEPT).contains(&len));
    }

    #[test]
    fn a_job_past_the_most_at_once_waits_for_one_to_end() {
        let turns: Vec<Turn> = (0..JOBS_MAX).map(|_| Turn::take()).collect();
        let started = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&started);
        let waiting = thread::spawn(move || {
            let _turn = Turn::take();
            flag.store(true, Ordering::Release);
        });
        thread::sleep(Duration::from_millis(200));
        assert!(!started.load(Ordering::Acquire));
        drop(turns);
        waiting.join().unwrap();
        assert!(started.load(Ordering::Acquire));
        assert_eq!(*RUNNING.0.lock().unwrap(), 0);
    }

    #[test]
    fn a_job_that_waits_on_its_owner_says_how_to_let_it_run() {
        assert!(not_run_words(&NotRun::SignedOut).contains("crontab --keep-running on"));
        assert!(not_run_words(&NotRun::Barred).contains("administrator"));
    }
}
