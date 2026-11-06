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
/// and `root.json`, the root they make, which a system trusts.
pub struct Keys {
    root: Vec<u8>,
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
        Ok(Keys { root, signers })
    }

    /// The keys in `dir`, checked against the root there.
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
        let mut signers = Vec::new();
        for role in ROLES {
            let key = Ed25519::from_pkcs8(&read(&format!("{role}.key"))?)?;
            let named =
                parsed.signed.roles.get(role).is_some_and(|r| r.keyids.contains(&key.key().id()));
            if !named {
                return Err(format!(
                    "{}/{role}.key is not the key root.json names for {role}",
                    dir.display()
                ));
            }
            signers.push((role, key));
        }
        Ok(Keys { root, signers })
    }

    /// The root, as a system that trusts these keys keeps it.
    #[must_use]
    pub fn root(&self) -> &[u8] {
        &self.root
    }

    fn signer(&self, role: &str) -> &dyn Signer {
        &self.signers.iter().find(|(r, _)| *r == role).expect("every role has a key").1
    }
}

/// The expiry of `role`'s metadata for a release built at `built`.
fn expires(built: i64, role: &str) -> String {
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
        &[keys.signer("targets")],
    )?;
    write(out.join("metadata/1.targets.json"), &targets)?;
    let meta = BTreeMap::from([("targets.json".to_string(), repo::meta(1, &targets))]);
    let snapshot = repo::sign(
        &repo::snapshot(1, &expires(built, "snapshot"), &meta),
        &[keys.signer("snapshot")],
    )?;
    write(out.join("metadata/1.snapshot.json"), &snapshot)?;
    let timestamp = repo::timestamp(1, &expires(built, "timestamp"), 1, &snapshot);
    write(
        out.join("metadata/timestamp.json"),
        &repo::sign(&timestamp, &[keys.signer("timestamp")])?,
    )?;
    write(out.join("metadata/1.root.json"), keys.root())?;
    write(out.join("metadata/root.json"), keys.root())?;

    let published = open(out, keys.root(), Some(&release.version))?;
    if published.release != *release {
        return Err("the release list read back is not the one written".into());
    }
    check_files(out, &published.release).map_err(|problems| problems.join("\n"))?;
    Ok(published)
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

/// A new folder for the client's metadata while it reads a repository.
fn scratch_dir() -> PathBuf {
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
