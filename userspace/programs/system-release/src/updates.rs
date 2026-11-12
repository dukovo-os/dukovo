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
//! targets/firmware/<sha256>.<version>.json each firmware set's list (ADR 0468)
//! targets/channels/<sha256>.<name>.json  a channel: the release it offers,
//!                                        and the firmware set
//! files/<sha256>                         every file of every release and set, once
//! ```
//!
//! [`add`] adds a release that `make` signed, [`add_firmware`] a firmware
//! set, [`set_channel`] points a channel at a release and
//! [`set_channel_firmware`] at a set, and [`refresh`] signs a new
//! snapshot and timestamp, as
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
use slots::version::Version;
use tuf::repo as signing;

use crate::archive::{Archive, Writer};
use crate::firmware::{self, Load, Set};
use crate::list::{Release, hex, sha256_file};
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

/// A channel's file: the release it offers now, and the firmware set,
/// for machines that take the optional firmware package (ADR 0468).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Channel {
    /// Its name, which must be the one asked for, so that a file signed
    /// for one channel is not taken for another's.
    pub channel: String,
    /// The version of the release it offers.
    pub version: String,
    /// The version of the firmware set it offers, if it offers one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub firmware: Option<String>,
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
    /// Every target, by path, as the newest targets list it.
    targets: BTreeMap<String, Value>,
    /// The newest targets', snapshot's and timestamp's versions, 0 for
    /// none: a repository whose snapshot and timestamp the update server
    /// signs has none here.
    targets_version: u64,
    snapshot_version: u64,
    timestamp_version: u64,
    /// The targets version the newest snapshot lists, and when that
    /// snapshot expires.
    snapshot: Option<(u64, i64)>,
}

fn read(path: &Path) -> Result<Vec<u8>, String> {
    std::fs::read(path).map_err(|err| format!("{}: {err}", path.display()))
}

fn write(path: &Path, bytes: &[u8]) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|err| format!("{}: {err}", parent.display()))?;
    }
    // Whole or not at all, for a server that serves it as it changes; and
    // a new file, never the old one written over, which a copy of the
    // folder made of hard links may share.
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

/// The newest version of `role`'s metadata in the folder `metadata`, by
/// the names `N.role.json`, or 0.
fn newest(metadata: &Path, role: &str) -> Result<u64, String> {
    let entries = match std::fs::read_dir(metadata) {
        Ok(entries) => entries,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(err) => return Err(format!("{}: {err}", metadata.display())),
    };
    let suffix = format!(".{role}.json");
    let mut found = 0;
    for entry in entries {
        let entry = entry.map_err(|err| format!("{}: {err}", metadata.display()))?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if let Some(version) = name.strip_suffix(&suffix).and_then(|n| n.parse::<u64>().ok()) {
            found = found.max(version);
        }
    }
    Ok(found)
}

/// Reads what the repository at `dir` says now, if it offers anything.
fn current(dir: &Path) -> Result<Option<Current>, String> {
    let metadata = dir.join("metadata");
    let targets_version = newest(&metadata, "targets")?;
    if targets_version == 0 {
        return Ok(None);
    }
    let json = |name: String| -> Result<Value, String> {
        let path = metadata.join(&name);
        serde_json::from_slice(&read(&path)?).map_err(|err| format!("{}: {err}", path.display()))
    };
    let malformed = |name: &str| format!("{}: {name} is malformed", metadata.display());
    let snapshot_version = newest(&metadata, "snapshot")?;
    let snapshot = if snapshot_version == 0 {
        None
    } else {
        let snapshot = json(format!("{snapshot_version}.snapshot.json"))?;
        let lists =
            listed_version(&snapshot, "targets.json").ok_or_else(|| malformed("the snapshot"))?;
        let expires = snapshot["signed"]["expires"]
            .as_str()
            .and_then(tuf::time::parse)
            .ok_or_else(|| malformed("the snapshot"))?;
        Some((lists, expires))
    };
    let timestamp_version = if metadata.join("timestamp.json").exists() {
        json("timestamp.json".into())?["signed"]["version"]
            .as_u64()
            .ok_or_else(|| malformed("timestamp.json"))?
    } else {
        0
    };
    let targets = json(format!("{targets_version}.targets.json"))?;
    let targets = targets["signed"]["targets"]
        .as_object()
        .ok_or_else(|| malformed("the targets"))?
        .iter()
        .map(|(path, info)| (path.clone(), info.clone()))
        .collect();
    Ok(Some(Current { targets, targets_version, snapshot_version, timestamp_version, snapshot }))
}

/// Writes every root of `keys` into the metadata folder `metadata`, by
/// version, and the newest as `root.json`, leaving those already there
/// as they are. Gives whether there was a root to write.
fn write_roots(metadata: &Path, keys: &Keys) -> Result<bool, String> {
    let mut files: Vec<(String, &[u8])> = keys
        .roots()
        .iter()
        .enumerate()
        .map(|(n, root)| (format!("{}.root.json", n + 1), root.as_slice()))
        .collect();
    files.push(("root.json".into(), keys.root()));
    let mut wrote = false;
    for (name, bytes) in files {
        let path = metadata.join(name);
        if std::fs::read(&path).ok().as_deref() != Some(bytes) {
            write(&path, bytes)?;
            wrote = true;
        }
    }
    Ok(wrote)
}

/// What [`sign`] signed.
enum Signed {
    /// The targets, and every root: the update server signs the snapshot
    /// and timestamp, as these keys do not.
    Targets,
    /// The snapshot and timestamp too, of these versions.
    All { snapshot: u64, timestamp: u64 },
}

/// Signs the repository at `dir` anew at `now`: the targets `targets` as
/// the next version, if given, and every root of `keys`; then, if `keys`
/// sign for them, the next snapshot and timestamp.
fn sign(
    dir: &Path,
    keys: &Keys,
    current: Option<&Current>,
    targets: Option<&BTreeMap<String, Value>>,
    now: i64,
) -> Result<Signed, String> {
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
    write_roots(&metadata, keys)?;
    if !repo::ONLINE.iter().all(|role| keys.has(role)) {
        return Ok(Signed::Targets);
    }
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
    let timestamp =
        sign_timestamp(&metadata, keys, timestamp_version + 1, snapshot_version, &snapshot, now)?;
    Ok(Signed::All { snapshot: snapshot_version, timestamp })
}

/// Signs and writes the timestamp `version`, naming the snapshot
/// `snapshot_version`, whose bytes are `snapshot`.
fn sign_timestamp(
    metadata: &Path,
    keys: &Keys,
    version: u64,
    snapshot_version: u64,
    snapshot: &[u8],
    now: i64,
) -> Result<u64, String> {
    let timestamp = repo::sign_role(
        &signing::timestamp(version, &expires(now, "timestamp"), snapshot_version, snapshot),
        keys,
        "timestamp",
    )?;
    write(&metadata.join("timestamp.json"), &timestamp)?;
    Ok(version)
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
    let signed = sign(dir, keys, current.as_ref(), Some(&targets), now)?;
    // Signed through, it reads back here; else the update server reads it
    // back once it signs the snapshot and timestamp.
    let published = match signed {
        Signed::All { .. } => repo::open(dir, first, Some(&release.version))?,
        Signed::Targets => published,
    };
    Ok(Added { published, new_files, already: false })
}

/// Points the channel `channel` of the repository at `dir` at its release
/// `version`, keeping the firmware set it offers, and signs the targets,
/// snapshot and timestamp anew.
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
    let firmware = held_channel(dir, &current, channel)?.and_then(|held| held.firmware);
    let channel = Channel { channel: channel.into(), version: version.into(), firmware };
    write_channel(dir, keys, &current, &channel, now)
}

/// Points the channel `channel` of the repository at `dir`, which offers
/// a release already, at its firmware set `firmware`, or at none, and
/// signs the targets, snapshot and timestamp anew (ADR 0468).
///
/// # Errors
///
/// If the channel offers no release yet, the repository does not offer
/// the set, or a write fails.
pub fn set_channel_firmware(
    dir: &Path,
    keys: &Keys,
    channel: &str,
    firmware: Option<&str>,
    now: i64,
) -> Result<(), String> {
    let current = current(dir)?.ok_or_else(|| format!("{} offers no release", dir.display()))?;
    let held = held_channel(dir, &current, channel)?.ok_or_else(|| {
        format!("the {channel} channel of {} offers no release yet", dir.display())
    })?;
    if let Some(version) = firmware
        && !current.targets.contains_key(&firmware::target_path(version))
    {
        return Err(format!("{} does not offer firmware {version}", dir.display()));
    }
    let channel = Channel { firmware: firmware.map(String::from), ..held };
    write_channel(dir, keys, &current, &channel, now)
}

/// Writes `channel`'s file among the targets of the repository at `dir`,
/// and signs them anew.
fn write_channel(
    dir: &Path,
    keys: &Keys,
    current: &Current,
    channel: &Channel,
    now: i64,
) -> Result<(), String> {
    let bytes = channel.to_bytes();
    let name = hex(ring::digest::digest(&ring::digest::SHA256, &bytes).as_ref());
    write(&dir.join(format!("targets/channels/{name}.{}.json", channel.channel)), &bytes)?;
    let mut targets = current.targets.clone();
    targets.insert(channel_path(&channel.channel), signing::file_info(&bytes));
    sign(dir, keys, Some(current), Some(&targets), now)?;
    Ok(())
}

/// The file of the channel `name` that the newest targets of the
/// repository at `dir` list, if they list one.
fn held_channel(dir: &Path, current: &Current, name: &str) -> Result<Option<Channel>, String> {
    let Some(info) = current.targets.get(&channel_path(name)) else { return Ok(None) };
    let sha256 = info["hashes"]["sha256"]
        .as_str()
        .ok_or_else(|| format!("{}: the {name} channel is listed without a hash", dir.display()))?;
    let bytes = read(&dir.join(format!("targets/channels/{sha256}.{name}.json")))?;
    Channel::parse(&bytes, name).map(Some)
}

/// What [`add_firmware`] did.
#[derive(Debug)]
pub struct AddedFirmware {
    /// The set, as signed.
    pub set: Set,
    /// How many of its files the store did not hold already.
    pub new_files: usize,
    /// Whether the repository offered it already, with the same list.
    pub already: bool,
}

/// One file to add to a firmware set: where it is, how it is loaded and
/// its license.
pub struct FirmwareFile<'a> {
    /// The file.
    pub path: &'a Path,
    /// How it is loaded.
    pub load: Load,
    /// Its license, as an SPDX expression.
    pub license: &'a str,
}

/// Adds the firmware set `version` of `files` to the update repository
/// at `dir`, made if it is not there: its list among the targets, signed
/// with `keys` as a release's is, and its files in the store, each once,
/// each named in the list by its file name (ADR 0468).
///
/// # Errors
///
/// If a file cannot be read, the set does not check, the repository
/// offers the version already with another list (a version is never
/// used twice), or a write fails.
pub fn add_firmware(
    dir: &Path,
    keys: &Keys,
    version: &str,
    files: &[FirmwareFile<'_>],
    now: i64,
) -> Result<AddedFirmware, String> {
    let mut listed = Vec::new();
    for file in files {
        let name = file
            .path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .ok_or_else(|| format!("{}: not a file", file.path.display()))?;
        let size = std::fs::metadata(file.path)
            .map_err(|err| format!("{}: {err}", file.path.display()))?
            .len();
        listed.push(firmware::File {
            name,
            load: file.load,
            sha256: sha256_file(file.path)?,
            size,
            license: file.license.to_string(),
        });
    }
    let set = Set { firmware: version.to_string(), files: listed };
    set.check()?;
    let list = set.to_bytes();
    let info = signing::file_info(&list);
    let current = current(dir)?;
    let mut targets = current.as_ref().map_or_else(BTreeMap::new, |c| c.targets.clone());
    let path = firmware::target_path(version);
    if let Some(offered) = targets.get(&path) {
        if *offered != info {
            return Err(format!(
                "{} offers firmware {version} already, with another list: a version is never \
                 used twice",
                dir.display()
            ));
        }
        return Ok(AddedFirmware { set, new_files: 0, already: true });
    }
    let store = dir.join("files");
    std::fs::create_dir_all(&store).map_err(|err| format!("{}: {err}", store.display()))?;
    let mut new_files = 0;
    for (file, listed) in files.iter().zip(&set.files) {
        let to = store.join(&listed.sha256);
        if to.exists() {
            continue;
        }
        let partial = store.join(format!(".{}.partial", listed.sha256));
        if copy_hashing(file.path, &partial)? != listed.sha256 {
            let _ = std::fs::remove_file(&partial);
            return Err(format!("{}: it changed while it was read", file.path.display()));
        }
        std::fs::rename(&partial, &to).map_err(|err| format!("{}: {err}", to.display()))?;
        new_files += 1;
    }
    let sha256 = hex(ring::digest::digest(&ring::digest::SHA256, &list).as_ref());
    write(&dir.join(format!("targets/firmware/{sha256}.{version}.json")), &list)?;
    targets.insert(path, info);
    sign(dir, keys, current.as_ref(), Some(&targets), now)?;
    Ok(AddedFirmware { set, new_files, already: false })
}

/// What [`refresh`] signed.
#[derive(Debug, PartialEq, Eq)]
pub enum Refreshed {
    /// A timestamp alone, of this version: the snapshot lists the newest
    /// targets and is good for two more days.
    Timestamp(u64),
    /// A snapshot and a timestamp, of these versions.
    All {
        /// The snapshot's version.
        snapshot: u64,
        /// The timestamp's version.
        timestamp: u64,
    },
}

/// How long a snapshot must still be good for a refresh to sign a
/// timestamp alone, as the catalog's (ADR 0382).
const SNAPSHOT_MARGIN: i64 = 2 * 86_400;

/// Signs the repository at `dir` again as of `now`, the targets as they
/// are: a timestamp alone while the snapshot lists the newest targets,
/// is good for two more days and no new root has replaced a key, or else
/// a snapshot and a timestamp, as the update server does every six hours
/// with `keys`, its online key.
///
/// # Errors
///
/// If the repository offers nothing, `keys` do not sign for snapshot and
/// timestamp, or a write fails.
pub fn refresh(dir: &Path, keys: &Keys, now: i64) -> Result<Refreshed, String> {
    let current = current(dir)?.ok_or_else(|| format!("{} offers no release", dir.display()))?;
    for role in repo::ONLINE {
        keys.signer(role)?;
    }
    let metadata = dir.join("metadata");
    let new_root = write_roots(&metadata, keys)?;
    if let Some((lists, expires)) = current.snapshot
        && lists == current.targets_version
        && expires > now + SNAPSHOT_MARGIN
        && !new_root
    {
        let snapshot = read(&metadata.join(format!("{}.snapshot.json", current.snapshot_version)))?;
        let version = sign_timestamp(
            &metadata,
            keys,
            current.timestamp_version + 1,
            current.snapshot_version,
            &snapshot,
            now,
        )?;
        return Ok(Refreshed::Timestamp(version));
    }
    match sign(dir, keys, Some(&current), None, now)? {
        Signed::All { snapshot, timestamp } => Ok(Refreshed::All { snapshot, timestamp }),
        Signed::Targets => Err("these keys do not sign for snapshot and timestamp".into()),
    }
}

/// What [`publish`] published.
#[derive(Debug)]
pub struct Deployed {
    /// The versions of every release offered, oldest first.
    pub releases: Vec<String>,
    /// The versions of every firmware set offered, oldest first.
    pub firmware: Vec<String>,
    /// Each channel, and the release and firmware set it offers.
    pub channels: Vec<Channel>,
    /// How many files of the deploy the store did not hold already.
    pub new_files: usize,
    /// How the repository was signed.
    pub signed: Refreshed,
}

/// Publishes what a deploy left in `incoming` into the update server's
/// repository at `dir`, with its online key `secret` (ADR 0467): the
/// roots and targets the release keys signed, the release lists and
/// channels, and the files, each checked against its SHA-256 as it is
/// copied, into a store that never replaces one. Then signs a snapshot
/// and timestamp, and reads every release and channel back as a computer
/// online does, from the first root, and checks that the store holds
/// every file each release lists, of its size.
///
/// # Errors
///
/// If a file of the deploy would replace one with other bytes (the first
/// root above all: every later root and targets must follow from it), is
/// not what its name says, the newest root does not name `secret`'s key,
/// or what was signed does not read back. What was written stays in
/// `dir`, which the server then does not serve.
pub fn publish(dir: &Path, incoming: &Path, secret: &[u8], now: i64) -> Result<Deployed, String> {
    take_metadata(&incoming.join("metadata"), &dir.join("metadata"))?;
    for folder in ["targets/system", "targets/firmware", "targets/channels"] {
        take_named(&incoming.join(folder), &dir.join(folder), false)?;
    }
    let new_files = take_named(&incoming.join("files"), &dir.join("files"), true)?;
    let keys = server_keys(dir, secret)?;
    let first = keys.roots()[0].clone();
    let signed = refresh(dir, &keys, now)?;
    let store = dir.join("files");
    let Offered { releases, firmware, channels } =
        offered(repo::read_targets(dir, &first)?, Some(&store))?;
    Ok(Deployed { releases, firmware, channels, new_files, signed })
}

/// What a repository offers, as a computer reads it.
#[derive(Debug)]
pub struct Offered {
    /// The versions of every release, oldest first.
    pub releases: Vec<String>,
    /// The versions of every firmware set, oldest first.
    pub firmware: Vec<String>,
    /// Each channel, and the release and firmware set it offers.
    pub channels: Vec<Channel>,
}

/// Reads the repository an update server serves at `base` as a computer
/// online does, from `root`: every release list and channel, each
/// checked (ADR 0467).
///
/// # Errors
///
/// If the client refuses what is served, a list or channel is malformed,
/// or a channel offers a release the repository does not.
pub fn served(base: &tuf::Url, root: &[u8]) -> Result<Offered, String> {
    offered(repo::read_served(base, root)?, None)
}

/// The releases, firmware sets and channels among `targets`, as the
/// client read them: each list where its version says, each channel
/// offering a release and a set listed, and, with `store`, every file of
/// every release and set there, of its size.
fn offered(targets: Vec<(String, Vec<u8>)>, store: Option<&Path>) -> Result<Offered, String> {
    let held = |sha256: &str, size: u64| {
        store.is_none_or(|store| {
            std::fs::metadata(store.join(sha256)).is_ok_and(|m| m.len() == size)
        })
    };
    let mut firmware = Vec::new();
    for (set, _) in firmware::sets(&targets)? {
        if let Some(file) = set.files.iter().find(|f| !held(&f.sha256, f.size)) {
            return Err(format!(
                "firmware {}: the store holds no file {} of {} bytes",
                set.firmware, file.sha256, file.size
            ));
        }
        firmware.push(set.firmware);
    }
    let mut releases = Vec::new();
    let mut channels = Vec::new();
    for (path, bytes) in targets {
        if let Some(name) = path.strip_prefix("channels/").and_then(|p| p.strip_suffix(".json")) {
            channels.push(Channel::parse(&bytes, name)?);
        } else if path.starts_with("system/") {
            let release = Release::parse(&bytes)?;
            if repo::target_path(&release.version) != path {
                return Err(format!("{path} holds the list of release {}", release.version));
            }
            for (sha256, size) in release
                .files
                .iter()
                .filter_map(crate::list::Entry::file)
                .filter(|_| store.is_some())
            {
                let held = store
                    .and_then(|store| std::fs::metadata(store.join(sha256)).ok())
                    .map(|m| m.len());
                if held != Some(size) {
                    return Err(format!(
                        "Dukovo {}: the store holds no file {sha256} of {size} bytes",
                        release.version
                    ));
                }
            }
            releases.push(release.version);
        }
    }
    for channel in &channels {
        if !releases.contains(&channel.version) {
            return Err(format!(
                "the {} channel offers Dukovo {}, which the repository does not",
                channel.channel, channel.version
            ));
        }
        if let Some(set) = channel.firmware.as_ref().filter(|set| !firmware.contains(set)) {
            return Err(format!(
                "the {} channel offers firmware {set}, which the repository does not",
                channel.channel
            ));
        }
    }
    releases.sort_by(|a, b| match (Version::parse(a), Version::parse(b)) {
        (Some(a), Some(b)) => a.cmp(&b),
        _ => a.cmp(b),
    });
    Ok(Offered { releases, firmware, channels })
}

/// The update server's keys for the repository at `dir`: its online key,
/// whose secret half is `secret`, with the repository's roots.
///
/// # Errors
///
/// If the repository has no root, or the newest does not name the key
/// for snapshot and timestamp.
pub fn server_keys(dir: &Path, secret: &[u8]) -> Result<Keys, String> {
    let metadata = dir.join("metadata");
    let mut roots = Vec::new();
    for version in 1..=newest(&metadata, "root")? {
        roots.push(read(&metadata.join(format!("{version}.root.json")))?);
    }
    if roots.is_empty() {
        return Err(format!("{} has no root", dir.display()));
    }
    Keys::online(roots, secret)
}

/// Copies the roots, `N.root.json`, and targets, `N.targets.json`, from
/// the deploy's `from` into `to`; one already there must be the same
/// bytes. Nothing else of the deploy's metadata is read: the snapshot and
/// timestamp are the server's own.
fn take_metadata(from: &Path, to: &Path) -> Result<(), String> {
    let entries = std::fs::read_dir(from).map_err(|err| format!("{}: {err}", from.display()))?;
    let mut names = Vec::new();
    for entry in entries {
        let name = entry
            .map_err(|err| format!("{}: {err}", from.display()))?
            .file_name()
            .to_string_lossy()
            .into_owned();
        let numbered = |role: &str| {
            name.strip_suffix(&format!(".{role}.json"))
                .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
        };
        if numbered("root") || numbered("targets") {
            names.push(name);
        }
    }
    names.sort();
    for name in names {
        let bytes = read(&from.join(&name))?;
        let path = to.join(&name);
        match std::fs::read(&path) {
            Ok(held) if held == bytes => {}
            Ok(_) if name == "1.root.json" => {
                return Err(
                    "1.root.json is not the first root this server published; nothing was \
                     published"
                        .into(),
                );
            }
            Ok(_) => {
                return Err(format!(
                    "{name} is not the {name} this server published; nothing was published"
                ));
            }
            Err(_) => write(&path, &bytes)?,
        }
    }
    let newest = newest(to, "root")?;
    if newest > 0 {
        let root = read(&to.join(format!("{newest}.root.json")))?;
        write_if_changed(&to.join("root.json"), &root)?;
    }
    Ok(())
}

fn write_if_changed(path: &Path, bytes: &[u8]) -> Result<(), String> {
    if std::fs::read(path).ok().as_deref() == Some(bytes) { Ok(()) } else { write(path, bytes) }
}

/// Copies each file in the deploy's folder `from` that `to` does not hold
/// yet, named by its SHA-256 (`sha256` for the store, `sha256.rest` for a
/// list or a channel), checked against it as it is copied. Gives how
/// many were copied; none is ever replaced.
fn take_named(from: &Path, to: &Path, store: bool) -> Result<usize, String> {
    let entries = match std::fs::read_dir(from) {
        Ok(entries) => entries,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(err) => return Err(format!("{}: {err}", from.display())),
    };
    std::fs::create_dir_all(to).map_err(|err| format!("{}: {err}", to.display()))?;
    let mut copied = 0;
    for entry in entries {
        let entry = entry.map_err(|err| format!("{}: {err}", from.display()))?;
        let name = entry.file_name().to_string_lossy().into_owned();
        let sha256 = if store { name.as_str() } else { name.split('.').next().unwrap_or("") };
        let named = sha256.len() == 64 && sha256.bytes().all(|b| b.is_ascii_hexdigit());
        // Lists and channels are written `.json`, and only so.
        let json = Path::new(&name).extension().is_some_and(|ext| ext == "json");
        if !named || (!store && !json) {
            return Err(format!("{}: {name} is not part of a deploy", from.display()));
        }
        let path = to.join(&name);
        if path.exists() {
            continue;
        }
        let partial = to.join(format!(".{name}.partial"));
        let found = copy_hashing(&entry.path(), &partial)?;
        if found != sha256 {
            let _ = std::fs::remove_file(&partial);
            return Err(format!("{}: {name} is not the file its name says", from.display()));
        }
        std::fs::rename(&partial, &path).map_err(|err| format!("{}: {err}", path.display()))?;
        copied += 1;
    }
    Ok(copied)
}

/// Copies `from` to `to`, giving its SHA-256 in hex.
fn copy_hashing(from: &Path, to: &Path) -> Result<String, String> {
    use std::io::{Read as _, Write as _};
    let mut source =
        std::fs::File::open(from).map_err(|err| format!("{}: {err}", from.display()))?;
    let mut out = std::fs::File::create(to).map_err(|err| format!("{}: {err}", to.display()))?;
    let mut digest = ring::digest::Context::new(&ring::digest::SHA256);
    let mut buffer = vec![0; 1 << 16];
    loop {
        let n = source.read(&mut buffer).map_err(|err| format!("{}: {err}", from.display()))?;
        if n == 0 {
            break;
        }
        digest.update(&buffer[..n]);
        out.write_all(&buffer[..n]).map_err(|err| format!("{}: {err}", to.display()))?;
    }
    out.sync_all().map_err(|err| format!("{}: {err}", to.display()))?;
    Ok(hex(digest.finish().as_ref()))
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
        assert_eq!(refresh(&repo_dir, &keys, now).unwrap(), Refreshed::Timestamp(before + 1));
        let current = current(&repo_dir).unwrap().unwrap();
        assert!(current.targets.contains_key("channels/stable.json"));
        assert_eq!(current.targets.len(), 3);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// Adds the firmware set `version` of one microcode file holding
    /// `bytes` to the repository at `dir`.
    fn firmware_set(dir: &Path, keys: &Keys, version: &str, bytes: &[u8]) -> AddedFirmware {
        let file = dir.with_extension(format!("{version}.bin"));
        std::fs::write(&file, bytes).unwrap();
        let listed =
            FirmwareFile { path: &file, load: Load::Microcode, license: "LicenseRef-Test" };
        add_firmware(dir, keys, version, &[listed], tuf::time::now()).unwrap()
    }

    #[test]
    fn firmware_sets_are_offered_beside_releases() {
        let dir = scratch("updates-firmware");
        let keys = Keys::create(&dir.join("keys"), BUILT).unwrap();
        tree(&dir.join("slot"));
        made(&keys, &dir.join("slot"), "0.15.0", &dir.join("r1"));
        let repo_dir = dir.join("repo");
        let now = tuf::time::now();
        add(&repo_dir, &keys, &dir.join("r1"), now).unwrap();
        let added = firmware_set(&repo_dir, &keys, "2026.11.12", b"microcode, first");
        assert_eq!((added.new_files, added.already), (1, false));
        assert!(firmware_set(&repo_dir, &keys, "2026.11.12", b"microcode, first").already);
        let err = add_firmware(&repo_dir, &keys, "2026.11.12", &[], now).unwrap_err();
        assert!(err.contains("never used twice"), "{err}");
        // A channel offers a set only beside a release, and keeps it when
        // it moves to another release.
        let err = set_channel_firmware(&repo_dir, &keys, "stable", Some("2026.11.12"), now);
        assert!(err.unwrap_err().contains("offers no release yet"));
        set_channel(&repo_dir, &keys, "stable", "0.15.0", now).unwrap();
        let err = set_channel_firmware(&repo_dir, &keys, "stable", Some("2026.11.13"), now);
        assert!(err.unwrap_err().contains("does not offer firmware 2026.11.13"));
        set_channel_firmware(&repo_dir, &keys, "stable", Some("2026.11.12"), now).unwrap();
        set_channel(&repo_dir, &keys, "stable", "0.15.0", now).unwrap();
        let read = |repo_dir: &Path| {
            let offered =
                offered(repo::read_targets(repo_dir, keys.root()).unwrap(), None).unwrap();
            (offered.firmware, offered.channels[0].firmware.clone())
        };
        assert_eq!(read(&repo_dir), (vec!["2026.11.12".into()], Some("2026.11.12".into())));
        let (set, _) = firmware::open(&repo_dir, keys.root(), None).unwrap();
        assert_eq!(set.microcode().unwrap().size, 16);
        set_channel_firmware(&repo_dir, &keys, "stable", None, now).unwrap();
        assert_eq!(read(&repo_dir).1, None);
        // A set whose file the store lacks is not offered.
        let file = repo_dir.join("files").join(&set.files[0].sha256);
        std::fs::remove_file(&file).unwrap();
        let targets = repo::read_targets(&repo_dir, keys.root()).unwrap();
        let err = offered(targets, Some(&repo_dir.join("files"))).unwrap_err();
        assert!(err.contains("firmware 2026.11.12: the store holds no file"), "{err}");
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
    fn the_server_publishes_a_deploy_with_its_own_key() {
        let dir = scratch("updates-server");
        let keys_dir = dir.join("keys");
        Keys::create(&keys_dir, BUILT).unwrap();
        let (online, secret) = signing::Ed25519::generate().unwrap();
        let now = tuf::time::now();
        let keys = Keys::set_online(&keys_dir, signing::Signer::key(&online), now).unwrap();
        assert!(keys.has("snapshot") && keys.has("timestamp"));
        tree(&dir.join("slot"));
        made(&keys, &dir.join("slot"), "0.15.0", &dir.join("r1"));
        let maintainer = dir.join("maintainer");
        add(&maintainer, &keys, &dir.join("r1"), now).unwrap();
        set_channel(&maintainer, &keys, "stable", "0.15.0", now).unwrap();
        firmware_set(&maintainer, &keys, "2026.11.12", b"microcode");
        set_channel_firmware(&maintainer, &keys, "stable", Some("2026.11.12"), now).unwrap();

        // The server takes the roots, targets, lists and files, and signs
        // its own snapshot and timestamp, which a computer reads from the
        // first root.
        let server = dir.join("server");
        let deployed = publish(&server, &maintainer, &secret, now).unwrap();
        assert_eq!(deployed.releases, ["0.15.0"]);
        assert_eq!(deployed.firmware, ["2026.11.12"]);
        let stable = Channel {
            channel: "stable".into(),
            version: "0.15.0".into(),
            firmware: Some("2026.11.12".into()),
        };
        assert_eq!(deployed.channels, [stable]);
        assert_eq!(deployed.new_files, 3);
        assert_eq!(deployed.signed, Refreshed::All { snapshot: 1, timestamp: 1 });
        let timestamp: Value =
            serde_json::from_slice(&read(&server.join("metadata/timestamp.json")).unwrap())
                .unwrap();
        assert_eq!(timestamp["signatures"][0]["keyid"], signing::Signer::key(&online).id());
        let first = keys.roots()[0].clone();
        assert_eq!(repo::open(&server, &first, Some("0.15.0")).unwrap().release.version, "0.15.0");

        // The same deploy again copies nothing and signs a timestamp; so
        // does a refresh, until the snapshot has two days left.
        let again = publish(&server, &maintainer, &secret, now).unwrap();
        assert_eq!((again.new_files, again.signed), (0, Refreshed::Timestamp(2)));
        let online_keys = server_keys(&server, &secret).unwrap();
        assert_eq!(refresh(&server, &online_keys, now).unwrap(), Refreshed::Timestamp(3));
        assert_eq!(
            refresh(&server, &online_keys, now + 6 * 86_400).unwrap(),
            Refreshed::All { snapshot: 2, timestamp: 4 }
        );

        // Refused: another first root, a file that is not what its name
        // says, and a key the root does not name.
        let copy = |name: &str| {
            let to = dir.join(name);
            let status = std::process::Command::new("cp")
                .arg("-a")
                .arg(&maintainer)
                .arg(&to)
                .status()
                .unwrap();
            assert!(status.success());
            to
        };
        let other = Keys::create(&dir.join("other"), BUILT).unwrap();
        let forged = copy("forged");
        std::fs::write(forged.join("metadata/1.root.json"), other.root()).unwrap();
        let err = publish(&server, &forged, &secret, now).unwrap_err();
        assert!(err.contains("is not the first root this server published"), "{err}");
        let damaged = copy("damaged");
        std::fs::write(damaged.join("files").join("ab".repeat(32)), b"not that file").unwrap();
        let err = publish(&server, &damaged, &secret, now).unwrap_err();
        assert!(err.contains("is not the file its name says"), "{err}");
        let (_, wrong) = signing::Ed25519::generate().unwrap();
        let err = publish(&server, &maintainer, &wrong, now).unwrap_err();
        assert!(err.contains("does not name this server's online key"), "{err}");
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
