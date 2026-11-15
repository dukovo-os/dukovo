// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The entry points of a fuzz target (ADR 0515): the console's answers to
//! the requests that tell it works, each parser's wrapper on an input that
//! is right and one that is not, and a few thousand inputs from a
//! generator with a seed of its own through all of them, which must not
//! panic.

use ring::digest;
use ring::rand::SystemRandom;
use ring::signature::{Ed25519KeyPair, KeyPair as _};
use serde_json::json;

use super::*;

/// The cookie of a session and of a trusted browser.
const SESSION_COOKIE: &str = "__Host-dukovo";
const TRUST_COOKIE: &str = "__Host-dukovo-trust";

/// What the console answers to `GET path` as a browser at its address asks.
fn get(console: &Console, path: &str) -> (u16, Vec<u8>) {
    console.request(b"GET", path.as_bytes(), &[(b"host", HOST.as_bytes())], b"")
}

/// The ID of the error in the JSON answer `body`.
fn error_of(body: &[u8]) -> String {
    let json: Value = serde_json::from_slice(body).expect("a JSON answer");
    json["error"]["id"].as_str().expect("an error with an ID").to_string()
}

#[test]
fn the_console_answers_health_with_ok() {
    let console = Console::new();
    let (status, body) = get(&console, "/api/1/health");
    assert_eq!(status, 200);
    assert_eq!(body, br#"{"ok":true}"#);
}

#[test]
fn a_request_with_no_session_is_401() {
    let console = Console::new();
    for path in ["/api/1/session", "/api/1/actions", "/api/1/account/preferences"] {
        let (status, body) = get(&console, path);
        assert_eq!(status, 401, "{path}");
        assert_eq!(error_of(&body), "ADM-107", "{path}");
    }
    // A POST that changes something, with what a browser's page sends.
    let headers: [(&[u8], &[u8]); 3] = [
        (b"host", HOST.as_bytes()),
        (b"origin", ORIGIN.as_bytes()),
        (b"content-type", b"application/json"),
    ];
    let (status, _) = console.request(b"POST", b"/api/1/lock", &headers, b"{}");
    assert_eq!(status, 401);
}

#[test]
fn the_files_of_the_console_are_served_and_a_name_that_is_not_its_own_is_refused() {
    let console = Console::new();
    let (status, body) = get(&console, "/");
    assert_eq!(status, 200);
    assert!(String::from_utf8(body).unwrap().contains("Dukovo Admin"));
    let wrong = console.request(b"GET", b"/api/1/health", &[(b"host", b"evil.example")], b"");
    assert_eq!(wrong.0, 421);
    assert_eq!(error_of(&wrong.1), "ADM-114");
}

#[test]
fn bytes_that_make_no_request_are_400_with_no_body_and_admind_is_not_asked() {
    let console = Console::new();
    let host: (&[u8], &[u8]) = (b"host", HOST.as_bytes());
    let none: &[(&[u8], &[u8])] = &[host];
    let refused = [
        console.request(b"", b"/api/1/health", none, b""),
        console.request(b"GE T", b"/api/1/health", none, b""),
        console.request(b"GET", b"", none, b""),
        console.request(b"GET", b"/api/1/he alth", none, b""),
        console.request(b"GET", b"/api/1/\x00", none, b""),
        console.request(b"GET", b"/api/1/health", &[host, (b"bad name", b"x")], b""),
        console.request(b"GET", b"/api/1/health", &[host, (b"x-a", b"line\nbreak")], b""),
        console.request(b"GET", b"/api/1/health", &[host, (b"", b"x")], b""),
    ];
    for (at, answer) in refused.iter().enumerate() {
        // admind's own answers to a request are JSON with an error.
        assert_eq!(answer, &(400, Vec::new()), "{at}");
    }
}

#[test]
fn one_console_serves_many_requests_and_the_limits_that_count_a_minute_do_not_run_out() {
    let console = Console::new();
    let headers: [(&[u8], &[u8]); 4] = [
        (b"host", HOST.as_bytes()),
        (b"origin", ORIGIN.as_bytes()),
        (b"content-type", b"application/json"),
        (b"sec-fetch-site", b"same-origin"),
    ];
    let body = br#"{"name":"chase","password":"wrong"}"#;
    // More than an address may make in a minute, and each is answered as
    // the first: authd's refusal, not the wait.
    for at in 0..40 {
        let (status, answer) = console.request(b"POST", b"/api/1/sign-in", &headers, body);
        assert_eq!(status, 403, "{at}: {}", String::from_utf8_lossy(&answer));
    }
    // And for a token that is wrong.
    let token: [(&[u8], &[u8]); 2] =
        [(b"host", HOST.as_bytes()), (b"authorization", b"Bearer dkv_chase_aa")];
    for at in 0..40 {
        let (status, answer) = console.request(b"GET", b"/api/1/actions", &token, b"");
        assert_eq!(status, 401, "{at}: {}", String::from_utf8_lossy(&answer));
    }
    for _ in 0..2000 {
        assert_eq!(get(&console, "/api/1/health").0, 200);
    }
}

#[test]
fn two_consoles_in_one_process_do_not_share_their_files() {
    let (one, two) = (Console::new(), Console::default());
    assert_eq!(get(&one, "/api/1/health").0, 200);
    assert_eq!(get(&two, "/api/1/health").0, 200);
    drop(one);
    assert_eq!(get(&two, "/api/1/health").0, 200);
}

#[test]
fn an_address_names_an_action_with_its_arguments_decoded() {
    let service = |path: &str| find(true, path);
    assert_eq!(
        service("/api/1/services/nginx/restart"),
        Some(("service.restart", r#"{"name":"nginx"}"#.into()))
    );
    assert_eq!(
        service("/api/1/services/a%20b/restart"),
        Some(("service.restart", r#"{"name":"a b"}"#.into()))
    );
    assert_eq!(find(false, "/api/1/services"), Some(("service.list", "{}".into())));
    // The other method, an address that nothing has, and a part that is
    // not one argument.
    assert_eq!(find(false, "/api/1/services/nginx/restart"), None);
    assert_eq!(service("/api/1/nothing"), None);
    for part in ["%2e%2e%2fx", "a%00b", "%", "%zz", "", "%ff"] {
        assert_eq!(service(&format!("/api/1/services/{part}/restart")), None, "{part:?}");
    }
}

#[test]
fn a_part_of_an_address_is_one_decoded_argument_or_none() {
    assert_eq!(decode("nginx").as_deref(), Some("nginx"));
    assert_eq!(decode("a%2Bb%20c").as_deref(), Some("a+b c"));
    assert_eq!(decode("%C3%A9").as_deref(), Some("\u{e9}"));
    for part in ["", "%", "%2", "%2g", "a%2Fb", "a%0Ab", "%C3", &"a".repeat(256)] {
        assert_eq!(decode(part), None, "{part:?}");
    }
    assert_eq!(decode(&"a".repeat(255)).map(|text| text.len()), Some(255));
}

#[test]
fn the_arguments_of_a_request_are_its_address_and_its_body() {
    assert_eq!(arguments(&[("name", "nginx")], b"").unwrap(), r#"{"name":"nginx"}"#);
    assert_eq!(arguments(&[], br#"{"wait":true}"#).unwrap(), r#"{"wait":true}"#);
    assert_eq!(
        arguments(&[("name", "nginx")], br#"{"a":1}"#).unwrap(),
        r#"{"a":1,"name":"nginx"}"#
    );
    for (params, body) in [
        (&[("name", "nginx")][..], &br#"{"name":"other"}"#[..]),
        (&[], b"[]"),
        (&[], b"{"),
        (&[], b"\xff"),
        (&[], b"null"),
    ] {
        let error = arguments(params, body).unwrap_err();
        assert!(error.starts_with("ADM-108: "), "{error}");
    }
}

#[test]
fn every_action_takes_json_arguments_or_says_why_not() {
    let ids = action_ids();
    assert!(ids.contains(&"service.restart") && ids.contains(&"service.list"));
    let mut sorted = ids.clone();
    sorted.sort_unstable();
    sorted.dedup();
    assert_eq!(sorted.len(), ids.len(), "an ID is in the list twice");
    assert_eq!(
        prepare("service.restart", br#"{"name":"nginx"}"#).unwrap(),
        ["service", "nginx", "restart"]
    );
    assert_eq!(prepare("service.list", b"{}").unwrap(), ["service", "list", "--json"]);
    for (id, args) in [
        ("service.restart", &b"{}"[..]),
        ("service.restart", br#"{"name":"nginx","extra":1}"#),
        ("service.restart", b"[]"),
        ("service.restart", br#"{"name":"-x"}"#),
        ("service.restart", br#"{"name":null}"#),
    ] {
        let error = prepare(id, args).unwrap_err();
        assert!(error.starts_with("ADM-1"), "{id} {error}");
    }
    assert!(prepare("service.restart", b"{").is_err());
    assert_eq!(prepare("nothing.at-all", b"{}").unwrap_err(), "there is no action nothing.at-all");
    // Whatever an action takes, any JSON is answered.
    for id in ids {
        for args in ["{}", "null", "[]", "1", "\"x\"", r#"{"name":"a b","wait":1}"#] {
            let _ = prepare(id, args.as_bytes());
        }
    }
}

#[test]
fn an_api_token_comes_as_one_bearer_header() {
    assert_eq!(bearer(&[]), Ok(false));
    assert_eq!(bearer(&[b"Bearer dkv_chase_00ff"]), Ok(true));
    assert_eq!(bearer(&[b"bearer  dkv_chase_00ff"]), Ok(true));
    assert_eq!(bearer(&[b"Basic Y2hhc2U6c2VjcmV0"]), Ok(false));
    for values in [
        &[&b"Bearer dkv_chase_00ff"[..], b"Bearer dkv_chase_00ee"][..],
        &[b"Bearer nothing"],
        &[b"Bearer dkv_a b"],
        &[b"Bearer dkv_\xff"],
        &[b"Bearer dkv_a\nb"],
    ] {
        let error = bearer(values).unwrap_err();
        assert!(!error.is_empty(), "{values:?}");
    }
}

#[test]
fn a_cookie_is_found_once_by_its_name() {
    let one: &[&[u8]] = &[b"theme=dark; __Host-dukovo=abc; x=1"];
    assert_eq!(cookie(one, SESSION_COOKIE).as_deref(), Some("abc"));
    assert_eq!(cookie(one, TRUST_COOKIE), None);
    let apart: &[&[u8]] = &[b"__Host-dukovo=abc", b"x=1; __Host-dukovo-trust=def"];
    assert_eq!(cookie(apart, TRUST_COOKIE).as_deref(), Some("def"));
    // A cookie set twice is no cookie, and a value that is no header's.
    let twice: &[&[u8]] = &[b"__Host-dukovo=abc; __Host-dukovo=def"];
    assert_eq!(cookie(twice, SESSION_COOKIE), None);
    assert_eq!(cookie(&[b"a=b\nc=d"], "a"), None);
}

#[test]
fn the_scopes_of_a_token_are_a_list_of_areas() {
    for list in ["*", "services", "services:read", "services:read,firewall,time-zone:read"] {
        assert!(scopes(list), "{list}");
    }
    for list in ["", ",", "services,", "Services", "services:write", "a b", "9lives", "*:read"] {
        assert!(!scopes(list), "{list:?}");
    }
}

#[test]
fn a_preferences_body_has_the_keys_a_preference_has_and_values_that_can_work() {
    for body in [&br"{}"[..], br#"{"theme":"light"}"#, br#"{"text":150,"motion":"less"}"#] {
        assert_eq!(preferences_body(body), Ok(()), "{}", String::from_utf8_lossy(body));
    }
    // An empty zone is none.
    assert_eq!(preferences_body(br#"{"zone":""}"#), Ok(()));
    for body in [
        &br#"{"theme":"neon"}"#[..],
        br#"{"text":101}"#,
        br#"{"motion":3}"#,
        br#"{"colour":"red"}"#,
        br#"{"zone":"../etc/passwd"}"#,
        br#"{"zone":"/etc/passwd"}"#,
        b"null",
        b"",
    ] {
        assert!(preferences_body(body).is_err(), "{}", String::from_utf8_lossy(body));
    }
    let refused = preferences_body(br#"{"theme":"neon"}"#).unwrap_err();
    assert!(refused.starts_with("ADM-125: "), "{refused}");
}

#[test]
fn a_preferences_change_is_made_to_a_settings_text_and_keeps_its_comments() {
    let text = "# Mine.\ntheme = \"dark\" # the best\n\n# Size.\ntext = 100\n";
    let light = br#"{"theme":"light","text":112}"#;
    assert_eq!(
        settings_text(text, light).unwrap().as_deref(),
        Some("# Mine.\ntheme = \"light\" # the best\n\n# Size.\ntext = 112\n")
    );
    // A change that moves nothing, and one that asks nothing.
    assert_eq!(settings_text(text, br#"{"theme":"dark"}"#), Ok(None));
    assert_eq!(settings_text(text, b"{}"), Ok(None));
    assert_eq!(
        settings_text("", br#"{"motion":"less"}"#).unwrap().as_deref(),
        Some("motion = \"less\"\n")
    );
    assert!(settings_text("theme = \n", b"{}").is_err());
    assert!(settings_text(text, b"{").is_err());
    assert!(settings_text(text, br#"{"other":1}"#).is_err());
}

/// An Ed25519 passkey, and what an authenticator writes of it: enough for
/// the passkey checks to take what it gives.
struct Authenticator {
    pair: Ed25519KeyPair,
}

impl Authenticator {
    fn new() -> Authenticator {
        let pkcs8 = Ed25519KeyPair::generate_pkcs8(&SystemRandom::new()).unwrap();
        Authenticator { pair: Ed25519KeyPair::from_pkcs8(pkcs8.as_ref()).unwrap() }
    }

    /// Its public key as a COSE key: OKP, EdDSA, Ed25519.
    fn cose(&self) -> Vec<u8> {
        let mut key = vec![0xa4, 0x01, 0x01, 0x03, 0x27, 0x20, 0x06, 0x21, 0x58, 0x20];
        key.extend_from_slice(self.pair.public_key().as_ref());
        key
    }

    /// Authenticator data for the relying party with `flags` and `count`,
    /// and when `new`, the passkey's ID and key.
    fn data(&self, flags: u8, count: u32, new: bool) -> Vec<u8> {
        let mut data = digest::digest(&digest::SHA256, RELYING_PARTY.as_bytes()).as_ref().to_vec();
        data.push(flags);
        data.extend_from_slice(&count.to_be_bytes());
        if new {
            data.extend_from_slice(&[0; 16]);
            data.extend_from_slice(&4u16.to_be_bytes());
            data.extend_from_slice(b"cred");
            data.extend_from_slice(&self.cose());
        }
        data
    }

    /// The attestation object, of format `none`, that holds `data`.
    fn attestation(data: &[u8]) -> Vec<u8> {
        let mut object = vec![0xa3, 0x63];
        object.extend_from_slice(b"fmt");
        object.extend_from_slice(&[0x64, b'n', b'o', b'n', b'e', 0x67]);
        object.extend_from_slice(b"attStmt");
        object.extend_from_slice(&[0xa0, 0x68]);
        object.extend_from_slice(b"authData");
        object.extend_from_slice(&[0x58, u8::try_from(data.len()).unwrap()]);
        object.extend_from_slice(data);
        object
    }

    /// The signature over `data` and the hash of the client data of a use.
    fn sign(&self, data: &[u8]) -> Vec<u8> {
        let hash = digest::digest(&digest::SHA256, &client_data(false));
        let message = [data, hash.as_ref()].concat();
        self.pair.sign(&message).as_ref().to_vec()
    }
}

#[test]
fn client_data_is_for_the_ceremony_the_challenge_and_the_origin() {
    assert_eq!(webauthn_client_data(true, &client_data(true)), Ok(()));
    assert_eq!(webauthn_client_data(false, &client_data(false)), Ok(()));
    let other =
        String::from_utf8(client_data(false)).unwrap().replace(ORIGIN, "https://evil.example");
    for (create, data) in [
        (true, client_data(false)),
        (false, client_data(true)),
        (false, other.into_bytes()),
        (false, b"{}".to_vec()),
        (false, b"".to_vec()),
        (false, vec![b' '; 5000]),
        (false, br#"{"type":"webauthn.get","challenge":"!","origin":"x"}"#.to_vec()),
    ] {
        assert!(webauthn_client_data(create, &data).is_err(), "{}", String::from_utf8_lossy(&data));
    }
}

#[test]
fn authenticator_data_is_for_the_relying_party_and_says_the_person_was_there() {
    let passkey = Authenticator::new();
    // User present; verified; a new passkey's, with its key.
    assert_eq!(webauthn_authenticator_data(false, false, &passkey.data(0x01, 1, false)), Ok(()));
    assert_eq!(webauthn_authenticator_data(false, true, &passkey.data(0x05, 1, false)), Ok(()));
    assert_eq!(webauthn_authenticator_data(true, false, &passkey.data(0x41, 0, true)), Ok(()));
    for (create, verified, data) in [
        (false, false, passkey.data(0x00, 1, false)),
        (false, true, passkey.data(0x01, 1, false)),
        (true, false, passkey.data(0x01, 0, false)),
        (false, false, passkey.data(0x41, 0, true)),
        (false, false, vec![0; 36]),
        (false, false, vec![0; 37]),
        (false, false, [passkey.data(0x01, 1, false), vec![0]].concat()),
        (true, false, passkey.data(0x41, 0, true)[..60].to_vec()),
    ] {
        assert!(webauthn_authenticator_data(create, verified, &data).is_err(), "{data:?}");
    }
}

#[test]
fn a_cose_key_is_one_of_the_three_kinds_with_their_sizes() {
    let passkey = Authenticator::new();
    assert_eq!(webauthn_key(&passkey.cose()), Ok(()));
    let mut short = passkey.cose();
    short.truncate(short.len() - 1);
    for key in [short, vec![], vec![0xa0], vec![0x80], b"\xa1\x01\x01".to_vec(), vec![0xff; 12]] {
        assert!(webauthn_key(&key).is_err(), "{key:?}");
    }
    // A P-256 key whose coordinates are 1 byte, and an unknown algorithm.
    assert!(webauthn_key(&[0xa5, 1, 2, 3, 0x26, 0x20, 1, 0x21, 0x41, 0, 0x22, 0x41, 0]).is_err());
    assert!(webauthn_key(&[0xa2, 1, 2, 3, 0x38, 0x62]).is_err());
}

#[test]
fn a_new_passkeys_attestation_holds_its_authenticator_data_and_a_key_that_reads() {
    let passkey = Authenticator::new();
    let object = Authenticator::attestation(&passkey.data(0x41, 0, true));
    assert_eq!(webauthn_attestation(&object), Ok(()));
    let without_key = Authenticator::attestation(&passkey.data(0x01, 0, false));
    for bad in
        [&object[..object.len() - 1], &object[..4], &[0xa0], &[], &without_key, &[0xf6], &[0x80]]
    {
        assert!(webauthn_attestation(bad).is_err(), "{bad:?}");
    }
}

#[test]
fn the_use_of_a_passkey_is_signed_by_its_key_and_its_count_does_not_fall() {
    let passkey = Authenticator::new();
    let data = passkey.data(0x01, 1, false);
    let signature = passkey.sign(&data);
    assert_eq!(webauthn_assertion(&data, &signature, &passkey.cose()), Ok(()));
    let mut bent = signature.clone();
    bent[3] ^= 1;
    let other = Authenticator::new();
    for (data, signature, key) in [
        (data.clone(), bent, passkey.cose()),
        (data.clone(), signature.clone(), other.cose()),
        (data.clone(), signature.clone(), vec![]),
        (data.clone(), vec![], passkey.cose()),
        (passkey.data(0x00, 1, false), signature.clone(), passkey.cose()),
        (vec![], signature, passkey.cose()),
    ] {
        assert!(webauthn_assertion(&data, &signature, &key).is_err());
    }
    // A count that is 0, as a synced passkey keeps, is taken.
    let counted = passkey.data(0x01, 0, false);
    assert_eq!(webauthn_assertion(&counted, &passkey.sign(&counted), &passkey.cose()), Ok(()));
}

#[test]
fn the_challenge_is_what_the_client_data_names_in_base64url() {
    assert_eq!(webauthn_challenge(&client_data(false)), Ok(CHALLENGE.to_vec()));
    for data in [&b"{}"[..], b"", b"[]", br#"{"challenge":"***"}"#, br#"{"challenge":3}"#] {
        let error = webauthn_challenge(data).unwrap_err();
        assert!(error.starts_with("ADM-108: "), "{error}");
    }
}

#[test]
fn what_a_browser_sends_of_a_passkey_is_json_of_base64url() {
    let passkey = Authenticator::new();
    let b64 = |bytes: &[u8]| URL_SAFE_NO_PAD.encode(bytes);
    let object = Authenticator::attestation(&passkey.data(0x41, 0, true));
    let made = json!({ "response": {
        "clientDataJSON": b64(&client_data(true)), "attestationObject": b64(&object),
    } });
    assert_eq!(webauthn_json(true, made.to_string().as_bytes()), Ok(()));
    let data = passkey.data(0x01, 1, false);
    let used = json!({ "id": b64(b"cred"), "response": {
        "clientDataJSON": b64(&client_data(false)),
        "authenticatorData": b64(&data),
        "signature": b64(&passkey.sign(&data)),
    } });
    assert_eq!(webauthn_json(false, used.to_string().as_bytes()), Ok(()));
    let mut no_handle = used.clone();
    no_handle["response"]["userHandle"] = json!(b64(b"chase"));
    assert_eq!(webauthn_json(false, no_handle.to_string().as_bytes()), Ok(()));
    let mut bad = used.clone();
    bad["response"]["signature"] = json!("***");
    let mut no_challenge = made.clone();
    no_challenge["response"]["clientDataJSON"] = json!(b64(b"{}"));
    for (create, json) in [
        (false, bad.to_string()),
        (false, made.to_string()),
        (true, used.to_string()),
        (true, no_challenge.to_string()),
        (true, "{".into()),
        (false, "null".into()),
        (false, String::new()),
    ] {
        assert!(webauthn_json(create, json.as_bytes()).is_err(), "{create} {json}");
    }
}

/// A generator of numbers with a seed of its own (xorshift64*), so that a
/// run is the same each time and needs no crate.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }

    /// A number below `n`, which is not 0.
    fn below(&mut self, n: usize) -> usize {
        usize::try_from(self.next() % u64::try_from(n).unwrap()).unwrap()
    }

    /// True `percent` times in a hundred.
    fn chance(&mut self, percent: usize) -> bool {
        self.below(100) < percent
    }

    fn pick<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        &items[self.below(items.len())]
    }

    /// Up to `max` bytes: half of them from what the parsers look at,
    /// the rest from any.
    fn bytes(&mut self, max: usize) -> Vec<u8> {
        const SPECIAL: &[u8] = b"%/\\{}[]\",:;= \r\n\0.-_~*?#&<>'+";
        let length = self.below(max + 1);
        (0..length)
            .map(|_| match self.below(4) {
                0 => *self.pick(SPECIAL),
                1 => b'a' + u8::try_from(self.below(26)).unwrap(),
                _ => u8::try_from(self.next() & 0xff).unwrap(),
            })
            .collect()
    }

    /// Text of up to `max` characters, with some that are more than a
    /// byte.
    fn text(&mut self, max: usize) -> String {
        const SPECIAL: &[char] = &[
            '%',
            '/',
            '{',
            '}',
            '"',
            ',',
            ':',
            ';',
            '=',
            ' ',
            '\n',
            '\0',
            '.',
            '-',
            '\u{e9}',
            '\u{65e5}',
            '\u{1f600}',
        ];
        let length = self.below(max + 1);
        (0..length)
            .map(|_| {
                if self.chance(40) {
                    *self.pick(SPECIAL)
                } else {
                    char::from(b'a' + u8::try_from(self.below(26)).unwrap())
                }
            })
            .collect()
    }

    /// `bytes`, altered in a few places: a byte changed, one put in, one
    /// taken out, or the end cut off.
    fn mutate(&mut self, bytes: &[u8]) -> Vec<u8> {
        let mut out = bytes.to_vec();
        for _ in 0..=self.below(3) {
            if out.is_empty() {
                out.push(u8::try_from(self.next() & 0xff).unwrap());
                continue;
            }
            let at = self.below(out.len());
            match self.below(4) {
                0 => out[at] ^= 1 << self.below(8),
                1 => out.insert(at, u8::try_from(self.next() & 0xff).unwrap()),
                2 => {
                    out.remove(at);
                }
                _ => out.truncate(at),
            }
        }
        out
    }

    /// A JSON value of any kind, nested at most `depth` deep.
    fn json(&mut self, depth: usize) -> Value {
        match self.below(if depth == 0 { 5 } else { 7 }) {
            0 => Value::Null,
            1 => Value::Bool(self.chance(50)),
            2 => json!(
                i64::try_from(self.next() >> 1).unwrap() - i64::try_from(self.next() >> 2).unwrap()
            ),
            3 => json!(self.below(200)),
            4 => Value::String(self.text(24)),
            5 => Value::Array((0..self.below(4)).map(|_| self.json(depth - 1)).collect()),
            _ => Value::Object(
                (0..self.below(4)).map(|_| (self.text(8), self.json(depth - 1))).collect(),
            ),
        }
    }
}

/// A JSON object of arguments for the action `id`: its own names, mostly,
/// with values of the kind each takes, mostly.
fn arguments_for(rng: &mut Rng, id: &str) -> Value {
    let Some(action) = REGISTRY.iter().find(|action| action.id() == id) else {
        return rng.json(2);
    };
    let mut object = Map::new();
    for field in action.fields() {
        if !field.required && rng.chance(50) {
            continue;
        }
        let value = if rng.chance(15) {
            rng.json(1)
        } else {
            match field.kind {
                actions::Type::Text { min, max, .. } => {
                    let length = min + rng.below(max.min(40).saturating_sub(min) + 1);
                    let word: String = (0..length)
                        .map(|_| char::from(b'a' + u8::try_from(rng.below(26)).unwrap()))
                        .collect();
                    Value::String(word)
                }
                actions::Type::OneOf(words) => json!(rng.pick(words)),
                actions::Type::Number { min, max } => {
                    json!(min + i64::try_from(rng.below(10)).unwrap().min(max - min))
                }
                actions::Type::Flag => Value::Bool(rng.chance(50)),
            }
        };
        object.insert(field.name.to_string(), value);
    }
    if rng.chance(10) {
        object.insert(rng.text(6), rng.json(1));
    }
    Value::Object(object)
}

/// The paths a request asks for, and the few that answer anyone: the
/// routes, the actions' addresses with a name, and files.
fn paths() -> (Vec<String>, Vec<&'static str>) {
    let open =
        vec!["/", "/index.html", "/app.js", "/manifest.txt", "/api/1/health", "/api/1/fingerprint"];
    let routes = [
        "/api/1/sign-in",
        "/api/1/sign-in/code",
        "/api/1/sign-in/passkey/options",
        "/api/1/sign-in/passkey",
        "/api/1/session",
        "/api/1/sign-out",
        "/api/1/events",
        "/api/1/unlock",
        "/api/1/unlock/passkey/options",
        "/api/1/unlock/extend",
        "/api/1/lock",
        "/api/1/account/second-step",
        "/api/1/account/second-step/app/begin",
        "/api/1/account/second-step/app/confirm",
        "/api/1/account/second-step/passkey/add",
        "/api/1/account/second-step/codes",
        "/api/1/account/tokens/add",
        "/api/1/account/tokens/remove",
        "/api/1/account/preferences",
        "/api/1/actions",
        "/api/1/show",
        "/api/1/nothing",
        "/nothing",
        "/../etc/passwd",
    ];
    let mut paths: Vec<String> = open.iter().chain(&routes).map(ToString::to_string).collect();
    paths.extend(REGISTRY.iter().map(|action| action.path().replace("{name}", "nginx")));
    (paths, open)
}

/// Whether the route at `path` takes a `POST`: one of the actions that
/// change something, or one of the routes that sign in and out, unlock and
/// lock, and change the account.
fn takes_post(path: &str) -> bool {
    const POSTS: [&str; 7] =
        ["/sign-in", "/sign-out", "/unlock", "/lock", "/second-step/", "/tokens/", "/show"];
    find(true, path).is_some() || POSTS.iter().any(|part| path.contains(part))
}

/// The bytes of a request.
struct Sent {
    method: Vec<u8>,
    target: Vec<u8>,
    headers: Vec<(Vec<u8>, Vec<u8>)>,
    body: Vec<u8>,
}

/// The bytes of one request, made at random from what a browser or a
/// script sends, with a part of each wrong now and then.
fn random_request(rng: &mut Rng, paths: &[String]) -> Sent {
    let path = rng.pick(paths);
    // The method the route takes, mostly.
    let method = if rng.chance(5) {
        rng.bytes(8)
    } else if rng.chance(90) {
        (if takes_post(path) != rng.chance(8) { "POST" } else { "GET" }).into()
    } else {
        rng.pick(&[&b"HEAD"[..], b"PUT", b"DELETE", b"OPTIONS"]).to_vec()
    };
    let mut target = if rng.chance(92) { path.clone().into_bytes() } else { rng.bytes(40) };
    if rng.chance(15) {
        target.push(b'?');
        // Mostly a query that an address may have.
        let query = rng.bytes(20);
        let fit = rng.chance(85);
        target.extend(query.iter().map(|&byte| {
            if fit && !(byte.is_ascii_alphanumeric() || b"=&%-._~".contains(&byte)) {
                b'x'
            } else {
                byte
            }
        }));
    }
    let mut headers: Vec<(Vec<u8>, Vec<u8>)> = Vec::new();
    let mut put = |name: &str, value: Vec<u8>| headers.push((name.as_bytes().to_vec(), value));
    put("host", if rng.chance(92) { HOST.into() } else { rng.bytes(20) });
    match rng.below(10) {
        0..=6 => put("origin", ORIGIN.into()),
        7 => put("origin", rng.bytes(30)),
        _ => {}
    }
    if rng.chance(85) {
        put(
            "content-type",
            if rng.chance(92) { b"application/json".to_vec() } else { rng.bytes(16) },
        );
    }
    if rng.chance(50) {
        put(
            "sec-fetch-site",
            rng.pick(&[&b"same-origin"[..], b"cross-site", b"none", b"same-site"]).to_vec(),
        );
    }
    if rng.chance(25) {
        let mut cookie = format!("{SESSION_COOKIE}={}", rng.text(70)).into_bytes();
        if rng.chance(30) {
            cookie.extend_from_slice(format!("; {TRUST_COOKIE}=aa55bb66").as_bytes());
        }
        put("cookie", cookie);
    }
    if rng.chance(20) {
        let token = format!("Bearer dkv_chase_{}", rng.text(60));
        put("authorization", if rng.chance(80) { token.into_bytes() } else { rng.bytes(30) });
    }
    if rng.chance(20) {
        put("x-dukovo-csrf", rng.bytes(40));
    }
    for _ in 0..rng.below(3) {
        let name = format!("x-{}", rng.text(6).replace(|c: char| !c.is_ascii_alphanumeric(), "z"));
        put(&name, rng.bytes(20));
    }
    // The values are mostly such as a header may have, which has no control
    // character; those of a request now and then are not.
    if !rng.chance(4) {
        for byte in headers.iter_mut().flat_map(|(_, value)| value.iter_mut()) {
            if (*byte < b' ' && *byte != b'\t') || *byte == 0x7f {
                *byte = b'_';
            }
        }
    }
    let body = if method == b"GET" || rng.chance(25) {
        Vec::new()
    } else if rng.chance(25) {
        rng.bytes(120)
    } else {
        let ids = action_ids();
        let id = *rng.pick(&ids);
        match rng.below(4) {
            0 => json!({ "name": rng.text(12), "password": rng.text(12) }),
            1 => json!({ "ticket": rng.text(40), "code": rng.text(8), "trust": rng.chance(50) }),
            2 => rng.json(3),
            _ => arguments_for(rng, id),
        }
        .to_string()
        .into_bytes()
    };
    Sent { method, target, headers, body }
}

/// Calls each parser's wrapper with inputs made at random: some right
/// and altered, some noise.
fn random_parsers(rng: &mut Rng, passkey: &Authenticator, ids: &[&'static str]) {
    let noise = |rng: &mut Rng| rng.bytes(60);
    // What the passkey checks read.
    let created = passkey.data(0x41, 0, true);
    let used = passkey.data(0x01, 1, false);
    let object = Authenticator::attestation(&created);
    let samples = [
        client_data(true),
        client_data(false),
        created,
        used.clone(),
        object.clone(),
        passkey.cose(),
        passkey.sign(&used),
    ];
    let sample = |rng: &mut Rng| -> Vec<u8> {
        if rng.chance(15) {
            noise(rng)
        } else {
            let picked = rng.pick(&samples).clone();
            rng.mutate(&picked)
        }
    };
    let text = |rng: &mut Rng| rng.text(50);
    // Addresses and arguments.
    let path = if rng.chance(60) {
        let (paths, _) = paths();
        let mut path = rng.pick(&paths).replace("nginx", &rng.text(10).replace('/', "%2F"));
        if rng.chance(30) {
            path.push_str(&rng.text(6));
        }
        path
    } else {
        text(rng)
    };
    let _ = find(rng.chance(50), &path);
    let _ = decode(&text(rng));
    let pair = (rng.text(6), rng.text(10));
    let params = if rng.chance(50) { vec![(pair.0.as_str(), pair.1.as_str())] } else { Vec::new() };
    let body = match rng.below(3) {
        0 => noise(rng),
        1 => rng.json(2).to_string().into_bytes(),
        _ => Vec::new(),
    };
    let _ = arguments(&params, &body);
    let id = if rng.chance(95) { *rng.pick(ids) } else { "x" };
    let args = if rng.chance(70) {
        arguments_for(rng, id).to_string().into_bytes()
    } else {
        rng.json(2).to_string().into_bytes()
    };
    let _ = prepare(id, &args);
    // Headers.
    let values: Vec<Vec<u8>> = (0..rng.below(3))
        .map(|_| {
            if rng.chance(60) {
                format!("Bearer dkv_{}", rng.text(20)).into_bytes()
            } else {
                noise(rng)
            }
        })
        .collect();
    let values: Vec<&[u8]> = values.iter().map(Vec::as_slice).collect();
    let _ = bearer(&values);
    let _ = cookie(&values, if rng.chance(50) { SESSION_COOKIE } else { TRUST_COOKIE });
    let _ = scopes(&text(rng));
    let _ = scopes(
        &["services", ":read", ",", "*", "a-b"]
            .iter()
            .map(|part| if rng.chance(50) { (*part).to_string() } else { text(rng) })
            .collect::<Vec<_>>()
            .join(","),
    );
    // Preferences.
    let change = match rng.below(3) {
        0 => noise(rng),
        1 => rng.json(2).to_string().into_bytes(),
        _ => json!({ "theme": rng.pick(&["dark", "light", "x"]), "text": rng.pick(&[100, 112, 99]), "zone": rng.text(8) }).to_string().into_bytes(),
    };
    let _ = preferences_body(&change);
    let settings = [
        "theme = \"dark\"\n",
        "text = 112 # size\n",
        "# note\n",
        "[a]\n",
        "x = [1, 2\n",
        "zone = \"\"\n",
        "motion = 'less'\n",
    ];
    let file: String = (0..rng.below(4))
        .map(|_| if rng.chance(85) { (*rng.pick(&settings)).to_string() } else { rng.text(12) })
        .collect();
    let _ = settings_text(&file, &change);
    // What a passkey sends.
    let (data, create) = (sample(rng), rng.chance(50));
    let _ = webauthn_client_data(create, &data);
    let _ = webauthn_authenticator_data(create, rng.chance(50), &sample(rng));
    let _ = webauthn_key(&sample(rng));
    let _ = webauthn_attestation(&sample(rng));
    let _ = webauthn_assertion(&sample(rng), &sample(rng), &sample(rng));
    let _ = webauthn_challenge(&data);
    let b64 = |bytes: &[u8]| URL_SAFE_NO_PAD.encode(bytes);
    let json = if create {
        json!({ "response": { "clientDataJSON": b64(&data), "attestationObject": b64(&sample(rng)) } })
    } else {
        json!({ "id": b64(&sample(rng)), "response": { "clientDataJSON": b64(&data), "authenticatorData": b64(&sample(rng)), "signature": b64(&sample(rng)) } })
    };
    let _ = webauthn_json(create, json.to_string().as_bytes());
    let _ = webauthn_json(create, &sample(rng));
}

#[test]
fn a_few_thousand_random_inputs_through_every_entry_point_do_not_panic() {
    let console = Console::new();
    let passkey = Authenticator::new();
    let ids = action_ids();
    let (paths, open) = paths();
    let mut rng = Rng(0x5eed_c2c0_ffee_1234);
    let mut answered = std::collections::BTreeMap::new();
    for _ in 0..3000 {
        let Sent { method, target, headers, body } = random_request(&mut rng, &paths);
        let headers: Vec<(&[u8], &[u8])> =
            headers.iter().map(|(name, value)| (name.as_slice(), value.as_slice())).collect();
        let (status, _) = console.request(&method, &target, &headers, &body);
        *answered.entry(status).or_insert(0) += 1;
        // Nobody is signed in and no token is right: what is answered 2xx
        // is what is open to anyone, the health and the fingerprint, and
        // a page of the console, whose first page answers a name that has
        // no file.
        if (200..300).contains(&status) {
            let path = String::from_utf8_lossy(&target);
            let path = path.split('?').next().unwrap_or_default();
            let page = matches!(method.as_slice(), b"GET" | b"HEAD") && !path.starts_with("/api/");
            assert!(page || open.contains(&path), "{status} for {method:?} {path}");
        }
        random_parsers(&mut rng, &passkey, &ids);
    }
    // The requests reached admind's checks, and not only the first of them.
    assert!(answered.len() >= 6, "{answered:?}");
    assert!(answered.get(&401).copied().unwrap_or(0) > 100, "{answered:?}");
    assert!(answered.get(&200).copied().unwrap_or(0) > 100, "{answered:?}");
    // Some asked authd, which refused.
    assert!(answered.get(&403).copied().unwrap_or(0) > 20, "{answered:?}");
}
