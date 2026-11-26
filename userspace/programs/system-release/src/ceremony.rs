// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The system releases' hardware keys and the first root they sign, in
//! the key ceremony (ADRs 0457 and 0538). The source tree's `releases/`
//! keeps their public halves: `keys/yubikey-SERIAL.json` for each of the
//! two YubiKeys' slot-95 keys, `keys/online.json` for the update server's
//! online key, and `metadata/1.root.json`, which every repository the
//! server serves starts from.
//!
//! The root names both YubiKeys for all four roles, either signing
//! alone, and the online key beside them for snapshot and timestamp: the
//! server refreshes those two every few hours, and an install stick or an
//! update file, made offline, has them signed by a YubiKey instead. Both
//! YubiKeys sign the first root, so that the ceremony proves each one
//! works. When `root` fails, `tools/yubikey-piv.py public SERIAL 95`
//! shows whether the YubiKey's slot holds the key its file names.

use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use tuf::keys::Key;
use tuf::metadata::{Root, Signed};
use tuf::repo::{self, Signer};

use crate::repo::{ONLINE, ROLES};
use crate::yubikey::{self, RELEASE_SLOT, YubiKey};

/// How many YubiKeys hold the release keys: one kept at hand and one kept
/// apart (ADR 0457).
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
/// in, through `helper`, and keeps its public half in `dir`.
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
    let offline: Vec<&Key> = yubikeys.iter().map(|y| &y.key).collect();
    let roles: Vec<(&str, Vec<&Key>, u64)> = ROLES
        .into_iter()
        .map(|role| {
            let mut keys = offline.clone();
            if ONLINE.contains(&role) {
                keys.push(online);
            }
            (role, keys, 1)
        })
        .collect();
    let expires = tuf::time::format(now + ROOT_DAYS * 86_400);
    let signed = repo::root(1, &expires, &roles, true);
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

    /// A stand-in for tools/yubikey-piv.py with YubiKeys 1001 and 1002,
    /// whose slot 95 keys openssl holds and sign as a YubiKey does:
    /// ECDSA with SHA-256, in DER. Slot 9c holds no key on either.
    pub(crate) fn fake_yubikeys(dir: &Path) -> PathBuf {
        let openssl = |args: &[&str]| {
            let status = Command::new("openssl").args(args).current_dir(dir).status();
            assert!(status.is_ok_and(|s| s.success()), "openssl {args:?} failed");
        };
        for serial in ["1001", "1002"] {
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
