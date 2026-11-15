// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! logtrim(8): trims the system's log (ADR 0211).
//!
//! logd(8) keeps the log in a file a day, and the security events of the
//! audit log in a file a day of their own; it trims both as
//! `/system/settings/log` says (log(5)): the log's records after 90 days,
//! the audit log's after a year, and the oldest of the log's files, then
//! the audit log's, while the two take more than 4 GiB or a tenth of
//! their file system. It trims when it starts, when a day begins, after
//! each 64 MiB it writes, and when asked: logtrim asks it, now or with
//! limits of its own for the log (not the audit log), as journalctl's
//! `--vacuum-size` and `--vacuum-time` do. The file being written is
//! never trimmed, and each trim is a security event in the audit log.
//!
//! For the web console, `--show` tells how much the log takes and what
//! its limits are (as JSON with `--json`), and `set` changes the limits
//! in the settings file, keeping its comments, and has logd read it
//! again.

use std::ffi::OsStr;
use std::fs;
use std::io;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::args::{Arg, Args};
use crate::df;
use crate::logd::{self, Request};
use crate::service;
use crate::settings_edit;
use crate::util::{self, Fail, fail, usage};

/// The settings file.
pub const SETTINGS: &str = "/system/settings/log";

/// How much of the log logd keeps, as the settings file sets it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct Limits {
    /// Days the log keeps records; 0 for no limit.
    pub keep_days: u64,
    /// The most bytes the log and the audit log take together; 0 for no
    /// limit.
    pub max_bytes: u64,
    /// The most they take, in percent of their file system; 0 for no
    /// limit.
    pub max_percent: u64,
    /// Days the audit log keeps security events; 0 for no limit.
    pub audit_keep_days: u64,
}

impl Default for Limits {
    fn default() -> Limits {
        Limits { keep_days: 90, max_bytes: 4 << 30, max_percent: 10, audit_keep_days: 365 }
    }
}

impl Limits {
    /// The limits settings file text `text` sets, the defaults for the
    /// rest, and what is wrong in it, a line each.
    pub fn parse(text: &str) -> (Limits, Vec<String>) {
        let mut limits = Limits::default();
        let mut wrong = Vec::new();
        for (at, line) in text.lines().enumerate() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let Some((key, value)) = line.split_once('=') else {
                wrong.push(format!("{SETTINGS}:{}: not `setting = value`", at + 1));
                continue;
            };
            let (key, value) = (key.trim(), value.trim());
            let number = match key {
                "max-size" => util::parse_size(value),
                _ => value.parse().ok(),
            };
            let slot = match key {
                "keep-days" => &mut limits.keep_days,
                "max-size" => &mut limits.max_bytes,
                "max-percent" => &mut limits.max_percent,
                "audit-keep-days" => &mut limits.audit_keep_days,
                _ => {
                    wrong.push(format!("{SETTINGS}:{}: no setting is called {key}", at + 1));
                    continue;
                }
            };
            match number {
                Some(n) if key != "max-percent" || n <= 100 => *slot = n,
                _ => wrong.push(format!("{SETTINGS}:{}: '{value}' is not a {key}", at + 1)),
            }
        }
        (limits, wrong)
    }

    /// The limits the settings file sets, and what is wrong in it.
    pub fn read() -> (Limits, Vec<String>) {
        match std::fs::read_to_string(SETTINGS) {
            Ok(text) => Limits::parse(&text),
            Err(_) => (Limits::default(), Vec::new()),
        }
    }

    /// The most bytes the log and the audit log may take on a file system
    /// of `fs_size` bytes; 0 for no limit.
    pub fn cap(&self, fs_size: u64) -> u64 {
        let share = match (self.max_percent, fs_size) {
            (0, _) | (_, 0) => 0,
            (percent, size) => (u128::from(size) * u128::from(percent) / 100) as u64,
        };
        match (self.max_bytes, share) {
            (0, cap) | (cap, 0) => cap,
            (a, b) => a.min(b),
        }
    }
}

/// What one trim goes by.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Policy {
    /// Days the log keeps: of 0, today's alone; None for no limit.
    pub days: Option<u64>,
    /// Days the audit log keeps; None for no limit.
    pub audit_days: Option<u64>,
    /// The most bytes both take; 0 for no limit.
    pub cap: u64,
    /// The most bytes the log takes, without the audit log; 0 for no
    /// limit.
    pub log_cap: u64,
}

/// What a trim removed, or would: the day files, the audit log's
/// as `audit/YYYY-MM-DD.jsonl`, and their bytes; and the bytes the log
/// and the audit log take after it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Trimmed {
    pub files: Vec<String>,
    pub bytes: u64,
    pub left: u64,
}

/// The day of day file `name` (`YYYY-MM-DD.jsonl`), in days since the
/// epoch.
fn day_of(name: &str) -> Option<i64> {
    let year = name.get(..4)?.parse().ok()?;
    let month = name.get(5..7)?.parse().ok()?;
    let day = name.get(8..10)?.parse().ok()?;
    Some(logd::days(year, month, day))
}

/// The day files to remove of `log` and `audit`, each (name, bytes),
/// oldest first, to keep to `policy` on day `today` (days since the
/// epoch): first the ones past their days, then the oldest of the log,
/// then of the audit log, while they take more than the caps. Never the
/// one of `today`, nor the ones `writing`.
pub fn plan(
    log: &[(String, u64)],
    audit: &[(String, u64)],
    today: i64,
    policy: &Policy,
    writing: &[&str],
) -> Vec<(String, u64)> {
    let today_name = logd::day_file(today as u64 * 86_400 * 1_000_000_000);
    let kept = |name: &str| name == today_name || writing.contains(&name);
    let old = |name: &str, days: Option<u64>| {
        days.is_some_and(|days| {
            !kept(name) && day_of(name).is_some_and(|day| day < today.saturating_sub_unsigned(days))
        })
    };
    let mut doomed: Vec<(String, u64)> = Vec::new();
    let mut log_left: Vec<&(String, u64)> = Vec::new();
    let mut audit_left: Vec<&(String, u64)> = Vec::new();
    for file in log {
        if old(&file.0, policy.days) { doomed.push(file.clone()) } else { log_left.push(file) }
    }
    for file in audit {
        if old(&file.0, policy.audit_days) {
            doomed.push((format!("{}/{}", logd::AUDIT, file.0), file.1));
        } else {
            audit_left.push(file);
        }
    }
    let mut log_bytes: u64 = log_left.iter().map(|f| f.1).sum();
    let mut total = log_bytes + audit_left.iter().map(|f| f.1).sum::<u64>();
    let over = |total: u64, log_bytes: u64| {
        policy.cap > 0 && total > policy.cap || policy.log_cap > 0 && log_bytes > policy.log_cap
    };
    for file in log_left {
        if !over(total, log_bytes) {
            break;
        }
        if !kept(&file.0) {
            doomed.push(file.clone());
            total -= file.1;
            log_bytes -= file.1;
        }
    }
    for file in audit_left {
        if policy.cap == 0 || total <= policy.cap {
            break;
        }
        if !kept(&file.0) {
            doomed.push((format!("{}/{}", logd::AUDIT, file.0), file.1));
            total -= file.1;
        }
    }
    doomed
}

/// The usage of logtrim.
const USAGE: &str = "usage: logtrim [-n] [-s SIZE] [-d DAYS]\n       logtrim --show [--json]\n       \
                     logtrim set KEY VALUE [KEY VALUE]...";

/// logtrim(8).
pub fn main(args: &mut Args) -> Result<i32, Fail> {
    if let Some(command) = args.subcommand() {
        return if command == "set" {
            set(args)
        } else {
            Err(usage(format!("'{}' is not a command\n{USAGE}", command.to_string_lossy())))
        };
    }
    let mut request = Request { ask: "trim".into(), ..Request::default() };
    let (mut show, mut json, mut trims) = (false, false, false);
    while let Some(arg) = args.next()? {
        match arg {
            Arg::Short('n') => {
                request.dry_run = true;
                trims = true;
            }
            Arg::Long(l) if l == "dry-run" => {
                request.dry_run = true;
                trims = true;
            }
            Arg::Short('s') => {
                request.keep_bytes = Some(size(&args.text()?)?);
                trims = true;
            }
            Arg::Long(l) if l == "size" => {
                request.keep_bytes = Some(size(&args.text()?)?);
                trims = true;
            }
            Arg::Short('d') => {
                request.keep_days = Some(days(&args.text()?)?);
                trims = true;
            }
            Arg::Long(l) if l == "days" => {
                request.keep_days = Some(days(&args.text()?)?);
                trims = true;
            }
            Arg::Long(l) if l == "show" => show = true,
            Arg::Long(l) if l == "json" => json = true,
            other => return Err(args.unknown(&other)),
        }
    }
    if json && !show {
        return Err(usage(format!("--json goes with --show\n{USAGE}")));
    }
    if show {
        if trims {
            return Err(usage(format!(
                "--show does not trim, so it takes no -n, -s or -d\n{USAGE}"
            )));
        }
        return show_log(json);
    }
    let trimmed = ask(&request)?;
    report(&trimmed, request.dry_run, false);
    Ok(0)
}

/// The most days a limit of days may be: ten years.
const DAYS_MOST: u64 = 3650;

/// The keys `set` takes, in the order of the settings file.
const KEYS: [&str; 4] = ["keep-days", "max-size", "max-percent", "audit-keep-days"];

/// The usage of `set`, for the words that follow a mistake.
const SET_USAGE: &str = "usage: logtrim set KEY VALUE [KEY VALUE]...\n       the keys are \
                         keep-days, max-size, max-percent and audit-keep-days";

/// How the log's limits are, for `--show`: what the log takes, the
/// limits the settings file gives, and what they come to.
#[derive(Debug, PartialEq, Eq, Serialize)]
struct Show {
    usage: Used,
    limits: Limits,
    /// The most bytes the log and the audit log may take on their file
    /// system, 0 for no limit.
    cap: u64,
    /// The size of the file system the log is on, null where it is not
    /// known.
    filesystem_size: Option<u64>,
    /// What the settings file has that logd cannot read, a line each.
    wrong: Vec<String>,
}

/// What the log takes.
#[derive(Debug, PartialEq, Eq, Serialize)]
struct Used {
    /// The log and the audit log together.
    bytes: u64,
    files: u64,
    audit_bytes: u64,
    /// Where the day files are.
    directory: &'static str,
}

/// How the log stands, from logd's `usage` and the limits `limits` the
/// settings file gave (`wrong` its mistakes) on a file system of
/// `filesystem_size` bytes.
fn show_of(
    usage: &logd::Usage,
    (limits, wrong): (Limits, Vec<String>),
    filesystem_size: Option<u64>,
) -> Show {
    Show {
        usage: Used {
            bytes: usage.bytes,
            files: usage.files,
            audit_bytes: usage.audit_bytes,
            directory: logd::DIRECTORY,
        },
        limits,
        cap: limits.cap(filesystem_size.unwrap_or(0)),
        filesystem_size,
        wrong,
    }
}

/// A limit as a person reads it: 0 is none.
fn limit_text(value: u64, what: impl Fn(u64) -> String) -> String {
    if value == 0 { "no limit".to_string() } else { what(value) }
}

/// What `--show` says without `--json`.
fn show_text(show: &Show) -> String {
    let (usage, limits) = (&show.usage, &show.limits);
    let files = if usage.files == 1 { "file" } else { "files" };
    let mut text = format!(
        "The log takes up {} in {} {files} in {}, {} of it the audit log.\n",
        util::human(usage.bytes, false),
        usage.files,
        usage.directory,
        util::human(usage.audit_bytes, false)
    );
    let days = |days: u64| format!("{days} days");
    let bytes = |bytes: u64| util::human(bytes, false);
    for (key, value) in [
        ("keep-days", limit_text(limits.keep_days, days)),
        ("max-size", limit_text(limits.max_bytes, bytes)),
        ("max-percent", limit_text(limits.max_percent, |percent| format!("{percent}%"))),
        ("audit-keep-days", limit_text(limits.audit_keep_days, days)),
    ] {
        text += &format!("  {key:<16}{value}\n");
    }
    text += &match (show.cap, show.filesystem_size) {
        (0, _) => "The log and the audit log may take up as much as there is.\n".to_string(),
        (cap, None) => format!("The log and the audit log may take up {}.\n", bytes(cap)),
        (cap, Some(size)) => format!(
            "The log and the audit log may take up {} of the file system's {}.\n",
            bytes(cap),
            bytes(size)
        ),
    };
    for mistake in &show.wrong {
        text += &format!("Not read: {mistake}\n");
    }
    text
}

/// `--show`: tells how the log stands.
fn show_log(json: bool) -> Result<i32, Fail> {
    let usage = logd::disk_usage()?;
    let size = df::statvfs(OsStr::new(logd::DIRECTORY)).ok().map(|space| space.size);
    let show = show_of(&usage, Limits::read(), size);
    if json {
        let line = serde_json::to_string(&show).map_err(|error| fail(error.to_string()))?;
        println!("{line}");
    } else {
        print!("{}", show_text(&show));
    }
    Ok(0)
}

/// Checks that `value` is one `key` takes, and gives it as it is
/// written to the file: a number of days to ten years, a size or 0, a
/// share to 100.
pub(crate) fn checked(key: &str, value: &str) -> Result<String, Fail> {
    let number = |most: u64| {
        let digits = !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit());
        digits.then(|| value.parse::<u64>().ok()).flatten().filter(|&n| n <= most)
    };
    let (ok, what) = match key {
        "keep-days" | "audit-keep-days" => (
            number(DAYS_MOST).map(|n| n.to_string()),
            format!("a number of days, 0 to {DAYS_MOST}"),
        ),
        "max-percent" => {
            (number(100).map(|n| n.to_string()), "a share in percent, 0 to 100".into())
        }
        _ => (
            util::parse_size(value).map(|_| value.to_string()),
            "a size, as 500M or 4G, or 0 for no limit".to_string(),
        ),
    };
    ok.ok_or_else(|| usage(format!("'{value}' is not {what}\n{SET_USAGE}")))
}

/// The changes `words` ask for, KEY VALUE pairs, each key once, checked,
/// in the order of the settings file.
fn changes_of(words: &[String]) -> Result<Vec<(&'static str, String)>, Fail> {
    if words.is_empty() || !words.len().is_multiple_of(2) {
        return Err(usage(format!("say each setting and its value\n{SET_USAGE}")));
    }
    let mut changes: Vec<(&'static str, String)> = Vec::new();
    for pair in words.chunks(2) {
        let Some(key) = KEYS.iter().copied().find(|key| *key == pair[0]) else {
            return Err(usage(format!("'{}' is not a setting\n{SET_USAGE}", pair[0])));
        };
        if changes.iter().any(|(given, _)| *given == key) {
            return Err(usage(format!("{key} is given twice\n{SET_USAGE}")));
        }
        changes.push((key, checked(key, &pair[1])?));
    }
    changes.sort_by_key(|(key, _)| KEYS.iter().position(|known| known == key));
    Ok(changes)
}

/// Whether `line` is the setting `key`, as `Limits::parse` reads one:
/// `key = value`, not a comment.
fn is_setting(line: &str, key: &str) -> bool {
    let line = line.trim();
    !line.starts_with('#') && line.split_once('=').is_some_and(|(name, _)| name.trim() == key)
}

/// The settings file text `text` with each of `changes` made. A setting
/// is changed on its line, the last one it has, which is the one that
/// counts (a later line overrides an earlier one); one the file does not
/// have is added at its end. Every other line, comments and blank lines
/// among them, stays as it is; a setting that has its value already
/// leaves its line alone, so a change that changes nothing returns the
/// text it was given, which is how admind's check mode tells that
/// `logtrim set` would write nothing.
pub(crate) fn edited(text: &str, changes: &[(&str, String)]) -> String {
    let mut lines: Vec<String> = text.split_inclusive('\n').map(String::from).collect();
    for (key, value) in changes {
        let found = lines.iter().rposition(|line| is_setting(line, key));
        let Some(at) = found else {
            if let Some(last) = lines.last_mut()
                && !last.ends_with('\n')
            {
                last.push('\n');
            }
            lines.push(format!("{key} = {value}\n"));
            continue;
        };
        let line = &lines[at];
        let body = line.trim_end_matches(['\r', '\n']);
        let has = body.split_once('=').is_some_and(|(_, has)| has.trim() == value);
        if !has {
            let end = line[body.len()..].to_string();
            lines[at] = format!("{key} = {value}{end}");
        }
    }
    lines.concat()
}

/// `set`: changes the limits in the settings file and has logd read it
/// again.
fn set(args: &mut Args) -> Result<i32, Fail> {
    let mut words = Vec::new();
    while let Some(arg) = args.next()? {
        match arg {
            Arg::Operand(word) => words.push(word.to_string_lossy().into_owned()),
            other => return Err(args.unknown(&other)),
        }
    }
    let changes = changes_of(&words)?;
    if util::own_user() != 0 {
        return Err(fail(format!(
            "only root changes the log's limits: try elevate logtrim set {}",
            words.join(" ")
        )));
    }
    let old = match fs::read_to_string(SETTINGS) {
        Ok(text) => text,
        Err(err) if err.kind() == io::ErrorKind::NotFound => String::new(),
        Err(err) => return Err(util::failed(&SETTINGS, err)),
    };
    let new = edited(&old, &changes);
    if new == old {
        println!("The limits are as asked already: {SETTINGS} is as it was.");
        return Ok(0);
    }
    settings_edit::write(Path::new(SETTINGS), new.as_bytes(), 0o644)
        .map_err(|err| util::failed(&SETTINGS, err))?;
    println!("Changed {SETTINGS}.");
    // logd reads its settings when it starts and when it is told to, and
    // trims at once with what it reads.
    service::ask("reload logd").map_err(|why| {
        fail(format!("{SETTINGS} is changed, but logd did not read it again: {}", why.message))
    })?;
    println!("logd has read the limits again, and trims the log by them.");
    Ok(0)
}

/// A size of `--size` or journalctl's `--vacuum-size`.
pub fn size(text: &str) -> Result<u64, Fail> {
    util::parse_size(text).ok_or_else(|| usage(format!("'{text}' is not a size")))
}

/// A number of days of `--days`.
fn days(text: &str) -> Result<u64, Fail> {
    text.parse().map_err(|_| usage(format!("'{text}' is not a number of days")))
}

/// Sends trim request `request` to logd and returns its answer.
pub fn ask(request: &Request) -> Result<Trimmed, Fail> {
    let mut trimmed = None;
    logd::ask(request, |line| {
        trimmed = serde_json::from_str::<Trimmed>(line).ok();
        Ok(false)
    })?;
    trimmed.ok_or_else(|| fail("logd did not say what it trimmed".into()))
}

/// Says what was trimmed, as logtrim does or, for `journalctl`, as
/// journalctl's `--vacuum-*` do.
pub fn report(trimmed: &Trimmed, dry_run: bool, journalctl: bool) {
    let verb = if dry_run { "Would remove" } else { "Removed" };
    for file in &trimmed.files {
        println!("{verb} {}/{file}", logd::DIRECTORY);
    }
    let freed = util::human(trimmed.bytes, false);
    let left = util::human(trimmed.left, false);
    if journalctl {
        println!("Vacuuming done, freed {freed} of archived journals from {}.", logd::DIRECTORY);
    } else if dry_run {
        println!("Would free {freed}; the log would take up {left}.");
    } else {
        println!("Freed {freed}; the log takes up {left}.");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn files(names: &[(&str, u64)]) -> Vec<(String, u64)> {
        names.iter().map(|(n, b)| (n.to_string(), *b)).collect()
    }

    #[test]
    fn settings() {
        let (limits, wrong) = Limits::parse(
            "# comment\nkeep-days = 30\nmax-size = 512M\nmax-percent=5\naudit-keep-days = 0\n",
        );
        assert!(wrong.is_empty(), "{wrong:?}");
        assert_eq!(
            limits,
            Limits { keep_days: 30, max_bytes: 512 << 20, max_percent: 5, audit_keep_days: 0 }
        );
        let (limits, wrong) = Limits::parse("keep-days = soon\nmax-percent = 200\nloud = 1\nx\n");
        assert_eq!(limits, Limits::default());
        assert_eq!(wrong.len(), 4);
        assert!(wrong[2].contains("no setting is called loud"));
        let (limits, wrong) = Limits::parse("max-size = 4\u{e9}\n");
        assert_eq!((limits, wrong.len()), (Limits::default(), 1));
        assert_eq!(Limits::default().cap(100 << 30), 4 << 30);
        assert_eq!(Limits::default().cap(10 << 30), 1 << 30);
        assert_eq!(Limits::default().cap(0), 4 << 30);
        assert_eq!(Limits { max_bytes: 0, max_percent: 0, ..Limits::default() }.cap(1 << 30), 0);
    }

    #[test]
    fn what_goes() {
        let today = logd::days(2026, 10, 30);
        let log = files(&[
            ("2026-07-01.jsonl", 10),
            ("2026-08-01.jsonl", 20),
            ("2026-10-29.jsonl", 30),
            ("2026-10-30.jsonl", 40),
        ]);
        let audit = files(&[("2025-10-01.jsonl", 1), ("2026-10-30.jsonl", 2)]);
        let names = |doomed: Vec<(String, u64)>| -> Vec<String> {
            doomed.into_iter().map(|f| f.0).collect()
        };
        // By days: the log's of July, and the audit log's of a year ago.
        let policy = Policy { days: Some(90), audit_days: Some(365), ..Policy::default() };
        assert_eq!(
            names(plan(&log, &audit, today, &policy, &[])),
            ["2026-07-01.jsonl", "audit/2025-10-01.jsonl"]
        );
        // By space: the oldest of the log first, then of the audit log;
        // never today's, nor the one being written.
        let policy = Policy { cap: 75, ..Policy::default() };
        assert_eq!(
            names(plan(&log, &audit, today, &policy, &[])),
            ["2026-07-01.jsonl", "2026-08-01.jsonl"]
        );
        let policy = Policy { cap: 1, ..Policy::default() };
        let writing = ["2026-10-29.jsonl"];
        assert_eq!(
            names(plan(&log, &audit, today, &policy, &writing)),
            ["2026-07-01.jsonl", "2026-08-01.jsonl", "audit/2025-10-01.jsonl"]
        );
        // journalctl's --vacuum-size: the log alone.
        let policy = Policy { log_cap: 70, ..Policy::default() };
        assert_eq!(
            names(plan(&log, &audit, today, &policy, &[])),
            ["2026-07-01.jsonl", "2026-08-01.jsonl"]
        );
        assert!(plan(&log, &audit, today, &Policy::default(), &[]).is_empty());
        // Days of 0: every day file of the log but today's.
        let policy = Policy { days: Some(0), ..Policy::default() };
        assert_eq!(
            names(plan(&log, &audit, today, &policy, &[])),
            ["2026-07-01.jsonl", "2026-08-01.jsonl", "2026-10-29.jsonl"]
        );
    }

    /// logd's answer for the log the design's example has.
    fn usage() -> logd::Usage {
        logd::Usage { bytes: 1_288_490_188, files: 92, audit_bytes: 83_886_080 }
    }

    #[test]
    fn show_has_the_usage_the_limits_and_what_they_come_to() {
        let show = show_of(&usage(), Limits::parse(""), Some(255_999_999_999));
        assert_eq!(
            serde_json::to_value(&show).unwrap(),
            serde_json::json!({
                "usage": {"bytes": 1_288_490_188u64, "files": 92, "audit_bytes": 83_886_080u64,
                    "directory": "/system/state/log"},
                "limits": {"keep_days": 90, "max_bytes": 4_294_967_296u64, "max_percent": 10,
                    "audit_keep_days": 365},
                "cap": 4_294_967_296u64,
                "filesystem_size": 255_999_999_999u64,
                "wrong": [],
            })
        );
        // The fields come in the order of the design.
        let line = serde_json::to_string(&show).unwrap();
        assert!(line.starts_with("{\"usage\":{\"bytes\":1288490188,\"files\":92,\"audit_bytes\""));
        assert!(line.contains("\"cap\":4294967296,\"filesystem_size\":255999999999,\"wrong\":[]}"));
    }

    #[test]
    fn a_small_file_system_lowers_the_cap_and_one_that_is_not_known_leaves_it() {
        // A tenth of 10 GiB.
        let small = show_of(&usage(), Limits::parse(""), Some(10 << 30));
        assert_eq!(small.cap, 1 << 30);
        let unknown = show_of(&usage(), Limits::parse(""), None);
        assert_eq!((unknown.cap, unknown.filesystem_size), (4 << 30, None));
        let value = serde_json::to_value(&unknown).unwrap();
        assert_eq!(value["filesystem_size"], serde_json::Value::Null);
        // No limits at all: no cap.
        let free = Limits::parse("max-size = 0\nmax-percent = 0\nkeep-days = 0\n");
        assert_eq!(show_of(&usage(), free, Some(1 << 30)).cap, 0);
    }

    #[test]
    fn show_gives_the_mistakes_of_the_settings_file() {
        let show = show_of(&usage(), Limits::parse("keep-days = soon\nloud = 1\n"), None);
        assert_eq!(show.limits, Limits::default());
        assert_eq!(show.wrong.len(), 2);
        assert!(show.wrong[0].contains("'soon' is not a keep-days"), "{:?}", show.wrong);
        let text = show_text(&show);
        assert!(
            text.contains("Not read: /system/settings/log:1: 'soon' is not a keep-days\n"),
            "{text}"
        );
    }

    #[test]
    fn show_says_it_in_words_and_a_limit_of_nothing_is_no_limit() {
        let limits = Limits::parse("keep-days = 0\nmax-percent = 5\nmax-size = 0\n");
        let text = show_text(&show_of(&usage(), limits, Some(100 << 30)));
        assert_eq!(
            text,
            "The log takes up 1.2G in 92 files in /system/state/log, 80M of it the audit log.\n\
             \x20 keep-days       no limit\n\
             \x20 max-size        no limit\n\
             \x20 max-percent     5%\n\
             \x20 audit-keep-days 365 days\n\
             The log and the audit log may take up 5.0G of the file system's 100G.\n"
        );
        let none = Limits::parse("max-size = 0\nmax-percent = 0\n");
        let text = show_text(&show_of(&usage(), none, None));
        assert!(text.ends_with("may take up as much as there is.\n"), "{text}");
    }

    #[test]
    fn a_value_is_checked_by_what_its_key_takes() {
        for (key, good) in [
            ("keep-days", "0"),
            ("keep-days", "3650"),
            ("audit-keep-days", "365"),
            ("max-percent", "0"),
            ("max-percent", "100"),
            ("max-size", "0"),
            ("max-size", "4G"),
            ("max-size", "500MB"),
            ("max-size", "123"),
        ] {
            assert_eq!(checked(key, good).unwrap(), good, "{key} {good}");
        }
        for (key, bad) in [
            ("keep-days", "3651"),
            ("keep-days", "-1"),
            ("keep-days", "+5"),
            ("keep-days", "5d"),
            ("keep-days", ""),
            ("keep-days", "1_0"),
            ("audit-keep-days", "99999999999999999999999"),
            ("max-percent", "101"),
            ("max-percent", "5%"),
            ("max-size", "lots"),
            ("max-size", "4G "),
            ("max-size", "1.5G"),
            ("max-size", ""),
            ("max-size", "4G\nkeep-days = 0"),
        ] {
            let refusal = checked(key, bad).err().unwrap_or_else(|| panic!("{key} {bad:?}"));
            assert!(refusal.usage && refusal.message.contains("is not"), "{key} {bad:?}");
        }
    }

    fn words(text: &str) -> Vec<String> {
        text.split_whitespace().map(String::from).collect()
    }

    #[test]
    fn the_changes_are_pairs_of_a_setting_and_a_value() {
        let changes = changes_of(&words("max-percent 5 keep-days 60 max-size 20G")).unwrap();
        // In the order of the file, whatever the order asked.
        assert_eq!(
            changes,
            [
                ("keep-days", "60".to_string()),
                ("max-size", "20G".to_string()),
                ("max-percent", "5".to_string())
            ]
        );
        for bad in [
            "",
            "keep-days",
            "keep-days 1 max-size",
            "keep-day 1",
            "keep-days 1 keep-days 2",
            "keep-days soon",
        ] {
            let refusal = changes_of(&words(bad)).err().unwrap_or_else(|| panic!("{bad}"));
            assert!(refusal.usage, "{bad}");
        }
    }

    #[test]
    fn a_setting_changes_on_its_line_and_everything_else_stays() {
        let old = "# How much.\nkeep-days = 90\n\n# Space.\nmax-size = 4G\nmax-percent = 10\n";
        let new = edited(old, &[("keep-days", "60".into())]);
        assert_eq!(
            new,
            "# How much.\nkeep-days = 60\n\n# Space.\nmax-size = 4G\nmax-percent = 10\n"
        );
        let new = edited(old, &[("max-size", "20G".into()), ("max-percent", "0".into())]);
        assert_eq!(
            new,
            "# How much.\nkeep-days = 90\n\n# Space.\nmax-size = 20G\nmax-percent = 0\n"
        );
    }

    #[test]
    fn a_setting_the_file_lacks_is_added_at_its_end() {
        let new = edited("keep-days = 90\n", &[("audit-keep-days", "30".into())]);
        assert_eq!(new, "keep-days = 90\naudit-keep-days = 30\n");
        // A file that does not end in a newline gets one before the line.
        let new = edited("keep-days = 90", &[("max-percent", "5".into())]);
        assert_eq!(new, "keep-days = 90\nmax-percent = 5\n");
        assert_eq!(edited("", &[("keep-days", "7".into())]), "keep-days = 7\n");
        // A comment is not the setting.
        let new = edited("# keep-days = 5\n", &[("keep-days", "7".into())]);
        assert_eq!(new, "# keep-days = 5\nkeep-days = 7\n");
    }

    #[test]
    fn a_setting_given_twice_changes_on_the_line_that_counts() {
        // Limits::parse lets the later line win, so that is the one.
        let old = "keep-days = 90\nmax-size = 1G\nkeep-days = 30\n";
        let new = edited(old, &[("keep-days", "60".into())]);
        assert_eq!(new, "keep-days = 90\nmax-size = 1G\nkeep-days = 60\n");
        assert_eq!(Limits::parse(&new).0.keep_days, 60);
        // And the line that counts may be one logd cannot read.
        let old = "keep-days = 90\nkeep-days = soon\n";
        let new = edited(old, &[("keep-days", "60".into())]);
        assert_eq!(Limits::parse(&new), (Limits { keep_days: 60, ..Limits::default() }, vec![]));
    }

    #[test]
    fn a_change_that_changes_nothing_gives_back_the_text() {
        for old in [
            "keep-days = 90\n",
            "keep-days=90\n",
            "  keep-days   =   90  \n",
            "keep-days = 90",
            "# c\r\nkeep-days = 90\r\n",
        ] {
            assert_eq!(edited(old, &[("keep-days", "90".into())]), old, "{old:?}");
        }
        let old = "# c\nkeep-days = 90\nmax-size = 4G\n";
        assert_eq!(edited(old, &[("keep-days", "90".into()), ("max-size", "4G".into())]), old);
    }

    #[test]
    fn the_line_ending_of_a_line_stays() {
        let new = edited(
            "# c\r\nkeep-days = 90\r\nmax-size = 4G",
            &[("keep-days", "7".into()), ("max-size", "1G".into())],
        );
        assert_eq!(new, "# c\r\nkeep-days = 7\r\nmax-size = 1G");
    }

    #[test]
    fn what_is_written_reads_back_as_asked() {
        let old =
            fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../defaults/log")).unwrap();
        let changes =
            changes_of(&words("keep-days 60 max-size 20G max-percent 0 audit-keep-days 0"))
                .unwrap();
        let new = edited(&old, &changes);
        let (limits, wrong) = Limits::parse(&new);
        assert!(wrong.is_empty(), "{wrong:?}");
        assert_eq!(
            limits,
            Limits { keep_days: 60, max_bytes: 20 << 30, max_percent: 0, audit_keep_days: 0 }
        );
        // The shipped file's comments are all still there.
        for comment in old.lines().filter(|line| line.starts_with('#')) {
            assert!(new.lines().any(|line| line == comment), "{comment}");
        }
        // Asking for what it says changes nothing.
        let same =
            changes_of(&words("keep-days 90 max-size 4G max-percent 10 audit-keep-days 365"))
                .unwrap();
        assert_eq!(edited(&old, &same), old);
    }
}
