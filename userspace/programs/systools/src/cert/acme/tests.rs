// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The tests of what `cert get`, `cert renew` and certd decide before and
//! after they talk to an authority: which names, which directory, which
//! account file, when to renew, and what gets installed.

use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use rcgen::{CertificateParams, ExtendedKeyUsagePurpose, IsCa, Issuer, KeyPair, SerialNumber};
use time::OffsetDateTime;

use std::path::Path;

use super::*;
use crate::cert::fixtures::{self, DAY, NOW, Scratch};

fn names(list: &[&str]) -> Vec<String> {
    list.iter().map(|name| (*name).to_string()).collect()
}

#[test]
fn names_are_lowercase_domains_and_a_wildcard_needs_dns() {
    for fine in ["example.com", "www.example.com", "a-b.c1.example.co.uk", "xn--bcher-kva.example"]
    {
        assert_eq!(check_dns_name(fine, false), Ok(()), "{fine}");
    }
    assert_eq!(check_dns_name("*.example.com", true), Ok(()));
    assert_eq!(
        check_dns_name("*.example.com", false).unwrap_err(),
        "*.example.com is a wildcard, which the authority proves only through DNS: add --dns"
    );
    for wrong in [
        "example",
        "Example.com",
        "-a.example.com",
        "a-.example.com",
        "a..example.com",
        ".example.com",
        "example.com.",
        "a_b.example.com",
        "b\u{fc}cher.example",
        "a b.example.com",
        "*.*.example.com",
        "www.*.example.com",
        "*example.com",
    ] {
        assert!(check_dns_name(wrong, true).is_err(), "{wrong}");
    }
    assert!(check_dns_name(&format!("{}.example.com", "a".repeat(64)), false).is_err());
    assert!(check_dns_name(&format!("{}.com", "a".repeat(250)), false).is_err());
    assert!(check_dns_name("203.0.113.7", false).unwrap_err().contains("is an IP address"));
    assert!(check_dns_name("2001:db8::1", false).unwrap_err().contains("is an IP address"));
}

#[test]
fn an_order_has_one_to_a_hundred_names_each_once() {
    assert_eq!(check_names(&names(&["a.example", "b.example"]), false), Ok(()));
    assert!(check_names(&[], false).unwrap_err().starts_with("a certificate has 1 to 100 names"));
    let hundred: Vec<String> = (0..100).map(|n| format!("h{n}.example")).collect();
    assert_eq!(check_names(&hundred, false), Ok(()));
    let more: Vec<String> = (0..101).map(|n| format!("h{n}.example")).collect();
    assert!(check_names(&more, false).unwrap_err().contains("not 101"));
    assert_eq!(
        check_names(&names(&["a.example", "b.example", "a.example"]), false).unwrap_err(),
        "a.example is named twice"
    );
}

#[test]
fn a_directory_is_https_or_http_on_a_private_address() {
    let nowhere = |_: &str| Vec::new();
    assert!(check_directory(LETS_ENCRYPT, &nowhere).is_ok());
    assert!(check_directory("https://pebble.test:14000/dir", &nowhere).is_ok());
    assert!(check_directory("http://10.0.2.2:14000/dir", &nowhere).is_ok());
    assert!(check_directory("http://127.0.0.1:14000/dir", &nowhere).is_ok());
    assert!(check_directory("http://[fd00::1]/dir", &nowhere).is_ok());
    let private = |_: &str| vec!["192.168.1.10".parse::<IpAddr>().unwrap()];
    assert!(check_directory("http://ca.corp.example/acme/directory", &private).is_ok());
    let public = |_: &str| vec!["203.0.113.9".parse::<IpAddr>().unwrap()];
    let mixed = |_: &str| vec!["192.168.1.10".parse().unwrap(), "203.0.113.9".parse().unwrap()];
    for refused in ["http://203.0.113.9/dir", "http://acme.example/dir", "http://acme.example/dir"]
    {
        let why = check_directory(refused, &public).unwrap_err();
        assert!(why.contains("an authority on the internet is reached over https"), "{why}");
    }
    assert!(check_directory("http://ca.corp.example/dir", &mixed).is_err());
    assert!(check_directory("http://unknown.example/dir", &nowhere).is_err());
    assert!(check_directory("ftp://a.example/dir", &public).unwrap_err().contains("use https://"));
    assert!(check_directory("not a url", &public).is_err());
}

#[test]
fn an_account_is_kept_in_a_file_named_for_its_directory() {
    let file = |text: &str| account_file(&Url::parse(text).unwrap());
    assert_eq!(file(LETS_ENCRYPT).unwrap(), "acme-v02.api.letsencrypt.org.json");
    assert_eq!(file(LETS_ENCRYPT_STAGING).unwrap(), "acme-staging-v02.api.letsencrypt.org.json");
    assert_eq!(file("https://pebble.test:14000/dir").unwrap(), "pebble.test-14000.json");
    assert_eq!(file("http://10.0.2.2:14000/dir").unwrap(), "10.0.2.2-14000.json");
    assert_eq!(file("https://[fd00::1]:9000/dir").unwrap(), "fd00__1-9000.json");
    // Another directory of the same host and port shares the account.
    assert_eq!(file("https://pebble.test:14000/other").unwrap(), "pebble.test-14000.json");
}

#[test]
fn the_terms_of_service_are_in_the_directorys_meta() {
    let json = br#"{"newNonce":"x","meta":{"termsOfService":"https://ca.example/terms.pdf"}}"#;
    assert_eq!(terms_in(json).unwrap().as_deref(), Some("https://ca.example/terms.pdf"));
    assert_eq!(terms_in(br#"{"newNonce":"x","meta":{}}"#).unwrap(), None);
    assert_eq!(terms_in(br#"{"newNonce":"x"}"#).unwrap(), None);
    assert!(terms_in(b"<html>").unwrap_err().starts_with("the authority's directory is not JSON"));
}

#[test]
fn a_service_is_running_by_the_line_of_sparks_status() {
    let status = "sshd\trunning\t41\t0\t-\talways\tSSH\nnginx\trunning\t52\t0\t-\talways\tweb\n\
                  certd\twaiting\t-\t0\t-\talways\tcerts\n#\tshutdown\tpower-off\t5\n";
    assert!(service_running(status, "nginx"));
    assert!(service_running(status, "sshd"));
    assert!(!service_running(status, "certd"));
    assert!(!service_running(status, "ngin"));
    assert!(!service_running("nginx\tstopped\t-\t0\t-\talways\tweb\n", "nginx"));
    assert!(!service_running("", "nginx"));
}

#[test]
fn port_80_is_found_taken_when_another_holds_it() {
    let held = listen("0.0.0.0:0".parse().unwrap()).unwrap();
    let port = held.local_addr().unwrap().port();
    assert!(matches!(bind(port), Ok(Port80::Taken)));
    drop(held);
    match bind(port) {
        Ok(Port80::Free(listeners)) => assert!((1..=2).contains(&listeners.len())),
        _ => panic!("a free port is found free"),
    }
}

#[test]
fn without_the_authoritys_word_a_certificate_is_renewed_after_two_thirds_of_its_life() {
    let start = 1_700_000_000;
    assert_eq!(third_rule(start, start + 90 * DAY), start + 60 * DAY);
    assert_eq!(third_rule(start, start + 45 * DAY), start + 30 * DAY);
    assert_eq!(third_rule(start, start + 6 * DAY), start + 4 * DAY);
    // 30 days before it ends for 90 and 15 for 45.
    assert_eq!(start + 90 * DAY - third_rule(start, start + 90 * DAY), 30 * DAY);
    assert_eq!(start + 45 * DAY - third_rule(start, start + 45 * DAY), 15 * DAY);
}

#[test]
fn the_renewal_time_is_inside_the_window_and_kept_from_look_to_look() {
    let life = (NOW - 60 * DAY, NOW + 30 * DAY);
    let window = Some((NOW + 10 * DAY, NOW + 12 * DAY));
    let first = renew_time(None, window, life, NOW, 1u64 << 62);
    assert_eq!(first, NOW + 10 * DAY + DAY / 2);
    // Any number picks a time inside it.
    for random in [0, 1, u64::MAX / 2, u64::MAX] {
        let at = renew_time(None, window, life, NOW, random);
        assert!((NOW + 10 * DAY..=NOW + 12 * DAY).contains(&at), "{random}: {at}");
    }
    assert_eq!(renew_time(None, window, life, NOW, 0), NOW + 10 * DAY);
    // The same window and another number: what was kept stays.
    assert_eq!(renew_time(Some(first), window, life, NOW + 3600, u64::MAX / 2), first);
    assert_eq!(renew_time(Some(first), window, life, NOW + DAY, 7), first);
}

#[test]
fn a_window_that_moves_sooner_replaces_the_kept_time() {
    let life = (NOW - 60 * DAY, NOW + 30 * DAY);
    let kept = NOW + 11 * DAY;
    // The authority now wants it replaced within the next two days.
    let sooner = Some((NOW + 100, NOW + 2 * DAY));
    let at = renew_time(Some(kept), sooner, life, NOW, 0);
    assert_eq!(at, NOW + 100);
    let at = renew_time(Some(kept), sooner, life, NOW, u64::MAX);
    assert!((NOW + 100..=NOW + 2 * DAY).contains(&at), "{at}");
    // One that moved later does too: what was kept falls before it.
    let later = Some((NOW + 20 * DAY, NOW + 22 * DAY));
    assert_eq!(renew_time(Some(kept), later, life, NOW, 0), NOW + 20 * DAY);
}

#[test]
fn a_window_that_has_begun_is_cut_to_start_now_and_one_that_ended_means_now() {
    let life = (NOW - 60 * DAY, NOW + 30 * DAY);
    let begun = Some((NOW - DAY, NOW + 2 * DAY));
    assert_eq!(renew_time(None, begun, life, NOW, 0), NOW);
    let at = renew_time(None, begun, life, NOW, u64::MAX);
    assert!((NOW..=NOW + 2 * DAY).contains(&at), "{at}");
    let over = Some((NOW - 3 * DAY, NOW - DAY));
    assert_eq!(renew_time(None, over, life, NOW, 12345), NOW);
    assert_eq!(renew_time(None, over, life, NOW, u64::MAX), NOW);
    // A time kept inside a window that has begun is due: it is in the past.
    assert_eq!(renew_time(Some(NOW - 3600), begun, life, NOW, 99), NOW - 3600);
}

#[test]
fn without_a_window_the_third_rule_decides_whatever_was_kept() {
    let life = (NOW - 60 * DAY, NOW + 30 * DAY);
    let want = third_rule(life.0, life.1);
    assert_eq!(renew_time(None, None, life, NOW, 5), want);
    assert_eq!(renew_time(Some(NOW + 5 * DAY), None, life, NOW, 5), want);
    assert_eq!(want, NOW);
}

#[test]
fn a_window_given_the_wrong_way_round_is_read_the_right_way() {
    let life = (NOW - 60 * DAY, NOW + 30 * DAY);
    let at = renew_time(None, Some((NOW + 12 * DAY, NOW + 10 * DAY)), life, NOW, 0);
    assert_eq!(at, NOW + 10 * DAY);
}

/// A leaf certificate for `names` that `issuer` signed, with the authority
/// key identifier and a serial number of its own.
fn leaf(issuer: &fixtures::Authority, names: &[&str], serial: &[u8]) -> fixtures::Leaf {
    leaf_from(issuer, names, serial, (NOW - DAY, NOW + 90 * DAY))
}

/// A certificate valid from `life.0` to `life.1`.
fn leaf_from(
    issuer: &fixtures::Authority,
    names: &[&str],
    serial: &[u8],
    life: (i64, i64),
) -> fixtures::Leaf {
    let mut params =
        CertificateParams::new(names.iter().map(|name| (*name).to_string()).collect::<Vec<_>>())
            .unwrap();
    params.not_before = OffsetDateTime::from_unix_timestamp(life.0).unwrap();
    params.not_after = OffsetDateTime::from_unix_timestamp(life.1).unwrap();
    params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
    params.is_ca = IsCa::ExplicitNoCa;
    params.use_authority_key_identifier_extension = true;
    params.serial_number = Some(SerialNumber::from_slice(serial));
    let key = KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).unwrap();
    let issuer: &Issuer<'_, KeyPair> = issuer;
    let certificate = params.signed_by(&key, issuer).unwrap();
    fixtures::Leaf {
        der: certificate.der().clone(),
        key: inspect::pem_encode("PRIVATE KEY", &key.serialize_der()),
    }
}

#[test]
fn a_certificate_is_identified_to_the_authority_by_its_key_identifier_and_serial() {
    let root = fixtures::root("Test Root");
    let made = leaf(&root, &["a.example"], &[0x80, 0x01, 0x02]);
    let identifier = identifier_of(&fixtures::chain_file(&[&made.der])).unwrap();
    let text = identifier.to_string();
    let (key, serial) = text.split_once('.').unwrap();
    // The serial number as its DER integer holds it: a leading zero keeps
    // 0x80 from being negative.
    assert_eq!(URL_SAFE_NO_PAD.decode(serial).unwrap(), [0x00, 0x80, 0x01, 0x02]);
    // The authority's key identifier is a SHA-1 of its key, or longer.
    assert!(URL_SAFE_NO_PAD.decode(key).unwrap().len() >= 20);
    let again = identifier_of(&fixtures::chain_file(&[&made.der])).unwrap();
    assert_eq!(identifier, again);
    let other =
        identifier_of(&fixtures::chain_file(&[&leaf(&root, &["a.example"], &[7]).der])).unwrap();
    assert_eq!(other.authority_key_identifier, identifier.authority_key_identifier);
    assert_ne!(other.serial, identifier.serial);
    assert_eq!(URL_SAFE_NO_PAD.decode(other.serial.as_ref()).unwrap(), [7]);
}

#[test]
fn a_certificate_without_an_authority_key_identifier_has_no_identifier() {
    let own = fixtures::self_signed(&["a.example"]);
    let why = identifier_of(&fixtures::chain_file(&[&own.der])).unwrap_err();
    assert_eq!(why, "it has no authority key identifier");
    assert!(identifier_of("").is_err());
}

#[test]
fn a_request_names_the_names_with_the_first_as_the_common_name() {
    for key in [KeyKind::EcdsaP256, KeyKind::EcdsaP384] {
        let (pem, request) = make_request(&names(&["a.example", "*.a.example"]), key).unwrap();
        assert!(pem.starts_with("-----BEGIN PRIVATE KEY-----"));
        // A request is a DER sequence that names both names and the first as its subject.
        assert_eq!(request[0], 0x30);
        let text = String::from_utf8_lossy(&request);
        assert!(text.contains("a.example"));
        assert!(text.contains("*.a.example"));
    }
    let (first, _) = make_request(&names(&["a.example"]), KeyKind::EcdsaP256).unwrap();
    let (second, _) = make_request(&names(&["a.example"]), KeyKind::EcdsaP256).unwrap();
    assert_ne!(first, second, "a key is made anew each time");
}

fn store_in(scratch: &Scratch) -> Store {
    Store::at(&scratch.0.join("certificates"), Duration::from_secs(1))
}

fn acme_for(names: &[&str]) -> Acme {
    Acme {
        directory: String::from("https://pebble.test:14000/dir"),
        names: self::names(names),
        challenge: Challenge::Http01,
        key: KeyKind::EcdsaP256,
        profile: None,
        renew_at: None,
        revoked: false,
    }
}

#[test]
fn what_the_authority_gave_is_checked_and_installed_with_what_it_was_asked() {
    let scratch = Scratch::new("acme-install");
    let store = store_in(&scratch);
    let root = fixtures::root("Pebble Root");
    let middle = fixtures::intermediate(&root, "Pebble Intermediate");
    let made = leaf(&middle, &["a.example", "b.example"], &[1, 2, 3]);
    let issued =
        Issued { chain: fixtures::chain_file(&[&made.der, middle.der()]), key: made.key.clone() };
    let lock = store.lock().unwrap();
    let acme = acme_for(&["a.example", "b.example"]);
    let (details, notes) = install(&store, &lock, "site", &issued, &acme, NOW).unwrap();
    assert_eq!(details.names, ["a.example", "b.example"]);
    // An authority that is not Let's Encrypt's is the administrator's to trust.
    assert_eq!(notes.len(), 1, "{notes:?}");
    assert!(notes[0].starts_with("not trusted here: "), "{notes:?}");
    let stored = store.get("site").unwrap();
    assert_eq!(stored.kind, Kind::Acme);
    assert_eq!(stored.made, NOW);
    assert_eq!(stored.acme, Some(acme));
    let (_, key) = store.with_key("site").unwrap();
    assert_eq!(String::from_utf8(key).unwrap(), made.key);
}

#[test]
fn a_certificate_that_starts_a_few_minutes_ahead_of_the_clock_is_installed() {
    // An authority that dates a certificate from the minute it signs it, and
    // a clock a little behind it, must not lose a certificate that took an
    // order and a rate limit to get.
    let root = fixtures::root("Pebble Root");
    // The last is a certificate of 150 seconds, which starts ahead of the
    // clock and would be over before the skew was: it is still taken.
    for (ahead, lasts, installed) in
        [(CLOCK_SKEW - 1, 90 * DAY, true), (CLOCK_SKEW + 60, 90 * DAY, false), (30, 150, true)]
    {
        let scratch = Scratch::new("acme-skew");
        let store = store_in(&scratch);
        let made = leaf_from(&root, &["a.example"], &[1], (NOW + ahead, NOW + ahead + lasts));
        let issued = Issued { chain: fixtures::chain_file(&[&made.der]), key: made.key.clone() };
        let lock = store.lock().unwrap();
        let result = install(&store, &lock, "site", &issued, &acme_for(&["a.example"]), NOW);
        match (installed, result) {
            (true, Ok(_)) => {}
            (false, Err(why)) => assert!(why.contains("which is not yet"), "{why}"),
            (_, other) => panic!("{ahead} s ahead: {other:?}"),
        }
    }
}

#[test]
fn a_certificate_that_lacks_a_name_asked_for_is_not_installed() {
    let scratch = Scratch::new("acme-lacks");
    let store = store_in(&scratch);
    let root = fixtures::root("Pebble Root");
    let made = leaf(&root, &["a.example"], &[1]);
    let issued = Issued { chain: fixtures::chain_file(&[&made.der]), key: made.key.clone() };
    let lock = store.lock().unwrap();
    let why = install(&store, &lock, "site", &issued, &acme_for(&["a.example", "b.example"]), NOW)
        .unwrap_err();
    assert_eq!(why, "the authority's certificate does not name b.example: it names a.example");
    assert!(store.generation("site").unwrap().is_none());
}

#[test]
fn a_key_that_is_not_the_certificates_is_not_installed() {
    let scratch = Scratch::new("acme-key");
    let store = store_in(&scratch);
    let root = fixtures::root("Pebble Root");
    let made = leaf(&root, &["a.example"], &[1]);
    let other = leaf(&root, &["a.example"], &[2]);
    let issued = Issued { chain: fixtures::chain_file(&[&made.der]), key: other.key };
    let lock = store.lock().unwrap();
    let why = install(&store, &lock, "site", &issued, &acme_for(&["a.example"]), NOW).unwrap_err();
    assert!(why.contains("the key is not the certificate's"), "{why}");
}

#[test]
fn answers_for_nginx_are_files_that_are_taken_away_when_the_order_ends() {
    let scratch = Scratch::new("acme-files");
    let folder = scratch.0.join("live/acme-challenge");
    let serving = Serving::Files(folder.clone());
    let answers = vec![
        (String::from("tok-1"), String::from("tok-1.thumb")),
        (String::from("tok_2"), String::from("tok_2.thumb")),
    ];
    {
        let guard = serve(&serving, &answers).unwrap();
        assert_eq!(fs::read_to_string(folder.join("tok-1")).unwrap(), "tok-1.thumb");
        assert_eq!(fs::read_to_string(folder.join("tok_2")).unwrap(), "tok_2.thumb");
        let mode = |name: &str| {
            std::os::unix::fs::PermissionsExt::mode(
                &fs::metadata(folder.join(name)).unwrap().permissions(),
            ) & 0o777
        };
        assert_eq!(mode("tok-1"), 0o644);
        drop(guard);
    }
    assert_eq!(fs::read_dir(&folder).unwrap().count(), 0);
}

#[test]
fn a_token_that_would_leave_the_folder_is_refused_before_anything_is_written() {
    let scratch = Scratch::new("acme-token");
    let folder = scratch.0.join("acme-challenge");
    let serving = Serving::Files(folder.clone());
    for token in ["../escape", "a/b", "", ".hidden", "a b"] {
        let answers = vec![(token.to_string(), String::from("k"))];
        let why = serve(&serving, &answers).map(|_| ()).unwrap_err();
        assert!(why.contains("which is not one"), "{token}: {why}");
    }
    assert!(!scratch.0.join("escape").exists());
    assert_eq!(fs::read_dir(&folder).unwrap().count(), 0);
}

#[test]
fn an_account_file_keeps_what_the_library_needs_and_when_it_was_made() {
    let scratch = Scratch::new("acme-account");
    let store = store_in(&scratch);
    let lock = store.lock().unwrap();
    assert!(store.account("pebble.test-14000.json").unwrap().is_none());
    let text = r#"{"id":"https://pebble.test:14000/my-account/1","key_pkcs8":"AAAA","directory":"https://pebble.test:14000/dir","created":"2027-01-15T08:00:00Z","contact":null}"#;
    assert!(store.add_account(&lock, "pebble.test-14000.json", text).unwrap());
    // A second writer does not replace it.
    assert!(!store.add_account(&lock, "pebble.test-14000.json", "other").unwrap());
    assert_eq!(
        String::from_utf8(store.account("pebble.test-14000.json").unwrap().unwrap()).unwrap(),
        text
    );
    let kept: AccountFile = serde_json::from_str(text).unwrap();
    assert_eq!(kept.created, "2027-01-15T08:00:00Z");
    assert_eq!(kept.contact, None);
    let again = serde_json::to_value(&kept).unwrap();
    assert_eq!(again["directory"], "https://pebble.test:14000/dir");
    assert_eq!(again["id"], "https://pebble.test:14000/my-account/1");
    let mode = |path: &Path| {
        std::os::unix::fs::PermissionsExt::mode(&fs::metadata(path).unwrap().permissions()) & 0o777
    };
    assert_eq!(mode(&store.accounts()), 0o700);
    assert_eq!(mode(&store.accounts().join("pebble.test-14000.json")), 0o600);
}

#[test]
fn the_reasons_cert_revoke_takes_are_the_four_it_names() {
    assert!(matches!(revocation_reason("unspecified"), Ok(RevocationReason::Unspecified)));
    assert!(matches!(revocation_reason("key-compromise"), Ok(RevocationReason::KeyCompromise)));
    assert!(matches!(revocation_reason("superseded"), Ok(RevocationReason::Superseded)));
    assert!(matches!(
        revocation_reason("cessation-of-operation"),
        Ok(RevocationReason::CessationOfOperation)
    ));
    assert_eq!(
        revocation_reason("because").unwrap_err(),
        "`because` is not a reason: unspecified, key-compromise, superseded or cessation-of-operation"
    );
}

#[test]
fn key_types_are_the_two_ecdsa_curves() {
    assert_eq!(key_kind("ecdsa-p256"), Ok(KeyKind::EcdsaP256));
    assert_eq!(key_kind("ecdsa-p384"), Ok(KeyKind::EcdsaP384));
    assert_eq!(
        key_kind("rsa-2048").unwrap_err(),
        "`rsa-2048` is not a key type: ecdsa-p256 or ecdsa-p384"
    );
}
