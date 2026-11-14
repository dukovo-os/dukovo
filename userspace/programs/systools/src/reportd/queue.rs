// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The reports waiting to be sent, and those sent (ADR 0531).
//!
//! A report is queued the moment the person sends it, before any try: its
//! bytes as `queue/<id>.txt` (the ciphertext for the security box) and
//! what reportd needs of it as `queue/<id>.json`, with the key made for
//! it. A try that fails for want of a network or of a working server is
//! tried again 1, 5, 15 and 60 minutes after the one before, then every
//! hour, for [`DAYS_TRIED`] days, and so is one the server refuses, whose
//! refusal is logged, as a server's mistake passes too. Once sent, the
//! queue's files go and `sent/<number>.json` keeps the number, the key,
//! the title and who sent it, for `report status` and `report delete`;
//! the text itself is not kept.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use ring::rand::{SecureRandom, SystemRandom};
use serde::{Deserialize, Serialize};

use super::store::write_whole;

/// How long a report is tried before it is dropped.
pub const DAYS_TRIED: u64 = 30;
const DAY: u64 = 86_400;
/// The waits after each failed try, in seconds; the last repeats.
const WAITS: [u64; 4] = [60, 5 * 60, 15 * 60, 60 * 60];

/// A report in the queue, as its `.json` holds it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    pub id: String,
    /// Who sent it.
    pub user: u32,
    pub title: String,
    /// When it was queued, and is next tried, in seconds since 1970.
    pub since: u64,
    pub next: u64,
    pub tries: u32,
    /// Why the last try failed.
    pub error: Option<String>,
    pub security: bool,
    /// The crash it reports.
    pub crash: Option<String>,
    /// Its key, in hex, which the server knows by its hash.
    pub key: String,
}

/// A report sent, as `sent/<number>.json` holds it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Sent {
    pub number: u64,
    pub user: u32,
    pub title: String,
    /// When it was sent, in seconds since 1970.
    pub sent: u64,
    pub security: bool,
    pub crash: Option<String>,
    pub key: String,
}

/// The queue's and the sent reports' folders.
pub struct Queue {
    dir: PathBuf,
}

impl Queue {
    /// The queue in `dir`, the folder [`super::store::Store`] makes.
    pub fn new(dir: &Path) -> Queue {
        Queue { dir: dir.to_path_buf() }
    }

    fn waiting_dir(&self) -> PathBuf {
        self.dir.join("queue")
    }

    fn sent_dir(&self) -> PathBuf {
        self.dir.join("sent")
    }

    /// Queues `bytes` for `user`, to be tried at once: the entry, with a
    /// new ID and key.
    ///
    /// # Errors
    ///
    /// If no random bytes or no file can be had.
    pub fn add(
        &self,
        bytes: &[u8],
        user: u32,
        title: &str,
        security: bool,
        crash: Option<String>,
        now: u64,
    ) -> io::Result<Entry> {
        let random = SystemRandom::new();
        let mut id = [0u8; 8];
        let mut key = [0u8; 32];
        random.fill(&mut id).and_then(|()| random.fill(&mut key)).map_err(|_| {
            io::Error::other("the system has no random numbers to make the report's key with")
        })?;
        let entry = Entry {
            id: super::server::hex(&id),
            user,
            title: title.to_string(),
            since: now,
            next: now,
            tries: 0,
            error: None,
            security,
            crash,
            key: super::server::hex(&key),
        };
        write_whole(&self.waiting_dir(), &format!("{}.txt", entry.id), bytes)?;
        self.put(&entry)?;
        Ok(entry)
    }

    fn put(&self, entry: &Entry) -> io::Result<()> {
        let text = serde_json::to_string_pretty(entry).map_err(io::Error::other)?;
        write_whole(&self.waiting_dir(), &format!("{}.json", entry.id), text.as_bytes())
    }

    /// Every report waiting, oldest first.
    pub fn waiting(&self) -> Vec<Entry> {
        let mut entries: Vec<Entry> = read_all(&self.waiting_dir());
        entries.sort_by(|a, b| a.since.cmp(&b.since).then(a.id.cmp(&b.id)));
        entries
    }

    /// The bytes of the report `entry` is.
    ///
    /// # Errors
    ///
    /// If its file cannot be read.
    pub fn bytes(&self, entry: &Entry) -> io::Result<Vec<u8>> {
        fs::read(self.waiting_dir().join(format!("{}.txt", entry.id)))
    }

    /// Notes that a try of `entry` at `now` failed for `why`.
    ///
    /// # Errors
    ///
    /// If its file cannot be written.
    pub fn failed(&self, entry: &mut Entry, why: &str, now: u64) -> io::Result<()> {
        entry.tries = entry.tries.saturating_add(1);
        entry.error = Some(why.to_string());
        entry.next = now + wait_after(entry.tries);
        self.put(entry)
    }

    /// Moves `entry` from the queue to the sent reports as `number`.
    ///
    /// # Errors
    ///
    /// If the sent report's file cannot be written; the queue's files then
    /// stay, and the server may get the report twice, which is better
    /// than losing its key.
    pub fn sent(&self, entry: &Entry, number: u64, now: u64) -> io::Result<Sent> {
        let sent = Sent {
            number,
            user: entry.user,
            title: entry.title.clone(),
            sent: now,
            security: entry.security,
            crash: entry.crash.clone(),
            key: entry.key.clone(),
        };
        let text = serde_json::to_string_pretty(&sent).map_err(io::Error::other)?;
        write_whole(&self.sent_dir(), &format!("{number}.json"), text.as_bytes())?;
        self.remove(&entry.id);
        Ok(sent)
    }

    /// Takes report `id` out of the queue.
    pub fn remove(&self, id: &str) {
        if is_hex(id) {
            let _ = fs::remove_file(self.waiting_dir().join(format!("{id}.txt")));
            let _ = fs::remove_file(self.waiting_dir().join(format!("{id}.json")));
        }
    }

    /// Drops the reports tried for [`DAYS_TRIED`] days by `now`: those
    /// dropped.
    pub fn drop_old(&self, now: u64) -> Vec<Entry> {
        let old: Vec<Entry> = self
            .waiting()
            .into_iter()
            .filter(|e| now.saturating_sub(e.since) > DAYS_TRIED * DAY)
            .collect();
        for entry in &old {
            self.remove(&entry.id);
        }
        old
    }

    /// The reports sent, newest first.
    pub fn all_sent(&self) -> Vec<Sent> {
        let mut sent: Vec<Sent> = read_all(&self.sent_dir());
        sent.sort_by(|a, b| b.sent.cmp(&a.sent).then(b.number.cmp(&a.number)));
        sent
    }

    /// The report sent as `number`.
    pub fn get_sent(&self, number: u64) -> Option<Sent> {
        let text = fs::read_to_string(self.sent_dir().join(format!("{number}.json"))).ok()?;
        serde_json::from_str(&text).ok()
    }

    /// Forgets report `number`, once the server deleted it.
    pub fn forget_sent(&self, number: u64) {
        let _ = fs::remove_file(self.sent_dir().join(format!("{number}.json")));
    }
}

/// How long to wait after the `tries`th failed try.
pub fn wait_after(tries: u32) -> u64 {
    let at = (tries.max(1) as usize - 1).min(WAITS.len() - 1);
    WAITS[at]
}

/// Every `.json` in `dir` that reads as a `T`.
fn read_all<T: serde::de::DeserializeOwned>(dir: &Path) -> Vec<T> {
    let Ok(entries) = fs::read_dir(dir) else { return Vec::new() };
    entries
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
        .filter_map(|e| serde_json::from_str(&fs::read_to_string(e.path()).ok()?).ok())
        .collect()
}

fn is_hex(id: &str) -> bool {
    !id.is_empty() && id.len() <= 32 && id.bytes().all(|b| b.is_ascii_hexdigit())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reportd::store::Store;

    fn queue(test: &str) -> (Queue, PathBuf) {
        let dir = std::env::temp_dir().join(format!("reportd-queue-{}-{test}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        Store::new(&dir).make().unwrap();
        (Queue::new(&dir), dir)
    }

    #[test]
    fn the_waits_grow_then_stay_at_an_hour() {
        let waits: Vec<u64> = (1..=6).map(wait_after).collect();
        assert_eq!(waits, [60, 300, 900, 3600, 3600, 3600]);
    }

    #[test]
    fn a_report_waits_until_sent_and_keeps_its_key_after() {
        let (queue, dir) = queue("sent");
        let now = 1_763_000_000;
        let mut entry = queue.add(b"text", 1000, "The disk", false, None, now).unwrap();
        assert_eq!((entry.id.len(), entry.key.len()), (16, 64));
        assert_eq!(queue.bytes(&entry).unwrap(), b"text");
        queue.failed(&mut entry, "no network", now).unwrap();
        let waiting = queue.waiting();
        assert_eq!(waiting.len(), 1);
        assert_eq!((waiting[0].tries, waiting[0].next), (1, now + 60));
        let sent = queue.sent(&entry, 7, now + 60).unwrap();
        assert!(queue.waiting().is_empty());
        assert_eq!(queue.get_sent(7).unwrap(), sent);
        assert_eq!(sent.key, entry.key);
        queue.forget_sent(7);
        assert!(queue.all_sent().is_empty());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_report_tried_for_thirty_days_is_dropped() {
        let (queue, dir) = queue("old");
        let now = 1_763_000_000;
        queue.add(b"old", 0, "old", false, None, now - 31 * DAY).unwrap();
        queue.add(b"new", 0, "new", false, None, now - DAY).unwrap();
        let dropped = queue.drop_old(now);
        assert_eq!(dropped.len(), 1);
        assert_eq!(dropped[0].title, "old");
        assert_eq!(queue.waiting().len(), 1);
        fs::remove_dir_all(dir).unwrap();
    }
}
