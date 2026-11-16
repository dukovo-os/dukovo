// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The checks that were kept, one line each: `hardware history`, and the
//! file the web console reads for the days a check ran on
//! (`/system/state/hardware/history`). A run that ends adds a line beside
//! the result it keeps, and the file holds the newest 200, written whole
//! as the result is.
//!
//! A line is a JSON object of its own: when the run ended, in
//! milliseconds since 1970; whether it was a full check; its exit status;
//! how many checks passed, warned, failed and were skipped; and the IDs of
//! what it found.

use std::fs;
use std::io;
use std::path::Path;

use serde::{Deserialize, Serialize};

use super::check::Counts;
use super::saved::{self, Saved};
use crate::util::{self, Fail};

/// Where the history is kept.
pub(crate) const FILE: &str = "/system/state/hardware/history";

/// The name of the file in its folder.
const NAME: &str = "history";

/// The most lines kept: the newest.
pub(crate) const KEEP: usize = 200;

/// One kept run.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Line {
    /// When the run ended, in milliseconds since 1970.
    pub(crate) at_ms: i64,
    /// Whether it was a full check.
    pub(crate) full: bool,
    /// Its exit status.
    pub(crate) status: i32,
    pub(crate) passed: usize,
    pub(crate) warnings: usize,
    pub(crate) failed: usize,
    pub(crate) skipped: usize,
    /// The IDs of what it found, each once.
    pub(crate) ids: Vec<String>,
}

impl Line {
    /// The line of the run `saved` holds.
    pub(crate) fn of(saved: &Saved) -> Line {
        let counts = Counts::of(&saved.checks);
        Line {
            at_ms: saved.time.saturating_mul(1000),
            full: saved.full,
            status: saved.status,
            passed: counts.passed,
            warnings: counts.warnings,
            failed: counts.failed,
            skipped: counts.skipped,
            ids: saved.ids().into_iter().map(String::from).collect(),
        }
    }

    /// What a terminal shows of the run: when, what kind, what it found.
    pub(crate) fn text(&self) -> String {
        let counts = Counts {
            passed: self.passed,
            warnings: self.warnings,
            failed: self.failed,
            skipped: self.skipped,
        };
        let time = util::time_text(self.at_ms.div_euclid(1000), "%F %R", false);
        let kind = if self.full { "full" } else { "quick" };
        let mut text = format!("{time}  {kind:<5}  {}", counts.text());
        if !self.ids.is_empty() {
            text.push_str(&format!(" ({})", self.ids.join(", ")));
        }
        text
    }
}

/// Adds the line of a run to the history in `dir`, and drops the oldest
/// beyond [`KEEP`]. A line of the file that is not a line of this kind is
/// dropped with them.
pub(crate) fn append(dir: &Path, line: &Line) -> io::Result<()> {
    let old = match fs::read_to_string(dir.join(NAME)) {
        Ok(text) => text,
        Err(error) if error.kind() == io::ErrorKind::NotFound => String::new(),
        Err(error) => return Err(error),
    };
    let new = serde_json::to_string(line).map_err(io::Error::other)?;
    let mut lines: Vec<&str> =
        old.lines().filter(|line| serde_json::from_str::<Line>(line).is_ok()).collect();
    lines.push(&new);
    let first = lines.len().saturating_sub(KEEP);
    saved::write_whole(dir, NAME, &lines[first..].join("\n"))
}

/// The lines of the history in `file`, the oldest first. A file that is not
/// there is no history; a line that is not a line of this kind is left out.
pub(crate) fn read(file: &Path) -> Result<Vec<Line>, Fail> {
    let text = match fs::read_to_string(file) {
        Ok(text) => text,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(util::failed(&file.display(), error)),
    };
    let lines: Vec<Line> =
        text.lines().filter_map(|line| serde_json::from_str(line).ok()).collect();
    let first = lines.len().saturating_sub(KEEP);
    Ok(lines[first..].to_vec())
}

#[cfg(test)]
mod tests {
    use super::super::check::{Area, Check, Problem};
    use super::*;

    fn run(time: i64) -> Saved {
        Saved {
            time,
            version: "0.16.0".into(),
            full: false,
            status: 2,
            checks: vec![
                Check::passed(Area::Memory, "memory test", String::new()),
                Check::warning(Area::Disks, "sda health", Problem::DiskEarlyWarning, String::new()),
                Check::warning(Area::Disks, "sdb health", Problem::DiskEarlyWarning, String::new()),
                Check::skipped(Area::Battery, "battery", String::new()),
            ],
        }
    }

    fn folder(call: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("hardware-history-{}-{call}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn a_line_says_when_what_kind_how_it_ended_how_many_and_which_ids() {
        let line = Line::of(&run(1_794_000_000));
        assert_eq!(
            serde_json::to_value(&line).unwrap(),
            serde_json::json!({
                "at_ms": 1_794_000_000_000i64, "full": false, "status": 2,
                "passed": 1, "warnings": 2, "failed": 0, "skipped": 1, "ids": ["HW-115"],
            })
        );
        // The members come in the order the design gives them.
        let text = serde_json::to_string(&line).unwrap();
        assert!(
            text.starts_with("{\"at_ms\":1794000000000,\"full\":false,\"status\":2,\"passed\":1")
        );
        assert!(text.ends_with("\"skipped\":1,\"ids\":[\"HW-115\"]}"), "{text}");
        assert_eq!(serde_json::from_str::<Line>(&text).unwrap(), line);
    }

    #[test]
    fn the_text_says_the_time_the_kind_the_counts_and_the_ids() {
        let line = Line::of(&run(1_794_000_000));
        let time = util::time_text(1_794_000_000, "%F %R", false);
        assert_eq!(
            line.text(),
            format!("{time}  quick  1 passed, 2 warnings, 0 FAILED, 1 skipped (HW-115)")
        );
        let clean = Line { ids: Vec::new(), full: true, ..line };
        assert!(clean.text().ends_with("full   1 passed, 2 warnings, 0 FAILED, 1 skipped"));
    }

    #[test]
    fn each_run_adds_a_line_in_the_order_they_ended() {
        let dir = folder("add");
        for time in [100, 200, 300] {
            append(&dir, &Line::of(&run(time))).unwrap();
        }
        let lines = read(&dir.join(NAME)).unwrap();
        let times: Vec<i64> = lines.iter().map(|line| line.at_ms).collect();
        assert_eq!(times, [100_000, 200_000, 300_000]);
        // One line each, in a file that ends with a line end and has no
        // leftover of the write.
        let text = fs::read_to_string(dir.join(NAME)).unwrap();
        assert_eq!(text.lines().count(), 3);
        assert!(text.ends_with("}\n"));
        assert!(!dir.join("history.new").exists());
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn the_history_is_cut_to_the_newest_200() {
        let dir = folder("cut");
        for time in 1..=KEEP as i64 + 5 {
            append(&dir, &Line::of(&run(time))).unwrap();
        }
        let lines = read(&dir.join(NAME)).unwrap();
        assert_eq!(lines.len(), KEEP);
        assert_eq!(lines.first().map(|line| line.at_ms), Some(6_000));
        assert_eq!(lines.last().map(|line| line.at_ms), Some(205_000));
        assert_eq!(fs::read_to_string(dir.join(NAME)).unwrap().lines().count(), KEEP);
        // A file somebody made longer is read to the newest 200 as well.
        let long: Vec<String> = (1..=KEEP as i64 + 3)
            .map(|time| serde_json::to_string(&Line::of(&run(time))).unwrap())
            .collect();
        fs::write(dir.join(NAME), long.join("\n")).unwrap();
        let lines = read(&dir.join(NAME)).unwrap();
        assert_eq!((lines.len(), lines[0].at_ms), (KEEP, 4_000));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn a_line_that_is_not_a_line_of_the_history_is_dropped_not_kept_and_not_an_error() {
        let dir = folder("foreign");
        fs::create_dir_all(&dir).unwrap();
        let good = serde_json::to_string(&Line::of(&run(100))).unwrap();
        fs::write(dir.join(NAME), format!("not json\n{good}\n{{\"at_ms\": 1}}\n\n")).unwrap();
        assert_eq!(read(&dir.join(NAME)).unwrap().len(), 1);
        append(&dir, &Line::of(&run(200))).unwrap();
        let text = fs::read_to_string(dir.join(NAME)).unwrap();
        assert_eq!(text.lines().count(), 2, "{text}");
        assert!(!text.contains("not json"));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn no_file_is_an_empty_history_and_a_folder_in_its_place_is_an_error() {
        let dir = folder("none");
        assert_eq!(read(&dir.join(NAME)).unwrap(), Vec::new());
        fs::create_dir_all(dir.join(NAME)).unwrap();
        assert!(read(&dir.join(NAME)).is_err());
        assert!(append(&dir, &Line::of(&run(1))).is_err());
        let _ = fs::remove_dir_all(dir);
    }
}
