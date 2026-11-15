// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The snapshots' actions (ADR 0461, ADR 0515): the list of the places'
//! snapshots, a folder as a snapshot has it, a snapshot taken, and files
//! copied back from one. The area is `snapshots`, the scope a token needs.
//!
//! The reads and the person's restore run as the person, whose own rights
//! decide what they see and may copy; `snapshots.browse-all` and
//! `snapshots.restore-all` are the same commands as an administrator, who
//! reads and writes any person's files, and `snapshots.take` makes a
//! snapshot, which needs one. A restore that runs as an administrator
//! copies into folders that other accounts own, which is why `snap restore
//! --beside` takes the owner's rights for the copy before it creates
//! anything (the design's section 22, item 6).
//!
//! Every path is absolute, at most 4096 bytes, with no NUL and no `.` or
//! `..` part, as [`path_ok`] says; a folder to browse is within the
//! place it is browsed in. A snapshot's name is a file name that does not
//! start with a dot, and a label is 1 to 40 printable characters with no
//! `/`.

use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::json;

use super::{Action, Args, Bad, Examples, Field, Runs, Tests};
use crate::snap::keep;

/// The area of these actions: the scope a token needs, `snapshots`.
const AREA: &str = "snapshots";

/// The tests of these actions, which `lab/actions.toml` names.
const TESTS: Tests = Tests {
    checks: "admind::actions::snapshots::tests",
    route: "admin-backups",
    same_as_command: "admin-backups",
    browser: "web-console/snapshots",
    ansible: "web-console/ansible",
};

/// The most bytes of a path.
pub(super) const PATH_MAX: usize = 4096;

/// What a path matches: from `/`, no NUL, and no `.` or `..` part. The
/// check, [`path_ok`], is the exact one.
pub(super) const PATH_PATTERN: &str = "^(?![^]*/\\.\\.?(/|$))/[^\u{0}]*$";

/// The most bytes of a snapshot's name.
const SNAPSHOT_MAX: usize = 255;

/// What a snapshot's name matches: it does not start with a dot, and has
/// no `/` and no NUL. A name that is a time is the check's alone.
const SNAPSHOT_PATTERN: &str = "^[^/\u{0}.][^/\u{0}]*$";

/// The most characters of a label.
const LABEL_MAX: usize = 40;

/// What a label matches: no control character, no `/`, and not `.` or
/// `..` (the design's section 22, item 6). Control characters are those
/// of `char::is_control`.
const LABEL_PATTERN: &str = "^(?!\\.\\.?$)[^/\u{0}-\u{1f}\u{7f}-\u{9f}]*$";

/// The most paths one restore names.
const PATHS_MOST: usize = 100;

/// What the lines of `paths` match: each a path from `/`, with no `.` or
/// `..` part. How many, and how long each is, are the check's alone.
const PATHS_PATTERN: &str = "^(?![^]*/\\.\\.?(/|\n|$))/[^\u{0}\n]*(\n/[^\u{0}\n]*)*$";

/// The words a path is described by, for a refusal: the rule, and that
/// nothing was done.
fn path_words(what: &str) -> String {
    format!(
        "The {what} is a path that starts with /, is at most {PATH_MAX} bytes long, and has no \
         . or .. part. Nothing was done."
    )
}

/// Whether `text` is a path as these actions take one: from `/`, at most
/// [`PATH_MAX`] bytes, with no NUL and no `.` or `..` part.
pub(super) fn path_ok(text: &str) -> bool {
    text.len() <= PATH_MAX
        && text.starts_with('/')
        && !text.contains('\0')
        && !text.split('/').any(|part| part == "." || part == "..")
}

/// Checks that `text` is a path, for the field `field`, whose words call it
/// `what`.
pub(super) fn check_path(field: &'static str, what: &str, text: &str) -> Result<(), Bad> {
    if path_ok(text) { Ok(()) } else { Err(Bad { field, why: path_words(what) }) }
}

/// Whether `name` can name a snapshot of a place to browse or restore: a
/// file name, 1 to 255 bytes, that does not start with a dot.
fn snapshot_ok(name: &str) -> bool {
    (1..=SNAPSHOT_MAX).contains(&name.len())
        && !name.starts_with('.')
        && !name.contains(['/', '\0'])
}

fn check_snapshot(name: &str) -> Result<(), Bad> {
    if snapshot_ok(name) {
        return Ok(());
    }
    Err(Bad {
        field: "snapshot",
        why: format!(
            "A snapshot's name is a file name of 1 to {SNAPSHOT_MAX} bytes that does not start \
             with a dot, as snap list shows it. Nothing was done."
        ),
    })
}

/// Whether `label` can be the words in a restored file's name: 1 to 40
/// printable characters, no `/`, and not `.` or `..`.
fn label_ok(label: &str) -> bool {
    (1..=LABEL_MAX).contains(&label.chars().count())
        && label != "."
        && label != ".."
        && !label.contains('/')
        && !label.chars().any(char::is_control)
}

/// The place's field.
const PLACE: Field = Field::text(
    "place",
    "The place: the top folder of a subvolume, as snap list shows it. /users.",
    1,
    PATH_MAX,
    Some(PATH_PATTERN),
);

/// The snapshot's field.
const SNAPSHOT: Field = Field::text(
    "snapshot",
    "The snapshot's name, as snap list shows it.",
    1,
    SNAPSHOT_MAX,
    Some(SNAPSHOT_PATTERN),
);

/// The snapshots of one place, or each place's count.
#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Listed {
    /// The place whose snapshots to list; every place's count without it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub place: Option<String>,
}

impl Args for Listed {
    const FIELDS: &'static [Field] = &[PLACE.optional()];

    fn check(&self) -> Result<(), Bad> {
        match &self.place {
            Some(place) => check_path("place", "place", place),
            None => Ok(()),
        }
    }
}

/// `snap list [PLACE] --json`: each place's snapshots, or the count of
/// each place's.
///
/// It answers with its JSON when it exits 1 too: a place that fails is
/// named on standard error and left out, as `snap list` does at a
/// terminal.
pub struct List;

impl Action for List {
    const ID: &'static str = "snapshots.list";
    const AREA: &'static str = AREA;
    const CHANGES: bool = false;
    const PATH: &'static str = "/api/1/snapshots";
    const RUNS: Runs = Runs::AsThePerson;
    const ANSWERS: &'static [i32] = &[0, 1];
    const TESTS: Tests = TESTS;
    const TITLE: &'static str = "List the snapshots";
    type Args = Listed;

    fn argv(args: &Listed) -> Vec<String> {
        let mut argv = vec!["snap".to_string(), "list".to_string()];
        argv.extend(args.place.clone());
        argv.push("--json".into());
        argv
    }

    fn examples() -> Examples<Listed> {
        Examples {
            good: vec![Listed::default(), Listed { place: Some("/users".into()) }],
            bad: vec![
                json!({ "place": "users" }),
                json!({ "place": "/users/../shared" }),
                json!({ "all": true }),
            ],
            undo: None,
        }
    }
}

/// A folder of a snapshot of a place.
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Browsed {
    pub place: String,
    pub snapshot: String,
    /// The folder, within the place; the place's top without it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub folder: Option<String>,
}

impl Args for Browsed {
    const FIELDS: &'static [Field] = &[
        PLACE,
        SNAPSHOT,
        Field::text(
            "folder",
            "The folder to show, within the place: the place's top without it.",
            1,
            PATH_MAX,
            Some(PATH_PATTERN),
        )
        .optional(),
    ];

    fn check(&self) -> Result<(), Bad> {
        check_path("place", "place", &self.place)?;
        check_snapshot(&self.snapshot)?;
        if let Some(folder) = &self.folder {
            check_path("folder", "folder", folder)?;
            if !Path::new(folder).starts_with(&self.place) {
                return Err(Bad {
                    field: "folder",
                    why: format!(
                        "The folder is the place, {}, or within it. Nothing was done.",
                        self.place
                    ),
                });
            }
        }
        Ok(())
    }
}

/// `snap browse`'s words: the snapshot comes after `--`, as a name that
/// starts with `-` can.
fn browse_argv(args: &Browsed) -> Vec<String> {
    let mut argv: Vec<String> = ["snap", "browse", "--json", "--"].map(String::from).to_vec();
    argv.extend([args.place.clone(), args.snapshot.clone()]);
    argv.extend(args.folder.clone());
    argv
}

fn browse_examples() -> Examples<Browsed> {
    let (place, snapshot) = ("/users".to_string(), "lab-before".to_string());
    Examples {
        good: vec![
            Browsed {
                place: place.clone(),
                snapshot: snapshot.clone(),
                folder: Some("/users/lab-reader".into()),
            },
            Browsed { place, snapshot, folder: None },
        ],
        bad: vec![
            json!({ "place": "/users", "snapshot": "lab-before", "folder": "/shared" }),
            json!({ "place": "/users", "snapshot": "../x" }),
            json!({ "place": "/users", "snapshot": "lab-before", "folder": "/users/../shared" }),
            json!({ "place": "/users" }),
        ],
        undo: None,
    }
}

/// `snap browse PLACE SNAPSHOT [FOLDER] --json` as the person, who sees
/// what they could read when the snapshot was made.
pub struct Browse;

impl Action for Browse {
    const ID: &'static str = "snapshots.browse";
    const AREA: &'static str = AREA;
    const CHANGES: bool = false;
    const PATH: &'static str = "/api/1/snapshots/browse";
    const RUNS: Runs = Runs::AsThePerson;
    const ERRORS: &'static [&'static str] = &["BAK-105"];
    const TESTS: Tests = TESTS;
    const TITLE: &'static str = "Browse a snapshot";
    type Args = Browsed;

    fn argv(args: &Browsed) -> Vec<String> {
        browse_argv(args)
    }

    fn subject(args: &Browsed) -> Option<String> {
        Some(args.snapshot.clone())
    }

    fn examples() -> Examples<Browsed> {
        browse_examples()
    }
}

/// The same as an administrator, who sees everyone's files.
pub struct BrowseAll;

impl Action for BrowseAll {
    const ID: &'static str = "snapshots.browse-all";
    const AREA: &'static str = AREA;
    const CHANGES: bool = false;
    const PATH: &'static str = "/api/1/snapshots/browse/all";
    const RUNS: Runs = Runs::AsAdministrator;
    const ERRORS: &'static [&'static str] = &["BAK-105"];
    const TESTS: Tests = TESTS;
    const TITLE: &'static str = "Browse a snapshot as an administrator";
    type Args = Browsed;

    fn argv(args: &Browsed) -> Vec<String> {
        browse_argv(args)
    }

    fn subject(args: &Browsed) -> Option<String> {
        Some(args.snapshot.clone())
    }

    fn examples() -> Examples<Browsed> {
        browse_examples()
    }
}

/// A snapshot to take of a place, and what to call it.
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Taken {
    pub place: String,
    /// Its name; its time without one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

impl Args for Taken {
    const FIELDS: &'static [Field] = &[
        PLACE,
        Field::text(
            "name",
            "What to call it: a file name that does not start with a dot and is not a time. Its \
             time without one.",
            1,
            SNAPSHOT_MAX,
            Some(SNAPSHOT_PATTERN),
        )
        .optional(),
    ];

    fn check(&self) -> Result<(), Bad> {
        check_path("place", "place", &self.place)?;
        if self.name.as_deref().is_some_and(|name| !keep::may_name(name)) {
            return Err(Bad {
                field: "name",
                why: "A snapshot's name is a file name that does not start with a dot and is not \
                      a time, which snap gives its own. Nothing was done."
                    .into(),
            });
        }
        Ok(())
    }
}

/// `snap create [--name NAME] PLACE`: a read-only snapshot of the place,
/// which needs an administrator.
pub struct Take;

impl Action for Take {
    const ID: &'static str = "snapshots.take";
    const AREA: &'static str = AREA;
    const CHANGES: bool = true;
    const PATH: &'static str = "/api/1/snapshots/take";
    const RUNS: Runs = Runs::AsAdministrator;
    const TESTS: Tests = TESTS;
    const TITLE: &'static str = "Take a snapshot";
    type Args = Taken;

    fn argv(args: &Taken) -> Vec<String> {
        let mut argv = vec!["snap".to_string(), "create".to_string()];
        if let Some(name) = &args.name {
            argv.extend(["--name".to_string(), name.clone()]);
        }
        argv.push(args.place.clone());
        argv
    }

    fn subject(args: &Taken) -> Option<String> {
        Some(args.place.clone())
    }

    fn examples() -> Examples<Taken> {
        Examples {
            good: vec![
                Taken { place: "/users".into(), name: Some("lab-before".into()) },
                Taken { place: "/users".into(), name: None },
            ],
            bad: vec![
                json!({ "place": "users" }),
                json!({ "place": "/users", "name": ".x" }),
                json!({ "place": "/users", "name": "a/b" }),
                json!({ "place": "/users", "name": "2026-11-11-0300" }),
                json!({ "name": "lab-before" }),
            ],
            undo: None,
        }
    }
}

/// Files to copy back from a snapshot, and the label that names the
/// copies.
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Restored {
    pub snapshot: String,
    /// The words in each copy's name, as `notes (lab).txt`.
    pub label: String,
    /// The paths to copy back, one a line.
    pub paths: String,
}

impl Args for Restored {
    const FIELDS: &'static [Field] = &[
        SNAPSHOT,
        Field::text(
            "label",
            "The words in each copy's name: notes (label).txt. 1 to 40 characters, with no /.",
            1,
            LABEL_MAX,
            Some(LABEL_PATTERN),
        ),
        Field::text(
            "paths",
            "The files and folders to copy back, as paths from /, one a line: 1 to 100.",
            1,
            PATHS_MOST * (PATH_MAX + 1),
            Some(PATHS_PATTERN),
        ),
    ];

    fn check(&self) -> Result<(), Bad> {
        check_snapshot(&self.snapshot)?;
        if !label_ok(&self.label) {
            return Err(Bad {
                field: "label",
                why: format!(
                    "A label is 1 to {LABEL_MAX} printable characters, with no /, and is not \".\" or \
                     \"..\". Nothing was restored."
                ),
            });
        }
        let paths: Vec<&str> = self.paths.split('\n').collect();
        if paths.len() > PATHS_MOST {
            return Err(Bad {
                field: "paths",
                why: format!("At most {PATHS_MOST} paths at once. Nothing was restored."),
            });
        }
        for path in paths {
            if !path_ok(path) {
                return Err(Bad {
                    field: "paths",
                    why: format!(
                        "Each path starts with /, is at most {PATH_MAX} bytes long, and has no . \
                         or .. part. Nothing was restored."
                    ),
                });
            }
        }
        Ok(())
    }
}

/// `snap restore --from SNAPSHOT --beside LABEL PATH...`: each path's copy
/// from the snapshot goes beside it, never over it.
fn restore_argv(args: &Restored) -> Vec<String> {
    let mut argv: Vec<String> = ["snap", "restore", "--from"].map(String::from).to_vec();
    argv.extend([args.snapshot.clone(), "--beside".into(), args.label.clone()]);
    argv.extend(args.paths.split('\n').map(String::from));
    argv
}

fn restore_examples() -> Examples<Restored> {
    let good = Restored {
        snapshot: "lab-before".into(),
        label: "lab".into(),
        paths: "/users/lab-reader/notes.txt".into(),
    };
    let many = vec!["/a"; PATHS_MOST + 1].join("\n");
    Examples {
        good: vec![good],
        bad: vec![
            json!({ "snapshot": "lab-before", "label": "a/b", "paths": "/users/lab-reader/notes.txt" }),
            json!({ "snapshot": "lab-before", "label": "lab", "paths": "" }),
            json!({ "snapshot": "lab-before", "label": "lab", "paths": many }),
            json!({ "snapshot": "lab-before", "label": "lab", "paths": "notes.txt" }),
            json!({ "snapshot": "lab-before", "label": "..", "paths": "/users/lab-reader/notes.txt" }),
        ],
        undo: None,
    }
}

/// `snap restore --from SNAPSHOT --beside LABEL PATH...` as the person,
/// who may copy what they could read, into the folders they may write.
pub struct Restore;

impl Action for Restore {
    const ID: &'static str = "snapshots.restore";
    const AREA: &'static str = AREA;
    const CHANGES: bool = true;
    const PATH: &'static str = "/api/1/snapshots/restore";
    const RUNS: Runs = Runs::AsThePerson;
    const ERRORS: &'static [&'static str] = &["BAK-105", "BAK-106"];
    const TESTS: Tests = TESTS;
    const TITLE: &'static str = "Restore files from a snapshot";
    type Args = Restored;

    fn argv(args: &Restored) -> Vec<String> {
        restore_argv(args)
    }

    fn subject(args: &Restored) -> Option<String> {
        Some(args.snapshot.clone())
    }

    fn examples() -> Examples<Restored> {
        restore_examples()
    }
}

/// The same as an administrator, for files that are other people's.
pub struct RestoreAll;

impl Action for RestoreAll {
    const ID: &'static str = "snapshots.restore-all";
    const AREA: &'static str = AREA;
    const CHANGES: bool = true;
    const PATH: &'static str = "/api/1/snapshots/restore/all";
    const RUNS: Runs = Runs::AsAdministrator;
    const ERRORS: &'static [&'static str] = &["BAK-105", "BAK-106"];
    const TESTS: Tests = TESTS;
    const TITLE: &'static str = "Restore files from a snapshot as an administrator";
    type Args = Restored;

    fn argv(args: &Restored) -> Vec<String> {
        restore_argv(args)
    }

    fn subject(args: &Restored) -> Option<String> {
        Some(args.snapshot.clone())
    }

    fn examples() -> Examples<Restored> {
        restore_examples()
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

    fn refused(id: &str, args: serde_json::Value) -> (Kind, Option<&'static str>, String) {
        let failure = action(id).prepare(args).unwrap_err();
        (failure.kind, failure.field, failure.detail)
    }

    /// What `snap list --json` prints of one place and of every place,
    /// as its golden files in `tests/json` hold them, which
    /// `snapshots.list` answers as printed: it reads neither.
    const WITH_PLACE: &str = include_str!("../../../tests/json/snap-list-place.json");
    const WITHOUT_PLACE: &str = include_str!("../../../tests/json/snap-list.json");

    #[test]
    fn the_list_is_snap_list_with_json_for_every_place_or_for_one() {
        let list = action("snapshots.list");
        assert_eq!(
            (list.changes(), list.runs(), list.area(), list.path()),
            (false, Runs::AsThePerson, "snapshots", "/api/1/snapshots")
        );
        let all = list.prepare(json!({})).unwrap();
        assert_eq!(all.argv, ["snap", "list", "--json"]);
        let one = list.prepare(json!({ "place": "/users" })).unwrap();
        assert_eq!(one.argv, ["snap", "list", "/users", "--json"]);
        assert_eq!(one.args, json!({ "place": "/users" }));
        // A place that fails is told of on standard error and left out, with
        // the exit status 1 and the JSON all the same.
        assert_eq!(list.answers(), [0, 1]);
        let task = &one.ansible;
        assert_eq!(task.failed_when.as_deref(), Some("answer.rc not in [0, 1]"));
        assert_eq!(task.changed_when.as_deref(), Some("false"));
        assert_eq!(task.register, Some("answer"));
    }

    #[test]
    fn what_the_list_answers_is_what_the_command_printed_whichever_form_it_is() {
        // The shapes the page reads, held here so that a change of the
        // command's is seen: each place with its folder, the count or the
        // snapshots, and the times `Date.parse` reads, or null.
        let without: serde_json::Value = serde_json::from_str(WITHOUT_PLACE).unwrap();
        let places = without["places"].as_array().unwrap();
        for place in places {
            for member in ["place", "folder", "newest", "newest_made", "snapshots", "in_service"] {
                assert!(place.get(member).is_some(), "{member}");
            }
            assert!(place["snapshots"].is_u64());
        }
        assert!(places[0]["newest_made"].as_str().is_some_and(|made| made.contains('T')));
        assert!(without["settings"]["daily"].is_u64() && without["update"].is_array());
        assert!(without["update"][0]["made"].as_str().is_some_and(|made| made.contains('T')));
        let with: serde_json::Value = serde_json::from_str(WITH_PLACE).unwrap();
        assert_eq!(
            (&with["place"], &with["folder"]),
            (&json!("/users"), &json!("/snapshots/users"))
        );
        for snapshot in with["snapshots"].as_array().unwrap() {
            for member in ["name", "made", "named", "kept", "prunable", "restored"] {
                assert!(snapshot.get(member).is_some(), "{member}");
            }
            assert!(
                snapshot["made"].is_null()
                    || snapshot["made"].as_str().is_some_and(|made| made.contains('T'))
            );
        }
    }

    #[test]
    fn a_place_is_a_path_and_nothing_else_is_taken() {
        for place in ["users", "", "/users/../shared", "/users/./x", "/.."] {
            let (kind, field, _) = refused("snapshots.list", json!({ "place": place }));
            assert!(
                kind == Kind::BadValue && field == Some("place") || place.is_empty(),
                "{place:?}"
            );
        }
        let (kind, field, words) = refused("snapshots.list", json!({ "place": "users" }));
        assert_eq!((kind, field), (Kind::BadValue, Some("place")));
        assert_eq!(
            words,
            "The place is a path that starts with /, is at most 4096 bytes long, and has no . or \
             .. part. Nothing was done."
        );
        assert_eq!(refused("snapshots.list", json!({ "all": true })).0, Kind::BadRequest);
        let long = format!("/{}", "a".repeat(4096));
        assert_eq!(refused("snapshots.list", json!({ "place": long })).0, Kind::BadValue);
        assert!(
            action("snapshots.list")
                .prepare(json!({ "place": format!("/{}", "a".repeat(4095)) }))
                .is_ok()
        );
        assert!(
            action("snapshots.list").prepare(json!({ "place": "/volumes/sdb1/@data" })).is_ok()
        );
    }

    #[test]
    fn a_browse_asks_for_json_and_names_the_snapshot_after_the_dashes() {
        for (id, runs, path) in [
            ("snapshots.browse", Runs::AsThePerson, "/api/1/snapshots/browse"),
            ("snapshots.browse-all", Runs::AsAdministrator, "/api/1/snapshots/browse/all"),
        ] {
            let browse = action(id);
            assert_eq!(
                (browse.changes(), browse.runs(), browse.path()),
                (false, runs, path),
                "{id}"
            );
            assert_eq!(browse.errors(), ["BAK-105"], "{id}");
            let args = json!({ "place": "/users", "snapshot": "lab-before", "folder": "/users/lab-reader" });
            let prepared = browse.prepare(args).unwrap();
            assert_eq!(
                prepared.argv,
                ["snap", "browse", "--json", "--", "/users", "lab-before", "/users/lab-reader"],
                "{id}"
            );
            let top = browse.prepare(json!({ "place": "/users", "snapshot": "-odd" })).unwrap();
            assert_eq!(top.argv, ["snap", "browse", "--json", "--", "/users", "-odd"], "{id}");
            assert_eq!(prepared.ansible.become_root, runs == Runs::AsAdministrator, "{id}");
        }
    }

    #[test]
    fn a_folder_is_the_place_or_within_it_and_a_snapshot_is_a_name() {
        let browse = |args| refused("snapshots.browse", args);
        let (kind, field, words) =
            browse(json!({ "place": "/users", "snapshot": "lab-before", "folder": "/shared" }));
        assert_eq!((kind, field), (Kind::BadValue, Some("folder")));
        assert_eq!(words, "The folder is the place, /users, or within it. Nothing was done.");
        // A folder whose name only starts with the place's is not within it.
        let beside = json!({ "place": "/users", "snapshot": "s", "folder": "/users2/x" });
        assert_eq!(browse(beside).1, Some("folder"));
        for snapshot in ["../x", ".hidden", "a/b", "", "."] {
            let (kind, field, _) = browse(json!({ "place": "/users", "snapshot": snapshot }));
            assert_eq!((kind, field), (Kind::BadValue, Some("snapshot")), "{snapshot:?}");
        }
        let long = "a".repeat(256);
        assert_eq!(browse(json!({ "place": "/users", "snapshot": long })).1, Some("snapshot"));
        let accepted = |snapshot: &str| {
            action("snapshots.browse")
                .prepare(json!({ "place": "/users", "snapshot": snapshot, "folder": "/users/" }))
                .is_ok()
        };
        assert!(
            accepted("lab-before") && accepted("2026-11-13-0302") && accepted("r\u{e9}sum\u{e9}")
        );
        assert!(accepted("-dash") && accepted("a b"));
        assert!(!accepted(&"\u{e9}".repeat(128)));
    }

    #[test]
    fn a_snapshot_is_taken_as_an_administrator_with_a_name_that_is_not_a_time() {
        let take = action("snapshots.take");
        assert_eq!(
            (take.changes(), take.runs(), take.long(), take.path()),
            (true, Runs::AsAdministrator, false, "/api/1/snapshots/take")
        );
        assert!(!take.compares());
        let named = take.prepare(json!({ "place": "/users", "name": "lab-before" })).unwrap();
        assert_eq!(named.argv, ["snap", "create", "--name", "lab-before", "/users"]);
        assert!(named.files.is_empty() && named.probe.is_none());
        let timed = take.prepare(json!({ "place": "/users" })).unwrap();
        assert_eq!(timed.argv, ["snap", "create", "/users"]);
        assert!(named.ansible.become_root);
        for name in [".x", "a/b", "2026-11-11-0300", "2026-11-11-0300.2"] {
            let (kind, field, _) =
                refused("snapshots.take", json!({ "place": "/users", "name": name }));
            assert_eq!((kind, field), (Kind::BadValue, Some("name")), "{name}");
        }
        assert_eq!(refused("snapshots.take", json!({ "place": "users" })).1, Some("place"));
        assert_eq!(refused("snapshots.take", json!({ "name": "lab-before" })).0, Kind::BadRequest);
        assert!(take.prepare(json!({ "place": "/users", "name": "2026-11-11-03" })).is_ok());
    }

    #[test]
    fn files_are_restored_beside_each_path_with_a_label() {
        for (id, runs, path) in [
            ("snapshots.restore", Runs::AsThePerson, "/api/1/snapshots/restore"),
            ("snapshots.restore-all", Runs::AsAdministrator, "/api/1/snapshots/restore/all"),
        ] {
            let restore = action(id);
            assert_eq!(
                (restore.changes(), restore.runs(), restore.path()),
                (true, runs, path),
                "{id}"
            );
            assert_eq!(restore.errors(), ["BAK-105", "BAK-106"], "{id}");
            assert!(!restore.long() && !restore.compares(), "{id}");
            let args = json!({
                "snapshot": "lab-before",
                "label": "lab",
                "paths": "/users/lab-reader/notes.txt\n/users/lab-reader/photos"
            });
            let prepared = restore.prepare(args).unwrap();
            assert_eq!(
                prepared.argv,
                [
                    "snap",
                    "restore",
                    "--from",
                    "lab-before",
                    "--beside",
                    "lab",
                    "/users/lab-reader/notes.txt",
                    "/users/lab-reader/photos"
                ],
                "{id}"
            );
            assert!(prepared.files.is_empty() && prepared.probe.is_none(), "{id}");
        }
    }

    #[test]
    fn a_label_is_printable_without_a_slash_and_paths_are_one_to_a_hundred() {
        let with = |label: &str, paths: &str| json!({ "snapshot": "lab-before", "label": label, "paths": paths });
        let restore = |args| refused("snapshots.restore", args);
        for label in ["a/b", "", ".", "..", "a\nb", "tab\there", "\u{7f}", &"x".repeat(41)] {
            let (kind, field, _) = restore(with(label, "/users/a"));
            assert_eq!((kind, field), (Kind::BadValue, Some("label")), "{label:?}");
        }
        let ok = |label: &str| action("snapshots.restore").prepare(with(label, "/users/a")).is_ok();
        assert!(ok("lab") && ok("my backup (old)") && ok("\u{e9}t\u{e9}") && ok(&"x".repeat(40)));
        assert!(ok("...") && ok(".a"));
        for paths in ["", "notes.txt", "/users/a\n", "/users/a\n\n/users/b", "/users/../a", "/a\0b"]
        {
            let (kind, field, _) = restore(with("lab", paths));
            let expected = if paths.is_empty() { Kind::BadRequest } else { Kind::BadValue };
            assert_eq!(
                (field, kind == expected || kind == Kind::BadValue),
                (Some("paths"), true),
                "{paths:?}"
            );
        }
        let hundred = vec!["/a"; 100].join("\n");
        assert!(action("snapshots.restore").prepare(with("lab", &hundred)).is_ok());
        let (kind, field, words) = restore(with("lab", &vec!["/a"; 101].join("\n")));
        assert_eq!((kind, field), (Kind::BadValue, Some("paths")));
        assert_eq!(words, "At most 100 paths at once. Nothing was restored.");
    }

    #[test]
    fn the_routes_are_found_by_their_methods_and_the_all_addresses_are_not_a_folders() {
        for (post, path, id) in [
            (false, "/api/1/snapshots", "snapshots.list"),
            (false, "/api/1/snapshots/browse", "snapshots.browse"),
            (false, "/api/1/snapshots/browse/all", "snapshots.browse-all"),
            (true, "/api/1/snapshots/take", "snapshots.take"),
            (true, "/api/1/snapshots/restore", "snapshots.restore"),
            (true, "/api/1/snapshots/restore/all", "snapshots.restore-all"),
        ] {
            let found = find(REGISTRY, post, path);
            assert!(matches!(found, Found::Action(action, _) if action.id() == id), "{path}");
            let other = find(REGISTRY, !post, path);
            assert!(matches!(other, Found::WrongMethod), "{path}");
        }
    }

    #[test]
    fn a_token_reads_with_the_read_scope_and_changes_with_the_whole_one() {
        let call = |list: &str, id: &str| {
            let scopes = Scopes::parse(list).unwrap();
            authorize(&Caller::Token { name: "ci", scopes: &scopes }, action(id))
        };
        // A read as the person needs the read scope; a read as an
        // administrator, as every command that runs as one, the full one.
        assert!(call("snapshots:read", "snapshots.list").is_ok());
        assert!(call("snapshots:read", "snapshots.browse").is_ok());
        assert_eq!(
            call("snapshots:read", "snapshots.browse-all").unwrap_err().kind,
            Kind::OutOfScope
        );
        assert!(call("snapshots", "snapshots.browse-all").is_ok());
        for id in ["snapshots.take", "snapshots.restore", "snapshots.restore-all"] {
            assert_eq!(call("snapshots:read", id).unwrap_err().kind, Kind::OutOfScope, "{id}");
            assert!(call("snapshots", id).is_ok(), "{id}");
            assert_eq!(call("backups", id).unwrap_err().kind, Kind::OutOfScope, "{id}");
        }
    }

    #[test]
    fn the_examples_are_what_the_boot_and_lab_tests_run() {
        let (good, bad, undo) = action("snapshots.restore").example_values();
        assert_eq!(
            good,
            [
                json!({ "snapshot": "lab-before", "label": "lab", "paths": "/users/lab-reader/notes.txt" })
            ]
        );
        assert_eq!(undo, None);
        assert_eq!(bad.len(), 5);
        let (good, _, undo) = action("snapshots.take").example_values();
        assert_eq!(good[0], json!({ "place": "/users", "name": "lab-before" }));
        assert_eq!(undo, None);
        let (good, bad, _) = action("snapshots.browse").example_values();
        assert_eq!(
            good[0],
            json!({ "place": "/users", "snapshot": "lab-before", "folder": "/users/lab-reader" })
        );
        for args in bad {
            assert!(action("snapshots.browse").prepare(args.clone()).is_err(), "{args}");
        }
    }
}
