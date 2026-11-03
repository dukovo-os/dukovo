// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The catalog's TUF repository (ADR 0381), made and signed on the
//! computer its YubiKeys are plugged into, through `ob catalog`.
//!
//! What is signed offline is kept in the source tree, in `catalog/`:
//! the public half of each key in `keys/`, and in `metadata/` every
//! version of root, `N.root.json`, the newest also as `root.json`, and the
//! top-level targets, `targets.json`, which lists each bundle with its
//! length, SHA-256, ID and version. Root and targets are signed by a
//! YubiKey, either of the two alone; snapshot and timestamp, which only
//! say which targets are current and that it is still so, by the update
//! server's key, online. The bundles' bytes wait in a store until they
//! are published: `publish` lays out what a server serves, every file by
//! its version or hash, and reads it back with the client before it is
//! done.

pub mod keys;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use tuf::metadata::{Root, Signed, Targets};
use tuf::repo::{self, Signer};

use crate::keys::{KeyFile, Signers};

/// The four top-level roles.
pub const ROLES: [&str; 4] = ["root", "targets", "snapshot", "timestamp"];

/// How long each role's metadata is good for, in days, unless it is told
/// otherwise: root a year, targets half one, so that a batch is signed at
/// least twice a year; snapshot a week and timestamp a day, re-signed by
/// the server every six hours.
pub const EXPIRES: [(&str, i64); 4] =
    [("root", 365), ("targets", 182), ("snapshot", 7), ("timestamp", 1)];

/// How long before it expires `status` warns that a role needs signing.
pub const WARN_DAYS: i64 = 30;

/// The days `role`'s metadata is good for, by default.
#[must_use]
pub fn expires_days(role: &str) -> i64 {
    EXPIRES.iter().find(|(r, _)| *r == role).map_or(1, |(_, days)| *days)
}

/// The catalog's repository in the source tree.
pub struct Repo {
    dir: PathBuf,
}

/// A role's metadata as read: its bytes and what they say.
pub struct Loaded<T> {
    /// The file.
    pub bytes: Vec<u8>,
    /// What it says, checked as metadata.
    pub signed: Signed<T>,
}

fn read<T: tuf::metadata::Kind>(path: &Path) -> Result<Option<Loaded<T>>, String> {
    match std::fs::read(path) {
        Ok(bytes) => {
            let signed = Signed::parse(&bytes).map_err(|e| format!("{}: {e}", path.display()))?;
            Ok(Some(Loaded { bytes, signed }))
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(format!("{}: {err}", path.display())),
    }
}

fn write(path: &Path, data: &[u8]) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
    }
    let partial = path.with_extension("partial");
    std::fs::write(&partial, data).map_err(|e| format!("{}: {e}", partial.display()))?;
    std::fs::rename(&partial, path).map_err(|e| format!("{}: {e}", path.display()))
}

/// What a batch changes in targets.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Changes {
    /// Paths added, with what they list.
    pub added: BTreeMap<String, Value>,
    /// Paths taken out.
    pub removed: Vec<String>,
}

impl Changes {
    /// Whether it changes nothing.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.added.is_empty() && self.removed.is_empty()
    }

    /// One line for each change.
    #[must_use]
    pub fn lines(&self) -> Vec<String> {
        let mut lines = Vec::new();
        for (path, info) in &self.added {
            let custom = &info["custom"];
            lines.push(format!(
                "+ {path} ({} {}, {} bytes)",
                custom["id"].as_str().unwrap_or("?"),
                custom["version"].as_str().unwrap_or("?"),
                info["length"]
            ));
        }
        lines.extend(self.removed.iter().map(|path| format!("- {path}")));
        lines
    }
}

/// A bundle's target: its path, `bundles/ID-VERSION.dkv`, and what
/// targets lists for it: length, SHA-256, and its ID and version.
///
/// # Errors
///
/// If `data` is not a bundle with an ID and version.
pub fn bundle_target(data: &[u8]) -> Result<(String, Value), String> {
    let archive = dkv::zip::Archive::read(data)?;
    let entry = archive.file(dkv::MANIFEST).ok_or("no manifest.json")?;
    let manifest: Value = serde_json::from_slice(&archive.read_small(entry, 1 << 20)?)
        .map_err(|e| format!("manifest.json: {e}"))?;
    let field = |name: &str| {
        manifest[name].as_str().map(str::to_string).ok_or(format!("manifest.json has no {name}"))
    };
    let (id, version) = (field("id")?, field("version")?);
    let safe = |text: &str| {
        !text.is_empty()
            && !text.starts_with('.')
            && text.bytes().all(|b| b.is_ascii_alphanumeric() || b"-_.+~".contains(&b))
    };
    if !safe(&id) || !safe(&version) {
        return Err(format!("{id} {version} cannot name a file"));
    }
    let mut info = repo::file_info(data);
    let mut custom = json!({ "id": id, "version": version });
    if let Ok(name) = field("name") {
        custom["name"] = name.into();
    }
    info["custom"] = custom;
    Ok((format!("bundles/{id}-{version}.{}", dkv::EXTENSION), info))
}

impl Repo {
    /// The repository in `dir`, the source tree's `catalog/`.
    #[must_use]
    pub fn new(dir: &Path) -> Repo {
        Repo { dir: dir.to_path_buf() }
    }

    /// Where the public keys are.
    #[must_use]
    pub fn keys_dir(&self) -> PathBuf {
        self.dir.join("keys")
    }

    fn metadata(&self, name: &str) -> PathBuf {
        self.dir.join("metadata").join(name)
    }

    /// The newest root, if there is one yet.
    ///
    /// # Errors
    ///
    /// If it cannot be read or is not root metadata.
    pub fn root(&self) -> Result<Option<Loaded<Root>>, String> {
        read(&self.metadata("root.json"))
    }

    /// The targets, if any have been signed.
    ///
    /// # Errors
    ///
    /// If they cannot be read or are not targets metadata.
    pub fn targets(&self) -> Result<Option<Loaded<Targets>>, String> {
        read(&self.metadata("targets.json"))
    }

    /// Makes and signs the next root, from the keys in `keys/`: each role
    /// signed for by every key that names it, any one of them alone. It is
    /// signed by every key that `signers` can sign for of the old root's
    /// root keys and the new one's, and must be signed by enough of both.
    ///
    /// # Errors
    ///
    /// If a role has no key, or it cannot be signed by enough keys.
    pub fn sign_root(&self, signers: &Signers, days: i64) -> Result<u64, String> {
        let keys = keys::read_all(&self.keys_dir())?;
        let old = self.root()?;
        let version = old.as_ref().map_or(1, |old| old.signed.version + 1);
        let mut roles = Vec::new();
        for role in ROLES {
            let role_keys: Vec<_> =
                keys.iter().filter(|k| k.signs_for(role)).map(|k| &k.key).collect();
            if role_keys.is_empty() {
                return Err(format!("no key in {} signs for {role}", self.keys_dir().display()));
            }
            roles.push((role, role_keys, 1));
        }
        let signed = repo::root(version, &repo::expires_in(days), &roles, true);
        // The old root's root keys sign too, those still in keys/ through
        // what holds them, any others only through a file given.
        let mut eligible: Vec<KeyFile> =
            keys.iter().filter(|k| k.signs_for("root")).cloned().collect();
        if let Some(old) = &old {
            for id in &old.signed.signed.roles["root"].keyids {
                let Some(key) = old.signed.signed.keys.get(id) else { continue };
                if !keys.iter().any(|k| &k.key == key) {
                    let name = id.clone();
                    eligible.push(KeyFile { name, roles: vec![], yubikey: None, key: key.clone() });
                }
            }
        }
        let found: Vec<Box<dyn Signer + '_>> =
            eligible.iter().filter_map(|k| signers.for_key(k)).collect();
        if found.is_empty() {
            return Err(
                "nothing here can sign for root: plug in a YubiKey that holds a root key".into()
            );
        }
        let refs: Vec<&dyn Signer> = found.iter().map(AsRef::as_ref).collect();
        let bytes = repo::sign(&signed, &refs)?;
        let new = Signed::<Root>::parse(&bytes).map_err(|e| e.to_string())?;
        new.verified_by(&new.signed, "root").map_err(|e| format!("the new root: {e}"))?;
        if let Some(old) = &old {
            new.verified_by(&old.signed.signed, "root")
                .map_err(|e| format!("the old root's keys: {e}"))?;
        }
        write(&self.metadata(&format!("{version}.root.json")), &bytes)?;
        write(&self.metadata("root.json"), &bytes)?;
        Ok(version)
    }

    /// What signing the bundles in `bundles` and taking out `remove`
    /// would change in targets.
    ///
    /// # Errors
    ///
    /// If a bundle cannot be read, or a path already listed would list
    /// other bytes: a version, once signed, is never signed again as
    /// something else.
    pub fn changes(&self, bundles: &[PathBuf], remove: &[String]) -> Result<Changes, String> {
        let listed = self.targets()?.map_or_default(|t| t.signed.signed.targets);
        let mut changes = Changes::default();
        for file in bundles {
            let data = std::fs::read(file).map_err(|e| format!("{}: {e}", file.display()))?;
            let (path, info) =
                bundle_target(&data).map_err(|e| format!("{}: {e}", file.display()))?;
            match listed.get(&path) {
                Some(old)
                    if old.length == data.len() as u64
                        && tuf::metadata::check_file(
                            &data,
                            Some(old.length),
                            Some(&old.hashes),
                        )
                        .is_ok() => {}
                Some(_) => {
                    return Err(format!(
                        "{}: {path} is signed already as other bytes; give the bundle a new version",
                        file.display()
                    ));
                }
                None => {
                    changes.added.insert(path, info);
                }
            }
        }
        for path in remove {
            if !listed.contains_key(path) {
                return Err(format!("{path} is not listed"));
            }
            changes.removed.push(path.clone());
        }
        Ok(changes)
    }

    /// Signs the next targets with `changes`, with one key: one touch of a
    /// YubiKey for the whole batch. The new bundles' bytes are put in
    /// `store`, to be published.
    ///
    /// # Errors
    ///
    /// If there is no root yet, nothing can sign for targets, or a file
    /// cannot be written.
    pub fn sign_targets(
        &self,
        changes: &Changes,
        bundles: &[PathBuf],
        store: &Path,
        signers: &Signers,
        days: i64,
    ) -> Result<u64, String> {
        let root = self.root()?.ok_or("there is no root yet: make one with `root` first")?;
        let keys = keys::read_all(&self.keys_dir())?;
        let ids = &root.signed.signed.roles["targets"].keyids;
        let signer = keys
            .iter()
            .filter(|k| ids.contains(&k.key.id()))
            .find_map(|k| signers.for_key(k))
            .ok_or("nothing here can sign for targets: plug in one of the catalog's YubiKeys")?;
        let old = self.targets()?;
        let version = old.as_ref().map_or(1, |t| t.signed.version + 1);
        let mut listed: BTreeMap<String, Value> = match &old {
            Some(old) => {
                let file: Value = serde_json::from_slice(&old.bytes).map_err(|e| e.to_string())?;
                serde_json::from_value(file["signed"]["targets"].clone())
                    .map_err(|e| e.to_string())?
            }
            None => BTreeMap::new(),
        };
        for path in &changes.removed {
            listed.remove(path);
        }
        for file in bundles {
            let data = std::fs::read(file).map_err(|e| format!("{}: {e}", file.display()))?;
            let (path, _) = bundle_target(&data)?;
            if changes.added.contains_key(&path) {
                write(&store.join(&path), &data)?;
            }
        }
        listed.extend(changes.added.clone());
        let signed = repo::targets(version, &repo::expires_in(days), &listed);
        let bytes = repo::sign(&signed, &[signer.as_ref()])?;
        let new = Signed::<Targets>::parse(&bytes).map_err(|e| e.to_string())?;
        new.verified_by(&root.signed.signed, "targets").map_err(|e| e.to_string())?;
        write(&self.metadata("targets.json"), &bytes)?;
        Ok(version)
    }

    /// Lays out in `out` what a server serves, with consistent snapshots:
    /// every root by its version, the targets by theirs, a new snapshot
    /// and timestamp signed by `signers`' online key, and each target
    /// from `store` by its SHA-256. The client then reads it all back
    /// from `out`, starting from the first root, as a new computer does.
    ///
    /// # Errors
    ///
    /// If there is no root or targets yet, a target is missing or not
    /// what targets says, nothing here signs for snapshot and timestamp,
    /// or the client refuses what was made.
    pub fn publish(
        &self,
        store: &Path,
        out: &Path,
        signers: &Signers,
        days: (i64, i64),
    ) -> Result<(u64, u64), String> {
        let root = self.root()?.ok_or("there is no root yet")?;
        let targets = self.targets()?.ok_or("no targets have been signed yet")?;
        let keys = keys::read_all(&self.keys_dir())?;
        let (snapshot_signer, timestamp_signer) = (
            online_signer(&root, &keys, signers, "snapshot")?,
            online_signer(&root, &keys, signers, "timestamp")?,
        );
        for version in 1..=root.signed.version {
            let name = format!("{version}.root.json");
            let bytes = std::fs::read(self.metadata(&name)).map_err(|e| format!("{name}: {e}"))?;
            write(&out.join("metadata").join(&name), &bytes)?;
        }
        write(&out.join("metadata/root.json"), &root.bytes)?;
        let targets_version = targets.signed.version;
        write(&out.join(format!("metadata/{targets_version}.targets.json")), &targets.bytes)?;
        for (path, target) in &targets.signed.signed.targets {
            let data = std::fs::read(store.join(path)).map_err(|e| format!("{path}: {e}"))?;
            tuf::metadata::check_file(&data, Some(target.length), Some(&target.hashes))
                .map_err(|e| format!("{path} in the store: {e}"))?;
            let (_, sha256) = target
                .hashes
                .0
                .iter()
                .find(|(name, _)| name == "sha256")
                .ok_or(format!("{path} has no SHA-256"))?;
            let (dir, name) = path.rsplit_once('/').unwrap_or(("", path));
            let served = if dir.is_empty() {
                format!("{sha256}.{name}")
            } else {
                format!("{dir}/{sha256}.{name}")
            };
            write(&out.join("targets").join(served), &data)?;
        }
        let previous: Option<Value> = std::fs::read(out.join("metadata/timestamp.json"))
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok());
        let (timestamp_version, snapshot_version) = match &previous {
            Some(file) => (
                file["signed"]["version"].as_u64().unwrap_or(0) + 1,
                file["signed"]["meta"]["snapshot.json"]["version"].as_u64().unwrap_or(0) + 1,
            ),
            None => (1, 1),
        };
        let meta = BTreeMap::from([(
            "targets.json".to_string(),
            repo::meta(targets_version, &targets.bytes),
        )]);
        let snapshot = repo::sign(
            &repo::snapshot(snapshot_version, &repo::expires_in(days.0), &meta),
            &[snapshot_signer.as_ref()],
        )?;
        write(&out.join(format!("metadata/{snapshot_version}.snapshot.json")), &snapshot)?;
        let timestamp = repo::sign(
            &repo::timestamp(
                timestamp_version,
                &repo::expires_in(days.1),
                snapshot_version,
                &snapshot,
            ),
            &[timestamp_signer.as_ref()],
        )?;
        write(&out.join("metadata/timestamp.json"), &timestamp)?;
        check_served(out, &targets.signed.signed)?;
        Ok((snapshot_version, timestamp_version))
    }

    /// Keeps what `out` serves current, as the update server does every
    /// six hours: a new timestamp alone while `out` serves this root and
    /// a snapshot of these targets good for more than [`RENEW_DAYS`], and
    /// everything again, as [`Repo::publish`], otherwise.
    ///
    /// # Errors
    ///
    /// As [`Repo::publish`].
    pub fn refresh(
        &self,
        store: &Path,
        out: &Path,
        signers: &Signers,
        days: (i64, i64),
    ) -> Result<Refreshed, String> {
        let root = self.root()?.ok_or("there is no root yet")?;
        let targets = self.targets()?.ok_or("no targets have been signed yet")?;
        if let Some(served) = Served::read(out)
            && served.root == root.bytes
            && served.targets_version == Some(targets.signed.version)
            && served.snapshot_expires.is_some_and(|t| t - tuf::time::now() > RENEW_DAYS * 86_400)
        {
            let keys = keys::read_all(&self.keys_dir())?;
            let signer = online_signer(&root, &keys, signers, "timestamp")?;
            let version = served.timestamp_version + 1;
            let timestamp = repo::sign(
                &repo::timestamp(
                    version,
                    &repo::expires_in(days.1),
                    served.snapshot_version,
                    &served.snapshot,
                ),
                &[signer.as_ref()],
            )?;
            write(&out.join("metadata/timestamp.json"), &timestamp)?;
            check_served(out, &targets.signed.signed)?;
            return Ok(Refreshed::Timestamp(version));
        }
        let (snapshot, timestamp) = self.publish(store, out, signers, days)?;
        Ok(Refreshed::All { snapshot, timestamp })
    }

    /// Reads the catalog served at `url` with the client, from this
    /// repository's first root, as a new computer would, and fetches
    /// every target these targets list. Returns how many there are.
    ///
    /// # Errors
    ///
    /// If the client refuses what is served or cannot fetch a target.
    pub fn check_url(&self, url: &str) -> Result<usize, String> {
        let first =
            std::fs::read(self.metadata("1.root.json")).map_err(|e| format!("1.root.json: {e}"))?;
        let targets = self.targets()?.ok_or("no targets have been signed yet")?;
        let url = if url.ends_with('/') { url.to_string() } else { format!("{url}/") };
        let base = tuf::Url::parse(&url).map_err(|e| format!("{url}: {e}"))?;
        check_at(&base, &first, &targets.signed.signed)?;
        Ok(targets.signed.signed.targets.len())
    }
}

/// How many days before the served snapshot expires `refresh` signs a new
/// one, though targets have not changed.
pub const RENEW_DAYS: i64 = 2;

/// What [`Repo::refresh`] signed.
#[derive(Debug, PartialEq, Eq)]
pub enum Refreshed {
    /// A timestamp alone, of this version.
    Timestamp(u64),
    /// Everything again, as `publish`: these versions of snapshot and
    /// timestamp.
    All {
        /// The snapshot's version.
        snapshot: u64,
        /// The timestamp's version.
        timestamp: u64,
    },
}

/// What a published repository serves now, as `refresh` reads it.
struct Served {
    root: Vec<u8>,
    timestamp_version: u64,
    snapshot_version: u64,
    snapshot: Vec<u8>,
    targets_version: Option<u64>,
    snapshot_expires: Option<i64>,
}

impl Served {
    fn read(out: &Path) -> Option<Served> {
        let metadata = out.join("metadata");
        let timestamp: Value =
            serde_json::from_slice(&std::fs::read(metadata.join("timestamp.json")).ok()?).ok()?;
        let snapshot_version = timestamp["signed"]["meta"]["snapshot.json"]["version"].as_u64()?;
        let snapshot =
            std::fs::read(metadata.join(format!("{snapshot_version}.snapshot.json"))).ok()?;
        let parsed: Value = serde_json::from_slice(&snapshot).ok()?;
        Some(Served {
            root: std::fs::read(metadata.join("root.json")).ok()?,
            timestamp_version: timestamp["signed"]["version"].as_u64()?,
            snapshot_version,
            targets_version: parsed["signed"]["meta"]["targets.json"]["version"].as_u64(),
            snapshot_expires: parsed["signed"]["expires"].as_str().and_then(tuf::time::parse),
            snapshot,
        })
    }
}

/// What signs for `role`, one of the online roles: a key of `root`'s for
/// it that `signers` holds.
fn online_signer<'a>(
    root: &Loaded<Root>,
    keys: &'a [KeyFile],
    signers: &'a Signers,
    role: &str,
) -> Result<Box<dyn Signer + 'a>, String> {
    let ids = &root.signed.signed.roles[role].keyids;
    keys.iter()
        .filter(|k| ids.contains(&k.key.id()))
        .find_map(|k| signers.for_key(k))
        .ok_or(format!("nothing here can sign for {role}: give the online key's file"))
}

/// Reads `out` with the client, from its first root, as a new computer
/// would, and finds every target there.
fn check_served(out: &Path, targets: &Targets) -> Result<(), String> {
    let first = std::fs::read(out.join("metadata/1.root.json")).map_err(|e| e.to_string())?;
    let absolute = std::fs::canonicalize(out).map_err(|e| format!("{}: {e}", out.display()))?;
    let base =
        tuf::Url::parse(&format!("file://{}/", absolute.display())).map_err(|e| e.to_string())?;
    check_at(&base, &first, targets)
}

/// Reads the repository at `base` with the client, from the root `first`,
/// in a folder of its own, and fetches every target `targets` lists.
fn check_at(base: &tuf::Url, first: &[u8], targets: &Targets) -> Result<(), String> {
    let client = std::env::temp_dir().join(format!("catalog-repo-check-{}", std::process::id()));
    let client = client.as_path();
    let _ = std::fs::remove_dir_all(client);
    tuf::init(client, first).map_err(|e| e.to_string())?;
    let metadata = base.resolve("metadata/").map_err(|e| e.to_string())?;
    let served = base.resolve("targets/").map_err(|e| e.to_string())?;
    let result = (|| {
        let mut updater =
            tuf::Updater::new(client, &metadata, Some(&served)).map_err(|e| e.to_string())?;
        updater.refresh().map_err(|e| format!("the client refuses {base}: {e}"))?;
        for path in targets.targets.keys() {
            let target = updater
                .target(path)
                .map_err(|e| e.to_string())?
                .ok_or(format!("the client cannot find {path}"))?;
            let file = client.join("target");
            updater.download_target(path, &target, &file).map_err(|e| format!("{path}: {e}"))?;
        }
        Ok(())
    })();
    let _ = std::fs::remove_dir_all(client);
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    fn scratch(what: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("catalog-repo-{}-{what}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// A stand-in for tools/yubikey-piv.py, YubiKey 12345678 with a P-256
    /// key that openssl holds, which signs as the YubiKey does: ECDSA with
    /// SHA-256, in DER.
    fn fake_yubikey(dir: &Path) -> PathBuf {
        let openssl = |args: &[&str]| {
            let status = Command::new("openssl").args(args).current_dir(dir).status();
            assert!(
                status.is_ok_and(|s| s.success()),
                "openssl {args:?} failed: the catalog's tests need openssl"
            );
        };
        openssl(&["ecparam", "-name", "prime256v1", "-genkey", "-noout", "-out", "secret.pem"]);
        openssl(&["ec", "-in", "secret.pem", "-pubout", "-out", "public.pem"]);
        let helper = dir.join("yubikey");
        let script = format!(
            "#!/bin/sh\ncd '{}' || exit 1\ncase \"$1 $2\" in\n\
             'list ') echo 12345678 ;;\n\
             'public 12345678') cat public.pem ;;\n\
             'sign 12345678') openssl dgst -sha256 -sign secret.pem | od -An -tx1 | tr -d ' \\n'; echo ;;\n\
             *) exit 1 ;;\nesac\n",
            dir.display()
        );
        std::fs::write(&helper, script).unwrap();
        Command::new("chmod").arg("755").arg(&helper).status().unwrap();
        helper
    }

    fn bundle(dir: &Path, id: &str, version: &str, content: &[u8]) -> PathBuf {
        let manifest = format!(r#"{{"id": "{id}", "version": "{version}", "name": "Test"}}"#);
        let path = dir.join(format!("{id}-{version}-{}.dkv", content.len()));
        let mut zip = dkv::zip::Writer::new(std::fs::File::create(&path).unwrap());
        for (name, data) in [(dkv::MANIFEST, manifest.as_bytes()), ("bin/tool", content)] {
            zip.add(&dkv::zip::Input { path: name, executable: false, data }).unwrap();
        }
        zip.finish().unwrap();
        path
    }

    #[test]
    fn a_catalog_is_signed_and_published_as_the_client_reads_it() {
        let dir = scratch("catalog");
        let repo = Repo::new(&dir.join("catalog"));
        let helper = fake_yubikey(&dir);
        let yubikey = keys::from_yubikey(&helper, None).unwrap();
        assert_eq!(
            (yubikey.name.as_str(), yubikey.yubikey),
            ("yubikey-12345678", Some(12_345_678))
        );
        yubikey.write(&repo.keys_dir()).unwrap();
        assert!(yubikey.write(&repo.keys_dir()).is_err(), "a key is not written over");
        let roles = vec!["snapshot".to_string(), "timestamp".to_string()];
        let online_file = dir.join("online.key");
        keys::new_file_key("online", roles.clone(), &online_file)
            .unwrap()
            .write(&repo.keys_dir())
            .unwrap();
        assert!(
            keys::new_file_key("again", roles, &online_file).is_err(),
            "a secret is not written over"
        );

        assert!(
            repo.sign_root(&Signers::default(), 365).unwrap_err().contains("plug in a YubiKey")
        );
        let both = Signers::new(Some(&helper), std::slice::from_ref(&online_file)).unwrap();
        assert_eq!(repo.sign_root(&both, 365), Ok(1));

        let ripgrep = bundle(&dir, "org.dukovo.ripgrep", "14.1.1", b"rg");
        let bat = bundle(&dir, "org.dukovo.bat", "0.25.0", b"bat");
        let changes = repo.changes(&[ripgrep.clone(), bat.clone()], &[]).unwrap();
        assert_eq!(changes.lines().len(), 2);
        assert!(
            changes.lines()[1].starts_with(
                "+ bundles/org.dukovo.ripgrep-14.1.1.dkv (org.dukovo.ripgrep 14.1.1, "
            )
        );
        let store = dir.join("store");
        let online_only = Signers::new(None, std::slice::from_ref(&online_file)).unwrap();
        assert!(
            repo.sign_targets(&changes, &[ripgrep.clone(), bat.clone()], &store, &online_only, 182)
                .is_err()
        );
        assert_eq!(
            repo.sign_targets(&changes, &[ripgrep.clone(), bat.clone()], &store, &both, 182),
            Ok(1)
        );
        assert!(repo.changes(std::slice::from_ref(&ripgrep), &[]).unwrap().is_empty());
        let other = bundle(&dir, "org.dukovo.ripgrep", "14.1.1", b"something else");
        assert!(repo.changes(&[other], &[]).unwrap_err().contains("new version"));

        let out = dir.join("site");
        assert_eq!(repo.publish(&store, &out, &online_only, (7, 1)), Ok((1, 1)));
        assert_eq!(repo.publish(&store, &out, &online_only, (7, 1)), Ok((2, 2)));
        assert!(out.join("metadata/1.targets.json").is_file());
        assert!(out.join("metadata/2.snapshot.json").is_file());

        // The server's refresh: a timestamp alone while the snapshot is
        // good for long enough, and everything again once it is not.
        assert_eq!(repo.refresh(&store, &out, &online_only, (7, 1)), Ok(Refreshed::Timestamp(3)));
        assert_eq!(repo.publish(&store, &out, &online_only, (1, 1)), Ok((3, 4)));
        assert_eq!(
            repo.refresh(&store, &out, &online_only, (7, 1)),
            Ok(Refreshed::All { snapshot: 4, timestamp: 5 })
        );

        // A second root, signed by the same YubiKey, and a batch that
        // takes a bundle out, still read back from the first root.
        assert_eq!(repo.sign_root(&both, 365), Ok(2));
        let gone = vec!["bundles/org.dukovo.bat-0.25.0.dkv".to_string()];
        let changes = repo.changes(&[], &gone).unwrap();
        assert_eq!(changes.lines(), vec!["- bundles/org.dukovo.bat-0.25.0.dkv"]);
        assert_eq!(repo.sign_targets(&changes, &[], &store, &both, 182), Ok(2));
        // A new root or targets: refresh signs everything again.
        assert_eq!(
            repo.refresh(&store, &out, &online_only, (7, 1)),
            Ok(Refreshed::All { snapshot: 5, timestamp: 6 })
        );
        assert!(out.join("metadata/2.root.json").is_file());
        let targets = repo.targets().unwrap().unwrap();
        assert_eq!(targets.signed.signed.targets.len(), 1);
        let served = std::fs::canonicalize(&out).unwrap();
        assert_eq!(repo.check_url(&format!("file://{}", served.display())), Ok(1));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
