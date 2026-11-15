// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The backups' actions (ADR 0463, ADR 0515): the places a machine backs
//! up to, what each holds, a place added or taken away, its recovery key,
//! the machine's key for servers, and the three that take long, a round,
//! a check and a restore. The area is `backups`, the scope a token needs.
//!
//! All but the list run as an administrator: what a destination holds and
//! its recovery key are not a person's to read, and every change is the
//! machine's. The list is the person's own, which `backup list --json`
//! gives from what any account may see (the machine's own settings, state
//! and whether a volume is plugged in), and the whole-machine restore
//! staged for the one who may see it.
//!
//! A destination is named by `backup::may_name`'s rule. A new one is not
//! called `ssh-key`: the address of the machine's key is
//! `/api/1/backups/ssh-key`, which would be where a place of that name is
//! read.
//!
//! The recovery key is a secret (ADR 0561). `backups.key` prints it and
//! `backups.add` prints the new place's, so both say so
//! ([`Action::SECRET_ANSWER`]): their routes are `POST`s, they are never
//! tasks, and nothing of what they print is kept.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::snapshots::{PATH_MAX, PATH_PATTERN, check_path};
use super::{Action, Args, Bad, Examples, Field, Nothing, Runs, Tests, Undo};
use crate::backup::{check_to, is_time, key_file, may_name, settings_file};

/// The area of these actions: the scope a token needs, `backups`.
const AREA: &str = "backups";

/// The tests of these actions, which `lab/actions.toml` names.
const TESTS: Tests = Tests {
    checks: "admind::actions::backups::tests",
    route: "admin-backups",
    same_as_command: "admin-backups",
    browser: "web-console/snapshots",
    ansible: "web-console/ansible",
};

/// The most characters of a destination's name.
const NAME_MAX: usize = 32;

/// What a destination's name matches: letters, digits, `-` and `_`, and
/// not starting with `-`.
const NAME_PATTERN: &str = "^[A-Za-z0-9_][A-Za-z0-9_-]*$";

/// The name no new destination has, which is an address of its own.
const RESERVED: &str = "ssh-key";

/// What the name of a new destination matches: [`NAME_PATTERN`], and not
/// [`RESERVED`].
const NEW_NAME_PATTERN: &str = "^(?!ssh-key$)[A-Za-z0-9_][A-Za-z0-9_-]*$";

/// The most bytes of the place a destination is at.
const TO_MAX: usize = 1024;

/// What a time of a backup matches: `YYYY-MM-DD-HHMMSS`, as `backup show`
/// lists them.
pub(super) const TIME_PATTERN: &str =
    "^[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]-[0-9][0-9][0-9][0-9][0-9][0-9]$";

/// How often a destination is backed up to.
const EVERY: &[&str] = &["day", "week", "hand"];

/// How often a destination is checked.
const VERIFY: &[&str] = &["never", "week", "month"];

/// How a destination keeps its backups.
const FORMAT: &[&str] = &["btrfs", "restic"];

/// A destination's name field, for each action that takes one.
const NAME: Field = Field::text(
    "name",
    "The backup place's name, as backup list shows it.",
    1,
    NAME_MAX,
    Some(NAME_PATTERN),
);

/// Checks `name`, which names a destination there already is.
fn check_name(name: &str) -> Result<(), Bad> {
    if may_name(name) {
        return Ok(());
    }
    Err(Bad {
        field: "name",
        why: format!(
            "A backup place's name is 1 to {NAME_MAX} letters, digits, - and _, and does not \
             start with -. Nothing was done."
        ),
    })
}

/// A destination, by its name.
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Named {
    pub name: String,
}

impl Args for Named {
    const FIELDS: &'static [Field] = &[NAME];

    fn check(&self) -> Result<(), Bad> {
        check_name(&self.name)
    }
}

/// Examples for an action that takes a destination's name.
fn named_examples() -> Examples<Named> {
    Examples {
        good: vec![Named { name: "lab".into() }],
        bad: vec![json!({ "name": "../x" }), json!({ "all": true }), json!({})],
        undo: None,
    }
}

/// The names of the files a destination's settings and key are in, which
/// `backup add` writes and `backup remove` takes away.
fn files_of(name: &str) -> Vec<String> {
    [settings_file(name), key_file(name)].map(|file| file.display().to_string()).to_vec()
}

/// The command that reads what adding or removing a destination moves:
/// the list.
fn list_argv() -> Vec<String> {
    ["backup", "list", "--json"].map(String::from).to_vec()
}

/// What `backup list --json` says of destination `name`, of what its two
/// files make: where it is, and how it is kept and when. What it last did
/// is not that: a round changes it with no change of files. `none` for a
/// list with no such destination.
fn picked(name: &str, output: &str) -> Option<String> {
    let document: Value = serde_json::from_str(output).ok()?;
    let found = document
        .get("places")?
        .as_array()?
        .iter()
        .find(|place| place.get("name").and_then(Value::as_str) == Some(name));
    Some(match found {
        Some(place) => {
            let member = |key: &str| place.get(key).cloned().unwrap_or(Value::Null);
            json!({
                "to": member("to"),
                "format": member("format"),
                "every": member("every"),
                "verify": member("verify"),
            })
            .to_string()
        }
        None => "none".into(),
    })
}

/// `backup list --json` as the person: the destinations, each with what it
/// last did and when one is overdue.
pub struct List;

impl Action for List {
    const ID: &'static str = "backups.list";
    const AREA: &'static str = AREA;
    const CHANGES: bool = false;
    const PATH: &'static str = "/api/1/backups";
    const RUNS: Runs = Runs::AsThePerson;
    const TESTS: Tests = TESTS;
    const TITLE: &'static str = "List the backup places";
    type Args = Nothing;

    fn argv(_: &Nothing) -> Vec<String> {
        list_argv()
    }

    fn examples() -> Examples<Nothing> {
        Examples { good: vec![Nothing {}], bad: vec![json!({ "all": true })], undo: None }
    }
}

/// `backup ssh-key --json`: the machine's public key for servers, which a
/// server is told to accept.
///
/// It is before [`Show`] in the list, since its address is also one of a
/// destination called `ssh-key`, which no new one is.
pub struct SshKey;

impl Action for SshKey {
    const ID: &'static str = "backups.ssh-key";
    const AREA: &'static str = AREA;
    const CHANGES: bool = false;
    const PATH: &'static str = "/api/1/backups/ssh-key";
    const RUNS: Runs = Runs::AsAdministrator;
    const TESTS: Tests = TESTS;
    const TITLE: &'static str = "Show the machine's key for servers";
    type Args = Nothing;

    fn argv(_: &Nothing) -> Vec<String> {
        ["backup", "ssh-key", "--json"].map(String::from).to_vec()
    }

    fn examples() -> Examples<Nothing> {
        Examples { good: vec![Nothing {}], bad: vec![json!({ "all": true })], undo: None }
    }
}

/// `backup show NAME --json`: what a destination holds, by place.
pub struct Show;

impl Action for Show {
    const ID: &'static str = "backups.show";
    const AREA: &'static str = AREA;
    const CHANGES: bool = false;
    const PATH: &'static str = "/api/1/backups/{name}";
    const RUNS: Runs = Runs::AsAdministrator;
    const ERRORS: &'static [&'static str] = &["BAK-101"];
    const TESTS: Tests = TESTS;
    const TITLE: &'static str = "Show what a backup place holds";
    type Args = Named;

    fn argv(args: &Named) -> Vec<String> {
        vec!["backup".into(), "show".into(), args.name.clone(), "--json".into()]
    }

    fn subject(args: &Named) -> Option<String> {
        Some(args.name.clone())
    }

    fn examples() -> Examples<Named> {
        named_examples()
    }
}

/// `backup run --progress NAME`: a round now, as a task.
pub struct Run;

impl Action for Run {
    const ID: &'static str = "backups.run";
    const AREA: &'static str = AREA;
    const CHANGES: bool = true;
    const PATH: &'static str = "/api/1/backups/{name}/run";
    const RUNS: Runs = Runs::AsAdministrator;
    const LONG: bool = true;
    const ERRORS: &'static [&'static str] = &["BAK-101", "BAK-102"];
    const TESTS: Tests = TESTS;
    const TITLE: &'static str = "Back up now";
    type Args = Named;

    fn argv(args: &Named) -> Vec<String> {
        let mut argv: Vec<String> = ["backup", "run", "--progress"].map(String::from).to_vec();
        argv.push(args.name.clone());
        argv
    }

    fn subject(args: &Named) -> Option<String> {
        Some(args.name.clone())
    }

    fn examples() -> Examples<Named> {
        named_examples()
    }
}

/// `backup verify --progress --restore NAME`: the backups read through and
/// the newest of each place restored and compared, as a task.
pub struct Verify;

impl Action for Verify {
    const ID: &'static str = "backups.verify";
    const AREA: &'static str = AREA;
    const CHANGES: bool = true;
    const PATH: &'static str = "/api/1/backups/{name}/verify";
    const RUNS: Runs = Runs::AsAdministrator;
    const LONG: bool = true;
    const ERRORS: &'static [&'static str] = &["BAK-101", "BAK-102"];
    const TESTS: Tests = TESTS;
    const TITLE: &'static str = "Check a backup place";
    type Args = Named;

    fn argv(args: &Named) -> Vec<String> {
        let mut argv: Vec<String> =
            ["backup", "verify", "--progress", "--restore"].map(String::from).to_vec();
        argv.push(args.name.clone());
        argv
    }

    fn subject(args: &Named) -> Option<String> {
        Some(args.name.clone())
    }

    fn examples() -> Examples<Named> {
        named_examples()
    }
}

/// A destination to add, and how it is used.
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Added {
    pub name: String,
    /// Where it is: a folder from `/`, or an `ssh://` address.
    pub to: String,
    pub every: String,
    pub verify: String,
    /// How it keeps backups; the command's own choice without one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub format: Option<String>,
}

impl Args for Added {
    const FIELDS: &'static [Field] = &[
        Field::text(
            "name",
            "What to call it: 1 to 32 letters, digits, - and _, not starting with -, and not \
             ssh-key.",
            1,
            NAME_MAX,
            Some(NEW_NAME_PATTERN),
        ),
        Field::text(
            "to",
            "Where it is: a folder from /, as /volumes/BACKUP, in which the backups go in \
             dukovo-backup, or an ssh:// address.",
            1,
            TO_MAX,
            None,
        ),
        Field::one_of("every", "How often it is backed up to: day, week or by hand.", EVERY),
        Field::one_of("verify", "How often it is checked: never, week or month.", VERIFY),
        Field::one_of(
            "format",
            "How it keeps backups: btrfs, or restic. Btrfs for a folder or a server, without it.",
            FORMAT,
        )
        .optional(),
    ];

    fn check(&self) -> Result<(), Bad> {
        check_name(&self.name)?;
        if self.name == RESERVED {
            return Err(Bad {
                field: "name",
                why: format!(
                    "A new backup place is not called {RESERVED}, which is the address of the \
                     machine's key. Nothing was added."
                ),
            });
        }
        let refused = |why: String| Bad { field: "to", why };
        if self.to.is_empty() || self.to.len() > TO_MAX || self.to.chars().any(char::is_control) {
            return Err(refused(format!(
                "A place is 1 to {TO_MAX} bytes on one line. Nothing was added."
            )));
        }
        check_to(&self.to).map_err(|why| refused(format!("{why}. Nothing was added.")))?;
        for (field, value, words) in
            [("every", &self.every, EVERY), ("verify", &self.verify, VERIFY)]
        {
            if !words.contains(&value.as_str()) {
                return Err(Bad {
                    field,
                    why: format!("{field} is {}. Nothing was added.", or_list(words)),
                });
            }
        }
        if let Some(format) = &self.format
            && !FORMAT.contains(&format.as_str())
        {
            return Err(Bad {
                field: "format",
                why: format!("format is {}. Nothing was added.", or_list(FORMAT)),
            });
        }
        Ok(())
    }
}

/// `words` as "a, b or c".
fn or_list(words: &[&str]) -> String {
    match words.split_last() {
        Some((last, [])) => (*last).to_string(),
        Some((last, rest)) => format!("{} or {last}", rest.join(", ")),
        None => String::new(),
    }
}

/// `backup add [--format F] --every W --verify V NAME TO`: adds a place,
/// makes its recovery key and prints it, once.
pub struct Add;

impl Action for Add {
    const ID: &'static str = "backups.add";
    const AREA: &'static str = AREA;
    const CHANGES: bool = true;
    const PATH: &'static str = "/api/1/backups/add";
    const RUNS: Runs = Runs::AsAdministrator;
    const SECRET_ANSWER: bool = true;
    const TESTS: Tests = TESTS;
    const TITLE: &'static str = "Add a backup place";
    type Args = Added;

    fn argv(args: &Added) -> Vec<String> {
        let mut argv = vec!["backup".to_string(), "add".to_string()];
        if let Some(format) = &args.format {
            argv.extend(["--format".to_string(), format.clone()]);
        }
        argv.extend(["--every".to_string(), args.every.clone()]);
        argv.extend(["--verify".to_string(), args.verify.clone()]);
        argv.extend([args.name.clone(), args.to.clone()]);
        argv
    }

    fn subject(args: &Added) -> Option<String> {
        Some(args.name.clone())
    }

    fn files(args: &Added) -> Vec<String> {
        files_of(&args.name)
    }

    fn probe(_: &Added) -> Option<Vec<String>> {
        Some(list_argv())
    }

    fn probed(args: &Added, output: &str) -> Option<String> {
        picked(&args.name, output)
    }

    fn examples() -> Examples<Added> {
        // A folder on the boot test's second disk.
        let good = Added {
            name: "lab".into(),
            to: "/volumes/vdb".into(),
            every: "hand".into(),
            verify: "never".into(),
            format: None,
        };
        let with = |name: &str, to: &str, every: &str| json!({ "name": name, "to": to, "every": every, "verify": "never" });
        Examples {
            bad: vec![
                with("-x", "/volumes/vdb", "hand"),
                with("ssh-key", "/volumes/vdb", "hand"),
                with("lab", "s3://bucket/x", "hand"),
                with("lab", "relative/folder", "hand"),
                with("lab", "/volumes/vdb", "hourly"),
            ],
            undo: Some(Undo { action: "backups.remove", args: json!({ "name": "lab" }) }),
            good: vec![good],
        }
    }
}

/// `backup remove NAME`: takes a place away; the backups stay where they
/// are, and its recovery key is the only way to read them.
pub struct Remove;

impl Action for Remove {
    const ID: &'static str = "backups.remove";
    const AREA: &'static str = AREA;
    const CHANGES: bool = true;
    const PATH: &'static str = "/api/1/backups/{name}/remove";
    const RUNS: Runs = Runs::AsAdministrator;
    const ERRORS: &'static [&'static str] = &["BAK-101"];
    const TESTS: Tests = TESTS;
    const TITLE: &'static str = "Remove a backup place";
    type Args = Named;

    fn argv(args: &Named) -> Vec<String> {
        vec!["backup".into(), "remove".into(), args.name.clone()]
    }

    fn subject(args: &Named) -> Option<String> {
        Some(args.name.clone())
    }

    fn files(args: &Named) -> Vec<String> {
        files_of(&args.name)
    }

    fn probe(_: &Named) -> Option<Vec<String>> {
        Some(list_argv())
    }

    fn probed(args: &Named, output: &str) -> Option<String> {
        picked(&args.name, output)
    }

    fn examples() -> Examples<Named> {
        named_examples()
    }
}

/// `backup key NAME --json`: a place's recovery key, which is shown to
/// the one who asked and kept nowhere.
pub struct Key;

impl Action for Key {
    const ID: &'static str = "backups.key";
    const AREA: &'static str = AREA;
    const CHANGES: bool = false;
    const PATH: &'static str = "/api/1/backups/{name}/key";
    const RUNS: Runs = Runs::AsAdministrator;
    const SECRET_ANSWER: bool = true;
    const ERRORS: &'static [&'static str] = &["BAK-101"];
    const TESTS: Tests = TESTS;
    const TITLE: &'static str = "Show a backup place's recovery key";
    type Args = Named;

    fn argv(args: &Named) -> Vec<String> {
        vec!["backup".into(), "key".into(), args.name.clone(), "--json".into()]
    }

    fn subject(args: &Named) -> Option<String> {
        Some(args.name.clone())
    }

    fn examples() -> Examples<Named> {
        named_examples()
    }
}

/// A place of a destination to restore, and from when.
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Restored {
    pub name: String,
    /// The place, by its path on this machine: `/users`.
    pub place: String,
    /// The time of the backup, as `backup show` lists it; the newest
    /// without one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<String>,
}

impl Args for Restored {
    const FIELDS: &'static [Field] = &[
        NAME,
        Field::text(
            "place",
            "The place to restore, by its path here: /users.",
            1,
            PATH_MAX,
            Some(PATH_PATTERN),
        ),
        Field::text(
            "at",
            "The time of the backup, as the place lists it: YYYY-MM-DD-HHMMSS. The newest without \
             it.",
            17,
            17,
            Some(TIME_PATTERN),
        )
        .optional(),
    ];

    fn check(&self) -> Result<(), Bad> {
        check_name(&self.name)?;
        check_path("place", "place", &self.place)?;
        if self.at.as_deref().is_some_and(|at| !is_time(at)) {
            return Err(Bad {
                field: "at",
                why: "A time is written as backup show lists it: year, month, day, then hour, \
                      minute and second, as 2026-11-11-030012. Nothing was restored."
                    .into(),
            });
        }
        Ok(())
    }
}

/// `backup restore --progress --place PLACE [--at TIME] NAME`: the newest
/// backup of the place, or that of `TIME`, received as a read-only
/// snapshot beside the place's others, as a task. It writes over nothing:
/// `snapshots.restore` copies files back from it.
pub struct Restore;

impl Action for Restore {
    const ID: &'static str = "backups.restore";
    const AREA: &'static str = AREA;
    const CHANGES: bool = true;
    const PATH: &'static str = "/api/1/backups/{name}/restore";
    const RUNS: Runs = Runs::AsAdministrator;
    const LONG: bool = true;
    const ERRORS: &'static [&'static str] = &["BAK-101", "BAK-102"];
    const TESTS: Tests = TESTS;
    const TITLE: &'static str = "Restore a place from a backup place";
    type Args = Restored;

    fn argv(args: &Restored) -> Vec<String> {
        let mut argv: Vec<String> =
            ["backup", "restore", "--progress", "--place"].map(String::from).to_vec();
        argv.push(args.place.clone());
        if let Some(at) = &args.at {
            argv.extend(["--at".to_string(), at.clone()]);
        }
        argv.push(args.name.clone());
        argv
    }

    fn subject(args: &Restored) -> Option<String> {
        Some(args.name.clone())
    }

    fn examples() -> Examples<Restored> {
        Examples {
            good: vec![Restored { name: "lab".into(), place: "/users".into(), at: None }],
            bad: vec![
                json!({ "name": "lab", "place": "users" }),
                json!({ "name": "lab", "place": "/users", "at": "yesterday" }),
                json!({ "name": "lab", "place": "/users", "at": "2026-11-11-0300" }),
                json!({ "name": "lab" }),
            ],
            undo: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::{Caller, Found, REGISTRY, Registered, authorize, find};
    use super::*;
    use crate::admind::errors::Kind;
    use crate::admind::scopes::Scopes;

    fn action(id: &str) -> &'static dyn Registered {
        *REGISTRY.iter().find(|action| action.id() == id).unwrap()
    }

    /// Why `id` refuses `args`: the kind, the field and the words.
    fn refused(id: &str, args: Value) -> (Kind, Option<&'static str>, String) {
        let failure = action(id).prepare(args).unwrap_err();
        (failure.kind, failure.field, failure.detail)
    }

    /// What `backup list --json` prints, as the console's own commit of it
    /// shapes it, with the destinations the machine has.
    const LIST: &str = r#"{
        "format": 1,
        "places": [
            {"name": "stick", "to": "/volumes/stick", "kind": "folder",
             "format": "btrfs", "every": "day", "verify": "month",
             "keeps": {"chain": 30, "chains": 2, "daily": 7, "weekly": 4, "monthly": 6},
             "places": ["/users", "/shared"], "append_only": false, "recipients": 0,
             "plugged_in": true, "key_ms": 1763100000000,
             "state": {"tried_ms": 1763100100000, "done_ms": 1763100100000, "failed": null,
                       "verified_ms": null, "verify_failed": null},
             "overdue_days": 0, "running": false},
            {"name": "nas", "to": "ssh://backup@nas.example/srv/backups", "kind": "ssh",
             "format": "restic", "every": "week", "verify": "never",
             "state": {"tried_ms": 1, "done_ms": 1, "failed": "no route", "verified_ms": null,
                       "verify_failed": null},
             "overdue_days": 3, "running": true}
        ],
        "staged": null
    }"#;

    #[test]
    fn the_reads_are_the_commands_json_and_only_the_list_is_the_persons() {
        for (id, runs, path, argv) in [
            ("backups.list", Runs::AsThePerson, "/api/1/backups", vec!["backup", "list", "--json"]),
            (
                "backups.ssh-key",
                Runs::AsAdministrator,
                "/api/1/backups/ssh-key",
                vec!["backup", "ssh-key", "--json"],
            ),
        ] {
            let read = action(id);
            assert_eq!(
                (read.changes(), read.runs(), read.path(), read.long()),
                (false, runs, path, false)
            );
            assert!(!read.secret_answer() && !read.posts() && !read.compares(), "{id}");
            assert_eq!(read.prepare(json!({})).unwrap().argv, argv, "{id}");
            assert_eq!(
                read.prepare(json!({ "all": true })).unwrap_err().kind,
                Kind::BadRequest,
                "{id}"
            );
        }
        let show = action("backups.show");
        assert_eq!(
            (show.changes(), show.runs(), show.path(), show.errors()),
            (false, Runs::AsAdministrator, "/api/1/backups/{name}", ["BAK-101"].as_slice())
        );
        assert_eq!(
            show.prepare(json!({ "name": "stick" })).unwrap().argv,
            ["backup", "show", "stick", "--json"]
        );
        assert!(show.prepare(json!({ "name": "stick" })).unwrap().ansible.become_root);
    }

    #[test]
    fn a_name_is_the_rule_of_backup_and_nothing_else_is_taken() {
        for name in ["../x", "-x", "", "a b", "a/b", "a.b", "\u{e9}", &"a".repeat(33)] {
            let (kind, field, words) = refused("backups.show", json!({ "name": name }));
            assert_eq!((kind, field), (Kind::BadValue, Some("name")), "{name:?}");
            assert_eq!(
                words,
                "A backup place's name is 1 to 32 letters, digits, - and _, and does not start \
                 with -. Nothing was done.",
                "{name:?}"
            );
        }
        for name in ["lab", "a", "_x", "my-stick_2", "A9", &"a".repeat(32)] {
            assert!(action("backups.show").prepare(json!({ "name": name })).is_ok(), "{name:?}");
        }
        assert_eq!(refused("backups.show", json!({})).0, Kind::BadRequest);
        assert_eq!(
            refused("backups.show", json!({ "name": "lab", "all": true })).0,
            Kind::BadRequest
        );
        // Each of those that takes a name takes it the same way.
        for id in ["backups.run", "backups.verify", "backups.remove", "backups.key"] {
            assert_eq!(refused(id, json!({ "name": "../x" })).1, Some("name"), "{id}");
        }
    }

    #[test]
    fn a_round_a_check_and_a_restore_are_tasks_of_an_administrator() {
        for (id, path, argv) in [
            (
                "backups.run",
                "/api/1/backups/{name}/run",
                vec!["backup", "run", "--progress", "lab"],
            ),
            (
                "backups.verify",
                "/api/1/backups/{name}/verify",
                vec!["backup", "verify", "--progress", "--restore", "lab"],
            ),
        ] {
            let long = action(id);
            assert_eq!(
                (long.changes(), long.runs(), long.long(), long.path(), long.secret_answer()),
                (true, Runs::AsAdministrator, true, path, false),
                "{id}"
            );
            assert_eq!(long.errors(), ["BAK-101", "BAK-102"], "{id}");
            let prepared = long.prepare(json!({ "name": "lab" })).unwrap();
            assert_eq!(prepared.argv, argv, "{id}");
            // Nothing of the settings is written, and nothing is read to
            // compare: a round always did something.
            assert!(
                prepared.files.is_empty() && prepared.probe.is_none() && !long.compares(),
                "{id}"
            );
            assert!(prepared.ansible.become_root, "{id}");
        }
        let restore = action("backups.restore");
        assert_eq!(
            (restore.changes(), restore.runs(), restore.long(), restore.path()),
            (true, Runs::AsAdministrator, true, "/api/1/backups/{name}/restore")
        );
        assert_eq!(restore.errors(), ["BAK-101", "BAK-102"]);
        let newest = restore.prepare(json!({ "name": "lab", "place": "/users" })).unwrap();
        assert_eq!(newest.argv, ["backup", "restore", "--progress", "--place", "/users", "lab"]);
        let then = restore
            .prepare(json!({ "name": "lab", "place": "/users", "at": "2026-11-11-030012" }))
            .unwrap();
        assert_eq!(
            then.argv,
            [
                "backup",
                "restore",
                "--progress",
                "--place",
                "/users",
                "--at",
                "2026-11-11-030012",
                "lab"
            ]
        );
        assert!(newest.files.is_empty() && newest.probe.is_none());
    }

    #[test]
    fn a_restore_names_a_place_by_its_path_and_a_backup_by_its_time() {
        let with = |place: &str, at: Option<&str>| match at {
            Some(at) => json!({ "name": "lab", "place": place, "at": at }),
            None => json!({ "name": "lab", "place": place }),
        };
        for place in ["users", "/users/../x", "", "/a\0b"] {
            assert_eq!(refused("backups.restore", with(place, None)).1, Some("place"), "{place:?}");
        }
        let (kind, field, words) = refused("backups.restore", with("users", None));
        assert_eq!((kind, field), (Kind::BadValue, Some("place")));
        assert_eq!(
            words,
            "The place is a path that starts with /, is at most 4096 bytes long, and has no . or \
             .. part. Nothing was done."
        );
        for at in [
            "yesterday",
            "2026-11-11-0300",
            "2026-11-11-0300121",
            "2026-11-11 030012",
            "",
            "2026-11-11-03001a",
        ] {
            let (kind, field, _) = refused("backups.restore", with("/users", Some(at)));
            assert_eq!((kind, field), (Kind::BadValue, Some("at")), "{at:?}");
        }
        assert!(
            action("backups.restore").prepare(with("/users", Some("2026-11-11-030012"))).is_ok()
        );
        assert_eq!(refused("backups.restore", json!({ "name": "lab" })).0, Kind::BadRequest);
    }

    #[test]
    fn a_place_is_added_with_its_two_files_and_the_command_prints_its_key() {
        let add = action("backups.add");
        assert_eq!(
            (add.changes(), add.runs(), add.long(), add.secret_answer(), add.path()),
            (true, Runs::AsAdministrator, false, true, "/api/1/backups/add")
        );
        // A change is a POST, and so is one that prints a secret.
        assert!(add.posts() && add.compares());
        assert!(add.errors().is_empty());
        let args =
            json!({ "name": "lab", "to": "/volumes/vdb", "every": "hand", "verify": "never" });
        let prepared = add.prepare(args).unwrap();
        assert_eq!(
            prepared.argv,
            ["backup", "add", "--every", "hand", "--verify", "never", "lab", "/volumes/vdb"]
        );
        assert_eq!(
            prepared.files,
            ["/system/settings/backup/lab.toml", "/system/settings/backup/lab.key"]
        );
        assert_eq!(
            prepared.probe.as_deref(),
            Some(&["backup", "list", "--json"].map(String::from)[..])
        );
        // The playbook keeps the key and shows nothing of the task.
        assert!(prepared.ansible.no_log && prepared.ansible.become_root);
        assert_eq!(prepared.ansible.register, Some("answer"));
        let restic = add
            .prepare(json!({
                "name": "nas",
                "to": "ssh://backup@nas.example/srv/backups",
                "every": "day",
                "verify": "month",
                "format": "restic"
            }))
            .unwrap();
        assert_eq!(
            restic.argv,
            [
                "backup",
                "add",
                "--format",
                "restic",
                "--every",
                "day",
                "--verify",
                "month",
                "nas",
                "ssh://backup@nas.example/srv/backups"
            ]
        );
        let (_, _, undo) = add.example_values();
        assert_eq!(undo, Some(Undo { action: "backups.remove", args: json!({ "name": "lab" }) }));
    }

    #[test]
    fn an_s3_place_is_added_at_a_terminal_for_now() {
        let with =
            |to: &str| json!({ "name": "lab", "to": to, "every": "hand", "verify": "never" });
        let words = "S3 places are added at a terminal for now: backup add reads its keys from the \
                     environment. Nothing was added.";
        for to in [
            "s3://bucket/x",
            "s3://s3.example.org/bucket/backups",
            "s3+http://nas.lan:9000/bucket",
            "s3://",
            "s3://no-bucket",
        ] {
            let (kind, field, why) = refused("backups.add", with(to));
            assert_eq!((kind, field, why.as_str()), (Kind::BadValue, Some("to"), words), "{to}");
        }
    }

    #[test]
    fn a_place_is_a_folder_or_an_ssh_address_on_one_line() {
        let add = |to: &str| {
            action("backups.add")
                .prepare(json!({ "name": "lab", "to": to, "every": "hand", "verify": "never" }))
        };
        for to in
            ["/volumes/vdb", "/", "ssh://u@host/path", "ssh://u@host:2222/~/b", "ssh://u@[::1]/b"]
        {
            assert!(add(to).is_ok(), "{to}");
        }
        for to in [
            "relative/folder",
            "",
            "ftp://x/y",
            "ssh://host/path",
            "ssh://u@host",
            "/a\nb",
            "/a\0b",
            &format!("/{}", "a".repeat(1024)),
        ] {
            let failure = add(to).unwrap_err();
            assert_eq!((failure.kind, failure.field), (Kind::BadValue, Some("to")), "{to:?}");
        }
        let (_, _, words) = refused(
            "backups.add",
            json!({ "name": "lab", "to": "relative/folder", "every": "hand", "verify": "never" }),
        );
        assert_eq!(
            words,
            "'relative/folder' is a folder from /, or an ssh:// address. Nothing was added."
        );
        assert!(add(&format!("/{}", "a".repeat(1023))).is_ok());
    }

    #[test]
    fn how_often_and_how_it_is_kept_are_words_of_the_command() {
        let with = |every: &str, verify: &str, format: Option<&str>| {
            let mut args = json!({ "name": "lab", "to": "/x", "every": every, "verify": verify });
            if let Some(format) = format {
                args["format"] = json!(format);
            }
            args
        };
        for (args, field, words) in [
            (with("hourly", "never", None), "every", "every is day, week or hand."),
            (with("day", "daily", None), "verify", "verify is never, week or month."),
            (with("day", "never", Some("zfs")), "format", "format is btrfs or restic."),
            (with("Day", "never", None), "every", "every is day, week or hand."),
        ] {
            let (kind, got, why) = refused("backups.add", args);
            assert_eq!((kind, got), (Kind::BadValue, Some(field)));
            assert_eq!(why, format!("{words} Nothing was added."));
        }
        for every in ["day", "week", "hand"] {
            for verify in ["never", "week", "month"] {
                assert!(action("backups.add").prepare(with(every, verify, None)).is_ok());
            }
        }
        for format in ["btrfs", "restic"] {
            assert!(action("backups.add").prepare(with("day", "week", Some(format))).is_ok());
        }
        // Both are required: the command has no default a person did not see.
        for left_out in ["every", "verify", "to", "name"] {
            let mut args = with("day", "week", None);
            args.as_object_mut().unwrap().remove(left_out);
            assert_eq!(refused("backups.add", args).0, Kind::BadRequest, "{left_out}");
        }
        assert_eq!(or_list(&["a"]), "a");
        assert_eq!(or_list(&["a", "b"]), "a or b");
        assert_eq!(or_list(&["a", "b", "c"]), "a, b or c");
    }

    #[test]
    fn a_new_place_is_not_called_ssh_key_and_the_others_may_be() {
        let add = |name: &str| {
            action("backups.add")
                .prepare(json!({ "name": name, "to": "/x", "every": "hand", "verify": "never" }))
        };
        let failure = add("ssh-key").unwrap_err();
        assert_eq!((failure.kind, failure.field), (Kind::BadValue, Some("name")));
        assert_eq!(
            failure.detail,
            "A new backup place is not called ssh-key, which is the address of the machine's key. \
             Nothing was added."
        );
        for name in ["ssh-keys", "ssh_key", "my-ssh-key", "Ssh-key"] {
            assert!(add(name).is_ok(), "{name}");
        }
        // A place of that name made at a terminal is still reached by the
        // actions whose address has more after the name.
        for (path, id) in [
            ("/api/1/backups/ssh-key/run", "backups.run"),
            ("/api/1/backups/ssh-key/remove", "backups.remove"),
        ] {
            assert!(
                matches!(find(REGISTRY, true, path), Found::Action(found, _) if found.id() == id)
            );
        }
    }

    #[test]
    fn removing_a_place_names_the_same_two_files_and_compares_the_list() {
        let remove = action("backups.remove");
        assert_eq!(
            (
                remove.changes(),
                remove.runs(),
                remove.long(),
                remove.secret_answer(),
                remove.compares()
            ),
            (true, Runs::AsAdministrator, false, false, true)
        );
        assert_eq!(remove.errors(), ["BAK-101"]);
        let prepared = remove.prepare(json!({ "name": "lab" })).unwrap();
        assert_eq!(prepared.argv, ["backup", "remove", "lab"]);
        assert_eq!(
            prepared.files,
            ["/system/settings/backup/lab.toml", "/system/settings/backup/lab.key"]
        );
        assert_eq!(
            prepared.probe.as_deref(),
            Some(&["backup", "list", "--json"].map(String::from)[..])
        );
        // Its key is gone with it: nothing puts it back.
        assert_eq!(remove.example_values().2, None);
    }

    #[test]
    fn the_probe_picks_how_the_place_is_set_and_not_what_it_last_did() {
        let add = action("backups.add");
        let args =
            json!({ "name": "stick", "to": "/volumes/stick", "every": "day", "verify": "month" });
        let picked = add.probed(&args, LIST).unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(&picked).unwrap(),
            json!({ "to": "/volumes/stick", "format": "btrfs", "every": "day", "verify": "month" })
        );
        // A round that ran since changes the state and not what is picked.
        let later = LIST.replace("\"done_ms\": 1763100100000", "\"done_ms\": 1763200100000");
        assert_ne!(later, LIST);
        assert_eq!(add.probed(&args, &later), Some(picked.clone()));
        // The other place is another's, and one the list lacks is none.
        let nas = json!({ "name": "nas", "to": "/x", "every": "day", "verify": "never" });
        assert!(add.probed(&nas, LIST).unwrap().contains("\"restic\""));
        let lab = json!({ "name": "lab", "to": "/x", "every": "day", "verify": "never" });
        assert_eq!(add.probed(&lab, LIST), Some("none".into()));
        assert_eq!(
            action("backups.remove").probed(&json!({ "name": "lab" }), LIST),
            Some("none".into())
        );
        assert_eq!(
            action("backups.remove").probed(&json!({ "name": "stick" }), LIST),
            Some(picked)
        );
        // What does not read says nothing.
        for output in ["", "not json", "[]", r#"{"places": 3}"#, r#"{"format": 1}"#] {
            assert_eq!(add.probed(&lab, output), None, "{output:?}");
        }
        assert_eq!(add.probed(&json!({ "name": 3 }), LIST), None);
    }

    #[test]
    fn the_key_is_a_secret_answer_a_post_and_never_kept() {
        let key = action("backups.key");
        assert_eq!(
            (key.changes(), key.runs(), key.long(), key.secret_answer(), key.posts()),
            (false, Runs::AsAdministrator, false, true, true)
        );
        assert_eq!(key.path(), "/api/1/backups/{name}/key");
        assert_eq!(key.errors(), ["BAK-101"]);
        let prepared = key.prepare(json!({ "name": "lab" })).unwrap();
        assert_eq!(prepared.argv, ["backup", "key", "lab", "--json"]);
        assert!(prepared.files.is_empty() && prepared.probe.is_none() && prepared.input.is_none());
        assert!(prepared.ansible.no_log && prepared.ansible.become_root);
        assert_eq!(prepared.ansible.register, Some("answer"));
        // Neither a read of the address nor a change to it is the same.
        assert!(
            matches!(find(REGISTRY, true, "/api/1/backups/lab/key"), Found::Action(found, _) if found.id() == "backups.key")
        );
        assert!(matches!(find(REGISTRY, false, "/api/1/backups/lab/key"), Found::WrongMethod));
    }

    #[test]
    fn the_routes_are_found_by_their_methods_and_the_key_of_the_machine_comes_first() {
        for (post, path, id) in [
            (false, "/api/1/backups", "backups.list"),
            (false, "/api/1/backups/ssh-key", "backups.ssh-key"),
            (false, "/api/1/backups/lab", "backups.show"),
            (false, "/api/1/backups/my-stick_2", "backups.show"),
            (true, "/api/1/backups/add", "backups.add"),
            (true, "/api/1/backups/lab/run", "backups.run"),
            (true, "/api/1/backups/lab/verify", "backups.verify"),
            (true, "/api/1/backups/lab/remove", "backups.remove"),
            (true, "/api/1/backups/lab/key", "backups.key"),
            (true, "/api/1/backups/lab/restore", "backups.restore"),
        ] {
            let found = find(REGISTRY, post, path);
            assert!(
                matches!(&found, Found::Action(action, _) if action.id() == id),
                "{path}: {found:?}"
            );
        }
        let Found::Action(_, params) = find(REGISTRY, false, "/api/1/backups/lab") else {
            panic!()
        };
        assert_eq!(Value::Object(params), json!({ "name": "lab" }));
        // The address of an action with the other method is told apart.
        assert!(matches!(find(REGISTRY, true, "/api/1/backups"), Found::WrongMethod));
        assert!(
            matches!(find(REGISTRY, false, "/api/1/backups/add"), Found::Action(a, _) if a.id() == "backups.show")
        );
        assert!(matches!(find(REGISTRY, false, "/api/1/backups/lab/run"), Found::WrongMethod));
    }

    #[test]
    fn a_token_needs_the_area_and_a_read_that_runs_as_an_administrator_needs_all_of_it() {
        let call = |list: &str, id: &str| {
            let scopes = Scopes::parse(list).unwrap();
            authorize(&Caller::Token { name: "ci", scopes: &scopes }, action(id))
        };
        assert!(call("backups:read", "backups.list").is_ok());
        for id in ["backups.show", "backups.ssh-key", "backups.key"] {
            assert_eq!(call("backups:read", id).unwrap_err().kind, Kind::OutOfScope, "{id}");
            assert!(call("backups", id).is_ok(), "{id}");
        }
        for id in
            ["backups.add", "backups.remove", "backups.run", "backups.verify", "backups.restore"]
        {
            assert_eq!(call("backups:read", id).unwrap_err().kind, Kind::OutOfScope, "{id}");
            assert_eq!(call("snapshots", id).unwrap_err().kind, Kind::OutOfScope, "{id}");
            assert!(call("backups", id).is_ok(), "{id}");
            assert!(call("*", id).is_ok(), "{id}");
        }
    }

    #[test]
    fn an_administrator_runs_what_runs_as_one_and_a_person_the_list() {
        let session = |admin: bool, unlocked: bool, id: &str| {
            authorize(&Caller::Session { name: "ann", admin, unlocked }, action(id))
                .map_err(|f| f.kind)
        };
        assert_eq!(session(false, false, "backups.list"), Ok(()));
        for id in ["backups.show", "backups.key", "backups.add", "backups.run"] {
            assert_eq!(session(false, true, id), Err(Kind::NotAdmin), "{id}");
            assert_eq!(session(true, false, id), Err(Kind::Locked), "{id}");
            assert_eq!(session(true, true, id), Ok(()), "{id}");
        }
    }

    #[test]
    fn the_examples_are_what_the_boot_and_lab_tests_run() {
        let (good, bad, undo) = action("backups.add").example_values();
        assert_eq!(
            good,
            [json!({ "name": "lab", "to": "/volumes/vdb", "every": "hand", "verify": "never" })]
        );
        assert_eq!(undo, Some(Undo { action: "backups.remove", args: json!({ "name": "lab" }) }));
        assert_eq!(bad.len(), 5);
        for args in bad {
            assert!(action("backups.add").prepare(args.clone()).is_err(), "{args}");
        }
        for id in ["backups.show", "backups.run", "backups.verify", "backups.remove", "backups.key"]
        {
            let (good, bad, undo) = action(id).example_values();
            assert_eq!((good, undo), (vec![json!({ "name": "lab" })], None), "{id}");
            for args in bad {
                assert!(action(id).prepare(args.clone()).is_err(), "{id}: {args}");
            }
        }
        let (good, bad, undo) = action("backups.restore").example_values();
        assert_eq!(good, [json!({ "name": "lab", "place": "/users" })]);
        assert_eq!(undo, None);
        for args in bad {
            assert!(action("backups.restore").prepare(args.clone()).is_err(), "{args}");
        }
    }
}
