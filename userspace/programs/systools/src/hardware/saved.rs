// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The result of the last run, which `hardware last` and the web console
//! read: `/system/state/hardware/last.json`, written whole by a new file
//! renamed over the old one so that a reader never finds half of it.
//! [`write_whole`] does that for the other file kept beside it, the
//! history.

use std::fs::{self, File};
use std::io::{self, Write};
use std::path::Path;

use serde::{Deserialize, Serialize};

use super::check::{Check, summary};
use crate::util::{self, Fail, fail};

/// Where the result is kept.
pub(crate) const DIR: &str = "/system/state/hardware";
pub(crate) const FILE: &str = "/system/state/hardware/last.json";

/// What is said when no run has kept a result.
pub(crate) const NONE: &str = "no check has run yet: run elevate hardware check";

/// Writes `text` and a line end to `dir/name`: to a new file beside it,
/// which is synced and renamed over it, and then the folder is synced, so
/// that a power cut leaves the old file or the new, never part of one.
pub(crate) fn write_whole(dir: &Path, name: &str, text: &str) -> io::Result<()> {
    fs::create_dir_all(dir)?;
    let new = dir.join(format!("{name}.new"));
    let mut file = File::create(&new)?;
    file.write_all(text.as_bytes())?;
    file.write_all(b"\n")?;
    file.sync_all()?;
    fs::rename(&new, dir.join(name))?;
    File::open(dir)?.sync_all()
}

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

    /// Writes the result to `dir/last.json`, whole.
    pub(crate) fn save(&self, dir: &Path) -> io::Result<()> {
        let text = self.json().map_err(|f| io::Error::other(f.message))?;
        write_whole(dir, "last.json", &text)
    }

    /// The result in `file`, or none when there is no such file: no check
    /// has kept one.
    pub(crate) fn read(file: &Path) -> Result<Option<Saved>, Fail> {
        let text = match fs::read_to_string(file) {
            Ok(text) => text,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(util::failed(&file.display(), error)),
        };
        serde_json::from_str(&text)
            .map(Some)
            .map_err(|e| fail(format!("{}: not the result of a check: {e}", file.display())))
    }

    /// What a run says first: when it ended, and what ran it.
    pub(crate) fn heading(&self) -> String {
        let time = util::time_text(self.time, "%F %R", false);
        let kind = if self.full { "full" } else { "quick" };
        format!("A {kind} check ended {time}, on Dukovo {}.", self.version)
    }

    /// The IDs of what the run found, each once, in order.
    pub(crate) fn ids(&self) -> Vec<&str> {
        let mut ids: Vec<&str> =
            self.checks.iter().filter_map(|check| check.id.as_deref()).collect();
        ids.sort_unstable();
        ids.dedup();
        ids
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
        let ids = self.ids();
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
        assert_eq!(Saved::read(&dir.join("last.json")).unwrap(), Some(saved()));
        let mut later = saved();
        later.status = 0;
        later.checks.truncate(1);
        later.save(&dir).unwrap();
        assert_eq!(Saved::read(&dir.join("last.json")).unwrap(), Some(later));
        assert!(!dir.join("last.json.new").exists());
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn a_missing_file_is_no_result_and_a_foreign_one_says_what_it_is() {
        let dir = folder("load");
        assert_eq!(Saved::read(&dir.join("last.json")).unwrap(), None);
        assert_eq!(NONE, "no check has run yet: run elevate hardware check");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("last.json"), "{\"time\": 1}").unwrap();
        let foreign = Saved::read(&dir.join("last.json")).unwrap_err();
        assert!(foreign.message.contains("not the result of a check"), "{}", foreign.message);
        // A folder where the file should be is an error of its own, not
        // the lack of a result.
        fs::remove_file(dir.join("last.json")).unwrap();
        fs::create_dir_all(dir.join("last.json")).unwrap();
        assert!(Saved::read(&dir.join("last.json")).is_err());
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn a_file_is_written_whole_with_a_line_end_and_no_leftover() {
        let dir = folder("whole");
        write_whole(&dir, "history", "one").unwrap();
        write_whole(&dir, "history", "two\nlines").unwrap();
        assert_eq!(fs::read_to_string(dir.join("history")).unwrap(), "two\nlines\n");
        assert!(!dir.join("history.new").exists());
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
