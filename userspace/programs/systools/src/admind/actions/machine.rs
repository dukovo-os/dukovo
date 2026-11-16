// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The machine's name (hostname(5)): what it is called, and calling it
//! something else. Both run `hostnamectl`, hostnamectl(1): the name is
//! `/system/settings/hostname`, which admind sees, so a rename compares
//! that file and what `hostnamectl status` says of the name.
//!
//! A rename puts the new name to use at once (`--apply`): discoverd answers
//! for it on the local network, certd makes the machine's certificate for
//! it, and admind reads the names it answers to again. A trial of settings
//! is not begun, since the old name keeps working by the machine's
//! addresses.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::trial::refusal;
use super::{Action, Args, Bad, Examples, Field, Nothing, Runs, Tests, Undo};
use crate::hostnamectl::{NAME_MAX, check_name};

/// The area of these actions: the scope a token needs, `machine`.
const AREA: &str = "machine";

/// The tests of these actions, which `lab/actions.toml` names.
const TESTS: Tests = Tests {
    checks: "admind::actions::machine::tests",
    route: "admin-certificates",
    same_as_command: "admin-certificates",
    browser: "web-console/certificates",
    ansible: "web-console/ansible",
};

/// What a host name matches, as `check_name` checks it: words of letters,
/// digits and `-`, none starting or ending with `-`, 63 characters at most
/// each, with a dot between them. Its length in all is the field's.
const NAME_PATTERN: &str = "^[A-Za-z0-9]([A-Za-z0-9-]{0,61}[A-Za-z0-9])?\
                            (\\.[A-Za-z0-9]([A-Za-z0-9-]{0,61}[A-Za-z0-9])?)*$";

/// The file that holds the name.
const HOSTNAME_FILE: &str = "/system/settings/hostname";

/// `hostnamectl status --json`: the machine's name, its first word and its
/// domain, and what it is called on the local network.
pub struct Name;

impl Action for Name {
    const ID: &'static str = "machine.name";
    const AREA: &'static str = AREA;
    const CHANGES: bool = false;
    const PATH: &'static str = "/api/1/machine/name";
    const RUNS: Runs = Runs::AsThePerson;
    const TESTS: Tests = TESTS;
    const TITLE: &'static str = "Show the machine's name";
    type Args = Nothing;

    fn argv(_: &Nothing) -> Vec<String> {
        ["hostnamectl", "status", "--json"].map(String::from).to_vec()
    }

    fn examples() -> Examples<Nothing> {
        Examples {
            good: vec![Nothing {}],
            bad: vec![json!({ "name": "lab" })],
            undo: None,
        }
    }
}

/// What the machine is to be called.
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Renaming {
    /// The full name, with its domain if it has one.
    pub name: String,
}

impl Args for Renaming {
    const FIELDS: &'static [Field] = &[Field::text(
        "name",
        "The machine's full name: words of letters, digits and -, with a dot between them, 64 characters at most.",
        1,
        NAME_MAX,
        Some(NAME_PATTERN),
    )];

    fn check(&self) -> Result<(), Bad> {
        check_name(&self.name).map_err(|why| Bad { field: "name", why: refusal(&why) })
    }
}

/// `hostnamectl set-hostname FULL --apply`: calls the machine something
/// else and has the services that answer to the name read it at once. What
/// puts it back is itself, with the name it had.
pub struct Rename;

impl Action for Rename {
    const ID: &'static str = "machine.rename";
    const AREA: &'static str = AREA;
    const CHANGES: bool = true;
    const PATH: &'static str = "/api/1/machine/name";
    const RUNS: Runs = Runs::AsAdministrator;
    const TESTS: Tests = TESTS;
    const TITLE: &'static str = "Rename the machine";
    type Args = Renaming;

    fn argv(args: &Renaming) -> Vec<String> {
        ["hostnamectl", "set-hostname", args.name.as_str(), "--apply"].map(String::from).to_vec()
    }

    fn subject(args: &Renaming) -> Option<String> {
        Some(args.name.clone())
    }

    fn files(_: &Renaming) -> Vec<String> {
        vec![HOSTNAME_FILE.into()]
    }

    /// `hostnamectl status --json`, which everyone may run.
    fn probe(_: &Renaming) -> Option<Vec<String>> {
        Some(["hostnamectl", "status", "--json"].map(String::from).to_vec())
    }

    /// The machine's name, which is what a rename moves.
    fn probed(_: &Renaming, output: &str) -> Option<String> {
        let status: Value = serde_json::from_str(output).ok()?;
        status.get("name").and_then(Value::as_str).map(String::from)
    }

    /// The machine already has the name.
    fn settled(args: &Renaming, now: &str) -> Option<bool> {
        Some(now == args.name)
    }

    /// The command writes the name on a line of its own, and nothing when
    /// the name is already the one asked for.
    fn keeps_file(args: &Renaming, _: &str, text: Option<&str>) -> Option<bool> {
        text.map(|text| text.trim() == args.name)
    }

    fn examples() -> Examples<Renaming> {
        Examples {
            good: vec![Renaming { name: "lab-renamed.example.org".into() }],
            bad: vec![
                json!({ "name": "-x" }),
                json!({ "name": "a b" }),
                json!({ "name": "" }),
                json!({ "name": "a..b" }),
                json!({ "name": "x-" }),
                json!({ "name": "a".repeat(NAME_MAX + 1) }),
                json!({ "name": "lab", "domain": "example.org" }),
                json!({}),
            ],
            // The name the tests' machine starts with, which every image
            // has until it is renamed.
            undo: Some(Undo { action: Self::ID, args: json!({ "name": "dukovo" }) }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::{REGISTRY, Registered};
    use super::*;
    use crate::admind::errors::Kind;

    /// The registered action `id`.
    fn registered(id: &str) -> &'static dyn Registered {
        *REGISTRY.iter().find(|action| action.id() == id).unwrap()
    }

    #[test]
    fn the_name_is_read_by_anyone_and_a_rename_is_an_administrator_s() {
        let name = registered("machine.name");
        assert_eq!(
            (name.changes(), name.runs(), name.area(), name.path()),
            (false, Runs::AsThePerson, "machine", "/api/1/machine/name")
        );
        let prepared = name.prepare(json!({})).unwrap();
        assert_eq!(prepared.argv, ["hostnamectl", "status", "--json"]);
        assert!(!name.compares());
        assert_eq!(prepared.ansible.changed_when.as_deref(), Some("false"));
        let rename = registered("machine.rename");
        assert_eq!(
            (rename.changes(), rename.runs(), rename.area(), rename.path()),
            (true, Runs::AsAdministrator, "machine", "/api/1/machine/name")
        );
        assert!(!rename.long());
        assert_eq!(rename.answers(), [0]);
        for action in [name, rename] {
            let tests = action.tests();
            assert_eq!(tests.checks, "admind::actions::machine::tests");
            assert_eq!(
                (tests.route, tests.same_as_command, tests.browser, tests.ansible),
                (
                    "admin-certificates",
                    "admin-certificates",
                    "web-console/certificates",
                    "web-console/ansible"
                )
            );
        }
        assert!(name.prepare(json!({ "name": "x" })).is_err());
    }

    #[test]
    fn a_rename_applies_the_name_at_once_and_names_the_file_the_name_is_in() {
        let rename = registered("machine.rename");
        let prepared = rename.prepare(json!({ "name": "lab-renamed.example.org" })).unwrap();
        assert_eq!(
            prepared.argv,
            ["hostnamectl", "set-hostname", "lab-renamed.example.org", "--apply"]
        );
        assert_eq!(prepared.files, ["/system/settings/hostname"]);
        assert_eq!(prepared.probe.as_deref(), Some(&["hostnamectl", "status", "--json"].map(String::from)[..]));
        assert!(rename.compares());
        assert_eq!(prepared.trial, None);
        assert!(prepared.ansible.become_root);
        assert_eq!(prepared.ansible.name, "Rename the machine: lab-renamed.example.org");
        assert!(prepared.input.is_none());
    }

    #[test]
    fn what_is_compared_is_the_name_the_command_prints_and_the_name_in_its_file() {
        let rename = registered("machine.rename");
        let args = json!({ "name": "lab-renamed.example.org" });
        let status = r#"{"format":1,"name":"dukovo","first":"dukovo","domain":null,"local":"dukovo.local"}"#;
        assert_eq!(rename.probed(&args, status).as_deref(), Some("dukovo"));
        assert_eq!(rename.probed(&args, "{}"), None);
        assert_eq!(rename.probed(&args, "dukovo"), None);
        assert_eq!(rename.settled(&args, "dukovo"), Some(false));
        assert_eq!(rename.settled(&args, "lab-renamed.example.org"), Some(true));
        let file = "/system/settings/hostname";
        assert_eq!(rename.keeps_file(&args, file, Some("lab-renamed.example.org\n")), Some(true));
        assert_eq!(rename.keeps_file(&args, file, Some("lab-renamed.example.org")), Some(true));
        assert_eq!(rename.keeps_file(&args, file, Some("dukovo\n")), Some(false));
        assert_eq!(rename.keeps_file(&args, file, None), None);
    }

    #[test]
    fn a_name_is_what_hostname_asks_for_and_what_is_not_is_refused_with_the_field() {
        let rename = registered("machine.rename");
        for fine in
            ["a", "lab", "lab-renamed", "lab-renamed.example.org", "A1.b-2.c", &"a".repeat(63), &format!("{}.{}", "a".repeat(30), "b".repeat(33))]
        {
            let argv = rename.prepare(json!({ "name": fine })).unwrap().argv;
            assert_eq!(argv[2], fine);
        }
        for wrong in [
            "",
            "-x",
            "x-",
            "a b",
            "a_b",
            "a..b",
            ".a",
            "a.",
            "a/b",
            "caf\u{e9}",
            "a\nb",
            &"a".repeat(64),
            &format!("{}.{}", "a".repeat(32), "b".repeat(32)),
        ] {
            let refused = rename.prepare(json!({ "name": wrong })).unwrap_err();
            assert_eq!(
                (refused.kind, refused.field),
                (Kind::BadValue, Some("name")),
                "{wrong:?}"
            );
            assert!(refused.detail.ends_with("Nothing was done."), "{}", refused.detail);
        }
        assert_eq!(rename.prepare(json!({})).unwrap_err().kind, Kind::BadRequest);
        assert_eq!(
            rename.prepare(json!({ "name": "lab", "domain": "x" })).unwrap_err().kind,
            Kind::BadRequest
        );
    }

    #[test]
    fn a_rename_is_put_back_by_one_with_the_name_the_machine_started_with() {
        let (good, bad, undo) = registered("machine.rename").example_values();
        assert_eq!(good, [json!({ "name": "lab-renamed.example.org" })]);
        assert!(bad.contains(&json!({ "name": "-x" })) && bad.contains(&json!({ "name": "a b" })));
        let undo = undo.unwrap();
        assert_eq!((undo.action, undo.args), ("machine.rename", json!({ "name": "dukovo" })));
    }
}
