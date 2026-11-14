// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The console's own actions (ADR 0514, ADR 0516): the settings of admind
//! itself, which live in `/system/settings/admin.toml` (admin.toml(5)).
//! The first is `console.reach`, whom the console answers. It runs `admin
//! reach`, admin(8), as an administrator, as `elevate admin reach any`
//! does at a terminal: the command changes the file, keeping every comment
//! in it, and has Spark reload admind.
//!
//! The area is `console`, the scope a token needs. It is the thing the
//! actions are done to, as `services` and `certificates` are, and it is not
//! a wider name such as `settings`, which a later action of another thing's
//! settings would share: a token that may change the reach may open the
//! console to every address, which no token for something else should
//! carry with it.
//!
//! The reach says whether it changed anything by reading `admin reach`
//! before and after (ADR 0515).

use serde::{Deserialize, Serialize};
use serde_json::json;

use super::{Action, Args, Bad, Examples, Field, Runs, Tests, Undo};
use crate::admin_settings::{self, Reach};

/// The area of these actions: the scope a token needs, `console`.
const AREA: &str = "console";

/// The tests of these actions, which `lab/actions.toml` names.
const TESTS: Tests = Tests {
    checks: "admind::actions::console::tests",
    route: "admin-actions",
    same_as_command: "admin-actions",
    browser: "web-console/actions",
    ansible: "web-console/ansible",
};

/// Whom the console is to answer.
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Reaching {
    /// `private` or `any`, as admin.toml writes it.
    pub reach: String,
}

impl Args for Reaching {
    const FIELDS: &'static [Field] = &[Field::one_of(
        "reach",
        "Whom the console answers: private for machines on private networks, any for every machine.",
        &["private", "any"],
    )];

    fn check(&self) -> Result<(), Bad> {
        if Reach::parse(&self.reach).is_some() {
            return Ok(());
        }
        Err(Bad {
            field: "reach",
            why: "The console's reach is private or any. Nothing was done.".into(),
        })
    }
}

/// The reach the examples use, which is the one admin.toml has when it says
/// nothing. The boot and lab tests run the examples on machines they reach
/// from private addresses, QEMU's user network and the lab PC's, so that
/// the example leaves the console answering them, and a machine that has
/// not been changed is as the example leaves it.
const EXAMPLE: Reach = Reach::Private;

/// `admin reach WORD`: makes admind answer the machines on private
/// networks only, or any machine, and has it read the setting at once.
/// What puts it back is itself, with the reach the console has when
/// admin.toml says nothing.
pub struct SetReach;

impl Action for SetReach {
    const ID: &'static str = "console.reach";
    const AREA: &'static str = AREA;
    const CHANGES: bool = true;
    const PATH: &'static str = "/api/1/console/reach";
    const RUNS: Runs = Runs::AsAdministrator;
    const TESTS: Tests = TESTS;
    const TITLE: &'static str = "Set whom the console answers";
    type Args = Reaching;

    fn argv(args: &Reaching) -> Vec<String> {
        ["admin", "reach", args.reach.as_str()].map(String::from).to_vec()
    }

    fn subject(args: &Reaching) -> Option<String> {
        Some(args.reach.clone())
    }

    fn files(_: &Reaching) -> Vec<String> {
        vec![admin_settings::PATH.into()]
    }

    /// `admin reach` with no word, which prints the reach the console has.
    fn probe(_: &Reaching) -> Option<Vec<String>> {
        Some(["admin", "reach"].map(String::from).to_vec())
    }

    /// The reach it printed, if it is one.
    fn probed(_: &Reaching, output: &str) -> Option<String> {
        Reach::parse(output.trim()).map(|reach| reach.word().to_string())
    }

    fn examples() -> Examples<Reaching> {
        let word = EXAMPLE.word();
        Examples {
            good: vec![Reaching { reach: word.into() }],
            bad: vec![
                json!({ "reach": "everyone" }),
                json!({ "reach": "" }),
                json!({ "reach": "Any" }),
                json!({ "reach": word, "port": 443 }),
                json!({}),
            ],
            undo: Some(Undo { action: Self::ID, args: json!({ "reach": word }) }),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::net::IpAddr;

    use super::super::{REGISTRY, Registered};
    use super::*;
    use crate::admind::errors::Kind;
    use crate::admind::reach;
    use crate::admind::scopes::Scopes;

    /// The action, which is in the registry.
    fn registered() -> &'static dyn Registered {
        *REGISTRY.iter().find(|action| action.id() == "console.reach").unwrap()
    }

    #[test]
    fn the_command_is_admin_reach_with_the_word_after_it() {
        for word in ["private", "any"] {
            let prepared = registered().prepare(json!({ "reach": word })).unwrap();
            assert_eq!(prepared.argv, ["admin", "reach", word]);
            assert_eq!(prepared.args, json!({ "reach": word }));
        }
    }

    #[test]
    fn a_reach_is_private_or_any_and_the_refusal_names_the_field() {
        let words = "The console's reach is private or any. Nothing was done.";
        for reach in ["everyone", "", "Any", "PRIVATE", " any", "any ", "private\n", "0", "*"] {
            let failure = registered().prepare(json!({ "reach": reach })).unwrap_err();
            assert_eq!((failure.kind, failure.field), (Kind::BadValue, Some("reach")), "{reach:?}");
            assert_eq!(failure.detail, words, "{reach:?}");
        }
    }

    #[test]
    fn the_probe_reads_the_reach_the_console_has_and_compares_the_word() {
        let action = registered();
        assert!(action.compares());
        for word in ["private", "any"] {
            let prepared = action.prepare(json!({ "reach": word })).unwrap();
            assert_eq!(prepared.probe.as_deref(), Some(&["admin".into(), "reach".into()][..]));
            // `admin reach` prints the word and a line end.
            let probed = |output: &str| action.probed(&prepared.args, output);
            assert_eq!(probed("private\n").as_deref(), Some("private"));
            assert_eq!(probed("any\n").as_deref(), Some("any"));
            assert_eq!(probed(" any \n\n").as_deref(), Some("any"));
        }
        // Anything else is not a reach, and the change then says it changed.
        let args = json!({ "reach": "any" });
        for output in ["", "\n", "Any\n", "everyone", "any private", "The reach is any.\n"] {
            assert_eq!(action.probed(&args, output), None, "{output:?}");
        }
        // The reach asked for has no part in what the probe picks.
        assert_eq!(action.probed(&json!({ "reach": "private" }), "any\n").as_deref(), Some("any"));
    }

    #[test]
    fn arguments_the_action_does_not_take_do_not_read() {
        for args in [
            json!({}),
            json!({ "reach": "any", "port": 443 }),
            json!({ "reach": true }),
            json!({ "reach": ["any"] }),
            json!({ "reach": null }),
        ] {
            let failure = registered().prepare(args.clone()).unwrap_err();
            assert_eq!(failure.kind, Kind::BadRequest, "{args}");
        }
    }

    #[test]
    fn it_changes_the_settings_file_as_an_administrator_and_is_a_task_for_nobody() {
        let action = registered();
        assert_eq!(
            (action.changes(), action.runs(), action.area(), action.long()),
            (true, Runs::AsAdministrator, "console", false)
        );
        assert_eq!(action.path(), "/api/1/console/reach");
        assert!(action.errors().is_empty());
        let prepared = action.prepare(json!({ "reach": "any" })).unwrap();
        assert_eq!(prepared.files, ["/system/settings/admin.toml"]);
        assert_eq!(prepared.ansible.name, "Set whom the console answers: any");
        assert_eq!(prepared.ansible.module, "ansible.builtin.command");
        assert_eq!(prepared.ansible.params["argv"], json!(["admin", "reach", "any"]));
        assert!(prepared.ansible.become_root);
        assert_eq!(prepared.ansible.changed_when, None);
    }

    #[test]
    fn a_token_needs_the_console_scope_to_change_it_and_may_read_nothing_of_it() {
        let call = |list: &str| {
            let scopes = Scopes::parse(list).unwrap();
            let caller = super::super::Caller::Token { name: "ci", scopes: &scopes };
            super::super::authorize(&caller, registered())
        };
        assert!(call("console").is_ok());
        assert!(call("*").is_ok());
        for list in ["console:read", "services", "certificates,services:read"] {
            let refused = call(list).unwrap_err();
            assert_eq!(refused.kind, Kind::OutOfScope, "{list}");
            assert_eq!(
                refused.detail,
                "the API token \"ci\" may not do this: it needs the scope console or *"
            );
        }
    }

    #[test]
    fn the_example_is_the_default_and_leaves_the_machines_the_tests_run_from_answered() {
        let (good, bad, undo) = registered().example_values();
        assert_eq!(good, [json!({ "reach": "private" })]);
        assert_eq!(Reach::parse("private"), Some(Reach::default()));
        // The boot test's guest is reached from QEMU's user network, and
        // the lab's machine from the lab PC, which is on a private
        // network: neither is an address that the example shuts out.
        let from = ["10.0.2.2", "192.168.1.20", "172.16.0.9", "127.0.0.1"];
        for address in from {
            let ip: IpAddr = address.parse().unwrap();
            assert!(reach::answers(EXAMPLE, ip, &[]), "{address}");
        }
        assert!(!reach::answers(EXAMPLE, "203.0.113.9".parse().unwrap(), &[]));
        // Nothing but itself puts it back: it sets the reach to a word, and
        // the word of the example is the one the machine starts with.
        assert_eq!(
            undo,
            Some(Undo { action: "console.reach", args: json!({ "reach": "private" }) })
        );
        for args in bad {
            assert!(registered().prepare(args.clone()).is_err(), "{args}");
        }
        assert_eq!(
            registered().prepare(good[0].clone()).unwrap().argv,
            ["admin", "reach", "private"]
        );
    }
}
