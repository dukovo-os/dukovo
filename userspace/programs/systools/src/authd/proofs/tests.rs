// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The proofs' rules, with a software authenticator of the tests' own
//! for passkeys: it makes keys with ring, writes what a browser and an
//! authenticator write, and signs as one does.

use super::*;
use ring::rand::SystemRandom;
use ring::signature::{EcdsaKeyPair, Ed25519KeyPair, KeyPair as _, RsaKeyPair};

/// RFC 6238 Appendix B's SHA-1 secret.
const RFC_SECRET: &[u8] = b"12345678901234567890";

#[test]
fn app_codes_are_rfc_6238s() {
    // Appendix B's SHA-1 column, last six of its eight digits.
    for (seconds, code) in [
        (59, 287_082),
        (1_111_111_109, 81_804),
        (1_111_111_111, 50_471),
        (1_234_567_890, 5_924),
        (2_000_000_000, 279_037),
        (20_000_000_000, 353_130),
    ] {
        assert_eq!(app_code(RFC_SECRET, seconds / STEP_SECONDS), code, "at {seconds}");
    }
}

#[test]
fn an_app_code_is_taken_in_its_window_and_once() {
    let now = 1_234_567_890;
    let step = now / STEP_SECONDS;
    let code = |step: u64| format!("{:06}", app_code(RFC_SECRET, step));
    assert_eq!(check_app_code(RFC_SECRET, &code(step), now, None), Ok(step));
    assert_eq!(check_app_code(RFC_SECRET, &code(step - 1), now, None), Ok(step - 1));
    assert_eq!(
        check_app_code(RFC_SECRET, &format!(" {} ", code(step + 1)), now, None),
        Ok(step + 1)
    );
    assert_eq!(check_app_code(RFC_SECRET, &code(step - 2), now, None), Err(CodeRefusal::Wrong));
    assert_eq!(check_app_code(RFC_SECRET, &code(step + 2), now, None), Err(CodeRefusal::Wrong));
    // Taken once: not again, and no earlier step's either.
    assert_eq!(check_app_code(RFC_SECRET, &code(step), now, Some(step)), Err(CodeRefusal::Used));
    assert_eq!(
        check_app_code(RFC_SECRET, &code(step - 1), now, Some(step)),
        Err(CodeRefusal::Used)
    );
    assert_eq!(check_app_code(RFC_SECRET, &code(step + 1), now, Some(step)), Ok(step + 1));
    for typed in ["", "12345", "1234567", "12a456", "１２３４５６", "-12345"] {
        assert_eq!(
            check_app_code(RFC_SECRET, typed, now, None),
            Err(CodeRefusal::NotACode),
            "{typed}"
        );
    }
}

#[test]
fn base32_is_rfc_4648s() {
    for (plain, coded) in [
        ("", ""),
        ("f", "MY"),
        ("fo", "MZXQ"),
        ("foo", "MZXW6"),
        ("foob", "MZXW6YQ"),
        ("fooba", "MZXW6YTB"),
        ("foobar", "MZXW6YTBOI"),
    ] {
        assert_eq!(base32(plain.as_bytes()), coded);
        assert_eq!(from_base32(coded).as_deref(), Some(plain.as_bytes()));
        assert_eq!(from_base32(&coded.to_lowercase()).as_deref(), Some(plain.as_bytes()));
    }
    assert_eq!(from_base32("MZXW6YQ="), None);
    assert_eq!(from_base32("MZXW1"), None);
    // "MZ" leaves bits that are not 0.
    assert_eq!(from_base32("MZ"), None);
    let secret = [0xa5u8; SECRET_BYTES];
    assert_eq!(from_base32(&base32(&secret)).as_deref(), Some(&secret[..]));
}

#[test]
fn the_app_uri_names_the_machine_and_the_account() {
    assert_eq!(
        app_uri("duke-nas", "chase", b"12345678901234567890"),
        "otpauth://totp/Dukovo%20%28duke-nas%29:chase?secret=GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ\
         &issuer=Dukovo%20%28duke-nas%29&algorithm=SHA1&digits=6&period=30"
    );
    assert!(app_uri("h", "a b&c", b"x").contains(":a%20b%26c?"));
}

#[test]
fn recovery_codes_read_however_they_are_typed() {
    let code = recovery_code(&[0, 1, 2, 3, 4, 5, 6, 7, 8, 9]);
    assert_eq!(code, "AAAQ-EAYE-AUDA-OCAJ");
    let hash = recovery_hash(&code).unwrap();
    assert_eq!(recovery_hash("aaaq eaye auda ocaj"), Some(hash));
    assert_eq!(recovery_hash("AAAQEAYEAUDAOCAJ"), Some(hash));
    assert_ne!(recovery_hash("AAAQ-EAYE-AUDA-OCAK"), Some(hash));
    for typed in [
        "",
        "AAAQ-EAYE-AUDA-OCA",
        "AAAQ-EAYE-AUDA-OCAJ1",
        "AAAQ-EAYE-AUDA-OCA1",
        "AAAQ_EAYE_AUDA_OCAJ",
    ] {
        assert_eq!(recovery_hash(typed), None, "{typed}");
    }
}

#[test]
fn origins_and_relying_parties() {
    assert_eq!(origin_host("https://duke-nas.local:9090"), Some("duke-nas.local"));
    assert_eq!(origin_host("https://example.org"), Some("example.org"));
    for origin in [
        "http://duke-nas.local:9090",
        "https://duke-nas.local:9090/",
        "https://evil.example/duke-nas.local",
        "https://user@duke-nas.local",
        "https://duke-nas.local:",
        "https://duke-nas.local:90a",
        "https://",
    ] {
        assert_eq!(origin_host(origin), None, "{origin}");
    }
    assert!(host_may_use("duke-nas.local", "duke-nas.local"));
    assert!(host_may_use("console.example.org", "example.org"));
    assert!(!host_may_use("badexample.org", "example.org"));
    assert!(!host_may_use("example.org", "console.example.org"));
    assert!(!host_may_use("example.org", ""));
}

// A software authenticator.

/// What the tests' authenticator and browser use.
pub(crate) const ORIGIN: &str = "https://duke-nas.local:9090";
pub(crate) const PARTY: &str = "duke-nas.local";
pub(crate) const CHALLENGE: [u8; 32] = [7; 32];

/// The three kinds of key, each made once.
pub(crate) enum Signer {
    P256(EcdsaKeyPair),
    Ed25519(Ed25519KeyPair),
    Rsa(RsaKeyPair),
}

impl Signer {
    pub(crate) fn new(algorithm: i64) -> Signer {
        let random = SystemRandom::new();
        match algorithm {
            ES256 => {
                let alg = &signature::ECDSA_P256_SHA256_ASN1_SIGNING;
                let pkcs8 = EcdsaKeyPair::generate_pkcs8(alg, &random).unwrap();
                Signer::P256(EcdsaKeyPair::from_pkcs8(alg, pkcs8.as_ref(), &random).unwrap())
            }
            EDDSA => {
                let pkcs8 = Ed25519KeyPair::generate_pkcs8(&random).unwrap();
                Signer::Ed25519(Ed25519KeyPair::from_pkcs8(pkcs8.as_ref()).unwrap())
            }
            _ => Signer::Rsa(
                RsaKeyPair::from_pkcs8(include_bytes!("fixtures/rsa-2048.pk8")).unwrap(),
            ),
        }
    }

    /// The COSE_Key of its public key.
    pub(crate) fn cose(&self) -> Vec<u8> {
        match self {
            Signer::P256(pair) => {
                let point = pair.public_key().as_ref();
                map(&[
                    (int(1), int(2)),
                    (int(3), int(ES256)),
                    (int(-1), int(1)),
                    (int(-2), bytes(&point[1..33])),
                    (int(-3), bytes(&point[33..])),
                ])
            }
            Signer::Ed25519(pair) => map(&[
                (int(1), int(1)),
                (int(3), int(EDDSA)),
                (int(-1), int(6)),
                (int(-2), bytes(pair.public_key().as_ref())),
            ]),
            Signer::Rsa(pair) => {
                let public = signature::RsaPublicKeyComponents::<Vec<u8>>::from(pair.public());
                map(&[
                    (int(1), int(3)),
                    (int(3), int(RS256)),
                    (int(-1), bytes(&public.n)),
                    (int(-2), bytes(&public.e)),
                ])
            }
        }
    }

    pub(crate) fn sign(&self, message: &[u8]) -> Vec<u8> {
        let random = SystemRandom::new();
        match self {
            Signer::P256(pair) => pair.sign(&random, message).unwrap().as_ref().to_vec(),
            Signer::Ed25519(pair) => pair.sign(message).as_ref().to_vec(),
            Signer::Rsa(pair) => {
                let mut out = vec![0; pair.public().modulus_len()];
                pair.sign(&signature::RSA_PKCS1_SHA256, &random, message, &mut out).unwrap();
                out
            }
        }
    }
}

// A CBOR writer, for the tests' authenticator.

fn head(major: u8, value: u64) -> Vec<u8> {
    let major = major << 5;
    match value {
        0..=23 => vec![major | value as u8],
        24..=0xff => vec![major | 0x18, value as u8],
        0x100..=0xffff => [vec![major | 0x19], (value as u16).to_be_bytes().to_vec()].concat(),
        0x1_0000..=0xffff_ffff => {
            [vec![major | 0x1a], (value as u32).to_be_bytes().to_vec()].concat()
        }
        _ => [vec![major | 0x1b], value.to_be_bytes().to_vec()].concat(),
    }
}

fn int(value: i64) -> Vec<u8> {
    if value >= 0 { head(0, value as u64) } else { head(1, (-1 - value) as u64) }
}

fn bytes(value: &[u8]) -> Vec<u8> {
    [head(2, value.len() as u64), value.to_vec()].concat()
}

fn text(value: &str) -> Vec<u8> {
    [head(3, value.len() as u64), value.as_bytes().to_vec()].concat()
}

fn map(pairs: &[(Vec<u8>, Vec<u8>)]) -> Vec<u8> {
    let mut out = head(5, pairs.len() as u64);
    for (key, value) in pairs {
        out.extend_from_slice(key);
        out.extend_from_slice(value);
    }
    out
}

pub(crate) fn client_data(kind: &str, challenge: &[u8], origin: &str) -> Vec<u8> {
    format!(
        r#"{{"type":"{kind}","challenge":"{}","origin":"{origin}","crossOrigin":false}}"#,
        URL_SAFE_NO_PAD.encode(challenge)
    )
    .into_bytes()
}

fn party_hash(party: &str) -> Vec<u8> {
    digest::digest(&digest::SHA256, party.as_bytes()).as_ref().to_vec()
}

/// Authenticator data with `flags` and `count`, and `attested` after.
pub(crate) fn authenticator_data(party: &str, flags: u8, count: u32, attested: &[u8]) -> Vec<u8> {
    [party_hash(party), vec![flags], count.to_be_bytes().to_vec(), attested.to_vec()].concat()
}

pub(crate) const ID: &[u8] = b"credential-1";

pub(crate) fn attested(cose: &[u8]) -> Vec<u8> {
    [vec![0; 16], (ID.len() as u16).to_be_bytes().to_vec(), ID.to_vec(), cose.to_vec()].concat()
}

pub(crate) fn attestation(data: &[u8]) -> Vec<u8> {
    map(&[
        (text("fmt"), text("none")),
        (text("attStmt"), map(&[])),
        (text("authData"), bytes(data)),
    ])
}

fn creating() -> Expected<'static> {
    Expected {
        ceremony: Ceremony::Create,
        challenge: &CHALLENGE,
        origin: ORIGIN,
        relying_party: PARTY,
        verified: true,
    }
}

fn getting() -> Expected<'static> {
    Expected { ceremony: Ceremony::Get, ..creating() }
}

pub(crate) const UP_UV: u8 = USER_PRESENT | USER_VERIFIED;

/// Adds a passkey of `algorithm` and returns it, with its signer.
fn added(algorithm: i64) -> (Signer, NewPasskey) {
    let signer = Signer::new(algorithm);
    let data = authenticator_data(PARTY, UP_UV | ATTESTED, 0, &attested(&signer.cose()));
    let new = check_registration(
        &creating(),
        &client_data("webauthn.create", &CHALLENGE, ORIGIN),
        &attestation(&data),
    )
    .unwrap();
    (signer, new)
}

/// A use of `signer`'s passkey: the client data, the authenticator data
/// and the signature.
fn used(signer: &Signer, flags: u8, count: u32) -> (Vec<u8>, Vec<u8>, Vec<u8>) {
    let client = client_data("webauthn.get", &CHALLENGE, ORIGIN);
    let data = authenticator_data(PARTY, flags, count, &[]);
    let hash = digest::digest(&digest::SHA256, &client);
    let signed = signer.sign(&[data.clone(), hash.as_ref().to_vec()].concat());
    (client, data, signed)
}

#[test]
fn each_kind_of_passkey_is_added_and_used() {
    for algorithm in ALGORITHMS {
        let (signer, new) = added(algorithm);
        assert_eq!(new.id, ID);
        assert_eq!(new.algorithm, algorithm);
        assert_eq!(new.key, signer.cose());
        let (client, data, signed) = used(&signer, UP_UV, 5);
        let use_ = check_assertion(&getting(), &client, &data, &signed, &new.key, 4);
        assert_eq!(use_, Ok(Used { count: 5, verified: true }), "alg {algorithm}");
        // A passkey that keeps no count gives 0 each time.
        let (client, data, signed) = used(&signer, UP_UV, 0);
        assert!(check_assertion(&getting(), &client, &data, &signed, &new.key, 0).is_ok());
    }
}

#[test]
fn a_count_that_does_not_rise_is_refused() {
    let (signer, new) = added(ES256);
    for (stored, given) in [(5, 5), (5, 4), (5, 0)] {
        let (client, data, signed) = used(&signer, UP_UV, given);
        let err =
            check_assertion(&getting(), &client, &data, &signed, &new.key, stored).unwrap_err();
        assert!(err.contains("may have been copied"), "{stored} {given}: {err}");
    }
}

#[test]
fn a_use_is_refused_for_each_thing_wrong_with_it() {
    let (signer, new) = added(ES256);
    let (client, data, signed) = used(&signer, UP_UV, 1);
    let check = |expected: &Expected<'_>, client: &[u8], data: &[u8], signed: &[u8]| {
        check_assertion(expected, client, data, signed, &new.key, 0)
    };
    assert!(check(&getting(), &client, &data, &signed).is_ok());

    // The client data's type, challenge, origin, and frames.
    let re_sign = |client: &[u8], data: &[u8]| {
        let hash = digest::digest(&digest::SHA256, client);
        signer.sign(&[data.to_vec(), hash.as_ref().to_vec()].concat())
    };
    for (client, why) in [
        (client_data("webauthn.create", &CHALLENGE, ORIGIN), "is for webauthn.create"),
        (client_data("webauthn.get", &[8; 32], ORIGIN), "another challenge"),
        (client_data("webauthn.get", &CHALLENGE, "https://evil.example"), "was used on https://evil.example"),
        (client_data("webauthn.get", &CHALLENGE, "https://duke-nas.local:9091"), "was used on"),
        (
            format!(
                r#"{{"type":"webauthn.get","challenge":"{}","origin":"{ORIGIN}","crossOrigin":true}}"#,
                URL_SAFE_NO_PAD.encode(CHALLENGE)
            )
            .into_bytes(),
            "inside another site",
        ),
        (
            format!(
                r#"{{"type":"webauthn.get","challenge":"{}","origin":"{ORIGIN}","topOrigin":"https://a.example"}}"#,
                URL_SAFE_NO_PAD.encode(CHALLENGE)
            )
            .into_bytes(),
            "inside another site",
        ),
        (b"{\"type\":\"webauthn.get\"".to_vec(), "does not read"),
        (
            format!(
                r#"{{"type":"webauthn.get","type":"webauthn.get","challenge":"{}","origin":"{ORIGIN}"}}"#,
                URL_SAFE_NO_PAD.encode(CHALLENGE)
            )
            .into_bytes(),
            "does not read",
        ),
        (vec![b' '; CLIENT_DATA_MAX + 1], "longer than 4 KiB"),
    ] {
        let signed = re_sign(&client, &data);
        let err = check(&getting(), &client, &data, &signed).unwrap_err();
        assert!(err.contains(why), "{why}: {err}");
    }

    // The authenticator data.
    for (data, why) in [
        (authenticator_data("evil.example", UP_UV, 1, &[]), "not one for duke-nas.local"),
        (authenticator_data(PARTY, USER_VERIFIED, 1, &[]), "no one touched it"),
        (authenticator_data(PARTY, USER_PRESENT, 1, &[]), "did not check that it is you"),
        (authenticator_data(PARTY, UP_UV, 1, &[0]), "bytes after its end"),
        (
            authenticator_data(PARTY, UP_UV | ATTESTED, 1, &attested(&signer.cose())),
            "a key where none belongs",
        ),
        (authenticator_data(PARTY, UP_UV | EXTENSIONS, 1, &[]), "extensions do not read"),
        (authenticator_data(PARTY, UP_UV, 1, &[])[..36].to_vec(), "cut short"),
    ] {
        let signed = re_sign(&client, &data);
        let err = check(&getting(), &client, &data, &signed).unwrap_err();
        assert!(err.contains(why), "{why}: {err}");
    }
    // Extensions that read are skipped.
    let data =
        authenticator_data(PARTY, UP_UV | EXTENSIONS, 1, &map(&[(text("credProtect"), int(2))]));
    let signed = re_sign(&client, &data);
    assert!(check(&getting(), &client, &data, &signed).is_ok());
    // Without verification asked for, presence is enough.
    let data = authenticator_data(PARTY, USER_PRESENT, 1, &[]);
    let signed = re_sign(&client, &data);
    let present = Expected { verified: false, ..getting() };
    assert_eq!(check(&present, &client, &data, &signed), Ok(Used { count: 1, verified: false }));

    // The signature: one bit of it, of the data or of the client data.
    let mut bad = signed.clone();
    bad[10] ^= 1;
    assert!(check(&present, &client, &data, &bad).unwrap_err().contains("not its key's"));
    let mut other = data.clone();
    other[36] ^= 1;
    assert!(check(&present, &client, &other, &signed).unwrap_err().contains("not its key's"));
    let (_, _, theirs) = used(&Signer::new(ES256), USER_PRESENT, 1);
    assert!(check(&present, &client, &data, &theirs).unwrap_err().contains("not its key's"));
    // A registration's expectations are not a use's.
    assert!(check(&creating(), &client, &data, &signed).is_err());
}

#[test]
fn a_registration_is_refused_for_each_thing_wrong_with_it() {
    let signer = Signer::new(EDDSA);
    let client = client_data("webauthn.create", &CHALLENGE, ORIGIN);
    let good = authenticator_data(PARTY, UP_UV | ATTESTED, 0, &attested(&signer.cose()));
    assert!(check_registration(&creating(), &client, &attestation(&good)).is_ok());
    let fmt = (text("fmt"), text("none"));
    let statement = (text("attStmt"), map(&[]));
    let data = (text("authData"), bytes(&good));
    for (object, why) in [
        (map(&[fmt.clone(), statement.clone()]), "lacks a field"),
        (
            map(&[fmt.clone(), statement.clone(), data.clone(), (text("x"), int(1))]),
            "a field WebAuthn does not",
        ),
        (map(&[fmt.clone(), fmt.clone(), statement.clone(), data.clone()]), "a field twice"),
        (map(&[(text("fmt"), int(1)), statement.clone(), data.clone()]), "the wrong kind"),
        ([attestation(&good), vec![0]].concat(), "does not read"),
        (text("none"), "not a map"),
    ] {
        let err = check_registration(&creating(), &client, &object).unwrap_err();
        assert!(err.contains(why), "{why}: {err}");
    }
    let no_key = authenticator_data(PARTY, UP_UV, 0, &[]);
    assert!(
        check_registration(&creating(), &client, &attestation(&no_key))
            .unwrap_err()
            .contains("holds no key")
    );
    let long_id =
        [vec![0; 16], 1024u16.to_be_bytes().to_vec(), vec![1; 1024], signer.cose()].concat();
    let data = authenticator_data(PARTY, UP_UV | ATTESTED, 0, &long_id);
    assert!(
        check_registration(&creating(), &client, &attestation(&data))
            .unwrap_err()
            .contains("1024 bytes")
    );
    let cut = attested(&signer.cose());
    let data = authenticator_data(PARTY, UP_UV | ATTESTED, 0, &cut[..cut.len() - 1]);
    assert!(
        check_registration(&creating(), &client, &attestation(&data))
            .unwrap_err()
            .contains("does not read")
    );
    let get = client_data("webauthn.get", &CHALLENGE, ORIGIN);
    assert!(
        check_registration(&creating(), &get, &attestation(&good))
            .unwrap_err()
            .contains("webauthn.get")
    );
}

#[test]
fn only_three_kinds_of_key_are_taken() {
    let key = |pairs: &[(Vec<u8>, Vec<u8>)]| read_key(&map(pairs));
    let x = bytes(&[1; 32]);
    // ES384, ES256 on P-384, Ed448, PS256: refused by name.
    for pairs in [
        vec![
            (int(1), int(2)),
            (int(3), int(-35)),
            (int(-1), int(2)),
            (int(-2), x.clone()),
            (int(-3), x.clone()),
        ],
        vec![
            (int(1), int(2)),
            (int(3), int(ES256)),
            (int(-1), int(2)),
            (int(-2), x.clone()),
            (int(-3), x.clone()),
        ],
        vec![(int(1), int(1)), (int(3), int(EDDSA)), (int(-1), int(7)), (int(-2), x.clone())],
        vec![
            (int(1), int(3)),
            (int(3), int(-37)),
            (int(-1), bytes(&[1; 256])),
            (int(-2), bytes(&[1, 0, 1])),
        ],
    ] {
        assert!(key(&pairs).unwrap_err().contains("does not check"), "{pairs:?}");
    }
    let short = bytes(&[1; 31]);
    assert!(
        key(&[
            (int(1), int(2)),
            (int(3), int(ES256)),
            (int(-1), int(1)),
            (int(-2), short.clone()),
            (int(-3), x.clone())
        ])
        .unwrap_err()
        .contains("32 bytes")
    );
    assert!(
        key(&[(int(1), int(1)), (int(3), int(EDDSA)), (int(-1), int(6)), (int(-2), short)])
            .unwrap_err()
            .contains("32 bytes")
    );
    // RSA of 1024 bits, and leading zeros that hide a short modulus.
    let small = bytes(&[0xc1; 128]);
    assert!(
        key(&[
            (int(1), int(3)),
            (int(3), int(RS256)),
            (int(-1), small),
            (int(-2), bytes(&[1, 0, 1]))
        ])
        .unwrap_err()
        .contains("1024 bits")
    );
    let padded = bytes(&[vec![0; 130], vec![0xc1; 126]].concat());
    assert!(
        key(&[
            (int(1), int(3)),
            (int(3), int(RS256)),
            (int(-1), padded),
            (int(-2), bytes(&[1, 0, 1]))
        ])
        .unwrap_err()
        .contains("1008 bits")
    );
    assert!(
        key(&[
            (int(1), int(3)),
            (int(3), int(RS256)),
            (int(-1), bytes(&[0xc1; 256])),
            (int(-2), bytes(&[1; 4]))
        ])
        .unwrap_err()
        .contains("exponent")
    );
    // A label twice, a missing one, and not a map.
    assert!(
        key(&[
            (int(1), int(1)),
            (int(1), int(1)),
            (int(3), int(EDDSA)),
            (int(-1), int(6)),
            (int(-2), x.clone())
        ])
        .unwrap_err()
        .contains("twice")
    );
    assert!(
        key(&[(int(1), int(1)), (int(-1), int(6)), (int(-2), x)])
            .unwrap_err()
            .contains("algorithm")
    );
    assert!(read_key(&int(1)).unwrap_err().contains("not a COSE key"));
}
