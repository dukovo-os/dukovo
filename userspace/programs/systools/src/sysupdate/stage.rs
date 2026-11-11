// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! `sysupdate stage`: a signed release built into the spare slot (ADR
//! 0451). The spare starts as a snapshot of the running slot,
//! `@system.new`, and only the files the release changes are written
//! into it, from the repository's store, so a repository needs to hold
//! only those; what the release no longer has is removed. Then every
//! file is read back against the signed list, and the slot is sealed
//! read-only in the spare's place. The settings and the state are
//! snapshotted for a fallback (ADR 0454), and last the slot state names
//! the spare as the update to try. Until that last write the running
//! slot is the one that starts, whatever happens; a slot left half built
//! is removed at the next start, by the health check.

use std::collections::HashSet;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};

use slots::restore;
use slots::version::Version;
use system_release::list::{Entry, Kind, Release, sha256_file};
use system_release::store::Store;

use super::place::{self, BUILDING, Root};
use crate::subvolume;
use crate::util::{Fail, fail};

/// The root this system trusts to sign its releases (ADR 0308).
const TRUSTED_ROOT: &str = "/system/resources/updates/root.json";

/// A step after which a boot test can have the stage stop, as a power cut
/// there would (`--test-stop-after`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stop {
    /// `@system.new` is built, and not yet sealed.
    Built,
}

/// What `sysupdate stage` was asked.
pub struct Asked<'a> {
    /// The repository: a folder with the release's TUF metadata and its
    /// store, as `ob release` makes one and the update server serves it.
    pub repo: &'a Path,
    /// The release's version, when the repository has more than one.
    pub version: Option<&'a str>,
    /// Where to stop, for a boot test.
    pub stop: Option<Stop>,
}

/// Says what the stage is doing.
fn say(what: &str) {
    println!("sysupdate: {what}");
}

/// Stages the release `asked` names in the spare slot.
///
/// # Errors
///
/// What stopped it, in words for the administrator: the slot state
/// refuses (an update is being tried, or this is not the default), the
/// release is not signed by the trusted root, is not newer, or needs a
/// file its repository lacks, or a write failed. The running slot and
/// the slot state are untouched by a stage that fails.
pub fn stage(root: &Root, asked: &Asked<'_>) -> Result<(), Fail> {
    let _lock = place::lock()?;
    let (copies, state) = place::state(root)?;
    // Refused now, before anything is built, as it would be at the end.
    let next = state.stage(root.running).map_err(|why| fail(why.to_string()))?;
    let spare = root.running.other();

    say("checking the release against the root this system trusts");
    let trusted =
        std::fs::read(TRUSTED_ROOT).map_err(|err| crate::util::failed(&TRUSTED_ROOT, err))?;
    let published =
        system_release::repo::open(asked.repo, &trusted, asked.version).map_err(fail)?;
    let release = published.release;
    newer(root, &release)?;

    let building = root.path(BUILDING);
    if building.exists() {
        say("removing a slot an earlier stage left half built");
        subvolume::remove(&building)?;
    }
    say(&format!(
        "building slot {} from slot {} and the release's changes",
        spare.letter().to_ascii_uppercase(),
        root.running.letter().to_ascii_uppercase()
    ));
    subvolume::snapshot_to(&root.slot(root.running), &building, false)?;
    let store = Store::at(&asked.repo.join("files"));
    let changed = build(&building, &release, &store)?;
    say(&format!("wrote {changed} changed files; reading the slot back against the signed list"));
    check(&building, &release)?;
    if asked.stop == Some(Stop::Built) {
        say("stopped after building, as --test-stop-after asks");
        loop {
            std::thread::sleep(std::time::Duration::from_secs(3600));
        }
    }

    // Sealed in the spare's place: what it held, the version before the
    // running one or an update not tried, goes, once no copy of the slot
    // state names it, the older included, so that no start goes to it
    // while it is rebuilt (SlotState.tla).
    let named = copies
        .iter()
        .flat_map(|esp| [esp.first, esp.second])
        .flatten()
        .any(|copy| copy.names(spare));
    if named {
        say(&format!(
            "slot {} is no longer offered at startup while it is rebuilt",
            spare.letter().to_ascii_uppercase()
        ));
        place::write_both(root, state.forget(spare))?;
    }
    let slot = root.slot(spare);
    if slot.exists() {
        subvolume::remove(&slot)?;
    }
    subvolume::snapshot_to(&building, &slot, true)?;
    subvolume::remove(&building)?;

    say("snapshotting the settings and the state, for a fallback");
    snapshot_for_fallback(root, &release)?;

    let (copies, _) = place::state(root)?;
    place::write(&copies, next)?;
    say(&format!(
        "staged Dukovo {} in slot {}: it starts at the next restart, with {} tries to prove \
         itself, and Dukovo {} starts again if it does not",
        release.version,
        spare.letter().to_ascii_uppercase(),
        slots::state::TRIES,
        root.about(root.running).map_or_else(|| "of the running slot".into(), |a| a.version)
    ));
    Ok(())
}

/// Refuses a release that is not newer than the running slot's, or one
/// the running version cannot update to directly.
fn newer(root: &Root, release: &Release) -> Result<(), Fail> {
    let running = root.about(root.running).ok_or_else(|| {
        fail("the running slot does not say its version (startup/slot.env)".into())
    })?;
    let (Some(from), Some(to)) =
        (Version::parse(&running.version), Version::parse(&release.version))
    else {
        return Err(fail(format!(
            "cannot compare the versions {} and {}",
            running.version, release.version
        )));
    };
    // By precedence: a build of the same version is not newer.
    if to <= from {
        return Err(fail(format!(
            "the release is Dukovo {}, not newer than the running {}",
            release.version, running.version
        )));
    }
    if let Some(oldest) = Version::parse(&release.updates_from)
        && from < oldest
    {
        return Err(fail(format!(
            "Dukovo {} updates only from {} on; stage a release between them first",
            release.version, release.updates_from
        )));
    }
    Ok(())
}

/// Makes the tree at `slot`, a writable snapshot of the running slot, the
/// release's: what it does not list is removed, and each file it lists
/// whose contents differ is written from `store`. Returns how many files
/// were written.
fn build(slot: &Path, release: &Release, store: &Store) -> Result<usize, Fail> {
    let listed: HashSet<&str> = release.files.iter().map(|e| e.path.as_str()).collect();
    // Directories writable while the slot is built; the release's modes
    // come last.
    let mut found = Vec::new();
    walk(slot, "", &mut found)?;
    for (path, is_dir) in &found {
        let full = slot.join(path);
        if !listed.contains(path.as_str()) {
            let removed =
                if *is_dir { std::fs::remove_dir_all(&full) } else { std::fs::remove_file(&full) };
            removed.map_err(|err| crate::util::failed(&full.display(), err))?;
        } else if *is_dir {
            set_mode(&full, 0o755)?;
        }
    }
    let mut written = 0;
    for entry in &release.files {
        let full = slot.join(&entry.path);
        let there = std::fs::symlink_metadata(&full).ok();
        match entry.kind {
            Kind::Directory => {
                if there.as_ref().is_some_and(|m| !m.is_dir()) {
                    remove(&full)?;
                }
                if !full.is_dir() {
                    std::fs::create_dir(&full)
                        .map_err(|err| crate::util::failed(&full.display(), err))?;
                    set_mode(&full, 0o755)?;
                }
            }
            Kind::Link => {
                let target = entry.target.as_deref().unwrap_or_default();
                let same = std::fs::read_link(&full).is_ok_and(|t| t == Path::new(target));
                if !same {
                    if there.is_some() {
                        remove(&full)?;
                    }
                    std::os::unix::fs::symlink(target, &full)
                        .map_err(|err| crate::util::failed(&full.display(), err))?;
                }
            }
            Kind::File => {
                let (sha256, size) =
                    entry.file().ok_or_else(|| fail(format!("{}: no hash", entry.path)))?;
                let same = there.as_ref().is_some_and(|m| m.is_file() && m.len() == size)
                    && sha256_file(&full).is_ok_and(|h| h == sha256);
                if !same {
                    if there.is_some() {
                        remove(&full)?;
                    }
                    let from = store.path(sha256);
                    if !from.is_file() {
                        return Err(fail(format!(
                            "the repository does not hold /system/{} ({sha256}), which this \
                             system's differs from: it was made for an update from another version",
                            entry.path
                        )));
                    }
                    std::fs::copy(&from, &full)
                        .map_err(|err| crate::util::failed(&full.display(), err))?;
                    written += 1;
                }
                set_mode(&full, entry.mode_bits())?;
            }
        }
    }
    // The directories' own modes, deepest first.
    for entry in release.files.iter().rev().filter(|e| e.kind == Kind::Directory) {
        set_mode(&slot.join(&entry.path), entry.mode_bits())?;
    }
    Ok(written)
}

/// Checks the tree at `slot` against the release, file by file, read back
/// from the disk: every entry there, of its kind, each file's SHA-256 and
/// size, each link's target, each mode, and nothing more.
fn check(slot: &Path, release: &Release) -> Result<(), Fail> {
    let mut problems = Vec::new();
    for entry in &release.files {
        if let Err(why) = check_entry(&slot.join(&entry.path), entry) {
            problems.push(format!("/system/{}: {why}", entry.path));
        }
    }
    let mut found = Vec::new();
    walk(slot, "", &mut found)?;
    if found.len() != release.files.len() {
        problems.push(format!(
            "the slot holds {} entries, not the release's {}",
            found.len(),
            release.files.len()
        ));
    }
    if problems.is_empty() {
        return Ok(());
    }
    problems.truncate(10);
    Err(fail(format!("the slot read back is not the release: {}", problems.join("; "))))
}

/// Checks one entry of the slot read back.
fn check_entry(full: &Path, entry: &Entry) -> Result<(), String> {
    let meta = std::fs::symlink_metadata(full).map_err(|err| err.to_string())?;
    match entry.kind {
        Kind::Directory if !meta.is_dir() => return Err("not a directory".into()),
        Kind::Link => {
            let target = std::fs::read_link(full).map_err(|err| err.to_string())?;
            if target != Path::new(entry.target.as_deref().unwrap_or_default()) {
                return Err(format!("leads to {}", target.display()));
            }
            return Ok(());
        }
        Kind::File => {
            let (sha256, size) = entry.file().ok_or("no hash")?;
            if !meta.is_file() || meta.len() != size || sha256_file(full)? != sha256 {
                return Err("not the file the release lists".into());
            }
        }
        Kind::Directory => {}
    }
    if meta.permissions().mode() & 0o7777 != entry.mode_bits() {
        return Err(format!("mode {:o}, not {}", meta.permissions().mode() & 0o7777, entry.mode));
    }
    Ok(())
}

/// Every entry beneath `dir`, as paths relative to the slot (`prefix`
/// and on), parents first, and whether each is a directory.
fn walk(dir: &Path, prefix: &str, out: &mut Vec<(String, bool)>) -> Result<(), Fail> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .map_err(|err| crate::util::failed(&dir.display(), err))?
        .filter_map(Result::ok)
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    for name in names {
        let path = if prefix.is_empty() { name.clone() } else { format!("{prefix}/{name}") };
        let full = dir.join(&name);
        let is_dir = std::fs::symlink_metadata(&full).is_ok_and(|m| m.is_dir());
        out.push((path.clone(), is_dir));
        if is_dir {
            walk(&full, &path, out)?;
        }
    }
    Ok(())
}

/// Removes what is at `path`: a directory with all it holds, or a file
/// or link.
fn remove(path: &Path) -> Result<(), Fail> {
    let removed = if std::fs::symlink_metadata(path).is_ok_and(|m| m.is_dir()) {
        std::fs::remove_dir_all(path)
    } else {
        std::fs::remove_file(path)
    };
    removed.map_err(|err| crate::util::failed(&path.display(), err))
}

/// Gives `path` the mode `mode`.
fn set_mode(path: &Path, mode: u32) -> Result<(), Fail> {
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
        .map_err(|err| crate::util::failed(&path.display(), err))
}

/// Snapshots `@settings` and `@state`, read-only, into the update's
/// folder in `@snapshots`, in place of an earlier update's, with what
/// the update is (ADR 0454).
fn snapshot_for_fallback(root: &Root, release: &Release) -> Result<(), Fail> {
    let update = root.path(restore::UPDATE);
    clear_update(&update)?;
    std::fs::create_dir_all(&update).map_err(|err| crate::util::failed(&update.display(), err))?;
    subvolume::snapshot_to(
        &root.path(restore::SETTINGS),
        &update.join(restore::BEFORE_SETTINGS),
        true,
    )?;
    subvolume::snapshot_to(&root.path("@state"), &update.join(restore::BEFORE_STATE), true)?;
    let from = root.about(root.running).map_or_default(|about| about.version);
    let about = format!(
        "from = {from}\nto = {}\nslot = {}\nstaged = {}\n",
        release.version,
        root.running.other().letter(),
        crate::util::now_ns() / 1_000_000_000
    );
    let path = update.join(restore::ABOUT);
    std::fs::write(&path, about).map_err(|err| crate::util::failed(&path.display(), err))
}

/// Removes an earlier update's folder: its snapshots, which are
/// subvolumes, then the rest.
pub fn clear_update(update: &Path) -> Result<(), Fail> {
    if !update.exists() {
        return Ok(());
    }
    for name in [restore::BEFORE_SETTINGS, restore::BEFORE_STATE, restore::FAILED_SETTINGS] {
        let path: PathBuf = update.join(name);
        if path.exists() {
            subvolume::remove(&path)?;
        }
    }
    std::fs::remove_dir_all(update).map_err(|err| crate::util::failed(&update.display(), err))
}

/// What the about file of the update's folder says, as `key = value`
/// lines, for `sysupdate status`.
pub fn about_update(root: &Root) -> Option<Vec<(String, String)>> {
    let text = std::fs::read_to_string(root.path(restore::UPDATE).join(restore::ABOUT)).ok()?;
    Some(
        text.lines()
            .filter_map(|line| line.split_once(" = "))
            .map(|(key, value)| (key.to_string(), value.to_string()))
            .collect(),
    )
}

/// Whether a fallback put the settings from before the update back.
pub fn restored(root: &Root) -> bool {
    root.path(restore::UPDATE).join(restore::RESTORED).exists()
}
