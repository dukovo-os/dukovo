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
//!   but not its store: the files are those of `system` and `startup/`;
//!   and `version`, which names the release among those the metadata
//!   lists, as the update server's lists many.
//!
//! A new copy is written beside the old in `boot/rescue.new`, read back,
//! and then switched in by two renames, so that a power cut leaves one
//! whole copy: `rescue` or, between the renames, `rescue.old`, which the
//! boot menu starts in its place.
//!
//! The installer and `ob image` write it from a release's repository; the
//! running system writes it again from each release that proves itself
//! (ADR 0543), from the metadata its TUF client kept
//! ([`repository_from_client`]) and the files of the running slot.

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

/// The file in Rescue's `release/` that names the release's version.
pub const VERSION: &str = "version";

/// The folders of a repository Rescue keeps: not `files`, its store.
const KEPT: [&str; 2] = ["metadata", "targets"];

/// What a FAT cluster can take beyond a file's bytes: up to 64 KiB.
const CLUSTER: u64 = 64 * 1024;

/// Room kept for the signed metadata and the folders: a server's
/// metadata lists every release it offers.
const METADATA_ROOM: u64 = 8 << 20;

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

/// Writes Rescue of `release` in place of the copy there, for a
/// partition without room for both: the copy there goes first, so that
/// until the new one is written and read back the partition holds no
/// Rescue. The caller reads `file`'s files back against the signed list
/// first, so that only a write to the partition can fail on the way.
///
/// # Errors
///
/// As [`write`]; the partition then holds no Rescue.
pub fn replace(
    esp: &Path,
    repo: &Path,
    release: &Release,
    file: &dyn Fn(&Entry) -> PathBuf,
) -> Result<Written, String> {
    for dir in [NEW, OLD, DIR] {
        remove(&esp.join(dir))?;
    }
    write(esp, repo, release, file)
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
    let named = new.join(RELEASE).join(VERSION);
    std::fs::write(&named, format!("{}\n", release.version))
        .map_err(|err| format!("{}: {err}", named.display()))?;
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
/// `release` but the startup folder's, and nothing else, each file the
/// bytes listed, and its startup folder the rest. The archive is read a
/// block at a time, never whole, as the stage that writes Rescue again on
/// a running system has little memory (ADR 0543).
fn check(dir: &Path, release: &Release) -> Result<(), String> {
    for entry in release.files.iter().filter(|e| e.path.starts_with("startup/")) {
        let name = &entry.path["startup/".len()..];
        let (sha256, size) = entry.file().unwrap_or_default();
        let path = dir.join(STARTUP).join(name);
        let data = std::fs::read(&path).map_err(|err| format!("{}: {err}", path.display()))?;
        if data.len() as u64 != size || sha256_of(&data) != sha256 {
            return Err(format!("{} does not read back as written", path.display()));
        }
    }
    let mut wanted: std::collections::BTreeMap<String, &Entry> = release
        .files
        .iter()
        .filter(|e| e.path != STARTUP && !e.path.starts_with("startup/"))
        .map(|e| (format!("{ARCHIVE}/{}", e.path), e))
        .collect();
    let archive = dir.join(ARCHIVE);
    let unread = |why: &str| format!("{} does not read back: {why}", archive.display());
    let file =
        std::fs::File::open(&archive).map_err(|err| format!("{}: {err}", archive.display()))?;
    let mut input = std::io::BufReader::with_capacity(1 << 20, file);
    let mut block = [0u8; radix_bootfs::BLOCK];
    let mut buffer = vec![0u8; 1 << 20];
    loop {
        input.read_exact(&mut block).map_err(|err| unread(&err.to_string()))?;
        if block.iter().all(|&b| b == 0) {
            break;
        }
        let header = Header::read(&block).ok_or_else(|| unread("a header does not check"))?;
        let mut digest = ring::digest::Context::new(&ring::digest::SHA256);
        let mut left = header.size;
        while left > 0 {
            let take = usize::try_from(left.min(buffer.len() as u64)).unwrap_or(buffer.len());
            input.read_exact(&mut buffer[..take]).map_err(|err| unread(&err.to_string()))?;
            digest.update(&buffer[..take]);
            left -= take as u64;
        }
        input
            .read_exact(&mut buffer[..padding(header.size)])
            .map_err(|err| unread(&err.to_string()))?;
        if header.path == ARCHIVE && header.kind == b'5' {
            continue;
        }
        let shown = header.path.strip_prefix("system/").unwrap_or(&header.path).to_string();
        let entry = wanted.remove(&header.path).ok_or_else(|| {
            format!(
                "the boot archive read back holds /system/{shown}, which the release does not list"
            )
        })?;
        let same = match entry.kind {
            Kind::Directory => header.kind == b'5',
            Kind::Link => {
                header.kind == b'2' && header.link == entry.target.as_deref().unwrap_or_default()
            }
            Kind::File => {
                let (sha256, _) = entry.file().unwrap_or_default();
                header.kind == b'0' && hex(digest.finish().as_ref()) == sha256
            }
        };
        if !same {
            return Err(format!("the boot archive read back differs at /system/{shown}"));
        }
    }
    match wanted.values().next() {
        Some(entry) => Err(format!("the boot archive read back lacks /system/{}", entry.path)),
        None => Ok(()),
    }
}

/// What [`check`] reads of a header of the boot archive, which
/// [`write_archive`] wrote as POSIX ustar.
struct Header {
    /// Its path, the prefix and the name joined.
    path: String,
    /// `0` a file, `5` a folder, `2` a link.
    kind: u8,
    /// The bytes of a file.
    size: u64,
    /// A link's target.
    link: String,
}

impl Header {
    /// The header in `block`, if its checksum and its fields read.
    fn read(block: &[u8; radix_bootfs::BLOCK]) -> Option<Header> {
        let field = |offset: usize, len: usize| {
            let bytes = &block[offset..offset + len];
            let end = bytes.iter().position(|&b| b == 0).unwrap_or(len);
            std::str::from_utf8(&bytes[..end]).ok()
        };
        let octal = |offset: usize, len: usize| {
            let text = field(offset, len)?.trim_matches(|c| c == ' ' || c == '\0');
            u64::from_str_radix(text, 8).ok()
        };
        let mut summed = *block;
        summed[148..156].fill(b' ');
        let sum: u64 = summed.iter().map(|&b| u64::from(b)).sum();
        if &block[257..263] != b"ustar\0" || octal(148, 8)? != sum {
            return None;
        }
        let (name, prefix) = (field(0, 100)?, field(345, 155)?);
        let path = if prefix.is_empty() { name.to_string() } else { format!("{prefix}/{name}") };
        Some(Header {
            path,
            kind: block[156],
            size: octal(124, 12)?,
            link: field(157, 100)?.into(),
        })
    }
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
    let dir = rescue.join(RELEASE);
    // A Rescue written before it named its version holds one release.
    let named = dir.join(VERSION);
    let text = match std::fs::read_to_string(&named) {
        Ok(text) => Some(text),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => None,
        Err(err) => return Err(format!("{}: {err}", named.display())),
    };
    let version = match text.as_deref().map(str::trim) {
        Some(text) => {
            Some(plain_version(text).ok_or_else(|| format!("{}: not a version", named.display()))?)
        }
        None => None,
    };
    repo::open(&dir, root, version)
}

/// `text`, if it can only be a version: letters, digits, `.`, `-` and
/// `+`, and no more than 64 of them.
fn plain_version(text: &str) -> Option<&str> {
    let plain = !text.is_empty()
        && text.len() <= 64
        && !text.starts_with('.')
        && text.bytes().all(|b| b.is_ascii_alphanumeric() || b".-+".contains(&b));
    plain.then_some(text)
}

/// The bytes Rescue of `release` takes on a FAT partition, with room to
/// spare: each startup file in whole clusters, the boot archive's
/// headers and files in its blocks, and the signed metadata.
#[must_use]
pub fn needed(release: &Release) -> u64 {
    let block = radix_bootfs::BLOCK as u64;
    let mut bytes = METADATA_ROOM + 2 * block + CLUSTER;
    for entry in &release.files {
        let size = entry.file().map_or(0, |(_, size)| size);
        bytes += if entry.path.starts_with("startup/") {
            size.div_ceil(CLUSTER) * CLUSTER
        } else {
            block + size.div_ceil(block) * block
        };
    }
    bytes
}

/// Lays out in `out`, made afresh, the repository of the release whose
/// signed list is `list`, from what a TUF client kept as it read the
/// update server: in `kept`, each role's newest metadata, written under
/// its own name and, as a server with consistent snapshots names it, its
/// version's; in `roots`, the roots it walked to, by version
/// ([`tuf::Updater::keeping_roots`]), if it walked to any; and the list,
/// under its name and its SHA-256's. The client checked each before it
/// kept it, and [`repo::open`] checks them all again from the root the
/// system trusts, as Rescue does. Returns the release's version.
///
/// # Errors
///
/// If the list is not a release's, or a file cannot be read or written.
pub fn repository_from_client(
    kept: &Path,
    roots: &Path,
    list: &[u8],
    out: &Path,
) -> Result<String, String> {
    let release = Release::parse(list)?;
    remove(out)?;
    let metadata = out.join("metadata");
    make_dir(&metadata)?;
    for (dir, wanted) in [(kept, true), (roots, false)] {
        let entries = match std::fs::read_dir(dir) {
            Ok(entries) => entries,
            Err(err) if !wanted && err.kind() == std::io::ErrorKind::NotFound => continue,
            Err(err) => return Err(format!("{}: {err}", dir.display())),
        };
        for entry in entries {
            let entry = entry.map_err(|err| format!("{}: {err}", dir.display()))?;
            let Some(name) = entry.file_name().to_str().map(str::to_string) else { continue };
            let is_file = entry.file_type().is_ok_and(|kind| kind.is_file());
            let json = Path::new(&name).extension().is_some_and(|e| e == "json");
            if !is_file || !json || name.starts_with('.') {
                continue;
            }
            let path = entry.path();
            let data = std::fs::read(&path).map_err(|err| format!("{}: {err}", path.display()))?;
            put(&metadata.join(&name), &data)?;
            if !by_version(&name) {
                let version = signed_version(&data)
                    .ok_or_else(|| format!("{}: no version", path.display()))?;
                put(&metadata.join(format!("{version}.{name}")), &data)?;
            }
        }
    }
    let lists = out.join("targets").join("system");
    make_dir(&lists)?;
    put(&lists.join(format!("{}.{}.json", sha256_of(list), release.version)), list)?;
    put(&lists.join(format!("{}.json", release.version)), list)?;
    Ok(release.version)
}

/// Whether the metadata file `name` is named by its version already, as
/// `2.root.json` is.
fn by_version(name: &str) -> bool {
    name.split_once('.')
        .is_some_and(|(number, _)| !number.is_empty() && number.bytes().all(|b| b.is_ascii_digit()))
}

/// The version signed metadata `data` says it is.
fn signed_version(data: &[u8]) -> Option<u64> {
    let value: serde_json::Value = serde_json::from_slice(data).ok()?;
    value["signed"]["version"].as_u64()
}

/// Writes `data` as the new file `path`.
fn put(path: &Path, data: &[u8]) -> Result<(), String> {
    std::fs::write(path, data).map_err(|err| format!("{}: {err}", path.display()))
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

    #[test]
    fn rescue_of_a_server_release_is_written_from_what_its_client_kept() {
        let dir = scratch("rescue-server");
        let (keys, older) = published(&dir);
        let first = keys.root().to_vec();
        // A newer release beside it, on a server whose keys have been
        // replaced since the first root: timestamp's, then root's.
        let slot = dir.join("slot");
        std::fs::write(slot.join("commands/spark"), b"spark, newer").unwrap();
        let about = About {
            version: "0.15.1".into(),
            commit: "01d66b7".into(),
            toolchain: "nightly-2026-10-15".into(),
            built: 1_793_923_200,
            updates_from: older.version.clone(),
        };
        let release = Release::of_tree(&slot, about).unwrap();
        repo::publish(&keys, &slot, &release, &dir.join("repo2")).unwrap();
        let server = dir.join("server");
        let now = tuf::time::now();
        crate::updates::add(&server, &keys, &dir.join("repo"), now).unwrap();
        crate::updates::add(&server, &keys, &dir.join("repo2"), now).unwrap();
        drop(keys);
        drop(Keys::rotate(&dir.join("keys"), "timestamp", now).unwrap());
        let keys = Keys::rotate(&dir.join("keys"), "root", now).unwrap();
        crate::updates::refresh(&server, &keys, None, now).unwrap();

        // The client of a system that trusts the first root reads it.
        let (client, roots) = (dir.join("client"), dir.join("roots"));
        tuf::init(&client, &first).unwrap();
        let url = |sub: &str| tuf::Url::parse(&format!("file://{}/{sub}/", server.display()));
        let mut updater =
            tuf::Updater::new(&client, &url("metadata").unwrap(), Some(&url("targets").unwrap()))
                .unwrap()
                .keeping_roots(&roots);
        updater.refresh().unwrap();
        let path = repo::target_path("0.15.1");
        let target = updater.target(&path).unwrap().unwrap();
        updater.download_target(&path, &target, &dir.join("list.json")).unwrap();
        let list = std::fs::read(dir.join("list.json")).unwrap();
        assert!(roots.join("2.root.json").is_file() && roots.join("3.root.json").is_file());
        assert!(!client.join("2.root.json").exists());

        let laid = dir.join("laid");
        assert_eq!(repository_from_client(&client, &roots, &list, &laid).unwrap(), "0.15.1");
        // The server lists both releases: Rescue names its own.
        assert!(repo::open(&laid, &first, None).unwrap_err().contains("2 releases"));
        assert_eq!(repo::open(&laid, &first, Some("0.15.1")).unwrap().release, release);
        // Written as a running system writes it, from its own slot.
        let esp = dir.join("esp");
        let from_slot = |entry: &Entry| slot.join(&entry.path);
        let written = write(&esp, &laid, &release, &from_slot).unwrap();
        assert!(needed(&release) >= written.bytes);
        let rescue = esp.join(DIR);
        assert_eq!(open(&rescue, &first).unwrap().release, release);

        // Without the roots walked to, the first root cannot reach it.
        let bare = dir.join("bare");
        repository_from_client(&client, &dir.join("no-roots"), &list, &bare).unwrap();
        assert!(repo::open(&bare, &first, Some("0.15.1")).is_err());
        // A version that is not one is refused, not used.
        std::fs::write(rescue.join(RELEASE).join(VERSION), "../0.15.1\n").unwrap();
        assert!(open(&rescue, &first).unwrap_err().contains("not a version"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_copy_written_in_place_of_the_one_there_leaves_no_other() {
        let dir = scratch("rescue-replace");
        let (keys, release) = published(&dir);
        let esp = dir.join("esp");
        let store = Store::at(&dir.join("repo/files"));
        let from_store = |entry: &Entry| store.path(entry.file().unwrap().0);
        write(&esp, &dir.join("repo"), &release, &from_store).unwrap();
        std::fs::create_dir_all(esp.join(NEW)).unwrap();
        std::fs::write(esp.join(NEW).join("left"), b"half").unwrap();
        let written = replace(&esp, &dir.join("repo"), &release, &from_store).unwrap();
        assert_eq!(written.files, 4);
        assert!(!esp.join(NEW).exists() && !esp.join(OLD).exists());
        assert_eq!(open(&esp.join(DIR), keys.root()).unwrap().release, release);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_boot_archive_changed_cut_short_or_of_other_files_does_not_read_back() {
        let dir = scratch("rescue-check");
        let (_, release) = published(&dir);
        let esp = dir.join("esp");
        let store = Store::at(&dir.join("repo/files"));
        let from_store = |entry: &Entry| store.path(entry.file().unwrap().0);
        write(&esp, &dir.join("repo"), &release, &from_store).unwrap();
        let rescue = esp.join(DIR);
        check(&rescue, &release).unwrap();
        let archive = rescue.join(ARCHIVE);
        let bytes = std::fs::read(&archive).unwrap();
        let fails = |bytes: &[u8], release: &Release| {
            std::fs::write(&archive, bytes).unwrap();
            check(&rescue, release).unwrap_err()
        };

        let at = bytes.windows(13).position(|w| w == b"spark's bytes").unwrap();
        let mut changed = bytes.clone();
        changed[at] = b'S';
        assert!(fails(&changed, &release).contains("differs at /system/commands/spark"));
        assert!(fails(&bytes[..bytes.len() - 1024], &release).contains("does not read back"));
        let mut header = bytes.clone();
        header[0] ^= 1;
        assert!(fails(&header, &release).contains("a header does not check"));

        let mut fewer = release.clone();
        fewer.files.retain(|e| e.path != "empty");
        assert!(fails(&bytes, &fewer).contains("/system/empty, which the release does not list"));
        let mut more = release.clone();
        let mut extra = more.files.iter().find(|e| e.path == "empty").unwrap().clone();
        extra.path = "zzz".into();
        more.files.push(extra);
        assert!(fails(&bytes, &more).contains("lacks /system/zzz"));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
