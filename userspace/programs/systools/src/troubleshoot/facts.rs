// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! What is known of one start (ADR 0474), which the signatures are
//! matched against: its record and what changed since the last good
//! start, its log lines, and, for the newest start only, since they are
//! read as they are now, which services failed and how full each
//! filesystem is. [`Facts`] is data: `troubleshoot --save-facts` writes
//! it to a file and `--facts` reads it back, so that each shipped
//! signature is tested against the facts of a planted fault.

use std::io::Write as _;
use std::os::unix::fs::OpenOptionsExt as _;
use std::path::Path;

use serde::{Deserialize, Serialize};
use spark_api::show::{self, State};

use crate::logd::{self, Record, Request};
use crate::sysupdate::health;
use crate::sysupdate::starts::{self, Start};
use crate::util::{self, Fail, fail};
use crate::{df, power, proc};

/// The most lines of a start's log that are read: the newest of its
/// window.
pub const LOG_LINES: usize = 20_000;

/// How long after the health check began, in seconds, the window of a
/// start's log reaches. A record has no time its check ended, and a boot
/// may have run for days after it, its log long past [`LOG_LINES`] lines:
/// the newest lines of the boot would hold nothing of the start. The
/// check is over well within an hour: it has its deadline, ten minutes
/// unless health.toml says otherwise, its commands' timeouts, and it
/// waits for a stage that holds the slots' state for half an hour at most.
const WINDOW_AFTER: i64 = 3600;

/// The most bytes of a line that are kept.
pub const LINE_BYTES: usize = 400;

/// How many seconds a start's record may be from its boot's records.
const SLACK: i64 = 5;

/// Where `--save-facts` and `--facts` keep the facts, in their folder.
pub const FILE: &str = "facts.json";

/// What is known of one start.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Facts {
    /// Its record.
    pub start: Start,
    /// The kinds of item that differ from the last good start's.
    pub changed: Vec<String>,
    /// Its log, oldest first.
    pub log: Vec<Line>,
    /// The services that failed, when this is the newest start.
    pub failed: Vec<String>,
    /// The services the health check needs.
    pub essential: Vec<String>,
    /// How full the filesystems are, when this is the newest start.
    pub full: Vec<Full>,
    /// What could not be found out, in words.
    #[serde(default)]
    pub missing: Vec<String>,
}

/// A line of the log.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Line {
    /// Its level's number, as logd numbers them: 3 for an error.
    pub level: u8,
    /// `kernel`, `service` or `program`.
    pub from: String,
    /// The service's or program's name; `kernel` for the kernel's.
    pub name: String,
    /// What it says, to [`LINE_BYTES`] bytes.
    pub text: String,
}

/// How full a filesystem is.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Full {
    pub path: String,
    /// Of what the people who use it can have, rounded up, as df says.
    pub percent: u8,
}

impl Line {
    /// The line `record` makes.
    #[must_use]
    pub fn of(record: &Record) -> Line {
        let mut end = record.text.len().min(LINE_BYTES);
        while !record.text.is_char_boundary(end) {
            end -= 1;
        }
        Line {
            level: record.level_number(),
            from: record.from.clone(),
            name: record.name.clone(),
            text: record.text[..end].to_string(),
        }
    }
}

impl Facts {
    /// Facts of `start` with nothing known of its log or the system now.
    #[must_use]
    pub fn of(start: &Start) -> Facts {
        Facts {
            start: start.clone(),
            changed: Vec::new(),
            log: Vec::new(),
            failed: Vec::new(),
            essential: Vec::new(),
            full: Vec::new(),
            missing: Vec::new(),
        }
    }

    /// Facts of `start`, one of `records`, as the system has them: what
    /// cannot be found out is said in `missing`. `paths` are those the
    /// signatures ask the fullness of, which are looked at as well as the
    /// mounts, since a service sees some of its own.
    #[must_use]
    pub fn gather(records: &[Start], start: &Start, paths: &[String]) -> Facts {
        let mut facts = Facts::of(start);
        if let Some(good) = starts::last_good(records, start.number) {
            facts.changed = starts::changes(records, good, start)
                .into_iter()
                .map(|change| change.kind)
                .collect();
            facts.changed.sort();
            facts.changed.dedup();
        }
        match log_of(start) {
            Ok(Some(log)) => facts.log = log,
            Ok(None) => facts.missing.push("no boot of the log holds this start".into()),
            Err(why) => facts.missing.push(format!("the log: {}", why.message)),
        }
        // What is read as it is now says something of the start only while
        // it is the newest.
        if records.last().is_some_and(|last| last.number == start.number) {
            // Read quietly: `--json` output holds nothing but the facts.
            let (config, why) = health::Config::read();
            facts.essential = config.essential;
            facts.missing.extend(why);
            match services_failed() {
                Ok(failed) => facts.failed = failed,
                Err(why) => facts.missing.push(format!("the services: {}", why.message)),
            }
            match filesystems_full(paths) {
                Ok(full) => facts.full = full,
                Err(why) => facts.missing.push(format!("the filesystems: {}", why.message)),
            }
        }
        facts
    }

    /// Writes the facts to [`FILE`] in `dir`, for whoever asks, which may
    /// be the administrator in a folder others can make things in: the
    /// folder is made, or is one, and not a link to somewhere else; the
    /// facts are written to a name of their own that must not exist, and
    /// not through a link, then renamed into place over whatever the name
    /// was, a link or not, which is replaced and not followed.
    ///
    /// # Errors
    ///
    /// If it cannot be written.
    pub fn save(&self, dir: &Path) -> Result<(), Fail> {
        let mut text = serde_json::to_string_pretty(self).map_err(|err| fail(err.to_string()))?;
        text.push('\n');
        make_folder(dir)?;
        let path = dir.join(FILE);
        let partial = dir.join(format!("{FILE}.partial"));
        // A name a person left there is not ours to write to, link or file.
        match std::fs::remove_file(&partial) {
            Ok(()) => {}
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => return Err(util::failed(&partial.display(), err)),
        }
        let written = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(&partial)
            .and_then(|mut file| file.write_all(text.as_bytes()).and_then(|()| file.flush()))
            .and_then(|()| std::fs::rename(&partial, &path));
        written.map_err(|err| {
            let _ = std::fs::remove_file(&partial);
            util::failed(&path.display(), err)
        })
    }

    /// The facts [`Facts::save`] wrote to [`FILE`] in `dir`.
    ///
    /// # Errors
    ///
    /// If they cannot be read.
    pub fn load(dir: &Path) -> Result<Facts, Fail> {
        let path = dir.join(FILE);
        let text =
            std::fs::read_to_string(&path).map_err(|err| util::failed(&path.display(), err))?;
        serde_json::from_str(&text).map_err(|err| fail(format!("{}: {err}", path.display())))
    }
}

/// Makes the folder `dir` if it is not there. A link to a folder, or
/// anything that is not a folder, is refused: the facts written in it
/// would go where the link points, which someone else may have chosen.
fn make_folder(dir: &Path) -> Result<(), Fail> {
    let made = std::fs::create_dir(dir).or_else(|err| match err.kind() {
        std::io::ErrorKind::AlreadyExists => Ok(()),
        std::io::ErrorKind::NotFound => {
            let parent = dir.parent().filter(|parent| !parent.as_os_str().is_empty());
            parent.map_or(Ok(()), std::fs::create_dir_all).and_then(|()| std::fs::create_dir(dir))
        }
        _ => Err(err),
    });
    match made {
        Ok(()) => {}
        // Another made it in the meantime: what it is is looked at below.
        Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(err) => return Err(util::failed(&dir.display(), err)),
    }
    let kind = std::fs::symlink_metadata(dir)
        .map_err(|err| util::failed(&dir.display(), err))?
        .file_type();
    if kind.is_symlink() {
        return Err(fail(format!(
            "{} is a symbolic link, and the facts are not written through one",
            dir.display()
        )));
    }
    if !kind.is_dir() {
        return Err(fail(format!("{} is not a folder", dir.display())));
    }
    Ok(())
}

/// The boot of `boots` that holds a start made at `started`, in seconds
/// since 1970: the newest whose records are within [`SLACK`] seconds of it.
fn boot_holding(boots: &[logd::Boot], started: i64) -> Option<&logd::Boot> {
    let seconds = |time: &str| {
        logd::parse_rfc3339(time).map(|ns| i64::try_from(ns / 1_000_000_000).unwrap_or(0))
    };
    boots.iter().rev().find(|boot| {
        let (Some(first), Some(last)) = (seconds(&boot.first), seconds(&boot.last)) else {
            return false;
        };
        (first - SLACK..=last + SLACK).contains(&started)
    })
}

/// What logd is asked for the log of `start`, made in `boot`: the newest
/// [`LOG_LINES`] records of the boot up to [`WINDOW_AFTER`] seconds after
/// the health check began. logd's `until` is what bounds the window, so
/// that a boot that has run for days still gives the start's own lines and
/// not the days after it.
fn records_request(boot: &str, start: &Start) -> Request {
    let until = start
        .started
        .checked_add(WINDOW_AFTER)
        .and_then(|end| u64::try_from(end).ok())
        .and_then(|seconds| seconds.checked_mul(1_000_000_000));
    Request {
        ask: "records".into(),
        boot: boot.into(),
        until,
        lines: Some(LOG_LINES),
        ..Request::default()
    }
}

/// The log of the boot `start` was made in, as far as its window goes
/// ([`records_request`]); `None` if the log has no such boot.
fn log_of(start: &Start) -> Result<Option<Vec<Line>>, Fail> {
    let mut boots: Vec<logd::Boot> = Vec::new();
    let request = Request { ask: "boots".into(), ..Request::default() };
    logd::ask(&request, |text| {
        if let Ok(boot) = serde_json::from_str::<logd::Boot>(text) {
            boots.push(boot);
        }
        Ok(true)
    })?;
    let Some(boot) = boot_holding(&boots, start.started) else {
        return Ok(None);
    };
    let mut lines = Vec::new();
    logd::ask(&records_request(&boot.boot, start), |text| {
        if let Ok(record) = serde_json::from_str::<Record>(text) {
            lines.push(Line::of(&record));
        }
        Ok(true)
    })?;
    Ok(Some(lines))
}

/// Whether `service` is one that failed, as Spark says: it gave up on
/// it, or it ended in a way it was not meant to.
#[must_use]
pub fn has_failed(service: &show::Service) -> bool {
    match service.state {
        State::Failed => true,
        State::Stopped => {
            !service.off_as_meant
                && (service.why.is_some() || service.last_status.is_some_and(|status| status != 0))
        }
        _ => false,
    }
}

/// The names of the services Spark says have failed.
fn services_failed() -> Result<Vec<String>, Fail> {
    let answer = power::request("show")?;
    let document = show::parse(&answer).map_err(fail)?;
    Ok(document
        .services
        .iter()
        .filter(|service| has_failed(service))
        .map(|s| s.name.clone())
        .collect())
}

/// How full each mounted filesystem with a size is, and each of `paths`.
fn filesystems_full(paths: &[String]) -> Result<Vec<Full>, Fail> {
    let mounts = proc::mounts().map_err(|err| util::failed(&"/processes/mounts", err))?;
    let mounted = mounts.iter().map(|mount| df::mount_path(mount).to_string_lossy().into_owned());
    let stat = |path: &str| df::statvfs(std::ffi::OsStr::new(path)).ok();
    Ok(fullness(mounted.chain(paths.iter().cloned()), &stat))
}

/// Where to ask for the space of the filesystem at `path`, in order. In
/// the narrowed view of a service, such as the health check's,
/// `/system/state` is a folder the view makes of its own, and what it
/// shows of the state is attached inside it: that folder's space is not
/// the state's, but a place inside the state, the starts' records which
/// the health check keeps, is.
fn places_to_ask(path: &str) -> Vec<&str> {
    if path == "/system/state" { vec![starts::DIR, path] } else { vec![path] }
}

/// How full each of `paths` is, by `stat`, which gives the space at a
/// place: the first of [`places_to_ask`] that has any, each path once.
fn fullness(
    paths: impl Iterator<Item = String>,
    stat: &dyn Fn(&str) -> Option<df::Space>,
) -> Vec<Full> {
    let mut found: Vec<Full> = Vec::new();
    for path in paths {
        let Some(space) = places_to_ask(&path).into_iter().find_map(stat) else { continue };
        let Some(percent) = percent_full(&space) else { continue };
        if !found.iter().any(|seen| seen.path == path) {
            found.push(Full { path, percent });
        }
    }
    found
}

/// How full `space` is as df's Use% says: of what has been used and what
/// the people who use it can still have, rounded up; `None` for no size.
#[must_use]
pub fn percent_full(space: &df::Space) -> Option<u8> {
    let used = u128::from(space.size.saturating_sub(space.free));
    let of = used + u128::from(space.available);
    (of > 0).then(|| u8::try_from((used * 100).div_ceil(of)).unwrap_or(100))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sysupdate::starts::{Item, Outcome};

    pub fn start(number: u64, outcome: Outcome, error: Option<&str>) -> Start {
        Start {
            number,
            started: 1_800_000_000 + i64::try_from(number).unwrap() * 100,
            slot: "a".into(),
            stage: "services".into(),
            outcome,
            error: error.map(str::to_string),
            why: error.map(|id| format!("it failed ({id})")),
            items: vec![Item::new("system", "Dukovo", "0.15.0", None)],
            times: None,
        }
    }

    #[test]
    fn a_line_keeps_at_most_400_bytes_whole_characters() {
        let record = |text: &str| Record {
            level: "error".into(),
            from: "kernel".into(),
            name: "kernel".into(),
            text: text.into(),
            ..Record::default()
        };
        let short = Line::of(&record("blk: I/O error"));
        assert_eq!((short.level, short.text.as_str()), (3, "blk: I/O error"));
        assert_eq!(Line::of(&record(&"a".repeat(LINE_BYTES + 50))).text.len(), LINE_BYTES);
        // Two-byte characters, so that the limit falls inside one.
        let long = Line::of(&record(&format!("a{}", "\u{e9}".repeat(LINE_BYTES))));
        assert_eq!(long.text.len(), 399);
    }

    #[test]
    fn use_is_of_what_is_used_and_what_can_still_be_had() {
        let space =
            |size, free, available| df::Space { size, free, available, ..df::Space::default() };
        assert_eq!(percent_full(&space(0, 0, 0)), None);
        assert_eq!(percent_full(&space(1000, 1000, 1000)), Some(0));
        assert_eq!(percent_full(&space(1000, 500, 500)), Some(50));
        // Rounded up, as df does.
        assert_eq!(percent_full(&space(1000, 11, 11)), Some(99));
        assert_eq!(percent_full(&space(1000, 10, 10)), Some(99));
        assert_eq!(percent_full(&space(1000, 0, 0)), Some(100));
        // Space only the administrator can use is not what people have.
        assert_eq!(percent_full(&space(1000, 100, 0)), Some(100));
        assert_eq!(percent_full(&space(u64::MAX, 0, 0)), Some(100));
    }

    #[test]
    fn facts_are_saved_and_read_back() {
        let dir = std::env::temp_dir().join(format!("facts-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut facts = Facts::of(&start(7, Outcome::Failed, Some("BOOT-102")));
        facts.changed = vec!["app".into(), "options".into()];
        facts.log.push(Line {
            level: 3,
            from: "service".into(),
            name: "sshd".into(),
            text: "x".into(),
        });
        facts.failed.push("sshd".into());
        facts.full.push(Full { path: "/system/state".into(), percent: 99 });
        facts.save(&dir).unwrap();
        assert_eq!(Facts::load(&dir).unwrap(), facts);
        assert!(Facts::load(&dir.join("none")).is_err());
        std::fs::write(dir.join(FILE), "{").unwrap();
        assert!(Facts::load(&dir).is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    fn scratch(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("facts-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn facts_are_not_saved_through_a_link_that_someone_else_may_have_left() {
        use std::os::unix::fs::symlink;
        let root = scratch("links");
        let victim = root.join("victim");
        std::fs::create_dir(&victim).unwrap();
        let facts = Facts::of(&start(7, Outcome::Failed, Some("BOOT-102")));
        // A folder that is a link is refused, and nothing is written where it points.
        symlink(&victim, root.join("folder")).unwrap();
        let why = facts.save(&root.join("folder")).unwrap_err().message;
        assert!(why.contains("is a symbolic link"), "{why}");
        assert_eq!(std::fs::read_dir(&victim).unwrap().count(), 0);
        // A file that is not a folder is refused too.
        std::fs::write(root.join("plain"), "x").unwrap();
        let why = facts.save(&root.join("plain")).unwrap_err().message;
        assert!(why.contains("is not a folder"), "{why}");
        // A link where the facts go, and where they are written first, is
        // replaced and not followed: what it pointed to is as it was.
        let folder = root.join("ours");
        std::fs::create_dir(&folder).unwrap();
        let precious = root.join("precious");
        std::fs::write(&precious, "keep").unwrap();
        symlink(&precious, folder.join(FILE)).unwrap();
        symlink(&precious, folder.join(format!("{FILE}.partial"))).unwrap();
        facts.save(&folder).unwrap();
        assert_eq!(std::fs::read_to_string(&precious).unwrap(), "keep");
        assert_eq!(Facts::load(&folder).unwrap(), facts);
        assert!(!std::fs::symlink_metadata(folder.join(FILE)).unwrap().file_type().is_symlink());
        // Nothing but the facts is left, and they are for their owner.
        let names: Vec<_> =
            std::fs::read_dir(&folder).unwrap().flatten().map(|e| e.file_name()).collect();
        assert_eq!(names, [std::ffi::OsString::from(FILE)]);
        let mode = std::os::unix::fs::PermissionsExt::mode(
            &std::fs::metadata(folder.join(FILE)).unwrap().permissions(),
        );
        assert_eq!(mode & 0o777, 0o600);
        // A folder that is not there, and its parents, are made, and saving
        // again replaces what was saved.
        let deep = root.join("a").join("b");
        facts.save(&deep).unwrap();
        facts.save(&deep).unwrap();
        assert_eq!(Facts::load(&deep).unwrap(), facts);
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn the_boot_of_a_start_is_the_newest_whose_records_are_near_it() {
        let boot = |name: &str, first: &str, last: &str| logd::Boot {
            boot: name.into(),
            first: first.into(),
            last: last.into(),
            records: 1,
        };
        // 2026-11-13T10:00:00Z is 1794564000.
        let boots = [
            boot("a", "2026-11-13T10:00:00.000000Z", "2026-11-13T11:00:00.000000Z"),
            boot("b", "2026-11-14T10:00:00.000000Z", "2026-11-17T10:00:00.000000Z"),
            boot("bad", "never", "never"),
        ];
        let found = |started: i64| boot_holding(&boots, started).map(|boot| boot.boot.as_str());
        assert_eq!(found(1_794_564_000), Some("a"));
        assert_eq!(found(1_794_564_000 - 5), Some("a"));
        assert_eq!(found(1_794_564_000 - 6), None);
        assert_eq!(found(1_794_567_600 + 5), Some("a"));
        assert_eq!(found(1_794_567_600 + 6), None);
        assert_eq!(found(1_794_700_000), Some("b"));
    }

    #[test]
    fn the_log_of_a_start_is_read_up_to_an_hour_after_its_check_began() {
        let mut record = start(3, Outcome::Failed, None);
        record.started = 1_794_477_600;
        let request = records_request("a-boot", &record);
        // Not to the end of a boot that ran on for days: the start's own
        // lines are within the newest of what is asked for.
        assert_eq!(request.until, Some((1_794_477_600 + 3600) * 1_000_000_000));
        assert_eq!(request.lines, Some(LOG_LINES));
        assert_eq!((request.ask.as_str(), request.boot.as_str()), ("records", "a-boot"));
        assert_eq!(request.since, None);
        // A record whose time makes no sense is read to the end rather than to a wrong time.
        record.started = -5_000;
        assert_eq!(records_request("a-boot", &record).until, None);
        record.started = i64::MAX;
        assert_eq!(records_request("a-boot", &record).until, None);
    }

    #[test]
    fn fullness_is_asked_inside_the_state_where_a_narrowed_view_has_its_own_folder() {
        assert_eq!(places_to_ask("/system/state"), [starts::DIR, "/system/state"]);
        assert_eq!(places_to_ask("/users"), ["/users"]);
        assert_eq!(starts::DIR, "/system/state/starts");
        let space = |size: u64, free: u64| df::Space {
            size,
            free,
            available: free,
            ..df::Space::default()
        };
        let paths = || {
            ["/", "/system/state", "/users", "/system/state", "/nowhere"]
                .map(String::from)
                .into_iter()
        };
        // The view's own folder at /system/state is empty and has no size;
        // inside it is the state's filesystem, 99% full.
        let narrowed = |path: &str| match path {
            "/" => Some(space(1000, 500)),
            "/system/state" => Some(space(0, 0)),
            "/system/state/starts" => Some(space(1000, 10)),
            "/users" => Some(space(1000, 900)),
            _ => None,
        };
        let full = fullness(paths(), &narrowed);
        let said: Vec<(&str, u8)> = full.iter().map(|f| (f.path.as_str(), f.percent)).collect();
        assert_eq!(said, [("/", 50), ("/system/state", 99), ("/users", 10)]);
        // Where nothing is attached inside the state, as on a system that
        // is not narrowed, the state itself is asked.
        let plain = |path: &str| match path {
            "/system/state" => Some(space(1000, 100)),
            _ => None,
        };
        let full = fullness(paths(), &plain);
        assert_eq!(full.len(), 1);
        assert_eq!((full[0].path.as_str(), full[0].percent), ("/system/state", 90));
    }

    #[test]
    fn a_service_has_failed_when_spark_gave_up_or_it_ended_wrong() {
        let service = |state, off_as_meant, why: Option<&str>, status| show::Service {
            state,
            off_as_meant,
            why: why.map(str::to_string),
            last_status: status,
            ..show::Service::default()
        };
        assert!(has_failed(&service(State::Failed, false, None, None)));
        assert!(has_failed(&service(State::Stopped, false, Some("it requires db"), None)));
        assert!(has_failed(&service(State::Stopped, false, None, Some(256))));
        assert!(!has_failed(&service(State::Stopped, false, None, Some(0))));
        assert!(!has_failed(&service(State::Stopped, false, None, None)));
        assert!(!has_failed(&service(State::Stopped, true, Some("no cards"), None)));
        assert!(!has_failed(&service(State::Running, false, None, None)));
    }
}
