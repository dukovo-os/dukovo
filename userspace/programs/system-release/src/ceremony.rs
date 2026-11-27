// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The system releases' hardware keys and the roots they sign (ADRs
//! 0457, 0538 and 0780). The source tree's `releases/` keeps their public
//! halves: `keys/yubikey-SERIAL.json` for each YubiKey's slot-95 key,
//! `keys/online.json` for the update server's online key, and every root
//! by version in `metadata/`, from `1.root.json`, which every repository
//! the server serves starts from.
//!
//! A root names the YubiKeys for all four roles, any one signing alone,
//! and the online key beside them for snapshot and timestamp: the server
//! refreshes those two every few hours, and an install stick or an update
//! file, made offline, has them signed by a YubiKey instead. Both
//! YubiKeys sign the first root, in the key ceremony, so that it proves
//! each one works.
//!
//! The next root ([`sign_next_root`]) names the keys `keys/` holds then:
//! a YubiKey lost or stolen is dropped by taking its file out, a new one
//! is added with `./ob release key yubikey`, and a new online key by its
//! file. The root before it signs it, through a YubiKey that root names
//! for root, and every YubiKey new to the root signs it too. When `root`
//! fails, `tools/yubikey-piv.py public SERIAL 95` shows whether the
//! YubiKey's slot holds the key its file names.

use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use tuf::keys::Key;
#[cfg(test)]
use tuf::metadata::Targets;
use tuf::metadata::{Root, Signed};
use tuf::repo::{self, Signer};

use crate::repo::{self as releases, Keys, ONLINE, ROLES};
use crate::updates;
use crate::yubikey::{self, RELEASE_SLOT, YubiKey};

/// How many YubiKeys hold the release keys: one kept at hand and one kept
/// apart (ADR 0457). A root after the first may name one alone, from the
/// day one is lost until its replacement is added.
pub const YUBIKEYS: usize = 2;

/// How long the root is good for, in days: a year, signed again at ten
/// months (ADR 0457).
pub const ROOT_DAYS: i64 = 365;

/// The name of the first root in the metadata folder.
pub const FIRST_ROOT: &str = "1.root.json";

/// One YubiKey's release key, as its file in `keys/` keeps it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReleaseKey {
    /// The YubiKey's serial number, printed on its label.
    pub serial: u64,
    /// The P-256 key in its slot 95.
    pub key: Key,
}

impl ReleaseKey {
    /// Its file's name in `keys/`.
    #[must_use]
    pub fn file_name(&self) -> String {
        format!("yubikey-{}.json", self.serial)
    }

    /// The name of the file beside it that holds the key alone, in PEM,
    /// which `openssl` checks the install images' checksums with (ADR
    /// 0779).
    #[must_use]
    pub fn pem_name(&self) -> String {
        format!("yubikey-{}.pem", self.serial)
    }

    fn to_json(&self) -> Value {
        json!({ "yubikey": self.serial, "slot": RELEASE_SLOT, "key": self.key })
    }

    fn from_json(value: &Value) -> Result<ReleaseKey, String> {
        let serial = value["yubikey"].as_u64().ok_or("no YubiKey serial number")?;
        if value["slot"].as_str() != Some(RELEASE_SLOT) {
            return Err(format!("not a key of slot {RELEASE_SLOT}, the release keys' slot"));
        }
        let key: Key = serde_json::from_value(value["key"].clone()).map_err(|e| e.to_string())?;
        if !key.is_p256() {
            return Err("not a YubiKey's P-256 key".into());
        }
        Ok(ReleaseKey { serial, key })
    }
}

/// Reads the release key of YubiKey `serial`, or of the only one plugged
/// in, through `helper`, and keeps its public half in `dir`: its file,
/// which it returns, and the key alone in PEM beside it.
///
/// # Errors
///
/// If the YubiKey cannot be found, its slot holds no P-256 key made on
/// it, or `dir` has its file already.
pub fn add_yubikey(dir: &Path, helper: &Path, serial: Option<u64>) -> Result<PathBuf, String> {
    let serial = yubikey::one(helper, serial)?;
    let key = ReleaseKey { serial, key: yubikey::public_key(helper, serial, RELEASE_SLOT)? };
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let path = dir.join(key.file_name());
    if path.exists() {
        return Err(format!("{} is there already", path.display()));
    }
    let mut text = serde_json::to_string_pretty(&key.to_json()).map_err(|e| e.to_string())?;
    text.push('\n');
    std::fs::write(&path, text).map_err(|e| format!("{}: {e}", path.display()))?;
    let pem = dir.join(key.pem_name());
    std::fs::write(&pem, &key.key.keyval.public).map_err(|e| format!("{}: {e}", pem.display()))?;
    Ok(path)
}

/// The YubiKeys' release keys in `dir`, by serial number, and the online
/// key, `dir/online.json`.
///
/// # Errors
///
/// If a file cannot be read or is not a key, or there is no online key.
pub fn read_keys(dir: &Path) -> Result<(Vec<ReleaseKey>, Key), String> {
    let read = |path: &Path| {
        let text = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
        serde_json::from_slice::<Value>(&text).map_err(|e| format!("{}: {e}", path.display()))
    };
    let online_path = dir.join("online.json");
    if !online_path.exists() {
        return Err(format!(
            "{} is not there: `./ob release server setup` writes it, before the root",
            online_path.display()
        ));
    }
    let online: Key = serde_json::from_value(read(&online_path)?)
        .map_err(|e| format!("{}: not a key: {e}", online_path.display()))?;
    let mut yubikeys = Vec::new();
    let entries = std::fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    for entry in entries {
        let path = entry.map_err(|e| format!("{}: {e}", dir.display()))?.path();
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or_default();
        if !name.starts_with("yubikey-") || path.extension().is_none_or(|e| e != "json") {
            continue;
        }
        let key =
            ReleaseKey::from_json(&read(&path)?).map_err(|e| format!("{}: {e}", path.display()))?;
        if key.file_name() != name {
            return Err(format!("{} holds YubiKey {}'s key", path.display(), key.serial));
        }
        yubikeys.push(key);
    }
    yubikeys.sort_by_key(|key| key.serial);
    Ok((yubikeys, online))
}

/// The first root, signed by `signers`: `yubikeys` for every role and
/// `online` beside them for snapshot and timestamp, each role's
/// threshold 1, good for [`ROOT_DAYS`] from `now`.
///
/// # Errors
///
/// If there are not [`YUBIKEYS`] distinct release keys, the online key
/// is one of them, or the root is not signed by each YubiKey.
pub fn first_root(
    yubikeys: &[ReleaseKey],
    online: &Key,
    signers: &[&dyn Signer],
    now: i64,
) -> Result<Vec<u8>, String> {
    if yubikeys.len() != YUBIKEYS {
        return Err(format!(
            "the release keys are {YUBIKEYS} YubiKeys' and {} {} in keys/: add each with \
             `./ob release key yubikey`",
            yubikeys.len(),
            if yubikeys.len() == 1 { "is" } else { "are" }
        ));
    }
    if yubikeys[0].key == yubikeys[1].key {
        return Err("both YubiKeys' files name the same key".into());
    }
    if yubikeys.iter().any(|y| &y.key == online) {
        return Err("the online key is one of the YubiKeys' keys".into());
    }
    let expires = tuf::time::format(now + ROOT_DAYS * 86_400);
    let signed = repo::root(1, &expires, &roles(yubikeys, online), true);
    let bytes = repo::sign(&signed, signers)?;
    let root = Signed::<Root>::parse(&bytes).map_err(|e| format!("the new root: {e}"))?;
    for yubikey in yubikeys {
        let signature = root.signatures.get(&yubikey.key.id());
        if !signature.is_some_and(|sig| yubikey.key.verifies(&root.bytes, sig)) {
            return Err(format!("YubiKey {} did not sign the root", yubikey.serial));
        }
    }
    root.verified_by(&root.signed, "root").map_err(|e| format!("the new root: {e}"))?;
    Ok(bytes)
}

/// Each role of a root and the keys it names: `yubikeys` for every role,
/// and `online` beside them for snapshot and timestamp, each threshold 1.
fn roles<'a>(
    yubikeys: &'a [ReleaseKey],
    online: &'a Key,
) -> Vec<(&'static str, Vec<&'a Key>, u64)> {
    ROLES
        .into_iter()
        .map(|role| {
            let mut keys: Vec<&Key> = yubikeys.iter().map(|y| &y.key).collect();
            if ONLINE.contains(&role) {
                keys.push(online);
            }
            (role, keys, 1)
        })
        .collect()
}

/// The next root after `previous`, signed by `signers`: `yubikeys` for
/// every role and `online` beside them for snapshot and timestamp, as the
/// first root names them, good for [`ROOT_DAYS`] from `now`.
///
/// # Errors
///
/// If there is no YubiKey or more than [`YUBIKEYS`], two name the same
/// key, the online key is one of them, a YubiKey that `previous` does not
/// name for root did not sign it, or it does not follow `previous`.
pub fn next_root(
    previous: &[u8],
    yubikeys: &[ReleaseKey],
    online: &Key,
    signers: &[&dyn Signer],
    now: i64,
) -> Result<Vec<u8>, String> {
    if yubikeys.is_empty() || yubikeys.len() > YUBIKEYS {
        return Err(format!(
            "a root names one or {YUBIKEYS} YubiKeys' release keys, and {} {} in keys/",
            yubikeys.len(),
            if yubikeys.len() == 1 { "is" } else { "are" }
        ));
    }
    if yubikeys.len() == YUBIKEYS && yubikeys[0].key == yubikeys[1].key {
        return Err("both YubiKeys' files name the same key".into());
    }
    if yubikeys.iter().any(|y| &y.key == online) {
        return Err("the online key is one of the YubiKeys' keys".into());
    }
    let old = Signed::<Root>::parse(previous).map_err(|e| format!("the root before: {e}"))?;
    let expires = tuf::time::format(now + ROOT_DAYS * 86_400);
    let signed = repo::root(old.version + 1, &expires, &roles(yubikeys, online), true);
    let bytes = repo::sign(&signed, signers)?;
    let root = Signed::<Root>::parse(&bytes).map_err(|e| format!("the new root: {e}"))?;
    for yubikey in yubikeys.iter().filter(|y| !names(&old.signed, "root", &y.key)) {
        let signature = root.signatures.get(&yubikey.key.id());
        if !signature.is_some_and(|sig| yubikey.key.verifies(&root.bytes, sig)) {
            return Err(format!(
                "YubiKey {} is new to the root and did not sign it",
                yubikey.serial
            ));
        }
    }
    releases::follows(previous, &bytes)?;
    Ok(bytes)
}

/// Whether `root` names `key` for `role`.
fn names(root: &Root, role: &str, key: &Key) -> bool {
    root.roles.get(role).is_some_and(|r| r.keyids.contains(&key.id()))
}

/// The next root, as [`sign_next_root`] will sign it, for the person who
/// signs it to see first.
#[derive(Debug, PartialEq, Eq)]
pub struct NextRoot {
    /// Its version.
    pub version: u64,
    /// When it expires, in seconds since 1970.
    pub expires: i64,
    /// The YubiKeys it names for every role, by serial number.
    pub yubikeys: Vec<u64>,
    /// Those of them the root before it does not name.
    pub new_yubikeys: Vec<u64>,
    /// The ID of the online key it names for snapshot and timestamp.
    pub online: String,
    /// Whether the root before it names another online key.
    pub new_online: bool,
    /// The IDs of the keys the root before it names that it does not:
    /// those of the YubiKeys dropped, and of the online key replaced.
    pub dropped: Vec<String>,
    /// The YubiKeys that sign it, plugged in.
    pub signers: Vec<u64>,
    /// The version the repository's targets are signed again as, under
    /// it, or `None` without a repository or when it offers nothing yet.
    pub targets: Option<u64>,
}

/// What [`sign_next_root`] signed.
#[derive(Debug)]
pub struct SignedRoot {
    /// The root, as written to the metadata folder.
    pub root: Vec<u8>,
    /// What was shown before it was signed.
    pub shown: NextRoot,
}

/// Signs the next root of the system's releases from the keys in `keys`,
/// after the newest root in `metadata`, the source tree's, on the
/// YubiKeys plugged in and found through `helper`, once `confirm` has
/// passed what is to be signed, and writes it to `metadata` by its
/// version. With `repo`, a repository fetched from the update server or
/// deployed to it before, the tree's roots it lacks are added to it, the
/// root goes into it too, and its targets are signed again under it as
/// they are, on one of those YubiKeys, so that a deploy of it serves the
/// new root (ADR 0780).
///
/// It is signed by every YubiKey plugged in whose key either root names
/// for root, at least one of them the root before's, and every YubiKey new
/// to the root must be among them. A YubiKey dropped from the root, if it
/// is still at hand, signs as a key of the root before.
///
/// # Errors
///
/// If the tree's roots do not chain, `repo` holds a root the tree does
/// not, the keys do not make a root, a YubiKey needed is not plugged in or
/// does not hold the key its file names, `confirm` refuses, or signing
/// fails. Nothing is written to `metadata` then.
pub fn sign_next_root(
    keys: &Path,
    metadata: &Path,
    helper: &Path,
    repo: Option<&Path>,
    now: i64,
    confirm: impl FnOnce(&NextRoot) -> Result<(), String>,
) -> Result<SignedRoot, String> {
    if !metadata.join(FIRST_ROOT).exists() {
        return Err(format!(
            "{} is not there: the key ceremony signs the first root before any other",
            metadata.join(FIRST_ROOT).display()
        ));
    }
    let mut roots = releases::root_chain(metadata)?;
    let previous = roots.last().cloned().ok_or("the source tree holds no root")?;
    if let Some(repo) = repo {
        releases::add_roots(repo, metadata)?;
    }
    let old = Signed::<Root>::parse(&previous).map_err(|e| format!("the root before: {e}"))?;
    let version = old.version + 1;
    let (yubikeys, online) = read_keys(keys)?;
    let plugged = yubikey::plugged(helper)?;
    let mut signers = Vec::new();
    for &serial in &plugged {
        let filed = yubikeys.iter().find(|y| y.serial == serial);
        let key = match yubikey::public_key(helper, serial, RELEASE_SLOT) {
            Ok(key) => key,
            // A YubiKey that holds no release key and that no file names
            // has nothing to do with the root.
            Err(_) if filed.is_none() => continue,
            Err(err) => return Err(err),
        };
        if let Some(filed) = filed
            && filed.key != key
        {
            return Err(format!(
                "YubiKey {serial}'s slot {RELEASE_SLOT} holds another key than {}",
                keys.join(filed.file_name()).display()
            ));
        }
        if filed.is_some() || names(&old.signed, "root", &key) {
            signers.push(YubiKey::new(helper, serial, RELEASE_SLOT, key));
        }
    }
    for yubikey in &yubikeys {
        if !names(&old.signed, "root", &yubikey.key) && !plugged.contains(&yubikey.serial) {
            return Err(format!(
                "YubiKey {} is new to the root and is not plugged in: it signs root {version} \
                 too, so that it is shown to work",
                yubikey.serial
            ));
        }
    }
    if !signers.iter().any(|s| names(&old.signed, "root", s.key())) {
        return Err(format!(
            "no YubiKey plugged in holds a key that root {} names for root: plug in the one at \
             hand",
            old.version
        ));
    }
    let named: Vec<String> =
        yubikeys.iter().map(|y| y.key.id()).chain(std::iter::once(online.id())).collect();
    let mut dropped: Vec<String> =
        old.signed.keys.keys().filter(|id| !named.contains(id)).cloned().collect();
    dropped.sort();
    let shown = NextRoot {
        version,
        expires: now + ROOT_DAYS * 86_400,
        yubikeys: yubikeys.iter().map(|y| y.serial).collect(),
        new_yubikeys: yubikeys
            .iter()
            .filter(|y| !names(&old.signed, "root", &y.key))
            .map(|y| y.serial)
            .collect(),
        online: online.id(),
        new_online: !names(&old.signed, "timestamp", &online),
        dropped,
        signers: signers.iter().map(YubiKey::serial).collect(),
        targets: match repo {
            Some(repo) => match updates::targets_version(repo)? {
                0 => None,
                offered => Some(offered + 1),
            },
            None => None,
        },
    };
    confirm(&shown)?;
    let refs: Vec<&dyn Signer> = signers.iter().map(|s| s as &dyn Signer).collect();
    let root = next_root(&previous, &yubikeys, &online, &refs, now)?;
    roots.push(root.clone());
    if let Some(repo) = repo {
        // A YubiKey the new root names for targets, which signed it.
        let signer = signers
            .iter()
            .find(|s| yubikeys.iter().any(|y| &y.key == s.key()))
            .ok_or("no YubiKey the new root names is plugged in")?;
        let keys = Keys::hardware(roots, signer, &["targets"])?;
        let folder = repo.join("metadata");
        for (name, bytes) in [(format!("{version}.root.json"), &root), ("root.json".into(), &root)]
        {
            write_whole(&folder.join(name), bytes)?;
        }
        updates::sign_targets_again(repo, &keys, now)?;
    }
    write_whole(&metadata.join(format!("{version}.root.json")), &root)?;
    Ok(SignedRoot { root, shown })
}

/// Writes `bytes` as `path`, whole under another name first, so that a
/// root half written is never taken for one.
fn write_whole(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let partial = path.with_extension("json.new");
    std::fs::write(&partial, bytes)
        .and_then(|()| std::fs::rename(&partial, path))
        .map_err(|e| format!("{}: {e}", path.display()))
}

/// Signs the first root with both YubiKeys, plugged in and found through
/// `helper`, from the keys in `keys`, and writes it to
/// `metadata/1.root.json`.
///
/// # Errors
///
/// If the first root is there already, a YubiKey is not plugged in or
/// does not hold the key its file names, or signing fails.
pub fn sign_first_root(
    keys: &Path,
    metadata: &Path,
    helper: &Path,
    now: i64,
) -> Result<Vec<u8>, String> {
    let path = metadata.join(FIRST_ROOT);
    if path.exists() {
        return Err(format!(
            "{} is there already: the first root is signed once, and every later one is the \
             next version",
            path.display()
        ));
    }
    let (yubikeys, online) = read_keys(keys)?;
    let plugged = yubikey::plugged(helper)?;
    let mut signers = Vec::new();
    for release_key in &yubikeys {
        let serial = release_key.serial;
        if !plugged.contains(&serial) {
            return Err(format!("YubiKey {serial} is not plugged in: both sign the first root"));
        }
        if yubikey::public_key(helper, serial, RELEASE_SLOT)? != release_key.key {
            return Err(format!(
                "YubiKey {serial}'s slot {RELEASE_SLOT} holds another key than {}",
                keys.join(release_key.file_name()).display()
            ));
        }
        signers.push(YubiKey::new(helper, serial, RELEASE_SLOT, release_key.key.clone()));
    }
    let refs: Vec<&dyn Signer> = signers.iter().map(|s| s as &dyn Signer).collect();
    let root = first_root(&yubikeys, &online, &refs, now)?;
    std::fs::create_dir_all(metadata).map_err(|e| format!("{}: {e}", metadata.display()))?;
    // Written whole under another name first, so that a root half written
    // is never taken for the first root.
    let partial = metadata.join(format!("{FIRST_ROOT}.new"));
    std::fs::write(&partial, &root)
        .and_then(|()| std::fs::rename(&partial, &path))
        .map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(root)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::list::tests::tree;
    use crate::updates::tests::Server;
    use ring::rand::SystemRandom;
    use ring::signature::{ECDSA_P256_SHA256_ASN1_SIGNING, EcdsaKeyPair, KeyPair as _};
    use std::process::Command;
    use tuf::repo::Ed25519;

    /// A P-256 key such as a YubiKey's slot holds.
    pub(crate) struct P256 {
        key: Key,
        pair: EcdsaKeyPair,
    }

    impl P256 {
        pub(crate) fn new() -> P256 {
            let rng = SystemRandom::new();
            let pkcs8 =
                EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_ASN1_SIGNING, &rng).unwrap();
            let pair =
                EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_ASN1_SIGNING, pkcs8.as_ref(), &rng)
                    .unwrap();
            let key = repo::p256_key(&repo::p256_pem(pair.public_key().as_ref())).unwrap();
            P256 { key, pair }
        }
    }

    impl Signer for P256 {
        fn key(&self) -> &Key {
            &self.key
        }

        fn sign(&self, message: &[u8]) -> Result<Vec<u8>, String> {
            let sig = self.pair.sign(&SystemRandom::new(), message).map_err(|e| e.to_string())?;
            Ok(sig.as_ref().to_vec())
        }
    }

    fn scratch(what: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("system-release-{}-{what}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// A stand-in for tools/yubikey-piv.py with YubiKeys 1001 and 1002
    /// plugged in, and 1003 at hand to replace one, as the file `plugged`
    /// in `dir` lists them: their slot 95 keys openssl holds and sign as a
    /// YubiKey does, ECDSA with SHA-256, in DER. Slot 9c holds no key.
    pub(crate) fn fake_yubikeys(dir: &Path) -> PathBuf {
        let openssl = |args: &[&str]| {
            let status = Command::new("openssl").args(args).current_dir(dir).status();
            assert!(status.is_ok_and(|s| s.success()), "openssl {args:?} failed");
        };
        for serial in ["1001", "1002", "1003"] {
            let secret = format!("{serial}.pem");
            openssl(&["ecparam", "-name", "prime256v1", "-genkey", "-noout", "-out", &secret]);
            openssl(&["ec", "-in", &secret, "-pubout", "-out", &format!("{serial}.pub")]);
        }
        let helper = dir.join("yubikey");
        let script = format!(
            "#!/bin/sh\ncd '{}' || exit 1\n\
             [ -f plugged ] || printf '1001\\n1002\\n' > plugged\n\
             case \"$1 $3\" in\n\
             'list ') cat plugged ;;\n\
             'public 95') grep -qx \"$2\" plugged && cat \"$2.pub\" ;;\n\
             'sign 95') grep -qx \"$2\" plugged && \
             openssl dgst -sha256 -sign \"$2.pem\" | od -An -tx1 | tr -d ' \\n'; echo ;;\n\
             *) exit 1 ;;\nesac\n",
            dir.display()
        );
        std::fs::write(&helper, script).unwrap();
        Command::new("chmod").arg("755").arg(&helper).status().unwrap();
        helper
    }

    pub(crate) fn online_key(keys: &Path) -> Key {
        let (online, _) = Ed25519::generate().unwrap();
        std::fs::create_dir_all(keys).unwrap();
        std::fs::write(keys.join("online.json"), serde_json::to_vec(online.key()).unwrap())
            .unwrap();
        online.key().clone()
    }

    #[test]
    fn the_first_root_names_both_yubikeys_for_every_role_and_the_server_for_two() {
        let (a, b) = (P256::new(), P256::new());
        let (online, _) = Ed25519::generate().unwrap();
        let yubikeys = [
            ReleaseKey { serial: 1, key: a.key.clone() },
            ReleaseKey { serial: 2, key: b.key.clone() },
        ];
        let now = 1_800_000_000;
        let bytes = first_root(&yubikeys, online.key(), &[&a, &b], now).unwrap();
        let root = Signed::<Root>::parse(&bytes).unwrap();
        assert_eq!((root.version, root.expires), (1, now + 365 * 86_400));
        assert_eq!(root.signatures.len(), 2);
        let ids = |role: &str| {
            let mut ids = root.signed.roles[role].keyids.clone();
            ids.sort();
            (ids, root.signed.roles[role].threshold)
        };
        let mut both = vec![a.key.id(), b.key.id()];
        both.sort();
        let mut with_online = vec![a.key.id(), b.key.id(), online.key().id()];
        with_online.sort();
        assert_eq!(ids("root"), (both.clone(), 1));
        assert_eq!(ids("targets"), (both, 1));
        assert_eq!(ids("snapshot"), (with_online.clone(), 1));
        assert_eq!(ids("timestamp"), (with_online, 1));
    }

    #[test]
    fn a_root_without_both_yubikeys_signing_is_refused() {
        let (a, b) = (P256::new(), P256::new());
        let (online, _) = Ed25519::generate().unwrap();
        let both = [
            ReleaseKey { serial: 1, key: a.key.clone() },
            ReleaseKey { serial: 2, key: b.key.clone() },
        ];
        let error = first_root(&both, online.key(), &[&a], 0).unwrap_err();
        assert_eq!(error, "YubiKey 2 did not sign the root");
        let error = first_root(&both[..1], online.key(), &[&a], 0).unwrap_err();
        assert!(error.contains("2 YubiKeys' and 1 is in keys/"), "{error}");
        let same = [both[0].clone(), ReleaseKey { serial: 2, key: a.key.clone() }];
        assert!(first_root(&same, online.key(), &[&a], 0).unwrap_err().contains("same key"));
        let online_too = first_root(&both, &a.key, &[&a, &b], 0).unwrap_err();
        assert!(online_too.contains("online key is one of"), "{online_too}");
    }

    #[test]
    fn the_ceremony_signs_the_first_root_once_with_both_yubikeys_plugged_in() {
        let dir = scratch("ceremony");
        let helper = fake_yubikeys(&dir);
        let (keys, metadata) = (dir.join("releases/keys"), dir.join("releases/metadata"));
        let missing = sign_first_root(&keys, &metadata, &helper, 0).unwrap_err();
        assert!(missing.contains("./ob release server setup"), "{missing}");
        let online = online_key(&keys);

        let error = add_yubikey(&keys, &helper, None).unwrap_err();
        assert!(error.contains("more than one YubiKey"), "{error}");
        let path = add_yubikey(&keys, &helper, Some(1001)).unwrap();
        assert_eq!(path, keys.join("yubikey-1001.json"));
        assert!(add_yubikey(&keys, &helper, Some(1001)).unwrap_err().contains("already"));
        let error = sign_first_root(&keys, &metadata, &helper, 0).unwrap_err();
        assert!(error.contains("1 is in keys/"), "{error}");
        add_yubikey(&keys, &helper, Some(1002)).unwrap();

        std::fs::write(dir.join("plugged"), "1001\n").unwrap();
        let error = sign_first_root(&keys, &metadata, &helper, 0).unwrap_err();
        assert_eq!(error, "YubiKey 1002 is not plugged in: both sign the first root");
        assert!(!metadata.join(FIRST_ROOT).exists());

        std::fs::write(dir.join("plugged"), "1001\n1002\n").unwrap();
        let now = tuf::time::now();
        let bytes = sign_first_root(&keys, &metadata, &helper, now).unwrap();
        assert_eq!(std::fs::read(metadata.join(FIRST_ROOT)).unwrap(), bytes);
        let root = Signed::<Root>::parse(&bytes).unwrap();
        let (yubikeys, read_online) = read_keys(&keys).unwrap();
        assert_eq!(read_online, online);
        for yubikey in &yubikeys {
            assert!(root.signatures.contains_key(&yubikey.key.id()));
        }
        assert!(root.signed.roles["snapshot"].keyids.contains(&online.id()));

        let again = sign_first_root(&keys, &metadata, &helper, now).unwrap_err();
        assert!(again.contains("is there already"), "{again}");
        assert_eq!(std::fs::read(metadata.join(FIRST_ROOT)).unwrap(), bytes);
    }

    /// Takes YubiKey `lost`'s files out of `keys`, as when it is lost or
    /// stolen, and adds YubiKey 1003's, plugged in with `keep`.
    fn replace_yubikey(server: &Server, lost: u64, keep: u64) {
        let keys = server.keys();
        for name in [format!("yubikey-{lost}.json"), format!("yubikey-{lost}.pem")] {
            std::fs::remove_file(keys.join(name)).unwrap();
        }
        plug(server, &[keep, 1003]);
        add_yubikey(&keys, &server.helper, Some(1003)).unwrap();
    }

    /// Plugs in the YubiKeys `serials` alone.
    fn plug(server: &Server, serials: &[u64]) {
        let lines: Vec<String> = serials.iter().map(u64::to_string).collect();
        std::fs::write(server.dir.join("plugged"), lines.join("\n") + "\n").unwrap();
    }

    fn targets(work: &Path, version: u64) -> Signed<Targets> {
        let path = work.join(format!("metadata/{version}.targets.json"));
        Signed::<Targets>::parse(&std::fs::read(path).unwrap()).unwrap()
    }

    #[test]
    fn a_lost_yubikey_is_dropped_by_the_next_root_which_the_server_then_serves() {
        let dir = scratch("next-root");
        let server = Server::new(&dir);
        let slot = dir.join("slot");
        tree(&slot);
        // The release was signed on YubiKey 1002, which is then lost.
        server.sign_and_serve(&slot, "0.18.0", 1002);
        replace_yubikey(&server, 1002, 1001);
        let work = dir.join("root-2");
        server.fetch(&work);
        let mut shown = None;
        let signed = sign_next_root(
            &server.keys(),
            &server.metadata(),
            &server.helper,
            Some(&work),
            tuf::time::now(),
            |next| {
                shown = Some(format!("{next:?}"));
                Ok(())
            },
        )
        .unwrap();
        let next = &signed.shown;
        assert_eq!(shown, Some(format!("{next:?}")));
        assert_eq!((next.version, next.targets), (2, Some(2)));
        assert_eq!(
            (next.yubikeys.as_slice(), next.new_yubikeys.as_slice()),
            ([1001, 1003].as_slice(), [1003].as_slice())
        );
        assert_eq!(
            (next.signers.as_slice(), next.dropped.len(), next.new_online),
            ([1001, 1003].as_slice(), 1, false)
        );
        assert_eq!(std::fs::read(server.metadata().join("2.root.json")).unwrap(), signed.root);

        // What the lost YubiKey signed does not check under the new root;
        // the same targets, signed again, do.
        let root = Signed::<Root>::parse(&signed.root).unwrap();
        assert!(targets(&work, 1).verified_by(&root.signed, "targets").is_err());
        targets(&work, 2).verified_by(&root.signed, "targets").unwrap();
        let listed = |version: u64| {
            let path = work.join(format!("metadata/{version}.targets.json"));
            serde_json::from_slice::<Value>(&std::fs::read(path).unwrap()).unwrap()["signed"]
                ["targets"]
                .clone()
        };
        assert_eq!(listed(2), listed(1));

        // Once deployed, a computer walks from the first root to the
        // second and is offered the same release; the lost YubiKey can
        // sign nothing it takes.
        server.deploy(&work, &server.secret).unwrap();
        let after = dir.join("after");
        let fetched = releases::fetch(&server.url, &server.first, &after).unwrap();
        assert_eq!((fetched.root, fetched.targets), (2, 2));
        plug(&server, &[1002]);
        let lost = yubikey::release_key(&server.helper, Some(1002)).unwrap();
        let refused = Keys::hardware(releases::roots_in(&after).unwrap(), &lost, &["targets"]);
        assert!(refused.is_err_and(|err| err.contains("does not name YubiKey 1002's key")));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_next_root_replaces_the_servers_online_key_and_delegates_the_rollouts_to_it() {
        let dir = scratch("next-root-online");
        let server = Server::new(&dir);
        let slot = dir.join("slot");
        tree(&slot);
        server.sign_and_serve(&slot, "0.18.0", 1001);
        let (_, old) = read_keys(&server.keys()).unwrap();
        let (online, secret) = Ed25519::generate().unwrap();
        let new = online.key().clone();
        std::fs::write(server.keys().join("online.json"), serde_json::to_vec(&new).unwrap())
            .unwrap();
        plug(&server, &[1001]);
        let work = dir.join("root-2");
        server.fetch(&work);
        let signed = sign_next_root(
            &server.keys(),
            &server.metadata(),
            &server.helper,
            Some(&work),
            tuf::time::now(),
            |_| Ok(()),
        )
        .unwrap();
        let next = &signed.shown;
        assert!(next.new_online && next.new_yubikeys.is_empty());
        assert_eq!(
            (next.dropped.as_slice(), next.signers.as_slice()),
            ([old.id()].as_slice(), [1001].as_slice())
        );
        let delegated = &targets(&work, 2).signed.delegations.unwrap().keys;
        assert!(delegated.contains_key(&new.id()) && !delegated.contains_key(&old.id()));

        // The old key signs the server's snapshots no more.
        let refused = server.deploy(&work, &server.secret).unwrap_err();
        assert!(refused.contains("does not name this server's online key"), "{refused}");
        server.deploy(&work, &secret).unwrap();
        let fetched = releases::fetch(&server.url, &server.first, &dir.join("after")).unwrap();
        assert_eq!((fetched.root, fetched.targets), (2, 2));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_next_root_signed_before_any_release_goes_out_with_the_first() {
        let dir = scratch("next-root-first-release");
        let server = Server::new(&dir);
        replace_yubikey(&server, 1002, 1001);
        let now = tuf::time::now();
        let work = dir.join("root-2");
        server.fetch(&work);
        let signed = sign_next_root(
            &server.keys(),
            &server.metadata(),
            &server.helper,
            Some(&work),
            now,
            |_| Ok(()),
        )
        .unwrap();
        assert_eq!(signed.shown.targets, None);
        // The yearly root, the same keys again, from the tree alone.
        let yearly =
            sign_next_root(&server.keys(), &server.metadata(), &server.helper, None, now, |next| {
                assert!(
                    next.dropped.is_empty() && next.new_yubikeys.is_empty() && !next.new_online
                );
                Ok(())
            })
            .unwrap();
        assert_eq!(yearly.shown.version, 3);
        let slot = dir.join("slot");
        tree(&slot);
        plug(&server, &[1003]);
        server.sign_and_serve(&slot, "0.18.0", 1003);
        let fetched = releases::fetch(&server.url, &server.first, &dir.join("after")).unwrap();
        assert_eq!((fetched.root, fetched.targets), (3, 1));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_next_root_is_refused_without_the_yubikeys_it_needs_or_the_roots_it_follows() {
        let dir = scratch("next-root-refused");
        let server = Server::new(&dir);
        let (keys, metadata) = (server.keys(), server.metadata());
        let sign = |repo: Option<&Path>, confirm: &dyn Fn(&NextRoot) -> Result<(), String>| {
            sign_next_root(&keys, &metadata, &server.helper, repo, 0, confirm)
        };
        replace_yubikey(&server, 1002, 1001);
        plug(&server, &[1001]);
        let error = sign(None, &|_| panic!("nothing is shown")).unwrap_err();
        assert!(error.contains("YubiKey 1003 is new to the root and is not plugged in"), "{error}");
        plug(&server, &[1003]);
        let error = sign(None, &|_| panic!("nothing is shown")).unwrap_err();
        assert!(error.contains("no YubiKey plugged in holds a key that root 1 names"), "{error}");
        plug(&server, &[1001, 1003]);
        let error = sign(None, &|_| Err("not typed".into())).unwrap_err();
        assert_eq!(error, "not typed");
        assert!(!metadata.join("2.root.json").exists());

        // A repository fetched without the tree's newer roots is given
        // them; one with a root the tree does not hold, or holds
        // otherwise, is refused.
        sign(None, &|_| Ok(())).unwrap();
        let work = dir.join("plain");
        releases::fetch(&server.url, &server.first, &work).unwrap();
        sign(Some(&work), &|next| {
            assert_eq!(next.version, 3);
            Err("not typed".into())
        })
        .unwrap_err();
        assert_eq!(releases::roots_in(&work).unwrap().len(), 2);
        let behind = dir.join("behind");
        std::fs::create_dir_all(&behind).unwrap();
        std::fs::copy(metadata.join(FIRST_ROOT), behind.join(FIRST_ROOT)).unwrap();
        let error = releases::add_roots(&work, &behind).unwrap_err();
        assert!(error.contains("the source tree is behind"), "{error}");
        std::fs::write(work.join("metadata/2.root.json"), b"{}").unwrap();
        assert!(releases::add_roots(&work, &metadata).unwrap_err().contains("is not"));
        let empty = dir.join("empty");
        let error = sign_next_root(&keys, &empty, &server.helper, None, 0, |_| Ok(())).unwrap_err();
        assert!(error.contains("the key ceremony signs the first root"), "{error}");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_key_file_must_be_the_release_slots_and_named_for_its_yubikey() {
        let dir = scratch("key-files");
        let keys = dir.join("keys");
        online_key(&keys);
        let key = P256::new().key;
        let catalogs = json!({ "yubikey": 5, "slot": "9c", "key": key });
        std::fs::write(keys.join("yubikey-5.json"), catalogs.to_string()).unwrap();
        let error = read_keys(&keys).unwrap_err();
        assert!(error.contains("not a key of slot 95"), "{error}");
        let misnamed = json!({ "yubikey": 6, "slot": "95", "key": key });
        std::fs::write(keys.join("yubikey-5.json"), misnamed.to_string()).unwrap();
        let error = read_keys(&keys).unwrap_err();
        assert!(error.contains("holds YubiKey 6's key"), "{error}");
    }
}
