// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Reading certificates (ADR 0512): the PEM blocks of a file, what a
//! certificate says of itself (its names, dates, issuer, serial,
//! fingerprint and key), and whether the system's roots trust a chain.
//!
//! `cert list` and `cert show` read only what `fullchain.pem` holds, so
//! that nothing is stored twice; `cert add` runs its checks on the same
//! values. When a chain is refused for a reason that is not plain, the
//! words in [`refusal`] are the ones to look at.

use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use ring::digest;
use rustls::RootCertStore;
use rustls::crypto::ring::sign::any_supported_type;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, UnixTime};
use rustls::sign::CertifiedKey;
use rustls::{Error as TlsError, InconsistentKeys};
use webpki::{EndEntityCert, KeyUsage};
use x509_cert::Certificate;
use x509_cert::der::asn1::UintRef;
use x509_cert::der::{Decode, Reader, SliceReader};
use x509_cert::ext::pkix::name::GeneralName;
use x509_cert::ext::pkix::{BasicConstraints, SubjectAltName};

use crate::tls;
use crate::util;

/// The label of a certificate's PEM block.
pub const CERTIFICATE: &str = "CERTIFICATE";

/// One block of a PEM file: its label (`CERTIFICATE`, `PRIVATE KEY`) and
/// what it holds.
#[derive(Debug, PartialEq, Eq)]
pub struct Block {
    pub label: String,
    pub der: Vec<u8>,
}

/// The blocks of PEM `text`, which holds nothing else but whitespace.
///
/// # Errors
///
/// With the line that is not PEM, a block that is not closed, or one
/// whose base64 is damaged or that has header lines, as an encrypted
/// block has.
pub fn pem_blocks(text: &str) -> Result<Vec<Block>, String> {
    let mut blocks = Vec::new();
    let mut open: Option<(String, String)> = None;
    for (number, line) in text.lines().enumerate() {
        let line = line.trim();
        let Some((label, body)) = open.as_mut() else {
            if line.is_empty() {
                continue;
            }
            let label = line.strip_prefix("-----BEGIN ").and_then(|l| l.strip_suffix("-----"));
            let Some(label) = label else {
                return Err(format!(
                    "line {} is not PEM: it is not inside a -----BEGIN block",
                    number + 1
                ));
            };
            open = Some((label.to_string(), String::new()));
            continue;
        };
        if let Some(end) = line.strip_prefix("-----END ").and_then(|l| l.strip_suffix("-----")) {
            if end != label {
                return Err(format!("line {}: the block {label} ends as {end}", number + 1));
            }
            let der =
                STANDARD.decode(body.as_bytes()).ok().filter(|der| !der.is_empty()).ok_or_else(
                    || format!("the {label} block ending at line {} is not base64", number + 1),
                )?;
            blocks.push(Block { label: std::mem::take(label), der });
            open = None;
        } else if line.contains(':') {
            return Err(format!(
                "line {}: the {label} block has header lines, as an encrypted block has",
                number + 1
            ));
        } else {
            body.push_str(line);
        }
    }
    match open {
        Some((label, _)) => Err(format!("the {label} block is not closed by an -----END line")),
        None => Ok(blocks),
    }
}

/// `der` as a PEM block labelled `label`, with 64 characters to a line.
pub fn pem_encode(label: &str, der: &[u8]) -> String {
    let body = STANDARD.encode(der);
    let mut text = format!("-----BEGIN {label}-----\n");
    for line in body.as_bytes().chunks(64) {
        text.push_str(&String::from_utf8_lossy(line));
        text.push('\n');
    }
    text.push_str(&format!("-----END {label}-----\n"));
    text
}

/// The certificates of a chain file.
///
/// # Errors
///
/// If the file is not PEM, or holds anything but certificates.
pub fn chain_of(text: &str) -> Result<Vec<CertificateDer<'static>>, String> {
    pem_blocks(text)?
        .into_iter()
        .map(|block| match block.label.as_str() {
            CERTIFICATE => Ok(CertificateDer::from(block.der)),
            other => Err(format!("it holds a {other} block, and certificates alone belong in it")),
        })
        .collect()
}

/// The private key in `block`, whichever of the three encodings it is in.
///
/// # Errors
///
/// If the block holds no private key, or an encrypted one.
pub fn private_key(block: &Block) -> Result<PrivateKeyDer<'static>, String> {
    let der = block.der.clone();
    match block.label.as_str() {
        "PRIVATE KEY" => Ok(PrivateKeyDer::Pkcs8(der.into())),
        "RSA PRIVATE KEY" => Ok(PrivateKeyDer::Pkcs1(der.into())),
        "EC PRIVATE KEY" => Ok(PrivateKeyDer::Sec1(der.into())),
        "ENCRYPTED PRIVATE KEY" => Err(String::from(
            "the key is encrypted: take its passphrase off first, as `openssl pkey -in KEY -out PLAIN` does",
        )),
        other => Err(format!("a {other} block is no private key")),
    }
}

/// Checks that `key` is one the system can sign with, and the first
/// certificate of `chain`'s.
///
/// # Errors
///
/// If it is of a kind the system cannot sign with (RSA, ECDSA and Ed25519
/// are), or belongs to another certificate.
pub fn key_fits(
    chain: &[CertificateDer<'static>],
    key: &PrivateKeyDer<'static>,
) -> Result<(), String> {
    let signer = any_supported_type(key).map_err(|err| {
        format!(
            "the key is not one the system can sign with (RSA, ECDSA and Ed25519 keys are): {err}"
        )
    })?;
    // ring's keys of all three kinds give their public key, so a key that
    // cannot be checked is refused rather than taken on trust.
    match CertifiedKey::new(chain.to_vec(), signer).keys_match() {
        Ok(()) => Ok(()),
        Err(TlsError::InconsistentKeys(InconsistentKeys::KeyMismatch)) => {
            Err(String::from("the key is not the certificate's"))
        }
        Err(err) => Err(format!("the key cannot be checked against the certificate: {err}")),
    }
}

/// What a certificate says of itself.
#[derive(Clone, Debug)]
pub struct Details {
    /// Its subject and issuer, as RFC 4514 writes names.
    pub subject: String,
    pub issuer: String,
    /// The DNS and IP names it is valid for, as it lists them.
    pub names: Vec<String>,
    /// When it starts and ends, in seconds since 1970.
    pub not_before: i64,
    pub not_after: i64,
    /// The serial number, in bytes as hex with colons.
    pub serial: String,
    /// The SHA-256 of its DER encoding.
    pub fingerprint: [u8; 32],
    /// Its public key's type, as `ECDSA P-256`.
    pub key: String,
    /// Whether it says it is a certificate authority's.
    pub authority: bool,
    /// Whether its subject is its issuer.
    pub self_issued: bool,
}

impl Details {
    /// The fingerprint as lowercase hex, `openssl`'s without colons.
    pub fn fingerprint_hex(&self) -> String {
        hex(&self.fingerprint)
    }

    /// The fingerprint as a browser shows it, `AB:CD:...`.
    pub fn fingerprint_colons(&self) -> String {
        colons(&self.fingerprint)
    }

    /// The whole days from `now` until it ends; negative once it has.
    pub fn days_left(&self, now: i64) -> i64 {
        (self.not_after - now).div_euclid(86_400)
    }
}

/// What `der` says of its certificate.
///
/// # Errors
///
/// If it is not a certificate.
pub fn details(der: &CertificateDer<'_>) -> Result<Details, String> {
    let certificate = Certificate::from_der(der.as_ref())
        .map_err(|err| format!("a certificate in it does not read: {err}"))?;
    let tbs = certificate.tbs_certificate();
    let validity = tbs.validity();
    let seconds = |time: x509_cert::time::Time| {
        i64::try_from(time.to_unix_duration().as_secs()).unwrap_or(i64::MAX)
    };
    let names = match tbs.get_extension::<SubjectAltName>() {
        Ok(Some((_, SubjectAltName(names)))) => names.iter().filter_map(name_text).collect(),
        Ok(None) => Vec::new(),
        Err(err) => return Err(format!("its subject alternative names do not read: {err}")),
    };
    let authority = match tbs.get_extension::<BasicConstraints>() {
        Ok(found) => found.is_some_and(|(_, constraints)| constraints.ca),
        Err(err) => return Err(format!("its basic constraints do not read: {err}")),
    };
    Ok(Details {
        subject: tbs.subject().to_string(),
        issuer: tbs.issuer().to_string(),
        names,
        not_before: seconds(validity.not_before),
        not_after: seconds(validity.not_after),
        serial: colons(tbs.serial_number().as_bytes()),
        fingerprint: fingerprint(der.as_ref()),
        key: key_type(tbs.subject_public_key_info()),
        authority,
        self_issued: tbs.subject() == tbs.issuer(),
    })
}

/// The SHA-256 of `der`.
pub fn fingerprint(der: &[u8]) -> [u8; 32] {
    let mut sum = [0u8; 32];
    sum.copy_from_slice(digest::digest(&digest::SHA256, der).as_ref());
    sum
}

/// A DNS name or an IP address of a subject alternative name; the other
/// kinds are not names a client connects to.
fn name_text(name: &GeneralName) -> Option<String> {
    match name {
        GeneralName::DnsName(dns) => Some(dns.to_string()),
        GeneralName::IpAddress(address) => {
            let bytes = address.as_bytes();
            if let Ok(v4) = <[u8; 4]>::try_from(bytes) {
                Some(std::net::Ipv4Addr::from(v4).to_string())
            } else {
                <[u8; 16]>::try_from(bytes).ok().map(|v6| std::net::Ipv6Addr::from(v6).to_string())
            }
        }
        _ => None,
    }
}

/// The type of a public key: `RSA 2048`, `ECDSA P-256`, `Ed25519`, or
/// its algorithm's number.
fn key_type(info: &x509_cert::spki::SubjectPublicKeyInfoOwned) -> String {
    let algorithm = info.algorithm.oid.to_string();
    match algorithm.as_str() {
        "1.2.840.113549.1.1.1" => match info.subject_public_key.as_bytes().and_then(rsa_bits) {
            Some(bits) => format!("RSA {bits}"),
            None => String::from("RSA"),
        },
        "1.2.840.10045.2.1" => {
            let curve = info
                .algorithm
                .parameters
                .as_ref()
                .and_then(|any| any.decode_as::<x509_cert::der::asn1::ObjectIdentifier>().ok())
                .map(|oid| oid.to_string());
            match curve.as_deref() {
                Some("1.2.840.10045.3.1.7") => String::from("ECDSA P-256"),
                Some("1.3.132.0.34") => String::from("ECDSA P-384"),
                Some("1.3.132.0.35") => String::from("ECDSA P-521"),
                Some(other) => format!("ECDSA on curve {other}"),
                None => String::from("ECDSA"),
            }
        }
        "1.3.101.112" => String::from("Ed25519"),
        "1.3.101.113" => String::from("Ed448"),
        other => format!("a key of algorithm {other}"),
    }
}

/// The bits of the modulus in an RSA public key's DER.
fn rsa_bits(key: &[u8]) -> Option<usize> {
    let mut reader = SliceReader::new(key).ok()?;
    let modulus = reader
        .sequence(|sequence| {
            let modulus = UintRef::decode(sequence)?.as_bytes().to_vec();
            let _exponent = UintRef::decode(sequence)?;
            Ok::<_, x509_cert::der::Error>(modulus)
        })
        .ok()?;
    let first = *modulus.first()?;
    Some(modulus.len() * 8 - first.leading_zeros() as usize)
}

/// `bytes` as lowercase hex.
pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect::<Vec<String>>().concat()
}

/// `bytes` as uppercase hex pairs with colons, as browsers show them.
pub fn colons(bytes: &[u8]) -> String {
    let pairs: Vec<String> = bytes.iter().map(|byte| format!("{byte:02X}")).collect();
    pairs.join(":")
}

/// Whether the system's roots trust a chain.
#[derive(Debug, PartialEq, Eq)]
pub enum Trust {
    /// The chain ends at a root the system trusts.
    Trusted,
    /// One certificate that its own key signed: browsers warn.
    SelfSigned,
    /// The chain stops at this issuer, which the system's roots do not
    /// lead to.
    UnknownIssuer(String),
    /// Refused for another reason, in words.
    Refused(String),
}

impl Trust {
    /// Why the system's roots do not trust the chain, or none if they do.
    pub fn why_not(&self) -> Option<String> {
        match self {
            Trust::Trusted => None,
            Trust::SelfSigned => {
                Some(String::from("self-signed: browsers warn unless told to trust it"))
            }
            Trust::UnknownIssuer(issuer) => Some(format!(
                "the chain stops at {issuer}, which no authority Dukovo trusts signed"
            )),
            Trust::Refused(why) => Some(why.clone()),
        }
    }
}

/// Checks `chain`, the certificate first, against `roots` as a server's
/// at time `now` (seconds since 1970), with the chain's other
/// certificates as intermediates.
pub fn trust(chain: &[CertificateDer<'static>], roots: &RootCertStore, now: i64) -> Trust {
    let Some(first) = chain.first() else {
        return Trust::Refused(String::from("there is no certificate to check"));
    };
    let Ok(end) = EndEntityCert::try_from(first) else {
        return Trust::Refused(String::from("its first certificate does not read"));
    };
    if chain.len() == 1 && details(first).is_ok_and(|first| first.self_issued) {
        return Trust::SelfSigned;
    }
    let time = UnixTime::since_unix_epoch(Duration::from_secs(u64::try_from(now).unwrap_or(0)));
    let algorithms = tls::provider().signature_verification_algorithms.all;
    let verified = end.verify_for_usage(
        algorithms,
        &roots.roots,
        &chain[1..],
        time,
        KeyUsage::server_auth(),
        None,
        None,
    );
    match verified {
        Ok(_) => Trust::Trusted,
        Err(webpki::Error::UnknownIssuer) => {
            let top = chain.last().and_then(|top| details(top).ok());
            Trust::UnknownIssuer(
                top.map_or_else(|| String::from("an unknown issuer"), |t| t.issuer),
            )
        }
        Err(other) => Trust::Refused(refusal(&other)),
    }
}

/// webpki's refusal `error` in plain words.
fn refusal(error: &webpki::Error) -> String {
    match error {
        webpki::Error::CertExpired { not_after, .. } => format!(
            "a certificate of the chain ran out on {}",
            util::time_text(i64::try_from(not_after.as_secs()).unwrap_or(0), "%F", true)
        ),
        webpki::Error::CertNotValidYet { not_before, .. } => format!(
            "a certificate of the chain starts on {}",
            util::time_text(i64::try_from(not_before.as_secs()).unwrap_or(0), "%F", true)
        ),
        webpki::Error::CaUsedAsEndEntity => {
            String::from("its first certificate is an authority's, not a server's")
        }
        webpki::Error::RequiredEkuNotFoundContext(_) => {
            String::from("it is not for serving: its extended key usage lacks serverAuth")
        }
        webpki::Error::InvalidSignatureForPublicKey => {
            String::from("a certificate of the chain was not signed by the one after it")
        }
        webpki::Error::CertRevoked | webpki::Error::CertDistrusted => String::from(
            "an authority the system trusts has withdrawn trust in a certificate of the chain",
        ),
        other => format!("the chain does not check: {other}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cert::fixtures::{
        self, DAY, ECDSA, ED25519, KNOWN_END, KNOWN_START, NOW, RSA, Server,
    };

    fn known(known: &fixtures::Known) -> Details {
        let chain = chain_of(known.pem).unwrap();
        details(&chain[0]).unwrap()
    }

    #[test]
    fn the_fingerprint_is_the_sha_256_openssl_prints() {
        for known_one in [&RSA, &ECDSA, &ED25519] {
            assert_eq!(known(known_one).fingerprint_colons(), known_one.fingerprint);
        }
        let ecdsa = known(&ECDSA);
        assert_eq!(ecdsa.fingerprint_hex(), ECDSA.fingerprint.replace(':', "").to_lowercase());
        assert_eq!(ecdsa.fingerprint_hex().len(), 64);
    }

    #[test]
    fn rsa_ecdsa_and_ed25519_keys_are_named() {
        for known_one in [&RSA, &ECDSA, &ED25519] {
            assert_eq!(known(known_one).key, known_one.key);
        }
    }

    #[test]
    fn names_dates_serial_and_issuer_are_read_from_the_certificate() {
        let ecdsa = known(&ECDSA);
        assert_eq!(ecdsa.names, ["ec.example", "www.ec.example", "192.0.2.7", "2001:db8::1"]);
        assert_eq!((ecdsa.not_before, ecdsa.not_after), (KNOWN_START, KNOWN_END));
        assert_eq!(ecdsa.serial, ECDSA.serial);
        assert_eq!(ecdsa.subject, "CN=ec.example");
        assert_eq!(ecdsa.issuer, "CN=ec.example");
        assert!(ecdsa.self_issued);
        assert!(!ecdsa.authority);
    }

    #[test]
    fn days_left_counts_whole_days_and_goes_negative_after() {
        let ecdsa = known(&ECDSA);
        assert_eq!(ecdsa.days_left(KNOWN_END - 3 * DAY - 5), 3);
        assert_eq!(ecdsa.days_left(KNOWN_END - 1), 0);
        assert_eq!(ecdsa.days_left(KNOWN_END + 1), -1);
    }

    #[test]
    fn a_certificate_that_is_an_authority_says_so() {
        let root = fixtures::root("Test Root");
        let details = details(root.der()).unwrap();
        assert!(details.authority && details.self_issued);
        assert_eq!(details.subject, "CN=Test Root");
    }

    #[test]
    fn pem_blocks_hold_what_is_between_the_lines_and_whitespace_alone() {
        let text = format!(
            "\n  {}\n\n{}",
            pem_encode("CERTIFICATE", b"first der"),
            pem_encode("PRIVATE KEY", b"second der")
        );
        let blocks = pem_blocks(&text).unwrap();
        assert_eq!(
            blocks,
            [
                Block { label: "CERTIFICATE".into(), der: b"first der".to_vec() },
                Block { label: "PRIVATE KEY".into(), der: b"second der".to_vec() },
            ]
        );
        assert_eq!(pem_blocks("").unwrap(), []);
    }

    #[test]
    fn pem_lines_are_64_characters_and_end_with_a_newline() {
        let text = pem_encode("CERTIFICATE", &[7u8; 100]);
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines[0], "-----BEGIN CERTIFICATE-----");
        assert_eq!(lines[1].len(), 64);
        assert_eq!(lines[lines.len() - 1], "-----END CERTIFICATE-----");
        assert!(text.ends_with('\n') && !text.contains('\r'));
    }

    #[test]
    fn text_outside_the_blocks_is_refused() {
        let block = pem_encode("CERTIFICATE", b"der");
        assert!(pem_blocks(&format!("{block}trailing junk\n")).unwrap_err().contains("line 4"));
        assert!(pem_blocks(&format!("Bag Attributes\n{block}")).unwrap_err().contains("line 1"));
    }

    #[test]
    fn a_block_that_does_not_close_or_decode_is_refused() {
        assert!(
            pem_blocks("-----BEGIN CERTIFICATE-----\nAAAA\n").unwrap_err().contains("not closed")
        );
        assert!(
            pem_blocks("-----BEGIN CERTIFICATE-----\nAAAA\n-----END PRIVATE KEY-----\n").is_err()
        );
        assert!(
            pem_blocks("-----BEGIN CERTIFICATE-----\n!!!!\n-----END CERTIFICATE-----\n").is_err()
        );
        assert!(pem_blocks("-----BEGIN CERTIFICATE-----\n-----END CERTIFICATE-----\n").is_err());
    }

    #[test]
    fn an_encrypted_block_is_refused_for_its_header_lines() {
        let text = "-----BEGIN RSA PRIVATE KEY-----\nProc-Type: 4,ENCRYPTED\nDEK-Info: AES-128-CBC,00\n\nAAAA\n-----END RSA PRIVATE KEY-----\n";
        assert!(pem_blocks(text).unwrap_err().contains("header lines"));
    }

    #[test]
    fn a_chain_file_holds_certificates_alone() {
        let key = pem_encode("PRIVATE KEY", b"x");
        assert!(chain_of(&key).unwrap_err().contains("PRIVATE KEY"));
        assert_eq!(chain_of(RSA.pem).unwrap().len(), 1);
    }

    #[test]
    fn the_three_key_encodings_and_nothing_else_are_private_keys() {
        let block = |label: &str| Block { label: label.into(), der: vec![1, 2, 3] };
        for label in ["PRIVATE KEY", "RSA PRIVATE KEY", "EC PRIVATE KEY"] {
            assert!(private_key(&block(label)).is_ok(), "{label}");
        }
        assert!(private_key(&block("ENCRYPTED PRIVATE KEY")).unwrap_err().contains("passphrase"));
        assert!(private_key(&block("CERTIFICATE")).is_err());
    }

    #[test]
    fn a_chain_to_a_trusted_root_through_an_intermediate_is_trusted() {
        let root = fixtures::root("Test Root");
        let middle = fixtures::intermediate(&root, "Test Intermediate");
        let leaf = fixtures::server(&middle, &Server::default());
        let roots = fixtures::roots(&[&root]);
        let chain = [leaf.der.clone(), middle.der().clone()];
        assert_eq!(trust(&chain, &roots, NOW), Trust::Trusted);
    }

    #[test]
    fn a_chain_without_its_intermediate_stops_at_the_intermediate() {
        let root = fixtures::root("Test Root");
        let middle = fixtures::intermediate(&root, "Test Intermediate");
        let leaf = fixtures::server(&middle, &Server::default());
        let roots = fixtures::roots(&[&root]);
        assert_eq!(
            trust(std::slice::from_ref(&leaf.der), &roots, NOW),
            Trust::UnknownIssuer("CN=Test Intermediate".into())
        );
    }

    #[test]
    fn one_certificate_that_signed_itself_is_self_signed_not_unknown() {
        let roots = fixtures::roots(&[]);
        let chain = chain_of(ECDSA.pem).unwrap();
        let trusted = trust(&chain, &roots, NOW);
        assert_eq!(trusted, Trust::SelfSigned);
        assert!(trusted.why_not().unwrap().starts_with("self-signed"));
    }

    #[test]
    fn an_expired_certificate_in_the_chain_is_refused_in_words() {
        let root = fixtures::root("Test Root");
        let leaf = fixtures::server(&root, &Server::default());
        let roots = fixtures::roots(&[&root]);
        let later = NOW + 400 * DAY;
        match trust(std::slice::from_ref(&leaf.der), &roots, later) {
            Trust::Refused(why) => assert!(why.contains("ran out on 2027-04-15"), "{why}"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_key_fits_its_own_certificate_and_no_other() {
        let root = fixtures::root("Test Root");
        let (one, other) = (
            fixtures::server(&root, &Server::default()),
            fixtures::server(&root, &Server::default()),
        );
        let key_of =
            |leaf: &fixtures::Leaf| private_key(&pem_blocks(&leaf.key).unwrap()[0]).unwrap();
        assert_eq!(key_fits(std::slice::from_ref(&one.der), &key_of(&one)), Ok(()));
        assert_eq!(
            key_fits(std::slice::from_ref(&one.der), &key_of(&other)),
            Err(String::from("the key is not the certificate's"))
        );
    }

    #[test]
    fn a_key_that_is_not_a_key_cannot_sign() {
        let root = fixtures::root("Test Root");
        let leaf = fixtures::server(&root, &Server::default());
        let nonsense = PrivateKeyDer::Pkcs8(vec![1, 2, 3].into());
        assert!(key_fits(&[leaf.der], &nonsense).unwrap_err().contains("can sign with"));
    }
}
