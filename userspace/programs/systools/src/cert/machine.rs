// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The machine's own certificate, `this-machine` (ADR 0512): what certd
//! makes at its first start, when to make it again, and the check of
//! `cert check` that certd's readiness rests on.
//!
//! It is self-signed, so that the console and every service that names it
//! can serve HTTPS from the first boot; a person trusts it by its
//! fingerprint (dukovo-certificates(7)). ECDSA P-256 is what every
//! browser accepts and what ring can make. It lasts [`LIFETIME_DAYS`],
//! under the 825 days Apple's systems accept of any server certificate.
//!
//! A clock that has not been set says 1970 or the year a battery died, and
//! a certificate made then would be dated that year: while the clock says a
//! time before [`CLOCK_FLOOR`], nothing is made or judged. certd logs that
//! it waits, and `cert check` says why it fails.

use rcgen::{
    CertificateParams, DistinguishedName, DnType, ExtendedKeyUsagePurpose, IsCa, KeyPair,
    KeyUsagePurpose, PKCS_ECDSA_P256_SHA256, SerialNumber,
};
use ring::rand::{SecureRandom, SystemRandom};
use rustls::pki_types::CertificateDer;
use time::OffsetDateTime;

use super::inspect::{self, Details};
use super::store::{Files, Kind, Lock, Store, Stored};
use crate::util;

/// The certificate's name in the store, which `cert add` and `cert
/// remove` leave alone.
pub const NAME: &str = "this-machine";

/// How long it lasts, from an hour before it is made.
const LIFETIME_DAYS: i64 = 800;

/// How far before its making it starts, for machines whose clocks differ.
const BACKDATE: i64 = 3600;

/// certd makes it again once this many days or fewer are left.
pub const REMAKE_DAYS: i64 = 30;

/// A clock before this (2026-11-01 UTC, before the first system that had
/// certd) is wrong: no system had been made then.
const CLOCK_FLOOR: i64 = 1_793_491_200;

/// A certificate and its key, as PEM files.
pub struct Pair {
    pub chain: String,
    pub key: String,
}

/// What reading `this-machine` found: its details, or none when it is not
/// there, or why it does not read.
pub type Loaded = Result<Option<Details>, String>;

/// The names `this-machine` holds for a machine called `host`: the name,
/// the name with `.local`, which discoverd answers, and `localhost`. A
/// machine still called `localhost` has that alone.
pub fn names(host: &str) -> Vec<String> {
    if host == "localhost" {
        return vec![String::from("localhost")];
    }
    vec![host.to_string(), format!("{host}.local"), String::from("localhost")]
}

/// Why the clock cannot be trusted at time `now`, or none if it can.
pub fn wrong_clock(now: i64) -> Option<String> {
    (now < CLOCK_FLOOR)
        .then(|| format!("the clock is wrong ({})", util::time_text(now, "%F", true)))
}

/// Makes the certificate of a machine called `host`, at time `now` (seconds
/// since 1970).
///
/// # Errors
///
/// If `host` is not a name a certificate can hold, or no key can be made.
pub fn make(host: &str, now: i64) -> Result<Pair, String> {
    let mut params = CertificateParams::new(names(host))
        .map_err(|err| format!("{host} is not a name a certificate can hold: {err}"))?;
    let mut subject = DistinguishedName::new();
    subject.push(DnType::CommonName, host);
    params.distinguished_name = subject;
    let start = now - BACKDATE;
    params.not_before = at(start)?;
    params.not_after = at(start + LIFETIME_DAYS * 86_400)?;
    params.is_ca = IsCa::ExplicitNoCa;
    params.key_usages = vec![KeyUsagePurpose::DigitalSignature];
    params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
    params.serial_number = Some(SerialNumber::from_slice(&serial()?));
    let key = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256)
        .map_err(|err| format!("cannot make a key: {err}"))?;
    let certificate =
        params.self_signed(&key).map_err(|err| format!("cannot sign the certificate: {err}"))?;
    Ok(Pair {
        chain: inspect::pem_encode(inspect::CERTIFICATE, certificate.der()),
        key: inspect::pem_encode("PRIVATE KEY", &key.serialize_der()),
    })
}

/// A positive 16-byte serial number.
fn serial() -> Result<[u8; 16], String> {
    let mut bytes = [0u8; 16];
    SystemRandom::new()
        .fill(&mut bytes)
        .map_err(|_| String::from("no random numbers to make a serial number"))?;
    // The top bit clear keeps the number positive, the next one set keeps
    // it 16 bytes long.
    bytes[0] = (bytes[0] & 0x7f) | 0x40;
    Ok(bytes)
}

fn at(seconds: i64) -> Result<OffsetDateTime, String> {
    OffsetDateTime::from_unix_timestamp(seconds)
        .map_err(|err| format!("{seconds} is no time: {err}"))
}

/// Makes `this-machine` anew for a machine called `host`, at time `now`,
/// and returns what the new certificate says of itself.
///
/// # Errors
///
/// If it cannot be made or written.
pub fn renew(store: &Store, lock: &Lock, host: &str, now: i64) -> Result<Details, String> {
    let pair = make(host, now)?;
    let files =
        Files { chain: &pair.chain, key: &pair.key, kind: Kind::SelfSigned, made: now, acme: None };
    store.install(lock, NAME, &files)?;
    let chain = inspect::chain_of(&pair.chain)?;
    inspect::details(&chain[0])
}

/// Reads `this-machine`'s certificate.
pub fn load(store: &Store) -> Loaded {
    if store.generation(NAME)?.is_none() {
        return Ok(None);
    }
    let (_, details) = chain_and_details(&store.get(NAME)?)?;
    Ok(Some(details))
}

/// As [`load`], and the key must be the certificate's too: what only root
/// can read. Both come from one generation.
pub fn load_with_key(store: &Store) -> Loaded {
    if store.generation(NAME)?.is_none() {
        return Ok(None);
    }
    let (stored, key) = store.with_key(NAME)?;
    let (chain, details) = chain_and_details(&stored)?;
    let text = String::from_utf8(key).map_err(|_| String::from("key.pem is not text"))?;
    let blocks = inspect::pem_blocks(&text)?;
    let [block] = blocks.as_slice() else {
        return Err(format!("key.pem holds {} blocks, not one", blocks.len()));
    };
    inspect::key_fits(&chain, &inspect::private_key(block)?)?;
    Ok(Some(details))
}

/// The certificates of `stored`'s chain, and what the first says.
fn chain_and_details(stored: &Stored) -> Result<(Vec<CertificateDer<'static>>, Details), String> {
    let text = std::str::from_utf8(&stored.chain)
        .map_err(|_| String::from("fullchain.pem is not text"))?;
    let chain = inspect::chain_of(text)?;
    let first = chain.first().ok_or_else(|| String::from("fullchain.pem holds no certificate"))?;
    let details = inspect::details(first)?;
    Ok((chain, details))
}

/// Why `cert check` fails at time `now`, or none when the clock is right
/// and `loaded` is there, and has more than [`REMAKE_DAYS`] days left.
pub fn unhealthy(loaded: &Loaded, now: i64) -> Option<String> {
    if let Some(why) = wrong_clock(now) {
        return Some(why);
    }
    match loaded {
        Ok(None) => Some(format!("{NAME} does not exist: certd makes it when it starts")),
        Err(why) => Some(format!("{NAME} does not read: {why}")),
        Ok(Some(details)) if expiring(details, now) => Some(expiring_words(details, now)),
        Ok(Some(_)) => None,
    }
}

/// Why certd makes `this-machine` again for a machine called `host` at
/// time `now`, or none if it stands.
pub fn remake_reason(loaded: &Loaded, host: &str, now: i64) -> Option<String> {
    match loaded {
        Ok(None) => Some(String::from("it does not exist yet")),
        Err(why) => Some(format!("it does not read: {why}")),
        Ok(Some(details)) if details.names.first().map(String::as_str) != Some(host) => {
            Some(format!(
                "the machine is called {host} and the certificate {}",
                details.names.first().map_or_else(
                    || String::from("names nothing"),
                    |first| format!("is for {first}")
                )
            ))
        }
        Ok(Some(details)) if expiring(details, now) => Some(expiring_words(details, now)),
        Ok(Some(_)) => None,
    }
}

/// Whether `details` has [`REMAKE_DAYS`] days or fewer left at time `now`.
fn expiring(details: &Details, now: i64) -> bool {
    details.not_after - now <= REMAKE_DAYS * 86_400
}

fn expiring_words(details: &Details, now: i64) -> String {
    let days = details.days_left(now);
    if days < 0 {
        format!("it ran out on {}", util::local_time(details.not_after, "%F"))
    } else {
        format!("it runs out on {} ({days} days)", util::local_time(details.not_after, "%F"))
    }
}

#[cfg(test)]
mod tests {
    use x509_cert::Certificate;
    use x509_cert::der::Decode;
    use x509_cert::ext::pkix::ExtendedKeyUsage;

    use super::*;
    use crate::cert::fixtures::{DAY, NOW, Scratch};

    fn made(host: &str) -> (Pair, Details) {
        let pair = make(host, NOW).unwrap();
        let chain = inspect::chain_of(&pair.chain).unwrap();
        let details = inspect::details(&chain[0]).unwrap();
        (pair, details)
    }

    #[test]
    fn a_machine_has_its_name_then_the_name_with_dot_local_then_localhost() {
        assert_eq!(names("duke-nas"), ["duke-nas", "duke-nas.local", "localhost"]);
        assert_eq!(names("localhost"), ["localhost"]);
    }

    #[test]
    fn the_certificate_names_the_machine_and_is_for_800_days_from_an_hour_ago() {
        let (_, details) = made("duke-nas");
        assert_eq!(details.names, ["duke-nas", "duke-nas.local", "localhost"]);
        assert_eq!(details.subject, "CN=duke-nas");
        assert_eq!(details.not_before, NOW - 3600);
        assert_eq!(details.not_after, NOW - 3600 + 800 * DAY);
        assert_eq!(details.key, "ECDSA P-256");
        assert!(details.self_issued && !details.authority);
    }

    #[test]
    fn the_certificate_is_for_serving_and_no_authority() {
        let (pair, _) = made("duke-nas");
        let chain = inspect::chain_of(&pair.chain).unwrap();
        let certificate = Certificate::from_der(chain[0].as_ref()).unwrap();
        let (_, usage) = certificate
            .tbs_certificate()
            .get_extension::<ExtendedKeyUsage>()
            .unwrap()
            .expect("an extended key usage");
        let server_auth = x509_cert::der::asn1::ObjectIdentifier::new_unwrap("1.3.6.1.5.5.7.3.1");
        assert_eq!(usage.0, [server_auth]);
    }

    #[test]
    fn the_serial_number_is_16_bytes_and_positive_and_never_twice_the_same() {
        let (first_pair, first) = made("duke-nas");
        let (_, second) = made("duke-nas");
        assert_eq!(first.serial.split(':').count(), 16);
        assert!(u8::from_str_radix(&first.serial[..2], 16).unwrap() < 0x80);
        assert_ne!(first.serial, second.serial);
        assert!(first_pair.chain.ends_with("-----END CERTIFICATE-----\n"));
    }

    #[test]
    fn the_key_is_pkcs8_and_fits_the_certificate() {
        let (pair, _) = made("duke-nas");
        assert!(pair.key.starts_with("-----BEGIN PRIVATE KEY-----\n"));
        let chain = inspect::chain_of(&pair.chain).unwrap();
        let blocks = inspect::pem_blocks(&pair.key).unwrap();
        let key = inspect::private_key(&blocks[0]).unwrap();
        assert_eq!(inspect::key_fits(&chain, &key), Ok(()));
    }

    #[test]
    fn it_is_remade_at_thirty_days_left_and_not_at_thirty_one() {
        let (_, details) = made("duke-nas");
        let loaded: Loaded = Ok(Some(details.clone()));
        let at = |days_left: i64| details.not_after - days_left * DAY;
        assert_eq!(remake_reason(&loaded, "duke-nas", at(31)), None);
        assert!(remake_reason(&loaded, "duke-nas", at(30)).unwrap().contains("runs out"));
        assert!(remake_reason(&loaded, "duke-nas", at(-1)).unwrap().contains("ran out"));
    }

    #[test]
    fn it_is_remade_when_the_machine_is_called_something_else() {
        let (_, details) = made("duke-nas");
        let loaded: Loaded = Ok(Some(details));
        assert_eq!(remake_reason(&loaded, "duke-nas", NOW), None);
        let why = remake_reason(&loaded, "office-nas", NOW).unwrap();
        assert!(why.contains("office-nas") && why.contains("duke-nas"), "{why}");
    }

    #[test]
    fn it_is_made_when_it_is_missing_or_does_not_read() {
        assert_eq!(remake_reason(&Ok(None), "duke-nas", NOW).unwrap(), "it does not exist yet");
        let broken: Loaded = Err(String::from("fullchain.pem: damaged"));
        assert!(remake_reason(&broken, "duke-nas", NOW).unwrap().contains("damaged"));
    }

    #[test]
    fn a_clock_before_the_first_system_makes_and_judges_nothing() {
        assert_eq!(wrong_clock(NOW), None);
        assert_eq!(wrong_clock(CLOCK_FLOOR), None);
        assert_eq!(wrong_clock(0).unwrap(), "the clock is wrong (1970-01-01)");
        assert_eq!(wrong_clock(CLOCK_FLOOR - 1).unwrap(), "the clock is wrong (2026-10-31)");
        let (_, details) = made("duke-nas");
        assert!(
            unhealthy(&Ok(Some(details)), 946_684_800).unwrap().starts_with("the clock is wrong")
        );
    }

    #[test]
    fn the_health_check_wants_it_there_readable_and_more_than_thirty_days_from_the_end() {
        let (_, details) = made("duke-nas");
        let at = |days_left: i64| details.not_after - days_left * DAY;
        let loaded: Loaded = Ok(Some(details.clone()));
        assert_eq!(unhealthy(&loaded, at(31)), None);
        assert!(unhealthy(&loaded, at(30)).unwrap().contains("runs out"));
        assert!(unhealthy(&Ok(None), NOW).unwrap().contains("does not exist"));
        assert!(unhealthy(&Err("damaged".into()), NOW).unwrap().contains("damaged"));
    }

    #[test]
    fn a_renewal_installs_a_new_generation_with_a_key_that_fits() {
        let scratch = Scratch::new("machine-renew");
        let store = Store::at(&scratch.0.join("certificates"), std::time::Duration::from_secs(5));
        assert!(matches!(load(&store), Ok(None)));
        let lock = store.lock().unwrap();
        let first = renew(&store, &lock, "duke-nas", NOW).unwrap();
        let second = renew(&store, &lock, "duke-nas", NOW + 10).unwrap();
        drop(lock);
        assert_ne!(first.fingerprint, second.fingerprint);
        assert_eq!(store.generation(NAME).unwrap(), Some(2));
        let loaded = load_with_key(&store).unwrap().unwrap();
        assert_eq!(loaded.fingerprint, second.fingerprint);
        let stored = store.get(NAME).unwrap();
        assert_eq!((stored.kind, stored.made), (Kind::SelfSigned, NOW + 10));
    }

    #[test]
    fn a_key_that_is_another_certificates_does_not_read_as_this_machines() {
        let scratch = Scratch::new("machine-wrongkey");
        let store = Store::at(&scratch.0.join("certificates"), std::time::Duration::from_secs(5));
        let (one, other) = (make("duke-nas", NOW).unwrap(), make("duke-nas", NOW).unwrap());
        let lock = store.lock().unwrap();
        let files = Files {
            chain: &one.chain,
            key: &other.key,
            kind: Kind::SelfSigned,
            made: NOW,
            acme: None,
        };
        store.install(&lock, NAME, &files).unwrap();
        assert!(load(&store).unwrap().is_some());
        assert!(load_with_key(&store).unwrap_err().contains("not the certificate's"));
    }
}
