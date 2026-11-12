// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The record of every start (ADR 0500). The health check writes one as
//! it begins, unfinished, and finishes it with the start's outcome: good,
//! when the start was seen through or proved itself, or failed, with
//! why and its error ID. A start that never got that far stays
//! unfinished. Each record lists what the start ran: the system's version
//! and slot, the kernel, the firmware set and its microcode, the boot
//! options, the network cards and the root's partitions, and the apps
//! installed for everyone, each with when it was installed where that is
//! known. changes(1) compares two records. The records are kept in the
//! state, `/system/state/starts/<number>.json`, the last [`KEEP`] of
//! them.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Where the records are kept.
pub const DIR: &str = "/system/state/starts";

/// How many records are kept.
pub const KEEP: u64 = 100;

/// How a start ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Outcome {
    /// The health check has not finished, or never did.
    Unfinished,
    /// The start was seen through, or an update proved itself.
    Good,
    /// The health check failed it.
    Failed,
}

/// One thing a start ran.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Item {
    /// What kind of thing: `system`, `kernel`, `firmware`, `microcode`,
    /// `options`, `card`, `root` or `app`.
    pub kind: String,
    /// Which one of its kind.
    pub name: String,
    /// Its version, or what tells it apart.
    pub value: String,
    /// When it was installed, in seconds since 1970, where that is known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub installed: Option<i64>,
}

impl Item {
    /// An item.
    #[must_use]
    pub fn new(kind: &str, name: &str, value: &str, installed: Option<i64>) -> Item {
        Item { kind: kind.into(), name: name.into(), value: value.into(), installed }
    }
}

/// The record of one start.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Start {
    /// The start's number, one more than the start before it.
    pub number: u64,
    /// When the health check began, in seconds since 1970.
    pub started: i64,
    /// The slot it started, `a` or `b`.
    pub slot: String,
    /// How far it got: `services` once the health check runs.
    pub stage: String,
    /// How it ended.
    pub outcome: Outcome,
    /// The error ID of what failed, such as `FW-101`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// What failed, in words.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub why: Option<String>,
    /// What it ran.
    pub items: Vec<Item>,
}

/// Every record in `dir`, oldest first; one that cannot be read is left
/// out.
#[must_use]
pub fn all_in(dir: &Path) -> Vec<Start> {
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut starts: Vec<Start> = entries
        .filter_map(Result::ok)
        .filter(|entry| entry.file_name().to_string_lossy().ends_with(".json"))
        .filter_map(|entry| std::fs::read(entry.path()).ok())
        .filter_map(|bytes| serde_json::from_slice(&bytes).ok())
        .collect();
    starts.sort_by_key(|start| start.number);
    starts
}

/// Every record kept, oldest first.
#[must_use]
pub fn all() -> Vec<Start> {
    all_in(Path::new(DIR))
}

/// The path of record `number` in `dir`.
fn path(dir: &Path, number: u64) -> PathBuf {
    dir.join(format!("{number}.json"))
}

/// Keeps `start` in `dir`, whole or not at all, and removes the records
/// more than [`KEEP`] starts older.
///
/// # Errors
///
/// If it cannot be written.
pub fn write_in(dir: &Path, start: &Start) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let mut bytes = serde_json::to_vec_pretty(start).map_err(std::io::Error::other)?;
    bytes.push(b'\n');
    let partial = dir.join(format!("{}.partial", start.number));
    std::fs::write(&partial, bytes)?;
    std::fs::rename(&partial, path(dir, start.number))?;
    for old in all_in(dir).iter().filter(|old| old.number + KEEP <= start.number) {
        let _ = std::fs::remove_file(path(dir, old.number));
    }
    Ok(())
}

/// Begins the record of this start, unfinished, as of `now`, in `dir`:
/// the next number, the slot it started and what it runs.
///
/// # Errors
///
/// If it cannot be written.
pub fn begin_in(dir: &Path, slot: &str, items: Vec<Item>, now: i64) -> std::io::Result<Start> {
    let number = all_in(dir).last().map_or(1, |last| last.number + 1);
    let start = Start {
        number,
        started: now,
        slot: slot.into(),
        stage: "services".into(),
        outcome: Outcome::Unfinished,
        error: None,
        why: None,
        items,
    };
    write_in(dir, &start)?;
    Ok(start)
}

/// The error ID that ends `why`, such as `(FW-101)`.
#[must_use]
pub fn error_id(why: &str) -> Option<String> {
    let inner = why.trim_end().strip_suffix(')')?;
    let id = &inner[inner.rfind('(')? + 1..];
    let (letters, digits) = id.split_once('-')?;
    let fits = (2..=5).contains(&letters.len())
        && letters.bytes().all(|b| b.is_ascii_uppercase())
        && digits.len() == 3
        && digits.bytes().all(|b| b.is_ascii_digit());
    fits.then(|| id.to_string())
}

/// Finishes `start` with `outcome`, and why it failed, in `dir`.
///
/// # Errors
///
/// If it cannot be written.
pub fn finish_in(
    dir: &Path,
    mut start: Start,
    outcome: Outcome,
    why: Option<&str>,
) -> std::io::Result<Start> {
    start.outcome = outcome;
    start.error = why.and_then(error_id);
    start.why = why.map(str::to_string);
    write_in(dir, &start)?;
    Ok(start)
}

/// The last good start of `starts` before start `before`.
#[must_use]
pub fn last_good(starts: &[Start], before: u64) -> Option<&Start> {
    starts.iter().rev().find(|start| start.number < before && start.outcome == Outcome::Good)
}

/// One difference between two starts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Change {
    /// What kind of thing, as [`Item::kind`].
    pub kind: String,
    /// Which one.
    pub name: String,
    /// What the earlier start ran, if it had it.
    pub before: Option<String>,
    /// What the later start runs, if it has it.
    pub after: Option<String>,
    /// When what the later start runs was installed, where known.
    pub installed: Option<i64>,
    /// The first start of `starts` that ran it, and when.
    pub first_used: Option<(u64, i64)>,
}

/// What changed from start `from` to start `to`, with when each new
/// thing was installed and which of `starts` first used it, newest first.
#[must_use]
pub fn changes(starts: &[Start], from: &Start, to: &Start) -> Vec<Change> {
    let key = |item: &Item| (item.kind.clone(), item.name.clone());
    let mut found: Vec<Change> = Vec::new();
    for item in &to.items {
        let before = from.items.iter().find(|old| key(old) == key(item));
        if before.is_some_and(|old| old.value == item.value) {
            continue;
        }
        let first_used = starts
            .iter()
            .find(|start| {
                start.items.iter().any(|seen| key(seen) == key(item) && seen.value == item.value)
            })
            .map(|start| (start.number, start.started));
        found.push(Change {
            kind: item.kind.clone(),
            name: item.name.clone(),
            before: before.map(|old| old.value.clone()),
            after: Some(item.value.clone()),
            installed: item.installed,
            first_used,
        });
    }
    for old in from.items.iter().filter(|old| !to.items.iter().any(|item| key(item) == key(old))) {
        found.push(Change {
            kind: old.kind.clone(),
            name: old.name.clone(),
            before: Some(old.value.clone()),
            after: None,
            installed: None,
            first_used: None,
        });
    }
    // Newest first: by when each was first used, then installed.
    found.sort_by_key(|change| {
        std::cmp::Reverse((change.first_used.map(|(n, _)| n), change.installed))
    });
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_are_kept_numbered_and_compared() {
        let dir = std::env::temp_dir().join(format!("starts-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let items = |set: &str, card: bool| {
            let mut items = vec![
                Item::new("system", "Dukovo", "0.15.0", Some(100)),
                Item::new("firmware", "set", set, Some(200)),
            ];
            if card {
                items.push(Item::new(
                    "card",
                    "0000:00:03.0 52:54:00:12:34:56",
                    "Intel 82540EM",
                    None,
                ));
            }
            items
        };
        let first = begin_in(&dir, "a", items("2026.1.1", true), 1000).unwrap();
        assert_eq!((first.number, first.outcome), (1, Outcome::Unfinished));
        finish_in(&dir, first, Outcome::Good, None).unwrap();
        let second = begin_in(&dir, "b", items("2026.2.1", false), 2000).unwrap();
        let why = "the CPU microcode of firmware 2026.2.1 has data that failed its checks (FW-101)";
        let second = finish_in(&dir, second, Outcome::Failed, Some(why)).unwrap();
        assert_eq!(second.error.as_deref(), Some("FW-101"));
        let starts = all_in(&dir);
        assert_eq!(starts.len(), 2);
        let good = last_good(&starts, second.number).unwrap();
        assert_eq!(good.number, 1);
        let found = changes(&starts, good, &second);
        assert_eq!(found.len(), 2);
        assert_eq!(
            (found[0].before.as_deref(), found[0].after.as_deref(), found[0].installed),
            (Some("2026.1.1"), Some("2026.2.1"), Some(200))
        );
        assert_eq!(found[0].first_used, Some((2, 2000)));
        assert_eq!((found[1].kind.as_str(), found[1].after.as_deref()), ("card", None));
        // Only the last KEEP are kept.
        let mut last = second;
        for _ in 0..KEEP {
            last = begin_in(&dir, "b", Vec::new(), 3000).unwrap();
        }
        let kept = all_in(&dir);
        assert_eq!((kept.len() as u64, kept[0].number), (KEEP, last.number + 1 - KEEP));
        assert_eq!(error_id("no ID here"), None);
        assert_eq!(error_id("it is missing (BOOT-101)").as_deref(), Some("BOOT-101"));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
