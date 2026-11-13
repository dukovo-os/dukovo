// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Settings tried with an undo (ADR 0472): `try` keeps a copy of the
//! files it names before they change, and Spark puts the copies back and
//! restarts what reads them unless `keep` comes in time. A service the
//! trial restarts that fails to start with the change, or fails its
//! health check, undoes it at once; a start of the machine that follows
//! one never kept undoes it before any service starts. So a change that
//! cuts a remote administrator off comes undone by itself, as Junos's
//! `commit confirmed` does for a router.
//!
//! Each trial is a folder in [`TRIALS`] with the copies, named by their
//! number, and the [`Record`] in `trial`, written last: a folder without
//! one is a `try` that never answered, whose files nothing changed.

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use libradix::abi::capability::Rights;
use libradix::abi::directory::{FileKind, OpenFlags};
use libradix::abi::errors::File as FileError;
use radix_rt::fs::{Dir, Stat};
use radix_rt::println;
use spark_api::show;
use spark_api::trial::{
    FILE_MAX, Kept, Named, Record, Request, SETTINGS, Saved, TRIALS_MAX, Why, check_id,
};

use super::watch::tell;
use super::{MS, SECOND, State, Supervisor, now};
use crate::services;

/// Where the trials are kept, beneath the root.
pub const TRIALS: &str = "system/state/spark/trials";

/// The name of a trial's record in its folder.
const RECORD: &str = "trial";

/// The rights to read and change a folder.
const READ_WRITE: Rights = Rights::READ.union(Rights::WRITE);

/// A trial that runs.
pub(super) struct Trial {
    /// Its name.
    pub(super) id: String,
    /// What it keeps and what an undo restarts.
    record: Record,
    /// The user it was begun for.
    user: u32,
    /// When it is undone unless it is kept.
    pub(super) ends: u64,
    /// Each service it restarts or reloads, by index, with its failed
    /// starts when it began: one more is a start that failed with the
    /// change.
    failed_before: Vec<(usize, u64)>,
}

impl Supervisor {
    /// The answer to `try ID SECONDS file=PATH... restart=NAME...
    /// reload=NAME...` from process `peer`, acting for `user`.
    pub(super) fn answer_try(&mut self, words: &[&str], (peer, user): (u64, u32)) -> String {
        match self.begin_trial(words, user) {
            Ok(trial) => {
                let id = trial.id.clone();
                let seconds = trial.record.seconds;
                let paths: Vec<&str> = trial.record.files.iter().map(|k| k.path.as_str()).collect();
                audit!(
                    "began the trial {id} of {} for {seconds} s, by process {peer}",
                    paths.join(", ")
                );
                self.trials.push(trial);
                format!(
                    "ok: trying {id}: it is undone in {seconds} s unless it is kept (`keep {id}`)\n"
                )
            }
            Err(err) => format!("error: {err}\n"),
        }
    }

    /// Checks the request in `words`, keeps a copy of each file it names
    /// and writes its record; the trial, or why not.
    fn begin_trial(&mut self, words: &[&str], user: u32) -> Result<Trial, String> {
        if self.rescue {
            return Err("Rescue tries nothing: change the files, then start the services".into());
        }
        if matches!(self.shutdown, Some(super::Shutdown::Stopping { .. })) {
            return Err("shutting down".into());
        }
        let request = Request::parse(words)?;
        if self.trials.iter().any(|t| t.id == request.id) {
            return Err(format!("the trial {} runs already: keep it or undo it first", request.id));
        }
        if self.trials.len() >= TRIALS_MAX {
            return Err(format!("{TRIALS_MAX} trials run already: keep or undo one first"));
        }
        for path in &request.files {
            if let Some(trial) =
                self.trials.iter().find(|t| t.record.files.iter().any(|k| &k.path == path))
            {
                return Err(format!("{path} is being tried in the trial {} already", trial.id));
            }
        }
        let mut failed_before = Vec::new();
        let mut restart = Vec::new();
        for name in request.restart.iter().chain(&request.reload) {
            let index = self
                .services
                .iter()
                .position(|s| &s.spec.name == name)
                .ok_or_else(|| format!("no service is called {name}"))?;
            failed_before.push((index, self.services[index].failed_starts));
            if request.restart.contains(name) {
                let ran = !matches!(self.services[index].state, State::Stopped | State::Failed);
                restart.push(Named { name: name.clone(), ran });
            }
        }

        let folder = format!("{TRIALS}/{}", request.id);
        // Left by a `try` that never answered: nothing was changed by it.
        remove_folder(&self.root, &folder);
        let kept = keep_copies(&self.root, &folder, &request.files).and_then(|files| {
            let record =
                Record { seconds: request.seconds, files, restart, reload: request.reload };
            let dir = self
                .root
                .open_dir(&folder, READ_WRITE)
                .map_err(|err| format!("/{folder}: {err}"))?;
            // Written last: until it is there, the trial is not.
            services::write_new(&dir, RECORD, &record.text())?;
            Ok(record)
        });
        let record = match kept {
            Ok(record) => record,
            Err(err) => {
                remove_folder(&self.root, &folder);
                return Err(err);
            }
        };
        let ends = now().saturating_add(u64::from(request.seconds) * SECOND);
        Ok(Trial { id: request.id, record, user, ends, failed_before })
    }

    /// The answer to `keep ID` from process `peer`.
    pub(super) fn answer_keep(&mut self, id: Option<&str>, peer: u64) -> String {
        let at = match self.find_trial(id) {
            Ok(at) => at,
            Err(err) => return format!("error: {err}\n"),
        };
        let trial = self.trials.remove(at);
        remove_folder(&self.root, &format!("{TRIALS}/{}", trial.id));
        audit!("kept the trial {}, by process {peer}", trial.id);
        format!("ok: kept {}\n", trial.id)
    }

    /// The answer to `undo ID` from process `peer`.
    pub(super) fn answer_undo(&mut self, id: Option<&str>, peer: u64) -> String {
        let at = match self.find_trial(id) {
            Ok(at) => at,
            Err(err) => return format!("error: {err}\n"),
        };
        audit!("asked to undo the trial {}, by process {peer}", self.trials[at].id);
        let words = self.undo(at, &Why::Asked);
        format!("ok: {words}\n")
    }

    /// Where the trial `id` is among those that run, or why it is not.
    fn find_trial(&self, id: Option<&str>) -> Result<usize, String> {
        let id = id.ok_or("keep and undo need a trial's name")?;
        check_id(id)?;
        self.trials
            .iter()
            .position(|t| t.id == id)
            .ok_or_else(|| format!("no trial called {id} runs: it was kept or undone"))
    }

    /// Undoes each trial whose time is up, or one of whose services
    /// failed to start or failed its health check since it began.
    pub(super) fn step_trials(&mut self, time: u64) {
        let mut at = 0;
        while at < self.trials.len() {
            let trial = &self.trials[at];
            let failed = trial
                .failed_before
                .iter()
                .find(|&&(index, before)| self.services[index].failed_starts > before)
                .map(|&(index, _)| index);
            if trial.ends <= time {
                self.undo(at, &Why::TimeUp);
            } else if let Some(index) = failed {
                let name = self.services[index].spec.name.clone();
                self.undo(at, &Why::Failed(name));
            } else {
                at += 1;
            }
        }
    }

    /// When the next trial is undone unless it is kept.
    pub(super) fn trials_deadline(&self) -> Option<u64> {
        self.trials.iter().map(|t| t.ends).min()
    }

    /// Undoes the trial at `at`, as `why` says: puts its copies back,
    /// removes the files it found missing, then restarts the services
    /// that ran when it began, stops those that did not, and asks the
    /// others to reload. Returns what it did, as words after "ok: ".
    fn undo(&mut self, at: usize, why: &Why) -> String {
        let trial = self.trials.remove(at);
        let folder = format!("{TRIALS}/{}", trial.id);
        let (put_back, failed) = put_back(&self.root, &folder, &trial.record);
        if failed.is_empty() {
            remove_folder(&self.root, &folder);
        }
        // Each is started again as its file now says, which is the copy.
        let mut restarted = Vec::new();
        for named in &trial.record.restart {
            let Some(index) = self.services.iter().position(|s| s.spec.name == named.name) else {
                continue;
            };
            let running = !matches!(self.services[index].state, State::Stopped | State::Failed);
            if named.ran {
                let _ = self.answer_restart(index);
                restarted.push(format!("restarted {}", named.name));
            } else if running {
                let _ = self.answer_stop(index);
                restarted.push(format!("stopped {}", named.name));
            }
        }
        for name in &trial.record.reload {
            if self.services.iter().any(|s| &s.spec.name == name && s.state.pid().is_some()) {
                let _ = self.answer_reload(Some(name), 0);
                restarted.push(format!("asked {name} to reload"));
            }
        }
        let mut words = format!("undid {} ({})", trial.id, why.words());
        if !put_back.is_empty() {
            words.push_str(&format!(": put back {}", put_back.join(", ")));
        }
        if !restarted.is_empty() {
            words.push_str(&format!("; {}", restarted.join(", ")));
        }
        for err in &failed {
            words.push_str(&format!("; {err}, so the next start tries again"));
        }
        // Logged once, in the audit log too; the watch of the first
        // service hears of it, as a `service try` that waits on one.
        audit!("{words}");
        if let Some(&(index, _)) = trial.failed_before.first() {
            tell(&mut self.told, index, words.clone());
        }
        // A wait on the start that failed ends with it, though the undo
        // has started the service again as it was.
        if let Why::Failed(name) = why
            && let Some(index) = self.services.iter().position(|s| &s.spec.name == name)
        {
            let line = format!("error: {name} failed with the change, so Spark undid {}", trial.id);
            self.end_watches(index, &line);
        }
        self.note_undone(trial.id, why);
        words
    }

    /// Keeps that the trial `id` was undone, as `why` says, for `show`.
    pub(super) fn note_undone(&mut self, id: String, why: &Why) {
        if self.undone.len() == show::UNDONE_KEPT {
            self.undone.remove(0);
        }
        self.undone.push(show::Undone { id, why: why.words(), at_ms: now() / MS });
    }

    /// The trials that run, for `show`.
    pub(super) fn shown_trials(&self, time: u64) -> Vec<show::Trial> {
        self.trials
            .iter()
            .map(|trial| show::Trial {
                id: trial.id.clone(),
                files: trial.record.files.iter().map(|k| k.path.clone()).collect(),
                services: trial
                    .record
                    .restart
                    .iter()
                    .map(|n| n.name.clone())
                    .chain(trial.record.reload.iter().cloned())
                    .collect(),
                user: trial.user,
                left_ms: trial.ends.saturating_sub(time) / MS,
            })
            .collect()
    }
}

/// Undoes each trial a start before this one left unkept, before any
/// service starts: the machine started again while it ran, as a change
/// that hangs it or cuts it off would have it do. Returns their names.
pub fn undo_left(root: &Dir) -> Vec<String> {
    let mut undone = Vec::new();
    let Ok(dir) = root.open_dir(TRIALS, Rights::READ) else { return undone };
    let Ok(entries) = dir.entries() else { return undone };
    for entry in entries.iter().filter(|e| e.kind == FileKind::Directory) {
        let id = &entry.name;
        let folder = format!("{TRIALS}/{id}");
        let text = match root.read_file(&format!("{folder}/{RECORD}")) {
            Ok(bytes) => String::from_utf8(bytes).ok(),
            // A `try` that never answered: its files never changed.
            Err(err) if err.is(FileError::NotFound) => {
                remove_folder(root, &folder);
                continue;
            }
            Err(_) => None,
        };
        let Some(record) = text.and_then(|text| Record::parse(&text).ok()) else {
            // Never half undone: it stays for a person to look at.
            println!("spark: cannot read the record of the trial {id} in /{folder}; it is left");
            continue;
        };
        let (put_back, failed) = put_back(root, &folder, &record);
        if failed.is_empty() {
            remove_folder(root, &folder);
        }
        let mut words = format!("undid {id} ({})", Why::Restarted.words());
        if !put_back.is_empty() {
            words.push_str(&format!(": put back {}", put_back.join(", ")));
        }
        for err in &failed {
            words.push_str(&format!("; {err}"));
        }
        println!("<86>spark: {words}");
        undone.push(id.clone());
    }
    undone
}

/// Keeps a copy of each file at `paths` in the trial folder `folder`,
/// which only root reads: what the record says of each.
fn keep_copies(root: &Dir, folder: &str, paths: &[String]) -> Result<Vec<Kept>, String> {
    services::make_dirs(root, folder)?;
    // The copies may be of private files, whatever their own modes say.
    root.set_access(folder, Some(0), Some(0), Some(0o700))
        .map_err(|err| format!("/{folder}: {err}"))?;
    let dir = root.open_dir(folder, READ_WRITE).map_err(|err| format!("/{folder}: {err}"))?;
    let mut files = Vec::new();
    for (number, path) in paths.iter().enumerate() {
        let copy = match read_plain(root, path)? {
            Some((bytes, stat)) => {
                let name = number.to_string();
                services::write_bytes(&dir, &name, &bytes)?;
                Some(Saved { name, mode: stat.mode, owner: stat.owner, group: stat.group })
            }
            None => None,
        };
        files.push(Kept { path: path.clone(), copy });
    }
    Ok(files)
}

/// Puts back what `record`, kept in `folder`, holds: each copy over its
/// file, whole or not at all, with the file's mode, owner and group, and
/// each file it found missing removed. Returns the paths put back or
/// removed, and what could not be.
fn put_back(root: &Dir, folder: &str, record: &Record) -> (Vec<String>, Vec<String>) {
    let (mut done, mut failed) = (Vec::new(), Vec::new());
    for kept in &record.files {
        let path = &kept.path[1..];
        let result = no_links_above(root, &kept.path).and_then(|()| match &kept.copy {
            Some(copy) => {
                let bytes = root
                    .read_file(&format!("{folder}/{}", copy.name))
                    .map_err(|err| format!("/{folder}/{}: {err}", copy.name))?;
                let (parent, name) = path.rsplit_once('/').unwrap_or(("", path));
                services::make_dirs(root, parent)?;
                let dir =
                    root.open_dir(parent, READ_WRITE).map_err(|err| format!("/{parent}: {err}"))?;
                write_as(&dir, name, &bytes, copy)
            }
            None => match root.remove(path) {
                Err(err) if !err.is(FileError::NotFound) => Err(format!("/{path}: {err}")),
                _ => Ok(()),
            },
        });
        match result {
            Ok(()) => done.push(kept.path.clone()),
            Err(err) => failed.push(format!("cannot put back {}: {err}", kept.path)),
        }
    }
    (done, failed)
}

/// Writes `bytes` to `name` in `dir` whole or not at all, as
/// [`services::write_bytes`] does, with the mode, owner and group `copy`
/// kept, set before the bytes are in it.
fn write_as(dir: &Dir, name: &str, bytes: &[u8], copy: &Saved) -> Result<(), String> {
    let new = format!("{name}.new");
    let written = dir.create(&new).and_then(|file| {
        dir.set_access(&new, Some(copy.owner), Some(copy.group), Some(copy.mode))?;
        file.write_all_at(0, bytes)?;
        file.sync()?;
        dir.rename(&new, dir, name)?;
        dir.sync()
    });
    written.map_err(|err| {
        let _ = dir.remove(&new);
        format!("cannot write {name}: {err}")
    })
}

/// The file at `path`, an absolute path that [`check_path`] passed, and
/// what it is, if there is one: a link is refused rather than followed,
/// on the way to it too, and so is anything but a file and a file over
/// [`FILE_MAX`] bytes.
///
/// [`check_path`]: spark_api::trial::check_path
fn read_plain(root: &Dir, path: &str) -> Result<Option<(Vec<u8>, Stat)>, String> {
    no_links_above(root, path)?;
    let file = match root.open(&path[1..], OpenFlags::NO_FOLLOW, Rights::READ) {
        Ok(file) => file,
        Err(err) if err.is(FileError::NotFound) => return Ok(None),
        Err(err) if err.is(FileError::TooManyLinks) => {
            return Err(format!("{path} is a link: a trial keeps files alone"));
        }
        Err(err) => return Err(format!("{path}: {err}")),
    };
    let stat = file.stat().map_err(|err| format!("{path}: {err}"))?;
    if stat.kind != FileKind::File {
        return Err(format!("{path} is not a file: a trial keeps files alone"));
    }
    if stat.size > FILE_MAX as u64 {
        return Err(format!(
            "{path} is larger than {} KiB: a trial keeps settings alone",
            FILE_MAX >> 10
        ));
    }
    let bytes = file.read_to_end().map_err(|err| format!("{path}: {err}"))?;
    Ok(Some((bytes, stat)))
}

/// Whether each folder between [`SETTINGS`] and the file at `path` is a
/// folder and not a link, so that what a trial writes is beneath the
/// settings, as the path says.
fn no_links_above(root: &Dir, path: &str) -> Result<(), String> {
    let inner = path.strip_prefix(SETTINGS).unwrap_or(path);
    let base = &SETTINGS[1..];
    let mut at = String::from(base.trim_end_matches('/'));
    let mut names: Vec<&str> = inner.split('/').collect();
    names.pop();
    for name in names {
        at.push('/');
        at.push_str(name);
        if root.read_link(&at).is_ok() {
            return Err(format!("/{at} is a link: a trial writes beneath {SETTINGS} alone"));
        }
    }
    Ok(())
}

/// Removes the trial folder `folder` and what it holds, if it is there.
fn remove_folder(root: &Dir, folder: &str) {
    if let Ok(dir) = root.open_dir(folder, READ_WRITE)
        && let Ok(entries) = dir.entries()
    {
        for entry in entries {
            let _ = dir.remove(&entry.name);
        }
    }
    let _ = root.remove(folder);
}
