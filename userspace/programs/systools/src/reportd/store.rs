// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The crashes reportd keeps (ADR 0531), each a file of JSON in
//! `/system/state/reports/crashes`, root's alone, and the signatures it has
//! offered, in `offered.json`.
//!
//! A crash holds what a report of it needs and nothing of the program's
//! memory: the program, its version and service, who it ran as, when,
//! what ended it, its trace as lines, and its signature. A crash whose
//! signature a kept crash has, not reported or put away, counts on that
//! one rather than adding a file. At most [`KEPT_MAX`] crashes in
//! [`BYTES_MAX`] are kept, none longer than [`DAYS_KEPT`] days; a
//! signature offered stays in `offered.json` for a year, so that the same
//! crash is never offered twice.

use std::collections::BTreeMap;
use std::fs;
use std::io::{self, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::reports::protocol::Crash;

/// Where reportd keeps what it keeps.
pub const DIR: &str = "/system/state/reports";
/// The most crashes kept, and their most bytes in all.
pub const KEPT_MAX: usize = 100;
pub const BYTES_MAX: u64 = 8 * 1024 * 1024;
/// How long a crash is kept, and a signature known as offered.
pub const DAYS_KEPT: u64 = 30;
const OFFERED_DAYS: u64 = 365;
const DAY: u64 = 86_400;

/// A crash as its file holds it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Kept {
    /// What `crashes` lists of it.
    pub crash: Crash,
    /// The signature itself, of which `crash.signature` is the ID.
    pub signature: String,
    /// The lines of a report's Crash section: what ended it, the
    /// signature's ID, then the trace, a frame a line.
    pub lines: Vec<String>,
    /// Put away with `report forget`: listed, never offered.
    #[serde(default)]
    pub forgotten: bool,
    /// Whether this crash made the offer of its signature, which the
    /// sign-in notice shows until it is reported or put away.
    #[serde(default)]
    pub offered: bool,
}

/// The files of a store: in [`DIR`], or another folder for tests.
pub struct Store {
    dir: PathBuf,
}

impl Store {
    pub fn new(dir: &Path) -> Store {
        Store { dir: dir.to_path_buf() }
    }

    /// The folder.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn crashes(&self) -> PathBuf {
        self.dir.join("crashes")
    }

    /// Makes the folders, root's alone.
    ///
    /// # Errors
    ///
    /// If one cannot be made.
    pub fn make(&self) -> io::Result<()> {
        for dir in [self.dir.clone(), self.crashes(), self.dir.join("queue"), self.dir.join("sent")]
        {
            fs::DirBuilder::new().recursive(true).mode(0o700).create(&dir)?;
        }
        Ok(())
    }

    /// Every crash kept, newest first; files that do not read are left
    /// out.
    pub fn all(&self) -> Vec<Kept> {
        let Ok(entries) = fs::read_dir(self.crashes()) else { return Vec::new() };
        let mut kept: Vec<Kept> = entries
            .flatten()
            .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
            .filter_map(|e| serde_json::from_str(&fs::read_to_string(e.path()).ok()?).ok())
            .collect();
        kept.sort_by(|a, b| b.crash.at.cmp(&a.crash.at).then(b.crash.id.cmp(&a.crash.id)));
        kept
    }

    /// The crash `id`, if kept.
    pub fn get(&self, id: &str) -> Option<Kept> {
        if !valid_id(id) {
            return None;
        }
        serde_json::from_str(&fs::read_to_string(self.crashes().join(format!("{id}.json"))).ok()?)
            .ok()
    }

    /// Writes `kept`, whole or not at all.
    ///
    /// # Errors
    ///
    /// If it cannot be written.
    pub fn put(&self, kept: &Kept) -> io::Result<()> {
        let text = serde_json::to_string_pretty(kept).map_err(io::Error::other)?;
        write_whole(&self.crashes(), &format!("{}.json", kept.crash.id), text.as_bytes())
    }

    /// Takes in `new`: counted on a kept crash of its signature that is
    /// neither reported nor put away, which it returns; else kept as a new
    /// one. Then trims the store.
    ///
    /// # Errors
    ///
    /// If a file cannot be written.
    pub fn add(&self, new: Kept, now: u64) -> io::Result<Kept> {
        let same = self.all().into_iter().find(|k| {
            k.crash.signature == new.crash.signature && k.crash.reported.is_none() && !k.forgotten
        });
        let kept = match same {
            Some(mut same) => {
                same.crash.seen = same.crash.seen.saturating_add(1);
                same
            }
            None => new,
        };
        self.put(&kept)?;
        self.trim(now);
        Ok(kept)
    }

    /// Deletes the crashes past [`DAYS_KEPT`] days old, and the oldest
    /// past [`KEPT_MAX`] or [`BYTES_MAX`].
    pub fn trim(&self, now: u64) {
        let Ok(entries) = fs::read_dir(self.crashes()) else { return };
        let mut files: Vec<(u64, PathBuf, u64)> = entries
            .flatten()
            .filter_map(|e| {
                let kept: Kept = serde_json::from_str(&fs::read_to_string(e.path()).ok()?).ok()?;
                Some((kept.crash.at, e.path(), e.metadata().ok()?.len()))
            })
            .collect();
        files.sort_by_key(|file| std::cmp::Reverse(file.0));
        let mut bytes = 0;
        for (count, (at, path, len)) in files.into_iter().enumerate() {
            bytes += len;
            if count >= KEPT_MAX || bytes > BYTES_MAX || now.saturating_sub(at) > DAYS_KEPT * DAY {
                let _ = fs::remove_file(path);
            }
        }
    }

    /// The signatures offered, with when.
    pub fn offered(&self) -> BTreeMap<String, u64> {
        fs::read_to_string(self.dir.join("offered.json"))
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default()
    }

    /// Notes `signature` as offered at `now`, and forgets those offered
    /// more than a year ago; whether it was offered before.
    ///
    /// # Errors
    ///
    /// If the file cannot be written.
    pub fn offer(&self, signature: &str, now: u64) -> io::Result<bool> {
        let mut offered = self.offered();
        offered.retain(|_, at| now.saturating_sub(*at) <= OFFERED_DAYS * DAY);
        if offered.contains_key(signature) {
            return Ok(true);
        }
        offered.insert(signature.to_string(), now);
        let text = serde_json::to_string(&offered).map_err(io::Error::other)?;
        write_whole(&self.dir, "offered.json", text.as_bytes())?;
        Ok(false)
    }
}

/// Whether `id` is a crash's ID: digits, `T` and `-` only, so that it
/// names a file in the folder and nothing outside it.
pub fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 40
        && id.chars().all(|c| c.is_ascii_digit() || c == 'T' || c == '-')
}

/// A crash's ID from when it was, in seconds since 1970, and its process
/// number: `20261113T100211-4012`.
pub fn crash_id(at: u64, process: u64) -> String {
    let (year, month, day) = crate::logd::civil(i64::try_from(at / DAY).unwrap_or(0));
    let seconds = at % DAY;
    format!(
        "{year:04}{month:02}{day:02}T{:02}{:02}{:02}-{process}",
        seconds / 3600,
        seconds / 60 % 60,
        seconds % 60
    )
}

/// Writes `bytes` as `name` in `dir`: to another name first, synced, then
/// renamed over it and the folder synced, mode 0600.
///
/// # Errors
///
/// If any step fails.
pub fn write_whole(dir: &Path, name: &str, bytes: &[u8]) -> io::Result<()> {
    let new = dir.join(format!(".{name}.new"));
    let mut file =
        fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&new)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    fs::rename(&new, dir.join(name))?;
    fs::File::open(dir)?.sync_all()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reports::protocol::Kind;

    fn kept(id: &str, at: u64, signature: &str) -> Kept {
        Kept {
            crash: Crash {
                id: id.into(),
                kind: Kind::Signal,
                program: "crash-test".into(),
                version: None,
                service: Some("crash-test".into()),
                user: 0,
                at,
                what: "SIGSEGV in report_test::fault_here".into(),
                signature: signature.into(),
                seen: 1,
                reported: None,
            },
            signature: format!("crash-test SIGSEGV {signature}"),
            lines: vec!["line".into()],
            forgotten: false,
            offered: false,
        }
    }

    /// A store in a folder of its own for the test `test`.
    fn store(test: &str) -> (Store, PathBuf) {
        let dir = std::env::temp_dir().join(format!("reportd-store-{}-{test}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let store = Store::new(&dir);
        store.make().unwrap();
        (store, dir)
    }

    #[test]
    fn the_same_crash_counts_on_the_one_kept_until_it_is_reported() {
        let (store, dir) = store("same");
        let now = 1_763_000_000;
        store.add(kept("1-1", now, "aaaa"), now).unwrap();
        let again = store.add(kept("1-2", now + 5, "aaaa"), now).unwrap();
        assert_eq!((again.crash.id.as_str(), again.crash.seen), ("1-1", 2));
        assert_eq!(store.all().len(), 1);
        let mut reported = store.get("1-1").unwrap();
        reported.crash.reported = Some(7);
        store.put(&reported).unwrap();
        store.add(kept("1-3", now + 9, "aaaa"), now).unwrap();
        let ids: Vec<String> = store.all().into_iter().map(|k| k.crash.id).collect();
        assert_eq!(ids, ["1-3", "1-1"]);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn old_and_surplus_crashes_go() {
        let (store, dir) = store("old");
        let now = 1_763_000_000;
        store.put(&kept("1-1", now - 31 * DAY, "old")).unwrap();
        for n in 0..(KEPT_MAX + 2) {
            store.put(&kept(&format!("2-{n}"), now - n as u64, &format!("s{n}"))).unwrap();
        }
        store.trim(now);
        let all = store.all();
        assert_eq!(all.len(), KEPT_MAX);
        assert!(all.iter().all(|k| k.crash.id != "1-1"));
        assert_eq!(all[0].crash.id, "2-0");
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_signature_is_offered_once() {
        let (store, dir) = store("offered");
        assert!(!store.offer("aaaa", 100).unwrap());
        assert!(store.offer("aaaa", 200).unwrap());
        // A year and a day on, it is forgotten.
        assert!(!store.offer("aaaa", 100 + 366 * DAY).unwrap());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn an_id_names_a_file_in_the_folder_only() {
        assert_eq!(crash_id(1_763_028_131, 4012), "20251113T100211-4012");
        assert!(valid_id("20261113T100211-4012"));
        assert!(!valid_id("../x"));
        assert!(!valid_id(""));
    }
}
