// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! A person's scheduled jobs, as authd(8) runs them (ADR 0478): timerd
//! asks authd to run a crontab line or an `at` job for its owner, and
//! authd runs it as it starts any session, while they are signed in;
//! while they are not, only when they keep their jobs running and no
//! administrator barred it; root's always.
//!
//! This module is the asking side, for timerd and crontab(1); authd
//! answers in `authd::jobs`.

use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::Shutdown;
use std::os::fd::{AsRawFd, OwnedFd};
use std::os::unix::net::UnixStream;

pub use accounts::request::KeepRunning;
use accounts::request::{self, Answer, Request};

use crate::signin;

/// A job to run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Job {
    /// Whose it is: the user's name.
    pub owner: String,
    /// What the log calls it: `crontab alice` or `at 12`.
    pub label: String,
    /// The folder it starts in: the owner's home for a crontab line, the
    /// folder `at` was run in for an `at` job.
    pub folder: String,
    /// Its environment, `NAME=value` each, beyond what a session has.
    pub environment: Vec<String>,
    /// The command, which `/bin/sh -c` runs.
    pub command: String,
}

/// Why a job did not run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NotRun {
    /// Its owner has no session, and does not keep their jobs running
    /// while signed out.
    SignedOut,
    /// Its owner has no session, and an administrator barred their jobs
    /// from running while they are signed out.
    Barred,
    /// Its owner's account is disabled.
    Disabled,
    /// No such user.
    NoUser,
    /// authd refused it for another reason, in words.
    Refused(String),
}

impl NotRun {
    /// Why authd refused, from its words.
    fn of(why: String) -> NotRun {
        match why.as_str() {
            request::JOB_SIGNED_OUT => NotRun::SignedOut,
            request::JOB_BARRED => NotRun::Barred,
            request::JOB_DISABLED => NotRun::Disabled,
            request::JOB_NO_USER => NotRun::NoUser,
            _ => NotRun::Refused(why),
        }
    }
}

/// A job that started: the connection on which authd says when it
/// ended.
#[derive(Debug)]
pub struct Started {
    /// Where [`wait`] reads its end.
    connection: BufReader<UnixStream>,
}

/// Sends `request` to authd with the descriptors `fds`, and reads its
/// answer's line: the answer, and the connection, open for what authd
/// says after it.
fn ask(request: &Request, fds: &[&OwnedFd]) -> io::Result<(Answer, BufReader<UnixStream>)> {
    let stream = UnixStream::connect(accounts::SOCKET)?;
    let bytes = request.encode();
    if fds.is_empty() {
        (&stream).write_all(&bytes)?;
    } else {
        let fds: Vec<_> = fds.iter().map(|fd| fd.as_raw_fd()).collect();
        signin::send_with_fds(&stream, &bytes, &fds)?;
    }
    stream.shutdown(Shutdown::Write)?;
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    if reader.read_line(&mut line)? == 0 {
        return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "authd did not answer"));
    }
    Ok((Answer::decode(&line), reader))
}

/// Asks authd to run `job`, with `output` as its standard output and
/// standard error, and the null device as its input.
///
/// # Errors
///
/// When authd cannot be asked; a job it does not run is `Ok(Err(..))`.
pub fn start(job: &Job, output: OwnedFd) -> io::Result<Result<Started, NotRun>> {
    let asked = Request::Job {
        owner: job.owner.clone(),
        label: job.label.clone(),
        folder: job.folder.clone(),
        command: job.command.clone(),
        environment: job.environment.clone(),
    };
    let (answer, connection) = ask(&asked, &[&output])?;
    // authd holds its own copy now: the job's, which ends with it.
    drop(output);
    match answer {
        // `ok` and the job's process.
        Answer::Done(process) if process.parse::<u32>().is_ok() => Ok(Ok(Started { connection })),
        Answer::Done(text) => Err(io::Error::other(format!("authd answered `ok {text}`"))),
        Answer::Refused(why) => Ok(Err(NotRun::of(why))),
    }
}

/// Waits for a started job's end: its exit status, or 128 plus the signal
/// that ended it, as a shell says.
///
/// # Errors
///
/// When the connection ends without saying.
pub fn wait(started: Started) -> io::Result<i32> {
    let mut connection = started.connection;
    let mut text = String::new();
    // authd writes one line more, and closes the connection.
    connection.by_ref().take(256).read_to_string(&mut text)?;
    match request::ended_status(&text) {
        Some(status) => Ok(exit_code(status)),
        None if text.is_empty() => {
            Err(io::Error::new(io::ErrorKind::UnexpectedEof, "authd went away before it ended"))
        }
        None => Err(io::Error::other(format!("authd said `{}`", text.trim_end()))),
    }
}

/// The exit status a shell gives for the wait status `status`.
fn exit_code(status: i32) -> i32 {
    if libc::WIFEXITED(status) {
        libc::WEXITSTATUS(status)
    } else if libc::WIFSIGNALED(status) {
        128 + libc::WTERMSIG(status)
    } else {
        1
    }
}

/// What authd's answer to a keep-running request says.
fn setting(request: &Request) -> Result<KeepRunning, String> {
    let (answer, _) = ask(request, &[]).map_err(|err| {
        format!("cannot reach authd at {}: {}", accounts::SOCKET, crate::util::strip_os_error(&err))
    })?;
    match answer {
        Answer::Done(words) => {
            KeepRunning::from_words(&words).ok_or_else(|| format!("authd answered `ok {words}`"))
        }
        Answer::Refused(why) => Err(why),
    }
}

/// The asker's setting.
///
/// # Errors
///
/// When authd cannot be asked, or refuses, in words.
pub fn keep_running() -> Result<KeepRunning, String> {
    setting(&Request::KeepRunning)
}

/// Sets the asker's setting: on, off, or back to the system's default.
///
/// # Errors
///
/// When authd cannot be asked, or refuses, in words.
pub fn set_keep_running(chosen: Option<bool>) -> Result<KeepRunning, String> {
    setting(&Request::SetKeepRunning { chosen })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn authd_s_refusals_say_why_a_job_did_not_run() {
        let of = |why: &str| NotRun::of(why.to_string());
        assert_eq!(of(request::JOB_SIGNED_OUT), NotRun::SignedOut);
        assert_eq!(of(request::JOB_BARRED), NotRun::Barred);
        assert_eq!(of(request::JOB_DISABLED), NotRun::Disabled);
        assert_eq!(of(request::JOB_NO_USER), NotRun::NoUser);
        assert_eq!(of("no slots"), NotRun::Refused("no slots".into()));
    }

    #[test]
    fn a_job_s_end_is_a_shell_s_exit_status() {
        assert_eq!(exit_code(0), 0);
        assert_eq!(exit_code(3 << 8), 3);
        assert_eq!(exit_code(libc::SIGKILL), 128 + libc::SIGKILL);
    }
}
