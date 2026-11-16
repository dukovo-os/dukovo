// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The forms of report(1) that print JSON, for the web console and for a
//! script: `report draft`, `send` and `redact`, `crashes`, and `--json` of
//! `list`, `status` and `retry`.
//!
//! They ask reportd what the interactive `report` asks. reportd keeps a
//! draft for the person who asked for it: `send`, `redact` and `text` name
//! it by its ID and the SHA-256 of its text, and what is sent is the text
//! reportd drafted, as it was. Every answer has `format`, so that a script
//! can tell when a member is added; report(1) describes them.
//!
//! A mistake in the command line, or in the edits `redact` reads, ends
//! with status 2; a refusal of reportd, with 1.

use std::io::Read;

use serde::Serialize;

use super::client::{self, Client};
use super::protocol::{
    ANSWER_MAX, About, Answers, Crash, CutBy, CutByYou, Drafted, EDITS_MAX, Edits, Kind, Place,
    Request, Sent, Size, Status, Waiting,
};
use crate::util::{Fail, fail, fail_with, usage};

/// The version of the members of these answers.
const FORMAT: u32 = 1;

/// What `report draft` was asked for, checked.
#[derive(Debug, Default, PartialEq, Eq)]
pub(super) struct DraftArgs {
    /// What the report is about: a word of [`ABOUT`].
    pub(super) about: String,
    /// The crash's ID, for a report about a crash.
    pub(super) crash: Option<String>,
    /// The service's or program's name.
    pub(super) name: Option<String>,
    pub(super) happened: String,
    pub(super) expected: String,
    pub(super) again: String,
    pub(super) without_logs: bool,
}

/// What a report may be about, as `--about` says it. `update` is `updates`
/// as the web console says it.
const ABOUT: [&str; 8] =
    ["crash", "service", "program", "failure", "hardware", "updates", "update", "other"];

/// The words of [`ABOUT`] a mistake names.
const ABOUT_WORDS: &str = "crash, service, program, failure, hardware, updates or other";

/// What the options of `report draft` that name its subject were.
#[derive(Debug, Default)]
pub(super) struct Given {
    pub(super) about: Option<String>,
    pub(super) crash: Option<String>,
    pub(super) name: Option<String>,
    pub(super) happened: Option<String>,
    pub(super) expected: Option<String>,
    pub(super) again: Option<String>,
    pub(super) without_logs: bool,
}

impl DraftArgs {
    /// The draft `given` asks for.
    ///
    /// # Errors
    ///
    /// What is missing, what does not go with what is asked, and an answer
    /// that is empty where it is needed or too long.
    pub(super) fn of(given: Given) -> Result<DraftArgs, Fail> {
        let about = given
            .about
            .ok_or_else(|| usage(format!("report draft needs --about: {ABOUT_WORDS}")))?;
        if !ABOUT.contains(&about.as_str()) {
            return Err(usage(format!("--about names what the report is about: {ABOUT_WORDS}")));
        }
        let needs = |kind: &str| about == kind;
        match (&given.crash, needs("crash")) {
            (None, true) => return Err(usage("--about crash needs --crash ID".into())),
            (Some(_), false) => return Err(usage("--crash goes with --about crash".into())),
            _ => {}
        }
        match (&given.name, needs("service") || needs("program")) {
            (None, true) => return Err(usage(format!("--about {about} needs --name NAME"))),
            (Some(_), false) => {
                return Err(usage("--name goes with --about service or program".into()));
            }
            _ => {}
        }
        // The failed start is told by its notice and its log: its answers
        // are all optional, as the terminal's are.
        let optional = needs("failure");
        let happened = given.happened.unwrap_or_default();
        if !optional && happened.trim().is_empty() {
            return Err(usage("report draft needs --happened: what happened".into()));
        }
        let expected = given.expected.unwrap_or_default();
        let again = given.again.unwrap_or_default();
        for (option, text) in
            [("--happened", &happened), ("--expected", &expected), ("--again", &again)]
        {
            if text.chars().count() > ANSWER_MAX {
                return Err(usage(format!("{option} is at most {ANSWER_MAX} characters")));
            }
        }
        Ok(DraftArgs {
            about,
            crash: given.crash,
            name: given.name,
            happened,
            expected,
            again,
            without_logs: given.without_logs,
        })
    }

    /// What reportd is told the report is about; the failed start's notice
    /// comes from `notice`, which says none when no start has failed.
    ///
    /// # Errors
    ///
    /// If the report is about a failed start and none has failed.
    pub(super) fn about(
        &self,
        notice: impl FnOnce() -> Option<Vec<String>>,
    ) -> Result<About, Fail> {
        let text = |value: &Option<String>| value.clone().unwrap_or_default();
        Ok(match self.about.as_str() {
            "crash" => About::Crash { id: text(&self.crash) },
            "service" => About::Service { name: text(&self.name) },
            "program" => About::Program { name: text(&self.name) },
            "failure" => About::Failure {
                notice: notice()
                    .ok_or_else(|| fail("no start has failed since this one began".into()))?,
            },
            "hardware" => About::Hardware,
            "updates" | "update" => About::Updates,
            _ => About::Other,
        })
    }

    /// The request that drafts it, kept by reportd for the asker.
    pub(super) fn request(&self, about: About) -> Request {
        Request {
            ask: "draft".into(),
            about: Some(about),
            answers: Some(Answers {
                happened: self.happened.clone(),
                expected: self.expected.clone(),
                again: self.again.clone(),
            }),
            without_logs: self.without_logs,
            keep: true,
            ..Request::default()
        }
    }
}

/// What `report send` was asked for.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct SendArgs {
    pub(super) draft: String,
    pub(super) sha256: String,
    pub(super) title: String,
    pub(super) crash: Option<String>,
    pub(super) security: bool,
}

impl SendArgs {
    /// The request that sends the kept draft.
    fn request(&self) -> Request {
        Request {
            ask: "send".into(),
            draft: Some(self.draft.clone()),
            sha256: Some(self.sha256.clone()),
            title: Some(self.title.clone()),
            crash: self.crash.clone(),
            security: self.security,
            ..Request::default()
        }
    }
}

/// A draft or a redaction, as `--json` says it.
#[derive(Debug, PartialEq, Eq, Serialize)]
struct DraftJson {
    format: u32,
    /// The kept draft's ID.
    draft: String,
    /// The SHA-256 of `text`, in hex: what `send` names to send it.
    sha256: String,
    /// The text's length in bytes.
    bytes: usize,
    text: String,
    /// How many pieces redaction took out.
    cuts: u32,
    has_logs: bool,
    cut_by: CutBy,
    parts: Vec<Size>,
    sections: Vec<Place>,
    typed: Vec<Place>,
    cut_by_you: CutByYou,
}

/// The JSON of a kept draft's answer.
///
/// # Errors
///
/// If reportd did not keep it: it gave no ID or no hash.
fn draft_json(drafted: Drafted) -> Result<DraftJson, Fail> {
    let (Some(draft), Some(sha256)) = (drafted.draft, drafted.sha256) else {
        return Err(fail("reportd did not keep the draft: it gave no ID or no hash".into()));
    };
    Ok(DraftJson {
        format: FORMAT,
        draft,
        sha256,
        bytes: drafted.text.len(),
        text: drafted.text,
        cuts: drafted.cuts,
        has_logs: drafted.has_logs,
        cut_by: drafted.cut_by,
        parts: drafted.parts,
        sections: drafted.sections,
        typed: drafted.typed,
        cut_by_you: drafted.cut_by_you.unwrap_or_default(),
    })
}

/// Prints `value` as the line of JSON that is the answer.
fn print_json(value: &impl Serialize) -> Result<i32, Fail> {
    let text = serde_json::to_string(value)
        .map_err(|error| fail(format!("cannot write JSON: {error}")))?;
    println!("{text}");
    Ok(0)
}

/// `report draft ... --json`: drafts the report and keeps it with reportd.
pub(super) fn draft(
    client: &Client,
    args: &DraftArgs,
    notice: impl FnOnce() -> Option<Vec<String>>,
) -> Result<i32, Fail> {
    let request = args.request(args.about(notice)?);
    print_json(&draft_json(client.one(&request, client::QUICK)?)?)
}

/// What `send` says of a report, as `--json` says it.
#[derive(Debug, PartialEq, Eq, Serialize)]
struct SentJson {
    format: u32,
    /// The number the reports server gave it, once sent.
    number: Option<u64>,
    /// `sent`, or `queued` while it waits to be sent.
    state: &'static str,
    /// Why it waits, when it does.
    why: Option<String>,
}

/// The JSON of `sent`.
fn sent_json(sent: Sent) -> SentJson {
    let queued = sent.waiting.is_some();
    SentJson {
        format: FORMAT,
        number: sent.number,
        state: if queued { "queued" } else { "sent" },
        why: sent.waiting,
    }
}

/// `report send ... --json`: sends the kept draft, as the person read it.
pub(super) fn send(client: &Client, args: &SendArgs) -> Result<i32, Fail> {
    let sent: Sent = client.one(&args.request(), client::ONLINE)?;
    print_json(&sent_json(sent))
}

/// A mistake in what `redact` was given to read, which ends with status 2
/// as a mistake in its command line does.
fn mistake(message: String) -> Fail {
    Fail { usage: true, ..fail_with(2, message) }
}

/// The edits on `input`: the whole list of cuts, and the answers if they
/// change.
///
/// # Errors
///
/// If they are longer than reportd takes, or are no list of edits.
pub(super) fn read_edits(input: &mut impl Read) -> Result<Edits, Fail> {
    let mut bytes = Vec::new();
    input
        .take(EDITS_MAX as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| fail(format!("cannot read the edits: {error}")))?;
    if bytes.len() > EDITS_MAX {
        return Err(mistake(format!("the edits are longer than {} KiB", EDITS_MAX / 1024)));
    }
    let not_edits = |why: String| mistake(format!("the edits are not a list of edits: {why}"));
    let value: serde_json::Value =
        serde_json::from_slice(&bytes).map_err(|error| not_edits(error.to_string()))?;
    // A JSON array would read as an empty list of edits.
    if !value.is_object() {
        return Err(not_edits("a JSON object with \"cuts\" is wanted".into()));
    }
    serde_json::from_value(value).map_err(|error| not_edits(error.to_string()))
}

/// `report redact DRAFT --sha256 HEX --json`: applies the edits on `input`
/// to the kept draft, and prints what it is now.
pub(super) fn redact(
    client: &Client,
    draft: &str,
    sha256: &str,
    input: &mut impl Read,
) -> Result<i32, Fail> {
    let edits = read_edits(input)?;
    let request = Request {
        ask: "redact".into(),
        draft: Some(draft.into()),
        sha256: Some(sha256.into()),
        edits: Some(edits),
        ..Request::default()
    };
    print_json(&draft_json(client.one(&request, client::QUICK)?)?)
}

/// A crash, as `--json` says it.
#[derive(Debug, PartialEq, Eq, Serialize)]
struct CrashJson {
    id: String,
    kind: Kind,
    program: String,
    version: Option<String>,
    service: Option<String>,
    /// When it happened, in milliseconds since 1970.
    at_ms: u64,
    what: String,
    /// How many times this crash was seen while it was kept.
    seen: u32,
    /// The report's number, once reported.
    reported: Option<u64>,
}

/// The crashes as `report crashes --json` says them.
#[derive(Debug, PartialEq, Eq, Serialize)]
struct CrashesJson {
    format: u32,
    crashes: Vec<CrashJson>,
}

/// The JSON of `crashes`, in the order reportd gave them.
fn crashes_json(crashes: Vec<Crash>) -> CrashesJson {
    CrashesJson {
        format: FORMAT,
        crashes: crashes
            .into_iter()
            .map(|crash| CrashJson {
                id: crash.id,
                kind: crash.kind,
                program: crash.program,
                version: crash.version,
                service: crash.service,
                at_ms: crash.at.saturating_mul(1000),
                what: crash.what,
                seen: crash.seen,
                reported: crash.reported,
            })
            .collect(),
    }
}

/// `report crashes --json`.
pub(super) fn crashes(client: &Client) -> Result<i32, Fail> {
    let crashes: Vec<Crash> =
        client.list(&Request { ask: "crashes".into(), ..Request::default() }, client::QUICK)?;
    print_json(&crashes_json(crashes))
}

/// A report that waits or was sent, as `report list --json` says it.
#[derive(Debug, PartialEq, Eq, Serialize)]
struct RowJson {
    /// The number the reports server gave it: none while it waits.
    number: Option<u64>,
    title: String,
    /// `queued` while it waits to be sent, `sent` after.
    state: &'static str,
    /// When it was queued; none for one sent, which keeps no such time.
    queued_ms: Option<u64>,
    /// When it was sent.
    sent_ms: Option<u64>,
    /// How many times it was tried.
    tries: Option<u32>,
    /// When it is tried next.
    next_ms: Option<u64>,
    /// The ID the queue gave it, while it waits.
    receipt: Option<String>,
    /// Whether it goes to the security team, encrypted.
    security: bool,
    /// Why the last try failed.
    why: Option<String>,
}

/// The reports as `report list --json` says them.
#[derive(Debug, PartialEq, Eq, Serialize)]
struct ListJson {
    format: u32,
    reports: Vec<RowJson>,
}

/// The rows of `waiting`, oldest first as the queue keeps them, and then
/// of `sent`, newest first.
fn list_json(waiting: Vec<Waiting>, sent: Vec<Status>) -> ListJson {
    let waits = waiting.into_iter().map(|report| RowJson {
        number: None,
        title: report.title,
        state: "queued",
        queued_ms: Some(report.since.saturating_mul(1000)),
        sent_ms: None,
        tries: Some(report.tries),
        next_ms: Some(report.next.saturating_mul(1000)),
        receipt: Some(report.id),
        security: report.security,
        why: report.error,
    });
    let sends = sent.into_iter().map(|report| RowJson {
        number: Some(report.number),
        title: report.title,
        state: "sent",
        queued_ms: None,
        sent_ms: Some(report.sent.saturating_mul(1000)),
        tries: None,
        next_ms: None,
        receipt: None,
        security: report.security,
        why: None,
    });
    ListJson { format: FORMAT, reports: waits.chain(sends).collect() }
}

/// `report list --json`: what waits and what was sent.
///
/// reportd has no question that lists the reports sent without asking the
/// reports server where each stands, so this asks its `status`, and
/// leaves the answers out.
pub(super) fn list(client: &Client) -> Result<i32, Fail> {
    let waiting: Vec<Waiting> =
        client.list(&Request { ask: "queue".into(), ..Request::default() }, client::QUICK)?;
    let sent: Vec<Status> =
        client.list(&Request { ask: "status".into(), ..Request::default() }, client::ONLINE)?;
    print_json(&list_json(waiting, sent))
}

/// Where a sent report stands, as `report status --json` says it.
#[derive(Debug, PartialEq, Eq, Serialize)]
struct StandingJson {
    number: u64,
    title: String,
    /// When it was sent, in milliseconds since 1970.
    sent_ms: u64,
    /// The ID a queue gave it, which a sent report no longer has.
    receipt: Option<String>,
    /// Where it stands, in words; none when the server could not be asked.
    status: Option<String>,
    /// Whether the server could not be asked.
    unreachable: bool,
    /// Why it could not be.
    why: Option<String>,
}

/// The reports as `report status --json` says them.
#[derive(Debug, PartialEq, Eq, Serialize)]
struct StatusJson {
    format: u32,
    reports: Vec<StandingJson>,
}

/// The JSON of `reports`, where each stands in `words`'s words.
fn status_json(reports: Vec<Status>, words: fn(&Status) -> String) -> StatusJson {
    StatusJson {
        format: FORMAT,
        reports: reports
            .into_iter()
            .map(|report| {
                let unreachable = report.status.is_none();
                StandingJson {
                    number: report.number,
                    title: report.title.clone(),
                    sent_ms: report.sent.saturating_mul(1000),
                    receipt: None,
                    status: (!unreachable).then(|| words(&report)),
                    unreachable,
                    why: report.error,
                }
            })
            .collect(),
    }
}

/// `report status [N] --json`: asks the reports server for each report.
pub(super) fn status(
    client: &Client,
    number: Option<u64>,
    words: fn(&Status) -> String,
) -> Result<i32, Fail> {
    let ask = Request { ask: "status".into(), number, ..Request::default() };
    let reports: Vec<Status> = client.list(&ask, client::ONLINE)?;
    print_json(&status_json(reports, words))
}

/// The reports `retry` tried, as `--json` says them.
#[derive(Debug, PartialEq, Eq, Serialize)]
struct TriedJson {
    format: u32,
    reports: Vec<TryJson>,
}

/// One report `retry` tried.
#[derive(Debug, PartialEq, Eq, Serialize)]
struct TryJson {
    /// The ID the queue gave it.
    receipt: String,
    number: Option<u64>,
    state: &'static str,
    why: Option<String>,
}

/// The JSON of what a retry sent.
fn tried_json(tried: Vec<Sent>) -> TriedJson {
    TriedJson {
        format: FORMAT,
        reports: tried
            .into_iter()
            .map(|sent| {
                let id = sent.id.clone();
                let said = sent_json(sent);
                TryJson { receipt: id, number: said.number, state: said.state, why: said.why }
            })
            .collect(),
    }
}

/// `report retry --json`: sends what waits now. It ends with 0 when
/// reportd answered, whatever became of each report.
pub(super) fn retry(client: &Client) -> Result<i32, Fail> {
    let tried: Vec<Sent> =
        client.list(&Request { ask: "retry".into(), ..Request::default() }, client::ONLINE)?;
    print_json(&tried_json(tried))
}

#[cfg(test)]
mod tests {
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::net::UnixListener;
    use std::path::PathBuf;
    use std::thread::{self, JoinHandle};

    use super::*;

    /// A reportd that answers one request with `lines`, then closes, and
    /// the request it was sent.
    fn scripted(name: &str, lines: Vec<String>) -> (PathBuf, JoinHandle<String>) {
        let dir = std::env::temp_dir().join(format!("report-forms-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let socket = dir.join("socket");
        let listener = UnixListener::bind(&socket).unwrap();
        let served = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = String::new();
            BufReader::new(&stream).read_line(&mut request).unwrap();
            for line in lines {
                stream.write_all(format!("{line}\n").as_bytes()).unwrap();
            }
            request
        });
        (socket, served)
    }

    fn given(about: &str) -> Given {
        Given { about: Some(about.into()), happened: Some("It stopped".into()), ..Given::default() }
    }

    fn refused(given: Given) -> String {
        let error = DraftArgs::of(given).unwrap_err();
        assert!(error.usage, "{}", error.message);
        error.message
    }

    #[test]
    fn a_draft_names_what_it_is_about_and_what_goes_with_that() {
        let draft = DraftArgs::of(given("hardware")).unwrap();
        assert_eq!(draft.about(|| None).unwrap(), About::Hardware);
        assert_eq!(
            DraftArgs::of(given("updates")).unwrap().about(|| None).unwrap(),
            About::Updates
        );
        // The web console's word for it, and the terminal's, are the same.
        assert_eq!(DraftArgs::of(given("update")).unwrap().about(|| None).unwrap(), About::Updates);
        assert_eq!(DraftArgs::of(given("other")).unwrap().about(|| None).unwrap(), About::Other);
        let crash = Given { crash: Some("20261113T100211-4012".into()), ..given("crash") };
        assert_eq!(
            DraftArgs::of(crash).unwrap().about(|| None).unwrap(),
            About::Crash { id: "20261113T100211-4012".into() }
        );
        let service = Given { name: Some("web".into()), ..given("service") };
        assert_eq!(
            DraftArgs::of(service).unwrap().about(|| None).unwrap(),
            About::Service { name: "web".into() }
        );
        let program = Given { name: Some("nginx".into()), ..given("program") };
        assert_eq!(
            DraftArgs::of(program).unwrap().about(|| None).unwrap(),
            About::Program { name: "nginx".into() }
        );
    }

    #[test]
    fn a_mistake_in_a_draft_says_what_is_missing_or_does_not_go() {
        assert_eq!(
            refused(Given { about: None, ..given("") }),
            "report draft needs --about: crash, service, program, failure, hardware, updates or other"
        );
        assert!(refused(given("computer")).starts_with("--about names what the report is about"));
        assert_eq!(refused(given("crash")), "--about crash needs --crash ID");
        assert_eq!(refused(given("service")), "--about service needs --name NAME");
        assert_eq!(refused(given("program")), "--about program needs --name NAME");
        assert_eq!(
            refused(Given { crash: Some("x".into()), ..given("other") }),
            "--crash goes with --about crash"
        );
        assert_eq!(
            refused(Given { name: Some("x".into()), ..given("hardware") }),
            "--name goes with --about service or program"
        );
        assert_eq!(
            refused(Given { happened: None, ..given("other") }),
            "report draft needs --happened: what happened"
        );
        assert_eq!(
            refused(Given { happened: Some("  \n".into()), ..given("other") }),
            "report draft needs --happened: what happened"
        );
        let long = "x".repeat(ANSWER_MAX + 1);
        assert_eq!(
            refused(Given { again: Some(long.clone()), ..given("other") }),
            "--again is at most 2000 characters"
        );
        assert_eq!(
            refused(Given { happened: Some(long), ..given("other") }),
            "--happened is at most 2000 characters"
        );
        // The longest there may be is not too long, and counts characters.
        let most = "é".repeat(ANSWER_MAX);
        assert!(DraftArgs::of(Given { expected: Some(most), ..given("other") }).is_ok());
    }

    #[test]
    fn a_failed_start_needs_no_answer_and_needs_a_start_that_failed() {
        let bare = Given { about: Some("failure".into()), ..Given::default() };
        let draft = DraftArgs::of(bare).unwrap();
        assert_eq!(draft.happened, "");
        let about = draft.about(|| Some(vec!["Start 2 failed".into()])).unwrap();
        assert_eq!(about, About::Failure { notice: vec!["Start 2 failed".into()] });
        let error = draft.about(|| None).unwrap_err();
        assert_eq!(error.message, "no start has failed since this one began");
        assert!(!error.usage);
    }

    #[test]
    fn the_draft_request_keeps_the_draft_with_the_answers_as_typed() {
        let given = Given {
            expected: Some("It runs".into()),
            again: Some("Start it\nTwice".into()),
            without_logs: true,
            ..given("hardware")
        };
        let draft = DraftArgs::of(given).unwrap();
        let request = draft.request(About::Hardware);
        assert_eq!(
            serde_json::to_string(&request).unwrap(),
            r#"{"ask":"draft","about":{"kind":"hardware"},"answers":{"happened":"It stopped","expected":"It runs","again":"Start it\nTwice"},"without_logs":true,"keep":true}"#
        );
    }

    fn drafted_line(text: &str, more: &str) -> String {
        format!(
            r#"{{"text":{},"cuts":3,"has_logs":true,"cut_by":{{"user":1,"ipv4":2,"you":3}},"draft":"0123456789abcdef0123456789abcdef","sha256":"{}","parts":[{{"title":"What this computer says of itself","bytes":7}}],"sections":[{{"id":"head","title":"The head","first_line":0,"lines":2}},{{"id":"happened","title":"What happened","first_line":3,"lines":1}}],"typed":[{{"id":"happened","title":"What happened","first_line":3,"lines":1}}],"cut_by_you":{{"lines":2,"parts":1}}{more}}}"#,
            serde_json::to_string(text).unwrap(),
            "ab".repeat(32)
        )
    }

    #[test]
    fn a_kept_draft_is_said_with_its_id_its_hash_its_size_its_marks_and_its_places() {
        let (socket, served) = scripted("draft", vec![drafted_line("Dukovo é\n", "")]);
        let draft = DraftArgs::of(given("hardware")).unwrap();
        let request = draft.request(About::Hardware);
        let drafted: Drafted = Client::at(&socket).one(&request, client::QUICK).unwrap();
        served.join().unwrap();
        let said = draft_json(drafted).unwrap();
        assert_eq!(said.bytes, "Dukovo é\n".len());
        assert_eq!(said.draft, "0123456789abcdef0123456789abcdef");
        assert_eq!((said.cut_by.user, said.cut_by.ipv4, said.cut_by.you), (1, 2, 3));
        let text = serde_json::to_string(&said).unwrap();
        // The members come in the order report(1) lists them.
        let order = [
            "\"format\":1",
            "\"draft\"",
            "\"sha256\"",
            "\"bytes\"",
            "\"text\"",
            "\"cuts\"",
            "\"has_logs\"",
            "\"cut_by\"",
            "\"parts\"",
            "\"sections\"",
            "\"typed\"",
            "\"cut_by_you\"",
        ];
        let at: Vec<usize> = order.iter().map(|word| text.find(word).unwrap()).collect();
        assert!(at.windows(2).all(|pair| pair[0] < pair[1]), "{text}");
        let value: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(value["cut_by"]["you"], 3);
        assert_eq!(value["cut_by_you"], serde_json::json!({"lines": 2, "parts": 1}));
        assert_eq!(value["sections"][1]["first_line"], 3);
        assert_eq!(value["typed"][0]["id"], "happened");
        assert_eq!(value["parts"][0]["bytes"], 7);
    }

    #[test]
    fn a_draft_reportd_did_not_keep_is_an_error() {
        let drafted = Drafted { text: "x".into(), ..Drafted::default() };
        let error = draft_json(drafted).unwrap_err();
        assert!(error.message.starts_with("reportd did not keep the draft"), "{}", error.message);
    }

    #[test]
    fn what_a_send_came_to_is_sent_with_its_number_or_queued_with_why() {
        let sent = |number, waiting: Option<&str>| Sent {
            id: "a1b2".into(),
            number,
            waiting: waiting.map(String::from),
        };
        assert_eq!(
            serde_json::to_string(&sent_json(sent(Some(7), None))).unwrap(),
            r#"{"format":1,"number":7,"state":"sent","why":null}"#
        );
        assert_eq!(
            serde_json::to_string(&sent_json(sent(None, Some("the report desk did not answer"))))
                .unwrap(),
            r#"{"format":1,"number":null,"state":"queued","why":"the report desk did not answer"}"#
        );
    }

    #[test]
    fn a_send_names_the_kept_draft_and_the_title_and_the_security_box() {
        let args = SendArgs {
            draft: "0123456789abcdef0123456789abcdef".into(),
            sha256: "ab".repeat(32),
            title: "php-fpm stopped".into(),
            crash: Some("20261113T100211-4012".into()),
            security: true,
        };
        let (socket, served) =
            scripted("send", vec![r#"{"id":"a1b2c3d4","number":7}"#.to_string()]);
        let sent: Sent = Client::at(&socket).one(&args.request(), client::ONLINE).unwrap();
        assert_eq!(sent.number, Some(7));
        assert_eq!(
            served.join().unwrap(),
            format!(
                "{{\"ask\":\"send\",\"draft\":\"0123456789abcdef0123456789abcdef\",\"sha256\":\"{}\",\"title\":\"php-fpm stopped\",\"security\":true,\"crash\":\"20261113T100211-4012\"}}\n",
                "ab".repeat(32)
            )
        );
    }

    #[test]
    fn the_edits_are_read_whole_and_a_list_that_is_none_or_too_long_is_a_mistake() {
        let edits = |text: &str| read_edits(&mut text.as_bytes());
        let ok = edits(
            r#"{"cuts":[{"section":"log","line":41,"through":57},{"section":"crash","line":3,"from":12,"to":40}],"answers":{"happened":"a","expected":"b","again":"c"}}"#,
        )
        .unwrap();
        assert_eq!(ok.cuts.len(), 2);
        assert_eq!(ok.cuts[0].through, Some(57));
        assert_eq!((ok.cuts[1].from, ok.cuts[1].to), (Some(12), Some(40)));
        assert_eq!(ok.answers.unwrap().again, "c");
        // None cut is undo all; answers left out keep the answers.
        let none = edits(r#"{"cuts":[]}"#).unwrap();
        assert_eq!((none.cuts.len(), none.answers), (0, None));
        for wrong in ["", "{", "[]", r#"{"cut":[]}"#, r#"{"cuts":[{"section":"log"}]}"#, "null"] {
            let error = edits(wrong).unwrap_err();
            assert!(error.usage, "{wrong}");
            assert!(error.message.starts_with("the edits are not a list of edits"), "{wrong}");
        }
        let long = format!(r#"{{"cuts":[],"answers":{{"happened":"{}"}}}}"#, "x".repeat(EDITS_MAX));
        let error = edits(&long).unwrap_err();
        assert_eq!(error.message, "the edits are longer than 64 KiB");
        // The most there may be is taken.
        let most = format!(r#"{{"cuts":[],"answers":{{"happened":"{}"}}}}"#, "x".repeat(2000));
        assert!(edits(&most).is_ok());
    }

    #[test]
    fn a_redaction_sends_the_edits_to_the_kept_draft_and_says_what_it_is_now() {
        let (socket, served) = scripted("redact", vec![drafted_line("Dukovo\n", "")]);
        let mut input = r#"{"cuts":[{"section":"log","line":0}]}"#.as_bytes();
        let edits = read_edits(&mut input).unwrap();
        let request = Request {
            ask: "redact".into(),
            draft: Some("0123456789abcdef0123456789abcdef".into()),
            sha256: Some("cd".repeat(32)),
            edits: Some(edits),
            ..Request::default()
        };
        let drafted: Drafted = Client::at(&socket).one(&request, client::QUICK).unwrap();
        assert!(draft_json(drafted).is_ok());
        assert_eq!(
            served.join().unwrap(),
            format!(
                "{{\"ask\":\"redact\",\"draft\":\"0123456789abcdef0123456789abcdef\",\"sha256\":\"{}\",\"edits\":{{\"cuts\":[{{\"section\":\"log\",\"line\":0}}]}}}}\n",
                "cd".repeat(32)
            )
        );
    }

    #[test]
    fn a_draft_reportd_has_not_or_a_text_that_is_not_the_one_read_is_reportds_refusal() {
        let (socket, served) = scripted(
            "refused",
            vec![r#"{"error":"That draft has ended: draft it again (RPT-101)"}"#.to_string()],
        );
        let mut input = r#"{"cuts":[]}"#.as_bytes();
        let error = redact(&Client::at(&socket), "x", "y", &mut input).unwrap_err();
        assert_eq!(error.message, "That draft has ended: draft it again (RPT-101)");
        assert_eq!(error.code, 1);
        served.join().unwrap();
    }

    fn crash(id: &str, reported: Option<u64>) -> Crash {
        Crash {
            id: id.into(),
            kind: Kind::Signal,
            program: "php-fpm".into(),
            version: Some("8.3".into()),
            service: None,
            user: 33,
            at: 1_794_564_000,
            what: "SIGSEGV in ngx_http_core_run_phases".into(),
            signature: "3fa2c4d19e0b7a61".into(),
            seen: 2,
            reported,
        }
    }

    #[test]
    fn the_crashes_are_said_newest_first_with_the_time_in_milliseconds() {
        let said = crashes_json(vec![
            crash("20261113T100211-4012", None),
            crash("20261112T090000-7", Some(5)),
        ]);
        let value = serde_json::to_value(&said).unwrap();
        assert_eq!(value["format"], 1);
        assert_eq!(
            value["crashes"][0],
            serde_json::json!({
                "id": "20261113T100211-4012", "kind": "signal", "program": "php-fpm",
                "version": "8.3", "service": null, "at_ms": 1_794_564_000_000u64,
                "what": "SIGSEGV in ngx_http_core_run_phases", "seen": 2, "reported": null
            })
        );
        assert_eq!(value["crashes"][1]["reported"], 5);
        assert_eq!(
            serde_json::to_string(&crashes_json(Vec::new())).unwrap(),
            r#"{"format":1,"crashes":[]}"#
        );
    }

    fn waiting() -> Waiting {
        Waiting {
            id: "a1b2c3d4".into(),
            title: "Disk light".into(),
            since: 100,
            tries: 4,
            error: Some("no route to host".into()),
            next: 200,
            security: false,
        }
    }

    fn sent(number: u64, at: u64) -> Status {
        Status {
            number,
            title: format!("Report {number}"),
            sent: at,
            status: Some("duplicate".into()),
            duplicate_of: Some(3),
            fixed_in: None,
            reason: None,
            error: None,
            security: true,
        }
    }

    #[test]
    fn the_list_has_what_waits_first_and_what_was_sent_after_it() {
        let said = list_json(vec![waiting()], vec![sent(9, 500), sent(7, 300)]);
        let value = serde_json::to_value(&said).unwrap();
        assert_eq!(
            value["reports"][0],
            serde_json::json!({
                "number": null, "title": "Disk light", "state": "queued", "queued_ms": 100_000,
                "sent_ms": null, "tries": 4, "next_ms": 200_000, "receipt": "a1b2c3d4",
                "security": false, "why": "no route to host"
            })
        );
        assert_eq!(
            value["reports"][1],
            serde_json::json!({
                "number": 9, "title": "Report 9", "state": "sent", "queued_ms": null,
                "sent_ms": 500_000, "tries": null, "next_ms": null, "receipt": null,
                "security": true, "why": null
            })
        );
        assert_eq!(value["reports"][2]["number"], 7);
        assert_eq!(
            serde_json::to_string(&list_json(Vec::new(), Vec::new())).unwrap(),
            r#"{"format":1,"reports":[]}"#
        );
    }

    #[test]
    fn a_status_is_the_words_where_the_server_answered_and_unreachable_where_it_did_not() {
        let words: fn(&Status) -> String = |report| format!("words of {}", report.number);
        let mut lost = sent(8, 400);
        lost.status = None;
        lost.error = Some("the reports server did not answer".into());
        let said = status_json(vec![sent(9, 500), lost], words);
        let value = serde_json::to_value(&said).unwrap();
        assert_eq!(
            value["reports"][0],
            serde_json::json!({
                "number": 9, "title": "Report 9", "sent_ms": 500_000, "receipt": null,
                "status": "words of 9", "unreachable": false, "why": null
            })
        );
        assert_eq!(
            value["reports"][1],
            serde_json::json!({
                "number": 8, "title": "Report 8", "sent_ms": 400_000, "receipt": null,
                "status": null, "unreachable": true, "why": "the reports server did not answer"
            })
        );
    }

    #[test]
    fn a_retry_says_for_each_report_it_tried_whether_it_went() {
        let said = tried_json(vec![
            Sent { id: "a1".into(), number: Some(11), waiting: None },
            Sent { id: "b2".into(), number: None, waiting: Some("no network".into()) },
        ]);
        let value = serde_json::to_value(&said).unwrap();
        assert_eq!(
            value["reports"],
            serde_json::json!([
                {"receipt": "a1", "number": 11, "state": "sent", "why": null},
                {"receipt": "b2", "number": null, "state": "queued", "why": "no network"}
            ])
        );
        assert_eq!(
            serde_json::to_string(&tried_json(Vec::new())).unwrap(),
            r#"{"format":1,"reports":[]}"#
        );
    }
}
