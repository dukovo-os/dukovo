// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Public keys as TUF metadata lists them, and their signatures checked
//! with ring: Ed25519, ECDSA on P-256 and P-384, and RSA with PSS or
//! PKCS #1 v1.5 padding, the key types and schemes securesystemslib
//! writes. A key of another type parses, but no signature by it counts.

use base64::Engine;
use ring::signature::{self, UnparsedPublicKey, VerificationAlgorithm};
use serde::Deserialize;

/// A public key, as a role's metadata lists it.
#[derive(Debug, Clone, Deserialize)]
pub struct Key {
    /// `ed25519`, `ecdsa` (or its older name, `ecdsa-sha2-nistp256`) or
    /// `rsa`.
    pub keytype: String,
    /// How it signs, such as `ed25519` or `ecdsa-sha2-nistp256`.
    pub scheme: String,
    /// Its public half.
    #[serde(default)]
    pub keyval: KeyVal,
}

/// A key's public half.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct KeyVal {
    /// Hex for Ed25519; PEM, a SubjectPublicKeyInfo, for the others.
    #[serde(default)]
    pub public: String,
}

/// The object identifier of an elliptic curve public key, then that of
/// the curve: P-256 and P-384; then RSA's.
const EC_PUBLIC_KEY: &[u8] = &[0x2a, 0x86, 0x48, 0xce, 0x3d, 0x02, 0x01];
const P256: &[u8] = &[0x2a, 0x86, 0x48, 0xce, 0x3d, 0x03, 0x01, 0x07];
const P384: &[u8] = &[0x2b, 0x81, 0x04, 0x00, 0x22];
const RSA: &[u8] = &[0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x01, 0x01];

/// What a key's public bytes are.
enum Form {
    /// Raw, in hex: an Ed25519 key.
    Hex,
    /// A SubjectPublicKeyInfo in PEM, of the algorithm and curve (if any)
    /// these object identifiers name.
    Spki(&'static [u8], Option<&'static [u8]>),
}

impl Key {
    /// How signatures by this key are checked, and what its public bytes
    /// are; `None` for a key type or scheme that is not supported.
    fn algorithm(&self) -> Option<(&'static dyn VerificationAlgorithm, Form)> {
        let ecdsa = self.keytype == "ecdsa" || self.keytype == "ecdsa-sha2-nistp256";
        Some(match (self.keytype.as_str(), self.scheme.as_str()) {
            ("ed25519", "ed25519") => (&signature::ED25519, Form::Hex),
            (_, "ecdsa-sha2-nistp256") if ecdsa => {
                (&signature::ECDSA_P256_SHA256_ASN1, Form::Spki(EC_PUBLIC_KEY, Some(P256)))
            }
            ("ecdsa", "ecdsa-sha2-nistp384") => {
                (&signature::ECDSA_P384_SHA384_ASN1, Form::Spki(EC_PUBLIC_KEY, Some(P384)))
            }
            ("rsa", "rsassa-pss-sha256") => {
                (&signature::RSA_PSS_2048_8192_SHA256, Form::Spki(RSA, None))
            }
            ("rsa", "rsassa-pss-sha384") => {
                (&signature::RSA_PSS_2048_8192_SHA384, Form::Spki(RSA, None))
            }
            ("rsa", "rsassa-pss-sha512") => {
                (&signature::RSA_PSS_2048_8192_SHA512, Form::Spki(RSA, None))
            }
            ("rsa", "rsa-pkcs1v15-sha256") => {
                (&signature::RSA_PKCS1_2048_8192_SHA256, Form::Spki(RSA, None))
            }
            ("rsa", "rsa-pkcs1v15-sha384") => {
                (&signature::RSA_PKCS1_2048_8192_SHA384, Form::Spki(RSA, None))
            }
            ("rsa", "rsa-pkcs1v15-sha512") => {
                (&signature::RSA_PKCS1_2048_8192_SHA512, Form::Spki(RSA, None))
            }
            _ => return None,
        })
    }

    /// Whether `sig`, a signature in hex, is this key's over `message`.
    #[must_use]
    pub fn verifies(&self, message: &[u8], sig: &str) -> bool {
        let Some((algorithm, form)) = self.algorithm() else { return false };
        let Some(sig) = hex(sig) else { return false };
        let public = match form {
            Form::Hex => hex(&self.keyval.public),
            Form::Spki(oid, curve) => {
                pem(&self.keyval.public).and_then(|der| spki(&der, oid, curve).map(<[u8]>::to_vec))
            }
        };
        let Some(public) = public else { return false };
        UnparsedPublicKey::new(algorithm, public).verify(message, &sig).is_ok()
    }
}

/// The bytes of `text` in hex, of either case.
#[must_use]
pub fn hex(text: &str) -> Option<Vec<u8>> {
    if !text.len().is_multiple_of(2) {
        return None;
    }
    let digit = |b: u8| (b as char).to_digit(16);
    text.as_bytes()
        .chunks(2)
        .map(|pair| u8::try_from(digit(pair[0])? * 16 + digit(pair[1])?).ok())
        .collect()
}

/// The DER of a PEM `PUBLIC KEY`.
fn pem(text: &str) -> Option<Vec<u8>> {
    let body = text.trim().strip_prefix("-----BEGIN PUBLIC KEY-----")?;
    let body = body.trim_end().strip_suffix("-----END PUBLIC KEY-----")?;
    let base64: String = body.chars().filter(|c| !c.is_ascii_whitespace()).collect();
    base64::engine::general_purpose::STANDARD.decode(base64).ok()
}

/// One DER element at the start of `der`: its tag, its contents and what
/// follows it.
fn element(der: &[u8]) -> Option<(u8, &[u8], &[u8])> {
    let (&tag, rest) = der.split_first()?;
    let (&first, rest) = rest.split_first()?;
    let (length, rest) = if first < 0x80 {
        (usize::from(first), rest)
    } else {
        let count = usize::from(first & 0x7f);
        if count == 0 || count > 4 || rest.len() < count {
            return None;
        }
        let length = rest[..count].iter().fold(0usize, |n, &b| (n << 8) | usize::from(b));
        (length, &rest[count..])
    };
    if rest.len() < length {
        return None;
    }
    Some((tag, &rest[..length], &rest[length..]))
}

/// The public key in `der`, a SubjectPublicKeyInfo of the algorithm
/// `oid` (and `curve`, for an elliptic curve key): an EC point, or RSA's
/// RSAPublicKey, as ring takes them.
fn spki<'a>(der: &'a [u8], oid: &[u8], curve: Option<&[u8]>) -> Option<&'a [u8]> {
    const SEQUENCE: u8 = 0x30;
    const OID: u8 = 0x06;
    const BIT_STRING: u8 = 0x03;
    let (SEQUENCE, info, []) = element(der)? else { return None };
    let (SEQUENCE, algorithm, rest) = element(info)? else { return None };
    let (BIT_STRING, key, []) = element(rest)? else { return None };
    let (OID, named, parameters) = element(algorithm)? else { return None };
    if named != oid {
        return None;
    }
    if let Some(curve) = curve {
        let (OID, named, []) = element(parameters)? else { return None };
        if named != curve {
            return None;
        }
    }
    // No unused bits in the last byte.
    let (&0, key) = key.split_first()? else { return None };
    Some(key)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ring::rand::SystemRandom;
    use ring::signature::{EcdsaKeyPair, Ed25519KeyPair, KeyPair};

    fn hex_of(bytes: &[u8]) -> String {
        use std::fmt::Write as _;
        bytes.iter().fold(String::new(), |mut text, b| {
            let _ = write!(text, "{b:02x}");
            text
        })
    }

    fn key(keytype: &str, scheme: &str, public: String) -> Key {
        Key { keytype: keytype.into(), scheme: scheme.into(), keyval: KeyVal { public } }
    }

    /// A P-256 public key's SubjectPublicKeyInfo in PEM.
    fn p256_pem(point: &[u8]) -> String {
        let mut der = vec![0x30, 0x59, 0x30, 0x13, 0x06, 0x07];
        der.extend_from_slice(EC_PUBLIC_KEY);
        der.extend_from_slice(&[0x06, 0x08]);
        der.extend_from_slice(P256);
        der.extend_from_slice(&[0x03, 0x42, 0x00]);
        der.extend_from_slice(point);
        let body = base64::engine::general_purpose::STANDARD.encode(der);
        format!(
            "-----BEGIN PUBLIC KEY-----\n{}\n{}\n-----END PUBLIC KEY-----\n",
            &body[..64],
            &body[64..]
        )
    }

    #[test]
    fn ed25519_and_p256_signatures_are_checked() {
        let rng = SystemRandom::new();
        let pkcs8 = Ed25519KeyPair::generate_pkcs8(&rng).unwrap();
        let pair = Ed25519KeyPair::from_pkcs8(pkcs8.as_ref()).unwrap();
        let ed = key("ed25519", "ed25519", hex_of(pair.public_key().as_ref()));
        let sig = hex_of(pair.sign(b"signed").as_ref());
        assert!(ed.verifies(b"signed", &sig));
        assert!(!ed.verifies(b"signed!", &sig));
        assert!(!ed.verifies(b"signed", &sig[2..]));

        let algorithm = &signature::ECDSA_P256_SHA256_ASN1_SIGNING;
        let pkcs8 = EcdsaKeyPair::generate_pkcs8(algorithm, &rng).unwrap();
        let pair = EcdsaKeyPair::from_pkcs8(algorithm, pkcs8.as_ref(), &rng).unwrap();
        let sig = hex_of(pair.sign(&rng, b"signed").unwrap().as_ref());
        let pem = p256_pem(pair.public_key().as_ref());
        for keytype in ["ecdsa", "ecdsa-sha2-nistp256"] {
            let ec = key(keytype, "ecdsa-sha2-nistp256", pem.clone());
            assert!(ec.verifies(b"signed", &sig), "{keytype}");
            assert!(!ec.verifies(b"other", &sig));
        }
        // The same key under a scheme it is not, or a type not known.
        assert!(!key("ecdsa", "ecdsa-sha2-nistp384", pem.clone()).verifies(b"signed", &sig));
        assert!(!key("ml-dsa", "ml-dsa-65/1", pem).verifies(b"signed", &sig));
    }

    #[test]
    fn hex_and_der_take_only_what_they_should() {
        assert_eq!(hex("00fFa0"), Some(vec![0, 255, 160]));
        assert_eq!(hex("0"), None);
        assert_eq!(hex("zz"), None);
        assert_eq!(element(&[0x04, 0x81, 0x02, 1, 2, 3]), Some((0x04, &[1u8, 2][..], &[3u8][..])));
        assert_eq!(element(&[0x04, 0x05, 1]), None);
        assert_eq!(element(&[0x04, 0x80]), None);
        assert!(pem("-----BEGIN PRIVATE KEY-----\nAAAA\n-----END PRIVATE KEY-----").is_none());
    }
}
