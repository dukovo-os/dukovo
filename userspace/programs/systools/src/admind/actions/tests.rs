// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

use std::collections::HashSet;

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
    assert_eq!(prepared.ansible.module, "ansible.builtin.command");
    assert_eq!(prepared.ansible.params["argv"], json!(["service", "nginx", "restart"]));
    assert!(prepared.ansible.become_root);
    assert_eq!(prepared.ansible.changed_when, None);
    assert!(prepared.files.is_empty(), "the action writes no file it names");

    let read = LIST.prepare(json!({})).unwrap();
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
        for args in good.iter().chain(&undo) {
            let prepared =
                action.prepare(args.clone()).unwrap_or_else(|f| panic!("{id}: {}", f.detail));
            assert!(!prepared.argv.is_empty());
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
