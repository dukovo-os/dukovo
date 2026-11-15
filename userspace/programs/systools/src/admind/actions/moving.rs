// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The Move to new hardware page's actions (ADR 0464, ADR 0561): the
//! backups of other machines on the disks plugged in, what a restore of
//! one would bring across and what differs from this machine, staging it
//! for the next start, and withdrawing what was staged. The area is `move`
//! (a keyword, so the file is `moving`), the scope a token needs.
//!
//! All four are an administrator's: they read every machine's backups and
//! write the places the next start puts in place. `move.find` is `backup
//! find --json`. `move.plan` is `backup restore --machine --plan --json`,
//! which receives nothing and changes nothing; it takes the recovery key,
//! so it is a read with a secret and a `POST` (ADR 0561). `move.stage` is
//! `backup restore --machine --progress`, a task that receives each place
//! into `@snapshots/machine-restore/staged`, which the settings do not
//! list and no probe reads; its undo is `move.cancel`.
//!
//! The recovery key is given as a file (`--key FILE`) or typed. A typed
//! key is a secret field: it is never a word of the command, which reads
//! it from its standard input (`--key -`), and the arguments a task keeps
//! show it as `(secret)`. Exactly one of the two is given.
//!
//! The cards are lines of `OLD=NEW` or `OLD=automatic`, one `--card` each.
//! Each is read by `backup`'s own reader ([`Asked::parse`]), so that the
//! action and the command never disagree about one.

use serde::{Deserialize, Serialize};
use serde_json::json;

use super::backups::TIME_PATTERN;
use super::snapshots::{PATH_MAX, PATH_PATTERN, check_path};
use super::trial::refusal;
use super::{Action, Args, Bad, Examples, Field, Nothing, Runs, Secret, Tests, Undo};
use crate::backup::cards::Asked;
use crate::backup::{is_time, may_name};

/// The area of these actions: the scope a token needs, `move`.
const AREA: &str = "move";

/// The tests of these actions, which `lab/actions.toml` names.
const TESTS: Tests = Tests {
    checks: "admind::actions::moving::tests",
    route: "admin-move",
    same_as_command: "admin-move",
    browser: "web-console/move",
    ansible: "web-console/ansible",
};

/// What a source matches: a backup place's name, or a path from `/` with
/// no `.` or `..` part. How long a name is, is the check's alone.
const SOURCE_PATTERN: &str = "^(?![^]*/\\.\\.?(/|$))(/[^\u{0}]*|[A-Za-z0-9_][A-Za-z0-9_-]*)$";

/// The most characters of a typed recovery key: an age key is 74, and a
/// line with a few spaces around it is still one.
const KEY_MAX: usize = 256;

/// The most cards a move gives a choice for: more than any machine has.
const CARDS_MOST: usize = 8;

/// The most characters of the lines of `cards`: each `OLD=NEW` is a name
/// or a MAC address, `=`, and a MAC address or `automatic`, with room for
/// spaces around them, and a line end between.
const CARDS_MAX: usize = CARDS_MOST * 65;

/// What the lines of `cards` match: each has one `=` with something on
/// each side. That each side is a card, and how many lines there are, are
/// the check's alone.
const CARDS_PATTERN: &str = "^[^\n=]+=[^\n=]+(\n[^\n=]+=[^\n=]+)*$";

/// Checks the `source` of a restore: the name of a backup place, or the
/// whole path of a folder.
fn check_source(source: &str) -> Result<(), Bad> {
    if source.starts_with('/') {
        return check_path("source", "source folder", source);
    }
    if may_name(source) {
        return Ok(());
    }
    let why = "The source is the name of a backup place, or the whole path of a folder that holds \
               a machine's backups";
    Err(Bad { field: "source", why: refusal(why) })
}

/// Checks the lines of `cards`: at most [`CARDS_MOST`], each a card choice
/// `backup restore --card` takes.
fn check_cards(cards: &str) -> Result<(), Bad> {
    let bad = |why: String| Err(Bad { field: "cards", why: refusal(&why) });
    let lines: Vec<&str> = cards.split('\n').collect();
    if lines.len() > CARDS_MOST || cards.len() > CARDS_MAX {
        return bad(format!("There are at most {CARDS_MOST} card choices, a line each"));
    }
    for (number, line) in (1..).zip(lines) {
        if Asked::parse(line).is_err() {
            return bad(format!(
                "Line {number} is not OLD=NEW or OLD=automatic, OLD a card of the backup by its \
                 MAC address or name and NEW a card of this machine by its MAC address"
            ));
        }
    }
    Ok(())
}

/// The backup to move from, and how it is opened.
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Moving {
    /// A backup place's name, or the folder that holds a machine's backups.
    pub source: String,
    /// The file that holds the recovery key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key_file: Option<String>,
    /// The recovery key, typed: read from the command's standard input.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<Secret>,
    /// The backup's time; the newest without it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<String>,
    /// Where each old network card goes: a line of `OLD=NEW` or
    /// `OLD=automatic` for each.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cards: Option<String>,
}

impl Moving {
    /// The command, whose own words after `restore --machine` are `mode`:
    /// then the key, the time, a `--card` for each line, and the source.
    fn argv(&self, mode: &[&str]) -> Vec<String> {
        let mut argv: Vec<String> = ["backup", "restore", "--machine"].map(String::from).to_vec();
        argv.extend(mode.iter().map(|word| (*word).to_string()));
        argv.extend(["--key".to_string(), self.key_file.clone().unwrap_or_else(|| "-".into())]);
        if let Some(at) = &self.at {
            argv.extend(["--at".to_string(), at.clone()]);
        }
        for line in self.cards.iter().flat_map(|cards| cards.split('\n')) {
            argv.extend(["--card".to_string(), line.to_string()]);
        }
        argv.push(self.source.clone());
        argv
    }

    /// What the command reads on its input: the typed key's line.
    fn input(&self) -> Option<Secret> {
        self.key.as_ref().map(|key| Secret::lines(&[key]))
    }
}

impl Args for Moving {
    const FIELDS: &'static [Field] = &[
        Field::text(
            "source",
            "The backups to move from: a backup place's name, or the whole path of the folder \
             that holds a machine's backups.",
            1,
            PATH_MAX,
            Some(SOURCE_PATTERN),
        ),
        Field::text(
            "key_file",
            "The whole path of the file that holds the recovery key. Give this or key.",
            1,
            PATH_MAX,
            Some(PATH_PATTERN),
        )
        .optional(),
        Field::secret("key", "The recovery key, typed. Give this or key_file.", KEY_MAX).optional(),
        Field::text(
            "at",
            "The time of the backup, as backup show lists it: 2026-11-14-013000. The newest \
             without it.",
            17,
            17,
            Some(TIME_PATTERN),
        )
        .optional(),
        Field::text(
            "cards",
            "Where each old network card goes, a line each: OLD=NEW, OLD a card of the backup by \
             its MAC address or name and NEW a card of this machine by its MAC address, or \
             OLD=automatic to have it ask for an address.",
            3,
            CARDS_MAX,
            Some(CARDS_PATTERN),
        )
        .optional(),
    ];

    fn check(&self) -> Result<(), Bad> {
        check_source(&self.source)?;
        match (&self.key_file, &self.key) {
            (Some(_), Some(_)) => {
                let why = "Give the key as a file or typed, not both";
                return Err(Bad { field: "key", why: refusal(why) });
            }
            (None, None) => {
                let why = "Give the key as a file or typed: the recovery key opens the backups";
                return Err(Bad { field: "key", why: refusal(why) });
            }
            (Some(file), None) => check_path("key_file", "key file", file)?,
            (None, Some(_)) => {}
        }
        if self.at.as_deref().is_some_and(|at| !is_time(at)) {
            let why = "A backup's time is written YYYY-MM-DD-HHMMSS, as backup show lists it";
            return Err(Bad { field: "at", why: refusal(why) });
        }
        self.cards.as_deref().map_or(Ok(()), check_cards)
    }
}

/// What the plan and the stage are tried with: the backups of a machine in
/// a folder, and the key file of the place it was backed up to.
fn moving_examples() -> Examples<Moving> {
    let folder = "/volumes/LAB/dukovo-backup/lab-machine";
    let key_file = "/system/settings/backup/lab.key";
    let given = |extra: serde_json::Value| {
        let mut args = json!({ "source": folder, "key_file": key_file });
        for (name, value) in extra.as_object().into_iter().flatten() {
            args[name] = value.clone();
        }
        args
    };
    let too_many = ["a=automatic"; CARDS_MOST + 1].join("\n");
    Examples {
        good: vec![Moving {
            source: folder.into(),
            key_file: Some(key_file.into()),
            key: None,
            at: None,
            cards: None,
        }],
        bad: vec![
            json!({ "source": "x y", "key_file": key_file }),
            json!({ "source": "relative/folder", "key_file": key_file }),
            json!({ "source": "/volumes/../etc", "key_file": key_file }),
            json!({ "source": folder }),
            given(json!({ "key": "AGE-SECRET-KEY-1TEST" })),
            json!({ "source": folder, "key": "" }),
            given(json!({ "key_file": "relative.key" })),
            given(json!({ "at": "2026-11-14" })),
            given(json!({ "cards": "nonsense" })),
            given(json!({ "cards": "52:54:00:aa:00:01=net0" })),
            given(json!({ "cards": too_many })),
            given(json!({ "now": true })),
        ],
        undo: None,
    }
}

/// `backup find --json`: the machines' backups and the recovery keys on
/// the disks plugged in.
pub struct Find;

impl Action for Find {
    const ID: &'static str = "move.find";
    const AREA: &'static str = AREA;
    const CHANGES: bool = false;
    const PATH: &'static str = "/api/1/move/sources";
    const RUNS: Runs = Runs::AsAdministrator;
    const TESTS: Tests = TESTS;
    const TITLE: &'static str = "Find the backups of other machines";
    type Args = Nothing;

    fn argv(_: &Nothing) -> Vec<String> {
        ["backup", "find", "--json"].map(String::from).to_vec()
    }

    fn examples() -> Examples<Nothing> {
        Examples { good: vec![Nothing {}], bad: vec![json!({ "all": true })], undo: None }
    }
}

/// `backup restore --machine --plan --json`: what a restore of a whole
/// machine would bring across, and how this machine differs, with nothing
/// received.
pub struct Plan;

impl Action for Plan {
    const ID: &'static str = "move.plan";
    const AREA: &'static str = AREA;
    const CHANGES: bool = false;
    const PATH: &'static str = "/api/1/move/plan";
    const RUNS: Runs = Runs::AsAdministrator;
    const ERRORS: &'static [&'static str] = &["BAK-103", "BAK-107"];
    const TESTS: Tests = TESTS;
    const TITLE: &'static str = "Check what a move brings across";
    type Args = Moving;

    fn argv(args: &Moving) -> Vec<String> {
        args.argv(&["--plan", "--json"])
    }

    fn input(args: &Moving) -> Option<Secret> {
        args.input()
    }

    fn subject(args: &Moving) -> Option<String> {
        Some(args.source.clone())
    }

    fn examples() -> Examples<Moving> {
        moving_examples()
    }
}

/// `backup restore --machine --progress`: stages each place of the
/// machine for the next start, as a task.
pub struct Stage;

impl Action for Stage {
    const ID: &'static str = "move.stage";
    const AREA: &'static str = AREA;
    const CHANGES: bool = true;
    const PATH: &'static str = "/api/1/move/stage";
    const RUNS: Runs = Runs::AsAdministrator;
    const LONG: bool = true;
    const ERRORS: &'static [&'static str] = &["BAK-103", "BAK-104", "BAK-107"];
    const TESTS: Tests = TESTS;
    const TITLE: &'static str = "Stage a move for the next start";
    type Args = Moving;

    fn argv(args: &Moving) -> Vec<String> {
        args.argv(&["--progress"])
    }

    fn input(args: &Moving) -> Option<Secret> {
        args.input()
    }

    fn subject(args: &Moving) -> Option<String> {
        Some(args.source.clone())
    }

    /// What it writes is `@snapshots/machine-restore/staged`, which is no
    /// settings file, and no read says what it moved: it always did
    /// something, and `move.cancel` takes it away.
    fn examples() -> Examples<Moving> {
        let examples = moving_examples();
        Examples { undo: Some(Undo { action: "move.cancel", args: json!({}) }), ..examples }
    }
}

/// `backup restore --machine --cancel`: takes what was staged away.
pub struct Cancel;

impl Action for Cancel {
    const ID: &'static str = "move.cancel";
    const AREA: &'static str = AREA;
    const CHANGES: bool = true;
    const PATH: &'static str = "/api/1/move/cancel";
    const RUNS: Runs = Runs::AsAdministrator;
    const TESTS: Tests = TESTS;
    const TITLE: &'static str = "Cancel a staged move";
    type Args = Nothing;

    fn argv(_: &Nothing) -> Vec<String> {
        ["backup", "restore", "--machine", "--cancel"].map(String::from).to_vec()
    }

    fn examples() -> Examples<Nothing> {
        Examples { good: vec![Nothing {}], bad: vec![json!({ "now": true })], undo: None }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::Value;

    use super::super::{
        Caller, Found, Params, REGISTRY, Registered, authorize, find, listed, query_arguments,
        schema,
    };
    use super::*;
    use crate::admind::errors::Kind;
    use crate::admind::scopes::Scopes;

    /// The registered action `id`.
    fn action(id: &str) -> &'static dyn Registered {
        *REGISTRY.iter().find(|action| action.id() == id).unwrap()
    }

    /// Why `id` refuses `args`: the kind, the field and the words.
    fn refused(id: &str, args: Value) -> (Kind, Option<&'static str>, String) {
        let failure = action(id).prepare(args).unwrap_err();
        (failure.kind, failure.field, failure.detail)
    }

    const FOLDER: &str = "/volumes/LAB/dukovo-backup/lab-machine";
    const KEY_FILE: &str = "/volumes/KEYS/lab-machine-recovery.key";
    const TYPED: &str = "AGE-SECRET-KEY-1QQQQ typed";
    const OLD: &str = "52:54:00:aa:00:01";
    const NEW: &str = "52:54:00:bb:00:01";

    /// The four actions: id, route, whether it changes anything, whether it
    /// is a task, and the command its first example makes.
    const ACTIONS: [(&str, &str, bool, bool, &[&str]); 4] = [
        ("move.find", "/api/1/move/sources", false, false, &["backup", "find", "--json"]),
        (
            "move.plan",
            "/api/1/move/plan",
            false,
            false,
            &[
                "backup",
                "restore",
                "--machine",
                "--plan",
                "--json",
                "--key",
                "/system/settings/backup/lab.key",
                "/volumes/LAB/dukovo-backup/lab-machine",
            ],
        ),
        (
            "move.stage",
            "/api/1/move/stage",
            true,
            true,
            &[
                "backup",
                "restore",
                "--machine",
                "--progress",
                "--key",
                "/system/settings/backup/lab.key",
                "/volumes/LAB/dukovo-backup/lab-machine",
            ],
        ),
        (
            "move.cancel",
            "/api/1/move/cancel",
            true,
            false,
            &["backup", "restore", "--machine", "--cancel"],
        ),
    ];

    #[test]
    fn each_action_runs_the_command_the_design_names_as_an_administrator() {
        for (id, path, changes, long, words) in ACTIONS {
            let action = action(id);
            let example = action.example_values().0.remove(0);
            let prepared = action.prepare(example).unwrap();
            assert_eq!(prepared.argv, words, "{id}");
            assert_eq!(
                (action.changes(), action.runs(), action.area(), action.path(), action.long()),
                (changes, Runs::AsAdministrator, "move", path, long),
                "{id}"
            );
            assert!(prepared.ansible.become_root, "{id}");
            assert_eq!(prepared.ansible.params["argv"], json!(words), "{id}");
            assert_eq!(prepared.ansible.changed_when.as_deref() == Some("false"), !changes, "{id}");
            // A read is told by its JSON, and a task by its steps.
            assert_eq!(words.contains(&"--json"), !changes, "{id}");
            assert_eq!(prepared.trial, None, "{id}");
            let tests = action.tests();
            assert_eq!(tests.checks, "admind::actions::moving::tests", "{id}");
            assert_eq!((tests.route, tests.same_as_command), ("admin-move", "admin-move"), "{id}");
            assert_eq!((tests.browser, tests.ansible), ("web-console/move", "web-console/ansible"));
        }
        let ours: Vec<_> = REGISTRY.iter().filter(|a| a.id().starts_with("move.")).collect();
        assert_eq!(ours.len(), ACTIONS.len());
    }

    #[test]
    fn only_the_plan_is_a_read_with_a_secret_and_so_a_post_and_none_answers_one() {
        for (id, _, changes, ..) in ACTIONS {
            let action = action(id);
            assert!(!action.secret_answer(), "{id}");
            assert_eq!(
                action.posts(),
                changes || id == "move.plan",
                "{id}: a change is a POST, and so is a read that takes a key"
            );
            let method = if action.posts() { "POST" } else { "GET" };
            assert_eq!(listed(&[action])[0]["method"], method, "{id}");
        }
        for (path, post, id) in [
            ("/api/1/move/sources", false, "move.find"),
            ("/api/1/move/plan", true, "move.plan"),
            ("/api/1/move/stage", true, "move.stage"),
            ("/api/1/move/cancel", true, "move.cancel"),
        ] {
            let Found::Action(found, _) = find(REGISTRY, post, path) else { panic!("{path}") };
            assert_eq!(found.id(), id);
            assert!(matches!(find(REGISTRY, !post, path), Found::WrongMethod), "{path}");
        }
    }

    #[test]
    fn the_stage_is_the_only_task_and_the_plan_and_the_stage_say_their_errors() {
        for (id, ..) in ACTIONS {
            let action = action(id);
            let errors: &[&str] = match id {
                "move.plan" => &["BAK-103", "BAK-107"],
                "move.stage" => &["BAK-103", "BAK-104", "BAK-107"],
                _ => &[],
            };
            assert_eq!(
                (action.errors(), action.answers(), action.lines()),
                (errors, &[0][..], false)
            );
            assert_eq!(action.long(), id == "move.stage", "{id}");
        }
    }

    #[test]
    fn nothing_is_compared_for_the_stage_writes_no_settings_file_and_no_read_says_what_it_moved() {
        for (id, ..) in ACTIONS {
            let action = action(id);
            let prepared = action.prepare(action.example_values().0.remove(0)).unwrap();
            assert!(!action.compares(), "{id}");
            assert!(prepared.probe.is_none() && prepared.files.is_empty(), "{id}");
        }
    }

    #[test]
    fn a_typed_key_is_read_from_the_input_and_is_no_word_of_the_command() {
        for id in ["move.plan", "move.stage"] {
            let args = json!({ "source": "nas", "key": TYPED });
            let prepared = action(id).prepare(args).unwrap();
            let key = prepared.argv.iter().position(|word| word == "--key").unwrap();
            assert_eq!(prepared.argv[key + 1], "-", "{id}");
            assert_eq!(prepared.argv.last().map(String::as_str), Some("nas"), "{id}");
            assert_eq!(prepared.input.as_ref().unwrap().bytes(), format!("{TYPED}\n").as_bytes());
            // What a task keeps of its arguments, the journal and the Show
            // command have none of it.
            assert_eq!(prepared.args["key"], "(secret)", "{id}");
            for text in
                [prepared.argv.join(" "), prepared.args.to_string(), format!("{prepared:?}")]
            {
                assert!(!text.contains("QQQQ"), "{id}: {text}");
            }
            assert!(prepared.ansible.no_log, "{id}");
            assert_eq!(prepared.ansible.params["stdin"], "{{ key }}", "{id}");
        }
        // A key file is a path, and the command is given no input.
        let prepared =
            action("move.plan").prepare(json!({ "source": "nas", "key_file": KEY_FILE }));
        let prepared = prepared.unwrap();
        assert!(prepared.input.is_none() && !prepared.ansible.no_log);
        assert!(prepared.argv.windows(2).any(|pair| pair == ["--key", KEY_FILE]));
    }

    #[test]
    fn the_time_and_each_card_are_words_of_the_command_before_the_source() {
        let cards = format!("{OLD}={NEW}\nnet1=automatic");
        let args = json!({ "source": FOLDER, "key_file": KEY_FILE, "at": "2026-11-14-013000", "cards": cards });
        for (id, mode) in
            [("move.plan", ["--plan", "--json"].as_slice()), ("move.stage", &["--progress"])]
        {
            let argv = action(id).prepare(args.clone()).unwrap().argv;
            let mut wanted: Vec<String> =
                ["backup", "restore", "--machine"].map(String::from).to_vec();
            wanted.extend(mode.iter().map(ToString::to_string));
            for word in [
                "--key",
                KEY_FILE,
                "--at",
                "2026-11-14-013000",
                "--card",
                &format!("{OLD}={NEW}"),
                "--card",
                "net1=automatic",
                FOLDER,
            ] {
                wanted.push(word.to_string());
            }
            assert_eq!(argv, wanted, "{id}");
        }
        // What was asked is kept as it was, and what was left out is not made up.
        let prepared =
            action("move.stage").prepare(json!({ "source": "nas", "key_file": KEY_FILE })).unwrap();
        assert_eq!(prepared.args, json!({ "source": "nas", "key_file": KEY_FILE }));
    }

    #[test]
    fn a_source_is_a_backup_place_s_name_or_a_whole_folder_and_nothing_else() {
        for id in ["move.plan", "move.stage"] {
            for source in [
                "nas",
                "a",
                "off_site",
                "Nas-2",
                "x".repeat(32).as_str(),
                "/",
                FOLDER,
                "/with space/x",
                "/a/..b/c..",
            ] {
                let args = json!({ "source": source, "key_file": KEY_FILE });
                assert_eq!(
                    action(id).prepare(args).unwrap().argv.last().map(String::as_str),
                    Some(source),
                    "{id} {source:?}"
                );
            }
            let place = "The source is the name of a backup place, or the whole path of a folder \
                         that holds a machine's backups. Nothing was done.";
            let folder = "The source folder is a path that starts with /, is at most 4096 bytes \
                          long, and has no . or .. part. Nothing was done.";
            for (source, why) in [
                ("", place),
                ("x y", place),
                ("-x", place),
                (".", place),
                ("a/b", place),
                ("../x", place),
                ("ssh://nas/backups", place),
                ("x".repeat(33).as_str(), place),
                ("/volumes/../etc", folder),
                ("/..", folder),
                ("/a/./b", folder),
                ("/a\u{0}b", folder),
            ] {
                let args = json!({ "source": source, "key_file": KEY_FILE });
                assert_eq!(
                    refused(id, args),
                    (Kind::BadValue, Some("source"), why.into()),
                    "{id} {source:?}"
                );
            }
            let long = format!("/{}", "a".repeat(4096));
            let args = json!({ "source": long, "key_file": KEY_FILE });
            assert_eq!(refused(id, args).1, Some("source"));
        }
    }

    #[test]
    fn one_of_the_key_file_and_the_typed_key_is_given_and_not_both() {
        for id in ["move.plan", "move.stage"] {
            let both = json!({ "source": FOLDER, "key_file": KEY_FILE, "key": TYPED });
            assert_eq!(
                refused(id, both),
                (
                    Kind::BadValue,
                    Some("key"),
                    "Give the key as a file or typed, not both. Nothing was done.".into()
                )
            );
            let neither = refused(id, json!({ "source": FOLDER }));
            assert_eq!(
                neither,
                (
                    Kind::BadValue,
                    Some("key"),
                    "Give the key as a file or typed: the recovery key opens the backups. Nothing was done.".into()
                )
            );
            // A null is an argument left out.
            let nulls = json!({ "source": FOLDER, "key_file": KEY_FILE, "key": null });
            assert!(action(id).prepare(nulls).is_ok());
            let files = ["relative.key", "", "/a/../b", "/k\u{0}"];
            for file in files {
                let (kind, field, _) = refused(id, json!({ "source": FOLDER, "key_file": file }));
                assert_eq!((kind, field), (Kind::BadValue, Some("key_file")), "{file:?}");
            }
            // A typed key is one line, and a refusal never quotes it.
            for key in ["", "two\nlines", "a\rb", "a\0b", &"k".repeat(KEY_MAX + 1)] {
                let (kind, field, why) = refused(id, json!({ "source": FOLDER, "key": key }));
                assert_eq!((kind, field), (Kind::BadValue, Some("key")), "{key:?}");
                assert!(!why.contains("kkkk") && !why.contains("two"), "{why}");
            }
            let widest = "k".repeat(KEY_MAX);
            assert!(action(id).prepare(json!({ "source": FOLDER, "key": widest })).is_ok());
        }
    }

    #[test]
    fn a_time_is_the_one_backup_show_lists_and_a_card_choice_is_the_one_backup_reads() {
        let with = |extra: Value| {
            let mut args = json!({ "source": FOLDER, "key_file": KEY_FILE });
            args[extra.as_object().unwrap().keys().next().unwrap().as_str()] =
                extra.as_object().unwrap().values().next().unwrap().clone();
            args
        };
        for at in ["2026-11-14-013000", "1999-01-01-000000"] {
            assert!(action("move.stage").prepare(with(json!({ "at": at }))).is_ok(), "{at}");
        }
        let at_words = "A backup's time is written YYYY-MM-DD-HHMMSS, as backup show lists it. Nothing was done.";
        for at in [
            "",
            "2026-11-14",
            "2026-11-14-0130",
            "2026-11-14 013000",
            "2026-11-14-01300x",
            "2026-11-14-0130000",
            "latest",
        ] {
            assert_eq!(
                refused("move.stage", with(json!({ "at": at }))),
                (Kind::BadValue, Some("at"), at_words.into()),
                "{at:?}"
            );
        }
        // The command's own reader decides each line, so the action and the
        // command never disagree.
        let lines = [
            format!("{OLD}={NEW}"),
            format!("{OLD}=automatic"),
            "net0=automatic".into(),
            format!(" NET1 = {} ", NEW.to_uppercase()),
            format!("{OLD}=net0"),
            format!("{OLD}="),
            format!("={NEW}"),
            "nonsense".into(),
            format!("{OLD}={NEW}={NEW}"),
            "=".into(),
            format!("{OLD}=auto"),
            format!("a b={NEW}"),
            format!("{OLD}=52:54:00:bb:00"),
        ];
        for line in lines {
            let ours = action("move.plan").prepare(with(json!({ "cards": line }))).is_ok();
            assert_eq!(ours, Asked::parse(&line).is_ok(), "{line:?}");
        }
        let words = |number: usize| {
            format!(
                "Line {number} is not OLD=NEW or OLD=automatic, OLD a card of the backup by its MAC \
                 address or name and NEW a card of this machine by its MAC address. Nothing was done."
            )
        };
        let cards = format!("{OLD}={NEW}\nnonsense");
        assert_eq!(
            refused("move.plan", with(json!({ "cards": cards }))),
            (Kind::BadValue, Some("cards"), words(2))
        );
        assert_eq!(
            refused("move.plan", with(json!({ "cards": format!("{OLD}={NEW}\n") }))).2,
            words(2)
        );
    }

    #[test]
    fn at_most_eight_cards_are_asked_for() {
        let line = |n: usize| format!("52:54:00:aa:00:{n:02x}=automatic");
        let cards = |count: usize| (1..=count).map(line).collect::<Vec<_>>().join("\n");
        let args =
            |cards: String| json!({ "source": FOLDER, "key_file": KEY_FILE, "cards": cards });
        let prepared = action("move.stage").prepare(args(cards(8))).unwrap();
        assert_eq!(prepared.argv.iter().filter(|word| *word == "--card").count(), 8);
        assert_eq!(
            refused("move.stage", args(cards(9))),
            (
                Kind::BadValue,
                Some("cards"),
                "There are at most 8 card choices, a line each. Nothing was done.".into()
            )
        );
        let spaced = format!("{}{}", " ".repeat(CARDS_MAX), line(1));
        assert_eq!(refused("move.plan", args(spaced)).1, Some("cards"));
    }

    #[test]
    fn the_schemas_say_which_arguments_are_given_and_that_the_key_is_never_given_back() {
        let plan = schema(action("move.plan").fields());
        assert_eq!(plan["required"], json!(["source"]));
        assert_eq!(plan["additionalProperties"], false);
        let key = &plan["properties"]["key"];
        assert_eq!(
            (&key["writeOnly"], &key["maxLength"], &key["minLength"]),
            (&json!(true), &json!(256), &json!(1))
        );
        assert_eq!(plan["properties"]["key_file"]["pattern"], PATH_PATTERN);
        assert_eq!(plan["properties"]["source"]["maxLength"], 4096);
        assert_eq!(plan["properties"]["at"]["pattern"], TIME_PATTERN);
        assert_eq!(plan["properties"]["cards"]["maxLength"], 520);
        assert_eq!(schema(action("move.stage").fields()), plan);
        for id in ["move.find", "move.cancel"] {
            assert_eq!(schema(action(id).fields())["properties"], json!({}), "{id}");
        }
        // Nothing of the examples holds the key.
        let listed = listed(&[action("move.plan"), action("move.stage")]);
        assert!(!listed.to_string().contains("AGE-SECRET"));
    }

    #[test]
    fn the_key_is_never_read_from_an_address() {
        let fields = action("move.plan").fields();
        let refused = query_arguments(Params::new(), "source=nas&key=hunter2", fields).unwrap_err();
        assert_eq!(refused.kind, Kind::BadRequest);
        assert!(!refused.detail.contains("hunter2"));
    }

    #[test]
    fn every_action_is_an_administrator_s_a_reader_and_a_locked_session_are_refused() {
        for (id, ..) in ACTIONS {
            let action = action(id);
            let session = |admin, unlocked| {
                authorize(&Caller::Session { name: "ann", admin, unlocked }, action)
            };
            assert_eq!(session(false, true).unwrap_err().kind, Kind::NotAdmin, "{id}");
            assert_eq!(session(true, false).unwrap_err().kind, Kind::Locked, "{id}");
            assert!(session(true, true).is_ok(), "{id}");
        }
    }

    #[test]
    fn a_token_needs_the_whole_of_the_move_scope_even_to_read() {
        let call = |list: &str, id: &str| {
            let scopes = Scopes::parse(list).unwrap();
            authorize(&Caller::Token { name: "ci", scopes: &scopes }, action(id))
        };
        for (id, ..) in ACTIONS {
            for list in ["move", "*"] {
                assert!(call(list, id).is_ok(), "{id} {list}");
            }
            let refused = call("move:read", id).unwrap_err();
            assert_eq!(refused.kind, Kind::OutOfScope, "{id}");
            assert_eq!(
                refused.detail,
                "the API token \"ci\" may not do this: it needs the scope move or *"
            );
            assert_eq!(call("backups", id).unwrap_err().kind, Kind::OutOfScope, "{id}");
        }
    }

    #[test]
    fn the_examples_work_the_bad_ones_do_not_and_the_stage_is_put_back_by_the_cancel() {
        for (id, ..) in ACTIONS {
            let action = action(id);
            let (good, bad, undo) = action.example_values();
            assert!(!bad.is_empty(), "{id}");
            assert_eq!(undo.is_some(), id == "move.stage", "{id}: only a stage is put back");
            for args in &good {
                assert!(action.prepare(args.clone()).is_ok(), "{id}");
            }
            for args in bad {
                assert!(action.prepare(args.clone()).is_err(), "{id} {args}");
            }
        }
        let (good, bad, _) = action("move.plan").example_values();
        assert_eq!(
            good,
            [json!({ "source": FOLDER, "key_file": "/system/settings/backup/lab.key" })]
        );
        assert!(
            bad.contains(
                &json!({ "source": "x y", "key_file": "/system/settings/backup/lab.key" })
            )
        );
        assert!(bad.contains(&json!({ "source": FOLDER, "key_file": "/system/settings/backup/lab.key", "cards": "nonsense" })));
        assert_eq!(action("move.stage").example_values().0, good);
        let (_, _, undo) = action("move.stage").example_values();
        assert_eq!(undo, Some(Undo { action: "move.cancel", args: json!({}) }));
        let (_, bad, _) = action("move.cancel").example_values();
        assert_eq!(bad, [json!({ "now": true })]);
    }
}
