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
//! elimination.

use std::io::Write as _;

use crate::args::{Arg, Args};
use crate::power;
use crate::sysupdate::bisect::{self, Apart, End, Kind, Lever, Search, Verdict};
use crate::sysupdate::starts::{self, Change, Outcome, Start};
use crate::util::{self, Fail, fail};

const USAGE: &str = "usage: changes [--failed | --between START START | --list]\n       changes \
                     bisect [--yes | --status | --stop | --confirm [--yes]]";

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
    while let Some(arg) = args.next()? {
        match arg {
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
    let tried = start.trial.map_or_else(String::new, |n| format!(", try {n} of changes bisect"));
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
    let mut yes = false;
    while let Some(arg) = args.next()? {
        let Arg::Long(name) = arg else { return Err(util::usage(USAGE.into())) };
        let said = match name.as_str() {
            "yes" => {
                yes = true;
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
    if yes && matches!(asked, Bisect::Status | Bisect::Stop) {
        return Err(util::usage(USAGE.into()));
    }
    match asked {
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
