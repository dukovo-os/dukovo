// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! What crontab(1), at(1) and service(8) say to timerd(8) on its socket,
//! `/system/live/timerd/socket` (ADR 0478): a connection sends one
//! request as a line of JSON and reads one line of JSON back, after
//! which timerd closes it. Who asks is the user the kernel gives for the
//! connection; a request names no one but the user whose crontab it is
//! about, which only root may name.

use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixStream;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::deadline::Deadline;

/// timerd's socket.
pub const SOCKET: &str = "/system/live/timerd/socket";

/// The most bytes a request is.
pub const REQUEST_MAX: usize = 256 * 1024;

/// The most bytes an answer is.
pub const REPLY_MAX: usize = 8 * 1024 * 1024;

/// The time a request has to arrive and an answer to be read.
pub const TIME: Duration = Duration::from_secs(10);

/// A request.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "request", rename_all = "kebab-case")]
pub enum Request {
    /// The timers of the service files, with their last and next runs.
    Timers,
    /// Whether `text` is a crontab.
    CrontabCheck { text: String },
    /// Keeps `text` as the crontab of `user`, or of the asker.
    CrontabInstall { user: Option<String>, text: String },
    /// The text of the crontab of `user`, or of the asker.
    CrontabShow { user: Option<String> },
    /// Removes the crontab of `user`, or of the asker.
    CrontabRemove { user: Option<String> },
    /// Each line of the crontab of `user`, or of the asker, with its
    /// next and last run.
    CrontabStatus { user: Option<String> },
    /// Keeps a job to run at `time`, seconds since 1970.
    AtAdd { time: i64, folder: String, environment: Vec<String>, command: String },
    /// The asker's `at` jobs; root's are all of them.
    AtList,
    /// Removes `at` job `id`.
    AtRemove { id: u32 },
    /// The script of `at` job `id`.
    AtShow { id: u32 },
}

/// An answer.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "reply", rename_all = "kebab-case")]
pub enum Reply {
    /// The request failed, in words.
    Error { message: String },
    /// It was done.
    Done,
    /// The service timers.
    Timers { timers: Vec<TimerRow> },
    /// A text: a crontab, or an `at` job's script.
    Text { text: String },
    /// The lines of a crontab.
    Status { lines: Vec<LineRow> },
    /// A job `at` kept.
    Added { id: u32, time: i64 },
    /// The `at` jobs.
    Jobs { jobs: Vec<JobRow> },
}

/// A service's timer.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TimerRow {
    pub name: String,
    /// When Spark last started it for the timer, seconds since 1970.
    pub last: Option<i64>,
    /// When it next starts; none when no time comes within five years.
    pub next: Option<i64>,
}

/// A line of a crontab, and how its runs went.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LineRow {
    pub line: usize,
    pub time: String,
    pub command: String,
    pub next: Option<i64>,
    pub last_start: Option<i64>,
    pub last_end: Option<i64>,
    pub status: Option<i32>,
    /// Why it did not run last, when authd did not run it.
    pub not_run: Option<String>,
    /// The tail of its last output.
    pub output: String,
}

/// An `at` job.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct JobRow {
    pub id: u32,
    pub owner: String,
    pub time: i64,
    /// Its first line of commands.
    pub command: String,
}

/// Sends `request` to timerd and reads its answer.
///
/// # Errors
///
/// When timerd cannot be reached or answers badly, in words; a request
/// it refuses is `Ok(Reply::Error { .. })`.
pub fn ask(request: &Request) -> Result<Reply, String> {
    let stream = UnixStream::connect(SOCKET).map_err(|err| {
        format!(
            "timerd cannot be reached ({}): is its service running?",
            crate::util::strip_os_error(&err)
        )
    })?;
    let mut line = serde_json::to_string(request).map_err(|err| err.to_string())?;
    line.push('\n');
    if line.len() > REQUEST_MAX {
        return Err(format!(
            "that is more than the {} KiB timerd takes at once",
            REQUEST_MAX / 1024
        ));
    }
    let slow = |err: std::io::Error| {
        format!("timerd did not answer: {}", crate::util::strip_os_error(&err))
    };
    Deadline::new(&stream, TIME)
        .and_then(|mut out| out.write_all(line.as_bytes()))
        .map_err(slow)?;
    let input = Deadline::new(&stream, TIME).map_err(slow)?;
    let mut text = Vec::new();
    BufReader::new(input).take(REPLY_MAX as u64).read_until(b'\n', &mut text).map_err(slow)?;
    serde_json::from_slice(&text)
        .map_err(|_| "timerd gave an answer that does not read".to_string())
}

/// `reply`'s error as text, or the reply.
///
/// # Errors
///
/// What the reply or the attempt to ask says.
pub fn answer(request: &Request) -> Result<Reply, String> {
    match ask(request)? {
        Reply::Error { message } => Err(message),
        other => Ok(other),
    }
}
