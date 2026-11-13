// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Which alerts are active and what has happened to them (ADR 0485): the
//! rules that turn a raise, an event, a clear or a set into recorded
//! changes, and the files that keep the result across restarts.
//!
//! An alert's identity is its source and its key. A change is recorded
//! only when the owner has something new to hear: a new alert, one that
//! went from warning to critical, one that cleared, or an event. A raise
//! of an active alert that only changes its title or detail updates what
//! `alerts list` shows and tells nobody, so a long-standing alert is
//! never repeated.
//!
//! The state is plain data and every rule takes the time it happens at,
//! so the rules are tested without a clock. alertd keeps it under one
//! lock and writes the files from a copy after releasing the lock
//! ([`Store`]). When alertd keeps the wrong alerts, `alerts list` and
//! `alerts history` show what it holds and `/system/state/alerts` what it
//! saved.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use crate::alerts::protocol::{Active, Alert, Change, State as Kind};

/// Where alertd keeps what it must not lose.
pub const DIR: &str = "/system/state/alerts";

/// The most alerts active at once, from all sources together; a raise
/// past it is refused, so that programs in loops cannot fill the memory.
pub const ACTIVE_MAX: usize = 1000;

/// The most alerts one source may have active: a source in a loop fills
/// its own share and is refused there, and the other sources can still
/// raise.
pub const SOURCE_MAX: usize = 100;

/// The history is cut to its newest [`HISTORY_KEEP`] lines when it passes
/// [`HISTORY_MAX`], so it is not rewritten line by line.
const HISTORY_MAX: usize = 2000;
const HISTORY_KEEP: usize = 1000;

/// The active alerts, one file of JSON.
const ACTIVE_FILE: &str = "active.json";
/// The history, a change a line.
const HISTORY_FILE: &str = "history";
/// The deliveries still to make, one file of JSON.
pub const OUTBOX_FILE: &str = "outbox.json";

/// The active alerts and the history, in memory.
#[derive(Debug, Default)]
pub struct State {
    active: Vec<Active>,
    /// Oldest first.
    history: Vec<Change>,
    active_dirty: bool,
    history_dirty: bool,
}

impl State {
    /// Raises `alert` for `source` at `now`: the change to tell of, if
    /// there is one.
    ///
    /// # Errors
    ///
    /// If `source` has [`SOURCE_MAX`] alerts active, or all sources have
    /// [`ACTIVE_MAX`], and this one is not among them.
    pub fn raise(
        &mut self,
        source: &str,
        alert: Alert,
        now: u64,
    ) -> Result<Option<Change>, String> {
        let Some(at) = self.position(source, &alert.key) else {
            self.make_room(source, 1)?;
            self.active.push(Active {
                source: source.to_string(),
                alert: alert.clone(),
                since: now,
                changed: now,
            });
            self.active_dirty = true;
            return Ok(Some(self.record(source, Kind::Raised, alert, now)));
        };
        let held = &mut self.active[at];
        let rose = alert.severity > held.alert.severity;
        if held.alert != alert {
            held.alert = alert.clone();
            held.changed = now;
            self.active_dirty = true;
        }
        Ok(rose.then(|| self.record(source, Kind::Raised, alert, now)))
    }

    /// Records `alert` as an event of `source`: told once, never active.
    pub fn event(&mut self, source: &str, alert: Alert, now: u64) -> Change {
        self.record(source, Kind::Event, alert, now)
    }

    /// Records `alert` as the test of `source`.
    pub fn test(&mut self, source: &str, alert: Alert, now: u64) -> Change {
        self.record(source, Kind::Test, alert, now)
    }

    /// Clears `source`'s alert `key`: the change to tell of, or none when
    /// it was not active.
    pub fn clear(&mut self, source: &str, key: &str, now: u64) -> Option<Change> {
        let at = self.position(source, key)?;
        let gone = self.active.remove(at);
        self.active_dirty = true;
        Some(self.record(source, Kind::Cleared, gone.alert, now))
    }

    /// Makes `alerts` the whole of `source`'s active alerts: each is
    /// raised, and each active alert of `source` not among them is
    /// cleared. Other sources' alerts are left as they are.
    ///
    /// # Errors
    ///
    /// If it would leave `source` with more than [`SOURCE_MAX`] alerts
    /// active, or all sources with more than [`ACTIVE_MAX`]; nothing is
    /// changed then.
    pub fn set(
        &mut self,
        source: &str,
        alerts: Vec<Alert>,
        now: u64,
    ) -> Result<Vec<Change>, String> {
        let listed = |key: &str| alerts.iter().any(|alert| alert.key == key);
        let leaving = self.keys_of(source).iter().filter(|key| !listed(key)).count();
        let arriving = alerts.iter().filter(|alert| self.position(source, &alert.key).is_none());
        let arriving = arriving.count();
        let own = self.keys_of(source).len() - leaving + arriving;
        if own > SOURCE_MAX {
            return Err(too_many_from(source));
        }
        if self.active.len() - leaving + arriving > ACTIVE_MAX {
            return Err(too_many());
        }
        // The clears come first, so that what leaves makes room for what
        // arrives.
        let mut changes = Vec::new();
        for key in self.keys_of(source).into_iter().filter(|key| !listed(key)) {
            changes.extend(self.clear(source, &key, now));
        }
        for alert in alerts {
            changes.extend(self.raise(source, alert, now)?);
        }
        Ok(changes)
    }

    /// The active alerts, the critical first and then the oldest.
    pub fn active(&self) -> Vec<Active> {
        let mut alerts = self.active.clone();
        alerts.sort_by(|a, b| {
            b.alert
                .severity
                .cmp(&a.alert.severity)
                .then(a.since.cmp(&b.since))
                .then_with(|| (&a.source, &a.alert.key).cmp(&(&b.source, &b.alert.key)))
        });
        alerts
    }

    /// The newest `count` changes, oldest first.
    pub fn newest(&self, count: usize) -> &[Change] {
        &self.history[self.history.len().saturating_sub(count)..]
    }

    /// The text of the files that changed since the last call, as
    /// `(active, history)`.
    pub fn take_text(&mut self) -> (Option<String>, Option<String>) {
        let active = std::mem::take(&mut self.active_dirty).then(|| {
            // Plain data that serde_json can always write.
            serde_json::to_string(&self.active).unwrap_or_else(|_| "[]".into())
        });
        let history = std::mem::take(&mut self.history_dirty).then(|| {
            let mut text = String::new();
            for change in &self.history {
                if let Ok(line) = serde_json::to_string(change) {
                    text.push_str(&line);
                    text.push('\n');
                }
            }
            text
        });
        (active, history)
    }

    /// The state saved in `dir`, and what could not be read of it, a
    /// line each, for the log. A file that is missing is a fresh start.
    pub fn load(dir: &Path) -> (State, Vec<String>) {
        let mut problems = Vec::new();
        let mut state = State::default();
        match fs::read_to_string(dir.join(ACTIVE_FILE)) {
            Ok(text) => match serde_json::from_str(&text) {
                Ok(active) => state.active = active,
                Err(err) => problems.push(format!(
                    "{}: {err}; no alert is active until the sources raise them again",
                    dir.join(ACTIVE_FILE).display()
                )),
            },
            Err(err) if err.kind() == io::ErrorKind::NotFound => {}
            Err(err) => problems.push(unreadable(&dir.join(ACTIVE_FILE), &err)),
        }
        match fs::read_to_string(dir.join(HISTORY_FILE)) {
            Ok(text) => {
                let lines = text.lines().filter(|line| !line.is_empty());
                let (kept, bad): (Vec<_>, Vec<_>) =
                    lines.map(serde_json::from_str::<Change>).partition(Result::is_ok);
                state.history = kept.into_iter().flatten().collect();
                if !bad.is_empty() {
                    problems.push(format!(
                        "{}: {} lines are not changes and were left out",
                        dir.join(HISTORY_FILE).display(),
                        bad.len()
                    ));
                }
            }
            Err(err) if err.kind() == io::ErrorKind::NotFound => {}
            Err(err) => problems.push(unreadable(&dir.join(HISTORY_FILE), &err)),
        }
        (state, problems)
    }

    /// The index of `source`'s alert `key`.
    fn position(&self, source: &str, key: &str) -> Option<usize> {
        self.active.iter().position(|held| held.source == source && held.alert.key == key)
    }

    fn keys_of(&self, source: &str) -> Vec<String> {
        let own = self.active.iter().filter(|held| held.source == source);
        own.map(|held| held.alert.key.clone()).collect()
    }

    /// Whether `source` may have `more` alerts active besides those it has.
    fn make_room(&self, source: &str, more: usize) -> Result<(), String> {
        let own = self.active.iter().filter(|held| held.source == source).count();
        if own + more > SOURCE_MAX {
            Err(too_many_from(source))
        } else if self.active.len() + more > ACTIVE_MAX {
            Err(too_many())
        } else {
            Ok(())
        }
    }

    /// Appends a change to the history, cutting it when it is long.
    fn record(&mut self, source: &str, kind: Kind, alert: Alert, now: u64) -> Change {
        let change = Change { at: now, source: source.to_string(), state: kind, alert };
        self.history.push(change.clone());
        if self.history.len() > HISTORY_MAX {
            self.history.drain(..self.history.len() - HISTORY_KEEP);
        }
        self.history_dirty = true;
        change
    }
}

fn too_many_from(source: &str) -> String {
    format!(
        "{source} has {SOURCE_MAX} active alerts already, the most one source may have; clear \
         some, or find what raises so many"
    )
}

fn too_many() -> String {
    format!(
        "alertd holds {ACTIVE_MAX} active alerts already; clear some, or find what raises so many"
    )
}

fn unreadable(path: &Path, err: &io::Error) -> String {
    format!("{}: {}", path.display(), crate::util::strip_os_error(err))
}

/// What the files of the state need written: the text of each that
/// changed.
#[derive(Debug, Default)]
pub struct Snapshot {
    pub active: Option<String>,
    pub history: Option<String>,
    pub outbox: Option<String>,
}

impl Snapshot {
    /// Puts what `later` holds over what this holds.
    fn replace_with(&mut self, later: Snapshot) {
        for (mine, theirs) in [
            (&mut self.active, later.active),
            (&mut self.history, later.history),
            (&mut self.outbox, later.outbox),
        ] {
            if theirs.is_some() {
                *mine = theirs;
            }
        }
    }

    /// The files of this snapshot, by name.
    fn files(&mut self) -> [(&'static str, &mut Option<String>); 3] {
        [
            (ACTIVE_FILE, &mut self.active),
            (HISTORY_FILE, &mut self.history),
            (OUTBOX_FILE, &mut self.outbox),
        ]
    }
}

/// Writes the state's files.
///
/// A caller that changed the state takes a [`Snapshot`] under the state's
/// lock, calls [`Store::begin`] while it still holds that lock, releases
/// it, and then writes. Another caller cannot take its snapshot before
/// this one has the store, so snapshots reach the disk in the order they
/// were taken, and no lock of the state is held across a write.
#[derive(Debug)]
pub struct Store {
    dir: PathBuf,
    inner: Mutex<Waiting>,
}

/// What is still to be written, and whether the last write failed.
#[derive(Debug, Default)]
struct Waiting {
    snapshot: Snapshot,
    failing: bool,
}

impl Store {
    pub fn new(dir: &Path) -> Store {
        Store { dir: dir.to_path_buf(), inner: Mutex::new(Waiting::default()) }
    }

    /// Takes the store, to write a snapshot once the caller has released
    /// the state's lock.
    pub fn begin(&self) -> Writer<'_> {
        Writer {
            dir: &self.dir,
            waiting: self.inner.lock().unwrap_or_else(std::sync::PoisonError::into_inner),
        }
    }
}

/// The store, taken: see [`Store::begin`].
pub struct Writer<'a> {
    dir: &'a Path,
    waiting: std::sync::MutexGuard<'a, Waiting>,
}

impl Writer<'_> {
    /// Writes `snapshot` and whatever an earlier write could not. A
    /// failure is logged once, until a write works again: the state stays
    /// in memory and alertd goes on.
    pub fn write(mut self, snapshot: Snapshot) {
        let waiting = &mut *self.waiting;
        waiting.snapshot.replace_with(snapshot);
        let mut failed = None;
        for (name, text) in waiting.snapshot.files() {
            let Some(content) = text else { continue };
            match write_file(self.dir, name, content) {
                Ok(()) => *text = None,
                Err(err) => failed = Some(format!("{name}: {}", crate::util::strip_os_error(&err))),
            }
        }
        match (failed, waiting.failing) {
            (Some(why), false) => eprintln!(
                "<3>alertd: cannot save its state in {}: {why}; it keeps it in memory and tries \
                 again",
                self.dir.display()
            ),
            (None, true) => eprintln!("alertd: saves its state in {} again", self.dir.display()),
            _ => {}
        }
        waiting.failing = waiting.snapshot.files().iter().any(|(_, text)| text.is_some());
    }
}

/// Writes `text` as file `name` of `dir`: to a new file beside it, which
/// is synced and renamed over it, and then the folder is synced, so that a
/// power cut leaves the old file or the new, never part of one. The files
/// are root's alone: they hold what alerts say.
fn write_file(dir: &Path, name: &str, text: &str) -> io::Result<()> {
    fs::create_dir_all(dir)?;
    let path = dir.join(name);
    let new = dir.join(format!("{name}.new"));
    let mut file =
        OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&new)?;
    file.write_all(text.as_bytes())?;
    file.sync_all()?;
    fs::rename(&new, &path)?;
    File::open(dir)?.sync_all()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::alerts::protocol::Severity;

    fn alert(key: &str, severity: Severity, title: &str) -> Alert {
        Alert { key: key.into(), severity, title: title.into(), detail: String::new() }
    }

    fn warning(key: &str) -> Alert {
        alert(key, Severity::Warning, "a warning")
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("alertd-state-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn a_new_alert_becomes_active_and_is_recorded_as_raised() {
        let mut state = State::default();
        let change = state.raise("metricsd", warning("fs-full:/data"), 100).unwrap().unwrap();
        assert_eq!(
            (change.at, change.source.as_str(), change.state),
            (100, "metricsd", Kind::Raised)
        );
        let held = state.active();
        assert_eq!(held.len(), 1);
        assert_eq!((held[0].since, held[0].changed), (100, 100));
        assert_eq!(state.newest(10), [change]);
    }

    #[test]
    fn raising_an_active_alert_again_at_the_same_severity_updates_it_and_records_nothing() {
        let mut state = State::default();
        state.raise("metricsd", alert("k", Severity::Warning, "93% full"), 100).unwrap();
        let again = alert("k", Severity::Warning, "94% full");
        assert_eq!(state.raise("metricsd", again, 160).unwrap(), None);
        let held = state.active();
        assert_eq!(held[0].alert.title, "94% full");
        assert_eq!((held[0].since, held[0].changed), (100, 160));
        assert_eq!(state.newest(10).len(), 1);
        // The same alert again changes nothing at all.
        state.raise("metricsd", alert("k", Severity::Warning, "94% full"), 220).unwrap();
        assert_eq!(state.active()[0].changed, 160);
    }

    #[test]
    fn going_from_warning_to_critical_is_raised_again_and_going_back_is_not() {
        let mut state = State::default();
        state.raise("metricsd", alert("k", Severity::Warning, "93% full"), 100).unwrap();
        let worse = state.raise("metricsd", alert("k", Severity::Critical, "97% full"), 200);
        let change = worse.unwrap().unwrap();
        assert_eq!(
            (change.state, change.alert.severity, change.at),
            (Kind::Raised, Severity::Critical, 200)
        );
        let better = state.raise("metricsd", alert("k", Severity::Warning, "95% full"), 300);
        assert_eq!(better.unwrap(), None);
        let held = &state.active()[0];
        assert_eq!(
            (held.alert.severity, held.alert.title.as_str()),
            (Severity::Warning, "95% full")
        );
        assert_eq!((held.since, held.changed), (100, 300));
        // Warning again after going back down is not "higher" than what it holds.
        assert_eq!(state.newest(10).len(), 2);
    }

    #[test]
    fn a_clear_removes_the_alert_and_says_what_it_was() {
        let mut state = State::default();
        state.raise("backup", alert("backup-failed:a", Severity::Warning, "failed"), 100).unwrap();
        let change = state.clear("backup", "backup-failed:a", 160).unwrap();
        assert_eq!(
            (change.state, change.alert.title.as_str(), change.at),
            (Kind::Cleared, "failed", 160)
        );
        assert!(state.active().is_empty());
        // Clearing what is not active is fine, and says nothing.
        assert_eq!(state.clear("backup", "backup-failed:a", 200), None);
        assert_eq!(state.newest(10).len(), 2);
    }

    #[test]
    fn an_event_is_recorded_and_never_active() {
        let mut state = State::default();
        let change = state.event(
            "sysupdate",
            alert("update-fell-back", Severity::Critical, "fell back"),
            50,
        );
        assert_eq!((change.state, change.source.as_str()), (Kind::Event, "sysupdate"));
        assert!(state.active().is_empty());
        assert_eq!(state.newest(10), [change]);
    }

    #[test]
    fn two_sources_may_use_the_same_key() {
        let mut state = State::default();
        state.raise("one", warning("k"), 1).unwrap();
        state.raise("two", warning("k"), 2).unwrap();
        assert_eq!(state.active().len(), 2);
        assert!(state.clear("one", "k", 3).is_some());
        assert_eq!(state.active()[0].source, "two");
    }

    #[test]
    fn a_set_raises_what_is_listed_and_clears_only_the_senders_others() {
        let mut state = State::default();
        state.raise("metricsd", warning("a"), 1).unwrap();
        state.raise("metricsd", warning("b"), 2).unwrap();
        state.raise("backup", warning("a"), 3).unwrap();
        let changes = state
            .set("metricsd", vec![warning("b"), alert("c", Severity::Critical, "new")], 10)
            .unwrap();
        let told: Vec<(&str, Kind)> =
            changes.iter().map(|c| (c.alert.key.as_str(), c.state)).collect();
        // b was active already, so only c is new; a is gone.
        assert_eq!(told, [("a", Kind::Cleared), ("c", Kind::Raised)]);
        let mut keys: Vec<String> =
            state.active().iter().map(|a| format!("{}:{}", a.source, a.alert.key)).collect();
        keys.sort();
        assert_eq!(keys, ["backup:a", "metricsd:b", "metricsd:c"]);
    }

    #[test]
    fn an_empty_set_clears_everything_the_sender_has() {
        let mut state = State::default();
        state.raise("metricsd", warning("a"), 1).unwrap();
        state.raise("backup", warning("a"), 2).unwrap();
        let changes = state.set("metricsd", Vec::new(), 3).unwrap();
        assert_eq!(changes.len(), 1);
        assert_eq!(state.active()[0].source, "backup");
        assert_eq!(state.set("metricsd", Vec::new(), 4).unwrap(), []);
    }

    #[test]
    fn the_active_alerts_list_the_critical_first_and_then_the_oldest() {
        let mut state = State::default();
        state.raise("s", alert("old-warning", Severity::Warning, "w"), 10).unwrap();
        state.raise("s", alert("new-critical", Severity::Critical, "c"), 30).unwrap();
        state.raise("s", alert("old-critical", Severity::Critical, "c"), 20).unwrap();
        let keys: Vec<String> = state.active().into_iter().map(|a| a.alert.key).collect();
        assert_eq!(keys, ["old-critical", "new-critical", "old-warning"]);
    }

    /// Raises `count` alerts for `source`, named `<source>-<n>`.
    fn fill(state: &mut State, source: &str, count: usize) {
        for n in 0..count {
            state.raise(source, warning(&format!("{source}-{n}")), 1).unwrap();
        }
    }

    #[test]
    fn one_source_may_hold_no_more_than_its_share() {
        let mut state = State::default();
        fill(&mut state, "loop", SOURCE_MAX);
        let refused = state.raise("loop", warning("one-too-many"), 2).unwrap_err();
        assert!(refused.starts_with("loop has 100 active alerts already"), "{refused}");
        // One that is held can still be raised again.
        assert!(state.raise("loop", warning("loop-5"), 2).is_ok());
        // A set that would pass it changes nothing.
        let more: Vec<Alert> = (0..=SOURCE_MAX).map(|n| warning(&format!("j{n}"))).collect();
        assert!(state.set("loop", more, 3).unwrap_err().starts_with("loop has 100"));
        assert_eq!(state.active().len(), SOURCE_MAX);
        // One that makes room for what it brings is taken.
        assert!(state.set("loop", vec![warning("fresh")], 4).is_ok());
        assert_eq!(state.active().len(), 1);
    }

    #[test]
    fn a_second_source_can_still_raise_when_the_first_has_its_share() {
        let mut state = State::default();
        fill(&mut state, "loop", SOURCE_MAX);
        assert!(state.raise("loop", warning("more"), 2).is_err());
        assert!(state.raise("backup", warning("backup-overdue:/mnt"), 2).is_ok());
        assert!(state.set("diskhealth", vec![warning("smart:x")], 3).is_ok());
        assert_eq!(state.active().len(), SOURCE_MAX + 2);
    }

    #[test]
    fn no_more_than_the_most_are_held_by_all_sources() {
        let mut state = State::default();
        for source in 0..ACTIVE_MAX / SOURCE_MAX {
            fill(&mut state, &format!("source{source}"), SOURCE_MAX);
        }
        assert_eq!(state.active().len(), ACTIVE_MAX);
        let refused = state.raise("latecomer", warning("one-too-many"), 2).unwrap_err();
        assert!(refused.starts_with("alertd holds 1000 active alerts already"), "{refused}");
        // One that is held can still be raised.
        assert!(state.raise("source0", warning("source0-5"), 2).is_ok());
        // A set that would pass the most changes nothing.
        let more: Vec<Alert> = (0..3).map(|n| warning(&format!("j{n}"))).collect();
        assert!(state.set("latecomer", more, 3).is_err());
        assert_eq!(state.active().len(), ACTIVE_MAX);
        // One that makes room for what it brings is taken.
        assert!(state.set("source0", vec![warning("fresh")], 4).is_ok());
        assert_eq!(state.active().len(), ACTIVE_MAX - SOURCE_MAX + 1);
        assert!(state.raise("latecomer", warning("fits-now"), 5).is_ok());
    }

    #[test]
    fn the_history_is_cut_to_the_newest_thousand_when_it_passes_two_thousand() {
        let mut state = State::default();
        for n in 0..HISTORY_MAX {
            state.event("s", warning(&format!("k{n}")), n as u64);
        }
        assert_eq!(state.newest(usize::MAX).len(), HISTORY_MAX);
        state.event("s", warning("last"), 5000);
        let kept = state.newest(usize::MAX);
        assert_eq!(kept.len(), HISTORY_KEEP);
        assert_eq!(kept.last().unwrap().alert.key, "last");
        assert_eq!(kept[0].alert.key, format!("k{}", HISTORY_MAX + 1 - HISTORY_KEEP));
        assert_eq!(state.newest(2).len(), 2);
        assert_eq!(state.newest(0).len(), 0);
    }

    #[test]
    fn what_is_saved_is_loaded_again() {
        let dir = scratch("roundtrip");
        let mut state = State::default();
        state.raise("metricsd", alert("a", Severity::Critical, "full"), 10).unwrap();
        state.event("dumps", warning("panic"), 20);
        let (active, history) = state.take_text();
        Store::new(&dir).begin().write(Snapshot { active, history, outbox: None });
        let (loaded, problems) = State::load(&dir);
        assert_eq!(problems, Vec::<String>::new());
        assert_eq!(loaded.active(), state.active());
        assert_eq!(loaded.newest(10), state.newest(10));
        for name in [ACTIVE_FILE, HISTORY_FILE] {
            let mode = fs::metadata(dir.join(name)).unwrap();
            assert_eq!(std::os::unix::fs::PermissionsExt::mode(&mode.permissions()) & 0o777, 0o600);
        }
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_file_is_written_only_when_it_changed() {
        let mut state = State::default();
        assert_eq!(state.take_text(), (None, None));
        state.event("s", warning("k"), 1);
        let (active, history) = state.take_text();
        assert!(active.is_none() && history.is_some());
        assert_eq!(state.take_text(), (None, None));
        state.raise("s", warning("k"), 2).unwrap();
        let (active, history) = state.take_text();
        assert!(active.is_some() && history.is_some());
    }

    #[test]
    fn a_fresh_start_loads_nothing_and_a_damaged_file_is_said() {
        let dir = scratch("damaged");
        let (state, problems) = State::load(&dir);
        assert!(state.active().is_empty() && problems.is_empty());
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(ACTIVE_FILE), "[{\"source\":").unwrap();
        let good = r#"{"at":5,"source":"s","state":"event","alert":{"key":"k","severity":"warning","title":"t"}}"#;
        fs::write(dir.join(HISTORY_FILE), format!("{good}\nnot a change\n{good}\n")).unwrap();
        let (state, problems) = State::load(&dir);
        assert!(state.active().is_empty());
        assert_eq!(state.newest(10).len(), 2);
        assert_eq!(problems.len(), 2, "{problems:?}");
        assert!(problems[0].contains("active.json") && problems[0].contains("raise them again"));
        assert!(problems[1].contains("1 lines are not changes"), "{problems:?}");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_write_that_fails_is_kept_and_made_when_the_folder_is_there() {
        let dir = scratch("failing");
        fs::create_dir_all(&dir).unwrap();
        // A file where the folder should be: every write fails.
        let blocked = dir.join("state");
        fs::write(&blocked, "in the way").unwrap();
        let store = Store::new(&blocked);
        let first = Snapshot { active: Some("[]".into()), ..Snapshot::default() };
        store.begin().write(first);
        assert!(store.inner.lock().unwrap().failing);
        // A later snapshot replaces the file's text, and the other is kept.
        let second = Snapshot { history: Some("h\n".into()), ..Snapshot::default() };
        store.begin().write(second);
        fs::remove_file(&blocked).unwrap();
        store.begin().write(Snapshot::default());
        assert!(!store.inner.lock().unwrap().failing);
        assert_eq!(fs::read_to_string(blocked.join(ACTIVE_FILE)).unwrap(), "[]");
        assert_eq!(fs::read_to_string(blocked.join(HISTORY_FILE)).unwrap(), "h\n");
        fs::remove_dir_all(&dir).unwrap();
    }
}
