// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! changes(1): what changed between two starts (ADR 0500), from the
//! record the health check keeps of each: by default, from the last
//! good start to this one; with `--failed`, from the last good start to
//! the newest that failed; with `--between N M`, from start N to start
//! M; and with `--list`, the starts kept. Each change says when what is
//! new was installed, where that is known, and when a start first used
//! it: a change installed days before it was first used points at the
//! restart that brought it in.
//!
//! `changes bisect` finds which of the changes from the last good start
//! to the newest failed one broke the start, by halving them across
//! restarts, each a start with some of them left out
//! ([`crate::sysupdate::bisect`], ADR 0475): `--status` follows it,
//! `--stop` stops it, and `--confirm` makes sure of a change it named by
//! elimination. `--later` restarts for the first try a minute from now,
//! through Spark, instead of at once: the web console asks so, to answer
//! the page before the machine goes.
//!
//! `--json` goes with `--list`, with `--failed` and with `bisect
//! --status`, for the web console (changes(1)): what each start, each
//! change and the search hold, as the records say it.

use std::io::Write as _;

use serde::Serialize;

use crate::args::{Arg, Args};
use crate::hardware::memtest::MEMTEST_TRY;
use crate::power;
use crate::sysupdate::bisect::{self, Apart, End, Kind, Lever, Search, Verdict};
use crate::sysupdate::starts::{self, Change, Outcome, Start};
use crate::util::{self, Fail, fail};

const USAGE: &str = "usage: changes [--failed | --between START START | --list]\n       changes \
                     (--list | --failed) --json\n       changes bisect [--confirm] [--yes] \
                     [--later]\n       changes bisect (--status [--json] | --stop)";

/// What to compare.
enum Asked {
    /// The last good start before this one, and this one.
    Now,
    /// The last good start before the newest failed one, and it.
    Failed,
    /// Two starts by number.
    Between(u64, u64),
    /// The starts kept.
    List,
}

/// changes(1).
pub fn changes(args: &mut Args) -> Result<i32, Fail> {
    if let Some(sub) = args.subcommand() {
        return match sub.to_str() {
            Some("bisect") => bisect(args),
            _ => Err(util::usage(USAGE.into())),
        };
    }
    let mut asked = Asked::Now;
    let mut json = false;
    while let Some(arg) = args.next()? {
        match arg {
            Arg::Long(name) if name == "json" => json = true,
            Arg::Long(name) if name == "failed" => asked = Asked::Failed,
            Arg::Long(name) if name == "list" => asked = Asked::List,
            Arg::Long(name) if name == "between" => {
                let from = args.number()?;
                let to = args.number()?;
                asked = Asked::Between(from, to);
            }
            Arg::Long(name) if name == "help" => {
                println!("{USAGE}");
                return Ok(0);
            }
            _ => return Err(fail(USAGE.into())),
        }
    }
    if json {
        if matches!(asked, Asked::Now | Asked::Between(..)) {
            return Err(util::usage(USAGE.into()));
        }
        let records = starts::all();
        return if matches!(asked, Asked::List) {
            print_json(&list_json(&records))
        } else {
            print_json(&failed_json(&records, bisect::kept().as_ref()))
        };
    }
    let records = starts::all();
    let Some(newest) = records.last() else {
        println!("changes: no start has been recorded yet");
        return Ok(0);
    };
    let (from, to) = match asked {
        Asked::List => {
            for start in records.iter().rev() {
                println!("{}", start_line(start));
            }
            return Ok(0);
        }
        Asked::Now => (None, newest),
        Asked::Failed => {
            let Some(failed) = records.iter().rev().find(|s| s.outcome == Outcome::Failed) else {
                println!("changes: no start kept has failed");
                return Ok(0);
            };
            println!("{}", start_line(failed));
            (None, failed)
        }
        Asked::Between(from, to) => {
            let find = |number: u64| {
                records
                    .iter()
                    .find(|s| s.number == number)
                    .ok_or_else(|| fail(format!("no record of start {number} is kept")))
            };
            (Some(find(from)?), find(to)?)
        }
    };
    for line in report(&records, from, to) {
        println!("{line}");
    }
    Ok(0)
}

/// What changed from start `from`, the last good start before `to` when
/// it is `None`, to start `to`, of `records`, in lines: a heading, then
/// a change a line, indented by two spaces.
#[must_use]
pub fn report(records: &[Start], from: Option<&Start>, to: &Start) -> Vec<String> {
    let Some(from) = from.or_else(|| starts::last_good(records, to.number)) else {
        return vec![format!("No good start before start {} is kept.", to.number)];
    };
    let found = starts::changes(records, from, to);
    let since = if from.outcome == Outcome::Good { "the last good start, " } else { "start " };
    if found.is_empty() {
        return vec![format!(
            "Nothing changed from {since}{} to start {}.",
            short(from),
            to.number
        )];
    }
    let mut lines = vec![format!("From {since}{}, to start {}:", short(from), to.number)];
    lines.extend(found.iter().map(|change| format!("  {}", change_line(change))));
    lines
}

/// A start, in a few words: its number, when and what it ran.
fn short(start: &Start) -> String {
    let version = start
        .items
        .iter()
        .find(|item| item.kind == "system")
        .map_or_else(String::new, |item| format!(", Dukovo {}", item.value));
    format!("{} on {}{version}", start.number, date(start.started))
}

/// A line of `--list`.
#[must_use]
pub fn start_line(start: &Start) -> String {
    let outcome = match start.outcome {
        Outcome::Good => "good".to_string(),
        Outcome::Unfinished => "unfinished".to_string(),
        Outcome::Failed => {
            format!("failed: {}", start.why.as_deref().unwrap_or("the health check failed"))
        }
    };
    let tried = start.trial.map_or_else(String::new, |n| {
        if n == MEMTEST_TRY {
            ", memory test at the start".to_string()
        } else {
            format!(", try {n} of changes bisect")
        }
    });
    format!("Start {}, slot {}{tried}: {outcome}", short(start), start.slot.to_uppercase())
}

/// A time, for a person.
fn date(seconds: i64) -> String {
    util::local_time(seconds, "%Y-%m-%d %H:%M")
}

/// A change, in a line.
fn change_line(change: &Change) -> String {
    let what = match change.kind.as_str() {
        "system" => "Dukovo".to_string(),
        "kernel" => "the kernel".to_string(),
        "firmware" => "firmware set".to_string(),
        "microcode" => "CPU microcode".to_string(),
        "options" => "boot options".to_string(),
        "card" => format!("network card {}", change.name),
        "root" => format!("root partition {}", change.name),
        "app" => format!("app {}", change.name),
        other => format!("{other} {}", change.name),
    };
    let how = match (&change.before, &change.after) {
        (Some(before), Some(after)) => format!("{before} to {after}"),
        (None, Some(after)) => format!("new, {after}"),
        (Some(before), None) => format!("gone (was {before})"),
        (None, None) => String::new(),
    };
    let mut line = format!("{what}: {how}");
    if let Some(installed) = change.installed {
        line.push_str(&format!("; installed {}", date(installed)));
    }
    if let Some((number, when)) = change.first_used {
        line.push_str(&format!("; first used {} (start {number})", date(when)));
    }
    line
}

/// A change, in `--json`: what it is about, the version the later start
/// ran (none for one that is gone) and when that was installed, in Unix
/// milliseconds, where known.
#[derive(Debug, PartialEq, Eq, Serialize)]
struct ChangeJson {
    what: String,
    version: Option<String>,
    at_ms: Option<i64>,
}

/// A start, in `--json`. Where the record holds nothing for a field it
/// is `null`.
#[derive(Debug, PartialEq, Eq, Serialize)]
struct StartJson {
    start: u64,
    /// When the health check began, in Unix milliseconds.
    at_ms: i64,
    /// The version of Dukovo it ran.
    release: Option<String>,
    slot: String,
    /// `good`, `failed`, `fell-back` or `bisect-try`; none while the
    /// health check has not finished it.
    result: Option<&'static str>,
    /// The error ID of what failed it.
    check: Option<String>,
    /// When its services settled, in milliseconds after the kernel
    /// started, once its times are kept.
    ready_ms: Option<u64>,
    /// What it ran that the start before it did not.
    changes: Vec<ChangeJson>,
}

/// What `--failed --json` gives.
#[derive(Debug, PartialEq, Eq, Serialize)]
struct FailedJson {
    failed: Option<StartJson>,
    since_good: Vec<ChangeJson>,
    bisect: BisectJson,
}

/// A search, in `--json`.
#[derive(Debug, PartialEq, Eq, Serialize)]
struct BisectJson {
    running: bool,
    /// The try the next start is, while one is waited on.
    #[serde(rename = "try")]
    attempt: Option<u8>,
    /// The most tries the search takes, counting those it has made.
    of: Option<u32>,
}

/// What `bisect --status --json` gives: the search's state, as `--failed
/// --json` has it, and what a page shows of it: the failed start, the
/// changes it can and cannot try apart, each try, when the next restart
/// is, and what it found.
#[derive(Debug, PartialEq, Eq, Serialize)]
struct BisectStatusJson {
    #[serde(flatten)]
    state: BisectJson,
    /// The start the search is about; none when no search has run.
    failed_start: Option<u64>,
    /// The changes it can leave out, in the order `--status` numbers them.
    levers: Vec<LeverJson>,
    /// The changes it cannot try apart, and why.
    cannot: Vec<CannotJson>,
    tries: Vec<TryJson>,
    /// When the machine restarts for the next try, in milliseconds since
    /// 1970, while a restart is set and the search runs.
    next_ms: Option<u64>,
    /// What it found, once it has ended.
    result: Option<ResultJson>,
}

/// A change a search can leave out, in `--json`: the app's ID, `system`
/// or `boot options`, what the failed start ran, and when that was
/// installed, where known.
#[derive(Debug, PartialEq, Eq, Serialize)]
struct LeverJson {
    what: String,
    kind: Kind,
    version: String,
    at_ms: Option<i64>,
}

/// A change a search cannot try apart, in `--json`.
#[derive(Debug, PartialEq, Eq, Serialize)]
struct CannotJson {
    what: String,
    kind: String,
    why: String,
}

/// A try, in `--json`: the changes it left out, by [`LeverJson::what`],
/// what it found once judged, and the start that was it, and when.
#[derive(Debug, PartialEq, Eq, Serialize)]
struct TryJson {
    #[serde(rename = "try")]
    attempt: u8,
    left_out: Vec<String>,
    confirm: bool,
    verdict: Option<Verdict>,
    start: Option<u64>,
    at_ms: Option<i64>,
}

/// How a search ended, in `--json`: `found`, `not-found`, `otherwise`,
/// `not-started`, `broken` or `stopped`; for `found`, the change, whether
/// a try confirmed it, and how to undo it; and changes(1)'s own words.
#[derive(Debug, PartialEq, Eq, Serialize)]
struct ResultJson {
    end: &'static str,
    what: Option<String>,
    version: Option<String>,
    confirmed: bool,
    undo: Vec<String>,
    words: Vec<String>,
}

/// `bisect --status --json` for `search`, with the start times of
/// `records` and `next_ms`, when the restart Spark has set is; or, when
/// `planned` is a search of a newer failed start, that search before it
/// begins: its changes, and no try.
fn bisect_status_json(
    search: Option<&Search>,
    planned: Option<&Search>,
    records: &[Start],
    next_ms: Option<u64>,
) -> BisectStatusJson {
    if let Some(plan) = planned {
        return BisectStatusJson {
            state: bisect_json(None),
            failed_start: Some(plan.failed),
            levers: levers_json(plan),
            cannot: cannot_json(plan),
            tries: Vec::new(),
            next_ms: None,
            result: None,
        };
    }
    let state = bisect_json(search);
    let Some(search) = search else {
        return BisectStatusJson {
            state,
            failed_start: None,
            levers: Vec::new(),
            cannot: Vec::new(),
            tries: Vec::new(),
            next_ms: None,
            result: None,
        };
    };
    let name = |place: &usize| search.levers.get(*place).map_or_default(|lever| lever.name.clone());
    let started = |number: u64| {
        let start = records.iter().find(|start| start.number == number)?;
        Some(start.started.saturating_mul(1000))
    };
    let result = search.end.as_ref().map(|end| {
        let (end, found) = match end {
            End::Found { lever, confirmed } => ("found", Some((*lever, *confirmed))),
            End::NotFound => ("not-found", None),
            End::Otherwise { .. } => ("otherwise", None),
            End::NotStarted { .. } => ("not-started", None),
            End::Broken { .. } => ("broken", None),
            End::Stopped => ("stopped", None),
        };
        let lever = found.and_then(|(place, _)| search.levers.get(place));
        ResultJson {
            end,
            what: lever.map(|lever| lever.name.clone()),
            version: lever.map(|lever| lever.after.clone()),
            confirmed: found.is_some_and(|(_, confirmed)| confirmed),
            undo: lever.map_or_default(|lever| fix(search, lever)),
            words: bisect_result(search),
        }
    });
    BisectStatusJson {
        failed_start: Some(search.failed),
        levers: levers_json(search),
        cannot: cannot_json(search),
        tries: search
            .tries
            .iter()
            .map(|t| TryJson {
                attempt: t.number,
                left_out: t.undone.iter().map(name).collect(),
                confirm: t.confirm,
                verdict: t.verdict,
                start: t.start,
                at_ms: t.start.and_then(started),
            })
            .collect(),
        next_ms: next_ms.filter(|_| state.running),
        result,
        state,
    }
}

/// The changes `search` can leave out, in `--json`.
fn levers_json(search: &Search) -> Vec<LeverJson> {
    search
        .levers
        .iter()
        .map(|lever| LeverJson {
            what: lever.name.clone(),
            kind: lever.kind,
            version: lever.after.clone(),
            at_ms: lever.installed.map(|seconds| seconds.saturating_mul(1000)),
        })
        .collect()
}

/// The changes `search` cannot try apart, in `--json`.
fn cannot_json(search: &Search) -> Vec<CannotJson> {
    search
        .apart
        .iter()
        .map(|apart| CannotJson {
            what: apart.name.clone(),
            kind: apart.kind.clone(),
            why: apart.why.says(),
        })
        .collect()
}

/// When the restart Spark has set is, in milliseconds since 1970; none
/// when none is set, or Spark does not answer, which is no reason to fail
/// the read of the search.
fn next_restart() -> Option<u64> {
    let document = spark_api::show::parse(&power::read("show").ok()?).ok()?;
    let now_ms = util::now_ns() / 1_000_000;
    power::pending(&document, now_ms).filter(|set| set.end == "reboot").map(|set| set.at_ms)
}

/// Writes `value` as a line of JSON.
fn print_json<T: Serialize>(value: &T) -> Result<i32, Fail> {
    println!("{}", serde_json::to_string(value).map_err(|e| fail(e.to_string()))?);
    Ok(0)
}

/// A change, in `--json`.
fn change_json(change: &Change) -> ChangeJson {
    ChangeJson {
        what: change.name.clone(),
        version: change.after.clone(),
        at_ms: change.installed.map(|seconds| seconds.saturating_mul(1000)),
    }
}

/// How start `start`, of `records`, ended, in `--json`'s words. A try of
/// `changes bisect` is one whatever its end. A start that failed `fell-back`
/// when the next start that is no try ran the other slot: the records do
/// not say that the machine fell back, and this is how it shows in them.
fn result_of(records: &[Start], start: &Start) -> Option<&'static str> {
    if start.trial.is_some() {
        return Some("bisect-try");
    }
    match start.outcome {
        Outcome::Good => Some("good"),
        Outcome::Unfinished => None,
        Outcome::Failed => {
            let next = records.iter().find(|s| s.number > start.number && s.trial.is_none());
            Some(if next.is_some_and(|next| next.slot != start.slot) {
                "fell-back"
            } else {
                "failed"
            })
        }
    }
}

/// The start at place `at` of `records`, in `--json`, with the changes
/// from the start before it.
fn start_json(records: &[Start], at: usize) -> StartJson {
    let start = &records[at];
    let changes = at.checked_sub(1).map_or_else(Vec::new, |before| {
        starts::changes(records, &records[before], start).iter().map(change_json).collect()
    });
    StartJson {
        start: start.number,
        at_ms: start.started.saturating_mul(1000),
        release: start.items.iter().find(|item| item.kind == "system").map(|i| i.value.clone()),
        slot: start.slot.clone(),
        result: result_of(records, start),
        check: start.error.clone(),
        ready_ms: start
            .times
            .as_ref()
            .filter(|times| crate::startup::complete(times))
            .map(|times| times.settled_ms),
        changes,
    }
}

/// `--list --json`: each start kept, newest first.
fn list_json(records: &[Start]) -> Vec<StartJson> {
    (0..records.len()).rev().map(|at| start_json(records, at)).collect()
}

/// `--failed --json`: the newest start that failed and was no try, the
/// changes from the last good start to it, and the search. With no failed
/// start, or no good start before it, there is no start, or no change.
fn failed_json(records: &[Start], search: Option<&Search>) -> FailedJson {
    let at = records.iter().rposition(|s| s.outcome == Outcome::Failed && s.trial.is_none());
    let since_good = at
        .and_then(|at| {
            let failed = &records[at];
            let good = starts::last_good(records, failed.number)?;
            Some(starts::changes(records, good, failed).iter().map(change_json).collect())
        })
        .unwrap_or_default();
    FailedJson {
        failed: at.map(|at| start_json(records, at)),
        since_good,
        bisect: bisect_json(search),
    }
}

/// The search `search` is, for `--json`: whether it runs, the try it
/// waits on, and the most tries it takes in all.
fn bisect_json(search: Option<&Search>) -> BisectJson {
    let Some(search) = search else {
        return BisectJson { running: false, attempt: None, of: None };
    };
    let waiting = search.waiting().map(|t| t.number);
    BisectJson {
        running: search.end.is_none(),
        attempt: waiting,
        of: waiting.map(|number| u32::from(number).saturating_sub(1) + search.restarts_left()),
    }
}

/// How long `--later` waits before the restart for the first try: the time
/// a page takes to be answered and to count down, and as long as the
/// health check waits before each try after.
const LATER: std::time::Duration = std::time::Duration::from_mins(1);

/// What `changes bisect` was asked.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Bisect {
    Begin,
    Status,
    Stop,
    Confirm,
}

/// `changes bisect`.
fn bisect(args: &mut Args) -> Result<i32, Fail> {
    let mut asked = Bisect::Begin;
    let (mut yes, mut json, mut later) = (false, false, false);
    while let Some(arg) = args.next()? {
        let Arg::Long(name) = arg else { return Err(util::usage(USAGE.into())) };
        let said = match name.as_str() {
            "yes" => {
                yes = true;
                continue;
            }
            "later" => {
                later = true;
                continue;
            }
            "json" => {
                json = true;
                continue;
            }
            "status" => Bisect::Status,
            "stop" => Bisect::Stop,
            "confirm" => Bisect::Confirm,
            _ => return Err(util::usage(USAGE.into())),
        };
        if asked != Bisect::Begin {
            return Err(util::usage(USAGE.into()));
        }
        asked = said;
    }
    if (yes || later) && matches!(asked, Bisect::Status | Bisect::Stop) {
        return Err(util::usage(USAGE.into()));
    }
    if json && asked != Bisect::Status {
        return Err(util::usage(USAGE.into()));
    }
    match asked {
        Bisect::Status if json => {
            let records = starts::all();
            let search = bisect::kept();
            let running = search.as_ref().filter(|search| search.end.is_none());
            let next_ms = running.and_then(|_| next_restart());
            let planned = bisect::planned(&records, search.as_ref());
            print_json(&bisect_status_json(search.as_ref(), planned.as_ref(), &records, next_ms))
        }
        Bisect::Status => {
            let Some(search) = bisect::kept() else {
                println!("changes bisect: no search has run");
                return Ok(0);
            };
            for line in bisect_status(&search) {
                println!("{line}");
            }
            Ok(0)
        }
        Bisect::Stop => {
            match bisect::stop()? {
                Some(search) if search.end == Some(End::Stopped) => {
                    println!("changes bisect: stopped; the next start is the usual one");
                }
                _ => println!("changes bisect: no search was under way"),
            }
            Ok(0)
        }
        Bisect::Begin | Bisect::Confirm => {
            let search = if asked == Bisect::Begin {
                let search = bisect::plan_now()?;
                for line in bisect_plan(&search) {
                    println!("{line}");
                }
                search
            } else {
                let search = bisect::confirm_now()?;
                for line in bisect_confirm(&search) {
                    println!("{line}");
                }
                search
            };
            if !yes && !agree("Restart for the first try now? [y/N] ")? {
                println!("changes bisect: nothing was written");
                return Ok(1);
            }
            let number = bisect::write_first(search)?;
            if later {
                power::request(&format!("reboot {}", LATER.as_secs()))?;
                println!(
                    "changes bisect: restarting in {} seconds for try {number}; `elevate shutdown \
                     -c` cancels the restart, and the try is then the next start",
                    LATER.as_secs()
                );
                return Ok(0);
            }
            println!("changes bisect: restarting for try {number}");
            power::request("reboot")?;
            Ok(0)
        }
    }
}

/// Asks `question` on the terminal: whether the line typed starts with
/// `y`.
fn agree(question: &str) -> Result<bool, Fail> {
    print!("{question}");
    std::io::stdout().flush().map_err(|e| util::failed(&"the output", e))?;
    let mut line = String::new();
    std::io::stdin().read_line(&mut line).map_err(|e| util::failed(&"the input", e))?;
    Ok(line.trim_start().chars().next().is_some_and(|c| c.eq_ignore_ascii_case(&'y')))
}

/// A lever, in a few words: what it is and what changed.
fn lever_what(lever: &Lever) -> String {
    match lever.kind {
        Kind::Slot => format!("the system, {}", lever.after),
        Kind::Options => format!("the boot options `{}`", lever.after),
        Kind::App => format!("{} {}", lever.name, lever.after),
    }
}

/// A lever, in a line: what changed, from what, and when.
fn lever_line(lever: &Lever) -> String {
    let mut line = match (lever.kind, &lever.before) {
        (Kind::Slot, Some(before)) => format!("the system: {before} to {}", lever.after),
        (Kind::Options, Some(before)) => {
            format!("the boot options: `{before}` to `{}`", lever.after)
        }
        (Kind::App, Some(before)) => format!("app {}: {before} to {}", lever.name, lever.after),
        (_, _) => format!("app {}: new, {}", lever.name, lever.after),
    };
    if let Some(installed) = lever.installed {
        line.push_str(&format!("; installed {}", date(installed)));
    }
    if let Some((number, when)) = lever.first_used {
        line.push_str(&format!("; first used {} (start {number})", date(when)));
    }
    line
}

/// A change that cannot be tried apart, in a line, with why.
fn apart_line(apart: &Apart) -> String {
    let change = Change {
        kind: apart.kind.clone(),
        name: apart.name.clone(),
        before: apart.before.clone(),
        after: apart.after.clone(),
        installed: None,
        first_used: None,
    };
    format!("{}: {}", change_line(&change), apart.why.says())
}

/// What `changes bisect` will do, before it asks to go on: the failed
/// start, the changes it can leave out and those it cannot, and the
/// restarts it takes.
#[must_use]
pub fn bisect_plan(search: &Search) -> Vec<String> {
    let mut lines = header(search);
    lines.push(format!(
        "Finding which one takes up to {} restart{}. The machine restarts by itself for each \
         try, a start with some of the changes left out for that start alone: nothing is \
         changed for good, and a try that hangs costs only that try. `elevate changes bisect \
         --stop` stops it at any point, and `changes bisect --status` follows it.",
        search.restarts_left(),
        if search.restarts_left() == 1 { "" } else { "s" }
    ));
    lines
}

/// What `--confirm` will do.
fn bisect_confirm(search: &Search) -> Vec<String> {
    let lever = search.waiting().map(|t| &search.levers[t.undone[0]]);
    vec![format!(
        "One more restart, leaving out only {}, makes sure: the try confirms it if it starts \
         well. `elevate changes bisect --stop` stops it.",
        lever.map_or_else(String::new, lever_what)
    )]
}

/// The failed start, and the changes a search can and cannot try apart,
/// numbered as `--status` names them.
fn header(search: &Search) -> Vec<String> {
    let mut lines = vec![format!(
        "Start {} failed its health check: {}",
        search.failed,
        search.why.as_deref().unwrap_or("a check failed")
    )];
    lines.push(format!(
        "From the last good start, {}, these changes can each be left out for one start:",
        search.good
    ));
    for (place, lever) in search.levers.iter().enumerate() {
        lines.push(format!("  {}. {}", place + 1, lever_line(lever)));
    }
    if !search.apart.is_empty() {
        lines.push("These cannot be tried apart:".into());
        lines.extend(search.apart.iter().map(|apart| format!("  {}", apart_line(apart))));
    }
    lines
}

/// `changes bisect --status`: the search, each try and what it found,
/// and the try it waits on or how it ended.
#[must_use]
pub fn bisect_status(search: &Search) -> Vec<String> {
    let mut lines = header(search);
    for t in &search.tries {
        let left_out: Vec<String> = t.undone.iter().map(|place| (place + 1).to_string()).collect();
        let at = t.start.map_or_else(String::new, |n| format!(", start {n}"));
        let found = match t.verdict {
            None => "waiting: the next start is this try".to_string(),
            Some(Verdict::Good) => "it started well".into(),
            Some(Verdict::Failed) => format!("it failed as start {} did", search.failed),
            Some(Verdict::Unfinished) => "it never reached its health check".into(),
            Some(Verdict::Otherwise) => {
                format!("it failed otherwise than start {} did", search.failed)
            }
            Some(Verdict::NotStarted) => "the boot menu did not start it".into(),
        };
        let what = if t.confirm { "to confirm, left out" } else { "left out" };
        lines.push(format!("Try {}{at}, {what} {}: {found}", t.number, left_out.join(", ")));
    }
    if search.waiting().is_some() {
        let left = search.restarts_left();
        lines.push(format!("At most {left} restart{} to go.", if left == 1 { "" } else { "s" }));
    }
    lines.extend(bisect_result(search));
    lines
}

/// What a search found, once it has ended, and what to do about it;
/// nothing while it runs.
#[must_use]
pub fn bisect_result(search: &Search) -> Vec<String> {
    let Some(end) = &search.end else { return Vec::new() };
    let failed = search.failed;
    let mut lines = Vec::new();
    match end {
        End::Found { lever, confirmed } => {
            let lever = &search.levers[*lever];
            let mut found = format!("The start fails with {}", lever_what(lever));
            if let Some(installed) = lever.installed {
                found.push_str(&format!(", installed {}", date(installed)));
            }
            if let Some((number, _)) = lever.first_used {
                found.push_str(&format!(", first started at start {number}"));
            }
            if *confirmed {
                found.push_str(", and starts without it.");
            } else {
                found.push_str(
                    ", named by elimination: the tries cleared every other change. `elevate \
                     changes bisect --confirm` makes sure, with one more restart that leaves out \
                     only it.",
                );
            }
            lines.push(found);
            lines.extend(fix(search, lever));
            lines.push(format!(
                "Nothing was changed: this start was the last try, and the next usual start is \
                 start {failed}'s again until a fix is taken."
            ));
        }
        End::NotFound => {
            let only = search.tries.last().map_or(0, |t| t.number);
            lines.push(format!(
                "Try {only} left out only that change, and the start failed as start {failed} did: \
                 no one change that can be tried apart explains it. It may take two together, or \
                 something the record does not hold; `changes --failed` lists every change."
            ));
        }
        End::Otherwise { number, start } => lines.push(format!(
            "Try {number}, start {start}, failed otherwise than start {failed} did, so its halves \
             cannot be judged: `changes --list` shows both failures."
        )),
        End::NotStarted { number } => lines.push(format!(
            "The boot menu did not start try {number}, so the search ended there. It starts a try \
             only from a whole block on the EFI system partition it starts from, while no update \
             waits to be tried: a read-only or failing partition, or an update staged meanwhile, \
             stops it."
        )),
        End::Broken { number, why } => {
            lines.push(format!("The search ended at try {number}: {why}."));
        }
        End::Stopped => lines.push("The search was stopped.".into()),
    }
    lines
}

/// The fix of `lever`, which the owner takes.
fn fix(search: &Search, lever: &Lever) -> Vec<String> {
    match (lever.kind, &lever.before) {
        (Kind::App, Some(before)) => vec![format!(
            "To go back to {before}, with its data as it was then: `elevate bundle rollback {id} \
             --for-everyone`; to keep it from coming back: `elevate bundle hold {id} \
             --for-everyone`.",
            id = lever.name
        )],
        (Kind::App, None) => {
            vec![format!("To remove it: `elevate bundle remove {} --for-everyone`.", lever.name)]
        }
        (Kind::Slot, before) => vec![format!(
            "To go back to {}: `elevate update undo system`; to keep the system there: `elevate \
             update hold system`.",
            before.as_deref().unwrap_or("the previous version")
        )],
        (Kind::Options, before) => vec![format!(
            "Start {} ran with `{}`, and start {} with `{}`. serial(8) sets the console's \
             flags; the others are the slot's, or the machine's own in boot/grub/installed.cfg \
             on the EFI system partition.",
            search.good,
            before.as_deref().unwrap_or_default(),
            search.failed,
            lever.after
        )],
    }
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;

    use super::*;
    use crate::sysupdate::starts::{Item, ServiceTime, Times};

    /// What a start ran: a release, and apps with a version and when each
    /// was installed.
    fn items(release: &str, apps: &[(&str, &str, Option<i64>)]) -> Vec<Item> {
        let mut items = vec![
            Item::new("system", "Dukovo", release, Some(10)),
            Item::new("options", "boot options", "root=UUID=r panic=10 quiet", None),
        ];
        items.extend(apps.iter().map(|(id, version, at)| Item::new("app", id, version, *at)));
        items
    }

    /// A start numbered `number`, begun `number` thousand seconds in.
    fn start(number: u64, slot: &str, outcome: Outcome, items: Vec<Item>) -> Start {
        let failed = outcome == Outcome::Failed;
        Start {
            number,
            started: i64::try_from(number).unwrap() * 1000,
            slot: slot.into(),
            stage: "services".into(),
            outcome,
            error: failed.then(|| "BOOT-104".into()),
            why: failed.then(|| "a required check failed (BOOT-104)".into()),
            failing: None,
            trial: None,
            items,
            times: None,
        }
    }

    /// Seven starts: two good ones with an app updated between them, an
    /// update that failed and the start that went back, a failure that
    /// went on in the same slot, a failed try of the search and the
    /// start that has not finished.
    fn records() -> Vec<Start> {
        let nginx = |version: &str, at: i64| ("org.nginx.nginx", version.to_string(), Some(at));
        let with = |release: &str, apps: &[(&str, String, Option<i64>)]| {
            let apps: Vec<(&str, &str, Option<i64>)> =
                apps.iter().map(|(id, v, at)| (*id, v.as_str(), *at)).collect();
            items(release, &apps)
        };
        let mut first = start(1, "a", Outcome::Good, with("0.15.0", &[nginx("1.30.5", 100)]));
        first.times = Some(Times {
            settled_ms: 31_200,
            services: vec![ServiceTime {
                name: "netd".into(),
                started_ms: 2000,
                ready_ms: Some(3000),
                after: Vec::new(),
            }],
            ..Times::default()
        });
        let second = start(2, "a", Outcome::Good, with("0.15.0", &[nginx("1.30.6", 5000)]));
        let third = start(3, "b", Outcome::Failed, with("0.16.0", &[nginx("1.30.6", 5000)]));
        let fourth = start(4, "a", Outcome::Good, with("0.15.0", &[nginx("1.30.6", 5000)]));
        let demo = ("org.demo.app", "2.0".to_string(), Some(7000));
        let fifth = start(5, "a", Outcome::Failed, with("0.15.0", &[nginx("1.30.6", 5000), demo]));
        let mut sixth = start(6, "a", Outcome::Failed, with("0.15.0", &[nginx("1.30.6", 5000)]));
        sixth.trial = Some(1);
        let mut seventh = start(7, "a", Outcome::Unfinished, with("0.15.0", &[]));
        // A sign-in's time alone is no start's times.
        seventh.times = Some(Times { sign_in_ms: Some(9000), ..Times::default() });
        vec![first, second, third, fourth, fifth, sixth, seventh]
    }

    /// A search over five apps, as it begins.
    fn search() -> Search {
        let records = records();
        let levers = (0..5)
            .map(|n| Lever {
                kind: Kind::App,
                name: format!("org.app{n}"),
                before: Some("1".into()),
                after: "2".into(),
                installed: None,
                first_used: None,
                services: Vec::new(),
            })
            .collect();
        Search::new(&records[3], &records[4], levers, Vec::new(), 1000, 7).unwrap()
    }

    #[test]
    fn a_start_is_good_failed_fell_back_a_try_or_not_finished() {
        let records = records();
        let results: Vec<Option<&str>> = records.iter().map(|s| result_of(&records, s)).collect();
        assert_eq!(
            results,
            [
                Some("good"),
                Some("good"),
                // The update in slot b failed and slot a started after it.
                Some("fell-back"),
                Some("good"),
                // The next start that is no try ran the same slot.
                Some("failed"),
                Some("bisect-try"),
                None,
            ]
        );
        // The newest failed start has none after it.
        assert_eq!(result_of(&records[..5], &records[4]), Some("failed"));
    }

    #[test]
    fn a_try_is_named_as_a_try_of_the_search_and_the_memory_test_as_itself() {
        let mut one = start(5, "a", Outcome::Good, Vec::new());
        assert!(start_line(&one).contains(", slot A: good"), "{}", start_line(&one));
        one.trial = Some(2);
        assert!(start_line(&one).contains(", slot A, try 2 of changes bisect: good"));
        one.trial = Some(99);
        let line = start_line(&one);
        assert!(line.contains(", slot A, memory test at the start: good"), "{line}");
        assert!(!line.contains("changes bisect"), "{line}");
    }

    #[test]
    fn the_list_is_newest_first_with_what_each_start_changed() {
        let list = list_json(&records());
        let numbers: Vec<u64> = list.iter().map(|s| s.start).collect();
        assert_eq!(numbers, [7, 6, 5, 4, 3, 2, 1]);
        let value = serde_json::to_value(&list).unwrap();
        // The second start brought nginx 1.30.6, installed at 5000 s.
        assert_eq!(
            value[5],
            serde_json::json!({
                "start": 2, "at_ms": 2_000_000, "release": "0.15.0", "slot": "a",
                "result": "good", "check": null, "ready_ms": null,
                "changes": [{"what": "org.nginx.nginx", "version": "1.30.6", "at_ms": 5_000_000}]
            })
        );
        // The first has none before it to differ from, and its times.
        assert_eq!(
            value[6],
            serde_json::json!({
                "start": 1, "at_ms": 1_000_000, "release": "0.15.0", "slot": "a",
                "result": "good", "check": null, "ready_ms": 31_200, "changes": []
            })
        );
        // The failed update: its release, and the error ID that failed it.
        assert_eq!(value[4]["result"], "fell-back");
        assert_eq!(value[4]["check"], "BOOT-104");
        assert_eq!(value[4]["release"], "0.16.0");
        assert_eq!(value[4]["changes"][0]["what"], "Dukovo");
        assert_eq!(value[4]["changes"][0]["version"], "0.16.0");
        assert_eq!(value[4]["changes"][0]["at_ms"], 10_000);
        // The start before it gone back to what ran two starts before.
        assert_eq!(value[3]["changes"][0]["version"], "0.15.0");
        // A start that is not finished says no result, and a sign-in's
        // time is not when its services settled.
        assert_eq!(value[0]["result"], serde_json::Value::Null);
        assert_eq!(value[0]["ready_ms"], serde_json::Value::Null);
        assert_eq!(value[1]["result"], "bisect-try");
    }

    #[test]
    fn a_start_has_the_keys_of_the_design_in_its_order() {
        let text = serde_json::to_string(&list_json(&records())[6]).unwrap();
        assert_eq!(
            text,
            r#"{"start":1,"at_ms":1000000,"release":"0.15.0","slot":"a","result":"good","check":null,"ready_ms":31200,"changes":[]}"#
        );
        assert_eq!(serde_json::to_string(&list_json(&[])).unwrap(), "[]");
    }

    #[test]
    fn a_record_with_no_release_or_times_says_null() {
        let mut bare = start(9, "b", Outcome::Good, Vec::new());
        bare.times = None;
        let text = serde_json::to_string(&list_json(&[bare])).unwrap();
        assert_eq!(
            text,
            r#"[{"start":9,"at_ms":9000000,"release":null,"slot":"b","result":"good","check":null,"ready_ms":null,"changes":[]}]"#
        );
    }

    #[test]
    fn failed_is_the_newest_failed_start_that_is_no_try_with_what_came_since_the_good_one() {
        let records = records();
        let value = serde_json::to_value(failed_json(&records, None)).unwrap();
        assert_eq!(value["failed"]["start"], 5);
        assert_eq!(value["failed"]["result"], "failed");
        assert_eq!(value["failed"]["check"], "BOOT-104");
        // Since start 4, the last good: the demo app, and nothing else.
        assert_eq!(
            value["since_good"],
            serde_json::json!([{"what": "org.demo.app", "version": "2.0", "at_ms": 7_000_000}])
        );
        assert_eq!(value["bisect"], serde_json::json!({"running": false, "try": null, "of": null}));
        assert_eq!(
            serde_json::to_string(&failed_json(&records, None)).unwrap().find("\"failed\":{"),
            Some(1),
            "the failed start comes first"
        );
    }

    #[test]
    fn with_no_failed_start_failed_is_null_and_with_no_good_one_there_is_no_change() {
        let records = records();
        let good = &records[..2];
        let value = serde_json::to_value(failed_json(good, None)).unwrap();
        assert_eq!(
            value,
            serde_json::json!({
                "failed": null, "since_good": [],
                "bisect": {"running": false, "try": null, "of": null}
            })
        );
        let lone = [start(3, "a", Outcome::Failed, items("0.15.0", &[]))];
        let value = serde_json::to_value(failed_json(&lone, None)).unwrap();
        assert_eq!(value["failed"]["start"], 3);
        assert_eq!(value["since_good"], serde_json::json!([]));
        let none = serde_json::to_value(failed_json(&[], None)).unwrap();
        assert_eq!(none["failed"], serde_json::Value::Null);
    }

    #[test]
    fn the_bisect_object_has_the_try_waited_on_and_the_most_tries_in_all() {
        assert_eq!(
            serde_json::to_string(&bisect_json(None)).unwrap(),
            r#"{"running":false,"try":null,"of":null}"#
        );
        // Five changes take at most three restarts, whichever the cause.
        assert_eq!(
            serde_json::to_string(&bisect_json(Some(&search()))).unwrap(),
            r#"{"running":true,"try":1,"of":3}"#
        );
        // The first try failed as the start did: the cause is among the
        // two changes it kept, and one more try settles it.
        let mut running = search();
        running.judge(Verdict::Failed, Some(8), 8);
        let after = bisect_json(Some(&running));
        assert_eq!((after.running, after.attempt, after.of), (true, Some(2), Some(2)));
        // It started well: the cause is among the three it left out, and
        // two more tries at most.
        let mut running = search();
        running.judge(Verdict::Good, Some(8), 8);
        let after = bisect_json(Some(&running));
        assert_eq!((after.running, after.attempt, after.of), (true, Some(2), Some(3)));
        // A search that has ended does not run, and waits on no try.
        let mut done = search();
        done.stop();
        assert_eq!(
            serde_json::to_string(&bisect_json(Some(&done))).unwrap(),
            r#"{"running":false,"try":null,"of":null}"#
        );
        // The failed object carries it.
        let value = serde_json::to_value(failed_json(&records(), Some(&search()))).unwrap();
        assert_eq!(value["bisect"], serde_json::json!({"running": true, "try": 1, "of": 3}));
    }

    #[test]
    fn the_status_has_the_changes_each_try_the_next_restart_and_what_was_found() {
        let records = records();
        let none = serde_json::to_value(bisect_status_json(None, None, &records, Some(1))).unwrap();
        assert_eq!(
            none,
            serde_json::json!({
                "running": false, "try": null, "of": null, "failed_start": null, "levers": [],
                "cannot": [], "tries": [], "next_ms": null, "result": null,
            })
        );

        // The first try was start 6, and failed as start 5 did.
        let mut running = search();
        running.judge(Verdict::Failed, Some(6), 6);
        let value = bisect_status_json(Some(&running), None, &records, Some(1_800_000_060_000));
        let value = serde_json::to_value(value).unwrap();
        assert_eq!((value["running"].clone(), value["try"].clone()), (true.into(), 2.into()));
        assert_eq!(value["failed_start"], 5);
        assert_eq!(
            value["levers"][0],
            serde_json::json!({ "what": "org.app0", "kind": "app", "version": "2", "at_ms": null })
        );
        assert_eq!(value["levers"].as_array().unwrap().len(), 5);
        let first = &value["tries"][0];
        assert_eq!(first["verdict"], "failed");
        assert_eq!((first["start"].clone(), first["at_ms"].clone()), (6.into(), 6_000_000.into()));
        let left_out = first["left_out"].as_array().unwrap();
        assert!(
            !left_out.is_empty()
                && left_out.iter().all(|what| what.as_str().unwrap().starts_with("org.app"))
        );
        assert_eq!(value["tries"][1]["verdict"], serde_json::Value::Null, "the try waited on");
        assert_eq!(value["next_ms"], 1_800_000_060_000_u64);
        assert_eq!(value["result"], serde_json::Value::Null);

        // Once it has ended there is no next restart of its own, and the
        // result says how it ended, in changes(1)'s words too.
        let mut stopped = search();
        stopped.stop();
        let value = bisect_status_json(Some(&stopped), None, &records, Some(1));
        let value = serde_json::to_value(value).unwrap();
        assert_eq!(value["next_ms"], serde_json::Value::Null);
        assert_eq!(value["result"]["end"], "stopped");
        assert_eq!(value["result"]["what"], serde_json::Value::Null);
        assert_eq!(value["result"]["undo"], serde_json::json!([]));
        let words = bisect_result(&stopped);
        assert_eq!(value["result"]["words"], serde_json::json!(words));

        // The plan of a newer failed start is the search before it begins:
        // its changes and no try, whatever search ended before.
        let value = bisect_status_json(Some(&stopped), Some(&search()), &records, Some(1));
        let value = serde_json::to_value(value).unwrap();
        assert_eq!(
            (value["running"].clone(), value["try"].clone()),
            (false.into(), serde_json::Value::Null)
        );
        assert_eq!(value["failed_start"], 5);
        assert_eq!(value["levers"].as_array().unwrap().len(), 5);
        assert_eq!(value["tries"], serde_json::json!([]));
        assert_eq!(value["next_ms"], serde_json::Value::Null);
        assert_eq!(value["result"], serde_json::Value::Null);
    }

    /// Runs `changes` on `items`.
    fn run(items: &[&str]) -> Result<i32, Fail> {
        changes(&mut Args::new("changes", items.iter().map(OsString::from).collect()))
    }

    #[test]
    fn json_goes_with_list_failed_and_bisect_status_alone() {
        for refused in [
            &["--json"][..],
            &["--json", "--between", "1", "2"],
            &["--between", "1", "2", "--json"],
            &["bisect", "--json"],
            &["bisect", "--stop", "--json"],
            &["bisect", "--confirm", "--json"],
            &["bisect", "--yes", "--json"],
            &["bisect", "--status", "--stop", "--json"],
            &["bisect", "--status", "--later"],
            &["bisect", "--stop", "--later"],
        ] {
            let err = run(refused).err().unwrap_or_else(|| panic!("{refused:?} is refused"));
            assert!(err.usage, "{refused:?}");
            assert_eq!(err.message, USAGE, "{refused:?}");
            assert_eq!(err.code, 1);
        }
        let err = run(&["bisect", "--status", "--yes", "--json"]).unwrap_err();
        assert!(err.usage);
        assert!(USAGE.contains("(--list | --failed) --json"));
        assert!(USAGE.contains("--status [--json]"));
    }
}
