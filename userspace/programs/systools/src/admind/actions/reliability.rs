// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The Reliability page's actions (ADR 0518): the starts the machine has
//! made and how each went, the crash dumps, the alerts, and copying the
//! settings into the person's own Git. The area is `reliability`, the
//! scope a token needs, except for `console.export`, which is the console's
//! own like the settings it copies.
//!
//! Three reads are of the start records the health check keeps (ADR 0500,
//! changes(1) and startup(8)): `reliability.starts` is `changes --list
//! --json`, `reliability.failed` is `changes --failed --json` and
//! `reliability.start` is `startup --start N --compare --json`. The records
//! are readable by everyone, as `admin about` reads them, so these run as
//! the person.
//!
//! `startup` ends with status 1, after text that is not JSON, for a start
//! with no times, and with an error for one that is not kept, so its answer
//! is its output after status 0 alone, and any other status is ADM-123 with
//! the command's own last line of error output, which the page says is "No
//! times are kept for this start". A start with no good start before it to
//! compare with, as a computer's first, answers with `compared_with` null.
//!
//! The crash dumps and the alerts are root's (dumps(8), alerts(8)), so
//! `reliability.dumps` and `reliability.alerts` run as an administrator, as
//! `elevate dumps list` and `elevate alerts history` do. alertd answers a
//! line of JSON for each change, which the answer takes as an array.
//!
//! `console.export` runs `admin export DIR` (ADR 0516) as the person, not
//! elevated: the command copies what the person can read of the settings
//! into a folder of theirs, as a Git commit, and lists the rest in
//! `SECRETS.md`. It writes nothing outside the folder, and nothing on the
//! machine that the journal watches, so it names no file. It says whether
//! it changed anything by reading the repository's newest commit before and
//! after: an export of settings that did not change makes no commit.

use serde::{Deserialize, Serialize};
use serde_json::json;

use super::{Action, Args, Bad, Examples, Field, Nothing, Runs, Tests};

/// The area of these actions: the scope a token needs, `reliability`.
const AREA: &str = "reliability";

/// The tests of these actions, which `lab/actions.toml` names.
const TESTS: Tests = Tests {
    checks: "admind::actions::reliability::tests",
    route: "admin-actions",
    same_as_command: "admin-actions",
    browser: "web-console/reliability",
    ansible: "web-console/ansible",
};

/// The greatest start number a read takes. The records number a start
/// above the one before it, so this is far past what a machine reaches.
const START_MAX: i64 = 4_294_967_295;

/// How many of the alerts' changes the history gives.
const ALERTS_SHOWN: &str = "200";

/// The most bytes a path of the machine has.
const PATH_MAX: usize = 4096;

/// What an absolute path matches, apart from its length: it starts with
/// `/` and has no NUL.
const PATH_PATTERN: &str = "^/[^\u{0}]*$";

/// `changes --list --json`: the starts kept, newest first, and how each
/// went.
pub struct Starts;

impl Action for Starts {
    const ID: &'static str = "reliability.starts";
    const AREA: &'static str = AREA;
    const CHANGES: bool = false;
    const PATH: &'static str = "/api/1/starts";
    const RUNS: Runs = Runs::AsThePerson;
    const TESTS: Tests = TESTS;
    const TITLE: &'static str = "List the starts";
    type Args = Nothing;

    fn argv(_: &Nothing) -> Vec<String> {
        ["changes", "--list", "--json"].map(String::from).to_vec()
    }

    fn examples() -> Examples<Nothing> {
        read_examples()
    }
}

/// `changes --failed --json`: the newest start that failed, what changed
/// since the last good one and the search for the change, if one runs.
pub struct Failed;

impl Action for Failed {
    const ID: &'static str = "reliability.failed";
    const AREA: &'static str = AREA;
    const CHANGES: bool = false;
    const PATH: &'static str = "/api/1/starts/failed";
    const RUNS: Runs = Runs::AsThePerson;
    const TESTS: Tests = TESTS;
    const TITLE: &'static str = "Show the failed start";
    type Args = Nothing;

    fn argv(_: &Nothing) -> Vec<String> {
        ["changes", "--failed", "--json"].map(String::from).to_vec()
    }

    fn examples() -> Examples<Nothing> {
        read_examples()
    }
}

/// A start, by its number.
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Numbered {
    /// The start's number, from 1.
    pub start: i64,
}

impl Args for Numbered {
    const FIELDS: &'static [Field] = &[Field::number(
        "start",
        "The start's number, from 1: the first the machine made is 1.",
        1,
        START_MAX,
    )];

    fn check(&self) -> Result<(), Bad> {
        if (1..=START_MAX).contains(&self.start) {
            return Ok(());
        }
        Err(Bad {
            field: "start",
            why: format!(
                "A start's number is a whole number from 1 to {START_MAX}. Nothing was done."
            ),
        })
    }
}

/// `startup --start N --compare --json`: the steps of a start, beside the
/// last good start's.
pub struct Start;

impl Action for Start {
    const ID: &'static str = "reliability.start";
    const AREA: &'static str = AREA;
    const CHANGES: bool = false;
    const PATH: &'static str = "/api/1/starts/{start}";
    const RUNS: Runs = Runs::AsThePerson;
    const TESTS: Tests = TESTS;
    const TITLE: &'static str = "Show a start's times";
    type Args = Numbered;

    fn argv(args: &Numbered) -> Vec<String> {
        let start = args.start.to_string();
        ["startup", "--start", start.as_str(), "--compare", "--json"].map(String::from).to_vec()
    }

    fn subject(args: &Numbered) -> Option<String> {
        Some(args.start.to_string())
    }

    fn examples() -> Examples<Numbered> {
        Examples {
            good: vec![Numbered { start: 1 }],
            bad: vec![
                json!({ "start": 0 }),
                json!({ "start": -1 }),
                json!({ "start": START_MAX + 1 }),
                json!({ "start": 1, "compare": 2 }),
                json!({}),
            ],
            undo: None,
        }
    }
}

/// `dumps list --json`: the crash dumps kept, newest first.
pub struct Dumps;

impl Action for Dumps {
    const ID: &'static str = "reliability.dumps";
    const AREA: &'static str = AREA;
    const CHANGES: bool = false;
    const PATH: &'static str = "/api/1/dumps";
    const RUNS: Runs = Runs::AsAdministrator;
    const TESTS: Tests = TESTS;
    const TITLE: &'static str = "List the crash dumps";
    type Args = Nothing;

    fn argv(_: &Nothing) -> Vec<String> {
        ["dumps", "list", "--json"].map(String::from).to_vec()
    }

    fn examples() -> Examples<Nothing> {
        read_examples()
    }
}

/// `alerts history -n 200 --json`: the newest changes to the alerts, a
/// line of JSON each.
pub struct Alerts;

impl Action for Alerts {
    const ID: &'static str = "reliability.alerts";
    const AREA: &'static str = AREA;
    const CHANGES: bool = false;
    const PATH: &'static str = "/api/1/alerts/history";
    const RUNS: Runs = Runs::AsAdministrator;
    const LINES: bool = true;
    const TESTS: Tests = TESTS;
    const TITLE: &'static str = "Show the alert history";
    type Args = Nothing;

    fn argv(_: &Nothing) -> Vec<String> {
        ["alerts", "history", "-n", ALERTS_SHOWN, "--json"].map(String::from).to_vec()
    }

    fn examples() -> Examples<Nothing> {
        read_examples()
    }
}

/// What the reads that take nothing are tried with: nothing, and a
/// word that none of them takes.
fn read_examples() -> Examples<Nothing> {
    Examples { good: vec![Nothing {}], bad: vec![json!({ "all": true })], undo: None }
}

/// The folder an export goes into.
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Folder {
    /// The folder, as a path from `/`.
    pub dir: String,
}

impl Args for Folder {
    const FIELDS: &'static [Field] = &[Field::text(
        "dir",
        "The folder to copy the settings into, as an absolute path such as /users/ann/host-settings.",
        1,
        PATH_MAX,
        Some(PATH_PATTERN),
    )];

    fn check(&self) -> Result<(), Bad> {
        let why = if !self.dir.starts_with('/') {
            "The folder is a path that starts with /"
        } else if self.dir.len() > PATH_MAX {
            "The folder's path is at most 4096 bytes long"
        } else if self.dir.contains('\0') {
            "The folder's path has no NUL in it"
        } else {
            return Ok(());
        };
        Err(Bad { field: "dir", why: format!("{why}. Nothing was done.") })
    }
}

/// The folder the examples copy into: made, and filled, by the test that
/// runs them.
const EXAMPLE_DIR: &str = "/tmp/admin-export-example";

/// `admin export DIR`: copies what the person can read of the settings into
/// a Git repository, as a commit, and lists what it left out.
pub struct Export;

impl Action for Export {
    const ID: &'static str = "console.export";
    const AREA: &'static str = "console";
    const CHANGES: bool = true;
    const PATH: &'static str = "/api/1/console/export";
    const RUNS: Runs = Runs::AsThePerson;
    const TESTS: Tests = TESTS;
    const TITLE: &'static str = "Copy the settings to Git";
    type Args = Folder;

    fn argv(args: &Folder) -> Vec<String> {
        ["admin", "export", args.dir.as_str()].map(String::from).to_vec()
    }

    fn subject(args: &Folder) -> Option<String> {
        Some(args.dir.clone())
    }

    /// `git -C DIR rev-parse --verify -q HEAD`: the repository's newest
    /// commit, which the export moves unless nothing changed. A folder that
    /// is no repository yet, or has no commit, says nothing and the export
    /// has changed it.
    fn probe(args: &Folder) -> Option<Vec<String>> {
        let probe = ["git", "-C", args.dir.as_str(), "rev-parse", "--verify", "-q", "HEAD"];
        Some(probe.map(String::from).to_vec())
    }

    fn examples() -> Examples<Folder> {
        Examples {
            good: vec![Folder { dir: EXAMPLE_DIR.into() }],
            bad: vec![
                json!({ "dir": "relative" }),
                json!({ "dir": "" }),
                json!({ "dir": "~/settings" }),
                json!({ "dir": "/tmp/x\u{0}y" }),
                json!({ "dir": "/".repeat(PATH_MAX + 1) }),
                json!({ "dir": EXAMPLE_DIR, "message": "x" }),
                json!({}),
            ],
            undo: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::Value;

    use super::super::{Found, REGISTRY, Registered, find, schema};
    use super::*;
    use crate::admind::errors::Kind;
    use crate::admind::scopes::Scopes;

    /// The registered action `id`.
    fn action(id: &str) -> &'static dyn Registered {
        *REGISTRY.iter().find(|action| action.id() == id).unwrap()
    }

    /// The command `id` makes for `args`.
    fn argv(id: &str, args: Value) -> Vec<String> {
        action(id).prepare(args).unwrap().argv
    }

    /// Why `id` refuses `args`: the kind, the field and the words.
    fn refused(id: &str, args: Value) -> (Kind, Option<&'static str>, String) {
        let failure = action(id).prepare(args).unwrap_err();
        (failure.kind, failure.field, failure.detail)
    }

    const READS: [(&str, &str, Runs, &[&str]); 5] = [
        (
            "reliability.starts",
            "/api/1/starts",
            Runs::AsThePerson,
            &["changes", "--list", "--json"],
        ),
        (
            "reliability.failed",
            "/api/1/starts/failed",
            Runs::AsThePerson,
            &["changes", "--failed", "--json"],
        ),
        ("reliability.dumps", "/api/1/dumps", Runs::AsAdministrator, &["dumps", "list", "--json"]),
        (
            "reliability.alerts",
            "/api/1/alerts/history",
            Runs::AsAdministrator,
            &["alerts", "history", "-n", "200", "--json"],
        ),
        (
            "reliability.start",
            "/api/1/starts/{start}",
            Runs::AsThePerson,
            &["startup", "--start", "1", "--compare", "--json"],
        ),
    ];

    #[test]
    fn each_read_runs_its_command_with_json_as_the_person_or_the_administrator_the_design_names() {
        for (id, path, runs, words) in READS {
            let args = if id == "reliability.start" { json!({ "start": 1 }) } else { json!({}) };
            let prepared = action(id).prepare(args).unwrap();
            assert_eq!(prepared.argv, words, "{id}");
            let action = action(id);
            assert_eq!(
                (action.changes(), action.runs(), action.area(), action.path(), action.long()),
                (false, runs, "reliability", path, false),
                "{id}"
            );
            assert!(action.errors().is_empty(), "{id}");
            assert!(!action.compares(), "{id}");
            assert_eq!((prepared.probe, prepared.files), (None, Vec::new()), "{id}");
            assert_eq!(action.tests().browser, "web-console/reliability", "{id}");
            assert_eq!(action.tests().checks, "admind::actions::reliability::tests", "{id}");
            assert_eq!(prepared.ansible.become_root, runs == Runs::AsAdministrator, "{id}");
            assert_eq!(prepared.ansible.changed_when.as_deref(), Some("false"), "{id}");
        }
    }

    #[test]
    fn only_the_alerts_come_a_line_each_and_only_startup_could_answer_after_another_status() {
        for action in REGISTRY.iter().filter(|a| a.id().starts_with("reliability.")) {
            assert_eq!(action.lines(), action.id() == "reliability.alerts", "{}", action.id());
            // startup's status 1 prints text that is not JSON, or an error:
            // ADM-123 with the command's own words.
            assert_eq!(action.answers(), [0], "{}", action.id());
        }
    }

    #[test]
    fn a_start_is_a_number_from_1_which_the_command_is_given_whole() {
        for start in [1, 2, 41, 100, 4_294_967_295_i64] {
            assert_eq!(
                argv("reliability.start", json!({ "start": start })),
                ["startup", "--start", &start.to_string(), "--compare", "--json"]
            );
        }
        let words = "A start's number is a whole number from 1 to 4294967295. Nothing was done.";
        for start in [0, -1, 4_294_967_296_i64, i64::MIN, i64::MAX] {
            assert_eq!(
                refused("reliability.start", json!({ "start": start })),
                (Kind::BadValue, Some("start"), words.into()),
                "{start}"
            );
        }
        for args in [
            json!({}),
            json!({ "start": "1" }),
            json!({ "start": 1.5 }),
            json!({ "start": null }),
            json!({ "start": true }),
            json!({ "start": 1, "compare": 2 }),
            json!({ "start": 1, "json": true }),
        ] {
            assert_eq!(
                action("reliability.start").prepare(args.clone()).unwrap_err().kind,
                Kind::BadRequest,
                "{args}"
            );
        }
    }

    #[test]
    fn a_read_that_takes_nothing_refuses_each_argument() {
        for id in
            ["reliability.starts", "reliability.failed", "reliability.dumps", "reliability.alerts"]
        {
            for args in [
                json!({ "all": true }),
                json!({ "start": 1 }),
                json!({ "n": 20 }),
                json!({ "json": true }),
            ] {
                assert_eq!(
                    action(id).prepare(args.clone()).unwrap_err().kind,
                    Kind::BadRequest,
                    "{id} {args}"
                );
            }
        }
    }

    #[test]
    fn the_failed_start_is_found_before_a_start_by_its_number_in_the_same_place() {
        for (path, id, start) in [
            ("/api/1/starts", "reliability.starts", None),
            ("/api/1/starts/failed", "reliability.failed", None),
            ("/api/1/starts/1", "reliability.start", Some("1")),
            ("/api/1/starts/4294967295", "reliability.start", Some("4294967295")),
            ("/api/1/dumps", "reliability.dumps", None),
            ("/api/1/alerts/history", "reliability.alerts", None),
        ] {
            let found = find(REGISTRY, false, path);
            let Found::Action(action, params) = found else { panic!("{path}: {found:?}") };
            assert_eq!(action.id(), id, "{path}");
            assert_eq!(params.get("start").and_then(Value::as_str), start, "{path}");
        }
        for path in ["/api/1/starts/", "/api/1/starts/1/x", "/api/1/dumps/x", "/api/1/alerts"] {
            assert!(matches!(find(REGISTRY, false, path), Found::None), "{path}");
        }
        for path in ["/api/1/starts", "/api/1/starts/failed", "/api/1/starts/7", "/api/1/dumps"] {
            assert!(matches!(find(REGISTRY, true, path), Found::WrongMethod), "{path}");
        }
        // The number is read as the number it is, from the address, and a
        // word that is not one is no start.
        let args = |path: &str| {
            let Found::Action(action, params) = find(REGISTRY, false, path) else { panic!() };
            super::super::query_arguments(params, "", action.fields())
        };
        assert_eq!(args("/api/1/starts/41").unwrap(), json!({ "start": 41 }));
        assert_eq!(args("/api/1/starts/x").unwrap_err().kind, Kind::BadRequest);
        assert_eq!(args("/api/1/starts/1.5").unwrap_err().kind, Kind::BadRequest);
        // A number below 1 is read as the number it is, and `check` refuses it.
        assert_eq!(args("/api/1/starts/0").unwrap(), json!({ "start": 0 }));
        assert_eq!(
            refused("reliability.start", args("/api/1/starts/-1").unwrap()).1,
            Some("start")
        );
    }

    #[test]
    fn a_session_reads_the_starts_and_an_administrator_the_dumps_and_alerts() {
        use super::super::{Caller, authorize};
        for (id, person, unlocked_admin, locked_admin) in [
            ("reliability.starts", true, true, true),
            ("reliability.failed", true, true, true),
            ("reliability.start", true, true, true),
            ("reliability.dumps", false, true, false),
            ("reliability.alerts", false, true, false),
        ] {
            let action = action(id);
            let session = |admin, unlocked| {
                authorize(&Caller::Session { name: "ann", admin, unlocked }, action)
            };
            assert_eq!(session(false, false).is_ok(), person, "{id}");
            assert_eq!(session(false, true).is_ok(), person, "{id}");
            assert_eq!(session(true, true).is_ok(), unlocked_admin, "{id}");
            assert_eq!(session(true, false).is_ok(), locked_admin, "{id}");
        }
        let refused = |id: &str, admin, unlocked| {
            authorize(&Caller::Session { name: "ann", admin, unlocked }, action(id)).unwrap_err()
        };
        assert_eq!(refused("reliability.dumps", false, true).kind, Kind::NotAdmin);
        assert_eq!(refused("reliability.alerts", true, false).kind, Kind::Locked);
    }

    #[test]
    fn a_token_needs_the_reliability_scope_and_for_a_read_that_is_elevated_the_whole_of_it() {
        use super::super::{Caller, authorize};
        let call = |list: &str, id: &str| {
            let scopes = Scopes::parse(list).unwrap();
            authorize(&Caller::Token { name: "ci", scopes: &scopes }, action(id))
        };
        for id in ["reliability.starts", "reliability.failed", "reliability.start"] {
            for list in ["reliability", "reliability:read", "*"] {
                assert!(call(list, id).is_ok(), "{id} {list}");
            }
            for list in ["services", "console", "console:read"] {
                assert_eq!(call(list, id).unwrap_err().kind, Kind::OutOfScope, "{id} {list}");
            }
        }
        for id in ["reliability.dumps", "reliability.alerts"] {
            for list in ["reliability", "*"] {
                assert!(call(list, id).is_ok(), "{id} {list}");
            }
            let refused = call("reliability:read", id).unwrap_err();
            assert_eq!(refused.kind, Kind::OutOfScope, "{id}");
            assert_eq!(
                refused.detail,
                "the API token \"ci\" may not do this: it needs the scope reliability or *",
                "{id}"
            );
        }
    }

    #[test]
    fn the_export_runs_admin_export_with_the_folder_as_the_person_and_names_no_file() {
        let prepared =
            action("console.export").prepare(json!({ "dir": "/users/ann/nas-settings" })).unwrap();
        assert_eq!(prepared.argv, ["admin", "export", "/users/ann/nas-settings"]);
        assert!(prepared.files.is_empty());
        assert_eq!(prepared.trial, None);
        // A folder's name that looks like an option is still a folder: it
        // starts with /.
        assert_eq!(
            argv("console.export", json!({ "dir": "/users/ann/-x --force" })),
            ["admin", "export", "/users/ann/-x --force"]
        );
        let export = action("console.export");
        assert_eq!(
            (export.changes(), export.runs(), export.area(), export.path(), export.long()),
            (true, Runs::AsThePerson, "console", "/api/1/console/export", false)
        );
        assert_eq!(export.title(), "Copy the settings to Git");
        assert!(export.errors().is_empty());
        assert_eq!(export.answers(), [0]);
        assert!(!export.lines());
        assert!(export.compares());
        assert_eq!(export.tests().checks, "admind::actions::reliability::tests");
        assert_eq!(export.tests().browser, "web-console/reliability");
        assert_eq!(prepared.ansible.name, "Copy the settings to Git: /users/ann/nas-settings");
        assert_eq!(prepared.ansible.module, "ansible.builtin.command");
        assert_eq!(
            prepared.ansible.params["argv"],
            json!(["admin", "export", "/users/ann/nas-settings"])
        );
        assert!(!prepared.ansible.become_root);
        assert_eq!(export.example_values().2, None, "nothing puts an export back");
    }

    #[test]
    fn the_export_s_probe_is_the_newest_commit_of_the_folder_and_nothing_else_is_compared() {
        let prepared = action("console.export").prepare(json!({ "dir": "/users/ann/s" })).unwrap();
        assert_eq!(
            prepared.probe.as_deref(),
            Some(
                &["git", "-C", "/users/ann/s", "rev-parse", "--verify", "-q", "HEAD"]
                    .map(String::from)[..]
            )
        );
        let sha = "3f2a9c1d0b7e6a5581c0e2d4f9a3b7163f2a9c1d";
        let picked = |output: &str| {
            action("console.export").probed(&json!({ "dir": "/users/ann/s" }), output)
        };
        // The commit's name, as git printed it, with its line's end.
        assert_eq!(picked(&format!("{sha}\n")).as_deref(), Some(sha));
        assert_eq!(picked(sha), picked(&format!("{sha}\n")));
        // An export that made a commit moved it.
        let other = "81c0e2d4f9a3b7163f2a9c1d0b7e6a5581c0e2d4";
        assert_ne!(picked(&format!("{sha}\n")), picked(&format!("{other}\n")));
        // Arguments that are not the action's pick nothing.
        assert_eq!(action("console.export").probed(&json!({ "dir": 7 }), sha), None);
    }

    #[test]
    fn the_export_s_folder_is_an_absolute_path_of_at_most_4096_bytes_with_no_nul() {
        for dir in [
            "/",
            "/users/ann/host-settings",
            "/tmp/admin-export-example",
            "/users/ann/with space",
            "/users/ann/caf\u{e9}",
            "/a/../b",
            "/users/ann/new\nline",
            &format!("/{}", "a".repeat(4095)),
        ] {
            assert_eq!(argv("console.export", json!({ "dir": dir }))[2], dir, "{dir:?}");
        }
        let end = "Nothing was done.";
        for (dir, why) in [
            ("relative", "The folder is a path that starts with /"),
            ("./here", "The folder is a path that starts with /"),
            ("~/settings", "The folder is a path that starts with /"),
            (" /users/ann", "The folder is a path that starts with /"),
            ("", "The folder is a path that starts with /"),
            ("/users/ann\u{0}/x", "The folder's path has no NUL in it"),
        ] {
            assert_eq!(
                refused("console.export", json!({ "dir": dir })),
                (Kind::BadValue, Some("dir"), format!("{why}. {end}")),
                "{dir:?}"
            );
        }
        let long = format!("/{}", "a".repeat(4096));
        assert_eq!(
            refused("console.export", json!({ "dir": long })),
            (
                Kind::BadValue,
                Some("dir"),
                format!("The folder's path is at most 4096 bytes long. {end}")
            )
        );
        // Bytes, not characters: 2049 two-byte letters make 4098 bytes.
        let wide = format!("/{}", "\u{e9}".repeat(2049));
        assert_eq!(refused("console.export", json!({ "dir": wide })).1, Some("dir"));
        for args in [
            json!({}),
            json!({ "dir": 7 }),
            json!({ "dir": null }),
            json!({ "dir": "/tmp/x", "message": "x" }),
        ] {
            assert_eq!(
                action("console.export").prepare(args.clone()).unwrap_err().kind,
                Kind::BadRequest,
                "{args}"
            );
        }
    }

    #[test]
    fn the_schemas_say_the_start_s_range_and_the_folder_s_form() {
        let start = schema(action("reliability.start").fields());
        assert_eq!(start["properties"]["start"]["minimum"], 1);
        assert_eq!(start["properties"]["start"]["maximum"], 4_294_967_295_i64);
        assert_eq!(start["required"], json!(["start"]));
        let export = schema(action("console.export").fields());
        assert_eq!(export["properties"]["dir"]["minLength"], 1);
        assert_eq!(export["properties"]["dir"]["maxLength"], 4096);
        assert_eq!(export["properties"]["dir"]["pattern"], PATH_PATTERN);
        assert_eq!(export["required"], json!(["dir"]));
        for id in
            ["reliability.starts", "reliability.failed", "reliability.dumps", "reliability.alerts"]
        {
            assert_eq!(schema(action(id).fields())["properties"], json!({}), "{id}");
        }
    }

    #[test]
    fn the_examples_work_and_the_bad_ones_do_not() {
        for id in [
            "reliability.starts",
            "reliability.failed",
            "reliability.start",
            "reliability.dumps",
            "reliability.alerts",
            "console.export",
        ] {
            let action = action(id);
            let (good, bad, undo) = action.example_values();
            assert!(!bad.is_empty() && undo.is_none(), "{id}");
            for args in &good {
                assert!(action.prepare(args.clone()).is_ok(), "{id}");
            }
            for args in bad {
                assert!(action.prepare(args.clone()).is_err(), "{id} {args}");
            }
        }
        let (good, _, _) = action("reliability.start").example_values();
        assert_eq!(good, [json!({ "start": 1 })]);
        let (good, bad, _) = action("console.export").example_values();
        assert_eq!(good, [json!({ "dir": "/tmp/admin-export-example" })]);
        assert!(bad.contains(&json!({ "dir": "relative" })) && bad.contains(&json!({ "dir": "" })));
    }
}
