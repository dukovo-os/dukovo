// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The health checks' action: `checks.last`, which runs `dukovo-check
//! --last --json` as the person and answers what the last run of the
//! checks kept, as the command printed it. The area is `security`, the
//! scope a token needs.
//!
//! The command is not in this tree yet: it comes with the checks
//! themselves. Until it does the worker finds no program of that name in
//! `/system/commands` or `/applications/commands`, and the answer is
//! ADM-129, "This server does not have that command yet" (501), which the
//! Overview shows as its own place for the checks, not as a failure. The
//! action's route, schema and Ansible task are all there already, and the
//! answer is what the command prints as soon as it is.

use serde_json::json;

use super::{Action, Examples, Nothing, Runs, Tests};

/// The tests of this action, which `lab/actions.toml` names.
const TESTS: Tests = Tests {
    checks: "admind::actions::checks::tests",
    route: "admin-actions",
    same_as_command: "admin-actions",
    browser: "web-console/actions",
    ansible: "web-console/ansible",
};

/// The program of the checks.
pub const PROGRAM: &str = "dukovo-check";

/// `dukovo-check --last --json`: the result the last run of the checks
/// kept.
pub struct Last;

impl Action for Last {
    const ID: &'static str = "checks.last";
    const AREA: &'static str = "security";
    const CHANGES: bool = false;
    const PATH: &'static str = "/api/1/checks/last";
    const RUNS: Runs = Runs::AsThePerson;
    const TESTS: Tests = TESTS;
    const TITLE: &'static str = "Show the last checks";
    type Args = Nothing;

    fn argv(_: &Nothing) -> Vec<String> {
        [PROGRAM, "--last", "--json"].map(String::from).to_vec()
    }

    fn examples() -> Examples<Nothing> {
        Examples {
            good: vec![Nothing {}],
            bad: vec![json!({ "last": true }), json!({ "all": true })],
            undo: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::super::{Registered, Runs, authorize};
    use super::*;
    use crate::admind::actions::{Caller, REGISTRY};
    use crate::admind::errors::Kind;
    use crate::admind::scopes::Scopes;

    fn action() -> &'static dyn Registered {
        *REGISTRY.iter().find(|action| action.id() == "checks.last").unwrap()
    }

    #[test]
    fn it_runs_the_checks_command_for_the_last_result_as_json() {
        let action = action();
        assert_eq!(
            (action.changes(), action.runs(), action.area(), action.long()),
            (false, Runs::AsThePerson, "security", false)
        );
        assert_eq!(action.path(), "/api/1/checks/last");
        assert_eq!(action.title(), "Show the last checks");
        let prepared = action.prepare(json!({})).unwrap();
        assert_eq!(prepared.argv, ["dukovo-check", "--last", "--json"]);
        assert_eq!(prepared.argv[0], PROGRAM);
        assert_eq!(prepared.probe, None);
        assert!(prepared.files.is_empty());
        assert_eq!(prepared.ansible.name, "Show the last checks");
        assert_eq!(prepared.ansible.params["argv"], json!(["dukovo-check", "--last", "--json"]));
        assert!(!prepared.ansible.become_root);
        assert_eq!(prepared.ansible.changed_when.as_deref(), Some("false"));
    }

    #[test]
    fn it_takes_nothing() {
        let (good, bad, undo) = action().example_values();
        assert_eq!((good, undo), (vec![json!({})], None));
        for args in bad {
            assert_eq!(action().prepare(args.clone()).unwrap_err().kind, Kind::BadRequest);
        }
        assert!(action().fields().is_empty());
    }

    #[test]
    fn a_token_needs_the_security_scope_to_read_it() {
        let call = |list: &str| {
            let scopes = Scopes::parse(list).unwrap();
            authorize(&Caller::Token { name: "ci", scopes: &scopes }, action())
        };
        for list in ["security", "security:read", "*", "services,security:read"] {
            assert!(call(list).is_ok(), "{list}");
        }
        for list in ["console", "metrics", "services:read"] {
            let refused = call(list).unwrap_err();
            assert_eq!(refused.kind, Kind::OutOfScope, "{list}");
            assert_eq!(
                refused.detail,
                "the API token \"ci\" may not do this: it needs the scope security:read, \
                 security or *"
            );
        }
    }
}
