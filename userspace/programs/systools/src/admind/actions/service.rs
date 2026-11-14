// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The services' actions (ADR 0515): the list, a service's explanation,
//! and starting, stopping, restarting, enabling and disabling one. Each
//! runs `service`, service(8), which asks Spark.
//!
//! The reads run as the person with `--json`. The changes run as an
//! administrator, as `elevate service NAME restart` does at a terminal.
//! The console is made of admind and authd, so an action that would end
//! either is refused: the answer to it would never arrive.
//!
//! A service's name is checked by the rule Spark checks the name of a
//! service file by. A name that `service` reads as one of its own commands
//! (`service check restart` checks a file) is refused too, since it would
//! run another command than the one shown.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::{Action, Args, Bad, Examples, Nothing, Runs, Tests, Undo};

/// The area of these actions: the scope a token needs, `services`.
const AREA: &str = "services";

/// The tests of these actions, which `lab/actions.toml` names.
const TESTS: Tests = Tests {
    checks: "admind::actions::service::tests",
    route: "admin-actions",
    same_as_command: "admin-actions",
    browser: "web-console/actions",
    ansible: "web-console/ansible",
};

/// The services the console is made of. Stopping, restarting or disabling
/// one ends the console or its sign-in, so it is done at a terminal.
const CONSOLE: [&str; 2] = ["admind", "authd"];

/// The words `service` takes as commands of its own where a service's name
/// would be (service.rs).
const OWN_COMMANDS: [&str; 7] = ["check", "new", "edit", "try", "keep", "undo", "secret"];

/// The service the examples use: every image runs it, nothing needs it,
/// and neither the console nor the tests depend on it, so that stopping
/// and starting it again leaves nothing different. It only looks at the
/// disks every half hour (diskhealth(8)).
const EXAMPLE: &str = "diskhealth";

/// A service, by its name.
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Named {
    /// The service's name.
    pub name: String,
}

/// A service, by its name, for an action that ends its process: the
/// console's own are refused.
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Guarded {
    /// The service's name.
    pub name: String,
}

/// Checks that `name` can be a service's name, for `service` to take.
fn check_name(name: &str) -> Result<(), Bad> {
    let max = radix_abi::resource_domain::NAME_MAX;
    if manifest::service::check_name(name).is_err() {
        return Err(Bad {
            field: "name",
            why: format!(
                "A service's name has 1 to {max} letters, digits, hyphens, underscores and dots, \
                 and starts with a letter, a digit or an underscore. Nothing was done."
            ),
        });
    }
    if OWN_COMMANDS.contains(&name) {
        return Err(Bad {
            field: "name",
            why: format!(
                "The service command takes {name} as a command of its own, not as a service's \
                 name. Nothing was done."
            ),
        });
    }
    Ok(())
}

impl Args for Named {
    fn check(&self) -> Result<(), Bad> {
        check_name(&self.name)
    }
}

impl Args for Guarded {
    fn check(&self) -> Result<(), Bad> {
        check_name(&self.name)?;
        if CONSOLE.contains(&self.name.as_str()) {
            let name = &self.name;
            return Err(Bad {
                field: "name",
                why: format!(
                    "The console runs on {name}: stop or restart it at a terminal, with elevate \
                     service {name} restart. Nothing was done."
                ),
            });
        }
        Ok(())
    }
}

/// `service list --json`: every service, as Spark's `show` document.
pub struct List;

impl Action for List {
    const ID: &'static str = "service.list";
    const AREA: &'static str = AREA;
    const CHANGES: bool = false;
    const PATH: &'static str = "/api/1/services";
    const RUNS: Runs = Runs::AsThePerson;
    const TESTS: Tests = TESTS;
    const TITLE: &'static str = "List the services";
    type Args = Nothing;

    fn argv(_: &Nothing) -> Vec<String> {
        ["service", "list", "--json"].map(String::from).to_vec()
    }

    fn examples() -> Examples<Nothing> {
        Examples {
            good: vec![Nothing {}],
            bad: vec![json!({ "name": EXAMPLE }), json!({ "all": true })],
            undo: None,
        }
    }
}

/// `service NAME explain --json`: why a service is as it is.
pub struct Explain;

impl Action for Explain {
    const ID: &'static str = "service.explain";
    const AREA: &'static str = AREA;
    const CHANGES: bool = false;
    const PATH: &'static str = "/api/1/services/{name}/explain";
    const RUNS: Runs = Runs::AsThePerson;
    const TESTS: Tests = TESTS;
    const TITLE: &'static str = "Explain a service";
    type Args = Named;

    fn argv(args: &Named) -> Vec<String> {
        ["service", args.name.as_str(), "explain", "--json"].map(String::from).to_vec()
    }

    fn subject(args: &Named) -> Option<String> {
        Some(args.name.clone())
    }

    fn examples() -> Examples<Named> {
        Examples {
            good: vec![example()],
            bad: vec![json!({ "name": "" }), json!({ "name": "../etc" }), json!({})],
            undo: None,
        }
    }
}

/// The example service's arguments.
fn example() -> Named {
    Named { name: EXAMPLE.into() }
}

/// Arguments no service action takes: an empty name, a path, and a
/// field the action has none of.
fn bad_names() -> Vec<Value> {
    vec![
        json!({ "name": "" }),
        json!({ "name": "../etc/passwd" }),
        json!({ "name": EXAMPLE, "force": true }),
    ]
}

/// [`bad_names`], and the console's own services, for the actions that
/// refuse them.
fn bad_names_and_the_console() -> Vec<Value> {
    let mut bad = bad_names();
    bad.extend(CONSOLE.iter().map(|name| json!({ "name": name })));
    bad
}

/// Declares the action that runs `service NAME VERB` as an administrator,
/// for the arguments `$args`, with `$bad` for its bad examples. `$undo` is
/// the other action that puts back what its example did, if there is one:
/// it takes the same arguments.
macro_rules! change {
    (
        $(#[$doc:meta])*
        $action:ident, $verb:literal, $title:literal, $args:ident, $bad:expr, undo: $undo:expr
    ) => {
        $(#[$doc])*
        pub struct $action;

        impl Action for $action {
            const ID: &'static str = concat!("service.", $verb);
            const AREA: &'static str = AREA;
            const CHANGES: bool = true;
            const PATH: &'static str = concat!("/api/1/services/{name}/", $verb);
            const RUNS: Runs = Runs::AsAdministrator;
            const TESTS: Tests = TESTS;
            const TITLE: &'static str = $title;
            type Args = $args;

            fn argv(args: &$args) -> Vec<String> {
                ["service", args.name.as_str(), $verb].map(String::from).to_vec()
            }

            fn subject(args: &$args) -> Option<String> {
                Some(args.name.clone())
            }

            fn examples() -> Examples<$args> {
                Examples {
                    good: vec![$args { name: EXAMPLE.into() }],
                    bad: $bad,
                    undo: $undo.map(|action| Undo { action, args: json!({ "name": EXAMPLE }) }),
                }
            }
        }
    };
}

change! {
    /// `service NAME start`: starts a service. `service.stop` puts back
    /// what its example did.
    Start, "start", "Start a service", Named, bad_names(), undo: Some("service.stop")
}

change! {
    /// `service NAME stop`: stops a service, unless it is the console's.
    /// `service.start` puts back what its example did.
    Stop, "stop", "Stop a service", Guarded, bad_names_and_the_console(), undo: Some("service.start")
}

change! {
    /// `service NAME restart`: stops a service and starts it again, unless
    /// it is the console's. Nothing puts back what it did.
    Restart, "restart", "Restart a service", Guarded, bad_names_and_the_console(), undo: None
}

change! {
    /// `service NAME enable`: starts a service now and at each boot.
    /// `service.disable` puts back what its example did.
    Enable, "enable", "Enable a service", Named, bad_names(), undo: Some("service.disable")
}

change! {
    /// `service NAME disable`: stops a service and keeps it from starting
    /// at boot, unless it is the console's. `service.enable` puts back what
    /// its example did.
    Disable, "disable", "Disable a service", Guarded, bad_names_and_the_console(), undo: Some("service.enable")
}

#[cfg(test)]
mod tests {
    use super::super::{REGISTRY, Registered};
    use super::*;
    use crate::admind::errors::Kind;

    /// The registered action `id`.
    fn registered(id: &str) -> &'static dyn Registered {
        *REGISTRY.iter().find(|action| action.id() == id).unwrap()
    }

    /// The command `id` makes for `args`.
    fn argv(id: &str, args: Value) -> Vec<String> {
        registered(id).prepare(args).unwrap().argv
    }

    /// Why `id` refuses `args`: the field and the words.
    fn refused(id: &str, args: Value) -> (Kind, Option<&'static str>, String) {
        let failure = registered(id).prepare(args).unwrap_err();
        (failure.kind, failure.field, failure.detail)
    }

    const CHANGES: [&str; 5] = ["start", "stop", "restart", "enable", "disable"];

    #[test]
    fn the_reads_run_service_with_json() {
        assert_eq!(argv("service.list", json!({})), ["service", "list", "--json"]);
        assert_eq!(
            argv("service.explain", json!({ "name": "nginx" })),
            ["service", "nginx", "explain", "--json"]
        );
    }

    #[test]
    fn each_change_runs_service_with_the_name_before_the_verb() {
        for verb in CHANGES {
            let id = format!("service.{verb}");
            assert_eq!(argv(&id, json!({ "name": "nginx" })), ["service", "nginx", verb], "{id}");
            let action = registered(&id);
            assert_eq!(action.runs(), Runs::AsAdministrator, "{id}");
            assert_eq!(action.path(), format!("/api/1/services/{{name}}/{verb}"), "{id}");
        }
    }

    #[test]
    fn the_reads_are_the_person_s_and_have_the_services_scope() {
        for id in ["service.list", "service.explain"] {
            let action = registered(id);
            assert_eq!(
                (action.changes(), action.runs(), action.area()),
                (false, Runs::AsThePerson, "services")
            );
        }
        assert_eq!(registered("service.list").path(), "/api/1/services");
        assert_eq!(registered("service.explain").path(), "/api/1/services/{name}/explain");
    }

    #[test]
    fn a_service_s_name_is_the_one_spark_s_files_have() {
        for name in ["nginx", "my-app_2", "a.b", "_x", "0", "name-of-23-characters"] {
            assert_eq!(argv("service.explain", json!({ "name": name }))[1], name);
        }
        let words = "A service's name has 1 to 23 letters, digits, hyphens, underscores and dots, \
                     and starts with a letter, a digit or an underscore. Nothing was done.";
        for name in
            ["", ".hidden", "-x", "a b", "a/b", "../x", "a@b", "caf\u{e9}", "a\nb", &"a".repeat(24)]
        {
            for id in ["service.explain", "service.start", "service.restart"] {
                assert_eq!(
                    refused(id, json!({ "name": name })),
                    (Kind::BadValue, Some("name"), words.into()),
                    "{id} {name:?}"
                );
            }
        }
    }

    #[test]
    fn a_name_service_takes_for_a_command_of_its_own_is_refused() {
        for name in OWN_COMMANDS {
            let (kind, field, why) = refused("service.start", json!({ "name": name }));
            assert_eq!((kind, field), (Kind::BadValue, Some("name")));
            assert_eq!(
                why,
                format!(
                    "The service command takes {name} as a command of its own, not as a \
                     service's name. Nothing was done."
                )
            );
        }
        // `list` is read as the command only when nothing but options
        // follows it, so it is a name here.
        assert_eq!(argv("service.start", json!({ "name": "list" })), ["service", "list", "start"]);
    }

    #[test]
    fn the_console_s_own_services_are_not_stopped_restarted_or_disabled_here() {
        for name in CONSOLE {
            for verb in ["stop", "restart", "disable"] {
                assert_eq!(
                    refused(&format!("service.{verb}"), json!({ "name": name })),
                    (
                        Kind::BadValue,
                        Some("name"),
                        format!(
                            "The console runs on {name}: stop or restart it at a terminal, with \
                             elevate service {name} restart. Nothing was done."
                        )
                    ),
                    "{verb} {name}"
                );
            }
            // Starting or enabling what runs already does no harm.
            for verb in ["start", "enable"] {
                assert_eq!(
                    argv(&format!("service.{verb}"), json!({ "name": name })),
                    ["service", name, verb]
                );
            }
        }
        // Only the whole name is the console's.
        assert_eq!(argv("service.restart", json!({ "name": "admind2" }))[1], "admind2");
    }

    #[test]
    fn arguments_the_action_does_not_take_do_not_read() {
        let wrong = json!({ "name": "nginx", "force": true });
        for id in ["service.explain", "service.start", "service.stop"] {
            assert_eq!(
                registered(id).prepare(wrong.clone()).unwrap_err().kind,
                Kind::BadRequest,
                "{id}"
            );
        }
        for args in [json!({}), json!({ "name": 7 }), json!({ "name": null })] {
            assert_eq!(
                registered("service.start").prepare(args).unwrap_err().kind,
                Kind::BadRequest
            );
        }
        assert_eq!(
            registered("service.list").prepare(json!({ "name": "x" })).unwrap_err().kind,
            Kind::BadRequest
        );
    }

    #[test]
    fn the_ansible_task_is_named_for_the_action_and_the_service() {
        let task =
            registered("service.restart").prepare(json!({ "name": "nginx" })).unwrap().ansible;
        assert_eq!(task.name, "Restart a service: nginx");
        assert!(task.become_root);
        assert_eq!(task.changed_when, None);
        let task = registered("service.list").prepare(json!({})).unwrap().ansible;
        assert_eq!(task.name, "List the services");
        assert!(!task.become_root);
        assert_eq!(task.changed_when.as_deref(), Some("false"));
    }

    #[test]
    fn a_service_change_names_no_file() {
        for verb in CHANGES {
            let prepared =
                registered(&format!("service.{verb}")).prepare(json!({ "name": "nginx" })).unwrap();
            assert!(prepared.files.is_empty(), "{verb}");
        }
    }

    #[test]
    fn the_examples_work_and_the_bad_ones_do_not() {
        for action in REGISTRY.iter().filter(|action| action.id().starts_with("service.")) {
            let id = action.id();
            let (good, bad, undo) = action.example_values();
            assert!(!bad.is_empty(), "{id}");
            for args in &good {
                assert!(action.prepare(args.clone()).is_ok(), "{id}");
            }
            for args in bad {
                assert!(action.prepare(args.clone()).is_err(), "{id} {args}");
            }
            let verb = id.strip_prefix("service.").unwrap();
            assert_eq!(
                undo.is_some(),
                matches!(verb, "start" | "stop" | "enable" | "disable"),
                "{id}"
            );
            assert!(
                good.iter().all(|args| args.get("name").is_none_or(|name| name == EXAMPLE)),
                "{id}"
            );
        }
    }

    #[test]
    fn what_a_change_puts_back_is_the_change_the_examples_name() {
        // The other action of each pair takes the arguments the first
        // took.
        for (verb, back) in
            [("start", "stop"), ("stop", "start"), ("enable", "disable"), ("disable", "enable")]
        {
            let (good, _, undo) = registered(&format!("service.{verb}")).example_values();
            let undo = undo.unwrap();
            assert_eq!(undo.action, format!("service.{back}"), "{verb}");
            assert_eq!(Some(&undo.args), good.first(), "{verb}");
            assert!(registered(undo.action).prepare(undo.args).is_ok(), "{back}");
        }
        assert!(registered("service.restart").example_values().2.is_none());
    }
}
