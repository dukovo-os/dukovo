// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! What was done to the certificates, kept in `/system/state/certd/history`
//! (certd(8)): one JSON line for each event, written by certd and by `cert`
//! while they hold the store's lock.
//!
//! A line is written with one `write` to a file opened for appending, so
//! that a reader sees whole lines or none. When the file passes
//! [`CUT_AT`] lines it is rewritten with the newest [`KEEP`], through a
//! temporary file and a rename, so that a reader never sees it half cut.
//! The file is 0644: certificates are public and the words hold no secret.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::{FileExt, OpenOptionsExt, PermissionsExt};
use std::path::Path;

use serde::{Deserialize, Serialize};

use super::store::{Lock, Store};
use crate::util;

/// How many lines are kept when the file is cut.
pub const KEEP: usize = 1_000;

/// How many lines the file may hold before it is cut.
pub const CUT_AT: usize = 2_000;

/// What happened.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Event {
    /// certd made the machine's own certificate.
    Made,
    /// A certificate from an authority was renewed.
    Renewed,
    /// A certificate was got from an authority for the first time.
    Got,
    /// A certificate was added by hand.
    Added,
    /// A certificate added by hand was replaced.
    Replaced,
    /// A certificate was revoked at its authority.
    Revoked,
    /// A certificate was removed.
    Removed,
    /// Getting or renewing a certificate failed.
    Failed,
}

impl Event {
    /// The word the file and `cert history` use.
    pub fn word(self) -> &'static str {
        match self {
            Event::Made => "made",
            Event::Renewed => "renewed",
            Event::Got => "got",
            Event::Added => "added",
            Event::Replaced => "replaced",
            Event::Revoked => "revoked",
            Event::Removed => "removed",
            Event::Failed => "failed",
        }
    }
}

/// Who did it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum By {
    Certd,
    Cert,
}

impl By {
    /// The word the file and `cert history` use.
    pub fn word(self) -> &'static str {
        match self {
            By::Certd => "certd",
            By::Cert => "cert",
        }
    }
}

/// One line of the history.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    /// When, in milliseconds since 1970.
    pub at_ms: i64,
    /// The certificate's name.
    pub name: String,
    pub event: Event,
    /// In words, with an error ID at the end in parentheses when there is
    /// one.
    pub detail: String,
    /// How long an order took, for those that were.
    pub took_ms: Option<u64>,
    pub by: By,
}

impl Entry {
    /// An event of certificate `name` that happened at `now`, in seconds
    /// since 1970.
    pub fn new(now: i64, name: &str, event: Event, detail: &str, by: By) -> Entry {
        Entry {
            at_ms: now.saturating_mul(1_000),
            name: name.to_string(),
            event,
            detail: detail.to_string(),
            took_ms: None,
            by,
        }
    }

    /// The entry, with how long an order took.
    pub fn took(self, took_ms: u64) -> Entry {
        Entry { took_ms: Some(took_ms), ..self }
    }
}

/// Appends `entry` as one line, and cuts the file when it holds more than
/// [`CUT_AT`] lines. The lock says that the caller holds the store's.
///
/// # Errors
///
/// With why the file cannot be written.
pub fn append(store: &Store, _lock: &Lock, entry: &Entry) -> Result<(), String> {
    append_to(&store.history(), entry)
}

/// Appends `entry` as [`append`] does, holding the store's lock. A failure
/// is told on standard error and goes no further: a history that cannot be
/// written does not stop what it records.
pub fn record(store: &Store, lock: &Lock, entry: &Entry) {
    if let Err(why) = append(store, lock, entry) {
        eprintln!("cert: the history cannot be written: {why}");
    }
}

/// Appends `entry`, taking the store's lock for the moment it takes. A
/// failure is told on standard error and goes no further: a history that
/// cannot be written does not stop what it records.
pub fn note(store: &Store, entry: &Entry) {
    let result = store.lock().and_then(|lock| append(store, &lock, entry));
    if let Err(why) = result {
        eprintln!("cert: the history cannot be written: {why}");
    }
}

fn append_to(path: &Path, entry: &Entry) -> Result<(), String> {
    let mut line = serde_json::to_string(entry).map_err(|why| why.to_string())?;
    line.push('\n');
    let mut file = OpenOptions::new()
        .read(true)
        .append(true)
        .create(true)
        .mode(0o644)
        .open(path)
        .map_err(|err| failed(path, &err))?;
    file.set_permissions(fs::Permissions::from_mode(0o644)).map_err(|err| failed(path, &err))?;
    // A crash may have left the last line unfinished: this one starts on a
    // line of its own, so that only the torn one is lost.
    let length = file.metadata().map_err(|err| failed(path, &err))?.len();
    let mut last = *b"\n";
    if length > 0 {
        file.read_exact_at(&mut last, length - 1).map_err(|err| failed(path, &err))?;
    }
    if last != *b"\n" {
        line.insert(0, '\n');
    }
    file.write_all(line.as_bytes()).map_err(|err| failed(path, &err))?;
    file.sync_all().map_err(|err| failed(path, &err))?;
    cut(path)
}

/// Rewrites the file with its newest [`KEEP`] lines if it holds more than
/// [`CUT_AT`].
fn cut(path: &Path) -> Result<(), String> {
    let text = fs::read_to_string(path).map_err(|err| failed(path, &err))?;
    let lines: Vec<&str> = text.lines().collect();
    if lines.len() <= CUT_AT {
        return Ok(());
    }
    let mut kept = lines[lines.len() - KEEP..].join("\n");
    kept.push('\n');
    let temporary = path.with_extension("new");
    let _ = fs::remove_file(&temporary);
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o644)
        .open(&temporary)
        .map_err(|err| failed(&temporary, &err))?;
    file.set_permissions(fs::Permissions::from_mode(0o644))
        .map_err(|err| failed(&temporary, &err))?;
    file.write_all(kept.as_bytes()).map_err(|err| failed(&temporary, &err))?;
    file.sync_all().map_err(|err| failed(&temporary, &err))?;
    fs::rename(&temporary, path).map_err(|err| failed(path, &err))?;
    if let Some(folder) = path.parent() {
        File::open(folder)
            .and_then(|folder| folder.sync_all())
            .map_err(|err| failed(folder, &err))?;
    }
    Ok(())
}

/// Every entry of the history, the oldest first. A line that is not an
/// entry, as a crash may leave at the end, is passed over; no file is no
/// entry.
///
/// # Errors
///
/// With why the file cannot be read.
pub fn read(store: &Store) -> Result<Vec<Entry>, String> {
    read_from(&store.history())
}

fn read_from(path: &Path) -> Result<Vec<Entry>, String> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => return Err(failed(path, &err)),
    };
    Ok(text.lines().filter_map(|line| serde_json::from_str(line).ok()).collect())
}

/// `path: reason`, without Rust's " (os error N)".
fn failed(path: &Path, err: &io::Error) -> String {
    format!("{}: {}", path.display(), util::strip_os_error(err))
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::MetadataExt;

    use super::*;
    use crate::cert::fixtures::Scratch;

    fn entry(number: i64) -> Entry {
        Entry::new(
            1_800_000_000 + number,
            "a.example",
            Event::Renewed,
            &format!("n{number}"),
            By::Certd,
        )
    }

    #[test]
    fn a_line_is_json_with_the_documented_members_in_order() {
        let line = serde_json::to_string(&entry(5).took(4_000)).unwrap();
        assert_eq!(
            line,
            "{\"at_ms\":1800000005000,\"name\":\"a.example\",\"event\":\"renewed\",\
             \"detail\":\"n5\",\"took_ms\":4000,\"by\":\"certd\"}"
        );
        let plain =
            serde_json::to_string(&Entry::new(1, "x", Event::Failed, "no", By::Cert)).unwrap();
        assert!(plain.contains("\"took_ms\":null") && plain.ends_with("\"by\":\"cert\"}"));
        for (event, word) in [
            (Event::Renewed, "renewed"),
            (Event::Made, "made"),
            (Event::Got, "got"),
            (Event::Added, "added"),
            (Event::Replaced, "replaced"),
            (Event::Revoked, "revoked"),
            (Event::Removed, "removed"),
            (Event::Failed, "failed"),
        ] {
            assert_eq!(serde_json::to_string(&event).unwrap(), format!("\"{word}\""));
            assert_eq!(event.word(), word);
        }
        assert_eq!(By::Certd.word(), "certd");
        assert_eq!(By::Cert.word(), "cert");
    }

    #[test]
    fn entries_are_appended_in_order_to_a_file_anyone_reads() {
        let scratch = Scratch::new("history-append");
        let path = scratch.0.join("history");
        for number in 0..3 {
            append_to(&path, &entry(number)).unwrap();
        }
        let read = read_from(&path).unwrap();
        assert_eq!(read, [entry(0), entry(1), entry(2)]);
        assert_eq!(fs::metadata(&path).unwrap().mode() & 0o7777, 0o644);
        assert!(read_from(&scratch.0.join("none")).unwrap().is_empty());
    }

    #[test]
    fn the_file_is_cut_to_the_newest_thousand_when_it_passes_two_thousand() {
        let scratch = Scratch::new("history-cut");
        let path = scratch.0.join("history");
        for number in 0..i64::try_from(CUT_AT).unwrap() {
            append_to(&path, &entry(number)).unwrap();
        }
        // 2,000 lines are not yet more than 2,000.
        assert_eq!(read_from(&path).unwrap().len(), CUT_AT);
        append_to(&path, &entry(2_000)).unwrap();
        let read = read_from(&path).unwrap();
        assert_eq!(read.len(), KEEP);
        assert_eq!(read[0], entry(1_001));
        assert_eq!(read[KEEP - 1], entry(2_000));
        assert_eq!(fs::metadata(&path).unwrap().mode() & 0o7777, 0o644);
        assert!(!path.with_extension("new").exists());
        append_to(&path, &entry(2_001)).unwrap();
        assert_eq!(read_from(&path).unwrap().len(), KEEP + 1);
    }

    #[test]
    fn a_torn_line_is_passed_over() {
        let scratch = Scratch::new("history-torn");
        let path = scratch.0.join("history");
        append_to(&path, &entry(1)).unwrap();
        fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap()
            .write_all(b"{\"at_ms\":18")
            .unwrap();
        assert_eq!(read_from(&path).unwrap(), [entry(1)]);
        // The next line is not lost with it.
        append_to(&path, &entry(2)).unwrap();
        assert_eq!(read_from(&path).unwrap(), [entry(1), entry(2)]);
    }
}
