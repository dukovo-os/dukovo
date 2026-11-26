// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The public record of a release's signature (ADRs 0457 and 0778): an
//! entry for Sigstore's Rekor log, of the kind `hashedrekord`, which says
//! that a release key signed the targets metadata whose SHA-256 it names.
//! The log is public and only grows, so a release signed in secret would
//! stand out. Machines never read it.
//!
//! `system-release offer --record FILE` writes the entry with [`entry`]
//! once the YubiKey has signed; `./ob release sign` sends it to the log,
//! and `./ob release record` sends it again when the log could not be
//! reached. The log checks an ECDSA signature against the SHA-256 it is
//! given, which is what a YubiKey's P-256 key signs in TUF: the SHA-256
//! of the metadata's canonical JSON. When the log refuses an entry, its
//! answer says why, and the entry is plain JSON to read beside it.

use serde_json::json;
use tuf::keys::Key;
use tuf::metadata::{Signed, Targets};
use tuf::repo::to_base64;

use crate::list::hex;

/// The Rekor entry of the signature `key` made of the targets metadata
/// `metadata`: the SHA-256 of what was signed, the signature in DER, and
/// the key in PEM, each in base64 as the log takes them.
///
/// # Errors
///
/// If `key` is not a P-256 key, as only a YubiKey's signature is
/// recorded, `metadata` is not targets metadata, or it carries no
/// signature of `key` that checks.
pub fn entry(metadata: &[u8], key: &Key) -> Result<Vec<u8>, String> {
    let id = key.id();
    if !key.is_p256() {
        return Err(format!(
            "key {} is not a YubiKey's: only a release key's signature is recorded",
            &id[..16]
        ));
    }
    let signed = Signed::<Targets>::parse(metadata).map_err(|err| format!("the targets: {err}"))?;
    let der = signed
        .signatures
        .get(&id)
        .filter(|sig| key.verifies(&signed.bytes, sig))
        .and_then(|sig| tuf::keys::hex(sig))
        .ok_or_else(|| {
            format!("the targets carry no signature of key {} that checks", &id[..16])
        })?;
    let sha256 = hex(ring::digest::digest(&ring::digest::SHA256, &signed.bytes).as_ref());
    let body = json!({
        "apiVersion": "0.0.1",
        "kind": "hashedrekord",
        "spec": {
            "data": { "hash": { "algorithm": "sha256", "value": sha256 } },
            "signature": {
                "content": to_base64(&der),
                "publicKey": { "content": to_base64(key.keyval.public.as_bytes()) },
            },
        },
    });
    let mut bytes = serde_json::to_vec_pretty(&body).map_err(|err| err.to_string())?;
    bytes.push(b'\n');
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ceremony::tests::P256;
    use crate::list::tests::scratch;
    use std::collections::BTreeMap;
    use std::process::Command;
    use tuf::repo::{self, Ed25519, Signer as _};

    fn targets(signer: &dyn tuf::repo::Signer) -> Vec<u8> {
        let listed = BTreeMap::from([("system/1.0.0.json".to_string(), repo::file_info(b"list"))]);
        repo::sign(&repo::targets(3, "2027-05-27T00:00:00Z", &listed), &[signer]).unwrap()
    }

    /// The entry checks as the log checks it, with openssl: the signature
    /// in it, by the key in it, over the SHA-256 it names.
    #[test]
    fn the_entry_checks_against_the_hash_it_names() {
        let dir = scratch("rekor");
        let yubikey = P256::new();
        let metadata = targets(&yubikey);
        let entry: serde_json::Value =
            serde_json::from_slice(&entry(&metadata, yubikey.key()).unwrap()).unwrap();
        assert_eq!(
            (entry["kind"].as_str(), entry["apiVersion"].as_str()),
            (Some("hashedrekord"), Some("0.0.1"))
        );
        let canonical = Signed::<Targets>::parse(&metadata).unwrap().bytes;
        std::fs::write(dir.join("signed"), &canonical).unwrap();
        let spec = &entry["spec"];
        for (file, value) in [
            ("sig.b64", &spec["signature"]["content"]),
            ("key.b64", &spec["signature"]["publicKey"]["content"]),
        ] {
            std::fs::write(dir.join(file), value.as_str().unwrap()).unwrap();
        }
        let openssl = |args: &[&str]| {
            let out = Command::new("openssl").args(args).current_dir(&dir).output().unwrap();
            assert!(
                out.status.success(),
                "openssl {args:?}: {}",
                String::from_utf8_lossy(&out.stderr)
            );
            String::from_utf8(out.stdout).unwrap()
        };
        openssl(&["base64", "-d", "-A", "-in", "sig.b64", "-out", "sig.der"]);
        openssl(&["base64", "-d", "-A", "-in", "key.b64", "-out", "key.pem"]);
        openssl(&["dgst", "-sha256", "-verify", "key.pem", "-signature", "sig.der", "signed"]);
        let digest = openssl(&["dgst", "-sha256", "-r", "signed"]);
        assert_eq!(Some(digest.split(' ').next().unwrap()), spec["data"]["hash"]["value"].as_str());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn only_a_yubikeys_signature_that_checks_is_recorded() {
        let (online, _) = Ed25519::generate().unwrap();
        let error = entry(&targets(&online), online.key()).unwrap_err();
        assert!(error.contains("not a YubiKey's"), "{error}");
        let (signer, other) = (P256::new(), P256::new());
        let error = entry(&targets(&signer), other.key()).unwrap_err();
        assert!(error.contains("no signature of key"), "{error}");
        let mut tampered = targets(&signer);
        let at = tampered.windows(5).position(|w| w == b"1.0.0").unwrap();
        tampered[at] = b'2';
        assert!(entry(&tampered, signer.key()).unwrap_err().contains("no signature of key"));
    }
}
