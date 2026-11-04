// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Bundles' own update feeds (ADR 0390): a bundle from no catalog names
//! in its manifest, `update`, a small TUF repository and the minisign key
//! it is rooted in, and bundled reads it with the client it reads the
//! catalogs with (ADR 0383).
//!
//! - The client starts from the repository's first root,
//!   `metadata/1.root.json`, which must name that key, an Ed25519 key,
//!   alone for root and be signed by it. From there TUF guards it as it
//!   guards a catalog: against an old feed served again, one held back
//!   past its time, and files other than those it signs.
//! - It keeps a client for each place and bundle, in [`STATE`]`/PLACE/ID`,
//!   with the key and address it started from; a manifest that names
//!   another starts it over, unless the root it trusts already names that
//!   key, which the publisher has rotated to.
//! - The feed may list other bundles of its publisher: bundled takes the
//!   newest version of the bundle's own ID and line.

use std::path::{Path, PathBuf};

use dkv::minisign::PublicKey;
use tuf::metadata::{Root, Signed};

use super::catalogs::{self, Open};
use super::registry::Place;

/// Where bundled keeps each feed's client.
pub const STATE: &str = "/system/state/bundled/feeds";

/// In a feed's client: the key and address it started from.
const KEY_FILE: &str = "key";
const URL_FILE: &str = "url";

/// The most a first root may take.
const ROOT_MAX: u64 = 512 * 1024;

/// Where the client of bundle `id`'s feed in `place` is kept.
#[must_use]
pub fn dir(state: &Path, place: Place, id: &str) -> PathBuf {
    state.join(place.to_string()).join(id)
}

/// A feed's address as its repository's: ending in `/`.
fn base(address: &str) -> String {
    if address.ends_with('/') { address.to_string() } else { format!("{address}/") }
}

/// Checks `data`, a feed's first root, against `key`: it must name that
/// key alone for root, one signature enough, and be signed by it.
///
/// # Errors
///
/// If it is not root metadata, or not rooted in `key`.
pub fn check_first_root(data: &[u8], key: &PublicKey) -> Result<(), String> {
    let root = Signed::<Root>::parse(data).map_err(|e| format!("its first root: {e}"))?;
    let id = catalog_repo::keys::minisign_key(key).id();
    let role = root.signed.roles.get("root").ok_or("its first root names no root keys")?;
    if role.keyids != [id] || role.threshold != 1 {
        return Err(format!(
            "its first root is not rooted in the key its manifest names, {}",
            key.encoded()
        ));
    }
    root.verified_by(&root.signed, "root").map_err(|e| format!("its first root: {e}"))
}

/// Reads anew the feed at `address`, rooted in `key`, for bundle `id` in
/// `place`, with the client kept under `state`, and gives it, its index
/// written.
///
/// # Errors
///
/// If the key is not a minisign key, the feed cannot be read, its first
/// root is not rooted in the key, or TUF refuses what it serves.
pub fn open(
    state: &Path,
    place: Place,
    id: &str,
    address: &str,
    key: &str,
) -> Result<Open, String> {
    let public = PublicKey::decode(key).map_err(|e| format!("its feed's key: {e}"))?;
    let url = base(address);
    let dir = dir(state, place, id);
    let read = |name: &str| std::fs::read_to_string(dir.join(name)).ok();
    let same_url = read(URL_FILE).as_deref() == Some(url.as_str());
    let same_key = read(KEY_FILE).as_deref() == Some(key);
    // A key the publisher has rotated to, which the root trusted now names.
    let rotated = || {
        let id = catalog_repo::keys::minisign_key(&public).id();
        catalogs::trusted_root(&dir)
            .and_then(|root| root.roles.get("root").map(|r| r.keyids.contains(&id)))
            .unwrap_or(false)
    };
    let start = !catalogs::started(&dir) || !same_url || !(same_key || rotated());
    let first = if start {
        let at = tuf::Url::parse(&format!("{url}metadata/1.root.json"))
            .map_err(|e| format!("its feed {address}: {e}"))?;
        let data = tuf::fetch::fetch(&at, ROOT_MAX).map_err(|e| format!("{at}: {e}"))?;
        check_first_root(&data, &public)?;
        Some(data)
    } else {
        None
    };
    let open = catalogs::open_dir(&dir, &url, first.as_deref())?;
    for (name, text) in [(URL_FILE, url.as_str()), (KEY_FILE, key)] {
        let path = dir.join(name);
        std::fs::write(&path, text).map_err(|e| format!("{}: {e}", path.display()))?;
    }
    Ok(open)
}

/// Removes what is on its way in for every feed: a download is kept only
/// while its install runs.
pub fn tidy(state: &Path) {
    for place in std::fs::read_dir(state).into_iter().flatten().flatten() {
        catalogs::tidy(&place.path());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use catalog_repo::keys::{KeyFile, Signers, from_minisign, minisign_key};
    use dkv::minisign::SecretKey;

    fn scratch(what: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("feeds-{}-{what}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// A feed's repository in `dir`, rooted in `key`, published to
    /// `dir/site`.
    fn feed(dir: &Path, key: &SecretKey) {
        let repo = catalog_repo::Repo::new(&dir.join("repository"));
        let roles = catalog_repo::ROLES.iter().map(|r| (*r).to_string()).collect();
        let key_file = KeyFile {
            name: "publisher".into(),
            roles,
            yubikey: None,
            key: minisign_key(&key.public()),
        };
        key_file.write(&repo.keys_dir()).unwrap();
        let mut signers = Signers::default();
        signers.add(from_minisign(key).unwrap());
        repo.sign_root(&signers, 365).unwrap();
        let changes = repo.changes(&[], &[]).unwrap();
        repo.sign_targets(&changes, &[], &dir.join("store"), &signers, 30).unwrap();
        repo.publish(&dir.join("store"), &dir.join("site"), &signers, (30, 30)).unwrap();
    }

    #[test]
    fn a_feed_is_read_from_the_key_its_manifest_names() {
        let dir = scratch("read");
        let key = SecretKey::generate(&[9u8; 40]).unwrap();
        let other = SecretKey::generate(&[10u8; 40]).unwrap();
        feed(&dir, &key);
        let first = std::fs::read(dir.join("site/metadata/1.root.json")).unwrap();
        check_first_root(&first, &key.public()).unwrap();
        let err = check_first_root(&first, &other.public()).unwrap_err();
        assert!(err.contains("not rooted in the key its manifest names"), "{err}");
        let address = format!("file://{}/site", dir.display());
        let state = dir.join("state");
        let public = key.public().encoded();
        let place = Place::User(1000);
        let id = "org.example.notes";
        let read = open(&state, place, id, &address, &public).unwrap();
        assert!(read.index.entries.is_empty());
        let client = super::dir(&state, place, id);
        assert_eq!(std::fs::read_to_string(client.join(KEY_FILE)).unwrap(), public);
        assert!(std::fs::read_to_string(client.join(URL_FILE)).unwrap().ends_with("/site/"));
        // Read again, from what it trusts; another key starts it over,
        // and is refused.
        open(&state, place, id, &address, &public).unwrap();
        let err = open(&state, place, id, &address, &other.public().encoded()).err().unwrap();
        assert!(err.contains("not rooted in the key"), "{err}");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
