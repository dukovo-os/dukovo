// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! crontab(1): a person's crontab and whether their jobs run while they
//! are signed out (ADR 0478).
//!
//! The crontab is kept by timerd(8), which checks the text and refuses a
//! wrong line with its line and column; crontab sends it over timerd's
//! socket, where who asks is the user the kernel gives, so `-u USER`
//! works for root alone. `-e` edits a copy with `VISUAL`, `EDITOR` or
//! `vi`, and checks it as `service edit` checks a service file: a text
//! that does not read is shown with its mistake, and the person edits it
//! again or leaves it as it was. `--keep-running` is the setting authd(8)
//! keeps ([`crate::jobs`]).
//!
//! The boot test `crontab` covers it.

use std::fs::{self, OpenOptions};
use std::io::{self, BufRead, Read, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::process::Command;

use crate::args::{Arg, Args};
use crate::jobs::{self, KeepRunning};
use crate::timerd::crontab::{self, TEXT_MAX};
use crate::timerd::protocol::{self, LineRow, Reply, Request};
use crate::util::{self, Fail, fail, usage};

const USAGE: &str = "Usage: crontab [-u USER] FILE | -   (install a crontab)\n\
                     \x20      crontab [-u USER] -l | -e | -r [-i] | --status\n\
                     \x20      crontab --keep-running [on | off | default]";

/// What the command line asks for.
enum Job {
    Install(String),
    List,
    Edit,
    Remove,
    Status,
    KeepRunning(Option<Option<bool>>),
}

/// `crontab`.
pub fn main(args: &mut Args) -> Result<i32, Fail> {
    let mut user = None;
    let mut job = None;
    let mut ask = false;
    let set = |new: Job, job: &mut Option<Job>| {
        if job.is_some() {
            return Err(usage("give one of FILE, -l, -e, -r, --status and --keep-running".into()));
        }
        *job = Some(new);
        Ok(())
    };
    while let Some(arg) = args.next()? {
        match arg {
            Arg::Short('u') => user = Some(args.text()?),
            Arg::Short('l') => set(Job::List, &mut job)?,
            Arg::Short('e') => set(Job::Edit, &mut job)?,
            Arg::Short('r') => set(Job::Remove, &mut job)?,
            Arg::Short('i') => ask = true,
            Arg::Long(name) if name == "status" => set(Job::Status, &mut job)?,
            Arg::Long(name) if name == "keep-running" => {
                let word = args.optional_value().or_else(|| {
                    args.optional_operand(|v| matches!(v.to_str(), Some("on" | "off" | "default")))
                });
                let chosen = match word.as_deref().and_then(|w| w.to_str()) {
                    None => None,
                    Some("on") => Some(Some(true)),
                    Some("off") => Some(Some(false)),
                    Some("default") => Some(None),
                    Some(other) => {
                        return Err(usage(format!(
                            "--keep-running is on, off or default, not '{other}'"
                        )));
                    }
                };
                set(Job::KeepRunning(chosen), &mut job)?;
            }
            Arg::Operand(file) => {
                let file = file.to_string_lossy().into_owned();
                set(Job::Install(file), &mut job)?;
            }
            other => return Err(args.unknown(&other)),
        }
    }
    match job {
        None => Err(usage(USAGE.into())),
        Some(Job::KeepRunning(_)) if user.is_some() => {
            Err(usage("--keep-running is for the person who asks: leave out -u".into()))
        }
        Some(Job::KeepRunning(chosen)) => keep_running(chosen),
        Some(Job::Install(file)) => install(user, &file),
        Some(Job::List) => list(user),
        Some(Job::Edit) => edit(user),
        Some(Job::Remove) => remove(user, ask),
        Some(Job::Status) => status(user),
    }
}

fn reply(request: &Request) -> Result<Reply, Fail> {
    protocol::answer(request).map_err(fail)
}

/// `crontab FILE`: the file, or standard input for `-`.
fn install(user: Option<String>, file: &str) -> Result<i32, Fail> {
    let mut bytes = Vec::new();
    let limit = (TEXT_MAX + 1) as u64;
    if file == "-" {
        io::stdin()
            .take(limit)
            .read_to_end(&mut bytes)
            .map_err(|e| util::failed(&"standard input", e))?;
    } else {
        fs::File::open(file)
            .and_then(|f| f.take(limit).read_to_end(&mut bytes))
            .map_err(|e| util::failed(&file, e))?;
    }
    let text = String::from_utf8(bytes).map_err(|_| fail(format!("{file} is not UTF-8 text")))?;
    reply(&Request::CrontabInstall { user, text })?;
    Ok(0)
}

/// `crontab -l`.
fn list(user: Option<String>) -> Result<i32, Fail> {
    match reply(&Request::CrontabShow { user })? {
        Reply::Text { text } => {
            print!("{text}");
            Ok(0)
        }
        _ => Err(fail("timerd's answer was not a crontab".into())),
    }
}

/// `crontab -r`, and `-r -i`, which asks first.
fn remove(user: Option<String>, ask: bool) -> Result<i32, Fail> {
    if ask {
        let whose = user.as_deref().map_or("your".to_string(), |u| format!("{u}'s"));
        eprint!("crontab: really remove {whose} crontab? (y/n) ");
        let _ = io::stderr().flush();
        let mut answer = String::new();
        let read = io::stdin().lock().read_line(&mut answer).unwrap_or(0);
        if read == 0 || !answer.trim_start().starts_with(['y', 'Y']) {
            return Ok(1);
        }
    }
    reply(&Request::CrontabRemove { user })?;
    Ok(0)
}

/// `crontab -e`: edits a copy of the crontab, and installs it once it
/// reads.
fn edit(user: Option<String>) -> Result<i32, Fail> {
    let original = match protocol::answer(&Request::CrontabShow { user: user.clone() }) {
        Ok(Reply::Text { text }) => text,
        Ok(_) => return Err(fail("timerd's answer was not a crontab".into())),
        // A crontab that is not there is edited from nothing.
        Err(why) if why.starts_with("no crontab for") => String::new(),
        Err(why) => return Err(fail(why)),
    };
    let path = std::env::temp_dir().join(format!("crontab.{}", std::process::id()));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&path)
        .map_err(|e| util::failed(&path.display(), e))?;
    let written = file.write_all(original.as_bytes());
    drop(file);
    let result = written
        .map_err(|e| util::failed(&path.display(), e))
        .and_then(|()| edit_copy(&path, &original, user));
    let _ = fs::remove_file(&path); // a copy of the person's own text, in their own folder
    result
}

fn edit_copy(path: &std::path::Path, original: &str, user: Option<String>) -> Result<i32, Fail> {
    let editor = ["VISUAL", "EDITOR"]
        .iter()
        .find_map(|v| std::env::var(v).ok().filter(|e| !e.trim().is_empty()))
        .unwrap_or_else(|| "vi".into());
    loop {
        // Through the shell, so that EDITOR may hold arguments, as `vi -n`;
        // the file is $1, never part of the command.
        let status = Command::new("sh")
            .arg("-c")
            .arg(format!("{editor} \"$1\""))
            .arg("sh")
            .arg(path)
            .status()
            .map_err(|e| util::failed(&editor, e))?;
        if !status.success() {
            return Err(fail(format!("{editor} failed; the crontab is as it was")));
        }
        let edited = fs::read_to_string(path).map_err(|e| util::failed(&path.display(), e))?;
        if edited == original {
            println!("crontab: no change");
            return Ok(0);
        }
        match crontab::parse(&edited) {
            Ok(_) => {
                reply(&Request::CrontabInstall { user, text: edited })?;
                println!("crontab: installed");
                return Ok(0);
            }
            Err(mistake) => eprintln!("crontab: {mistake}"),
        }
        eprint!("What now? (e)dit again, or e(x)it without saving: ");
        let _ = io::stderr().flush();
        let mut answer = String::new();
        let read = io::stdin().lock().read_line(&mut answer).unwrap_or(0);
        if read == 0 || !answer.trim_start().starts_with('e') {
            eprintln!("crontab: the crontab is as it was");
            return Ok(1);
        }
    }
}

/// `crontab --status`.
fn status(user: Option<String>) -> Result<i32, Fail> {
    let Reply::Status { lines } = reply(&Request::CrontabStatus { user })? else {
        return Err(fail("timerd's answer was not a status".into()));
    };
    print!("{}", status_text(&lines, &|seconds| util::local_time(seconds, "%Y-%m-%d %H:%M:%S")));
    Ok(0)
}

/// The text `--status` prints for `lines`, with `clock` writing a time.
fn status_text(lines: &[LineRow], clock: &dyn Fn(i64) -> String) -> String {
    let mut text = String::new();
    for line in lines {
        text.push_str(&format!("line {}: {}  {}\n", line.line, line.time, line.command));
        match line.next {
            Some(next) => text.push_str(&format!("  next run: {}\n", clock(next))),
            None if line.time == "@reboot" => {
                text.push_str("  next run: at the next start of the machine\n");
            }
            None => text.push_str("  next run: none within five years\n"),
        }
        match (&line.not_run, line.last_start, line.last_end, line.status) {
            (Some(why), None, _, _) => text.push_str(&format!("  last: did not run: {why}\n")),
            (_, Some(start), end, status) => {
                let ended =
                    end.map_or_else(|| "not ended".to_string(), |e| format!("ended {}", clock(e)));
                let status = status.map_or_else(String::new, |s| format!(", status {s}"));
                text.push_str(&format!("  last run: {}, {ended}{status}\n", clock(start)));
            }
            _ => text.push_str("  last run: never\n"),
        }
        let output = line.output.trim_end();
        if !output.is_empty() {
            text.push_str("  output:\n");
            for out in output.lines() {
                text.push_str(&format!("    {out}\n"));
            }
        }
    }
    text
}

/// `crontab --keep-running [on|off|default]`.
fn keep_running(chosen: Option<Option<bool>>) -> Result<i32, Fail> {
    let setting = match chosen {
        Some(chosen) => jobs::set_keep_running(chosen),
        None => jobs::keep_running(),
    }
    .map_err(fail)?;
    println!("{}", setting_text(setting));
    Ok(0)
}

/// What the setting is, in words.
fn setting_text(setting: KeepRunning) -> String {
    let word = |on: bool| if on { "on" } else { "off" };
    let mut text = format!(
        "Your jobs {} while you are signed out.",
        if setting.on() { "run" } else { "do not run" }
    );
    match setting.chosen {
        Some(chosen) => text.push_str(&format!(" You turned it {}.", word(chosen))),
        None => {
            text.push_str(&format!(" That is the system's default, {}.", word(setting.default)));
        }
    }
    if setting.barred {
        text.push_str(" An administrator barred it for your account.");
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    fn clock(seconds: i64) -> String {
        format!("t{seconds}")
    }

    fn row() -> LineRow {
        LineRow {
            line: 3,
            time: "30 2 * * *".into(),
            command: "backup".into(),
            next: Some(100),
            last_start: Some(10),
            last_end: Some(12),
            status: Some(1),
            not_run: None,
            output: "bad\nworse\n".into(),
        }
    }

    #[test]
    fn the_status_shows_the_next_and_last_run_and_the_output() {
        let text = status_text(&[row()], &clock);
        assert_eq!(
            text,
            "line 3: 30 2 * * *  backup\n  next run: t100\n  last run: t10, ended t12, status 1\n  \
             output:\n    bad\n    worse\n"
        );
    }

    #[test]
    fn the_status_says_why_a_job_did_not_run_and_that_one_never_did() {
        let mut line = row();
        line.last_start = None;
        line.last_end = None;
        line.status = None;
        line.output.clear();
        line.not_run = Some("signed out".into());
        assert!(status_text(&[line.clone()], &clock).contains("last: did not run: signed out\n"));
        line.not_run = None;
        line.next = None;
        let text = status_text(&[line], &clock);
        assert!(text.contains("last run: never") && text.contains("none within five years"));
    }

    #[test]
    fn the_setting_is_said_in_words() {
        let on = KeepRunning { chosen: Some(true), default: false, barred: false };
        assert_eq!(setting_text(on), "Your jobs run while you are signed out. You turned it on.");
        let barred = KeepRunning { chosen: Some(true), default: false, barred: true };
        assert!(setting_text(barred).starts_with("Your jobs do not run"));
        assert!(setting_text(barred).ends_with("An administrator barred it for your account."));
        let default = KeepRunning { chosen: None, default: false, barred: false };
        assert_eq!(
            setting_text(default),
            "Your jobs do not run while you are signed out. That is the system's default, off."
        );
    }
}
