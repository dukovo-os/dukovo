// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! `website count` (ADR 0570): how many requests each site answered on a
//! day and how big its folder is, counted once a day and kept for
//! `website list` and the web console.
//!
//! nginx writes each request of a site to a file of the site's own in its
//! data folder ([`LOGS`]): the time, the status and the bytes sent, and no
//! address, path or agent. logturn(8) turns the file over when a day has
//! passed since it last did, not at midnight, and keeps two turned copies,
//! `NAME.log.1` and `NAME.log.2.zst`. So the lines of one UTC day can be in
//! the live file and in both copies: a count reads the three and keeps the
//! lines of the day it is asked for, by the time each line carries. The
//! daily count asks for yesterday, `--now` for today so far.
//!
//! The folder is nginx's, and the account that runs nginx may put a link
//! in it, so it is held open and every name is looked up beneath it
//! without following a link, as the sites' settings folder is. A file is
//! read as far as [`READ_BYTES`] or [`READ_TIME`] go, and what it gave is
//! then marked as at least. The size of each site's folder is walked by
//! the walk of the bundles' sizes, in a process of its own at the idle
//! priority, with its limits.
//!
//! What is counted is kept in [`COUNTS`], which anyone may read: it says
//! how many and how big, never who or what. A site keeps [`DAYS_KEPT`]
//! days, and what is kept of a site that is gone stays [`GONE_DAYS`] more.

use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{self, BufRead, BufReader, Read};
use std::path::Path;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use super::change::{Ctx, Refusal, Site, load_with};
use super::host::{Paths, STATE};
use super::plan::LOGS;
use super::stage::logs_dir;
use crate::bundles::folder::{Dir, fd_stat};
use crate::bundles::sizes::{Line, Request};
use crate::util::{human, strip_os_error};
use crate::{logd, settings_edit};

/// Where the counts are kept.
pub const COUNTS: &str = "/system/state/websites/counts.json";

/// The most days kept of a site.
pub const DAYS_KEPT: usize = 400;

/// How many days what is kept of a site that is gone stays.
pub const GONE_DAYS: u64 = 30;

/// The most days of a site that `list` shows.
pub const DAYS_SHOWN: usize = 30;

/// The most bytes read of a log file, as it is when it is read.
pub const READ_BYTES: u64 = 2 * 1024 * 1024 * 1024;

/// The most time spent reading a log file.
pub const READ_TIME: Duration = Duration::from_secs(60);

/// The seconds of a day.
const DAY: i64 = 86_400;

/// The milliseconds of a day.
const DAY_MS: u64 = 86_400_000;

/// The longest line of a log file that is looked at: a line of the format
/// is under 50 bytes, so a longer one is not a request of it.
const LINE_MAX: usize = 128;

/// How many lines are read between two looks at the clock.
const LOOK_EVERY: u64 = 1024;

/// The widest window a compressed copy may ask the decoder for, as `2 **`
/// this many bytes. logturn's level makes a window of 2 MiB; a copy that
/// asks for more is not one logturn made, and nginx's account may write
/// there.
const WINDOW_LOG_MAX: u32 = 23;

/// What a day's requests of a site came to.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct Day {
    /// The UTC day, as `2026-11-15`.
    pub day: String,
    /// The requests answered with a status of 200 to 399.
    pub visits: u64,
    /// The requests answered with a status of 400 to 599.
    pub errors: u64,
    /// The bytes of the bodies sent for both.
    pub bytes: u64,
    /// False when a file was read only as far as the limits went, or could
    /// not be read, so that the numbers are at least.
    pub complete: bool,
}

/// What a site's folder takes, as the walk of the bundles' sizes found.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct Size {
    /// What the folder costs on disk, in bytes.
    pub bytes: u64,
    /// The files in it.
    pub files: u64,
    /// False when a limit stopped the walk, so that this is at least.
    pub complete: bool,
    /// When it was walked, in milliseconds since 1970.
    pub measured_ms: u64,
}

/// What is kept of a site.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct SiteCounts {
    /// Its days, the newest first.
    #[serde(default)]
    pub days: Vec<Day>,
    /// Its folder, as last walked.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<Size>,
    /// When the site was first missed, in milliseconds since 1970; none
    /// while it is there.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gone_since_ms: Option<u64>,
}

/// What `list --json` says of a site's days.
#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct Visits {
    /// The visits of the day before today; 0 when it was not counted.
    pub yesterday: u64,
    /// The visits of the 7 days before today, and of today when it was
    /// counted.
    pub last_7_days: u64,
    /// The visits of the 30 days before today, and of today when it was
    /// counted.
    pub last_30_days: u64,
    /// Whether every day of `days` was counted to its end.
    pub complete: bool,
    /// The newest days, each as `[day, visits, errors, bytes]`.
    pub days: Vec<(String, u64, u64, u64)>,
}

/// `counts.json`.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct Counts {
    /// 1.
    pub format: u32,
    /// What is kept of each site, by its name.
    pub sites: BTreeMap<String, SiteCounts>,
}

impl Default for Counts {
    fn default() -> Counts {
        Counts { format: 1, sites: BTreeMap::new() }
    }
}

impl Counts {
    /// Reads the file's text.
    ///
    /// # Errors
    ///
    /// What is wrong with it, when it is not what [`Counts::text`] writes.
    pub fn parse(text: &str) -> Result<Counts, String> {
        let counts: Counts = serde_json::from_str(text).map_err(|error| error.to_string())?;
        if counts.format != 1 {
            return Err(format!("its format is {}, and only 1 is read", counts.format));
        }
        Ok(counts)
    }

    /// The file's text.
    ///
    /// # Errors
    ///
    /// When it cannot be written as JSON.
    pub fn text(&self) -> Result<String, String> {
        serde_json::to_string(self).map_err(|error| error.to_string())
    }

    /// Puts `day` among the days of the site `name`, in place of the day
    /// of that date, and keeps the newest [`DAYS_KEPT`].
    pub fn record(&mut self, name: &str, day: Day) {
        let site = self.sites.entry(name.to_string()).or_default();
        match site.days.iter_mut().find(|kept| kept.day == day.day) {
            Some(kept) => *kept = day,
            None => site.days.push(day),
        }
        site.days.sort_by(|a, b| b.day.cmp(&a.day));
        site.days.truncate(DAYS_KEPT);
    }

    /// Notes which sites are `present` at `now_ms`, and drops what is kept
    /// of a site that has been gone longer than [`GONE_DAYS`].
    pub fn forget_gone(&mut self, present: &[&str], now_ms: u64) {
        for (name, site) in &mut self.sites {
            if present.contains(&name.as_str()) {
                site.gone_since_ms = None;
            } else {
                site.gone_since_ms.get_or_insert(now_ms);
            }
        }
        self.sites.retain(|_, site| {
            site.gone_since_ms
                .is_none_or(|since| now_ms.saturating_sub(since) <= GONE_DAYS * DAY_MS)
        });
    }

    /// Takes in the folders a walk measured at `now_ms`: each one it
    /// finished replaces what was kept of the site, and a site it did not
    /// reach keeps its old size.
    pub fn apply_sizes(&mut self, lines: &[Line], now_ms: u64) {
        for line in lines {
            let Line::App { id, walked, .. } = line else { continue };
            self.sites.entry(id.clone()).or_default().size = Some(Size {
                bytes: walked.bytes,
                files: walked.files,
                complete: walked.complete,
                measured_ms: now_ms,
            });
        }
    }
}

impl SiteCounts {
    /// What `list` says of the days as of `today`, a day counted from
    /// 1970; none for a site with no day.
    pub fn visits(&self, today: i64) -> Option<Visits> {
        if self.days.is_empty() {
            return None;
        }
        let numbered: Vec<(i64, &Day)> =
            self.days.iter().filter_map(|day| Some((day_number(&day.day)?, day))).collect();
        let sum = |span: i64| {
            numbered
                .iter()
                .filter(|(number, _)| (today - span..=today).contains(number))
                .fold(0u64, |sum, (_, day)| sum.saturating_add(day.visits))
        };
        let shown = self.days.iter().take(DAYS_SHOWN);
        Some(Visits {
            yesterday: numbered
                .iter()
                .find(|(number, _)| *number == today - 1)
                .map_or(0, |(_, day)| day.visits),
            last_7_days: sum(7),
            last_30_days: sum(30),
            complete: shown.clone().all(|day| day.complete),
            days: shown.map(|day| (day.day.clone(), day.visits, day.errors, day.bytes)).collect(),
        })
    }
}

/// The date of `day`, counted from 1970, as `2026-11-15`.
pub fn day_text(day: i64) -> String {
    let (year, month, date) = logd::civil(day);
    format!("{year:04}-{month:02}-{date:02}")
}

/// The number of the day `text` names, counted from 1970; none for text
/// that is not a date as [`day_text`] writes it.
pub fn day_number(text: &str) -> Option<i64> {
    let mut parts = text.splitn(3, '-');
    let year: i64 = parts.next().filter(|part| part.len() == 4)?.parse().ok()?;
    let month: u32 = parts.next().filter(|part| part.len() == 2)?.parse().ok()?;
    let date: u32 = parts.next().filter(|part| part.len() == 2)?.parse().ok()?;
    ((1..=12).contains(&month) && (1..=31).contains(&date)).then(|| logd::days(year, month, date))
}

/// What is kept, read from `paths`: none kept when there is no file.
///
/// # Errors
///
/// When the file cannot be read, or is not what is written there.
pub fn kept(paths: &Paths) -> Result<Counts, String> {
    let words = |why: String| format!("{COUNTS}: {why}");
    match fs::read(paths.real(COUNTS)) {
        Ok(bytes) => Counts::parse(&String::from_utf8_lossy(&bytes)).map_err(words),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Counts::default()),
        Err(error) => Err(words(strip_os_error(&error))),
    }
}

/// Keeps `counts`, for anyone to read.
fn keep(paths: &Paths, counts: &Counts) -> Result<(), Refusal> {
    let words = |error: io::Error| Refusal::plain(format!("{COUNTS}: {}", strip_os_error(&error)));
    let folder = paths.real(STATE);
    fs::create_dir_all(&folder).map_err(words)?;
    // The lock is in the folder too, and is root's alone as a file.
    set_mode(&folder, 0o755).map_err(words)?;
    let text = counts.text().map_err(Refusal::plain)?;
    settings_edit::write(&paths.real(COUNTS), text.as_bytes(), 0o644).map(|_| ()).map_err(words)
}

/// Gives `path` the mode `mode`.
fn set_mode(path: &Path, mode: u32) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(mode))
}

/// The request one line of a site's log holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Logged {
    /// The day it was made, counted from 1970.
    day: i64,
    /// What it was answered.
    status: u16,
    /// The bytes of the body.
    bytes: u64,
}

/// The number the digits `word` are; none when it holds anything else.
fn number(word: &str) -> Option<u64> {
    if word.is_empty() || !word.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    word.parse().ok()
}

/// The request in `line`, which is `$msec $status $body_bytes_sent`: seconds
/// and three digits of a thousandth, a status of three digits, and the
/// bytes. None for anything else.
fn parse(line: &[u8]) -> Option<Logged> {
    let mut words = std::str::from_utf8(line).ok()?.split(' ');
    let (time, status, bytes) = (words.next()?, words.next()?, words.next()?);
    if words.next().is_some() {
        return None;
    }
    let (seconds, thousandths) = time.split_once('.')?;
    if thousandths.len() != 3 || number(thousandths).is_none() || status.len() != 3 {
        return None;
    }
    let seconds = i64::try_from(number(seconds)?).ok()?;
    Some(Logged {
        day: seconds.div_euclid(DAY),
        status: u16::try_from(number(status)?).ok()?,
        bytes: number(bytes)?,
    })
}

/// The limits a file is read under.
#[derive(Clone, Copy, Debug)]
pub struct Limits {
    /// The most bytes read.
    pub bytes: u64,
    /// The most time spent.
    pub time: Duration,
}

impl Limits {
    /// What a file is read under unless a test says otherwise.
    pub const STANDARD: Limits = Limits { bytes: READ_BYTES, time: READ_TIME };
}

/// What one day of a site's log came to.
#[derive(Debug, Default, PartialEq, Eq)]
struct Tally {
    visits: u64,
    errors: u64,
    bytes: u64,
    /// Whether a file of the site's log was there.
    found: bool,
    /// False when a file was cut short or could not be read.
    incomplete: bool,
    /// What could not be read, as words.
    problems: Vec<String>,
}

impl Tally {
    /// Counts the request `logged`, when its status is one of a visit or
    /// of an error.
    fn add(&mut self, logged: Logged) {
        match logged.status {
            200..=399 => self.visits = self.visits.saturating_add(1),
            400..=599 => self.errors = self.errors.saturating_add(1),
            _ => return,
        }
        self.bytes = self.bytes.saturating_add(logged.bytes);
    }

    /// What was counted, as the day `day` is kept.
    fn day(&self, day: i64) -> Day {
        Day {
            day: day_text(day),
            visits: self.visits,
            errors: self.errors,
            bytes: self.bytes,
            complete: !self.incomplete,
        }
    }
}

/// Reads on to the end of the line that `reader` is in, as many bytes as
/// that is.
fn skip_line<R: BufRead>(reader: &mut R) -> io::Result<u64> {
    let mut skipped = 0;
    loop {
        let chunk = reader.fill_buf()?;
        if chunk.is_empty() {
            return Ok(skipped);
        }
        let (used, done) = match chunk.iter().position(|byte| *byte == b'\n') {
            Some(at) => (at + 1, true),
            None => (chunk.len(), false),
        };
        reader.consume(used);
        skipped += used as u64;
        if done {
            return Ok(skipped);
        }
    }
}

/// Counts into `tally` the requests of `day` that `reader` holds, until it
/// ends or `limits` stop it. A line that is not a request of the format
/// is passed over, and so is a last line that is not ended: it is cut, or
/// still being written, and a file written to has it whole at the next
/// count.
///
/// # Errors
///
/// What reading says.
fn read_requests<R: BufRead>(
    reader: &mut R,
    day: i64,
    limits: Limits,
    tally: &mut Tally,
) -> io::Result<()> {
    let began = Instant::now();
    let (mut read, mut lines) = (0u64, 0u64);
    let mut line = Vec::with_capacity(LINE_MAX + 1);
    loop {
        let late = lines.is_multiple_of(LOOK_EVERY) && began.elapsed() >= limits.time;
        if read >= limits.bytes || late {
            tally.incomplete |= !reader.fill_buf()?.is_empty();
            return Ok(());
        }
        lines += 1;
        line.clear();
        let got = reader.by_ref().take(LINE_MAX as u64 + 1).read_until(b'\n', &mut line)?;
        if got == 0 {
            return Ok(());
        }
        read += got as u64;
        match line.last() {
            Some(b'\n') => {
                if let Some(logged) = parse(&line[..line.len() - 1])
                    && logged.day == day
                {
                    tally.add(logged);
                }
            }
            _ if got > LINE_MAX => read += skip_line(reader)?,
            _ => return Ok(()),
        }
    }
}

/// Opens the log file `file` of `logs`, not through a link: none when it
/// is not there.
///
/// # Errors
///
/// When it is a link, or not a file, or cannot be opened.
fn open_log(logs: &Dir, file: &str) -> io::Result<Option<File>> {
    let fd = match logs.open_at(file, false) {
        Ok(fd) => fd,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    if fd_stat(&fd)?.st_mode & libc::S_IFMT != libc::S_IFREG {
        return Err(io::Error::other("it is not a file"));
    }
    Ok(Some(File::from(fd)))
}

/// Counts into `tally` the requests of `day` in the log file `file`, which
/// holds a zstd frame when `packed`.
fn count_file(logs: &Dir, file: &str, packed: bool, day: i64, limits: Limits, tally: &mut Tally) {
    let counted = open_log(logs, file).and_then(|opened| {
        let Some(handle) = opened else { return Ok(false) };
        let source: Box<dyn Read> = if packed {
            let mut decoder = zstd::stream::read::Decoder::new(handle)?;
            decoder.window_log_max(WINDOW_LOG_MAX)?;
            Box::new(decoder)
        } else {
            Box::new(handle)
        };
        read_requests(&mut BufReader::with_capacity(64 * 1024, source), day, limits, tally)?;
        Ok(true)
    });
    match counted {
        Ok(found) => tally.found |= found,
        Err(error) => {
            tally.found = true;
            tally.incomplete = true;
            tally.problems.push(format!("{file}: {}", strip_os_error(&error)));
        }
    }
}

/// What the files of the site `name` in `logs` hold of `day`: its live
/// file and its two turned copies, the older of them compressed once
/// logturn has done so. None when it has no file.
fn tally_site(logs: &Dir, name: &str, day: i64, limits: Limits) -> Option<Tally> {
    let mut tally = Tally::default();
    let live = format!("{name}.log");
    count_file(logs, &format!("{live}.2.zst"), true, day, limits, &mut tally);
    // The compressed copy takes the place of the plain one when it is made,
    // and both are there for the moment between.
    if !tally.found {
        count_file(logs, &format!("{live}.2"), false, day, limits, &mut tally);
    }
    count_file(logs, &format!("{live}.1"), false, day, limits, &mut tally);
    count_file(logs, &live, false, day, limits, &mut tally);
    tally.found.then_some(tally)
}

/// Whether `file` is a file of the site `name`'s log: `NAME.log`, or a copy
/// of it that logturn(8) made or is making, `NAME.log.1`, `NAME.log.2.zst`
/// and `NAME.log.2.zst.new`.
pub fn is_log_of(file: &str, name: &str) -> bool {
    let Some(rest) = file.strip_prefix(name).and_then(|rest| rest.strip_prefix(".log")) else {
        return false;
    };
    if rest.is_empty() {
        return true;
    }
    let Some(copy) = rest.strip_prefix('.') else { return false };
    let copy = copy.strip_suffix(".new").unwrap_or(copy);
    let copy = copy.strip_suffix(".zst").unwrap_or(copy);
    number(copy).is_some()
}

/// Removes the files of the site `name`'s log, not through a link, as the
/// site is gone and with it the reason to keep a day of its requests.
///
/// # Errors
///
/// What stopped it.
pub fn remove_logs(paths: &Paths, name: &str) -> Result<(), String> {
    let words = |error: io::Error| format!("{LOGS}: {}", strip_os_error(&error));
    let Some(logs) = logs_dir(paths, false).map_err(words)? else { return Ok(()) };
    for file in logs.names().map_err(words)? {
        if is_log_of(&file, name) {
            logs.unlink(&file)
                .map_err(|error| format!("{LOGS}/{file}: {}", strip_os_error(&error)))?;
        }
    }
    Ok(())
}

/// What a count says of one site.
#[derive(Debug, Serialize)]
pub struct SiteReport {
    /// The site's name.
    pub name: String,
    /// What its log came to for the day; none when it has no log file.
    pub day: Option<Day>,
    /// Its folder, as last walked.
    pub size: Option<Size>,
    /// What could not be read of it.
    pub problems: Vec<String>,
}

/// What a count says.
#[derive(Debug, Serialize)]
pub struct Report {
    /// The UTC day that was counted.
    pub day: String,
    /// Each site.
    pub sites: Vec<SiteReport>,
    /// What went wrong that is no site's.
    pub problems: Vec<String>,
}

/// `bytes`, as the lines say them.
fn sized(bytes: u64) -> String {
    if bytes < 1024 { format!("{bytes} bytes") } else { human(bytes, false) }
}

impl SiteReport {
    /// What was counted of the site, in a line.
    fn line(&self) -> String {
        let mut parts = Vec::new();
        match &self.day {
            Some(day) => {
                let least = if day.complete { "" } else { "at least " };
                parts.push(format!(
                    "{}: {least}{} visits, {} errors, {} sent",
                    day.day,
                    day.visits,
                    day.errors,
                    sized(day.bytes)
                ));
            }
            None => parts.push("no log of its requests".to_string()),
        }
        if let Some(size) = &self.size {
            let least = if size.complete { "" } else { "at least " };
            parts.push(format!("folder {least}{} in {} files", sized(size.bytes), size.files));
        }
        parts.extend(self.problems.iter().map(|problem| format!("not read: {problem}")));
        format!("{}: {}", self.name, parts.join("; "))
    }
}

impl Report {
    /// The report as JSON.
    ///
    /// # Errors
    ///
    /// When it cannot be written as JSON.
    pub fn json(&self) -> Result<String, String> {
        serde_json::to_string_pretty(self).map_err(|error| error.to_string())
    }

    /// The report in words, a line for each site and each problem.
    pub fn plain(&self) -> String {
        let sites = self.sites.iter().map(SiteReport::line);
        let problems = self.problems.iter().cloned();
        let mut text = String::new();
        for line in sites.chain(problems) {
            text.push_str(&line);
            text.push('\n');
        }
        text
    }
}

/// The folders to walk: each site's own, asked for under its name.
fn folders(sites: &[&Site]) -> Vec<Request> {
    sites
        .iter()
        .filter_map(|site| {
            let folder = site.facts.as_ref().ok()?.folder.clone()?;
            Some(Request { id: site.name.clone(), folders: vec![folder] })
        })
        .collect()
}

/// Counts the requests every site's log holds of yesterday, or of today so
/// far when `today` says so, and walks the sites' folders, and keeps both.
///
/// # Errors
///
/// When the sites cannot be read or what is kept cannot be written.
pub fn count(ctx: &Ctx<'_>, today: bool, limits: Limits) -> Result<Report, Refusal> {
    let state = load_with(ctx, false)?;
    let now = ctx.host.now();
    let day = now.div_euclid(DAY) - i64::from(!today);
    let mut report = Report { day: day_text(day), sites: Vec::new(), problems: Vec::new() };
    let mut counts = kept(ctx.paths).unwrap_or_else(|why| {
        report.problems.push(format!("{why}; what it kept is made anew"));
        Counts::default()
    });
    // A name that is not a site's name is one `website` does not write: it
    // makes no log file of its own, and none is looked for.
    let sites: Vec<&Site> =
        state.sites.iter().filter(|site| super::check_site_name(&site.name).is_ok()).collect();
    let logs = logs_dir(ctx.paths, false).unwrap_or_else(|error| {
        report.problems.push(format!("{LOGS}: {}", strip_os_error(&error)));
        None
    });
    for site in &sites {
        let tally = logs.as_ref().and_then(|logs| tally_site(logs, &site.name, day, limits));
        let counted = tally.as_ref().map(|tally| tally.day(day));
        if let Some(counted) = &counted {
            counts.record(&site.name, counted.clone());
        }
        report.sites.push(SiteReport {
            name: site.name.clone(),
            day: counted,
            size: None,
            problems: tally.map_or_else(Vec::new, |tally| tally.problems),
        });
    }
    let names: Vec<&str> = state.sites.iter().map(|site| site.name.as_str()).collect();
    let millis = |seconds: i64| u64::try_from(seconds).unwrap_or(0).saturating_mul(1000);
    counts.forget_gone(&names, millis(now));
    keep(ctx.paths, &counts)?;
    // The walk takes the longest, so what was counted is kept before it.
    let asked = folders(&sites);
    if !asked.is_empty() {
        match ctx.host.measure(&asked) {
            Ok(said) => {
                for line in &said {
                    if let Line::Error(why) = line {
                        report.problems.push(format!("the sites' folders: {why}"));
                    }
                }
                counts.apply_sizes(&said, millis(ctx.host.now()));
                keep(ctx.paths, &counts)?;
            }
            Err(why) => {
                report.problems.push(format!("the sites' folders were not measured: {why}"));
            }
        }
    }
    for site in &mut report.sites {
        site.size = counts.sites.get(&site.name).and_then(|counted| counted.size);
    }
    // A machine with no website has nothing to say each day.
    if !report.sites.is_empty() {
        ctx.host.log(&format!("counted {} websites for {}", report.sites.len(), report.day));
    }
    for problem in &report.problems {
        ctx.host.log(&format!("count: {problem}"));
    }
    for problem in report.sites.iter().flat_map(|site| &site.problems) {
        ctx.host.log(&format!("count: {problem}"));
    }
    Ok(report)
}

#[cfg(test)]
mod tests;
