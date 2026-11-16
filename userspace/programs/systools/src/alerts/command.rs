// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! alerts(8): shows the alerts alertd keeps, sends a test through every
//! channel, sets the channels up, and raises and clears an alert from a
//! script (ADR 0485).
//!
//! `alerts` lists what is active, critical first; `history` shows the
//! newest changes; `test` sends a test alert through every channel and
//! says how each went, in the words of the server that refused it; `status`
//! shows each channel's last success, last error and waiting messages.
//! `raise`, `event` and `clear` are for root's scripts: their source is
//! the script's resource domain, as alertd sees it. `email`, `webhook`,
//! `push` and `channel` set up where alerts go and which kinds of alert go
//! there ([`super::channels`] and [`super::browsers`]).
//!
//! Every subcommand needs root, as alertd refuses anyone else, and says to
//! use `elevate`.

use std::ffi::OsString;
use std::io::{self, BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::time::Duration;

use super::browsers::{self, Push};
use super::channels::{self, Email, Told, Webhook};
use super::client;
use super::protocol::{
    About, Active, Alert, Change, Channel, Delivery, Request, SOCKET, Severity, State,
};
use crate::alertd::text;
use crate::args::{Arg, Args};
use crate::deadline::Deadline;
use crate::ps::print_table;
use crate::util::{self, Fail, fail, usage};

const HELP: &str = "Usage: alerts [list] [--json]
       alerts history [-n COUNT] [--json]
       alerts test [--json]
       alerts status [--json]
       alerts raise KEY --title TEXT [--detail TEXT] [--critical] [--about WORD]
       alerts event KEY --title TEXT [--detail TEXT] [--critical] [--about WORD]
       alerts clear KEY
       alerts email set --to ADDRESS --server HOST [--port 587|465] [--from ADDRESS]
                        [--user NAME (--password - | --keep-password)]
                        [--severity S] [--cleared yes|no] [--about WORDS]
       alerts email remove
       alerts webhook add KIND [--room ID] [--severity S] [--cleared yes|no]
                               [--about WORDS]
       alerts webhook remove NUMBER
       alerts push key [--json]
       alerts push add --name NAME [--severity S] [--cleared yes|no] [--about WORDS]
       alerts push remove ID
       alerts channel CHANNEL --about WORDS

Shows the alerts that alertd(8) keeps, as alerts(8) describes. Every
command needs root; try elevate alerts.

  list     the active alerts, critical first (the default)
  history  the newest changes, 20 unless -n says another number
  test     sends a test alert to every channel and says how each went; it
           exits 1 if any failed
  status   each channel's last success, last error and waiting messages
  raise    raises an alert, as a root script does; --critical for one
           that costs data, --about for what it is about: data, stopped
           or hardware, which decides the channels it goes to
  event    tells of something once, with no alert left active
  clear    clears an alert that raise made
  email    sets up the email channel in alerts.toml, or takes it away; the
           password, with --password -, is the first line of the standard
           input
  webhook  adds a webhook of KIND (ntfy, matrix, slack, discord or json),
           whose address and then token are the first two lines of the
           standard input, or removes the one NUMBER says
  push     keys, adds and removes the browsers that get alerts: key gives
           the machine's public key, add reads the browser's subscription,
           JSON, from the standard input
  channel  says which kinds of alert a channel is told of: CHANNEL is email,
           webhook:NUMBER or push:ID, as status gives them, and WORDS is
           all, or data, stopped and hardware separated by commas
  --json   prints what alertd answered, a line of JSON each; test prints
           the deliveries in one object

An email, webhook or channel change is checked as alertd reads the file,
written with the file's comments kept, and alertd is asked to read it again.";

/// How long `list`, `history` and `status` wait for alertd, and `test`,
/// which waits for each channel in turn up to 30 seconds.
pub(super) const WAIT: Duration = Duration::from_secs(10);
const TEST_WAIT: Duration = Duration::from_secs(40);

/// How many changes `history` shows without `-n`.
const HISTORY_DEFAULT: usize = 20;

/// The alerts that are active now, as `alerts list` shows them, for a
/// caller that is root.
pub(crate) fn active() -> Result<Vec<Active>, Fail> {
    items(&request("list"), WAIT, false)
}

/// Where the channels stand now, as `alerts status` shows them, for a
/// caller that is root.
pub(crate) fn channels() -> Result<Vec<Channel>, Fail> {
    items(&request("status"), WAIT, false)
}

/// What the command line asks for.
#[derive(Debug, PartialEq, Eq)]
enum Command {
    List { json: bool },
    History { count: usize, json: bool },
    Test { json: bool },
    Status { json: bool },
    Raise(Alert),
    Event(Alert),
    Clear(String),
    Email(Email),
    Webhook(Webhook),
    Push(Push),
    Channel(Told),
}

/// alerts(8).
pub fn main(args: &mut Args) -> Result<i32, Fail> {
    let command = match parse(args)? {
        Parsed::Help => {
            println!("{HELP}");
            return Ok(0);
        }
        Parsed::Run(command) => command,
    };
    root_only()?;
    match command {
        Command::List { json } => list(json),
        Command::History { count, json } => history(count, json),
        Command::Test { json } => test(json),
        Command::Status { json } => status(json),
        Command::Raise(alert) => client::raise(&alert).map(|()| 0).map_err(fail),
        Command::Event(alert) => {
            client::event(&alert, client::Screen::Tell).map(|()| 0).map_err(fail)
        }
        Command::Clear(key) => client::clear(&key).map(|()| 0).map_err(fail),
        Command::Email(email) => channels::run_email(&email),
        Command::Webhook(webhook) => channels::run_webhook(&webhook),
        Command::Push(push) => browsers::run(&push),
        Command::Channel(told) => channels::run_channel(&told),
    }
}

/// Fails unless the program runs as root, whose alerts these are.
fn root_only() -> Result<(), Fail> {
    if util::own_user() == 0 {
        Ok(())
    } else {
        Err(fail("only root reads and raises alerts; try elevate alerts".into()))
    }
}

#[derive(Debug, PartialEq, Eq)]
enum Parsed {
    Help,
    Run(Command),
}

/// What the command line says.
fn parse(args: &mut Args) -> Result<Parsed, Fail> {
    let Some(name) = args.subcommand() else {
        return match args.next()? {
            Some(Arg::Long(name)) if name == "help" => Ok(Parsed::Help),
            Some(Arg::Long(name)) if name == "json" => {
                nothing_more(args)?;
                Ok(Parsed::Run(Command::List { json: true }))
            }
            Some(other) => Err(args.unknown(&other)),
            None => Ok(Parsed::Run(Command::List { json: false })),
        };
    };
    let command = match name.to_str() {
        Some("list") => Command::List { json: json_only(args)? },
        Some("history") => history_options(args)?,
        Some("test") => Command::Test { json: json_only(args)? },
        Some("status") => Command::Status { json: json_only(args)? },
        Some("raise") => Command::Raise(alert_options(args)?),
        Some("event") => Command::Event(alert_options(args)?),
        Some("clear") => Command::Clear(key_operand(args)?),
        Some("email") => Command::Email(channels::parse_email(args)?),
        Some("webhook") => Command::Webhook(channels::parse_webhook(args)?),
        Some("push") => Command::Push(browsers::parse(args)?),
        Some("channel") => Command::Channel(channels::parse_channel(args)?),
        _ => return Err(usage(format!("no such command: {}\n{HELP}", name.to_string_lossy()))),
    };
    Ok(Parsed::Run(command))
}

/// Fails if anything is left in `args`.
pub(super) fn nothing_more(args: &mut Args) -> Result<(), Fail> {
    match args.next()? {
        None => Ok(()),
        Some(other) => Err(args.unknown(&other)),
    }
}

/// Whether `--json` is the only option left.
pub(super) fn json_only(args: &mut Args) -> Result<bool, Fail> {
    let mut json = false;
    while let Some(arg) = args.next()? {
        match arg {
            Arg::Long(name) if name == "json" => json = true,
            other => return Err(args.unknown(&other)),
        }
    }
    Ok(json)
}

fn history_options(args: &mut Args) -> Result<Command, Fail> {
    let (mut count, mut json) = (HISTORY_DEFAULT, false);
    while let Some(arg) = args.next()? {
        match arg {
            Arg::Short('n') => count = args.number()?,
            Arg::Long(name) if name == "count" => count = args.number()?,
            Arg::Long(name) if name == "json" => json = true,
            other => return Err(args.unknown(&other)),
        }
    }
    Ok(Command::History { count, json })
}

/// The key, the title, the detail and the severity of `raise` and
/// `event`.
fn alert_options(args: &mut Args) -> Result<Alert, Fail> {
    let (mut key, mut title, mut detail) = (None, None, String::new());
    let (mut severity, mut about) = (Severity::Warning, None);
    while let Some(arg) = args.next()? {
        match arg {
            Arg::Long(name) if name == "title" => title = Some(args.text()?),
            Arg::Long(name) if name == "detail" => detail = args.text()?,
            Arg::Long(name) if name == "critical" => severity = Severity::Critical,
            Arg::Long(name) if name == "about" => about = Some(about_word(&args.text()?)?),
            Arg::Operand(operand) if key.is_none() => key = Some(text_of(operand)?),
            other => return Err(args.unknown(&other)),
        }
    }
    let key = key.ok_or_else(|| usage("a KEY is needed, such as backup-late".into()))?;
    let title = title.ok_or_else(|| usage("--title TEXT is needed".into()))?;
    let alert = Alert {
        key,
        severity,
        title,
        detail,
        about: about.map(|about: About| about.word().into()),
    };
    match alert.invalid() {
        Some(why) => Err(usage(why)),
        None => Ok(alert),
    }
}

/// The one kind of alert `word` names; a word that is none is a mistake
/// where it is made, so that a script's typo is caught there.
fn about_word(word: &str) -> Result<About, Fail> {
    About::parse(word).ok_or_else(|| {
        usage(format!(
            "'{word}' is no kind of alert; --about takes one of {}",
            super::protocol::ABOUT.join(", ")
        ))
    })
}

fn key_operand(args: &mut Args) -> Result<String, Fail> {
    let mut key = None;
    while let Some(arg) = args.next()? {
        match arg {
            Arg::Operand(operand) if key.is_none() => key = Some(text_of(operand)?),
            other => return Err(args.unknown(&other)),
        }
    }
    key.ok_or_else(|| usage("a KEY is needed".into()))
}

pub(super) fn text_of(operand: OsString) -> Result<String, Fail> {
    operand
        .into_string()
        .map_err(|operand| usage(format!("'{}' is not UTF-8", operand.to_string_lossy())))
}

/// Sends `request` to alertd and calls `each` with each line of the
/// answer, waiting at most `wait` for each.
pub(super) fn ask(
    request: &Request,
    wait: Duration,
    mut each: impl FnMut(&str) -> Result<(), Fail>,
) -> Result<(), Fail> {
    let stream = UnixStream::connect(SOCKET).map_err(|err| match err.kind() {
        io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused => {
            fail("alertd is not running; try service alertd start".into())
        }
        _ => util::failed(&SOCKET, err),
    })?;
    let to_fail = |err: io::Error| util::failed(&SOCKET, err);
    let mut timed = Deadline::each(&stream, wait).map_err(to_fail)?;
    let mut line = serde_json::to_string(request).map_err(|err| fail(err.to_string()))?;
    line.push('\n');
    timed.write_all(line.as_bytes()).map_err(to_fail)?;
    for line in BufReader::new(timed).lines() {
        let line = line.map_err(|err| match err.kind() {
            io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut => {
                fail(format!("alertd did not answer within {} seconds", wait.as_secs()))
            }
            _ => util::failed(&SOCKET, err),
        })?;
        if let Some(why) = refusal(&line) {
            return Err(fail(why));
        }
        if is_end(&line) {
            break;
        }
        each(&line)?;
    }
    Ok(())
}

/// Whether `line` is the line alertd ends a list with, which is the
/// socket's and not a command's: `--json` prints the items alone, so that
/// a list with nothing in it is no line at all.
fn is_end(line: &str) -> bool {
    let value: Option<serde_json::Value> = serde_json::from_str(line).ok();
    value.as_ref().and_then(serde_json::Value::as_object).is_some_and(|object| {
        object.len() == 1 && object.get("end").and_then(serde_json::Value::as_bool) == Some(true)
    })
}

/// What alertd refused in `line`, if it is a refusal: a line of JSON
/// with an `error` and nothing else (a failed delivery has one too).
fn refusal(line: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(line).ok()?;
    let object = value.as_object()?;
    if object.len() != 1 {
        return None;
    }
    object.get("error")?.as_str().map(str::to_string)
}

/// Asks `request` and parses each line of the answer as `T`; `json` has
/// each line printed as it came instead.
pub(super) fn items<T: serde::de::DeserializeOwned>(
    request: &Request,
    wait: Duration,
    json: bool,
) -> Result<Vec<T>, Fail> {
    let mut items = Vec::new();
    ask(request, wait, |line| {
        if json {
            println!("{line}");
        }
        if let Ok(item) = serde_json::from_str::<T>(line) {
            items.push(item);
        }
        Ok(())
    })?;
    Ok(items)
}

fn request(ask: &str) -> Request {
    Request { ask: ask.into(), ..Request::default() }
}

fn list(json: bool) -> Result<i32, Fail> {
    let active: Vec<Active> = items(&request("list"), WAIT, json)?;
    if json {
        return Ok(0);
    }
    if active.is_empty() {
        println!("No alerts are active.");
        return Ok(0);
    }
    print_table(&list_rows(&active), &[false, false, false, false], None)?;
    Ok(0)
}

/// The rows of `alerts list`, after its heading.
fn list_rows(active: &[Active]) -> Vec<Vec<String>> {
    let heading = ["SEVERITY", "SINCE (UTC)", "SOURCE", "TITLE"].map(String::from).to_vec();
    let rows = active.iter().map(|held| {
        vec![
            held.alert.severity.name().to_string(),
            text::date_time(held.since, false),
            held.source.clone(),
            held.alert.title.clone(),
        ]
    });
    std::iter::once(heading).chain(rows).collect()
}

fn history(count: usize, json: bool) -> Result<i32, Fail> {
    let request = Request { ask: "history".into(), count: Some(count), ..Request::default() };
    let changes: Vec<Change> = items(&request, WAIT, json)?;
    if json {
        return Ok(0);
    }
    if changes.is_empty() {
        println!("Nothing has happened to any alert.");
        return Ok(0);
    }
    print_table(&history_rows(&changes), &[false, false, false, false, false], None)?;
    Ok(0)
}

/// The rows of `alerts history`, after its heading.
fn history_rows(changes: &[Change]) -> Vec<Vec<String>> {
    let heading = ["WHEN (UTC)", "WHAT", "SEVERITY", "SOURCE", "TITLE"].map(String::from).to_vec();
    let rows = changes.iter().map(|change| {
        let what = match change.state {
            State::Raised => "raised",
            State::Cleared => "cleared",
            State::Event => "event",
            State::Test => "test",
        };
        vec![
            text::date_time(change.at, true),
            what.to_string(),
            change.alert.severity.name().to_string(),
            change.source.clone(),
            change.alert.title.clone(),
        ]
    });
    std::iter::once(heading).chain(rows).collect()
}

fn test(json: bool) -> Result<i32, Fail> {
    let deliveries: Vec<Delivery> = items(&request("test"), TEST_WAIT, false)?;
    if json {
        println!("{}", test_json(&deliveries));
        return Ok(i32::from(deliveries.iter().any(|delivery| !delivery.ok)));
    }
    if deliveries.is_empty() {
        println!("{NO_CHANNELS}");
        return Ok(0);
    }
    for line in test_lines(&deliveries) {
        println!("{line}");
    }
    Ok(i32::from(deliveries.iter().any(|delivery| !delivery.ok)))
}

/// What `alerts test --json` prints: every delivery in one object, with
/// the words of a failure and the ID of the page about it when there are
/// some, else null.
fn test_json(deliveries: &[Delivery]) -> String {
    let each: Vec<serde_json::Value> = deliveries
        .iter()
        .map(|delivery| {
            serde_json::json!({
                "channel": delivery.channel,
                "ok": delivery.ok,
                "error": delivery.error,
                "error_id": delivery.error_id,
            })
        })
        .collect();
    serde_json::json!({"format": 1, "deliveries": each}).to_string()
}

const NO_CHANNELS: &str = "No channels are set up; notices go to the screen. See alerts.toml(5).";

/// A line for each channel `alerts test` tried.
fn test_lines(deliveries: &[Delivery]) -> Vec<String> {
    deliveries
        .iter()
        .map(|delivery| match (&delivery.error, delivery.ok) {
            (_, true) => format!("{}: sent", delivery.channel),
            (Some(why), false) => format!("{}: failed: {why}", delivery.channel),
            (None, false) => format!("{}: failed", delivery.channel),
        })
        .collect()
}

fn status(json: bool) -> Result<i32, Fail> {
    let channels: Vec<Channel> = items(&request("status"), WAIT, json)?;
    if json {
        return Ok(0);
    }
    if channels.is_empty() {
        println!("{NO_CHANNELS}");
        return Ok(0);
    }
    for line in status_lines(&channels) {
        println!("{line}");
    }
    Ok(0)
}

/// The lines of `alerts status`: a block for each channel.
fn status_lines(channels: &[Channel]) -> Vec<String> {
    let mut lines = Vec::new();
    for channel in channels {
        lines.push(match channel.told_of() {
            Some(told) => format!("{}, told of: {told}", channel.channel),
            None => channel.channel.clone(),
        });
        let ok = channel.last_ok.map_or("never".to_string(), text::utc);
        lines.push(format!("  last sent   {ok}"));
        if let Some((at, why)) = &channel.last_error {
            lines.push(format!("  last failed {}: {why}", text::utc(*at)));
        }
        let plural = if channel.waiting == 1 { "" } else { "s" };
        lines.push(format!("  waiting     {} message{plural}", channel.waiting));
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parsed(words: &[&str]) -> Result<Parsed, Fail> {
        let items = words.iter().map(OsString::from).collect();
        parse(&mut Args::new("alerts", items))
    }

    fn command(words: &[&str]) -> Command {
        match parsed(words) {
            Ok(Parsed::Run(command)) => command,
            other => panic!("{words:?} gave {other:?}"),
        }
    }

    fn alert(key: &str, severity: Severity, title: &str, detail: &str) -> Alert {
        Alert { key: key.into(), severity, title: title.into(), detail: detail.into(), about: None }
    }

    /// A channel of `kind` that has taken and failed nothing.
    fn standing(name: &str, kind: &str) -> Channel {
        Channel {
            channel: name.into(),
            last_ok: None,
            last_error: None,
            waiting: 0,
            kind: kind.into(),
            host: None,
            severity: Severity::Warning,
            cleared: true,
            number: None,
            id: None,
            name: None,
            error_id: None,
            about: None,
        }
    }

    #[test]
    fn no_words_and_list_show_the_active_alerts() {
        assert_eq!(command(&[]), Command::List { json: false });
        assert_eq!(command(&["list"]), Command::List { json: false });
        assert_eq!(command(&["--json"]), Command::List { json: true });
        assert_eq!(command(&["list", "--json"]), Command::List { json: true });
    }

    #[test]
    fn history_takes_a_count_and_has_twenty_without() {
        assert_eq!(command(&["history"]), Command::History { count: 20, json: false });
        assert_eq!(command(&["history", "-n", "5"]), Command::History { count: 5, json: false });
        assert_eq!(
            command(&["history", "-n7", "--json"]),
            Command::History { count: 7, json: true }
        );
        assert!(parsed(&["history", "-n", "many"]).is_err());
    }

    #[test]
    fn raise_and_event_take_a_key_a_title_a_detail_and_critical() {
        let wanted = alert("backup-late", Severity::Critical, "Backup is late", "Run backup now.");
        let words = [
            "raise",
            "backup-late",
            "--title",
            "Backup is late",
            "--detail",
            "Run backup now.",
            "--critical",
        ];
        assert_eq!(command(&words), Command::Raise(wanted.clone()));
        let words = [
            "event",
            "--critical",
            "--title=Backup is late",
            "backup-late",
            "--detail=Run backup now.",
        ];
        assert_eq!(command(&words), Command::Event(wanted));
        let plain = command(&["raise", "k", "--title", "t"]);
        assert_eq!(plain, Command::Raise(alert("k", Severity::Warning, "t", "")));
    }

    #[test]
    fn an_alert_alertd_would_refuse_is_refused_before_it_is_sent() {
        let says = |words: &[&str]| parsed(words).unwrap_err().message;
        assert_eq!(says(&["raise", "--title", "t"]), "a KEY is needed, such as backup-late");
        assert_eq!(says(&["raise", "k"]), "--title TEXT is needed");
        assert!(says(&["raise", "Upper", "--title", "t"]).contains("is not 1 to 128"));
        assert!(says(&["raise", "k", "--title", "two\nlines"]).contains("not one line"));
        assert!(says(&["raise", "k", "k2", "--title", "t"]).contains("unexpected argument"));
    }

    #[test]
    fn clear_takes_a_key_and_test_and_status_take_nothing() {
        assert_eq!(command(&["clear", "k"]), Command::Clear("k".into()));
        assert!(parsed(&["clear"]).is_err());
        assert!(parsed(&["clear", "a", "b"]).is_err());
        assert_eq!(command(&["test"]), Command::Test { json: false });
        assert_eq!(command(&["test", "--json"]), Command::Test { json: true });
        assert!(parsed(&["test", "now"]).is_err());
        assert_eq!(command(&["status", "--json"]), Command::Status { json: true });
    }

    #[test]
    fn the_channel_commands_are_read_by_their_own_modules() {
        assert!(matches!(command(&["email", "remove"]), Command::Email(Email::Remove)));
        assert!(matches!(
            command(&["webhook", "remove", "2"]),
            Command::Webhook(Webhook::Remove(2))
        ));
        assert!(matches!(command(&["push", "key"]), Command::Push(Push::Key { json: false })));
        assert!(matches!(command(&["channel", "email", "--about", "all"]), Command::Channel(_)));
        // Each is a mistake in the command line, whose status is 2.
        for words in [
            &["email"][..],
            &["email", "set"],
            &["webhook", "add"],
            &["webhook", "remove", "x"],
            &["push", "add"],
            &["channel", "email"],
            &["channel", "sms", "--about", "all"],
        ] {
            assert!(parsed(words).unwrap_err().usage, "{words:?}");
        }
    }

    #[test]
    fn a_test_as_json_is_one_object_with_a_member_for_each_failure_that_is_null_when_there_is_none()
    {
        let deliveries = [
            Delivery {
                channel: "email to ops@example.com".into(),
                ok: true,
                error: None,
                error_id: None,
            },
            Delivery {
                channel: "ntfy (ntfy.sh)".into(),
                ok: false,
                error: Some("ntfy answered 403 Forbidden: no".into()),
                error_id: Some("ALR-103".into()),
            },
        ];
        assert_eq!(
            test_json(&deliveries),
            r#"{"deliveries":[{"channel":"email to ops@example.com","error":null,"error_id":null,"ok":true},{"channel":"ntfy (ntfy.sh)","error":"ntfy answered 403 Forbidden: no","error_id":"ALR-103","ok":false}],"format":1}"#
        );
        assert_eq!(test_json(&[]), r#"{"deliveries":[],"format":1}"#);
    }

    #[test]
    fn help_and_an_unknown_command_are_said() {
        assert_eq!(parsed(&["--help"]).unwrap(), Parsed::Help);
        assert!(
            parsed(&["frobnicate"]).unwrap_err().message.starts_with("no such command: frobnicate")
        );
        assert!(parsed(&["--nope"]).is_err());
    }

    #[test]
    fn a_refusal_is_a_line_with_an_error_and_nothing_else() {
        assert_eq!(
            refusal(r#"{"error":"only root and the system's services raise and read alerts"}"#),
            Some("only root and the system's services raise and read alerts".into())
        );
        // A failed delivery has an error and more.
        assert_eq!(refusal(r#"{"channel":"a","ok":false,"error":"x"}"#), None);
        assert_eq!(refusal(r#"{"ok":true}"#), None);
        assert_eq!(refusal("not json"), None);
    }

    #[test]
    fn the_list_has_a_heading_and_a_row_each() {
        let active = [Active {
            source: "metricsd".into(),
            alert: alert("fs-full:/data", Severity::Critical, "/data is 97% full", ""),
            since: 1_763_030_531,
            changed: 1_763_030_531,
        }];
        assert_eq!(
            list_rows(&active),
            [
                vec!["SEVERITY", "SINCE (UTC)", "SOURCE", "TITLE"],
                vec!["critical", "2025-11-13 10:42", "metricsd", "/data is 97% full"],
            ]
        );
    }

    #[test]
    fn the_history_has_the_time_with_seconds_and_what_happened() {
        let changes = [Change {
            at: 1_763_030_531,
            source: "backup".into(),
            state: State::Cleared,
            alert: alert("k", Severity::Warning, "Backup is late", ""),
        }];
        assert_eq!(
            history_rows(&changes)[1],
            ["2025-11-13 10:42:11", "cleared", "warning", "backup", "Backup is late"]
        );
    }

    #[test]
    fn a_test_says_sent_or_failed_with_the_servers_words() {
        let deliveries = [
            Delivery {
                channel: "email to ops@example.com".into(),
                ok: true,
                error: None,
                error_id: None,
            },
            Delivery {
                channel: "ntfy (ntfy.sh)".into(),
                ok: false,
                error: Some("ntfy answered 403 Forbidden: no".into()),
                error_id: Some("ALR-103".into()),
            },
            Delivery {
                channel: "json (hooks.test)".into(),
                ok: false,
                error: None,
                error_id: None,
            },
        ];
        assert_eq!(
            test_lines(&deliveries),
            [
                "email to ops@example.com: sent",
                "ntfy (ntfy.sh): failed: ntfy answered 403 Forbidden: no",
                "json (hooks.test): failed",
            ]
        );
    }

    #[test]
    fn the_status_gives_the_last_success_the_last_error_and_the_waiting() {
        let channels = [
            Channel {
                last_ok: Some(1_763_030_531),
                ..standing("email to ops@example.com", "email")
            },
            Channel {
                last_error: Some((1_763_030_600, "cannot reach ntfy.sh: refused".into())),
                waiting: 1,
                ..standing("ntfy (ntfy.sh)", "ntfy")
            },
        ];
        assert_eq!(
            status_lines(&channels),
            [
                "email to ops@example.com",
                "  last sent   2025-11-13 10:42:11 UTC",
                "  waiting     0 messages",
                "ntfy (ntfy.sh)",
                "  last sent   never",
                "  last failed 2025-11-13 10:43:20 UTC: cannot reach ntfy.sh: refused",
                "  waiting     1 message",
            ]
        );
    }

    #[test]
    fn the_socket_s_end_line_is_no_item_and_ends_the_answer() {
        assert!(is_end(r#"{"end":true}"#));
        assert!(!is_end(r#"{"end":false}"#));
        assert!(!is_end(r#"{"kind":"email","end":true}"#), "an item that holds an end is an item");
        assert!(!is_end(r#"{"key":"disk-health:vda","severity":"warning"}"#));
        assert!(!is_end("not json"));
    }
}
