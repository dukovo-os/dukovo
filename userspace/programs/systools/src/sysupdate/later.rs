// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The reminder put off (ADR 0565), in `/system/state/sysupdate/later.json`:
//! `update later ITEM --for 1d|7d` writes an entry, and every reader that
//! would remind of the update asks [`stands`] whether the entry still
//! puts it off. One rule decides for all of them: updated's daily check,
//! dukovo-check's `waiting` line and the documents the web console reads.
//!
//! An entry holds the version offered when it was made. It puts off the
//! reminder only while its time has not come, the version offered now is
//! that one, and the update fixes no advisory: a newer release, an
//! advisory that names the version, or a reader that cannot tell whether
//! one does (it has no advisories, or could not read them) ends it, so
//! that a reminder put off never hides a fix. The file is not
//! updates.toml(5), which has no key for it.
//!
//! The advisories a reader has are its own: updated's are those it read
//! from the server in the same round, and a reader with no network
//! (dukovo-check, `update settings`) goes by the last check root kept in
//! `/system/state/sysupdate/last-check.json` ([`Fixes::in_record`]).

use std::io;
use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::json;

/// The file, one line of JSON, 0644, replaced whole.
pub const FILE: &str = "/system/state/sysupdate/later.json";

/// The mode of the file: everyone reads it.
const MODE: u32 = 0o644;

/// The most entries kept: an entry is for an item that has an offer.
const MOST: usize = 256;

/// An update put off.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    /// `system`, `firmware` or an app's ID.
    pub item: String,
    /// The version offered when it was put off.
    pub version: String,
    /// Until when, in milliseconds since 1970.
    pub until_ms: u64,
    /// The user ID of who put it off; for an administrator who used
    /// `elevate`, their own and not root's.
    pub by: u32,
    /// When, in milliseconds since 1970.
    pub at_ms: u64,
}

/// What the file holds: one entry for each item.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Later {
    pub later: Vec<Entry>,
}

/// Whether the update offered fixes an advisory, as far as a reader can
/// tell.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fixes {
    /// It fixes none that applies.
    No,
    /// It fixes one.
    Yes,
    /// The reader cannot tell.
    Unknown,
}

/// Whether `entry` puts off the reminder of the version `offered` now, at
/// `now_ms` milliseconds since 1970, given what the reader knows of the
/// advisories: only while its time has not come, the version offered is
/// the one it was made for, and the update fixes no advisory. A reader
/// that cannot tell, [`Fixes::Unknown`], shows the reminder.
#[must_use]
pub fn stands(entry: &Entry, offered: &str, fixes: Fixes, now_ms: u64) -> bool {
    now_ms < entry.until_ms && entry.version == offered && fixes == Fixes::No
}

/// Whether `item` is an app's ID, and not the system or the firmware.
#[must_use]
pub fn is_app(item: &str) -> bool {
    item != "system" && item != "firmware"
}

impl Later {
    /// The entries kept, none when there is no file or it does not read.
    #[must_use]
    pub fn read() -> Later {
        Later::read_from(Path::new(FILE))
    }

    /// The entries `file` keeps.
    #[must_use]
    pub fn read_from(file: &Path) -> Later {
        Later::parse(&std::fs::read_to_string(file).unwrap_or_default())
    }

    /// The entries the file text `text` holds: none for a text that is not
    /// the record or is of another format.
    #[must_use]
    pub fn parse(text: &str) -> Later {
        let Ok(value) = serde_json::from_str::<Value>(text) else { return Later::default() };
        if value.get("format").and_then(Value::as_u64) != Some(u64::from(json::FORMAT)) {
            return Later::default();
        }
        serde_json::from_value(value).unwrap_or_default()
    }

    /// The entry of `item`, whether it stands or not.
    #[must_use]
    pub fn entry(&self, item: &str) -> Option<&Entry> {
        self.later.iter().find(|entry| entry.item == item)
    }

    /// The entry of `item` if it puts off the reminder of `offered` now.
    #[must_use]
    pub fn standing(&self, item: &str, offered: &str, fixes: Fixes, now_ms: u64) -> Option<&Entry> {
        self.entry(item).filter(|entry| stands(entry, offered, fixes, now_ms))
    }

    /// Keeps `entry` for its item, in place of the one there was.
    pub fn put(&mut self, entry: Entry) {
        self.later.retain(|one| one.item != entry.item);
        self.later.push(entry);
    }

    /// Takes away the entry of `item`; whether there was one.
    pub fn off(&mut self, item: &str) -> bool {
        let before = self.later.len();
        self.later.retain(|entry| entry.item != item);
        self.later.len() != before
    }

    /// Takes away the entries whose time is past at `now_ms`, which no
    /// reader would count, and the oldest that are over [`MOST`].
    pub fn tidy(&mut self, now_ms: u64) {
        self.later.retain(|entry| now_ms < entry.until_ms);
        if self.later.len() > MOST {
            self.later.sort_by_key(|entry| entry.at_ms);
            let over = self.later.len() - MOST;
            self.later.drain(..over);
        }
    }

    /// The file's text: one line, with its newline.
    ///
    /// # Panics
    ///
    /// Never: strings and numbers always serialize.
    #[must_use]
    pub fn text(&self) -> String {
        let mut line = json::line(self).expect("entries are strings and numbers");
        line.push('\n');
        line
    }

    /// Writes the entries to the file.
    ///
    /// # Errors
    ///
    /// If the file cannot be replaced.
    pub fn save(&self) -> io::Result<()> {
        self.save_to(Path::new(FILE))
    }

    /// Writes the entries to `file`.
    ///
    /// # Errors
    ///
    /// If `file` cannot be replaced.
    pub fn save_to(&self, file: &Path) -> io::Result<()> {
        crate::util::replace_whole(file, &self.text(), MODE)
    }
}

/// What the last check root kept, `last-check.json`, says of an item: for
/// a reader that has no network.
///
/// The record holds the document `update check --json` printed, whose
/// offers say the version each item was offered, and whose `security` part
/// says which advisories fix the system's offer and how many flaws the
/// apps have.
impl Fixes {
    /// Whether the update `offered` of `item` fixes an advisory, going by
    /// the last check's `record` (its text, or none when none is kept).
    /// The record tells of the system's advisories in full and of the
    /// apps' by count alone, so that an app's update is `No` only while no
    /// app has a known flaw. A record whose security part was not read,
    /// or none, cannot tell.
    #[must_use]
    pub fn in_record(record: Option<&str>, item: &str, offered: &str) -> Fixes {
        let Some(updates) = record.and_then(updates_of) else { return Fixes::Unknown };
        let Some(security) = updates.get("security") else { return Fixes::Unknown };
        let unread = updates
            .get("unchecked")
            .and_then(Value::as_array)
            .is_none_or(|parts| parts.iter().any(|part| part["item"] == "security"));
        if unread {
            return Fixes::Unknown;
        }
        if is_app(item) {
            return match security.get("app_flaws").and_then(Value::as_u64) {
                Some(0) => Fixes::No,
                _ => Fixes::Unknown,
            };
        }
        let system = security.get("system").and_then(Value::as_array);
        let fixed = system.is_some_and(|flaws| flaws.iter().any(|flaw| flaw["release"] == offered));
        match (system, fixed) {
            (None, _) => Fixes::Unknown,
            (Some(_), true) => Fixes::Yes,
            (Some(_), false) => Fixes::No,
        }
    }
}

/// The document `update check --json` printed, in the record's text of it.
fn updates_of(record: &str) -> Option<Value> {
    let mut record: Value = serde_json::from_str(record).ok()?;
    if record.get("format").and_then(Value::as_u64) != Some(u64::from(json::FORMAT)) {
        return None;
    }
    let updates = record.get_mut("updates")?.take();
    (updates.get("format").and_then(Value::as_u64) == Some(u64::from(json::FORMAT)))
        .then_some(updates)
}

/// The version the last check's `record` says is on offer for `item`, if
/// an update is: the one `update later` would put off.
#[must_use]
pub fn offered_in(record: Option<&str>, item: &str) -> Option<String> {
    let updates = record.and_then(updates_of)?;
    let offer = updates
        .get("offers")?
        .as_array()?
        .iter()
        .find(|offer| offer["item"] == item && offer["state"] == "update")?;
    offer.get("to")?.as_str().map(str::to_string)
}

/// An entry as `update settings --json` says it.
#[derive(Debug, Serialize)]
pub struct Said {
    pub item: String,
    pub version: String,
    pub until_ms: u64,
    pub by: u32,
    pub at_ms: u64,
    /// Whether it puts off the reminder now, by the last check.
    pub stands: bool,
}

impl Later {
    /// The entries as `update settings --json` says them, `stands` by the
    /// last check's `record` and the time `now_ms`.
    #[must_use]
    pub fn said(&self, record: Option<&str>, now_ms: u64) -> Vec<Said> {
        self.later
            .iter()
            .map(|entry| {
                let offered = offered_in(record, &entry.item);
                let stands = offered.as_deref().is_some_and(|offered| {
                    let fixes = Fixes::in_record(record, &entry.item, offered);
                    stands(entry, offered, fixes, now_ms)
                });
                Said {
                    item: entry.item.clone(),
                    version: entry.version.clone(),
                    until_ms: entry.until_ms,
                    by: entry.by,
                    at_ms: entry.at_ms,
                    stands,
                }
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use serde_json::json;

    use super::*;

    const NOW: u64 = 1_794_657_630_000;
    const DAY: u64 = 86_400_000;

    fn entry(item: &str, version: &str, until_ms: u64) -> Entry {
        Entry { item: item.into(), version: version.into(), until_ms, by: 1000, at_ms: NOW }
    }

    fn scratch(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("later-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("later.json")
    }

    #[test]
    fn an_entry_stands_only_before_its_time_for_its_version_and_a_fix_that_is_not_one() {
        let put = entry("system", "1.0.0-alpha.2", NOW + DAY);
        assert!(stands(&put, "1.0.0-alpha.2", Fixes::No, NOW));
        assert!(stands(&put, "1.0.0-alpha.2", Fixes::No, NOW + DAY - 1));
        // Its time has come, to the millisecond.
        assert!(!stands(&put, "1.0.0-alpha.2", Fixes::No, NOW + DAY));
        assert!(!stands(&put, "1.0.0-alpha.2", Fixes::No, NOW + 2 * DAY));
        // A newer release is not what was put off.
        assert!(!stands(&put, "1.0.0-alpha.3", Fixes::No, NOW));
        // An update that fixes an advisory is never put off, and one that cannot be told of is not.
        assert!(!stands(&put, "1.0.0-alpha.2", Fixes::Yes, NOW));
        assert!(!stands(&put, "1.0.0-alpha.2", Fixes::Unknown, NOW));
    }

    #[test]
    fn an_entry_is_kept_for_each_item_and_replaced_or_taken_away() {
        let mut later = Later::default();
        later.put(entry("system", "1.0.0-alpha.2", NOW + DAY));
        later.put(entry("org.example.notes", "1.3.0", NOW + 7 * DAY));
        later.put(entry("system", "1.0.0-alpha.3", NOW + 7 * DAY));
        assert_eq!(later.later.len(), 2);
        assert_eq!(later.entry("system").unwrap().version, "1.0.0-alpha.3");
        assert_eq!(
            later.standing("system", "1.0.0-alpha.3", Fixes::No, NOW).map(|e| e.by),
            Some(1000)
        );
        assert_eq!(later.standing("system", "1.0.0-alpha.2", Fixes::No, NOW), None);
        assert_eq!(later.standing("firmware", "2026.3", Fixes::No, NOW), None);
        assert!(later.off("system"));
        assert!(!later.off("system"));
        assert_eq!(later.later.len(), 1);
    }

    #[test]
    fn an_entry_whose_time_is_past_is_tidied_away_and_the_most_kept_are_the_newest() {
        let mut later = Later::default();
        later.put(entry("system", "1.0.0-alpha.2", NOW - 1));
        later.put(entry("firmware", "2026.3", NOW + DAY));
        later.tidy(NOW);
        assert_eq!(later.later.iter().map(|e| e.item.as_str()).collect::<Vec<_>>(), ["firmware"]);
        let mut many = Later::default();
        for n in 0..(MOST + 5) {
            let mut one = entry(&format!("org.example.app{n}"), "1.0.0", NOW + DAY);
            one.at_ms = NOW + n as u64;
            many.put(one);
        }
        many.tidy(NOW);
        assert_eq!(many.later.len(), MOST);
        assert!(many.entry("org.example.app0").is_none());
        assert!(many.entry(&format!("org.example.app{}", MOST + 4)).is_some());
    }

    #[test]
    fn the_entries_are_written_whole_and_read_back() {
        let file = scratch("whole");
        let mut later = Later::default();
        later.put(entry("system", "1.0.0-alpha.2", NOW + DAY));
        later.save_to(&file).unwrap();
        assert_eq!(
            std::fs::read_to_string(&file).unwrap(),
            "{\"format\":1,\"later\":[{\"item\":\"system\",\"version\":\"1.0.0-alpha.2\",\
             \"until_ms\":1794744030000,\"by\":1000,\"at_ms\":1794657630000}]}\n"
        );
        assert_eq!(std::fs::metadata(&file).unwrap().permissions().mode() & 0o777, 0o644);
        assert_eq!(Later::read_from(&file), later);
        Later::default().save_to(&file).unwrap();
        assert_eq!(Later::read_from(&file), Later::default());
        assert!(!file.with_extension("json.partial").exists());
        let _ = std::fs::remove_dir_all(file.parent().unwrap()); // a test's leftovers
    }

    #[test]
    fn a_file_that_is_missing_or_not_the_record_holds_no_entry() {
        let file = scratch("missing");
        assert_eq!(Later::read_from(&file), Later::default());
        for text in [
            "later\n",
            "{\"later\":[]}",
            "{\"format\":2,\"later\":[{\"item\":\"system\",\"version\":\"1\",\"until_ms\":9,\"by\":0,\"at_ms\":1}]}",
            "{\"format\":1,\"later\":[{\"item\":\"system\"}]}",
        ] {
            std::fs::write(&file, text).unwrap();
            assert_eq!(Later::read_from(&file), Later::default(), "{text:?}");
        }
        let _ = std::fs::remove_dir_all(file.parent().unwrap()); // a test's leftovers
    }

    /// The record of a check that offered the system 1.0.0-alpha.2, which
    /// fixes `DKSA-2026-0007` when `fixes`, the firmware 2026.3 and an app
    /// 1.3.0, and that read its advisories unless `unread`.
    fn record(fixes: bool, unread: bool, app_flaws: u64) -> String {
        let flaws = if fixes {
            json!([{"id": "DKSA-2026-0007", "severity": "high", "release": "1.0.0-alpha.2"}])
        } else {
            json!([{"id": "DKSA-2026-0003", "severity": "low", "release": null}])
        };
        let unchecked = if unread {
            json!([{"item": "security", "why": "the advisories cannot be read"}])
        } else {
            json!([])
        };
        json!({
            "format": 1, "checked_ms": NOW, "by": "updated",
            "updates": {
                "format": 1,
                "offers": [
                    {"item": "system", "from": "1.0.0-alpha.1", "to": "1.0.0-alpha.2", "state": "update"},
                    {"item": "firmware", "from": "2026.2", "to": "2026.3", "state": "held"},
                    {"item": "org.example.notes", "from": "1.2.0", "to": "1.3.0", "state": "update"},
                ],
                "unchecked": unchecked,
                "security": {"system": flaws, "app_flaws": app_flaws},
            }
        })
        .to_string()
    }

    #[test]
    fn the_last_check_says_whether_an_offer_fixes_an_advisory_and_what_it_cannot_tell() {
        let said =
            |text: &str, item: &str, offered: &str| Fixes::in_record(Some(text), item, offered);
        let plain = record(false, false, 0);
        assert_eq!(said(&plain, "system", "1.0.0-alpha.2"), Fixes::No);
        assert_eq!(said(&plain, "org.example.notes", "1.3.0"), Fixes::No);
        // An advisory whose fix is this release.
        let fixing = record(true, false, 0);
        assert_eq!(said(&fixing, "system", "1.0.0-alpha.2"), Fixes::Yes);
        // ... and a release newer than the one it names is not that one.
        assert_eq!(said(&fixing, "system", "1.0.0-alpha.3"), Fixes::No);
        // The apps' flaws are told by count: any known flaw leaves an app's update unknown.
        assert_eq!(said(&record(false, false, 2), "org.example.notes", "1.3.0"), Fixes::Unknown);
        assert_eq!(said(&record(false, false, 2), "system", "1.0.0-alpha.2"), Fixes::No);
        // A security part that was not read, no record, and one that is not.
        assert_eq!(said(&record(false, true, 0), "system", "1.0.0-alpha.2"), Fixes::Unknown);
        assert_eq!(Fixes::in_record(None, "system", "1.0.0-alpha.2"), Fixes::Unknown);
        for text in
            ["", "null", "{}", "{\"format\":1}", "{\"format\":1,\"updates\":{\"format\":1}}"]
        {
            assert_eq!(said(text, "system", "1.0.0-alpha.2"), Fixes::Unknown, "{text:?}");
        }
    }

    #[test]
    fn the_last_check_says_the_version_on_offer_only_for_an_update() {
        let text = record(false, false, 0);
        assert_eq!(offered_in(Some(&text), "system").as_deref(), Some("1.0.0-alpha.2"));
        assert_eq!(offered_in(Some(&text), "org.example.notes").as_deref(), Some("1.3.0"));
        // Held is not on offer to put off, and an item the check did not name is not either.
        assert_eq!(offered_in(Some(&text), "firmware"), None);
        assert_eq!(offered_in(Some(&text), "org.example.tea"), None);
        assert_eq!(offered_in(None, "system"), None);
    }

    #[test]
    fn the_settings_say_each_entry_and_whether_it_stands_by_the_last_check() {
        let mut later = Later::default();
        later.put(entry("system", "1.0.0-alpha.2", NOW + DAY));
        later.put(entry("org.example.notes", "1.2.0", NOW + DAY));
        later.put(entry("firmware", "2026.3", NOW + DAY));
        let plain = record(false, false, 0);
        let said = later.said(Some(&plain), NOW);
        let stands: Vec<(&str, bool)> = said.iter().map(|s| (s.item.as_str(), s.stands)).collect();
        // The app's entry is for a version the check no longer offers, and the firmware's is held.
        assert_eq!(stands, [("system", true), ("org.example.notes", false), ("firmware", false)]);
        // After its time, with an advisory that fixes it, or with no check kept, it does not.
        assert!(!later.said(Some(&plain), NOW + DAY)[0].stands);
        assert!(!later.said(Some(&record(true, false, 0)), NOW)[0].stands);
        assert!(!later.said(None, NOW)[0].stands);
        let value = serde_json::to_value(&said[0]).unwrap();
        assert_eq!(
            value,
            json!({"item": "system", "version": "1.0.0-alpha.2", "until_ms": 1_794_744_030_000_u64,
                   "by": 1000, "at_ms": 1_794_657_630_000_u64, "stands": true})
        );
    }
}
