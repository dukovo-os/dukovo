// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The journal: what changed on the machine, and who changed it (ADR
//! 0516). Its rows and the versions of the files they name are in the
//! [store](super::store); this is what makes them.
//!
//! **A console change** has a row when its command has ended, worked or
//! not (a command that was not run, because authd or the caller's rights
//! refused it, changed nothing and has none; the log says it). Before the
//! command runs the journal looks at each file the action names
//! ([`Journal::begin`]); when it has ended it looks again and adds the row
//! ([`Begun::end`]), whose files say what each was before and after, by
//! hash. Each text is kept as a version, so the page can show the diff and
//! `admin put-back` can write the earlier one.
//!
//! **A hand edit** is found by the round every 30 seconds
//! ([`Journal::scan`], [`Journal::record`]). Dukovo has no notification of
//! a file's change, so the round compares the size and modification time
//! of each file under `/system/settings`, and of each file in a folder
//! that an action's files named, with what they were at the last look,
//! and reads and hashes only those that differ. A file made, changed or
//! removed that no console change is under way for has a `by hand` row,
//! one for each person [`audit`](super::audit) names and one for the rest.
//! The round's own work is a few hundred stats; nothing here waits, so
//! it is done on admind's thread, with the clock a value that the caller
//! gives, which is how the tests drive it.
//!
//! **The tabs are told.** A row, once the store has kept it, is announced
//! ([`News`]), and the stream of events tells each open tab, as everyone's
//! to read, `event: change` with the row's number, how it was made and
//! when: no person, action or file.
//!
//! **What is not shown.** Everyone who is signed in reads the journal,
//! and not everyone may read the files. A file under a service's
//! `secrets`, one that not everyone may read (no read bit for others), and
//! one in a folder that others may not pass through, is listed by name and
//! never read: no hash, no version, no diff. admind runs as root, so it
//! follows each path from the root a part at a time and never through a
//! link, and does not watch links ([`find`]). A file admind could not read
//! is listed as unreadable. A file longer than 1 MiB is listed with its
//! hash and size, and no text. `admin export` copies the files of the
//! settings that the round watches and leaves out the same secrets
//! ([`settings_files`], [`is_hidden`]).

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{File, Metadata};
use std::io::{self, Read};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use ring::digest;

pub use self::news::{News, Row};

use super::audit::{self, Editor, NOBODY};
use super::store::changes::{Change, FileChange, NewChange, Seen};
use super::store::versions::sha256;
use super::store::watched::Watched;
use super::store::{Result, Store};
use super::terminal::Rooted;
use crate::bundles::folder::{self, Dir};

/// Where the settings are, which the round watches whole.
pub const SETTINGS: &str = "/system/settings";

/// Where Spark keeps the services' secrets.
pub const SECRETS: &str = "/system/state/spark/secrets";

/// How deep the round goes into the settings: they are a few levels.
const DEPTH_SETTINGS: usize = 8;
/// How deep it goes into a folder an action named, which is a service's
/// own.
const DEPTH_NAMED: usize = 4;

/// The most files a round looks at, so that a folder of thousands that an
/// action named costs it no more than the settings do.
const FILES_MAX: usize = 10_000;

/// What a file's change is told by: its size and modification time.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Stamp {
    /// The size in bytes.
    pub size: u64,
    /// The modification time in nanoseconds since 1970.
    pub mtime: i64,
}

impl Stamp {
    /// The stamp of a file whose status is `st`. The times are `i64` on the
    /// 64-bit targets this builds for, and the conversions say what they
    /// are for wherever they are not.
    #[allow(clippy::useless_conversion)]
    fn of_stat(st: &libc::stat) -> Stamp {
        let seconds = i64::from(st.st_mtime);
        Stamp {
            size: u64::try_from(st.st_size).unwrap_or(0),
            mtime: seconds.saturating_mul(1_000_000_000) + i64::from(st.st_mtime_nsec),
        }
    }

    /// The Unix second of the modification.
    pub fn seconds(self) -> i64 {
        self.mtime.div_euclid(1_000_000_000)
    }
}

/// Whether `path` is under a service's secrets.
fn is_secret(path: &str) -> bool {
    Path::new(path).starts_with(SECRETS)
}

/// Whether the file at `path`, whose metadata is `md`, is a secret by the
/// journal's rule for the file itself: one under a service's secrets, or
/// one that not everyone may read. `admin export`, which runs as the
/// person, leaves such files out; the journal also hides a file in a
/// folder others may not pass through ([`find`]).
pub fn is_hidden(path: &str, md: &Metadata) -> bool {
    is_secret(path) || md.mode() & 0o004 == 0
}

/// The settings under a folder, as the round watches them.
pub struct SettingsFiles {
    /// The regular files, to the depth and in the number the round looks
    /// at them.
    pub files: BTreeSet<String>,
    /// The folders that could not be opened, whose files are not in `files`.
    pub closed: Vec<String>,
}

/// The settings files under `dir`, which `admin export` copies.
pub fn settings_files(dir: &Path) -> SettingsFiles {
    let (mut files, mut room) = (BTreeSet::new(), FILES_MAX);
    let closed = walk(dir, DEPTH_SETTINGS, &mut files, &mut room);
    SettingsFiles { files, closed }
}

/// A file as the journal finds it by its path (see [`find`]).
enum Found {
    /// There is no such file.
    Absent,
    /// Something the journal does not read: a link, a folder or another
    /// kind of file that is not a regular one, or a path it could not
    /// follow; with its stamp when it is there.
    Unreadable(Option<Stamp>),
    /// A regular file that not everyone may read by its path: one of a
    /// service's secrets, one with no read bit for others, or one in a
    /// folder that others may not pass through.
    Hidden(Stamp),
    /// A regular file that everyone may read, open when it was asked to
    /// be.
    Public(Option<File>, Stamp),
}

/// Whether a folder whose status is `st` lets others pass through it.
fn others_pass(st: &libc::stat) -> bool {
    st.st_mode & 0o001 != 0
}

/// Whether what `st` describes is a regular file.
fn is_regular(st: &libc::stat) -> bool {
    st.st_mode & libc::S_IFMT == libc::S_IFREG
}

/// Finds the file at `path`, and opens it when `open` and everyone may
/// read it.
///
/// admind runs as root, which passes every folder whatever its mode and
/// follows any link, and everyone who is signed in reads what the journal
/// keeps. So the path is followed from the root a part at a time, never
/// through a link: a service's account could put a link in a folder of its
/// own to lead admind to a file it may not read itself, and have its text
/// shown. A file is everyone's only when each folder on its way lets
/// others pass and the file lets others read it, as for a person who is
/// not root. A file is opened only once it is known to be a regular one,
/// so that no device or pipe is ever opened, and only when it is still that
/// file once open.
fn find(path: &str, open: bool) -> Found {
    let Some(parts) = path.strip_prefix('/') else { return Found::Unreadable(None) };
    let parts: Vec<&str> = parts.split('/').collect();
    let Some((name, folders)) = parts.split_last() else { return Found::Unreadable(None) };
    let Ok(mut dir) = Dir::open("/") else { return Found::Unreadable(None) };
    let mut public = dir.stat().is_ok_and(|st| others_pass(&st));
    for part in folders {
        // Not through a link, and only a folder: anything else fails here.
        let next = match dir.dir_at(part) {
            Ok(next) => next,
            Err(err) if err.kind() == io::ErrorKind::NotFound => return Found::Absent,
            Err(_) => return Found::Unreadable(None),
        };
        match next.stat() {
            Ok(st) => public &= others_pass(&st),
            Err(_) => return Found::Unreadable(None),
        }
        dir = next;
    }
    let st = match dir.stat_at(name) {
        Ok(Some(st)) => st,
        Ok(None) => return Found::Absent,
        Err(_) => return Found::Unreadable(None),
    };
    let stamp = Stamp::of_stat(&st);
    if !is_regular(&st) {
        return Found::Unreadable(Some(stamp));
    }
    if !public || st.st_mode & 0o004 == 0 || is_secret(path) {
        return Found::Hidden(stamp);
    }
    if !open {
        return Found::Public(None, stamp);
    }
    let Ok(fd) = dir.open_at(name, false) else { return Found::Unreadable(Some(stamp)) };
    match folder::fd_stat(&fd) {
        Ok(now) if folder::same(&st, &now) && is_regular(&now) && now.st_mode & 0o004 != 0 => {
            Found::Public(Some(File::from(fd)), Stamp::of_stat(&now))
        }
        _ => Found::Unreadable(Some(stamp)),
    }
}

/// The stamp of the file at `path` when it is there, found as [`find`]
/// finds it, without opening it.
fn stamp_of(path: &str) -> Option<Stamp> {
    match find(path, false) {
        Found::Absent | Found::Unreadable(None) => None,
        Found::Unreadable(Some(stamp)) | Found::Hidden(stamp) | Found::Public(_, stamp) => {
            Some(stamp)
        }
    }
}

/// What `path` is now, and its stamp when it is there, and keeps the text
/// as a version of the file at Unix time `now`. A secret, a file not
/// everyone may read and anything that is not a regular file are never
/// read (see [`find`]).
///
/// # Errors
///
/// If the store fails; a file that cannot be read is [`Seen::Unreadable`].
pub fn look(store: &Store, path: &str, now: i64) -> Result<(Seen, Option<Stamp>)> {
    let (file, stamp) = match find(path, true) {
        Found::Absent => return Ok((Seen::Absent, None)),
        Found::Unreadable(stamp) => return Ok((Seen::Unreadable, stamp)),
        // Everyone who is signed in reads the journal: a file that not
        // everyone reads has its text kept out of it.
        Found::Hidden(stamp) => return Ok((Seen::Secret, Some(stamp))),
        Found::Public(None, stamp) => return Ok((Seen::Unreadable, Some(stamp))),
        Found::Public(Some(file), stamp) => (file, stamp),
    };
    let limit = store.limits.file;
    let mut bytes = Vec::new();
    // One byte past the limit: a file longer than the store keeps is read
    // on only to be hashed, from the same descriptor.
    let mut file = file;
    if (&mut file).take(limit + 1).read_to_end(&mut bytes).is_err() {
        return Ok((Seen::Unreadable, Some(stamp)));
    }
    if bytes.len() as u64 > limit {
        let seen = hash_rest(bytes, file)
            .map_or(Seen::Unreadable, |(sha256, size)| Seen::Large { sha256, size });
        return Ok((seen, Some(stamp)));
    }
    store.keep_version(path, &bytes, now)?;
    Ok((Seen::Text { sha256: sha256(&bytes), size: bytes.len() as u64 }, Some(stamp)))
}

/// The SHA-256 and size of a file whose first bytes are `first` and the
/// rest of which `file` reads, in pieces.
fn hash_rest(first: Vec<u8>, mut file: File) -> io::Result<(String, u64)> {
    let mut context = digest::Context::new(&digest::SHA256);
    context.update(&first);
    let mut size = first.len() as u64;
    let mut buffer = first;
    buffer.resize(64 << 10, 0);
    loop {
        match file.read(&mut buffer)? {
            0 => break,
            n => {
                context.update(&buffer[..n]);
                size += n as u64;
            }
        }
    }
    Ok((accounts::request::hex(context.finish().as_ref()), size))
}

/// A file the round found changed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Edited {
    /// Its path.
    pub path: String,
    /// What it was at the last look.
    pub before: Seen,
    /// What it is.
    pub after: Seen,
    /// Its stamp now, when it is there.
    pub stamp: Option<Stamp>,
}

/// What a round found: the files changed, to be given a person and
/// recorded.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Scan {
    /// The files, in the order of their paths.
    pub edits: Vec<Edited>,
}

impl Scan {
    /// The path and the Unix time of each change, for [`audit`] to find
    /// who made them.
    pub fn times(&self, now: i64) -> Vec<(String, i64)> {
        self.edits
            .iter()
            .map(|edit| {
                let edited = edit.stamp.map_or(now, |stamp| stamp.seconds().min(now));
                (edit.path.clone(), edited)
            })
            .collect()
    }
}

/// The journal's state in memory: the files console changes are under way
/// on, which the round leaves alone until they end, and the rows that
/// ended, for the streams of events to tell.
pub struct Journal {
    /// Where the settings are: [`SETTINGS`], but in a test.
    settings: PathBuf,
    pending: RefCell<BTreeMap<String, usize>>,
    /// The rows that ended lately: `event: change`.
    pub news: News,
}

/// A console change that has begun: what each file named was before.
#[must_use = "a change that begins is ended"]
pub struct Begun<'a> {
    journal: &'a Journal,
    files: Vec<(String, Seen)>,
}

/// What the end of a console change found.
#[derive(Debug, PartialEq, Eq)]
pub struct Ending {
    /// The number of its row in the journal, when the store kept one.
    pub row: Option<i64>,
    /// Whether each file the change names reads as it did before the
    /// change: true when it names none, false when the journal cannot
    /// tell (no store, a secret, a file it may not read).
    pub same: bool,
}

/// How a console change ended.
#[derive(Debug)]
pub struct Ended<'a> {
    /// The person.
    pub who: &'a str,
    /// `console` or `token NAME`.
    pub how: &'a str,
    /// The action's ID.
    pub action: &'a str,
    /// The command it ran.
    pub argv: &'a [String],
    /// The command's exit status, if it ended with one.
    pub status: Option<i32>,
    /// The task it ran as.
    pub task: Option<i64>,
    /// What else there is to say.
    pub note: Option<&'a str>,
}

impl Journal {
    /// A journal that watches the settings in `settings`.
    pub fn new(settings: &Path) -> Journal {
        Journal { settings: settings.to_path_buf(), pending: RefCell::default(), news: News::new() }
    }

    /// Begins the change of the files `files`: looks at each, which keeps
    /// the version it has now, and has the round leave them alone until
    /// the change ends. A store that fails is said in the log, and the
    /// row will say what it can.
    pub fn begin<'a>(&'a self, store: Option<&Store>, files: &[String], now: i64) -> Begun<'a> {
        let mut seen = Vec::with_capacity(files.len());
        for path in files {
            *self.pending.borrow_mut().entry(path.clone()).or_default() += 1;
            let before = store
                .map(|store| look(store, path, now))
                .transpose()
                .unwrap_or_else(|why| {
                    super::log(4, &format!("the journal could not look at {path}: {why}"));
                    None
                })
                .map_or(Seen::Unreadable, |(seen, _)| seen);
            seen.push((path.clone(), before));
        }
        Begun { journal: self, files: seen }
    }

    /// How many files console changes are under way on, for the tests.
    #[cfg(test)]
    pub fn pending_files(&self) -> usize {
        self.pending.borrow().len()
    }

    /// Whether a console change is under way on `path`.
    fn is_pending(&self, path: &str) -> bool {
        self.pending.borrow().contains_key(path)
    }

    /// The folders an action named files in, which the round watches too:
    /// those of the watched files that are not under the settings, and
    /// not the services' secrets.
    fn named_folders(&self, watched: &BTreeMap<String, Watched>) -> BTreeSet<PathBuf> {
        watched
            .keys()
            .map(Path::new)
            .filter(|path| !path.starts_with(&self.settings) && !path.starts_with(SECRETS))
            .filter_map(Path::parent)
            .map(Path::to_path_buf)
            .collect()
    }

    /// The folders of `named`, files a change names, that are not
    /// watched yet: outside the settings and the secrets, and with no
    /// watched file in them.
    fn fresh_folders<'a>(
        &self,
        store: &Store,
        named: impl Iterator<Item = &'a String>,
    ) -> Result<BTreeSet<PathBuf>> {
        let watched = store.watched()?;
        let known = self.named_folders(&watched);
        Ok(named
            .map(Path::new)
            .filter(|path| !path.starts_with(&self.settings) && !path.starts_with(SECRETS))
            .filter_map(Path::parent)
            .filter(|folder| !known.contains(*folder))
            .map(Path::to_path_buf)
            .collect())
    }

    /// Notes the files in `folder` that are not noted yet as they are,
    /// for a folder that is watched from now on.
    fn note_folder(store: &Store, folder: &Path, now: i64) -> Result<()> {
        let watched = store.watched()?;
        let (mut paths, mut room) = (BTreeSet::new(), FILES_MAX);
        walk(folder, DEPTH_NAMED, &mut paths, &mut room);
        for path in paths.iter().filter(|path| !watched.contains_key(*path)) {
            if let (seen, Some(stamp)) = look(store, path, now)? {
                store.set_watched(path, &Watched { size: stamp.size, mtime: stamp.mtime, seen })?;
            }
        }
        Ok(())
    }

    /// The round: compares each watched file with what it was at the last
    /// look, at Unix time `now`, and reads and hashes those that differ.
    /// Files made, changed or removed are in the answer, for
    /// [`Journal::record`] once it knows who; a file that was touched and
    /// not changed, or that is seen for the first time of all, is only
    /// noted. The first round of a store that has no watched files notes
    /// everything it finds and reports none, since nothing changed then.
    ///
    /// # Errors
    ///
    /// If the store fails.
    pub fn scan(&self, store: &Store, now: i64) -> Result<Scan> {
        // One transaction: the first round's hundreds of files are one
        // write to the disk, and one that fails leaves the round to be
        // done again.
        store.atomically(|| self.scan_in(store, now))
    }

    /// [`Journal::scan`], in the transaction its caller has begun.
    fn scan_in(&self, store: &Store, now: i64) -> Result<Scan> {
        let watched = store.watched()?;
        let first = watched.is_empty();
        let mut paths = BTreeSet::new();
        let mut room = FILES_MAX;
        walk(&self.settings, DEPTH_SETTINGS, &mut paths, &mut room);
        for folder in self.named_folders(&watched) {
            walk(&folder, DEPTH_NAMED, &mut paths, &mut room);
        }
        paths.extend(watched.keys().cloned());
        let mut edits = Vec::new();
        for path in paths {
            if self.is_pending(&path) {
                continue;
            }
            let known = watched.get(&path);
            let now_stamp = stamp_of(&path);
            if known.is_some()
                && now_stamp.map(|s| (s.size, s.mtime)) == known.map(|w| (w.size, w.mtime))
            {
                continue;
            }
            let (after, stamp) = look(store, &path, now)?;
            match (known, stamp) {
                // Gone since the last look.
                (Some(known), None) if known.seen != Seen::Absent => {
                    edits.push(Edited { path, before: known.seen.clone(), after, stamp });
                }
                (Some(_) | None, None) => {}
                // Touched, and what it holds is what it held.
                (Some(known), Some(stamp)) if known.seen.same_as(&after) => {
                    store.set_watched(
                        &path,
                        &Watched { size: stamp.size, mtime: stamp.mtime, seen: after },
                    )?;
                }
                (Some(known), Some(stamp)) => {
                    edits.push(Edited {
                        path,
                        before: known.seen.clone(),
                        after,
                        stamp: Some(stamp),
                    });
                }
                // Seen for the first time.
                (None, Some(stamp)) if first => {
                    store.set_watched(
                        &path,
                        &Watched { size: stamp.size, mtime: stamp.mtime, seen: after },
                    )?;
                }
                (None, Some(stamp)) => {
                    edits.push(Edited { path, before: Seen::Absent, after, stamp: Some(stamp) });
                }
            }
        }
        Ok(Scan { edits })
    }

    /// Adds the `by hand` rows for what [`Journal::scan`] found at Unix
    /// time `now`, one for each person `who` names (path to person) and
    /// one for the files with none, and notes what the files are now.
    /// A file that a console change has taken since the scan is left to
    /// that change. Gives the rows made.
    ///
    /// # Errors
    ///
    /// If the store fails; none of it is then kept, and the next round
    /// finds the files again.
    pub fn record(
        &self,
        store: &Store,
        scan: &Scan,
        who: &BTreeMap<String, Editor>,
        now: i64,
    ) -> Result<usize> {
        let made = store.atomically(|| {
            let watched = store.watched()?;
            // A row for each person, and the account files in rows of
            // their own: a change to them is never put back with others.
            let mut by_person: BTreeMap<(&str, Option<&str>, bool), Vec<FileChange>> =
                BTreeMap::new();
            for edit in &scan.edits {
                let noted = watched.get(&edit.path).map_or(&Seen::Absent, |noted| &noted.seen);
                if self.is_pending(&edit.path) || *noted != edit.before {
                    continue;
                }
                let (person, note) = who
                    .get(&edit.path)
                    .map_or((NOBODY, None), |editor| (editor.name.as_str(), editor.note()));
                by_person.entry((person, note, is_accounts(&edit.path))).or_default().push(
                    FileChange {
                        path: edit.path.clone(),
                        before: edit.before.clone(),
                        after: edit.after.clone(),
                    },
                );
                match edit.stamp {
                    Some(stamp) => store.set_watched(
                        &edit.path,
                        &Watched { size: stamp.size, mtime: stamp.mtime, seen: edit.after.clone() },
                    )?,
                    None => store.forget_watched(&edit.path)?,
                }
            }
            let mut rows = Vec::with_capacity(by_person.len());
            for ((person, note, _), files) in &by_person {
                rows.push(store.add_change(&NewChange {
                    at: now,
                    who: person,
                    how: "by hand",
                    action: None,
                    argv: None,
                    status: None,
                    task: None,
                    note: *note,
                    files,
                })?);
            }
            Ok(rows)
        })?;
        // The rows are kept: the tabs are told of them.
        for id in &made {
            self.news.announce(*id, "by hand", now);
        }
        Ok(made.len())
    }

    /// One round, as the server runs it every 30 seconds: [`scan`], and
    /// for what it found, who [`audit`](super::audit) says, from the logd
    /// at `logd`, then [`record`]. A logd that does not answer leaves the
    /// rows saying "outside the console". No transaction is open while
    /// logd is waited for. Gives the rows made.
    ///
    /// # Errors
    ///
    /// If the store fails.
    ///
    /// [`scan`]: Journal::scan
    /// [`record`]: Journal::record
    pub async fn round(
        &self,
        store: &Store,
        logd: &str,
        rooted: &[Rooted],
        now: i64,
    ) -> Result<usize> {
        let scan = self.scan(store, now)?;
        if scan.edits.is_empty() {
            return Ok(0);
        }
        let times = scan.times(now);
        let earliest = times.iter().map(|(_, at)| *at).min().unwrap_or(now);
        let mut who = match audit::elevated_between(logd, earliest - audit::WINDOW, now).await {
            Ok(commands) => audit::who_edited_each(&commands, &times),
            Err(why) => {
                super::log(4, &format!("the journal cannot tell who changed a file: {why}"));
                BTreeMap::new()
            }
        };
        audit::add_terminal_editors(&mut who, rooted, &times);
        self.record(store, &scan, &who, now)
    }
}

impl Begun<'_> {
    /// Ends the change: looks at each file again, adds the row for the
    /// command that ran (`ended`), and notes what the files are now, so
    /// that the round does not take the change for a hand edit. Gives
    /// the row's number, and whether the files read as they did. The files
    /// are free to the round again.
    pub fn end(self, store: Option<&Store>, ended: &Ended<'_>, now: i64) -> Ending {
        let Some(store) = store else {
            return Ending { row: None, same: self.files.is_empty() };
        };
        let mut same = true;
        let made = store.atomically(|| {
            let fresh =
                self.journal.fresh_folders(store, self.files.iter().map(|(path, _)| path))?;
            let mut files = Vec::with_capacity(self.files.len());
            for (path, before) in &self.files {
                let (after, stamp) = look(store, path, now)?;
                same &= before.same_as(&after);
                match stamp {
                    Some(stamp) => store.set_watched(
                        path,
                        &Watched { size: stamp.size, mtime: stamp.mtime, seen: after.clone() },
                    )?,
                    None => store.forget_watched(path)?,
                }
                files.push(FileChange { path: path.clone(), before: before.clone(), after });
            }
            // What else is in a folder that is watched from now on was
            // there before, and is not a change.
            for folder in fresh {
                Journal::note_folder(store, &folder, now)?;
            }
            store.add_change(&NewChange {
                at: now,
                who: ended.who,
                how: ended.how,
                action: Some(ended.action),
                argv: Some(ended.argv),
                status: ended.status,
                task: ended.task,
                note: ended.note,
                files: &files,
            })
        });
        match made {
            Ok(id) => {
                self.journal.news.announce(id, ended.how, now);
                Ending { row: Some(id), same }
            }
            Err(why) => {
                super::log(
                    4,
                    &format!("the journal could not keep {}'s change: {why}", ended.action),
                );
                Ending { row: None, same: false }
            }
        }
    }

    /// Ends the change with no row, for a command that was not run.
    pub fn cancel(self) {
        drop(self);
    }
}

impl Drop for Begun<'_> {
    fn drop(&mut self) {
        let mut pending = self.journal.pending.borrow_mut();
        for (path, _) in &self.files {
            if let Some(count) = pending.get_mut(path) {
                *count -= 1;
                if *count == 0 {
                    pending.remove(path);
                }
            }
        }
    }
}

/// A file `admin put-back` writes, and what it writes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Restore {
    /// The file.
    pub path: String,
    /// The earlier text.
    pub text: Vec<u8>,
}

/// Whether `path` is one of the account files, the accounts' and the
/// groups' lists, which authd writes together with each account's
/// password, home and sharing password. The journal shows a change to
/// them, in a row of its own, and never puts one back: a list written
/// back alone could take away an account, or an administrator's group,
/// and leave the rest of it behind.
pub fn is_accounts(path: &str) -> bool {
    path == accounts::PASSWD || path == accounts::GROUP
}

/// What putting `change` back writes: the earlier text of each file it
/// changed, in the order the row names them.
///
/// # Errors
///
/// In words, why it cannot be put back: a file it made has no earlier
/// text; a secret, a file not everyone may read, one admind could not
/// read and one longer than the store keeps have none kept; the store
/// keeps only the last 20 versions of a file; and the account files are
/// authd's ([`is_accounts`]).
pub fn restore_plan(store: &Store, change: &Change) -> std::result::Result<Vec<Restore>, String> {
    let mut plan = Vec::new();
    for file in &change.files {
        let path = &file.path;
        if file.before.same_as(&file.after) {
            continue;
        }
        if is_accounts(path) {
            return Err(format!(
                "{path} is one of the account files, which authd keeps together with each \
                 account's password and home, so the console never writes it: change accounts \
                 with `elevate user`, as user(8) says"
            ));
        }
        let hash = match &file.before {
            Seen::Text { sha256, .. } => sha256,
            Seen::Absent => {
                return Err(format!(
                    "{path} was not there before this change, so there is no earlier text to put back"
                ));
            }
            Seen::Large { .. } => {
                return Err(format!(
                    "{path} is longer than 1 MiB, and the console keeps no text of it"
                ));
            }
            Seen::Secret => {
                return Err(format!(
                    "{path} is a secret or a file that not everyone may read, and the console keeps no text of it"
                ));
            }
            Seen::Unreadable => {
                return Err(format!("the console could not read {path}, and keeps no text of it"));
            }
        };
        let kept = store.version_text(hash).map_err(|why| why.to_string())?;
        match kept {
            Some(text) if sha256(&text) == *hash => {
                plan.push(Restore { path: path.clone(), text });
            }
            _ => {
                return Err(format!(
                    "the earlier text of {path} is not kept any more: the console keeps the last 20 versions of a file"
                ));
            }
        }
    }
    if plan.is_empty() {
        return Err("this change changed no file, so there is nothing to put back".into());
    }
    Ok(plan)
}

/// The regular files under the folder `dir`, at most `depth` folders
/// deeper, into `out`, while `room` lasts; gives the folders that could not
/// be opened. Names that begin with a dot are left out (an editor's or a
/// writer's file that is on its way to its name), and so is every link: the folders are opened from the root a part at a time, never
/// through a link, as [`find`] does, so that a link a service's account put
/// in its own folder leads the round nowhere.
fn walk(dir: &Path, depth: usize, out: &mut BTreeSet<String>, room: &mut usize) -> Vec<String> {
    let mut closed = Vec::new();
    match open_folder(dir) {
        Ok(opened) => walk_in(&opened, dir, depth, out, room, &mut closed),
        Err(_) => closed.extend(dir.to_str().map(str::to_string)),
    }
    closed
}

/// The folder at `path`, opened from the root a part at a time and never
/// through a link.
fn open_folder(path: &Path) -> io::Result<Dir> {
    let not_from_root = || io::Error::new(io::ErrorKind::InvalidInput, "not a path from the root");
    let text = path.to_str().ok_or_else(not_from_root)?;
    let parts = text.strip_prefix('/').ok_or_else(not_from_root)?;
    let mut dir = Dir::open("/")?;
    for part in parts.split('/').filter(|part| !part.is_empty()) {
        dir = dir.dir_at(part)?;
    }
    Ok(dir)
}

/// [`walk`] in the open folder `dir`, which is at `path`, gathering the
/// folders that could not be opened in `closed`.
fn walk_in(
    dir: &Dir,
    path: &Path,
    depth: usize,
    out: &mut BTreeSet<String>,
    room: &mut usize,
    closed: &mut Vec<String>,
) {
    let Ok(mut names) = dir.names() else {
        closed.extend(path.to_str().map(str::to_string));
        return;
    };
    // In order, so that which files a full round leaves out is the same
    // from one round to the next.
    names.sort();
    for name in names {
        if name.starts_with('.') {
            continue;
        }
        let Ok(Some(st)) = dir.stat_at(&name) else { continue };
        let inner = path.join(&name);
        match st.st_mode & libc::S_IFMT {
            libc::S_IFDIR if depth > 0 => match dir.dir_at(&name) {
                Ok(sub) => walk_in(&sub, &inner, depth - 1, out, room, closed),
                Err(_) => closed.extend(inner.to_str().map(str::to_string)),
            },
            libc::S_IFREG if *room > 0 => {
                if let Some(text) = inner.to_str() {
                    *room -= 1;
                    out.insert(text.to_string());
                }
            }
            _ => {}
        }
    }
}

mod news;
#[cfg(test)]
mod tests;
