// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The health checks' actions: `checks.last`, which runs `dukovo-check
//! --last --json` as the person and answers what the last run of the
//! checks kept, as the command printed it, and `checks.run`, which runs
//! the checks again as an administrator. The area is `security`, the scope
//! a token needs.
//!
//! `dukovo-check --last` exits with the status of the result it prints:
//! 0 when every check passed, 1 when one failed and 2 when one warned.
//! Each is an answer, the result as JSON. It exits 3 when no result is
//! kept, before the `dukovo-check` service's first morning run or a run by
//! hand: the answer is then ADM-123 with the command's words, which the
//! Overview shows as checks that have not run yet. On an older image
//! without the command the answer is ADM-129, "This server does not have
//! that command yet" (501), which the Overview shows as its own place for
//! the checks.
//!
//! `checks.run` runs `dukovo-check --json`, which does the checks that
//! only root can (`elevate dukovo-check` at a terminal) and keeps the
//! result as it does. It is a change for that reason, a `POST`: it writes
//! the kept result, which a `GET` does not. It is a change with no
//! journal files and no undo, as `console.export` is, and it compares
//! nothing: its `changed` is always true. It exits as `--last` does, 0 for
//! checks that all passed, 1 with a failure and 2 with a warning, each an
//! answer; 3, when it could not run, is ADM-123. The console never runs it
//! on a timer: the person asks.

use serde_json::json;

use super::{Action, Examples, Nothing, Runs, Tests};

/// The tests of `checks.last`, which `lab/actions.toml` names.
const TESTS: Tests = Tests {
    checks: "admind::actions::checks::tests",
    route: "admin-actions",
    same_as_command: "admin-actions",
    browser: "web-console/actions",
    ansible: "web-console/ansible",
};

/// The tests of `checks.run`: the Security page's boot and lab tests call
/// it, as they call the other actions of the page.
const RUN_TESTS: Tests = Tests {
    checks: "admind::actions::checks::tests",
    route: "admin-security",
    same_as_command: "admin-security",
    browser: "web-console/security",
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
    /// 1 and 2 are a kept result with a failure or a warning: the result
    /// is what is asked for then most.
    const ANSWERS: &'static [i32] = &[0, 1, 2];
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

/// `dukovo-check --json`: runs the checks as an administrator, and keeps
/// the result, which `checks.last` then answers.
pub struct Run;

impl Action for Run {
    const ID: &'static str = "checks.run";
    const AREA: &'static str = "security";
    const CHANGES: bool = true;
    const PATH: &'static str = "/api/1/checks/run";
    /// As `checks.last`: 1 and 2 are a result with a failure or a warning.
    const ANSWERS: &'static [i32] = &[0, 1, 2];
    const RUNS: Runs = Runs::AsAdministrator;
    const TESTS: Tests = RUN_TESTS;
    const TITLE: &'static str = "Run the checks";
    type Args = Nothing;

    fn argv(_: &Nothing) -> Vec<String> {
        [PROGRAM, "--json"].map(String::from).to_vec()
    }

    fn examples() -> Examples<Nothing> {
        Examples {
            good: vec![Nothing {}],
            bad: vec![json!({ "last": true }), json!({ "json": true })],
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
        assert_eq!(action.answers(), [0, 1, 2], "a result with failures or warnings is one too");
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

    /// The registered `checks.run`.
    fn run() -> &'static dyn Registered {
        *REGISTRY.iter().find(|action| action.id() == "checks.run").unwrap()
    }

    #[test]
    fn running_the_checks_is_a_post_of_an_administrator_that_answers_what_they_found() {
        let action = run();
        assert_eq!(
            (action.changes(), action.runs(), action.area(), action.long()),
            (true, Runs::AsAdministrator, "security", false)
        );
        assert_eq!((action.path(), action.title()), ("/api/1/checks/run", "Run the checks"));
        assert!(action.posts(), "a change is a POST, as it writes the kept result");
        assert_eq!(action.answers(), [0, 1, 2], "a result with failures or warnings is one too");
        assert!(action.errors().is_empty());
        assert!(!action.secret_answer());
        let prepared = action.prepare(json!({})).unwrap();
        assert_eq!(prepared.argv, ["dukovo-check", "--json"]);
        assert_eq!(prepared.argv[0], PROGRAM);
        assert_eq!((prepared.probe, prepared.files.len(), prepared.trial), (None, 0, None));
        assert_eq!(prepared.ansible.name, "Run the checks");
        assert_eq!(prepared.ansible.params["argv"], json!(["dukovo-check", "--json"]));
        assert!(prepared.ansible.become_root);
        assert_eq!(prepared.ansible.changed_when, None);
    }

    #[test]
    fn running_the_checks_takes_nothing_has_no_undo_and_compares_nothing() {
        let (good, bad, undo) = run().example_values();
        assert_eq!((good, undo), (vec![json!({})], None));
        for args in bad {
            assert_eq!(run().prepare(args.clone()).unwrap_err().kind, Kind::BadRequest, "{args}");
        }
        assert!(run().fields().is_empty());
        assert!(!run().compares(), "it always did something: it wrote a result");
        assert_eq!(run().tests().route, "admin-security");
        assert_eq!(run().tests().browser, "web-console/security");
    }

    #[test]
    fn a_token_needs_the_security_scope_to_run_the_checks_and_its_read_scope_is_not_enough() {
        let call = |list: &str| {
            let scopes = Scopes::parse(list).unwrap();
            authorize(&Caller::Token { name: "ci", scopes: &scopes }, run())
        };
        for list in ["security", "*", "services,security"] {
            assert!(call(list).is_ok(), "{list}");
        }
        for list in ["security:read", "console", "metrics"] {
            let refused = call(list).unwrap_err();
            assert_eq!(refused.kind, Kind::OutOfScope, "{list}");
            assert_eq!(
                refused.detail,
                "the API token \"ci\" may not do this: it needs the scope security or *"
            );
        }
    }
}
