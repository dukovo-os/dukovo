// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! What MCP clients did (ADR 0690), for the page's "What they did":
//! each change a client asked for, with what became of it, and its
//! reads, which are many and so are joined: a client's reads that
//! follow one another are one entry, with their count and the areas
//! they read.
//!
//! The entries are in admind's memory and, as JSON lines, in
//! `/system/state/admind/clients.jsonl`, so that they outlast a
//! restart. A line is added when something changes, and a later line
//! for the same request, or for the same run of reads, stands for the
//! earlier ones. The file is written again with the newest 1000 entries
//! once it has grown past 2000 lines. An entry names the action and
//! never its arguments: what a change did is in the journal, with the
//! administrator who approved it.
//!
//! This is what a person reads to see what a client has been doing. It
//! is no audit log: authd's has each client's reads as an
//! administrator, and admind's log a line for each call.

use std::collections::VecDeque;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::super::log;
use super::requests::Request;
use crate::util;

/// Where the entries are kept.
pub const FILE: &str = "/system/state/admind/clients.jsonl";

/// The most entries kept, in memory and in the file once it is written
/// again.
const KEPT: usize = 1000;
/// The lines past which the file is written again.
const REWRITE_AT: usize = 2000;
/// The seconds without a read after which a client's next read begins a
/// new entry.
const RUN_GAP: i64 = 3600;

/// What an entry is of.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum What {
    /// A run of reads.
    Read {
        /// The Unix time of the first, which with the client names the
        /// run.
        since: i64,
        /// How many.
        count: u32,
        /// The areas they read, in the order first read.
        areas: Vec<String>,
    },
    /// A change asked for.
    Ask {
        /// The request's number.
        request: u64,
        /// The action's ID.
        action: String,
        /// What was asked for: the action's title with what it is done
        /// to, as the request's task is named.
        title: String,
        /// Where the request stands: a [`State::word`](super::requests::State::word).
        state: String,
        /// The administrator who approved or refused it.
        #[serde(default, skip_serializing_if = "String::is_empty")]
        by: String,
        /// The task the change runs as, when it is long.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        task: Option<i64>,
        /// The Unix time the request was answered or lapsed.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        ended: Option<i64>,
    },
}

/// One entry.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    /// The Unix time: of a request, when it was asked; of a run of
    /// reads, its last.
    pub at: i64,
    /// The administrator whose client it was.
    pub owner: String,
    /// The client's name.
    pub client: String,
    /// What it did.
    #[serde(flatten)]
    pub what: What,
}

impl Entry {
    /// Whether `other` is a later word on the same thing: the same
    /// request, or the same client's same run of reads.
    fn same(&self, other: &Entry) -> bool {
        match (&self.what, &other.what) {
            (What::Ask { request, .. }, What::Ask { request: theirs, .. }) => request == theirs,
            (What::Read { since, .. }, What::Read { since: theirs, .. }) => {
                since == theirs && self.owner == other.owner && self.client == other.client
            }
            _ => false,
        }
    }
}

/// What the clients did.
#[derive(Default)]
pub struct History {
    /// The entries, in the order each began.
    entries: VecDeque<Entry>,
    /// The runs of reads that have grown since they were last written,
    /// by owner, client and beginning.
    grown: Vec<(String, String, i64)>,
    /// The file, when there is one: none in a test, and none when it
    /// could not be read.
    file: Option<PathBuf>,
    /// The lines the file holds.
    lines: usize,
}

impl History {
    /// Reads the entries kept in `file`, which it then keeps up. A
    /// request that waited when admind stopped has lapsed. Lines that do
    /// not read are left out, and said in the log.
    ///
    /// # Errors
    ///
    /// When there is a file and it cannot be read: the history then
    /// stays in memory, and the file as it is.
    pub fn load(&mut self, file: &Path) -> io::Result<()> {
        let text = match fs::read_to_string(file) {
            Ok(text) => text,
            Err(err) if err.kind() == io::ErrorKind::NotFound => String::new(),
            Err(err) => return Err(err),
        };
        let (mut bad, mut lines) = (0_usize, 0_usize);
        for line in text.lines().filter(|line| !line.trim().is_empty()) {
            lines += 1;
            match serde_json::from_str::<Entry>(line) {
                Ok(entry) => self.put(entry),
                Err(_) => bad += 1,
            }
        }
        if bad > 0 {
            log(4, &format!("{}: {bad} lines did not read, and are left out", file.display()));
        }
        for entry in &mut self.entries {
            if let What::Ask { state, .. } = &mut entry.what
                && state == "waiting"
            {
                *state = "lapsed".into();
            }
        }
        self.file = Some(file.to_path_buf());
        self.lines = lines;
        Ok(())
    }

    /// The number of the last request there is an entry of.
    pub fn last_request(&self) -> u64 {
        let numbers = self.entries.iter().filter_map(|entry| match entry.what {
            What::Ask { request, .. } => Some(request),
            What::Read { .. } => None,
        });
        numbers.max().unwrap_or(0)
    }

    /// The newest `limit` entries, the newest first.
    pub fn newest(&self, limit: usize) -> Vec<&Entry> {
        let mut entries: Vec<&Entry> = self.entries.iter().collect();
        entries.sort_by_key(|entry| std::cmp::Reverse(entry.at));
        entries.truncate(limit);
        entries
    }

    /// The entry of the request `id`, if it is `client`'s of `owner`.
    pub fn request(&self, id: u64, owner: &str, client: &str) -> Option<&Entry> {
        self.entries.iter().find(|entry| {
            matches!(entry.what, What::Ask { request, .. } if request == id)
                && entry.owner == owner
                && entry.client == client
        })
    }

    /// Puts `entry` in the place of the one it is a later word on, or
    /// after the others, dropping the oldest past [`KEPT`].
    fn put(&mut self, entry: Entry) {
        match self.entries.iter_mut().find(|kept| kept.same(&entry)) {
            Some(kept) => *kept = entry,
            None => {
                self.entries.push_back(entry);
                if self.entries.len() > KEPT {
                    self.entries.pop_front();
                }
            }
        }
    }

    /// Notes that `client` of `owner` read something of `area` at the
    /// Unix time `at`. It joins the client's run of reads when that is
    /// the last thing the client did and no more than an hour ago, and
    /// begins a run when not. Written with the next [`History::flush`].
    pub fn read(&mut self, owner: &str, client: &str, area: &str, at: i64) {
        let last = self
            .entries
            .iter_mut()
            .rev()
            .find(|entry| entry.owner == owner && entry.client == client);
        if let Some(entry) = last
            && at.saturating_sub(entry.at) < RUN_GAP
            && let What::Read { since, count, areas } = &mut entry.what
        {
            *count = count.saturating_add(1);
            if !areas.iter().any(|known| known == area) {
                areas.push(area.into());
            }
            entry.at = entry.at.max(at);
            let run = (owner.to_string(), client.to_string(), *since);
            if !self.grown.contains(&run) {
                self.grown.push(run);
            }
            return;
        }
        // Two runs of one client never begin in the same second: the
        // first would have been joined.
        self.grown.push((owner.into(), client.into(), at));
        self.put(Entry {
            at,
            owner: owner.into(),
            client: client.into(),
            what: What::Read { since: at, count: 1, areas: vec![area.into()] },
        });
    }

    /// Notes the request `request` as it stands at the Unix time `at`:
    /// when it is filed, and each time its state changes.
    pub fn request_is(&mut self, request: &Request, at: i64) {
        use super::requests::{Outcome, State};
        let (by, task) = match &request.state {
            State::Approved { by, outcome: Outcome::Task(task) } => (by.clone(), Some(*task)),
            State::Approved { by, .. } | State::Refused { by } => (by.clone(), None),
            State::Waiting | State::Lapsed { .. } => (String::new(), None),
        };
        let entry = Entry {
            at: request.asked,
            owner: request.owner.clone(),
            client: request.client.clone(),
            what: What::Ask {
                request: request.id,
                action: request.action.id().into(),
                title: request.name.clone(),
                state: request.state.word().into(),
                by,
                task,
                ended: (request.state != State::Waiting).then_some(at),
            },
        };
        self.append(&[&entry]);
        self.put(entry);
    }

    /// Writes the runs of reads that have grown since the last time, and
    /// writes the file again when it has grown long.
    pub fn flush(&mut self) {
        let grown = std::mem::take(&mut self.grown);
        let runs: Vec<&Entry> = self
            .entries
            .iter()
            .filter(|entry| match entry.what {
                What::Read { since, .. } => grown.iter().any(|(owner, client, began)| {
                    *began == since && *owner == entry.owner && *client == entry.client
                }),
                What::Ask { .. } => false,
            })
            .collect();
        let lines = Self::lines_of(&runs);
        drop(runs);
        self.write(&lines);
        if self.lines > REWRITE_AT {
            self.rewrite();
        }
    }

    /// Adds `entries` to the file.
    fn append(&mut self, entries: &[&Entry]) {
        let lines = Self::lines_of(entries);
        self.write(&lines);
    }

    /// `entries` as the file's lines.
    fn lines_of(entries: &[&Entry]) -> Vec<String> {
        entries.iter().filter_map(|entry| serde_json::to_string(entry).ok()).collect()
    }

    /// Adds `lines` to the file, if there is one. A file that cannot be
    /// written is said in the log, and the history goes on in memory.
    fn write(&mut self, lines: &[String]) {
        let Some(file) = &self.file else { return };
        if lines.is_empty() {
            return;
        }
        let mut text = lines.join("\n");
        text.push('\n');
        let written = OpenOptions::new()
            .append(true)
            .create(true)
            .mode(0o600)
            .open(file)
            .and_then(|mut open| open.write_all(text.as_bytes()));
        match written {
            Ok(()) => self.lines += lines.len(),
            Err(err) => {
                log(4, &format!("cannot write {}: {}", file.display(), util::strip_os_error(&err)));
            }
        }
    }

    /// Writes the file again with the entries kept, beside it first and
    /// then in its place, so that a stop half way leaves the old one.
    fn rewrite(&mut self) {
        let Some(file) = &self.file else { return };
        let entries: Vec<&Entry> = self.entries.iter().collect();
        let mut text = Self::lines_of(&entries).join("\n");
        text.push('\n');
        let beside = file.with_extension("jsonl.new");
        let written = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&beside)
            .and_then(|mut open| {
                open.write_all(text.as_bytes())?;
                open.sync_all()
            })
            .and_then(|()| fs::rename(&beside, file));
        match written {
            Ok(()) => self.lines = entries.len(),
            Err(err) => {
                log(4, &format!("cannot write {}: {}", file.display(), util::strip_os_error(&err)));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use serde_json::json;

    use super::super::requests::{Asking, Outcome, Requests};
    use super::*;
    use crate::admind::actions::tests::LAB;
    use crate::cert::fixtures::Scratch;

    fn read_run(entry: &Entry) -> (i64, u32, Vec<&str>) {
        match &entry.what {
            What::Read { since, count, areas } => {
                (*since, *count, areas.iter().map(String::as_str).collect())
            }
            What::Ask { .. } => panic!("not a run of reads: {entry:?}"),
        }
    }

    fn state_of(entry: &Entry) -> &str {
        match &entry.what {
            What::Ask { state, .. } => state,
            What::Read { .. } => panic!("not a request: {entry:?}"),
        }
    }

    /// A request of chase's client `helper`, filed in `requests`.
    fn ask(requests: &mut Requests, name: &str) -> u64 {
        let asking = Asking {
            owner: "chase",
            client: "helper",
            action: LAB[0],
            name: format!("{}: {name}", LAB[0].title()),
            args: json!({ "name": name }),
            reason: "why",
        };
        requests.ask(asking, Instant::now(), 1_800_000_000).unwrap().0
    }

    #[test]
    fn reads_that_follow_one_another_are_one_entry() {
        let mut history = History::default();
        history.read("chase", "helper", "services", 100);
        history.read("chase", "helper", "logs", 130);
        history.read("chase", "helper", "services", 160);
        let newest = history.newest(10);
        assert_eq!(newest.len(), 1);
        assert_eq!(newest[0].at, 160);
        assert_eq!(read_run(newest[0]), (100, 3, vec!["services", "logs"]));
        // Another client's reads are its own, and so are another owner's
        // client of the same name.
        history.read("chase", "other", "storage", 170);
        history.read("alice", "helper", "storage", 180);
        assert_eq!(history.newest(10).len(), 3);
        // After an hour without a read, a new run begins.
        history.read("chase", "helper", "services", 160 + RUN_GAP);
        let newest = history.newest(10);
        assert_eq!(newest.len(), 4);
        assert_eq!(read_run(newest[0]), (160 + RUN_GAP, 1, vec!["services"]));
    }

    #[test]
    fn a_request_ends_a_run_of_reads_and_is_one_entry_whatever_becomes_of_it() {
        let mut history = History::default();
        let mut requests = Requests::default();
        history.read("chase", "helper", "services", 1_800_000_000 - 5);
        let id = ask(&mut requests, "nginx");
        history.request_is(requests.get(id).unwrap(), 1_800_000_000);
        history.read("chase", "helper", "services", 1_800_000_005);
        assert_eq!(history.newest(10).len(), 3);
        assert_eq!(state_of(history.request(id, "chase", "helper").unwrap()), "waiting");

        let approved = requests.approve(id, "alice", Instant::now()).unwrap();
        history.request_is(&approved, 1_800_000_060);
        requests.finish(id, Outcome::Task(7), Instant::now());
        history.request_is(requests.get(id).unwrap(), 1_800_000_061);
        assert_eq!(history.newest(10).len(), 3);
        let entry = history.request(id, "chase", "helper").unwrap();
        assert_eq!(
            serde_json::to_value(entry).unwrap(),
            json!({
                "at": 1_800_000_000, "owner": "chase", "client": "helper", "kind": "ask",
                "request": id, "action": "lab.restart", "title": "Restart a lab service: nginx",
                "state": "task", "by": "alice", "task": 7, "ended": 1_800_000_061,
            })
        );
        // Only its own client reads a request's entry.
        assert!(history.request(id, "chase", "other").is_none());
        assert!(history.request(id, "alice", "helper").is_none());
        assert_eq!(history.last_request(), id);
    }

    #[test]
    fn the_file_keeps_the_entries_over_a_restart() {
        let folder = Scratch::new("mcp-history");
        let file = folder.0.join("clients.jsonl");
        let mut requests = Requests::default();
        let (waits, answered);
        {
            let mut history = History::default();
            history.load(&file).unwrap();
            history.read("chase", "helper", "services", 100);
            history.read("chase", "helper", "logs", 110);
            // A run is written when it is flushed, and its later line
            // stands for the earlier.
            history.flush();
            history.read("chase", "helper", "logs", 120);
            history.flush();
            waits = ask(&mut requests, "nginx");
            history.request_is(requests.get(waits).unwrap(), 200);
            answered = ask(&mut requests, "postgres");
            history.request_is(requests.get(answered).unwrap(), 210);
            let refused = requests.refuse(answered, "alice", Instant::now()).unwrap();
            history.request_is(&refused, 220);
            // Nothing new: nothing is written.
            history.flush();
        }
        let text = fs::read_to_string(&file).unwrap();
        assert_eq!(text.lines().count(), 5, "{text}");
        assert!(!text.contains("why"), "a reason is not kept: {text}");
        std::fs::write(&file, format!("{text}not json\n\n{{\"at\": 1}}\n")).unwrap();

        let mut history = History::default();
        history.load(&file).unwrap();
        let newest = history.newest(10);
        assert_eq!(newest.len(), 3);
        let run = newest.iter().find(|entry| matches!(entry.what, What::Read { .. })).unwrap();
        assert_eq!(read_run(run), (100, 3, vec!["services", "logs"]));
        // The one that waited when admind stopped has lapsed.
        assert_eq!(state_of(history.request(waits, "chase", "helper").unwrap()), "lapsed");
        assert_eq!(state_of(history.request(answered, "chase", "helper").unwrap()), "refused");
        assert_eq!(history.last_request(), answered);
    }

    #[test]
    fn a_file_that_has_grown_long_is_written_again_with_what_is_kept() {
        let folder = Scratch::new("mcp-history-long");
        let file = folder.0.join("clients.jsonl");
        let mut history = History::default();
        history.load(&file).unwrap();
        // Each read is a run of its own, an hour after the last, and a
        // line of its own.
        for n in 0..=REWRITE_AT {
            history.read("chase", "helper", "services", i64::try_from(n).unwrap() * RUN_GAP);
            history.flush();
        }
        let text = fs::read_to_string(&file).unwrap();
        assert_eq!(text.lines().count(), KEPT);
        assert!(!file.with_extension("jsonl.new").exists());
        // The newest are the ones kept.
        let mut kept = History::default();
        kept.load(&file).unwrap();
        assert_eq!(kept.newest(1)[0].at, i64::try_from(REWRITE_AT).unwrap() * RUN_GAP);
        assert_eq!(kept.entries.len(), KEPT);
        // And the file is only the owner's to read.
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(fs::metadata(&file).unwrap().permissions().mode() & 0o777, 0o600);
    }

    #[test]
    fn with_no_file_the_history_is_in_memory() {
        let mut history = History::default();
        history.read("chase", "helper", "services", 100);
        history.flush();
        assert_eq!(history.newest(10).len(), 1);
        assert_eq!(history.last_request(), 0);
        assert!(History::default().load(Path::new("/proc/self/mem/none")).is_err());
    }
}
