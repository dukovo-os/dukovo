// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

use std::collections::HashSet;
use std::path::Path;

use serde::Deserialize;
use serde_json::json;

use super::*;
use crate::admind::run::error_id;

/// An action for the tests alone: restarting a lab service.
struct Restart;

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Named {
    name: String,
    #[serde(default)]
    wait: Option<u32>,
}

impl Args for Named {
    fn check(&self) -> Result<(), Bad> {
        if self.name.starts_with('-') {
            return Err(Bad { field: "name", why: "A name does not start with -.".into() });
        }
        if self.wait.is_some_and(|wait| wait > 60) {
            return Err(Bad { field: "wait", why: "Waits go up to 60 seconds.".into() });
        }
        Ok(())
    }
}

const TESTS: Tests = Tests {
    checks: "admind::actions::tests",
    route: "admin-actions",
    same_as_command: "admin-actions",
    browser: "web-console/actions",
    ansible: "web-console/ansible",
};

impl Action for Restart {
    const ID: &'static str = "lab.restart";
    const AREA: &'static str = "services";
    const CHANGES: bool = true;
    const PATH: &'static str = "/api/1/lab/{name}/restart";
    const RUNS: Runs = Runs::AsAdministrator;
    const ERRORS: &'static [&'static str] = &["SVC-104"];
    const TESTS: Tests = TESTS;
    const TITLE: &'static str = "Restart a lab service";
    type Args = Named;

    fn argv(args: &Named) -> Vec<String> {
        vec!["service".into(), args.name.clone(), "restart".into()]
    }

    fn subject(args: &Named) -> Option<String> {
        Some(args.name.clone())
    }

    fn examples() -> Examples<Named> {
        Examples {
            good: vec![Named { name: "lab-web".into(), wait: None }],
            bad: vec![],
            undo: None,
        }
    }
}

/// A read for the tests alone.
struct List;

impl Action for List {
    const ID: &'static str = "lab.list";
    const AREA: &'static str = "services";
    const CHANGES: bool = false;
    const PATH: &'static str = "/api/1/lab";
    const RUNS: Runs = Runs::AsThePerson;
    const TESTS: Tests = TESTS;
    const TITLE: &'static str = "List the lab services";
    type Args = Nothing;

    fn argv(_: &Nothing) -> Vec<String> {
        vec!["service".into(), "list".into(), "--json".into()]
    }

    fn examples() -> Examples<Nothing> {
        Examples { good: vec![Nothing {}], bad: vec![], undo: None }
    }
}

static RESTART: Entry<Restart> = Entry::new();
static LIST: Entry<List> = Entry::new();
/// These tests' actions, which the route tests use too.
pub static LAB: &[&dyn Registered] = &[&RESTART, &LIST];

#[test]
fn an_address_finds_its_action_and_gives_its_arguments() {
    let Found::Action(action, params) = find(LAB, true, "/api/1/lab/nginx/restart") else {
        panic!("not found");
    };
    assert_eq!(action.id(), "lab.restart");
    assert_eq!(params, json!({ "name": "nginx" }).as_object().unwrap().clone());
    let Found::Action(action, params) = find(LAB, false, "/api/1/lab") else { panic!() };
    assert_eq!(action.id(), "lab.list");
    assert!(params.is_empty());
}

#[test]
fn an_action_s_address_with_the_other_method_is_told_apart_from_none() {
    assert!(matches!(find(LAB, false, "/api/1/lab/nginx/restart"), Found::WrongMethod));
    assert!(matches!(find(LAB, true, "/api/1/lab"), Found::WrongMethod));
    for path in [
        "/api/1/lab/nginx",
        "/api/1/lab/nginx/restart/now",
        "/api/1/lab//restart",
        "/api/1/lab/nginx/restart/",
        "/api/1/labs",
        "/api/2/lab",
        "",
    ] {
        assert!(matches!(find(LAB, true, path), Found::None), "{path:?}");
    }
}

#[test]
fn an_argument_in_an_address_is_decoded_and_must_be_one_plain_part() {
    assert_eq!(decode("nginx"), Some("nginx".into()));
    assert_eq!(decode("my%20site"), Some("my site".into()));
    assert_eq!(decode("caf%C3%A9"), Some("café".into()));
    assert_eq!(decode(&"a".repeat(PART_MAX)), Some("a".repeat(PART_MAX)));
    for part in
        ["", "%", "%2", "%zz", "a%2Fb", "a%00b", "a%0Ab", "%C3", "%FF", &"a".repeat(PART_MAX + 1)]
    {
        assert_eq!(decode(part), None, "{part:?}");
    }
}

#[test]
fn arguments_come_from_the_address_and_the_body_and_never_both() {
    let params = json!({ "name": "nginx" }).as_object().unwrap().clone();
    assert_eq!(arguments(params.clone(), b"").unwrap(), json!({ "name": "nginx" }));
    assert_eq!(
        arguments(params.clone(), br#"{"wait": 5}"#).unwrap(),
        json!({ "name": "nginx", "wait": 5 })
    );
    for body in [&br#"{"name": "sshd"}"#[..], b"[1]", b"5", b"not json"] {
        let failure = arguments(params.clone(), body).unwrap_err();
        assert_eq!(failure.kind, Kind::BadRequest, "{}", String::from_utf8_lossy(body));
    }
}

#[test]
fn prepare_checks_the_arguments_and_makes_the_command() {
    let prepared = RESTART.prepare(json!({ "name": "nginx" })).unwrap();
    assert_eq!(prepared.argv, ["service", "nginx", "restart"]);
    assert_eq!(prepared.args, json!({ "name": "nginx", "wait": null }));
    assert_eq!(prepared.ansible.name, "Restart a lab service: nginx");
    assert_eq!(prepared.ansible.module, "ansible.builtin.command");
    assert_eq!(prepared.ansible.params["argv"], json!(["service", "nginx", "restart"]));
    assert!(prepared.ansible.become_root);
    assert_eq!(prepared.ansible.changed_when, None);
    assert!(prepared.files.is_empty(), "the action writes no file it names");

    let read = LIST.prepare(json!({})).unwrap();
    assert_eq!(read.ansible.name, "List the lab services");
    assert!(!read.ansible.become_root);
    assert_eq!(read.ansible.changed_when.as_deref(), Some("false"));
}

#[test]
fn prepare_refuses_what_the_action_does_not_take_and_values_that_cannot_work() {
    let wrong = RESTART.prepare(json!({ "name": "nginx", "force": true })).unwrap_err();
    assert_eq!(wrong.kind, Kind::BadRequest);
    let typed = RESTART.prepare(json!({ "name": 7 })).unwrap_err();
    assert_eq!(typed.kind, Kind::BadRequest);
    let bad = RESTART.prepare(json!({ "name": "-x" })).unwrap_err();
    assert_eq!((bad.kind, bad.field), (Kind::BadValue, Some("name")));
    assert_eq!(bad.body()["error"]["field"], "name");
    let bad = RESTART.prepare(json!({ "name": "nginx", "wait": 61 })).unwrap_err();
    assert_eq!((bad.kind, bad.field), (Kind::BadValue, Some("wait")));
    assert_eq!(bad.detail, "Waits go up to 60 seconds.");
}

#[test]
fn a_session_may_read_and_change_as_itself_and_needs_an_unlocked_admin_to_elevate() {
    let session = |admin, unlocked| Caller::Session { name: "alice", admin, unlocked };
    assert!(authorize(&session(false, false), &LIST).is_ok());
    assert!(authorize(&session(true, false), &LIST).is_ok());
    let not_admin = authorize(&session(false, true), &RESTART).unwrap_err();
    assert_eq!(not_admin.kind, Kind::NotAdmin);
    assert_eq!(not_admin.detail, "alice's account can look at services but not change it");
    let locked = authorize(&session(true, false), &RESTART).unwrap_err();
    assert_eq!(locked.kind, Kind::Locked);
    assert!(authorize(&session(true, true), &RESTART).is_ok());
}

#[test]
fn a_token_may_do_what_its_scopes_name() {
    let token = |list: &str| Scopes::parse(list).unwrap();
    let call = |scopes: &Scopes, action: &dyn Registered| {
        authorize(&Caller::Token { name: "ci", scopes }, action)
    };
    assert!(call(&token("*"), &RESTART).is_ok());
    assert!(call(&token("services"), &RESTART).is_ok());
    assert!(call(&token("services:read"), &LIST).is_ok());
    let read_only = call(&token("services:read"), &RESTART).unwrap_err();
    assert_eq!(read_only.kind, Kind::OutOfScope);
    assert_eq!(
        read_only.detail,
        "the API token \"ci\" may not do this: it needs the scope services or *"
    );
    let other = call(&token("firewall"), &LIST).unwrap_err();
    assert_eq!(other.kind, Kind::OutOfScope);
}

#[test]
fn the_list_says_each_action_s_method_and_address() {
    let listed = listed(LAB);
    assert_eq!(listed[0]["id"], "lab.restart");
    assert_eq!(listed[0]["method"], "POST");
    assert_eq!(listed[0]["path"], "/api/1/lab/{name}/restart");
    assert_eq!(listed[0]["runs"], "as-administrator");
    assert_eq!(listed[1]["method"], "GET");
    assert_eq!(listed[1]["runs"], "as-the-person");
    assert_eq!(
        listed[0]["tests"],
        json!({
            "checks": "admind::actions::tests",
            "route": "admin-actions",
            "same_as_command": "admin-actions",
            "browser": "web-console/actions",
            "ansible": "web-console/ansible",
        })
    );
}

#[test]
fn the_list_gives_each_action_s_examples_and_what_puts_back_the_first() {
    let lab = listed(LAB);
    assert_eq!(
        lab[0]["examples"],
        json!({ "good": [{ "name": "lab-web", "wait": null }], "undo": null })
    );
    assert_eq!(lab[1]["examples"], json!({ "good": [{}], "undo": null }));

    let console = listed(REGISTRY);
    let console = console.as_array().unwrap();
    let find = |id: &str| console.iter().find(|action| action["id"] == id).unwrap();
    assert_eq!(
        find("service.start")["examples"],
        json!({
            "good": [{ "name": "diskhealth" }],
            "undo": { "action": "service.stop", "args": { "name": "diskhealth" } },
        })
    );
    assert_eq!(find("service.restart")["examples"]["undo"], Value::Null);
    assert_eq!(find("service.list")["examples"], json!({ "good": [{}], "undo": null }));
    assert_eq!(
        find("certificate.show")["examples"],
        json!({ "good": [{ "name": "this-machine" }], "undo": null })
    );
    for action in console {
        assert!(action["examples"].as_object().is_some_and(|examples| examples.len() == 2));
        assert!(action["examples"].get("bad").is_none(), "the bad examples are the unit tests'");
    }
}

/// The rules every action of [`REGISTRY`] keeps, and these tests' own.
fn keeps_the_rules(actions: &'static [&'static dyn Registered]) {
    let mut ids = HashSet::new();
    let mut routes = HashSet::new();
    for action in actions {
        let id = action.id();
        assert!(ids.insert(id), "{id} is in the list twice");
        assert!(routes.insert((action.changes(), action.path())), "{id}'s route is another's");
        let (area, verb) = id.split_once('.').unwrap_or_else(|| panic!("{id} is area.verb"));
        assert!(!area.is_empty() && !verb.is_empty(), "{id}");
        assert!(crate::admind::scopes::area_name_ok(action.area()), "{id}'s area");
        assert!(action.path().starts_with("/api/1/"), "{id}'s address");
        assert!(
            action.runs() == Runs::AsThePerson || action.changes(),
            "{id}: a read runs as the person, never elevated"
        );
        for error in action.errors() {
            assert_eq!(error_id(&format!("({error})")), Some(*error), "{id}'s error {error}");
        }
        let tests = action.tests();
        for name in [tests.checks, tests.route, tests.same_as_command, tests.browser, tests.ansible]
        {
            assert!(!name.is_empty(), "{id} names each of its five tests");
        }
        assert!(!action.title().is_empty(), "{id}'s title");
        let (good, _, undo) = action.example_values();
        assert!(!good.is_empty(), "{id} has an example the tests can run");
        if let Some(undo) = &undo {
            assert!(action.changes(), "{id}: only a change is put back");
            let other = actions
                .iter()
                .find(|other| other.id() == undo.action)
                .unwrap_or_else(|| panic!("{id}'s undo, {}, is not in the list", undo.action));
            assert!(other.changes() && other.id() != id, "{id}'s undo is another change");
            assert!(other.prepare(undo.args.clone()).is_ok(), "{id}'s undo takes its arguments");
        }
        for args in &good {
            let prepared =
                action.prepare(args.clone()).unwrap_or_else(|f| panic!("{id}: {}", f.detail));
            assert!(!prepared.argv.is_empty());
            let in_address = action.path().split('/').filter_map(|part| {
                part.strip_prefix('{').and_then(|inner| inner.strip_suffix('}'))
            });
            for name in in_address {
                assert!(prepared.args.get(name).is_some(), "{id}: its address names {name}");
            }
            if !action.changes() {
                assert!(prepared.argv.iter().any(|w| w == "--json"), "{id}: a read asks for JSON");
            }
        }
    }
}

#[test]
fn every_action_keeps_the_rules() {
    keeps_the_rules(LAB);
    keeps_the_rules(REGISTRY);
}

/// The golden list of the actions, from the repository's root.
const GOLDEN: &str = "lab/actions.toml";

/// The comment `lab/actions.toml` starts with.
const GOLDEN_HEADER: &str = "\
# SPDX-FileCopyrightText: 2026 The Dukovo contributors
# SPDX-License-Identifier: Apache-2.0 OR MIT

# Every action of the web console, in the order of its registry
# (userspace/programs/systools/src/admind/actions), with the five tests
# that cover it: its host test of the arguments, the boot test that calls
# its route and the one that runs its shown command, the lab test that uses
# it in each browser, and the lab test that runs its Ansible task twice.
# `./ob lab coverage --check` fails when a test named here is not in the
# tree. The registry makes this file: change an action, then run
# `UPDATE_ACTIONS=1 ./ob test unit` to write it again.
";

/// The text of the golden list for `actions`: a `[[action]]` table each,
/// whose values are written as JSON writes them, which TOML reads the same.
fn golden(actions: &[&dyn Registered]) -> String {
    let mut text = GOLDEN_HEADER.to_string();
    for action in listed(actions).as_array().into_iter().flatten() {
        let tests = &action["tests"];
        text.push_str(&format!(
            "\n[[action]]\nid = {}\ntitle = {}\narea = {}\nmethod = {}\npath = {}\nruns = {}\n\
             long = {}\ntests = {{ checks = {}, route = {}, same_as_command = {}, browser = {}, \
             ansible = {} }}\n",
            action["id"],
            action["title"],
            action["area"],
            action["method"],
            action["path"],
            action["runs"],
            action["long"],
            tests["checks"],
            tests["route"],
            tests["same_as_command"],
            tests["browser"],
            tests["ansible"],
        ));
    }
    text
}

#[test]
fn actions_toml() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..").join(GOLDEN);
    let text = golden(REGISTRY);
    if std::env::var_os("UPDATE_ACTIONS").is_some_and(|value| value == "1") {
        std::fs::write(&path, &text).unwrap_or_else(|err| panic!("writing {GOLDEN}: {err}"));
        return;
    }
    let have = std::fs::read_to_string(&path).unwrap_or_default();
    assert!(
        have == text,
        "{GOLDEN} is not what the registry makes. Run UPDATE_ACTIONS=1 ./ob test unit to write \
         it, and commit it with the change."
    );
}

#[test]
fn the_golden_list_is_toml_that_says_what_the_registry_says() {
    let text = golden(REGISTRY);
    let table: toml::Table = text.parse().unwrap();
    let written = table["action"].as_array().unwrap();
    assert_eq!(written.len(), REGISTRY.len());
    for (action, table) in REGISTRY.iter().zip(written) {
        let tests = action.tests();
        assert_eq!(table["id"].as_str(), Some(action.id()));
        assert_eq!(table["title"].as_str(), Some(action.title()));
        assert_eq!(table["area"].as_str(), Some(action.area()));
        assert_eq!(table["method"].as_str(), Some(if action.changes() { "POST" } else { "GET" }));
        assert_eq!(table["path"].as_str(), Some(action.path()));
        assert_eq!(table["long"].as_bool(), Some(action.long()));
        let runs = serde_json::to_value(action.runs()).unwrap();
        assert_eq!(table["runs"].as_str(), runs.as_str());
        let named = table["tests"].as_table().unwrap();
        assert_eq!(named.len(), 5);
        assert_eq!(named["checks"].as_str(), Some(tests.checks));
        assert_eq!(named["route"].as_str(), Some(tests.route));
        assert_eq!(named["same_as_command"].as_str(), Some(tests.same_as_command));
        assert_eq!(named["browser"].as_str(), Some(tests.browser));
        assert_eq!(named["ansible"].as_str(), Some(tests.ansible));
    }
}
