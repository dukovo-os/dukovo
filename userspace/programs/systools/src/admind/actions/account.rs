// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Your account's actions (ADR 0515, ADR 0561): the person's own account
//! whole, their full name, their password and their SSH keys. The area is
//! `account`, the scope a token needs; the accounts of everyone are
//! [`super::accounts`]'s.
//!
//! Each runs as the person, with the person's own rights: no command here
//! names an account, so none can reach another's. `user show` and `user set
//! --full-name` without a name, and `user key` with one operand and `passwd`
//! with none, are the commands' own way to say "mine".
//!
//! Neither passwords nor keys are in a command's words (ADR 0561): the two
//! passwords of a change are read on the command's input, a line each, as
//! `passwd` reads them from a pipe: the current one first, then the new one.
//!
//! The actions cannot name the file of keys: it is `/users/NAME/.ssh/...`
//! and an action does not know whose it is. Adding and removing a key
//! compare what `user show` says of the keys, which the person may read.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::accounts::{
    EXAMPLE_FINGERPRINT, EXAMPLE_KEY, FINGERPRINT, FULL_NAME, LINE, TWO_PASSWORDS_CHARS,
    check_fingerprint, check_full_name, check_key_line, check_password,
};
use super::{Action, Args, Bad, Examples, Field, Nothing, Runs, Secret, Tests, Undo};

/// The area of these actions: the scope a token needs, `account`.
const AREA: &str = "account";

/// The tests of these actions, which `lab/actions.toml` names.
const TESTS: Tests = Tests {
    checks: "admind::actions::account::tests",
    route: "admin-accounts",
    same_as_command: "admin-accounts",
    browser: "web-console/accounts",
    ansible: "web-console/ansible",
};

/// The command that reads what changing a name or a key moves: the
/// person's own account, as `user show --json` gives it.
fn show_argv() -> Vec<String> {
    ["user", "show", "--json"].map(String::from).to_vec()
}

/// The part `key` of what `user show --json` says, as text to compare, or
/// None for output that is not that object.
fn picked(key: &str, output: &str) -> Option<String> {
    let shown: Value = serde_json::from_str(output).ok()?;
    shown.get(key).map(Value::to_string)
}

/// `user show --json`: the person's own account whole, with their keys,
/// second steps, tokens and sessions at the machine.
pub struct Show;

impl Action for Show {
    const ID: &'static str = "account.show";
    const AREA: &'static str = AREA;
    const CHANGES: bool = false;
    const PATH: &'static str = "/api/1/account/show";
    const RUNS: Runs = Runs::AsThePerson;
    const TESTS: Tests = TESTS;
    const TITLE: &'static str = "Show your account";
    type Args = Nothing;

    fn argv(_: &Nothing) -> Vec<String> {
        show_argv()
    }

    fn examples() -> Examples<Nothing> {
        Examples { good: vec![Nothing {}], bad: vec![json!({ "all": true })], undo: None }
    }
}

/// A full name for the person's own account.
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct FullName {
    pub full_name: String,
}

impl Args for FullName {
    const FIELDS: &'static [Field] = &[FULL_NAME];

    fn check(&self) -> Result<(), Bad> {
        check_full_name(&self.full_name)
    }
}

/// `user set --full-name TEXT`: changes the person's own full name, which
/// authd lets anyone do. An empty one clears it.
pub struct Name;

impl Action for Name {
    const ID: &'static str = "account.name";
    const AREA: &'static str = AREA;
    const CHANGES: bool = true;
    const PATH: &'static str = "/api/1/account/name";
    const RUNS: Runs = Runs::AsThePerson;
    const TESTS: Tests = TESTS;
    const TITLE: &'static str = "Change your full name";
    type Args = FullName;

    fn argv(args: &FullName) -> Vec<String> {
        vec!["user".into(), "set".into(), "--full-name".into(), args.full_name.clone()]
    }

    fn files(_: &FullName) -> Vec<String> {
        vec![accounts::PASSWD.to_string()]
    }

    fn probe(_: &FullName) -> Option<Vec<String>> {
        Some(show_argv())
    }

    fn probed(_: &FullName, output: &str) -> Option<String> {
        picked("account", output)
    }

    fn examples() -> Examples<FullName> {
        Examples {
            good: vec![FullName { full_name: "Lab reader".into() }],
            bad: vec![
                json!({ "full_name": "a:b" }),
                json!({ "full_name": "two\nlines" }),
                json!({}),
            ],
            undo: Some(Undo { action: "account.name", args: json!({ "full_name": "" }) }),
        }
    }
}

/// The current password and the new one, for the person's own account.
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Changing {
    pub current: Secret,
    pub new: Secret,
}

impl Args for Changing {
    const FIELDS: &'static [Field] = &[
        Field::secret("current", "Your password now.", TWO_PASSWORDS_CHARS),
        Field::secret("new", "The password to have from now on.", TWO_PASSWORDS_CHARS),
    ];

    fn check(&self) -> Result<(), Bad> {
        check_password("current", &self.current)?;
        check_password("new", &self.new)
    }
}

/// `passwd`: changes the person's own password, with the current one and
/// the new one on its input. A wrong current password is authd's refusal,
/// after its wait.
pub struct Password;

impl Action for Password {
    const ID: &'static str = "account.password";
    const AREA: &'static str = AREA;
    const CHANGES: bool = true;
    const PATH: &'static str = "/api/1/account/password";
    const RUNS: Runs = Runs::AsThePerson;
    const TESTS: Tests = TESTS;
    const TITLE: &'static str = "Change your password";
    type Args = Changing;

    fn argv(_: &Changing) -> Vec<String> {
        vec!["passwd".into()]
    }

    /// The two lines `passwd` reads from a pipe, in the order it asks.
    fn input(args: &Changing) -> Option<Secret> {
        Some(Secret::lines(&[&args.current, &args.new]))
    }

    fn examples() -> Examples<Changing> {
        Examples {
            good: vec![Changing {
                current: Secret::from("lab-pass-1".to_string()),
                new: Secret::from("lab-pass-2".to_string()),
            }],
            bad: vec![
                json!({ "current": "lab-pass-1" }),
                json!({ "new": "lab-pass-2" }),
                json!({ "current": "", "new": "lab-pass-2" }),
            ],
            undo: None,
        }
    }
}

/// A key to add to the person's own account.
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Line {
    pub line: String,
}

impl Args for Line {
    const FIELDS: &'static [Field] = &[LINE];

    fn check(&self) -> Result<(), Bad> {
        check_key_line(&self.line)
    }
}

/// `user key add LINE`: adds a key to the person's own `authorized_keys`.
pub struct KeyAdd;

impl Action for KeyAdd {
    const ID: &'static str = "account.key-add";
    const AREA: &'static str = AREA;
    const CHANGES: bool = true;
    const PATH: &'static str = "/api/1/account/keys/add";
    const RUNS: Runs = Runs::AsThePerson;
    const ERRORS: &'static [&'static str] = &["ACC-103"];
    const TESTS: Tests = TESTS;
    const TITLE: &'static str = "Add an SSH key to your account";
    type Args = Line;

    fn argv(args: &Line) -> Vec<String> {
        vec!["user".into(), "key".into(), "add".into(), args.line.clone()]
    }

    fn probe(_: &Line) -> Option<Vec<String>> {
        Some(show_argv())
    }

    fn probed(_: &Line, output: &str) -> Option<String> {
        picked("keys", output)
    }

    fn examples() -> Examples<Line> {
        Examples {
            good: vec![Line { line: EXAMPLE_KEY.into() }],
            bad: vec![
                json!({ "line": "ssh-dss AAAAB3NzaC1kc3MAAACBAP" }),
                json!({ "line": format!("command=\"x\" {EXAMPLE_KEY}") }),
                json!({}),
            ],
            undo: Some(Undo {
                action: "account.key-remove",
                args: json!({ "fingerprint": EXAMPLE_FINGERPRINT }),
            }),
        }
    }
}

/// A key to take from the person's own account.
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Print {
    pub fingerprint: String,
}

impl Args for Print {
    const FIELDS: &'static [Field] = &[FINGERPRINT];

    fn check(&self) -> Result<(), Bad> {
        check_fingerprint(&self.fingerprint)
    }
}

/// `user key remove FINGERPRINT`: takes a key from the person's own
/// `authorized_keys`.
pub struct KeyRemove;

impl Action for KeyRemove {
    const ID: &'static str = "account.key-remove";
    const AREA: &'static str = AREA;
    const CHANGES: bool = true;
    const PATH: &'static str = "/api/1/account/keys/remove";
    const RUNS: Runs = Runs::AsThePerson;
    const ERRORS: &'static [&'static str] = &["ACC-104"];
    const TESTS: Tests = TESTS;
    const TITLE: &'static str = "Remove an SSH key from your account";
    type Args = Print;

    fn argv(args: &Print) -> Vec<String> {
        vec!["user".into(), "key".into(), "remove".into(), args.fingerprint.clone()]
    }

    fn probe(_: &Print) -> Option<Vec<String>> {
        Some(show_argv())
    }

    fn probed(_: &Print, output: &str) -> Option<String> {
        picked("keys", output)
    }

    fn examples() -> Examples<Print> {
        Examples {
            good: vec![Print { fingerprint: EXAMPLE_FINGERPRINT.into() }],
            bad: vec![
                json!({ "fingerprint": "SHA256:short" }),
                json!({ "fingerprint": "MD5:00" }),
                json!({}),
            ],
            undo: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::{Caller, Found, REGISTRY, Registered, authorize, find};
    use super::*;
    use crate::admind::errors::Kind;
    use crate::admind::scopes::Scopes;

    fn action(id: &str) -> &'static dyn Registered {
        *REGISTRY.iter().find(|action| action.id() == id).unwrap()
    }

    /// Why `id` refuses `args`: the kind, the field and the words.
    fn refused(id: &str, args: Value) -> (Kind, Option<&'static str>, String) {
        let failure = action(id).prepare(args).unwrap_err();
        (failure.kind, failure.field, failure.detail)
    }

    /// What `user show --json` prints for a person, with what is kept
    /// apart from the account and its keys cut short.
    const SHOWN: &str = r#"{
        "format": 1,
        "account": {"name": "ann", "uid": 1000, "full_name": "Ann Lee", "home": "/users/ann",
                    "shell": "/system/commands/nukesh", "admin": false, "console": false,
                    "groups": ["ann"], "disabled": null, "password": null,
                    "since_ms": 1760900000000, "password_changed_ms": null},
        "keys": [{"type": "ssh-ed25519", "comment": "laptop", "fingerprint": "SHA256:x",
                  "line": 1, "options": false}],
        "steps": {"app": false, "passkeys": [], "recovery_left": null, "trusted_browsers": 0},
        "tokens": [],
        "sessions": [{"id": "41-1763110000", "kind": "ssh", "terminal": "pts/3",
                      "from": "10.8.0.2", "since_ms": 1763110000000, "processes": 5}]
    }"#;

    #[test]
    fn every_action_runs_as_the_person_and_names_no_account() {
        let ids = [
            "account.show",
            "account.name",
            "account.password",
            "account.key-add",
            "account.key-remove",
        ];
        for id in ids {
            let action = action(id);
            assert_eq!(action.runs(), Runs::AsThePerson, "{id}");
            assert_eq!(action.area(), "account", "{id}");
            assert!(!action.long() && !action.secret_answer(), "{id}");
            assert!(!action.path().contains('{'), "{id}: it has no account in its address");
            assert!(
                action.fields().iter().all(|field| field.name != "name"),
                "{id}: it takes no account's name"
            );
            let prepared = action.prepare(action.example_values().0.remove(0)).unwrap();
            assert!(!prepared.ansible.become_root, "{id}");
        }
        let show = action("account.show");
        assert_eq!(
            (show.changes(), show.path(), show.posts()),
            (false, "/api/1/account/show", false)
        );
        assert_eq!(show.prepare(json!({})).unwrap().argv, ["user", "show", "--json"]);
        assert_eq!(show.prepare(json!({ "all": true })).unwrap_err().kind, Kind::BadRequest);
        assert!(show.errors().is_empty() && !show.compares());
    }

    #[test]
    fn the_full_name_is_changed_with_no_name_for_the_command_to_take() {
        let name = action("account.name");
        assert_eq!(
            (name.changes(), name.path(), name.posts(), name.compares()),
            (true, "/api/1/account/name", true, true)
        );
        let prepared = name.prepare(json!({ "full_name": "Ann Lee" })).unwrap();
        assert_eq!(prepared.argv, ["user", "set", "--full-name", "Ann Lee"]);
        assert_eq!(prepared.files, ["/system/settings/passwd"]);
        assert_eq!(
            prepared.probe.as_deref(),
            Some(&["user", "show", "--json"].map(String::from)[..])
        );
        // An empty name clears it.
        assert_eq!(
            name.prepare(json!({ "full_name": "" })).unwrap().argv,
            ["user", "set", "--full-name", ""]
        );
        let words = "A full name has at most 256 bytes, and no colon or line break. Nothing was \
                     done.";
        for full_name in ["a:b", "a\nb", "a\rb", &"\u{e9}".repeat(129), &"a".repeat(257)] {
            let (kind, field, why) = refused("account.name", json!({ "full_name": full_name }));
            assert_eq!((kind, field, why.as_str()), (Kind::BadValue, Some("full_name"), words));
        }
        assert_eq!(refused("account.name", json!({})).0, Kind::BadRequest);
        // No account's name is taken: that would be another account's.
        assert_eq!(
            refused("account.name", json!({ "full_name": "x", "name": "bob" })).0,
            Kind::BadRequest
        );
    }

    #[test]
    fn the_password_is_changed_from_two_lines_the_current_one_first() {
        let password = action("account.password");
        assert_eq!(
            (password.changes(), password.path(), password.posts(), password.compares()),
            (true, "/api/1/account/password", true, false)
        );
        let prepared = password.prepare(json!({ "current": "old one", "new": "new one" })).unwrap();
        assert_eq!(prepared.argv, ["passwd"]);
        assert_eq!(prepared.input.as_ref().map(Secret::bytes), Some(&b"old one\nnew one\n"[..]));
        // authd's file of passwords is not in admind's view, and nothing
        // reads it to compare: a change of the password always changes it.
        assert!(prepared.files.is_empty() && prepared.probe.is_none());
        // The playbook reads both from its variables, in the same order.
        assert_eq!(prepared.ansible.params["stdin"], "{{ current }}\n{{ new }}");
        assert!(prepared.ansible.no_log);
        assert_eq!(password.example_values().2, None);
        for args in [
            json!({ "current": "x" }),
            json!({ "new": "x" }),
            json!({ "current": "x", "new": "y", "name": "bob" }),
        ] {
            assert_eq!(refused("account.password", args).0, Kind::BadRequest);
        }
    }

    #[test]
    fn two_passwords_fit_the_pipe_and_each_is_at_most_1024_bytes() {
        let with = |current: &str, new: &str| json!({ "current": current, "new": new });
        // 511 characters each: two lines of four bytes a character are
        // within the 4096 bytes that a pipe holds.
        let widest = "\u{10ffff}".repeat(TWO_PASSWORDS_CHARS);
        let words = "A password is at most 1024 bytes. Nothing was done.";
        let (kind, field, why) = refused("account.password", with(&widest, "x"));
        assert_eq!((kind, field, why.as_str()), (Kind::BadValue, Some("current"), words));
        let (kind, field, why) = refused("account.password", with("x", &widest));
        assert_eq!((kind, field, why.as_str()), (Kind::BadValue, Some("new"), words));
        let fits = "\u{e9}".repeat(TWO_PASSWORDS_CHARS);
        assert!(action("account.password").prepare(with(&fits, &fits)).is_ok());
        let (_, _, why) =
            (Kind::BadValue, 0, refused("account.password", with(&"a".repeat(512), "x")).2);
        assert_eq!(
            why,
            "It is one line of 1 to 511 characters, with no line break. Nothing was done."
        );
        for bad in ["", "two\nlines", "a\0b"] {
            assert_eq!(refused("account.password", with(bad, "x")).1, Some("current"), "{bad:?}");
            assert_eq!(refused("account.password", with("x", bad)).1, Some("new"), "{bad:?}");
        }
    }

    #[test]
    fn a_key_is_added_and_removed_from_the_account_of_whoever_asks() {
        let add = action("account.key-add");
        assert_eq!(
            (add.changes(), add.path(), add.errors(), add.compares()),
            (true, "/api/1/account/keys/add", ["ACC-103"].as_slice(), true)
        );
        let prepared = add.prepare(json!({ "line": EXAMPLE_KEY })).unwrap();
        // One operand is the line: the command takes it as the caller's own.
        assert_eq!(prepared.argv, ["user", "key", "add", EXAMPLE_KEY]);
        // The file's folder is named by the account, which the action does
        // not know: the keys `user show` lists are what is compared.
        assert!(prepared.files.is_empty());
        assert_eq!(
            prepared.probe.as_deref(),
            Some(&["user", "show", "--json"].map(String::from)[..])
        );
        let remove = action("account.key-remove");
        assert_eq!(
            (remove.changes(), remove.path(), remove.errors(), remove.compares()),
            (true, "/api/1/account/keys/remove", ["ACC-104"].as_slice(), true)
        );
        let prepared = remove.prepare(json!({ "fingerprint": EXAMPLE_FINGERPRINT })).unwrap();
        assert_eq!(prepared.argv, ["user", "key", "remove", EXAMPLE_FINGERPRINT]);
        assert_eq!(
            add.example_values().2,
            Some(Undo {
                action: "account.key-remove",
                args: json!({ "fingerprint": EXAMPLE_FINGERPRINT })
            })
        );
        assert_eq!(remove.example_values().2, None);
        // The rules are the Accounts page's.
        let words = "Dukovo takes Ed25519 and ECDSA (P-256 and P-384) keys, as one line the way \
                     ssh-keygen writes it, with no options before the key. Nothing was added.";
        for line in
            ["", "not a key", "ssh-dss AAAAB3NzaC1kc3M=", &format!("from=\"x\" {EXAMPLE_KEY}")]
        {
            let (kind, field, why) = refused("account.key-add", json!({ "line": line }));
            assert_eq!((kind, field), (Kind::BadValue, Some("line")), "{line:?}");
            assert!(why.ends_with(words), "{line:?}: {why}");
        }
        for fingerprint in ["", "SHA256:short", "MD5:abc"] {
            let (kind, field, _) =
                refused("account.key-remove", json!({ "fingerprint": fingerprint }));
            assert_eq!((kind, field), (Kind::BadValue, Some("fingerprint")), "{fingerprint:?}");
        }
    }

    #[test]
    fn the_probe_picks_the_account_for_a_name_and_the_keys_for_a_key() {
        let name = action("account.name");
        let account = name.probed(&json!({ "full_name": "x" }), SHOWN).unwrap();
        let account: Value = serde_json::from_str(&account).unwrap();
        assert_eq!(account["full_name"], "Ann Lee");
        assert!(account.get("keys").is_none() && account.get("sessions").is_none());
        // What moves from one moment to the next is not the account's.
        let later = SHOWN.replace("1763110000000", "1763110005000");
        assert_eq!(
            name.probed(&json!({ "full_name": "x" }), &later),
            name.probed(&json!({ "full_name": "x" }), SHOWN)
        );
        let add = action("account.key-add");
        let keys = add.probed(&json!({ "line": EXAMPLE_KEY }), SHOWN).unwrap();
        assert!(keys.contains("\"laptop\"") && !keys.contains("Ann Lee"), "{keys}");
        let none = SHOWN.replace("\"laptop\"", "\"phone\"");
        assert_ne!(add.probed(&json!({ "line": EXAMPLE_KEY }), &none), Some(keys.clone()));
        assert_eq!(
            action("account.key-remove")
                .probed(&json!({ "fingerprint": EXAMPLE_FINGERPRINT }), SHOWN),
            Some(keys)
        );
        for output in ["", "not json", "[]", "{}", r#"{"format": 1}"#] {
            assert_eq!(name.probed(&json!({ "full_name": "x" }), output), None, "{output:?}");
        }
    }

    #[test]
    fn the_routes_are_found_by_their_methods() {
        for (post, path, id) in [
            (false, "/api/1/account/show", "account.show"),
            (true, "/api/1/account/name", "account.name"),
            (true, "/api/1/account/password", "account.password"),
            (true, "/api/1/account/keys/add", "account.key-add"),
            (true, "/api/1/account/keys/remove", "account.key-remove"),
        ] {
            let found = find(REGISTRY, post, path);
            assert!(
                matches!(&found, Found::Action(action, _) if action.id() == id),
                "{path}: {found:?}"
            );
        }
        assert!(matches!(find(REGISTRY, true, "/api/1/account/show"), Found::WrongMethod));
        assert!(matches!(find(REGISTRY, false, "/api/1/account/name"), Found::WrongMethod));
        // The accounts' addresses are the other area's.
        assert!(
            matches!(find(REGISTRY, false, "/api/1/accounts/show"), Found::Action(a, _) if a.id() == "accounts.show")
        );
    }

    #[test]
    fn a_token_needs_the_area_to_change_and_its_read_scope_to_read() {
        let call = |list: &str, id: &str| {
            let scopes = Scopes::parse(list).unwrap();
            authorize(&Caller::Token { name: "ci", scopes: &scopes }, action(id))
        };
        assert!(call("account:read", "account.show").is_ok());
        assert!(call("account", "account.show").is_ok());
        for id in ["account.name", "account.password", "account.key-add", "account.key-remove"] {
            assert_eq!(call("account:read", id).unwrap_err().kind, Kind::OutOfScope, "{id}");
            // The accounts' scope is another area's.
            assert_eq!(call("accounts", id).unwrap_err().kind, Kind::OutOfScope, "{id}");
            assert!(call("account", id).is_ok(), "{id}");
            assert!(call("*", id).is_ok(), "{id}");
        }
        assert_eq!(call("account", "accounts.list").unwrap_err().kind, Kind::OutOfScope);
    }

    #[test]
    fn any_session_runs_them_as_itself() {
        for (admin, unlocked) in [(false, false), (false, true), (true, false), (true, true)] {
            for id in [
                "account.show",
                "account.name",
                "account.password",
                "account.key-add",
                "account.key-remove",
            ] {
                let caller = Caller::Session { name: "ann", admin, unlocked };
                assert!(authorize(&caller, action(id)).is_ok(), "{id}: {admin} {unlocked}");
            }
        }
    }

    #[test]
    fn the_examples_are_what_the_boot_and_lab_tests_run() {
        let (good, bad, undo) = action("account.name").example_values();
        assert_eq!(good, [json!({ "full_name": "Lab reader" })]);
        assert_eq!(undo, Some(Undo { action: "account.name", args: json!({ "full_name": "" }) }));
        assert_eq!(bad.len(), 3);
        let (good, bad, _) = action("account.password").example_values();
        assert_eq!(good, [json!({ "current": "(secret)", "new": "(secret)" })]);
        assert!(bad.contains(&json!({ "current": "lab-pass-1" })));
        for id in [
            "account.name",
            "account.password",
            "account.key-add",
            "account.key-remove",
            "account.show",
        ] {
            for args in action(id).example_values().1 {
                // The key kinds the schema cannot refuse are the code's.
                assert!(action(id).prepare(args.clone()).is_err(), "{id}: {args}");
            }
        }
    }
}
