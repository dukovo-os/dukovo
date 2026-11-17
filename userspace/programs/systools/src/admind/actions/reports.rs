// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The Report a problem page's actions (ADR 0568): the crashes a person
//! may report, a draft of a report that reportd keeps, its cuts, its send,
//! and the reports that wait or were sent. The area is `reports`, the scope
//! a token needs.
//!
//! Every action runs as the person, as `report` does at a terminal:
//! reportd knows who asks from the kernel, and shows each person their own
//! crashes, drafts and reports. A report is the person's, so no action
//! here has an Ansible task of its own kind: they are the commands the
//! console shows.
//!
//! `reports.draft` and `reports.redact` print the text of a report, which
//! is private to the person who asked: each is a `POST` that answers a
//! secret ([`Action::SECRET_ANSWER`]), never a task and never kept. The
//! page sends edits, not text, and `reports.redact` takes them as a
//! document its command reads on its input; what is sent is still what
//! reportd drafted, as the person read it (`reports.send` names the draft
//! and the SHA-256 of its text).
//!
//! `reports.send` and `reports.retry` have no probe: a send always does
//! something, and the one command that lists the reports asks the reports
//! server where each stands, which would make each of them wait for it
//! twice.

use serde::{Deserialize, Serialize};
use serde_json::json;

use super::trial::refusal;
use super::{Action, Args, Bad, Examples, Field, Nothing, Runs, Secret, Tests};

/// The area of these actions: the scope a token needs, `reports`.
const AREA: &str = "reports";

/// The tests of these actions, which `lab/actions.toml` names.
const TESTS: Tests = Tests {
    checks: "admind::actions::reports::tests",
    route: "admin-reports",
    same_as_command: "admin-reports",
    browser: "web-console/report",
    ansible: "web-console/ansible",
};

/// What a report may be about, as `report draft --about` takes it. The page
/// says `update`, and the command's own word is `updates`: both are taken.
const ABOUT: &[&str] =
    &["crash", "service", "program", "failure", "hardware", "update", "updates", "other"];

/// The words of [`ABOUT`] a refusal names.
const ABOUT_WORDS: &str = "crash, service, program, failure, hardware, update, updates or other";

/// What a crash's ID is made of, as reportd reads one: digits, `T` and
/// `-`, so that it names a file in reportd's folder and nothing outside it.
const CRASH_PATTERN: &str = "^[0-9T-]+$";

/// The most characters of a crash's ID.
const CRASH_MAX: usize = 40;

/// What a service's or a program's name is made of.
const NAME_PATTERN: &str = "^[A-Za-z0-9_][A-Za-z0-9_.+-]*$";

/// The most characters of a service's or a program's name.
const NAME_MAX: usize = 64;

/// The most characters of an answer, which reportd takes.
const ANSWER_MAX: usize = 2000;

/// The most characters of a title, which reportd cuts to its own.
const TITLE_MAX: usize = 120;

/// What free text is made of: anything but a NUL, which no word of a
/// command can hold. Line breaks stay.
const TEXT_PATTERN: &str = "^[^\0]*$";

/// The most bytes of the edits, which reportd takes as they are: the
/// document field's own most.
const EDITS_MAX: usize = 65_536;

/// What a draft's ID is: 32 digits of hex.
const DRAFT_PATTERN: &str = "^[0-9a-f]{32}$";

/// What the SHA-256 of a draft's text is: 64 digits of hex.
const SHA256_PATTERN: &str = "^[0-9a-f]{64}$";

/// The most a report's number can be: what reportd keeps in 31 bits.
const NUMBER_MAX: i64 = 2_147_483_648;

/// Whether `text` is `length` digits of lower-case hex.
fn is_hex(text: &str, length: usize) -> bool {
    text.len() == length && text.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// Whether `id` is a crash's ID, as reportd reads one.
fn is_crash_id(id: &str) -> bool {
    (1..=CRASH_MAX).contains(&id.len())
        && id.bytes().all(|b| b.is_ascii_digit() || b == b'T' || b == b'-')
}

/// Whether `name` is a service's or a program's name.
fn is_name(name: &str) -> bool {
    let mut letters = name.bytes();
    (1..=NAME_MAX).contains(&name.len())
        && letters.next().is_some_and(|b| b.is_ascii_alphanumeric() || b == b'_')
        && letters.all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'+' | b'-'))
}

/// Why `text`, up to `max` characters and with no NUL, is not what a free
/// text field takes, in words for `field`.
fn check_text(field: &'static str, text: &str, max: usize) -> Result<(), Bad> {
    if text.chars().count() > max || text.contains('\0') {
        let why = format!("It is text of up to {max} characters, with no NUL");
        return Err(Bad { field, why: refusal(&why) });
    }
    Ok(())
}

/// Checks `crash`, a crash's ID.
fn check_crash(crash: &str) -> Result<(), Bad> {
    if is_crash_id(crash) {
        return Ok(());
    }
    let why = "A crash's ID is its time and its process number, as 20261113T100211-4012 \
               (report crashes lists them)";
    Err(Bad { field: "crash", why: refusal(why) })
}

/// Checks `draft`, a kept draft's ID.
fn check_draft(draft: &str) -> Result<(), Bad> {
    if is_hex(draft, 32) {
        return Ok(());
    }
    Err(Bad { field: "draft", why: refusal("A draft's ID is 32 lower-case hex digits") })
}

/// Checks `sha256`, the hash of a kept draft's text.
fn check_sha256(sha256: &str) -> Result<(), Bad> {
    if is_hex(sha256, 64) {
        return Ok(());
    }
    Err(Bad {
        field: "sha256",
        why: refusal("The SHA-256 of the draft's text is 64 lower-case hex digits"),
    })
}

/// `report crashes --json`: the crashes that reportd keeps for the person,
/// newest first: each person's own, and an administrator's all.
pub struct Crashes;

impl Action for Crashes {
    const ID: &'static str = "reports.crashes";
    const AREA: &'static str = AREA;
    const CHANGES: bool = false;
    const PATH: &'static str = "/api/1/reports/crashes";
    const RUNS: Runs = Runs::AsThePerson;
    const TESTS: Tests = TESTS;
    const TITLE: &'static str = "List the crashes that can be reported";
    type Args = Nothing;

    fn argv(_: &Nothing) -> Vec<String> {
        ["report", "crashes", "--json"].map(String::from).to_vec()
    }

    fn examples() -> Examples<Nothing> {
        reads()
    }
}

/// What the reads are tried with: nothing, and any argument is refused.
fn reads() -> Examples<Nothing> {
    Examples { good: vec![Nothing {}], bad: vec![json!({ "all": true })], undo: None }
}

/// What a report is about, and what the person said of it.
#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Drafting {
    /// What it is about: one of [`ABOUT`].
    pub about: String,
    /// The crash's ID, for a report about a crash.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub crash: Option<String>,
    /// The service's or the program's name, for a report about one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// What happened: the first of the three questions.
    pub happened: String,
    /// What was expected.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected: Option<String>,
    /// How it can be made to happen again.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub again: Option<String>,
    /// Whether to leave the logs out.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub without_logs: bool,
}

impl Args for Drafting {
    const FIELDS: &'static [Field] = &[
        Field::one_of(
            "about",
            "What the report is about: crash, service, program, failure, hardware, update or other.",
            ABOUT,
        ),
        Field::text(
            "crash",
            "The crash to report, by its ID, for a report about a crash.",
            1,
            CRASH_MAX,
            Some(CRASH_PATTERN),
        )
        .optional(),
        Field::text(
            "name",
            "The service or program to report, for a report about one.",
            1,
            NAME_MAX,
            Some(NAME_PATTERN),
        )
        .optional(),
        Field::text(
            "happened",
            "What happened, in the person's words. Line breaks are kept.",
            1,
            ANSWER_MAX,
            Some(TEXT_PATTERN),
        ),
        Field::text(
            "expected",
            "What was expected to happen. Line breaks are kept.",
            0,
            ANSWER_MAX,
            Some(TEXT_PATTERN),
        )
        .optional(),
        Field::text(
            "again",
            "How to make it happen again. Line breaks are kept.",
            0,
            ANSWER_MAX,
            Some(TEXT_PATTERN),
        )
        .optional(),
        Field::flag("without_logs", "Whether to leave the logs out of the report.").optional(),
    ];

    fn check(&self) -> Result<(), Bad> {
        if !ABOUT.contains(&self.about.as_str()) {
            let why = format!("A report is about {ABOUT_WORDS}");
            return Err(Bad { field: "about", why: refusal(&why) });
        }
        let crash = self.about == "crash";
        match (&self.crash, crash) {
            (Some(id), true) => check_crash(id)?,
            (None, true) => {
                let why = "A report about a crash names the crash by its ID";
                return Err(Bad { field: "crash", why: refusal(why) });
            }
            (Some(_), false) => {
                let why = "A crash's ID goes with a report about a crash";
                return Err(Bad { field: "crash", why: refusal(why) });
            }
            (None, false) => {}
        }
        let named = matches!(self.about.as_str(), "service" | "program");
        match (&self.name, named) {
            (Some(name), true) if !is_name(name) => {
                let why = "A name is 1 to 64 letters, digits, and _ . + -, and starts with a \
                           letter, a digit or _";
                return Err(Bad { field: "name", why: refusal(why) });
            }
            (Some(_), true) | (None, false) => {}
            (None, true) => {
                let why = "A report about a service or a program names it";
                return Err(Bad { field: "name", why: refusal(why) });
            }
            (Some(_), false) => {
                let why = "A name goes with a report about a service or a program";
                return Err(Bad { field: "name", why: refusal(why) });
            }
        }
        check_text("happened", &self.happened, ANSWER_MAX)?;
        if self.happened.trim().is_empty() {
            return Err(Bad { field: "happened", why: refusal("Say what happened") });
        }
        check_text("expected", self.expected.as_deref().unwrap_or_default(), ANSWER_MAX)?;
        check_text("again", self.again.as_deref().unwrap_or_default(), ANSWER_MAX)
    }
}

/// `report draft --about KIND [--crash ID | --name NAME] --happened TEXT
/// [--expected TEXT] [--again TEXT] [--without-logs] --json`: drafts the
/// report with reportd, redacted, and keeps the draft for the person; the
/// answer is the text, how many marks it has, and the draft's ID and hash.
/// A read whose answer is the person's report, which is kept nowhere.
pub struct Draft;

impl Action for Draft {
    const ID: &'static str = "reports.draft";
    const AREA: &'static str = AREA;
    const CHANGES: bool = false;
    const PATH: &'static str = "/api/1/reports/drafts";
    const RUNS: Runs = Runs::AsThePerson;
    const SECRET_ANSWER: bool = true;
    const TESTS: Tests = TESTS;
    const TITLE: &'static str = "Draft a report";
    type Args = Drafting;

    fn argv(args: &Drafting) -> Vec<String> {
        let mut argv =
            ["report", "draft", "--about", args.about.as_str()].map(String::from).to_vec();
        for (option, value) in [("--crash", &args.crash), ("--name", &args.name)] {
            if let Some(value) = value {
                argv.extend([option.to_string(), value.clone()]);
            }
        }
        argv.extend(["--happened".to_string(), args.happened.clone()]);
        for (option, value) in [("--expected", &args.expected), ("--again", &args.again)] {
            if let Some(value) = value.as_ref().filter(|value| !value.is_empty()) {
                argv.extend([option.to_string(), value.clone()]);
            }
        }
        if args.without_logs {
            argv.push("--without-logs".into());
        }
        argv.push("--json".into());
        argv
    }

    fn examples() -> Examples<Drafting> {
        let about = |about: &str| Drafting {
            about: about.into(),
            happened: "The disk light stays on".into(),
            ..Drafting::default()
        };
        Examples {
            good: vec![
                Drafting { happened: "lab".into(), ..about("other") },
                Drafting {
                    expected: Some("It goes out\nafter a minute".into()),
                    again: Some("Copy a large file".into()),
                    without_logs: true,
                    ..about("hardware")
                },
                about("update"),
                about("updates"),
                Drafting { name: Some("nginx".into()), ..about("program") },
                Drafting { crash: Some("20261113T100211-4012".into()), ..about("crash") },
                Drafting { name: Some("diskhealth".into()), ..about("service") },
                about("failure"),
            ],
            bad: vec![
                json!({ "about": "crash" }),
                json!({ "about": "other" }),
                json!({ "about": "computer", "happened": "lab" }),
                json!({ "about": "other", "happened": "" }),
                json!({ "about": "other", "happened": "lab", "all": true }),
                json!({ "about": "crash", "happened": "lab" }),
                json!({ "about": "service", "happened": "lab" }),
                json!({ "about": "other", "crash": "20261113T100211-4012", "happened": "lab" }),
                json!({ "about": "other", "happened": "lab", "name": "nginx" }),
                json!({ "about": "program", "happened": "lab", "name": "../x" }),
                json!({ "about": "crash", "crash": "../x", "happened": "lab" }),
            ],
            undo: None,
        }
    }
}

/// The edits to a kept draft, and which draft.
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Redaction {
    /// The kept draft's ID, which the address gives.
    pub draft: String,
    /// The SHA-256 of the text the person read, which reportd holds the
    /// draft to.
    pub sha256: String,
    /// The edits, as a JSON document: `{"cuts": [...]}`, and the answers if
    /// they change. reportd reads it, as the command does.
    pub edits: String,
}

impl Args for Redaction {
    const FIELDS: &'static [Field] = &[
        Field::text(
            "draft",
            "The kept draft's ID, as reports.draft gave it.",
            32,
            32,
            Some(DRAFT_PATTERN),
        ),
        Field::text(
            "sha256",
            "The SHA-256 of the text the person read, as reports.draft gave it.",
            64,
            64,
            Some(SHA256_PATTERN),
        ),
        Field::document(
            "edits",
            "The cuts to make and the answers to change, as JSON: {\"cuts\": [...]}. The whole \
             list each time; none cut is {\"cuts\": []}.",
            EDITS_MAX,
        ),
    ];

    fn check(&self) -> Result<(), Bad> {
        check_draft(&self.draft)?;
        check_sha256(&self.sha256)
    }
}

/// `report redact DRAFT --sha256 HEX --json`, with the edits on its input:
/// applies the person's cuts and changed answers to the kept draft, as
/// reportd redacts the text, and answers what the draft is now. reportd
/// reads the edits and refuses those that cut what cannot be cut (RPT-104,
/// RPT-105). A read whose answer is the person's report.
pub struct Redact;

impl Action for Redact {
    const ID: &'static str = "reports.redact";
    const AREA: &'static str = AREA;
    const CHANGES: bool = false;
    const PATH: &'static str = "/api/1/reports/drafts/{draft}/redact";
    const RUNS: Runs = Runs::AsThePerson;
    const ERRORS: &'static [&'static str] = &["RPT-101", "RPT-102", "RPT-104", "RPT-105"];
    const SECRET_ANSWER: bool = true;
    /// The edits: a document, which a pipe may not hold at once.
    const INPUT_MAX: usize = EDITS_MAX;
    const TESTS: Tests = TESTS;
    const TITLE: &'static str = "Cut lines out of a draft";
    type Args = Redaction;

    fn argv(args: &Redaction) -> Vec<String> {
        ["report", "redact", args.draft.as_str(), "--sha256", args.sha256.as_str(), "--json"]
            .map(String::from)
            .to_vec()
    }

    fn input(args: &Redaction) -> Option<Secret> {
        Some(Secret::from(args.edits.clone()))
    }

    fn examples() -> Examples<Redaction> {
        let draft = "0123456789abcdef0123456789abcdef";
        let sha256 = "ab".repeat(32);
        Examples {
            good: vec![
                Redaction {
                    draft: draft.into(),
                    sha256: sha256.clone(),
                    edits: r#"{"cuts":[{"section":"log","line":0}]}"#.into(),
                },
                Redaction { draft: draft.into(), sha256, edits: r#"{"cuts":[]}"#.into() },
            ],
            bad: vec![
                json!({ "draft": "x" }),
                json!({ "edits": r#"{"cuts":[{"section":"head","line":0}]}"# }),
                json!({ "draft": draft, "edits": r#"{"cuts":[]}"# }),
                json!({ "draft": draft, "sha256": "x", "edits": r#"{"cuts":[]}"# }),
                json!({ "draft": draft, "sha256": "AB".repeat(32), "edits": r#"{"cuts":[]}"# }),
                json!({ "draft": "0123456789ABCDEF0123456789ABCDEF", "sha256": "ab".repeat(32), "edits": "{}" }),
                json!({ "draft": draft, "sha256": "ab".repeat(32), "edits": "" }),
                json!({ "draft": draft, "sha256": "ab".repeat(32), "edits": "a\u{0}b" }),
                json!({ "draft": draft, "sha256": "ab".repeat(32), "edits": 7 }),
            ],
            undo: None,
        }
    }
}

/// A kept draft, to send as the person read it.
#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Sending {
    /// The kept draft's ID.
    pub draft: String,
    /// The SHA-256 of the text the person read.
    pub sha256: String,
    /// What the report is kept under on this machine.
    pub title: String,
    /// The crash it reports, marked reported once it is sent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub crash: Option<String>,
    /// Whether it goes to the security team only, encrypted.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub security: bool,
}

impl Args for Sending {
    const FIELDS: &'static [Field] = &[
        Field::text(
            "draft",
            "The kept draft's ID, as reports.draft gave it.",
            32,
            32,
            Some(DRAFT_PATTERN),
        ),
        Field::text(
            "sha256",
            "The SHA-256 of the text the person read, as reports.draft gave it.",
            64,
            64,
            Some(SHA256_PATTERN),
        ),
        Field::text(
            "title",
            "What the report is kept under here: its first words.",
            1,
            TITLE_MAX,
            Some(TEXT_PATTERN),
        ),
        Field::text(
            "crash",
            "The crash the report is about, which is marked reported once it is sent.",
            1,
            CRASH_MAX,
            Some(CRASH_PATTERN),
        )
        .optional(),
        Field::flag("security", "Whether to send it to the security team only, encrypted.")
            .optional(),
    ];

    fn check(&self) -> Result<(), Bad> {
        check_draft(&self.draft)?;
        check_sha256(&self.sha256)?;
        check_text("title", &self.title, TITLE_MAX)?;
        if self.title.trim().is_empty() {
            return Err(Bad { field: "title", why: refusal("A report has a title") });
        }
        self.crash.as_deref().map_or(Ok(()), check_crash)
    }
}

/// `report send DRAFT --sha256 HEX --title TEXT [--crash ID] [--security]
/// --json`: sends the kept draft, exactly as the person read it, or queues
/// it when the reports server cannot be reached. The answer says which.
pub struct SendReport;

impl Action for SendReport {
    const ID: &'static str = "reports.send";
    const AREA: &'static str = AREA;
    const CHANGES: bool = true;
    const PATH: &'static str = "/api/1/reports/send";
    const RUNS: Runs = Runs::AsThePerson;
    const ERRORS: &'static [&'static str] = &["RPT-101", "RPT-102", "RPT-103"];
    const TESTS: Tests = TESTS;
    const TITLE: &'static str = "Send a report";
    type Args = Sending;

    fn argv(args: &Sending) -> Vec<String> {
        let mut argv =
            ["report", "send", args.draft.as_str(), "--sha256", args.sha256.as_str(), "--title"]
                .map(String::from)
                .to_vec();
        argv.push(args.title.clone());
        if let Some(crash) = &args.crash {
            argv.extend(["--crash".to_string(), crash.clone()]);
        }
        if args.security {
            argv.push("--security".into());
        }
        argv.push("--json".into());
        argv
    }

    fn examples() -> Examples<Sending> {
        let draft = "0123456789abcdef0123456789abcdef";
        let one = |title: &str| Sending {
            draft: draft.into(),
            sha256: "ab".repeat(32),
            title: title.into(),
            ..Sending::default()
        };
        Examples {
            good: vec![
                one("The disk light stays on"),
                Sending { crash: Some("20261113T100211-4012".into()), ..one("nginx stopped") },
                Sending { security: true, ..one("A hole in the console") },
            ],
            bad: vec![
                json!({ "draft": "x" }),
                json!({ "draft": draft, "sha256": "ab".repeat(32) }),
                json!({ "draft": draft, "sha256": "ab".repeat(32), "title": "" }),
                json!({ "draft": draft, "sha256": "ab".repeat(32), "title": "x".repeat(TITLE_MAX + 1) }),
                json!({ "draft": draft, "sha256": "AB".repeat(32), "title": "lab" }),
                json!({ "draft": draft, "sha256": "ab".repeat(32), "title": "lab", "crash": "../x" }),
                json!({ "draft": draft, "sha256": "ab".repeat(32), "title": "lab", "text": "x" }),
            ],
            undo: None,
        }
    }
}

/// `report list --json`: the reports that wait to be sent and those that
/// were sent. reportd has no question for the sent ones that does not ask
/// the reports server where each stands, so with none reachable the answer
/// waits for it.
pub struct List;

impl Action for List {
    const ID: &'static str = "reports.list";
    const AREA: &'static str = AREA;
    const CHANGES: bool = false;
    const PATH: &'static str = "/api/1/reports";
    const RUNS: Runs = Runs::AsThePerson;
    const TESTS: Tests = TESTS;
    const TITLE: &'static str = "List the reports";
    type Args = Nothing;

    fn argv(_: &Nothing) -> Vec<String> {
        ["report", "list", "--json"].map(String::from).to_vec()
    }

    fn examples() -> Examples<Nothing> {
        reads()
    }
}

/// `report status --json`: asks the reports server where each report that
/// was sent stands. A server that cannot be reached is an answer for each
/// report, `unreachable`, and not a failure.
pub struct Status;

impl Action for Status {
    const ID: &'static str = "reports.status";
    const AREA: &'static str = AREA;
    const CHANGES: bool = false;
    const PATH: &'static str = "/api/1/reports/status";
    const RUNS: Runs = Runs::AsThePerson;
    const TESTS: Tests = TESTS;
    const TITLE: &'static str = "Ask where the sent reports stand";
    type Args = Nothing;

    fn argv(_: &Nothing) -> Vec<String> {
        ["report", "status", "--json"].map(String::from).to_vec()
    }

    fn examples() -> Examples<Nothing> {
        reads()
    }
}

/// `report retry --json`: sends the reports that wait, now. It answers for
/// each what became of it, and ends with 0 when reportd answered.
pub struct Retry;

impl Action for Retry {
    const ID: &'static str = "reports.retry";
    const AREA: &'static str = AREA;
    const CHANGES: bool = true;
    const PATH: &'static str = "/api/1/reports/retry";
    const RUNS: Runs = Runs::AsThePerson;
    const TESTS: Tests = TESTS;
    const TITLE: &'static str = "Send the reports that wait";
    type Args = Nothing;

    fn argv(_: &Nothing) -> Vec<String> {
        ["report", "retry", "--json"].map(String::from).to_vec()
    }

    fn examples() -> Examples<Nothing> {
        Examples { good: vec![Nothing {}], bad: vec![json!({ "all": true })], undo: None }
    }
}

/// A report that was sent, by its number, which the address gives.
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Number {
    /// The number the reports server gave it.
    pub number: i64,
}

impl Args for Number {
    const FIELDS: &'static [Field] =
        &[Field::number("number", "The number of a report that was sent.", 1, NUMBER_MAX)];

    fn check(&self) -> Result<(), Bad> {
        if (1..=NUMBER_MAX).contains(&self.number) {
            return Ok(());
        }
        let why = format!("A report's number is 1 to {NUMBER_MAX}");
        Err(Bad { field: "number", why: refusal(&why) })
    }
}

/// `report delete N`: deletes a report that was sent, on the reports
/// server and on this machine. The record is gone with it.
pub struct Delete;

impl Action for Delete {
    const ID: &'static str = "reports.delete";
    const AREA: &'static str = AREA;
    const CHANGES: bool = true;
    const PATH: &'static str = "/api/1/reports/{number}/delete";
    const RUNS: Runs = Runs::AsThePerson;
    const TESTS: Tests = TESTS;
    const TITLE: &'static str = "Delete a report that was sent";
    type Args = Number;

    fn argv(args: &Number) -> Vec<String> {
        ["report".to_string(), "delete".to_string(), args.number.to_string()].to_vec()
    }

    fn subject(args: &Number) -> Option<String> {
        Some(args.number.to_string())
    }

    fn examples() -> Examples<Number> {
        Examples {
            good: vec![Number { number: 1 }],
            bad: vec![
                json!({ "number": 0 }),
                json!({ "number": -1 }),
                json!({ "number": NUMBER_MAX + 1 }),
                json!({ "number": "1" }),
                json!({}),
            ],
            undo: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::Value;

    use super::super::{
        Caller, DOCUMENT_MAX, DOCUMENT_PATTERN, Found, REGISTRY, Registered, arguments, authorize,
        find, listed, matches, schema, typed_params,
    };
    use super::*;
    use crate::admind::errors::Kind;
    use crate::admind::scopes::Scopes;

    /// The registered action `id`.
    fn action(id: &str) -> &'static dyn Registered {
        *REGISTRY.iter().find(|action| action.id() == id).unwrap()
    }

    /// The command `id` makes for `args`.
    fn argv(id: &str, args: Value) -> Vec<String> {
        action(id).prepare(args).unwrap().argv
    }

    /// Why `id` refuses `args`: the kind, the field and the words.
    fn refused(id: &str, args: Value) -> (Kind, Option<&'static str>, String) {
        let failure = action(id).prepare(args).unwrap_err();
        (failure.kind, failure.field, failure.detail)
    }

    const DRAFT: &str = "0123456789abcdef0123456789abcdef";

    fn sha256() -> String {
        "ab".repeat(32)
    }

    /// Each action of the page: its id, its address, whether it changes
    /// anything, whether it is a `POST`, and the command its first example
    /// makes.
    const ACTIONS: [(&str, &str, bool, bool, &[&str]); 8] = [
        (
            "reports.crashes",
            "/api/1/reports/crashes",
            false,
            false,
            &["report", "crashes", "--json"],
        ),
        (
            "reports.draft",
            "/api/1/reports/drafts",
            false,
            true,
            &["report", "draft", "--about", "other", "--happened", "lab", "--json"],
        ),
        (
            "reports.redact",
            "/api/1/reports/drafts/{draft}/redact",
            false,
            true,
            &[
                "report",
                "redact",
                DRAFT,
                "--sha256",
                "abababababababababababababababababababababababababababababababab",
                "--json",
            ],
        ),
        (
            "reports.send",
            "/api/1/reports/send",
            true,
            true,
            &[
                "report",
                "send",
                DRAFT,
                "--sha256",
                "abababababababababababababababababababababababababababababababab",
                "--title",
                "The disk light stays on",
                "--json",
            ],
        ),
        ("reports.list", "/api/1/reports", false, false, &["report", "list", "--json"]),
        ("reports.status", "/api/1/reports/status", false, false, &["report", "status", "--json"]),
        ("reports.retry", "/api/1/reports/retry", true, true, &["report", "retry", "--json"]),
        (
            "reports.delete",
            "/api/1/reports/{number}/delete",
            true,
            true,
            &["report", "delete", "1"],
        ),
    ];

    fn first_example(id: &str) -> Value {
        action(id).example_values().0.remove(0)
    }

    #[test]
    fn each_action_runs_the_command_the_design_names_as_the_person() {
        for (id, path, changes, posts, words) in ACTIONS {
            let action = action(id);
            let prepared = action.prepare(first_example(id)).unwrap();
            assert_eq!(prepared.argv, words, "{id}");
            assert_eq!(
                (action.changes(), action.path(), action.area()),
                (changes, path, "reports")
            );
            // A report is the person's: reportd shows each their own.
            assert_eq!(action.runs(), Runs::AsThePerson, "{id}");
            assert!(!prepared.ansible.become_root, "{id}");
            assert_eq!(prepared.ansible.module, "ansible.builtin.command", "{id}");
            assert_eq!(prepared.ansible.params["argv"], json!(words), "{id}");
            assert_eq!(prepared.ansible.changed_when.as_deref() == Some("false"), !changes, "{id}");
            let said = listed(&[action]);
            assert_eq!(said[0]["method"], if posts { "POST" } else { "GET" }, "{id}");
            let tests = action.tests();
            assert_eq!(tests.checks, "admind::actions::reports::tests", "{id}");
            assert_eq!(tests.route, "admin-reports", "{id}");
            assert_eq!(tests.same_as_command, "admin-reports", "{id}");
            assert_eq!(tests.browser, "web-console/report", "{id}");
            assert_eq!(tests.ansible, "web-console/ansible", "{id}");
            assert!(!action.long() && !action.lines(), "{id}");
            assert_eq!(action.not_self(), None, "{id}");
            assert_eq!(action.cuts_off_on(), None, "{id}");
            assert!(action.restarts().is_none(), "{id}");
            assert_eq!(action.answers(), [0], "{id}");
        }
        let ours: Vec<_> = REGISTRY.iter().filter(|a| a.id().starts_with("reports.")).collect();
        assert_eq!(ours.len(), ACTIONS.len());
    }

    #[test]
    fn nothing_is_compared_and_no_file_is_named_since_reportd_keeps_the_reports() {
        for (id, ..) in ACTIONS {
            let action = action(id);
            let prepared = action.prepare(first_example(id)).unwrap();
            assert!(!action.compares(), "{id}");
            assert_eq!(prepared.probe, None, "{id}");
            assert_eq!(prepared.files, Vec::<String>::new(), "{id}");
            assert_eq!(prepared.trial, None, "{id}");
        }
    }

    #[test]
    fn the_draft_and_the_redaction_answer_the_person_s_report_and_keep_it_nowhere() {
        for id in ["reports.draft", "reports.redact"] {
            let action = action(id);
            assert!(action.secret_answer() && !action.changes() && !action.long(), "{id}");
            let prepared = action.prepare(first_example(id)).unwrap();
            // The playbook keeps the answer, and shows none of it.
            assert_eq!(prepared.ansible.register, Some("answer"), "{id}");
            assert!(prepared.ansible.no_log, "{id}");
        }
        for id in ["reports.crashes", "reports.list", "reports.status", "reports.send"] {
            assert!(!action(id).secret_answer(), "{id}");
        }
    }

    #[test]
    fn the_draft_names_what_it_is_about_and_what_goes_with_that_as_the_command_takes_them() {
        let draft = |args: Value| argv("reports.draft", args);
        let head = ["report", "draft", "--about"];
        assert_eq!(
            draft(json!({ "about": "other", "happened": "lab" })),
            [&head[..], &["other", "--happened", "lab", "--json"]].concat()
        );
        // Each word the page may say, as it says it: the command takes both
        // `update` and `updates`.
        for about in ["hardware", "failure", "update", "updates", "other"] {
            let said = draft(json!({ "about": about, "happened": "x" }));
            assert_eq!(said[3], about);
        }
        assert_eq!(
            draft(json!({ "about": "crash", "crash": "20261113T100211-4012", "happened": "x" })),
            [&head[..], &["crash", "--crash", "20261113T100211-4012", "--happened", "x", "--json"]]
                .concat()
        );
        for about in ["service", "program"] {
            assert_eq!(
                draft(json!({ "about": about, "name": "php-fpm8.3", "happened": "x" })),
                [&head[..], &[about, "--name", "php-fpm8.3", "--happened", "x", "--json"]].concat()
            );
        }
        // The three answers, each as one word with its line breaks, and an
        // answer left empty is left out.
        assert_eq!(
            draft(json!({
                "about": "other", "happened": "It stopped\nat 3", "expected": "It runs",
                "again": "", "without_logs": true
            })),
            [
                &head[..],
                &[
                    "other",
                    "--happened",
                    "It stopped\nat 3",
                    "--expected",
                    "It runs",
                    "--without-logs",
                    "--json"
                ]
            ]
            .concat()
        );
        // A word that starts with a dash is a value, which the command reads.
        let said = draft(json!({ "about": "other", "happened": "--json" }));
        assert_eq!(said[5], "--json");
        // What was asked is kept as it was, and what was left out is not made up.
        let prepared = action("reports.draft")
            .prepare(json!({ "about": "other", "happened": "x", "expected": null }))
            .unwrap();
        assert_eq!(prepared.args, json!({ "about": "other", "happened": "x" }));
    }

    #[test]
    fn what_does_not_go_with_what_a_report_is_about_is_refused_in_words_the_page_shows() {
        let end = "Nothing was done.";
        let draft = |args: Value| refused("reports.draft", args);
        let value = |field, why: &str| (Kind::BadValue, Some(field), format!("{why}. {end}"));
        let said = draft(json!({ "about": "computer", "happened": "x" }));
        assert_eq!((said.0, said.1), (Kind::BadValue, Some("about")), "an unknown word");
        assert_eq!(draft(json!({ "about": 7, "happened": "x" })).0, Kind::BadRequest);
        assert_eq!(
            draft(json!({ "about": "crash", "happened": "x" })),
            value("crash", "A report about a crash names the crash by its ID")
        );
        assert_eq!(
            draft(json!({ "about": "other", "crash": "2026", "happened": "x" })),
            value("crash", "A crash's ID goes with a report about a crash")
        );
        assert_eq!(
            draft(json!({ "about": "crash", "crash": "../x", "happened": "x" })).1,
            Some("crash")
        );
        assert_eq!(
            draft(json!({ "about": "service", "happened": "x" })),
            value("name", "A report about a service or a program names it")
        );
        assert_eq!(
            draft(json!({ "about": "hardware", "name": "nginx", "happened": "x" })),
            value("name", "A name goes with a report about a service or a program")
        );
        for name in ["", "../x", "-x", ".x", "a b", "a/b", "a\nb", &"x".repeat(65)] {
            let said = draft(json!({ "about": "program", "name": name, "happened": "x" }));
            assert_eq!((said.0, said.1), (Kind::BadValue, Some("name")), "{name:?}");
        }
        for name in ["a", "_x", "php-fpm8.3", "g++", "x".repeat(64).as_str()] {
            assert!(
                action("reports.draft")
                    .prepare(json!({ "about": "program", "name": name, "happened": "x" }))
                    .is_ok(),
                "{name}"
            );
        }
        // Something has to be said of what happened, and not too much of
        // anything.
        for happened in ["", " ", "\n\t "] {
            let said = draft(json!({ "about": "other", "happened": happened }));
            assert_eq!((said.0, said.1), (Kind::BadValue, Some("happened")), "{happened:?}");
        }
        assert_eq!(
            draft(json!({ "about": "other", "happened": "x".repeat(2001) })),
            value("happened", "It is text of up to 2000 characters, with no NUL")
        );
        assert_eq!(
            draft(json!({ "about": "other", "happened": "x", "again": "x\0y" })).1,
            Some("again")
        );
        assert_eq!(
            draft(json!({ "about": "other", "happened": "x", "expected": "é".repeat(2001) })).1,
            Some("expected")
        );
        for (field, text) in [
            ("happened", "é".repeat(2000)),
            ("expected", "x".repeat(2000)),
            ("again", String::new()),
        ] {
            let mut args = json!({ "about": "other", "happened": "x" });
            args[field] = json!(text);
            assert!(action("reports.draft").prepare(args).is_ok(), "{field}");
        }
    }

    #[test]
    fn a_crash_is_the_digits_a_t_and_dashes_of_reportd_s_ids() {
        for id in ["20261113T100211-4012", "1", "T", "-", &"1".repeat(40)] {
            assert!(is_crash_id(id), "{id}");
        }
        for id in ["", "a", "2026 1113", "../x", "20261113t100211-4012", &"1".repeat(41), "1\n"] {
            assert!(!is_crash_id(id), "{id:?}");
        }
    }

    #[test]
    fn the_redaction_reads_the_edits_on_its_input_and_never_in_its_words() {
        let edits =
            r#"{"cuts":[{"section":"log","line":41,"through":57}],"answers":{"happened":"a\nb"}}"#;
        let args = json!({ "draft": DRAFT, "sha256": sha256(), "edits": edits });
        let prepared = action("reports.redact").prepare(args).unwrap();
        // The command's words name the draft and its hash; the edits are
        // what it reads, exactly as given.
        assert_eq!(prepared.argv, ["report", "redact", DRAFT, "--sha256", &sha256(), "--json"]);
        assert_eq!(prepared.input.as_ref().unwrap().bytes(), edits.as_bytes());
        assert!(!prepared.argv.join(" ").contains("cuts"));
        // The playbook gives them from its own variable.
        assert_eq!(prepared.ansible.params["stdin"], "{{ edits }}");
        assert_eq!(action("reports.redact").input_max(), 65_536);
        // The address gives the draft, and the body the rest.
        let Found::Action(found, params) =
            find(REGISTRY, true, &format!("/api/1/reports/drafts/{DRAFT}/redact"))
        else {
            panic!("no action at the address")
        };
        assert_eq!(found.id(), "reports.redact");
        let body = json!({ "sha256": sha256(), "edits": r#"{"cuts":[]}"# }).to_string();
        let given = arguments(params, body.as_bytes()).unwrap();
        assert_eq!(given["draft"], DRAFT);
        assert!(found.prepare(given).is_ok());
        // A draft in the body too is refused, so that neither stands for the other.
        let params =
            matches(found.path(), &format!("/api/1/reports/drafts/{DRAFT}/redact")).unwrap();
        assert!(arguments(params, json!({ "draft": DRAFT }).to_string().as_bytes()).is_err());
    }

    #[test]
    fn the_edits_are_a_document_of_one_to_64_kib_that_reportd_reads() {
        assert_eq!(EDITS_MAX, DOCUMENT_MAX);
        assert_eq!(TEXT_PATTERN, DOCUMENT_PATTERN);
        let args = |edits: Value| json!({ "draft": DRAFT, "sha256": sha256(), "edits": edits });
        let said = |edits: Value| refused("reports.redact", args(edits));
        // Any text the document takes is the command's to read, and reportd's
        // to refuse (RPT-104, RPT-105): the action holds no rule of the edits.
        for edits in ["{}", "[]", "x", "{\"cuts\":[]}", &"x".repeat(EDITS_MAX)] {
            assert!(action("reports.redact").prepare(args(json!(edits))).is_ok(), "{edits:.20}");
        }
        for (edits, why) in [
            (json!(""), "It is text of 1 to 65536 bytes, with no NUL. Nothing was done."),
            (
                json!("x".repeat(EDITS_MAX + 1)),
                "It is text of 1 to 65536 bytes, with no NUL. Nothing was done.",
            ),
            (json!("a\0b"), "It is text of 1 to 65536 bytes, with no NUL. Nothing was done."),
        ] {
            assert_eq!(said(edits), (Kind::BadValue, Some("edits"), why.into()));
        }
        assert_eq!(said(json!(7)).0, Kind::BadRequest);
        let fields = schema(action("reports.redact").fields());
        assert_eq!(fields["properties"]["edits"]["maxLength"], 65_536);
        assert_eq!(fields["properties"]["edits"]["pattern"], DOCUMENT_PATTERN);
        assert_eq!(fields["required"], json!(["draft", "sha256", "edits"]));
    }

    #[test]
    fn a_draft_and_its_hash_are_lower_case_hex_of_32_and_64_digits() {
        for (id, other) in [
            ("reports.redact", json!({ "edits": "{}" })),
            ("reports.send", json!({ "title": "t" })),
        ] {
            let with = |field: &str, text: &str| {
                let mut args = json!({ "draft": DRAFT, "sha256": sha256() });
                args[field] = json!(text);
                for (name, value) in other.as_object().unwrap() {
                    args[name] = value.clone();
                }
                refused(id, args)
            };
            for draft in [
                "",
                "x",
                &DRAFT[1..],
                &format!("{DRAFT}0"),
                &DRAFT.to_uppercase(),
                "g".repeat(32).as_str(),
            ] {
                let said = with("draft", draft);
                assert_eq!((said.0, said.1), (Kind::BadValue, Some("draft")), "{id} {draft:?}");
            }
            for hash in
                ["", "x", &sha256()[1..], &format!("{}0", sha256()), &sha256().to_uppercase()]
            {
                let said = with("sha256", hash);
                assert_eq!((said.0, said.1), (Kind::BadValue, Some("sha256")), "{id} {hash:?}");
            }
        }
        assert_eq!(
            refused("reports.redact", json!({ "draft": "x", "sha256": sha256(), "edits": "{}" })).2,
            "A draft's ID is 32 lower-case hex digits. Nothing was done."
        );
    }

    #[test]
    fn a_send_names_the_draft_its_hash_and_its_title_and_what_the_person_chose() {
        let send = |args: Value| argv("reports.send", args);
        let base = |extra: Value| {
            let mut args = json!({ "draft": DRAFT, "sha256": sha256(), "title": "nginx stopped" });
            for (name, value) in extra.as_object().unwrap() {
                args[name] = value.clone();
            }
            args
        };
        let head = ["report", "send", DRAFT, "--sha256", &sha256(), "--title", "nginx stopped"];
        assert_eq!(send(base(json!({}))), [&head[..], &["--json"]].concat());
        assert_eq!(
            send(base(json!({ "crash": "20261113T100211-4012", "security": true }))),
            [&head[..], &["--crash", "20261113T100211-4012", "--security", "--json"]].concat()
        );
        assert_eq!(send(base(json!({ "security": false }))), [&head[..], &["--json"]].concat());
        // A title is one word, whatever it holds, and is at most 120 characters.
        assert_eq!(send(base(json!({ "title": "-x\nb" })))[6], "-x\nb");
        let title = |text: String| refused("reports.send", base(json!({ "title": text })));
        assert!(action("reports.send").prepare(base(json!({ "title": "é".repeat(120) }))).is_ok());
        for text in ["é".repeat(121), String::new(), "  ".into(), "a\0b".into()] {
            let said = title(text.clone());
            assert_eq!((said.0, said.1), (Kind::BadValue, Some("title")), "{text:?}");
        }
        assert_eq!(title("\n".into()).2, "A report has a title. Nothing was done.");
        // Its errors are reportd's: the draft ended, or is not the one read,
        // and no key of the security team.
        assert_eq!(action("reports.send").errors(), ["RPT-101", "RPT-102", "RPT-103"]);
        assert!(action("reports.send").changes());
        assert_eq!(action("reports.redact").errors(), ["RPT-101", "RPT-102", "RPT-104", "RPT-105"]);
        // A send always does something: it has no probe, and the command that
        // lists the reports would ask the reports server twice.
        let prepared = action("reports.send").prepare(base(json!({}))).unwrap();
        assert_eq!((prepared.probe, action("reports.send").compares()), (None, false));
    }

    #[test]
    fn a_report_is_deleted_by_the_number_the_address_gives_from_1_to_2_to_the_31st() {
        assert_eq!(argv("reports.delete", json!({ "number": 7 })), ["report", "delete", "7"]);
        assert_eq!(
            argv("reports.delete", json!({ "number": 2_147_483_648_i64 })),
            ["report", "delete", "2147483648"]
        );
        for number in [0, -1, 2_147_483_649_i64, i64::MAX, i64::MIN] {
            let said = refused("reports.delete", json!({ "number": number }));
            assert_eq!(
                said,
                (
                    Kind::BadValue,
                    Some("number"),
                    "A report's number is 1 to 2147483648. Nothing was done.".into()
                ),
                "{number}"
            );
        }
        for number in [json!("7"), json!(1.5), json!(null), json!([7])] {
            let said = refused("reports.delete", json!({ "number": number }));
            assert_eq!(said.0, Kind::BadRequest, "{number}");
        }
        // The address gives it as text, and a post reads it as a number, as a
        // read's address does.
        let Found::Action(found, params) = find(REGISTRY, true, "/api/1/reports/7/delete") else {
            panic!("no action at the address")
        };
        assert_eq!(found.id(), "reports.delete");
        let typed = typed_params(params, found.fields()).unwrap();
        assert_eq!(arguments(typed, b"").unwrap(), json!({ "number": 7 }));
        let Found::Action(_, params) = find(REGISTRY, true, "/api/1/reports/seven/delete") else {
            panic!("no action at the address")
        };
        let failure = typed_params(params, found.fields()).unwrap_err();
        assert_eq!(failure.kind, Kind::BadRequest);
        assert_eq!(failure.detail, "\"number\" is a whole number");
        assert!(!found.long() && !found.compares());
    }

    #[test]
    fn each_address_finds_its_action_and_no_address_answers_the_other_method() {
        for (get, path, id) in [
            (true, "/api/1/reports/crashes", "reports.crashes"),
            (true, "/api/1/reports", "reports.list"),
            (true, "/api/1/reports/status", "reports.status"),
            (false, "/api/1/reports/drafts", "reports.draft"),
            (false, "/api/1/reports/send", "reports.send"),
            (false, "/api/1/reports/retry", "reports.retry"),
            (false, "/api/1/reports/9/delete", "reports.delete"),
            (
                false,
                "/api/1/reports/drafts/0123456789abcdef0123456789abcdef/redact",
                "reports.redact",
            ),
        ] {
            let Found::Action(found, _) = find(REGISTRY, !get, path) else { panic!("{path}") };
            assert_eq!(found.id(), id, "{path}");
            assert!(matches!(find(REGISTRY, get, path), Found::WrongMethod), "{path}");
        }
        for path in [
            "/api/1/reports/",
            "/api/1/reports/crashes/x",
            "/api/1/reports/status/1",
            "/api/1/reports/drafts/x",
            "/api/1/reports/drafts/x/redact/y",
            "/api/1/reports/9/delete/x",
            "/api/1/reports/9",
        ] {
            assert!(matches!(find(REGISTRY, true, path), Found::None), "{path}");
            assert!(matches!(find(REGISTRY, false, path), Found::None), "{path}");
        }
    }

    #[test]
    fn anyone_who_signs_in_reads_and_sends_their_own_reports_and_a_token_needs_the_scope() {
        for (id, _, changes, ..) in ACTIONS {
            let action = action(id);
            for (admin, unlocked) in [(false, false), (false, true), (true, false), (true, true)] {
                let caller = Caller::Session { name: "ann", admin, unlocked };
                assert!(authorize(&caller, action).is_ok(), "{id} {admin} {unlocked}");
            }
            let call = |list: &str| {
                let scopes = Scopes::parse(list).unwrap();
                authorize(&Caller::Token { name: "ci", scopes: &scopes }, action)
            };
            for list in ["reports", "*"] {
                assert!(call(list).is_ok(), "{id} {list}");
            }
            assert_eq!(call("reports:read").is_ok(), !changes, "{id}");
            for list in ["hardware", "storage", "alerts"] {
                assert_eq!(call(list).unwrap_err().kind, Kind::OutOfScope, "{id} {list}");
            }
        }
    }

    #[test]
    fn the_examples_work_and_the_bad_ones_do_not() {
        for (id, ..) in ACTIONS {
            let action = action(id);
            let (good, bad, undo) = action.example_values();
            assert!(!bad.is_empty() && undo.is_none(), "{id}: a report sent is sent");
            for args in &good {
                assert!(action.prepare(args.clone()).is_ok(), "{id} {args}");
            }
            for args in bad {
                assert!(action.prepare(args.clone()).is_err(), "{id} {args}");
            }
        }
        let (good, ..) = action("reports.draft").example_values();
        for args in [
            json!({ "about": "other", "happened": "lab" }),
            json!({ "about": "update", "happened": "The disk light stays on" }),
            json!({ "about": "updates", "happened": "The disk light stays on" }),
        ] {
            assert!(good.contains(&args), "{args}");
        }
        // The page's own words for what the report is about are all there.
        let about = &schema(action("reports.draft").fields())["properties"]["about"]["enum"];
        for word in ["crash", "service", "failure", "hardware", "update", "other"] {
            assert!(about.as_array().unwrap().contains(&json!(word)), "{word}");
        }
    }

    #[test]
    fn the_schemas_say_the_limits_of_each_field() {
        let draft = schema(action("reports.draft").fields());
        assert_eq!(draft["required"], json!(["about", "happened"]));
        assert_eq!(draft["additionalProperties"], false);
        let happened = &draft["properties"]["happened"];
        assert_eq!((&happened["minLength"], &happened["maxLength"]), (&json!(1), &json!(2000)));
        assert_eq!(draft["properties"]["expected"]["minLength"], 0);
        assert_eq!(draft["properties"]["crash"]["pattern"], CRASH_PATTERN);
        assert_eq!(draft["properties"]["name"]["pattern"], NAME_PATTERN);
        assert_eq!(draft["properties"]["without_logs"]["type"], "boolean");
        let send = schema(action("reports.send").fields());
        assert_eq!(send["required"], json!(["draft", "sha256", "title"]));
        assert_eq!(send["properties"]["draft"]["pattern"], "^[0-9a-f]{32}$");
        assert_eq!(send["properties"]["sha256"]["pattern"], "^[0-9a-f]{64}$");
        assert_eq!(send["properties"]["title"]["maxLength"], 120);
        let delete = schema(action("reports.delete").fields());
        assert_eq!(delete["properties"]["number"]["type"], "integer");
        assert_eq!(
            (
                &delete["properties"]["number"]["minimum"],
                &delete["properties"]["number"]["maximum"]
            ),
            (&json!(1), &json!(2_147_483_648_i64))
        );
        for id in ["reports.crashes", "reports.list", "reports.status", "reports.retry"] {
            assert_eq!(schema(action(id).fields())["properties"], json!({}), "{id}");
            assert!(action(id).fields().is_empty(), "{id}");
        }
    }
}
