// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The apps' actions (ADR 0515): the apps everyone has, one app's sheet,
//! and an update, a way back, a hold or a removal of one, as `bundle`
//! does them. The area is `apps`, the scope a token needs.
//!
//! An app is named by its ID as `manifest` checks it, with its line after
//! an `@` when it has one (`org.nodejs.node@22`). No ID is one bare word,
//! so `/api/1/apps/update`, the address that updates every app, is never
//! an app's.
//!
//! The list and the sheet are the person's own: what any account may see
//! of the apps. Every change is the machine's, made for everyone, which
//! is an administrator's: `--for-everyone`.
//!
//! The bundles live under `/applications`, which admind does not see, so
//! no change names a file and each says whether it changed anything from
//! a read of the list before and after it. A change that is a task runs no
//! probe: it always did something, and the probe is for the check that
//! asks whether it would.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

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

/// What the list says of app `id` that a change moves, its version and
/// whether it is held, as text to compare; `none` for an app that
/// everyone does not have.
pub(super) fn picked(id: &str, output: &str) -> Option<String> {
    Some(match row_of(id, output)? {
        Some(row) => {
            let member = |key: &str| row.get(key).cloned().unwrap_or(Value::Null);
            json!({ "version": member("version"), "held": member("held") }).to_string()
        }
        None => "none".into(),
    })
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
    const TESTS: Tests = TESTS;
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
    const TESTS: Tests = TESTS;
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
    const TESTS: Tests = TESTS;
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

/// `bundle hold ID --for-everyone`: updates leave the app at the
/// version it has.
pub struct Hold;

impl Action for Hold {
    const ID: &'static str = "apps.hold";
    const AREA: &'static str = AREA;
    const CHANGES: bool = true;
    const PATH: &'static str = "/api/1/apps/{id}/hold";
    const RUNS: Runs = Runs::AsAdministrator;
    const TESTS: Tests = TESTS;
    const TITLE: &'static str = "Hold an app back";
    type Args = Named;

    fn argv(args: &Named) -> Vec<String> {
        vec!["bundle".into(), "hold".into(), args.id.clone(), "--for-everyone".into()]
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
        held_in(now)
    }

    fn examples() -> Examples<Named> {
        named_examples(Some(Undo { action: "apps.unhold", args: json!({ "id": EXAMPLE }) }))
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
             "catalog": "dukovo", "held": true, "update": null,
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
            assert_eq!(good, [json!({ "id": "org.example.notes" })]);
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
            json!({ "version": "1.2.0", "held": false })
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

    #[test]
    fn the_examples_are_what_the_boot_and_lab_tests_run() {
        for id in ["apps.show", "apps.update", "apps.rollback", "apps.hold", "apps.unhold"] {
            let (good, bad, _) = action(id).example_values();
            assert_eq!(good, [json!({ "id": "org.example.notes" })], "{id}");
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
