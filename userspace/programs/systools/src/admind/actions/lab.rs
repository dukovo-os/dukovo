// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The lab's long action (ADR 0516): `lab.long`, a change that runs as a
//! task for some seconds, for the tests of tasks to run, since no action
//! of the console is long yet. It is in the host tests' set, and in the
//! actions a lab build offers ([`offered`](super::offered)); the build that
//! ships has neither.
//!
//! Its command, `admin lab-steps SECONDS`, prints `step N of 4: WORDS` and
//! `progress P%` lines over SECONDS seconds, with log lines between, and
//! ends at once and well when it is sent SIGTERM, as a command that puts
//! back what it did does. It changes nothing on the machine.

use serde::{Deserialize, Serialize};

use super::{Action, Args, Bad, Examples, Field, Runs, Tests};

/// How long the command runs when the request does not say, in seconds.
pub const SECONDS_DEFAULT: u32 = 20;

/// The longest it may be asked to run, in seconds.
pub const SECONDS_MAX: u32 = 120;

/// How long the command should run.
#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Seconds {
    /// In seconds, 1 to 120; 20 when it is not said.
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
            bad: vec![serde_json::json!({ "seconds": 0 }), serde_json::json!({ "seconds": 121 })],
            undo: None,
        }
    }
}
