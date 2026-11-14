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
//! A change says whether it changed anything by reading `service list
//! --json` before and after, and comparing the state Spark says the
//! service is in (ADR 0515); a restart always did something. Nothing that
//! moves by itself is compared: no process number, no time. Enabling and
//! disabling also write the service's file, whose text the journal reads
//! before and after, so either changed something when the file did. The
//! file, not Spark's document, says whether the service starts at boot:
//! Spark reads it again only as it starts the service, so the `enabled`
//! of its document does not follow `disable` of a service that is
//! stopped.
//!
//! A service's name is checked by the rule Spark checks the name of a
//! service file by. A name that `service` reads as one of its own commands
//! (`service check restart` checks a file) is refused too, since it would
//! run another command than the one shown.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use spark_api::show;

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

/// What a change of a service moves, as text to compare, from what Spark
/// says of the service.
type Pick = Option<fn(&show::Service) -> String>;

/// What a change of a service moves that Spark shows: what the service is
/// doing. Enabling and disabling move the service's file too, which the
/// journal compares (see the module's words). Spark says
/// it in nine words (`stopped`, `disabled`, `pending`, `starting`,
/// `running`, `done`, `waiting`, `stopping` and `failed`), so that
/// starting what runs and stopping what is stopped compare equal, and
/// starting what is stopped does not.
const STATE: Pick = Some(|service| service.state.word().to_string());

/// What a restart moves: nothing is compared, since it always does
/// something.
const ALWAYS: Pick = None;

/// The command that reads what Spark says of every service, for the
/// changes' probes.
fn probe_argv() -> Vec<String> {
    ["service", "list", "--json"].map(String::from).to_vec()
}

/// What `pick` takes from what Spark says of the service `name` in the
/// output of `service list --json`; None when `pick` is none, or the
/// output is not Spark's document or has no such service.
fn picked(pick: Pick, name: &str, output: &str) -> Option<String> {
    let document: show::Show = serde_json::from_str(output).ok()?;
    let service = document.services.iter().find(|service| service.name == name)?;
    Some(pick?(service))
}

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

/// The service file of `name`, which `enable` and `disable` write and the
/// journal keeps the versions of.
fn service_file(name: &str) -> String {
    format!("{}/{name}.toml", crate::servicefiles::DIR)
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
/// it takes the same arguments. `$pick` is what it moves, which its
/// `changed` compares.
macro_rules! change {
    (
        $(#[$doc:meta])*
        $action:ident, $verb:literal, $title:literal, $args:ident, $bad:expr, undo: $undo:expr,
        file: $file:literal, compares: $pick:expr
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

            fn files(args: &$args) -> Vec<String> {
                if $file { vec![service_file(&args.name)] } else { Vec::new() }
            }

            fn probe(_: &$args) -> Option<Vec<String>> {
                $pick.map(|_| probe_argv())
            }

            fn probed(args: &$args, output: &str) -> Option<String> {
                picked($pick, &args.name, output)
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
    Start, "start", "Start a service", Named, bad_names(), undo: Some("service.stop"),
    file: false, compares: STATE
}

change! {
    /// `service NAME stop`: stops a service, unless it is the console's.
    /// `service.start` puts back what its example did.
    Stop, "stop", "Stop a service", Guarded, bad_names_and_the_console(), undo: Some("service.start"),
    file: false, compares: STATE
}

change! {
    /// `service NAME restart`: stops a service and starts it again, unless
    /// it is the console's. Nothing puts back what it did.
    Restart, "restart", "Restart a service", Guarded, bad_names_and_the_console(), undo: None,
    file: false, compares: ALWAYS
}

change! {
    /// `service NAME enable`: starts a service now and at each boot, which
    /// it keeps in the service's file. `service.disable` puts back what its
    /// example did.
    Enable, "enable", "Enable a service", Named, bad_names(), undo: Some("service.disable"),
    file: true, compares: STATE
}

change! {
    /// `service NAME disable`: stops a service and keeps it from starting
    /// at boot, which it keeps in the service's file, unless it is the
    /// console's. `service.enable` puts back what its example did.
    Disable, "disable", "Disable a service", Guarded, bad_names_and_the_console(), undo: Some("service.enable"),
    file: true, compares: STATE
}

#[cfg(test)]
mod tests {
    use spark_api::show::State;

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

    /// What `service list --json` prints: Spark's document, on one line,
    /// with a service for each of `services`, as (name, state, enabled).
    fn document(services: &[(&str, State, bool)]) -> String {
        let services = services
            .iter()
            .map(|&(name, state, enabled)| show::Service {
                name: name.into(),
                state,
                enabled,
                pid: (state == State::Running).then_some(4242),
                since_ms: 90_000,
                started_ms: Some(80_000),
                restarts: 1,
                ..show::Service::default()
            })
            .collect();
        let shown = show::Show {
            show_version: show::VERSION,
            now_ms: 123_456,
            services,
            ..show::Show::default()
        };
        serde_json::to_string(&shown).unwrap()
    }

    /// What the probe of `id` picks for the service `name` from `output`.
    fn picks(id: &str, name: &str, output: &str) -> Option<String> {
        registered(id).probed(&json!({ "name": name }), output)
    }

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
    fn a_change_reads_what_spark_says_of_the_services_and_a_restart_reads_nothing() {
        let list = ["service", "list", "--json"];
        for verb in ["start", "stop", "enable", "disable"] {
            let id = format!("service.{verb}");
            let prepared = registered(&id).prepare(json!({ "name": "nginx" })).unwrap();
            assert_eq!(prepared.probe.as_deref(), Some(&list.map(String::from)[..]), "{id}");
            assert!(registered(&id).compares(), "{id}");
        }
        // It always does something.
        let restart = registered("service.restart");
        assert_eq!(restart.prepare(json!({ "name": "nginx" })).unwrap().probe, None);
        assert!(!restart.compares());
        // A read has nothing to compare.
        for id in ["service.list", "service.explain"] {
            assert!(!registered(id).compares(), "{id}");
        }
        assert_eq!(registered("service.list").prepare(json!({})).unwrap().probe, None);
    }

    #[test]
    fn start_and_stop_compare_the_state_and_nothing_that_moves_by_itself() {
        for id in ["service.start", "service.stop"] {
            for state in [
                State::Stopped,
                State::Disabled,
                State::Pending,
                State::Starting,
                State::Running,
                State::Done,
                State::Waiting,
                State::Stopping,
                State::Failed,
            ] {
                let output = document(&[("nginx", state, true)]);
                assert_eq!(picks(id, "nginx", &output).as_deref(), Some(state.word()), "{id}");
            }
        }
        // Starting what runs, and stopping what is stopped, read the same
        // before and after, whatever its process number and times say.
        let before = document(&[("nginx", State::Running, true)]);
        let later = before.replace("4242", "5151").replace("90000", "99000");
        assert_ne!(before, later);
        assert_eq!(
            picks("service.start", "nginx", &before),
            picks("service.start", "nginx", &later)
        );
        let stopped = document(&[("nginx", State::Stopped, true)]);
        assert_eq!(
            picks("service.stop", "nginx", &stopped),
            picks("service.stop", "nginx", &stopped)
        );
        // Starting what is stopped, and stopping what runs, do not.
        assert_ne!(
            picks("service.start", "nginx", &stopped),
            picks("service.start", "nginx", &before)
        );
        assert_ne!(
            picks("service.stop", "nginx", &before),
            picks("service.stop", "nginx", &stopped)
        );
        // A stop that is begun and not ended is not the stopped it ends in.
        let stopping = document(&[("nginx", State::Stopping, true)]);
        assert_ne!(
            picks("service.stop", "nginx", &stopping),
            picks("service.stop", "nginx", &stopped)
        );
    }

    #[test]
    fn enable_and_disable_compare_the_state_and_name_the_file_the_journal_compares() {
        for id in ["service.enable", "service.disable"] {
            let at = |state, enabled| picks(id, "nginx", &document(&[("nginx", state, enabled)]));
            assert_eq!(at(State::Running, true).as_deref(), Some("running"), "{id}");
            assert_eq!(at(State::Disabled, false).as_deref(), Some("disabled"), "{id}");
            // What Spark's document says of starting at boot is not
            // compared: it is the file Spark read last, which a disable
            // of a stopped service does not change. The file itself is.
            assert_eq!(at(State::Stopped, true), at(State::Stopped, false), "{id}");
            assert_ne!(at(State::Running, true), at(State::Stopped, true), "{id}");
            let prepared = registered(id).prepare(json!({ "name": "nginx" })).unwrap();
            assert_eq!(prepared.files, ["/system/settings/spark/services/nginx.toml"], "{id}");
        }
        for id in ["service.start", "service.stop", "service.restart"] {
            let prepared = registered(id).prepare(json!({ "name": "nginx" })).unwrap();
            assert!(prepared.files.is_empty(), "{id}");
        }
    }

    #[test]
    fn a_probe_picks_the_service_asked_for_and_none_when_the_output_does_not_say() {
        let output = document(&[
            ("nginx2", State::Failed, false),
            ("nginx", State::Running, true),
            ("sshd", State::Stopped, true),
        ]);
        assert_eq!(picks("service.start", "nginx", &output).as_deref(), Some("running"));
        assert_eq!(picks("service.start", "nginx2", &output).as_deref(), Some("failed"));
        assert_eq!(picks("service.disable", "sshd", &output).as_deref(), Some("stopped"));
        for (name, output) in [
            ("gone", output.as_str()),
            ("nginx", ""),
            ("nginx", "\n"),
            ("nginx", "not json"),
            ("nginx", "[]"),
            ("nginx", "[{\"name\":\"nginx\"}]"),
            ("nginx", "{}"),
            ("nginx", r#"{"services":[]}"#),
            ("nginx", r#"{"services":[{"name":"nginx","state":"sleeping"}]}"#),
        ] {
            assert_eq!(picks("service.start", name, output), None, "{name} {output:?}");
            assert_eq!(picks("service.enable", name, output), None, "{name} {output:?}");
        }
        // A service Spark has no state for is its default, `stopped`, and
        // the document's other fields may be left out.
        let bare = r#"{"services":[{"name":"nginx"}]}"#;
        assert_eq!(picks("service.start", "nginx", bare).as_deref(), Some("stopped"));
        // A restart compares nothing, so there is nothing to pick.
        assert_eq!(picks("service.restart", "nginx", &output), None);
        // Arguments that are not the action's pick nothing.
        assert_eq!(registered("service.start").probed(&json!({ "name": 7 }), &output), None);
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
    fn enable_and_disable_name_the_service_file_they_write_and_the_others_none() {
        for verb in CHANGES {
            let prepared =
                registered(&format!("service.{verb}")).prepare(json!({ "name": "nginx" })).unwrap();
            let writes = matches!(verb, "enable" | "disable");
            let want: &[&str] =
                if writes { &["/system/settings/spark/services/nginx.toml"] } else { &[] };
            assert_eq!(prepared.files, want, "{verb}");
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
