// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The certificates' actions (ADR 0514): the list, one certificate, and
//! renewing one. Each runs `cert`, cert(8), which reads the store certd
//! keeps; a renewal runs as an administrator, as `elevate cert renew NAME`
//! does at a terminal.
//!
//! A certificate's name is checked by the rule certd and `cert` check it
//! by, which is also the rule a service file's `certificates` follows.

use serde::{Deserialize, Serialize};
use serde_json::json;

use super::{Action, Args, Bad, Examples, Nothing, Runs, Tests};
use crate::cert::machine;
use manifest::service::{CERTIFICATE_NAME_MAX, check_certificate};

/// The area of these actions: the scope a token needs, `certificates`.
const AREA: &str = "certificates";

/// The tests of these actions, which `lab/actions.toml` names.
const TESTS: Tests = Tests {
    checks: "admind::actions::certificate::tests",
    route: "admin-actions",
    same_as_command: "admin-actions",
    browser: "web-console/actions",
    ansible: "web-console/ansible",
};

/// A certificate, by its name.
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Named {
    /// The certificate's name.
    pub name: String,
}

impl Args for Named {
    fn check(&self) -> Result<(), Bad> {
        if check_certificate(&self.name).is_ok() {
            return Ok(());
        }
        Err(Bad {
            field: "name",
            why: format!(
                "A certificate's name has 1 to {CERTIFICATE_NAME_MAX} lowercase letters, digits, \
                 hyphens and dots, starts with a letter or a digit, and has no two dots in a row. \
                 Nothing was done."
            ),
        })
    }
}

/// The machine's own certificate, which every image has: the examples use
/// it.
fn example() -> Named {
    Named { name: machine::NAME.into() }
}

/// Arguments no certificate action that takes a name takes: an empty name,
/// capitals, a path, and a field it has none of.
fn bad_names() -> Vec<serde_json::Value> {
    vec![
        json!({ "name": "" }),
        json!({ "name": "This-Machine" }),
        json!({ "name": "../etc/passwd" }),
        json!({ "name": machine::NAME, "force": true }),
    ]
}

/// `cert list --json`: every certificate.
pub struct List;

impl Action for List {
    const ID: &'static str = "certificate.list";
    const AREA: &'static str = AREA;
    const CHANGES: bool = false;
    const PATH: &'static str = "/api/1/certificates";
    const RUNS: Runs = Runs::AsThePerson;
    const TESTS: Tests = TESTS;
    const TITLE: &'static str = "List the certificates";
    type Args = Nothing;

    fn argv(_: &Nothing) -> Vec<String> {
        ["cert", "list", "--json"].map(String::from).to_vec()
    }

    fn examples() -> Examples<Nothing> {
        Examples {
            good: vec![Nothing {}],
            bad: vec![json!({ "name": machine::NAME }), json!({ "expired": true })],
            undo: None,
        }
    }
}

/// `cert show NAME --json`: one certificate, what it holds and whether it
/// can be trusted.
pub struct Show;

impl Action for Show {
    const ID: &'static str = "certificate.show";
    const AREA: &'static str = AREA;
    const CHANGES: bool = false;
    const PATH: &'static str = "/api/1/certificates/{name}";
    const RUNS: Runs = Runs::AsThePerson;
    const TESTS: Tests = TESTS;
    const TITLE: &'static str = "Show a certificate";
    type Args = Named;

    fn argv(args: &Named) -> Vec<String> {
        ["cert", "show", args.name.as_str(), "--json"].map(String::from).to_vec()
    }

    fn subject(args: &Named) -> Option<String> {
        Some(args.name.clone())
    }

    fn examples() -> Examples<Named> {
        Examples { good: vec![example()], bad: bad_names(), undo: None }
    }
}

/// `cert renew NAME`: makes the machine's own certificate anew, or asks
/// the authority for a certificate that came from one again. Nothing puts
/// back what it did.
pub struct Renew;

impl Action for Renew {
    const ID: &'static str = "certificate.renew";
    const AREA: &'static str = AREA;
    const CHANGES: bool = true;
    const PATH: &'static str = "/api/1/certificates/{name}/renew";
    const RUNS: Runs = Runs::AsAdministrator;
    const TESTS: Tests = TESTS;
    const TITLE: &'static str = "Renew a certificate";
    type Args = Named;

    fn argv(args: &Named) -> Vec<String> {
        ["cert", "renew", args.name.as_str()].map(String::from).to_vec()
    }

    fn subject(args: &Named) -> Option<String> {
        Some(args.name.clone())
    }

    fn examples() -> Examples<Named> {
        Examples { good: vec![example()], bad: bad_names(), undo: None }
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

    /// The command `id` makes for `args`.
    fn argv(id: &str, args: serde_json::Value) -> Vec<String> {
        registered(id).prepare(args).unwrap().argv
    }

    #[test]
    fn the_reads_run_cert_with_json() {
        assert_eq!(argv("certificate.list", json!({})), ["cert", "list", "--json"]);
        assert_eq!(
            argv("certificate.show", json!({ "name": "this-machine" })),
            ["cert", "show", "this-machine", "--json"]
        );
    }

    #[test]
    fn a_renewal_runs_as_an_administrator_with_the_name_after_the_verb() {
        assert_eq!(
            argv("certificate.renew", json!({ "name": "example.org" })),
            ["cert", "renew", "example.org"]
        );
        let renew = registered("certificate.renew");
        assert_eq!(
            (renew.changes(), renew.runs(), renew.area()),
            (true, Runs::AsAdministrator, "certificates")
        );
        assert_eq!(renew.path(), "/api/1/certificates/{name}/renew");
        for id in ["certificate.list", "certificate.show"] {
            let read = registered(id);
            assert_eq!(
                (read.changes(), read.runs(), read.area()),
                (false, Runs::AsThePerson, "certificates")
            );
        }
        assert_eq!(registered("certificate.list").path(), "/api/1/certificates");
        assert_eq!(registered("certificate.show").path(), "/api/1/certificates/{name}");
    }

    #[test]
    fn a_name_is_one_certd_and_cert_keep() {
        for name in ["this-machine", "example.org", "a", "0", "a-b.c-d", &"a".repeat(64)] {
            assert_eq!(argv("certificate.show", json!({ "name": name }))[2], name);
        }
        let words = "A certificate's name has 1 to 64 lowercase letters, digits, hyphens and dots, \
                     starts with a letter or a digit, and has no two dots in a row. Nothing was \
                     done.";
        for name in [
            "",
            "This-Machine",
            "-x",
            ".x",
            "a..b",
            "a_b",
            "a b",
            "a/b",
            "../x",
            "caf\u{e9}",
            &"a".repeat(65),
        ] {
            for id in ["certificate.show", "certificate.renew"] {
                let failure = registered(id).prepare(json!({ "name": name })).unwrap_err();
                assert_eq!(
                    (failure.kind, failure.field),
                    (Kind::BadValue, Some("name")),
                    "{id} {name:?}"
                );
                assert_eq!(failure.detail, words, "{id} {name:?}");
            }
        }
    }

    #[test]
    fn arguments_the_action_does_not_take_do_not_read() {
        let wrong = json!({ "name": "this-machine", "force": true });
        for id in ["certificate.show", "certificate.renew"] {
            assert_eq!(
                registered(id).prepare(wrong.clone()).unwrap_err().kind,
                Kind::BadRequest,
                "{id}"
            );
            assert_eq!(
                registered(id).prepare(json!({})).unwrap_err().kind,
                Kind::BadRequest,
                "{id}"
            );
        }
        let list = registered("certificate.list").prepare(json!({ "name": "this-machine" }));
        assert_eq!(list.unwrap_err().kind, Kind::BadRequest);
    }

    #[test]
    fn the_ansible_task_is_named_for_the_action_and_the_certificate() {
        let task = registered("certificate.renew")
            .prepare(json!({ "name": "this-machine" }))
            .unwrap()
            .ansible;
        assert_eq!(task.name, "Renew a certificate: this-machine");
        assert!(task.become_root);
        assert_eq!(task.changed_when, None);
        let task = registered("certificate.list").prepare(json!({})).unwrap().ansible;
        assert_eq!(task.name, "List the certificates");
        assert_eq!(task.changed_when.as_deref(), Some("false"));
    }

    #[test]
    fn a_renewal_names_no_file() {
        let prepared =
            registered("certificate.renew").prepare(json!({ "name": "this-machine" })).unwrap();
        assert!(prepared.files.is_empty());
    }

    #[test]
    fn the_examples_use_the_machine_s_own_certificate_and_the_bad_ones_do_not_work() {
        for action in REGISTRY.iter().filter(|action| action.id().starts_with("certificate.")) {
            let id = action.id();
            let (good, bad, undo) = action.example_values();
            assert!(
                undo.is_none(),
                "{id}: nothing puts back a renewal, and a read changes nothing"
            );
            assert!(!bad.is_empty(), "{id}");
            for args in &good {
                assert!(action.prepare(args.clone()).is_ok(), "{id}");
            }
            for args in bad {
                assert!(action.prepare(args.clone()).is_err(), "{id} {args}");
            }
        }
        let (good, _, _) = registered("certificate.renew").example_values();
        assert_eq!(good, [json!({ "name": "this-machine" })]);
    }
}
