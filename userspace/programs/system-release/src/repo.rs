// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! A release's repository: its list as a TUF target, signed through the
//! four top-level roles, and its files in a store beside it (ADR 0308).
//! It is laid out as the update server will serve it, so that an install
//! stick and the server are read by the same code:
//!
//! ```text
//! metadata/1.root.json, root.json      the root, by version and newest
//! metadata/1.targets.json              lists system/<version>.json
//! metadata/1.snapshot.json, timestamp.json
//! targets/system/<sha256>.<version>.json   the release list
//! files/<sha256>                       every file of the release, once
//! ```
//!
//! [`publish`] makes one and reads it back before it is done; [`open`]
//! reads one as a computer does, from the root it already trusts, and
//! checked offline: every signature, version and hash, but not expiry,
//! which says a server still vouches for the metadata and cannot be known
//! from a stick ([`tuf::Updater::offline`]).

use std::collections::BTreeMap;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};

use tuf::metadata::{Root, Signed};
use tuf::repo::{self, Ed25519, Signer};

use crate::list::{Release, hex};
use crate::store::{Checked, Store};

/// The four top-level roles, each with a key of its own.
pub const ROLES: [&str; 4] = ["root", "targets", "snapshot", "timestamp"];

/// How long each role's metadata is good for, in days from the release's
/// build: a development root ten years, since it signs only what one
/// machine builds and nothing online trusts it; the others as the
/// catalog's are (ADR 0381). An install stick is checked offline, so
/// these matter once the server serves a release.
pub const EXPIRES: [(&str, i64); 4] =
    [("root", 3650), ("targets", 182), ("snapshot", 7), ("timestamp", 1)];

/// Where a release's list is among the targets, for its version.
#[must_use]
pub fn target_path(version: &str) -> String {
    format!("system/{version}.json")
}

/// The keys that sign a repository, kept in a directory: each role's
/// Ed25519 secret as `<role>.key` (PKCS #8, readable by its owner alone),
/// and `root.json`, the newest root they make, which a system trusts.
/// Once a key has been replaced ([`Keys::rotate`]) every root is kept
/// too, by version, as `N.root.json`, for a client to walk from the root
/// it has to the newest (TUF 5.3).
pub struct Keys {
    /// Every root, oldest first; the last is `root.json`.
    roots: Vec<Vec<u8>>,
    signers: Vec<(&'static str, Ed25519)>,
}

impl Keys {
    /// Makes new keys in `dir`, and the first root, which names them.
    ///
    /// # Errors
    ///
    /// If `dir` has keys already, or they cannot be written.
    pub fn create(dir: &Path, built: i64) -> Result<Keys, String> {
        if dir.join("root.json").exists() {
            return Err(format!("{} has keys already; remove it to make new ones", dir.display()));
        }
        std::fs::create_dir_all(dir).map_err(|err| format!("{}: {err}", dir.display()))?;
        let mut signers = Vec::new();
        for role in ROLES {
            let (key, secret) = Ed25519::generate()?;
            let path = dir.join(format!("{role}.key"));
            std::fs::write(&path, &secret)
                .and_then(|()| {
                    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))
                })
                .map_err(|err| format!("{}: {err}", path.display()))?;
            signers.push((role, key));
        }
        let roles: Vec<_> = signers.iter().map(|(role, key)| (*role, vec![key.key()], 1)).collect();
        let signed = repo::root(1, &expires(built, "root"), &roles, true);
        let root_signer: &dyn Signer = &signers[0].1;
        let root = repo::sign(&signed, &[root_signer])?;
        std::fs::write(dir.join("root.json"), &root)
            .map_err(|err| format!("{}: {err}", dir.display()))?;
        Ok(Keys { roots: vec![root], signers })
    }

    /// The keys in `dir`, checked against the root there. A role whose
    /// key is not there is signed for elsewhere.
    ///
    /// # Errors
    ///
    /// If a file is missing or unreadable, or a key is not the one the
    /// root names for its role.
    pub fn load(dir: &Path) -> Result<Keys, String> {
        let read = |name: &str| {
            let path = dir.join(name);
            std::fs::read(&path).map_err(|err| format!("{}: {err}", path.display()))
        };
        let root = read("root.json")?;
        let parsed = Signed::<Root>::parse(&root)
            .map_err(|err| format!("{}/root.json: {err}", dir.display()))?;
        let mut roots = Vec::new();
        for version in 1..parsed.version {
            roots.push(read(&format!("{version}.root.json"))?);
        }
        roots.push(root);
        let mut signers = Vec::new();
        for role in ROLES {
            let name = format!("{role}.key");
            if !dir.join(&name).exists() {
                continue;
            }
            let key = Ed25519::from_pkcs8(&read(&name)?)?;
            if !names(&parsed.signed, role, key.key()) {
                return Err(format!(
                    "{}/{role}.key is not the key root.json names for {role}",
                    dir.display()
                ));
            }
            signers.push((role, key));
        }
        Ok(Keys { roots, signers })
    }

    /// The update server's keys: its online key, whose secret half is
    /// `secret` (PKCS #8), for snapshot and timestamp, which the newest of
    /// `roots`, the repository's, must name it for.
    ///
    /// # Errors
    ///
    /// If `secret` is not an Ed25519 key, there is no root, or the newest
    /// does not name the key for both roles.
    pub fn online(roots: Vec<Vec<u8>>, secret: &[u8]) -> Result<Keys, String> {
        let newest = roots.last().ok_or("the repository has no root")?;
        let parsed = Signed::<Root>::parse(newest).map_err(|err| format!("the root: {err}"))?;
        let mut signers = Vec::new();
        for role in ONLINE {
            let key = Ed25519::from_pkcs8(secret)?;
            if !names(&parsed.signed, role, key.key()) {
                return Err(format!(
                    "root version {} does not name this server's online key {} for {role}",
                    parsed.version,
                    key.key().id()
                ));
            }
            signers.push((role, key));
        }
        Ok(Keys { roots, signers })
    }

    /// Replaces `role`'s key in `dir` with a new one, and signs the next
    /// root, which names it, with the root key before it and, for root
    /// itself, the new one too, as TUF asks of a client that walks to it
    /// (TUF 5.3.4). A key the root names for the role that is not here,
    /// the update server's, stays named. Every root stays in `dir` by its
    /// version.
    ///
    /// # Errors
    ///
    /// If `role` is not one of [`ROLES`], the keys do not load, the root
    /// key is not among them, or a file cannot be written.
    pub fn rotate(dir: &Path, role: &str, now: i64) -> Result<Keys, String> {
        let role = ROLES
            .into_iter()
            .find(|r| *r == role)
            .ok_or_else(|| format!("no role called {role}; the roles are {}", ROLES.join(", ")))?;
        let (key, secret) = Ed25519::generate()?;
        let keys = Keys::load(dir)?;
        let old = keys.signers.iter().find(|(r, _)| *r == role).map(|(_, k)| k.key().id());
        let mut named = keys.named(role)?;
        named.retain(|k| Some(k.id()) != old);
        named.push(key.key().clone());
        next_root(dir, keys, &[(role, named)], Some((role, key, secret)), now)
    }

    /// Names the update server's online key `key` for snapshot and
    /// timestamp in the next root, beside this computer's own keys for
    /// them, which still sign install sticks and update files; a server
    /// key named before is named no longer (ADR 0467).
    ///
    /// # Errors
    ///
    /// If the keys do not load, the root key is not among them, or a file
    /// cannot be written.
    pub fn set_online(dir: &Path, key: &tuf::keys::Key, now: i64) -> Result<Keys, String> {
        let keys = Keys::load(dir)?;
        let mut changes = Vec::new();
        for role in ONLINE {
            let mut named: Vec<tuf::keys::Key> = keys
                .signers
                .iter()
                .filter(|(r, _)| *r == role)
                .map(|(_, k)| k.key().clone())
                .collect();
            named.push(key.clone());
            changes.push((role, named));
        }
        next_root(dir, keys, &changes, None, now)
    }

    /// The keys the newest root names for `role`.
    fn named(&self, role: &str) -> Result<Vec<tuf::keys::Key>, String> {
        let parsed = Signed::<Root>::parse(self.root()).map_err(|err| err.to_string())?;
        let ids = parsed.signed.roles.get(role).map_or_default(|r| r.keyids.clone());
        Ok(ids.iter().filter_map(|id| parsed.signed.keys.get(id).cloned()).collect())
    }

    /// The newest root, as a system built with these keys trusts it.
    ///
    /// # Panics
    ///
    /// Never: keys are made, and load, with their first root.
    #[must_use]
    pub fn root(&self) -> &[u8] {
        self.roots.last().expect("a root at least")
    }

    /// Every root these keys have made, oldest first.
    #[must_use]
    pub fn roots(&self) -> &[Vec<u8>] {
        &self.roots
    }

    /// Whether these keys sign for `role`, or another computer does.
    #[must_use]
    pub fn has(&self, role: &str) -> bool {
        self.signers.iter().any(|(r, _)| *r == role)
    }

    /// `role`'s key, to sign with.
    pub(crate) fn signer(&self, role: &str) -> Result<&dyn Signer, String> {
        let found = self.signers.iter().find(|(r, _)| *r == role).map(|(_, key)| key);
        found.map(|key| key as &dyn Signer).ok_or_else(|| {
            format!("these keys do not sign for {role}: the update server's online key does")
        })
    }
}

/// The roles the update server's online key signs for.
pub const ONLINE: [&str; 2] = ["snapshot", "timestamp"];

/// Whether `root` names `key` for `role`.
fn names(root: &Root, role: &str, key: &tuf::keys::Key) -> bool {
    root.roles.get(role).is_some_and(|r| r.keyids.contains(&key.id()))
}

/// Signs the next root of `keys`, those in `dir`, with each role of
/// `changes` naming the keys given, and the others as the newest root
/// names them, with the root key before it and, for a new root key, that
/// one too. Writes every root by version, then a new key `new` (the role,
/// its key and its secret half), then `root.json` last, so that keys that
/// stop half way still load as the root before.
fn next_root(
    dir: &Path,
    keys: Keys,
    changes: &[(&'static str, Vec<tuf::keys::Key>)],
    new: Option<(&'static str, Ed25519, Vec<u8>)>,
    now: i64,
) -> Result<Keys, String> {
    let mut role_keys = Vec::new();
    for role in ROLES {
        match changes.iter().find(|(r, _)| *r == role) {
            Some((_, named)) => role_keys.push((role, named.clone())),
            None => role_keys.push((role, keys.named(role)?)),
        }
    }
    let Keys { mut roots, mut signers } = keys;
    let newest = roots.last().ok_or_else(|| format!("{}: no root.json", dir.display()))?;
    let version = Signed::<Root>::parse(newest).map_err(|err| err.to_string())?.version + 1;
    let old_root = signers
        .iter()
        .position(|(r, _)| *r == "root")
        .ok_or_else(|| format!("{} holds no root key, which signs a new root", dir.display()))?;
    let roles: Vec<(&str, Vec<&tuf::keys::Key>, u64)> =
        role_keys.iter().map(|(role, keys)| (*role, keys.iter().collect(), 1)).collect();
    let signed = repo::root(version, &expires(now, "root"), &roles, true);
    let old: &dyn Signer = &signers[old_root].1;
    let root = match &new {
        Some(("root", key, _)) => repo::sign(&signed, &[old, key])?,
        _ => repo::sign(&signed, &[old])?,
    };
    let write = |name: &str, bytes: &[u8], mode: u32| {
        let path = dir.join(name);
        std::fs::write(&path, bytes)
            .and_then(|()| std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode)))
            .map_err(|err| format!("{}: {err}", path.display()))
    };
    for (n, earlier) in roots.iter().enumerate() {
        write(&format!("{}.root.json", n + 1), earlier, 0o644)?;
    }
    write(&format!("{version}.root.json"), &root, 0o644)?;
    if let Some((role, key, secret)) = new {
        let (partial, path) =
            (dir.join(format!("{role}.key.new")), dir.join(format!("{role}.key")));
        write(&format!("{role}.key.new"), &secret, 0o600)?;
        std::fs::rename(&partial, &path).map_err(|err| format!("{}: {err}", path.display()))?;
        signers.retain(|(r, _)| *r != role);
        signers.push((role, key));
    }
    write("root.json", &root, 0o644)?;
    roots.push(root);
    Ok(Keys { roots, signers })
}

/// The expiry of `role`'s metadata signed at `built`, or for a release
/// built then.
pub(crate) fn expires(built: i64, role: &str) -> String {
    let days = EXPIRES.iter().find(|(r, _)| *r == role).map_or(1, |(_, days)| *days);
    tuf::time::format(built + days * 86_400)
}

/// What [`publish`] and [`open`] say of a release.
#[derive(Debug)]
pub struct Published {
    /// The release's list.
    pub release: Release,
    /// The SHA-256 of its list, which names the release.
    pub name: String,
}

/// Lays out the repository of `release`, whose files are in the tree at
/// `tree`, in `out`, which is made afresh, signed by `keys`; then reads it
/// back from the root as a computer would and checks every file.
///
/// # Errors
///
/// If a file cannot be read or written, a signature does not check, or
/// what was made does not read back whole.
pub fn publish(
    keys: &Keys,
    tree: &Path,
    release: &Release,
    out: &Path,
) -> Result<Published, String> {
    if out.exists() {
        std::fs::remove_dir_all(out).map_err(|err| format!("{}: {err}", out.display()))?;
    }
    let built = tuf::time::parse(&release.built).ok_or("the release has no build time")?;
    let write = |path: PathBuf, bytes: &[u8]| -> Result<(), String> {
        let folder = path.parent().unwrap_or(out);
        std::fs::create_dir_all(folder)
            .and_then(|()| std::fs::write(&path, bytes))
            .map_err(|err| format!("{}: {err}", path.display()))
    };
    Store::at(&out.join("files")).add_tree(tree, release)?;

    let list = release.to_bytes();
    let name = hex(ring::digest::digest(&ring::digest::SHA256, &list).as_ref());
    write(out.join(format!("targets/system/{name}.{}.json", release.version)), &list)?;
    let listed = BTreeMap::from([(target_path(&release.version), repo::file_info(&list))]);
    let targets = repo::sign(
        &repo::targets(1, &expires(built, "targets"), &listed),
        &[keys.signer("targets")?],
    )?;
    write(out.join("metadata/1.targets.json"), &targets)?;
    let meta = BTreeMap::from([("targets.json".to_string(), repo::meta(1, &targets))]);
    let snapshot = repo::sign(
        &repo::snapshot(1, &expires(built, "snapshot"), &meta),
        &[keys.signer("snapshot")?],
    )?;
    write(out.join("metadata/1.snapshot.json"), &snapshot)?;
    let timestamp = repo::timestamp(1, &expires(built, "timestamp"), 1, &snapshot);
    write(
        out.join("metadata/timestamp.json"),
        &repo::sign(&timestamp, &[keys.signer("timestamp")?])?,
    )?;
    write_roots(&out.join("metadata"), keys)?;

    let published = open(out, &keys.roots()[0], Some(&release.version))?;
    if published.release != *release {
        return Err("the release list read back is not the one written".into());
    }
    check_files(out, &published.release).map_err(|problems| problems.join("\n"))?;
    Ok(published)
}

/// Writes every root of `keys` into the metadata folder `dir`, by version,
/// and the newest as `root.json`.
///
/// # Errors
///
/// If one cannot be written.
pub(crate) fn write_roots(dir: &Path, keys: &Keys) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|err| format!("{}: {err}", dir.display()))?;
    for (n, root) in keys.roots().iter().enumerate() {
        let path = dir.join(format!("{}.root.json", n + 1));
        std::fs::write(&path, root).map_err(|err| format!("{}: {err}", path.display()))?;
    }
    let path = dir.join("root.json");
    std::fs::write(&path, keys.root()).map_err(|err| format!("{}: {err}", path.display()))
}

/// Reads the release in the repository at `dir` with the TUF client,
/// starting from `root`, the root this computer trusts, offline: the
/// release of `version`, or the only one there.
///
/// # Errors
///
/// What fails, in words for the person installing: metadata that is not
/// signed by the trusted root's keys, a release list that is not what
/// the targets say, or none or several releases where one was wanted.
pub fn open(dir: &Path, root: &[u8], version: Option<&str>) -> Result<Published, String> {
    let work = scratch_dir();
    let result = open_in(dir, root, version, &work);
    let _ = std::fs::remove_dir_all(&work);
    result
}

fn open_in(
    dir: &Path,
    root: &[u8],
    version: Option<&str>,
    work: &Path,
) -> Result<Published, String> {
    let absolute = std::path::absolute(dir).map_err(|err| format!("{}: {err}", dir.display()))?;
    let url = |sub: &str| {
        tuf::Url::parse(&format!("file://{}/{sub}/", absolute.display()))
            .map_err(|err| format!("{}: {err}", dir.display()))
    };
    tuf::init(work, root).map_err(|err| err.to_string())?;
    let mut updater = tuf::Updater::offline(work, &url("metadata")?, Some(&url("targets")?))
        .map_err(|err| format!("the trusted root: {err}"))?;
    updater.refresh().map_err(|err| format!("{}: {err}", dir.display()))?;
    let path = match version {
        Some(version) => target_path(version),
        None => only_release(&updater, dir)?,
    };
    let target = updater
        .target(&path)
        .map_err(|err| format!("{}: {err}", dir.display()))?
        .ok_or_else(|| format!("{} holds no release {path}", dir.display()))?;
    let file = work.join("system.json");
    updater
        .download_target(&path, &target, &file)
        .map_err(|err| format!("{}: {err}", dir.display()))?;
    let list = std::fs::read(&file).map_err(|err| format!("{}: {err}", file.display()))?;
    let release = Release::parse(&list)?;
    if target_path(&release.version) != path {
        return Err(format!("{path} holds the list of release {}", release.version));
    }
    let name = hex(ring::digest::digest(&ring::digest::SHA256, &list).as_ref());
    Ok(Published { release, name })
}

/// Reads every target the repository at `dir` offers with the TUF
/// client, from `root`, as a computer online does: expiry checked, every
/// signature, version and hash. Gives each target's path and bytes.
///
/// # Errors
///
/// What fails, as the client says it.
pub fn read_targets(dir: &Path, root: &[u8]) -> Result<Vec<(String, Vec<u8>)>, String> {
    let absolute = std::path::absolute(dir).map_err(|err| format!("{}: {err}", dir.display()))?;
    let base = tuf::Url::parse(&format!("file://{}/", absolute.display()))
        .map_err(|err| format!("{}: {err}", dir.display()))?;
    read_served(&base, root)
}

/// Reads every target the repository at `base`, a `file`, `http` or
/// `https` address, offers with the TUF client, from `root`, as
/// [`read_targets`] does.
///
/// # Errors
///
/// What fails, as the client says it.
pub fn read_served(base: &tuf::Url, root: &[u8]) -> Result<Vec<(String, Vec<u8>)>, String> {
    let work = scratch_dir();
    let result = (|| {
        let url = |sub: &str| base.resolve(sub).map_err(|err| format!("{base}{sub}: {err}"));
        tuf::init(&work, root).map_err(|err| err.to_string())?;
        let mut updater = tuf::Updater::new(&work, &url("metadata/")?, Some(&url("targets/")?))
            .map_err(|err| format!("the first root: {err}"))?;
        updater.refresh().map_err(|err| format!("{base}: {err}"))?;
        let paths: Vec<String> = updater
            .trusted()
            .targets("targets")
            .ok_or("the targets did not load")?
            .signed
            .targets
            .keys()
            .cloned()
            .collect();
        let mut read = Vec::new();
        for path in paths {
            let target = updater
                .target(&path)
                .map_err(|err| format!("{base}: {err}"))?
                .ok_or_else(|| format!("{base}: {path} is not a target"))?;
            let file = work.join("target");
            updater
                .download_target(&path, &target, &file)
                .map_err(|err| format!("{base}: {err}"))?;
            let bytes = std::fs::read(&file).map_err(|err| format!("{}: {err}", file.display()))?;
            read.push((path, bytes));
        }
        Ok(read)
    })();
    let _ = std::fs::remove_dir_all(&work);
    result
}

/// The path of the one release the targets list.
fn only_release(updater: &tuf::Updater, dir: &Path) -> Result<String, String> {
    let targets = updater.trusted().targets("targets").ok_or("the targets did not load")?;
    let releases: Vec<&String> = targets
        .signed
        .targets
        .keys()
        .filter(|path| {
            path.starts_with("system/") && Path::new(path).extension().is_some_and(|e| e == "json")
        })
        .collect();
    match releases.as_slice() {
        [one] => Ok((*one).clone()),
        [] => Err(format!("{} holds no release", dir.display())),
        many => Err(format!(
            "{} holds {} releases; name the one to use with --version",
            dir.display(),
            many.len()
        )),
    }
}

/// Checks every file of `release` in the store of the repository at
/// `dir`.
///
/// # Errors
///
/// Each file that is missing or damaged, by its path in the release.
pub fn check_files(dir: &Path, release: &Release) -> Result<Checked, Vec<String>> {
    Store::at(&dir.join("files")).check(release)
}

/// Signs `signed` with `role`'s key of `keys`.
pub(crate) fn sign_role(
    signed: &serde_json::Value,
    keys: &Keys,
    role: &str,
) -> Result<Vec<u8>, String> {
    repo::sign(signed, &[keys.signer(role)?])
}

/// A new folder for the client's metadata while it reads a repository.
pub(crate) fn scratch_dir() -> PathBuf {
    use std::sync::atomic::{AtomicU32, Ordering};
    static NEXT: AtomicU32 = AtomicU32::new(0);
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!("system-release-{}-{n}", std::process::id()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::list::About;
    use crate::list::tests::{scratch, tree};

    fn release_of(dir: &Path, version: &str) -> Release {
        let about = About {
            version: version.into(),
            commit: "01d66b7".into(),
            toolchain: "nightly-2026-10-15".into(),
            // Long ago: an install stick's metadata expires, and is still
            // read offline.
            built: 1_000_000_000,
            updates_from: version.into(),
        };
        Release::of_tree(dir, about).unwrap()
    }

    #[test]
    fn a_published_release_reads_back_from_its_root() {
        let dir = scratch("publish");
        let slot = dir.join("slot");
        tree(&slot);
        let keys = Keys::create(&dir.join("keys"), 1_000_000_000).unwrap();
        let again = Keys::create(&dir.join("keys"), 0).err().unwrap();
        assert!(again.contains("has keys already"), "{again}");
        let keys_again = Keys::load(&dir.join("keys")).unwrap();
        assert_eq!(keys_again.root(), keys.root());

        let release = release_of(&slot, "0.14.0");
        let out = dir.join("release");
        let published = publish(&keys, &slot, &release, &out).unwrap();
        assert_eq!(published.release, release);
        let opened = open(&out, keys.root(), None).unwrap();
        assert_eq!(opened.name, published.name);
        assert!(out.join(format!("targets/system/{}.0.14.0.json", opened.name)).is_file());
        assert_eq!(check_files(&out, &opened.release).unwrap().files, 2);
        let missing = open(&out, keys.root(), Some("0.15.0")).unwrap_err();
        assert!(missing.contains("holds no release system/0.15.0.json"), "{missing}");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn another_root_or_a_changed_list_is_refused() {
        let dir = scratch("refuse");
        let slot = dir.join("slot");
        tree(&slot);
        let keys = Keys::create(&dir.join("keys"), 1_000_000_000).unwrap();
        let out = dir.join("release");
        let published = publish(&keys, &slot, &release_of(&slot, "0.14.0"), &out).unwrap();

        // A stick signed by someone else's keys.
        let other = Keys::create(&dir.join("other"), 1_000_000_000).unwrap();
        let refused = open(&out, other.root(), None).unwrap_err();
        assert!(refused.contains("timestamp") || refused.contains("sign"), "{refused}");

        // A release list changed after it was signed.
        let list = out.join(format!("targets/system/{}.0.14.0.json", published.name));
        let text = std::fs::read_to_string(&list).unwrap().replace("\"0755\"", "\"4755\"");
        std::fs::write(&list, text).unwrap();
        let refused = open(&out, keys.root(), None).unwrap_err();
        assert!(refused.contains("sha256 is "), "{refused}");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
