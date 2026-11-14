// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Rescue kept current (ADR 0543). Once a start of the release that runs
//! is seen through while Rescue holds another, the first being the start
//! that proves an update, the health check asks the stage,
//! `sysupdate-stage`, to write Rescue again on each EFI system partition
//! of the root's disks: from the release's signed metadata and the files
//! of the running slot, each checked against the signed list as it is
//! written ([`rescue::write`]). So a machine updated for a year starts,
//! and reinstalls, this year's release from Rescue, not the one it was
//! installed with.
//!
//! The write runs at idle disk priority, after the start has settled, and
//! only while [`HOLDS`] names an older version than the one that runs, or
//! none; a firmware set staged alone changes no version and writes
//! nothing, and a rollback leaves Rescue on the newer release, which
//! proved itself. The new copy is written beside the one there, which
//! goes only once the new one has been read back. A partition without
//! room for both, as a 512 MiB one has not for a large release, takes the
//! new copy in place of the old once the running slot has read back
//! against the signed list, so that only a write to the partition can
//! fail on the way: for the minute that takes it holds no Rescue, and the
//! slots are untouched. A partition without room even for that, and one
//! whose write fails, end with UPD-102, and the next start tries again.
//!
//! The metadata is the release's own, which a stage keeps in [`KEPT`] as
//! a repository: from the update server, what the TUF client kept as it
//! read it (and each root it walked to); from a repository folder or an
//! update file, theirs. A system whose stage kept none lays it out from
//! what its client keeps of the server, fetching the release's signed
//! list if it lacks it. Either is read back from the root this system
//! trusts before it is used, as Rescue reads it.

use std::fs::{File, OpenOptions};
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};
use std::time::Duration;

use radix_abi::directory::{self, MountFlags};
use slots::version::Version;
use system_release::archive::Archive;
use system_release::list::{Entry, Release};
use system_release::repo::{self, Published};
use system_release::rescue::{self, Written};

use super::online::{self, Settings};
use super::place::{self, Root};
use super::request::{self, What};
use super::stage::{self, TRUSTED_ROOT};
use crate::sys::{self, SYS};
use crate::util::{self, Fail, fail};

/// The version Rescue holds on every EFI system partition of the root's
/// disks, once a write has made it so or found it so.
pub const HOLDS: &str = "/system/state/sysupdate/rescue";

/// The signed metadata of the release staged last, as a repository.
pub const KEPT: &str = "/system/state/sysupdate/release";

/// Where the metadata is laid out from what the client keeps, when the
/// stage kept none.
const LAID_OUT: &str = "/system/state/sysupdate/release.laid-out";

/// Where each EFI system partition is mounted to be written, beneath the
/// place the stage's file names in `files.mount`.
pub const MOUNTS: &str = "/system/live/sysupdate-esp";

/// How long the write waits for the slot state's lock, which the health
/// check holds for a moment and `changes bisect` for longer.
const LOCK_WAIT: Duration = Duration::from_secs(600);

/// Whether Rescue is to be written: [`HOLDS`] names an older version
/// than `running`, or none.
#[must_use]
pub fn due(running: &str) -> bool {
    held().is_none_or(|held| older(&held, running))
}

/// Whether `held` is older than `running`, or, if either is not a
/// version, another.
fn older(held: &str, running: &str) -> bool {
    match (Version::parse(held), Version::parse(running)) {
        (Some(held), Some(running)) => held < running,
        _ => held != running,
    }
}

/// The version [`HOLDS`] names, for `sysupdate status`.
#[must_use]
pub fn held() -> Option<String> {
    let text = std::fs::read_to_string(HOLDS).ok()?;
    let version = text.trim();
    (!version.is_empty()).then(|| version.to_string())
}

/// For the health check, once a start of `root`'s running release is seen
/// through: asks the stage to write Rescue from it, if it is due, and
/// does not wait. Says what it did.
pub fn ask_when_due(root: &Root) {
    let Ok(version) = stage::running_version(root) else { return };
    if !due(&version) {
        return;
    }
    match request::start(&What::Rescue) {
        Ok(true) => println!(
            "sysupdate: Rescue is being written from Dukovo {version}, at idle disk priority; \
             `log sysupdate-stage` says how it went"
        ),
        Ok(false) => println!(
            "sysupdate: Rescue is written from Dukovo {version} at a later start, as an update \
             is being staged now"
        ),
        Err(why) => println!(
            "sysupdate: cannot ask for Rescue to be written from Dukovo {version}: {} (UPD-102)",
            why.message
        ),
    }
}

/// Where a stage's release came from, for [`keep`].
pub enum Source<'a> {
    /// The update server: what the client kept, and the list it fetched.
    Server,
    /// A repository folder.
    Folder(&'a Path),
    /// An update file.
    File(&'a Archive),
}

/// Keeps the signed metadata of `release`, just staged from `source`, in
/// [`KEPT`], for Rescue once the release proves itself. Metadata that
/// cannot be kept is said, and does not stop the stage: Rescue is then
/// written from the update server's.
pub fn keep(source: &Source<'_>, release: &Release) {
    let new = PathBuf::from(format!("{KEPT}.new"));
    let kept = (|| -> Result<(), String> {
        remove(&new)?;
        match source {
            Source::Server => {
                let list = online::kept_list(&release.version)
                    .ok_or("the client did not keep the release's list")?;
                rescue::repository_from_client(
                    &online::metadata_dir(),
                    &online::roots_dir(),
                    &list,
                    &new,
                )?;
            }
            Source::Folder(repo) => {
                copy_tree(&repo.join("metadata"), &new.join("metadata"))?;
                copy_lists(&repo.join("targets/system"), &new.join("targets/system"), release)?;
            }
            Source::File(archive) => {
                archive.unpack("metadata", &new)?;
                archive.unpack("targets", &new)?;
            }
        }
        // It reads back from the root this system trusts, as Rescue
        // reads it.
        let trusted =
            std::fs::read(TRUSTED_ROOT).map_err(|err| format!("{TRUSTED_ROOT}: {err}"))?;
        repo::open(&new, &trusted, Some(&release.version))?;
        remove(Path::new(KEPT))?;
        std::fs::rename(&new, KEPT).map_err(|err| format!("{KEPT}: {err}"))
    })();
    if let Err(why) = kept {
        let _ = remove(&new);
        stage::say(&format!(
            "the release's signed metadata is not kept for Rescue ({why}); once it proves \
             itself, Rescue is written from the update server's"
        ));
    }
}

/// `sysupdate stage --request` asking `rescue`: writes Rescue of the
/// release that runs on each EFI system partition of `root`'s disks that
/// holds another, and records it in [`HOLDS`].
///
/// # Errors
///
/// UPD-102, with each partition that was not written and why; or if the
/// release's metadata cannot be had.
pub fn write_current(root: &Root) -> Result<(), Fail> {
    let version = stage::running_version(root)?;
    let _lock = place::lock_within(LOCK_WAIT)?;
    if !crate::priority::idle_io() {
        stage::say(
            "the kernel did not put this write at idle disk priority; it goes on at the usual",
        );
    }
    let trusted = std::fs::read(TRUSTED_ROOT).map_err(|err| util::failed(&TRUSTED_ROOT, err))?;
    let (repo, published) = metadata(&version, &trusted)?;
    let slot = root.slot(root.running);
    let file = |entry: &Entry| slot.join(&entry.path);
    let read_back = || stage::check(&slot, &published.release).map_err(|why| why.message);
    let mut problems = Vec::new();
    for name in place::partitions(root)? {
        match write_on(&name, &repo, &published, &trusted, &file, &read_back) {
            Ok(Some(written)) => stage::say(&format!(
                "Rescue on {name} holds Dukovo {version} now: {} files of {} MiB, each matching \
                 the signed list",
                written.files,
                written.bytes >> 20
            )),
            Ok(None) => stage::say(&format!("Rescue on {name} holds Dukovo {version} already")),
            Err(why) => problems.push(format!("{name}: {why}")),
        }
    }
    if repo == Path::new(LAID_OUT) {
        let _ = remove(&repo);
    }
    if !problems.is_empty() {
        return Err(fail(format!(
            "Rescue is left as it was on {}; the next start tries again (UPD-102)",
            problems.join("; ")
        )));
    }
    std::fs::write(HOLDS, format!("{version}\n")).map_err(|err| util::failed(&HOLDS, err))?;
    Ok(())
}

/// The signed metadata of Dukovo `version`, as a repository, and what it
/// says: the stage's, or else laid out from what the client keeps of the
/// update server; each read from the root this system trusts.
fn metadata(version: &str, trusted: &[u8]) -> Result<(PathBuf, Published), Fail> {
    let kept = Path::new(KEPT);
    if let Ok(published) = repo::open(kept, trusted, Some(version)) {
        return Ok((kept.to_path_buf(), published));
    }
    stage::say(&format!(
        "the stage of Dukovo {version} kept no signed metadata of it: laying it out from what \
         the update server signed"
    ));
    let list = match online::kept_list(version) {
        Some(list) => list,
        None => {
            let mut client = online::Client::connect(Settings::load()?)?;
            let server = client.settings.server.clone();
            client
                .release(version)?
                .ok_or_else(|| fail(format!("{server} does not offer Dukovo {version}")))?;
            online::kept_list(version)
                .ok_or_else(|| fail(format!("the list of Dukovo {version} was not kept")))?
        }
    };
    let laid = Path::new(LAID_OUT);
    rescue::repository_from_client(&online::metadata_dir(), &online::roots_dir(), &list, laid)
        .map_err(fail)?;
    let published = repo::open(laid, trusted, Some(version)).map_err(|why| {
        let _ = remove(laid);
        fail(format!(
            "what the update server signed does not read back from this system's root: {why}"
        ))
    })?;
    Ok((laid.to_path_buf(), published))
}

/// Writes Rescue of `published` on the EFI system partition `name`,
/// mounted for the while: beside the copy there, or in its place, after
/// `read_back` has read the files back, when there is no room for both.
/// `None` if it holds that release's signed list already.
fn write_on(
    name: &str,
    repo: &Path,
    published: &Published,
    trusted: &[u8],
    file: &dyn Fn(&Entry) -> PathBuf,
    read_back: &dyn Fn() -> Result<(), String>,
) -> Result<Option<Written>, String> {
    let at = Path::new(MOUNTS).join(name);
    std::fs::create_dir_all(&at).map_err(|err| format!("{}: {err}", at.display()))?;
    let device = OpenOptions::new()
        .read(true)
        .write(true)
        .open(format!("/devices/{name}"))
        .map_err(|err| format!("/devices/{name}: {}", util::strip_os_error(&err)))?;
    if let Err(why) = mount(&at, &device) {
        let _ = std::fs::remove_dir(&at);
        return Err(format!("cannot mount it: {why}"));
    }
    let release = &published.release;
    let result = (|| {
        let current = at.join(rescue::DIR);
        if rescue::open(&current, trusted).is_ok_and(|held| held.name == published.name) {
            return Ok(None);
        }
        let space = crate::df::statvfs(at.as_os_str())
            .map_err(|err| format!("how full it is: {}", util::strip_os_error(&err)))?;
        // What the write removes before it starts: a copy it left half
        // written, and one a switch cut short left behind a whole Rescue.
        let left = if current.exists() { size_of(&at.join(rescue::OLD)) } else { 0 };
        let free = space.available + size_of(&at.join(rescue::NEW)) + left;
        let needed = rescue::needed(release);
        if free >= needed {
            if left > 0 {
                stage::say(&format!(
                    "removing {} on {name}, which a cut-short write left",
                    rescue::OLD
                ));
            }
            return rescue::write(&at, repo, release, file).map(Some);
        }
        let held = size_of(&current);
        if free + held < needed {
            return Err(format!(
                "it has {} MiB free besides Rescue's {} MiB, and Rescue of Dukovo {} needs {} MiB",
                free >> 20,
                held >> 20,
                release.version,
                needed >> 20
            ));
        }
        stage::say(&format!(
            "{name} has no room for the new copy beside the one there: once the running slot has \
             read back against the signed list, it takes the new one in its place, and for the \
             minute that takes it holds no Rescue"
        ));
        read_back()?;
        rescue::replace(&at, repo, release, file).map(Some)
    })();
    let unmounted = unmount(&at);
    let _ = std::fs::remove_dir(&at);
    match (result, unmounted) {
        (Err(why), _) => Err(why),
        (Ok(_), Err(why)) => Err(format!("cannot unmount it once written: {why}")),
        (Ok(done), Ok(())) => Ok(done),
    }
}

/// Mounts the FAT filesystem of `device` at `at`, through the place the
/// stage's file names in `files.mount`, as nothing is mounted through a
/// view.
fn mount(at: &Path, device: &File) -> Result<(), String> {
    let (base, path) = crate::volumed::mount_base(&at.to_string_lossy());
    let source = sys::slot_of(device.as_raw_fd());
    directory::mount(&SYS, base, path.as_bytes(), b"fat", source, MountFlags::NONE)
        .map_err(|status| util::strip_os_error(&sys::io_error(status)))
}

/// Unmounts what [`mount`] mounted at `at`.
fn unmount(at: &Path) -> Result<(), String> {
    let (base, path) = crate::volumed::mount_base(&at.to_string_lossy());
    directory::unmount(&SYS, base, path.as_bytes())
        .map_err(|status| util::strip_os_error(&sys::io_error(status)))
}

/// The bytes of the files beneath `path`, or 0 if there is nothing there.
fn size_of(path: &Path) -> u64 {
    let Ok(meta) = std::fs::symlink_metadata(path) else { return 0 };
    if !meta.is_dir() {
        return meta.len();
    }
    std::fs::read_dir(path)
        .map_or(0, |entries| entries.flatten().map(|entry| size_of(&entry.path())).sum())
}

/// Copies the files of the tree at `from` into `to`, folders and all.
fn copy_tree(from: &Path, to: &Path) -> Result<(), String> {
    std::fs::create_dir_all(to).map_err(|err| format!("{}: {err}", to.display()))?;
    let entries = std::fs::read_dir(from).map_err(|err| format!("{}: {err}", from.display()))?;
    for entry in entries {
        let entry = entry.map_err(|err| format!("{}: {err}", from.display()))?;
        let (source, target) = (entry.path(), to.join(entry.file_name()));
        let kind = entry.file_type().map_err(|err| format!("{}: {err}", source.display()))?;
        if kind.is_dir() {
            copy_tree(&source, &target)?;
        } else if kind.is_file() {
            std::fs::copy(&source, &target)
                .map_err(|err| format!("{} to {}: {err}", source.display(), target.display()))?;
        }
    }
    Ok(())
}

/// Copies the lists of `release` in the folder `from`, a repository's
/// `targets/system`, under the names a client fetches them by, into `to`.
fn copy_lists(from: &Path, to: &Path, release: &Release) -> Result<(), String> {
    std::fs::create_dir_all(to).map_err(|err| format!("{}: {err}", to.display()))?;
    let plain = format!("{}.json", release.version);
    let hashed = format!(".{plain}");
    let entries = std::fs::read_dir(from).map_err(|err| format!("{}: {err}", from.display()))?;
    let mut copied = 0;
    for entry in entries.flatten() {
        let Some(name) = entry.file_name().to_str().map(str::to_string) else { continue };
        if name == plain || name.ends_with(&hashed) {
            std::fs::copy(entry.path(), to.join(&name))
                .map_err(|err| format!("{}: {err}", entry.path().display()))?;
            copied += 1;
        }
    }
    if copied == 0 {
        return Err(format!("{} holds no list of Dukovo {}", from.display(), release.version));
    }
    Ok(())
}

/// Removes `path` and all it holds, if it is there.
fn remove(path: &Path) -> Result<(), String> {
    match std::fs::remove_dir_all(path) {
        Err(err) if err.kind() != std::io::ErrorKind::NotFound => {
            Err(format!("removing {}: {err}", path.display()))
        }
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rescue_follows_a_newer_release_and_stays_on_a_newer_one_after_a_rollback() {
        assert!(older("0.15.0", "0.15.1"));
        assert!(older("0.16.0-alpha.1", "0.16.0"));
        assert!(!older("0.15.1", "0.15.1"));
        assert!(!older("0.15.1", "0.15.0"));
        assert!(older("built-by-hand", "0.15.1"));
        assert!(!older("x", "x"));
    }

    #[test]
    fn a_file_or_a_folder_of_files_is_counted_whole_and_nothing_as_nothing() {
        let dir = std::env::temp_dir().join(format!("keep-rescue-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("a/b")).unwrap();
        std::fs::write(dir.join("a/one"), [0u8; 10]).unwrap();
        std::fs::write(dir.join("a/b/two"), [0u8; 32]).unwrap();
        assert_eq!(size_of(&dir.join("a")), 42);
        assert_eq!(size_of(&dir.join("a/one")), 10);
        assert_eq!(size_of(&dir.join("none")), 0);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn only_the_lists_of_the_release_are_copied() {
        let dir = std::env::temp_dir().join(format!("keep-rescue-lists-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let from = dir.join("from");
        std::fs::create_dir_all(&from).unwrap();
        for name in ["ab12.0.15.1.json", "0.15.1.json", "cd34.0.15.0.json", "0.15.10.json"] {
            std::fs::write(from.join(name), name).unwrap();
        }
        let release = Release {
            format: 1,
            version: "0.15.1".into(),
            commit: "01d66b7".into(),
            toolchain: "nightly-2026-10-15".into(),
            built: "2026-11-14T00:00:00Z".into(),
            updates_from: "0.15.0".into(),
            files: Vec::new(),
        };
        copy_lists(&from, &dir.join("to"), &release).unwrap();
        let mut names: Vec<String> = std::fs::read_dir(dir.join("to"))
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        names.sort();
        assert_eq!(names, ["0.15.1.json", "ab12.0.15.1.json"]);
        let none = Release { version: "0.16.0".into(), ..release };
        assert!(copy_lists(&from, &dir.join("none"), &none).unwrap_err().contains("no list"));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
