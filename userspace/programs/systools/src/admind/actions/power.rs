// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Restarting and shutting down the machine from the console (ADR 0565):
//! the area `power`, on the Hardware page and wherever a page offers
//! Restart now (Updates, Move).
//!
//! Each change runs shutdown(8) as an administrator, as `elevate shutdown
//! -r +1` does at a terminal, and Spark ends the machine a minute after it
//! answers, or at the time asked for. The minute is what lets admind answer
//! and the page count down before Spark stops admind, and what gives a
//! person who clicked by mistake the time to cancel: `power.cancel` runs
//! `shutdown -c`. `--no-wall` keeps the request off the terminals, since
//! the console tells its own pages (`event: power`).
//!
//! `power.restart` and `power.shutdown` restart the machine
//! ([`Restarts::Machine`]): admind refuses them while a trial runs, so that
//! a restart does not end one half way (ADM-142), and while `changes
//! bisect` runs, whose tries restart the machine on their own (UPD-106).
//!
//! `power.status` reads the shutdown that is set, `shutdown --show
//! --json`, as the person: Spark's read socket answers anyone. It is each
//! change's probe, so a cancel with nothing set says it changed nothing.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::{Action, Args, Bad, Examples, Field, Nothing, Restarts, Runs, Tests, Undo};

/// The area of these actions: the scope a token needs, `power`.
pub(super) const AREA: &str = "power";

/// The tests of these actions, which `lab/actions.toml` names. The boot
/// test restarts the machine once, at its end; the lab test sets a restart
/// and a shut down and cancels each within its minute.
const TESTS: Tests = Tests {
    checks: "admind::actions::power::tests",
    route: "admin-hardware",
    same_as_command: "admin-hardware",
    browser: "web-console/hardware",
    ansible: "web-console/ansible",
};

/// The words of `shutdown --show --json`, the read of what is set.
fn shown() -> Vec<String> {
    ["shutdown", "--show", "--json"].map(String::from).to_vec()
}

/// The shutdown that is set, from what `shutdown --show --json` printed,
/// as text to compare: `null` for none. None when it does not read.
fn pending(output: &str) -> Option<String> {
    let value: Value = serde_json::from_str(output.trim()).ok()?;
    let shutdown = value.get("shutdown")?;
    if shutdown.is_null() {
        return Some("null".into());
    }
    // The time left moves on its own between two reads, and the time it
    // gives is to the second; what is set is its end and when it begins,
    // to the nearest minute, so that a time asked for, which is on the
    // minute, reads the same each time.
    let end = shutdown.get("end")?.as_str()?;
    let at = shutdown.get("at_ms")?.as_u64()?.saturating_add(30_000) / 60_000;
    Some(format!("{end} {at}"))
}

/// `shutdown --show --json`: the restart or shut down that is set, if one
/// is.
pub struct Status;

impl Action for Status {
    const ID: &'static str = "power.status";
    const AREA: &'static str = AREA;
    const CHANGES: bool = false;
    const PATH: &'static str = "/api/1/power";
    const RUNS: Runs = Runs::AsThePerson;
    const TESTS: Tests = TESTS;
    const TITLE: &'static str = "Show a restart or shut down that is set";
    type Args = Nothing;

    fn argv(_: &Nothing) -> Vec<String> {
        shown()
    }

    fn examples() -> Examples<Nothing> {
        Examples { good: vec![Nothing {}], bad: vec![json!({ "at": "03:00" })], undo: None }
    }
}

/// When to restart.
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct When {
    /// A time of day, `HH:MM` by the machine's clock, the next time it
    /// comes; null for a minute from now.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<String>,
}

impl Args for When {
    const FIELDS: &'static [Field] = &[Field::text(
        "at",
        "When to restart: a time of day as HH:MM by the machine's clock, or leave it out for a \
         minute from now.",
        5,
        5,
        Some("^([01][0-9]|2[0-3]):[0-5][0-9]$"),
    )
    .optional()];

    fn check(&self) -> Result<(), Bad> {
        match &self.at {
            None => Ok(()),
            Some(at) if is_time(at) => Ok(()),
            Some(_) => Err(Bad {
                field: "at",
                why: "A time is written HH:MM, from 00:00 to 23:59, such as 03:00. Nothing was \
                      set."
                    .into(),
            }),
        }
    }
}

/// Whether `at` is a time of day as `HH:MM`, from 00:00 to 23:59.
fn is_time(at: &str) -> bool {
    let bytes = at.as_bytes();
    let digits = |range: std::ops::Range<usize>| bytes[range].iter().all(u8::is_ascii_digit);
    if bytes.len() != 5 || bytes[2] != b':' || !digits(0..2) || !digits(3..5) {
        return false;
    }
    let number = |range: std::ops::Range<usize>| at[range].parse::<u32>().ok();
    matches!((number(0..2), number(3..5)), (Some(h), Some(m)) if h <= 23 && m <= 59)
}

/// The undo of a restart or a shut down: cancelling it.
fn cancelled() -> Option<Undo> {
    Some(Undo { action: Cancel::ID, args: json!({}) })
}

/// `shutdown -r (+1 | HH:MM) --no-wall`: restarts the machine a minute from
/// now, or at a time of day.
pub struct Restart;

impl Action for Restart {
    const ID: &'static str = "power.restart";
    const AREA: &'static str = AREA;
    const CHANGES: bool = true;
    const PATH: &'static str = "/api/1/power/restart";
    const RUNS: Runs = Runs::AsAdministrator;
    const RESTARTS: Option<Restarts> = Some(Restarts::Machine);
    const TESTS: Tests = TESTS;
    const TITLE: &'static str = "Restart this machine";
    type Args = When;

    fn argv(args: &When) -> Vec<String> {
        let at = args.at.clone().unwrap_or_else(|| "+1".into());
        vec!["shutdown".into(), "-r".into(), at, "--no-wall".into()]
    }

    fn subject(args: &When) -> Option<String> {
        args.at.as_ref().map(|at| format!("at {at}"))
    }

    fn probe(_: &When) -> Option<Vec<String>> {
        Some(shown())
    }

    fn probed(_: &When, output: &str) -> Option<String> {
        pending(output)
    }

    fn examples() -> Examples<When> {
        Examples {
            good: vec![When { at: None }, When { at: Some("03:00".into()) }],
            bad: vec![
                json!({ "at": "3am" }),
                json!({ "at": "24:00" }),
                json!({ "at": "03:00", "now": true }),
            ],
            undo: cancelled(),
        }
    }
}

/// `shutdown -P +1 --no-wall`: turns the machine off a minute from now.
/// Nothing turns it on again but its power button, which the page says
/// before it asks.
pub struct Shutdown;

impl Action for Shutdown {
    const ID: &'static str = "power.shutdown";
    const AREA: &'static str = AREA;
    const CHANGES: bool = true;
    const PATH: &'static str = "/api/1/power/shutdown";
    const RUNS: Runs = Runs::AsAdministrator;
    const RESTARTS: Option<Restarts> = Some(Restarts::Machine);
    const TESTS: Tests = TESTS;
    const TITLE: &'static str = "Shut down this machine";
    type Args = Nothing;

    fn argv(_: &Nothing) -> Vec<String> {
        ["shutdown", "-P", "+1", "--no-wall"].map(String::from).to_vec()
    }

    fn probe(_: &Nothing) -> Option<Vec<String>> {
        Some(shown())
    }

    fn probed(_: &Nothing, output: &str) -> Option<String> {
        pending(output)
    }

    fn examples() -> Examples<Nothing> {
        Examples { good: vec![Nothing {}], bad: vec![json!({ "at": "03:00" })], undo: cancelled() }
    }
}

/// `shutdown -c --no-wall`: cancels the restart or shut down that is set.
/// With none set it changes nothing, and says so.
pub struct Cancel;

impl Action for Cancel {
    const ID: &'static str = "power.cancel";
    const AREA: &'static str = AREA;
    const CHANGES: bool = true;
    const PATH: &'static str = "/api/1/power/cancel";
    const RUNS: Runs = Runs::AsAdministrator;
    const TESTS: Tests = TESTS;
    const TITLE: &'static str = "Cancel a restart or shut down";
    type Args = Nothing;

    fn argv(_: &Nothing) -> Vec<String> {
        ["shutdown", "-c", "--no-wall"].map(String::from).to_vec()
    }

    fn probe(_: &Nothing) -> Option<Vec<String>> {
        Some(shown())
    }

    fn probed(_: &Nothing, output: &str) -> Option<String> {
        pending(output)
    }

    /// Nothing is set to cancel when `shutdown --show --json` says none.
    fn settled(_: &Nothing, now: &str) -> Option<bool> {
        Some(now == "null")
    }

    fn examples() -> Examples<Nothing> {
        // What was set before is in the journal's row for it; a cancel puts
        // nothing back.
        Examples { good: vec![Nothing {}], bad: vec![json!({ "all": true })], undo: None }
    }
}

#[cfg(test)]
mod tests {
    use super::super::{REGISTRY, Registered};
    use super::*;
    use crate::admind::errors::Kind;

    /// The registered action `id`.
    fn action(id: &str) -> &'static dyn Registered {
        *REGISTRY.iter().find(|action| action.id() == id).unwrap()
    }

    /// The command `id` makes of `args`.
    fn argv(id: &str, args: Value) -> Vec<String> {
        action(id).prepare(args).unwrap().argv
    }

    #[test]
    fn a_restart_is_a_minute_from_now_or_at_a_time_of_day() {
        assert_eq!(argv("power.restart", json!({})), ["shutdown", "-r", "+1", "--no-wall"]);
        let at_null = argv("power.restart", json!({ "at": null }));
        assert_eq!(at_null, ["shutdown", "-r", "+1", "--no-wall"]);
        let at = argv("power.restart", json!({ "at": "03:00" }));
        assert_eq!(at, ["shutdown", "-r", "03:00", "--no-wall"]);
        assert_eq!(argv("power.shutdown", json!({})), ["shutdown", "-P", "+1", "--no-wall"]);
        assert_eq!(argv("power.cancel", json!({})), ["shutdown", "-c", "--no-wall"]);
        assert_eq!(argv("power.status", json!({})), ["shutdown", "--show", "--json"]);
    }

    #[test]
    fn each_is_where_and_whom_the_design_says() {
        for (id, path, changes, runs) in [
            ("power.status", "/api/1/power", false, Runs::AsThePerson),
            ("power.restart", "/api/1/power/restart", true, Runs::AsAdministrator),
            ("power.shutdown", "/api/1/power/shutdown", true, Runs::AsAdministrator),
            ("power.cancel", "/api/1/power/cancel", true, Runs::AsAdministrator),
        ] {
            let action = action(id);
            assert_eq!((action.path(), action.changes(), action.runs()), (path, changes, runs));
            assert_eq!((action.area(), action.long()), ("power", false), "{id}");
            assert_eq!(action.compares(), changes, "{id}: a change compares what is set");
        }
        let restart = action("power.restart").prepare(json!({ "at": "03:00" })).unwrap();
        assert_eq!(restart.ansible.name, "Restart this machine: at 03:00");
        assert!(restart.ansible.become_root);
        assert_eq!(restart.probe.as_deref(), Some(&shown()[..]));
        assert!(restart.files.is_empty());
    }

    #[test]
    fn a_time_is_hours_and_minutes_of_one_day() {
        for good in ["00:00", "03:00", "23:59", "12:30"] {
            assert!(is_time(good), "{good}");
        }
        for bad in
            ["3:00", "24:00", "12:60", "03-00", "03:000", "+1", "now", "\u{661}\u{662}:30", ""]
        {
            assert!(!is_time(bad), "{bad}");
        }
        for (bad, kind) in [
            (json!({ "at": "3am" }), Kind::BadValue),
            (json!({ "at": "24:00" }), Kind::BadValue),
            (json!({ "at": "03:00", "now": true }), Kind::BadRequest),
        ] {
            let failure = action("power.restart").prepare(bad.clone()).unwrap_err();
            assert_eq!(failure.kind, kind, "{bad}");
        }
    }

    #[test]
    fn what_is_set_is_compared_by_its_end_and_its_minute() {
        let none = r#"{"format":1,"shutdown":null}"#;
        let set = r#"{"format":1,"shutdown":{"end":"reboot","in_seconds":60,"at_ms":120000}}"#;
        let later = r#"{"format":1,"shutdown":{"end":"reboot","in_seconds":59,"at_ms":120900}}"#;
        let off = r#"{"format":1,"shutdown":{"end":"poweroff","in_seconds":60,"at_ms":120000}}"#;
        assert_eq!(pending(none).as_deref(), Some("null"));
        assert_eq!(pending(set), pending(later), "the same restart, read again");
        let early = r#"{"format":1,"shutdown":{"end":"reboot","in_seconds":1,"at_ms":179400}}"#;
        let late = r#"{"format":1,"shutdown":{"end":"reboot","in_seconds":0,"at_ms":180300}}"#;
        assert_eq!(pending(early), pending(late), "a time on the minute, read either side");
        assert_ne!(pending(set), pending(off));
        assert_eq!(pending("not json"), None);
        assert_eq!(pending(r#"{"format":1}"#), None);
        let cancel = action("power.cancel");
        let args = json!({});
        assert_eq!(cancel.settled(&args, "null"), Some(true));
        assert_eq!(cancel.settled(&args, "reboot 2"), Some(false));
    }

    #[test]
    fn restarting_and_shutting_down_restart_the_machine_and_nothing_else_does() {
        assert_eq!(action("power.restart").restarts(), Some(Restarts::Machine));
        assert_eq!(action("power.shutdown").restarts(), Some(Restarts::Machine));
        assert_eq!(action("power.cancel").restarts(), None);
        assert_eq!(action("power.status").restarts(), None);
    }
}
