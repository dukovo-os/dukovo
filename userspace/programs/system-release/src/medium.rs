// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The release an install medium carries, as the update server serves it
//! (ADR 0779): the project's install stick and ISO hold the release the
//! release key signed and the server published, so that a system
//! installed from one trusts the release root and takes its updates from
//! updates.dukovo.org.
//!
//! [`write()`] reads the update server with the TUF client from the first
//! root, as a computer online reads it, expiry checked, and takes the
//! release list it offers for the version, which must be the list built
//! here, byte for byte. It lays the release's repository out as Rescue's
//! is laid out from what a client kept
//! ([`rescue::repository_from_client`]): the roots walked to, the
//! newest timestamp, snapshot and targets as served, and the list. Then
//! come the signatures troubleshoot(8) matches, if the server offers
//! them, the version in [`repo::VERSION_FILE`], as the server's targets
//! list every release it offers, and the files, from the tree built
//! here, each checked against the list. It is all read back offline from
//! the first root, as the installer reads it. Nothing is signed here.

use std::path::Path;

use crate::list::{Release, hex};
use crate::repo::{self, SIGNATURES_TARGET};
use crate::rescue;
use crate::store::Store;

/// What [`write()`] laid out.
#[derive(Debug, PartialEq, Eq)]
pub struct Medium {
    /// The release's version.
    pub version: String,
    /// The SHA-256 of its list, which names it.
    pub name: String,
    /// The version of the newest root the server serves, which the
    /// medium holds, as it holds the next three.
    pub root: u64,
    /// The version of the timestamp served.
    pub timestamp: u64,
    /// The version of the snapshot served.
    pub snapshot: u64,
    /// The version of the targets served.
    pub targets: u64,
    /// How many files the release has.
    pub files: usize,
    /// Their size in bytes.
    pub bytes: u64,
}

/// Lays out in `out`, a new folder, the release whose list is `list` as
/// the update server at `base` serves it, read from the first root
/// `first`, with its files from the tree `tree`.
///
/// # Errors
///
/// If `out` is there already, the server cannot be read or serves what
/// does not check, it does not offer the release, the release it offers
/// of that version is not `list`, or a file of the tree is not what the
/// list says. Nothing is left in `out` then.
pub fn write(
    base: &tuf::Url,
    first: &[u8],
    list: &[u8],
    tree: &Path,
    out: &Path,
) -> Result<Medium, String> {
    if out.exists() {
        return Err(format!(
            "{} is there already: a medium's release is laid out in a new folder",
            out.display()
        ));
    }
    let release = Release::parse(list)?;
    if !repo::serves(base)? {
        return Err(format!(
            "{base} serves no releases yet: Dukovo {} is signed and deployed first",
            release.version
        ));
    }
    let work = repo::scratch_dir();
    let laid = lay_out(base, first, list, &release, tree, out, &work);
    let _ = std::fs::remove_dir_all(&work);
    let medium = laid.and_then(|medium| {
        read_back(out, first, list)?;
        Ok(medium)
    });
    if medium.is_err() {
        let _ = std::fs::remove_dir_all(out);
    }
    medium
}

/// The heart of [`write()`], with the TUF client's folders in `work`.
fn lay_out(
    base: &tuf::Url,
    first: &[u8],
    list: &[u8],
    release: &Release,
    tree: &Path,
    out: &Path,
    work: &Path,
) -> Result<Medium, String> {
    let (client, roots) = (work.join("client"), work.join("roots"));
    let url = |sub: &str| base.resolve(sub).map_err(|err| format!("{base}{sub}: {err}"));
    tuf::init(&client, first).map_err(|err| err.to_string())?;
    let mut updater = tuf::Updater::new(&client, &url("metadata/")?, Some(&url("targets/")?))
        .map_err(|err| format!("the first root: {err}"))?
        .keeping_roots(&roots);
    updater.refresh().map_err(|err| format!("{base}: {err}"))?;
    let version = &release.version;
    let path = repo::target_path(version);
    let target = updater
        .target(&path)
        .map_err(|err| format!("{base}: {err}"))?
        .ok_or_else(|| format!("{base} does not offer Dukovo {version}: it is deployed first"))?;
    let served = work.join("list.json");
    updater.download_target(&path, &target, &served).map_err(|err| format!("{base}: {err}"))?;
    if read(&served)? != list {
        return Err(format!(
            "the Dukovo {version} that {base} serves is not the one built here: their release \
             lists differ"
        ));
    }
    rescue::repository_from_client(&client, &roots, list, out)?;
    if let Some(target) =
        updater.target(SIGNATURES_TARGET).map_err(|err| format!("{base}: {err}"))?
    {
        let file = work.join("signatures.toml");
        updater
            .download_target(SIGNATURES_TARGET, &target, &file)
            .map_err(|err| format!("{base}: {err}"))?;
        let bytes = read(&file)?;
        put(&repo::target_file(out, SIGNATURES_TARGET, &bytes), &bytes)?;
    }
    put(&out.join(repo::VERSION_FILE), format!("{version}\n").as_bytes())?;
    Store::at(&out.join("files")).add_tree(tree, release)?;
    let trusted = updater.trusted();
    let targets = trusted.targets("targets").ok_or("the targets did not load")?;
    Ok(Medium {
        version: version.clone(),
        name: hex(ring::digest::digest(&ring::digest::SHA256, list).as_ref()),
        root: trusted.root().version,
        timestamp: trusted.timestamp().map_or(0, |timestamp| timestamp.version),
        snapshot: trusted.snapshot().map_or(0, |snapshot| snapshot.version),
        targets: targets.version,
        files: release.files.iter().filter(|entry| entry.file().is_some()).count(),
        bytes: release.bytes(),
    })
}

/// Reads the repository at `out` as the installer does, offline from the
/// first root, and checks that it is the release of `list` and that
/// every file is there and what the list says.
fn read_back(out: &Path, first: &[u8], list: &[u8]) -> Result<(), String> {
    let published = repo::open_named(out, first, None)?;
    if published.name != hex(ring::digest::digest(&ring::digest::SHA256, list).as_ref()) {
        return Err(format!("{} reads back as another release", out.display()));
    }
    repo::check_files(out, &published.release).map(|_| ()).map_err(|problems| {
        format!(
            "{} of the release's files did not read back: {}",
            problems.len(),
            problems.first().map_or("", String::as_str)
        )
    })
}

fn read(path: &Path) -> Result<Vec<u8>, String> {
    std::fs::read(path).map_err(|err| format!("{}: {err}", path.display()))
}

/// Writes `bytes` as the new file `path`, in a folder made if need be.
fn put(path: &Path, bytes: &[u8]) -> Result<(), String> {
    if let Some(folder) = path.parent() {
        std::fs::create_dir_all(folder).map_err(|err| format!("{}: {err}", folder.display()))?;
    }
    std::fs::write(path, bytes).map_err(|err| format!("{}: {err}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::list::tests::{scratch, tree};
    use crate::updates::tests::{about, ceremony, yubikey_keys};
    use crate::updates::{NewRelease, offer, publish};
    use std::path::PathBuf;

    /// An update server in `dir/server`, of a key ceremony's first root.
    struct Server {
        dir: PathBuf,
        helper: PathBuf,
        first: Vec<u8>,
        secret: Vec<u8>,
        url: tuf::Url,
    }

    impl Server {
        fn new(dir: &Path) -> Server {
            let (helper, first, secret) = ceremony(dir);
            let url =
                tuf::Url::parse(&format!("file://{}/", dir.join("server").display())).unwrap();
            Server { dir: dir.to_path_buf(), helper, first, secret, url }
        }

        /// Signs the release of `slot` as Dukovo `version` on YubiKey
        /// 1001, from what the server serves, and deploys it, as `./ob
        /// release sign` and `deploy` do. Gives its list.
        fn sign_and_serve(&self, slot: &Path, version: &str) -> Vec<u8> {
            let list = Release::of_tree(slot, about(version, "0.14.1")).unwrap().to_bytes();
            let work = self.dir.join(format!("work-{version}"));
            repo::fetch(&self.url, &self.first, &work).unwrap();
            let keys = yubikey_keys(&work, &self.helper, 1001);
            let new = NewRelease { tree: slot, list: &list, channel: "beta", advisories: None };
            let now = tuf::time::now();
            offer(&work, &keys, &new, now, |_| Ok(())).unwrap();
            publish(&self.dir.join("server"), &work, &self.secret, None, now).unwrap();
            list
        }
    }

    #[test]
    fn a_medium_holds_the_release_as_the_server_serves_it_and_reads_back_offline() {
        let dir = scratch("medium");
        let server = Server::new(&dir);
        let (url, first) = (&server.url, &server.first);
        let slot = dir.join("slot");
        tree(&slot);
        let list = server.sign_and_serve(&slot, "0.18.0");

        let out = dir.join("medium-1");
        let medium = write(url, first, &list, &slot, &out).unwrap();
        assert_eq!((medium.version.as_str(), medium.files), ("0.18.0", 2));
        assert_eq!((medium.root, medium.targets), (1, 1));
        assert_eq!(std::fs::read_to_string(out.join(repo::VERSION_FILE)).unwrap(), "0.18.0\n");
        let published = repo::open_named(&out, first, None).unwrap();
        assert_eq!(published.name, medium.name);
        assert!(repo::check_files(&out, &published.release).is_ok());

        // Once the server offers another, its targets list both, and the
        // medium names its own, for the installer and for Rescue.
        std::fs::write(slot.join("commands/spark"), b"spark, newer").unwrap();
        let newer = server.sign_and_serve(&slot, "0.18.1");
        let out = dir.join("medium-2");
        let medium = write(url, first, &newer, &slot, &out).unwrap();
        assert_eq!((medium.version.as_str(), medium.targets), ("0.18.1", 2));
        assert!(repo::open(&out, first, None).unwrap_err().contains("2 releases"));
        let published = repo::open_named(&out, first, None).unwrap();
        assert_eq!(published.release.version, "0.18.1");
        let store = Store::at(&out.join("files"));
        let file = |entry: &crate::list::Entry| store.path(entry.file().map_or("", |(sha, _)| sha));
        let esp = dir.join("esp");
        rescue::write(&esp, &out, &published.release, &file).unwrap();
        let kept = rescue::open(&esp.join(rescue::DIR), first).unwrap();
        assert_eq!(kept.release.version, "0.18.1");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_medium_is_refused_for_a_release_the_server_does_not_serve_as_built_here() {
        let dir = scratch("medium-refused");
        let server = Server::new(&dir);
        let (url, first) = (&server.url, &server.first);
        let slot = dir.join("slot");
        tree(&slot);
        let out = dir.join("medium");
        let list = Release::of_tree(&slot, about("0.18.0", "0.14.1")).unwrap().to_bytes();
        let error = write(url, first, &list, &slot, &out).unwrap_err();
        assert!(error.contains("serves no releases yet"), "{error}");
        server.sign_and_serve(&slot, "0.18.0");
        let refused = |list: &[u8]| {
            let error = write(url, first, list, &slot, &out).unwrap_err();
            assert!(!out.exists(), "{error}");
            error
        };
        // A version not offered, and a list of the same version that is
        // not the one signed: built at another time.
        let other = Release::of_tree(&slot, about("0.18.2", "0.14.1")).unwrap().to_bytes();
        assert!(refused(&other).contains("does not offer Dukovo 0.18.2"));
        let mut later = about("0.18.0", "0.14.1");
        later.built += 1;
        let later = Release::of_tree(&slot, later).unwrap().to_bytes();
        assert!(refused(&later).contains("is not the one built here"));
        // A tree whose file is not the list's.
        std::fs::write(slot.join("commands/spark"), b"spark, changed").unwrap();
        assert!(refused(&list).contains("changed since it was listed"));
        std::fs::write(slot.join("commands/spark"), b"spark's bytes").unwrap();
        write(url, first, &list, &slot, &out).unwrap();
        assert!(write(url, first, &list, &slot, &out).unwrap_err().contains("is there already"));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
