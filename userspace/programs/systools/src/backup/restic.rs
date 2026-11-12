// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Restic repositories as destinations (ADR 0465), through rustic's
//! library: deduplicated, compressed, encrypted with restic's own keys,
//! and read by Linux's `restic` as well as by backup(8).
//!
//! A destination's repository is the machine's folder at it, reached
//! through its store (a folder, as a disk's or a share's, a server over
//! SSH, or S3-compatible storage), and opened with the destination's
//! recovery key as its password. Each round backs up each place's
//! `backup-<time>` snapshot as a restic snapshot of the place's path,
//! tagged with the round's time and the place's folder, and the
//! machine's record (ADR 0464) as a snapshot of [`RECORD`]; then it
//! forgets the snapshots the destination does not keep, and prunes what
//! only they used, unless the repository is append-only.

use std::fs;
use std::io::{Read, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::{Arc, Mutex, PoisonError};

use bytes::Bytes;
use rustic_core::jiff::tz::TimeZone;
use rustic_core::jiff::{Timestamp, Zoned};
use rustic_core::repofile::{SnapshotFile, StringList};
use rustic_core::{
    BackupOptions, BytesList, CheckOptions, ConfigOptions, Credentials, ErrorKind, Excludes,
    FileType, Id, IndexedFullStatus, IndexedIdsStatus, KeepOptions, KeyOptions, LocalDestination,
    LocalSource, LocalSourceFilterOptions, LocalSourceSaveOptions, LsOptions, OpenStatus,
    PruneOptions, ReadBackend, ReadSource, ReadSourceEntry, Repository, RepositoryBackends,
    RepositoryOptions, RestoreOptions, RusticError, RusticResult, SnapshotOptions, WriteBackend,
};

use super::layout;
use super::store::Store;
use crate::util::{self, Fail};

/// The path the machine's record is backed up as.
pub const RECORD: &str = "/dukovo-machine";

/// The record's file in it.
const RECORD_FILE: &str = "record.toml";

/// The folders restic's own `init` makes, which its other tools expect.
pub fn folders() -> Vec<String> {
    let mut folders: Vec<String> =
        ["keys", "snapshots", "index", "locks"].map(String::from).to_vec();
    folders.extend((0..=255u8).map(|b| format!("data/{b:02x}")));
    folders
}

/// The path of a repository's file in its folder, as restic lays them
/// out: `config`, `TYPE/ID`, and packs in `data/` by their first byte.
fn file_path(tpe: FileType, id: &Id) -> String {
    let hex = id.to_hex();
    let hex = hex.as_str();
    match tpe {
        FileType::Config => "config".into(),
        FileType::Pack => format!("data/{}/{hex}", &hex[..2]),
        _ => format!("{}/{hex}", tpe.dirname()),
    }
}

/// A destination's store as rustic's backend. rustic calls it from many
/// threads; the store answers one call at a time.
struct Backend {
    store: Mutex<Box<dyn Store>>,
    place: String,
}

impl Backend {
    fn with<T>(&self, call: impl FnOnce(&mut dyn Store) -> Result<T, Fail>) -> RusticResult<T> {
        let mut store = self.store.lock().unwrap_or_else(PoisonError::into_inner);
        call(store.as_mut()).map_err(|fail| {
            RusticError::new(ErrorKind::Backend, "{message}")
                .attach_context("message", fail.message)
        })
    }
}

impl ReadBackend for Backend {
    fn location(&self) -> String {
        self.place.clone()
    }

    fn list_with_size(&self, tpe: FileType) -> RusticResult<Vec<(Id, u32)>> {
        let clamp = |size: u64| u32::try_from(size).unwrap_or(u32::MAX);
        if tpe == FileType::Config {
            let size = self.with(|store| store.size("config"))?;
            return Ok(size.map(|size| (Id::default(), clamp(size))).into_iter().collect());
        }
        let found = self.with(|store| store.list_sizes(tpe.dirname()))?;
        // Files of other names, as `.partial` ones left by a cut, are not
        // the repository's.
        Ok(found
            .into_iter()
            .filter_map(|(path, size)| {
                let id = Id::from_str(path.rsplit('/').next()?).ok()?;
                (path == file_path(tpe, &id)).then(|| (id, clamp(size)))
            })
            .collect())
    }

    fn read_full(&self, tpe: FileType, id: &Id) -> RusticResult<Bytes> {
        let path = file_path(tpe, id);
        self.with(|store| {
            let mut data = Vec::new();
            store
                .read(&path)?
                .read_to_end(&mut data)
                .map_err(|e| util::failed(&format!("{}/{path}", store.place()), e))?;
            Ok(Bytes::from(data))
        })
    }

    fn read_partial(
        &self,
        tpe: FileType,
        id: &Id,
        _cacheable: bool,
        offset: u32,
        length: u32,
    ) -> RusticResult<Bytes> {
        let path = file_path(tpe, id);
        self.with(|store| store.read_range(&path, offset.into(), length.into())).map(Bytes::from)
    }

    fn warmup_path(&self, tpe: FileType, id: &Id) -> String {
        file_path(tpe, id)
    }
}

impl WriteBackend for Backend {
    fn create(&self) -> RusticResult<()> {
        self.with(|store| store.make_folders(&folders()))
    }

    fn write_bytes(
        &self,
        tpe: FileType,
        id: &Id,
        _cacheable: bool,
        content: BytesList,
    ) -> RusticResult<()> {
        let path = file_path(tpe, id);
        let mut data = Vec::with_capacity(content.size());
        for part in content.slice() {
            data.extend_from_slice(part);
        }
        self.with(|store| store.put_whole(&path, &data))
    }

    fn remove(&self, tpe: FileType, id: &Id, _cacheable: bool) -> RusticResult<()> {
        let path = file_path(tpe, id);
        self.with(|store| store.remove(&[path]))
    }
}

/// A folder as rustic reads it to back it up, its own entry first, named
/// as the last part of the path it is backed up as. rustic's own source
/// leaves out the folder it starts from, and with it the owner, mode,
/// times and extended attributes of a place's top, which a restore needs
/// as much as the rest: this one starts from the folder's parent, with
/// globs that keep the folder's files and leave out the other folders
/// there (the place's other snapshots), which rustic's walk would
/// otherwise go through, as globs of files keep no folder out. An entry
/// of a folder made there since is passed over.
struct Top {
    source: LocalSource,
    top: PathBuf,
    name: String,
}

/// `path` as a glob that matches it alone.
fn glob_of(path: &Path) -> String {
    let mut glob = String::new();
    for ch in path.to_string_lossy().chars() {
        match ch {
            '*' | '?' | '[' | ']' | '{' | '}' | '\\' => {
                glob.push('[');
                glob.push(ch);
                glob.push(']');
            }
            _ => glob.push(ch),
        }
    }
    glob
}

impl Top {
    fn new(top: &Path, name: &str) -> RusticResult<Top> {
        let parent = top.parent().unwrap_or(Path::new("/"));
        let glob = glob_of(top);
        let mut globs = vec![glob.clone(), format!("{glob}/**")];
        if let Ok(entries) = std::fs::read_dir(parent) {
            for entry in entries.flatten() {
                if entry.path() != top {
                    globs.push(format!("!{}", glob_of(&entry.path())));
                }
            }
        }
        let excludes = Excludes::default().globs(globs);
        let source = LocalSource::new(
            LocalSourceSaveOptions::default(),
            &excludes,
            &LocalSourceFilterOptions::default(),
            &[parent],
        )?;
        Ok(Top { source, top: top.to_path_buf(), name: name.into() })
    }
}

/// [`Top`]'s entries.
struct TopEntries {
    inner: <LocalSource as ReadSource>::Iter,
    top: PathBuf,
    name: String,
}

impl Iterator for TopEntries {
    type Item = RusticResult<ReadSourceEntry<<LocalSource as ReadSource>::Open>>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            let item = match self.inner.next()? {
                Ok(entry) if !entry.path.starts_with(&self.top) => continue,
                item => item,
            };
            return Some(item.map(|mut entry| {
                if entry.path == self.top {
                    entry.node.name.clone_from(&self.name);
                }
                entry
            }));
        }
    }
}

impl ReadSource for Top {
    type Open = <LocalSource as ReadSource>::Open;
    type Iter = TopEntries;

    fn size(&self) -> RusticResult<Option<u64>> {
        Ok(None)
    }

    fn entries(&self) -> TopEntries {
        TopEntries { inner: self.source.entries(), top: self.top.clone(), name: self.name.clone() }
    }
}

/// What rustic said, as a message: its words, without its advice on
/// filing reports.
fn failed(place: &str, error: &RusticError) -> Fail {
    if error.is_incorrect_password() {
        return util::fail(format!("{place}: the key given does not open its repository"));
    }
    let text = error.display_log();
    let text = text.strip_prefix("Error: ").unwrap_or(&text);
    let mut text = text.split(" (kind: ").next().unwrap_or(text).trim().to_string();
    if let Some(source) = std::error::Error::source(error) {
        text = format!("{text}: {source}");
    }
    util::fail(format!("{place}: {text}"))
}

/// The time `ns` since 1970 as rustic keeps times, in UTC.
fn zoned(ns: u64) -> Result<Zoned, Fail> {
    Timestamp::from_nanosecond(i128::from(ns))
        .map(|time| time.to_zoned(TimeZone::UTC))
        .map_err(|e| util::fail(format!("the time {ns}: {e}")))
}

/// How many backups a destination keeps besides its newest: the newest
/// of each of so many days, weeks and months.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Keep {
    pub daily: u32,
    pub weekly: u32,
    pub monthly: u32,
}

impl Default for Keep {
    fn default() -> Keep {
        Keep { daily: 7, weekly: 4, monthly: 12 }
    }
}

/// A snapshot backup(8) made: of a place, or of the machine's record.
#[derive(Clone, Debug)]
pub struct Snapshot {
    /// The time of its round, as backups are named.
    pub time: String,
    /// The place's path, or [`RECORD`].
    pub path: String,
    /// The place's folder name, or the records' (`machine`).
    pub folder: String,
    pub host: String,
    file: SnapshotFile,
}

impl Snapshot {
    /// The snapshot `file` is, if backup(8) made it: tagged with a time
    /// and a folder, of one path.
    fn of(file: SnapshotFile) -> Option<Snapshot> {
        let tags: Vec<&String> = file.tags.iter().collect();
        let time = String::clone(tags.iter().find(|t| layout::is_time(t))?);
        let folder = String::clone(tags.iter().find(|t| !layout::is_time(t))?);
        let paths: Vec<&String> = file.paths.iter().collect();
        let [path] = paths.as_slice() else { return None };
        Some(Snapshot {
            time,
            path: String::clone(path),
            folder,
            host: file.hostname.clone(),
            file,
        })
    }

    /// Whether it is of the machine's record.
    pub fn is_record(&self) -> bool {
        self.path == RECORD && self.folder == layout::RECORDS
    }
}

/// What a backup added: bytes new to the repository, of how many in how
/// many files.
#[derive(Clone, Copy, Debug, Default)]
pub struct Made {
    pub added: u64,
    pub read: u64,
    pub files: u64,
}

/// An open repository. rustic indexes it by value, so it is taken out
/// for that and put back after; it stays out if the index cannot be
/// read.
pub struct Repo {
    repo: Option<Repository<OpenStatus>>,
    place: String,
}

/// The repository of `store`, not yet opened.
fn repository(store: Box<dyn Store>) -> Result<(Repository<()>, String), Fail> {
    let place = store.place();
    let backend: Arc<dyn WriteBackend> =
        Arc::new(Backend { store: Mutex::new(store), place: place.clone() });
    let backends = RepositoryBackends::new(backend, None);
    // Nothing is kept on the machine: each round reads what it needs.
    let options = RepositoryOptions::default().no_cache(true);
    let repo = Repository::new(&options, &backends).map_err(|e| failed(&place, &e))?;
    Ok((repo, place))
}

impl Repo {
    /// Whether `store` holds a repository.
    pub fn exists(store: &mut dyn Store) -> Result<bool, Fail> {
        Ok(store.size("config")?.is_some())
    }

    /// Makes a repository in `store`, opened with `password`, and
    /// append-only if `append_only`.
    pub fn init(store: Box<dyn Store>, password: &str, append_only: bool) -> Result<Repo, Fail> {
        let (repo, place) = repository(store)?;
        let config = ConfigOptions::default().set_append_only(append_only.then_some(true));
        let repo = repo
            .init(&Credentials::password(password), &KeyOptions::default(), &config)
            .map_err(|e| failed(&place, &e))?;
        Ok(Repo { repo: Some(repo), place })
    }

    /// Opens the repository in `store` with `password`.
    pub fn open(store: Box<dyn Store>, password: &str) -> Result<Repo, Fail> {
        let (repo, place) = repository(store)?;
        let repo = repo.open(&Credentials::password(password)).map_err(|e| failed(&place, &e))?;
        Ok(Repo { repo: Some(repo), place })
    }

    pub fn place(&self) -> &str {
        &self.place
    }

    fn failed(&self, error: &RusticError) -> Fail {
        failed(&self.place, error)
    }

    fn opened(&self) -> Result<&Repository<OpenStatus>, Fail> {
        self.repo
            .as_ref()
            .ok_or_else(|| util::fail(format!("{}: its index could not be read", self.place)))
    }

    /// Runs `work` on the repository indexed by its blobs' ids, as a
    /// backup needs it.
    fn with_ids<T>(
        &mut self,
        work: impl FnOnce(&Repository<IndexedIdsStatus>) -> RusticResult<T>,
    ) -> Result<T, Fail> {
        self.opened()?;
        let open = self.repo.take().ok_or_else(|| util::fail(self.place.clone()))?;
        let indexed = open.to_indexed_ids().map_err(|e| self.failed(&e))?;
        let done = work(&indexed);
        self.repo = Some(indexed.drop_index());
        done.map_err(|e| self.failed(&e))
    }

    /// Runs `work` on the repository fully indexed, as a restore needs it.
    fn with_index<T>(
        &mut self,
        work: impl FnOnce(&Repository<IndexedFullStatus>) -> RusticResult<T>,
    ) -> Result<T, Fail> {
        self.opened()?;
        let open = self.repo.take().ok_or_else(|| util::fail(self.place.clone()))?;
        let indexed = open.to_indexed().map_err(|e| self.failed(&e))?;
        let done = work(&indexed);
        self.repo = Some(indexed.drop_index());
        done.map_err(|e| self.failed(&e))
    }

    /// Whether nothing may be removed from it.
    pub fn append_only(&self) -> bool {
        self.repo.as_ref().is_some_and(|repo| repo.config().append_only == Some(true))
    }

    /// The snapshots backup(8) made of machine `machine`, or of any
    /// machine with none, oldest first.
    pub fn snapshots(&self, machine: Option<&str>) -> Result<Vec<Snapshot>, Fail> {
        let files = self.opened()?.get_all_snapshots().map_err(|e| self.failed(&e))?;
        let mut found: Vec<Snapshot> = files
            .into_iter()
            .filter_map(Snapshot::of)
            .filter(|s| machine.is_none_or(|m| s.host == m))
            .collect();
        found.sort_by(|a, b| (&a.time, &a.path).cmp(&(&b.time, &b.path)));
        Ok(found)
    }

    /// Backs up the folder `from` as the path `path` (a place's, or
    /// [`RECORD`]), with the folder name `folder`, for machine `machine`
    /// at the round of time `time`, `ns` since 1970.
    pub fn back_up(
        &mut self,
        from: &Path,
        path: &str,
        folder: &str,
        machine: &str,
        time: &str,
        ns: u64,
    ) -> Result<Made, Fail> {
        let tags = StringList::from_str(&format!("{time},{folder}"))
            .map_err(|e| util::fail(format!("{time}, {folder}: {e}")))?;
        let options = SnapshotOptions::default()
            .host(machine.to_string())
            .time(zoned(ns)?)
            .tags(vec![tags])
            .command("backup run".to_string());
        let snap = SnapshotFile::from_options(&options).map_err(|e| self.failed(&e))?;
        let backup = BackupOptions::default().as_path(PathBuf::from(path)).no_scan(true);
        let name = Path::new(path).file_name().and_then(|n| n.to_str()).unwrap_or("");
        let source = Top::new(from, name).map_err(|e| self.failed(&e))?;
        // Indexed afresh for each, so that what one backup added is not
        // added again by the next.
        let made =
            self.with_ids(|repo| repo.archive(&backup, &source, snap, &[from.to_path_buf()]))?;
        let summary = made.summary.unwrap_or_default();
        Ok(Made {
            added: summary.data_added,
            read: summary.total_bytes_processed,
            files: summary.total_files_processed,
        })
    }

    /// Backs up the machine's record `text` of the round of time `time`,
    /// written for that in the folder `scratch`, made and removed.
    pub fn back_up_record(
        &mut self,
        text: &str,
        scratch: &Path,
        machine: &str,
        time: &str,
        ns: u64,
    ) -> Result<Made, Fail> {
        let _ = fs::remove_dir_all(scratch);
        fs::DirBuilder::new()
            .mode(0o700)
            .create(scratch)
            .map_err(|e| util::failed(&scratch.display(), e))?;
        let file = scratch.join(RECORD_FILE);
        let written = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&file)
            .and_then(|mut f| f.write_all(text.as_bytes()))
            .map_err(|e| util::failed(&file.display(), e));
        let made = written
            .and_then(|()| self.back_up(scratch, RECORD, layout::RECORDS, machine, time, ns));
        let _ = fs::remove_dir_all(scratch);
        made
    }

    /// Forgets the snapshots of machine `machine` that `keep` does not
    /// keep, each place's and the records' alike, the newest of each
    /// always kept, and prunes the data only they used. How many it
    /// forgot.
    pub fn forget(&self, keep: Keep, machine: &str, now: u64) -> Result<usize, Fail> {
        let count = |n: u32| i32::try_from(n).unwrap_or(i32::MAX);
        let options = KeepOptions::default()
            .keep_last(1)
            .keep_daily(count(keep.daily))
            .keep_weekly(count(keep.weekly))
            .keep_monthly(count(keep.monthly));
        let now = zoned(now)?;
        let mut groups: std::collections::BTreeMap<String, Vec<SnapshotFile>> =
            std::collections::BTreeMap::new();
        for snapshot in self.snapshots(Some(machine))? {
            groups.entry(snapshot.path).or_default().push(snapshot.file);
        }
        let mut going = Vec::new();
        for (_, group) in groups {
            for decided in options.apply(group, &now).map_err(|e| self.failed(&e))? {
                if !decided.keep {
                    going.push(decided.snapshot.id);
                }
            }
        }
        if going.is_empty() {
            return Ok(0);
        }
        let repo = self.opened()?;
        repo.delete_snapshots(&going).map_err(|e| self.failed(&e))?;
        // The machine alone writes its repository, and the lock of
        // backup(8) keeps its rounds apart: packs no snapshot uses go at
        // once, which leaves the repository as Linux's restic expects.
        let prune = PruneOptions::default().instant_delete(true);
        let plan = repo.prune_plan(&prune).map_err(|e| self.failed(&e))?;
        repo.prune(&prune, plan).map_err(|e| self.failed(&e))?;
        Ok(going.len())
    }

    /// Checks the repository: its index, packs and snapshots' trees, and
    /// with `read_data` every pack's data, decrypted and hashed.
    pub fn check(&self, read_data: bool) -> Result<(), Fail> {
        let results = self
            .opened()?
            .check(CheckOptions::default().read_data(read_data))
            .map_err(|e| self.failed(&e))?;
        if results.is_ok().is_ok() {
            return Ok(());
        }
        let found: Vec<String> = results.0.iter().take(3).map(|(_, e)| e.to_string()).collect();
        Err(util::fail(format!("{}: {}", self.place, found.join("; "))))
    }

    /// Restores `snapshot` into the folder `into`, which is there and
    /// empty: its files, folders and links with their owners, modes,
    /// times and extended attributes, and the folder's own.
    pub fn restore(&mut self, snapshot: &Snapshot, into: &Path) -> Result<(), Fail> {
        let dest = LocalDestination::new(&format!("{}/", into.display()), true, false)
            .map_err(|e| self.failed(&e))?;
        self.with_index(|repo| {
            let node = repo.node_from_snapshot_and_path(&snapshot.file, &snapshot.path)?;
            // The folder's own entry first, whose owner, mode and times
            // are set last, after what it holds.
            let nodes = || -> RusticResult<_> {
                let top = std::iter::once(Ok((PathBuf::new(), node.clone())));
                Ok(top.chain(repo.ls(&node, &LsOptions::default())?))
            };
            let options = RestoreOptions::default().numeric_id(true);
            let plan = repo.prepare_restore(&options, nodes()?, &dest, false)?;
            repo.restore(plan, &options, nodes()?, &dest)
        })
    }

    /// The machine's record that `snapshot` holds.
    pub fn read_record(&mut self, snapshot: &Snapshot) -> Result<String, Fail> {
        let path = format!("{RECORD}/{RECORD_FILE}");
        let data = self.with_index(|repo| {
            let node = repo.node_from_snapshot_and_path(&snapshot.file, &path)?;
            let mut data = Vec::new();
            repo.dump(&node, &mut data)?;
            Ok(data)
        })?;
        String::from_utf8(data).map_err(|_| {
            util::fail(format!("{}: {path} of {}: not text", self.place, snapshot.time))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::super::store::Folder;
    use super::*;

    /// A folder of its own, removed when it goes.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str) -> Scratch {
            let path = std::env::temp_dir().join(format!("restic-{name}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&path);
            fs::create_dir_all(&path).unwrap();
            Scratch(path)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    const KEY: &str =
        "AGE-SECRET-KEY-1QQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQS3QDY0";
    const DAY: u64 = 86_400 * 1_000_000_000;

    fn store(at: &Path) -> Box<dyn Store> {
        Box::new(Folder::new(at, Some("atlas")).unwrap())
    }

    #[test]
    fn files_are_where_restic_puts_them() {
        let id = Id::from_str(&"ab".repeat(32)).unwrap();
        assert_eq!(file_path(FileType::Config, &Id::default()), "config");
        assert_eq!(file_path(FileType::Pack, &id), format!("data/ab/{}", "ab".repeat(32)));
        assert_eq!(file_path(FileType::Snapshot, &id), format!("snapshots/{}", "ab".repeat(32)));
        assert_eq!(file_path(FileType::Key, &id), format!("keys/{}", "ab".repeat(32)));
        assert_eq!(folders().len(), 4 + 256);
    }

    #[test]
    fn a_place_is_backed_up_restored_kept_and_checked() {
        let disk = Scratch::new("disk");
        let place = Scratch::new("place");
        let back = Scratch::new("back");
        let source = place.0.join("backup-1");
        fs::create_dir_all(source.join("notes")).unwrap();
        // The place's other snapshots, beside it, are not backed up.
        fs::create_dir_all(place.0.join("2026-05-27-0300/deep")).unwrap();
        fs::write(place.0.join("2026-05-27-0300/deep/old"), b"old\n").unwrap();
        fs::create_dir_all(place.0.join("backup-10")).unwrap();
        fs::write(place.0.join("backup-10/other"), b"other\n").unwrap();
        fs::write(source.join("notes/today"), b"the rabbit was late\n").unwrap();
        fs::write(source.join("big"), vec![7u8; 3 << 20]).unwrap();
        std::os::unix::fs::symlink("notes/today", source.join("link")).unwrap();

        let mut probe = Folder::new(&disk.0, Some("atlas")).unwrap();
        assert!(!Repo::exists(&mut probe).unwrap());
        let mut repo = Repo::init(store(&disk.0), KEY, false).unwrap();
        assert!(Repo::exists(&mut probe).unwrap());
        assert!(disk.0.join("dukovo-backup/atlas/data/00").is_dir());
        assert!(!repo.append_only());

        let start = 1_780_000_000 * 1_000_000_000;
        let first =
            repo.back_up(&source, "/users", "users", "atlas", "2026-05-28-120000", start).unwrap();
        assert!(first.added > 0 && first.files == 3, "{first:?}");
        // The same files again add nearly nothing.
        let second = repo
            .back_up(&source, "/users", "users", "atlas", "2026-05-29-120000", start + DAY)
            .unwrap();
        assert!(second.added < 4096, "{second:?}");
        repo.back_up_record(
            "machine = \"atlas\"\n",
            &back.0.join("record"),
            "atlas",
            "2026-05-29-120000",
            start + DAY,
        )
        .unwrap();
        assert!(!back.0.join("record").exists());

        let mut repo = Repo::open(store(&disk.0), KEY).unwrap();
        let snapshots = repo.snapshots(Some("atlas")).unwrap();
        let times: Vec<(&str, &str)> =
            snapshots.iter().map(|s| (s.time.as_str(), s.path.as_str())).collect();
        assert_eq!(
            times,
            [
                ("2026-05-28-120000", "/users"),
                ("2026-05-29-120000", RECORD),
                ("2026-05-29-120000", "/users")
            ]
        );
        assert!(snapshots[1].is_record() && !snapshots[0].is_record());
        assert_eq!(repo.read_record(&snapshots[1]).unwrap(), "machine = \"atlas\"\n");
        assert!(repo.snapshots(Some("other")).unwrap().is_empty());

        let into = back.0.join("restored");
        fs::create_dir(&into).unwrap();
        repo.restore(&snapshots[2], &into).unwrap();
        assert_eq!(fs::read(into.join("notes/today")).unwrap(), b"the rabbit was late\n");
        assert_eq!(fs::read_link(into.join("link")).unwrap(), Path::new("notes/today"));
        super::super::compare::same(&into, &source).unwrap();

        // A third day: keeping one a day for two days forgets the first.
        repo.back_up(&source, "/users", "users", "atlas", "2026-05-30-120000", start + 2 * DAY)
            .unwrap();
        let keep = Keep { daily: 2, weekly: 0, monthly: 0 };
        assert_eq!(repo.forget(keep, "atlas", start + 2 * DAY).unwrap(), 1);
        let left: Vec<String> = repo
            .snapshots(None)
            .unwrap()
            .into_iter()
            .filter(|s| !s.is_record())
            .map(|s| s.time)
            .collect();
        assert_eq!(left, ["2026-05-29-120000", "2026-05-30-120000"]);
        repo.check(true).unwrap();

        // A pack changed is found.
        let packs = Folder::new(&disk.0, Some("atlas")).unwrap().list_sizes("data").unwrap();
        let (pack, _) = packs.iter().max_by_key(|(_, size)| *size).unwrap();
        let path = disk.0.join("dukovo-backup/atlas").join(pack);
        let mut data = fs::read(&path).unwrap();
        data[100] ^= 1;
        fs::write(&path, data).unwrap();
        assert!(repo.check(true).is_err());
    }

    #[test]
    fn another_key_does_not_open_it_and_append_only_is_kept() {
        let disk = Scratch::new("keys");
        let repo = Repo::init(store(&disk.0), KEY, true).unwrap();
        assert!(repo.append_only());
        let other = KEY.replace("S3QDY0", "S3QDY1");
        let fail = Repo::open(store(&disk.0), &other).err().unwrap();
        assert!(
            fail.message.ends_with("the key given does not open its repository"),
            "{}",
            fail.message
        );
        assert!(Repo::open(store(&disk.0), KEY).unwrap().append_only());
    }
}
