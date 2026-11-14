// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The result of the last run, which `dukovo-check --last` and the web
//! console's health score read: `/system/state/dukovo-check/last.json`,
//! readable by administrators alone, written whole by a new file renamed
//! over the old one so that a reader never finds half of it. Its format
//! is versioned, and dukovo-check(8) describes it.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt, fchown};
use std::path::Path;

use serde::{Deserialize, Serialize};

use super::context::Context;
use super::line::{Counts, Line, ids};
use crate::util::{self, Fail, fail};

/// Where the result is kept.
pub(crate) const DIR: &str = "/system/state/dukovo-check";
pub(crate) const FILE: &str = "/system/state/dukovo-check/last.json";

/// The version of the format. A reader refuses a later one, since it may
/// mean something else by what it has in common.
pub(crate) const VERSION: u32 = 1;

/// A finished run.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Saved {
    pub(crate) version: u32,
    /// When it ended, in UTC: `2026-11-20T05:17:03Z`.
    pub(crate) checked: String,
    /// The release of the system that ran it.
    pub(crate) release: String,
    pub(crate) results: Vec<Line>,
    pub(crate) counts: Counts,
}

impl Saved {
    /// The result of a run that ended now, in `context`.
    pub(crate) fn of(context: &Context, results: Vec<Line>) -> Saved {
        let time = i64::try_from(context.now).unwrap_or(0);
        Saved {
            version: VERSION,
            checked: util::time_text(time, "%Y-%m-%dT%H:%M:%SZ", true),
            release: context.release.clone(),
            counts: Counts::of(&results),
            results,
        }
    }

    /// The result as the file holds it.
    pub(crate) fn json(&self) -> Result<String, Fail> {
        serde_json::to_string_pretty(self).map_err(|e| fail(format!("cannot write JSON: {e}")))
    }

    /// Writes the result to `dir/last.json` for the group `readers` to
    /// read: to a new file beside it, with mode 0640, which is synced and
    /// renamed over it, and then the folder is synced, so that a power cut
    /// leaves the old result or the new, never part of one.
    pub(crate) fn save(&self, dir: &Path, readers: u32) -> io::Result<()> {
        fs::create_dir_all(dir)?;
        let new = dir.join("last.json.new");
        let text = self.json().map_err(|f| io::Error::other(f.message))?;
        let mut file =
            OpenOptions::new().write(true).create(true).truncate(true).mode(0o640).open(&new)?;
        // The mode given at creation is cut by the umask, and a file left by
        // a run that was cut short keeps its own.
        file.set_permissions(fs::Permissions::from_mode(0o640))?;
        fchown(&file, None, Some(readers))?;
        file.write_all(text.as_bytes())?;
        file.write_all(b"\n")?;
        file.sync_all()?;
        fs::rename(&new, dir.join("last.json"))?;
        File::open(dir)?.sync_all()
    }

    /// The result in `file`.
    pub(crate) fn load(file: &Path) -> Result<Saved, Fail> {
        let text = fs::read_to_string(file).map_err(|error| match error.kind() {
            io::ErrorKind::NotFound => {
                fail("no check has run yet: run elevate dukovo-check".to_string())
            }
            io::ErrorKind::PermissionDenied => fail(
                "the kept result is for administrators: sign in as one, or run elevate \
                 dukovo-check --last"
                    .to_string(),
            ),
            _ => util::failed(&file.display(), error),
        })?;
        let not_a_result = |why: &dyn std::fmt::Display| {
            fail(format!("{}: not the result of a check: {why}", file.display()))
        };
        let value: serde_json::Value = serde_json::from_str(&text).map_err(|e| not_a_result(&e))?;
        let version = value.get("version").and_then(serde_json::Value::as_u64);
        if version.is_some_and(|version| version > u64::from(VERSION)) {
            return Err(fail(format!(
                "{}: a later dukovo-check wrote it, in version {} of the format, and this one \
                 reads version {VERSION}",
                file.display(),
                version.unwrap_or_default()
            )));
        }
        serde_json::from_value(value).map_err(|e| not_a_result(&e))
    }

    /// What a run says first when it is read back: when it ended, and what
    /// ran it.
    pub(crate) fn heading(&self) -> String {
        format!("The last check ended {}, on Dukovo {}.", self.checked, self.release)
    }

    /// The syslog level of the run's line in the log, and the line: the
    /// IDs of what it found are in it, so that a search of the log finds
    /// them.
    pub(crate) fn log_line(&self) -> (u8, String) {
        let status = self.counts.status();
        let level = match status {
            1 => 3,
            2 => 4,
            _ => 6,
        };
        let mut line = format!("check ended with status {status}: {}", self.counts.summary());
        let found = ids(&self.results);
        if !found.is_empty() {
            line.push_str(&format!(" ({})", found.join(", ")));
        }
        (level, line)
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::MetadataExt;

    use super::super::context::planted::System;
    use super::super::line::{Area, Outcome, Problem};
    use super::*;

    fn lines() -> Vec<Line> {
        vec![
            Line { area: Area::Time, check: "clock".into(), outcome: Outcome::passed("ok") },
            Line {
                area: Area::Ssh,
                check: "passwords".into(),
                outcome: Outcome::warning(Problem::SshPasswords, "passwords sign in"),
            },
            Line {
                area: Area::Health,
                check: "hardware".into(),
                outcome: Outcome::skipped("none"),
            },
        ]
    }

    fn saved() -> Saved {
        let system = System::new(Vec::new());
        Saved::of(&system.context, lines())
    }

    fn folder(call: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("dukovo-check-saved-{}-{call}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn the_file_holds_the_version_the_time_the_release_the_results_and_the_counts() {
        let value = serde_json::to_value(saved()).unwrap();
        assert_eq!(value["version"], 1);
        assert_eq!(value["checked"], "2026-11-14T12:00:30Z");
        assert_eq!(value["release"], "0.16.0");
        assert_eq!(
            value["counts"],
            serde_json::json!({"passed": 1, "warning": 1, "failed": 0, "skipped": 1})
        );
        let results = value["results"].as_array().unwrap();
        assert_eq!(results.len(), 3);
        assert_eq!(
            results[1],
            serde_json::json!({
                "area": "ssh", "check": "passwords", "result": "warning",
                "detail": "passwords sign in", "id": "SEC-110",
                "fix": Problem::SshPasswords.advice()
            })
        );
    }

    #[test]
    fn what_is_saved_is_read_back_and_the_old_file_is_replaced_whole() {
        let dir = folder("save");
        let group = own_group();
        saved().save(&dir, group).unwrap();
        assert_eq!(Saved::load(&dir.join("last.json")).unwrap(), saved());
        let mut later = saved();
        later.results.truncate(1);
        later.counts = Counts::of(&later.results);
        later.save(&dir, group).unwrap();
        assert_eq!(Saved::load(&dir.join("last.json")).unwrap(), later);
        assert!(!dir.join("last.json.new").exists());
        let _ = fs::remove_dir_all(dir);
    }

    /// The group of the test's own process, which it may give a file.
    fn own_group() -> u32 {
        // SAFETY: getegid has no preconditions.
        unsafe { libc::getegid() }
    }

    #[test]
    fn the_file_is_for_the_readers_group_and_nobody_else() {
        let dir = folder("mode");
        let group = own_group();
        // The new file a run that was cut short left, with other modes, is
        // written over and put right.
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("last.json.new"), "half a").unwrap();
        fs::set_permissions(dir.join("last.json.new"), fs::Permissions::from_mode(0o666)).unwrap();
        saved().save(&dir, group).unwrap();
        let file = dir.join("last.json");
        let meta = fs::metadata(&file).unwrap();
        assert_eq!(meta.mode() & 0o777, 0o640);
        assert_eq!(meta.gid(), group);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn a_missing_or_foreign_or_later_file_says_what_it_is() {
        let dir = folder("load");
        let missing = Saved::load(&dir.join("last.json")).unwrap_err();
        assert_eq!(missing.message, "no check has run yet: run elevate dukovo-check");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("last.json"), "{\"time\": 1}").unwrap();
        let foreign = Saved::load(&dir.join("last.json")).unwrap_err();
        assert!(foreign.message.contains("not the result of a check"), "{}", foreign.message);
        fs::write(dir.join("last.json"), "{\"version\": 2, \"results\": 3}").unwrap();
        let later = Saved::load(&dir.join("last.json")).unwrap_err();
        assert!(
            later.message.contains("version 2 of the format, and this one reads version 1"),
            "{}",
            later.message
        );
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn the_log_line_says_the_status_the_counts_and_the_ids() {
        let (level, line) = saved().log_line();
        assert_eq!(level, 4);
        assert_eq!(line, "check ended with status 2: 1 passed, 1 warning, 1 skipped (SEC-110)");
        let mut clean = saved();
        clean.results.retain(|line| line.outcome.id.is_none());
        clean.counts = Counts::of(&clean.results);
        let (level, line) = clean.log_line();
        assert_eq!(level, 6);
        assert_eq!(line, "check ended with status 0: 1 passed, 0 warnings, 1 skipped");
    }

    #[test]
    fn the_heading_says_when_it_ended_and_which_release_ran_it() {
        assert_eq!(
            saved().heading(),
            "The last check ended 2026-11-14T12:00:30Z, on Dukovo 0.16.0."
        );
    }
}
