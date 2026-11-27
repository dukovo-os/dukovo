// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! scrub(8): reads every block of a Btrfs filesystem and checks it, as
//! Linux's `btrfs scrub start -B` does, with its summary (ADR 0150),
//! through the directory interface (radix-directory(2)).
//!
//! For the web console it also keeps what a scrub found in
//! `/system/state/scrub/UUID.json`, a file a filesystem, so that
//! `storage show` can say when the last one ran and how it went; it says
//! how far it has got, with `--progress`; and it refuses a second scrub
//! of a filesystem while one runs, by a lock in `/system/live`.
//!
//! The kernel's scrub is one call that returns when it is done and says
//! nothing before. It runs in a thread of its own, and the progress is
//! worked out from the sectors the filesystem's disks count as read since
//! it began, against the bytes the filesystem holds times its copies: an
//! estimate, which stays below 100% until the scrub has ended.

use std::fmt::Write as _;
use std::fs::{self, File};
use std::io::{self, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::{Duration, Instant};

use radix_abi::directory::{self, ScrubProgress as P};
use radix_abi::errors::Kernel;
use serde::{Deserialize, Serialize};

use crate::args::{Arg, Args};
use crate::df;
use crate::disk_errors;
use crate::probe;
use crate::proc::{self, Mount};
use crate::sys::{self, SYS};
use crate::util::{self, Fail};
use sysapi::signal::{self, Caught};

/// The exit status of a scrub that found errors no copy mends, as Linux's.
const UNCORRECTABLE: i32 = 3;

/// Where each filesystem's last scrub is kept, a file `UUID.json` each.
pub(crate) const KEPT: &str = "/system/state/scrub";

/// Where a scrub's lock is, a file `scrub-UUID.lock` each.
const LIVE: &str = "/system/live";

/// How often the running scrub is looked at.
const SLICE: Duration = Duration::from_millis(250);

/// How a scrub ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Ended {
    /// It found nothing, or mended everything it found.
    Ok,
    /// It found errors that no copy mends.
    Errors,
    /// It was stopped, or failed, before the end.
    Stopped,
}

/// What a scrub found: the file `KEPT` keeps of each filesystem's last
/// one, and what `storage show` gives of it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Kept {
    /// When it ended, in milliseconds since 1970.
    pub(crate) at_ms: u64,
    /// How long it took.
    pub(crate) seconds: u64,
    /// The data and tree bytes it read; for a scrub that stopped, the
    /// bytes its disks were read for while it ran.
    pub(crate) bytes: u64,
    /// The sectors that were bad on read, in a superblock, in what they
    /// held or in their checksums.
    pub(crate) errors: u64,
    /// How many of them were mended from a good copy.
    pub(crate) fixed: u64,
    /// How many no copy could mend.
    pub(crate) unfixable: u64,
    pub(crate) result: Ended,
}

impl Kept {
    /// What the progress `progress` of a scrub that ended says, after
    /// `seconds`, ending at `at_ms`.
    fn of(progress: &[u8; P::SIZE], seconds: u64, at_ms: u64) -> Kept {
        let word =
            |at: usize| u64::from_le_bytes(progress[at..at + 8].try_into().unwrap_or([0; 8]));
        let unfixable = word(P::OFFSET_UNCORRECTABLE_ERRORS);
        Kept {
            at_ms,
            seconds,
            bytes: word(P::OFFSET_DATA_BYTES) + word(P::OFFSET_TREE_BYTES),
            errors: word(P::OFFSET_READ_ERRORS)
                + word(P::OFFSET_SUPER_ERRORS)
                + word(P::OFFSET_VERIFY_ERRORS)
                + word(P::OFFSET_CSUM_ERRORS),
            fixed: word(P::OFFSET_CORRECTED_ERRORS),
            unfixable,
            result: if unfixable > 0 { Ended::Errors } else { Ended::Ok },
        }
    }

    /// A scrub that stopped after `seconds`, ending at `at_ms`, having
    /// read `bytes`: the kernel says nothing of what it found.
    fn stopped(bytes: u64, seconds: u64, at_ms: u64) -> Kept {
        Kept { at_ms, seconds, bytes, errors: 0, fixed: 0, unfixable: 0, result: Ended::Stopped }
    }
}

/// Whether `text` is a UUID as `probe::uuid` writes one, which a file
/// name may hold.
pub(crate) fn is_uuid(text: &str) -> bool {
    text.len() == 36
        && text.bytes().enumerate().all(|(at, byte)| match at {
            8 | 13 | 18 | 23 => byte == b'-',
            _ => byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase(),
        })
}

/// Keeps `kept` as the last scrub of the filesystem `uuid`, in `dir`,
/// whole or not at all: to a file beside it, renamed over it. Anyone may
/// read it.
fn keep(dir: &Path, uuid: &str, kept: &Kept) -> io::Result<()> {
    fs::DirBuilder::new().recursive(true).mode(0o755).create(dir)?;
    let text = serde_json::to_string(kept).map_err(io::Error::other)?;
    let new = dir.join(format!("{uuid}.json.new"));
    let mut file =
        fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o644).open(&new)?;
    writeln!(file, "{text}")?;
    file.sync_all()?;
    drop(file);
    fs::rename(new, dir.join(format!("{uuid}.json")))
}

/// What is kept of the filesystem `uuid`'s last scrub in `dir`, if it is
/// whole.
pub(crate) fn kept_in(dir: &Path, uuid: &str) -> Option<Kept> {
    let text = fs::read_to_string(dir.join(format!("{uuid}.json"))).ok()?;
    serde_json::from_str(&text).ok()
}

/// Whether process `pid` is there.
fn alive(pid: i32) -> bool {
    pid > 0
        && match sysapi::process::kill(pid, 0) {
            Ok(()) => true,
            Err(err) => err.raw_os_error() == Some(libc::EPERM),
        }
}

/// The lock of a filesystem's scrub, removed when it goes.
struct Lock {
    path: PathBuf,
}

impl Lock {
    /// Takes the lock at `path`, unless a scrub still running holds it:
    /// the lock names the process, so that one a crash left is found
    /// stale and taken over. `shown` is what the refusal calls the
    /// filesystem.
    fn take(path: &Path, shown: &str) -> Result<Lock, Fail> {
        for _ in 0..2 {
            match File::options().write(true).create_new(true).mode(0o600).open(path) {
                Ok(mut file) => {
                    let _ = write!(file, "{}", std::process::id());
                    return Ok(Lock { path: path.to_path_buf() });
                }
                Err(err) if err.kind() == io::ErrorKind::AlreadyExists => {
                    let holder = fs::read_to_string(path).ok().and_then(|t| t.trim().parse().ok());
                    if holder.is_some_and(alive) {
                        return Err(disk_errors::being_checked(shown));
                    }
                    let _ = fs::remove_file(path);
                }
                Err(err) => return Err(util::failed(&path.display(), err)),
            }
        }
        Err(disk_errors::being_checked(shown))
    }
}

impl Drop for Lock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

/// The Btrfs filesystem a path is on, before its scrub begins.
struct Volume {
    uuid: String,
    /// The devices that hold it.
    disks: Vec<String>,
}

/// The volume `path` is on, if it is a Btrfs filesystem from a device
/// whose UUID `found` gives (its kind and UUID), and which devices
/// `holding` says hold it.
fn volume_of(
    path: &Path,
    mounts: &[Mount],
    found: &dyn Fn(&str) -> Option<(String, String)>,
    holding: &dyn Fn(&str) -> Vec<String>,
) -> Option<Volume> {
    let mount = df::mount_of(mounts, path)?;
    let (kind, uuid) = found(&mount.source)?;
    (kind == "btrfs" && is_uuid(&uuid)).then(|| Volume { disks: holding(&uuid), uuid })
}

/// The kind and UUID of the filesystem on device `name`.
fn found_on(name: &str) -> Option<(String, String)> {
    let mut read = probe::reader(name)?;
    let found = probe::filesystem(&mut read)?;
    Some((found.kind.to_string(), found.uuid?))
}

/// The sectors a disk's `stat` file counts as read: its third number.
fn sectors_read(stat: &str) -> Option<u64> {
    stat.split_whitespace().nth(2)?.parse().ok()
}

/// The bytes counted as read from the disks `devices` are on, from the
/// `stat` files in `blocks`.
fn bytes_read(blocks: &Path, devices: &[String]) -> u64 {
    let mut disks: Vec<&str> = devices.iter().map(|name| probe::disk_of(name)).collect();
    disks.sort_unstable();
    disks.dedup();
    disks
        .into_iter()
        .filter_map(|disk| fs::read_to_string(blocks.join(disk).join("stat")).ok())
        .filter_map(|text| sectors_read(&text))
        .map(|sectors| sectors.saturating_mul(512))
        .fold(0, u64::saturating_add)
}

/// How many copies a scrub reads of data the profiles in a `profiles`
/// file keep: the most of any of the data's profiles, one if it says
/// none.
fn copies(profiles: &str) -> u64 {
    let Some(words) = profiles.lines().find_map(|line| line.strip_prefix("data ")) else {
        return 1;
    };
    words
        .split_whitespace()
        .map(|word| match word {
            "dup" | "raid1" | "raid10" => 2,
            "raid1c3" => 3,
            "raid1c4" => 4,
            _ => 1,
        })
        .max()
        .unwrap_or(1)
}

/// How far a scrub has got, in percent, having read `read` of the
/// `expected` bytes: never 100, which only the end says.
fn percent(read: u64, expected: u64) -> u8 {
    if expected == 0 {
        return 0;
    }
    let share = (u128::from(read) * 100 / u128::from(expected)).min(99);
    u8::try_from(share).unwrap_or(99)
}

/// What a running scrub is measured by.
struct Watch {
    blocks: PathBuf,
    devices: Vec<String>,
    /// What the devices had been read for when the scrub began.
    before: u64,
    /// The bytes the scrub is expected to read.
    expected: u64,
}

impl Watch {
    /// Looks at the disks now, before the scrub begins. The scrub reads
    /// each copy of what the filesystem uses.
    fn begin(blocks: &Path, devices: Vec<String>, used: u64, copies: u64) -> Watch {
        Watch {
            blocks: blocks.to_path_buf(),
            before: bytes_read(blocks, &devices),
            devices,
            expected: used.saturating_mul(copies),
        }
    }

    /// The bytes the disks have been read for since.
    fn read(&self) -> u64 {
        bytes_read(&self.blocks, &self.devices).saturating_sub(self.before)
    }

    /// How far the scrub has got.
    fn percent(&self) -> u8 {
        percent(self.read(), self.expected)
    }
}

/// When a line of progress is written: at most one a second, and none
/// that says what the last did.
struct Pace {
    last: Option<(Instant, u8)>,
}

impl Pace {
    /// The line to write at `now`, if there is one, when the scrub is
    /// `percent` of the way.
    fn line(&mut self, now: Instant, percent: u8) -> Option<String> {
        if let Some((at, said)) = self.last
            && (now.duration_since(at) < Duration::from_secs(1) || said == percent)
        {
            return None;
        }
        self.last = Some((now, percent));
        Some(format!("progress {percent}%"))
    }
}

static INTERRUPTED: Caught = Caught::new();
static TERMINATED: Caught = Caught::new();
static HUNG_UP: Caught = Caught::new();

/// The number of the signal that asked scrub to stop, or 0.
fn stopped_by() -> i32 {
    [(libc::SIGINT, &INTERRUPTED), (libc::SIGTERM, &TERMINATED), (libc::SIGHUP, &HUNG_UP)]
        .into_iter()
        .find(|(_, caught)| caught.is_set())
        .map_or(0, |(signal, _)| signal)
}

/// Has Ctrl-C, SIGTERM (which the web console's stop sends) and SIGHUP
/// stop the scrub: it is told of, and ends the command, which writes what
/// it has.
fn stop_on_signals() {
    let _ = signal::catch(libc::SIGINT, &INTERRUPTED, 0);
    let _ = signal::catch(libc::SIGTERM, &TERMINATED, 0);
    let _ = signal::catch(libc::SIGHUP, &HUNG_UP, 0);
}

/// Why a scrub did not end.
enum Failure {
    /// The filesystem is not a Btrfs one.
    NotBtrfs,
    Io(io::Error),
}

/// What the kernel's scrub answered: its progress, or why it failed.
type Answer = Result<[u8; P::SIZE], Failure>;

/// Runs the scrub of the filesystem `dir` is in, in a thread, which
/// sends its answer when it is done.
fn launch(dir: File, repair: bool) -> Result<mpsc::Receiver<Answer>, Fail> {
    let (send, receive) = mpsc::channel();
    std::thread::Builder::new()
        .name("scrub".into())
        .spawn(move || {
            let mut progress = [0u8; P::SIZE];
            let slot = sys::slot_of(dir.as_raw_fd());
            let answer = match directory::scrub(&SYS, slot, repair, &mut progress) {
                Ok(()) => Ok(progress),
                Err(status) if Kernel::from_status(status) == Some(Kernel::NotSupported) => {
                    Err(Failure::NotBtrfs)
                }
                Err(status) => Err(Failure::Io(sys::io_error(status))),
            };
            // The command may have gone already, having been stopped.
            let _ = send.send(answer);
        })
        .map_err(|e| util::failed(&"the scrub's thread", e))?;
    Ok(receive)
}

/// `bytes` as btrfs-progs prints a size: two decimals, in powers of 1024.
fn size(bytes: u64) -> String {
    let units = ["B", "KiB", "MiB", "GiB", "TiB", "PiB", "EiB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit + 1 < units.len() {
        value /= 1024.0;
        unit += 1;
    }
    format!("{value:.2}{}", units[unit])
}

/// A UUID as it is written: 8-4-4-4-12 hexadecimal digits.
fn uuid(bytes: &[u8]) -> String {
    let hex = bytes.iter().fold(String::new(), |mut hex, b| {
        let _ = write!(hex, "{b:02x}");
        hex
    });
    format!("{}-{}-{}-{}-{}", &hex[..8], &hex[8..12], &hex[12..16], &hex[16..20], &hex[20..])
}

/// The summary of a scrub that ended, as btrfs-progs prints it.
fn summary(out: &mut impl Write, progress: &[u8; P::SIZE], started_ns: u64, seconds: u64) {
    let word = |at: usize| u64::from_le_bytes(progress[at..at + 8].try_into().unwrap_or([0; 8]));
    let bytes = word(P::OFFSET_DATA_BYTES) + word(P::OFFSET_TREE_BYTES);
    let fsid = uuid(&progress[P::OFFSET_FSID..P::OFFSET_FSID + 16]);
    let started = util::local_time((started_ns / 1_000_000_000) as i64, "%a %b %e %H:%M:%S %Y");
    let _ = writeln!(out, "scrub done for {fsid}");
    let _ = writeln!(out, "Scrub started:    {started}");
    let _ = writeln!(out, "Status:           finished");
    let duration = format!("{}:{:02}:{:02}", seconds / 3600, seconds / 60 % 60, seconds % 60);
    let _ = writeln!(out, "Duration:         {duration}");
    let _ = writeln!(out, "Total to scrub:   {}", size(bytes));
    let _ = writeln!(out, "Rate:             {}/s", size(bytes / seconds.max(1)));
    let errors = [
        ("read", word(P::OFFSET_READ_ERRORS)),
        ("super", word(P::OFFSET_SUPER_ERRORS)),
        ("verify", word(P::OFFSET_VERIFY_ERRORS)),
        ("csum", word(P::OFFSET_CSUM_ERRORS)),
    ];
    let mut line = String::from("Error summary:   ");
    if errors.iter().all(|&(_, count)| count == 0) && word(P::OFFSET_UNCORRECTABLE_ERRORS) == 0 {
        line.push_str(" no errors found");
    } else {
        for (name, count) in errors.into_iter().filter(|&(_, count)| count > 0) {
            line.push_str(&format!(" {name}={count}"));
        }
        line.push_str(&format!("\n  Corrected:      {}", word(P::OFFSET_CORRECTED_ERRORS)));
        line.push_str(&format!("\n  Uncorrectable:  {}", word(P::OFFSET_UNCORRECTABLE_ERRORS)));
        line.push_str(&format!("\n  Unverified:     {}", word(P::OFFSET_UNVERIFIED_ERRORS)));
    }
    let _ = writeln!(out, "{line}");
}

/// What `main` makes of its command line.
struct Options {
    read_only: bool,
    quiet: bool,
    progress: bool,
    path: PathBuf,
}

/// The options of `scrub start`.
fn parse(args: &mut Args) -> Result<Options, Fail> {
    let usage = || util::usage("say start, and the path of a Btrfs filesystem".into());
    let Some(action) = args.subcommand() else { return Err(usage()) };
    if action != "start" {
        return Err(usage());
    }
    let (mut read_only, mut quiet, mut progress, mut path) = (false, false, false, None);
    while let Some(arg) = args.next()? {
        match arg {
            // It always waits for the scrub, as -B asks.
            Arg::Short('B') => {}
            Arg::Short('r') => read_only = true,
            Arg::Short('q') => quiet = true,
            Arg::Long(long) if long == "progress" => progress = true,
            Arg::Operand(operand) if path.is_none() => path = Some(PathBuf::from(operand)),
            other => return Err(args.unknown(&other)),
        }
    }
    let Some(path) = path else { return Err(usage()) };
    Ok(Options { read_only, quiet, progress, path })
}

/// scrub(8).
pub fn main(args: &mut Args) -> Result<i32, Fail> {
    let options = parse(args)?;
    let path = &options.path;
    let dir = File::open(path).map_err(|e| util::failed(&path.display(), e))?;
    // The filesystem, if it can be found, which the lock and the kept
    // result are named by.
    let on = fs::canonicalize(path).unwrap_or_else(|_| path.clone());
    let mounts = proc::mounts().unwrap_or_default();
    let volume = volume_of(&on, &mounts, &found_on, &probe::holding);
    let _lock = match &volume {
        Some(volume) => {
            let lock = Path::new(LIVE).join(format!("scrub-{}.lock", volume.uuid));
            Some(Lock::take(&lock, &path.display().to_string())?)
        }
        None => None,
    };
    let watch = {
        let used =
            df::statvfs(on.as_os_str()).map_or(0, |space| space.size - space.free.min(space.size));
        let profiles = volume.as_ref().and_then(|volume| {
            fs::read_to_string(format!("/kernel/fs/btrfs/{}/profiles", volume.uuid)).ok()
        });
        let devices = volume.as_ref().map_or_default(|volume| volume.disks.clone());
        Watch::begin(Path::new(probe::BLOCKS), devices, used, profiles.as_deref().map_or(1, copies))
    };
    let mut out = io::stdout();
    if !options.quiet {
        let _ = writeln!(out, "Starting scrub on devid 1");
        let _ = out.flush();
    }
    stop_on_signals();
    let started = util::now_ns();
    let answered = launch(dir, !options.read_only)?;
    let mut pace = Pace { last: None };
    let answer = loop {
        match answered.recv_timeout(SLICE) {
            Ok(answer) => break Some(answer),
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break None,
        }
        if stopped_by() != 0 {
            break None;
        }
        if options.progress
            && let Some(line) = pace.line(Instant::now(), watch.percent())
        {
            let _ = writeln!(out, "{line}");
            let _ = out.flush();
        }
    };
    let ended = util::now_ns();
    let seconds = (ended - started) / 1_000_000_000;
    let at_ms = ended / 1_000_000;
    let keep_in = |uuid: &str, kept: &Kept| {
        if let Err(err) = keep(Path::new(KEPT), uuid, kept) {
            eprintln!("scrub: {KEPT}: {}", util::strip_os_error(&err));
        }
    };
    let stopped = |why: &str| {
        if let Some(volume) = &volume {
            keep_in(&volume.uuid, &Kept::stopped(watch.read(), seconds, at_ms));
        }
        eprintln!("scrub: {why}");
    };
    match answer {
        Some(Ok(progress)) => {
            if !options.quiet {
                summary(&mut out, &progress, started, seconds);
            }
            let kept = Kept::of(&progress, seconds, at_ms);
            let fsid = uuid(&progress[P::OFFSET_FSID..P::OFFSET_FSID + 16]);
            keep_in(&fsid, &kept);
            if options.progress {
                let _ = writeln!(out, "progress 100%");
            }
            let _ = out.flush();
            if kept.unfixable > 0 {
                eprintln!("scrub: there are {} uncorrectable errors", kept.unfixable);
                return Ok(UNCORRECTABLE);
            }
            if kept.fixed > 0 {
                eprintln!("scrub: errors detected during scrubbing, {} corrected", kept.fixed);
            }
            Ok(0)
        }
        Some(Err(Failure::NotBtrfs)) => {
            Err(util::fail(format!("not a btrfs filesystem: {}", path.display())))
        }
        Some(Err(Failure::Io(err))) => {
            stopped("it failed before the end");
            Err(util::failed(&path.display(), err))
        }
        None => match stopped_by() {
            0 => {
                stopped("it ended without an answer");
                Err(util::fail("the scrub ended without an answer".into()))
            }
            signal => {
                stopped("stopped");
                Ok(128 + signal)
            }
        },
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    /// The progress of a scrub that read 100 data and 20 tree bytes and
    /// found what `found` gives: (read, super, verify, csum, corrected,
    /// uncorrectable).
    fn progress(found: [u64; 6]) -> [u8; P::SIZE] {
        let mut bytes = [0u8; P::SIZE];
        for (at, value) in [
            (P::OFFSET_DATA_BYTES, 100),
            (P::OFFSET_TREE_BYTES, 20),
            (P::OFFSET_READ_ERRORS, found[0]),
            (P::OFFSET_SUPER_ERRORS, found[1]),
            (P::OFFSET_VERIFY_ERRORS, found[2]),
            (P::OFFSET_CSUM_ERRORS, found[3]),
            (P::OFFSET_CORRECTED_ERRORS, found[4]),
            (P::OFFSET_UNCORRECTABLE_ERRORS, found[5]),
        ] {
            bytes[at..at + 8].copy_from_slice(&value.to_le_bytes());
        }
        bytes[P::OFFSET_FSID..P::OFFSET_FSID + 16].copy_from_slice(&[
            0x8f, 0x0c, 0x2a, 0x4e, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xaa,
            0xbb, 0xcc,
        ]);
        bytes
    }

    const UUID: &str = "8f0c2a4e-1122-3344-5566-778899aabbcc";

    /// A folder of its own for each test, as the tests run at once.
    fn folder(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("scrub-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_clean_scrub_is_ok_with_its_bytes() {
        let kept = Kept::of(&progress([0; 6]), 10_512, 1_761_980_000_000);
        assert_eq!(
            serde_json::to_value(kept).unwrap(),
            json!({"at_ms": 1_761_980_000_000u64, "seconds": 10_512, "bytes": 120, "errors": 0,
                "fixed": 0, "unfixable": 0, "result": "ok"})
        );
    }

    #[test]
    fn errors_that_a_good_copy_mended_are_ok_and_the_others_are_errors() {
        // Four sectors bad in their checksums, all mended.
        let mended = Kept::of(&progress([0, 0, 0, 4, 4, 0]), 1, 2);
        assert_eq!((mended.errors, mended.fixed, mended.unfixable), (4, 4, 0));
        assert_eq!(mended.result, Ended::Ok);
        // The four kinds add up, and two no copy mends.
        let lost = Kept::of(&progress([1, 2, 3, 4, 7, 2]), 1, 2);
        assert_eq!((lost.errors, lost.fixed, lost.unfixable), (10, 7, 2));
        assert_eq!(lost.result, Ended::Errors);
    }

    #[test]
    fn a_stopped_scrub_has_what_was_read_and_nothing_found() {
        let kept = Kept::stopped(5_000, 30, 9);
        assert_eq!(
            serde_json::to_value(kept).unwrap(),
            json!({"at_ms": 9, "seconds": 30, "bytes": 5000, "errors": 0, "fixed": 0,
                "unfixable": 0, "result": "stopped"})
        );
    }

    #[test]
    fn a_kept_result_is_written_whole_and_read_back() {
        use std::os::unix::fs::PermissionsExt;
        let dir = folder("keep").join("scrub");
        let kept = Kept::of(&progress([0, 0, 0, 1, 1, 0]), 5, 6);
        keep(&dir, UUID, &kept).unwrap();
        let mode = |path: &Path| fs::metadata(path).unwrap().permissions().mode() & 0o777;
        let file = dir.join(format!("{UUID}.json"));
        assert_eq!((mode(&dir), mode(&file)), (0o755, 0o644));
        assert!(!dir.join(format!("{UUID}.json.new")).exists());
        assert_eq!(kept_in(&dir, UUID), Some(kept));
        // The next scrub replaces it.
        let next = Kept::stopped(1, 2, 3);
        keep(&dir, UUID, &next).unwrap();
        assert_eq!(kept_in(&dir, UUID), Some(next));
        // A torn file, and a filesystem with none, read as none.
        fs::write(&file, "{\"at_ms\": 1").unwrap();
        assert_eq!(kept_in(&dir, UUID), None);
        assert_eq!(kept_in(&dir, "00000000-0000-0000-0000-000000000000"), None);
        fs::remove_dir_all(dir.parent().unwrap()).unwrap();
    }

    #[test]
    fn only_a_uuid_names_a_file() {
        assert!(is_uuid(UUID));
        for bad in [
            "",
            "../x",
            "8F0C2A4E-1122-3344-5566-778899AABBCC",
            "8f0c2a4e11223344556677889",
            "8f0c2a4e-1122-3344-5566-778899aabbc/",
        ] {
            assert!(!is_uuid(bad), "{bad}");
        }
        assert!(is_uuid(&uuid(&progress([0; 6])[P::OFFSET_FSID..P::OFFSET_FSID + 16])));
    }

    #[test]
    fn a_second_scrub_of_a_filesystem_is_refused_until_the_first_ends() {
        let dir = folder("lock");
        let path = dir.join(format!("scrub-{UUID}.lock"));
        let first = Lock::take(&path, "/volumes/data").unwrap();
        let refusal = Lock::take(&path, "/volumes/data").err().unwrap();
        assert!(refusal.message.ends_with("(DSK-102)"), "{}", refusal.message);
        assert!(refusal.message.contains("/volumes/data"), "{}", refusal.message);
        assert_eq!(fs::read_to_string(&path).unwrap(), std::process::id().to_string());
        drop(first);
        assert!(!path.exists());
        drop(Lock::take(&path, "/volumes/data").unwrap());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_lock_a_crash_left_is_taken_over() {
        let dir = folder("stale");
        let path = dir.join("scrub.lock");
        // A process number that no process has: past the largest the
        // kernel gives.
        for stale in ["2147483646", "", "0", "-1", "junk"] {
            fs::write(&path, stale).unwrap();
            let lock =
                Lock::take(&path, "/").unwrap_or_else(|f| panic!("{stale:?}: {}", f.message));
            assert_eq!(fs::read_to_string(&path).unwrap(), std::process::id().to_string());
            drop(lock);
        }
        fs::remove_dir_all(&dir).unwrap();
    }

    fn mounts() -> Vec<Mount> {
        proc::parse_mounts(
            b"vdb1 /volumes/data btrfs rw label=data\nvdb1 /users btrfs rw label=data\n\
              vda1 /boot/efi vfat rw\ntmp /tmp tmpfs rw\n",
        )
    }

    #[test]
    fn the_volume_of_a_path_is_found_by_its_mount() {
        let found = |name: &str| match name {
            "vdb1" => Some(("btrfs".to_string(), UUID.to_string())),
            "vda1" => Some(("vfat".to_string(), "1234-ABCD".to_string())),
            _ => None,
        };
        let holding = |uuid: &str| {
            assert_eq!(uuid, UUID);
            vec!["vdb1".to_string(), "vdc1".to_string()]
        };
        let mounts = mounts();
        let volume = volume_of(Path::new("/volumes/data/deep/er"), &mounts, &found, &holding);
        let volume = volume.unwrap();
        assert_eq!((volume.uuid.as_str(), volume.disks.len()), (UUID, 2));
        assert!(volume_of(Path::new("/users"), &mounts, &found, &holding).is_some());
        // Not a Btrfs filesystem, not from a device, and no filesystem found.
        assert!(volume_of(Path::new("/boot/efi"), &mounts, &found, &holding).is_none());
        assert!(volume_of(Path::new("/tmp"), &mounts, &found, &holding).is_none());
        assert!(volume_of(Path::new("/volumes/data"), &mounts, &|_| None, &holding).is_none());
        // A UUID that is no file name is not taken.
        let hostile = |_: &str| Some(("btrfs".to_string(), "../../x".to_string()));
        assert!(volume_of(Path::new("/users"), &mounts, &hostile, &holding).is_none());
    }

    #[test]
    fn the_sectors_a_disk_read_come_from_its_stat() {
        let stat = "  5326  0  244128  2093  7  0  80  12  0  1250  2105  0 0 0 0  0 0\n";
        assert_eq!(sectors_read(stat), Some(244_128));
        assert_eq!(sectors_read("1 2"), None);
        assert_eq!(sectors_read("a b c"), None);
        let dir = folder("stat");
        for (disk, sectors) in [("vdb", 1000u64), ("vdc", 24)] {
            fs::create_dir_all(dir.join(disk)).unwrap();
            fs::write(dir.join(disk).join("stat"), format!("1 0 {sectors} 1 0 0 0 0 0 0 0\n"))
                .unwrap();
        }
        // A partition counts for its disk, once; a disk with no file for none.
        let devices: Vec<String> =
            ["vdb1", "vdb2", "vdc1", "vdd1"].iter().map(ToString::to_string).collect();
        assert_eq!(bytes_read(&dir, &devices), (1000 + 24) * 512);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_copies_come_from_the_data_profiles() {
        assert_eq!(copies("data single\nmetadata dup\nsystem dup\n"), 1);
        assert_eq!(copies("data raid1\nmetadata raid1\nsystem raid1\n"), 2);
        assert_eq!(copies("data single raid1c3\nmetadata dup\n"), 3);
        assert_eq!(copies("metadata raid1c4\n"), 1);
        assert_eq!(copies(""), 1);
    }

    #[test]
    fn progress_is_the_share_read_and_not_100_until_the_end() {
        assert_eq!(percent(0, 0), 0);
        assert_eq!(percent(50, 0), 0);
        assert_eq!(percent(0, 1000), 0);
        assert_eq!(percent(410, 1000), 41);
        assert_eq!(percent(999, 1000), 99);
        assert_eq!(percent(1000, 1000), 99);
        assert_eq!(percent(u64::MAX, 1), 99);
    }

    #[test]
    fn progress_lines_come_at_most_once_a_second_and_when_the_number_moves() {
        let start = Instant::now();
        let mut pace = Pace { last: None };
        let at = |millis| start + Duration::from_millis(millis);
        assert_eq!(pace.line(at(0), 0).as_deref(), Some("progress 0%"));
        assert_eq!(pace.line(at(250), 1), None);
        assert_eq!(pace.line(at(999), 2), None);
        // A second on, a number that moved.
        assert_eq!(pace.line(at(1000), 2).as_deref(), Some("progress 2%"));
        // A number that did not move is not said again.
        assert_eq!(pace.line(at(2500), 2), None);
        assert_eq!(pace.line(at(3000), 3).as_deref(), Some("progress 3%"));
    }

    #[test]
    fn the_watch_measures_what_the_disks_read_since_it_began() {
        let dir = folder("watch");
        fs::create_dir_all(dir.join("vdb")).unwrap();
        let stat = |sectors: u64| {
            fs::write(dir.join("vdb/stat"), format!("1 0 {sectors} 1 0 0 0 0 0 0 0\n")).unwrap();
        };
        stat(1_000_000);
        // 1 MiB used, two copies: 2 MiB to read.
        let watch = Watch::begin(&dir, vec!["vdb1".into()], 1 << 20, 2);
        assert_eq!((watch.read(), watch.percent()), (0, 0));
        stat(1_000_000 + 2048);
        assert_eq!((watch.read(), watch.percent()), (1 << 20, 50));
        stat(1_000_000 + 10_000);
        assert_eq!(watch.percent(), 99);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_summary_is_btrfs_progs_with_its_counts() {
        let mut clean = Vec::new();
        summary(&mut clean, &progress([0; 6]), 1_800_000_000_000_000_000, 3_725);
        let clean = String::from_utf8(clean).unwrap();
        assert!(clean.starts_with(&format!("scrub done for {UUID}\n")), "{clean}");
        assert!(clean.contains("Duration:         1:02:05\n"), "{clean}");
        assert!(clean.contains("Total to scrub:   120.00B\n"), "{clean}");
        assert!(clean.ends_with("Error summary:    no errors found\n"), "{clean}");
        let mut bad = Vec::new();
        summary(&mut bad, &progress([0, 0, 0, 4, 2, 2]), 0, 1);
        let bad = String::from_utf8(bad).unwrap();
        assert!(
            bad.contains("Error summary:    csum=4\n  Corrected:      2\n  Uncorrectable:  2\n"),
            "{bad}"
        );
    }
}
