// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! logturn(8) turns over the log files of services that have a `[logs]`
//! (ADR 0478). The system's own timer runs it hourly.
//!
//! For each file a pattern names that is due, because its day or week is
//! over since its last turn or because it holds the size the file says,
//! it
//!
//! 1. shifts the turned-over files up (`x.log.1` to `x.log.2`, a
//!    compressed `x.log.2.zst` to `x.log.3.zst`), and removes those past
//!    `keep`;
//! 2. renames `x.log` to `x.log.1`, which keeps its owner and mode;
//! 3. asks Spark to `reopen NAME` once for each service, which sends the
//!    signal the file names or restarts the service; and
//! 4. compresses the files turned over before this one, `x.log.2` on,
//!    never `x.log.1`, which the service may still write to until it has
//!    reopened.
//!
//! No line is lost: a rename keeps the file, so a service writes on into
//! `x.log.1` until it reopens, and nothing is ever truncated. A
//! compressed file has the owner, mode and times of the one it came from.
//!
//! A file is "due" by the time since its last turn, which logturn keeps
//! in `/system/state/logturn/turned.json`; a file it has not seen starts
//! its day or week when it first sees it. `--now` turns every file that
//! holds anything, due or not, for an administrator who wants it, and for
//! the boot tests `log-turnover` and `nginx`.
//!
//! A relative pattern is in the service's data folder: the `HOME` of its
//! environment, which bundled(8) sets to the account's folder in
//! `/shared/services`, else `/system/state/NAME`.

use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::fs;
use std::io;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use manifest::service::{Logs, Reopen, TurnOver};

use crate::args::{Arg, Args};
use crate::timerd::state::write_whole;
use crate::util::{self, Fail, fail};

/// Where logturn keeps when it turned each file over last.
const TURNED: &str = "/system/state/logturn/turned.json";

/// The zstd level: the default, 3.
const LEVEL: i32 = 3;

const DAY: i64 = 86_400;

fn log(level: u8, text: &str) {
    eprintln!("<{level}>logturn: {text}");
}

/// `logturn [--now] [NAME...]`.
pub fn main(args: &mut Args) -> Result<i32, Fail> {
    let mut now_wanted = false;
    let mut names = Vec::new();
    while let Some(arg) = args.next()? {
        match arg {
            Arg::Long(name) if name == "now" => now_wanted = true,
            Arg::Operand(name) => names.push(name.to_string_lossy().into_owned()),
            other => return Err(args.unknown(&other)),
        }
    }
    let (services, wrong) = crate::timerd::read_services(Path::new(crate::servicefiles::DIR));
    for (name, why) in wrong {
        log(4, &format!("{name}: its file does not read, so its logs are not turned over: {why}"));
    }
    if let Some(unknown) = names.iter().find(|n| !services.iter().any(|(s, _)| s == *n)) {
        return Err(fail(format!("{unknown}: unrecognized service")));
    }
    let mut turned: BTreeMap<String, i64> =
        fs::read(TURNED).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
    let now = now();
    let mut failed = false;
    for (name, service) in services {
        let Some(logs) = service.logs else { continue };
        if !names.is_empty() && !names.contains(&name) {
            continue;
        }
        let home = data_folder(&name, &service.environment);
        match turn_service(&name, &logs, &home, &mut turned, now, now_wanted, &|name| {
            crate::service::ask(&format!("reopen {name}")).map(|_| ()).map_err(|f| f.message)
        }) {
            Ok(count) if count > 0 => log(6, &format!("{name}: turned over {count} files")),
            Ok(_) => {}
            Err(why) => {
                log(3, &format!("{name}: {why}"));
                failed = true;
            }
        }
    }
    if let Some(dir) = Path::new(TURNED).parent() {
        let _ = fs::create_dir_all(dir); // the write below says if it failed
    }
    let bytes = serde_json::to_vec(&turned).map_err(|e| fail(e.to_string()))?;
    write_whole(Path::new(TURNED), &bytes).map_err(|e| util::failed(&TURNED, e))?;
    if failed {
        return Err(fail("some files could not be turned over; the log says which".into()));
    }
    Ok(0)
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
}

/// The folder relative patterns of the service `name` are in.
fn data_folder(name: &str, environment: &[(String, String)]) -> PathBuf {
    environment
        .iter()
        .find(|(key, _)| key == "HOME")
        .map_or_else(|| Path::new("/system/state").join(name), |(_, home)| PathBuf::from(home))
}

/// Turns over the files of `logs` that are due, or all that hold
/// anything when `all` says so, and has the service reopen: how many
/// files it turned. `reopen` asks Spark to reopen a service.
fn turn_service(
    name: &str,
    logs: &Logs,
    home: &Path,
    turned: &mut BTreeMap<String, i64>,
    now: i64,
    all: bool,
    reopen: &dyn Fn(&str) -> Result<(), String>,
) -> Result<usize, String> {
    let mut count = 0;
    let mut problems = Vec::new();
    for pattern in &logs.files {
        for file in expand(home, pattern) {
            let key = file.display().to_string();
            let size = fs::metadata(&file).map_or(0, |m| m.len());
            let last = *turned.entry(key.clone()).or_insert(now);
            if size == 0 || !(all || due(logs.turn_over, size, last, now)) {
                continue;
            }
            match shift_and_rename(&file, logs.keep) {
                Ok(()) => {
                    turned.insert(key, now);
                    count += 1;
                }
                Err(err) => {
                    problems.push(format!("{}: {}", file.display(), util::strip_os_error(&err)));
                }
            }
        }
    }
    if count > 0 {
        match logs.reopen {
            Reopen::Nothing => {}
            Reopen::Signal(_) | Reopen::Restart => {
                if let Err(why) = reopen(name) {
                    log(4, &format!("{name}: it was not told to open its files again: {why}"));
                }
            }
        }
    }
    if logs.compress {
        for pattern in &logs.files {
            for file in bases(home, pattern) {
                if let Err(err) = compress_older(&file, logs.keep) {
                    problems.push(format!(
                        "{}: compressing: {}",
                        file.display(),
                        util::strip_os_error(&err)
                    ));
                }
            }
        }
    }
    if problems.is_empty() { Ok(count) } else { Err(problems.join("; ")) }
}

/// Whether a file of `size` bytes, last turned at `last`, is due at
/// `now`.
fn due(turn_over: TurnOver, size: u64, last: i64, now: i64) -> bool {
    match turn_over {
        TurnOver::Daily => now - last >= DAY,
        TurnOver::Weekly => now - last >= 7 * DAY,
        TurnOver::Size(bytes) => size >= bytes,
    }
}

/// The files `pattern` names in `home`: those of its folder whose names
/// match its last part, in order, leaving out the files that are turned
/// over already and the partial ones this writes.
fn expand(home: &Path, pattern: &str) -> Vec<PathBuf> {
    let path = home.join(pattern);
    let (Some(dir), Some(last)) = (path.parent(), path.file_name().and_then(OsStr::to_str)) else {
        return Vec::new();
    };
    if !last.contains('*') {
        return if turned_name(last) || !path.is_file() { Vec::new() } else { vec![path] };
    }
    let mut found: Vec<PathBuf> = fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|entry| entry.file_type().is_ok_and(|t| t.is_file()))
        .filter(|entry| {
            let name = entry.file_name();
            util::fnmatch(last, &name) && !turned_name(&name.to_string_lossy())
        })
        .map(|entry| entry.path())
        .collect();
    found.sort();
    found
}

/// The files `pattern` names, as `expand` has them, and those that were
/// turned over and have not been written again: a service reopens its
/// file after a turn, but it may be a while before it does.
fn bases(home: &Path, pattern: &str) -> Vec<PathBuf> {
    let path = home.join(pattern);
    let (Some(dir), Some(last)) = (path.parent(), path.file_name().and_then(OsStr::to_str)) else {
        return Vec::new();
    };
    let mut found: Vec<PathBuf> = fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().into_string().ok()?;
            let stem = name.strip_suffix(".zst").unwrap_or(&name);
            let base =
                stem.rsplit_once('.').filter(|(_, n)| n.bytes().all(|b| b.is_ascii_digit()))?.0;
            (util::fnmatch(last, OsStr::new(base)) && !turned_name(base)).then(|| dir.join(base))
        })
        .collect();
    found.sort();
    found.dedup();
    found
}

/// Whether `name` is of a file turned over (`x.log.3`, `x.log.3.zst`) or
/// a partial one (`x.log.3.zst.new`).
// The names are the ones logturn makes, all lower case.
#[allow(clippy::case_sensitive_file_extension_comparisons)]
fn turned_name(name: &str) -> bool {
    if name.ends_with(".new") {
        return true;
    }
    let stem = name.strip_suffix(".zst").unwrap_or(name);
    stem.rsplit_once('.')
        .is_some_and(|(_, n)| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
}

/// The name of the turned-over copy `n` of `file`, compressed or not.
fn numbered(file: &Path, n: u32, compressed: bool) -> PathBuf {
    let mut name = file.as_os_str().to_owned();
    name.push(format!(".{n}"));
    if compressed {
        name.push(".zst");
    }
    PathBuf::from(name)
}

/// Shifts the turned-over copies of `file` up by one, removes those past
/// `keep`, and renames `file` to its first copy. A shift that fails
/// stops there with the files all still where they are or one step on:
/// no copy is ever replaced before it has moved.
fn shift_and_rename(file: &Path, keep: u32) -> io::Result<()> {
    for compressed in [false, true] {
        let oldest = numbered(file, keep, compressed);
        if oldest.exists() {
            fs::remove_file(&oldest)?;
        }
    }
    for n in (1..keep).rev() {
        for compressed in [false, true] {
            let from = numbered(file, n, compressed);
            if from.exists() {
                fs::rename(&from, numbered(file, n + 1, compressed))?;
            }
        }
    }
    fs::rename(file, numbered(file, 1, false))
}

/// Compresses each uncompressed copy of `file` from the second on, with
/// zstd, as a file with the owner, mode and times of the copy, and
/// removes the copy once the compressed file is in place.
fn compress_older(file: &Path, keep: u32) -> io::Result<()> {
    for n in 2..=keep {
        let plain = numbered(file, n, false);
        if !plain.is_file() {
            continue;
        }
        let target = numbered(file, n, true);
        let mut partial = target.as_os_str().to_owned();
        partial.push(".new");
        let partial = PathBuf::from(partial);
        let meta = fs::metadata(&plain)?;
        let mut from = fs::File::open(&plain)?;
        let to = fs::File::create(&partial)?;
        zstd::stream::copy_encode(&mut from, &to, LEVEL)?;
        to.sync_all()?;
        keep_owner_mode_times(&partial, &meta, &to)?;
        fs::rename(&partial, &target)?;
        fs::remove_file(&plain)?;
    }
    Ok(())
}

/// Gives `file`, which `handle` is open on, the owner, mode and times in
/// `meta`.
fn keep_owner_mode_times(file: &Path, meta: &fs::Metadata, handle: &fs::File) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let own = fs::metadata(file)?;
    if own.uid() != meta.uid() || own.gid() != meta.gid() {
        std::os::unix::fs::chown(file, Some(meta.uid()), Some(meta.gid()))?;
    }
    fs::set_permissions(file, fs::Permissions::from_mode(meta.mode() & 0o7777))?;
    let times = fs::FileTimes::new().set_modified(meta.modified()?);
    handle.set_times(times)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::timerd::state::tests::Temp;
    use std::io::Read;

    fn logs(turn_over: TurnOver, keep: u32, compress: bool) -> Logs {
        Logs {
            files: vec!["logs/*.log".into()],
            turn_over,
            keep,
            compress,
            reopen: Reopen::Nothing,
        }
    }

    fn write(path: &Path, text: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    fn unzstd(path: &Path) -> String {
        let mut out = String::new();
        zstd::stream::read::Decoder::new(fs::File::open(path).unwrap())
            .unwrap()
            .read_to_string(&mut out)
            .unwrap();
        out
    }

    fn run(
        dir: &Path,
        logs: &Logs,
        turned: &mut BTreeMap<String, i64>,
        now: i64,
        all: bool,
    ) -> usize {
        turn_service("web", logs, dir, turned, now, all, &|_| Ok(())).unwrap()
    }

    #[test]
    fn a_file_is_due_by_its_age_or_its_size() {
        assert!(!due(TurnOver::Daily, 5, 1000, 1000 + DAY - 1));
        assert!(due(TurnOver::Daily, 5, 1000, 1000 + DAY));
        assert!(!due(TurnOver::Weekly, 5, 1000, 1000 + 6 * DAY));
        assert!(due(TurnOver::Weekly, 5, 1000, 1000 + 7 * DAY));
        assert!(!due(TurnOver::Size(100), 99, 0, 1 << 40));
        assert!(due(TurnOver::Size(100), 100, 0, 0));
    }

    #[test]
    fn turned_over_names_are_told_from_log_names() {
        for turned in ["x.log.1", "x.log.12", "x.log.2.zst", "x.log.2.zst.new", "x.log.1.new"] {
            assert!(turned_name(turned), "{turned}");
        }
        for plain in ["x.log", "x.1log", "access.log", "a.b.log", "x.log.old", "x.log.zst"] {
            assert!(!turned_name(plain), "{plain}");
        }
    }

    #[test]
    fn a_pattern_names_the_files_of_a_folder_but_not_the_turned_ones() {
        let dir = Temp::new();
        for name in ["a.log", "b.log", "a.log.1", "a.log.2.zst", "c.txt"] {
            write(&dir.0.join("logs").join(name), "x");
        }
        let names = |pattern: &str| -> Vec<String> {
            expand(&dir.0, pattern)
                .iter()
                .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
                .collect()
        };
        assert_eq!(names("logs/*.log"), ["a.log", "b.log"]);
        assert_eq!(names("logs/a.log"), ["a.log"]);
        assert_eq!(names("logs/none.log"), Vec::<String>::new());
        assert_eq!(names("nowhere/*.log"), Vec::<String>::new());
        assert_eq!(expand(&dir.0, &format!("{}/logs/b.log", dir.0.display())).len(), 1);
    }

    #[test]
    fn a_turn_renames_the_file_and_shifts_the_older_ones_and_never_truncates() {
        let dir = Temp::new();
        let file = dir.0.join("logs/x.log");
        let spec = logs(TurnOver::Size(1), 3, false);
        let mut turned = BTreeMap::new();
        for generation in ["one", "two", "three", "four"] {
            write(&file, generation);
            assert_eq!(run(&dir.0, &spec, &mut turned, 100, false), 1);
        }
        // Three kept, newest first; "one" is past keep.
        assert!(!file.exists());
        let text = |n: u32| fs::read_to_string(numbered(&file, n, false)).unwrap();
        assert_eq!((text(1), text(2), text(3)), ("four".into(), "three".into(), "two".into()));
        assert!(!numbered(&file, 4, false).exists());
    }

    #[test]
    fn the_file_turned_last_is_not_compressed_but_the_one_before_it_is() {
        let dir = Temp::new();
        let file = dir.0.join("logs/x.log");
        let spec = logs(TurnOver::Size(1), 5, true);
        let mut turned = BTreeMap::new();
        write(&file, "first");
        run(&dir.0, &spec, &mut turned, 1, false);
        assert!(numbered(&file, 1, false).exists());
        assert!(!numbered(&file, 1, true).exists());
        write(&file, "second");
        run(&dir.0, &spec, &mut turned, 2, false);
        assert_eq!(fs::read_to_string(numbered(&file, 1, false)).unwrap(), "second");
        assert_eq!(unzstd(&numbered(&file, 2, true)), "first");
        assert!(!numbered(&file, 2, false).exists());
        write(&file, "third");
        run(&dir.0, &spec, &mut turned, 3, false);
        assert_eq!(unzstd(&numbered(&file, 3, true)), "first");
        assert_eq!(unzstd(&numbered(&file, 2, true)), "second");
        assert_eq!(fs::read_to_string(numbered(&file, 1, false)).unwrap(), "third");
    }

    #[test]
    fn every_line_is_found_once_and_in_order_across_the_files_after_five_turns() {
        let dir = Temp::new();
        let file = dir.0.join("logs/x.log");
        let spec = logs(TurnOver::Size(64), 3, true);
        let mut turned = BTreeMap::new();
        let mut next = 0;
        for _ in 0..5 {
            let mut text = String::new();
            while text.len() < 80 {
                text.push_str(&format!("{next}\n"));
                next += 1;
            }
            write(&file, &text);
            run(&dir.0, &spec, &mut turned, 5, false);
        }
        let mut all = String::new();
        for n in (1..=3).rev() {
            let (plain, packed) = (numbered(&file, n, false), numbered(&file, n, true));
            all.push_str(&if packed.exists() {
                unzstd(&packed)
            } else {
                fs::read_to_string(plain).unwrap()
            });
        }
        let numbers: Vec<u32> = all.lines().map(|l| l.parse().unwrap()).collect();
        let first = numbers[0];
        assert_eq!(numbers, (first..next).collect::<Vec<_>>());
        assert!(first > 0, "the oldest turn is past keep");
    }

    #[test]
    fn a_file_not_seen_before_starts_its_day_and_an_empty_one_is_left() {
        let dir = Temp::new();
        let file = dir.0.join("logs/x.log");
        let spec = logs(TurnOver::Daily, 3, false);
        let mut turned = BTreeMap::new();
        write(&file, "line\n");
        assert_eq!(run(&dir.0, &spec, &mut turned, 1000, false), 0);
        assert_eq!(run(&dir.0, &spec, &mut turned, 1000 + DAY - 1, false), 0);
        assert_eq!(run(&dir.0, &spec, &mut turned, 1000 + DAY, false), 1);
        write(&file, "");
        assert_eq!(
            run(&dir.0, &spec, &mut turned, 1000 + 3 * DAY, false),
            0,
            "an empty file has nothing to turn"
        );
        // `all` turns what holds anything, due or not.
        write(&file, "more\n");
        assert_eq!(run(&dir.0, &spec, &mut turned, 1000 + DAY + 5, true), 1);
    }

    #[test]
    fn the_service_reopens_once_after_its_files_are_turned_and_not_without() {
        let dir = Temp::new();
        let mut spec = logs(TurnOver::Size(1), 3, false);
        spec.reopen = Reopen::Signal(manifest::service::Signal::Usr1);
        for name in ["a.log", "b.log"] {
            write(&dir.0.join("logs").join(name), "x");
        }
        let asked = std::cell::RefCell::new(Vec::new());
        let ask = |name: &str| {
            asked.borrow_mut().push(name.to_string());
            Ok(())
        };
        let mut turned = BTreeMap::new();
        assert_eq!(turn_service("web", &spec, &dir.0, &mut turned, 1, false, &ask).unwrap(), 2);
        assert_eq!(*asked.borrow(), ["web"]);
        assert_eq!(turn_service("web", &spec, &dir.0, &mut turned, 2, false, &ask).unwrap(), 0);
        assert_eq!(asked.borrow().len(), 1);
    }

    #[test]
    fn a_service_that_cannot_reopen_still_has_its_files_turned() {
        let dir = Temp::new();
        let mut spec = logs(TurnOver::Size(1), 3, false);
        spec.reopen = Reopen::Restart;
        write(&dir.0.join("logs/a.log"), "x");
        let mut turned = BTreeMap::new();
        let refused = |_: &str| Err("web is not running".to_string());
        assert_eq!(turn_service("web", &spec, &dir.0, &mut turned, 1, false, &refused).unwrap(), 1);
        assert!(numbered(&dir.0.join("logs/a.log"), 1, false).exists());
    }

    #[test]
    fn a_compressed_file_keeps_the_mode_and_the_time_of_the_one_it_came_from() {
        use std::os::unix::fs::PermissionsExt;
        let dir = Temp::new();
        let file = dir.0.join("logs/x.log");
        let old = numbered(&file, 2, false);
        write(&old, "text\n");
        fs::set_permissions(&old, fs::Permissions::from_mode(0o640)).unwrap();
        let when = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_000_000);
        fs::File::options()
            .write(true)
            .open(&old)
            .unwrap()
            .set_times(fs::FileTimes::new().set_modified(when))
            .unwrap();
        compress_older(&file, 3).unwrap();
        let packed = fs::metadata(numbered(&file, 2, true)).unwrap();
        assert_eq!(packed.mode() & 0o7777, 0o640);
        assert_eq!(packed.modified().unwrap(), when);
    }

    #[test]
    fn a_relative_pattern_is_in_the_services_data_folder() {
        let env = vec![("HOME".to_string(), "/shared/services/org.x.y/web".to_string())];
        assert_eq!(data_folder("web", &env), Path::new("/shared/services/org.x.y/web"));
        assert_eq!(data_folder("web", &[]), Path::new("/system/state/web"));
    }
}
