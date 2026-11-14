// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

use super::super::proofs::tests::{
    CHALLENGE, ORIGIN, PARTY, Signer, UP_UV, attestation, attested, authenticator_data, client_data,
};
use super::*;

/// A peer for the tests: a process of root's.
fn admind(process: u64) -> Peer {
    Peer { process, user: 0 }
}

/// An account's file with one of everything.
fn full() -> Store {
    Store {
        handle: "00112233445566778899aabbccddeeff".into(),
        app: Some(App {
            secret: proofs::base32(b"12345678901234567890"),
            added: 1_794_500_000,
            last_step: Some(59_816_666),
        }),
        passkeys: vec![Passkey {
            id: "6964".into(),
            key: "a1".into(),
            algorithm: -7,
            relying_party: "duke-nas.local".into(),
            name: "YubiKey \"5C\" \\ ü".into(),
            count: 41,
            added: 1_794_500_000,
            used: 1_794_560_000,
        }],
        recovery: Some(Recovery { codes: vec!["aa".into(), "bb".into()], made: 1_794_500_000 }),
        trusted: vec![Trusted {
            hash: "cc".into(),
            until: 1_797_092_000,
            from: "Firefox on Linux".into(),
        }],
        tokens: vec![Token {
            name: "ansible".into(),
            hash: "dd".into(),
            scopes: vec!["*".into(), "services".into()],
            until: 1_797_092_000,
            made: 1_794_500_000,
            used: 0,
        }],
    }
}

#[test]
fn an_accounts_file_reads_back_as_it_was_written() {
    let store = full();
    let text = store.text();
    let read = Store::parse(&text).unwrap();
    assert_eq!(read.text(), text);
    assert_eq!(read.passkeys[0].name, "YubiKey \"5C\" \\ ü");
    assert_eq!(read.app.as_ref().unwrap().last_step, Some(59_816_666));
    assert!(Store::parse("").unwrap().is_empty());
    assert_eq!(Store::default().text(), "");
    // A key it does not know is refused, so a typo is never read as
    // nothing.
    assert!(Store::parse("handel = \"00\"\n").is_err());
    assert!(Store::parse("[app]\nsecret = \"A\"\n").is_err());
}

#[test]
fn a_sign_in_asks_for_a_second_step_by_the_rules() {
    let none = Store::default();
    let with_app = Store { app: full().app, ..Store::default() };
    for admin in [false, true] {
        assert_eq!(stage_for(SecondStep::IfSet, admin, &none), Stage::Done("password".into()));
        assert_eq!(stage_for(SecondStep::Everyone, admin, &none), Stage::NeedsStep);
        // One who has a step gives it, whatever the rules.
        for rule in [SecondStep::IfSet, SecondStep::Administrators, SecondStep::Everyone] {
            assert_eq!(stage_for(rule, admin, &with_app), Stage::Step("app".into()));
        }
    }
    assert_eq!(stage_for(SecondStep::Administrators, false, &none), Stage::Done("password".into()));
    assert_eq!(stage_for(SecondStep::Administrators, true, &none), Stage::NeedsStep);
    assert_eq!(full().kinds(), "app,passkey,recovery");
    let used_up = Store {
        passkeys: full().passkeys,
        recovery: Some(Recovery { codes: Vec::new(), made: 0 }),
        ..Store::default()
    };
    assert_eq!(used_up.kinds(), "passkey");
}

#[test]
fn passkeys_need_the_machines_name() {
    for name in ["duke-nas.local", "localhost", "a.b.example.org", "x1.local", "admin.example.org"]
    {
        assert!(relying_party_ok(name), "{name}");
    }
    let long = format!("{}.local", "a".repeat(250));
    for name in [
        "",
        "192.168.1.5",
        "10.0.0.1",
        "::1",
        "[::1]",
        "fe80::1",
        "a..b",
        "-a.local",
        "a-.local",
        "Duke.local",
        "x.123",
        "duke-nas.local.",
        long.as_str(),
        "duke nas.local",
        "duke_nas.local",
    ] {
        assert!(!relying_party_ok(name), "{name}");
    }
}

#[test]
fn names_labels_and_scopes() {
    assert!(label_ok("YubiKey 5C") && label_ok("Pixel 9 — Chase") && label_ok(&"é".repeat(64)));
    assert!(!label_ok("") && !label_ok("   ") && !label_ok("a\nb") && !label_ok(&"é".repeat(65)));
    assert!(token_name_ok("ansible") && token_name_ok("ci-2") && token_name_ok(&"a".repeat(32)));
    assert!(!token_name_ok("") && !token_name_ok("Ansible") && !token_name_ok("a_b"));
    assert!(!token_name_ok(&"a".repeat(33)));
    assert!(scope_ok("*") && scope_ok("services") && scope_ok("dns-records"));
    assert!(scope_ok("services:read"));
    assert!(!scope_ok("") && !scope_ok("**") && !scope_ok("1abc") && !scope_ok("Services"));
    assert!(!scope_ok(":read") && !scope_ok("services:write") && !scope_ok("*:read"));
    assert_eq!(address("192.0.2.7"), Some("192.0.2.7"));
    assert_eq!(address("2001:db8::1"), Some("2001:db8::1"));
    assert_eq!(address(""), None);
    assert_eq!(address("a b"), None);
    assert_eq!(address(&"1".repeat(65)), None);
}

#[test]
fn an_api_token_names_its_owner() {
    let secret = "0123456789abcdef".repeat(4);
    let token = format!("dkv_chase_{secret}");
    assert_eq!(token_owner(token.as_bytes()), Some(("chase", secret.as_str())));
    // A name with an underscore: the secret is after the last one.
    let token = format!("dkv_build_bot_{secret}");
    assert_eq!(token_owner(token.as_bytes()), Some(("build_bot", secret.as_str())));
    for wrong in [
        format!("dkv_chase_{}", secret.to_uppercase()),
        format!("dkv_chase_{}", &secret[1..]),
        format!("dkv_chase_{secret}0"),
        format!("dkx_chase_{secret}"),
        format!("dkv_{secret}"),
        format!("dkv_Chase_{secret}"),
        format!("dkv__{secret}"),
    ] {
        assert_eq!(token_owner(wrong.as_bytes()), None, "{wrong}");
    }
    assert_eq!(token_owner(b"dkv_chase_\xff"), None);
}

#[test]
fn a_code_is_taken_once_and_a_recovery_code_is_used_up() {
    let secret = b"12345678901234567890";
    let mut store = Store {
        app: Some(App { secret: proofs::base32(secret), added: 0, last_step: None }),
        ..Store::default()
    };
    let codes = store.new_codes(100).unwrap();
    assert_eq!(codes.len(), RECOVERY_CODES);
    let now = 1_794_500_000_i64;
    let code = format!("{:06}", proofs::app_code(secret, now as u64 / 30));
    assert_eq!(store.take_code(&code, now, true), Ok("app code".into()));
    assert_eq!(store.app.as_ref().unwrap().last_step, Some(now as u64 / 30));
    // The same code again is a replay.
    assert_eq!(store.take_code(&code, now, true), Err(CodeRefusal::Used));
    let next = format!("{:06}", proofs::app_code(secret, now as u64 / 30 + 1));
    assert_eq!(store.take_code(&next, now + 30, true), Ok("app code".into()));
    let wrong = if code == "000000" { "000001" } else { "000000" };
    assert_eq!(store.take_code(wrong, now + 60, true), Err(CodeRefusal::Wrong));
    assert_eq!(store.take_code("12345", now, true), Err(CodeRefusal::NotACode));
    // A recovery code, typed in lower case with spaces, is used once.
    let typed = codes[3].to_lowercase().replace('-', " ");
    assert_eq!(store.take_code(&typed, now, true), Ok("a recovery code, 9 left".into()));
    assert_eq!(store.take_code(&codes[3], now, true), Err(CodeRefusal::Wrong));
    // Unlocking takes the app's code alone.
    assert_eq!(store.take_code(&codes[4], now, false), Err(CodeRefusal::NotACode));
    assert_eq!(store.take_code(&codes[4], now, true), Ok("a recovery code, 8 left".into()));
    // Without an app, six digits are wrong, not a crash.
    let mut bare = Store::default();
    assert_eq!(bare.take_code("123456", now, true), Err(CodeRefusal::Wrong));
    assert_eq!(bare.take_code(&codes[5], now, true), Err(CodeRefusal::Wrong));
}

#[test]
fn recovery_codes_go_with_the_last_second_step() {
    let mut store = full();
    store.app = None;
    store.tidy_steps();
    assert!(store.recovery.is_some());
    store.passkeys.clear();
    store.tidy_steps();
    assert!(store.recovery.is_none());
    assert!(!store.has_step());
}

#[test]
fn a_browser_is_trusted_for_30_days_by_its_token() {
    let mut store = Store::default();
    let now = 1_794_500_000;
    store.trust(b"token one", "Firefox on Linux", now);
    assert!(store.trusts(b"token one", now + TRUSTED_FOR - 1));
    assert!(!store.trusts(b"token one", now + TRUSTED_FOR));
    assert!(!store.trusts(b"token two", now));
    assert!(!store.text().contains("token one"));
    // The oldest goes once there are as many as are kept.
    for n in 0..TRUSTED_MAX {
        store.trust(format!("token {n}").as_bytes(), "a\nbrowser", now + 1 + n as i64);
    }
    assert_eq!(store.trusted.len(), TRUSTED_MAX);
    assert!(!store.trusts(b"token one", now + 1));
    assert!(store.trusts(b"token 0", now + 1));
    assert_eq!(store.trusted[0].from, "a?browser");
}

#[test]
fn an_api_token_is_found_by_its_secret_until_it_expires() {
    let mut store = Store::default();
    let secret = "ab".repeat(32);
    store.tokens.push(Token {
        name: "ansible".into(),
        hash: hex(&sha256(secret.as_bytes())),
        scopes: vec!["*".into()],
        until: 2_000,
        made: 1_000,
        used: 0,
    });
    assert_eq!(store.token(&secret, 1_999).map(|t| t.name.clone()), Some("ansible".into()));
    assert!(store.token(&secret, 2_000).is_none());
    assert!(store.token(&"ac".repeat(32), 1_500).is_none());
}

/// A passkey of `signer`'s, kept as `passkey-add` keeps one.
fn kept(signer: &Signer) -> Passkey {
    let data = authenticator_data(PARTY, UP_UV | 0x40, 0, &attested(&signer.cose()));
    let expected = Expected {
        ceremony: Ceremony::Create,
        challenge: &CHALLENGE,
        origin: ORIGIN,
        relying_party: PARTY,
        verified: false,
    };
    let new = proofs::check_registration(
        &expected,
        &client_data("webauthn.create", &CHALLENGE, ORIGIN),
        &attestation(&data),
    )
    .unwrap();
    Passkey {
        id: hex(&new.id),
        key: hex(&new.key),
        algorithm: new.algorithm,
        relying_party: PARTY.into(),
        name: "YubiKey 5C".into(),
        count: new.count,
        added: 0,
        used: 0,
    }
}

/// An assertion by `signer` with `flags` and `count`.
fn assertion(signer: &Signer, id: &[u8], flags: u8, count: u32) -> Assertion {
    let client = client_data("webauthn.get", &CHALLENGE, ORIGIN);
    let data = authenticator_data(PARTY, flags, count, &[]);
    let signed = signer.sign(&[data.clone(), sha256(&client).to_vec()].concat());
    Assertion {
        challenge: CHALLENGE.to_vec(),
        origin: ORIGIN.into(),
        relying_party: PARTY.into(),
        credential: id.to_vec(),
        client_data: client,
        authenticator_data: data,
        signature: signed,
        user_handle: Vec::new(),
    }
}

#[test]
fn a_passkey_is_used_by_its_account_and_its_count_kept() {
    let signer = Signer::new(proofs::ES256);
    let mut store = Store { passkeys: vec![kept(&signer)], ..Store::default() };
    let id = from_hex(store.passkeys[0].id.as_bytes()).unwrap();
    // Present but not verified: a second step, not unlock.
    let touched = assertion(&signer, &id, 0x01, 5);
    assert_eq!(store.use_passkey(&touched, false, 77), Ok("YubiKey 5C".into()));
    assert_eq!((store.passkeys[0].count, store.passkeys[0].used), (5, 77));
    let touched_again = assertion(&signer, &id, 0x01, 6);
    assert!(store.use_passkey(&touched_again, true, 78).unwrap_err().contains("PIN"));
    // A count that does not rise may be a copied key: refused, and the
    // count kept.
    let copied = assertion(&signer, &id, UP_UV, 5);
    assert!(store.use_passkey(&copied, true, 79).unwrap_err().contains("copied"));
    assert_eq!(store.passkeys[0].count, 5);
    assert_eq!(
        store.use_passkey(&assertion(&signer, &id, UP_UV, 9), true, 80),
        Ok("YubiKey 5C".into())
    );
    // Another key, or the same ID for another site, is not this account's.
    let other = Signer::new(proofs::EDDSA);
    assert!(store.use_passkey(&assertion(&other, b"other", UP_UV, 10), true, 81).is_err());
    let mut elsewhere = assertion(&signer, &id, UP_UV, 10);
    elsewhere.relying_party = "other.local".into();
    assert!(store.use_passkey(&elsewhere, true, 81).unwrap_err().contains("not one of"));
    // A signature by another key under this ID is refused.
    let forged = assertion(&other, &id, UP_UV, 11);
    assert!(store.use_passkey(&forged, true, 82).is_err());
    assert_eq!(store.passkeys[0].count, 9);
}

#[test]
fn steps_tell_no_secret() {
    let store = full();
    let answer = steps(&store, true, 1_794_600_000);
    let json: serde_json::Value = serde_json::from_str(&answer).unwrap();
    assert_eq!(json["app"]["added"], 1_794_500_000);
    assert_eq!(json["passkeys"][0]["name"], "YubiKey \"5C\" \\ ü");
    assert_eq!(json["recovery"]["left"], 2);
    assert_eq!(json["trusted"], 1);
    assert_eq!(json["tokens"][0]["expired"], false);
    assert_eq!(json["needed"], true);
    for secret in [store.app.as_ref().unwrap().secret.as_str(), "\"aa\"", "\"cc\"", "\"dd\"", "a1"]
    {
        assert!(!answer.contains(secret), "{secret}");
    }
    assert!(!answer.contains('\n'));
}

#[test]
fn challenges_are_used_once_by_whom_they_were_given_to() {
    let mut state = State::default();
    let now = Instant::now();
    let challenge = |process| Challenge {
        purpose: For::Alone("192.0.2.7".into()),
        relying_party: PARTY.into(),
        process,
        given: now,
    };
    let bytes = state.add_challenge(challenge(10)).unwrap();
    assert_eq!(bytes.len(), 32);
    // Another process's try uses it up too.
    assert!(state.take_challenge(&bytes, admind(11), now).is_none());
    assert!(state.take_challenge(&bytes, admind(10), now).is_none());
    let bytes = state.add_challenge(challenge(10)).unwrap();
    assert!(state.take_challenge(&bytes, admind(10), now).is_some());
    assert!(state.take_challenge(&bytes, admind(10), now).is_none());
    // Too old.
    let bytes = state.add_challenge(challenge(10)).unwrap();
    assert!(state.take_challenge(&bytes, admind(10), now + SIGN_IN_TIME).is_none());
    // The oldest goes once there are as many as are kept.
    let first = state.add_challenge(challenge(10)).unwrap();
    for _ in 1..CHALLENGES_MAX {
        state
            .add_challenge(Challenge { given: now + Duration::from_secs(1), ..challenge(10) })
            .unwrap();
    }
    assert_eq!(state.challenges.len(), CHALLENGES_MAX);
    state
        .add_challenge(Challenge { given: now + Duration::from_secs(2), ..challenge(10) })
        .unwrap();
    assert_eq!(state.challenges.len(), CHALLENGES_MAX);
    assert!(!state.challenges.contains_key(&first));
}

#[test]
fn a_sign_in_is_its_admind_process_and_lasts_5_minutes() {
    let mut state = State::default();
    let now = Instant::now();
    let sign_in = SignIn {
        name: "chase".into(),
        from: "192.0.2.7".into(),
        process: 10,
        given: now,
        stage: Stage::Step("app".into()),
    };
    let ticket = state.add_sign_in(sign_in).unwrap();
    assert!(state.sign_in(&ticket, admind(10), now).is_some());
    assert!(state.sign_in(&ticket, admind(11), now).is_none());
    assert!(state.sign_in(&ticket, admind(10), now + SIGN_IN_TIME).is_none());
    let asked = Console::Code { ticket: ticket.clone(), code: b"123456".to_vec() };
    assert_eq!(
        state.turn(&asked, admind(10), "root"),
        Some(("chase".into(), "web 192.0.2.7".into()))
    );
    state.tidy(now + SIGN_IN_TIME);
    assert!(state.sign_ins.is_empty());
}

#[test]
fn wrong_tries_are_counted_for_the_account_and_where_they_come_from() {
    let state = State::default();
    let peer = Peer { process: 50, user: 1000 };
    let begin =
        Console::Begin { from: "192.0.2.7".into(), name: "chase".into(), password: b"x".to_vec() };
    assert_eq!(
        state.turn(&begin, admind(10), "root"),
        Some(("chase".into(), "web 192.0.2.7".into()))
    );
    let token = Console::Token { token: b"dkv_x".to_vec(), from: "192.0.2.8".into() };
    assert_eq!(
        state.turn(&token, admind(10), "root"),
        Some((String::new(), "web 192.0.2.8".into()))
    );
    let with_password = Console::Codes { proof: b"secret".to_vec() };
    assert_eq!(
        state.turn(&with_password, peer, "chase"),
        Some(("chase".into(), "user 1000".into()))
    );
    // Proved by an unlocked session, or asking nothing: no wait.
    assert_eq!(state.turn(&Console::Codes { proof: Vec::new() }, peer, "chase"), None);
    assert_eq!(state.turn(&Console::Steps, peer, "chase"), None);
}

#[test]
fn the_console_runs_the_systems_commands_and_never_a_shell() {
    let words = |line: &str| line.split(' ').map(|w| w.as_bytes().to_vec()).collect::<Vec<_>>();
    let refusal =
        |line: &str, program: &str| console_command_refusal(&words(line), program.as_bytes());
    assert_eq!(refusal("service restart nginx", "/system/commands/service"), None);
    assert_eq!(refusal("tool", "/applications/commands/tool"), None);
    for (line, program) in [
        ("sh -c id", "/system/commands/sh"),
        ("nukesh", "/system/commands/nukesh"),
        ("bash", "/applications/commands/bash"),
        ("/tmp/x", "/tmp/x"),
        ("/system/commands/../../tmp/x", "/system/commands/../../tmp/x"),
        ("/system/commands/./x", "/system/commands/./x"),
        ("/system/commands//x", "/system/commands//x"),
        ("/users/chase/bin/x", "/users/chase/bin/x"),
    ] {
        assert!(refusal(line, program).is_some(), "{line}");
    }
    let many = format!("echo{}", " a".repeat(ARGUMENTS_MAX + 1));
    assert!(refusal(&many, "/system/commands/echo").is_some());
    let enough = format!("echo{}", " a".repeat(ARGUMENTS_MAX));
    assert_eq!(refusal(&enough, "/system/commands/echo"), None);
    let long = format!("echo {}", "a".repeat(COMMAND_BYTES_MAX));
    assert!(refusal(&long, "/system/commands/echo").is_some());
}
