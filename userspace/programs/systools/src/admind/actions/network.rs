// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The networking actions (ADR 0518): the cards and their settings, the
//! connections, and changing a card's settings with a window to undo them.
//! The area is `network`, the scope a token needs.
//!
//! `network.show` and `network.connections` run `network show --json` and
//! `ss -t --json` as the person, who may read what both print. The changes
//! run `network set`, `network keep` and `network undo` as an
//! administrator, as `elevate network set ...` does at a terminal: the
//! settings file and Spark's control socket are root's.
//!
//! `network.set` builds the words `network set` takes after `set`, and
//! checks them with the command's own parser and checks
//! ([`crate::network::set::parse`] and [`crate::network::set::check`]), so
//! that the page, the API and the terminal refuse the same values with the
//! same words. With `try` the change is tried: the argv ends with `--try
//! SECONDS` and Spark undoes the change unless `network.keep` keeps it
//! (the trial `network`).
//!
//! A change says whether it changed anything by reading `network show CARD
//! --json` before and after, and comparing the card's `settings`, which
//! is what the file says of it: the addresses now and the lease move by
//! themselves. `network.undo` reads every card's, as it is not told which
//! card the trial was of.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::trial::{TRY, check_try, refusal, try_words};
use super::{Action, Args, Bad, Examples, Field, Nothing, Runs, Tests, Trial, Undo};
use crate::netd;
use crate::network::set::{self, Mode, TRIAL};

/// The area of these actions: the scope a token needs, `network`.
const AREA: &str = "network";

/// The tests of the reads, which `lab/actions.toml` names.
const READ_TESTS: Tests = Tests {
    checks: "admind::actions::network::tests",
    route: "admin-actions",
    same_as_command: "admin-actions",
    browser: "web-console/networking",
    ansible: "web-console/ansible",
};

/// The tests of the changes: the trials' boot test calls their routes.
const CHANGE_TESTS: Tests = Tests {
    checks: "admind::actions::network::tests",
    route: "admin-trials",
    same_as_command: "admin-actions",
    browser: "web-console/networking",
    ansible: "web-console/ansible",
};

/// The most characters of a card's name, and of a MAC address.
const CARD_MAX: usize = 17;

/// What a card's name or MAC address is made of, and starts with: the
/// schema's words. The command's own check is the exact one.
const CARD_PATTERN: &str = "^[A-Za-z0-9][A-Za-z0-9:._-]*$";

/// The words `mode` is.
const MODES: [&str; 2] = ["dhcp", "fixed"];

/// What addresses and DNS servers are made of: addresses with a prefix,
/// or without, and one space between them. The command's own check is
/// the exact one.
const ADDRESSES_PATTERN: &str = "^[0-9A-Fa-f:./]+( [0-9A-Fa-f:./]+)*$";
const ADDRESS_PATTERN: &str = "^[0-9A-Fa-f:.]+$";
const SERVERS_PATTERN: &str = "^[0-9A-Fa-f:.]+( [0-9A-Fa-f:.]+)*$";

/// The words `metered` is.
const METERED: [&str; 3] = ["auto", "yes", "no"];

/// `network show --json`: every card, with what the file says of it.
pub struct Show;

impl Action for Show {
    const ID: &'static str = "network.show";
    const AREA: &'static str = AREA;
    const CHANGES: bool = false;
    const PATH: &'static str = "/api/1/network";
    const RUNS: Runs = Runs::AsThePerson;
    const TESTS: Tests = READ_TESTS;
    const TITLE: &'static str = "Show the network cards";
    type Args = Nothing;

    fn argv(_: &Nothing) -> Vec<String> {
        ["network", "show", "--json"].map(String::from).to_vec()
    }

    fn examples() -> Examples<Nothing> {
        Examples {
            good: vec![Nothing {}],
            bad: vec![json!({ "all": true }), json!({ "card": "net0" })],
            undo: None,
        }
    }
}

/// `ss -t --json`: the TCP connections and the sockets that listen.
pub struct Connections;

impl Action for Connections {
    const ID: &'static str = "network.connections";
    const AREA: &'static str = AREA;
    const CHANGES: bool = false;
    const PATH: &'static str = "/api/1/network/connections";
    const RUNS: Runs = Runs::AsThePerson;
    const TESTS: Tests = READ_TESTS;
    const TITLE: &'static str = "List the connections";
    type Args = Nothing;

    fn argv(_: &Nothing) -> Vec<String> {
        ["ss", "-t", "--json"].map(String::from).to_vec()
    }

    fn examples() -> Examples<Nothing> {
        Examples {
            good: vec![Nothing {}],
            bad: vec![json!({ "all": true }), json!({ "state": "listen" })],
            undo: None,
        }
    }
}

/// What a card is set to: how it gets its addresses, and what else, as
/// `network set` takes them.
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Settings {
    /// The card, by its name or its MAC address.
    pub card: String,
    /// `dhcp` or `fixed`.
    pub mode: String,
    /// For `fixed`: its addresses, each `ADDRESS/PREFIX`, with a space
    /// between them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub addresses: Option<String>,
    /// For `fixed`: its router.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub router: Option<String>,
    /// For `fixed`: its DNS servers, with a space between them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dns: Option<String>,
    /// `auto`, `yes` or `no`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metered: Option<String>,
    /// How long the change is tried for, in seconds.
    #[serde(default, rename = "try", skip_serializing_if = "Option::is_none")]
    pub seconds: Option<i64>,
}

impl Settings {
    /// The words `network set` takes after `set`, in the order its usage
    /// gives: the card, the mode, the addresses, and the keys.
    fn words(&self) -> Vec<String> {
        let mut words = vec![self.card.clone(), self.mode.clone()];
        words.extend(self.addresses.iter().flat_map(|text| text.split(' ')).map(String::from));
        if let Some(router) = &self.router {
            words.extend(["router".to_string(), router.clone()]);
        }
        if let Some(dns) = &self.dns {
            words.push("dns".into());
            words.extend(dns.split(' ').map(String::from));
        }
        if let Some(metered) = &self.metered {
            words.extend(["metered".to_string(), metered.clone()]);
        }
        words
    }

    /// What the arguments ask of the command, which checks it.
    fn set(&self) -> set::Set {
        let words = |text: &Option<String>| -> Vec<String> {
            text.iter().flat_map(|text| text.split(' ')).map(String::from).collect()
        };
        set::Set {
            card: self.card.clone(),
            mode: if self.mode == "fixed" { Mode::Fixed } else { Mode::Dhcp },
            addresses: words(&self.addresses),
            router: self.router.clone(),
            dns: words(&self.dns),
            metered: self.metered.clone(),
        }
    }
}

/// Whether `card` can be what `network set` is given: the shape the
/// schema says, which the command's own check then holds to the exact
/// rule.
fn card_shape(card: &str) -> bool {
    card.len() <= CARD_MAX
        && card.starts_with(|c: char| c.is_ascii_alphanumeric())
        && card.bytes().all(|b| b.is_ascii_alphanumeric() || b":._-".contains(&b))
}

impl Args for Settings {
    const FIELDS: &'static [Field] = &[
        Field::text(
            "card",
            "The card's name, as network show gives it, or its MAC address.",
            1,
            CARD_MAX,
            Some(CARD_PATTERN),
        ),
        Field::one_of(
            "mode",
            "How the card gets its addresses: dhcp, or fixed for addresses of its own.",
            &MODES,
        ),
        Field::text(
            "addresses",
            "With fixed: one to four addresses, each ADDRESS/PREFIX, with a space between them.",
            1,
            256,
            Some(ADDRESSES_PATTERN),
        )
        .optional(),
        Field::text(
            "router",
            "With fixed: the card's router, an address on one of the networks of its addresses.",
            1,
            45,
            Some(ADDRESS_PATTERN),
        )
        .optional(),
        Field::text(
            "dns",
            "With fixed: up to three DNS servers, with a space between them.",
            1,
            140,
            Some(SERVERS_PATTERN),
        )
        .optional(),
        Field::one_of(
            "metered",
            "Whether the card's network is metered: auto, yes or no. Left as it is if not said.",
            &METERED,
        )
        .optional(),
        TRY,
    ];

    fn check(&self) -> Result<(), Bad> {
        if !card_shape(&self.card) {
            let why =
                "a card is named by 1 to 15 letters, digits, ., - and _, or by its MAC address";
            return Err(Bad { field: "card", why: refusal(why) });
        }
        if !MODES.contains(&self.mode.as_str()) {
            return Err(Bad { field: "mode", why: refusal("The mode is dhcp or fixed") });
        }
        let set = self.set();
        set::check(&set).map_err(|bad| Bad { field: bad.field, why: refusal(&bad.why) })?;
        // The command reads the words the console shows as the values it
        // checked: nothing a field holds is read as another word.
        let words = self.words();
        let words: Vec<&str> = words.iter().map(String::as_str).collect();
        if set::parse(&words).ok().as_ref() != Some(&set) {
            let why = "The words of the command would not be read as these values";
            return Err(Bad { field: "addresses", why: refusal(why) });
        }
        check_try(self.seconds)?;
        Ok(())
    }
}

/// `network set CARD dhcp|fixed ...`: a card's settings, at once or tried.
pub struct Change;

impl Action for Change {
    const ID: &'static str = "network.set";
    const AREA: &'static str = AREA;
    const CHANGES: bool = true;
    const PATH: &'static str = "/api/1/network/cards/{card}";
    const RUNS: Runs = Runs::AsAdministrator;
    const ERRORS: &'static [&'static str] = &["NET-176", "NET-178", "NET-181"];
    const TESTS: Tests = CHANGE_TESTS;
    const TITLE: &'static str = "Set a network card";
    type Args = Settings;

    fn argv(args: &Settings) -> Vec<String> {
        let mut argv = vec!["network".to_string(), "set".into()];
        argv.extend(args.words());
        argv.extend(try_words(args.seconds));
        argv
    }

    fn subject(args: &Settings) -> Option<String> {
        Some(args.card.clone())
    }

    fn files(_: &Settings) -> Vec<String> {
        vec![netd::SETTINGS.to_string()]
    }

    fn probe(args: &Settings) -> Option<Vec<String>> {
        Some(["network", "show", args.card.as_str(), "--json"].map(String::from).to_vec())
    }

    fn probed(args: &Settings, output: &str) -> Option<String> {
        let document: Value = serde_json::from_str(output).ok()?;
        let card = document.get("cards")?.as_array()?.iter().find(|card| {
            let is = |key: &str| card.get(key).and_then(Value::as_str);
            is("name") == Some(args.card.as_str())
                || is("mac").is_some_and(|mac| mac.eq_ignore_ascii_case(&args.card))
        })?;
        Some(card.get("settings")?.to_string())
    }

    fn trial(args: &Settings) -> Option<Trial> {
        args.seconds.map(|_| Trial::Begins(TRIAL))
    }

    fn examples() -> Examples<Settings> {
        // QEMU's user network: the card's own address, its router and its
        // name server are the ones the machine has already, so the change
        // is tried and undone with the network still there.
        Examples {
            good: vec![Settings {
                card: "net0".into(),
                mode: "fixed".into(),
                addresses: Some("10.0.2.15/24".into()),
                router: Some("10.0.2.2".into()),
                dns: Some("10.0.2.3".into()),
                metered: None,
                seconds: Some(30),
            }],
            bad: vec![
                json!({ "card": "net0", "mode": "fixed" }),
                json!({
                    "card": "net0", "mode": "fixed", "addresses": "10.0.2.15/24",
                    "router": "192.168.9.1"
                }),
                json!({
                    "card": "net0", "mode": "fixed",
                    "addresses": "10.0.2.11/24 10.0.2.12/24 10.0.2.13/24 10.0.2.14/24 10.0.2.15/24"
                }),
                json!({
                    "card": "net0", "mode": "fixed", "addresses": "10.0.2.15/24",
                    "dns": "10.0.2.3 10.0.2.4 10.0.2.5 10.0.2.6"
                }),
                json!({ "card": "net0", "mode": "fixed", "addresses": "10.0.2.15/24", "try": 5 }),
                json!({ "card": "net0", "mode": "dhcp", "addresses": "10.0.2.15/24" }),
                json!({ "card": "net0", "mode": "dhcp", "metered": "sometimes" }),
                json!({ "card": "net0", "mode": "static" }),
                json!({ "card": "../x", "mode": "dhcp" }),
            ],
            undo: Some(Undo { action: "network.undo", args: json!({}) }),
        }
    }
}

/// `network keep`: keeps the change that is being tried.
pub struct Keep;

impl Action for Keep {
    const ID: &'static str = "network.keep";
    const AREA: &'static str = AREA;
    const CHANGES: bool = true;
    const PATH: &'static str = "/api/1/network/trial/keep";
    const RUNS: Runs = Runs::AsAdministrator;
    const ERRORS: &'static [&'static str] = &["NET-179"];
    const TESTS: Tests = CHANGE_TESTS;
    const TITLE: &'static str = "Keep the network settings";
    type Args = Nothing;

    fn argv(_: &Nothing) -> Vec<String> {
        ["network", "keep"].map(String::from).to_vec()
    }

    fn trial(_: &Nothing) -> Option<Trial> {
        Some(Trial::Ends(TRIAL))
    }

    fn examples() -> Examples<Nothing> {
        // What puts it back is the Put back of the change that was tried
        // (`admin put-back`), which is no action.
        Examples { good: vec![Nothing {}], bad: vec![json!({ "id": "x" })], undo: None }
    }
}

/// `network undo`: undoes the change that is being tried.
pub struct Revert;

impl Action for Revert {
    const ID: &'static str = "network.undo";
    const AREA: &'static str = AREA;
    const CHANGES: bool = true;
    const PATH: &'static str = "/api/1/network/trial/undo";
    const RUNS: Runs = Runs::AsAdministrator;
    const ERRORS: &'static [&'static str] = &["NET-179"];
    const TESTS: Tests = CHANGE_TESTS;
    const TITLE: &'static str = "Undo the network settings";
    type Args = Nothing;

    fn argv(_: &Nothing) -> Vec<String> {
        ["network", "undo"].map(String::from).to_vec()
    }

    fn files(_: &Nothing) -> Vec<String> {
        vec![netd::SETTINGS.to_string()]
    }

    fn probe(_: &Nothing) -> Option<Vec<String>> {
        Some(["network", "show", "--json"].map(String::from).to_vec())
    }

    fn probed(_: &Nothing, output: &str) -> Option<String> {
        let document: Value = serde_json::from_str(output).ok()?;
        let mut all = serde_json::Map::new();
        for card in document.get("cards")?.as_array()? {
            all.insert(card.get("name")?.as_str()?.to_string(), card.get("settings")?.clone());
        }
        Some(Value::Object(all).to_string())
    }

    fn trial(_: &Nothing) -> Option<Trial> {
        Some(Trial::Ends(TRIAL))
    }

    fn examples() -> Examples<Nothing> {
        Examples { good: vec![Nothing {}], bad: vec![json!({ "now": true })], undo: None }
    }
}

#[cfg(test)]
mod tests {
    use super::super::{Caller, Found, Registered, authorize, find};
    use super::*;
    use crate::admind::actions::REGISTRY;
    use crate::admind::errors::Kind;
    use crate::admind::scopes::Scopes;

    fn action(id: &str) -> &'static dyn Registered {
        *REGISTRY.iter().find(|action| action.id() == id).unwrap()
    }

    fn argv(id: &str, args: Value) -> Vec<String> {
        action(id).prepare(args).unwrap().argv
    }

    /// What `network.set` refuses `args` with: ADM-125, the field and the
    /// words.
    fn refused(args: Value) -> (Option<&'static str>, String) {
        let failure = action("network.set").prepare(args).unwrap_err();
        assert_eq!(failure.kind, Kind::BadValue, "{}", failure.detail);
        (failure.field, failure.detail)
    }

    /// Arguments of a fixed card, with `more` added to them.
    fn fixed(more: &Value) -> Value {
        let mut args = json!({ "card": "net0", "mode": "fixed", "addresses": "10.0.2.15/24" });
        for (name, value) in more.as_object().unwrap() {
            args[name] = value.clone();
        }
        args
    }

    #[test]
    fn the_reads_run_network_and_ss_as_the_person_with_json() {
        assert_eq!(argv("network.show", json!({})), ["network", "show", "--json"]);
        assert_eq!(argv("network.connections", json!({})), ["ss", "-t", "--json"]);
        for (id, path) in [
            ("network.show", "/api/1/network"),
            ("network.connections", "/api/1/network/connections"),
        ] {
            let action = action(id);
            assert_eq!(
                (action.changes(), action.runs(), action.area(), action.path()),
                (false, Runs::AsThePerson, "network", path),
                "{id}"
            );
            assert!(action.errors().is_empty(), "{id}");
            assert_eq!(action.prepare(json!({})).unwrap().probe, None, "{id}");
            for bad in [json!({ "all": true }), json!({ "card": "net0" })] {
                assert_eq!(action.prepare(bad).unwrap_err().kind, Kind::BadRequest, "{id}");
            }
        }
    }

    #[test]
    fn a_card_with_its_own_addresses_is_set_with_the_words_of_the_usage_in_its_order() {
        let args = json!({
            "card": "net0", "mode": "fixed", "addresses": "10.0.2.15/24 10.0.3.15/24",
            "router": "10.0.2.2", "dns": "10.0.2.3 10.0.2.4", "metered": "no", "try": 30,
        });
        assert_eq!(
            argv("network.set", args),
            [
                "network",
                "set",
                "net0",
                "fixed",
                "10.0.2.15/24",
                "10.0.3.15/24",
                "router",
                "10.0.2.2",
                "dns",
                "10.0.2.3",
                "10.0.2.4",
                "metered",
                "no",
                "--try",
                "30"
            ]
        );
        // The keys go in the order of the usage, whatever the order the
        // fields were sent in, and each is left out when it is not given.
        assert_eq!(
            argv("network.set", fixed(&json!({ "dns": "10.0.2.3" }))),
            ["network", "set", "net0", "fixed", "10.0.2.15/24", "dns", "10.0.2.3"]
        );
        assert_eq!(
            argv("network.set", fixed(&json!({ "metered": "auto" }))),
            ["network", "set", "net0", "fixed", "10.0.2.15/24", "metered", "auto"]
        );
        assert_eq!(
            argv(
                "network.set",
                fixed(&json!({ "addresses": "2001:db8::10/64", "router": "fe80::1" }))
            ),
            ["network", "set", "net0", "fixed", "2001:db8::10/64", "router", "fe80::1"]
        );
    }

    #[test]
    fn a_card_that_asks_dhcp_is_set_with_the_mode_and_metering_alone() {
        assert_eq!(
            argv("network.set", json!({ "card": "net0", "mode": "dhcp" })),
            ["network", "set", "net0", "dhcp"]
        );
        assert_eq!(
            argv(
                "network.set",
                json!({ "card": "net0", "mode": "dhcp", "metered": "yes", "try": 60 })
            ),
            ["network", "set", "net0", "dhcp", "metered", "yes", "--try", "60"]
        );
    }

    #[test]
    fn a_card_is_named_by_its_name_or_its_mac_address_in_either_case() {
        for card in ["net0", "eth0.5", "a_b-c", "52:54:00:12:34:56", "52:54:0A:BC:34:EF"] {
            let words = argv("network.set", json!({ "card": card, "mode": "dhcp" }));
            assert_eq!(words[2], card);
        }
        let found = find(REGISTRY, true, "/api/1/network/cards/52%3A54%3A00%3A12%3A34%3A56");
        let Found::Action(found, params) = found else { panic!("{found:?}") };
        assert_eq!(found.id(), "network.set");
        assert_eq!(params["card"], "52:54:00:12:34:56");
        assert!(matches!(find(REGISTRY, false, "/api/1/network/cards/net0"), Found::WrongMethod));
        assert!(matches!(find(REGISTRY, true, "/api/1/network/cards"), Found::None));
    }

    #[test]
    fn the_words_are_read_by_the_command_as_the_values_that_were_checked() {
        for args in [
            fixed(&json!({})),
            fixed(&json!({ "router": "10.0.2.2", "dns": "10.0.2.3 10.0.2.4", "metered": "yes" })),
            json!({ "card": "net0", "mode": "dhcp", "metered": "auto", "try": 10 }),
            json!({ "card": "52:54:00:12:34:56", "mode": "dhcp" }),
        ] {
            let words = argv("network.set", args.clone());
            let after_set: Vec<&str> = words[2..].iter().map(String::as_str).collect();
            let after_set: Vec<&str> = match after_set.iter().position(|word| *word == "--try") {
                Some(at) => after_set[..at].to_vec(),
                None => after_set,
            };
            let set = set::parse(&after_set).unwrap();
            assert_eq!(set.card, args["card"]);
            assert_eq!(set.mode == Mode::Fixed, args["mode"] == "fixed");
            let split = |name: &str| -> Vec<String> {
                args[name]
                    .as_str()
                    .map_or_else(Vec::new, |t| t.split(' ').map(String::from).collect())
            };
            assert_eq!(set.addresses, split("addresses"));
            assert_eq!(set.dns, split("dns"));
            assert_eq!(set.router.as_deref(), args["router"].as_str());
            assert_eq!(set.metered.as_deref(), args["metered"].as_str());
        }
    }

    #[test]
    fn the_window_is_the_last_words_and_is_checked_as_spark_checks_it() {
        let prepared = action("network.set").prepare(fixed(&json!({ "try": 3600 }))).unwrap();
        assert_eq!(prepared.argv[prepared.argv.len() - 2..], ["--try", "3600"]);
        // The argument is `try`, as the route's body says it.
        assert_eq!(prepared.args["try"], 3600);
        assert!(prepared.args.get("seconds").is_none());
        let words = "A change is tried for 10 to 3600 seconds. Nothing was done.";
        for seconds in [json!(5), json!(9), json!(3601), json!(0), json!(-1), json!(86_400)] {
            assert_eq!(refused(fixed(&json!({ "try": seconds }))), (Some("try"), words.into()));
        }
        for seconds in [json!("30"), json!(1.5), json!(true), json!([30])] {
            let failure =
                action("network.set").prepare(fixed(&json!({ "try": seconds }))).unwrap_err();
            assert_eq!(failure.kind, Kind::BadRequest);
        }
        // A `null` is the window left out.
        let none = action("network.set").prepare(fixed(&json!({ "try": null }))).unwrap();
        assert_eq!(none.argv, ["network", "set", "net0", "fixed", "10.0.2.15/24"]);
    }

    #[test]
    fn a_value_that_cannot_work_is_adm_125_on_its_field_in_the_commands_own_words() {
        let nothing = "Nothing was done.";
        for (args, field, why) in [
            (
                json!({ "card": "net0", "mode": "fixed" }),
                "addresses",
                "A card has 1 to 4 addresses of its own.",
            ),
            (
                fixed(
                    &json!({ "addresses": "10.0.2.11/24 10.0.2.12/24 10.0.2.13/24 10.0.2.14/24 10.0.2.15/24" }),
                ),
                "addresses",
                "A card has 1 to 4 addresses of its own.",
            ),
            (
                fixed(&json!({ "addresses": "10.0.2.15" })),
                "addresses",
                "10.0.2.15 has no prefix: write it as ADDRESS/PREFIX, as 192.168.1.10/24.",
            ),
            (
                fixed(&json!({ "addresses": "10.0.2.15/33" })),
                "addresses",
                "10.0.2.15/33: an IPv4 prefix is 1 to 32.",
            ),
            (
                fixed(&json!({ "addresses": "169.254.1.1/16" })),
                "addresses",
                "169.254.1.1 is not an address a card may have.",
            ),
            (
                fixed(&json!({ "addresses": "10.0.2.15/24 10.0.2.15/25" })),
                "addresses",
                "10.0.2.15 is given twice.",
            ),
            (
                fixed(&json!({ "router": "192.168.9.1" })),
                "router",
                "The router 192.168.9.1 is on none of the networks of the card's addresses.",
            ),
            (
                fixed(&json!({ "router": "10.0.2.15" })),
                "router",
                "10.0.2.15 is the card's own address, not its router.",
            ),
            (
                fixed(&json!({ "router": "gateway" })),
                "router",
                "Gateway is not an address a router may have.",
            ),
            (
                fixed(&json!({ "dns": "10.0.2.3 10.0.2.4 10.0.2.5 10.0.2.6" })),
                "dns",
                "A card has at most 3 DNS servers.",
            ),
            (fixed(&json!({ "dns": "10.0.2.3 10.0.2.3" })), "dns", "10.0.2.3 is given twice."),
            (
                fixed(&json!({ "metered": "sometimes" })),
                "metered",
                "Metered is auto, yes or no, not sometimes.",
            ),
            (
                json!({ "card": "net0", "mode": "dhcp", "addresses": "10.0.2.15/24" }),
                "addresses",
                "A card that asks DHCP is given no addresses, router or DNS servers.",
            ),
            (
                json!({ "card": "net0", "mode": "dhcp", "router": "10.0.2.2" }),
                "addresses",
                "A card that asks DHCP is given no addresses, router or DNS servers.",
            ),
            (
                json!({ "card": "net0", "mode": "dhcp", "dns": "10.0.2.3" }),
                "addresses",
                "A card that asks DHCP is given no addresses, router or DNS servers.",
            ),
            (json!({ "card": "net0", "mode": "static" }), "mode", "The mode is dhcp or fixed."),
            (json!({ "card": "net0", "mode": "FIXED" }), "mode", "The mode is dhcp or fixed."),
        ] {
            assert_eq!(refused(args.clone()), (Some(field), format!("{why} {nothing}")), "{args}");
        }
    }

    #[test]
    fn a_card_that_is_no_name_or_mac_is_refused_before_anything_is_read() {
        let words = "A card is named by 1 to 15 letters, digits, ., - and _, or by its MAC address. \
                     Nothing was done.";
        for card in [
            "",
            "../x",
            "net 0",
            "net0\n",
            "-net",
            ".net",
            "_net",
            "net/0",
            "net0123456789abcde",
            "52:54:00:12:34",
            "52:54:00:12:34:56:78",
            "+1:2:3:4:5:6",
            "52:54:00:12:34:zz",
            "000:000:000:000:000:000",
        ] {
            let args = json!({ "card": card, "mode": "dhcp" });
            assert_eq!(refused(args), (Some("card"), words.into()), "{card:?}");
        }
    }

    #[test]
    fn a_value_that_would_be_read_as_another_word_is_refused_and_never_passed() {
        for (field, value) in [
            ("addresses", "10.0.2.15/24 router"),
            ("addresses", "10.0.2.15/24 dns"),
            ("addresses", "10.0.2.15/24 metered"),
            ("addresses", "10.0.2.15/24\n10.0.2.16/24"),
            ("addresses", "10.0.2.15/24  10.0.2.16/24"),
            ("addresses", " 10.0.2.15/24"),
            ("addresses", "10.0.2.15/24 "),
            ("addresses", ""),
            ("router", "10.0.2.2 dns 10.0.2.3"),
            ("router", "10.0.2.2 10.0.2.3"),
            ("router", ""),
            ("dns", "10.0.2.3 router 10.0.2.2"),
            ("dns", "10.0.2.3 --try 10"),
            ("dns", ""),
        ] {
            let (found, why) = refused(fixed(&json!({ field: value })));
            assert!(
                found.is_some() && why.ends_with("Nothing was done."),
                "{field} {value:?}: {why}"
            );
        }
    }

    #[test]
    fn the_settings_file_is_what_a_change_writes_and_its_card_is_what_it_reads_before_and_after() {
        let change = action("network.set").prepare(fixed(&json!({ "try": 30 }))).unwrap();
        assert_eq!(change.files, ["/system/settings/network"]);
        assert_eq!(
            change.probe.as_deref(),
            Some(&["network", "show", "net0", "--json"].map(String::from)[..])
        );
        assert!(action("network.set").compares());
        assert_eq!(
            action("network.set").errors(),
            ["NET-176", "NET-178", "NET-181"],
            "the refusals of the command that have an ID of their own"
        );
        let mac = action("network.set")
            .prepare(json!({ "card": "52:54:00:12:34:56", "mode": "dhcp" }))
            .unwrap();
        assert_eq!(mac.probe.unwrap()[2], "52:54:00:12:34:56");
    }

    /// What `network show --json` prints for cards: `(name, mac, address,
    /// mode)` each.
    fn shown(cards: &[(&str, &str, &str, &str)]) -> String {
        let cards: Vec<Value> = cards
            .iter()
            .map(|&(name, mac, address, mode)| {
                json!({
                    "name": name, "mac": mac, "up": true, "speed_mbps": 1000,
                    "addresses": [{ "address": address, "prefix": 24, "from": "dhcp" }],
                    "routers": [], "dns": [],
                    "settings": {
                        "section": null, "mode": mode, "addresses": [], "router": null,
                        "dns": [], "metered": "auto", "dhcp_server": false, "other": []
                    },
                    "lease": { "address": address, "ends_ms": 1_763_120_000_000_u64 }
                })
            })
            .collect();
        json!({ "cards": cards, "trial": null }).to_string()
    }

    #[test]
    fn a_change_is_compared_by_the_settings_of_its_card_and_not_by_what_moves_by_itself() {
        let picks = |card: &str, output: &str| {
            let args = json!({ "card": card, "mode": "dhcp" });
            action("network.set").probed(&args, output)
        };
        let before = shown(&[("net0", "52:54:00:12:34:56", "10.0.2.15", "dhcp")]);
        // The address now and the lease move on their own: not compared.
        let later = shown(&[("net0", "52:54:00:12:34:56", "10.0.2.99", "dhcp")]);
        assert_eq!(picks("net0", &before), picks("net0", &later));
        let fixed_now = shown(&[("net0", "52:54:00:12:34:56", "10.0.2.15", "fixed")]);
        assert_ne!(picks("net0", &before), picks("net0", &fixed_now));
        // By the name or by the MAC address, in either case.
        let picked = picks("net0", &before).unwrap();
        assert_eq!(picks("52:54:00:12:34:56", &before).as_deref(), Some(picked.as_str()));
        assert_eq!(
            picks("52:54:00:12:34:56".to_uppercase().as_str(), &before).as_deref(),
            Some(picked.as_str())
        );
        assert!(picked.contains(r#""mode":"dhcp""#), "{picked}");
        assert!(!picked.contains("10.0.2.15"), "{picked}");
        // Another card's settings are not this card's.
        let two = shown(&[
            ("net1", "52:54:00:12:34:57", "10.0.3.15", "fixed"),
            ("net0", "52:54:00:12:34:56", "10.0.2.15", "dhcp"),
        ]);
        assert_eq!(picks("net0", &two), picks("net0", &before));
        for output in
            ["", "not json", "[]", "{}", r#"{"cards":[]}"#, r#"{"cards":[{"name":"net0"}]}"#]
        {
            assert_eq!(picks("net0", output), None, "{output:?}");
        }
        assert_eq!(picks("net9", &before), None);
        // Arguments that are not the action's pick nothing.
        assert_eq!(action("network.set").probed(&json!({ "card": 7 }), &before), None);
    }

    #[test]
    fn keep_and_undo_run_network_as_an_administrator_with_no_arguments() {
        assert_eq!(argv("network.keep", json!({})), ["network", "keep"]);
        assert_eq!(argv("network.undo", json!({})), ["network", "undo"]);
        for (id, path) in [
            ("network.keep", "/api/1/network/trial/keep"),
            ("network.undo", "/api/1/network/trial/undo"),
            ("network.set", "/api/1/network/cards/{card}"),
        ] {
            let action = action(id);
            assert_eq!(
                (action.changes(), action.runs(), action.area(), action.path()),
                (true, Runs::AsAdministrator, "network", path),
                "{id}"
            );
        }
        for id in ["network.keep", "network.undo"] {
            assert_eq!(action(id).errors(), ["NET-179"], "{id}");
        }
        for (id, bad) in
            [("network.keep", json!({ "id": "x" })), ("network.undo", json!({ "now": true }))]
        {
            assert_eq!(action(id).prepare(bad).unwrap_err().kind, Kind::BadRequest, "{id}");
        }
    }

    #[test]
    fn keep_names_no_file_and_undo_the_settings_which_it_compares_for_every_card() {
        let keep = action("network.keep").prepare(json!({})).unwrap();
        assert_eq!((keep.files, keep.probe), (Vec::new(), None));
        assert!(!action("network.keep").compares());
        let undo = action("network.undo").prepare(json!({})).unwrap();
        assert_eq!(undo.files, ["/system/settings/network"]);
        assert_eq!(undo.probe, Some(["network", "show", "--json"].map(String::from).to_vec()));
        assert!(action("network.undo").compares());
        let two = shown(&[
            ("net0", "52:54:00:12:34:56", "10.0.2.15", "dhcp"),
            ("net1", "52:54:00:12:34:57", "10.0.3.15", "fixed"),
        ]);
        let picked = action("network.undo").probed(&json!({}), &two).unwrap();
        let by_name: Value = serde_json::from_str(&picked).unwrap();
        assert_eq!(by_name["net0"]["mode"], "dhcp");
        assert_eq!(by_name["net1"]["mode"], "fixed");
        // What moves by itself is not in it.
        let later = two.replace("10.0.2.15", "10.0.2.77");
        assert_eq!(action("network.undo").probed(&json!({}), &later).unwrap(), picked);
        for output in ["", "{}", r#"{"cards":[{"settings":{}}]}"#, r#"{"cards":"none"}"#] {
            assert_eq!(action("network.undo").probed(&json!({}), output), None, "{output:?}");
        }
    }

    #[test]
    fn the_examples_work_the_bad_ones_do_not_and_the_set_is_undone_by_the_undo() {
        for id in
            ["network.show", "network.connections", "network.set", "network.keep", "network.undo"]
        {
            let action = action(id);
            let (good, bad, undo) = action.example_values();
            assert!(!bad.is_empty(), "{id}");
            for args in &good {
                assert!(action.prepare(args.clone()).is_ok(), "{id}");
            }
            for args in bad {
                assert!(action.prepare(args.clone()).is_err(), "{id} {args}");
            }
            assert_eq!(undo.is_some(), id == "network.set", "{id}");
        }
        let (good, _, undo) = action("network.set").example_values();
        // QEMU's user network, as the card has it: tried for 30 seconds.
        assert_eq!(
            good,
            [json!({
                "card": "net0", "mode": "fixed", "addresses": "10.0.2.15/24", "router": "10.0.2.2",
                "dns": "10.0.2.3", "try": 30
            })]
        );
        let undo = undo.unwrap();
        assert_eq!((undo.action, undo.args), ("network.undo", json!({})));
        assert!(action("network.undo").prepare(json!({})).is_ok());
    }

    #[test]
    fn the_bad_examples_of_the_design_are_each_refused_by_the_field_they_are_about() {
        let (_, bad, _) = action("network.set").example_values();
        let fields: Vec<Option<&str>> = bad
            .iter()
            .map(|args| action("network.set").prepare(args.clone()).unwrap_err().field)
            .collect();
        assert_eq!(
            fields,
            [
                Some("addresses"),
                Some("router"),
                Some("addresses"),
                Some("dns"),
                Some("try"),
                Some("addresses"),
                Some("metered"),
                Some("mode"),
                Some("card"),
            ]
        );
    }

    #[test]
    fn a_token_changes_the_network_with_its_scope_and_reads_it_with_the_read_scope() {
        let token = |list: &str, id: &str| {
            let scopes = Scopes::parse(list).unwrap();
            authorize(&Caller::Token { name: "ci", scopes: &scopes }, action(id))
        };
        for id in ["network.show", "network.connections"] {
            for list in ["network", "network:read", "*"] {
                assert!(token(list, id).is_ok(), "{id} {list}");
            }
            assert_eq!(token("services", id).unwrap_err().kind, Kind::OutOfScope);
        }
        for id in ["network.set", "network.keep", "network.undo"] {
            for list in ["network", "*"] {
                assert!(token(list, id).is_ok(), "{id} {list}");
            }
            assert_eq!(token("network:read", id).unwrap_err().kind, Kind::OutOfScope, "{id}");
        }
        // A session that is not an administrator's reads, and does not
        // change.
        let reader = Caller::Session { name: "bob", admin: false, unlocked: false };
        assert!(authorize(&reader, action("network.show")).is_ok());
        assert_eq!(authorize(&reader, action("network.set")).unwrap_err().kind, Kind::NotAdmin);
        let locked = Caller::Session { name: "alice", admin: true, unlocked: false };
        assert_eq!(authorize(&locked, action("network.set")).unwrap_err().kind, Kind::Locked);
    }

    #[test]
    fn the_ansible_task_is_the_command_and_the_name_has_the_card() {
        let task = action("network.set").prepare(fixed(&json!({ "try": 30 }))).unwrap().ansible;
        assert_eq!(task.name, "Set a network card: net0");
        assert!(task.become_root);
        assert_eq!(task.changed_when, None);
        assert_eq!(
            task.params["argv"],
            json!(["network", "set", "net0", "fixed", "10.0.2.15/24", "--try", "30"])
        );
        let read = action("network.show").prepare(json!({})).unwrap().ansible;
        assert!(!read.become_root);
        assert_eq!(read.changed_when.as_deref(), Some("false"));
    }
}
