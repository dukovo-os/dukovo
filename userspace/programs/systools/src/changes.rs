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

use crate::args::{Arg, Args};
use crate::sysupdate::starts::{self, Change, Outcome, Start};
use crate::util::{self, Fail, fail};

const USAGE: &str = "usage: changes [--failed | --between START START | --list]";

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
    format!("Start {}, slot {}: {outcome}", short(start), start.slot.to_uppercase())
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
