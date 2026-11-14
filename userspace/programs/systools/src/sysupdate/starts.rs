// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The record of every start (ADR 0500). The health check writes one as
//! it begins, unfinished, and finishes it with the start's outcome: good,
//! when the start was seen through or proved itself, or failed, with
//! why and its error ID. A start that never got that far stays
//! unfinished. Each record lists what the start ran: the system's version
//! and slot, the kernel, the firmware set and its microcode, the boot
//! options, the network cards and the root's partitions, and the apps
//! installed for everyone, each with when it was installed where that is
//! known. changes(1) compares two records. Each also keeps how long the
//! start took, step by step and service by service ([`Times`], ADR 0474),
//! for startup(8). The records are kept in the state,
//! `/system/state/starts/<number>.json`, the last [`KEEP`] of them.
//!
//! Two programs change the record of the start that runs: the health
//! check, which begins and finishes it, and authd, which notes when the
//! first person signed in. Each takes [`LOCK`] among the records while it
//! reads, changes and writes one, so that neither writes back what the
//! other changed a moment before. Which record is this start's, authd
//! reads from [`THIS_START`], which the health check writes as it begins
//! the record; the time of the first sign-in waits in [`SIGN_IN`] for a
//! record begun after it.

use std::fs::OpenOptions;
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use spark_api::show;

/// Where the records are kept.
pub const DIR: &str = "/system/state/starts";

/// How many records are kept.
pub const KEEP: u64 = 100;

/// The lock among the records that a program changing one holds.
pub const LOCK: &str = "lock";

/// The folder of this start's notes for its record, which go when it
/// ends: [`THIS_START`] and [`SIGN_IN`].
pub const LIVE: &str = "/system/live/sysupdate";

/// In [`LIVE`]: the number of this start's record, once the health check
/// has begun it.
pub const THIS_START: &str = "start";

/// In [`LIVE`]: when the first person signed in, in milliseconds after
/// the kernel started.
pub const SIGN_IN: &str = "sign-in";

/// The mark Spark leaves among the records as a start reaches its
/// services (ADR 0503), which [`begin_in`] removes: while it is there, a
/// start after the newest record got that far but never to its health
/// check.
pub const REACHED: &str = "reached";

/// How a start ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Outcome {
    /// The health check has not finished, or never did.
    Unfinished,
    /// The start was seen through, or an update proved itself.
    Good,
    /// The health check failed it.
    Failed,
}

/// One thing a start ran.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Item {
    /// What kind of thing: `system`, `kernel`, `firmware`, `microcode`,
    /// `options`, `card`, `root` or `app`.
    pub kind: String,
    /// Which one of its kind.
    pub name: String,
    /// Its version, or what tells it apart.
    pub value: String,
    /// When it was installed, in seconds since 1970, where that is known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub installed: Option<i64>,
}

impl Item {
    /// An item.
    #[must_use]
    pub fn new(kind: &str, name: &str, value: &str, installed: Option<i64>) -> Item {
        Item { kind: kind.into(), name: name.into(), value: value.into(), installed }
    }
}

/// The record of one start.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Start {
    /// The start's number, one more than the start before it.
    pub number: u64,
    /// When the health check began, in seconds since 1970.
    pub started: i64,
    /// The slot it started, `a` or `b`.
    pub slot: String,
    /// How far it got: `services` once the health check runs.
    pub stage: String,
    /// How it ended.
    pub outcome: Outcome,
    /// The error ID of what failed, such as `FW-101`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// What failed, in words.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub why: Option<String>,
    /// What it ran.
    pub items: Vec<Item>,
    /// How long it took, once the health check has seen it through and
    /// before then, the first sign-in's time if someone has signed in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub times: Option<Times>,
}

/// How long a start took (ADR 0474), in milliseconds. Those of the
/// services are by the kernel's clock, since it started.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Times {
    /// How long the firmware took to start the boot loader, if it says.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub firmware_ms: Option<u64>,
    /// How long the boot loader took, to the end of the firmware's boot
    /// services, if the firmware says.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub loader_ms: Option<u64>,
    /// When the kernel started Spark: the kernel's own part.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spark_ms: Option<u64>,
    /// When the services settled: the last of those that start at boot
    /// to be ready.
    #[serde(default)]
    pub settled_ms: u64,
    /// When the first person signed in at a console.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sign_in_ms: Option<u64>,
    /// Each service that started at boot, in the order Spark starts them.
    #[serde(default)]
    pub services: Vec<ServiceTime>,
}

/// When a service started and was ready, and what it started after.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServiceTime {
    pub name: String,
    pub started_ms: u64,
    /// When it was ready, if it was by the time the times were taken.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ready_ms: Option<u64>,
    /// The services it waited for: those it requires, wants and is after.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub after: Vec<String>,
}

/// What the kernel says at `/kernel/boot/times`, in milliseconds, each
/// known only where the firmware or the kernel says it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Kernel {
    pub firmware_ms: Option<u64>,
    pub loader_ms: Option<u64>,
    pub spark_ms: Option<u64>,
}

/// Where the kernel says its times.
const BOOT_TIMES: &str = "/kernel/boot/times";

/// The service that is the health check, which is not part of what a
/// start's services settle on.
const HEALTH: &str = "sysupdate";

impl Kernel {
    /// What `text` says: `firmware_ns N`, `loader_ns N` and `spark_ns N`,
    /// a line each, where known.
    #[must_use]
    pub fn parse(text: &str) -> Kernel {
        let ms = |key: &str| {
            text.lines()
                .find_map(|line| line.strip_prefix(key)?.strip_prefix(' '))
                .and_then(|ns| ns.trim().parse::<u64>().ok())
                .map(|ns| ns / 1_000_000)
        };
        Kernel {
            firmware_ms: ms("firmware_ns"),
            loader_ms: ms("loader_ns"),
            spark_ms: ms("spark_ns"),
        }
    }

    /// What the kernel says now.
    #[must_use]
    pub fn read() -> Kernel {
        std::fs::read_to_string(BOOT_TIMES)
            .map_or_else(|_| Kernel::default(), |text| Kernel::parse(&text))
    }
}

impl Times {
    /// The times of the start `document` is Spark's `show` of, with what
    /// the `kernel` says. Left out are the health check, a service
    /// started by a request or again after it ended, since those say
    /// nothing of how the start went.
    #[must_use]
    pub fn of(document: &show::Show, kernel: Kernel) -> Times {
        let mut times = Times {
            firmware_ms: kernel.firmware_ms,
            loader_ms: kernel.loader_ms,
            spark_ms: kernel.spark_ms,
            ..Times::default()
        };
        for service in &document.services {
            let at_boot = service.started_by.as_deref() != Some("a request");
            let Some(started_ms) = service.started_ms.filter(|_| at_boot && service.restarts == 0)
            else {
                continue;
            };
            if service.name == HEALTH {
                continue;
            }
            let ready_ms = service.ready_ms;
            let mut after: Vec<String> = service
                .requires
                .iter()
                .chain(&service.wants)
                .chain(&service.after)
                .cloned()
                .collect();
            after.sort();
            after.dedup();
            times.settled_ms = times.settled_ms.max(ready_ms.unwrap_or(0));
            times.services.push(ServiceTime {
                name: service.name.clone(),
                started_ms,
                ready_ms,
                after,
            });
        }
        times
    }

    /// The times of this start, as Spark and the kernel say them now.
    ///
    /// # Errors
    ///
    /// Why Spark's `show` cannot be had.
    pub fn now(sign_in_ms: Option<u64>) -> Result<Times, String> {
        let answer = crate::power::request("show").map_err(|why| why.message)?;
        let document = show::parse(&answer)?;
        Ok(Times { sign_in_ms, ..Times::of(&document, Kernel::read()) })
    }

    /// The times of this start once the services settle: when no
    /// service started at boot is still on its way to ready ([`settling`]),
    /// or after `within`, with what was ready by then. The health check
    /// sees a start through as soon as the essential services run, which
    /// is often before the last service that starts at boot is ready.
    ///
    /// # Errors
    ///
    /// Why Spark's `show` cannot be had.
    pub fn settled(within: std::time::Duration) -> Result<Times, String> {
        let begun = std::time::Instant::now();
        loop {
            let answer = crate::power::request("show").map_err(|why| why.message)?;
            let document = show::parse(&answer)?;
            if settling(&document).is_empty() || begun.elapsed() >= within {
                return Ok(Times {
                    sign_in_ms: signed_in(),
                    ..Times::of(&document, Kernel::read())
                });
            }
            std::thread::sleep(SETTLE_EVERY);
        }
    }
}

/// How often [`Times::settled`] asks Spark.
const SETTLE_EVERY: std::time::Duration = std::time::Duration::from_millis(500);

/// The services of `document` that a start's times wait for: those started
/// at boot, as [`Times::of`] counts them, that are pending or starting.
/// One that ended, failed or waits to start again has settled, as far as
/// the start goes.
#[must_use]
pub fn settling(document: &show::Show) -> Vec<&str> {
    document
        .services
        .iter()
        .filter(|service| service.name != HEALTH && service.restarts == 0)
        .filter(|service| service.started_by.as_deref() != Some("a request"))
        .filter(|service| matches!(service.state, show::State::Pending | show::State::Starting))
        .map(|service| service.name.as_str())
        .collect()
}

/// Every record in `dir`, oldest first; one that cannot be read is left
/// out.
#[must_use]
pub fn all_in(dir: &Path) -> Vec<Start> {
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut starts: Vec<Start> = entries
        .filter_map(Result::ok)
        .filter(|entry| entry.file_name().to_string_lossy().ends_with(".json"))
        .filter_map(|entry| std::fs::read(entry.path()).ok())
        .filter_map(|bytes| serde_json::from_slice(&bytes).ok())
        .collect();
    starts.sort_by_key(|start| start.number);
    starts
}

/// Every record kept, oldest first.
#[must_use]
pub fn all() -> Vec<Start> {
    all_in(Path::new(DIR))
}

/// The path of record `number` in `dir`.
fn path(dir: &Path, number: u64) -> PathBuf {
    dir.join(format!("{number}.json"))
}

/// Holds [`LOCK`] in `dir` while `then` runs. The lock goes with the
/// descriptor, also when the program ends without letting it go.
fn locked<T>(dir: &Path, then: impl FnOnce() -> std::io::Result<T>) -> std::io::Result<T> {
    std::fs::create_dir_all(dir)?;
    let lock = OpenOptions::new().create(true).truncate(false).write(true).open(dir.join(LOCK))?;
    // SAFETY: flock takes a descriptor `lock` owns, and the lock goes when
    // `lock` closes it, at the end of this function.
    if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    then()
}

/// Keeps `start` in `dir`, whole or not at all, and removes the records
/// more than [`KEEP`] starts older; the caller holds [`LOCK`].
fn write_unlocked(dir: &Path, start: &Start) -> std::io::Result<()> {
    let mut bytes = serde_json::to_vec_pretty(start).map_err(std::io::Error::other)?;
    bytes.push(b'\n');
    let partial = dir.join(format!("{}.partial", start.number));
    std::fs::write(&partial, bytes)?;
    std::fs::rename(&partial, path(dir, start.number))?;
    for old in all_in(dir).iter().filter(|old| old.number + KEEP <= start.number) {
        let _ = std::fs::remove_file(path(dir, old.number));
    }
    Ok(())
}

/// Begins the record of this start, unfinished, as of `now`, in `dir`:
/// the next number, the slot it started and what it runs.
///
/// # Errors
///
/// If it cannot be written.
pub fn begin_in(
    dir: &Path,
    live: &Path,
    slot: &str,
    items: Vec<Item>,
    now: i64,
) -> std::io::Result<Start> {
    locked(dir, || {
        let number = all_in(dir).last().map_or(1, |last| last.number + 1);
        // This start reached its health check: Spark's mark has done its
        // job.
        let _ = std::fs::remove_file(dir.join(REACHED));
        let start = Start {
            number,
            started: now,
            slot: slot.into(),
            stage: "services".into(),
            outcome: Outcome::Unfinished,
            error: None,
            why: None,
            items,
            times: None,
        };
        write_unlocked(dir, &start)?;
        // Within the lock: authd, which takes it next, finds the record.
        std::fs::create_dir_all(live)?;
        write_whole(&live.join(THIS_START), format!("{number}\n").as_bytes())?;
        Ok(start)
    })
}

/// Writes `bytes` to `path`, whole or not at all.
fn write_whole(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let mut partial = path.as_os_str().to_owned();
    partial.push(".partial");
    std::fs::write(&partial, bytes)?;
    std::fs::rename(&partial, path)
}

/// The number of this start's record, from `live`, once the health check
/// has begun it.
#[must_use]
pub fn this_start_in(live: &Path) -> Option<u64> {
    std::fs::read_to_string(live.join(THIS_START)).ok()?.trim().parse().ok()
}

/// [`this_start_in`] for the start that runs.
#[must_use]
pub fn this_start() -> Option<u64> {
    this_start_in(Path::new(LIVE))
}

/// When the first person signed in on this start, from `live`.
#[must_use]
pub fn signed_in_in(live: &Path) -> Option<u64> {
    std::fs::read_to_string(live.join(SIGN_IN)).ok()?.trim().parse().ok()
}

/// [`signed_in_in`] for the start that runs.
#[must_use]
pub fn signed_in() -> Option<u64> {
    signed_in_in(Path::new(LIVE))
}

/// Whether `text` is an error ID: two to five capital letters, a hyphen
/// and three digits, as in `FW-101`.
#[must_use]
pub fn is_error_id(text: &str) -> bool {
    let Some((letters, digits)) = text.split_once('-') else { return false };
    (2..=5).contains(&letters.len())
        && letters.bytes().all(|b| b.is_ascii_uppercase())
        && digits.len() == 3
        && digits.bytes().all(|b| b.is_ascii_digit())
}

/// The error ID that ends `why`, such as `(FW-101)`.
#[must_use]
pub fn error_id(why: &str) -> Option<String> {
    let inner = why.trim_end().strip_suffix(')')?;
    let id = &inner[inner.rfind('(')? + 1..];
    is_error_id(id).then(|| id.to_string())
}

/// Finishes `start` with `outcome`, and why it failed, in `dir`.
///
/// # Errors
///
/// If it cannot be written.
pub fn finish_in(
    dir: &Path,
    mut start: Start,
    outcome: Outcome,
    why: Option<&str>,
    times: Option<Times>,
) -> std::io::Result<Start> {
    locked(dir, || {
        // A person who signed in since the record began has left the time
        // in the file, and it is kept.
        let signed_in = all_in(dir)
            .into_iter()
            .find(|record| record.number == start.number)
            .and_then(|record| record.times)
            .and_then(|times| times.sign_in_ms);
        start.times = times
            .or(start.times.take())
            .map(|times| Times { sign_in_ms: times.sign_in_ms.or(signed_in), ..times });
        start.outcome = outcome;
        start.error = why.and_then(error_id);
        start.why = why.map(str::to_string);
        write_unlocked(dir, &start)?;
        Ok(start)
    })
}

/// Notes that the first person signed in `ms` milliseconds after the
/// kernel started: in `live`, for the health check to put in the record
/// it finishes, and in the record of `dir` that `live` names as this
/// start's, if the health check has begun it and it does not say so
/// already. Only the first time counts.
///
/// # Errors
///
/// If it cannot be written.
pub fn note_sign_in_in(dir: &Path, live: &Path, ms: u64) -> std::io::Result<()> {
    std::fs::create_dir_all(live)?;
    match OpenOptions::new().write(true).create_new(true).open(live.join(SIGN_IN)) {
        Ok(mut file) => std::io::Write::write_all(&mut file, format!("{ms}\n").as_bytes())?,
        Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => return Ok(()),
        Err(err) => return Err(err),
    }
    // Then the record, under the lock the health check takes to finish
    // it: whichever comes second sees what the first wrote. A record
    // finished already takes the time now; one finished later finds it
    // here, or in the file, which the health check reads as it finishes.
    locked(dir, || {
        let Some(number) = this_start_in(live) else { return Ok(()) };
        let Some(mut start) = all_in(dir).into_iter().find(|start| start.number == number) else {
            return Ok(());
        };
        if start.times.as_ref().is_some_and(|times| times.sign_in_ms.is_some()) {
            return Ok(());
        }
        start.times.get_or_insert_with(Times::default).sign_in_ms = Some(ms);
        write_unlocked(dir, &start)
    })
}

/// [`note_sign_in_in`] for the start that runs: authd tells it once.
///
/// # Errors
///
/// If it cannot be written.
pub fn note_sign_in(ms: u64) -> std::io::Result<()> {
    note_sign_in_in(Path::new(DIR), Path::new(LIVE), ms)
}

/// The last good start of `starts` before start `before`.
#[must_use]
pub fn last_good(starts: &[Start], before: u64) -> Option<&Start> {
    starts.iter().rev().find(|start| start.number < before && start.outcome == Outcome::Good)
}

/// One difference between two starts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Change {
    /// What kind of thing, as [`Item::kind`].
    pub kind: String,
    /// Which one.
    pub name: String,
    /// What the earlier start ran, if it had it.
    pub before: Option<String>,
    /// What the later start runs, if it has it.
    pub after: Option<String>,
    /// When what the later start runs was installed, where known.
    pub installed: Option<i64>,
    /// The first start of `starts` that ran it, and when.
    pub first_used: Option<(u64, i64)>,
}

/// What changed from start `from` to start `to`, with when each new
/// thing was installed and which of `starts` first used it, newest first.
#[must_use]
pub fn changes(starts: &[Start], from: &Start, to: &Start) -> Vec<Change> {
    let key = |item: &Item| (item.kind.clone(), item.name.clone());
    let mut found: Vec<Change> = Vec::new();
    for item in &to.items {
        let before = from.items.iter().find(|old| key(old) == key(item));
        if before.is_some_and(|old| old.value == item.value) {
            continue;
        }
        let first_used = starts
            .iter()
            .find(|start| {
                start.items.iter().any(|seen| key(seen) == key(item) && seen.value == item.value)
            })
            .map(|start| (start.number, start.started));
        found.push(Change {
            kind: item.kind.clone(),
            name: item.name.clone(),
            before: before.map(|old| old.value.clone()),
            after: Some(item.value.clone()),
            installed: item.installed,
            first_used,
        });
    }
    for old in from.items.iter().filter(|old| !to.items.iter().any(|item| key(item) == key(old))) {
        found.push(Change {
            kind: old.kind.clone(),
            name: old.name.clone(),
            before: Some(old.value.clone()),
            after: None,
            installed: None,
            first_used: None,
        });
    }
    // Newest first: by when each was first used, then installed.
    found.sort_by_key(|change| {
        std::cmp::Reverse((change.first_used.map(|(n, _)| n), change.installed))
    });
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_are_kept_numbered_and_compared() {
        let dir = std::env::temp_dir().join(format!("starts-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let items = |set: &str, card: bool| {
            let mut items = vec![
                Item::new("system", "Dukovo", "0.15.0", Some(100)),
                Item::new("firmware", "set", set, Some(200)),
            ];
            if card {
                items.push(Item::new(
                    "card",
                    "0000:00:03.0 52:54:00:12:34:56",
                    "Intel 82540EM",
                    None,
                ));
            }
            items
        };
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(REACHED), "services\n").unwrap();
        let first = begin_in(&dir, &dir.join("live"), "a", items("2026.1.1", true), 1000).unwrap();
        assert!(!dir.join(REACHED).exists(), "the health check's record removes Spark's mark");
        assert_eq!((first.number, first.outcome), (1, Outcome::Unfinished));
        finish_in(&dir, first, Outcome::Good, None, None).unwrap();
        let second =
            begin_in(&dir, &dir.join("live"), "b", items("2026.2.1", false), 2000).unwrap();
        let why = "the CPU microcode of firmware 2026.2.1 has data that failed its checks (FW-101)";
        let second = finish_in(&dir, second, Outcome::Failed, Some(why), None).unwrap();
        assert_eq!(second.error.as_deref(), Some("FW-101"));
        let starts = all_in(&dir);
        assert_eq!(starts.len(), 2);
        let good = last_good(&starts, second.number).unwrap();
        assert_eq!(good.number, 1);
        let found = changes(&starts, good, &second);
        assert_eq!(found.len(), 2);
        assert_eq!(
            (found[0].before.as_deref(), found[0].after.as_deref(), found[0].installed),
            (Some("2026.1.1"), Some("2026.2.1"), Some(200))
        );
        assert_eq!(found[0].first_used, Some((2, 2000)));
        assert_eq!((found[1].kind.as_str(), found[1].after.as_deref()), ("card", None));
        // Only the last KEEP are kept.
        let mut last = second;
        for _ in 0..KEEP {
            last = begin_in(&dir, &dir.join("live"), "b", Vec::new(), 3000).unwrap();
        }
        let kept = all_in(&dir);
        assert_eq!((kept.len() as u64, kept[0].number), (KEEP, last.number + 1 - KEEP));
        assert_eq!(error_id("no ID here"), None);
        assert_eq!(error_id("it is missing (BOOT-101)").as_deref(), Some("BOOT-101"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_record_from_before_the_times_reads_and_one_with_them_reads_back() {
        let old = r#"{"number":3,"started":1000,"slot":"a","stage":"services","outcome":"good","items":[]}"#;
        let start: Start = serde_json::from_str(old).unwrap();
        assert_eq!(start.times, None);
        assert!(!serde_json::to_string(&start).unwrap().contains("times"));
        let times = Times {
            firmware_ms: Some(1800),
            spark_ms: Some(1200),
            settled_ms: 6000,
            services: vec![ServiceTime {
                name: "netd".into(),
                started_ms: 2000,
                ready_ms: Some(3000),
                after: vec!["logd".into()],
            }],
            ..Times::default()
        };
        let kept = Start { times: Some(times), ..start };
        let text = serde_json::to_string(&kept).unwrap();
        assert!(!text.contains("loader_ms") && !text.contains("sign_in_ms"), "{text}");
        assert_eq!(serde_json::from_str::<Start>(&text).unwrap(), kept);
    }

    #[test]
    fn the_kernel_says_its_times_in_nanoseconds_where_it_knows_them() {
        let all = "firmware_ns 1800000000\nloader_ns 900999999\nspark_ns 1234567890\n";
        let kernel = Kernel::parse(all);
        assert_eq!(
            (kernel.firmware_ms, kernel.loader_ms, kernel.spark_ms),
            (Some(1800), Some(900), Some(1234))
        );
        let spark = Kernel::parse("spark_ns 5000000\n");
        assert_eq!((spark.firmware_ms, spark.loader_ms, spark.spark_ms), (None, None, Some(5)));
        assert_eq!(Kernel::parse(""), Kernel::default());
        assert_eq!(Kernel::parse("firmware_ns many\nloader_ns\n"), Kernel::default());
    }

    fn service(name: &str, started: u64, ready: Option<u64>, after: &[&str]) -> show::Service {
        show::Service {
            name: name.into(),
            started_ms: Some(started),
            ready_ms: ready,
            after: after.iter().map(|name| (*name).to_string()).collect(),
            started_by: Some("boot".into()),
            ..show::Service::default()
        }
    }

    #[test]
    fn times_are_the_services_that_started_at_boot_and_when_they_settled() {
        let mut restarted = service("e1000d", 9000, Some(9500), &[]);
        restarted.restarts = 2;
        let mut asked = service("sysupdate-stage", 8000, Some(8100), &[]);
        asked.started_by = Some("a request".into());
        let mut web = service("web", 3000, Some(5000), &["db"]);
        web.requires = vec!["db".into(), "netd".into()];
        web.wants = vec!["netd".into()];
        let document = show::Show {
            services: vec![
                service("logd", 1500, Some(1600), &[]),
                service("db", 2000, Some(2900), &["logd"]),
                web,
                // The health check is not what the start settles on, and what
                // was started again or by a request says nothing of the start.
                service("sysupdate", 4000, Some(30_000), &[]),
                restarted,
                asked,
                show::Service { started_ms: None, ..service("off", 0, None, &[]) },
                service("slow", 2500, None, &[]),
            ],
            ..show::Show::default()
        };
        let kernel = Kernel { firmware_ms: Some(1800), loader_ms: None, spark_ms: Some(1200) };
        let times = Times::of(&document, kernel);
        assert_eq!(
            (times.firmware_ms, times.loader_ms, times.spark_ms),
            (Some(1800), None, Some(1200))
        );
        assert_eq!(times.settled_ms, 5000);
        let names: Vec<&str> = times.services.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, ["logd", "db", "web", "slow"]);
        let web = &times.services[2];
        assert_eq!((web.started_ms, web.ready_ms), (3000, Some(5000)));
        assert_eq!(web.after, ["db", "netd"]);
        assert_eq!(times.services[3].ready_ms, None);
        assert_eq!(times.sign_in_ms, None);
    }

    #[test]
    fn the_times_wait_for_the_services_started_at_boot_that_are_on_their_way() {
        let with = |state: show::State, name: &str| show::Service {
            state,
            ..service(name, 1000, None, &[])
        };
        let mut restarted = with(show::State::Starting, "e1000d");
        restarted.restarts = 1;
        let mut asked = with(show::State::Starting, "sysupdate-stage");
        asked.started_by = Some("a request".into());
        let document = show::Show {
            services: vec![
                with(show::State::Running, "logd"),
                with(show::State::Starting, "lingers"),
                with(show::State::Pending, "web"),
                with(show::State::Done, "setup"),
                with(show::State::Failed, "broken"),
                with(show::State::Waiting, "flaky"),
                with(show::State::Disabled, "off"),
                // The health check runs while it waits, and neither a
                // service started again nor one started by a request is
                // what the start settles on.
                with(show::State::Starting, "sysupdate"),
                restarted,
                asked,
            ],
            ..show::Show::default()
        };
        assert_eq!(settling(&document), ["lingers", "web"]);
        let settled = show::Show { services: document.services[3..].to_vec(), ..document };
        assert!(settling(&settled).is_empty());
    }

    #[test]
    fn the_first_sign_in_is_noted_once_in_the_record_of_the_start_that_runs() {
        let dir = std::env::temp_dir().join(format!("sign-in-{}", std::process::id()));
        let live = dir.join("live");
        let _ = std::fs::remove_dir_all(&dir);
        let earlier = begin_in(&dir, &live, "a", Vec::new(), 1000).unwrap();
        finish_in(&dir, earlier, Outcome::Good, None, None).unwrap();
        // A new boot: its notes are gone, and its record not begun yet.
        std::fs::remove_dir_all(&live).unwrap();
        // A sign-in before the health check begins this start's record
        // waits for it, and leaves the earlier start's record alone.
        note_sign_in_in(&dir, &live, 12_000).unwrap();
        assert_eq!(all_in(&dir)[0].times, None);
        assert_eq!(signed_in_in(&live), Some(12_000));
        let this = begin_in(&dir, &live, "a", Vec::new(), 2000).unwrap();
        assert_eq!(this_start_in(&live), Some(this.number));
        // Only the first sign-in counts.
        note_sign_in_in(&dir, &live, 20_000).unwrap();
        assert_eq!(signed_in_in(&live), Some(12_000));
        // The health check takes the waiting time as it finishes.
        let times = Times { settled_ms: 7000, sign_in_ms: signed_in_in(&live), ..Times::default() };
        let done = finish_in(&dir, this, Outcome::Good, None, Some(times)).unwrap();
        let kept = done.times.unwrap();
        assert_eq!((kept.settled_ms, kept.sign_in_ms), (7000, Some(12_000)));
        assert_eq!(all_in(&dir)[1].times, Some(kept));

        // A sign-in after the record began is noted in it, and the health
        // check finishing the record afterwards keeps it.
        std::fs::remove_dir_all(&live).unwrap();
        let next = begin_in(&dir, &live, "a", Vec::new(), 3000).unwrap();
        note_sign_in_in(&dir, &live, 15_000).unwrap();
        assert_eq!(all_in(&dir)[2].times.as_ref().and_then(|t| t.sign_in_ms), Some(15_000));
        let times = Times { settled_ms: 6000, ..Times::default() };
        let done = finish_in(&dir, next, Outcome::Good, None, Some(times)).unwrap();
        assert_eq!(done.times.and_then(|t| t.sign_in_ms), Some(15_000));

        // And one after the record was finished.
        std::fs::remove_dir_all(&live).unwrap();
        let last = begin_in(&dir, &live, "a", Vec::new(), 4000).unwrap();
        let times = Times { settled_ms: 5000, ..Times::default() };
        finish_in(&dir, last, Outcome::Good, None, Some(times)).unwrap();
        note_sign_in_in(&dir, &live, 30_000).unwrap();
        let kept = all_in(&dir)[3].times.clone().unwrap();
        assert_eq!((kept.settled_ms, kept.sign_in_ms), (5000, Some(30_000)));
        assert_eq!(all_in(&dir)[3].outcome, Outcome::Good);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
