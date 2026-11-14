// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! report(1): writes a problem report with the person, shows the exact
//! text that would leave this computer, and sends, saves or hands it to a
//! phone as they choose; and follows up on the reports they sent (ADRs
//! 0531 and 0532).
//!
//! `report` asks what the problem is with and three questions, `report
//! crash` and `report failure` start from a crash or the failed start the
//! sign-in notice tells of. reportd(8) drafts the text and cuts out what
//! names a person or a machine ([`super::draft`], [`super::redact`]),
//! [`super::preview`] shows it, and one key decides. Nothing is sent
//! before `S`; nothing leaves this computer before a key or `--save`
//! says so. `list`, `status`, `delete`, `forget` and `retry` ask reportd
//! about the crashes it keeps and the reports it sent or queued.
//!
//! The command makes no report of its own: reportd drafts it, as root,
//! and sends the bytes the person saw. When it says reportd is not
//! running, an administrator starts it with `elevate service reportd start`.

use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;
use std::path::PathBuf;

use super::client::{self, Client};
use super::preview::{self, Choice};
use super::protocol::{About, Answers, Crash, Drafted, Request, Sent, Status, Waiting, title_of};
use super::save::{self, Outcome};
use crate::alertd::text::date_time;
use crate::args::{Arg, Args};
use crate::prompt::{self, Answer};
use crate::ps::print_table;
use crate::util::{self, Fail, fail, usage};

const HELP: &str = "Usage: report [--security] [--save [FILE]] [--port PORT]
       report crash [ID] [--security] [--save [FILE]] [--port PORT]
       report failure [--security] [--save [FILE]] [--port PORT]
       report list
       report status [N]
       report delete N
       report forget ID
       report retry

Writes a problem report, shows the exact text that would leave this
computer, and sends it, saves it or hands it to a phone, as report(1)
describes. Nothing is sent before you press S.

  (none)        asks what the problem is with, then three questions
  crash [ID]    reports the newest crash not reported yet, or crash ID
  failure       reports the failed start the sign-in notice tells of; it
                needs an administrator
  list          the crashes kept, the reports waiting and the reports sent
  status [N]    where report N stands, or every report sent
  delete N      deletes report N, here and on the reports server
  forget ID     puts crash ID away, so that it is not offered again
  retry         sends the reports that wait now
  --security    sends it to the security team only, encrypted
  --save [FILE] saves it to FILE, or serves it once to a phone with no FILE,
                instead of sending it
  --port PORT   the port that download listens on, so that a firewall rule
                can be made for it first";

/// The words that start a subcommand, which `--save` does not take for a
/// file's name.
const SUBCOMMANDS: [&str; 7] = ["crash", "failure", "list", "status", "delete", "forget", "retry"];

/// What the person asked for.
#[derive(Debug, PartialEq, Eq)]
enum Command {
    /// A report that asks what it is about.
    Report,
    /// A crash, by its ID, or the newest not yet reported.
    Crash(Option<String>),
    /// The failed start.
    Failure,
    List,
    Status(Option<u64>),
    Delete(u64),
    Forget(String),
    Retry,
}

impl Command {
    /// Whether it writes a report, as the options `--security`, `--save`
    /// and `--port` are about.
    fn writes(&self) -> bool {
        matches!(self, Command::Report | Command::Crash(_) | Command::Failure)
    }
}

/// Where `--save` puts a report that is not sent.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Save {
    /// In a file.
    File(PathBuf),
    /// Once, to a phone.
    Phone,
}

/// A command line, understood.
#[derive(Debug, PartialEq, Eq)]
struct Run {
    command: Command,
    security: bool,
    save: Option<Save>,
    port: Option<u16>,
}

#[derive(Debug, PartialEq, Eq)]
enum Parsed {
    Help,
    Run(Run),
}

/// report(1).
pub fn main(args: &mut Args) -> Result<i32, Fail> {
    match parse(args)? {
        Parsed::Help => {
            println!("{HELP}");
            Ok(0)
        }
        Parsed::Run(run) => execute(&Client::new(), &run),
    }
}

/// Whether `word` names a subcommand.
fn is_subcommand(word: &OsStr) -> bool {
    SUBCOMMANDS.iter().any(|name| name.as_bytes() == word.as_bytes())
}

/// What the command line says.
fn parse(args: &mut Args) -> Result<Parsed, Fail> {
    let (mut security, mut save, mut port) = (false, None, None);
    let mut operands: Vec<String> = Vec::new();
    while let Some(arg) = args.next()? {
        match arg {
            Arg::Long(name) if name == "help" => return Ok(Parsed::Help),
            Arg::Long(name) if name == "security" => security = true,
            Arg::Long(name) if name == "save" => {
                // A word that starts a subcommand, or an option, is not a
                // file's name: `report --save crash`, `report --save
                // --security`.
                let file = args.optional_value_where(|word| {
                    !word.as_bytes().starts_with(b"-") && !is_subcommand(word)
                });
                save = Some(file.map_or(Save::Phone, |file| Save::File(PathBuf::from(file))));
            }
            Arg::Long(name) if name == "port" => {
                let number: u16 = args.number()?;
                if number == 0 {
                    return Err(usage("--port needs a port from 1 to 65535".into()));
                }
                port = Some(number);
            }
            Arg::Operand(word) => operands.push(
                word.into_string()
                    .map_err(|word| usage(format!("'{}' is not UTF-8", word.to_string_lossy())))?,
            ),
            other => return Err(args.unknown(&other)),
        }
    }
    let command = command_of(&operands)?;
    check_options(&command, security, save.as_ref(), port)?;
    Ok(Parsed::Run(Run { command, security, save, port }))
}

/// The command `operands` name: the first is the subcommand, and what
/// follows is its own.
fn command_of(operands: &[String]) -> Result<Command, Fail> {
    let Some((name, rest)) = operands.split_first() else { return Ok(Command::Report) };
    let one = |what: &str| -> Result<String, Fail> {
        match rest {
            [only] => Ok(only.clone()),
            _ => Err(usage(format!("report {name} takes {what}"))),
        }
    };
    let number = |text: &str| -> Result<u64, Fail> {
        text.parse().map_err(|_| usage(format!("'{text}' is not a report's number")))
    };
    Ok(match name.as_str() {
        "crash" if rest.len() <= 1 => Command::Crash(rest.first().cloned()),
        "crash" => return Err(usage("report crash takes the ID of one crash".into())),
        "failure" | "list" | "retry" if !rest.is_empty() => {
            return Err(usage(format!("report {name} takes nothing more")));
        }
        "failure" => Command::Failure,
        "list" => Command::List,
        "retry" => Command::Retry,
        "status" if rest.len() <= 1 => {
            Command::Status(rest.first().map(|text| number(text)).transpose()?)
        }
        "status" => return Err(usage("report status takes the number of one report".into())),
        "delete" => Command::Delete(number(&one("the number of a report")?)?),
        "forget" => Command::Forget(one("the ID of a crash")?),
        other => return Err(usage(format!("no such command: {other}\n{HELP}"))),
    })
}

/// Fails for an option that does not go with the command or with another.
fn check_options(
    command: &Command,
    security: bool,
    save: Option<&Save>,
    port: Option<u16>,
) -> Result<(), Fail> {
    if !command.writes() && (security || save.is_some() || port.is_some()) {
        return Err(usage(
            "--security, --save and --port are for a report: report, report crash or report failure"
                .into(),
        ));
    }
    if security && save.is_some() {
        return Err(usage(
            "--security sends the report to the security team, and --save keeps it here: \
             use one of them"
                .into(),
        ));
    }
    if port.is_some() && matches!(save, Some(Save::File(_))) {
        return Err(usage("--port is for the download to a phone, not for a file".into()));
    }
    Ok(())
}

/// Does what `run` asks.
fn execute(client: &Client, run: &Run) -> Result<i32, Fail> {
    match &run.command {
        Command::Report => {
            let about = ask_about()?;
            write(client, run, Subject { about, crash: None, optional: false })
        }
        Command::Crash(id) => {
            let crashes: Vec<Crash> = client.list(&request("crashes"), client::QUICK)?;
            let crash = pick_crash(&crashes, id.as_deref())?;
            println!("Reporting the crash of {}\n", crash_line(&crash));
            write(
                client,
                run,
                Subject {
                    about: About::Crash { id: crash.id.clone() },
                    crash: Some(crash),
                    optional: false,
                },
            )
        }
        Command::Failure => {
            let notice = failure_notice()
                .ok_or_else(|| fail("no start has failed since this one began".into()))?;
            write(
                client,
                run,
                Subject { about: About::Failure { notice }, crash: None, optional: true },
            )
        }
        Command::List => list(client),
        Command::Status(number) => status(client, *number),
        Command::Delete(number) => delete(client, *number),
        Command::Forget(id) => forget(client, id),
        Command::Retry => retry(client),
    }
}

/// The lines of the failure notice (ADR 0501), with what changed in the
/// start that failed, as `changes --failed` shows it; none if no start
/// has failed since this one began.
fn failure_notice() -> Option<Vec<String>> {
    use crate::sysupdate::{health, starts};
    let notice = std::fs::read_to_string(health::NOTICE).ok()?;
    let mut lines: Vec<String> = notice.lines().map(str::to_string).collect();
    let records = starts::all();
    if let Some(failed) = records.iter().rev().find(|s| s.outcome == starts::Outcome::Failed) {
        lines.push(String::new());
        lines.extend(crate::changes::report(&records, None, failed));
    }
    Some(lines)
}

/// A request to reportd with nothing but its name.
fn request(ask: &str) -> Request {
    Request { ask: ask.into(), ..Request::default() }
}

/// What a report is about, and what that is known to be.
struct Subject {
    about: About,
    /// The crash, for its title.
    crash: Option<Crash>,
    /// Whether every question may be skipped: the failed start's, which
    /// the notice and the log already tell.
    optional: bool,
}

/// What is sent, and what it is kept under.
struct Report {
    about: About,
    answers: Answers,
    title: String,
    crash: Option<String>,
    security: bool,
}

/// Asks the questions, drafts the report and shows it, until a key or
/// `--save` has it sent, saved or put away.
fn write(client: &Client, run: &Run, subject: Subject) -> Result<i32, Fail> {
    let answers = ask_answers(subject.optional)?;
    let report = Report {
        title: title_for(&subject.about, &answers.happened, subject.crash.as_ref()),
        about: subject.about,
        answers,
        crash: subject.crash.map(|crash| crash.id),
        security: run.security,
    };
    let mut drafted = draft(client, &report, false)?;
    let mut edited = false;
    loop {
        preview::show(&drafted.text, drafted.cuts, report.security)?;
        match &run.save {
            Some(Save::File(path)) => return save_file(path, &drafted.text).map(|()| 0),
            Some(Save::Phone) => {
                return phone(&drafted.text, run.port).map(|done| i32::from(!done));
            }
            None => {}
        }
        // An edit is not drafted again: leaving the logs out would drop it.
        let logs = drafted.has_logs && !edited;
        match preview::ask(logs, report.security)? {
            Choice::Send => return send(client, &report, &drafted.text),
            Choice::LeaveOutLogs => drafted = draft(client, &report, true)?,
            Choice::Edit => match preview::edit(&drafted.text) {
                Ok(Some(text)) => {
                    let again = Request { text: Some(text), ..request("redact") };
                    drafted = client.one(&again, client::QUICK)?;
                    edited = true;
                }
                Ok(None) => println!("The text is as it was.\n"),
                Err(why) => println!("{}\n", why.message),
            },
            Choice::Save => {
                if let Some(path) = ask_file()? {
                    match save_file(&path, &drafted.text) {
                        Ok(()) => return Ok(0),
                        Err(why) => println!("{}\n", why.message),
                    }
                }
            }
            Choice::Phone => match phone(&drafted.text, run.port) {
                Ok(true) => return Ok(0),
                Ok(false) => {}
                Err(why) => println!("{}\n", why.message),
            },
            Choice::Cancel => {
                println!("Cancelled. Nothing was sent or saved.");
                return Ok(0);
            }
        }
    }
}

/// The draft of `report`, with or without the logs.
fn draft(client: &Client, report: &Report, without_logs: bool) -> Result<Drafted, Fail> {
    let request = Request {
        about: Some(report.about.clone()),
        answers: Some(report.answers.clone()),
        without_logs,
        ..request("draft")
    };
    client.one(&request, client::QUICK)
}

/// Sends `text`, the bytes that were shown, and says what became of it.
fn send(client: &Client, report: &Report, text: &str) -> Result<i32, Fail> {
    let request = Request {
        text: Some(text.to_string()),
        title: Some(report.title.clone()),
        security: report.security,
        crash: report.crash.clone(),
        ..request("send")
    };
    let sent: Sent = client.one(&request, client::ONLINE)?;
    println!("{}", sent_words(&sent));
    Ok(0)
}

/// What a report's send came to.
fn sent_words(sent: &Sent) -> String {
    match (&sent.waiting, sent.number) {
        (Some(why), _) => format!(
            "Not sent yet: {why}. It is kept, and sent when it can be; report retry tries now."
        ),
        (None, Some(number)) => {
            format!("Sent as report {number}. report status {number} shows where it stands.")
        }
        (None, None) => format!("Sent: {}.", sent.id),
    }
}

/// Saves `text` to the file `path`, and says so.
fn save_file(path: &std::path::Path, text: &str) -> Result<(), Fail> {
    save::to_file(path, text.as_bytes()).map_err(|err| util::failed(&path.display(), err))?;
    println!("Saved to {}, readable by you alone. Nothing was sent.", path.display());
    Ok(())
}

/// Serves `text` once to a phone; whether it was downloaded.
fn phone(text: &str, port: Option<u16>) -> Result<bool, Fail> {
    match save::download(text, port)? {
        Outcome::Downloaded => {
            println!("\nDownloaded. Nothing was sent from this computer.");
            Ok(true)
        }
        Outcome::TimedOut => {
            println!(
                "\nNo one downloaded it in {} minutes: the address works no more.",
                save::LIMIT.as_secs() / 60
            );
            Ok(false)
        }
    }
}

/// The name of the file to save to, as a person types it: `report.txt`
/// in the current folder when they only press Enter; none to go back.
fn ask_file() -> Result<Option<PathBuf>, Fail> {
    let text =
        line(&format!("File name ({} when you press Enter, - to go back): ", save::FILE_NAME))?;
    Ok(match text.as_str() {
        "-" => None,
        "" => Some(PathBuf::from(save::FILE_NAME)),
        name => Some(PathBuf::from(name)),
    })
}

/// A line typed after `prompt_text`, trimmed.
fn line(prompt_text: &str) -> Result<String, Fail> {
    match prompt::ask(prompt_text, true) {
        Ok(Answer::Line(line)) => Ok(String::from_utf8_lossy(&line).trim().to_string()),
        Ok(Answer::Stopped) => Err(fail("stopped: nothing was sent or saved".into())),
        Err(err) => Err(util::failed(&"the terminal", err)),
    }
}

/// What the plain report may be about.
const ABOUT: [&str; 5] = [
    "A command or an app",
    "A service",
    "The hardware",
    "Installing or updating Dukovo",
    "Something else",
];

/// The number of an item in `ABOUT` that `text` is, counted from 1.
fn about_number(text: &str) -> Option<usize> {
    text.trim().parse().ok().filter(|number| (1..=ABOUT.len()).contains(number))
}

/// Asks what the report is about.
fn ask_about() -> Result<About, Fail> {
    println!("What is the problem with?\n");
    for (number, item) in ABOUT.iter().enumerate() {
        println!("  {}  {item}", number + 1);
    }
    println!();
    loop {
        let text = line(&format!("Choose 1 to {}: ", ABOUT.len()))?;
        match about_number(&text) {
            Some(1) => {
                return Ok(About::Program { name: ask_name("The command's or app's name: ")? });
            }
            Some(2) => return Ok(About::Service { name: ask_service()? }),
            Some(3) => return Ok(About::Hardware),
            Some(4) => return Ok(About::Updates),
            Some(5) => return Ok(About::Other),
            _ => println!("Type a number from 1 to {}.", ABOUT.len()),
        }
    }
}

/// A name typed after `prompt_text`, asked again until there is one.
fn ask_name(prompt_text: &str) -> Result<String, Fail> {
    loop {
        let name = line(prompt_text)?;
        if !name.is_empty() {
            return Ok(name);
        }
    }
}

/// Asks which service, showing the ones that exist.
fn ask_service() -> Result<String, Fail> {
    // The list is a help, not a need: a Spark that does not answer leaves
    // the person to type the name.
    let known: Vec<(String, String)> = crate::service::shown(None).map_or_default(|show| {
        show.services.into_iter().map(|service| (service.name, service.description)).collect()
    });
    if !known.is_empty() {
        let rows: Vec<Vec<String>> = known
            .iter()
            .map(|(name, description)| vec![name.clone(), description.clone()])
            .collect();
        println!();
        print_table(&rows, &[false, false], util::terminal_size(1).map(|(columns, _)| columns))?;
        println!();
    }
    loop {
        let name = ask_name("The service's name: ")?;
        if known.is_empty() || known.iter().any(|(known, _)| *known == name) {
            return Ok(name);
        }
        println!("No service is called {name}: type a name from the list.");
    }
}

/// Asks the three questions; the first may be skipped only when `optional`.
fn ask_answers(optional: bool) -> Result<Answers, Fail> {
    println!("Finish each answer with an empty line.\n");
    let skip = " (Enter to skip)";
    let happened =
        answer(&format!("What happened?{}", if optional { skip } else { "" }), !optional)?;
    // The failed start's questions are all optional, and say so; a report's
    // second question is asked as it is worded, and may be left empty too.
    let expected =
        answer(&format!("What did you expect?{}", if optional { skip } else { "" }), false)?;
    let again = answer(&format!("How can it be made to happen again?{skip}"), false)?;
    Ok(Answers { happened, expected, again })
}

/// One answer: the lines typed up to an empty one. A question that needs
/// an answer is asked again until it has one.
fn answer(question: &str, required: bool) -> Result<String, Fail> {
    loop {
        println!("{question}");
        let mut lines: Vec<String> = Vec::new();
        loop {
            let text = match prompt::ask("", true) {
                Ok(Answer::Line(text)) => String::from_utf8_lossy(&text).trim_end().to_string(),
                Ok(Answer::Stopped) => {
                    return Err(fail("stopped: nothing was sent or saved".into()));
                }
                Err(err) => return Err(util::failed(&"the terminal", err)),
            };
            if text.is_empty() {
                break;
            }
            lines.push(text);
        }
        println!();
        if !lines.is_empty() || !required {
            return Ok(lines.join("\n"));
        }
        println!("This one needs an answer.\n");
    }
}

/// What reportd keeps a report under: the first answer as it was typed,
/// which reportd cuts to a title, or, when nothing was said, a name for
/// the thing the report is about.
fn title_for(about: &About, happened: &str, crash: Option<&Crash>) -> String {
    if !title_of(happened).is_empty() {
        return happened.to_string();
    }
    match (about, crash) {
        (About::Failure { .. }, _) => "Failed start".to_string(),
        (_, Some(crash)) => format!("{} stopped: {}", crash.program, crash.what),
        (About::Service { name } | About::Program { name }, None) => format!("Problem with {name}"),
        _ => "Problem report".to_string(),
    }
}

/// The crash to report: `id`'s, or the newest not reported yet.
fn pick_crash(crashes: &[Crash], id: Option<&str>) -> Result<Crash, Fail> {
    match id {
        Some(id) => {
            let crash = crashes.iter().find(|crash| crash.id == id).ok_or_else(|| {
                fail(format!("no crash {id} is kept: report list shows the crashes"))
            })?;
            match crash.reported {
                Some(number) => Err(fail(format!(
                    "crash {id} was reported as report {number} already: report status {number} \
                     shows where it stands"
                ))),
                None => Ok(crash.clone()),
            }
        }
        None => crashes.iter().find(|crash| crash.reported.is_none()).cloned().ok_or_else(|| {
            fail("no crash waits to be reported: report list shows the crashes kept".into())
        }),
    }
}

/// A crash in one line: `nginx, SIGSEGV (address not mapped), 2026-11-13 10:02 UTC`.
fn crash_line(crash: &Crash) -> String {
    format!("{}, {}, {} UTC", crash.program, crash.what, date_time(crash.at, false))
}

/// `report list`: the crashes kept, the reports waiting and the reports
/// sent.
fn list(client: &Client) -> Result<i32, Fail> {
    let crashes: Vec<Crash> = client.list(&request("crashes"), client::QUICK)?;
    let waiting: Vec<Waiting> = client.list(&request("queue"), client::QUICK)?;
    let sent: Vec<Status> = client.list(&request("status"), client::ONLINE)?;
    let width = util::terminal_size(1).map(|(columns, _)| columns);
    println!("Crashes kept");
    table(&crash_rows(&crashes), &[false, false, false, true, false, false], width, "None.")?;
    println!("\nReports waiting to be sent");
    table(&waiting_rows(&waiting), &[false, false, true, false, false], width, "None.")?;
    println!("\nReports sent");
    table(&sent_rows(&sent), &[true, false, false, false], width, "None.")?;
    Ok(0)
}

/// Prints `rows`, whose first is the header, or `none` if it has only that.
fn table(
    rows: &[Vec<String>],
    right: &[bool],
    width: Option<usize>,
    none: &str,
) -> Result<(), Fail> {
    if rows.len() <= 1 {
        println!("{none}");
        return Ok(());
    }
    Ok(print_table(rows, right, width)?)
}

/// The crashes as a table, newest first as reportd gives them.
fn crash_rows(crashes: &[Crash]) -> Vec<Vec<String>> {
    let mut rows =
        vec![["ID", "WHEN (UTC)", "PROGRAM", "SEEN", "REPORT", "WHAT"].map(String::from).to_vec()];
    for crash in crashes {
        rows.push(vec![
            crash.id.clone(),
            date_time(crash.at, false),
            crash.program.clone(),
            crash.seen.to_string(),
            crash.reported.map_or_else(|| "-".into(), |number| number.to_string()),
            crash.what.clone(),
        ]);
    }
    rows
}

/// The queue as a table.
fn waiting_rows(waiting: &[Waiting]) -> Vec<Vec<String>> {
    let mut rows =
        vec![["ID", "QUEUED (UTC)", "TRIES", "NEXT TRY (UTC)", "TITLE"].map(String::from).to_vec()];
    for report in waiting {
        let mut title = report.title.clone();
        if report.security {
            title.push_str(" [security]");
        }
        if let Some(error) = &report.error {
            title.push_str(&format!(" (last try failed: {error})"));
        }
        rows.push(vec![
            report.id.clone(),
            date_time(report.since, false),
            report.tries.to_string(),
            date_time(report.next, false),
            title,
        ]);
    }
    rows
}

/// The reports sent, as a table.
fn sent_rows(sent: &[Status]) -> Vec<Vec<String>> {
    let mut rows = vec![["NUMBER", "SENT (UTC)", "STATUS", "TITLE"].map(String::from).to_vec()];
    for report in sent {
        let mut title = report.title.clone();
        if report.security {
            title.push_str(" [security]");
        }
        rows.push(vec![
            report.number.to_string(),
            date_time(report.sent, false),
            status_words(report),
            title,
        ]);
    }
    rows
}

/// Where a report stands, in the words of a person.
fn status_words(report: &Status) -> String {
    match report.status.as_deref() {
        Some("received") => "Received".into(),
        Some("confirmed") => "Confirmed".into(),
        Some("duplicate") => match report.duplicate_of {
            Some(number) => format!("A duplicate of report {number}"),
            None => "A duplicate".into(),
        },
        Some("fixed") => match &report.fixed_in {
            Some(version) => format!("Fixed in {version}"),
            None => "Fixed".into(),
        },
        Some("wont-fix") => match &report.reason {
            Some(reason) => format!("Will not be fixed: {reason}"),
            None => "Will not be fixed".into(),
        },
        Some(other) => other.to_string(),
        None => match &report.error {
            Some(why) => format!("Not known: {why}"),
            None => "Not known: the reports server was not asked".into(),
        },
    }
}

/// `report status [N]`.
fn status(client: &Client, number: Option<u64>) -> Result<i32, Fail> {
    let ask = Request { number, ..request("status") };
    let reports: Vec<Status> = client.list(&ask, client::ONLINE)?;
    if reports.is_empty() {
        println!("No report was sent from this computer.");
    }
    for (at, report) in reports.iter().enumerate() {
        if at > 0 {
            println!();
        }
        println!("Report {}: {}", report.number, report.title);
        println!(
            "  Sent {} UTC{}",
            date_time(report.sent, false),
            if report.security { ", to the security team" } else { "" }
        );
        println!("  Status: {}", status_words(report));
    }
    Ok(0)
}

/// `report delete N`.
fn delete(client: &Client, number: u64) -> Result<i32, Fail> {
    client.done(&Request { number: Some(number), ..request("delete") }, client::ONLINE)?;
    println!("Deleted report {number}.");
    Ok(0)
}

/// `report forget ID`.
fn forget(client: &Client, id: &str) -> Result<i32, Fail> {
    client.done(&Request { crash: Some(id.to_string()), ..request("forget") }, client::QUICK)?;
    println!("Forgot crash {id}: it is not offered again.");
    Ok(0)
}

/// `report retry`: 0 when no report waits any more.
fn retry(client: &Client) -> Result<i32, Fail> {
    let tried: Vec<Sent> = client.list(&request("retry"), client::ONLINE)?;
    if tried.is_empty() {
        println!("No report is waiting.");
    }
    for sent in &tried {
        println!("{}: {}", sent.id, sent_words(sent));
    }
    Ok(i32::from(tried.iter().any(|sent| sent.waiting.is_some())))
}

#[cfg(test)]
mod tests {
    use super::super::protocol::Kind;
    use super::*;
    use std::ffi::OsString;

    fn parsed(words: &[&str]) -> Result<Parsed, Fail> {
        let items: Vec<OsString> = words.iter().map(OsString::from).collect();
        parse(&mut Args::new("report", items))
    }

    fn run(words: &[&str]) -> Run {
        match parsed(words).unwrap() {
            Parsed::Run(run) => run,
            Parsed::Help => panic!("help"),
        }
    }

    fn failure(words: &[&str]) -> String {
        parsed(words).unwrap_err().message
    }

    #[test]
    fn the_subcommands_and_their_operands() {
        assert_eq!(run(&[]).command, Command::Report);
        assert_eq!(run(&["crash"]).command, Command::Crash(None));
        assert_eq!(
            run(&["crash", "20261113T100211-4012"]).command,
            Command::Crash(Some("20261113T100211-4012".into()))
        );
        assert_eq!(run(&["failure"]).command, Command::Failure);
        assert_eq!(run(&["list"]).command, Command::List);
        assert_eq!(run(&["status"]).command, Command::Status(None));
        assert_eq!(run(&["status", "7"]).command, Command::Status(Some(7)));
        assert_eq!(run(&["delete", "7"]).command, Command::Delete(7));
        assert_eq!(
            run(&["forget", "20261113T100211-4012"]).command,
            Command::Forget("20261113T100211-4012".into())
        );
        assert_eq!(run(&["retry"]).command, Command::Retry);
        assert_eq!(parsed(&["--help"]).unwrap(), Parsed::Help);
    }

    #[test]
    fn mistakes_in_the_command_line_are_said() {
        assert!(failure(&["frobnicate"]).starts_with("no such command: frobnicate"));
        assert!(failure(&["delete"]).contains("takes the number of a report"));
        assert!(failure(&["delete", "x"]).contains("'x' is not a report's number"));
        assert!(failure(&["delete", "1", "2"]).contains("takes the number of a report"));
        assert!(failure(&["status", "1", "2"]).contains("one report"));
        assert!(failure(&["forget"]).contains("the ID of a crash"));
        assert!(failure(&["crash", "a", "b"]).contains("one crash"));
        assert!(failure(&["list", "x"]).contains("takes nothing more"));
        assert!(failure(&["--port", "0"]).contains("from 1 to 65535"));
        assert!(failure(&["--port", "70000"]).contains("not a valid number"));
        assert!(failure(&["--bogus"]).contains("unrecognized option"));
    }

    #[test]
    fn options_go_before_or_after_the_subcommand() {
        let first = run(&["--security", "crash"]);
        let last = run(&["crash", "--security"]);
        assert_eq!(first, last);
        assert!(first.security);
        let port = run(&["--port", "8443", "--save"]);
        assert_eq!((port.save, port.port), (Some(Save::Phone), Some(8443)));
        assert_eq!(run(&["failure", "--port=9000"]).port, Some(9000));
    }

    #[test]
    fn save_takes_a_file_unless_what_follows_is_a_command_or_an_option() {
        assert_eq!(run(&["--save"]).save, Some(Save::Phone));
        assert_eq!(run(&["--save", "r.txt"]).save, Some(Save::File("r.txt".into())));
        assert_eq!(run(&["--save=r.txt", "crash"]).save, Some(Save::File("r.txt".into())));
        assert_eq!(
            run(&["crash", "--save", "/tmp/r.txt"]).save,
            Some(Save::File("/tmp/r.txt".into()))
        );
        let before = run(&["--save", "crash"]);
        assert_eq!((before.save, before.command), (Some(Save::Phone), Command::Crash(None)));
        let option = run(&["--save", "--port", "9000"]);
        assert_eq!((option.save, option.port), (Some(Save::Phone), Some(9000)));
        assert_eq!(run(&["crash", "ID1", "--save"]).command, Command::Crash(Some("ID1".into())));
    }

    #[test]
    fn options_that_do_not_go_together_are_refused() {
        assert!(failure(&["list", "--security"]).contains("are for a report"));
        assert!(failure(&["retry", "--save"]).contains("are for a report"));
        assert!(failure(&["status", "--port", "9000"]).contains("are for a report"));
        assert!(failure(&["--security", "--save"]).contains("use one of them"));
        assert!(failure(&["--save", "r.txt", "--port", "9000"]).contains("not for a file"));
    }

    #[test]
    fn the_about_menu_takes_the_numbers_it_shows() {
        assert_eq!(about_number("1"), Some(1));
        assert_eq!(about_number(" 5 "), Some(5));
        for text in ["0", "6", "", "x", "-1", "1.5"] {
            assert_eq!(about_number(text), None, "{text:?}");
        }
    }

    fn crash(id: &str, reported: Option<u64>) -> Crash {
        Crash {
            id: id.into(),
            kind: Kind::Signal,
            program: "nginx".into(),
            version: None,
            service: None,
            user: 0,
            at: 1_794_564_131,
            what: "SIGSEGV (address not mapped)".into(),
            signature: "3fa2c4d19e0b7a61".into(),
            seen: 2,
            reported,
        }
    }

    #[test]
    fn the_newest_crash_not_reported_is_the_one_without_an_id() {
        let crashes = [crash("c3", Some(4)), crash("c2", None), crash("c1", None)];
        assert_eq!(pick_crash(&crashes, None).unwrap().id, "c2");
        assert_eq!(pick_crash(&crashes, Some("c1")).unwrap().id, "c1");
        assert!(
            pick_crash(&crashes, Some("c9")).unwrap_err().message.contains("no crash c9 is kept")
        );
        let reported = pick_crash(&crashes, Some("c3")).unwrap_err().message;
        assert!(reported.contains("reported as report 4 already"), "{reported}");
        let all_reported = [crash("c3", Some(4))];
        assert!(pick_crash(&all_reported, None).unwrap_err().message.contains("no crash waits"));
        assert!(pick_crash(&[], None).is_err());
    }

    #[test]
    fn a_title_is_what_happened_or_else_the_thing() {
        let nginx = crash("c1", None);
        let service = About::Service { name: "web".into() };
        // The answer goes as typed: reportd takes its first line.
        let said = "\nThe site is down\nsince noon";
        assert_eq!(title_for(&service, said, None), said);
        assert_eq!(title_of(&title_for(&service, said, None)), "The site is down");
        assert_eq!(title_for(&About::Failure { notice: Vec::new() }, "", None), "Failed start");
        assert_eq!(
            title_for(&About::Crash { id: "c1".into() }, "", Some(&nginx)),
            "nginx stopped: SIGSEGV (address not mapped)"
        );
        assert_eq!(title_for(&service, "", None), "Problem with web");
        assert_eq!(title_for(&About::Hardware, "", None), "Problem report");
        assert_eq!(title_for(&About::Other, "  \n ", None), "Problem report");
    }

    #[test]
    fn what_became_of_a_send_is_said_in_one_line() {
        let sent = |number, waiting: Option<&str>| Sent {
            id: "q1".into(),
            number,
            waiting: waiting.map(String::from),
        };
        assert_eq!(
            sent_words(&sent(Some(7), None)),
            "Sent as report 7. report status 7 shows where it stands."
        );
        assert!(
            sent_words(&sent(None, Some("no network"))).starts_with("Not sent yet: no network.")
        );
        assert!(sent_words(&sent(None, Some("no network"))).contains("report retry"));
        assert_eq!(sent_words(&sent(None, None)), "Sent: q1.");
    }

    fn status_of(status: Option<&str>) -> Status {
        Status {
            number: 7,
            title: "The disk light blinks".into(),
            sent: 1_794_564_131,
            status: status.map(String::from),
            duplicate_of: None,
            fixed_in: None,
            reason: None,
            error: None,
            security: false,
        }
    }

    #[test]
    fn a_status_is_said_in_the_words_of_the_server() {
        assert_eq!(status_words(&status_of(Some("received"))), "Received");
        assert_eq!(status_words(&status_of(Some("confirmed"))), "Confirmed");
        let mut fixed = status_of(Some("fixed"));
        assert_eq!(status_words(&fixed), "Fixed");
        fixed.fixed_in = Some("0.16.1".into());
        assert_eq!(status_words(&fixed), "Fixed in 0.16.1");
        let mut duplicate = status_of(Some("duplicate"));
        duplicate.duplicate_of = Some(3);
        assert_eq!(status_words(&duplicate), "A duplicate of report 3");
        let mut wont = status_of(Some("wont-fix"));
        wont.reason = Some("works as designed".into());
        assert_eq!(status_words(&wont), "Will not be fixed: works as designed");
        assert_eq!(status_words(&status_of(Some("triaged"))), "triaged");
        let mut unknown = status_of(None);
        assert!(status_words(&unknown).starts_with("Not known: the reports server was not asked"));
        unknown.error = Some("no network".into());
        assert_eq!(status_words(&unknown), "Not known: no network");
    }

    #[test]
    fn the_tables_have_a_header_and_a_row_for_each_item() {
        let rows = crash_rows(&[crash("c1", None), crash("c2", Some(9))]);
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0][0], "ID");
        assert_eq!(
            rows[1],
            ["c1", "2026-11-13 10:02", "nginx", "2", "-", "SIGSEGV (address not mapped)"]
        );
        assert_eq!(rows[2][4], "9");
        let waiting = Waiting {
            id: "q1".into(),
            title: "The disk light blinks".into(),
            since: 1_794_564_131,
            tries: 3,
            error: Some("no network".into()),
            next: 1_794_567_731,
            security: true,
        };
        let rows = waiting_rows(&[waiting]);
        assert_eq!(rows[1][2], "3");
        assert_eq!(rows[1][4], "The disk light blinks [security] (last try failed: no network)");
        assert_eq!(rows[1][3], "2026-11-13 11:02");
        let rows = sent_rows(&[status_of(Some("received"))]);
        assert_eq!(rows[1], ["7", "2026-11-13 10:02", "Received", "The disk light blinks"]);
        assert_eq!(crash_rows(&[]).len(), 1);
    }

    #[test]
    fn a_crash_is_one_line() {
        assert_eq!(
            crash_line(&crash("c1", None)),
            "nginx, SIGSEGV (address not mapped), 2026-11-13 10:02 UTC"
        );
    }

    #[test]
    fn only_the_commands_that_write_a_report_have_the_report_options() {
        assert!(
            Command::Report.writes() && Command::Failure.writes() && Command::Crash(None).writes()
        );
        assert!(!Command::List.writes() && !Command::Retry.writes());
        assert!(!Command::Status(None).writes() && !Command::Delete(1).writes());
        assert!(!Command::Forget("x".into()).writes());
    }
}
