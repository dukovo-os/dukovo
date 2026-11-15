// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The lab's own actions: `lab.long` (ADR 0516), a change that runs as a
//! task for some seconds, for the tests of tasks to run, since no action
//! of the console is long yet, and `lab.missing`, a read whose program is
//! not on the machine, for the test of the error view to be given ADM-129.
//! Both are in the host tests' set, and in the actions a lab build offers
//! ([`offered`](super::offered)); the build that ships has neither.
//!
//! The long action's command, `admin lab-steps SECONDS`, prints `step N of
//! 4: WORDS` and `progress P%` lines over SECONDS seconds, with log lines
//! between, and ends at once and well when it is sent SIGTERM, as a command
//! that puts back what it did does. It changes nothing on the machine.
//!
//! The missing action's command, `lab-missing --json`, is in no image, so
//! the worker says there is no such command, ADM-129, with the program's
//! name, and never runs anything.

use serde::{Deserialize, Serialize};

use super::{Action, Args, Bad, Examples, Field, Nothing, Runs, Tests};

/// How long the command runs when the request does not say, in seconds.
pub const SECONDS_DEFAULT: u32 = 20;

/// The longest it may be asked to run, in seconds.
pub const SECONDS_MAX: u32 = 600;

/// How long the command should run.
#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Seconds {
    /// In seconds, 1 to 600; 20 when it is not said.
    #[serde(default)]
    pub seconds: Option<u32>,
}

impl Seconds {
    /// The seconds the command runs.
    pub fn get(&self) -> u32 {
        self.seconds.unwrap_or(SECONDS_DEFAULT)
    }
}

impl Args for Seconds {
    const FIELDS: &'static [Field] = &[Field::number(
        "seconds",
        "How long the command runs, in seconds: 20 when it is not given.",
        1,
        SECONDS_MAX as i64,
    )
    .optional()];

    fn check(&self) -> Result<(), Bad> {
        if self.seconds.is_some_and(|seconds| !(1..=SECONDS_MAX).contains(&seconds)) {
            return Err(Bad {
                field: "seconds",
                why: format!("Seconds go from 1 to {SECONDS_MAX}. Nothing was started."),
            });
        }
        Ok(())
    }
}

/// The tests of the lab's actions, which the lab's own boot and lab tests
/// are.
pub const TESTS: Tests = Tests {
    checks: "admind::actions::tests",
    route: "admin-journal",
    same_as_command: "admin-journal",
    browser: "web-console/tasks",
    ansible: "web-console/ansible",
};

/// `admin lab-steps`, as a task.
pub struct Long;

impl Action for Long {
    const ID: &'static str = "lab.long";
    const AREA: &'static str = "services";
    const CHANGES: bool = true;
    const PATH: &'static str = "/api/1/lab/long";
    const RUNS: Runs = Runs::AsAdministrator;
    const LONG: bool = true;
    const TESTS: Tests = TESTS;
    const TITLE: &'static str = "Run the lab's long test";
    type Args = Seconds;

    fn argv(args: &Seconds) -> Vec<String> {
        vec!["admin".into(), "lab-steps".into(), args.get().to_string()]
    }

    fn examples() -> Examples<Seconds> {
        Examples {
            good: vec![Seconds { seconds: Some(SECONDS_DEFAULT) }],
            bad: vec![serde_json::json!({ "seconds": 0 }), serde_json::json!({ "seconds": 601 })],
            undo: None,
        }
    }
}

/// The tests of `lab.missing`: the error's own are the unit tests of
/// `admind::errors` and `admind::worker`, and the lab test `errors` calls
/// it from the page.
pub const MISSING_TESTS: Tests = Tests {
    checks: "admind::actions::tests",
    route: "admin-journal",
    same_as_command: "admin-journal",
    browser: "web-console/errors",
    ansible: "web-console/ansible",
};

/// The program of [`Missing`], which no image has.
pub const MISSING_PROGRAM: &str = "lab-missing";

/// `lab-missing --json`, a command that is not there: its answer is
/// always ADM-129, naming it.
pub struct Missing;

impl Action for Missing {
    const ID: &'static str = "lab.missing";
    const AREA: &'static str = "services";
    const CHANGES: bool = false;
    const PATH: &'static str = "/api/1/lab/missing";
    const RUNS: Runs = Runs::AsThePerson;
    const TESTS: Tests = MISSING_TESTS;
    const TITLE: &'static str = "Run a command that is not there";
    type Args = Nothing;

    fn argv(_: &Nothing) -> Vec<String> {
        vec![MISSING_PROGRAM.into(), "--json".into()]
    }

    fn examples() -> Examples<Nothing> {
        Examples {
            good: vec![Nothing {}],
            bad: vec![serde_json::json!({ "name": "lab-missing" })],
            undo: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::super::tests::LAB;
    use super::super::{Found, Registered, find};
    use super::*;
    use crate::admind::errors::Kind;

    /// The lab's action `id`, from the tests' set.
    fn lab(id: &str) -> &'static dyn Registered {
        *LAB.iter().find(|action| action.id() == id).unwrap()
    }

    #[test]
    fn the_missing_action_is_a_read_of_a_command_that_is_in_no_image() {
        let action = lab("lab.missing");
        assert_eq!(
            (action.changes(), action.runs(), action.long()),
            (false, Runs::AsThePerson, false)
        );
        assert_eq!(action.path(), "/api/1/lab/missing");
        let prepared = action.prepare(json!({})).unwrap();
        assert_eq!(prepared.argv, [MISSING_PROGRAM, "--json"]);
        assert_eq!(prepared.probe, None);
        assert!(matches!(find(LAB, false, "/api/1/lab/missing"), Found::Action(..)));
        assert!(matches!(find(LAB, true, "/api/1/lab/missing"), Found::WrongMethod));
    }

    #[test]
    fn the_missing_action_takes_no_arguments() {
        let action = lab("lab.missing");
        let (good, bad, undo) = action.example_values();
        assert_eq!((good, undo), (vec![json!({})], None));
        for args in bad {
            let failure = action.prepare(args.clone()).unwrap_err();
            assert_eq!(failure.kind, Kind::BadRequest, "{args}");
        }
    }

    #[test]
    fn the_long_action_runs_admin_lab_steps_for_the_seconds_asked() {
        let action = lab("lab.long");
        assert_eq!(action.prepare(json!({})).unwrap().argv, ["admin", "lab-steps", "20"]);
        assert_eq!(action.prepare(json!({ "seconds": 600 })).unwrap().argv[2], "600");
        for seconds in [0, 601] {
            let failure = action.prepare(json!({ "seconds": seconds })).unwrap_err();
            assert_eq!((failure.kind, failure.field), (Kind::BadValue, Some("seconds")));
        }
    }
}
