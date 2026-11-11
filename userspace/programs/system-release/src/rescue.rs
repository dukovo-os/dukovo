// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Rescue on the EFI system partition (ADRs 0459 and 0460): the live
//! system of a release, kept in `boot/rescue` beside GRUB, which starts
//! when no slot will and can put the system back from what it holds.
//!
//! It is made of the release itself, so it is checked as the release is:
//!
//! - `startup/`: the release's own startup folder, its kernel and
//!   startup archive, the menu and what the slot says of itself. Rescue
//!   starts with that kernel, and a reinstall puts the folder back.
//! - `system`: the boot archive of every other file of the release under
//!   `system/`, as the install stick's is, which the kernel loads whole
//!   and Spark runs from memory with no disk as the root (`live`).
//! - `release/`: the repository's signed metadata and the release's list,
//!   but not its store: the files are those of `system` and `startup/`.
//!
//! A new copy is written beside the old in `boot/rescue.new`, read back,
//! and then switched in by two renames, so that a power cut leaves one
//! whole copy: `rescue` or, between the renames, `rescue.old`, which the
//! boot menu starts in its place.

use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};

use radix_bootfs::write::{END, Headers, padding};

use crate::list::{Entry, Kind, Release, hex};
use crate::repo::{self, Published};

/// Rescue's folder on the EFI system partition.
pub const DIR: &str = "boot/rescue";

/// Where a new copy is written before it is switched in.
pub const NEW: &str = "boot/rescue.new";

/// Where the copy it replaces is, between the two renames of a switch.
pub const OLD: &str = "boot/rescue.old";

/// The release's startup folder, in Rescue's folder and in the release.
pub const STARTUP: &str = "startup";

/// The boot archive, in Rescue's folder.
pub const ARCHIVE: &str = "system";

/// The signed metadata and the release's list, in Rescue's folder.
pub const RELEASE: &str = "release";

/// The folders of a repository Rescue keeps: not `files`, its store.
const KEPT: [&str; 2] = ["metadata", "targets"];

/// What [`write`] put on the partition.
#[derive(Debug, PartialEq, Eq)]
pub struct Written {
    /// How many of the release's files are in Rescue.
    pub files: usize,
    /// Rescue's size in bytes: the archive and the startup folder.
    pub bytes: u64,
}

/// Where `entry` of a release is in a Rescue at `rescue` that runs with
/// the release's `/system` at `system`: a startup file in Rescue's own
/// startup folder, and every other file in `/system`, which the boot
/// archive holds.
#[must_use]
pub fn source(rescue: &Path, system: &Path, entry: &Entry) -> PathBuf {
    match entry.path.strip_prefix("startup/") {
        Some(name) => rescue.join(STARTUP).join(name),
        None => system.join(&entry.path),
    }
}

/// Writes Rescue of `release`, whose signed metadata is in the
/// repository at `repo` and whose files `file` finds, onto the EFI
/// system partition mounted at `esp`, beside the copy there, and
/// switches to it once it is whole and read back.
///
/// # Errors
///
/// If a file cannot be read or written, or is not what the release's
/// list says. The copy that was there is then still there, and starts.
pub fn write(
    esp: &Path,
    repo: &Path,
    release: &Release,
    file: &dyn Fn(&Entry) -> PathBuf,
) -> Result<Written, String> {
    let (new, dir, old) = (esp.join(NEW), esp.join(DIR), esp.join(OLD));
    remove(&new)?;
    // A switch cut between its renames left the old copy alone whole: it
    // is Rescue again until the new one is.
    if !dir.exists() && old.exists() {
        std::fs::rename(&old, &dir)
            .map_err(|err| format!("{} to {}: {err}", old.display(), dir.display()))?;
    }
    remove(&old)?;
    let written = write_new(&new, repo, release, file);
    if written.is_err() {
        // Not left to fill the partition: the copy there still starts.
        let _ = remove(&new);
    }
    let written = written?;
    switch(esp)?;
    Ok(written)
}

/// Writes the new copy into `new`, and reads it back.
fn write_new(
    new: &Path,
    repo: &Path,
    release: &Release,
    file: &dyn Fn(&Entry) -> PathBuf,
) -> Result<Written, String> {
    make_dir(new)?;
    for kept in KEPT {
        copy_tree(&repo.join(kept), &new.join(RELEASE).join(kept))?;
    }
    let built = tuf::time::parse(&release.built)
        .and_then(|seconds| u64::try_from(seconds).ok())
        .ok_or("the release has no build time")?;
    let mut written = Written { files: 0, bytes: 0 };
    make_dir(&new.join(STARTUP))?;
    for entry in release.files.iter().filter(|e| e.path.starts_with("startup/")) {
        let Some((sha256, size)) = entry.file() else {
            return Err(format!("/system/{}: the startup folder holds only files", entry.path));
        };
        let to = new.join(&entry.path);
        let mut out = create(&to)?;
        copy_checked(&file(entry), &mut out, sha256, size, &entry.path)?;
        out.sync_all().map_err(|err| format!("{}: {err}", to.display()))?;
        written.files += 1;
        written.bytes += size;
    }
    let archive = new.join(ARCHIVE);
    let mut out = create(&archive)?;
    let archived = write_archive(&mut out, release, built, file)?;
    out.sync_all().map_err(|err| format!("{}: {err}", archive.display()))?;
    written.files += archived.0;
    written.bytes += archived.1;
    drop(out);
    check(new, release)?;
    Ok(written)
}

/// Writes the boot archive of `release`: `system/`, then each entry but
/// the startup folder's beneath it, dated `built`.
fn write_archive(
    out: &mut std::fs::File,
    release: &Release,
    built: u64,
    file: &dyn Fn(&Entry) -> PathBuf,
) -> Result<(usize, u64), String> {
    let mut entries: Vec<&Entry> = release
        .files
        .iter()
        .filter(|e| e.path != STARTUP && !e.path.starts_with("startup/"))
        .collect();
    // A directory before what is in it, as the archive's reader wants.
    entries.sort_by(|a, b| a.path.split('/').cmp(b.path.split('/')));
    let mut headers = Headers::new();
    let named = |path: &str| format!("system/{path}");
    let put = |out: &mut std::fs::File, bytes: &[u8]| {
        out.write_all(bytes).map_err(|err| format!("the boot archive: {err}"))
    };
    let header = |result: Result<[u8; radix_bootfs::BLOCK], radix_bootfs::Error>, path: &str| {
        result.map_err(|err| format!("the boot archive: /system/{path}: {err:?}"))
    };
    put(out, &header(headers.directory(ARCHIVE, 0o755, built), "")?)?;
    let (mut files, mut bytes) = (0, 0);
    for entry in entries {
        let path = named(&entry.path);
        match entry.kind {
            Kind::Directory => {
                put(
                    out,
                    &header(headers.directory(&path, entry.mode_bits(), built), &entry.path)?,
                )?;
            }
            Kind::Link => {
                let target = entry.target.as_deref().unwrap_or_default();
                put(out, &header(headers.symlink(&path, target, built), &entry.path)?)?;
            }
            Kind::File => {
                let (sha256, size) =
                    entry.file().ok_or_else(|| format!("/system/{}: no hash", entry.path))?;
                put(
                    out,
                    &header(headers.file(&path, entry.mode_bits(), built, size), &entry.path)?,
                )?;
                copy_checked(&file(entry), out, sha256, size, &entry.path)?;
                let zeros = padding(size);
                put(out, &END[..zeros])?;
                files += 1;
                bytes += size;
            }
        }
    }
    put(out, &END)?;
    Ok((files, bytes))
}

/// Copies the file at `from` to `out`, checking as it goes that it is
/// the `size` bytes of SHA-256 `sha256` the release lists for `path`.
fn copy_checked(
    from: &Path,
    out: &mut std::fs::File,
    sha256: &str,
    size: u64,
    path: &str,
) -> Result<(), String> {
    let mut input =
        std::fs::File::open(from).map_err(|err| format!("{}: {err}", from.display()))?;
    let mut digest = ring::digest::Context::new(&ring::digest::SHA256);
    let mut buffer = vec![0u8; 1 << 20];
    let mut copied = 0u64;
    loop {
        let read = input.read(&mut buffer).map_err(|err| format!("{}: {err}", from.display()))?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
        out.write_all(&buffer[..read]).map_err(|err| format!("writing /system/{path}: {err}"))?;
        copied += read as u64;
        if copied > size {
            break;
        }
    }
    if copied != size || hex(digest.finish().as_ref()) != sha256 {
        return Err(format!(
            "/system/{path}, from {}, is not the file the release lists",
            from.display()
        ));
    }
    Ok(())
}

/// Reads the new copy at `dir` back: its archive holds every entry of
/// `release` but the startup folder's, each file the bytes listed, and
/// its startup folder the rest.
fn check(dir: &Path, release: &Release) -> Result<(), String> {
    let archive = dir.join(ARCHIVE);
    let bytes = std::fs::read(&archive).map_err(|err| format!("{}: {err}", archive.display()))?;
    let opened = radix_bootfs::open(&bytes)
        .map_err(|err| format!("{} does not read back: {err:?}", archive.display()))?;
    for entry in &release.files {
        if let Some(name) = entry.path.strip_prefix("startup/") {
            let (sha256, size) = entry.file().unwrap_or_default();
            let path = dir.join(STARTUP).join(name);
            let data = std::fs::read(&path).map_err(|err| format!("{}: {err}", path.display()))?;
            if data.len() as u64 != size || sha256_of(&data) != sha256 {
                return Err(format!("{} does not read back as written", path.display()));
            }
            continue;
        }
        if entry.path == STARTUP {
            continue;
        }
        let found = opened
            .find(&format!("system/{}", entry.path))
            .ok_or_else(|| format!("the boot archive read back lacks /system/{}", entry.path))?;
        let same = match entry.kind {
            Kind::Directory => found.kind == radix_bootfs::Kind::Directory,
            Kind::Link => {
                found.kind == radix_bootfs::Kind::Symlink
                    && found.data == entry.target.as_deref().unwrap_or_default().as_bytes()
            }
            Kind::File => {
                let (sha256, _) = entry.file().unwrap_or_default();
                found.kind == radix_bootfs::Kind::File && sha256_of(found.data) == sha256
            }
        };
        if !same {
            return Err(format!("the boot archive read back differs at /system/{}", entry.path));
        }
    }
    Ok(())
}

/// The SHA-256 of `data`, in lowercase hex.
fn sha256_of(data: &[u8]) -> String {
    hex(ring::digest::digest(&ring::digest::SHA256, data).as_ref())
}

/// Switches the whole new copy in: the copy there becomes `rescue.old`,
/// the new one `rescue`, and then the old one goes.
fn switch(esp: &Path) -> Result<(), String> {
    let (dir, new, old) = (esp.join(DIR), esp.join(NEW), esp.join(OLD));
    if dir.exists() {
        std::fs::rename(&dir, &old)
            .map_err(|err| format!("{} to {}: {err}", dir.display(), old.display()))?;
    }
    std::fs::rename(&new, &dir)
        .map_err(|err| format!("{} to {}: {err}", new.display(), dir.display()))?;
    remove(&old)
}

/// The release of the Rescue at `rescue`, read from the root this system
/// trusts, `root`, offline, as an install stick's is.
///
/// # Errors
///
/// If there is no Rescue there, or its metadata is not signed by the
/// trusted root.
pub fn open(rescue: &Path, root: &[u8]) -> Result<Published, String> {
    repo::open(&rescue.join(RELEASE), root, None)
}

/// Removes `dir` and all it holds, if it is there.
fn remove(dir: &Path) -> Result<(), String> {
    match std::fs::remove_dir_all(dir) {
        Err(err) if err.kind() != std::io::ErrorKind::NotFound => {
            Err(format!("removing {}: {err}", dir.display()))
        }
        _ => Ok(()),
    }
}

/// Makes `dir` and the folders above it.
fn make_dir(dir: &Path) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|err| format!("{}: {err}", dir.display()))
}

/// A new file at `path`, in place of one there.
fn create(path: &Path) -> Result<std::fs::File, String> {
    std::fs::File::create(path).map_err(|err| format!("{}: {err}", path.display()))
}

/// Copies the files of the tree at `from` into `to`, folders and all.
fn copy_tree(from: &Path, to: &Path) -> Result<(), String> {
    make_dir(to)?;
    let entries = std::fs::read_dir(from).map_err(|err| format!("{}: {err}", from.display()))?;
    for entry in entries {
        let entry = entry.map_err(|err| format!("{}: {err}", from.display()))?;
        let (source, target) = (entry.path(), to.join(entry.file_name()));
        if source.is_dir() {
            copy_tree(&source, &target)?;
        } else {
            std::fs::copy(&source, &target)
                .map_err(|err| format!("{} to {}: {err}", source.display(), target.display()))?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt as _;

    use super::*;
    use crate::list::About;
    use crate::list::tests::{scratch, tree};
    use crate::repo::Keys;
    use crate::store::Store;

    /// A slot with a startup folder, and its signed repository.
    fn published(dir: &Path) -> (Keys, Release) {
        let slot = dir.join("slot");
        tree(&slot);
        std::fs::create_dir(slot.join("startup")).unwrap();
        std::fs::set_permissions(slot.join("startup"), std::fs::Permissions::from_mode(0o755))
            .unwrap();
        for (name, bytes) in [("radix", &b"the kernel"[..]), ("archive", b"spark alone")] {
            std::fs::write(slot.join("startup").join(name), bytes).unwrap();
            std::fs::set_permissions(
                slot.join("startup").join(name),
                std::fs::Permissions::from_mode(0o644),
            )
            .unwrap();
        }
        let keys = Keys::create(&dir.join("keys"), 1_793_923_200).unwrap();
        let about = About {
            version: "0.15.0".into(),
            commit: "01d66b7".into(),
            toolchain: "nightly-2026-10-15".into(),
            built: 1_793_923_200,
            updates_from: "0.14.0".into(),
        };
        let release = Release::of_tree(&slot, about).unwrap();
        repo::publish(&keys, &slot, &release, &dir.join("repo")).unwrap();
        (keys, release)
    }

    #[test]
    fn rescue_holds_the_release_and_reads_back_from_the_trusted_root() {
        let dir = scratch("rescue");
        let (keys, release) = published(&dir);
        let esp = dir.join("esp");
        let store = Store::at(&dir.join("repo/files"));
        let from_store = |entry: &Entry| store.path(entry.file().unwrap().0);
        let written = write(&esp, &dir.join("repo"), &release, &from_store).unwrap();
        assert_eq!(written.files, 4);
        let rescue = esp.join(DIR);
        assert!(!esp.join(NEW).exists() && !esp.join(OLD).exists());
        assert!(!rescue.join(RELEASE).join("files").exists());
        assert_eq!(std::fs::read(rescue.join("startup/radix")).unwrap(), b"the kernel");
        let opened = open(&rescue, keys.root()).unwrap();
        assert_eq!(opened.release, release);

        let bytes = std::fs::read(rescue.join(ARCHIVE)).unwrap();
        let archive = radix_bootfs::open(&bytes).unwrap();
        let paths: Vec<String> = archive.entries().map(|e| e.path().to_string()).collect();
        assert_eq!(
            paths,
            ["system", "system/bin", "system/commands", "system/commands/spark", "system/empty"]
        );
        let spark = archive.find("system/commands/spark").unwrap();
        assert_eq!(
            (spark.data, spark.mode, spark.mtime),
            (&b"spark's bytes"[..], 0o755, 1_793_923_200)
        );
        assert_eq!(archive.find("system/bin").unwrap().data, b"commands");

        // Rescue's own files are where a reinstall finds them.
        let live = dir.join("slot");
        for entry in release.files.iter().filter(|e| e.file().is_some()) {
            let found = std::fs::read(source(&rescue, &live, entry)).unwrap();
            assert_eq!(sha256_of(&found), entry.file().unwrap().0, "{}", entry.path);
        }

        // A switch cut between its renames is finished by the next write.
        std::fs::rename(&rescue, esp.join(OLD)).unwrap();
        write(&esp, &dir.join("repo"), &release, &from_store).unwrap();
        assert!(rescue.join(ARCHIVE).is_file() && !esp.join(OLD).exists());

        // A second copy replaces the first; one with a damaged file
        // leaves the first in place.
        write(&esp, &dir.join("repo"), &release, &from_store).unwrap();
        std::fs::write(dir.join("damaged"), b"spark's bytez").unwrap();
        let damaged = |entry: &Entry| {
            if entry.path == "commands/spark" { dir.join("damaged") } else { from_store(entry) }
        };
        let why = write(&esp, &dir.join("repo"), &release, &damaged).unwrap_err();
        assert!(why.contains("/system/commands/spark"), "{why}");
        assert!(!esp.join(NEW).exists());
        assert_eq!(open(&rescue, keys.root()).unwrap().release, release);
        assert_eq!(std::fs::read(rescue.join(ARCHIVE)).unwrap(), bytes);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
