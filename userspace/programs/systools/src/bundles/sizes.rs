// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! What an app's data takes on disk (ADR 0565), measured once a day.
//!
//! bundled(8) starts `bundled measure` in a process of its own, which
//! puts its disk requests in the idle class and its nice level at 19, and
//! walks each app's data folders as root without reading a file or keeping
//! a name: it counts what the folders cost on disk, in blocks, and how
//! many files are in them. The walk is bounded in each way a folder can be
//! made to take long: the entries of one app, the time of one app, the
//! time of all, the depth, the folders waiting to be walked, and a
//! mount inside a folder is not entered. bundled keeps the answer in
//! `sizes.json`, beside its registry and not one of its files, which
//! `bundle info` reads.

use std::collections::HashSet;
use std::fs;
use std::io::{self, BufRead, BufReader, Write};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

/// The file, in `/system/state/bundled`.
pub const FILE: &str = "sizes.json";
/// The program that walks, which is bundled's own.
pub const PROGRAM: &str = "/system/commands/bundled";
/// What `bundled` is given to walk.
pub const MODE: &str = "measure";
/// The most time one app's folders take.
pub const APP_TIME: Duration = Duration::from_mins(10);
/// The most entries one app's folders are walked for.
pub const APP_ENTRIES: u64 = 5_000_000;
/// The most time a day's walk takes before it starts no more apps.
pub const DAY_TIME: Duration = Duration::from_mins(60);
/// When bundled ends a walk that is still going.
pub const KILL_AFTER: Duration = Duration::from_mins(70);
/// The most folders deep a walk goes, the first one counted.
pub const DEPTH_MAX: usize = 4096;
/// The most folders that wait to be walked at once.
const WAITING_MAX: usize = 100_000;
/// How many entries pass between two looks at the clock.
const LOOK_EVERY: u64 = 1024;
/// How a disk counts a block, in bytes, in `st_blocks`.
const BLOCK: u64 = 512;

/// What an entry of a folder is, as far as the walk cares.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// A folder.
    Folder,
    /// A file, a link or anything else: counted, never followed.
    Other,
}

/// An entry as the walk sees it: its metadata alone.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub path: PathBuf,
    pub kind: Kind,
    /// The device and number that tell a file from another, and how many
    /// names it has.
    pub device: u64,
    pub inode: u64,
    pub links: u64,
    /// What it costs on disk.
    pub bytes: u64,
}

/// Where the walk finds its entries: a disk, or a planted tree.
pub trait Tree {
    /// The entry at `path`, itself and not what a link leads to.
    ///
    /// # Errors
    ///
    /// If there is none, or it cannot be read.
    fn stat(&mut self, path: &Path) -> io::Result<Entry>;

    /// Calls `visit` with each entry of folder `path`, until it says no.
    ///
    /// # Errors
    ///
    /// If the folder cannot be read.
    fn each(&mut self, path: &Path, visit: &mut dyn FnMut(Entry) -> bool) -> io::Result<()>;
}

/// The disk, read as the walk's own process may.
pub struct Disk;

impl Disk {
    fn entry(path: PathBuf, meta: &fs::Metadata) -> Entry {
        Entry {
            kind: if meta.is_dir() { Kind::Folder } else { Kind::Other },
            device: meta.dev(),
            inode: meta.ino(),
            links: meta.nlink(),
            bytes: meta.blocks().saturating_mul(BLOCK),
            path,
        }
    }
}

impl Tree for Disk {
    fn stat(&mut self, path: &Path) -> io::Result<Entry> {
        let meta = fs::symlink_metadata(path)?;
        Ok(Disk::entry(path.to_path_buf(), &meta))
    }

    fn each(&mut self, path: &Path, visit: &mut dyn FnMut(Entry) -> bool) -> io::Result<()> {
        for found in fs::read_dir(path)? {
            let found = found?;
            let Ok(meta) = fs::symlink_metadata(found.path()) else { continue };
            if !visit(Disk::entry(found.path(), &meta)) {
                break;
            }
        }
        Ok(())
    }
}

/// What one app's folders may take, and what is left of it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Budget {
    /// The entries still to be counted.
    pub entries: u64,
    /// When the app's time ends, as the clock reads then.
    pub until: Duration,
}

/// What a walk found of a folder.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Walked {
    /// What it costs on disk, in bytes.
    pub bytes: u64,
    /// The files in it, with the links and the rest that are not folders.
    pub files: u64,
    /// False when a bound stopped the walk, so that this is at least.
    pub complete: bool,
}

/// The walk of the folder `root`: the folder and what is below it, on its
/// own device. `seen` holds the files with more than one name that were
/// counted, so that each counts once, whichever folder it is met in.
///
/// # Errors
///
/// If `root` cannot be read at all.
pub fn walk(
    tree: &mut dyn Tree,
    root: &Path,
    budget: &mut Budget,
    seen: &mut HashSet<(u64, u64)>,
    clock: &dyn Fn() -> Duration,
) -> io::Result<Walked> {
    let top = tree.stat(root)?;
    let mut found = Walked { bytes: top.bytes, files: 0, complete: true };
    if top.kind == Kind::Other {
        found.files = 1;
        return Ok(found);
    }
    let mut waiting = vec![(root.to_path_buf(), 1usize)];
    let mut passed = 0u64;
    while let Some((folder, depth)) = waiting.pop() {
        if clock() >= budget.until {
            found.complete = false;
            break;
        }
        let read = tree.each(&folder, &mut |entry| {
            if budget.entries == 0 {
                found.complete = false;
                return false;
            }
            budget.entries -= 1;
            passed += 1;
            if passed.is_multiple_of(LOOK_EVERY) && clock() >= budget.until {
                found.complete = false;
                return false;
            }
            if entry.kind == Kind::Folder {
                // A mount inside is another disk's, not this folder's.
                if entry.device != top.device {
                    return true;
                }
                found.bytes = found.bytes.saturating_add(entry.bytes);
                if depth >= DEPTH_MAX || waiting.len() >= WAITING_MAX {
                    found.complete = false;
                } else {
                    waiting.push((entry.path, depth + 1));
                }
            } else if entry.links <= 1 || seen.insert((entry.device, entry.inode)) {
                found.bytes = found.bytes.saturating_add(entry.bytes);
                found.files += 1;
            }
            true
        });
        // A folder that cannot be read leaves the total at least.
        if read.is_err() {
            found.complete = false;
        }
        if !found.complete && (budget.entries == 0 || clock() >= budget.until) {
            break;
        }
    }
    Ok(found)
}

/// An app and the data folders of its services.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Request {
    pub id: String,
    pub folders: Vec<String>,
}

/// What the walk's process is given to walk: each folder after its app's
/// ID, the folders of an app one after another.
#[must_use]
pub fn arguments(requests: &[Request]) -> Vec<String> {
    let mut words = vec![MODE.to_string()];
    for request in requests {
        for folder in &request.folders {
            words.push(request.id.clone());
            words.push(folder.clone());
        }
    }
    words
}

/// Reads what [`arguments`] wrote, without its mode word: an app's folders
/// come together, in the order they were given.
///
/// # Errors
///
/// If a folder has no app before it.
pub fn read_arguments(words: &[String]) -> Result<Vec<Request>, String> {
    if !words.len().is_multiple_of(2) {
        return Err("each folder comes after the ID of its app".into());
    }
    let mut requests: Vec<Request> = Vec::new();
    for pair in words.chunks(2) {
        let [id, folder] = pair else { continue };
        match requests.last_mut() {
            Some(last) if last.id == *id => last.folders.push(folder.clone()),
            _ => requests.push(Request { id: id.clone(), folders: vec![folder.clone()] }),
        }
    }
    Ok(requests)
}

/// What the walk's process says: a line each, tab-separated, `\t` and
/// newlines never in a word.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Line {
    /// A folder of an app was walked.
    Folder { id: String, path: String, walked: Walked },
    /// An app is done: its total, and how long it took, in milliseconds.
    App { id: String, walked: Walked, took_ms: u64 },
    /// The walk cannot be done, and why.
    Error(String),
}

impl Line {
    /// The line, with its newline.
    #[must_use]
    pub fn text(&self) -> String {
        let done = |w: &Walked| format!("{}\t{}\t{}", w.bytes, w.files, u8::from(w.complete));
        match self {
            Line::Folder { id, path, walked } => {
                format!("folder\t{id}\t{path}\t{}\n", done(walked))
            }
            Line::App { id, walked, took_ms } => {
                format!("app\t{id}\t{}\t{took_ms}\n", done(walked))
            }
            Line::Error(why) => {
                let why: String =
                    why.chars().map(|c| if c.is_control() { ' ' } else { c }).collect();
                format!("error\t{why}\n")
            }
        }
    }

    /// Reads a line, with or without its newline.
    ///
    /// # Errors
    ///
    /// If it is none of the walk's.
    pub fn read(line: &str) -> Result<Line, String> {
        let words: Vec<&str> = line.trim_end_matches('\n').split('\t').collect();
        let number = |word: &str| word.parse::<u64>().map_err(|_| format!("{word:?} is no number"));
        let walked = |bytes: &str, files: &str, complete: &str| -> Result<Walked, String> {
            Ok(Walked {
                bytes: number(bytes)?,
                files: number(files)?,
                complete: match complete {
                    "1" => true,
                    "0" => false,
                    _ => return Err(format!("{complete:?} is neither 1 nor 0")),
                },
            })
        };
        match words.as_slice() {
            ["folder", id, path, bytes, files, complete] => Ok(Line::Folder {
                id: (*id).into(),
                path: (*path).into(),
                walked: walked(bytes, files, complete)?,
            }),
            ["app", id, bytes, files, complete, took] => Ok(Line::App {
                id: (*id).into(),
                walked: walked(bytes, files, complete)?,
                took_ms: number(took)?,
            }),
            ["error", why] => Ok(Line::Error((*why).into())),
            _ => Err("not a line the walk says".into()),
        }
    }
}

/// Walks what `requests` name, app by app, saying each folder and each app
/// as it is done to `out`, until `DAY_TIME` has passed: then it starts no
/// more apps. A folder that is not there is left out.
///
/// # Errors
///
/// If `out` cannot be written to.
pub fn run(
    tree: &mut dyn Tree,
    requests: &[Request],
    clock: &dyn Fn() -> Duration,
    out: &mut dyn Write,
) -> io::Result<()> {
    let mut seen = HashSet::new();
    for request in requests {
        let began = clock();
        if began >= DAY_TIME {
            break;
        }
        let mut budget = Budget { entries: APP_ENTRIES, until: began + APP_TIME };
        let mut total = Walked { bytes: 0, files: 0, complete: true };
        let mut any = false;
        for folder in &request.folders {
            let walked = match walk(tree, Path::new(folder), &mut budget, &mut seen, clock) {
                Ok(walked) => walked,
                Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
                Err(_) => Walked { bytes: 0, files: 0, complete: false },
            };
            any = true;
            total.bytes = total.bytes.saturating_add(walked.bytes);
            total.files += walked.files;
            total.complete &= walked.complete;
            let line = Line::Folder { id: request.id.clone(), path: folder.clone(), walked };
            out.write_all(line.text().as_bytes())?;
        }
        if any {
            let took_ms = u64::try_from((clock() - began).as_millis()).unwrap_or(u64::MAX);
            let line = Line::App { id: request.id.clone(), walked: total, took_ms };
            out.write_all(line.text().as_bytes())?;
            out.flush()?;
        }
    }
    Ok(())
}

/// Puts this process's disk requests in the idle class and its nice level
/// at 19, as work that can wait does.
///
/// # Errors
///
/// Which of the two the kernel did not do.
pub fn lower_priority() -> Result<(), String> {
    if !crate::priority::idle_io() {
        return Err("the kernel did not put the walk's disk requests in the idle class".into());
    }
    if let Err(error) = sysapi::process::set_priority(sysapi::process::Who::Process(0), 19) {
        return Err(format!("the walk's nice level was not set: {error}"));
    }
    Ok(())
}

/// `bundled measure ID FOLDER...`: the walk, in a process of its own.
#[must_use]
pub fn child(words: &[String]) -> i32 {
    let mut out = io::stdout().lock();
    let said = |out: &mut dyn Write, line: &Line| {
        let _ = out.write_all(line.text().as_bytes());
        let _ = out.flush();
    };
    let requests = match read_arguments(words) {
        Ok(requests) => requests,
        Err(why) => {
            said(&mut out, &Line::Error(why));
            return 2;
        }
    };
    // A walk at the usual priority is not worth it: nothing is measured.
    if let Err(why) = lower_priority() {
        said(&mut out, &Line::Error(why));
        return 1;
    }
    let started = Instant::now();
    match run(&mut Disk, &requests, &|| started.elapsed(), &mut out) {
        Ok(()) => 0,
        Err(_) => 1,
    }
}

/// Starts [`PROGRAM`]'s walk of `requests`, reads what it says, and ends
/// it when `kill_after` has passed: every line it said until then.
///
/// # Errors
///
/// If the process cannot be started.
pub fn measure(
    program: &str,
    requests: &[Request],
    kill_after: Duration,
) -> Result<Vec<Line>, String> {
    let mut process = Command::new(program)
        .args(arguments(requests))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .map_err(|e| format!("{program}: {e}"))?;
    let Some(stdout) = process.stdout.take() else {
        let _ = process.kill();
        let _ = process.wait();
        return Err("the walk has no output".into());
    };
    let (send, receive) = mpsc::channel();
    let reader = std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if send.send(line).is_err() {
                break;
            }
        }
    });
    let deadline = Instant::now() + kill_after;
    let mut said = Vec::new();
    // The reader ends with the process's output; one that is ended here is
    // not waited for, as nothing it could still say is wanted.
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        match receive.recv_timeout(left) {
            Ok(line) => {
                if let Ok(line) = Line::read(&line) {
                    said.push(line);
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                let _ = process.kill();
                said.push(Line::Error(format!(
                    "the walk was ended after {} minutes",
                    kill_after.as_secs() / 60
                )));
                break;
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
    let _ = process.wait();
    drop(reader);
    Ok(said)
}

/// A folder of an app, as `sizes.json` keeps it.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct FolderSize {
    pub path: String,
    pub bytes: u64,
    pub files: u64,
    pub complete: bool,
}

/// An app's data, as `sizes.json` keeps it.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct AppSize {
    pub id: String,
    pub bytes: u64,
    pub files: u64,
    pub complete: bool,
    pub measured_ms: u64,
    pub took_ms: u64,
    pub folders: Vec<FolderSize>,
}

/// What `bundle info --json` says of it: `data_size`.
#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
pub struct DataSize {
    pub bytes: u64,
    pub files: u64,
    pub complete: bool,
    pub measured_ms: u64,
}

/// `sizes.json`.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct Sizes {
    /// 1.
    pub format: u32,
    pub apps: Vec<AppSize>,
}

impl Sizes {
    /// Reads the file's text; what cannot be read is no sizes, which the
    /// next walk makes anew.
    #[must_use]
    pub fn read(text: &str) -> Sizes {
        match serde_json::from_str::<Sizes>(text) {
            Ok(sizes) if sizes.format == 1 => sizes,
            _ => Sizes { format: 1, apps: Vec::new() },
        }
    }

    /// The file's text.
    #[must_use]
    pub fn text(&self) -> String {
        serde_json::to_string(self).unwrap_or_else(|_| String::from("{\"format\":1,\"apps\":[]}"))
    }

    /// What is kept of app `id`'s data.
    #[must_use]
    pub fn data_size(&self, id: &str) -> Option<DataSize> {
        let app = self.apps.iter().find(|app| app.id == id)?;
        Some(DataSize {
            bytes: app.bytes,
            files: app.files,
            complete: app.complete,
            measured_ms: app.measured_ms,
        })
    }

    /// Takes in what a walk said, at `now_ms`: each app it finished
    /// replaces what was kept of it, and the apps that are not among
    /// `installed` are dropped. An app it did not reach keeps its old
    /// size. Whether anything changed.
    pub fn apply(&mut self, lines: &[Line], installed: &[String], now_ms: u64) -> bool {
        let before = self.clone();
        self.format = 1;
        for line in lines {
            let Line::App { id, walked, took_ms } = line else { continue };
            let folders: Vec<FolderSize> = lines
                .iter()
                .filter_map(|line| match line {
                    Line::Folder { id: of, path, walked } if of == id => Some(FolderSize {
                        path: path.clone(),
                        bytes: walked.bytes,
                        files: walked.files,
                        complete: walked.complete,
                    }),
                    _ => None,
                })
                .collect();
            let app = AppSize {
                id: id.clone(),
                bytes: walked.bytes,
                files: walked.files,
                complete: walked.complete,
                measured_ms: now_ms,
                took_ms: *took_ms,
                folders,
            };
            match self.apps.iter_mut().find(|kept| kept.id == *id) {
                Some(kept) => *kept = app,
                None => self.apps.push(app),
            }
        }
        self.apps.retain(|app| installed.contains(&app.id));
        self.apps.sort_by(|a, b| a.id.cmp(&b.id));
        *self != before
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::collections::HashMap;

    use super::*;

    /// A tree that is planted: each folder by its path, and what it holds.
    #[derive(Default)]
    struct Planted {
        folders: HashMap<PathBuf, Vec<Entry>>,
        entries: HashMap<PathBuf, Entry>,
    }

    impl Planted {
        fn add(&mut self, parent: &str, name: &str, kind: Kind, device: u64, bytes: u64) -> Entry {
            let path = Path::new(parent).join(name);
            let inode = u64::try_from(self.entries.len()).unwrap_or(0) + 100;
            let entry = Entry { path: path.clone(), kind, device, inode, links: 1, bytes };
            self.entries.insert(path.clone(), entry.clone());
            self.folders.entry(PathBuf::from(parent)).or_default().push(entry.clone());
            if kind == Kind::Folder {
                self.folders.entry(path).or_default();
            }
            entry
        }

        fn root(&mut self, path: &str, device: u64) {
            let entry = Entry {
                path: PathBuf::from(path),
                kind: Kind::Folder,
                device,
                inode: 1,
                links: 1,
                bytes: 4096,
            };
            self.entries.insert(PathBuf::from(path), entry);
            self.folders.entry(PathBuf::from(path)).or_default();
        }
    }

    impl Tree for Planted {
        fn stat(&mut self, path: &Path) -> io::Result<Entry> {
            self.entries.get(path).cloned().ok_or_else(|| io::ErrorKind::NotFound.into())
        }

        fn each(&mut self, path: &Path, visit: &mut dyn FnMut(Entry) -> bool) -> io::Result<()> {
            let entries = self.folders.get(path).ok_or(io::ErrorKind::NotFound)?;
            for entry in entries.clone() {
                if !visit(entry) {
                    break;
                }
            }
            Ok(())
        }
    }

    fn never() -> Duration {
        Duration::ZERO
    }

    fn budget() -> Budget {
        Budget { entries: APP_ENTRIES, until: APP_TIME }
    }

    fn found(tree: &mut Planted, root: &str) -> Walked {
        walk(tree, Path::new(root), &mut budget(), &mut HashSet::new(), &never).unwrap()
    }

    #[test]
    fn a_folder_costs_what_it_and_what_is_in_it_cost() {
        let mut tree = Planted::default();
        tree.root("/data", 1);
        tree.add("/data", "a", Kind::Other, 1, 8192);
        tree.add("/data", "sub", Kind::Folder, 1, 4096);
        tree.add("/data/sub", "b", Kind::Other, 1, 4096);
        let walked = found(&mut tree, "/data");
        assert_eq!(walked, Walked { bytes: 4096 + 8192 + 4096 + 4096, files: 2, complete: true });
    }

    #[test]
    fn a_file_with_two_names_is_counted_once() {
        let mut tree = Planted::default();
        tree.root("/data", 1);
        let mut one = tree.add("/data", "one", Kind::Other, 1, 4096);
        one.links = 2;
        let mut other = one.clone();
        other.path = PathBuf::from("/data/two");
        tree.folders.get_mut(Path::new("/data")).unwrap().clear();
        tree.folders.get_mut(Path::new("/data")).unwrap().extend([one, other]);
        let walked = found(&mut tree, "/data");
        assert_eq!((walked.bytes, walked.files), (4096 + 4096, 1));
    }

    #[test]
    fn a_link_is_counted_and_not_followed() {
        let mut tree = Planted::default();
        tree.root("/data", 1);
        // A link to `/`: an entry that is not a folder, whose own cost is its.
        tree.add("/data", "root", Kind::Other, 1, 0);
        let walked = found(&mut tree, "/data");
        assert_eq!((walked.bytes, walked.files, walked.complete), (4096, 1, true));
    }

    #[test]
    fn a_mount_inside_is_not_entered() {
        let mut tree = Planted::default();
        tree.root("/data", 1);
        tree.add("/data", "disk", Kind::Folder, 2, 4096);
        tree.add("/data/disk", "big", Kind::Other, 2, 1 << 30);
        tree.add("/data", "small", Kind::Other, 1, 4096);
        let walked = found(&mut tree, "/data");
        assert_eq!((walked.bytes, walked.files, walked.complete), (8192, 1, true));
    }

    #[test]
    fn five_thousand_levels_stop_at_the_depth_limit() {
        let mut tree = Planted::default();
        tree.root("/data", 1);
        let mut parent = String::from("/data");
        for level in 0..5000 {
            let name = format!("d{level}");
            tree.add(&parent, &name, Kind::Folder, 1, 4096);
            parent = format!("{parent}/{name}");
        }
        let walked = found(&mut tree, "/data");
        assert!(!walked.complete);
        // The root and the 4,095 folders below it that are entered, and
        // the one that is met at the limit and counted, not entered.
        assert_eq!(walked.bytes, 4096 * (DEPTH_MAX as u64 + 1));
    }

    #[test]
    fn the_entry_limit_ends_a_walk_with_what_it_has() {
        let mut tree = Planted::default();
        tree.root("/data", 1);
        for n in 0..10 {
            tree.add("/data", &format!("f{n}"), Kind::Other, 1, 4096);
        }
        let mut budget = Budget { entries: 4, until: APP_TIME };
        let walked =
            walk(&mut tree, Path::new("/data"), &mut budget, &mut HashSet::new(), &never).unwrap();
        assert_eq!((walked.files, walked.complete), (4, false));
        assert_eq!(budget.entries, 0);
    }

    #[test]
    fn the_time_limit_ends_a_walk_when_the_clock_says_so() {
        let mut tree = Planted::default();
        tree.root("/data", 1);
        tree.add("/data", "sub", Kind::Folder, 1, 4096);
        tree.add("/data/sub", "f", Kind::Other, 1, 4096);
        // The clock is past the app's time by the second look.
        let looks = Cell::new(0u32);
        let clock = || {
            looks.set(looks.get() + 1);
            if looks.get() > 1 { APP_TIME + Duration::from_secs(1) } else { Duration::ZERO }
        };
        let walked =
            walk(&mut tree, Path::new("/data"), &mut budget(), &mut HashSet::new(), &clock)
                .unwrap();
        assert!(!walked.complete);
        assert_eq!(walked.files, 0);
    }

    #[test]
    fn a_folder_that_is_not_there_is_an_error_and_a_file_is_one_file() {
        let mut tree = Planted::default();
        tree.root("/data", 1);
        tree.add("/data", "f", Kind::Other, 1, 4096);
        let missing =
            walk(&mut tree, Path::new("/gone"), &mut budget(), &mut HashSet::new(), &never);
        assert_eq!(missing.unwrap_err().kind(), io::ErrorKind::NotFound);
        let file = found(&mut tree, "/data/f");
        assert_eq!((file.bytes, file.files, file.complete), (4096, 1, true));
    }

    #[test]
    fn the_real_disk_is_counted_by_its_blocks() {
        let dir = std::env::temp_dir().join(format!("sizes-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("sub")).unwrap();
        fs::write(dir.join("sub/a"), vec![7u8; 10_000]).unwrap();
        fs::hard_link(dir.join("sub/a"), dir.join("b")).unwrap();
        std::os::unix::fs::symlink("/", dir.join("up")).unwrap();
        let walked = walk(&mut Disk, &dir, &mut budget(), &mut HashSet::new(), &never).unwrap();
        let _ = fs::remove_dir_all(&dir);
        // The file once, and the link as one more.
        assert_eq!(walked.files, 2);
        assert!(walked.complete);
        assert!(walked.bytes >= 10_000 && walked.bytes.is_multiple_of(BLOCK));
    }

    #[test]
    fn a_run_says_each_folder_and_each_app_and_leaves_out_what_is_not_there() {
        let mut tree = Planted::default();
        tree.root("/shared/services/org.example.a/a", 1);
        tree.add("/shared/services/org.example.a/a", "f", Kind::Other, 1, 4096);
        let requests = [
            Request {
                id: "org.example.a".into(),
                folders: vec![
                    "/shared/services/org.example.a/a".into(),
                    "/shared/services/org.example.a/gone".into(),
                ],
            },
            Request { id: "org.example.b".into(), folders: vec!["/shared/services/b/b".into()] },
        ];
        let mut out = Vec::new();
        run(&mut tree, &requests, &never, &mut out).unwrap();
        let lines: Vec<Line> =
            String::from_utf8(out).unwrap().lines().map(|l| Line::read(l).unwrap()).collect();
        assert_eq!(lines.len(), 2);
        assert!(matches!(&lines[0], Line::Folder { id, walked, .. }
            if id == "org.example.a" && walked.files == 1));
        assert!(matches!(&lines[1], Line::App { id, walked, .. }
            if id == "org.example.a" && walked.bytes == 8192));
    }

    #[test]
    fn a_day_that_is_over_starts_no_more_apps() {
        let mut tree = Planted::default();
        tree.root("/data", 1);
        let requests = [Request { id: "org.example.a".into(), folders: vec!["/data".into()] }];
        let mut out = Vec::new();
        run(&mut tree, &requests, &|| DAY_TIME, &mut out).unwrap();
        assert!(out.is_empty());
    }

    #[test]
    fn what_the_process_is_given_and_says_reads_back() {
        let requests = vec![
            Request { id: "org.example.a".into(), folders: vec!["/x/1".into(), "/x/2".into()] },
            Request { id: "org.example.b".into(), folders: vec!["/y".into()] },
        ];
        let words = arguments(&requests);
        assert_eq!(words[0], MODE);
        assert_eq!(read_arguments(&words[1..]).unwrap(), requests);
        assert!(read_arguments(&["org.example.a".to_string()]).is_err());
        for line in [
            Line::Folder {
                id: "org.example.a".into(),
                path: "/x/1".into(),
                walked: Walked { bytes: 9, files: 2, complete: false },
            },
            Line::App {
                id: "org.example.a".into(),
                walked: Walked { bytes: 9, files: 2, complete: true },
                took_ms: 31,
            },
            Line::Error("no idle class".into()),
        ] {
            assert_eq!(Line::read(&line.text()).unwrap(), line);
        }
        assert!(Line::read("app\torg.example.a\tmany\t2\t1\t3").is_err());
        assert!(Line::read("folder\ta\tb\t1\t2\tmaybe").is_err());
    }

    fn app(id: &str, bytes: u64) -> Line {
        Line::App { id: id.into(), walked: Walked { bytes, files: 3, complete: true }, took_ms: 5 }
    }

    #[test]
    fn sizes_are_kept_dropped_and_read_back() {
        let mut sizes = Sizes::default();
        let both = ["org.example.a".to_string(), "org.example.b".to_string()];
        let lines = [
            Line::Folder {
                id: "org.example.a".into(),
                path: "/x".into(),
                walked: Walked { bytes: 100, files: 3, complete: true },
            },
            app("org.example.a", 100),
            app("org.example.b", 50),
        ];
        assert!(sizes.apply(&lines, &both, 1_000));
        assert_eq!(sizes.apps[0].folders.len(), 1);
        let a = sizes.data_size("org.example.a").unwrap();
        assert_eq!((a.bytes, a.files, a.complete, a.measured_ms), (100, 3, true, 1_000));
        assert_eq!(sizes.data_size("org.example.c"), None);
        // The same words, a day later: only the time moves.
        assert!(sizes.apply(&[app("org.example.a", 100)], &both, 2_000));
        assert_eq!(sizes.data_size("org.example.b").unwrap().measured_ms, 1_000);
        // An app that is no longer installed is dropped.
        assert!(sizes.apply(&[], &["org.example.a".to_string()], 3_000));
        assert_eq!(sizes.data_size("org.example.b"), None);
        assert!(!sizes.apply(&[], &["org.example.a".to_string()], 4_000));
        assert_eq!(Sizes::read(&sizes.text()), sizes);
        assert_eq!(Sizes::read("not json"), Sizes { format: 1, apps: Vec::new() });
        assert_eq!(Sizes::read("{\"format\":2,\"apps\":[]}").apps.len(), 0);
    }

    /// A program that says what it is told to, as the walk's process does.
    fn script(name: &str, body: &str) -> String {
        use std::os::unix::fs::PermissionsExt;
        let path = std::env::temp_dir().join(format!("{name}-{}", std::process::id()));
        fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        path.to_string_lossy().into_owned()
    }

    #[test]
    fn the_lines_of_the_process_are_read_and_a_slow_one_is_ended() {
        let requests = [Request { id: "org.example.a".into(), folders: vec!["/x".into()] }];
        let quick = script(
            "sizes-quick",
            "printf 'app\\torg.example.a\\t9\\t2\\t1\\t3\\n'; printf 'nonsense\\n'",
        );
        let lines = measure(&quick, &requests, Duration::from_secs(20)).unwrap();
        let _ = fs::remove_file(&quick);
        assert_eq!(lines, vec![app_line()]);
        let slow =
            script("sizes-slow", "printf 'app\\torg.example.a\\t9\\t2\\t1\\t3\\n'; exec sleep 30");
        let lines = measure(&slow, &requests, Duration::from_millis(300)).unwrap();
        let _ = fs::remove_file(&slow);
        assert_eq!(lines.len(), 2);
        assert!(matches!(&lines[1], Line::Error(why) if why.contains("was ended")));
        assert!(measure("/nonexistent/walker", &requests, Duration::from_secs(1)).is_err());
    }

    fn app_line() -> Line {
        Line::App {
            id: "org.example.a".into(),
            walked: Walked { bytes: 9, files: 2, complete: true },
            took_ms: 3,
        }
    }
}
