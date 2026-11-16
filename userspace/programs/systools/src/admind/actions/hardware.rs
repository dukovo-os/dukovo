// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The Hardware page's actions: what this machine is, the memory modules it
//! holds, the last check of the hardware and the checks that came before
//! it, and the check itself. The area is `hardware`, the scope a token
//! needs.
//!
//! `hardware.show`, `hardware.last` and `hardware.history` run as the
//! person: what they print is what everyone may read. `hardware.inventory`
//! is the same command as `hardware.show` run as an administrator, and
//! that is the whole of the difference: with the platform's right to read
//! the firmware's tables, `hardware show` says whether the memory has error
//! correction and what each module is. `hardware.last` answers with any
//! status `hardware last` ends with ([`Last::ANSWERS`]): with nothing kept
//! it prints `null` and ends with 3.
//!
//! `hardware.check` runs `hardware check --json --progress` as a task: the
//! checks take minutes, and a full one hours. A check that finds a failure
//! or a warning ends with 1 or 2 and has printed its result, so both are
//! answers ([`Check::ANSWERS`]). Its areas are the words `hardware check`
//! takes, and the check of them is the command's own rule: each once, and
//! with a disk, the disks alone.

use serde::{Deserialize, Serialize};
use serde_json::json;

use super::trial::refusal;
use super::{Action, Args, Bad, Examples, Field, Nothing, Runs, Tests};
use crate::storage::is_disk_name;

/// The area of these actions: the scope a token needs, `hardware`.
const AREA: &str = "hardware";

/// The tests of these actions, which `lab/actions.toml` names.
const TESTS: Tests = Tests {
    checks: "admind::actions::hardware::tests",
    route: "admin-hardware",
    same_as_command: "admin-hardware",
    browser: "web-console/hardware",
    ansible: "web-console/ansible",
};

/// The areas `hardware check` has, in the order it checks them.
const AREAS: [&str; 6] = ["memory", "disks", "cpu", "sensors", "battery", "network"];

/// What a list of areas is made of: the words of [`AREAS`], with one space
/// between them. That each is there once is the rule that only `check`
/// says.
const AREAS_PATTERN: &str = "^(memory|disks|cpu|sensors|battery|network)\
                             ( (memory|disks|cpu|sensors|battery|network))*$";

/// The most characters of a list of areas: all six, with their spaces.
const AREAS_MAX: usize = 40;

/// What a disk's name is made of: the schema's words, which the rule
/// [`is_disk_name`] says exactly.
const DISK_PATTERN: &str = "^[a-z0-9]+$";

/// The most characters of a disk's name.
const DISK_MAX: usize = 32;

/// `hardware show --json`: what the machine is, as the person: its maker
/// and model, processor, memory and firmware.
pub struct Show;

impl Action for Show {
    const ID: &'static str = "hardware.show";
    const AREA: &'static str = AREA;
    const CHANGES: bool = false;
    const PATH: &'static str = "/api/1/hardware";
    const RUNS: Runs = Runs::AsThePerson;
    const TESTS: Tests = TESTS;
    const TITLE: &'static str = "Show what this machine is";
    type Args = Nothing;

    fn argv(_: &Nothing) -> Vec<String> {
        ["hardware", "show", "--json"].map(String::from).to_vec()
    }

    fn examples() -> Examples<Nothing> {
        reads()
    }
}

/// `hardware show --json`, as an administrator: the same document, with
/// the memory's error correction and each of its modules, which only the
/// firmware's tables say.
pub struct Inventory;

impl Action for Inventory {
    const ID: &'static str = "hardware.inventory";
    const AREA: &'static str = AREA;
    const CHANGES: bool = false;
    const PATH: &'static str = "/api/1/hardware/inventory";
    const RUNS: Runs = Runs::AsAdministrator;
    const TESTS: Tests = TESTS;
    const TITLE: &'static str = "Show the memory modules";
    type Args = Nothing;

    fn argv(_: &Nothing) -> Vec<String> {
        ["hardware", "show", "--json"].map(String::from).to_vec()
    }

    fn examples() -> Examples<Nothing> {
        reads()
    }
}

/// `hardware last --json`: the last check that was kept.
pub struct Last;

impl Action for Last {
    const ID: &'static str = "hardware.last";
    const AREA: &'static str = AREA;
    const CHANGES: bool = false;
    const PATH: &'static str = "/api/1/hardware/check";
    const RUNS: Runs = Runs::AsThePerson;
    /// `hardware last` ends with the status the kept check ended with: 0
    /// when it found nothing, 1 for a failure, 2 for a warning, 3 for a
    /// check that could not look at what it was to. With nothing kept it
    /// ends with 3 too, and prints `null`. In each case the output is the
    /// answer.
    const ANSWERS: &'static [i32] = &[0, 1, 2, 3];
    const TESTS: Tests = TESTS;
    const TITLE: &'static str = "Show the last hardware check";
    type Args = Nothing;

    fn argv(_: &Nothing) -> Vec<String> {
        ["hardware", "last", "--json"].map(String::from).to_vec()
    }

    fn examples() -> Examples<Nothing> {
        reads()
    }
}

/// `hardware history --json`: a line for each check that was kept, the
/// oldest first, up to the newest 200.
pub struct History;

impl Action for History {
    const ID: &'static str = "hardware.history";
    const AREA: &'static str = AREA;
    const CHANGES: bool = false;
    const PATH: &'static str = "/api/1/hardware/history";
    const RUNS: Runs = Runs::AsThePerson;
    const TESTS: Tests = TESTS;
    const TITLE: &'static str = "Show the hardware checks by day";
    type Args = Nothing;

    fn argv(_: &Nothing) -> Vec<String> {
        ["hardware", "history", "--json"].map(String::from).to_vec()
    }

    fn examples() -> Examples<Nothing> {
        reads()
    }
}

/// What the reads are tried with: nothing, and any argument is refused.
fn reads() -> Examples<Nothing> {
    Examples { good: vec![Nothing {}], bad: vec![json!({ "all": true })], undo: None }
}

/// What to check: each part optional.
#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Areas {
    /// The areas, as `hardware check` names them, with a space between:
    /// memory, disks, cpu, sensors, battery and network. Left out, all of
    /// them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub areas: Option<String>,
    /// One disk's name, to check that disk alone.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disk: Option<String>,
    /// Whether it is a full check, which takes hours.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub full: bool,
}

impl Areas {
    /// The words of the areas, none for all of them.
    fn words(&self) -> Vec<&str> {
        self.areas.as_deref().map_or_else(Vec::new, |areas| areas.split(' ').collect())
    }
}

impl Args for Areas {
    const FIELDS: &'static [Field] = &[
        Field::text(
            "areas",
            "What to check, as words with a space between: memory, disks, cpu, sensors, battery and network. Left out, all of them.",
            1,
            AREAS_MAX,
            Some(AREAS_PATTERN),
        )
        .optional(),
        Field::text(
            "disk",
            "One disk to check, by the name storage show gives it: vda, sda or nvme0n1. With it the areas are disks alone.",
            1,
            DISK_MAX,
            Some(DISK_PATTERN),
        )
        .optional(),
        Field::flag("full", "Whether to make a full check, which takes hours.").optional(),
    ];

    fn check(&self) -> Result<(), Bad> {
        let words = self.words();
        for (at, word) in words.iter().enumerate() {
            if !AREAS.contains(word) {
                let why = format!("The areas are {}", AREAS.join(", "));
                return Err(Bad { field: "areas", why: refusal(&why) });
            }
            if words[..at].contains(word) {
                let why = format!("Each area is named once, and {word} is named more than once");
                return Err(Bad { field: "areas", why: refusal(&why) });
            }
        }
        let Some(disk) = &self.disk else { return Ok(()) };
        if !is_disk_name(disk) {
            let why =
                "a disk's name is 1 to 32 lower-case letters and digits, as storage show gives it";
            return Err(Bad { field: "disk", why: refusal(why) });
        }
        if words != ["disks"] {
            return Err(Bad {
                field: "areas",
                why: refusal("With a disk, the area is disks and no other"),
            });
        }
        Ok(())
    }
}

/// `hardware check --json --progress [AREA...] [--disk DISK] [--full
/// --yes]`: looks at the memory, disks, processors, sensors, battery and
/// network cards, as a task, and keeps what it found.
pub struct Check;

impl Action for Check {
    const ID: &'static str = "hardware.check";
    const AREA: &'static str = AREA;
    const CHANGES: bool = true;
    const PATH: &'static str = "/api/1/hardware/check";
    const RUNS: Runs = Runs::AsAdministrator;
    const LONG: bool = true;
    /// A check that found a failure ends with 1 and one that found a
    /// warning with 2, and both have printed what they found, which the
    /// kept result says again. 3 is a check that could not run, or that
    /// skipped everything it was to do: an error of its own.
    const ANSWERS: &'static [i32] = &[0, 1, 2];
    const TESTS: Tests = TESTS;
    const TITLE: &'static str = "Check the hardware";
    type Args = Areas;

    fn argv(args: &Areas) -> Vec<String> {
        let mut argv = ["hardware", "check", "--json", "--progress"].map(String::from).to_vec();
        argv.extend(args.words().into_iter().map(String::from));
        if let Some(disk) = &args.disk {
            argv.extend(["--disk".to_string(), disk.clone()]);
        }
        if args.full {
            argv.extend(["--full", "--yes"].map(String::from));
        }
        argv
    }

    fn subject(args: &Areas) -> Option<String> {
        args.disk.clone().or_else(|| args.areas.clone())
    }

    /// Memory first: a machine may have no sensors and no battery, which a
    /// check that has nothing else to look at skips, and ends with 3.
    fn examples() -> Examples<Areas> {
        Examples {
            good: vec![
                Areas { areas: Some("memory".into()), ..Areas::default() },
                Areas { areas: Some("disks".into()), disk: Some("vda".into()), full: false },
                Areas { areas: Some("sensors cpu".into()), ..Areas::default() },
                Areas::default(),
            ],
            bad: vec![
                json!({ "areas": "gpu" }),
                json!({ "areas": "" }),
                json!({ "areas": "Memory" }),
                json!({ "areas": "memory," }),
                json!({ "areas": "memory  cpu" }),
                json!({ "areas": " memory" }),
                json!({ "areas": "memory " }),
                json!({ "areas": "memory memory" }),
                json!({ "areas": "disks cpu disks" }),
                json!({ "areas": 7 }),
                json!({ "areas": "memory", "disk": "vda" }),
                json!({ "areas": "disks cpu", "disk": "vda" }),
                json!({ "disk": "vda" }),
                json!({ "areas": "disks", "disk": "../x" }),
                json!({ "areas": "disks", "disk": "VDA" }),
                json!({ "areas": "disks", "disk": "" }),
                json!({ "areas": "disks", "disk": "x".repeat(DISK_MAX + 1) }),
                json!({ "full": "yes" }),
                json!({ "full": 1 }),
                json!({ "time": 60 }),
                json!({ "yes": true }),
            ],
            undo: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::Value;

    use super::super::{
        Caller, Found, REGISTRY, Registered, authorize, find, query_arguments, schema,
    };
    use super::*;
    use crate::admind::errors::Kind;
    use crate::admind::scopes::Scopes;

    /// The registered action `id`.
    fn action(id: &str) -> &'static dyn Registered {
        *REGISTRY.iter().find(|action| action.id() == id).unwrap()
    }

    /// The command `id` makes for `args`.
    fn argv(id: &str, args: Value) -> Vec<String> {
        action(id).prepare(args).unwrap().argv
    }

    /// Why `id` refuses `args`: the kind, the field and the words.
    fn refused(id: &str, args: Value) -> (Kind, Option<&'static str>, String) {
        let failure = action(id).prepare(args).unwrap_err();
        (failure.kind, failure.field, failure.detail)
    }

    /// Each action of the page: its id, its address, whether it changes
    /// anything, who runs it, and the command its first example makes.
    const ACTIONS: [(&str, &str, bool, Runs, &[&str]); 5] = [
        (
            "hardware.show",
            "/api/1/hardware",
            false,
            Runs::AsThePerson,
            &["hardware", "show", "--json"],
        ),
        (
            "hardware.inventory",
            "/api/1/hardware/inventory",
            false,
            Runs::AsAdministrator,
            &["hardware", "show", "--json"],
        ),
        (
            "hardware.last",
            "/api/1/hardware/check",
            false,
            Runs::AsThePerson,
            &["hardware", "last", "--json"],
        ),
        (
            "hardware.history",
            "/api/1/hardware/history",
            false,
            Runs::AsThePerson,
            &["hardware", "history", "--json"],
        ),
        (
            "hardware.check",
            "/api/1/hardware/check",
            true,
            Runs::AsAdministrator,
            &["hardware", "check", "--json", "--progress", "memory"],
        ),
    ];

    /// The arguments of the first example of `id`.
    fn first_example(id: &str) -> Value {
        action(id).example_values().0.remove(0)
    }

    #[test]
    fn each_action_runs_the_command_the_design_names_for_the_caller_it_names() {
        for (id, path, changes, runs, words) in ACTIONS {
            let action = action(id);
            let prepared = action.prepare(first_example(id)).unwrap();
            assert_eq!(prepared.argv, words, "{id}");
            assert_eq!(
                (action.changes(), action.runs(), action.area(), action.path()),
                (changes, runs, "hardware", path),
                "{id}"
            );
            assert_eq!(prepared.ansible.become_root, runs == Runs::AsAdministrator, "{id}");
            assert_eq!(prepared.ansible.module, "ansible.builtin.command", "{id}");
            assert_eq!(prepared.ansible.params["argv"], json!(words), "{id}");
            // A read changes nothing, and says so to Ansible.
            assert_eq!(prepared.ansible.changed_when.as_deref() == Some("false"), !changes, "{id}");
            assert_eq!(prepared.trial, None, "{id}");
            let tests = action.tests();
            assert_eq!(tests.checks, "admind::actions::hardware::tests", "{id}");
            assert_eq!(tests.route, "admin-hardware", "{id}");
            assert_eq!(tests.same_as_command, "admin-hardware", "{id}");
            assert_eq!(tests.browser, "web-console/hardware", "{id}");
            assert_eq!(tests.ansible, "web-console/ansible", "{id}");
            assert!(!action.lines(), "{id}");
            assert!(!action.secret_answer(), "{id}");
            assert_eq!(action.not_self(), None, "{id}");
            assert_eq!(action.cuts_off_card(), None, "{id}");
        }
        let ours: Vec<_> = REGISTRY.iter().filter(|a| a.id().starts_with("hardware.")).collect();
        assert_eq!(ours.len(), ACTIONS.len());
    }

    #[test]
    fn only_the_check_is_a_task_and_each_says_which_statuses_are_its_own() {
        for (id, ..) in ACTIONS {
            let action = action(id);
            assert_eq!(action.long(), id == "hardware.check", "{id}");
            let answers: &[i32] = match id {
                "hardware.last" => &[0, 1, 2, 3],
                "hardware.check" => &[0, 1, 2],
                _ => &[0],
            };
            assert_eq!(action.answers(), answers, "{id}");
            assert_eq!(action.errors(), &[] as &[&str], "{id}");
        }
        // The Ansible tasks that answer after another status say which.
        let last = action("hardware.last").prepare(json!({})).unwrap();
        assert_eq!(last.ansible.failed_when.as_deref(), Some("answer.rc not in [0, 1, 2, 3]"));
        let check = action("hardware.check").prepare(json!({ "areas": "memory" })).unwrap();
        assert_eq!(check.ansible.failed_when.as_deref(), Some("answer.rc not in [0, 1, 2]"));
    }

    #[test]
    fn nothing_is_compared_because_no_file_of_admind_s_or_probe_is_involved() {
        for (id, ..) in ACTIONS {
            let action = action(id);
            let prepared = action.prepare(first_example(id)).unwrap();
            // What a check writes is under `/system/state/hardware`, which
            // admind may not name: the change always answers `changed`.
            assert!(!action.compares(), "{id}");
            assert_eq!(prepared.probe, None, "{id}");
            assert_eq!(prepared.files, Vec::<String>::new(), "{id}");
            assert_eq!(prepared.not_self, None, "{id}");
            assert!(prepared.cuts_off.is_none(), "{id}");
            assert!(prepared.input.is_none(), "{id}");
        }
    }

    #[test]
    fn a_read_takes_no_arguments_and_the_inventory_is_the_show_as_an_administrator() {
        for id in ["hardware.show", "hardware.inventory", "hardware.last", "hardware.history"] {
            assert!(action(id).fields().is_empty(), "{id}");
            assert_eq!(action(id).prepare(json!({})).unwrap().args, json!({}), "{id}");
            for args in [json!({ "all": true }), json!({ "areas": "memory" }), json!([1])] {
                let failure = action(id).prepare(args.clone()).unwrap_err();
                assert_eq!(failure.kind, Kind::BadRequest, "{id} {args}");
            }
        }
        assert_eq!(
            argv("hardware.show", json!({})),
            argv("hardware.inventory", json!({})),
            "the same command, as another caller"
        );
        assert_ne!(
            action("hardware.show").runs(),
            action("hardware.inventory").runs(),
            "who runs it is the difference"
        );
    }

    #[test]
    fn the_check_names_each_area_it_is_to_look_at_as_the_command_takes_them() {
        let head = ["hardware", "check", "--json", "--progress"];
        let said = |args: Value| argv("hardware.check", args);
        assert_eq!(said(json!({})), head);
        assert_eq!(said(json!({ "areas": "memory" })), [&head[..], &["memory"]].concat());
        assert_eq!(
            said(json!({ "areas": "sensors cpu" })),
            [&head[..], &["sensors", "cpu"]].concat()
        );
        // All six, and in the order they were asked for, which the command
        // reads as it comes.
        assert_eq!(
            said(json!({ "areas": "network battery sensors cpu disks memory" })),
            [&head[..], &["network", "battery", "sensors", "cpu", "disks", "memory"]].concat()
        );
        for area in AREAS {
            assert_eq!(said(json!({ "areas": area })), [&head[..], &[area]].concat(), "{area}");
        }
        assert_eq!(
            said(json!({ "areas": "disks", "disk": "vda" })),
            [&head[..], &["disks", "--disk", "vda"]].concat()
        );
        assert_eq!(said(json!({ "full": true })), [&head[..], &["--full", "--yes"]].concat());
        assert_eq!(said(json!({ "full": false })), head);
        assert_eq!(
            said(json!({ "areas": "disks", "disk": "nvme0n1", "full": true })),
            [&head[..], &["disks", "--disk", "nvme0n1", "--full", "--yes"]].concat()
        );
        assert_eq!(
            said(json!({ "areas": "memory cpu", "full": true })),
            [&head[..], &["memory", "cpu", "--full", "--yes"]].concat()
        );
        // What was asked is kept as it was, and what was left out is not
        // made up. A `null` is a field left out.
        let prepared = action("hardware.check").prepare(json!({ "areas": "memory" })).unwrap();
        assert_eq!(prepared.args, json!({ "areas": "memory" }));
        let prepared = action("hardware.check").prepare(json!({ "areas": null, "disk": null }));
        assert_eq!(prepared.unwrap().argv, head);
        // The task's name says what it looks at.
        let named = |args: Value| action("hardware.check").prepare(args).unwrap().ansible.name;
        assert_eq!(named(json!({})), "Check the hardware");
        assert_eq!(named(json!({ "areas": "sensors cpu" })), "Check the hardware: sensors cpu");
        assert_eq!(named(json!({ "areas": "disks", "disk": "vda" })), "Check the hardware: vda");
    }

    #[test]
    fn an_area_that_is_not_one_or_is_named_twice_is_refused_in_words_the_page_shows() {
        let end = "Nothing was done.";
        let unknown = format!("The areas are memory, disks, cpu, sensors, battery, network. {end}");
        for areas in [
            "gpu",
            "",
            " ",
            "Memory",
            "MEMORY",
            "memory,",
            "memory,cpu",
            "memory  cpu",
            " memory",
            "memory ",
            "memory\n",
            "memory\tcpu",
            "all",
            "disk",
            "memory gpu",
            "cpus",
            "memor",
        ] {
            assert_eq!(
                refused("hardware.check", json!({ "areas": areas })),
                (Kind::BadValue, Some("areas"), unknown.clone()),
                "{areas:?}"
            );
        }
        for (areas, word) in [
            ("memory memory", "memory"),
            ("disks cpu disks", "disks"),
            ("cpu cpu cpu", "cpu"),
            ("memory cpu memory", "memory"),
        ] {
            assert_eq!(
                refused("hardware.check", json!({ "areas": areas })),
                (
                    Kind::BadValue,
                    Some("areas"),
                    format!("Each area is named once, and {word} is named more than once. {end}")
                ),
                "{areas:?}"
            );
        }
        for args in [
            json!({ "areas": 7 }),
            json!({ "areas": ["memory"] }),
            json!({ "areas": true }),
            json!({ "area": "memory" }),
            json!({ "areas": "memory", "time": 60 }),
            json!({ "yes": true }),
        ] {
            let failure = action("hardware.check").prepare(args.clone()).unwrap_err();
            assert_eq!(failure.kind, Kind::BadRequest, "{args}");
        }
    }

    #[test]
    fn a_disk_is_one_to_32_lower_case_letters_and_digits_and_is_checked_alone() {
        for disk in ["vda", "sda", "nvme0n1", "mmcblk0", "a", "x".repeat(32).as_str()] {
            let words = argv("hardware.check", json!({ "areas": "disks", "disk": disk }));
            assert_eq!(&words[4..], ["disks", "--disk", disk], "{disk}");
        }
        let name = "A disk's name is 1 to 32 lower-case letters and digits, as storage show \
                    gives it. Nothing was done.";
        for disk in [
            "../x",
            "VDA",
            "",
            "vda/",
            "v-a",
            "vd a",
            "vda\n",
            "vd\u{e1}",
            "vda\0",
            "x".repeat(33).as_str(),
        ] {
            assert_eq!(
                refused("hardware.check", json!({ "areas": "disks", "disk": disk })),
                (Kind::BadValue, Some("disk"), name.into()),
                "{disk:?}"
            );
        }
        // With a disk the area is the disks and no other, and left out is
        // not that: the command would check them all.
        let alone = "With a disk, the area is disks and no other. Nothing was done.";
        for args in [
            json!({ "disk": "vda" }),
            json!({ "areas": "memory", "disk": "vda" }),
            json!({ "areas": "disks cpu", "disk": "vda" }),
            json!({ "areas": "cpu disks", "disk": "vda" }),
        ] {
            assert_eq!(
                refused("hardware.check", args.clone()),
                (Kind::BadValue, Some("areas"), alone.into()),
                "{args}"
            );
        }
        // The areas are said first when both are wrong.
        let (_, field, _) = refused("hardware.check", json!({ "areas": "gpu", "disk": "../x" }));
        assert_eq!(field, Some("areas"));
        let failure = action("hardware.check").prepare(json!({ "disk": 1 })).unwrap_err();
        assert_eq!(failure.kind, Kind::BadRequest);
    }

    #[test]
    fn what_the_command_refuses_with_a_disk_the_action_refuses_with_the_same_rule() {
        // The command's `--disk` is for a list of areas that has the
        // disks, or none; the action takes the disks alone, which is
        // narrower and is what the page asks for.
        for areas in AREAS {
            let given = json!({ "areas": areas, "disk": "vda" });
            assert_eq!(
                action("hardware.check").prepare(given).is_ok(),
                areas == "disks",
                "{areas}"
            );
        }
    }

    #[test]
    fn the_full_flag_is_true_or_false_and_asks_the_command_to_go_ahead_with_it() {
        for (given, words) in [(json!(true), &["--full", "--yes"][..]), (json!(false), &[])] {
            let words: Vec<String> = words.iter().map(ToString::to_string).collect();
            let said = argv("hardware.check", json!({ "areas": "cpu", "full": given }));
            assert_eq!(said[5..], words, "{given}");
        }
        for given in [json!("true"), json!("yes"), json!(1), json!(0), json!([true])] {
            let failure = action("hardware.check")
                .prepare(json!({ "areas": "cpu", "full": given }))
                .unwrap_err();
            assert_eq!(failure.kind, Kind::BadRequest, "{given}");
        }
    }

    #[test]
    fn each_address_finds_its_action_and_the_check_shares_its_address_with_the_last_result() {
        for (path, id) in [
            ("/api/1/hardware", "hardware.show"),
            ("/api/1/hardware/inventory", "hardware.inventory"),
            ("/api/1/hardware/check", "hardware.last"),
            ("/api/1/hardware/history", "hardware.history"),
        ] {
            let found = find(REGISTRY, false, path);
            let Found::Action(action, params) = found else { panic!("{path}: {found:?}") };
            assert_eq!(action.id(), id, "{path}");
            assert!(params.is_empty(), "{path}");
            // The other method finds the same address, which is `check`'s
            // for the check, and a wrong method for the rest.
            let other = find(REGISTRY, true, path);
            if path == "/api/1/hardware/check" {
                let Found::Action(check, _) = other else { panic!("{path}: {other:?}") };
                assert_eq!(check.id(), "hardware.check");
            } else {
                assert!(matches!(other, Found::WrongMethod), "{path}");
            }
        }
        // There is no address beyond these.
        for path in [
            "/api/1/hardware/",
            "/api/1/hardware/check/",
            "/api/1/hardware/check/memory",
            "/api/1/hardware/inventory/x",
            "/api/1/hardware/histories",
            "/api/1/hardware/..%2Fx",
        ] {
            assert!(matches!(find(REGISTRY, false, path), Found::None), "{path}");
            assert!(matches!(find(REGISTRY, true, path), Found::None), "{path}");
        }
        // A read's arguments, which there are none of, come as the address
        // gives them.
        let Found::Action(show, params) = find(REGISTRY, false, "/api/1/hardware") else {
            panic!("an address with no name")
        };
        assert_eq!(query_arguments(params, "", show.fields()).unwrap(), json!({}));
    }

    #[test]
    fn a_session_reads_what_the_machine_is_and_an_administrator_the_modules_and_the_check() {
        for (id, person, unlocked_admin, locked_admin) in [
            ("hardware.show", true, true, true),
            ("hardware.last", true, true, true),
            ("hardware.history", true, true, true),
            ("hardware.inventory", false, true, false),
            ("hardware.check", false, true, false),
        ] {
            let action = action(id);
            let session = |admin, unlocked| {
                authorize(&Caller::Session { name: "ann", admin, unlocked }, action)
            };
            assert_eq!(session(false, false).is_ok(), person, "{id}");
            assert_eq!(session(false, true).is_ok(), person, "{id}");
            assert_eq!(session(true, true).is_ok(), unlocked_admin, "{id}");
            assert_eq!(session(true, false).is_ok(), locked_admin, "{id}");
        }
        let refused = |id: &str, admin, unlocked| {
            authorize(&Caller::Session { name: "ann", admin, unlocked }, action(id)).unwrap_err()
        };
        assert_eq!(refused("hardware.inventory", false, true).kind, Kind::NotAdmin);
        assert_eq!(refused("hardware.check", false, true).kind, Kind::NotAdmin);
        assert_eq!(refused("hardware.inventory", true, false).kind, Kind::Locked);
        assert_eq!(refused("hardware.check", true, false).kind, Kind::Locked);
    }

    #[test]
    fn a_token_needs_the_hardware_scope_and_for_what_runs_as_an_administrator_the_whole_of_it() {
        let call = |list: &str, id: &str| {
            let scopes = Scopes::parse(list).unwrap();
            authorize(&Caller::Token { name: "ci", scopes: &scopes }, action(id))
        };
        for id in ["hardware.show", "hardware.last", "hardware.history"] {
            for list in ["hardware", "hardware:read", "*"] {
                assert!(call(list, id).is_ok(), "{id} {list}");
            }
            for list in ["storage", "services", "console:read"] {
                assert_eq!(call(list, id).unwrap_err().kind, Kind::OutOfScope, "{id} {list}");
            }
        }
        for id in ["hardware.inventory", "hardware.check"] {
            for list in ["hardware", "*"] {
                assert!(call(list, id).is_ok(), "{id} {list}");
            }
            let refused = call("hardware:read", id).unwrap_err();
            assert_eq!(refused.kind, Kind::OutOfScope, "{id}");
            assert_eq!(
                refused.detail,
                "the API token \"ci\" may not do this: it needs the scope hardware or *",
                "{id}"
            );
            assert_eq!(call("storage", id).unwrap_err().kind, Kind::OutOfScope, "{id}");
        }
    }

    #[test]
    fn the_schema_says_the_areas_the_disk_and_that_none_is_required() {
        let schema = schema(action("hardware.check").fields());
        assert_eq!(schema["required"], json!([]));
        assert_eq!(schema["additionalProperties"], false);
        let areas = &schema["properties"]["areas"];
        assert_eq!((&areas["minLength"], &areas["maxLength"]), (&json!(1), &json!(40)));
        assert_eq!(areas["pattern"], AREAS_PATTERN);
        let disk = &schema["properties"]["disk"];
        assert_eq!((&disk["minLength"], &disk["maxLength"]), (&json!(1), &json!(32)));
        assert_eq!(disk["pattern"], "^[a-z0-9]+$");
        assert_eq!(schema["properties"]["full"]["type"], "boolean");
        for id in ["hardware.show", "hardware.inventory", "hardware.last", "hardware.history"] {
            assert_eq!(super::super::schema(action(id).fields())["properties"], json!({}), "{id}");
        }
    }

    #[test]
    fn the_areas_pattern_is_made_of_the_words_the_command_has_and_is_as_long_as_all_of_them() {
        let words = AREAS.join("|");
        assert_eq!(AREAS_PATTERN, format!("^({words})( ({words}))*$"));
        assert_eq!(AREAS.join(" ").len(), AREAS_MAX);
        // The names are the command's own.
        for area in AREAS {
            assert!(crate::hardware::check::Area::parse(area).is_some(), "{area}");
        }
        let known: Vec<&str> = crate::hardware::check::Area::ALL.iter().map(|a| a.name()).collect();
        assert_eq!(known, AREAS);
    }

    #[test]
    fn the_examples_work_and_the_bad_ones_do_not() {
        for (id, ..) in ACTIONS {
            let action = action(id);
            let (good, bad, undo) = action.example_values();
            assert!(!bad.is_empty(), "{id}");
            assert!(undo.is_none(), "{id}: a check leaves nothing to put back");
            for args in &good {
                assert!(action.prepare(args.clone()).is_ok(), "{id}");
            }
            for args in bad {
                assert!(action.prepare(args.clone()).is_err(), "{id} {args}");
            }
        }
        // The first is what the generic tests run, and it is a check a
        // machine without sensors or a battery can pass.
        assert_eq!(first_example("hardware.check"), json!({ "areas": "memory" }));
        let (good, bad, _) = action("hardware.check").example_values();
        assert!(good.contains(&json!({ "areas": "sensors cpu" })));
        assert!(good.contains(&json!({ "areas": "disks", "disk": "vda" })));
        assert!(good.contains(&json!({})));
        for args in [
            json!({ "areas": "gpu" }),
            json!({ "areas": "memory", "disk": "vda" }),
            json!({ "disk": "vda" }),
        ] {
            assert!(bad.contains(&args), "{args}");
        }
        let (_, bad, _) = action("hardware.show").example_values();
        assert_eq!(bad, [json!({ "all": true })]);
    }
}
