// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Where each destination of logforward(8) is in the log, kept on the
//! disk so that a restart or a boot goes on from there (ADR 0482), and
//! when what was sent counts as delivered.
//!
//! logd appends each record to the day file of the record's own time, so
//! a record can reach a day file after a later one has begun: one stamped
//! a moment before midnight that arrives after it, or any record written
//! after the clock was set back, as when the clock read from the hardware
//! was ahead and the time server corrects it. A position is therefore an
//! offset in each day file, the byte after the last whole line sent from
//! it, and a day file it does not name is sent from its start.
//!
//! A position is saved in `/system/state/logforward/<name>.position`, a
//! first line `logforward position 1`, then `<day file> <offset>` for
//! each day file: written to a new file beside it, synced, renamed over
//! it, and the folder synced, so that a power cut leaves the old position
//! or the new, never part of one.
//!
//! Over TCP and TLS, a server's kernel taking the bytes is not the server
//! having them, and RFC 5425 has no acknowledgment. So a position counts
//! as delivered only once the connection has been seen open [`SETTLE`]
//! after the position was flushed. After a failure, sending starts again
//! from the last delivered position, and the server may get the last few
//! seconds twice, never a gap. Over UDP, which nothing acknowledges
//! either, a position counts as delivered once it is sent.

use std::collections::{BTreeMap, VecDeque};
use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use super::connection::Transport;
use crate::logd;

/// Where the positions are kept.
pub const DIRECTORY: &str = "/system/state/logforward";
/// How long a connection must stay open after a flush for what it
/// carried to count as delivered.
pub const SETTLE: Duration = Duration::from_secs(10);
/// The least time between two saves of a destination's position.
const SAVE_EVERY: Duration = Duration::from_secs(1);
/// How long flushes count together, as one that settles from the last of
/// them: so that however fast lines are sent, only about SETTLE /
/// TOGETHER flushes wait to settle.
const TOGETHER: Duration = Duration::from_secs(1);
/// The first line of a position file, which tells a whole one from any
/// other file.
const HEADER: &str = "logforward position 1";

/// Where a destination is in the log: for each day file, the byte after
/// the last whole line sent from it. A day file it does not name is sent
/// from its start.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Position {
    offsets: BTreeMap<String, u64>,
}

impl Position {
    /// The position at `offset` in day file `file`, and the start of every
    /// other.
    #[cfg(test)]
    pub fn of(file: &str, offset: u64) -> Position {
        Position { offsets: BTreeMap::from([(file.to_string(), offset)]) }
    }

    /// The byte of day file `file` the next line is read from.
    pub fn offset(&self, file: &str) -> u64 {
        self.offsets.get(file).copied().unwrap_or(0)
    }

    /// The next line of day file `file` is read from byte `offset`.
    pub fn set(&mut self, file: &str, offset: u64) {
        if let Some(at) = self.offsets.get_mut(file) {
            *at = offset;
        } else {
            self.offsets.insert(file.to_string(), offset);
        }
    }

    /// Forgets the day files not in `files`, which logtrim(8) removed:
    /// each with the offset it had.
    pub fn keep_only(&mut self, files: &[String]) -> Vec<(String, u64)> {
        let gone: Vec<(String, u64)> = self
            .offsets
            .iter()
            .filter(|(file, _)| !files.contains(file))
            .map(|(file, offset)| (file.clone(), *offset))
            .collect();
        for (file, _) in &gone {
            self.offsets.remove(file);
        }
        gone
    }

    /// The position of destination `name` saved in `dir`, if there is one
    /// and it is whole.
    pub fn load(dir: &Path, name: &str) -> Option<Position> {
        let text = fs::read_to_string(path(dir, name)).ok()?;
        let mut lines = text.strip_suffix('\n')?.split('\n');
        if lines.next()? != HEADER {
            return None;
        }
        let mut position = Position::default();
        for line in lines {
            let (file, offset) = line.split_once(' ')?;
            if !logd::is_day_file(file) || position.offsets.contains_key(file) {
                return None;
            }
            position.offsets.insert(file.to_string(), offset.parse().ok()?);
        }
        Some(position)
    }

    /// Saves it as destination `name`'s position in `dir`, which it makes
    /// if it is not there.
    ///
    /// # Errors
    ///
    /// The file system's, when the new file cannot be written, synced or
    /// put in place; the old position is then still there, whole.
    pub fn save(&self, dir: &Path, name: &str) -> io::Result<()> {
        fs::create_dir_all(dir)?;
        let path = path(dir, name);
        let new = path.with_extension("position.new");
        let mut text = format!("{HEADER}\n");
        for (file, offset) in &self.offsets {
            text.push_str(&format!("{file} {offset}\n"));
        }
        let mut file = File::create(&new)?;
        file.write_all(text.as_bytes())?;
        file.sync_all()?;
        fs::rename(&new, &path)?;
        File::open(dir)?.sync_all()
    }
}

fn path(dir: &Path, name: &str) -> PathBuf {
    dir.join(format!("{name}.position"))
}

/// What has been sent to a destination and what of it counts as
/// delivered.
#[derive(Debug)]
pub struct Progress {
    transport: Transport,
    /// The last position that counts as delivered.
    delivered: Position,
    /// Positions flushed and not yet delivered, oldest first.
    flushed: VecDeque<Flushed>,
    /// The position last given to save, and when.
    saved: Position,
    saved_at: Option<Instant>,
}

/// Flushes that count together: the position of the last, when the first
/// was made, and when the last was.
#[derive(Debug)]
struct Flushed {
    position: Position,
    first: Instant,
    last: Instant,
}

impl Progress {
    /// The progress of a destination on `transport` whose delivered
    /// position, as saved, is `start`.
    pub fn new(start: Position, transport: Transport) -> Progress {
        Progress {
            transport,
            delivered: start.clone(),
            flushed: VecDeque::new(),
            saved: start,
            saved_at: None,
        }
    }

    /// Everything up to `at` was flushed to the server at `now`.
    pub fn sent(&mut self, at: Position, now: Instant) {
        if self.transport == Transport::Udp {
            self.delivered = at;
            return;
        }
        match self.flushed.back_mut() {
            Some(flushed) if now.saturating_duration_since(flushed.first) < TOGETHER => {
                flushed.position = at;
                flushed.last = now;
            }
            _ => self.flushed.push_back(Flushed { position: at, first: now, last: now }),
        }
    }

    /// The connection was seen open at `now` (`Connection::open`): what
    /// was flushed [`SETTLE`] or more before counts as delivered.
    pub fn still_open(&mut self, now: Instant) {
        let settled = self
            .flushed
            .iter()
            .take_while(|flushed| now.saturating_duration_since(flushed.last) >= SETTLE)
            .count();
        if let Some(flushed) = self.flushed.drain(..settled).next_back() {
            self.delivered = flushed.position;
        }
    }

    /// The position to save now, if one is due: delivered since the last
    /// one given, and at most once a second. It is given once: the caller
    /// keeps it until a save succeeds.
    pub fn save_due(&mut self, now: Instant) -> Option<&Position> {
        let recent = self.saved_at.is_some_and(|at| now.saturating_duration_since(at) < SAVE_EVERY);
        if recent || self.delivered == self.saved {
            return None;
        }
        self.saved.clone_from(&self.delivered);
        self.saved_at = Some(now);
        Some(&self.delivered)
    }

    /// The last position that counts as delivered: the one to save when
    /// logforward stops.
    pub fn delivered(&self) -> &Position {
        &self.delivered
    }

    /// After a failure: forgets what was flushed and not delivered, and
    /// gives the delivered position, where sending starts again.
    pub fn rewind(&mut self) -> Position {
        self.flushed.clear();
        self.delivered.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(file: &str, offset: u64) -> Position {
        Position::of(file, offset)
    }

    /// A folder of its own for each test.
    fn folder(test: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("logforward-{test}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn a_saved_position_loads_back_and_replaces_the_last_whole() {
        let dir = folder("save");
        assert_eq!(Position::load(&dir, "siem"), None);
        at("2026-11-13.jsonl", 4096).save(&dir, "siem").unwrap();
        at("2026-11-14.jsonl", 17).save(&dir, "siem").unwrap();
        assert_eq!(Position::load(&dir, "siem"), Some(at("2026-11-14.jsonl", 17)));
        assert_eq!(
            fs::read_to_string(dir.join("siem.position")).unwrap(),
            "logforward position 1\n2026-11-14.jsonl 17\n"
        );
        assert!(!dir.join("siem.position.new").exists());
        assert_eq!(Position::load(&dir, "other"), None);
        // An offset in each day file, a line each, and none at all.
        let mut both = at("2026-11-13.jsonl", 4096);
        both.set("2026-11-14.jsonl", 17);
        both.save(&dir, "siem").unwrap();
        assert_eq!(
            fs::read_to_string(dir.join("siem.position")).unwrap(),
            "logforward position 1\n2026-11-13.jsonl 4096\n2026-11-14.jsonl 17\n"
        );
        assert_eq!(Position::load(&dir, "siem"), Some(both));
        Position::default().save(&dir, "siem").unwrap();
        assert_eq!(Position::load(&dir, "siem"), Some(Position::default()));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_position_is_an_offset_in_each_day_file_and_forgets_the_trimmed() {
        let mut position = at("2026-11-13.jsonl", 100);
        assert_eq!(
            (position.offset("2026-11-13.jsonl"), position.offset("2026-11-12.jsonl")),
            (100, 0)
        );
        position.set("2026-11-12.jsonl", 7);
        position.set("2026-11-13.jsonl", 150);
        assert_eq!(
            (position.offset("2026-11-13.jsonl"), position.offset("2026-11-12.jsonl")),
            (150, 7)
        );
        let gone = position.keep_only(&["2026-11-13.jsonl".to_string()]);
        assert_eq!(gone, [("2026-11-12.jsonl".to_string(), 7)]);
        assert_eq!(position, at("2026-11-13.jsonl", 150));
    }

    #[test]
    fn a_position_file_that_is_not_whole_is_no_position() {
        let dir = folder("partial");
        fs::create_dir_all(&dir).unwrap();
        for text in [
            "logforward position 1\n2026-11-13.jsonl 4096",
            "logforward position 1\n2026-11-13.jsonl\n",
            "logforward position 1\n2026-11-13.jsonl -1\n",
            "logforward position 1\n2026-11-13.jsonl 40x\n",
            "logforward position 1\n../passwd 4096\n",
            "logforward position 1\n2026-11-13.jsonl 1\n2026-11-13.jsonl 2\n",
            "2026-11-13.jsonl 4096\n",
            "logforward position 1",
            "",
        ] {
            fs::write(dir.join("siem.position"), text).unwrap();
            assert_eq!(Position::load(&dir, "siem"), None, "{text:?}");
        }
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn over_tcp_a_position_counts_once_the_connection_stays_open_ten_seconds() {
        let start = Instant::now();
        let mut progress = Progress::new(at("2026-11-13.jsonl", 0), Transport::Tls);
        progress.sent(at("2026-11-13.jsonl", 100), start);
        progress.still_open(start + Duration::from_secs(9));
        assert_eq!(progress.delivered(), &at("2026-11-13.jsonl", 0));
        progress.still_open(start + SETTLE);
        assert_eq!(progress.delivered(), &at("2026-11-13.jsonl", 100));
    }

    #[test]
    fn after_a_failure_sending_starts_again_from_the_last_delivered() {
        let start = Instant::now();
        let mut progress = Progress::new(at("2026-11-13.jsonl", 0), Transport::Tcp);
        progress.sent(at("2026-11-13.jsonl", 100), start);
        progress.still_open(start + SETTLE);
        progress.sent(at("2026-11-13.jsonl", 200), start + SETTLE);
        assert_eq!(progress.rewind(), at("2026-11-13.jsonl", 100));
        // What was flushed before the failure never counts afterwards.
        progress.still_open(start + SETTLE * 3);
        assert_eq!(progress.delivered(), &at("2026-11-13.jsonl", 100));
    }

    #[test]
    fn over_udp_a_position_counts_once_sent() {
        let mut progress = Progress::new(at("2026-11-13.jsonl", 0), Transport::Udp);
        progress.sent(at("2026-11-13.jsonl", 100), Instant::now());
        assert_eq!(progress.delivered(), &at("2026-11-13.jsonl", 100));
        assert_eq!(progress.rewind(), at("2026-11-13.jsonl", 100));
    }

    #[test]
    fn flushes_within_a_second_settle_from_the_last_of_them() {
        let start = Instant::now();
        let mut progress = Progress::new(at("2026-11-13.jsonl", 0), Transport::Tls);
        progress.sent(at("2026-11-13.jsonl", 100), start);
        progress.sent(at("2026-11-13.jsonl", 200), start + Duration::from_millis(500));
        progress.still_open(start + SETTLE);
        assert_eq!(progress.delivered(), &at("2026-11-13.jsonl", 0));
        progress.still_open(start + SETTLE + Duration::from_millis(500));
        assert_eq!(progress.delivered(), &at("2026-11-13.jsonl", 200));
    }

    #[test]
    fn sending_without_a_pause_still_delivers_and_keeps_few_flushes() {
        let start = Instant::now();
        let mut progress = Progress::new(at("2026-11-13.jsonl", 0), Transport::Tls);
        for tenth in 1..=300u64 {
            let now = start + Duration::from_millis(tenth * 100);
            progress.sent(at("2026-11-13.jsonl", tenth), now);
            progress.still_open(now);
            assert!(progress.flushed.len() <= 12, "{} flushes wait", progress.flushed.len());
        }
        // At 30 s, what was flushed by 20 s has settled.
        let delivered = progress.delivered().offset("2026-11-13.jsonl");
        assert!((190..=200).contains(&delivered), "{delivered}");
    }

    #[test]
    fn a_position_is_due_to_save_when_it_moves_and_at_most_once_a_second() {
        let start = Instant::now();
        let mut progress = Progress::new(at("2026-11-13.jsonl", 0), Transport::Udp);
        assert_eq!(progress.save_due(start), None);
        progress.sent(at("2026-11-13.jsonl", 100), start);
        assert_eq!(progress.save_due(start), Some(&at("2026-11-13.jsonl", 100)));
        progress.sent(at("2026-11-14.jsonl", 5), start + Duration::from_millis(500));
        assert_eq!(progress.save_due(start + Duration::from_millis(500)), None);
        assert_eq!(progress.save_due(start + SAVE_EVERY), Some(&at("2026-11-14.jsonl", 5)));
        assert_eq!(progress.save_due(start + SAVE_EVERY * 3), None);
    }
}
