// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

use std::collections::HashSet;
use std::net::IpAddr;
use std::path::Path;

use serde::Deserialize;
use serde_json::json;

use super::*;
use crate::admind::run::error_id;

pub mod secrets;
pub mod validate;

/// An action for the tests alone: restarting a lab service.
struct Restart;

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Named {
    name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    wait: Option<u32>,
}

impl Args for Named {
    const FIELDS: &'static [Field] = &[
        Field::text("name", "A lab service's name.", 0, 255, Some("^(?!-)")),
        Field::number("wait", "Seconds to wait, up to 60.", 0, 60).optional(),
    ];

    fn check(&self) -> Result<(), Bad> {
        if self.name.starts_with('-') {
            return Err(Bad { field: "name", why: "A name does not start with -.".into() });
        }
        if self.name.chars().count() > 255 {
            return Err(Bad { field: "name", why: "A name has up to 255 characters.".into() });
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

/// A change for the tests alone, which has a probe: `probe NAME`, which
/// prints what the change moves, a semicolon, and what moves by itself.
struct Set;

impl Action for Set {
    const ID: &'static str = "lab.set";
    const AREA: &'static str = "services";
    const CHANGES: bool = true;
    const PATH: &'static str = "/api/1/lab/{name}/set";
    const RUNS: Runs = Runs::AsAdministrator;
    const ERRORS: &'static [&'static str] = &["SVC-104"];
    const TESTS: Tests = TESTS;
    const TITLE: &'static str = "Set a lab service";
    type Args = Named;

    fn argv(args: &Named) -> Vec<String> {
        vec!["service".into(), args.name.clone(), "set".into()]
    }

    fn subject(args: &Named) -> Option<String> {
        Some(args.name.clone())
    }

    fn probe(args: &Named) -> Option<Vec<String>> {
        Some(vec!["probe".into(), args.name.clone()])
    }

    fn probed(_: &Named, output: &str) -> Option<String> {
        output.split_once(';').map(|(moved, _)| moved.trim().to_string())
    }

    /// The change leaves what the probe reads `on`; any other word is
    /// none of the two it knows.
    fn settled(_: &Named, now: &str) -> Option<bool> {
        match now {
            "on" => Some(true),
            "off" => Some(false),
            _ => None,
        }
    }

    fn examples() -> Examples<Named> {
        Examples {
            good: vec![Named { name: "lab-web".into(), wait: None }],
            bad: vec![],
            undo: None,
        }
    }
}

/// A change for the tests alone that begins a trial, as the firewall's
/// rules do when they are tried for a window.
struct Try;

impl Action for Try {
    const ID: &'static str = "lab.try";
    const AREA: &'static str = "services";
    const CHANGES: bool = true;
    const PATH: &'static str = "/api/1/lab/{name}/try";
    const RUNS: Runs = Runs::AsAdministrator;
    const TESTS: Tests = TESTS;
    const TITLE: &'static str = "Try a lab service";
    type Args = Named;

    fn argv(args: &Named) -> Vec<String> {
        vec!["service".into(), args.name.clone(), "try".into()]
    }

    fn trial(_: &Named) -> Option<Trial> {
        Some(Trial::Begins("firewall"))
    }

    fn examples() -> Examples<Named> {
        Examples {
            good: vec![Named { name: "lab-web".into(), wait: None }],
            bad: vec![],
            undo: None,
        }
    }
}

/// A change for the tests alone that keeps what [`Try`] began.
struct Keep;

impl Action for Keep {
    const ID: &'static str = "lab.keep";
    const AREA: &'static str = "services";
    const CHANGES: bool = true;
    const PATH: &'static str = "/api/1/lab/{name}/keep";
    const RUNS: Runs = Runs::AsAdministrator;
    const TESTS: Tests = TESTS;
    const TITLE: &'static str = "Keep a lab service";
    type Args = Named;

    fn argv(args: &Named) -> Vec<String> {
        vec!["service".into(), args.name.clone(), "keep".into()]
    }

    fn trial(_: &Named) -> Option<Trial> {
        Some(Trial::Ends("firewall"))
    }

    fn examples() -> Examples<Named> {
        Examples {
            good: vec![Named { name: "lab-web".into(), wait: None }],
            bad: vec![],
            undo: None,
        }
    }
}

/// A read that runs as an administrator, takes its arguments from the
/// address's query, and answers in lines, as `logs.read-all` does.
struct Log;

#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct LogArgs {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    lines: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    service: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    kernel: bool,
}

impl Args for LogArgs {
    const FIELDS: &'static [Field] = &[
        Field::number("lines", "How many lines, up to 1000.", 1, 1000).optional(),
        Field::text("service", "A lab service's name.", 1, 64, None).optional(),
        Field::flag("kernel", "The machine's own lines.").optional(),
    ];

    fn check(&self) -> Result<(), Bad> {
        if self.lines.is_some_and(|lines| !(1..=1000).contains(&lines)) {
            return Err(Bad { field: "lines", why: "From 1 to 1000 lines.".into() });
        }
        if self.service.as_ref().is_some_and(|service| !(1..=64).contains(&service.chars().count()))
        {
            return Err(Bad { field: "service", why: "A name of 1 to 64 characters.".into() });
        }
        if self.service.is_some() && self.kernel {
            return Err(Bad { field: "kernel", why: "A service or the machine, not both.".into() });
        }
        Ok(())
    }
}

impl Action for Log {
    const ID: &'static str = "lab.log";
    const AREA: &'static str = "logs";
    const CHANGES: bool = false;
    const PATH: &'static str = "/api/1/lab/log";
    const RUNS: Runs = Runs::AsAdministrator;
    const LINES: bool = true;
    const TESTS: Tests = TESTS;
    const TITLE: &'static str = "Read the lab log";
    type Args = LogArgs;

    fn argv(args: &LogArgs) -> Vec<String> {
        let lines = args.lines.unwrap_or(200).to_string();
        let mut argv = ["lab-log", "--output", "json", "--lines"].map(String::from).to_vec();
        argv.push(lines);
        if let Some(service) = &args.service {
            argv.extend(["--service".into(), service.clone()]);
        }
        if args.kernel {
            argv.push("--kernel".into());
        }
        argv
    }

    fn examples() -> Examples<LogArgs> {
        Examples {
            // The first takes each field's values alone: with a service,
            // `kernel` would be refused for the pair.
            good: vec![
                LogArgs { lines: Some(5), service: None, kernel: false },
                LogArgs { lines: Some(5), service: Some("nginx".into()), kernel: false },
            ],
            bad: vec![json!({ "lines": 0 }), json!({ "service": "nginx", "kernel": true })],
            undo: None,
        }
    }
}

static RESTART: Entry<Restart> = Entry::new();
static LIST: Entry<List> = Entry::new();
static SET: Entry<Set> = Entry::new();
/// The lab's long action, whose own module is also in lab builds.
static LONG: Entry<lab::Long> = Entry::new();
/// The lab's action whose command is not there, likewise.
static MISSING: Entry<lab::Missing> = Entry::new();
static LOG: Entry<Log> = Entry::new();
/// These tests' actions, which the route tests use too.
pub static LAB: &[&dyn Registered] = &[&RESTART, &LIST, &LONG, &SET, &MISSING, &LOG];
static TRY: Entry<Try> = Entry::new();
static KEEP: Entry<Keep> = Entry::new();
/// The actions that begin and end a trial, for the route tests that watch
/// the trials told; apart from [`LAB`], whose tests count its actions.
pub static TRYING: &[&dyn Registered] = &[&TRY, &KEEP];

/// A change for the tests alone that can cut off the browsers that come by
/// `lo`, as `private.remove` can those that come by its WireGuard
/// interface (ADR 0562): its read, `lab-peers`, prints each lab device and
/// its addresses as `wireguard show --json` does, and fails for the device
/// `broken`.
struct Cut;

/// The devices `lab-peers` prints in the route tests: `phone`, whose
/// address is the tests' browser's, and `laptop`, another.
pub const LAB_PEERS: &str = r#"{"devices": [
    {"name": "phone", "addresses": ["192.0.2.7/32"]},
    {"name": "laptop", "addresses": ["198.51.100.4/32", "2001:db8::4/128"]}
]}"#;

impl Action for Cut {
    const ID: &'static str = "lab.cut";
    const AREA: &'static str = "services";
    const CHANGES: bool = true;
    const PATH: &'static str = "/api/1/lab/{name}/cut";
    const RUNS: Runs = Runs::AsAdministrator;
    const CUTS_OFF: Option<CutsOff<Named>> = Some(CutsOff {
        on: Through::Card("lo"),
        read: |args| vec!["lab-peers".into(), args.name.clone()],
        cuts: |args, browser, shown| {
            let from = browser.from;
            let shown: Value = serde_json::from_str(shown).map_err(|_| "no list".to_string())?;
            let devices = shown["devices"].as_array().ok_or("no devices")?;
            let device = devices.iter().find(|device| device["name"] == args.name.as_str());
            let addresses = device.and_then(|device| device["addresses"].as_array());
            let holds = |address: &Value| {
                let address = address.as_str().and_then(|text| text.split('/').next());
                address.and_then(|text| text.parse::<IpAddr>().ok()) == Some(from)
            };
            if addresses.is_some_and(|addresses| addresses.iter().any(holds)) {
                return Err(format!("this browser reaches the console through {}", args.name));
            }
            Ok(())
        },
    });
    const TESTS: Tests = TESTS;
    const TITLE: &'static str = "Remove a lab device";
    type Args = Named;

    fn argv(args: &Named) -> Vec<String> {
        vec!["lab-cut".into(), args.name.clone()]
    }

    fn examples() -> Examples<Named> {
        Examples {
            good: vec![Named { name: "laptop".into(), wait: None }],
            bad: vec![],
            undo: None,
        }
    }
}

static CUT: Entry<Cut> = Entry::new();
/// The action that can cut off its browser, for the route tests of the
/// guard.
pub static CUTTING: &[&dyn Registered] = &[&CUT, &RESTART];
static SERVE: Entry<super::console::SetCertificate> = Entry::new();
/// The console's certificate, whose guard is by the browser's name, for
/// the route tests of that guard.
pub static NAMING: &[&dyn Registered] = &[&SERVE];

#[test]
fn what_can_cut_off_its_browser_names_its_card_and_its_read_and_fails_closed() {
    keeps_the_rules(CUTTING);
    let listed = listed(CUTTING);
    assert_eq!((&listed[0]["cuts_off"], &listed[1]["cuts_off"]), (&json!("lo"), &Value::Null));
    let prepared = CUT.prepare(json!({ "name": "phone" })).unwrap();
    let read = prepared.cuts_off.as_ref().unwrap();
    assert_eq!(
        (read.on, &read.argv),
        (Through::Card("lo"), &vec!["lab-peers".to_string(), "phone".into()])
    );
    assert!(RESTART.prepare(json!({ "name": "nginx" })).unwrap().cuts_off.is_none());

    let phone: IpAddr = "192.0.2.7".parse().unwrap();
    let cut = |name: &str, from: IpAddr, shown: &str| {
        let browser = Browser { from, host: "lab.example.org" };
        CUT.cuts_off(&CUT.prepare(json!({ "name": name })).unwrap().args, &browser, shown)
    };
    // The browser's own device is refused; another, or one not there, is
    // not; nor is any for a browser that holds none of their addresses.
    let refused = cut("phone", phone, LAB_PEERS).unwrap_err();
    assert_eq!(refused, "this browser reaches the console through phone");
    assert_eq!(cut("laptop", phone, LAB_PEERS), Ok(()));
    assert_eq!(cut("tablet", phone, LAB_PEERS), Ok(()));
    assert!(cut("laptop", "2001:db8::4".parse().unwrap(), LAB_PEERS).is_err());
    assert_eq!(cut("phone", "198.51.100.9".parse().unwrap(), LAB_PEERS), Ok(()));
    // What does not read refuses, and so do arguments that do not.
    assert!(cut("laptop", phone, "").is_err());
    assert!(cut("laptop", phone, "{}").is_err());
    let browser = Browser { from: phone, host: "lab.example.org" };
    assert!(CUT.cuts_off(&json!({ "name": 3 }), &browser, LAB_PEERS).is_err());
    // An action with no guard never refuses.
    assert_eq!(RESTART.cuts_off(&json!({ "name": "nginx" }), &browser, ""), Ok(()));
}

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
    assert_eq!(prepared.args, json!({ "name": "nginx" }));
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
fn the_list_says_which_changes_compare_a_probe_and_which_always_changed() {
    let lab = listed(LAB);
    let compares: Vec<(&str, &Value)> = lab
        .as_array()
        .unwrap()
        .iter()
        .map(|a| (a["id"].as_str().unwrap(), &a["compares"]))
        .collect();
    assert_eq!(
        compares,
        [
            ("lab.restart", &json!(false)),
            ("lab.list", &json!(false)),
            ("lab.long", &json!(false)),
            ("lab.set", &json!(true)),
            ("lab.missing", &json!(false)),
            ("lab.log", &json!(false))
        ]
    );
    let console = listed(REGISTRY);
    let comparing: Vec<&str> = console
        .as_array()
        .unwrap()
        .iter()
        .filter(|action| action["compares"] == true)
        .map(|action| action["id"].as_str().unwrap())
        .collect();
    assert_eq!(
        comparing,
        [
            "service.start",
            "service.stop",
            "service.enable",
            "service.disable",
            "certificate.get",
            "certificate.add",
            "machine.rename",
            "console.reach",
            "console.certificate",
            "console.undo",
            "console.export",
            "network.set",
            "network.undo",
            "firewall.add",
            "firewall.replace",
            "firewall.remove",
            "firewall.undo",
            "storage.grow",
            "storage.log-set",
            "backups.add",
            "backups.remove",
            "accounts.add",
            "accounts.set",
            "accounts.remove",
            "account.name",
            "account.key-add",
            "account.key-remove",
            "security.sign-in-set",
            "security.never-block",
            "security.never-block-remove",
            "ssh.set",
            "ssh.on",
            "ssh.off",
            "ssh.undo",
            "power.restart",
            "power.shutdown",
            "power.cancel",
            "updates.bisect",
            "updates.bisect-stop",
            "updates.bisect-confirm",
            "updates.set",
            "updates.channel",
            "updates.check-add",
            "updates.check-remove",
            "updates.essential",
            "updates.unstage",
            "updates.undo",
            "updates.hold",
            "updates.unhold",
            "apps.update",
            "apps.rollback",
            "apps.hold",
            "apps.unhold",
            "apps.remove",
        ]
    );
    for action in console.as_array().unwrap() {
        assert!(action["compares"].is_boolean(), "{}", action["id"]);
        if action["changes"] == false {
            assert_eq!(action["compares"], false, "{}: a read has no `changed`", action["id"]);
        }
    }
}

#[test]
fn what_is_never_done_to_the_callers_own_account_names_a_required_field_of_a_change() {
    for (action, listed) in REGISTRY.iter().zip(listed(REGISTRY).as_array().unwrap()) {
        assert_eq!(listed["not_self"], json!(action.not_self()), "{}", action.id());
        let Some(field) = action.not_self() else { continue };
        let taken = action.fields().iter().find(|taken| taken.name == field);
        assert!(taken.is_some_and(|taken| taken.required), "{}: {field}", action.id());
        assert!(action.changes(), "{}: a read does nothing to an account", action.id());
    }
    // An action that names none is never refused for it.
    assert_eq!(RESTART.prepare(json!({ "name": "nginx" })).unwrap().not_self, None);
}

#[test]
fn a_probe_is_the_command_that_reads_what_a_change_moves_and_the_action_picks_from_it() {
    let prepared = SET.prepare(json!({ "name": "nginx" })).unwrap();
    assert_eq!(prepared.probe, Some(vec!["probe".to_string(), "nginx".to_string()]));
    assert!(SET.compares());
    assert_eq!(SET.probed(&prepared.args, "on; 12\n").as_deref(), Some("on"));
    assert_eq!(SET.probed(&prepared.args, "on").as_deref(), None);
    // Without a probe a change has no command to read, and the default
    // picks the whole output, trimmed.
    let restart = RESTART.prepare(json!({ "name": "nginx" })).unwrap();
    assert_eq!(restart.probe, None);
    assert!(!RESTART.compares());
    assert_eq!(RESTART.probed(&restart.args, "  on; 3 \n").as_deref(), Some("on; 3"));
    // A read has none either.
    assert_eq!(LIST.prepare(json!({})).unwrap().probe, None);
    assert!(!LIST.compares());
}

#[test]
fn the_list_gives_each_action_s_examples_and_what_puts_back_the_first() {
    let lab = listed(LAB);
    assert_eq!(lab[0]["examples"], json!({ "good": [{ "name": "lab-web" }], "undo": null }));
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
    /// The places of the tree that admind's service sees, where the
    /// journal can tell a file that changed.
    const SEEN: &[&str] = &["/system/settings/", "/system/state/admind/"];
    let mut unseen = Vec::new();
    let mut ids = HashSet::new();
    let mut routes = HashSet::new();
    for action in actions {
        let id = action.id();
        assert!(ids.insert(id), "{id} is in the list twice");
        assert!(routes.insert((action.posts(), action.path())), "{id}'s route is another's");
        let (area, verb) = id.split_once('.').unwrap_or_else(|| panic!("{id} is area.verb"));
        assert!(!area.is_empty() && !verb.is_empty(), "{id}");
        assert!(crate::admind::scopes::area_name_ok(action.area()), "{id}'s area");
        assert!(action.path().starts_with("/api/1/"), "{id}'s address");
        // A read runs as the person, or as an administrator when its
        // command needs root (ADR 0518).
        assert!(
            !action.lines() || !action.changes(),
            "{id}: only a read answers in lines; a change answers its output"
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
        assert!(
            !action.long() || (action.changes() && action.runs() == Runs::AsAdministrator),
            "{id}: a long action is a change that runs as an administrator, which is all a task \
             runs as"
        );
        // A change that could cut off its browser is refused before it
        // runs, which a task, started first and answered later, is not.
        assert!(
            action.cuts_off_on().is_none()
                || (action.changes() && action.runs() == Runs::AsAdministrator && !action.long()),
            "{id}: what can cut off its browser is a change an administrator runs at once"
        );
        // A change that restarts the machine is refused while a trial or
        // the other kind of restart runs, which authd's administrator reads.
        assert!(
            action.restarts().is_none()
                || (action.changes() && action.runs() == Runs::AsAdministrator),
            "{id}: what restarts the machine is a change an administrator runs"
        );
        secrets::keeps_secrets(*action);
        let (good, _, undo) = action.example_values();
        assert!(!good.is_empty(), "{id} has an example the tests can run");
        if let Some(undo) = &undo {
            assert!(action.changes(), "{id}: only a change is put back");
            let other = actions
                .iter()
                .find(|other| other.id() == undo.action)
                .unwrap_or_else(|| panic!("{id}'s undo, {}, is not in the list", undo.action));
            // An action that sets one value may be its own undo, with the
            // value the machine starts with: console.reach.
            assert!(other.changes(), "{id}'s undo is a change");
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
                let argv = &prepared.argv;
                // A command that prints a JSON value a line says so with
                // `--output json`, as log does, or `--json`, as alerts does.
                let json = argv.iter().any(|w| w == "--json")
                    || action.lines()
                        && argv.windows(2).any(|pair| pair[0] == "--output" && pair[1] == "json");
                assert!(json, "{id}: a read asks for JSON, or for JSON lines");
            }
            // An action has a probe, or names the files it writes, for every
            // example or for none, which `compares` says, and a read has
            // neither.
            let compared = prepared.probe.is_some() || !prepared.files.is_empty();
            assert_eq!(compared, action.compares(), "{id}: its probe and files");
            // admind's service sees the settings and its own folders, and no
            // home or other service's state (service.toml(5)): the journal
            // would find any other file absent, before a change and after
            // it, and the change would say that it changed nothing.
            for file in &prepared.files {
                if !SEEN.iter().any(|place| file.starts_with(place)) {
                    unseen.push(format!("{id}: {file}"));
                }
            }
            if let Some(probe) = &prepared.probe {
                assert!(!probe.is_empty() && action.changes(), "{id}: its probe");
            }
        }
    }
    assert!(unseen.is_empty(), "files that admind cannot see:\n{}", unseen.join("\n"));
}

#[test]
fn every_action_keeps_the_rules() {
    keeps_the_rules(LAB);
    keeps_the_rules(secrets::SECRETS);
    keeps_the_rules(secrets::DOCUMENTS);
    keeps_the_rules(REGISTRY);
}

/// The first heading of the error page `id`, if the page is there:
/// `# ID: The title`.
fn error_page_heading(id: &str) -> Option<String> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../docs/errors");
    let text = std::fs::read_to_string(path.join(format!("{id}.md"))).ok()?;
    text.lines().find_map(|line| line.strip_prefix("# ")).map(str::to_string)
}

#[test]
fn every_error_an_action_names_has_a_page_whose_heading_starts_with_its_id() {
    for action in offered() {
        for id in action.errors() {
            let heading = error_page_heading(id).unwrap_or_else(|| {
                panic!("{}'s error {id} has no page in docs/errors", action.id())
            });
            assert!(heading.starts_with(&format!("{id}: ")), "{id}'s heading: {heading}");
        }
    }
    // The check finds a page that is there and one that is not.
    assert!(error_page_heading("ADM-101").is_some());
    assert!(error_page_heading("NOPE-999").is_none());
}

#[test]
fn every_error_the_console_gives_has_a_page_titled_as_the_console_says() {
    for kind in crate::admind::errors::Kind::ALL {
        let heading = error_page_heading(kind.id())
            .unwrap_or_else(|| panic!("{} has no page in docs/errors", kind.id()));
        assert_eq!(heading, format!("{}: {}", kind.id(), kind.title()));
    }
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
            "\n[[action]]\nid = {}\ntitle = {}\narea = {}\nchanges = {}\nmethod = {}\npath = {}\n\
             runs = {}\n\
             long = {}\ncompares = {}\ntests = {{ checks = {}, route = {}, same_as_command = {}, browser = {}, \
             ansible = {} }}\n",
            action["id"],
            action["title"],
            action["area"],
            action["changes"],
            action["method"],
            action["path"],
            action["runs"],
            action["long"],
            action["compares"],
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
        assert_eq!(table["changes"].as_bool(), Some(action.changes()));
        assert_eq!(table["method"].as_str(), Some(if action.posts() { "POST" } else { "GET" }));
        assert_eq!(table["path"].as_str(), Some(action.path()));
        assert_eq!(table["long"].as_bool(), Some(action.long()));
        assert_eq!(table["compares"].as_bool(), Some(action.compares()));
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

/// The schema of an action's input, from the list.
fn input_of(actions: &[&dyn Registered], id: &str) -> Value {
    let listed = listed(actions);
    let action = listed.as_array().unwrap().iter().find(|action| action["id"] == id);
    action.unwrap_or_else(|| panic!("no {id}"))["input"].clone()
}

#[test]
fn a_schema_says_each_kind_of_field_and_which_are_required() {
    let fields = [
        Field::text("name", "A name.", 1, 23, Some("^[a-z]+$")),
        Field::text("note", "A note.", 0, 80, None).optional(),
        Field::one_of("reach", "Whom.", &["private", "any"]),
        Field::number("port", "A port.", 1, 65535),
        Field::flag("force", "Whether to."),
    ];
    assert_eq!(
        schema(&fields),
        json!({
            "type": "object",
            "properties": {
                "name": {
                    "type": "string", "description": "A name.",
                    "minLength": 1, "maxLength": 23, "pattern": "^[a-z]+$",
                },
                "note": { "type": "string", "description": "A note.", "minLength": 0, "maxLength": 80 },
                "reach": { "type": "string", "description": "Whom.", "enum": ["private", "any"] },
                "port": { "type": "integer", "description": "A port.", "minimum": 1, "maximum": 65535 },
                "force": { "type": "boolean", "description": "Whether to." },
            },
            "required": ["name", "reach", "port", "force"],
            "additionalProperties": false,
        })
    );
    // An action that takes nothing takes an empty object.
    assert_eq!(
        schema(&[]),
        json!({ "type": "object", "properties": {}, "required": [], "additionalProperties": false })
    );
    // A field is required unless it says otherwise, and says so once.
    assert!(fields[0].required && !fields[1].required);
    assert_eq!(fields[1].optional(), fields[1]);
}

#[test]
fn what_a_schema_says_is_held_to_by_the_validator_for_each_kind() {
    let fields = [
        Field::text("name", "A name.", 1, 5, Some("^[a-z]+$")),
        Field::one_of("reach", "Whom.", &["private", "any"]),
        Field::number("port", "A port.", 1, 65535),
        Field::flag("force", "Whether to."),
        Field::flag("quiet", "Whether not to.").optional(),
    ];
    let schema = schema(&fields);
    let good = json!({ "name": "abc", "reach": "any", "port": 80, "force": false });
    assert_eq!(validate::validate(&schema, &good), Ok(()));
    assert_eq!(
        validate::validate(
            &schema,
            &json!({ "quiet": true, "name": "a", "reach": "any", "port": 1, "force": true })
        ),
        Ok(())
    );
    let refused = |change: Value| {
        let mut args = good.clone();
        for (key, value) in change.as_object().unwrap() {
            args[key] = value.clone();
        }
        validate::validate(&schema, &args).unwrap_err()
    };
    assert!(refused(json!({ "name": "" })).contains("minLength"));
    assert!(refused(json!({ "name": "abcdef" })).contains("maxLength"));
    assert!(refused(json!({ "name": "ABC" })).contains("does not match"));
    assert!(refused(json!({ "reach": "all" })).contains("not one of the words"));
    assert!(refused(json!({ "port": 0 })).contains("minimum"));
    assert!(refused(json!({ "port": 65536 })).contains("maximum"));
    assert!(refused(json!({ "port": "80" })).contains("not a whole number"));
    assert!(refused(json!({ "force": "yes" })).contains("not true or false"));
    assert!(refused(json!({ "more": 1 })).contains("more is not one of the arguments"));
    assert!(
        validate::validate(&schema, &json!({ "name": "abc" })).unwrap_err().contains("is required")
    );
}

#[test]
fn the_list_gives_each_action_the_schema_of_what_it_takes() {
    assert_eq!(
        input_of(REGISTRY, "service.list"),
        json!({ "type": "object", "properties": {}, "required": [], "additionalProperties": false })
    );
    assert_eq!(
        input_of(REGISTRY, "console.reach"),
        json!({
            "type": "object",
            "properties": {
                "reach": {
                    "type": "string",
                    "description": "Whom the console answers: private for machines on private networks, any for every machine.",
                    "enum": ["private", "any"],
                },
                "try": {
                    "type": "integer",
                    "description": "Try the change for this many seconds, 10 to 3600: Spark undoes it unless it is kept.",
                    "minimum": 10,
                    "maximum": 3600,
                },
            },
            "required": ["reach"],
            "additionalProperties": false,
        })
    );
    let name = input_of(REGISTRY, "service.restart");
    assert_eq!(name["required"], json!(["name"]));
    assert_eq!(name["properties"]["name"]["minLength"], 1);
    assert_eq!(name["properties"]["name"]["maxLength"], 23);
    assert_eq!(
        name["properties"]["name"]["pattern"],
        "^(?!(check|new|edit|try|keep|undo|secret|admind|authd)$)[A-Za-z0-9_][A-Za-z0-9_.-]*$"
    );
    let certificate = input_of(REGISTRY, "certificate.show");
    assert_eq!(certificate["properties"]["name"]["maxLength"], 64);
    assert_eq!(certificate["properties"]["name"]["pattern"], "^(?!.*\\.\\.)[a-z0-9][a-z0-9.-]*$");
    // An address's part is an argument like any other.
    assert_eq!(input_of(LAB, "lab.list")["properties"], json!({}));
    assert_eq!(input_of(LAB, "lab.restart")["required"], json!(["name"]));
    for action in listed(REGISTRY).as_array().unwrap() {
        assert!(action["input"].is_object(), "{}", action["id"]);
    }
}

/// Values to try a text field with, for the checks and the schema to
/// agree on.
fn texts() -> Vec<Value> {
    let named = [
        "",
        "a",
        "nginx",
        "this-machine",
        "example.org",
        "my-app_2",
        "a.b",
        "_x",
        "0",
        ".hidden",
        "-x",
        "-",
        ".",
        "..",
        "a b",
        "a/b",
        "../x",
        "a@b",
        "a\u{e9}",
        "\u{e9}",
        "a\nb",
        "nginx\n",
        "\nnginx",
        "A",
        "UPPER",
        "x..y",
        "a..",
        "a-",
        "a.",
        "1.2.3",
        "admind",
        "authd",
        "admind2",
        "Admind",
        "list",
        "diskhealth",
        "any",
        "private",
    ];
    let mut values: Vec<Value> = named.iter().map(|text| json!(text)).collect();
    values.extend(service::OWN_COMMANDS.iter().map(|word| json!(word)));
    values.extend(service::CONSOLE.iter().map(|word| json!(word)));
    for length in [22, 23, 24, 63, 64, 65, 254, 255, 256] {
        values.push(json!("a".repeat(length)));
        values.push(json!(format!("a{}", "-".repeat(length - 1))));
        values.push(json!("\u{e9}".repeat(length)));
    }
    // What is not text at all.
    values.extend([json!(7), json!(null), json!(true), json!(["a"]), json!({})]);
    values
}

/// Values to try a number field with, from `min` and `max`.
fn numbers(min: i64, max: i64) -> Vec<Value> {
    let mut values =
        vec![json!(min), json!(max), json!(0), json!(1), json!(-1), json!(60), json!(61)];
    values.extend([min.checked_sub(1), max.checked_add(1)].into_iter().flatten().map(|n| json!(n)));
    values.extend([json!("5"), json!(null), json!(true), json!(1.5), json!([1])]);
    values
}

/// Values to try a field of one of `words` with.
fn words(words: &[&str]) -> Vec<Value> {
    let mut values: Vec<Value> = words.iter().map(|word| json!(word)).collect();
    for word in words {
        values.extend([
            json!(word.to_uppercase()),
            json!(format!(" {word}")),
            json!(format!("{word} ")),
            json!(format!("{word}\n")),
            json!(format!("{word}s")),
        ]);
    }
    values.extend([json!(""), json!("everyone"), json!(0), json!(null), json!(true)]);
    values
}

/// What each field of every action in `actions` says is what the action
/// takes: its good examples are what the schema describes, its bad ones are
/// not, an argument of its address is one of its required fields, and, for
/// each field, the schema and the action agree on whether each of the
/// values the tests try is acceptable.
fn fields_are_true_to_the_types(actions: &'static [&'static dyn Registered]) {
    for action in actions {
        let id = action.id();
        let schema = schema(action.fields());
        let properties = schema["properties"].as_object().unwrap();
        let required: Vec<&str> =
            schema["required"].as_array().unwrap().iter().filter_map(Value::as_str).collect();
        let (good, bad, _) = action.example_values();
        for args in &good {
            let given = args.as_object().unwrap_or_else(|| panic!("{id}: {args} is no object"));
            for name in &required {
                assert!(given.contains_key(*name), "{id}: its example {args} lacks {name}");
            }
            for name in given.keys() {
                assert!(properties.contains_key(name), "{id}: its example has {name}, no field");
            }
            validate::validate(&schema, args).unwrap_or_else(|why| panic!("{id}: {why}"));
        }
        for args in &bad {
            // A bad example is one `check` or the types refuse, and the schema
            // says it cannot be, unless it is one of BEYOND_THE_SCHEMA.
            if BEYOND_THE_SCHEMA.contains(&(id, args.to_string().as_str())) {
                continue;
            }
            assert!(
                validate::validate(&schema, args).is_err(),
                "{id}: the schema accepts the bad example {args}"
            );
        }
        for name in action
            .path()
            .split('/')
            .filter_map(|part| part.strip_prefix('{').and_then(|inner| inner.strip_suffix('}')))
        {
            assert!(
                required.contains(&name),
                "{id}: {name} of its address is not a required field"
            );
        }
        for field in action.fields() {
            let name = field.name;
            let candidates = match field.kind {
                Type::Text { .. } => texts(),
                Type::OneOf(list) => words(list),
                Type::Number { min, max } => numbers(min, max),
                Type::Flag => vec![json!(true), json!(false), json!(0), json!("true"), json!(null)],
                Type::Secret { max } => secrets::candidates(max),
                Type::Document { max, .. } => secrets::document_candidates(max),
            };
            for candidate in candidates {
                // A `null` is an optional argument left out, which the
                // schema does not say (`Field::optional`).
                if candidate.is_null() && !field.required {
                    continue;
                }
                let mut args = good[0].clone();
                args[name] = candidate.clone();
                let accepted = action.prepare(args.clone()).is_ok();
                let described = validate::validate(&schema, &args);
                if CODE_DECIDES.contains(&(id, name)) {
                    // The schema says what it can, the shape of the value;
                    // what the action takes is that, and no more.
                    assert!(
                        !accepted || described.is_ok(),
                        "{id}: {name} {candidate}: the action takes it and the schema says \
                         {described:?}"
                    );
                    continue;
                }
                assert_eq!(
                    accepted,
                    described.is_ok(),
                    "{id}: {name} {candidate}: the action {} it and the schema says {described:?}",
                    if accepted { "takes" } else { "refuses" }
                );
            }
        }
        // A field the action does not have is refused by both.
        let mut more = good[0].clone();
        more["unknown_field"] = json!(1);
        assert!(action.prepare(more.clone()).is_err(), "{id}");
        assert!(validate::validate(&schema, &more).is_err(), "{id}");
        // So is leaving out each one that is required.
        for name in &required {
            let mut less = good[0].clone();
            less.as_object_mut().unwrap().remove(*name);
            assert!(action.prepare(less.clone()).is_err(), "{id}: without {name}");
            assert!(validate::validate(&schema, &less).is_err(), "{id}: without {name}");
        }
    }
}

#[test]
fn every_action_s_fields_are_true_to_its_types() {
    fields_are_true_to_the_types(LAB);
    fields_are_true_to_the_types(secrets::SECRETS);
    fields_are_true_to_the_types(secrets::DOCUMENTS);
    fields_are_true_to_the_types(REGISTRY);
}

#[test]
fn the_service_patterns_are_made_of_the_words_the_checks_refuse() {
    let own = service::OWN_COMMANDS.join("|");
    let and_the_console = [&service::OWN_COMMANDS[..], &service::CONSOLE[..]].concat().join("|");
    let pattern = |words: &str| format!("^(?!({words})$)[A-Za-z0-9_][A-Za-z0-9_.-]*$");
    for (id, words) in [
        ("service.explain", &own),
        ("service.start", &own),
        ("service.enable", &own),
        ("service.stop", &and_the_console),
        ("service.restart", &and_the_console),
        ("service.disable", &and_the_console),
    ] {
        assert_eq!(input_of(REGISTRY, id)["properties"]["name"]["pattern"], pattern(words), "{id}");
    }
}

/// The fields whose rule is code that a schema cannot say: the clock for a
/// time, a command's parser for addresses and ports, and the fields that
/// cannot go together (a ping has no port, a card that asks DHCP has no
/// addresses). For these a candidate the action takes is one the schema
/// describes, which is all the schema promises a client; one the schema
/// describes may still be refused.
const CODE_DECIDES: &[(&str, &str)] = &[
    // The tests' read takes the key or its file, which one is the code's.
    ("lab.unlock", "file"),
    ("metrics.around", "at"),
    ("network.set", "card"),
    ("network.set", "mode"),
    ("network.set", "addresses"),
    ("network.set", "router"),
    ("network.set", "dns"),
    ("firewall.test", "from"),
    ("firewall.add", "protocol"),
    ("firewall.add", "ports"),
    ("firewall.add", "from"),
    ("firewall.remove", "protocol"),
    ("firewall.remove", "ports"),
    ("firewall.remove", "from"),
    ("firewall.replace", "old_protocol"),
    ("firewall.replace", "old_ports"),
    ("firewall.replace", "old_from"),
    ("firewall.replace", "new_protocol"),
    ("firewall.replace", "new_ports"),
    ("firewall.replace", "new_from"),
    // A snapshot's name and a path are counted in bytes, which a schema
    // counts as characters; a name given to a snapshot is also not a
    // time, and a restore's paths are 1 to 100, each as a path is.
    ("snapshots.browse", "snapshot"),
    ("snapshots.browse-all", "snapshot"),
    ("snapshots.take", "name"),
    ("snapshots.restore", "snapshot"),
    ("snapshots.restore", "paths"),
    ("snapshots.restore-all", "snapshot"),
    ("snapshots.restore-all", "paths"),
    // The place is a folder or an address `backup add` reads, which it says.
    ("backups.add", "to"),
    // An address or a network is read as authd reads it: IPv4, or IPv6 with
    // a prefix of 64 for a block; a network with no bits past its prefix.
    ("security.unblock", "address"),
    ("security.never-block", "network"),
    ("security.never-block-remove", "network"),
    // A list of names holds each once, `everyone` alone, and names of the
    // shape `check_name` takes, at most 32 characters each; and both
    // `passwords` and `keys` off is nobody to sign in.
    ("ssh.set", "keys"),
    ("ssh.set", "allow_users"),
    ("ssh.set", "allow_groups"),
    // The host, and the address and its length, are read as the command
    // reads them, with the network's own address and its broadcast one
    // refused.
    ("private.serve", "endpoint"),
    ("private.serve", "address"),
    // A source's name is counted in bytes, as a path is, and what it is
    // given with decides whether the key is: a key and its file are one or
    // the other, which only `check` says.
    ("move.plan", "source"),
    ("move.plan", "key"),
    ("move.stage", "source"),
    ("move.stage", "key"),
    // A full name, a password and a key's line are counted in bytes, as the
    // account files and the pipe count them, which a schema counts as
    // characters; a key's line is a key only if the parser authd signs in
    // with finds one in it; a session is two numbers that fit their types.
    ("accounts.add", "full_name"),
    ("accounts.add", "password"),
    ("accounts.set", "full_name"),
    ("accounts.password", "password"),
    ("accounts.key-add", "line"),
    ("accounts.end", "session"),
    ("account.name", "full_name"),
    ("account.password", "current"),
    ("account.password", "new"),
    ("account.key-add", "line"),
    // A list of areas holds each once, and with a disk it is the disks
    // alone, which a pattern cannot say.
    ("hardware.check", "areas"),
    ("hardware.check", "disk"),
    // Each name an order or a check is for is read as `cert get` reads it,
    // once, a wildcard only with dns, which a pattern of the letters a name
    // has cannot say; and the terms are agreed to only with true, which a
    // flag says as true or false.
    ("certificate.check", "names"),
    ("certificate.get", "names"),
    ("certificate.get", "agree_terms"),
    // A page or a service, one of the two, and a page's address is one the
    // check reads: no space or control character in it, which a pattern
    // cannot say as the check does.
    ("updates.check-add", "url"),
    ("updates.check-add", "service"),
];

/// The bad examples whose rule a schema cannot say, which `check` alone
/// refuses: two fields that cannot go together, a pattern that does not
/// compile, and a value that the clock, a parser or the other fields rule
/// out.
const BEYOND_THE_SCHEMA: &[(&str, &str)] = &[
    ("lab.unlock", "{}"),
    ("metrics.around", r#"{"at":1,"range":"1h"}"#),
    ("storage.scrub", r#"{"mount":"/volumes/../etc"}"#),
    ("storage.log-set", "{}"),
    ("network.set", r#"{"card":"net0","mode":"fixed"}"#),
    ("network.set", r#"{"addresses":"10.0.2.15/24","card":"net0","mode":"dhcp"}"#),
    (
        "network.set",
        r#"{"addresses":"10.0.2.15/24","card":"net0","mode":"fixed","router":"192.168.9.1"}"#,
    ),
    (
        "network.set",
        r#"{"addresses":"10.0.2.11/24 10.0.2.12/24 10.0.2.13/24 10.0.2.14/24 10.0.2.15/24","card":"net0","mode":"fixed"}"#,
    ),
    (
        "network.set",
        r#"{"addresses":"10.0.2.15/24","card":"net0","dns":"10.0.2.3 10.0.2.4 10.0.2.5 10.0.2.6","mode":"fixed"}"#,
    ),
    ("firewall.add", r#"{"action":"allow","ports":"70000","protocol":"tcp"}"#),
    ("firewall.add", r#"{"action":"allow","ports":"7","protocol":"ping"}"#),
    ("firewall.add", r#"{"action":"allow","ports":"9-7","protocol":"tcp"}"#),
    ("firewall.add", r#"{"action":"allow","from":"300.1.1.1","protocol":"tcp"}"#),
    ("firewall.remove", r#"{"action":"allow","ports":"70000","protocol":"tcp"}"#),
    ("firewall.remove", r#"{"action":"allow","ports":"7","protocol":"ping"}"#),
    (
        "firewall.replace",
        r#"{"new_action":"deny","new_ports":"7","new_protocol":"tcp","old_action":"allow","old_ports":"70000","old_protocol":"tcp"}"#,
    ),
    (
        "firewall.replace",
        r#"{"new_action":"deny","new_ports":"7","new_protocol":"ping","old_action":"allow","old_ports":"7","old_protocol":"tcp"}"#,
    ),
    ("lab.log", r#"{"kernel":true,"service":"nginx"}"#),
    ("logs.read", r#"{"since":1763103600,"until":1763100000}"#),
    ("logs.read", r#"{"grep":"("}"#),
    ("logs.read", r#"{"kernel":true,"service":"diskhealth"}"#),
    ("logs.read-all", r#"{"since":1763103600,"until":1763100000}"#),
    ("logs.read-all", r#"{"grep":"("}"#),
    ("logs.read-all", r#"{"kernel":true,"service":"diskhealth"}"#),
    // A folder within the place, a name that is not a time, a place that is
    // a folder or an address, and no more than a hundred paths: the code's.
    ("snapshots.browse", r#"{"folder":"/shared","place":"/users","snapshot":"lab-before"}"#),
    ("snapshots.browse-all", r#"{"folder":"/shared","place":"/users","snapshot":"lab-before"}"#),
    ("snapshots.take", r#"{"name":"2026-11-11-0300","place":"/users"}"#),
    (
        "snapshots.restore",
        r#"{"label":"lab","paths":"/a\n/a\n/a\n/a\n/a\n/a\n/a\n/a\n/a\n/a\n/a\n/a\n/a\n/a\n/a\n/a\n/a\n/a\n/a\n/a\n/a\n/a\n/a\n/a\n/a\n/a\n/a\n/a\n/a\n/a\n/a\n/a\n/a\n/a\n/a\n/a\n/a\n/a\n/a\n/a\n/a\n/a\n/a\n/a\n/a\n/a\n/a\n/a\n/a\n/a\n/a\n/a\n/a\n/a\n/a\n/a\n/a\n/a\n/a\n/a\n/a\n/a\n/a\n/a\n/a\n/a\n/a\n/a\n/a\n/a\n/a\n/a\n/a\n/a\n/a\n/a\n/a\n/a\n/a\n/a\n/a\n/a\n/a\n/a\n/a\n/a\n/a\n/a\n/a\n/a\n/a\n/a\n/a\n/a\n/a\n/a\n/a\n/a\n/a\n/a\n/a","snapshot":"lab-before"}"#,
    ),
    (
        "snapshots.restore-all",
        r#"{"label":"lab","paths":"/a\n/a\n/a\n/a\n/a\n/a\n/a\n/a\n/a\n/a\n/a\n/a\n/a\n/a\n/a\n/a\n/a\n/a\n/a\n/a\n/a\n/a\n/a\n/a\n/a\n/a\n/a\n/a\n/a\n/a\n/a\n/a\n/a\n/a\n/a\n/a\n/a\n/a\n/a\n/a\n/a\n/a\n/a\n/a\n/a\n/a\n/a\n/a\n/a\n/a\n/a\n/a\n/a\n/a\n/a\n/a\n/a\n/a\n/a\n/a\n/a\n/a\n/a\n/a\n/a\n/a\n/a\n/a\n/a\n/a\n/a\n/a\n/a\n/a\n/a\n/a\n/a\n/a\n/a\n/a\n/a\n/a\n/a\n/a\n/a\n/a\n/a\n/a\n/a\n/a\n/a\n/a\n/a\n/a\n/a\n/a\n/a\n/a\n/a\n/a\n/a","snapshot":"lab-before"}"#,
    ),
    ("backups.add", r#"{"every":"hand","name":"lab","to":"s3://bucket/x","verify":"never"}"#),
    // At least one rule, and one the command reads: the code's.
    ("security.sign-in-set", "{}"),
    ("security.unblock", r#"{"address":"300.1.1.1"}"#),
    ("security.unblock", r#"{"address":"192.0.2.0/24"}"#),
    ("security.unblock", r#"{"address":"127.0.0.1"}"#),
    ("security.never-block", r#"{"network":"300.1.1.1"}"#),
    ("security.never-block", r#"{"network":"192.0.2.5/24"}"#),
    ("security.never-block", r#"{"network":"192.0.2.0/33"}"#),
    ("security.never-block-remove", r#"{"network":"300.1.1.1"}"#),
    ("security.never-block-remove", r#"{"network":"192.0.2.5/24"}"#),
    // A change that says nothing, both ways of signing in off, a list of
    // names that is no list, and a name the command reads as a word.
    ("ssh.set", "{}"),
    ("ssh.set", r#"{"keys":"no","passwords":"no"}"#),
    ("ssh.set", r#"{"allow_users":"everyone alice"}"#),
    ("ssh.set", r#"{"allow_users":"alice alice"}"#),
    ("ssh.set", r#"{"allow_users":"alice keys yes"}"#),
    // The port and the host and address of the tunnel, as the command
    // reads them.
    ("private.serve", r#"{"address":"10.8.0.1/31"}"#),
    ("private.serve", r#"{"address":"10.8.0.0/24"}"#),
    ("private.serve", r#"{"address":"10.8.0.255/24"}"#),
    ("private.serve", r#"{"endpoint":"-lab.example.org"}"#),
    // An area named twice, and a disk with the areas that are not the disks
    // alone: the code's.
    ("hardware.check", r#"{"areas":"memory memory"}"#),
    ("hardware.check", r#"{"areas":"disks cpu disks"}"#),
    ("hardware.check", r#"{"areas":"memory","disk":"vda"}"#),
    ("hardware.check", r#"{"areas":"disks cpu","disk":"vda"}"#),
    ("hardware.check", r#"{"disk":"vda"}"#),
    ("backups.add", r#"{"every":"hand","name":"lab","to":"relative/folder","verify":"never"}"#),
    // A name with an empty word, a wildcard without dns, a name twice, a
    // name of one word for an order, and terms not agreed to: the code's.
    ("certificate.check", r#"{"names":"shop..example.org"}"#),
    ("certificate.check", r#"{"names":"*.example.org"}"#),
    ("certificate.check", r#"{"names":"a.example.org,a.example.org"}"#),
    ("certificate.get", r#"{"agree_terms":false,"name":"shop","names":"shop.example.org"}"#),
    ("certificate.get", r#"{"agree_terms":true,"name":"shop","names":"shop..example.org"}"#),
    ("certificate.get", r#"{"agree_terms":true,"name":"shop","names":"*.example.org"}"#),
    ("certificate.get", r#"{"agree_terms":true,"name":"shop","names":"shop"}"#),
    // The key as a file and typed, or as neither; a card choice that is no
    // card of this machine; and no more than eight of them: the code's.
    ("move.plan", r#"{"source":"/volumes/LAB/dukovo-backup/lab-machine"}"#),
    ("move.stage", r#"{"source":"/volumes/LAB/dukovo-backup/lab-machine"}"#),
    (
        "move.plan",
        r#"{"key":"AGE-SECRET-KEY-1TEST","key_file":"/system/settings/backup/lab.key","source":"/volumes/LAB/dukovo-backup/lab-machine"}"#,
    ),
    (
        "move.stage",
        r#"{"key":"AGE-SECRET-KEY-1TEST","key_file":"/system/settings/backup/lab.key","source":"/volumes/LAB/dukovo-backup/lab-machine"}"#,
    ),
    (
        "move.plan",
        r#"{"cards":"52:54:00:aa:00:01=net0","key_file":"/system/settings/backup/lab.key","source":"/volumes/LAB/dukovo-backup/lab-machine"}"#,
    ),
    (
        "move.stage",
        r#"{"cards":"52:54:00:aa:00:01=net0","key_file":"/system/settings/backup/lab.key","source":"/volumes/LAB/dukovo-backup/lab-machine"}"#,
    ),
    (
        "move.plan",
        r#"{"cards":"a=automatic\na=automatic\na=automatic\na=automatic\na=automatic\na=automatic\na=automatic\na=automatic\na=automatic","key_file":"/system/settings/backup/lab.key","source":"/volumes/LAB/dukovo-backup/lab-machine"}"#,
    ),
    (
        "move.stage",
        r#"{"cards":"a=automatic\na=automatic\na=automatic\na=automatic\na=automatic\na=automatic\na=automatic\na=automatic\na=automatic","key_file":"/system/settings/backup/lab.key","source":"/volumes/LAB/dukovo-backup/lab-machine"}"#,
    ),
    // A name that an account has already, nothing to change, and a key that
    // is not a kind Dukovo takes or has options before it: the code's.
    ("accounts.add", r#"{"name":"root"}"#),
    ("accounts.set", r#"{"name":"lab-made"}"#),
    ("accounts.key-add", r#"{"line":"ssh-dss AAAAB3NzaC1kc3MAAACBAP","name":"lab-made"}"#),
    (
        "accounts.key-add",
        r#"{"line":"command=\"x\" ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIAABAgMEBQYHCAkKCwwNDg8QERITFBUWFxgZGhscHR4f lab","name":"lab-made"}"#,
    ),
    ("account.key-add", r#"{"line":"ssh-dss AAAAB3NzaC1kc3MAAACBAP"}"#),
    (
        "account.key-add",
        r#"{"line":"command=\"x\" ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIAABAgMEBQYHCAkKCwwNDg8QERITFBUWFxgZGhscHR4f lab"}"#,
    ),
    // Nothing to change, a restart time for a machine that does not update
    // by itself, a check of both a page and a service, and an `insecure`
    // that is for a page: the code's, since each is a way of combining
    // fields.
    ("updates.set", "{}"),
    ("updates.set", r#"{"restart":"03:30","unattended":false}"#),
    ("updates.check-add", r#"{"name":"x"}"#),
    ("updates.check-add", r#"{"name":"x","service":"nginx","url":"https://a/"}"#),
    ("updates.check-add", r#"{"insecure":true,"name":"x","service":"nginx"}"#),
];

fn lab_action(id: &str) -> &'static dyn Registered {
    *LAB.iter().find(|action| action.id() == id).unwrap()
}

#[test]
fn a_read_takes_each_field_of_its_schema_from_the_query_as_its_type_reads_it() {
    let fields = lab_action("lab.log").fields();
    let read = |query: &str| query_arguments(Params::new(), query, fields);
    assert_eq!(read("").unwrap(), json!({}));
    assert_eq!(
        read("lines=20&service=web+one&kernel=false").unwrap(),
        json!({ "lines": 20, "service": "web one", "kernel": false })
    );
    // A form's encoding: `+` is a space, `%2B` a plus, `%XX` any byte.
    assert_eq!(read("service=a%2Bb%20c%C3%A9").unwrap(), json!({ "service": "a+b c\u{e9}" }));
    assert_eq!(read("kernel=true").unwrap(), json!({ "kernel": true }));
    // A value is checked by the action itself, after this.
    assert_eq!(read("lines=0").unwrap(), json!({ "lines": 0 }));
    for (query, why) in [
        ("since=-1h", "this read takes no \"since\""),
        ("lines=20&lines=30", "\"lines\" is given twice"),
        ("lines=many", "\"lines\" is a whole number"),
        ("kernel=1", "\"kernel\" is true or false"),
        ("service=%zz", "\"service\" is not text"),
        ("service=%C3", "\"service\" is not text"),
        ("service=%4", "\"service\" is not text"),
    ] {
        let failure = read(query).unwrap_err();
        assert_eq!((failure.kind, failure.detail.as_str()), (Kind::BadRequest, why), "{query}");
    }
    // A name the address already gives is not given again.
    let mut params = Params::new();
    params.insert("service".into(), json!("web"));
    let twice = query_arguments(params, "service=db", fields).unwrap_err();
    assert_eq!(twice.detail, "\"service\" is given twice");
}

#[test]
fn a_part_of_the_address_is_read_as_its_field_s_type_reads_it() {
    let fields = lab_action("lab.log").fields();
    let given = |name: &str, text: &str| {
        let mut params = Params::new();
        params.insert(name.into(), json!(text));
        query_arguments(params, "", fields)
    };
    assert_eq!(given("lines", "20").unwrap(), json!({ "lines": 20 }));
    assert_eq!(given("kernel", "true").unwrap(), json!({ "kernel": true }));
    // Text is kept as it came, and a part the read does not name is left for
    // the action to refuse.
    assert_eq!(given("service", "20").unwrap(), json!({ "service": "20" }));
    assert_eq!(given("other", "20").unwrap(), json!({ "other": "20" }));
    for (name, text, why) in [
        ("lines", "many", "\"lines\" is a whole number"),
        ("lines", "", "\"lines\" is a whole number"),
        ("kernel", "yes", "\"kernel\" is true or false"),
    ] {
        let failure = given(name, text).unwrap_err();
        assert_eq!((failure.kind, failure.detail.as_str()), (Kind::BadRequest, why), "{name}");
    }
}

#[test]
fn a_read_that_runs_as_an_administrator_needs_an_unlocked_one_or_a_token_s_full_scope() {
    let action = lab_action("lab.log");
    let session = |admin, unlocked| Caller::Session { name: "dave", admin, unlocked };
    let refused = |caller: Caller<'_>| authorize(&caller, action).unwrap_err();
    let not_admin = refused(session(false, true));
    assert_eq!(not_admin.kind, Kind::NotAdmin);
    assert_eq!(
        not_admin.detail,
        "dave's account can look at its own part of logs, and only an administrator at the rest"
    );
    let locked = refused(session(true, false));
    assert_eq!(
        (locked.kind, locked.detail.as_str()),
        (Kind::Locked, "unlock the console to read this")
    );
    assert!(authorize(&session(true, true), action).is_ok());
    // A token that may read logs may not have authd run a command as an
    // administrator; one with the area's full scope may.
    let read = Scopes::parse("logs:read").unwrap();
    let full = Scopes::parse("logs").unwrap();
    let token = |scopes| Caller::Token { name: "ci", scopes };
    assert_eq!(refused(token(&read)).kind, Kind::OutOfScope);
    assert!(authorize(&token(&full), action).is_ok());
    // A read that runs as the person needs only the read scope.
    let services = Scopes::parse("services:read").unwrap();
    assert!(authorize(&token(&services), lab_action("lab.list")).is_ok());
}

#[test]
fn a_change_tried_for_a_window_begins_its_trial_and_its_keep_and_undo_end_it() {
    let ends = [
        "network.keep",
        "network.undo",
        "firewall.keep",
        "firewall.undo",
        "console.keep",
        "console.undo",
        "ssh.keep",
        "ssh.undo",
    ];
    let (mut begins, mut ended) = (0, 0);
    for action in REGISTRY {
        let (good, _, _) = action.example_values();
        let trial = |args: &Value| action.prepare(args.clone()).unwrap().trial;
        for args in &good {
            if ends.contains(&action.id()) {
                assert_eq!(trial(args), Some(Trial::Ends(action.area())), "{}", action.id());
                ended += 1;
            } else if action.fields().iter().any(|field| field.name == "try") {
                // Tried, the trial is the area's (`network` or
                // `firewall`); at once, there is none.
                let (mut tried, mut at_once) = (args.clone(), args.clone());
                tried["try"] = json!(30);
                at_once.as_object_mut().unwrap().remove("try");
                assert_eq!(trial(&tried), Some(Trial::Begins(action.area())), "{}", action.id());
                assert_eq!(trial(&at_once), None, "{}", action.id());
                begins += 1;
            } else {
                assert_eq!(trial(args), None, "{}", action.id());
            }
        }
        let tries = action.fields().iter().any(|field| field.name == "try");
        if tries || ends.contains(&action.id()) {
            assert!(action.changes() && action.runs() == Runs::AsAdministrator, "{}", action.id());
        }
    }
    assert!(begins >= 4 && ended == ends.len(), "{begins} begin and {ended} end");
}
