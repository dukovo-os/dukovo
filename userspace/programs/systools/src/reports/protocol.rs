// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! What goes over reportd's socket (ADR 0531): a request, one line of
//! JSON, then the answer, one line of JSON for each item, after which
//! reportd closes the connection.
//!
//! | ask       | with                          | answer                                  |
//! |-----------|-------------------------------|-----------------------------------------|
//! | `crashes` |                               | a [`Crash`] a line, newest first, then [`End`] |
//! | `draft`   | `about`, `answers`, `without_logs`, `keep` | [`Drafted`]               |
//! | `redact`  | `text`, or `draft`, `sha256` and `edits` | [`Drafted`]                  |
//! | `send`    | `text` or `draft` and `sha256`; `title`, `security`, `crash` | [`Sent`]  |
//! | `text`    | `draft`, `sha256`             | [`KeptText`]                            |
//! | `queue`   |                               | a [`Waiting`] a line, then [`End`]      |
//! | `retry`   |                               | a [`Sent`] a line, one per report tried, then [`End`] |
//! | `status`  | `number` (all when none)      | a [`Status`] a line, then [`End`]       |
//! | `delete`  | `number`                      | [`Done`]                                |
//! | `forget`  | `crash`                       | [`Done`]                                |
//!
//! A draft asked for with `keep` is kept by reportd for the person who
//! asked (ADR 0568), by the ID and hash its answer gives: `send` sends it
//! as kept, `text` gives it back, and `redact` with a draft applies a list
//! of [`Edits`] to it as reportd drafted it.
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
/// The longest an answer may be, in characters.
pub const ANSWER_MAX: usize = 2000;
/// The most cuts a list of edits may hold, and the most bytes it may be
/// as JSON: a thousand cuts of words and three whole answers fit.
pub const CUTS_MAX: usize = 1000;
pub const EDITS_MAX: usize = 64 * 1024;

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
    /// draft: keep the draft, for `send`, `text` and `redact` to name.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub keep: bool,
    /// send, text and redact: the ID of a draft reportd keeps for the
    /// asker, in place of a text.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub draft: Option<String>,
    /// send, text and redact: the SHA-256 of the draft's text as the
    /// person read it, in hex.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
    /// redact: the person's changes to the draft, the whole list each time.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub edits: Option<Edits>,
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

/// A person's changes to a draft reportd keeps, against the draft as it
/// was drafted: the whole list each time.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Edits {
    /// The cuts of the lines the system gathered.
    #[serde(default)]
    pub cuts: Vec<Cut>,
    /// The person's answers, in place of those drafted; none keeps them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub answers: Option<Answers>,
}

/// A cut: the whole line `line` of `section` (to `through`, when given),
/// or the characters `from` to `to` of it, which become `[cut]`. Lines
/// count from 0 within their section, and characters are Unicode scalar
/// values, from 0, as the text reportd answered has them.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Cut {
    /// `crash`, `failed-start`, `log` or `start-log`.
    pub section: String,
    pub line: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub through: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to: Option<usize>,
    /// Never taken: a line the system gathered is cut, not typed into
    /// (RPT-105). Read only to refuse it with its own words.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
}

/// How many of each mark a text holds: what redaction took out, by kind,
/// and what the person cut.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct CutBy {
    pub user: u32,
    pub host: u32,
    pub ipv4: u32,
    pub ipv6: u32,
    pub mac: u32,
    pub email: u32,
    pub wifi: u32,
    pub serial: u32,
    pub machine: u32,
    /// The lines and the parts of lines the person cut.
    pub you: u32,
}

impl CutBy {
    /// The marks redaction put, of every kind.
    pub fn total(&self) -> u32 {
        self.user
            + self.host
            + self.ipv4
            + self.ipv6
            + self.mac
            + self.email
            + self.wifi
            + self.serial
            + self.machine
    }
}

/// What the person cut of a draft.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct CutByYou {
    /// Whole lines.
    pub lines: u32,
    /// Words of a line, each run counted once.
    pub parts: u32,
}

/// Where a section of a kept draft's text is: its ID (`head`, `happened`,
/// `expected`, `again`, `crash`, `failed-start`, `log`, `start-log`), its
/// title, the line of the text its first line is (from 0, after its
/// heading), and how many lines it has.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Place {
    pub id: String,
    pub title: String,
    pub first_line: usize,
    pub lines: usize,
}

/// A part of a report and its size in bytes, with its heading.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Size {
    pub title: String,
    pub bytes: usize,
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
    /// The marks in the text, by kind.
    #[serde(default)]
    pub cut_by: CutBy,
    /// A kept draft's ID.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub draft: Option<String>,
    /// A kept draft's text's SHA-256, in hex.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
    /// A kept draft's parts and their sizes: what the computer says of
    /// itself, the answers, and each section the system gathered.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub parts: Vec<Size>,
    /// Where each section of a kept draft starts.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sections: Vec<Place>,
    /// Where the answers the person typed are.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub typed: Vec<Place>,
    /// What the person cut of a kept draft.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cut_by_you: Option<CutByYou>,
}

/// A kept draft's text, as `text` gives it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeptText {
    pub text: String,
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
