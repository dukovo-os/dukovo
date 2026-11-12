// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The update repository (ADR 0466): every release a server offers, with
//! the channels that name one each, laid out as an install stick's
//! release is (ADR 0308) and read by the same TUF client:
//!
//! ```text
//! metadata/N.root.json, root.json        every root, by version, and the newest
//! metadata/N.targets.json                every release offered, and the channels
//! metadata/N.snapshot.json, timestamp.json
//! targets/system/<sha256>.<version>.json each release's list
//! targets/channels/<sha256>.<name>.json  a channel: the release it offers
//! files/<sha256>                         every file of every release, once
//! ```
//!
//! [`add`] adds a release that `make` signed, [`set_channel`] points a
//! channel at one, and [`refresh`] signs a new snapshot and timestamp, as
//! a server does every few hours. Each signs the next version of what it
//! changes and writes the timestamp last, so that a client reading the
//! repository while it changes sees the old one or the new one.
//!
//! [`pack`] writes one release as an update file ([`crate::archive`]),
//! and [`open_file`] reads one back as a computer does, from the root it
//! trusts.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tuf::repo as signing;

use crate::archive::{Archive, Writer};
use crate::list::{Release, hex};
use crate::repo::{self, Keys, Published, expires};
use crate::store::Checked;

/// The channels a machine follows one of (ADR 0456): stable, the default;
/// beta, two weeks before each stable; and nightly, every merge.
pub const CHANNELS: [&str; 3] = ["stable", "beta", "nightly"];

/// The folders an update file holds.
const FOLDERS: [&str; 3] = ["metadata", "targets", "files"];

/// Where the channel `name` is among the targets.
#[must_use]
pub fn channel_path(name: &str) -> String {
    format!("channels/{name}.json")
}

/// A channel's file: the release it offers now.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Channel {
    /// Its name, which must be the one asked for, so that a file signed
    /// for one channel is not taken for another's.
    pub channel: String,
    /// The version of the release it offers.
    pub version: String,
}

impl Channel {
    /// Reads a channel's file, which must be the channel `name`'s.
    ///
    /// # Errors
    ///
    /// If it is not a channel's file, or is another channel's.
    pub fn parse(bytes: &[u8], name: &str) -> Result<Channel, String> {
        let channel: Channel = serde_json::from_slice(bytes)
            .map_err(|err| format!("the {name} channel's file is malformed: {err}"))?;
        if channel.channel != name {
            return Err(format!("the {name} channel's file is the {} channel's", channel.channel));
        }
        Ok(channel)
    }

    /// The file as it is signed.
    ///
    /// # Panics
    ///
    /// Never: its fields are all JSON's.
    #[must_use]
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut bytes = serde_json::to_vec_pretty(self).expect("a channel is JSON");
        bytes.push(b'\n');
        bytes
    }
}

/// What the repository's metadata says now.
struct Current {
    /// Every target, by path, as the targets list it.
    targets: BTreeMap<String, Value>,
    targets_version: u64,
    snapshot_version: u64,
    timestamp_version: u64,
}

fn read(path: &Path) -> Result<Vec<u8>, String> {
    std::fs::read(path).map_err(|err| format!("{}: {err}", path.display()))
}

fn write(path: &Path, bytes: &[u8]) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|err| format!("{}: {err}", parent.display()))?;
    }
    // Whole or not at all, for a server that serves it as it changes.
    let partial = path.with_extension("partial");
    std::fs::write(&partial, bytes)
        .and_then(|()| std::fs::rename(&partial, path))
        .map_err(|err| format!("{}: {err}", path.display()))
}

/// The version of the metadata file `name` that `signed` lists in its
/// `meta`.
fn listed_version(signed: &Value, name: &str) -> Option<u64> {
    signed["signed"]["meta"][name]["version"].as_u64()
}

/// Reads what the repository at `dir` says now, if it has been signed.
fn current(dir: &Path) -> Result<Option<Current>, String> {
    let metadata = dir.join("metadata");
    if !metadata.join("timestamp.json").exists() {
        return Ok(None);
    }
    let json = |name: String| -> Result<Value, String> {
        let path = metadata.join(&name);
        serde_json::from_slice(&read(&path)?).map_err(|err| format!("{}: {err}", path.display()))
    };
    let malformed = |name: &str| format!("{}: {name} is malformed", metadata.display());
    let timestamp = json("timestamp.json".into())?;
    let timestamp_version =
        timestamp["signed"]["version"].as_u64().ok_or_else(|| malformed("timestamp.json"))?;
    let snapshot_version =
        listed_version(&timestamp, "snapshot.json").ok_or_else(|| malformed("timestamp.json"))?;
    let snapshot = json(format!("{snapshot_version}.snapshot.json"))?;
    let targets_version =
        listed_version(&snapshot, "targets.json").ok_or_else(|| malformed("the snapshot"))?;
    let targets = json(format!("{targets_version}.targets.json"))?;
    let targets = targets["signed"]["targets"]
        .as_object()
        .ok_or_else(|| malformed("the targets"))?
        .iter()
        .map(|(path, info)| (path.clone(), info.clone()))
        .collect();
    Ok(Some(Current { targets, targets_version, snapshot_version, timestamp_version }))
}

/// Signs the repository at `dir` anew at `now`: the targets `targets` as
/// the next version, if given, then the next snapshot and timestamp, and
/// every root of `keys`. Returns the timestamp's version.
fn sign(
    dir: &Path,
    keys: &Keys,
    current: Option<&Current>,
    targets: Option<&BTreeMap<String, Value>>,
    now: i64,
) -> Result<u64, String> {
    let metadata = dir.join("metadata");
    let (targets_version, snapshot_version, timestamp_version) =
        current.map_or((0, 0, 0), |c| (c.targets_version, c.snapshot_version, c.timestamp_version));
    let (targets_version, targets_bytes) = match targets {
        Some(targets) => {
            let version = targets_version + 1;
            let bytes = repo::sign_role(
                &signing::targets(version, &expires(now, "targets"), targets),
                keys,
                "targets",
            )?;
            write(&metadata.join(format!("{version}.targets.json")), &bytes)?;
            (version, bytes)
        }
        None => (targets_version, read(&metadata.join(format!("{targets_version}.targets.json")))?),
    };
    let meta = BTreeMap::from([(
        "targets.json".to_string(),
        signing::meta(targets_version, &targets_bytes),
    )]);
    let snapshot_version = snapshot_version + 1;
    let snapshot = repo::sign_role(
        &signing::snapshot(snapshot_version, &expires(now, "snapshot"), &meta),
        keys,
        "snapshot",
    )?;
    write(&metadata.join(format!("{snapshot_version}.snapshot.json")), &snapshot)?;
    repo::write_roots(&metadata, keys)?;
    let timestamp_version = timestamp_version + 1;
    let timestamp = repo::sign_role(
        &signing::timestamp(
            timestamp_version,
            &expires(now, "timestamp"),
            snapshot_version,
            &snapshot,
        ),
        keys,
        "timestamp",
    )?;
    write(&metadata.join("timestamp.json"), &timestamp)?;
    Ok(timestamp_version)
}

/// What [`add`] did.
#[derive(Debug)]
pub struct Added {
    /// The release, as read back from the repository.
    pub published: Published,
    /// How many of its files the store did not hold already.
    pub new_files: usize,
    /// Whether the repository offered it already, with the same list.
    pub already: bool,
}

/// Adds the release in `release_dir`, a repository that `make` signed
/// with `keys`, to the update repository at `dir`, made if it is not
/// there: its list among the targets, and its files in the store, each
/// once. Then signs the targets, snapshot and timestamp anew, and reads
/// the release back from the first root.
///
/// # Errors
///
/// If the release does not read back from the first root of `keys`, a
/// file of it is missing or damaged, the repository offers the version
/// already with another list (a version is never used twice), or a
/// write fails.
pub fn add(dir: &Path, keys: &Keys, release_dir: &Path, now: i64) -> Result<Added, String> {
    let first = &keys.roots()[0];
    let published = repo::open(release_dir, first, None)?;
    let release = &published.release;
    repo::check_files(release_dir, release).map_err(|problems| problems.join("\n"))?;
    let current = current(dir)?;
    let mut targets = current.as_ref().map_or_else(BTreeMap::new, |c| c.targets.clone());
    let path = repo::target_path(&release.version);
    let list_name = format!("system/{}.{}.json", published.name, release.version);
    let list = read(&release_dir.join("targets").join(&list_name))?;
    let info = signing::file_info(&list);
    if let Some(offered) = targets.get(&path) {
        if *offered != info {
            return Err(format!(
                "{} offers Dukovo {} already, with another list: a version is never used twice",
                dir.display(),
                release.version
            ));
        }
        let published = repo::open(dir, first, Some(&release.version))?;
        return Ok(Added { published, new_files: 0, already: true });
    }
    let mut new_files = 0;
    let files = dir.join("files");
    std::fs::create_dir_all(&files).map_err(|err| format!("{}: {err}", files.display()))?;
    let mut seen = std::collections::HashSet::new();
    for (sha256, _) in release.files.iter().filter_map(crate::list::Entry::file) {
        let to = files.join(sha256);
        if !seen.insert(sha256) || to.exists() {
            continue;
        }
        // Each file whole or not at all: a server may be serving the store.
        let partial = files.join(format!(".{sha256}.partial"));
        let from = release_dir.join("files").join(sha256);
        std::fs::copy(&from, &partial)
            .and_then(|_| std::fs::rename(&partial, &to))
            .map_err(|err| format!("{}: {err}", to.display()))?;
        new_files += 1;
    }
    write(&dir.join("targets").join(&list_name), &list)?;
    targets.insert(path, info);
    sign(dir, keys, current.as_ref(), Some(&targets), now)?;
    let published = repo::open(dir, first, Some(&release.version))?;
    Ok(Added { published, new_files, already: false })
}

/// Points the channel `channel` of the repository at `dir` at its release
/// `version`, and signs the targets, snapshot and timestamp anew.
///
/// # Errors
///
/// If `channel` is not one of [`CHANNELS`], the repository does not offer
/// `version`, or a write fails.
pub fn set_channel(
    dir: &Path,
    keys: &Keys,
    channel: &str,
    version: &str,
    now: i64,
) -> Result<(), String> {
    if !CHANNELS.contains(&channel) {
        return Err(format!("no channel called {channel}; they are {}", CHANNELS.join(", ")));
    }
    let current = current(dir)?.ok_or_else(|| format!("{} offers no release", dir.display()))?;
    if !current.targets.contains_key(&repo::target_path(version)) {
        return Err(format!("{} does not offer Dukovo {version}", dir.display()));
    }
    let bytes = Channel { channel: channel.into(), version: version.into() }.to_bytes();
    let name = hex(ring::digest::digest(&ring::digest::SHA256, &bytes).as_ref());
    write(&dir.join(format!("targets/channels/{name}.{channel}.json")), &bytes)?;
    let mut targets = current.targets.clone();
    targets.insert(channel_path(channel), signing::file_info(&bytes));
    sign(dir, keys, Some(&current), Some(&targets), now)?;
    Ok(())
}

/// Signs a new snapshot and timestamp for the repository at `dir`, as of
/// `now`, the targets as they are. Returns the timestamp's version.
///
/// # Errors
///
/// If the repository has not been signed, or a write fails.
pub fn refresh(dir: &Path, keys: &Keys, now: i64) -> Result<u64, String> {
    let current = current(dir)?.ok_or_else(|| format!("{} offers no release", dir.display()))?;
    sign(dir, keys, Some(&current), None, now)
}

/// What [`pack`] wrote.
#[derive(Debug)]
pub struct Packed {
    /// The release.
    pub published: Published,
    /// How many of its files the update file holds, each once.
    pub files: usize,
    /// The update file's length in bytes.
    pub bytes: u64,
}

/// Writes the release `version` of the repository at `dir` as an update
/// file at `out`: the repository's metadata, every root included, the
/// release's list and every file it lists, once each, in an order that
/// is always the same. The release is read from `root` first.
///
/// # Errors
///
/// If the release does not read back from `root`, a file is missing, or
/// a write fails.
pub fn pack(dir: &Path, root: &[u8], version: &str, out: &Path) -> Result<Packed, String> {
    let published = repo::open(dir, root, Some(version))?;
    let release = &published.release;
    let current = current(dir)?.ok_or_else(|| format!("{} offers no release", dir.display()))?;
    let mut roots: Vec<(u64, String)> = std::fs::read_dir(dir.join("metadata"))
        .map_err(|err| format!("{}: {err}", dir.join("metadata").display()))?
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            let version = name.strip_suffix(".root.json")?.parse().ok()?;
            Some((version, name))
        })
        .collect();
    roots.sort();
    let mut metadata: Vec<String> = roots.into_iter().map(|(_, name)| name).collect();
    metadata.extend([
        "root.json".to_string(),
        "timestamp.json".to_string(),
        format!("{}.snapshot.json", current.snapshot_version),
        format!("{}.targets.json", current.targets_version),
    ]);
    let list = format!("system/{}.{}.json", published.name, release.version);
    let mut files: Vec<(&str, u64)> =
        release.files.iter().filter_map(crate::list::Entry::file).collect();
    files.sort_unstable();
    files.dedup();

    let partial = out.with_extension("partial");
    let mtime = tuf::time::parse(&release.built).map_or(0, |s| u64::try_from(s).unwrap_or(0));
    let file =
        std::fs::File::create(&partial).map_err(|err| format!("{}: {err}", partial.display()))?;
    let mut writer = Writer::new(std::io::BufWriter::new(file), mtime);
    type Out = Writer<std::io::BufWriter<std::fs::File>>;
    let add = |writer: &mut Out, name: String, path: PathBuf| -> Result<(), String> {
        let mut source =
            std::fs::File::open(&path).map_err(|err| format!("{}: {err}", path.display()))?;
        let size = source.metadata().map_err(|err| format!("{}: {err}", path.display()))?.len();
        writer.file(&name, size, &mut source).map_err(|err| format!("{}: {err}", out.display()))
    };
    for folder in ["metadata", "targets", "targets/system", "files"] {
        writer.folder(folder)?;
    }
    for name in &metadata {
        add(&mut writer, format!("metadata/{name}"), dir.join("metadata").join(name))?;
    }
    add(&mut writer, format!("targets/{list}"), dir.join("targets").join(&list))?;
    for (sha256, _) in &files {
        add(&mut writer, format!("files/{sha256}"), dir.join("files").join(sha256))?;
    }
    writer.finish()?;
    std::fs::rename(&partial, out).map_err(|err| format!("{}: {err}", out.display()))?;
    let bytes = std::fs::metadata(out).map_err(|err| format!("{}: {err}", out.display()))?.len();
    // Read back as a computer would.
    let (again, archive) = open_file(out, root, Some(version))?;
    if again.release != published.release {
        return Err(format!("{}: the release read back is not the one written", out.display()));
    }
    check_archive(&archive, &published.release).map_err(|problems| problems.join("\n"))?;
    let files = files.len();
    Ok(Packed { published, files, bytes })
}

/// Reads the update file at `file` with the TUF client from `root`, the
/// root this computer trusts, offline, as an install stick's release is
/// read: the release of `version`, or the only one there. Gives the
/// release and the archive to copy its files from.
///
/// # Errors
///
/// If it is not an update file, or its release does not read from
/// `root`, in words for the person installing.
pub fn open_file(
    file: &Path,
    root: &[u8],
    version: Option<&str>,
) -> Result<(Published, Archive), String> {
    let archive = Archive::open(file)?;
    if let Some(name) =
        archive.names().find(|name| !FOLDERS.iter().any(|f| name.starts_with(&format!("{f}/"))))
    {
        return Err(format!("{}: {name} is not part of an update file", file.display()));
    }
    // The repository's metadata names every release it offers; the file
    // holds the list of one, which is the one to open unless another is
    // asked for.
    let held = archive
        .names()
        .filter_map(|name| name.strip_prefix("targets/system/")?.strip_suffix(".json"))
        .filter_map(|name| Some(name.split_once('.')?.1.to_string()))
        .collect::<Vec<_>>();
    let version = match (version, held.as_slice()) {
        (Some(version), _) => Some(version.to_string()),
        (None, [one]) => Some(one.clone()),
        (None, _) => None,
    };
    let work = repo::scratch_dir();
    let opened = (|| {
        archive.unpack("metadata", &work)?;
        archive.unpack("targets", &work)?;
        repo::open(&work, root, version.as_deref())
    })();
    let _ = std::fs::remove_dir_all(&work);
    let published = opened.map_err(|err| {
        format!("{}: {}", file.display(), err.replace(&format!("{}: ", work.display()), ""))
    })?;
    Ok((published, archive))
}

/// Checks every file of `release` in the update file `archive`: there,
/// of its size and SHA-256.
///
/// # Errors
///
/// Each file that is missing or damaged, by its path in the release.
pub fn check_archive(archive: &Archive, release: &Release) -> Result<Checked, Vec<String>> {
    let mut problems = Vec::new();
    let mut checked = std::collections::HashMap::new();
    let (mut files, mut bytes) = (0, 0);
    for entry in &release.files {
        let Some((sha256, size)) = entry.file() else { continue };
        files += 1;
        bytes += size;
        let name = format!("files/{sha256}");
        let result = checked.entry(sha256).or_insert_with(|| match archive.size(&name) {
            None => Err("missing".to_string()),
            Some(found) if found != size => Err(format!("{found} bytes, not {size}")),
            Some(_) => match archive.sha256(&name) {
                Ok(found) if found == sha256 => Ok(()),
                Ok(_) => Err("damaged: its SHA-256 differs".to_string()),
                Err(err) => Err(err),
            },
        });
        if let Err(why) = result {
            problems.push(format!("/system/{}: {why}", entry.path));
        }
    }
    if problems.is_empty() { Ok(Checked { files, bytes }) } else { Err(problems) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::list::About;
    use crate::list::tests::{scratch, tree};

    const BUILT: i64 = 1_793_923_200;

    fn about(version: &str, updates_from: &str) -> About {
        About {
            version: version.into(),
            commit: "01d66b7".into(),
            toolchain: "nightly-2026-10-15".into(),
            built: BUILT,
            updates_from: updates_from.into(),
        }
    }

    /// A release of the tree at `slot`, signed by `keys`, in `out`.
    fn made(keys: &Keys, slot: &Path, version: &str, out: &Path) {
        let release = Release::of_tree(slot, about(version, "0.14.0")).unwrap();
        repo::publish(keys, slot, &release, out).unwrap();
    }

    #[test]
    fn releases_and_channels_are_offered_and_read_back() {
        let dir = scratch("updates");
        let keys = Keys::create(&dir.join("keys"), BUILT).unwrap();
        tree(&dir.join("slot"));
        made(&keys, &dir.join("slot"), "0.15.0", &dir.join("r1"));
        std::fs::write(dir.join("slot/commands/spark"), b"spark, newer").unwrap();
        made(&keys, &dir.join("slot"), "0.15.1", &dir.join("r2"));
        let repo_dir = dir.join("repo");
        let now = tuf::time::now();
        let first = add(&repo_dir, &keys, &dir.join("r1"), now).unwrap();
        assert_eq!((first.new_files, first.already), (2, false));
        let second = add(&repo_dir, &keys, &dir.join("r2"), now).unwrap();
        assert_eq!((second.new_files, second.already), (1, false));
        assert!(add(&repo_dir, &keys, &dir.join("r2"), now).unwrap().already);
        // Both read back; the store holds each file once.
        for version in ["0.15.0", "0.15.1"] {
            assert_eq!(
                repo::open(&repo_dir, keys.root(), Some(version)).unwrap().release.version,
                version
            );
        }
        assert_eq!(std::fs::read_dir(repo_dir.join("files")).unwrap().count(), 3);
        // A version is never used twice.
        std::fs::write(dir.join("slot/commands/spark"), b"spark, other").unwrap();
        made(&keys, &dir.join("slot"), "0.15.1", &dir.join("r3"));
        assert!(
            add(&repo_dir, &keys, &dir.join("r3"), now).unwrap_err().contains("never used twice")
        );
        // Channels.
        set_channel(&repo_dir, &keys, "stable", "0.15.1", now).unwrap();
        assert!(
            set_channel(&repo_dir, &keys, "lts", "0.15.1", now).unwrap_err().contains("no channel")
        );
        assert!(
            set_channel(&repo_dir, &keys, "beta", "0.16.0", now)
                .unwrap_err()
                .contains("does not offer")
        );
        let before = current(&repo_dir).unwrap().unwrap().timestamp_version;
        assert_eq!(refresh(&repo_dir, &keys, now).unwrap(), before + 1);
        let current = current(&repo_dir).unwrap().unwrap();
        assert!(current.targets.contains_key("channels/stable.json"));
        assert_eq!(current.targets.len(), 3);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_rotated_key_is_walked_to_from_the_first_root() {
        let dir = scratch("updates-rotate");
        let keys = Keys::create(&dir.join("keys"), BUILT).unwrap();
        let first = keys.root().to_vec();
        tree(&dir.join("slot"));
        made(&keys, &dir.join("slot"), "0.15.0", &dir.join("r1"));
        let repo_dir = dir.join("repo");
        let now = tuf::time::now();
        add(&repo_dir, &keys, &dir.join("r1"), now).unwrap();
        let keys = Keys::rotate(&dir.join("keys"), "timestamp", now).unwrap();
        let keys = {
            drop(keys);
            Keys::rotate(&dir.join("keys"), "root", now).unwrap()
        };
        assert_eq!(keys.roots().len(), 3);
        assert_eq!(Keys::load(&dir.join("keys")).unwrap().roots().len(), 3);
        refresh(&repo_dir, &keys, now).unwrap();
        // A computer that trusts the first root walks to the third.
        assert_eq!(
            repo::open(&repo_dir, &first, Some("0.15.0")).unwrap().release.version,
            "0.15.0"
        );
        assert!(Keys::rotate(&dir.join("keys"), "mirror", now).err().unwrap().contains("no role"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn an_update_file_holds_one_release_and_refuses_damage() {
        let dir = scratch("updates-pack");
        let keys = Keys::create(&dir.join("keys"), BUILT).unwrap();
        tree(&dir.join("slot"));
        made(&keys, &dir.join("slot"), "0.15.0", &dir.join("r1"));
        made(&keys, &dir.join("slot"), "0.15.1", &dir.join("r2"));
        let repo_dir = dir.join("repo");
        add(&repo_dir, &keys, &dir.join("r1"), tuf::time::now()).unwrap();
        // The repository offers two; the file holds the list of one.
        add(&repo_dir, &keys, &dir.join("r2"), tuf::time::now()).unwrap();
        let out = dir.join("dukovo-0.15.0.update");
        let packed = pack(&repo_dir, keys.root(), "0.15.0", &out).unwrap();
        assert_eq!(packed.files, 2);
        // The same bytes every time.
        let bytes = std::fs::read(&out).unwrap();
        pack(&repo_dir, keys.root(), "0.15.0", &dir.join("again.update")).unwrap();
        assert_eq!(std::fs::read(dir.join("again.update")).unwrap(), bytes);
        let (published, archive) = open_file(&out, keys.root(), None).unwrap();
        assert_eq!(published.release.version, "0.15.0");
        assert_eq!(check_archive(&archive, &published.release).unwrap().files, 2);
        // A changed byte in a file is found; a changed byte in the
        // metadata fails the signatures.
        let at = |needle: &[u8]| bytes.windows(needle.len()).position(|w| w == needle).unwrap();
        let mut damaged = bytes.clone();
        damaged[at(b"spark's bytes")] ^= 1;
        std::fs::write(dir.join("damaged.update"), &damaged).unwrap();
        let (published, archive) =
            open_file(&dir.join("damaged.update"), keys.root(), None).unwrap();
        let problems = check_archive(&archive, &published.release).unwrap_err();
        assert_eq!(problems, ["/system/commands/spark: damaged: its SHA-256 differs"]);
        let mut forged = bytes.clone();
        let version = at(b"\"0.15.0\"");
        forged[version + 4] = b'9';
        std::fs::write(dir.join("forged.update"), &forged).unwrap();
        assert!(open_file(&dir.join("forged.update"), keys.root(), None).is_err());
        // Another computer's root trusts none of it.
        let other = Keys::create(&dir.join("other"), BUILT).unwrap();
        assert!(open_file(&out, other.root(), None).is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
