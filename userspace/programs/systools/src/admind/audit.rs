// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Who changed a file by hand (ADR 0516), from authd's audit records.
//!
//! authd writes a line for each command it elevates for someone at a
//! terminal: `NAME elevated on pts/1 (password typed): process 412 runs vi
//! /system/settings/admin.toml, for process 398`. logd keeps those
//! security events in the audit log and gives them only to root, over its
//! query socket (logd(8)); admind runs as root and has the socket in its
//! `files.read`. When the journal finds a file changed that no console
//! change explains, the last person whose elevated command named the
//! file's path in the 10 minutes before is who changed it. A change
//! made another way (an editor run as root at the machine's own console,
//! a file copied in, an edit through `service edit`, which names the
//! service and not the path) has no one: it is "outside the console".
//!
//! A file changed while an administrator's terminal of the console ran
//! (ADR 0517) has no elevated command: the shell is root's already. When
//! one account's administrator's pane ran at the time the file changed,
//! that account changed it, in a terminal of the console; when two
//! accounts' did, no one is named.
//!
//! The question to logd is asked on a connection of its own, with a time
//! for the whole answer, so that a logd that is slow or gone costs the
//! round of the journal its answer and not the console its thread.

use std::collections::BTreeMap;
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;

use super::terminal::Rooted;
use crate::logd::{self, Record, Request};

/// How long before a file's change a command that named it counts.
pub const WINDOW: i64 = 600;

/// How long logd has to answer: the whole answer, which is a few lines.
const ASK_TIME: Duration = Duration::from_secs(5);
/// The most lines of the answer that are read: a window of a few minutes
/// holds a few.
const LINES_MAX: usize = 10_000;

/// What the journal says of a change it cannot name a person for.
pub const NOBODY: &str = "outside the console";

/// The journal's note on a change made in an administrator's terminal of
/// the console.
pub const IN_TERMINAL: &str = "in an administrator's terminal of this console";

/// How long after an administrator's pane ended a change still counts as
/// made in it: an editor that is hung up may save as it goes.
const ROOTED_SLACK: i64 = 2;

/// Who changed a file by hand, and whether in an administrator's terminal
/// of the console.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Editor {
    /// The account's name.
    pub name: String,
    /// Whether the change was made in an administrator's terminal of the
    /// console, not with a command elevated at a terminal.
    pub in_terminal: bool,
}

impl Editor {
    /// The journal's note on the change.
    pub fn note(&self) -> Option<&'static str> {
        self.in_terminal.then_some(IN_TERMINAL)
    }
}

/// A command someone elevated at a terminal.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Elevated {
    /// Who, by account name.
    pub who: String,
    /// The command, as authd shows it.
    pub command: String,
    /// The Unix time authd wrote the line.
    pub at: i64,
}

impl Elevated {
    /// The command someone elevated at a terminal that `record` says,
    /// if it says one: a command run for a console session or a token is
    /// the console's own, and in the journal already.
    pub fn of(record: &Record) -> Option<Elevated> {
        let (who, rest) = record.text.split_once(" elevated on ")?;
        if who.is_empty() || who.contains(char::is_whitespace) {
            return None;
        }
        let (_, rest) = rest.split_once("): process ")?;
        let (_, command) = rest.split_once(" runs ")?;
        let command = command.rsplit_once(", for process ").map_or(command, |(command, _)| command);
        let ns = logd::parse_rfc3339(&record.time)?;
        Some(Elevated {
            who: who.to_string(),
            command: command.to_string(),
            at: i64::try_from(ns / 1_000_000_000).ok()?,
        })
    }

    /// Whether the command's words include the path `path`, as an editor
    /// is run on it.
    pub fn names(&self, path: &str) -> bool {
        self.command.split_whitespace().any(|word| word == path)
    }
}

/// Who changed `path` at Unix time `edited`: the person whose elevated
/// command in `commands` named it last, in the [`WINDOW`] before.
pub fn who_edited<'a>(commands: &'a [Elevated], path: &str, edited: i64) -> Option<&'a str> {
    commands
        .iter()
        .filter(|command| command.names(path))
        .filter(|command| command.at <= edited && edited - command.at <= WINDOW)
        .max_by_key(|command| command.at)
        .map(|command| command.who.as_str())
}

/// Who changed each of `edits`, a path and the Unix time it changed at,
/// by the commands in `commands`. A path no one is found for is not in
/// the answer.
pub fn who_edited_each(commands: &[Elevated], edits: &[(String, i64)]) -> BTreeMap<String, Editor> {
    edits
        .iter()
        .filter_map(|(path, edited)| {
            let name = who_edited(commands, path, *edited)?.to_string();
            Some((path.clone(), Editor { name, in_terminal: false }))
        })
        .collect()
}

/// Who changed a file at Unix time `edited` in an administrator's terminal
/// of the console, by the panes `rooted`: the one account whose pane ran
/// then. Two accounts' panes are no one's: the journal does not guess.
pub fn who_in_terminal(rooted: &[Rooted], edited: i64) -> Option<&str> {
    let mut ran = rooted
        .iter()
        .filter(|pane| {
            pane.started <= edited && pane.ended.is_none_or(|ended| edited <= ended + ROOTED_SLACK)
        })
        .map(|pane| pane.account.as_str());
    let first = ran.next()?;
    ran.all(|other| other == first).then_some(first)
}

/// Adds to `who` the editors of `edits`, a path and the Unix time it
/// changed at, that no elevated command names and an administrator's
/// terminal of the console does.
pub fn add_terminal_editors(
    who: &mut BTreeMap<String, Editor>,
    rooted: &[Rooted],
    edits: &[(String, i64)],
) {
    for (path, edited) in edits {
        if who.contains_key(path) {
            continue;
        }
        if let Some(name) = who_in_terminal(rooted, *edited) {
            who.insert(path.clone(), Editor { name: name.to_string(), in_terminal: true });
        }
    }
}

/// The commands elevated at terminals between Unix times `from` and `to`,
/// from the audit log of the logd at `socket`.
///
/// # Errors
///
/// In words, if logd cannot be reached, refuses, or does not answer in
/// time.
pub async fn elevated_between(socket: &str, from: i64, to: i64) -> Result<Vec<Elevated>, String> {
    let ns = |seconds: i64| u64::try_from(seconds).unwrap_or(0).saturating_mul(1_000_000_000);
    let request = Request {
        audit: true,
        services: vec!["authd".into()],
        since: Some(ns(from)),
        until: Some(ns(to)),
        grep: Some("elevated on".into()),
        ..Request::default()
    };
    let ask = async {
        let unreachable = |err: std::io::Error| format!("cannot ask logd at {socket}: {err}");
        let mut stream = UnixStream::connect(socket).await.map_err(unreachable)?;
        let mut line = serde_json::to_string(&request).map_err(|err| err.to_string())?;
        line.push('\n');
        stream.write_all(line.as_bytes()).await.map_err(unreachable)?;
        let mut lines = BufReader::new(stream).lines();
        let mut found = Vec::new();
        for _ in 0..LINES_MAX {
            let Some(line) = lines.next_line().await.map_err(unreachable)? else { break };
            if let Ok(refusal) = serde_json::from_str::<logd::Refusal>(&line) {
                return Err(format!("logd refused: {}", refusal.error));
            }
            if let Some(command) =
                serde_json::from_str::<Record>(&line).ok().as_ref().and_then(Elevated::of)
            {
                found.push(command);
            }
        }
        Ok(found)
    };
    tokio::time::timeout(ASK_TIME, ask)
        .await
        .map_err(|_| format!("logd did not answer in {} seconds", ASK_TIME.as_secs()))?
}

#[cfg(test)]
mod tests;
