// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The Security page's actions (ADR 0514, ADR 0562): the sign-in rules,
//! and the addresses that too many wrong passwords have blocked. The area
//! is `security`, the scope a token needs, which `checks.last` and
//! `checks.run` (`checks.rs`) are in too.
//!
//! The sign-in rules are in `admin.toml`, which every account may read:
//! `security.sign-in` runs `admin settings --json` as the person and gives
//! the whole of it, and the page reads the rules from there.
//! `security.sign-in-set` runs `admin sign-in` as an administrator, as
//! `elevate admin sign-in ...` does at a terminal, and checks the values
//! as the command does. It changes the file the console's trial covers
//! (`console.reach` with `try`), so while a change is being tried the
//! command refuses, and the error is ADM-138.
//!
//! The blocks are authd's: the list, lifting one, and the networks that
//! are never blocked all run `firewall blocked`, `unblock` and
//! `never-block` as an administrator, which ask authd. The address or
//! network is read as the command reads it ([`Blocks::parse`]), so that
//! the page, the API and the terminal refuse the same values with the
//! same words. The blocks are in a file only authd opens, which admind's
//! service does not see, so an unblock names no file; the never-block list
//! is in `blocking.toml`, which the journal keeps.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::trial::refusal;
use super::{Action, Args, Bad, Examples, Field, Nothing, Runs, Tests, Undo};
use crate::admin_settings::{self, SecondStep};
use crate::authd::blocking;
use crate::firewall::Blocks;

/// The area of these actions: the scope a token needs, `security`.
const AREA: &str = "security";

/// The tests of these actions, which `lab/actions.toml` names: the
/// Security page's boot and lab tests call their routes.
const TESTS: Tests = Tests {
    checks: "admind::actions::security::tests",
    route: "admin-security",
    same_as_command: "admin-security",
    browser: "web-console/security",
    ansible: "web-console/ansible",
};

/// The words `second_step` is: who gives a second step at sign-in.
const SECOND_STEPS: [&str; 3] = ["if-set", "administrators", "everyone"];

/// The fewest and most of the hours a session lasts, and of the minutes it
/// lasts with no page open: `admin sign-in`'s own ranges, which a test
/// holds these to.
const HOURS: (i64, i64) = (1, 168);
const MINUTES: (i64, i64) = (5, 1440);

/// What the schema says of an address or a network: the characters of
/// one, and a prefix. The command's own parse is the exact rule.
const ADDRESS_PATTERN: &str = "^[0-9A-Fa-f:.]+(/[0-9]+)?$";

/// The most characters of an address or a network: an IPv6 network and
/// its prefix, with room, as the firewall's actions give.
const ADDRESS_MAX: usize = 49;

/// `admin settings --json`: what `admin.toml` says, of which the page
/// reads the sign-in rules.
pub struct SignIn;

impl Action for SignIn {
    const ID: &'static str = "security.sign-in";
    const AREA: &'static str = AREA;
    const CHANGES: bool = false;
    const PATH: &'static str = "/api/1/security/sign-in";
    const RUNS: Runs = Runs::AsThePerson;
    const TESTS: Tests = TESTS;
    const TITLE: &'static str = "Show the sign-in rules";
    type Args = Nothing;

    fn argv(_: &Nothing) -> Vec<String> {
        ["admin", "settings", "--json"].map(String::from).to_vec()
    }

    fn examples() -> Examples<Nothing> {
        Examples {
            good: vec![Nothing {}],
            bad: vec![json!({ "json": true }), json!({ "all": true })],
            undo: None,
        }
    }
}

/// The sign-in rules to set: those given, and the rest left as they are.
#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Rules {
    /// Who gives a second step: `if-set`, `administrators` or `everyone`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub second_step: Option<String>,
    /// The hours a session lasts after its sign-in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_hours: Option<i64>,
    /// The minutes a session lasts with no console page open.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub idle_minutes: Option<i64>,
}

/// `value`, which is in `range`, or the refusal of `field` with `why`.
fn within(
    field: &'static str,
    value: Option<i64>,
    range: (i64, i64),
    why: &str,
) -> Result<(), Bad> {
    match value {
        Some(value) if !(range.0..=range.1).contains(&value) => {
            Err(Bad { field, why: refusal(why) })
        }
        _ => Ok(()),
    }
}

impl Args for Rules {
    const FIELDS: &'static [Field] = &[
        Field::one_of(
            "second_step",
            "Who gives a second step at sign-in: if-set for those who have added one, \
             administrators for them too, everyone.",
            &SECOND_STEPS,
        )
        .optional(),
        Field::number(
            "session_hours",
            "The hours a session lasts after its sign-in, 1 to 168.",
            HOURS.0,
            HOURS.1,
        )
        .optional(),
        Field::number(
            "idle_minutes",
            "The minutes a session lasts with no console page open, 5 to 1440.",
            MINUTES.0,
            MINUTES.1,
        )
        .optional(),
    ];

    fn check(&self) -> Result<(), Bad> {
        if self.second_step.is_none() && self.session_hours.is_none() && self.idle_minutes.is_none()
        {
            let why = "Nothing to change: say at least one of second_step, session_hours and \
                       idle_minutes.";
            return Err(Bad { field: "second_step", why: why.into() });
        }
        if let Some(step) = &self.second_step
            && SecondStep::parse(step).is_none()
        {
            let why = "The second step is if-set, administrators or everyone";
            return Err(Bad { field: "second_step", why: refusal(why) });
        }
        within(
            "session_hours",
            self.session_hours,
            HOURS,
            "A session lasts 1 to 168 hours after its sign-in",
        )?;
        within(
            "idle_minutes",
            self.idle_minutes,
            MINUTES,
            "A session may have no page open for 5 to 1440 minutes",
        )
    }
}

/// `admin sign-in [--second-step W] [--session-hours N] [--idle-minutes
/// N]`: sets the sign-in rules given, in `admin.toml`, and has admind read
/// them at once.
pub struct SignInSet;

impl Action for SignInSet {
    const ID: &'static str = "security.sign-in-set";
    const AREA: &'static str = AREA;
    const CHANGES: bool = true;
    const PATH: &'static str = "/api/1/security/sign-in";
    const RUNS: Runs = Runs::AsAdministrator;
    const ERRORS: &'static [&'static str] = &["ADM-138"];
    const TESTS: Tests = TESTS;
    const TITLE: &'static str = "Set the sign-in rules";
    type Args = Rules;

    fn argv(args: &Rules) -> Vec<String> {
        let mut argv = vec!["admin".to_string(), "sign-in".into()];
        if let Some(step) = &args.second_step {
            argv.extend(["--second-step".to_string(), step.clone()]);
        }
        if let Some(hours) = args.session_hours {
            argv.extend(["--session-hours".to_string(), hours.to_string()]);
        }
        if let Some(minutes) = args.idle_minutes {
            argv.extend(["--idle-minutes".to_string(), minutes.to_string()]);
        }
        argv
    }

    fn files(_: &Rules) -> Vec<String> {
        vec![admin_settings::PATH.into()]
    }

    /// `admin settings --json`, which every account may run.
    fn probe(_: &Rules) -> Option<Vec<String>> {
        Some(["admin", "settings", "--json"].map(String::from).to_vec())
    }

    /// The `sign_in` of the settings: the rules, as the file has them.
    fn probed(_: &Rules, output: &str) -> Option<String> {
        let document: Value = serde_json::from_str(output).ok()?;
        Some(document.get("sign_in")?.to_string())
    }

    /// The rules are as asked already when each one given is the one in
    /// force, which `now`, the `sign_in` that [`Self::probed`] picks, says.
    fn settled(args: &Rules, now: &str) -> Option<bool> {
        let now: Value = serde_json::from_str(now).ok()?;
        let step = now.get("second_step")?.as_str()?;
        let hours = now.get("session_hours")?.as_i64()?;
        let minutes = now.get("idle_minutes")?.as_i64()?;
        Some(
            args.second_step.as_deref().is_none_or(|asked| asked == step)
                && args.session_hours.is_none_or(|asked| asked == hours)
                && args.idle_minutes.is_none_or(|asked| asked == minutes),
        )
    }

    fn examples() -> Examples<Rules> {
        // The default is 12 hours, so 8 is a change; what puts it back is
        // the default, which a test holds to `admin_settings`'s.
        Examples {
            good: vec![Rules { session_hours: Some(8), ..Rules::default() }],
            bad: vec![
                json!({}),
                json!({ "session_hours": 0 }),
                json!({ "session_hours": 169 }),
                json!({ "idle_minutes": 4 }),
                json!({ "idle_minutes": 1441 }),
                json!({ "second_step": "some" }),
                json!({ "second_step": "Everyone" }),
                json!({ "session_hours": "8" }),
                json!({ "session_hours": 8.5 }),
                json!({ "session_hours": 8, "session-hours": 9 }),
            ],
            undo: Some(Undo { action: Self::ID, args: json!({ "session_hours": 12 }) }),
        }
    }
}

/// `firewall blocked --json`: the addresses blocked now, with the
/// settings of the blocks.
pub struct Blocked;

impl Action for Blocked {
    const ID: &'static str = "security.blocked";
    const AREA: &'static str = AREA;
    const CHANGES: bool = false;
    const PATH: &'static str = "/api/1/security/blocked";
    const RUNS: Runs = Runs::AsAdministrator;
    const TESTS: Tests = TESTS;
    const TITLE: &'static str = "List the blocked addresses";
    type Args = Nothing;

    fn argv(_: &Nothing) -> Vec<String> {
        ["firewall", "blocked", "--json"].map(String::from).to_vec()
    }

    fn examples() -> Examples<Nothing> {
        Examples {
            good: vec![Nothing {}],
            bad: vec![json!({ "address": "203.0.113.9" }), json!({ "all": true })],
            undo: None,
        }
    }
}

/// The address of a block, which `unblock` lifts.
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Address {
    /// An IPv4 address, or an IPv6 address or its network of 64 bits.
    pub address: String,
}

impl Args for Address {
    const FIELDS: &'static [Field] = &[Field::text(
        "address",
        "The address that is blocked, as 203.0.113.9: an IPv4 address, or an IPv6 address or its \
         network of 64 bits, as 2001:db8:4:7::/64.",
        1,
        ADDRESS_MAX,
        Some(ADDRESS_PATTERN),
    )];

    fn check(&self) -> Result<(), Bad> {
        Blocks::parse(&["unblock", &self.address])
            .map(drop)
            .map_err(|why| Bad { field: "address", why: refusal(&why) })
    }
}

/// `firewall unblock ADDRESS`: lifts the block of an address, so that it
/// is let in at once.
pub struct Unblock;

impl Action for Unblock {
    const ID: &'static str = "security.unblock";
    const AREA: &'static str = AREA;
    const CHANGES: bool = true;
    const PATH: &'static str = "/api/1/security/unblock";
    const RUNS: Runs = Runs::AsAdministrator;
    const TESTS: Tests = TESTS;
    const TITLE: &'static str = "Unblock an address";
    type Args = Address;

    fn argv(args: &Address) -> Vec<String> {
        ["firewall", "unblock", args.address.as_str()].map(String::from).to_vec()
    }

    fn subject(args: &Address) -> Option<String> {
        Some(args.address.clone())
    }

    /// There is no undo: nothing puts a block back that the wrong
    /// passwords did not make.
    fn examples() -> Examples<Address> {
        Examples {
            good: vec![Address { address: "203.0.113.9".into() }],
            bad: vec![
                json!({}),
                json!({ "address": "" }),
                json!({ "address": "300.1.1.1" }),
                json!({ "address": "192.0.2.0/24" }),
                json!({ "address": "127.0.0.1" }),
                json!({ "address": "blocked" }),
                json!({ "address": "203.0.113.9", "network": "203.0.113.0/24" }),
            ],
            undo: None,
        }
    }
}

/// A network that the blocks leave alone, or let block again.
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Network {
    /// An address, or a network as `192.0.2.0/24`.
    pub network: String,
}

/// The field of a network, which both `never-block` actions take.
const NETWORK: Field = Field::text(
    "network",
    "The network, as 192.0.2.0/24 or 2001:db8::/48, or one address alone.",
    1,
    ADDRESS_MAX,
    Some(ADDRESS_PATTERN),
);

/// What a network of the never-block list must be: a network as the
/// command reads it, which `words` puts in its place after the words that
/// make the command.
fn check_network(network: &str, remove: bool) -> Result<(), Bad> {
    let words: &[&str] =
        if remove { &["never-block", "remove", network] } else { &["never-block", network] };
    Blocks::parse(words).map(drop).map_err(|why| Bad { field: "network", why: refusal(&why) })
}

impl Args for Network {
    const FIELDS: &'static [Field] = &[NETWORK];

    fn check(&self) -> Result<(), Bad> {
        check_network(&self.network, false)
    }
}

/// The same, for the action that takes a network out of the list: it is
/// a type of its own, so that each action checks the command it runs.
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Taken {
    /// An address, or a network as `192.0.2.0/24`.
    pub network: String,
}

impl Args for Taken {
    const FIELDS: &'static [Field] = &[NETWORK];

    fn check(&self) -> Result<(), Bad> {
        check_network(&self.network, true)
    }
}

/// The network the examples use: a documentation network, which no
/// machine of the tests is on.
const EXAMPLE_NETWORK: &str = "192.0.2.0/24";

/// Whether blocking.toml, which holds `text` (None when there is no file),
/// lists `network` in `never`: None when the file or the network does not
/// read, which the command refuses and check mode cannot tell of.
fn never_lists(network: &str, text: Option<&str>) -> Option<bool> {
    let network = blocking::Network::parse(network).ok()?;
    let settings = blocking::Settings::parse(text.unwrap_or_default()).ok()?;
    Some(settings.never.contains(&network))
}

/// `firewall never-block NETWORK`: keeps a network out of the blocks, however
/// many wrong passwords come from it. What puts it back is the removal.
pub struct NeverBlock;

impl Action for NeverBlock {
    const ID: &'static str = "security.never-block";
    const AREA: &'static str = AREA;
    const CHANGES: bool = true;
    const PATH: &'static str = "/api/1/security/never-block";
    const RUNS: Runs = Runs::AsAdministrator;
    const TESTS: Tests = TESTS;
    const TITLE: &'static str = "Never block a network";
    type Args = Network;

    fn argv(args: &Network) -> Vec<String> {
        ["firewall", "never-block", args.network.as_str()].map(String::from).to_vec()
    }

    fn subject(args: &Network) -> Option<String> {
        Some(args.network.clone())
    }

    fn files(_: &Network) -> Vec<String> {
        vec![blocking::SETTINGS.into()]
    }

    /// authd writes nothing when `never` lists the network already.
    fn keeps_file(args: &Network, _: &str, text: Option<&str>) -> Option<bool> {
        never_lists(&args.network, text)
    }

    fn examples() -> Examples<Network> {
        Examples {
            good: vec![Network { network: EXAMPLE_NETWORK.into() }],
            bad: vec![
                json!({}),
                json!({ "network": "300.1.1.1" }),
                json!({ "network": "192.0.2.5/24" }),
                json!({ "network": "192.0.2.0/33" }),
                json!({ "network": "remove" }),
                json!({ "network": "" }),
                json!({ "network": "192.0.2.0/24", "address": "192.0.2.1" }),
            ],
            undo: Some(Undo {
                action: "security.never-block-remove",
                args: json!({ "network": EXAMPLE_NETWORK }),
            }),
        }
    }
}

/// `firewall never-block remove NETWORK`: lets the blocks count a network
/// again.
pub struct NeverBlockRemove;

impl Action for NeverBlockRemove {
    const ID: &'static str = "security.never-block-remove";
    const AREA: &'static str = AREA;
    const CHANGES: bool = true;
    const PATH: &'static str = "/api/1/security/never-block/remove";
    const RUNS: Runs = Runs::AsAdministrator;
    const TESTS: Tests = TESTS;
    const TITLE: &'static str = "Block a network again";
    type Args = Taken;

    fn argv(args: &Taken) -> Vec<String> {
        ["firewall", "never-block", "remove", args.network.as_str()].map(String::from).to_vec()
    }

    fn subject(args: &Taken) -> Option<String> {
        Some(args.network.clone())
    }

    fn files(_: &Taken) -> Vec<String> {
        vec![blocking::SETTINGS.into()]
    }

    /// authd writes nothing when `never` does not list the network.
    fn keeps_file(args: &Taken, _: &str, text: Option<&str>) -> Option<bool> {
        never_lists(&args.network, text).map(|listed| !listed)
    }

    fn examples() -> Examples<Taken> {
        Examples {
            good: vec![Taken { network: EXAMPLE_NETWORK.into() }],
            bad: vec![
                json!({}),
                json!({ "network": "300.1.1.1" }),
                json!({ "network": "192.0.2.5/24" }),
                json!({ "network": "remove" }),
                json!({ "network": "" }),
                json!({ "network": "192.0.2.0/24", "address": "192.0.2.1" }),
            ],
            undo: Some(Undo {
                action: "security.never-block",
                args: json!({ "network": EXAMPLE_NETWORK }),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::{Caller, REGISTRY, Registered, authorize};
    use super::*;
    use crate::admin_settings::{IDLE_MINUTES, SESSION_HOURS, SignIn as Rule};
    use crate::admind::errors::Kind;
    use crate::admind::scopes::Scopes;

    /// The registered action `id`.
    fn action(id: &str) -> &'static dyn Registered {
        *REGISTRY.iter().find(|action| action.id() == id).unwrap()
    }

    /// The failure of `args` for `id`: its kind, its field and its words.
    fn refused(id: &str, args: Value) -> (Kind, Option<&'static str>, String) {
        let failure = action(id).prepare(args).unwrap_err();
        (failure.kind, failure.field, failure.detail)
    }

    #[test]
    fn the_two_reads_give_the_settings_and_the_blocks_as_json() {
        for (id, path, runs, argv, title) in [
            (
                "security.sign-in",
                "/api/1/security/sign-in",
                Runs::AsThePerson,
                &["admin", "settings", "--json"][..],
                "Show the sign-in rules",
            ),
            (
                "security.blocked",
                "/api/1/security/blocked",
                Runs::AsAdministrator,
                &["firewall", "blocked", "--json"][..],
                "List the blocked addresses",
            ),
        ] {
            let action = action(id);
            assert_eq!(
                (action.changes(), action.runs(), action.area(), action.long()),
                (false, runs, "security", false),
                "{id}"
            );
            assert_eq!((action.path(), action.title(), action.posts()), (path, title, false));
            assert!(action.errors().is_empty() && !action.compares(), "{id}");
            assert_eq!(action.fields().len(), 0, "{id}");
            let prepared = action.prepare(json!({})).unwrap();
            assert_eq!(prepared.argv, argv, "{id}");
            assert_eq!((prepared.probe, prepared.files.len()), (None, 0), "{id}");
            assert_eq!(prepared.ansible.changed_when.as_deref(), Some("false"));
            assert_eq!(prepared.ansible.become_root, runs == Runs::AsAdministrator);
            let (good, bad, undo) = action.example_values();
            assert_eq!((good, undo), (vec![json!({})], None), "{id}");
            for args in bad {
                assert_eq!(action.prepare(args).unwrap_err().kind, Kind::BadRequest, "{id}");
            }
        }
    }

    #[test]
    fn the_rules_go_to_the_command_in_its_own_order_and_only_those_given() {
        let id = "security.sign-in-set";
        let set = |args: Value| action(id).prepare(args).unwrap().argv;
        assert_eq!(
            set(json!({ "session_hours": 8 })),
            ["admin", "sign-in", "--session-hours", "8"]
        );
        assert_eq!(
            set(json!({ "idle_minutes": 60, "second_step": "everyone", "session_hours": 24 })),
            [
                "admin",
                "sign-in",
                "--second-step",
                "everyone",
                "--session-hours",
                "24",
                "--idle-minutes",
                "60"
            ]
        );
        assert_eq!(
            set(json!({ "second_step": "if-set" })),
            ["admin", "sign-in", "--second-step", "if-set"]
        );
        let (min, max) = (HOURS.0, HOURS.1);
        assert_eq!(set(json!({ "session_hours": min }))[3], "1");
        assert_eq!(set(json!({ "session_hours": max }))[3], "168");
        assert_eq!(set(json!({ "idle_minutes": MINUTES.0 }))[3], "5");
        assert_eq!(set(json!({ "idle_minutes": MINUTES.1 }))[3], "1440");
        let prepared = action(id).prepare(json!({ "session_hours": 8 })).unwrap();
        assert_eq!(prepared.args, json!({ "session_hours": 8 }));
        assert_eq!(prepared.ansible.name, "Set the sign-in rules");
        assert!(prepared.ansible.become_root);
    }

    #[test]
    fn a_rule_out_of_its_range_or_none_at_all_is_refused_with_its_field_named() {
        let id = "security.sign-in-set";
        let nothing = "Nothing to change: say at least one of second_step, session_hours and \
                       idle_minutes.";
        assert_eq!(refused(id, json!({})), (Kind::BadValue, Some("second_step"), nothing.into()));
        let hours = "A session lasts 1 to 168 hours after its sign-in";
        let minutes = "A session may have no page open for 5 to 1440 minutes";
        let step = "The second step is if-set, administrators or everyone";
        for (args, field, words) in [
            (json!({ "session_hours": 0 }), "session_hours", hours),
            (json!({ "session_hours": 169 }), "session_hours", hours),
            (json!({ "session_hours": -1 }), "session_hours", hours),
            (json!({ "idle_minutes": 4 }), "idle_minutes", minutes),
            (json!({ "idle_minutes": 1441 }), "idle_minutes", minutes),
            (json!({ "second_step": "some" }), "second_step", step),
            (json!({ "second_step": "" }), "second_step", step),
            (json!({ "second_step": "Everyone" }), "second_step", step),
        ] {
            assert_eq!(
                refused(id, args.clone()),
                (Kind::BadValue, Some(field), format!("{words}. Nothing was done.")),
                "{args}"
            );
        }
        // A value of the wrong type is the types' refusal, not a rule's.
        for args in [
            json!({ "session_hours": "8" }),
            json!({ "session_hours": 8.5 }),
            json!({ "session_hours": null, "idle_minutes": "x" }),
            json!({ "second_step": 1 }),
            json!({ "session_hours": 8, "extra": 1 }),
        ] {
            assert_eq!(
                action(id).prepare(args.clone()).unwrap_err().kind,
                Kind::BadRequest,
                "{args}"
            );
        }
    }

    #[test]
    fn the_ranges_and_the_words_are_the_command_s() {
        assert_eq!(
            (HOURS.0, HOURS.1),
            (i64::from(*SESSION_HOURS.start()), i64::from(*SESSION_HOURS.end()))
        );
        assert_eq!(
            (MINUTES.0, MINUTES.1),
            (i64::from(*IDLE_MINUTES.start()), i64::from(*IDLE_MINUTES.end()))
        );
        for word in SECOND_STEPS {
            assert!(SecondStep::parse(word).is_some(), "{word}");
        }
        for step in [SecondStep::IfSet, SecondStep::Administrators, SecondStep::Everyone] {
            assert!(SECOND_STEPS.contains(&step.word()), "{}", step.word());
        }
        assert_eq!(SECOND_STEPS.len(), 3);
    }

    #[test]
    fn the_undo_of_the_example_is_the_rule_the_console_has_when_the_file_says_nothing() {
        let (good, _, undo) = action("security.sign-in-set").example_values();
        assert_eq!(good, [json!({ "session_hours": 8 })]);
        let default = Rule::default();
        assert_eq!(default.session_hours, 12);
        assert_ne!(default.session_hours, 8, "the example changes the default");
        let undo = undo.unwrap();
        assert_eq!(undo.action, "security.sign-in-set");
        assert_eq!(undo.args, json!({ "session_hours": default.session_hours }));
        // The undo is itself an action that reads, and the first example
        // is not the undo's.
        assert!(action("security.sign-in-set").prepare(undo.args).is_ok());
    }

    #[test]
    fn the_sign_in_rules_are_compared_in_the_settings_the_file_has() {
        let action = action("security.sign-in-set");
        assert!(action.compares());
        assert_eq!(action.errors(), ["ADM-138"]);
        let prepared = action.prepare(json!({ "session_hours": 8 })).unwrap();
        assert_eq!(prepared.files, ["/system/settings/admin.toml"]);
        assert_eq!(
            prepared.probe.as_deref(),
            Some(&["admin".to_string(), "settings".into(), "--json".into()][..])
        );
        let printed = |hours: u32, reach: &str| {
            format!(
                r#"{{"format":1,"sign_in":{{"second_step":"if-set","session_hours":{hours},"idle_minutes":30}},"console":{{"listen":9090,"reach":"{reach}","certificate":"c"}},"terminal":{{"allowed":"nobody","sessions_per_account":4}},"wrong":null}}"#
            )
        };
        let picked = |output: &str| action.probed(&prepared.args, output);
        assert_eq!(
            picked(&printed(12, "private")).as_deref(),
            Some(r#"{"idle_minutes":30,"second_step":"if-set","session_hours":12}"#)
        );
        assert_ne!(picked(&printed(12, "private")), picked(&printed(8, "private")));
        // What is not a sign-in rule is not compared.
        assert_eq!(picked(&printed(12, "private")), picked(&printed(12, "any")));
        for output in ["", "{}", "[]", "not json", r#"{"console":{}}"#] {
            assert_eq!(picked(output), None, "{output:?}");
        }
        // Check mode: the rules given are the ones in force, or one is not.
        let now = picked(&printed(12, "private")).unwrap();
        let settled = |args: Value| action.settled(&action.prepare(args).unwrap().args, &now);
        assert_eq!(settled(json!({ "session_hours": 8 })), Some(false));
        assert_eq!(settled(json!({ "session_hours": 12 })), Some(true));
        assert_eq!(settled(json!({ "session_hours": 12, "idle_minutes": 30 })), Some(true));
        assert_eq!(settled(json!({ "session_hours": 12, "second_step": "everyone" })), Some(false));
        assert_eq!(settled(json!({ "second_step": "if-set" })), Some(true));
        assert_eq!(action.settled(&prepared.args, "not json"), None);
    }

    #[test]
    fn an_address_is_read_as_the_command_reads_it() {
        let id = "security.unblock";
        for address in [
            "203.0.113.9",
            "10.0.0.1",
            "2001:db8:4:7::/64",
            "2001:db8:4:7::9",
            "::ffff:203.0.113.9",
        ] {
            let prepared = action(id).prepare(json!({ "address": address })).unwrap();
            assert_eq!(prepared.argv, ["firewall", "unblock", address], "{address}");
            // The blocks are in a file of authd's, which admind does not see.
            assert!(prepared.files.is_empty());
            assert_eq!(prepared.ansible.name, format!("Unblock an address: {address}"));
        }
        for address in [
            "300.1.1.1",
            "192.0.2.0/24",
            "203.0.113.9/32x",
            "127.0.0.1",
            "::1",
            "0.0.0.0",
            "2001:db8::/48",
            "blocked",
            "-1.2.3.4",
            "203.0.113.9 203.0.113.10",
            "",
        ] {
            let (kind, field, words) = refused(id, json!({ "address": address }));
            assert_eq!((kind, field), (Kind::BadValue, Some("address")), "{address:?}");
            assert!(words.ends_with(". Nothing was done."), "{words}");
        }
        let (_, _, words) = refused(id, json!({ "address": "300.1.1.1" }));
        assert!(words.starts_with("`300.1.1.1` is not an address that is blocked"), "{words}");
        // It has no undo, and compares nothing: the file of the blocks is
        // authd's, which admind does not see.
        let action = action(id);
        assert!(!action.compares() && action.errors().is_empty());
        assert_eq!(action.example_values().2, None);
        assert_eq!(action.prepare(json!({ "address": "203.0.113.9" })).unwrap().probe, None);
    }

    #[test]
    fn a_network_is_read_as_the_command_reads_it_and_the_two_undo_each_other() {
        for (id, argv, other) in [
            (
                "security.never-block",
                &["firewall", "never-block"][..],
                "security.never-block-remove",
            ),
            (
                "security.never-block-remove",
                &["firewall", "never-block", "remove"][..],
                "security.never-block",
            ),
        ] {
            let action = action(id);
            for network in ["192.0.2.0/24", "192.0.2.7", "2001:db8::/48", "10.0.0.0/8", "0.0.0.0/0"]
            {
                let prepared = action.prepare(json!({ "network": network })).unwrap();
                let mut want = argv.to_vec();
                want.push(network);
                assert_eq!(prepared.argv, want, "{id} {network}");
                assert_eq!(prepared.files, ["/system/settings/blocking.toml"]);
            }
            for network in [
                "300.1.1.1",
                "192.0.2.5/24",
                "192.0.2.0/33",
                "192.0.2.0/",
                "remove",
                "",
                "a/b",
                "::g",
            ] {
                let (kind, field, words) = refused(id, json!({ "network": network }));
                assert_eq!((kind, field), (Kind::BadValue, Some("network")), "{id} {network:?}");
                assert!(words.ends_with(". Nothing was done."), "{words}");
            }
            assert!(action.compares() && action.errors().is_empty(), "{id}");
            let (good, _, undo) = action.example_values();
            assert_eq!(good, [json!({ "network": "192.0.2.0/24" })]);
            let undo = undo.unwrap();
            assert_eq!((undo.action, undo.args), (other, json!({ "network": "192.0.2.0/24" })));
        }
        let (_, _, words) = refused("security.never-block", json!({ "network": "192.0.2.5/24" }));
        assert!(words.contains("bits set past its prefix"), "{words}");
    }

    #[test]
    fn check_mode_reads_the_never_list_that_each_leaves_as_it_is() {
        let path = "/system/settings/blocking.toml";
        let (add, remove) = (action("security.never-block"), action("security.never-block-remove"));
        let args = |action: &'static dyn Registered| {
            action.prepare(json!({ "network": "192.0.2.0/24" })).unwrap().args
        };
        let (added, removed) = (args(add), args(remove));
        let listed = "never = [\"10.0.0.0/8\", \"192.0.2.0/24\"]\n";
        for text in [None, Some(""), Some("never = [\"10.0.0.0/8\"]\n")] {
            assert_eq!(add.keeps_file(&added, path, text), Some(false), "{text:?}");
            assert_eq!(remove.keeps_file(&removed, path, text), Some(true), "{text:?}");
        }
        assert_eq!(add.keeps_file(&added, path, Some(listed)), Some(true));
        assert_eq!(remove.keeps_file(&removed, path, Some(listed)), Some(false));
        // One address is a network of its own: 192.0.2.7 is not 192.0.2.0/24.
        let one = add.prepare(json!({ "network": "192.0.2.7" })).unwrap().args;
        assert_eq!(add.keeps_file(&one, path, Some(listed)), Some(false));
        // A file that does not read is refused by the command: check mode cannot tell.
        assert_eq!(add.keeps_file(&added, path, Some("never = 3\n")), None);
    }

    #[test]
    fn a_token_needs_the_security_scope_for_the_changes_and_for_the_blocks() {
        let call = |list: &str, id: &str| {
            let scopes = Scopes::parse(list).unwrap();
            authorize(&Caller::Token { name: "ci", scopes: &scopes }, action(id))
        };
        // The sign-in rules are the person's read; the rest runs as an
        // administrator, which a token may do only with the full scope.
        for list in ["security", "security:read", "*"] {
            assert!(call(list, "security.sign-in").is_ok(), "{list}");
        }
        for id in [
            "security.sign-in-set",
            "security.blocked",
            "security.unblock",
            "security.never-block",
            "security.never-block-remove",
        ] {
            for list in ["security", "*"] {
                assert!(call(list, id).is_ok(), "{id} {list}");
            }
            for list in ["security:read", "console", "firewall"] {
                let refused = call(list, id).unwrap_err();
                assert_eq!(refused.kind, Kind::OutOfScope, "{id} {list}");
                assert_eq!(
                    refused.detail,
                    "the API token \"ci\" may not do this: it needs the scope security or *"
                );
            }
        }
    }
}
