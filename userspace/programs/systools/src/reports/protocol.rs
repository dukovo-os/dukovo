// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! What goes over reportd's socket (ADR 0531): a request, one line of
//! JSON, then the answer, one line of JSON for each item, after which
//! reportd closes the connection.
//!
//! | ask       | with                          | answer                                  |
//! |-----------|-------------------------------|-----------------------------------------|
//! | `crashes` |                               | a [`Crash`] a line, newest first, then [`End`] |
//! | `draft`   | `about`, `answers`, `without_logs` | [`Drafted`]                        |
//! | `redact`  | `text`                        | [`Drafted`]                             |
//! | `send`    | `text`, `title`, `security`, `crash` | [`Sent`]                          |
//! | `queue`   |                               | a [`Waiting`] a line, then [`End`]      |
//! | `retry`   |                               | a [`Sent`] a line, one per report tried, then [`End`] |
//! | `status`  | `number` (all when none)      | a [`Status`] a line, then [`End`]       |
//! | `delete`  | `number`                      | [`Done`]                                |
//! | `forget`  | `crash`                       | [`Done`]                                |
//!
//! A request reportd does not take is answered with a [`Refusal`].
//! reportd knows who asks from the kernel (`SO_PEERCRED`), never from the
//! request. Any user may ask; each sees the crashes of programs that ran
//! as them and the reports they sent, and an administrator sees all
//! crashes, a service's log, and the failed start. reportd drafts and
//! redacts the text as root, and `send` sends the exact bytes the person
//! saw, unchanged: nothing else makes or sends a report.

use serde::{Deserialize, Serialize};

/// The socket reportd answers on.
pub const SOCKET: &str = "/system/live/reports/socket";

/// The longest a report's text may be, in bytes.
pub const TEXT_MAX: usize = 192 * 1024;
/// The longest a request line may be: a text and the rest.
pub const REQUEST_MAX: usize = TEXT_MAX * 2;
/// The longest a title may be, in characters: the start of the first
/// answer, kept on the machine to name the report.
pub const TITLE_MAX: usize = 80;

/// What a report is about.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum About {
    /// A crash reportd keeps, by its ID.
    Crash { id: String },
    /// A service, by its name: its last 200 lines of log go with it.
    Service { name: String },
    /// A command or an app, by its name.
    Program { name: String },
    /// The hardware: the drivers' and firmware's versions go with it.
    Hardware,
    /// Installing or updating Dukovo.
    Updates,
    /// The failed start the failure notice tells of (ADR 0501): only an
    /// administrator's. report(1) sends the notice's lines and what
    /// changed, which every user may read: reportd starts before the
    /// health check writes them, so its view has no place for them.
    Failure {
        #[serde(default)]
        notice: Vec<String>,
    },
    /// Anything else.
    Other,
}

/// The person's answers to the three questions; the last two may be
/// empty.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Answers {
    /// What happened?
    pub happened: String,
    /// What did you expect?
    pub expected: String,
    /// How can it be made to happen again?
    pub again: String,
}

/// A request to reportd: a line of JSON.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Request {
    /// `crashes`, `draft`, `redact`, `send`, `queue`, `retry`, `status`,
    /// `delete` or `forget`.
    pub ask: String,
    /// draft: what the report is about.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub about: Option<About>,
    /// draft: the person's answers.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub answers: Option<Answers>,
    /// draft: leave out the log and the failed start's log lines.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub without_logs: bool,
    /// redact: the text the person edited; send: the exact bytes they
    /// saw, sent as they are.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// send: the title kept with the report on this machine, up to
    /// [`TITLE_MAX`] characters; it is not sent apart from the text.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// send: into the security box, encrypted to the security team's key.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub security: bool,
    /// send: the crash it reports, marked reported once it is sent;
    /// forget: the crash to put away.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub crash: Option<String>,
    /// status and delete: the report's number.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub number: Option<u64>,
}

/// The answer to a delete or a forget that reportd did.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Done {
    pub ok: bool,
}

/// The line after the last item of a list.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct End {
    pub end: bool,
}

/// Why reportd did not take a request.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Refusal {
    pub error: String,
}

/// What kind of crash.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    /// A program a signal ended (ADR 0530).
    Signal,
    /// A service that ended with a panic's status, 101.
    Panic,
    /// The kernel, from a dump saved after a panic (ADR 0483).
    Kernel,
}

/// A crash reportd keeps, as `crashes` lists it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Crash {
    /// Its ID: the time and the process number, `20261113T100211-4012`.
    pub id: String,
    pub kind: Kind,
    /// The program's file name: `nginx`; `kernel` for the kernel.
    pub program: String,
    /// The program's version, where its bundle or the system names one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// The service it ran for, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub service: Option<String>,
    /// The user ID it ran as.
    pub user: u32,
    /// When, in seconds since 1970.
    pub at: u64,
    /// What happened, in one line: "SIGSEGV (address not mapped) in
    /// ngx_http_core_run_phases", "panic at src/main.rs:12:5".
    pub what: String,
    /// The signature's ID, 16 hex digits: the same for the same crash.
    pub signature: String,
    /// How many times a crash with this signature was seen while this one
    /// was kept.
    pub seen: u32,
    /// The report's number, once reported.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reported: Option<u64>,
}

/// A drafted or redacted text.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Drafted {
    /// The text, redacted: the bytes that are sent if the person sends it.
    pub text: String,
    /// How many pieces redaction cut out, each marked in the text.
    pub cuts: u32,
    /// Whether the text has a log section that `without_logs` leaves out.
    pub has_logs: bool,
}

/// What became of a report the person sent.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Sent {
    /// Its ID in the queue until it is sent.
    pub id: String,
    /// The number the server gave it, once sent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub number: Option<u64>,
    /// Why it waits in the queue, if it does: "no network", the server's
    /// words.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub waiting: Option<String>,
}

/// A report that waits in the queue.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Waiting {
    pub id: String,
    pub title: String,
    /// When it was queued, in seconds since 1970.
    pub since: u64,
    /// How many times it was tried, and why the last try failed.
    pub tries: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// When it is tried next, in seconds since 1970.
    pub next: u64,
    pub security: bool,
}

/// Where a sent report stands, as the server says.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Status {
    pub number: u64,
    pub title: String,
    /// When it was sent, in seconds since 1970.
    pub sent: u64,
    /// `received`, `confirmed`, `duplicate`, `fixed` or `wont-fix`; none
    /// when the server could not be asked.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duplicate_of: Option<u64>,
    /// The version with the fix.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fixed_in: Option<String>,
    /// Why it will not be fixed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// Why the server could not be asked.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub security: bool,
}

/// The title kept for a report: the first line of `happened`, cut to
/// [`TITLE_MAX`] characters, the last of them `…` when cut.
pub fn title_of(happened: &str) -> String {
    let line = happened.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or_default();
    if line.chars().count() <= TITLE_MAX {
        return line.to_string();
    }
    let mut cut: String = line.chars().take(TITLE_MAX - 1).collect();
    cut.push('…');
    cut
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_draft_request_names_what_it_is_about_by_kind() {
        let request = Request {
            ask: "draft".into(),
            about: Some(About::Service { name: "web".into() }),
            ..Request::default()
        };
        let line = serde_json::to_string(&request).unwrap();
        assert_eq!(line, r#"{"ask":"draft","about":{"kind":"service","name":"web"}}"#);
        assert_eq!(serde_json::from_str::<Request>(&line).unwrap(), request);
        let failure = About::Failure { notice: vec!["Start 2 failed".into()] };
        let line = serde_json::to_string(&failure).unwrap();
        assert_eq!(line, r#"{"kind":"failure","notice":["Start 2 failed"]}"#);
        let bare: About = serde_json::from_str(r#"{"kind":"failure"}"#).unwrap();
        assert_eq!(bare, About::Failure { notice: Vec::new() });
    }

    #[test]
    fn the_title_is_the_first_line_cut_to_its_most() {
        assert_eq!(title_of("\n  The disk light blinks\nthen stops"), "The disk light blinks");
        let long = "x".repeat(100);
        let title = title_of(&long);
        assert_eq!(title.chars().count(), TITLE_MAX);
        assert!(title.ends_with('…'));
    }
}
