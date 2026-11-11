// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! snap(1): makes, lists, restores, removes and prunes read-only
//! snapshots of Btrfs subvolumes (ADR 0461).
//!
//! A place is a subvolume's top folder, as `/users`. Its snapshots are in
//! its filesystem's `@snapshots` subvolume, in a folder named after the
//! subvolume less a leading `@` (`users`), each named by the local time it
//! was made or by a name it was given ([`keep`]). On Dukovo's root, Spark
//! mounts `@snapshots` read-only at `/snapshots`, where they are read;
//! they are written through the top of the filesystem in `/volumes`.
//!
//! `snap auto`, which the `snap` service runs every hour with `--watch`,
//! makes a snapshot of each place `snap.toml` names that has none from
//! today, then prunes each. Two rounds at once can only leave one
//! snapshot too many until the next: a snapshot made during a prune only
//! makes the older ones less needed.

mod keep;

use std::fs;
use std::io::{self, Write};
use std::os::unix::fs::MetadataExt;
use std::path::{Component, Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use serde::Deserialize;

use crate::args::{Arg, Args};
use crate::proc;
use crate::subvolume::{self, Info};
use crate::util::{self, Fail};

use keep::Kept;

/// The settings: which places get a snapshot a day, and how many are
/// kept (snap.toml(5)).
const SETTINGS: &str = "/system/settings/snap.toml";

/// Where Spark mounts the root's `@snapshots`.
const SHOWN: &str = "/snapshots";

/// The subvolume of snapshots at the top of each filesystem.
const SNAPSHOTS: &str = "@snapshots";

/// The folder of `@snapshots` the update keeps (ADR 0454).
const UPDATE: &str = "update";

/// How long `auto --watch` waits between rounds.
const ROUND: Duration = Duration::from_hours(1);

/// The bits of `st_ino` that are the inode's own number on Radix (the
/// subvolume's number is above them), and a subvolume's top folder's.
const INODE_BITS: u64 = (1 << 40) - 1;
const TOP_INODE: u64 = 256;

/// The ID of the top of a Btrfs filesystem.
const TOP_ID: u64 = 5;

/// snap.toml as written.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct File {
    places: Option<Vec<String>>,
    daily: Option<usize>,
    weekly: Option<usize>,
}

/// What snap.toml says, with the defaults for what it leaves out.
#[derive(Debug, PartialEq, Eq)]
struct Settings {
    places: Vec<String>,
    daily: usize,
    weekly: usize,
}

impl Default for Settings {
    fn default() -> Settings {
        Settings {
            places: ["/users", "/shared", "/system/settings"].map(String::from).to_vec(),
            daily: 7,
            weekly: 4,
        }
    }
}

impl Settings {
    /// The settings in `text`.
    fn parse(text: &str) -> Result<Settings, String> {
        let file: File = toml::from_str(text).map_err(|err| err.to_string())?;
        let default = Settings::default();
        Ok(Settings {
            places: file.places.unwrap_or(default.places),
            daily: file.daily.unwrap_or(default.daily),
            weekly: file.weekly.unwrap_or(default.weekly),
        })
    }

    /// The settings in [`SETTINGS`], or the defaults if there is none.
    fn read() -> Result<Settings, Fail> {
        match fs::read_to_string(SETTINGS) {
            Ok(text) => {
                Settings::parse(&text).map_err(|err| util::fail(format!("{SETTINGS}: {err}")))
            }
            Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(Settings::default()),
            Err(err) => Err(util::failed(&SETTINGS, err)),
        }
    }
}

/// A subvolume snap keeps snapshots of.
struct Place {
    /// Its top folder.
    top: PathBuf,
    /// Where its snapshots are made and removed.
    folder: PathBuf,
    /// Where they are read: in [`SHOWN`] on the root.
    shown: PathBuf,
}

/// A snapshot of a place.
struct Snapshot {
    name: String,
    info: Info,
}

impl Snapshot {
    /// Whether it is named by its time, and so pruned.
    fn timed(&self) -> bool {
        keep::is_timed(&self.name)
    }
}

/// Whether the folder `path` is the top of a Btrfs subvolume.
fn is_top(path: &Path) -> bool {
    fs::metadata(path).is_ok_and(|m| m.is_dir() && m.ino() & INODE_BITS == TOP_INODE)
}

/// The name of a subvolume's folder in `@snapshots`: its own, without a
/// leading `@`; `top` for the top of a filesystem.
fn folder_name(info: &Info) -> String {
    if info.id == TOP_ID || info.name.is_empty() {
        return "top".into();
    }
    info.name.strip_prefix('@').unwrap_or(&info.name).to_string()
}

/// The place whose top folder is `path`.
fn place(path: &Path) -> Result<Place, Fail> {
    let top = fs::canonicalize(path).map_err(|e| util::failed(&path.display(), e))?;
    let info = match subvolume::info(&top)? {
        Some(info) if is_top(&top) => info,
        _ => {
            return Err(util::fail(format!(
                "{}: not the top of a Btrfs subvolume, which snap takes snapshots of",
                path.display()
            )));
        }
    };
    let name = folder_name(&info);
    if name == UPDATE {
        return Err(util::fail(format!(
            "{}: its snapshots would go in @snapshots/{UPDATE}, which updates keep",
            path.display()
        )));
    }
    let mounts = proc::mounts().map_err(|e| util::failed(&"/processes/mounts", e))?;
    let holding = |path: &Path| {
        mounts
            .iter()
            .filter(|m| path.starts_with(&m.path))
            .max_by_key(|m| Path::new(&m.path).components().count())
    };
    let Some(mount) = holding(&top).filter(|m| m.kind == "btrfs") else {
        return Err(util::fail(format!("{}: not on a Btrfs filesystem", path.display())));
    };
    let source = mount.source.clone();
    let fs_top = mounts
        .iter()
        .filter(|m| m.kind == "btrfs" && m.source == source)
        .map(|m| PathBuf::from(&m.path))
        .find(|at| subvolume::info(at).ok().flatten().is_some_and(|i| i.id == TOP_ID))
        .ok_or_else(|| {
            util::fail(format!(
                "{}: the top of its filesystem, {source}, is not mounted; mount(8) it in /volumes",
                path.display()
            ))
        })?;
    let folder = fs_top.join(SNAPSHOTS).join(&name);
    let shown = match mounts.iter().find(|m| m.path == SHOWN && m.source == source) {
        Some(_) => Path::new(SHOWN).join(&name),
        None => folder.clone(),
    };
    Ok(Place { top, folder, shown })
}

/// The snapshots of `place`, oldest first.
fn snapshots(place: &Place) -> Result<Vec<Snapshot>, Fail> {
    let entries = match fs::read_dir(&place.shown) {
        Ok(entries) => entries,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => return Err(util::failed(&place.shown.display(), err)),
    };
    let mut found = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|e| util::failed(&place.shown.display(), e))?;
        let path = entry.path();
        let Some(name) = entry.file_name().to_str().map(String::from) else { continue };
        if !is_top(&path) {
            continue;
        }
        if let Some(info) = subvolume::info(&path)? {
            found.push(Snapshot { name, info });
        }
    }
    found.sort_by(|a, b| (a.info.made, &a.name).cmp(&(b.info.made, &b.name)));
    Ok(found)
}

/// The local date and time of `seconds` since 1970: year, month, day,
/// hour and minute.
fn local(seconds: i64) -> (i64, u32, u32, u32, u32) {
    let time = seconds as libc::time_t;
    // SAFETY: an all-zero tm is valid, and localtime_r fills it.
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    // SAFETY: both pointers are valid for the call.
    if unsafe { libc::localtime_r(&raw const time, &raw mut tm) }.is_null() {
        // Before the clock or time zone can be read: as UTC.
        let day = seconds.div_euclid(86_400);
        let rest = seconds.rem_euclid(86_400) as u32;
        let (y, m, d) = civil(day);
        return (y, m, d, rest / 3600, rest / 60 % 60);
    }
    (
        i64::from(tm.tm_year) + 1900,
        (tm.tm_mon + 1) as u32,
        tm.tm_mday as u32,
        tm.tm_hour as u32,
        tm.tm_min as u32,
    )
}

/// The date of day `day` since 1970-01-01, by Howard Hinnant's
/// `civil_from_days`.
fn civil(day: i64) -> (i64, u32, u32) {
    let z = day + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

/// The local day, since 1970-01-01, of `ns` nanoseconds since 1970.
fn local_day(ns: u64) -> i64 {
    let (year, month, day, _, _) = local((ns / 1_000_000_000) as i64);
    keep::day_number(year, month, day)
}

/// When a snapshot was made, as `snap list` writes it.
fn made_text(info: &Info) -> String {
    if info.made == 0 {
        return "-".into();
    }
    util::local_time((info.made / 1_000_000_000) as i64, "%Y-%m-%d %H:%M")
}

/// Why each snapshot named by its time is kept, newest first, from
/// `snaps` oldest first: the indexes in `snaps`, with their reasons.
fn retention(snaps: &[Snapshot], settings: &Settings) -> Vec<(usize, Kept)> {
    let timed: Vec<usize> = (0..snaps.len()).rev().filter(|&i| snaps[i].timed()).collect();
    let days: Vec<i64> = timed.iter().map(|&i| local_day(snaps[i].info.made)).collect();
    timed.into_iter().zip(keep::keep(&days, settings.daily, settings.weekly)).collect()
}

/// Makes `@snapshots` and the place's folder in it if they are not there.
fn make_folder(place: &Place) -> Result<(), Fail> {
    let snapshots = place.folder.parent().unwrap_or(&place.folder);
    if !snapshots.exists() {
        subvolume::make(snapshots)?;
    }
    match fs::create_dir(&place.folder) {
        Err(err) if err.kind() != io::ErrorKind::AlreadyExists => {
            Err(util::failed(&place.folder.display(), err))
        }
        _ => Ok(()),
    }
}

/// Makes a read-only snapshot of `place`, named `name` or by the time
/// now; returns its name.
fn create(place: &Place, name: Option<&str>) -> Result<String, Fail> {
    make_folder(place)?;
    let name = match name {
        Some(name) => name.to_string(),
        None => {
            let (year, month, day, hour, minute) = local((util::now_ns() / 1_000_000_000) as i64);
            let base = keep::timed_name(year, month, day, hour, minute);
            (1..)
                .map(|n| if n == 1 { base.clone() } else { format!("{base}.{n}") })
                .find(|name| fs::symlink_metadata(place.folder.join(name)).is_err())
                .unwrap_or(base)
        }
    };
    subvolume::snapshot_to(&place.top, &place.folder.join(&name), true)?;
    Ok(name)
}

/// Removes the snapshots of `place` that retention does not keep;
/// returns their names.
fn prune(place: &Place, settings: &Settings) -> Result<Vec<String>, Fail> {
    let snaps = snapshots(place)?;
    let mut removed = Vec::new();
    for (i, kept) in retention(&snaps, settings) {
        if kept.any() {
            continue;
        }
        let name = &snaps[i].name;
        match subvolume::remove(&place.folder.join(name)) {
            Ok(()) => removed.push(name.clone()),
            // Another round removed it first.
            Err(_) if fs::symlink_metadata(place.folder.join(name)).is_err() => {}
            Err(fail) => return Err(fail),
        }
    }
    removed.reverse();
    Ok(removed)
}

/// One round of the service: a snapshot of each place with none from
/// today, then each pruned. A place that is not there, or fails, is
/// skipped and told of, unless `told` holds the same words from the
/// round before; a place not on Btrfs, as on the live system, is passed
/// over. Returns whether every place was done.
fn auto(settings: &Settings, told: &mut Vec<(String, String)>) -> bool {
    let today = local_day(util::now_ns());
    let mut out = io::stdout();
    let mut done = true;
    for path in &settings.places {
        let round = || -> Result<(), Fail> {
            if subvolume::info(Path::new(path)).is_ok_and(|info| info.is_none()) {
                return Ok(());
            }
            let place = place(Path::new(path))?;
            let snaps = snapshots(&place)?;
            if !snaps.iter().any(|s| s.timed() && local_day(s.info.made) == today) {
                let name = create(&place, None)?;
                let _ =
                    writeln!(out.lock(), "snap: {path}: made {}", place.shown.join(name).display());
            }
            for name in prune(&place, settings)? {
                let _ = writeln!(out.lock(), "snap: {path}: removed {name}");
            }
            Ok(())
        };
        let result = round();
        let before = told.iter().position(|(at, _)| at == path);
        match (result, before) {
            (Ok(()), Some(i)) => {
                told.remove(i);
            }
            (Ok(()), None) => {}
            (Err(fail), Some(i)) if told[i].1 == fail.message => done = false,
            (Err(fail), before) => {
                let _ = writeln!(out, "snap: {}; skipped", fail.message);
                if let Some(i) = before {
                    told.remove(i);
                }
                told.push((path.clone(), fail.message));
                done = false;
            }
        }
    }
    let _ = out.flush();
    done
}

/// The path `path` names, from the working folder, without `.` and with
/// each `..` taking the name before it.
fn absolute(path: &Path) -> Result<PathBuf, Fail> {
    let joined = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir().map_err(|e| util::failed(&".", e))?.join(path)
    };
    let mut out = PathBuf::from("/");
    for part in joined.components() {
        match part {
            Component::ParentDir => {
                out.pop();
            }
            Component::Normal(name) => out.push(name),
            _ => {}
        }
    }
    Ok(out)
}

/// The top folder of the subvolume that holds `path`, which need not be
/// there, with the real path of the deepest folder of it that is; and
/// `path` from that top.
fn top_of(path: &Path) -> Result<(PathBuf, PathBuf), Fail> {
    let mut there = path;
    let mut rest = PathBuf::new();
    let real = loop {
        if let Ok(real) = fs::canonicalize(there) {
            break real;
        }
        let (Some(parent), Some(name)) = (there.parent(), there.file_name()) else {
            return Err(util::fail(format!("{}: not found", path.display())));
        };
        rest = Path::new(name).join(rest);
        there = parent;
    };
    let real = real.join(&rest);
    let mut top = real.as_path();
    loop {
        if is_top(top) && subvolume::info(top)?.is_some() {
            let from = real.strip_prefix(top).unwrap_or(Path::new("")).to_path_buf();
            return Ok((top.to_path_buf(), from));
        }
        top = top
            .parent()
            .ok_or_else(|| util::fail(format!("{}: not on a Btrfs subvolume", path.display())))?;
    }
}

/// Copies `path` back from a snapshot: the newest that holds it, or
/// `from`; to `to`, or where it was.
fn restore(path: &Path, from: Option<&str>, to: Option<&Path>) -> Result<(), Fail> {
    let path = absolute(path)?;
    let (top, inner) = top_of(&path)?;
    let place = place(&top)?;
    let snaps = snapshots(&place)?;
    let shown = |snap: &Snapshot| place.shown.join(&snap.name).join(&inner);
    let snap = match from {
        Some(name) => snaps.iter().find(|s| s.name == name).ok_or_else(|| {
            util::fail(format!(
                "{}: no snapshot {name} in {}",
                path.display(),
                place.shown.display()
            ))
        })?,
        None => snaps.iter().rev().find(|s| fs::symlink_metadata(shown(s)).is_ok()).ok_or_else(
            || {
                util::fail(format!(
                    "{}: no snapshot of {} holds it",
                    path.display(),
                    place.top.display()
                ))
            },
        )?,
    };
    let source = shown(snap);
    if fs::symlink_metadata(&source).is_err() {
        return Err(util::fail(format!("{}: not in snapshot {}", path.display(), snap.name)));
    }
    let dest = match to {
        Some(to) => absolute(to)?,
        None => path.clone(),
    };
    if fs::symlink_metadata(&dest).is_ok() {
        return Err(util::fail(format!(
            "{}: is there; restore it beside it with --to",
            dest.display()
        )));
    }
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent).map_err(|e| util::failed(&parent.display(), e))?;
    }
    // cp(1) clones what it can, so the copy takes no room until changed,
    // and keeps the modes and times, and the owners where it may.
    let status = Command::new("/system/commands/cp")
        .args(["-a", "--reflink=auto", "--"])
        .arg(&source)
        .arg(&dest)
        .status()
        .map_err(|e| util::failed(&"/system/commands/cp", e))?;
    if !status.success() {
        return Err(util::fail(format!("{}: cp could not copy it back", dest.display())));
    }
    let _ = writeln!(io::stdout(), "Restored {} from {}", dest.display(), source.display());
    Ok(())
}

/// `snap list`: each place snap.toml names, or the snapshots of one.
fn list(place_path: Option<&Path>, settings: &Settings) -> Result<i32, Fail> {
    let mut out = io::stdout().lock();
    let Some(path) = place_path else {
        let mut rows = vec![["PLACE", "SNAPSHOTS", "NEWEST", "FOLDER"].map(String::from).to_vec()];
        let mut status = 0;
        for path in &settings.places {
            match place(Path::new(path)).and_then(|p| Ok((snapshots(&p)?, p))) {
                Ok((snaps, place)) => rows.push(vec![
                    path.clone(),
                    snaps.len().to_string(),
                    snaps.last().map_or("-".into(), |s| s.name.clone()),
                    place.shown.display().to_string(),
                ]),
                Err(fail) => {
                    eprintln!("snap: {}", fail.message);
                    status = 1;
                }
            }
        }
        write_rows(&mut out, &rows)?;
        return Ok(status);
    };
    let place = place(path)?;
    let snaps = snapshots(&place)?;
    let kept = retention(&snaps, settings);
    let mut rows = vec![["NAME", "MADE", "KEPT"].map(String::from).to_vec()];
    for (i, snap) in snaps.iter().enumerate() {
        let why = match kept.iter().find(|(at, _)| *at == i) {
            Some((_, kept)) => kept.words(),
            None => "named".into(),
        };
        rows.push(vec![snap.name.clone(), made_text(&snap.info), why]);
    }
    write_rows(&mut out, &rows)?;
    Ok(0)
}

/// Writes `rows` in columns two spaces apart, the last as it is.
fn write_rows(out: &mut impl Write, rows: &[Vec<String>]) -> io::Result<()> {
    let columns = rows.first().map_or(0, Vec::len);
    let widths: Vec<usize> = (0..columns)
        .map(|c| rows.iter().map(|r| r.get(c).map_or(0, |t| t.chars().count())).max().unwrap_or(0))
        .collect();
    for row in rows {
        let mut line = String::new();
        for (c, text) in row.iter().enumerate() {
            if c + 1 == row.len() {
                line.push_str(text);
            } else {
                line.push_str(&format!("{text:<width$}  ", width = widths[c]));
            }
        }
        writeln!(out, "{}", line.trim_end())?;
    }
    Ok(())
}

/// snap(1).
pub fn main(args: &mut Args) -> Result<i32, Fail> {
    let usage = || util::usage("say create, list, restore, delete, prune or auto (snap(1))".into());
    let Some(action) = args.subcommand() else { return Err(usage()) };
    let action = action.to_str().unwrap_or("").to_string();
    let (mut name, mut from, mut to, mut watch) = (None, None, None, false);
    let mut operands = Vec::new();
    while let Some(arg) = args.next()? {
        match arg {
            Arg::Long(long) if long == "name" && action == "create" => name = Some(args.text()?),
            Arg::Long(long) if long == "from" && action == "restore" => from = Some(args.text()?),
            Arg::Long(long) if long == "to" && action == "restore" => {
                to = Some(PathBuf::from(args.value()?));
            }
            Arg::Long(long) if long == "watch" && action == "auto" => watch = true,
            Arg::Operand(operand) => operands.push(PathBuf::from(operand)),
            other => return Err(args.unknown(&other)),
        }
    }
    // Read only by what needs it, so that restoring never waits on it.
    let settings = || Settings::read();
    match (action.as_str(), operands.as_slice()) {
        ("create", [path]) => {
            if let Some(name) = &name
                && !keep::may_name(name)
            {
                return Err(util::usage(format!(
                    "'{name}' cannot name a snapshot: a name is a file name that does not \
                     start with a dot and is not a time, which snap gives its own"
                )));
            }
            let place = place(path)?;
            let made = create(&place, name.as_deref())?;
            let _ = writeln!(io::stdout(), "{}", place.shown.join(made).display());
            Ok(0)
        }
        ("list", []) => list(None, &settings()?),
        ("list", [path]) => list(Some(path), &settings()?),
        ("restore", [path]) => {
            restore(path, from.as_deref(), to.as_deref())?;
            Ok(0)
        }
        ("delete", [path, names @ ..]) if !names.is_empty() => {
            let place = place(path)?;
            for name in names {
                let snap = place.folder.join(name);
                let shown = place.shown.join(name);
                if name.components().count() != 1 || !is_top(&shown) {
                    return Err(util::fail(format!(
                        "{}: no such snapshot of {}",
                        shown.display(),
                        place.top.display()
                    )));
                }
                subvolume::remove(&snap)?;
                let _ = writeln!(io::stdout(), "Removed {}", shown.display());
            }
            Ok(0)
        }
        ("prune", paths) => {
            let settings = settings()?;
            let paths: Vec<PathBuf> = if paths.is_empty() {
                settings.places.iter().map(PathBuf::from).collect()
            } else {
                paths.to_vec()
            };
            for path in paths {
                let place = place(&path)?;
                for name in prune(&place, &settings)? {
                    let _ = writeln!(io::stdout(), "Removed {}", place.shown.join(name).display());
                }
            }
            Ok(0)
        }
        ("auto", []) if watch => {
            let mut told = Vec::new();
            loop {
                // Read again each round, so that a change needs no restart.
                match settings() {
                    Ok(settings) => {
                        auto(&settings, &mut told);
                    }
                    Err(fail) => println!("snap: {}; nothing done this round", fail.message),
                }
                std::thread::sleep(ROUND);
            }
        }
        ("auto", []) => Ok(if auto(&settings()?, &mut Vec::new()) { 0 } else { 1 }),
        _ => Err(usage()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_take_the_defaults_for_what_they_leave_out() {
        assert_eq!(Settings::parse("").unwrap(), Settings::default());
        let set = Settings::parse("places = [\"/users\", \"/volumes/sdb1\"]\ndaily = 3\n").unwrap();
        assert_eq!(set.places, ["/users", "/volumes/sdb1"]);
        assert_eq!((set.daily, set.weekly), (3, 4));
        assert!(Settings::parse("monthly = 2\n").is_err());
        // The file the system ships says what the defaults are.
        let shipped = include_str!("../../../defaults/snap.toml");
        assert_eq!(Settings::parse(shipped).unwrap(), Settings::default());
    }

    #[test]
    fn a_subvolume_names_its_folder() {
        let info = |id: u64, name: &str| Info { id, name: name.into(), ..Info::default() };
        assert_eq!(folder_name(&info(257, "@users")), "users");
        assert_eq!(folder_name(&info(260, "@settings")), "settings");
        assert_eq!(folder_name(&info(300, "data")), "data");
        assert_eq!(folder_name(&info(TOP_ID, "")), "top");
    }

    #[test]
    fn days_go_back_to_dates() {
        for day in [-1, 0, 11_017, 21_915, 21_954, 100_000] {
            let (y, m, d) = civil(day);
            assert_eq!(keep::day_number(y, m, d), day, "{day}");
        }
        assert_eq!(civil(21_954), (2030, 2, 9));
    }

    #[test]
    fn paths_are_made_absolute_by_their_names() {
        assert_eq!(
            absolute(Path::new("/users/alice/../bob/./x")).unwrap(),
            Path::new("/users/bob/x")
        );
        assert_eq!(absolute(Path::new("/..")).unwrap(), Path::new("/"));
    }
}
