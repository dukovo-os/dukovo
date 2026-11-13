// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The result of the last run, which `hardware last` and the web console
//! read: `/system/state/hardware/last.json`, written whole by a new file
//! renamed over the old one so that a reader never finds half of it.

use std::fs::{self, File};
use std::io::{self, Write};
use std::path::Path;

use serde::{Deserialize, Serialize};

use super::check::{Check, summary};
use crate::util::{self, Fail, fail};

/// Where the result is kept.
pub(crate) const DIR: &str = "/system/state/hardware";
pub(crate) const FILE: &str = "/system/state/hardware/last.json";

/// A finished run.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Saved {
    /// When it ended, in seconds since 1970.
    pub(crate) time: i64,
    /// The release of the system that ran it.
    pub(crate) version: String,
    /// Whether it was a full check.
    pub(crate) full: bool,
    /// Its exit status.
    pub(crate) status: i32,
    pub(crate) checks: Vec<Check>,
}

impl Saved {
    /// The result as the file holds it.
    pub(crate) fn json(&self) -> Result<String, Fail> {
        serde_json::to_string_pretty(self).map_err(|e| fail(format!("cannot write JSON: {e}")))
    }

    /// Writes the result to `dir/last.json`: to a new file beside it,
    /// which is synced and renamed over it, and then the folder is synced,
    /// so that a power cut leaves the old result or the new, never part of
    /// one.
    pub(crate) fn save(&self, dir: &Path) -> io::Result<()> {
        fs::create_dir_all(dir)?;
        let new = dir.join("last.json.new");
        let text = self.json().map_err(|f| io::Error::other(f.message))?;
        let mut file = File::create(&new)?;
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
                fail("no check has run yet: run elevate hardware check".to_string())
            }
            _ => util::failed(&file.display(), error),
        })?;
        serde_json::from_str(&text)
            .map_err(|e| fail(format!("{}: not the result of a check: {e}", file.display())))
    }

    /// What a run says first: when it ended, and what ran it.
    pub(crate) fn heading(&self) -> String {
        let time = util::time_text(self.time, "%F %R", false);
        let kind = if self.full { "full" } else { "quick" };
        format!("A {kind} check ended {time}, on Dukovo {}.", self.version)
    }

    /// The syslog level of the run's line in the log, and the line: the
    /// IDs of what it found are in it, so that a search of the log finds
    /// them.
    pub(crate) fn log_line(&self) -> (u8, String) {
        let level = match self.status {
            1 => 3,
            2 => 4,
            _ => 6,
        };
        let mut ids: Vec<&str> =
            self.checks.iter().filter_map(|check| check.id.as_deref()).collect();
        ids.sort_unstable();
        ids.dedup();
        let kind = if self.full { "full" } else { "quick" };
        let mut line =
            format!("{kind} check ended with status {}: {}", self.status, summary(&self.checks));
        if !ids.is_empty() {
            line.push_str(&format!(" ({})", ids.join(", ")));
        }
        (level, line)
    }
}

#[cfg(test)]
mod tests {
    use super::super::check::{Area, Problem};
    use super::*;

    fn saved() -> Saved {
        Saved {
            time: 1_794_000_000,
            version: "0.16.0".into(),
            full: false,
            status: 1,
            checks: vec![
                Check::passed(Area::Memory, "memory test", "256 MiB tested".into()),
                Check::failed(
                    Area::Disks,
                    "sda read scan",
                    Problem::DiskUnreadable,
                    "sector 20000".into(),
                ),
                Check::skipped(Area::Battery, "battery", "no battery".into()),
            ],
        }
    }

    fn folder(call: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("hardware-test-{}-{call}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn the_file_holds_the_time_the_version_and_every_check() {
        let value = serde_json::to_value(saved()).unwrap();
        assert_eq!(value["time"], 1_794_000_000);
        assert_eq!(value["version"], "0.16.0");
        assert_eq!(value["full"], false);
        assert_eq!(value["status"], 1);
        let checks = value["checks"].as_array().unwrap();
        assert_eq!(checks.len(), 3);
        assert_eq!(
            checks[0],
            serde_json::json!({
                "area": "memory", "check": "memory test", "result": "passed",
                "detail": "256 MiB tested"
            })
        );
        assert_eq!(checks[1]["area"], "disks");
        assert_eq!(checks[1]["result"], "failed");
        assert_eq!(checks[1]["id"], "HW-113");
        assert_eq!(checks[1]["advice"], Problem::DiskUnreadable.advice());
        assert_eq!(checks[2]["result"], "skipped");
    }

    #[test]
    fn what_is_saved_is_read_back_and_the_old_file_is_replaced_whole() {
        let dir = folder("save");
        saved().save(&dir).unwrap();
        assert_eq!(Saved::load(&dir.join("last.json")).unwrap(), saved());
        let mut later = saved();
        later.status = 0;
        later.checks.truncate(1);
        later.save(&dir).unwrap();
        assert_eq!(Saved::load(&dir.join("last.json")).unwrap(), later);
        assert!(!dir.join("last.json.new").exists());
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn a_missing_or_foreign_file_says_what_to_do() {
        let dir = folder("load");
        let missing = Saved::load(&dir.join("last.json")).unwrap_err();
        assert_eq!(missing.message, "no check has run yet: run elevate hardware check");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("last.json"), "{\"time\": 1}").unwrap();
        let foreign = Saved::load(&dir.join("last.json")).unwrap_err();
        assert!(foreign.message.contains("not the result of a check"), "{}", foreign.message);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn the_log_line_says_the_status_the_counts_and_the_ids() {
        let (level, line) = saved().log_line();
        assert_eq!(level, 3);
        assert_eq!(
            line,
            "quick check ended with status 1: 1 passed, 0 warnings, 1 FAILED, 1 skipped (HW-113)"
        );
        let mut clean = saved();
        clean.status = 0;
        clean.checks.retain(|check| check.id.is_none());
        let (level, line) = clean.log_line();
        assert_eq!(level, 6);
        assert!(line.ends_with("1 passed, 0 warnings, 0 FAILED, 1 skipped"), "{line}");
    }
}
