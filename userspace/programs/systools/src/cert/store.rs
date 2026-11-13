// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! certd's store of certificates (ADR 0512): `/system/state/certd/certificates`.
//!
//! ```text
//! certificates/
//!   .lock                        writers hold it (flock, exclusive)
//!   NAME -> .NAME.7              a symbolic link, swapped by rename
//!   .NAME.7/fullchain.pem        the certificate first, then its chain
//!   .NAME.7/key.pem              the private key, root's alone
//!   .NAME.7/certificate.toml     what certd knows: its kind, when it was made
//!   .NAME.6/                     the one before, kept for readers still in it
//! ```
//!
//! A reader (`cert list`, Spark's copy) takes no lock: it follows the link,
//! and the link only ever leads to a generation whose files are all
//! written. Writers (certd, `cert add`, `cert remove`, `cert renew`) hold
//! the lock, so two never number generations alike. The generation
//! folders are named by [`manifest::service::certificate_generation`] and
//! read back by [`manifest::service::generation_of`], as Spark does.
//!
//! When a certificate does not read as it should, `cert show NAME` says
//! what is wrong with its files, and `ls -la` of the folder shows which
//! generation the link leads to.

use std::fs::{self, File, OpenOptions, TryLockError};
use std::io::{self, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};

use manifest::service::{self, CERTD_STATE, CERTIFICATE_FILES};
use serde::Deserialize;

use crate::util;

/// What the writers' lock is called, in the store.
const LOCK_FILE: &str = ".lock";

/// What certd knows of a certificate beyond its files.
pub const META_FILE: &str = "certificate.toml";

/// How long a writer waits for another to finish.
pub const LOCK_WAIT: Duration = Duration::from_secs(60);

/// How long a writer waits between looks at a lock another holds.
const LOCK_POLL: Duration = Duration::from_millis(100);

/// What the writer asks of a person when the lock stays taken.
const BUSY: &str = "certd or another `cert` is changing certificates; try again";

/// Who made a certificate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// certd made it for the machine itself.
    SelfSigned,
    /// An administrator added it with `cert add`.
    Added,
}

impl Kind {
    /// The word `certificate.toml` and `cert list` use.
    pub fn word(self) -> &'static str {
        match self {
            Kind::SelfSigned => "self-signed",
            Kind::Added => "added",
        }
    }
}

/// `certificate.toml`: the kind, and when it was made or added.
#[derive(Deserialize)]
struct Meta {
    kind: String,
    made: toml::value::Datetime,
}

/// What a writer puts in a new generation.
pub struct Files<'a> {
    /// `fullchain.pem`: the certificate, then its chain.
    pub chain: &'a str,
    /// `key.pem`.
    pub key: &'a str,
    pub kind: Kind,
    /// When it is made or added, in seconds since 1970.
    pub made: i64,
}

/// A certificate as the store holds it, without its key.
#[derive(Debug)]
pub struct Stored {
    pub name: String,
    pub kind: Kind,
    /// When certd made it or it was added, in seconds since 1970.
    pub made: i64,
    /// `fullchain.pem`.
    pub chain: Vec<u8>,
}

/// The store: a folder of certificates and its lock.
pub struct Store {
    dir: PathBuf,
    wait: Duration,
}

/// The writers' lock, held while this lives.
pub struct Lock {
    _file: File,
}

impl Store {
    /// The system's store.
    pub fn system() -> Store {
        Store::at(Path::new(CERTD_STATE), LOCK_WAIT)
    }

    /// The store in `dir`, whose lock a writer waits `wait` for.
    pub fn at(dir: &Path, wait: Duration) -> Store {
        Store { dir: dir.to_path_buf(), wait }
    }

    /// The folder of the store.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Takes the writers' lock, waiting for whoever holds it, and makes
    /// the store's folder if it is not there yet.
    ///
    /// # Errors
    ///
    /// If the lock stays taken for the store's wait, or the folder or its
    /// lock file cannot be made.
    pub fn lock(&self) -> Result<Lock, String> {
        make_dir(&self.dir)?;
        let path = self.dir.join(LOCK_FILE);
        let file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .open(&path)
            .map_err(|err| failed(&path, &err))?;
        let deadline = Instant::now() + self.wait;
        loop {
            match file.try_lock() {
                Ok(()) => return Ok(Lock { _file: file }),
                Err(TryLockError::WouldBlock) if Instant::now() < deadline => {
                    thread::sleep(LOCK_POLL);
                }
                Err(TryLockError::WouldBlock) => return Err(BUSY.to_string()),
                Err(TryLockError::Error(err)) => return Err(failed(&path, &err)),
            }
        }
    }

    /// The names of the certificates, sorted: the links in the folder.
    ///
    /// # Errors
    ///
    /// If the folder cannot be read. One that is not there holds none.
    pub fn names(&self) -> Result<Vec<String>, String> {
        let entries = match fs::read_dir(&self.dir) {
            Ok(entries) => entries,
            Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(err) => return Err(failed(&self.dir, &err)),
        };
        let mut names: Vec<String> = entries
            .flatten()
            .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_symlink()))
            .filter_map(|entry| entry.file_name().into_string().ok())
            .filter(|name| service::check_certificate(name).is_ok())
            .collect();
        names.sort();
        Ok(names)
    }

    /// The generation the link of certificate `name` leads to, or none
    /// when there is no such link.
    ///
    /// # Errors
    ///
    /// If `name` is a file or folder, or a link to anything but one of
    /// its generations.
    pub fn generation(&self, name: &str) -> Result<Option<u64>, String> {
        let path = self.dir.join(name);
        let target = match fs::read_link(&path) {
            Ok(target) => target,
            Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(err) => {
                return Err(format!(
                    "{} is not a link: {}",
                    path.display(),
                    util::strip_os_error(&err)
                ));
            }
        };
        let target = target.to_string_lossy();
        match service::generation_of(&target) {
            Some((of, generation)) if of == name => Ok(Some(generation)),
            _ => Err(format!(
                "{} leads to {target}, which is none of its generations",
                path.display()
            )),
        }
    }

    /// Certificate `name`, without its key.
    ///
    /// # Errors
    ///
    /// If there is no such certificate, or a file of it does not read.
    pub fn get(&self, name: &str) -> Result<Stored, String> {
        let generation =
            self.generation(name)?.ok_or_else(|| format!("there is no certificate {name}"))?;
        self.read_generation(name, generation)
    }

    /// Generation `generation` of certificate `name`, without its key.
    fn read_generation(&self, name: &str, generation: u64) -> Result<Stored, String> {
        let folder = self.dir.join(service::certificate_generation(name, generation));
        let chain = read(&folder.join(CERTIFICATE_FILES[0]))?;
        let meta = read(&folder.join(META_FILE))?;
        let (kind, made) = parse_meta(&meta)
            .map_err(|why| format!("{}: {why}", folder.join(META_FILE).display()))?;
        Ok(Stored { name: name.to_string(), kind, made, chain })
    }

    /// Where certificate `name`'s files are, through its link.
    pub fn files(&self, name: &str) -> [PathBuf; 2] {
        CERTIFICATE_FILES.map(|file| self.dir.join(name).join(file))
    }

    /// Certificate `name` and its private key, which only root reads, from
    /// the one generation the link led to when it was read: a writer that
    /// moves the link meanwhile cannot pair one generation's chain with the
    /// next's key.
    ///
    /// # Errors
    ///
    /// If there is no such certificate, or a file of it does not read.
    pub fn with_key(&self, name: &str) -> Result<(Stored, Vec<u8>), String> {
        let generation =
            self.generation(name)?.ok_or_else(|| format!("there is no certificate {name}"))?;
        let stored = self.read_generation(name, generation)?;
        let folder = self.dir.join(service::certificate_generation(name, generation));
        Ok((stored, read(&folder.join(CERTIFICATE_FILES[1]))?))
    }

    /// Makes certificate `name` the files `files` as its next generation
    /// and points its link there, and removes every generation but that
    /// and the one the link led to before. A reader sees the old pair
    /// whole or the new one, never a new chain with the old key. Returns
    /// the new generation.
    ///
    /// # Errors
    ///
    /// If a file or the link cannot be written; the link then still leads
    /// to the old generation.
    pub fn install(&self, _lock: &Lock, name: &str, files: &Files<'_>) -> Result<u64, String> {
        service::check_certificate(name)?;
        // A link that leads nowhere sensible has no generation to keep.
        let before = self.generation(name).ok().flatten();
        let generation = self.generations(name)?.into_iter().max().map_or(1, |newest| newest + 1);
        let folder = self.dir.join(service::certificate_generation(name, generation));
        let made = write_generation(&folder, files).and_then(|()| self.point(name, generation));
        if let Err(why) = made {
            // A generation no link leads to is of no use; one that stays
            // because it cannot be removed is numbered past by the next.
            let _ = fs::remove_dir_all(&folder);
            return Err(why);
        }
        // The link leads to the new generation now, so the install has
        // succeeded; an older one that stays is removed by the next.
        for older in self.generations(name).unwrap_or_default() {
            if older != generation && Some(older) != before {
                self.remove_generation(name, older);
            }
        }
        Ok(generation)
    }

    /// Removes certificate `name` and every generation of it: the link
    /// first, so that a reader finds none rather than a part.
    ///
    /// # Errors
    ///
    /// If the link cannot be removed.
    pub fn remove(&self, _lock: &Lock, name: &str) -> Result<(), String> {
        service::check_certificate(name)?;
        let link = self.dir.join(name);
        match fs::remove_file(&link) {
            Ok(()) => {}
            Err(err) if err.kind() == io::ErrorKind::NotFound => {}
            Err(err) => return Err(failed(&link, &err)),
        }
        for generation in self.generations(name)? {
            self.remove_generation(name, generation);
        }
        Ok(())
    }

    /// The numbers of the generations of `name` the folder holds, whether
    /// a link leads to them or not.
    fn generations(&self, name: &str) -> Result<Vec<u64>, String> {
        let entries = fs::read_dir(&self.dir).map_err(|err| failed(&self.dir, &err))?;
        Ok(entries
            .flatten()
            .filter_map(|entry| entry.file_name().into_string().ok())
            .filter_map(|entry| match service::generation_of(&entry) {
                Some((of, generation)) if of == name => Some(generation),
                _ => None,
            })
            .collect())
    }

    /// Removes generation `generation` of `name`, if it can be; one that
    /// stays is removed by the next write.
    fn remove_generation(&self, name: &str, generation: u64) {
        let folder = self.dir.join(service::certificate_generation(name, generation));
        // A folder that stays costs disk, and nothing reads it: the link
        // does not lead to it.
        let _ = fs::remove_dir_all(folder);
    }

    /// Points the link of `name` at generation `generation` through a
    /// new link renamed over the old, which is atomic.
    fn point(&self, name: &str, generation: u64) -> Result<(), String> {
        let new = self.dir.join(format!(".{name}.new"));
        match fs::remove_file(&new) {
            Ok(()) => {}
            Err(err) if err.kind() == io::ErrorKind::NotFound => {}
            Err(err) => return Err(failed(&new, &err)),
        }
        symlink(service::certificate_generation(name, generation), &new)
            .map_err(|err| failed(&new, &err))?;
        let link = self.dir.join(name);
        fs::rename(&new, &link).map_err(|err| failed(&link, &err))?;
        sync_dir(&self.dir)
    }
}

/// Makes the generation folder `folder` and writes the files into it.
fn write_generation(folder: &Path, files: &Files<'_>) -> Result<(), String> {
    make_dir(folder)?;
    let meta = format!(
        "kind = \"{}\"\nmade = {}\n",
        files.kind.word(),
        util::time_text(files.made, "%FT%TZ", true)
    );
    write_new(&folder.join(CERTIFICATE_FILES[0]), 0o644, files.chain)?;
    write_new(&folder.join(CERTIFICATE_FILES[1]), 0o600, files.key)?;
    write_new(&folder.join(META_FILE), 0o644, &meta)?;
    sync_dir(folder)
}

/// The kind and time `certificate.toml`'s `text` says.
fn parse_meta(text: &[u8]) -> Result<(Kind, i64), String> {
    let text = std::str::from_utf8(text).map_err(|_| "it is not UTF-8 text".to_string())?;
    let meta: Meta = toml::from_str(text).map_err(|err| err.message().to_string())?;
    let kind = match meta.kind.as_str() {
        "self-signed" => Kind::SelfSigned,
        "added" => Kind::Added,
        other => return Err(format!("its kind `{other}` is not one certd knows")),
    };
    let made = tuf::time::parse(&meta.made.to_string())
        .ok_or_else(|| format!("`{}` is not a time in UTC", meta.made))?;
    Ok((kind, made))
}

fn read(path: &Path) -> Result<Vec<u8>, String> {
    fs::read(path).map_err(|err| failed(path, &err))
}

/// Makes folder `path`, 0755, if it is not there.
fn make_dir(path: &Path) -> Result<(), String> {
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o755)
        .create(path)
        .map_err(|err| failed(path, &err))?;
    // The umask may have taken bits off the mode, and a reader that is
    // not root has to list the folder.
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).map_err(|err| failed(path, &err))
}

/// Writes `text` into the new file `path` with exactly `mode`, and makes
/// it durable. The mode is set before the text goes in, so that a key is
/// never readable by others.
fn write_new(path: &Path, mode: u32, text: &str) -> Result<(), String> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(mode)
        .open(path)
        .map_err(|err| failed(path, &err))?;
    file.set_permissions(fs::Permissions::from_mode(mode)).map_err(|err| failed(path, &err))?;
    file.write_all(text.as_bytes()).map_err(|err| failed(path, &err))?;
    file.sync_all().map_err(|err| failed(path, &err))
}

/// Makes the changes to folder `path` durable.
fn sync_dir(path: &Path) -> Result<(), String> {
    File::open(path).and_then(|folder| folder.sync_all()).map_err(|err| failed(path, &err))
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

    fn store(scratch: &Scratch) -> Store {
        Store::at(&scratch.0.join("certificates"), Duration::from_secs(5))
    }

    fn files<'a>(chain: &'a str, key: &'a str) -> Files<'a> {
        Files { chain, key, kind: Kind::Added, made: 1_800_000_000 }
    }

    fn install(store: &Store, name: &str, chain: &str) -> u64 {
        let lock = store.lock().unwrap();
        store.install(&lock, name, &files(chain, "key")).unwrap()
    }

    fn folders(store: &Store) -> Vec<String> {
        let mut found: Vec<String> = fs::read_dir(store.dir())
            .unwrap()
            .flatten()
            .map(|entry| entry.file_name().into_string().unwrap())
            .collect();
        found.sort();
        found
    }

    #[test]
    fn an_install_makes_generation_one_and_points_the_link_at_it() {
        let scratch = Scratch::new("store-first");
        let store = store(&scratch);
        assert_eq!(install(&store, "example.com", "chain one"), 1);
        assert_eq!(store.generation("example.com").unwrap(), Some(1));
        assert_eq!(
            fs::read_link(store.dir().join("example.com")).unwrap(),
            Path::new(".example.com.1")
        );
        let stored = store.get("example.com").unwrap();
        assert_eq!(stored.chain, b"chain one");
        assert_eq!((stored.kind, stored.made), (Kind::Added, 1_800_000_000));
        let (with, key) = store.with_key("example.com").unwrap();
        assert_eq!((with.chain, key), (b"chain one".to_vec(), b"key".to_vec()));
    }

    #[test]
    fn the_second_install_keeps_the_first_and_the_third_removes_it() {
        let scratch = Scratch::new("store-three");
        let store = store(&scratch);
        install(&store, "a.1", "one");
        assert_eq!(install(&store, "a.1", "two"), 2);
        assert_eq!(folders(&store), [".a.1.1", ".a.1.2", ".lock", "a.1"]);
        assert_eq!(install(&store, "a.1", "three"), 3);
        assert_eq!(folders(&store), [".a.1.2", ".a.1.3", ".lock", "a.1"]);
        assert_eq!(store.get("a.1").unwrap().chain, b"three");
    }

    #[test]
    fn generations_of_names_that_end_in_digits_are_told_apart() {
        let scratch = Scratch::new("store-digits");
        let store = store(&scratch);
        install(&store, "a", "a one");
        install(&store, "a.1", "a.1 one");
        install(&store, "a", "a two");
        assert_eq!(store.generation("a").unwrap(), Some(2));
        assert_eq!(store.generation("a.1").unwrap(), Some(1));
        assert_eq!(store.names().unwrap(), ["a", "a.1"]);
    }

    #[test]
    fn a_write_that_stops_before_the_link_moves_leaves_the_old_pair() {
        let scratch = Scratch::new("store-stopped");
        let store = store(&scratch);
        install(&store, "x.example", "old");
        // The files of generation 2 are written, and the link not yet
        // swapped, as when the machine stops there.
        write_generation(&store.dir().join(".x.example.2"), &files("new", "newkey")).unwrap();
        assert_eq!(store.generation("x.example").unwrap(), Some(1));
        assert_eq!(store.get("x.example").unwrap().chain, b"old");
        assert_eq!(store.with_key("x.example").unwrap().1, b"key");
    }

    #[test]
    fn an_install_that_fails_at_the_link_leaves_the_old_link_and_no_new_folder() {
        let scratch = Scratch::new("store-failed");
        let store = store(&scratch);
        install(&store, "x.example", "old");
        // A folder where the new link goes makes the swap fail.
        let blocked = store.dir().join(".x.example.new");
        fs::create_dir_all(blocked.join("inside")).unwrap();
        let lock = store.lock().unwrap();
        assert!(store.install(&lock, "x.example", &files("new", "newkey")).is_err());
        assert_eq!(store.generation("x.example").unwrap(), Some(1));
        assert_eq!(store.get("x.example").unwrap().chain, b"old");
        assert!(!store.dir().join(".x.example.2").exists());
    }

    #[test]
    fn files_are_written_with_the_modes_the_store_gives() {
        let scratch = Scratch::new("store-modes");
        let store = store(&scratch);
        install(&store, "x.example", "chain");
        let mode = |path: &str| fs::metadata(store.dir().join(path)).unwrap().mode() & 0o7777;
        assert_eq!(mode(".x.example.1"), 0o755);
        assert_eq!(mode(".x.example.1/fullchain.pem"), 0o644);
        assert_eq!(mode(".x.example.1/key.pem"), 0o600);
        assert_eq!(mode(".x.example.1/certificate.toml"), 0o644);
        assert_eq!(mode(".lock"), 0o600);
    }

    #[test]
    fn a_second_writer_waits_for_the_first() {
        let scratch = Scratch::new("store-wait");
        let store = store(&scratch);
        let first = store.lock().unwrap();
        let waiter = {
            let store = Store::at(store.dir(), Duration::from_secs(10));
            thread::spawn(move || {
                let started = Instant::now();
                let lock = store.lock();
                (lock.is_ok(), started.elapsed())
            })
        };
        thread::sleep(Duration::from_millis(400));
        drop(first);
        let (got_it, waited) = waiter.join().unwrap();
        assert!(got_it);
        assert!(waited >= Duration::from_millis(300), "{waited:?}");
    }

    #[test]
    fn a_writer_that_may_not_wait_is_told_to_try_again() {
        let scratch = Scratch::new("store-busy");
        let store = store(&scratch);
        let _held = store.lock().unwrap();
        let impatient = Store::at(store.dir(), Duration::ZERO);
        assert_eq!(
            impatient.lock().err().unwrap(),
            "certd or another `cert` is changing certificates; try again"
        );
    }

    #[test]
    fn names_are_the_links_alone_in_order() {
        let scratch = Scratch::new("store-names");
        let store = store(&scratch);
        assert_eq!(store.names().unwrap(), Vec::<String>::new());
        install(&store, "zeta.example", "z");
        install(&store, "alpha.example", "a");
        assert_eq!(store.names().unwrap(), ["alpha.example", "zeta.example"]);
    }

    #[test]
    fn a_removal_takes_the_link_and_every_generation() {
        let scratch = Scratch::new("store-remove");
        let store = store(&scratch);
        install(&store, "x.example", "one");
        install(&store, "x.example", "two");
        install(&store, "y.example", "other");
        let lock = store.lock().unwrap();
        store.remove(&lock, "x.example").unwrap();
        assert_eq!(folders(&store), [".lock", ".y.example.1", "y.example"]);
        assert_eq!(store.generation("x.example").unwrap(), None);
    }

    #[test]
    fn a_link_to_anything_but_its_own_generation_is_refused() {
        let scratch = Scratch::new("store-link");
        let store = store(&scratch);
        install(&store, "a.example", "a");
        install(&store, "b.example", "b");
        fs::remove_file(store.dir().join("a.example")).unwrap();
        symlink(".b.example.1", store.dir().join("a.example")).unwrap();
        assert!(store.generation("a.example").unwrap_err().contains("none of its generations"));
        fs::remove_file(store.dir().join("a.example")).unwrap();
        symlink("../elsewhere", store.dir().join("a.example")).unwrap();
        assert!(store.get("a.example").is_err());
    }

    #[test]
    fn certificate_toml_says_the_kind_and_the_time() {
        assert_eq!(
            parse_meta(b"kind = \"self-signed\"\nmade = 2026-11-13T15:00:00Z\n").unwrap(),
            (Kind::SelfSigned, 1_794_582_000)
        );
        assert!(parse_meta(b"kind = \"acme\"\nmade = 2026-11-13T15:00:00Z\n").is_err());
        assert!(parse_meta(b"kind = \"added\"\n").is_err());
        assert!(parse_meta(&[0xff, 0xfe]).is_err());
    }
}
