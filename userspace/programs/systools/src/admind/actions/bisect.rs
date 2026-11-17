// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The search for the change that broke a start, from the Updates page
//! (ADR 0565): changes(1)'s `bisect`, in the area `updates`.
//!
//! The search restarts the machine once for each try, a start with some of
//! the changes left out ([`Restarts::Search`]): admind refuses to begin or
//! confirm one while a trial of settings runs (ADM-142) and while a
//! restart or a shut down is set (UPD-106). It asks for the first try's
//! restart a minute later (`--later`), so that the page is answered and
//! counts down before Spark stops admind; the health check restarts for
//! each try after that by itself.
//!
//! The search's state is under `/system/state/bisect`, which every account
//! may read and admind's service does not see, so each change names no
//! file and says whether it changed anything from `changes bisect --status
//! --json`, read as the person before and after it.

use serde_json::{Value, json};

use super::{Action, Ansible, Examples, Nothing, Restarts, Runs, Tests, Undo, command_task};

/// The area of these actions: the Updates page's, and the scope a token
/// needs.
const AREA: &str = "updates";

/// The tests of these actions, which `lab/actions.toml` names. The boot
/// test begins no search, which takes restarts of its own: `bisect` does
/// that at a terminal, and the route test reads the state and is refused.
const TESTS: Tests = Tests {
    checks: "admind::actions::bisect::tests",
    route: "admin-updates",
    same_as_command: "admin-updates",
    browser: "web-console/updates",
    ansible: "web-console/ansible",
};

/// `changes bisect --status --json`: the search, its tries and what it
/// found.
fn status() -> Vec<String> {
    ["changes", "bisect", "--status", "--json"].map(String::from).to_vec()
}

/// Whether the search runs, and the try it waits on, from what `changes
/// bisect --status --json` printed: what the changes move.
fn running(output: &str) -> Option<String> {
    let value: Value = serde_json::from_str(output.trim()).ok()?;
    let running = value.get("running")?.as_bool()?;
    let waiting = value.get("try")?;
    Some(format!("{running} {waiting}"))
}

/// The undo of a search begun: stopping it.
fn stopped() -> Option<Undo> {
    Some(Undo { action: Stop::ID, args: json!({}) })
}

/// `changes bisect --status --json`: the search, as the page shows it.
pub struct Status;

impl Action for Status {
    const ID: &'static str = "updates.bisect-status";
    const AREA: &'static str = AREA;
    const CHANGES: bool = false;
    const PATH: &'static str = "/api/1/updates/bisect";
    const RUNS: Runs = Runs::AsThePerson;
    const TESTS: Tests = TESTS;
    const TITLE: &'static str = "Show the search for a bad change";
    type Args = Nothing;

    fn argv(_: &Nothing) -> Vec<String> {
        status()
    }

    fn examples() -> Examples<Nothing> {
        Examples { good: vec![Nothing {}], bad: vec![json!({ "try": 1 })], undo: None }
    }
}

/// `changes bisect --yes --later`: begins the search, and restarts the
/// machine for its first try a minute from now.
pub struct Begin;

impl Action for Begin {
    const ID: &'static str = "updates.bisect";
    const AREA: &'static str = AREA;
    const CHANGES: bool = true;
    const PATH: &'static str = "/api/1/updates/bisect";
    const RUNS: Runs = Runs::AsAdministrator;
    const RESTARTS: Option<Restarts> = Some(Restarts::Search);
    const TESTS: Tests = TESTS;
    const TITLE: &'static str = "Search for the change that broke a start";
    type Args = Nothing;

    fn argv(_: &Nothing) -> Vec<String> {
        ["changes", "bisect", "--yes", "--later"].map(String::from).to_vec()
    }

    fn ansible(args: &Nothing) -> Ansible {
        command_task::<Self>(args).coming_back()
    }

    fn probe(_: &Nothing) -> Option<Vec<String>> {
        Some(status())
    }

    fn probed(_: &Nothing, output: &str) -> Option<String> {
        running(output)
    }

    fn examples() -> Examples<Nothing> {
        Examples { good: vec![Nothing {}], bad: vec![json!({ "yes": true })], undo: stopped() }
    }
}

/// `changes bisect --stop`: stops the search. The next start is the usual
/// one; a restart the search asked for is left set, for the page to cancel
/// or keep.
pub struct Stop;

impl Action for Stop {
    const ID: &'static str = "updates.bisect-stop";
    const AREA: &'static str = AREA;
    const CHANGES: bool = true;
    const PATH: &'static str = "/api/1/updates/bisect/stop";
    const RUNS: Runs = Runs::AsAdministrator;
    const TESTS: Tests = TESTS;
    const TITLE: &'static str = "Stop the search for a bad change";
    type Args = Nothing;

    fn argv(_: &Nothing) -> Vec<String> {
        ["changes", "bisect", "--stop"].map(String::from).to_vec()
    }

    fn probe(_: &Nothing) -> Option<Vec<String>> {
        Some(status())
    }

    fn probed(_: &Nothing, output: &str) -> Option<String> {
        running(output)
    }

    /// Nothing runs to stop when the search does not run.
    fn settled(_: &Nothing, now: &str) -> Option<bool> {
        Some(now.starts_with("false "))
    }

    fn examples() -> Examples<Nothing> {
        // A stopped search begins again from its start, which is no undo.
        Examples { good: vec![Nothing {}], bad: vec![json!({ "all": true })], undo: None }
    }
}

/// `changes bisect --confirm --yes --later`: makes sure of a change the
/// search named by elimination, with one more try that leaves out only
/// it, a minute from now.
pub struct Confirm;

impl Action for Confirm {
    const ID: &'static str = "updates.bisect-confirm";
    const AREA: &'static str = AREA;
    const CHANGES: bool = true;
    const PATH: &'static str = "/api/1/updates/bisect/confirm";
    const RUNS: Runs = Runs::AsAdministrator;
    const RESTARTS: Option<Restarts> = Some(Restarts::Search);
    const TESTS: Tests = TESTS;
    const TITLE: &'static str = "Make sure of the change the search named";
    type Args = Nothing;

    fn argv(_: &Nothing) -> Vec<String> {
        ["changes", "bisect", "--confirm", "--yes", "--later"].map(String::from).to_vec()
    }

    fn ansible(args: &Nothing) -> Ansible {
        command_task::<Self>(args).coming_back()
    }

    fn probe(_: &Nothing) -> Option<Vec<String>> {
        Some(status())
    }

    fn probed(_: &Nothing, output: &str) -> Option<String> {
        running(output)
    }

    fn examples() -> Examples<Nothing> {
        Examples { good: vec![Nothing {}], bad: vec![json!({ "change": "x" })], undo: stopped() }
    }
}

#[cfg(test)]
mod tests {
    use super::super::{REGISTRY, Registered};
    use super::*;

    /// The registered action `id`.
    fn action(id: &str) -> &'static dyn Registered {
        *REGISTRY.iter().find(|action| action.id() == id).unwrap()
    }

    #[test]
    fn the_search_is_read_as_the_person_and_begun_stopped_and_confirmed_by_an_administrator() {
        for (id, path, changes, runs, argv) in [
            (
                "updates.bisect-status",
                "/api/1/updates/bisect",
                false,
                Runs::AsThePerson,
                &["changes", "bisect", "--status", "--json"][..],
            ),
            (
                "updates.bisect",
                "/api/1/updates/bisect",
                true,
                Runs::AsAdministrator,
                &["changes", "bisect", "--yes", "--later"],
            ),
            (
                "updates.bisect-stop",
                "/api/1/updates/bisect/stop",
                true,
                Runs::AsAdministrator,
                &["changes", "bisect", "--stop"],
            ),
            (
                "updates.bisect-confirm",
                "/api/1/updates/bisect/confirm",
                true,
                Runs::AsAdministrator,
                &["changes", "bisect", "--confirm", "--yes", "--later"],
            ),
        ] {
            let action = action(id);
            assert_eq!((action.path(), action.changes(), action.runs()), (path, changes, runs));
            assert_eq!((action.area(), action.long()), ("updates", false), "{id}");
            let prepared = action.prepare(json!({})).unwrap();
            assert_eq!(prepared.argv, argv, "{id}");
            assert!(prepared.files.is_empty(), "{id}: the search's state is not admind's to see");
            assert_eq!(prepared.probe.is_some(), changes, "{id}");
            assert!(action.prepare(json!({ "now": true })).is_err(), "{id} takes nothing");
        }
    }

    #[test]
    fn beginning_and_confirming_restart_the_machine_and_stopping_does_not() {
        assert_eq!(action("updates.bisect").restarts(), Some(Restarts::Search));
        assert_eq!(action("updates.bisect-confirm").restarts(), Some(Restarts::Search));
        assert_eq!(action("updates.bisect-stop").restarts(), None);
        assert_eq!(action("updates.bisect-status").restarts(), None);
    }

    #[test]
    fn what_moves_is_whether_it_runs_and_the_try_it_waits_on() {
        let none = r#"{"running":false,"try":null,"of":null,"failed_start":null}"#;
        let first = r#"{"running":true,"try":1,"of":3,"failed_start":42,"next_ms":5}"#;
        let later = r#"{"running":true,"try":1,"of":3,"failed_start":42,"next_ms":null}"#;
        assert_eq!(running(none).as_deref(), Some("false null"));
        assert_eq!(running(first).as_deref(), Some("true 1"));
        assert_eq!(running(first), running(later), "the time of the restart is no change");
        assert_eq!(running("not json"), None);
        assert_eq!(running(r#"{"try":1}"#), None);
        let stop = action("updates.bisect-stop");
        assert_eq!(stop.settled(&json!({}), "false null"), Some(true));
        assert_eq!(stop.settled(&json!({}), "true 2"), Some(false));
    }
}
