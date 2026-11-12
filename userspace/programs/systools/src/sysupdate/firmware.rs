// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! `sysupdate firmware`: the optional firmware package as firmware sets
//! that each slot names (ADR 0468). Closed firmware stays out of the
//! system (ADR 0007): a set's files are kept once each, by SHA-256, in
//! the root's `@firmware` subvolume, with each set's signed list, and a
//! slot names the set it starts with in `startup/firmware.env`, which
//! GRUB reads to load the set's microcode. A new set is staged as a new
//! slot: a snapshot of the running one that names it. So its first start
//! is a try, the health check judges it as it judges a new version, and
//! one that fails falls back to the slot before it, with the set before
//! it. A system update keeps the set the running slot names.
//!
//! A machine takes the firmware package when its owner stages a set the
//! first time, from the update server's channel, a repository folder or
//! a set's version; `sysupdate firmware remove` stages a slot that
//! names none. When a set will not start, `sysupdate firmware` says
//! which set each slot names, and the health check's line in the log
//! (`log sysupdate`) says what failed.

use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};

use slots::firmware::{Named, PATH, STORE};
use system_release::firmware::{self as sets, Load, Set};
use system_release::list::sha256_file;

use super::online;
use super::place::{self, BUILDING, Root};
use super::stage::{self, TRUSTED_ROOT, say};
use crate::args::{Arg, Args};
use crate::subvolume;
use crate::util::{self, Fail, fail};

/// `sysupdate firmware [stage [REPOSITORY] [--version VERSION] | remove]`.
pub fn main(args: &mut Args) -> Result<i32, Fail> {
    let action = args.subcommand().map(|a| a.to_string_lossy().into_owned());
    let (mut version, mut repo) = (None, None);
    while let Some(arg) = args.next()? {
        match arg {
            Arg::Long(name) if name == "version" && action.as_deref() == Some("stage") => {
                version = Some(args.text()?);
            }
            Arg::Operand(path) if action.as_deref() == Some("stage") && repo.is_none() => {
                repo = Some(PathBuf::from(path));
            }
            other => return Err(args.unknown(&other)),
        }
    }
    let root = place::root()?;
    match action.as_deref() {
        None => status(&root),
        Some("stage") => stage(&root, repo.as_deref(), version.as_deref()).map(|()| 0),
        Some("remove") => remove(&root).map(|()| 0),
        Some(other) => Err(util::usage(format!("no firmware action {other}: say stage or remove"))),
    }
}

/// The firmware set the slot at `slot` names, if it names one.
pub fn named(slot: &Path) -> Option<Named> {
    Named::decode(&std::fs::read(slot.join(PATH)).ok()?)
}

/// Makes the slot being built at `slot` name `set`, or none.
///
/// # Errors
///
/// If the file cannot be written or removed.
pub fn name(slot: &Path, set: Option<&Named>) -> Result<(), Fail> {
    let path = slot.join(PATH);
    match set {
        Some(set) => std::fs::write(&path, set.encode()),
        None if path.exists() => std::fs::remove_file(&path),
        None => return Ok(()),
    }
    .map_err(|err| util::failed(&path.display(), err))
}

/// `sysupdate firmware`: which set each slot names, and its files.
fn status(root: &Root) -> Result<i32, Fail> {
    let store = root.path(STORE);
    for slot in [slots::Slot::A, slots::Slot::B] {
        let path = root.slot(slot);
        let letter = slot.letter().to_ascii_uppercase();
        let running = if slot == root.running { ", running" } else { "" };
        if !path.exists() {
            println!("Slot {letter}:   empty");
            continue;
        }
        let Some(named) = named(&path) else {
            println!("Slot {letter}:   no firmware set{running}");
            continue;
        };
        println!("Slot {letter}:   firmware {}{running}", named.set);
        match read_set(&store, &named.set) {
            Ok((set, _)) => {
                for file in &set.files {
                    let load = match file.load {
                        Load::Microcode => "CPU microcode",
                    };
                    println!(
                        "            {} ({load}, {} KiB, {})",
                        file.name,
                        file.size.div_ceil(1024),
                        file.license
                    );
                }
            }
            Err(why) => println!("            {why}"),
        }
    }
    let takes = named(&root.slot(root.running)).is_some();
    println!(
        "Firmware package: {}",
        if takes {
            "taken; sysupdate firmware stage stages a newer set"
        } else {
            "not taken; sysupdate firmware stage takes it"
        }
    );
    Ok(0)
}

/// The set `version`'s list as the store keeps it, and its bytes.
fn read_set(store: &Path, version: &str) -> Result<(Set, Vec<u8>), String> {
    let path = store.join("sets").join(format!("{version}.json"));
    let bytes = std::fs::read(&path).map_err(|err| format!("{}: {err}", path.display()))?;
    let set = Set::parse(&bytes)?;
    if set.firmware != version {
        return Err(format!("{} is the list of firmware {}", path.display(), set.firmware));
    }
    Ok((set, bytes))
}

/// Where the files of a set come from.
enum Source {
    /// A repository folder's store.
    Folder(PathBuf),
    /// The update server.
    Server(Box<online::Client>),
}

/// `sysupdate firmware stage`: the set `version` of the repository folder
/// `repo`, or the newest there, or, with no folder, the set the update
/// server's channel offers or `version`, staged in the spare slot as a
/// snapshot of the running one that names it.
pub(crate) fn stage(root: &Root, repo: Option<&Path>, version: Option<&str>) -> Result<(), Fail> {
    let _lock = place::lock()?;
    let (copies, state) = place::state(root)?;
    let next = state.stage(root.running).map_err(|why| fail(why.to_string()))?;
    let (set, list, source) = match repo {
        Some(repo) => {
            say("checking the firmware set against the root this system trusts");
            let trusted =
                std::fs::read(TRUSTED_ROOT).map_err(|err| util::failed(&TRUSTED_ROOT, err))?;
            let (set, list) = sets::open(repo, &trusted, version).map_err(fail)?;
            (set, list, Source::Folder(repo.join("files")))
        }
        None => {
            let settings = online::Settings::load()?;
            say(&format!("reading {} against the root this system trusts", settings.server));
            let mut connected = online::Client::connect(settings)?;
            let server = connected.settings.server.clone();
            let version = match version {
                Some(version) => version.to_string(),
                None => connected.channel()?.firmware.ok_or_else(|| {
                    fail(format!(
                        "the {} channel of {server} offers no firmware set",
                        connected.settings.channel
                    ))
                })?,
            };
            let (set, list) = connected
                .firmware(&version)?
                .ok_or_else(|| fail(format!("{server} does not offer firmware {version}")))?;
            (set, list, Source::Server(Box::new(connected)))
        }
    };
    let running = named(&root.slot(root.running));
    if let Some(running) = &running {
        let compared = sets::compare(&set.firmware, &running.set);
        if compared != std::cmp::Ordering::Greater {
            return Err(fail(format!(
                "the set is firmware {}, not newer than the running slot's {}",
                set.firmware, running.set
            )));
        }
    }
    say(&format!("putting firmware {} in {STORE}", set.firmware));
    keep(root, &set, &list, &source)?;
    let microcode = set.microcode().map(|file| file.sha256.clone());
    let set_named = Named { set: set.firmware.clone(), microcode };
    let from = running
        .as_ref()
        .map_or_else(|| "no firmware set".into(), |n| format!("firmware {}", n.set));
    build(
        root,
        &copies,
        state,
        next,
        Some(&set_named),
        &from,
        &format!("firmware {}", set.firmware),
    )
}

/// `sysupdate firmware remove`: a slot that names no firmware set, staged
/// as an update.
fn remove(root: &Root) -> Result<(), Fail> {
    let _lock = place::lock()?;
    let (copies, state) = place::state(root)?;
    let Some(running) = named(&root.slot(root.running)) else {
        return Err(fail("the running slot starts with no firmware set: nothing to remove".into()));
    };
    let next = state.stage(root.running).map_err(|why| fail(why.to_string()))?;
    build(root, &copies, state, next, None, &format!("firmware {}", running.set), "no firmware set")
}

/// Builds the spare slot as a snapshot of the running one that names
/// `set`, seals it, snapshots the settings and the state for a fallback,
/// and stages it in the slot state as `next`.
fn build(
    root: &Root,
    copies: &[place::Copies],
    state: slots::State,
    next: slots::State,
    set: Option<&Named>,
    from: &str,
    to: &str,
) -> Result<(), Fail> {
    let spare = root.running.other();
    let building = root.path(BUILDING);
    if building.exists() {
        say("removing a slot an earlier stage left half built");
        subvolume::remove(&building)?;
    }
    say(&format!(
        "building slot {} from slot {}, with {to}",
        spare.letter().to_ascii_uppercase(),
        root.running.letter().to_ascii_uppercase()
    ));
    subvolume::snapshot_to(&root.slot(root.running), &building, false)?;
    name(&building, set)?;
    stage::seal(root, copies, state)?;
    say("snapshotting the settings and the state, for a fallback");
    stage::snapshot_for_fallback(root, from, to)?;
    let (copies, _) = place::state(root)?;
    place::write(&copies, next)?;
    collect(root);
    say(&format!(
        "staged {to} in slot {}: it starts at the next restart, with {} tries to prove itself, \
         and slot {}, with {from}, starts again if it does not",
        spare.letter().to_ascii_uppercase(),
        slots::state::TRIES,
        root.running.letter().to_ascii_uppercase()
    ));
    Ok(())
}

/// Puts the set's files and list into the store, each file checked
/// against its SHA-256 before it takes its name, and read-only; files
/// the store holds already are kept.
fn keep(root: &Root, set: &Set, list: &[u8], source: &Source) -> Result<(), Fail> {
    let store = root.path(STORE);
    if !store.exists() {
        subvolume::make(&store)?;
    }
    let files = store.join("files");
    let lists = store.join("sets");
    for dir in [&files, &lists] {
        std::fs::create_dir_all(dir).map_err(|err| util::failed(&dir.display(), err))?;
    }
    for file in &set.files {
        let to = files.join(&file.sha256);
        let held = std::fs::metadata(&to).is_ok_and(|m| m.len() == file.size)
            && sha256_file(&to).is_ok_and(|h| h == file.sha256);
        if held {
            continue;
        }
        match source {
            Source::Folder(from) => {
                let from = from.join(&file.sha256);
                let partial = files.join(format!(".{}.partial", file.sha256));
                std::fs::copy(&from, &partial).map_err(|err| util::failed(&from.display(), err))?;
                if sha256_file(&partial).ok().as_deref() != Some(file.sha256.as_str()) {
                    let _ = std::fs::remove_file(&partial);
                    return Err(fail(format!(
                        "{} is not the file {} of firmware {} the signed list names; nothing \
                         was changed",
                        from.display(),
                        file.name,
                        set.firmware
                    )));
                }
                std::fs::rename(&partial, &to).map_err(|err| util::failed(&to.display(), err))?;
            }
            Source::Server(client) => client.fetch_file(&file.sha256, file.size, &files)?,
        }
        std::fs::set_permissions(&to, std::fs::Permissions::from_mode(0o444))
            .map_err(|err| util::failed(&to.display(), err))?;
    }
    let path = lists.join(format!("{}.json", set.firmware));
    std::fs::write(&path, list).map_err(|err| util::failed(&path.display(), err))
}

/// Removes from the store every set no slot names, and every file none
/// of the named sets lists. Kept as it is if a named set's list cannot
/// be read; what is left is removed at a later stage.
pub fn collect(root: &Root) {
    let store = root.path(STORE);
    if !store.exists() {
        return;
    }
    let named: Vec<String> = [slots::Slot::A, slots::Slot::B]
        .iter()
        .filter_map(|&slot| named(&root.slot(slot)))
        .map(|named| named.set)
        .collect();
    let mut kept = std::collections::HashSet::new();
    for version in &named {
        match read_set(&store, version) {
            Ok((set, _)) => kept.extend(set.files.into_iter().map(|f| f.sha256)),
            Err(why) => {
                say(&format!("keeping every firmware file: {why}"));
                return;
            }
        }
    }
    let remove = |dir: &Path, keep: &dyn Fn(&str) -> bool| {
        let Ok(entries) = std::fs::read_dir(dir) else { return };
        for entry in entries.filter_map(Result::ok) {
            let name = entry.file_name().to_string_lossy().into_owned();
            if !keep(&name) {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    };
    remove(&store.join("sets"), &|name| {
        name.strip_suffix(".json").is_some_and(|v| named.iter().any(|n| n == v))
    });
    remove(&store.join("files"), &|name| kept.contains(name));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_slot_names_its_set_in_its_startup_folder() {
        let dir = std::env::temp_dir().join(format!("sysupdate-firmware-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("startup")).unwrap();
        assert_eq!(named(&dir), None);
        let set = Named { set: "2026.11.12".into(), microcode: Some("ab".repeat(32)) };
        name(&dir, Some(&set)).unwrap();
        assert_eq!(named(&dir), Some(set));
        name(&dir, None).unwrap();
        name(&dir, None).unwrap();
        assert_eq!(named(&dir), None);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
