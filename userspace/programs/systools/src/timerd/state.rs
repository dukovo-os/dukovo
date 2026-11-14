// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! What timerd(8) keeps in `/system/state/timerd` (ADR 0478), each file
//! written whole: to a file of its own name with `.new` after it, which
//! replaces the old one by a rename, so that a reader or a power cut
//! sees the old file or the new one and never half of either.
//!
//! - `seed`: this machine's seed for `spread`, drawn the first time it is
//!   needed.
//! - `timers.json`: each service timer's last run.
//! - `crontabs/USER`: a person's crontab, as they wrote it.
//! - `at/ID.json` and `at-counter`: the jobs of `at`, and the last number
//!   given.
//! - `runs/KEY.json`: the last run of a crontab line (`crontab.USER.LINE`)
//!   or an `at` job (`at.ID`): its start, end and exit status or why it
//!   did not run, and the last 64 KiB of its output.
//! - `reboot`: the start of the machine whose `@reboot` lines ran.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// The folder.
pub const DIR: &str = "/system/state/timerd";

/// The most bytes of a job's output kept.
pub const OUTPUT_KEPT: usize = 64 * 1024;

/// The most `at` jobs kept for all, so that a person cannot fill the
/// disk with them.
pub const AT_JOBS_MAX: usize = 1000;

/// One run of a job.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Run {
    /// When it started, seconds since 1970; none when authd did not run
    /// it.
    pub start: Option<i64>,
    /// When it ended.
    pub end: Option<i64>,
    /// Its exit status, as a shell says it.
    pub status: Option<i32>,
    /// Why authd did not run it.
    pub not_run: Option<String>,
    /// The last [`OUTPUT_KEPT`] bytes of what it wrote.
    pub output: String,
}

/// A job of `at`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AtJob {
    pub id: u32,
    /// The user it runs as.
    pub owner: String,
    /// When it runs, seconds since 1970.
    pub time: i64,
    /// The folder `at` was run in.
    pub folder: String,
    /// The environment `at` was run in, `NAME=value` each.
    pub environment: Vec<String>,
    /// The commands.
    pub command: String,
}

/// The files of timerd's state folder.
pub struct Store {
    dir: PathBuf,
}

/// Writes `bytes` to `path` whole: to `path.new`, flushed, then renamed.
///
/// # Errors
///
/// What the writing or the rename says.
pub fn write_whole(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let mut name = path.as_os_str().to_owned();
    name.push(".new");
    let new = PathBuf::from(name);
    let mut file = fs::File::create(&new)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    fs::rename(&new, path)
}

/// Whether `name` can be a user's name in a file's name: what accounts
/// allow, so that no name reaches outside the folder.
#[must_use]
pub fn plain_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && !name.starts_with(['.', '-'])
        && name.bytes().all(|b| b.is_ascii_alphanumeric() || b"_.-".contains(&b))
}

/// The last [`OUTPUT_KEPT`] bytes of `output`, as text, starting on a
/// character.
#[must_use]
pub fn tail(output: &[u8]) -> String {
    let from = output.len().saturating_sub(OUTPUT_KEPT);
    let text = String::from_utf8_lossy(&output[from..]).into_owned();
    if from == 0 { text } else { text.trim_start_matches('\u{fffd}').to_string() }
}

impl Store {
    /// The state in `dir`, made if it is not there.
    ///
    /// # Errors
    ///
    /// When a folder cannot be made.
    pub fn open(dir: &Path) -> io::Result<Store> {
        for sub in ["crontabs", "at", "runs"] {
            fs::create_dir_all(dir.join(sub))?;
        }
        Ok(Store { dir: dir.to_path_buf() })
    }

    fn read<T: for<'de> Deserialize<'de>>(&self, name: &str) -> Option<T> {
        serde_json::from_slice(&fs::read(self.dir.join(name)).ok()?).ok()
    }

    fn write<T: Serialize>(&self, name: &str, value: &T) -> io::Result<()> {
        let bytes = serde_json::to_vec(value).map_err(io::Error::other)?;
        write_whole(&self.dir.join(name), &bytes)
    }

    /// This machine's seed: drawn from the system's random numbers the
    /// first time, so that machines cloned from one image draw their own.
    pub fn seed(&self) -> String {
        let path = self.dir.join("seed");
        if let Ok(text) = fs::read_to_string(&path)
            && text.trim().len() == 32
        {
            return text.trim().to_string();
        }
        let mut bytes = [0u8; 16];
        let drawn = fs::File::open("/dev/urandom").and_then(|mut f| f.read_exact(&mut bytes));
        if drawn.is_err() {
            // Without random numbers the seed is the boot's clock: it
            // still differs between machines, if not by much.
            let now = crate::util::now_ns().to_le_bytes();
            bytes[..8].copy_from_slice(&now);
        }
        let seed = bytes.iter().fold(String::new(), |mut hex, byte| {
            let _ = write!(hex, "{byte:02x}");
            hex
        });
        let _ = write_whole(&path, seed.as_bytes()); // a seed not kept is drawn again
        seed
    }

    /// Each service timer's last run.
    pub fn last_runs(&self) -> BTreeMap<String, i64> {
        self.read("timers.json").unwrap_or_default()
    }

    /// Keeps each service timer's last run.
    ///
    /// # Errors
    ///
    /// When the file cannot be written.
    pub fn save_last_runs(&self, runs: &BTreeMap<String, i64>) -> io::Result<()> {
        self.write("timers.json", runs)
    }

    /// The users who have a crontab.
    pub fn crontab_users(&self) -> Vec<String> {
        let Ok(entries) = fs::read_dir(self.dir.join("crontabs")) else { return Vec::new() };
        // The names are the ones timerd makes, all lower case.
        #[allow(clippy::case_sensitive_file_extension_comparisons)]
        let mut users: Vec<String> = entries
            .flatten()
            .filter_map(|entry| entry.file_name().into_string().ok())
            .filter(|name| plain_name(name) && !name.ends_with(".new"))
            .collect();
        users.sort();
        users
    }

    /// `user`'s crontab.
    pub fn crontab(&self, user: &str) -> Option<String> {
        plain_name(user).then(|| fs::read_to_string(self.dir.join("crontabs").join(user)).ok())?
    }

    /// Keeps `text` as `user`'s crontab.
    ///
    /// # Errors
    ///
    /// When the file cannot be written.
    pub fn save_crontab(&self, user: &str, text: &str) -> io::Result<()> {
        write_whole(&self.dir.join("crontabs").join(user), text.as_bytes())
    }

    /// Removes `user`'s crontab and the records of its runs; whether it
    /// was there.
    pub fn remove_crontab(&self, user: &str) -> bool {
        let there = fs::remove_file(self.dir.join("crontabs").join(user)).is_ok();
        self.clear_runs(user);
        there
    }

    /// Removes the records of the runs of `user`'s crontab lines, which
    /// the lines' numbers no longer match once the crontab changes.
    pub fn clear_runs(&self, user: &str) {
        let prefix = format!("crontab.{user}.");
        if let Ok(entries) = fs::read_dir(self.dir.join("runs")) {
            for entry in entries.flatten() {
                if entry.file_name().to_string_lossy().starts_with(&prefix) {
                    let _ = fs::remove_file(entry.path()); // a record left behind is replaced by the next run
                }
            }
        }
    }

    /// The `at` jobs, by number.
    pub fn at_jobs(&self) -> BTreeMap<u32, AtJob> {
        let mut jobs = BTreeMap::new();
        let Ok(entries) = fs::read_dir(self.dir.join("at")) else { return jobs };
        for entry in entries.flatten() {
            let Ok(bytes) = fs::read(entry.path()) else { continue };
            if let Ok(job) = serde_json::from_slice::<AtJob>(&bytes) {
                jobs.insert(job.id, job);
            }
        }
        jobs
    }

    /// Gives `job` the next number and keeps it.
    ///
    /// # Errors
    ///
    /// When a file cannot be written.
    pub fn add_at_job(&self, mut job: AtJob, taken: u32) -> io::Result<AtJob> {
        let counter: u32 = fs::read_to_string(self.dir.join("at-counter"))
            .ok()
            .and_then(|text| text.trim().parse().ok())
            .unwrap_or(0);
        job.id = counter.max(taken) + 1;
        write_whole(&self.dir.join("at-counter"), job.id.to_string().as_bytes())?;
        self.write(&format!("at/{}.json", job.id), &job)?;
        Ok(job)
    }

    /// Removes `at` job `id`.
    pub fn remove_at_job(&self, id: u32) {
        let _ = fs::remove_file(self.dir.join(format!("at/{id}.json"))); // gone is gone
    }

    /// The last run of the job `key`.
    pub fn run(&self, key: &str) -> Option<Run> {
        self.read(&format!("runs/{key}.json"))
    }

    /// Keeps `run` as the last run of the job `key`.
    ///
    /// # Errors
    ///
    /// When the file cannot be written.
    pub fn save_run(&self, key: &str, run: &Run) -> io::Result<()> {
        self.write(&format!("runs/{key}.json"), run)
    }

    /// The machine start whose `@reboot` lines ran, seconds since 1970.
    pub fn reboot_run(&self) -> Option<i64> {
        self.read("reboot")
    }

    /// Keeps `boot` as the start whose `@reboot` lines ran.
    ///
    /// # Errors
    ///
    /// When the file cannot be written.
    pub fn save_reboot_run(&self, boot: i64) -> io::Result<()> {
        self.write("reboot", &boot)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// A folder of a test's own, removed when the test is done.
    pub struct Temp(pub PathBuf);

    impl Temp {
        pub fn new() -> Temp {
            static NEXT: AtomicUsize = AtomicUsize::new(0);
            let dir = std::env::temp_dir().join(format!(
                "timerd-test-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&dir).unwrap();
            Temp(dir)
        }
    }

    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0); // a test's leftovers
        }
    }

    #[test]
    fn a_file_is_written_whole_and_replaced() {
        let dir = Temp::new();
        let path = dir.0.join("x");
        write_whole(&path, b"one").unwrap();
        write_whole(&path, b"two").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"two");
        assert!(!dir.0.join("x.new").exists());
    }

    #[test]
    fn the_seed_is_drawn_once_and_kept() {
        let dir = Temp::new();
        let store = Store::open(&dir.0).unwrap();
        let seed = store.seed();
        assert_eq!(seed.len(), 32);
        assert_eq!(Store::open(&dir.0).unwrap().seed(), seed);
    }

    #[test]
    fn last_runs_and_crontabs_are_kept() {
        let dir = Temp::new();
        let store = Store::open(&dir.0).unwrap();
        assert!(store.last_runs().is_empty());
        store.save_last_runs(&BTreeMap::from([("backup".to_string(), 12)])).unwrap();
        assert_eq!(store.last_runs()["backup"], 12);
        store.save_crontab("alice", "* * * * * x\n").unwrap();
        store.save_crontab("bob", "@daily y\n").unwrap();
        assert_eq!(store.crontab_users(), ["alice", "bob"]);
        assert_eq!(store.crontab("alice").as_deref(), Some("* * * * * x\n"));
        assert_eq!(store.crontab("../x"), None);
        store.save_run("crontab.alice.1", &Run { status: Some(0), ..Run::default() }).unwrap();
        assert!(store.remove_crontab("alice"));
        assert!(!store.remove_crontab("alice"));
        assert_eq!(store.crontab("alice"), None);
        assert_eq!(store.run("crontab.alice.1"), None);
    }

    #[test]
    fn at_jobs_get_numbers_that_are_not_used_again() {
        let dir = Temp::new();
        let store = Store::open(&dir.0).unwrap();
        let job = |command: &str| AtJob {
            id: 0,
            owner: "alice".into(),
            time: 100,
            folder: "/home/alice".into(),
            environment: vec!["A=1".into()],
            command: command.into(),
        };
        let first = store.add_at_job(job("a"), 0).unwrap();
        let second = store.add_at_job(job("b"), 0).unwrap();
        assert_eq!((first.id, second.id), (1, 2));
        store.remove_at_job(2);
        assert_eq!(store.add_at_job(job("c"), 0).unwrap().id, 3);
        assert_eq!(store.at_jobs().keys().copied().collect::<Vec<_>>(), [1, 3]);
        assert_eq!(store.at_jobs()[&1], first);
    }

    #[test]
    fn the_output_kept_is_the_last_64_kib() {
        assert_eq!(tail(b"short"), "short");
        let long = vec![b'x'; OUTPUT_KEPT + 10];
        assert_eq!(tail(&long).len(), OUTPUT_KEPT);
        let mut multibyte = vec![b'x'; 3];
        multibyte.extend("\u{e9}".repeat(OUTPUT_KEPT).as_bytes());
        assert!(tail(&multibyte).chars().all(|c| c == '\u{e9}'));
    }

    #[test]
    fn a_name_in_a_file_name_is_plain() {
        for good in ["alice", "a.b-c_d", "web1"] {
            assert!(plain_name(good), "{good}");
        }
        for bad in ["", ".x", "-x", "a/b", "..", "a b", "x\0"] {
            assert!(!plain_name(bad), "{bad:?}");
        }
    }
}
