// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The apps' actions (ADR 0515): the apps everyone has, one app's sheet,
//! and an update, a way back, a hold or a removal of one, as `bundle`
//! does them, and what the catalogs list: a search of them, an app's sheet
//! before it is installed, and its install. The area is `apps`, the scope a
//! token needs.
//!
//! An app is named by its ID as `manifest` checks it, with its line after
//! an `@` when it has one (`org.nodejs.node@22`). No ID is one bare word,
//! so `/api/1/apps/update`, the address that updates every app, is never
//! an app's.
//!
//! The list, the sheet and what the catalogs list are the person's own:
//! what any account may see of the apps. Every change is the machine's,
//! made for everyone, which is an administrator's: `--for-everyone`.
//!
//! The bundles live under `/applications`, which admind does not see, so
//! no change names a file and each says whether it changed anything from
//! a read of the list before and after it. A change that is a task runs no
//! probe: it always did something, and the probe is for the check that
//! asks whether it would.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::updates::check_why;
use super::{Action, Ansible, Args, Bad, Examples, Field, Nothing, Runs, Tests, Undo};

/// The area of these actions: the scope a token needs, `apps`.
const AREA: &str = "apps";

/// The tests of these actions, which `lab/actions.toml` names.
const TESTS: Tests = Tests {
    checks: "admind::actions::apps::tests",
    route: "admin-updates",
    same_as_command: "admin-updates",
    browser: "web-console/apps",
    ansible: "web-console/ansible",
};

/// The tests of the actions that update an app and go back: the Updates
/// page offers them, and its lab test is the browser's.
const UPDATE_TESTS: Tests = Tests { browser: "web-console/updates", ..TESTS };

/// The tests of the actions that read the catalogs and install from them,
/// whose page is the one that adds an app.
const CATALOG_TESTS: Tests = Tests { browser: "web-console/add-app", ..TESTS };

/// Where the apps everyone has are: a folder of each, by its ID with its
/// line. admind does not see it, which is why a change here names no file.
const FOLDER: &str = "/applications";

/// The most characters of an app's ID with its line: the ID is at most
/// 255, which `check_full_id` holds, and its line three numbers.
pub(super) const ID_MAX: usize = 320;

/// What an app's ID matches, with the line after an `@` when it has one:
/// two labels or more of lowercase letters, digits and hyphens, none of
/// which starts or ends with a hyphen or has over 63 characters, and a
/// line of one to three numbers without leading zeros. The ID's own
/// length, of 255 at most, is `check_full_id`'s. A whole literal, which
/// the console's tests read; `updates` holds its patterns that take an ID
/// with other words to it.
pub(super) const ID_PATTERN: &str = "^[a-z0-9]([a-z0-9-]{0,61}[a-z0-9])?(\\.[a-z0-9]([a-z0-9-]{0,61}[a-z0-9])?)+\
     (@(0|[1-9][0-9]*)(\\.(0|[1-9][0-9]*)){0,2})?$";

/// The field of an app's ID, for each action that takes one.
const ID: Field = Field::text(
    "id",
    "The app's ID, as app list shows it, with its line after an @ when it has one.",
    3,
    ID_MAX,
    Some(ID_PATTERN),
);

/// What the reason kept with a hold matches: one line, with no control
/// character, as `updates` has it for the system's.
pub(super) const WHY_PATTERN: &str = "^[^\x00-\x1f\x7f]+$";

/// The reason kept with a hold: optional, one line of 1 to 200
/// characters that everyone who can sign in sees.
pub(super) const WHY: Field = Field::text(
    "why",
    "Why it is held, in one line of at most 200 characters; everyone who can sign in sees it.",
    1,
    200,
    Some(WHY_PATTERN),
)
.optional();

/// Checks `id`, which names an app, or an app of a line: the rule of
/// `manifest`, in the words the page shows under its field.
pub(super) fn check_id(field: &'static str, id: &str) -> Result<(), Bad> {
    if manifest::bundle::check_full_id(id).is_ok() {
        return Ok(());
    }
    Err(Bad {
        field,
        why: "An app's ID is a reverse DNS name in lowercase, such as org.example.app, with its \
              line after an @ when it has one, as org.nodejs.node@22. Nothing was done."
            .into(),
    })
}

/// An app, by its ID.
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Named {
    pub id: String,
}

impl Args for Named {
    const FIELDS: &'static [Field] = &[ID];

    fn check(&self) -> Result<(), Bad> {
        check_id("id", &self.id)
    }
}

/// The app the examples name: a bundle of the boot test's catalog, which
/// most machines do not have.
const EXAMPLE: &str = "org.example.notes";

/// An app, by its ID, and why it is held, if that is said.
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Holding {
    pub id: String,
    /// The reason kept with the hold.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub why: Option<String>,
}

impl Args for Holding {
    const FIELDS: &'static [Field] = &[ID, WHY];

    fn check(&self) -> Result<(), Bad> {
        check_id("id", &self.id)?;
        check_why(self.why.as_deref())
    }
}

/// Examples for an action that takes an app's ID.
fn named_examples(undo: Option<Undo>) -> Examples<Named> {
    Examples {
        good: vec![Named { id: EXAMPLE.into() }],
        bad: vec![json!({ "id": "../x" }), json!({ "id": "Notes" }), json!({ "all": true })],
        undo,
    }
}

/// `bundle list --json`, which says what the changes move: the apps, each
/// with its version and whether it is held.
pub(super) fn list_argv() -> Vec<String> {
    ["bundle", "list", "--json"].map(String::from).to_vec()
}

/// The row of `bundle list --json`'s `output` that is app `id` of
/// everyone's, if there is one: the apps of one person are not what an
/// administrator's change moves.
fn row_of(id: &str, output: &str) -> Option<Option<Value>> {
    let document: Value = serde_json::from_str(output).ok()?;
    let found = document.get("bundles")?.as_array()?.iter().find(|row| {
        row.get("id").and_then(Value::as_str) == Some(id)
            && row.get("for_everyone").and_then(Value::as_bool) == Some(true)
    });
    Some(found.cloned())
}

/// What the list says of app `id` that a change moves, its version,
/// whether it is held and why, as text to compare; `none` for an app that
/// everyone does not have.
pub(super) fn picked(id: &str, output: &str) -> Option<String> {
    Some(match row_of(id, output)? {
        Some(row) => {
            let member = |key: &str| row.get(key).cloned().unwrap_or(Value::Null);
            json!({
                "version": member("version"),
                "held": member("held"),
                "held_reason": member("held_reason"),
            })
            .to_string()
        }
        None => "none".into(),
    })
}

/// What the app of `now`, as [`picked`] says it, says of `key`.
fn said(now: &str, key: &str) -> Option<Value> {
    serde_json::from_str::<Value>(now).ok()?.get(key).cloned()
}

/// Whether the app of `now`, as [`picked`] says it, is held, if it says.
fn held_in(now: &str) -> Option<bool> {
    serde_json::from_str::<Value>(now).ok()?.get("held")?.as_bool()
}

/// `bundle list --json` as the person: the apps, with what each updates to
/// and the flaws known in it.
pub struct List;

impl Action for List {
    const ID: &'static str = "apps.list";
    const AREA: &'static str = AREA;
    const CHANGES: bool = false;
    const PATH: &'static str = "/api/1/apps";
    const RUNS: Runs = Runs::AsThePerson;
    const TESTS: Tests = TESTS;
    const TITLE: &'static str = "List the apps";
    type Args = Nothing;

    fn argv(_: &Nothing) -> Vec<String> {
        list_argv()
    }

    fn examples() -> Examples<Nothing> {
        Examples { good: vec![Nothing {}], bad: vec![json!({ "all": true })], undo: None }
    }
}

/// `bundle info ID --json` as the person: one app's sheet.
pub struct Show;

impl Action for Show {
    const ID: &'static str = "apps.show";
    const AREA: &'static str = AREA;
    const CHANGES: bool = false;
    const PATH: &'static str = "/api/1/apps/{id}";
    const RUNS: Runs = Runs::AsThePerson;
    const TESTS: Tests = TESTS;
    const TITLE: &'static str = "Show an app";
    type Args = Named;

    fn argv(args: &Named) -> Vec<String> {
        vec!["bundle".into(), "info".into(), args.id.clone(), "--json".into()]
    }

    fn subject(args: &Named) -> Option<String> {
        Some(args.id.clone())
    }

    fn examples() -> Examples<Named> {
        named_examples(None)
    }
}

/// An app to update, and whether to take what it asks for anew.
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Updating {
    pub id: String,
    /// Whether an update that asks for more than the version before is
    /// taken: without it, the command stops with the sheet's words.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub yes: Option<bool>,
}

impl Args for Updating {
    const FIELDS: &'static [Field] = &[
        ID,
        Field::flag(
            "yes",
            "Whether to take an update that asks for more than the version before. Without it \
             the update stops, and says what it asks for.",
        )
        .optional(),
    ];

    fn check(&self) -> Result<(), Bad> {
        check_id("id", &self.id)
    }
}

/// `bundle update ID --for-everyone [--yes]`: the app, for everyone, to
/// the newest version its catalog lists, as a task.
pub struct Update;

impl Action for Update {
    const ID: &'static str = "apps.update";
    const AREA: &'static str = AREA;
    const CHANGES: bool = true;
    const PATH: &'static str = "/api/1/apps/{id}/update";
    const RUNS: Runs = Runs::AsAdministrator;
    const LONG: bool = true;
    /// 1 is an update that stops to say what it asks for, which is the
    /// answer without `yes`.
    const ANSWERS: &'static [i32] = &[0, 1];
    const TESTS: Tests = UPDATE_TESTS;
    const TITLE: &'static str = "Update an app";
    type Args = Updating;

    fn argv(args: &Updating) -> Vec<String> {
        let mut argv = ["bundle", "update"].map(String::from).to_vec();
        argv.extend([args.id.clone(), "--for-everyone".to_string()]);
        if args.yes == Some(true) {
            argv.push("--yes".into());
        }
        argv
    }

    fn subject(args: &Updating) -> Option<String> {
        Some(args.id.clone())
    }

    fn probe(_: &Updating) -> Option<Vec<String>> {
        Some(list_argv())
    }

    fn probed(args: &Updating, output: &str) -> Option<String> {
        picked(&args.id, output)
    }

    fn examples() -> Examples<Updating> {
        Examples {
            good: vec![Updating { id: EXAMPLE.into(), yes: None }],
            bad: vec![
                json!({ "id": "../x" }),
                json!({ "id": "Notes" }),
                json!({ "id": EXAMPLE, "yes": "yes" }),
            ],
            // A rolled-back app is held, so that its way forward is a
            // release and an update: apps.unhold, then this.
            undo: Some(Undo { action: "apps.rollback", args: json!({ "id": EXAMPLE }) }),
        }
    }
}

/// `bundle update --for-everyone --yes`: every app everyone has, to the
/// newest version its catalog lists, as a task.
pub struct UpdateAll;

impl Action for UpdateAll {
    const ID: &'static str = "apps.update-all";
    const AREA: &'static str = AREA;
    const CHANGES: bool = true;
    const PATH: &'static str = "/api/1/apps/update";
    const RUNS: Runs = Runs::AsAdministrator;
    const LONG: bool = true;
    const TESTS: Tests = UPDATE_TESTS;
    const TITLE: &'static str = "Update every app";
    type Args = Nothing;

    fn argv(_: &Nothing) -> Vec<String> {
        ["bundle", "update", "--for-everyone", "--yes"].map(String::from).to_vec()
    }

    fn examples() -> Examples<Nothing> {
        Examples {
            good: vec![Nothing {}],
            bad: vec![json!({ "id": "org.example.notes" })],
            undo: None,
        }
    }
}

/// What `bundle list --json` says of app `id` that asking for tonight
/// moves: whether its update is asked for tonight, and what was asked;
/// `none` for an app that everyone has not got.
fn tonight_picked(id: &str, output: &str) -> Option<String> {
    Some(match row_of(id, output)? {
        Some(row) => {
            json!({ "tonight": row.get("tonight").cloned().unwrap_or(Value::Null) }).to_string()
        }
        None => "none".into(),
    })
}

/// `bundle update ID --for-everyone --tonight [--yes]`: the app's update,
/// for everyone, in tonight's window: the hour before the restart time of
/// `updates.toml`, else 2:00 to 3:00. An update that asks for more than
/// the version before stops with the sheet's words, as [`Update`] does,
/// and is queued with `yes`.
pub struct Tonight;

impl Action for Tonight {
    const ID: &'static str = "apps.tonight";
    const AREA: &'static str = AREA;
    const CHANGES: bool = true;
    const PATH: &'static str = "/api/1/apps/{id}/tonight";
    const RUNS: Runs = Runs::AsAdministrator;
    const LONG: bool = true;
    /// 1 is an update that stops to say what it asks for, as `apps.update`.
    const ANSWERS: &'static [i32] = &[0, 1];
    const TESTS: Tests = TESTS;
    const TITLE: &'static str = "Install an app's update tonight";
    type Args = Updating;

    fn argv(args: &Updating) -> Vec<String> {
        let mut argv = ["bundle", "update"].map(String::from).to_vec();
        argv.extend([args.id.clone(), "--for-everyone".to_string(), "--tonight".to_string()]);
        if args.yes == Some(true) {
            argv.push("--yes".into());
        }
        argv
    }

    fn subject(args: &Updating) -> Option<String> {
        Some(args.id.clone())
    }

    fn probe(_: &Updating) -> Option<Vec<String>> {
        Some(list_argv())
    }

    fn probed(args: &Updating, output: &str) -> Option<String> {
        tonight_picked(&args.id, output)
    }

    fn examples() -> Examples<Updating> {
        Examples {
            good: vec![Updating { id: EXAMPLE.into(), yes: None }],
            bad: vec![
                json!({ "id": "../x" }),
                json!({ "id": "Notes" }),
                json!({ "id": EXAMPLE, "yes": "yes" }),
            ],
            undo: Some(Undo { action: "apps.not-tonight", args: json!({ "id": EXAMPLE }) }),
        }
    }
}

/// What `bundle list --json` says of every app everyone has that asking
/// for tonight moves: each one's ID with what was asked, as text to
/// compare.
fn tonight_all_picked(output: &str) -> Option<String> {
    let document: Value = serde_json::from_str(output).ok()?;
    let mut asked = serde_json::Map::new();
    for row in document.get("bundles")?.as_array()? {
        if row.get("for_everyone").and_then(Value::as_bool) != Some(true) {
            continue;
        }
        let id = row.get("id")?.as_str()?;
        asked.insert(id.into(), row.get("tonight").cloned().unwrap_or(Value::Null));
    }
    Some(Value::Object(asked).to_string())
}

/// `bundle update --for-everyone --tonight --yes`: every app everyone has
/// that has an update, in tonight's window, taking what each asks for.
pub struct TonightAll;

impl Action for TonightAll {
    const ID: &'static str = "apps.tonight-all";
    const AREA: &'static str = AREA;
    const CHANGES: bool = true;
    const PATH: &'static str = "/api/1/apps/tonight";
    const RUNS: Runs = Runs::AsAdministrator;
    const TESTS: Tests = TESTS;
    const TITLE: &'static str = "Install every app's update tonight";
    type Args = Nothing;

    fn argv(_: &Nothing) -> Vec<String> {
        ["bundle", "update", "--for-everyone", "--tonight", "--yes"].map(String::from).to_vec()
    }

    fn probe(_: &Nothing) -> Option<Vec<String>> {
        Some(list_argv())
    }

    fn probed(_: &Nothing, output: &str) -> Option<String> {
        tonight_all_picked(output)
    }

    fn examples() -> Examples<Nothing> {
        Examples {
            good: vec![Nothing {}],
            bad: vec![json!({ "id": "org.example.notes" })],
            // Which apps it queued is in the history of the list: each is
            // taken off by name, with apps.not-tonight.
            undo: None,
        }
    }
}

/// `bundle update ID --for-everyone --not-tonight`: takes the app off
/// tonight's window.
pub struct NotTonight;

impl Action for NotTonight {
    const ID: &'static str = "apps.not-tonight";
    const AREA: &'static str = AREA;
    const CHANGES: bool = true;
    const PATH: &'static str = "/api/1/apps/{id}/not-tonight";
    const RUNS: Runs = Runs::AsAdministrator;
    const TESTS: Tests = TESTS;
    const TITLE: &'static str = "Do not install an app's update tonight";
    type Args = Named;

    fn argv(args: &Named) -> Vec<String> {
        ["bundle", "update"]
            .map(String::from)
            .into_iter()
            .chain([args.id.clone(), "--for-everyone".to_string(), "--not-tonight".to_string()])
            .collect()
    }

    fn subject(args: &Named) -> Option<String> {
        Some(args.id.clone())
    }

    fn probe(_: &Named) -> Option<Vec<String>> {
        Some(list_argv())
    }

    fn probed(args: &Named, output: &str) -> Option<String> {
        tonight_picked(&args.id, output)
    }

    /// Taken off when nothing is asked for it.
    fn settled(_: &Named, now: &str) -> Option<bool> {
        Some(said(now, "tonight")?.is_null())
    }

    fn examples() -> Examples<Named> {
        named_examples(Some(Undo { action: "apps.tonight", args: json!({ "id": EXAMPLE }) }))
    }
}

/// `bundle rollback ID --for-everyone`: the version the app had before its
/// last update, with the data it had then; bundled holds the app there.
pub struct Rollback;

impl Action for Rollback {
    const ID: &'static str = "apps.rollback";
    const AREA: &'static str = AREA;
    const CHANGES: bool = true;
    const PATH: &'static str = "/api/1/apps/{id}/rollback";
    const RUNS: Runs = Runs::AsAdministrator;
    const LONG: bool = true;
    const TESTS: Tests = UPDATE_TESTS;
    const TITLE: &'static str = "Go back to an app's version before";
    type Args = Named;

    fn argv(args: &Named) -> Vec<String> {
        vec!["bundle".into(), "rollback".into(), args.id.clone(), "--for-everyone".into()]
    }

    fn subject(args: &Named) -> Option<String> {
        Some(args.id.clone())
    }

    fn probe(_: &Named) -> Option<Vec<String>> {
        Some(list_argv())
    }

    fn probed(args: &Named, output: &str) -> Option<String> {
        picked(&args.id, output)
    }

    fn examples() -> Examples<Named> {
        named_examples(Some(Undo { action: "apps.update", args: json!({ "id": EXAMPLE }) }))
    }
}

/// `bundle hold ID --for-everyone [--why TEXT]`: updates leave the app at
/// the version it has.
pub struct Hold;

impl Action for Hold {
    const ID: &'static str = "apps.hold";
    const AREA: &'static str = AREA;
    const CHANGES: bool = true;
    const PATH: &'static str = "/api/1/apps/{id}/hold";
    const RUNS: Runs = Runs::AsAdministrator;
    const TESTS: Tests = TESTS;
    const TITLE: &'static str = "Hold an app back";
    type Args = Holding;

    fn argv(args: &Holding) -> Vec<String> {
        let mut argv =
            vec!["bundle".into(), "hold".into(), args.id.clone(), "--for-everyone".into()];
        if let Some(why) = &args.why {
            argv.extend(["--why".into(), why.clone()]);
        }
        argv
    }

    fn subject(args: &Holding) -> Option<String> {
        Some(args.id.clone())
    }

    fn probe(_: &Holding) -> Option<Vec<String>> {
        Some(list_argv())
    }

    fn probed(args: &Holding, output: &str) -> Option<String> {
        picked(&args.id, output)
    }

    /// Held, or not, as the change leaves it, when the app is there; and
    /// with the reason asked for, when one is.
    fn settled(args: &Holding, now: &str) -> Option<bool> {
        let held = held_in(now)?;
        let reason = said(now, "held_reason");
        Some(
            held && args
                .why
                .as_ref()
                .is_none_or(|why| reason.as_ref().and_then(Value::as_str) == Some(why)),
        )
    }

    fn examples() -> Examples<Holding> {
        let mut bad = named_examples(None).bad;
        bad.extend([
            json!({ "id": EXAMPLE, "why": "two\nlines" }),
            json!({ "id": EXAMPLE, "why": "x".repeat(201) }),
        ]);
        Examples {
            good: vec![Holding {
                id: EXAMPLE.into(),
                why: Some("app tests use this version".into()),
            }],
            bad,
            undo: Some(Undo { action: "apps.unhold", args: json!({ "id": EXAMPLE }) }),
        }
    }
}

/// `bundle unhold ID --for-everyone`: updates take the app again.
pub struct Unhold;

impl Action for Unhold {
    const ID: &'static str = "apps.unhold";
    const AREA: &'static str = AREA;
    const CHANGES: bool = true;
    const PATH: &'static str = "/api/1/apps/{id}/unhold";
    const RUNS: Runs = Runs::AsAdministrator;
    const TESTS: Tests = TESTS;
    const TITLE: &'static str = "Stop holding an app back";
    type Args = Named;

    fn argv(args: &Named) -> Vec<String> {
        vec!["bundle".into(), "unhold".into(), args.id.clone(), "--for-everyone".into()]
    }

    fn subject(args: &Named) -> Option<String> {
        Some(args.id.clone())
    }

    fn probe(_: &Named) -> Option<Vec<String>> {
        Some(list_argv())
    }

    fn probed(args: &Named, output: &str) -> Option<String> {
        picked(&args.id, output)
    }

    /// Held, or not, as the change leaves it, when the app is there.
    fn settled(_: &Named, now: &str) -> Option<bool> {
        held_in(now).map(|held| !held)
    }

    fn examples() -> Examples<Named> {
        named_examples(Some(Undo { action: "apps.hold", args: json!({ "id": EXAMPLE }) }))
    }
}

/// What a removal does with the app's data.
const DATA: &[&str] = &["keep", "purge"];

/// An app to remove, and what becomes of its data.
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Removing {
    pub id: String,
    /// `keep` or `purge`.
    pub data: String,
}

impl Args for Removing {
    const FIELDS: &'static [Field] = &[
        ID,
        Field::one_of(
            "data",
            "What becomes of the app's settings and data: keep them, or purge them with the app.",
            DATA,
        ),
    ];

    fn check(&self) -> Result<(), Bad> {
        check_id("id", &self.id)?;
        if !DATA.contains(&self.data.as_str()) {
            return Err(Bad {
                field: "data",
                why: "data is keep or purge. Nothing was removed.".into(),
            });
        }
        Ok(())
    }
}

/// `bundle remove ID --for-everyone (--keep-data | --purge)`: takes the
/// app away for everyone, with its data folder kept or gone, as a task.
/// A purge is what the page asks for the app's name to be typed for: the
/// playbook has to say it is meant too, in `dukovo_confirm_purge`.
pub struct Remove;

impl Action for Remove {
    const ID: &'static str = "apps.remove";
    const AREA: &'static str = AREA;
    const CHANGES: bool = true;
    const PATH: &'static str = "/api/1/apps/{id}/remove";
    const RUNS: Runs = Runs::AsAdministrator;
    const LONG: bool = true;
    const TESTS: Tests = TESTS;
    const TITLE: &'static str = "Remove an app";
    type Args = Removing;

    fn argv(args: &Removing) -> Vec<String> {
        let data = if args.data == "purge" { "--purge" } else { "--keep-data" };
        ["bundle", "remove"]
            .map(String::from)
            .into_iter()
            .chain([args.id.clone(), "--for-everyone".to_string(), data.to_string()])
            .collect()
    }

    fn subject(args: &Removing) -> Option<String> {
        Some(args.id.clone())
    }

    fn ansible(args: &Removing) -> Ansible {
        let task = Ansible::command(
            format!("{}: {}", Self::TITLE, args.id),
            &Self::argv(args),
            true,
            true,
        );
        if args.data == "purge" { task.when("dukovo_confirm_purge | bool") } else { task }
    }

    fn probe(_: &Removing) -> Option<Vec<String>> {
        Some(list_argv())
    }

    fn probed(args: &Removing, output: &str) -> Option<String> {
        picked(&args.id, output)
    }

    /// The app is gone when everyone's list has no row of it.
    fn settled(_: &Removing, now: &str) -> Option<bool> {
        Some(now == "none")
    }

    fn examples() -> Examples<Removing> {
        Examples {
            good: vec![Removing { id: EXAMPLE.into(), data: "keep".into() }],
            bad: vec![
                json!({ "id": EXAMPLE }),
                json!({ "id": EXAMPLE, "data": "some" }),
                json!({ "id": "../x", "data": "keep" }),
            ],
            // What is removed is installed again from its catalog, which
            // is the page's business.
            undo: None,
        }
    }
}

/// The most words a search takes.
const WORDS_MOST: usize = 5;

/// The most characters of a word of a search.
const WORD_MAX: usize = 64;

/// What the words of a search match, as the schema says it: none, or one
/// to five of one to 64 lowercase letters, digits, dots, `@` and hyphens,
/// each after a single space. Written as [`check_words`] enforces it.
const WORDS_PATTERN: &str = "^([a-z0-9.@-]{1,64}( [a-z0-9.@-]{1,64}){0,4})?$";

/// The words of a search, which the page lowercases and joins with
/// spaces.
const WORDS: Field = Field::text(
    "words",
    "What to look for in the apps' IDs and names: up to five words of lowercase letters, \
     digits, dots, @ and hyphens, separated by spaces. None lists every app.",
    0,
    WORDS_MOST * WORD_MAX + (WORDS_MOST - 1),
    Some(WORDS_PATTERN),
)
.optional();

/// Checks the words of a search: none, or up to [`WORDS_MOST`] of
/// [`WORDS_PATTERN`]'s characters, each after a single space.
fn check_words(words: &str) -> Result<(), Bad> {
    let one = |word: &str| {
        (1..=WORD_MAX).contains(&word.len())
            && word
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b".@-".contains(&b))
    };
    if words.is_empty() || (words.split(' ').count() <= WORDS_MOST && words.split(' ').all(one)) {
        return Ok(());
    }
    Err(Bad {
        field: "words",
        why: format!(
            "A search takes up to {WORDS_MOST} words of lowercase letters, digits, dots, @ and \
             hyphens, each after a single space. Nothing was searched."
        ),
    })
}

/// What a search looks for.
#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Searching {
    /// The words, or none for every app.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub words: Option<String>,
}

impl Args for Searching {
    const FIELDS: &'static [Field] = &[WORDS];

    fn check(&self) -> Result<(), Bad> {
        self.words.as_deref().map_or(Ok(()), check_words)
    }
}

/// `bundle search [WORD...] --json` as the person: the newest version of
/// each app the catalogs list, whose ID or name holds every word, with its
/// flaws and the version installed, and each catalog that could not be
/// read. When none could be read the command says so with its status, 1,
/// and its output is still the answer, which names them.
pub struct Search;

impl Action for Search {
    const ID: &'static str = "apps.search";
    const AREA: &'static str = AREA;
    const CHANGES: bool = false;
    const PATH: &'static str = "/api/1/apps/catalog/search";
    const RUNS: Runs = Runs::AsThePerson;
    const ERRORS: &'static [&'static str] = &["APP-103"];
    const ANSWERS: &'static [i32] = &[0, 1];
    const TESTS: Tests = CATALOG_TESTS;
    const TITLE: &'static str = "Search the catalogs' apps";
    type Args = Searching;

    /// The words come after `--`, so that none is read as an option.
    fn argv(args: &Searching) -> Vec<String> {
        let mut argv = ["bundle", "search", "--json"].map(String::from).to_vec();
        let words: Vec<&str> = args.words.iter().flat_map(|words| words.split(' ')).collect();
        if !words.is_empty() && words != [""] {
            argv.push("--".into());
            argv.extend(words.into_iter().map(String::from));
        }
        argv
    }

    fn examples() -> Examples<Searching> {
        Examples {
            good: vec![Searching { words: None }, Searching { words: Some("notes".into()) }],
            bad: vec![
                json!({ "words": "a b c d e f" }),
                json!({ "words": "../x" }),
                json!({ "words": "Notes" }),
                json!({ "words": "two  spaces" }),
                json!({ "words": ["notes"] }),
            ],
            undo: None,
        }
    }
}

/// `bundle sheet ID --json` as the person: the sheet of the newest version
/// of the app that the first catalog that lists it offers, as an install
/// shows it, with which file it is: nothing is installed. It fetches the
/// file, so a page asks for it only when a person opens an app.
pub struct Sheet;

impl Action for Sheet {
    const ID: &'static str = "apps.sheet";
    const AREA: &'static str = AREA;
    const CHANGES: bool = false;
    const PATH: &'static str = "/api/1/apps/catalog/{id}/sheet";
    const RUNS: Runs = Runs::AsThePerson;
    const ERRORS: &'static [&'static str] = &["APP-101", "APP-103"];
    const TESTS: Tests = CATALOG_TESTS;
    const TITLE: &'static str = "Read an app's sheet before it is installed";
    type Args = Named;

    fn argv(args: &Named) -> Vec<String> {
        vec!["bundle".into(), "sheet".into(), args.id.clone(), "--json".into()]
    }

    fn subject(args: &Named) -> Option<String> {
        Some(args.id.clone())
    }

    fn examples() -> Examples<Named> {
        named_examples(None)
    }
}

/// The SHA-256 of a file, as `bundle sheet --json` says it: 64 lowercase
/// hexadecimal digits.
const SHA256_PATTERN: &str = "^[0-9a-f]{64}$";

/// The hash of the file whose sheet was read: optional, 64 lowercase
/// hexadecimal digits.
const SHA256: Field = Field::text(
    "sha256",
    "The SHA-256 of the file whose sheet was read, as the sheet says it. The install is refused \
     when the catalog offers another file now.",
    64,
    64,
    Some(SHA256_PATTERN),
)
.optional();

/// An app to install from the catalogs, and the file whose sheet was read.
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Installing {
    pub id: String,
    /// The SHA-256 of the file the install may take, if it is said.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
}

impl Args for Installing {
    const FIELDS: &'static [Field] = &[ID, SHA256];

    fn check(&self) -> Result<(), Bad> {
        check_id("id", &self.id)?;
        let Some(sha256) = &self.sha256 else { return Ok(()) };
        if sha256.len() == 64
            && sha256.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Ok(());
        }
        Err(Bad {
            field: "sha256",
            why: "A file's SHA-256 is 64 lowercase hexadecimal digits, as its sheet says it. \
                  Nothing was installed."
                .into(),
        })
    }
}

/// `bundle install ID --for-everyone --yes [--sha256 HASH]`: the newest
/// version of the app that the catalogs list, for everyone, as a task; with
/// the hash of the sheet that was read, only that file.
pub struct Install;

impl Action for Install {
    const ID: &'static str = "apps.install";
    const AREA: &'static str = AREA;
    const CHANGES: bool = true;
    const PATH: &'static str = "/api/1/apps/catalog/{id}/install";
    const RUNS: Runs = Runs::AsAdministrator;
    const LONG: bool = true;
    const ERRORS: &'static [&'static str] = &["APP-101", "APP-102", "APP-103"];
    const TESTS: Tests = CATALOG_TESTS;
    const TITLE: &'static str = "Install an app from the catalogs";
    type Args = Installing;

    fn argv(args: &Installing) -> Vec<String> {
        let mut argv = ["bundle", "install"].map(String::from).to_vec();
        argv.extend([args.id.clone(), "--for-everyone".to_string(), "--yes".to_string()]);
        if let Some(sha256) = &args.sha256 {
            argv.extend(["--sha256".to_string(), sha256.clone()]);
        }
        argv
    }

    fn subject(args: &Installing) -> Option<String> {
        Some(args.id.clone())
    }

    /// Run again once the app is there, it does nothing: its folder is the
    /// sign that it was done.
    fn ansible(args: &Installing) -> Ansible {
        let mut task = Ansible::command(
            format!("{}: {}", Self::TITLE, args.id),
            &Self::argv(args),
            true,
            true,
        );
        task.params.insert("creates".into(), format!("{FOLDER}/{}", args.id).into());
        task
    }

    fn probe(_: &Installing) -> Option<Vec<String>> {
        Some(list_argv())
    }

    fn probed(args: &Installing, output: &str) -> Option<String> {
        picked(&args.id, output)
    }

    /// Installed already, whatever its version, when everyone's list has a
    /// row of it: the Ansible task is then skipped, and so is the install.
    fn settled(_: &Installing, now: &str) -> Option<bool> {
        Some(now != "none")
    }

    fn examples() -> Examples<Installing> {
        Examples {
            good: vec![Installing { id: EXAMPLE.into(), sha256: None }],
            bad: vec![
                json!({ "id": EXAMPLE, "sha256": "xyz" }),
                json!({ "id": EXAMPLE, "sha256": "AB".repeat(32) }),
                json!({ "id": "Notes" }),
                json!({ "id": "../x" }),
            ],
            // What was installed is taken away again, with the data the
            // app made kept.
            undo: Some(Undo {
                action: "apps.remove",
                args: json!({ "id": EXAMPLE, "data": "keep" }),
            }),
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

    /// What `bundle list --json` prints for an app that everyone has, held
    /// and not, and the same app of one person.
    const LIST: &str = r#"{
        "format": 1,
        "bundles": [
            {"id": "org.example.notes", "name": "notes", "version": "0.1.0", "for_everyone": false,
             "catalog": null, "held": true, "update": null,
             "flaws": {"state": "unknown", "count": 0, "worst": null}},
            {"id": "org.nginx.nginx", "name": "nginx", "version": "1.2.0", "for_everyone": true,
             "catalog": "dukovo", "held": false, "update": "1.3.0",
             "flaws": {"state": "known", "count": 2, "worst": "high"}},
            {"id": "org.nodejs.node@22", "name": "node", "version": "22.1.0", "for_everyone": true,
             "catalog": "dukovo", "held": true, "held_reason": "app tests use this version",
             "held_by": 1000, "held_since_ms": 1793100000000, "update": null,
             "flaws": {"state": "none", "count": 0, "worst": null}}
        ]
    }"#;

    #[test]
    fn the_list_and_the_sheet_are_the_commands_json_run_as_the_person() {
        for (id, path, argv, args) in [
            ("apps.list", "/api/1/apps", vec!["bundle", "list", "--json"], json!({})),
            (
                "apps.show",
                "/api/1/apps/{id}",
                vec!["bundle", "info", "org.nginx.nginx", "--json"],
                json!({ "id": "org.nginx.nginx" }),
            ),
        ] {
            let read = action(id);
            assert_eq!(
                (read.changes(), read.runs(), read.path(), read.long()),
                (false, Runs::AsThePerson, path, false),
                "{id}"
            );
            assert!(!read.posts() && !read.compares() && read.errors().is_empty(), "{id}");
            assert_eq!(read.answers(), [0], "{id}");
            let prepared = read.prepare(args).unwrap();
            assert_eq!(prepared.argv, argv, "{id}");
            assert!(prepared.files.is_empty() && prepared.probe.is_none(), "{id}");
            assert!(!prepared.ansible.become_root, "{id}");
        }
        assert_eq!(refused("apps.list", json!({ "all": true })).0, Kind::BadRequest);
        assert_eq!(refused("apps.show", json!({})).0, Kind::BadRequest);
    }

    #[test]
    fn an_id_is_the_rule_of_manifest_with_its_line_and_never_one_bare_word() {
        for id in [
            "../x",
            "Notes",
            "notes",
            "update",
            "org.Example.app",
            "org..app",
            "org.-app.x",
            "org.app-.x",
            "org.example.app@",
            "org.example.app@v22",
            "org.example.app@22.1.2.3",
            "org.example.app@022",
            "org.example.app@22@1",
            "org.example.a pp",
            "org.example.app\n",
            "",
        ] {
            let (kind, field, words) = refused("apps.show", json!({ "id": id }));
            assert_eq!((kind, field), (Kind::BadValue, Some("id")), "{id:?}");
            assert_eq!(
                words,
                "An app's ID is a reverse DNS name in lowercase, such as org.example.app, with \
                 its line after an @ when it has one, as org.nodejs.node@22. Nothing was done.",
                "{id:?}"
            );
        }
        for id in [
            "org.example.notes",
            "org.nodejs.node@22",
            "org.python.python@3.13",
            "org.example.app@0.0.1",
            "a.b",
            "io.github.sharkdp.bat",
            "org.my-app.x",
        ] {
            assert!(action("apps.show").prepare(json!({ "id": id })).is_ok(), "{id}");
        }
        // The pattern the schema carries is the one the check enforces.
        assert!(ID_PATTERN.starts_with("^[a-z0-9]") && ID_PATTERN.ends_with(")?$"));
        for id in ["apps.update", "apps.rollback", "apps.hold", "apps.unhold"] {
            assert_eq!(refused(id, json!({ "id": "update" })).1, Some("id"), "{id}");
        }
        assert_eq!(refused("apps.remove", json!({ "id": "update", "data": "keep" })).1, Some("id"));
    }

    #[test]
    fn an_update_stops_to_say_what_it_asks_for_unless_it_is_told_yes() {
        let update = action("apps.update");
        assert_eq!(
            (update.changes(), update.runs(), update.long(), update.path(), update.answers()),
            (true, Runs::AsAdministrator, true, "/api/1/apps/{id}/update", [0, 1].as_slice())
        );
        assert!(update.posts() && update.compares() && update.errors().is_empty());
        let without = update.prepare(json!({ "id": "org.example.notes" })).unwrap();
        assert_eq!(without.argv, ["bundle", "update", "org.example.notes", "--for-everyone"]);
        let with = update.prepare(json!({ "id": "org.example.notes", "yes": true })).unwrap();
        assert_eq!(with.argv, ["bundle", "update", "org.example.notes", "--for-everyone", "--yes"]);
        let no = update.prepare(json!({ "id": "org.example.notes", "yes": false })).unwrap();
        assert_eq!(no.argv, without.argv);
        for prepared in [&without, &with] {
            // Bundles are not under the settings, and a task runs no probe.
            assert!(prepared.files.is_empty());
            assert_eq!(
                prepared.probe.as_deref(),
                Some(&["bundle", "list", "--json"].map(String::from)[..])
            );
            assert!(prepared.ansible.become_root);
            // The task is not failed by the stop that says what is asked.
            assert_eq!(prepared.ansible.failed_when.as_deref(), Some("answer.rc not in [0, 1]"));
        }
        assert_eq!(
            refused("apps.update", json!({ "id": "org.example.notes", "yes": 1 })).0,
            Kind::BadRequest
        );
        let all = action("apps.update-all");
        assert_eq!(
            (all.changes(), all.runs(), all.long(), all.path(), all.answers(), all.compares()),
            (true, Runs::AsAdministrator, true, "/api/1/apps/update", [0].as_slice(), false)
        );
        let prepared = all.prepare(json!({})).unwrap();
        assert_eq!(prepared.argv, ["bundle", "update", "--for-everyone", "--yes"]);
        assert!(prepared.files.is_empty() && prepared.probe.is_none());
        assert_eq!(
            refused("apps.update-all", json!({ "id": "org.example.notes" })).0,
            Kind::BadRequest
        );
    }

    #[test]
    fn a_way_back_a_hold_and_a_release_are_each_the_others_undo() {
        let rollback = action("apps.rollback");
        assert_eq!(
            (rollback.changes(), rollback.runs(), rollback.long(), rollback.path()),
            (true, Runs::AsAdministrator, true, "/api/1/apps/{id}/rollback")
        );
        assert_eq!(
            rollback.prepare(json!({ "id": "org.nodejs.node@22" })).unwrap().argv,
            ["bundle", "rollback", "org.nodejs.node@22", "--for-everyone"]
        );
        for (id, word, other, path) in [
            ("apps.hold", "hold", "apps.unhold", "/api/1/apps/{id}/hold"),
            ("apps.unhold", "unhold", "apps.hold", "/api/1/apps/{id}/unhold"),
        ] {
            let held = action(id);
            assert_eq!(
                (held.changes(), held.runs(), held.long(), held.path(), held.compares()),
                (true, Runs::AsAdministrator, false, path, true),
                "{id}"
            );
            let prepared = held.prepare(json!({ "id": "org.example.notes" })).unwrap();
            assert_eq!(prepared.argv, ["bundle", word, "org.example.notes", "--for-everyone"]);
            assert!(prepared.files.is_empty() && prepared.probe.is_some(), "{id}");
            let (good, _, undo) = held.example_values();
            let wanted = if id == "apps.hold" {
                json!({ "id": "org.example.notes", "why": "app tests use this version" })
            } else {
                json!({ "id": "org.example.notes" })
            };
            assert_eq!(good, [wanted], "{id}");
            assert_eq!(
                undo,
                Some(Undo { action: other, args: json!({ "id": "org.example.notes" }) }),
                "{id}"
            );
        }
        // Rolled back, the app is held: the way forward is a release, then
        // an update, and the example's undo is the update.
        let (_, _, undo) = rollback.example_values();
        assert_eq!(undo.map(|undo| undo.action), Some("apps.update"));
        let (_, _, undo) = action("apps.update").example_values();
        assert_eq!(undo.map(|undo| undo.action), Some("apps.rollback"));
        assert_eq!(action("apps.update-all").example_values().2, None);
    }

    #[test]
    fn a_removal_says_what_becomes_of_the_data_and_a_purge_is_the_playbook_s_to_confirm() {
        let remove = action("apps.remove");
        assert_eq!(
            (remove.changes(), remove.runs(), remove.long(), remove.path(), remove.compares()),
            (true, Runs::AsAdministrator, true, "/api/1/apps/{id}/remove", true)
        );
        let keep = remove.prepare(json!({ "id": "org.example.notes", "data": "keep" })).unwrap();
        assert_eq!(
            keep.argv,
            ["bundle", "remove", "org.example.notes", "--for-everyone", "--keep-data"]
        );
        assert_eq!(keep.ansible.when, None);
        let purge = remove.prepare(json!({ "id": "org.example.notes", "data": "purge" })).unwrap();
        assert_eq!(
            purge.argv,
            ["bundle", "remove", "org.example.notes", "--for-everyone", "--purge"]
        );
        assert_eq!(purge.ansible.when.as_deref(), Some("dukovo_confirm_purge | bool"));
        let yaml = super::super::show::yaml(&purge.ansible);
        assert!(yaml.ends_with("  when: \"dukovo_confirm_purge | bool\"\n"), "{yaml}");
        assert!(!super::super::show::yaml(&keep.ansible).contains("when:"));
        for data in ["", "some", "KEEP", "keep ", "true"] {
            let (kind, field, _) =
                refused("apps.remove", json!({ "id": "org.example.notes", "data": data }));
            assert_eq!((kind, field), (Kind::BadValue, Some("data")), "{data:?}");
        }
        assert_eq!(
            refused("apps.remove", json!({ "id": "org.example.notes" })).0,
            Kind::BadRequest
        );
        // Nothing puts back what was taken away.
        assert_eq!(remove.example_values().2, None);
    }

    #[test]
    fn the_probe_picks_the_version_and_the_hold_of_the_app_everyone_has() {
        let args = json!({ "id": "org.nginx.nginx" });
        let picked = action("apps.hold").probed(&args, LIST).unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(&picked).unwrap(),
            json!({ "version": "1.2.0", "held": false, "held_reason": null })
        );
        // An update that is waiting is not part of it, nor the flaws.
        let later = LIST.replace("\"update\": \"1.3.0\"", "\"update\": \"1.4.0\"");
        assert_ne!(later, LIST);
        assert_eq!(action("apps.hold").probed(&args, &later), Some(picked.clone()));
        // One person's copy is not everyone's: the notes of the list are
        // that of one, so there is no row.
        let notes = json!({ "id": "org.example.notes" });
        assert_eq!(action("apps.hold").probed(&notes, LIST), Some("none".into()));
        // A line is an app of its own.
        let node = json!({ "id": "org.nodejs.node@22" });
        assert!(action("apps.update").probed(&node, LIST).unwrap().contains("22.1.0"));
        assert_eq!(
            action("apps.update").probed(&json!({ "id": "org.nodejs.node@20" }), LIST),
            Some("none".into())
        );
        for output in ["", "not json", "[]", r#"{"bundles": 3}"#, r#"{"format": 1}"#] {
            assert_eq!(action("apps.hold").probed(&args, output), None, "{output:?}");
        }
        assert_eq!(action("apps.hold").probed(&json!({ "id": 3 }), LIST), None);
    }

    #[test]
    fn a_hold_keeps_the_reason_it_is_given_as_one_line_of_at_most_200_characters() {
        let hold = action("apps.hold");
        for why in
            ["app tests use this version", "--force", "it's \"fine\" $(x) é", &"é".repeat(200)]
        {
            let prepared = hold.prepare(json!({ "id": "org.example.notes", "why": why })).unwrap();
            assert_eq!(
                prepared.argv,
                ["bundle", "hold", "org.example.notes", "--for-everyone", "--why", why],
                "{why:?}"
            );
        }
        // Without one, the command is as it was; a release takes none.
        let plain = hold.prepare(json!({ "id": "org.example.notes" })).unwrap();
        assert_eq!(plain.argv, ["bundle", "hold", "org.example.notes", "--for-everyone"]);
        let (kind, field, _) =
            refused("apps.unhold", json!({ "id": "org.example.notes", "why": "no" }));
        assert_eq!((kind, field), (Kind::BadRequest, None));
        for why in ["", "two\nlines", "a\ttab", "bell\u{7}", "del\u{7f}", &"x".repeat(201)] {
            let (kind, field, _) =
                refused("apps.hold", json!({ "id": "org.example.notes", "why": why }));
            assert_eq!((kind, field), (Kind::BadValue, Some("why")), "{why:?}");
        }
        // The probe says the reason, and a hold is settled only with it.
        let node = json!({ "id": "org.nodejs.node@22", "why": "app tests use this version" });
        let said = hold.probed(&node, LIST).unwrap();
        assert_eq!(hold.settled(&node, &said), Some(true), "{said}");
        let other = json!({ "id": "org.nodejs.node@22", "why": "Another reason" });
        assert_eq!(hold.settled(&other, &said), Some(false));
        let none = json!({ "id": "org.nodejs.node@22" });
        assert_eq!(hold.settled(&none, &said), Some(true));
    }

    #[test]
    fn check_mode_knows_a_hold_and_a_removal_that_are_made_and_cannot_tell_of_an_update() {
        let held = json!({ "version": "1.2.0", "held": true }).to_string();
        let free = json!({ "version": "1.2.0", "held": false }).to_string();
        let args = json!({ "id": "org.nginx.nginx" });
        assert_eq!(action("apps.hold").settled(&args, &held), Some(true));
        assert_eq!(action("apps.hold").settled(&args, &free), Some(false));
        assert_eq!(action("apps.unhold").settled(&args, &held), Some(false));
        assert_eq!(action("apps.unhold").settled(&args, &free), Some(true));
        // An app that is not there cannot be held or released.
        assert_eq!(action("apps.hold").settled(&args, "none"), None);
        let remove = json!({ "id": "org.nginx.nginx", "data": "keep" });
        assert_eq!(action("apps.remove").settled(&remove, "none"), Some(true));
        assert_eq!(action("apps.remove").settled(&remove, &free), Some(false));
        assert_eq!(action("apps.update").settled(&args, &free), None);
        assert_eq!(action("apps.rollback").settled(&args, &free), None);
    }

    #[test]
    fn the_routes_are_found_by_their_methods_and_update_is_no_app_s_id() {
        for (post, path, id) in [
            (false, "/api/1/apps", "apps.list"),
            (false, "/api/1/apps/org.example.notes", "apps.show"),
            (false, "/api/1/apps/org.nodejs.node@22", "apps.show"),
            (true, "/api/1/apps/update", "apps.update-all"),
            (true, "/api/1/apps/org.example.notes/update", "apps.update"),
            (true, "/api/1/apps/org.example.notes/rollback", "apps.rollback"),
            (true, "/api/1/apps/org.example.notes/hold", "apps.hold"),
            (true, "/api/1/apps/org.example.notes/unhold", "apps.unhold"),
            (true, "/api/1/apps/org.example.notes/remove", "apps.remove"),
        ] {
            let found = find(REGISTRY, post, path);
            assert!(
                matches!(&found, Found::Action(action, _) if action.id() == id),
                "{path}: {found:?}"
            );
        }
        let Found::Action(_, params) = find(REGISTRY, false, "/api/1/apps/org.nodejs.node@22")
        else {
            panic!()
        };
        assert_eq!(Value::Object(params), json!({ "id": "org.nodejs.node@22" }));
        // GET of the address that updates every app finds the sheet of an
        // app called update, which no ID is.
        assert_eq!(refused("apps.show", json!({ "id": "update" })).1, Some("id"));
        assert!(matches!(find(REGISTRY, true, "/api/1/apps"), Found::WrongMethod));
        assert!(matches!(
            find(REGISTRY, false, "/api/1/apps/org.example.notes/update"),
            Found::WrongMethod
        ));
    }

    #[test]
    fn a_token_needs_the_area_and_a_change_is_an_unlocked_administrator_s() {
        let call = |list: &str, id: &str| {
            let scopes = Scopes::parse(list).unwrap();
            authorize(&Caller::Token { name: "ci", scopes: &scopes }, action(id))
        };
        for id in ["apps.list", "apps.show"] {
            assert!(call("apps:read", id).is_ok(), "{id}");
            assert_eq!(call("updates", id).unwrap_err().kind, Kind::OutOfScope, "{id}");
        }
        for id in [
            "apps.update",
            "apps.update-all",
            "apps.rollback",
            "apps.hold",
            "apps.unhold",
            "apps.remove",
        ] {
            assert_eq!(call("apps:read", id).unwrap_err().kind, Kind::OutOfScope, "{id}");
            assert_eq!(call("updates", id).unwrap_err().kind, Kind::OutOfScope, "{id}");
            assert!(call("apps", id).is_ok(), "{id}");
            assert!(call("*", id).is_ok(), "{id}");
        }
        let session = |admin: bool, unlocked: bool, id: &str| {
            authorize(&Caller::Session { name: "ann", admin, unlocked }, action(id))
                .map_err(|f| f.kind)
        };
        for id in ["apps.list", "apps.show"] {
            assert_eq!(session(false, false, id), Ok(()), "{id}");
        }
        for id in ["apps.update", "apps.rollback", "apps.hold", "apps.remove"] {
            assert_eq!(session(false, true, id), Err(Kind::NotAdmin), "{id}");
            assert_eq!(session(true, false, id), Err(Kind::Locked), "{id}");
            assert_eq!(session(true, true, id), Ok(()), "{id}");
        }
    }

    /// What `bundle sheet ID --json` and the other reads of the catalogs
    /// are: the commands' JSON, run as the person.
    #[test]
    fn a_search_and_a_sheet_are_the_commands_json_run_as_the_person() {
        let search = action("apps.search");
        assert_eq!(
            (search.changes(), search.runs(), search.path(), search.long(), search.posts()),
            (false, Runs::AsThePerson, "/api/1/apps/catalog/search", false, false)
        );
        // 1 is no catalog that could be read, and the JSON says which.
        assert_eq!(
            (search.answers(), search.errors()),
            ([0, 1].as_slice(), ["APP-103"].as_slice())
        );
        assert!(!search.compares());
        let argv = |args: Value| search.prepare(args).unwrap().argv;
        assert_eq!(argv(json!({})), ["bundle", "search", "--json"]);
        assert_eq!(argv(json!({ "words": "" })), ["bundle", "search", "--json"]);
        assert_eq!(
            argv(json!({ "words": "notes" })),
            ["bundle", "search", "--json", "--", "notes"]
        );
        // A word is never an option, whatever it starts with.
        assert_eq!(
            argv(json!({ "words": "web-server -x @1 a.b" })),
            ["bundle", "search", "--json", "--", "web-server", "-x", "@1", "a.b"]
        );
        let five = "a b c d e";
        assert_eq!(argv(json!({ "words": five })).len(), 3 + 1 + 5);
        for words in [
            "a b c d e f",
            "../x",
            "Notes",
            "two  spaces",
            " leading",
            "trailing ",
            "tab\tword",
            "line\nbreak",
            "é",
            &"x".repeat(65),
        ] {
            let (kind, field, _) = refused("apps.search", json!({ "words": words }));
            assert_eq!((kind, field), (Kind::BadValue, Some("words")), "{words:?}");
        }
        assert!(search.prepare(json!({ "words": "x".repeat(64) })).is_ok());
        assert_eq!(refused("apps.search", json!({ "words": ["notes"] })).0, Kind::BadRequest);
        assert_eq!(refused("apps.search", json!({ "all": true })).0, Kind::BadRequest);
        // The pattern the schema carries is the one the check enforces.
        let pattern = WORDS_PATTERN;
        assert!(pattern.starts_with("^(") && pattern.ends_with(")?$"));
        assert_eq!(pattern.matches("{1,64}").count(), 2);
        assert!(pattern.contains(&format!("{{0,{}}}", WORDS_MOST - 1)));

        let sheet = action("apps.sheet");
        assert_eq!(
            (sheet.changes(), sheet.runs(), sheet.path(), sheet.long(), sheet.posts()),
            (false, Runs::AsThePerson, "/api/1/apps/catalog/{id}/sheet", false, false)
        );
        assert_eq!(
            (sheet.answers(), sheet.errors()),
            ([0].as_slice(), ["APP-101", "APP-103"].as_slice())
        );
        let prepared = sheet.prepare(json!({ "id": "org.nodejs.node@22" })).unwrap();
        assert_eq!(prepared.argv, ["bundle", "sheet", "org.nodejs.node@22", "--json"]);
        assert!(prepared.files.is_empty() && prepared.probe.is_none());
        assert!(!prepared.ansible.become_root);
        for id in ["Notes", "../x", "", "org.example.notes\n"] {
            let (kind, field, _) = refused("apps.sheet", json!({ "id": id }));
            assert_eq!((kind, field), (Kind::BadValue, Some("id")), "{id:?}");
        }
        assert_eq!(refused("apps.sheet", json!({})).0, Kind::BadRequest);
    }

    #[test]
    fn an_install_names_the_file_whose_sheet_was_read_and_is_a_task_that_leaves_the_app_there() {
        let install = action("apps.install");
        assert_eq!(
            (install.changes(), install.runs(), install.path(), install.long(), install.posts()),
            (true, Runs::AsAdministrator, "/api/1/apps/catalog/{id}/install", true, true)
        );
        assert_eq!(
            (install.answers(), install.errors()),
            ([0].as_slice(), ["APP-101", "APP-102", "APP-103"].as_slice())
        );
        assert!(install.compares());
        let without = install.prepare(json!({ "id": "org.example.notes" })).unwrap();
        assert_eq!(
            without.argv,
            ["bundle", "install", "org.example.notes", "--for-everyone", "--yes"]
        );
        let hash = "0123456789abcdef".repeat(4);
        let with = install.prepare(json!({ "id": "org.nodejs.node@22", "sha256": hash })).unwrap();
        assert_eq!(
            with.argv,
            [
                "bundle",
                "install",
                "org.nodejs.node@22",
                "--for-everyone",
                "--yes",
                "--sha256",
                &hash
            ]
        );
        // Bundles are not under the settings: no file, and a read of the list says what moved.
        for prepared in [&without, &with] {
            assert!(prepared.files.is_empty());
            assert_eq!(
                prepared.probe.as_deref(),
                Some(&["bundle", "list", "--json"].map(String::from)[..])
            );
        }
        // Ansible: the command, as root, that does nothing when the app's folder is there.
        assert!(with.ansible.become_root);
        assert_eq!(with.ansible.params["creates"], "/applications/org.nodejs.node@22");
        let yaml = super::super::show::yaml(&with.ansible);
        assert!(yaml.contains("    creates: \"/applications/org.nodejs.node@22\"\n"), "{yaml}");
        assert!(yaml.contains("  become: true\n"), "{yaml}");
        for hash in [
            "xyz",
            "",
            &"AB".repeat(32),
            &"a".repeat(63),
            &"a".repeat(65),
            &format!("{}g", "a".repeat(63)),
        ] {
            let (kind, field, _) =
                refused("apps.install", json!({ "id": "org.example.notes", "sha256": hash }));
            assert_eq!((kind, field), (Kind::BadValue, Some("sha256")), "{hash:?}");
        }
        assert_eq!(refused("apps.install", json!({ "id": "Notes" })).1, Some("id"));
        assert_eq!(
            refused("apps.install", json!({ "id": "org.example.notes", "yes": true })).0,
            Kind::BadRequest
        );
        assert_eq!(refused("apps.install", json!({})).0, Kind::BadRequest);
        // What the probe picks, and when the app is there already.
        let nginx = json!({ "id": "org.nginx.nginx" });
        let before = install.probed(&nginx, LIST).unwrap();
        assert!(before.contains("1.2.0"), "{before}");
        let notes = json!({ "id": "org.example.notes" });
        let none = install.probed(&notes, LIST).unwrap();
        assert_eq!(none, "none");
        assert_eq!(install.settled(&nginx, &before), Some(true));
        assert_eq!(install.settled(&notes, &none), Some(false));
        // Putting back is taking away, with the data kept.
        let (good, _, undo) = install.example_values();
        assert_eq!(good, [json!({ "id": "org.example.notes" })]);
        assert_eq!(
            undo,
            Some(Undo {
                action: "apps.remove",
                args: json!({ "id": "org.example.notes", "data": "keep" })
            })
        );
    }

    #[test]
    fn the_catalogs_routes_are_found_beside_the_apps_and_are_no_app_s() {
        for (post, path, id) in [
            (false, "/api/1/apps/catalog/search", "apps.search"),
            (false, "/api/1/apps/catalog/org.nginx.nginx/sheet", "apps.sheet"),
            (false, "/api/1/apps/catalog/org.nodejs.node@22/sheet", "apps.sheet"),
            (true, "/api/1/apps/catalog/org.nginx.nginx/install", "apps.install"),
            (false, "/api/1/apps/org.nginx.nginx", "apps.show"),
            (true, "/api/1/apps/org.nginx.nginx/update", "apps.update"),
        ] {
            let found = find(REGISTRY, post, path);
            assert!(
                matches!(&found, Found::Action(action, _) if action.id() == id),
                "{path}: {found:?}"
            );
        }
        let Found::Action(_, params) = find(REGISTRY, false, "/api/1/apps/catalog/org.a.b/sheet")
        else {
            panic!()
        };
        assert_eq!(Value::Object(params), json!({ "id": "org.a.b" }));
        assert!(matches!(find(REGISTRY, true, "/api/1/apps/catalog/search"), Found::WrongMethod));
        assert!(matches!(
            find(REGISTRY, false, "/api/1/apps/catalog/org.a.b/install"),
            Found::WrongMethod
        ));
        // `catalog` is one word, which no app's ID is.
        assert_eq!(refused("apps.show", json!({ "id": "catalog" })).1, Some("id"));
        // Searching goes by a query, as the reads that take arguments do.
        let search = action("apps.search");
        assert!(search.prepare(json!({ "words": "nginx web" })).is_ok());
    }

    #[test]
    fn a_search_and_a_sheet_are_a_persons_and_an_install_is_an_unlocked_administrators() {
        let call = |list: &str, id: &str| {
            let scopes = Scopes::parse(list).unwrap();
            authorize(&Caller::Token { name: "ci", scopes: &scopes }, action(id))
        };
        for id in ["apps.search", "apps.sheet"] {
            assert!(call("apps:read", id).is_ok(), "{id}");
            assert_eq!(call("updates", id).unwrap_err().kind, Kind::OutOfScope, "{id}");
        }
        assert_eq!(call("apps:read", "apps.install").unwrap_err().kind, Kind::OutOfScope);
        assert!(call("apps", "apps.install").is_ok());
        let session = |admin: bool, unlocked: bool, id: &str| {
            authorize(&Caller::Session { name: "ann", admin, unlocked }, action(id))
                .map_err(|f| f.kind)
        };
        for id in ["apps.search", "apps.sheet"] {
            assert_eq!(session(false, false, id), Ok(()), "{id}");
        }
        assert_eq!(session(false, true, "apps.install"), Err(Kind::NotAdmin));
        assert_eq!(session(true, false, "apps.install"), Err(Kind::Locked));
        assert_eq!(session(true, true, "apps.install"), Ok(()));
    }

    #[test]
    fn the_catalogs_examples_are_what_the_boot_and_lab_tests_run() {
        let (good, bad, undo) = action("apps.search").example_values();
        assert_eq!(good, [json!({}), json!({ "words": "notes" })]);
        assert_eq!(undo, None);
        for args in bad {
            assert!(action("apps.search").prepare(args.clone()).is_err(), "{args}");
        }
        let (good, bad, undo) = action("apps.sheet").example_values();
        assert_eq!(good, [json!({ "id": "org.example.notes" })]);
        assert_eq!(undo, None);
        for args in bad {
            assert!(action("apps.sheet").prepare(args.clone()).is_err(), "{args}");
        }
        let (_, bad, _) = action("apps.install").example_values();
        for args in bad {
            assert!(action("apps.install").prepare(args.clone()).is_err(), "{args}");
        }
    }

    #[test]
    fn the_examples_are_what_the_boot_and_lab_tests_run() {
        for id in ["apps.show", "apps.update", "apps.rollback", "apps.hold", "apps.unhold"] {
            let (good, bad, _) = action(id).example_values();
            let wanted = if id == "apps.hold" {
                json!({ "id": "org.example.notes", "why": "app tests use this version" })
            } else {
                json!({ "id": "org.example.notes" })
            };
            assert_eq!(good, [wanted], "{id}");
            assert!(!bad.is_empty(), "{id}");
            for args in bad {
                assert!(action(id).prepare(args.clone()).is_err(), "{id}: {args}");
            }
        }
        let (good, bad, undo) = action("apps.remove").example_values();
        assert_eq!(good, [json!({ "id": "org.example.notes", "data": "keep" })]);
        assert_eq!(undo, None);
        for args in bad {
            assert!(action("apps.remove").prepare(args.clone()).is_err(), "{args}");
        }
        let (good, bad, _) = action("apps.update-all").example_values();
        assert_eq!(good, [json!({})]);
        for args in bad {
            assert!(action("apps.update-all").prepare(args.clone()).is_err(), "{args}");
        }
    }
}
