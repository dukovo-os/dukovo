// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Making a repository's metadata (ADR 0381), as the catalog's tools do:
//! each role's `signed` part, signed over its canonical JSON by its keys,
//! with every signature checked by the client's own code before the file
//! is written, so that nothing is published that a client would refuse.

use std::collections::BTreeMap;

use base64::Engine;
use ring::rand::SystemRandom;
use ring::signature::{Ed25519KeyPair, KeyPair};
use serde_json::{Value, json};

use crate::keys::{Key, KeyVal};
use crate::{json as canonical_json, time};

/// The version of TUF's specification the metadata says it follows.
pub const SPEC_VERSION: &str = "1.0.31";

/// What signs for one key: a file's secret half, or a hardware key.
pub trait Signer {
    /// The key it signs for.
    fn key(&self) -> &Key;

    /// Its signature of `message`: Ed25519's 64 bytes, or ECDSA's in DER.
    ///
    /// # Errors
    ///
    /// Why it could not sign, in words for the person signing.
    fn sign(&self, message: &[u8]) -> Result<Vec<u8>, String>;
}

/// An Ed25519 key whose secret half is kept in a file, as PKCS #8: an
/// online role's key, or a test's.
pub struct Ed25519 {
    key: Key,
    pair: Ed25519KeyPair,
}

impl Ed25519 {
    /// A new key, and its secret half to keep.
    ///
    /// # Errors
    ///
    /// If the system's random numbers fail.
    pub fn generate() -> Result<(Ed25519, Vec<u8>), String> {
        let pkcs8 = Ed25519KeyPair::generate_pkcs8(&SystemRandom::new())
            .map_err(|_| "no random numbers for a new key".to_string())?;
        let key = Ed25519::from_pkcs8(pkcs8.as_ref())?;
        Ok((key, pkcs8.as_ref().to_vec()))
    }

    /// The key whose secret half is `der`, PKCS #8.
    ///
    /// # Errors
    ///
    /// If `der` is not an Ed25519 key in PKCS #8.
    pub fn from_pkcs8(der: &[u8]) -> Result<Ed25519, String> {
        let pair = Ed25519KeyPair::from_pkcs8_maybe_unchecked(der)
            .map_err(|_| "not an Ed25519 secret key in PKCS #8".to_string())?;
        Ok(Ed25519 { key: ed25519_key(pair.public_key().as_ref()), pair })
    }
}

impl Signer for Ed25519 {
    fn key(&self) -> &Key {
        &self.key
    }

    fn sign(&self, message: &[u8]) -> Result<Vec<u8>, String> {
        Ok(self.pair.sign(message).as_ref().to_vec())
    }
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    bytes.iter().fold(String::new(), |mut text, b| {
        let _ = write!(text, "{b:02x}");
        text
    })
}

/// The Ed25519 key whose public half is `public`, its 32 bytes.
#[must_use]
pub fn ed25519_key(public: &[u8]) -> Key {
    Key {
        keytype: "ed25519".into(),
        scheme: "ed25519".into(),
        keyval: KeyVal { public: hex(public) },
    }
}

/// The ECDSA P-256 key whose public half is `pem`, a SubjectPublicKeyInfo
/// in PEM, as a YubiKey's PIV slot gives it.
///
/// # Errors
///
/// If `pem` is not a P-256 public key.
pub fn p256_key(pem: &str) -> Result<Key, String> {
    let key = Key {
        keytype: "ecdsa".into(),
        scheme: "ecdsa-sha2-nistp256".into(),
        keyval: KeyVal { public: normal_pem(pem) },
    };
    if !key.is_p256() {
        return Err("not an ECDSA P-256 public key in PEM".into());
    }
    Ok(key)
}

/// `pem` with its base64 in lines of 64 and a newline at the end, as
/// securesystemslib writes it.
fn normal_pem(pem: &str) -> String {
    let body: String = pem
        .lines()
        .filter(|line| !line.starts_with("-----"))
        .flat_map(|line| line.chars().filter(|c| !c.is_ascii_whitespace()))
        .collect();
    let mut out = String::from("-----BEGIN PUBLIC KEY-----\n");
    for chunk in body.as_bytes().chunks(64) {
        out.push_str(&String::from_utf8_lossy(chunk));
        out.push('\n');
    }
    out.push_str("-----END PUBLIC KEY-----\n");
    out
}

/// The PEM SubjectPublicKeyInfo of the P-256 point `point`, uncompressed.
#[must_use]
pub fn p256_pem(point: &[u8]) -> String {
    let mut der = vec![0x30, 0x59, 0x30, 0x13, 0x06, 0x07];
    der.extend_from_slice(&[0x2a, 0x86, 0x48, 0xce, 0x3d, 0x02, 0x01]);
    der.extend_from_slice(&[0x06, 0x08, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x03, 0x01, 0x07]);
    der.extend_from_slice(&[0x03, 0x42, 0x00]);
    der.extend_from_slice(point);
    normal_pem(&base64::engine::general_purpose::STANDARD.encode(der))
}

/// The metadata file of `signed`, signed by each of `signers`: JSON in
/// two-space lines, so that its changes read well in git. Each signature
/// is checked as a client checks it before it is put in.
///
/// # Errors
///
/// If `signed` is not canonical JSON, a signer fails, or a signature
/// does not check.
pub fn sign(signed: &Value, signers: &[&dyn Signer]) -> Result<Vec<u8>, String> {
    let message = canonical_json::canonical(signed).map_err(|err| format!("{err:?}"))?;
    let mut signatures = Vec::new();
    for signer in signers {
        let key = signer.key();
        let sig = hex(&signer.sign(&message)?);
        if !key.verifies(&message, &sig) {
            return Err(format!("the signature by key {} does not check", key.id()));
        }
        signatures.push(json!({ "keyid": key.id(), "sig": sig }));
    }
    let file = json!({ "signatures": signatures, "signed": signed });
    let mut out = serde_json::to_vec_pretty(&file).map_err(|err| err.to_string())?;
    out.push(b'\n');
    Ok(out)
}

/// The length and SHA-256 of `data`, as `meta` and `targets` list a
/// file.
#[must_use]
pub fn file_info(data: &[u8]) -> Value {
    let sha256 = ring::digest::digest(&ring::digest::SHA256, data);
    json!({ "length": data.len(), "hashes": { "sha256": hex(sha256.as_ref()) } })
}

/// The time `days` from now, as metadata's `expires`.
#[must_use]
pub fn expires_in(days: i64) -> String {
    time::format(time::now() + days * 86_400)
}

fn common(kind: &str, version: u64, expires: &str) -> serde_json::Map<String, Value> {
    let mut signed = serde_json::Map::new();
    signed.insert("_type".into(), kind.into());
    signed.insert("spec_version".into(), SPEC_VERSION.into());
    signed.insert("version".into(), version.into());
    signed.insert("expires".into(), expires.into());
    signed
}

/// A root's `signed`: each of the four top-level roles with its keys and
/// threshold.
#[must_use]
pub fn root(
    version: u64,
    expires: &str,
    roles: &[(&str, Vec<&Key>, u64)],
    consistent_snapshot: bool,
) -> Value {
    let mut keys = BTreeMap::new();
    let mut by_role = BTreeMap::new();
    for (role, role_keys, threshold) in roles {
        let mut ids: Vec<String> = role_keys.iter().map(|key| key.id()).collect();
        ids.sort();
        ids.dedup();
        for key in role_keys {
            keys.insert(key.id(), json!(key));
        }
        by_role.insert((*role).to_string(), json!({ "keyids": ids, "threshold": threshold }));
    }
    let mut signed = common("root", version, expires);
    signed.insert("consistent_snapshot".into(), consistent_snapshot.into());
    signed.insert("keys".into(), json!(keys));
    signed.insert("roles".into(), json!(by_role));
    Value::Object(signed)
}

/// A targets role's `signed`: `targets` by path, each its
/// [`file_info`] and maybe `custom`.
#[must_use]
pub fn targets(version: u64, expires: &str, targets: &BTreeMap<String, Value>) -> Value {
    let mut signed = common("targets", version, expires);
    signed.insert("targets".into(), json!(targets));
    Value::Object(signed)
}

/// A snapshot's `signed`: each targets role's file, by name, with its
/// version and [`file_info`].
#[must_use]
pub fn snapshot(version: u64, expires: &str, meta: &BTreeMap<String, Value>) -> Value {
    let mut signed = common("snapshot", version, expires);
    signed.insert("meta".into(), json!(meta));
    Value::Object(signed)
}

/// A timestamp's `signed`: the snapshot `data`, of version
/// `snapshot_version`.
#[must_use]
pub fn timestamp(version: u64, expires: &str, snapshot_version: u64, data: &[u8]) -> Value {
    let mut meta = file_info(data);
    meta["version"] = snapshot_version.into();
    let mut signed = common("timestamp", version, expires);
    signed.insert("meta".into(), json!({ "snapshot.json": meta }));
    Value::Object(signed)
}

/// A role's file with its version and [`file_info`], as a snapshot lists
/// it.
#[must_use]
pub fn meta(version: u64, data: &[u8]) -> Value {
    let mut meta = file_info(data);
    meta["version"] = version.into();
    meta
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Updater, Url};
    use ring::signature::{ECDSA_P256_SHA256_ASN1_SIGNING, EcdsaKeyPair};
    use std::path::{Path, PathBuf};

    /// A P-256 key such as a YubiKey's PIV slot holds.
    struct P256 {
        key: Key,
        pair: EcdsaKeyPair,
    }

    impl P256 {
        fn new() -> P256 {
            let rng = SystemRandom::new();
            let pkcs8 =
                EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_ASN1_SIGNING, &rng).unwrap();
            let pair =
                EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_ASN1_SIGNING, pkcs8.as_ref(), &rng)
                    .unwrap();
            let key = p256_key(&p256_pem(pair.public_key().as_ref())).unwrap();
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
        let dir = std::env::temp_dir().join(format!("tuf-repo-{}-{what}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn put(path: &Path, data: &[u8]) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, data).unwrap();
    }

    #[test]
    fn keys_have_securesystemslibs_ids() {
        // As securesystemslib 1.6's compute_default_keyid makes it.
        let key = ed25519_key(&[0x42; 32]);
        assert_eq!(key.id(), "a5364f67a730e4f4e5df924c8cefc4992bac3f831c19f2f43c743f847d004fa6");
        let p256 = P256::new();
        assert!(p256_key("-----BEGIN PUBLIC KEY-----\nAAAA\n-----END PUBLIC KEY-----").is_err());
        assert_eq!(p256_key(&p256.key.keyval.public.replace('\n', "\r\n")), Ok(p256.key));
    }

    #[test]
    fn a_signature_that_does_not_check_is_refused() {
        struct Wrong(Ed25519);
        impl Signer for Wrong {
            fn key(&self) -> &Key {
                self.0.key()
            }
            fn sign(&self, message: &[u8]) -> Result<Vec<u8>, String> {
                self.0.sign(&[message, b"!"].concat())
            }
        }
        let (key, _) = Ed25519::generate().unwrap();
        let signed = targets(1, "2030-01-01T00:00:00Z", &BTreeMap::new());
        assert!(sign(&signed, &[&key]).is_ok());
        assert!(sign(&signed, &[&Wrong(key)]).unwrap_err().contains("does not check"));
    }

    /// A repository made here, with a P-256 key for root and targets, as
    /// the catalog's YubiKeys are, and Ed25519 for snapshot and timestamp,
    /// is read by the client, which fetches a target from it.
    #[test]
    fn the_client_takes_what_this_makes() {
        let dir = scratch("made");
        let offline = P256::new();
        let (online, secret) = Ed25519::generate().unwrap();
        let online_again = Ed25519::from_pkcs8(&secret).unwrap();
        assert_eq!(online_again.key(), online.key());
        let expires = expires_in(30);
        let roles = [
            ("root", vec![offline.key()], 1),
            ("targets", vec![offline.key()], 1),
            ("snapshot", vec![online.key()], 1),
            ("timestamp", vec![online.key()], 1),
        ];
        let root_file = sign(&root(1, &expires, &roles, true), &[&offline]).unwrap();
        put(&dir.join("metadata/1.root.json"), &root_file);

        let bundle = b"a bundle's bytes".as_slice();
        let mut info = file_info(bundle);
        info["custom"] = json!({ "id": "org.dukovo.ripgrep" });
        let listed = BTreeMap::from([("bundles/ripgrep.dkv".to_string(), info)]);
        let targets_file = sign(&targets(1, &expires, &listed), &[&offline]).unwrap();
        put(&dir.join("metadata/1.targets.json"), &targets_file);
        let sha256 = file_info(bundle)["hashes"]["sha256"].as_str().unwrap().to_string();
        put(&dir.join(format!("targets/bundles/{sha256}.ripgrep.dkv")), bundle);

        let meta_files = BTreeMap::from([("targets.json".to_string(), meta(1, &targets_file))]);
        let snapshot_file = sign(&snapshot(1, &expires, &meta_files), &[&online]).unwrap();
        put(&dir.join("metadata/1.snapshot.json"), &snapshot_file);
        let timestamp_file =
            sign(&timestamp(1, &expires, 1, &snapshot_file), &[&online_again]).unwrap();
        put(&dir.join("metadata/timestamp.json"), &timestamp_file);

        let client = dir.join("client");
        crate::init(&client, &root_file).unwrap();
        let base = Url::parse(&format!("file://{}/", dir.display())).unwrap();
        let metadata = base.resolve("metadata/").unwrap();
        let targets_url = base.resolve("targets/").unwrap();
        let mut updater = Updater::new(&client, &metadata, Some(&targets_url)).unwrap();
        updater.refresh().unwrap();
        let target = updater.target("bundles/ripgrep.dkv").unwrap().unwrap();
        assert_eq!(target.custom, Some(json!({ "id": "org.dukovo.ripgrep" })));
        let file = client.join("ripgrep.dkv");
        updater.download_target("bundles/ripgrep.dkv", &target, &file).unwrap();
        assert_eq!(std::fs::read(&file).unwrap(), bundle);
        assert!(updater.target("bundles/bat.dkv").unwrap().is_none());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// Makes in `dir` a repository of one target, `system/1.0.json`, whose
    /// metadata all expired yesterday, signed by one Ed25519 key; gives its
    /// root.
    fn expired_repository(dir: &Path) -> Vec<u8> {
        let (key, _) = Ed25519::generate().unwrap();
        let expires = expires_in(-1);
        let roles =
            ["root", "targets", "snapshot", "timestamp"].map(|role| (role, vec![key.key()], 1));
        let root_file = sign(&root(1, &expires, &roles, true), &[&key]).unwrap();
        put(&dir.join("metadata/1.root.json"), &root_file);
        let list = b"a release list".as_slice();
        let listed = BTreeMap::from([("system/1.0.json".to_string(), file_info(list))]);
        let targets_file = sign(&targets(1, &expires, &listed), &[&key]).unwrap();
        put(&dir.join("metadata/1.targets.json"), &targets_file);
        let meta_files = BTreeMap::from([("targets.json".to_string(), meta(1, &targets_file))]);
        let snapshot_file = sign(&snapshot(1, &expires, &meta_files), &[&key]).unwrap();
        put(&dir.join("metadata/1.snapshot.json"), &snapshot_file);
        let timestamp_file = sign(&timestamp(1, &expires, 1, &snapshot_file), &[&key]).unwrap();
        put(&dir.join("metadata/timestamp.json"), &timestamp_file);
        root_file
    }

    /// Expired metadata is refused online, and taken offline, as from an
    /// install stick the owner chose; a signature that does not check is
    /// refused either way.
    #[test]
    fn expiry_is_checked_online_but_not_offline() {
        let dir = scratch("expired");
        let root_file = expired_repository(&dir);
        let base = Url::parse(&format!("file://{}/", dir.display())).unwrap();
        let metadata = base.resolve("metadata/").unwrap();
        let client = dir.join("online");
        crate::init(&client, &root_file).unwrap();
        let err = Updater::new(&client, &metadata, None).unwrap().refresh().unwrap_err();
        assert!(err.to_string().contains("expired"), "{err}");

        let client = dir.join("offline");
        crate::init(&client, &root_file).unwrap();
        let mut updater = Updater::offline(&client, &metadata, None).unwrap();
        updater.refresh().unwrap();
        assert!(updater.target("system/1.0.json").unwrap().is_some());

        let path = dir.join("metadata/timestamp.json");
        let forged = String::from_utf8(std::fs::read(&path).unwrap())
            .unwrap()
            .replace("\"version\": 1", "\"version\": 2");
        std::fs::write(&path, forged).unwrap();
        let client = dir.join("forged");
        crate::init(&client, &root_file).unwrap();
        let err = Updater::offline(&client, &metadata, None).unwrap().refresh().unwrap_err();
        assert!(err.to_string().contains("sign"), "{err}");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
