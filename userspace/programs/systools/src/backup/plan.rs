// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! `backup restore --machine --plan --json`: what a restore of a whole
//! machine would bring across, compared with this machine, before anything
//! is received (ADR 0464). It reads the machine's record and nothing else
//! of the backups, and changes nothing.
//!
//! A record of format 2 carries the old machine's hardware, the bytes of
//! each place, the names of its accounts and its network and firewall
//! settings, so the plan can say what differs; one of format 1 has none of
//! these, and the plan says what it can: the places, the apps and whether
//! a restore is staged.

use std::collections::BTreeSet;

use serde_json::{Value, json};
use slots::machine as staged;

use super::cards::{self, Pair};
use super::json::time_ms;
use super::record::{Hardware, Record};
use crate::bundles::registry;
use crate::firewall;

/// What the plan compares the record with: this machine as it is now.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Here {
    pub hardware: Hardware,
    /// The bytes free on the root, for what is received; none when it
    /// cannot be read.
    pub free: Option<u64>,
    /// Whether a restore of a whole machine is staged already (BAK-104).
    pub staged: bool,
}

/// The apps the record's registry lists, each id and version once, by id.
fn apps(record: &Record) -> Vec<Value> {
    let path = format!("{}/{}", super::record::BUNDLED, registry::INSTALLED);
    let installed = record.state.get(&path).and_then(|text| registry::installed(text).ok());
    let found: BTreeSet<(String, String)> =
        installed.unwrap_or_default().into_iter().map(|app| (app.id, app.version)).collect();
    found.into_iter().map(|(id, version)| json!({"id": id, "version": version})).collect()
}

/// The bytes the root's places of the record need, received: none for a
/// record that does not give the bytes of each of them.
fn needed(record: &Record) -> Option<u64> {
    let mut sum = 0u64;
    let mut any = false;
    for (folder, _) in
        record.places.iter().filter(|(f, _)| staged::SUBVOLUMES.contains(&f.as_str()))
    {
        sum = sum.checked_add(*record.bytes.get(folder)?)?;
        any = true;
    }
    any.then_some(sum)
}

/// The firewall rules of the record that name a card of the old machine
/// that this one does not have, as the file writes them.
fn on_missing(record: &Record, here: &Here) -> Vec<String> {
    let (Some(then), Some(text)) =
        (&record.hardware, record.settings.as_ref().and_then(|s| s.firewall.as_deref()))
    else {
        return Vec::new();
    };
    let had = |name: &str| then.cards.iter().any(|card| card.name == name);
    let has = |name: &str| here.hardware.cards.iter().any(|card| card.name == name);
    firewall::rules_on_cards(text)
        .into_iter()
        .filter(|(_, card)| had(card) && !has(card))
        .map(|(rule, _)| rule)
        .collect()
}

/// A pair of cards as the plan writes it.
fn card_json(pair: &Pair) -> Value {
    json!({
        "old": {
            "name": pair.old.name,
            "mac": pair.old.mac,
            "addresses": pair.old.addresses,
            "router": pair.old.router,
        },
        "new": pair.new.as_ref().map(|card| json!({"name": card.name, "mac": card.mac})),
        "choice": pair.choice.to_string(),
    })
}

/// The plan of restoring `record` onto `here`, with its old cards paired
/// with this machine's as `pairs` says.
pub fn plan(record: &Record, here: &Here, pairs: &[Pair]) -> Value {
    let second = record.hardware.is_some();
    let places: Vec<Value> = record
        .places
        .iter()
        .map(|(folder, path)| {
            json!({"path": path, "folder": folder, "bytes": record.bytes.get(folder)})
        })
        .collect();
    let needed = needed(record);
    let fits = needed.zip(here.free).map(|(needed, free)| needed <= free);
    json!({
        "machine": record.machine,
        "at": record.time,
        "at_ms": time_ms(&record.time),
        "record_format": record.format,
        "staged": here.staged,
        "accounts": second.then_some(&record.accounts),
        "apps": apps(record),
        "places": places,
        "needed": needed,
        "free": here.free,
        "fits": fits,
        "hardware": record.hardware.as_ref().map(|then| json!({"then": then, "now": here.hardware})),
        "cards": pairs.iter().map(card_json).collect::<Vec<_>>(),
        "firewall_on_missing": on_missing(record, here),
    })
}

/// The old cards of `record` with addresses of their own, paired with
/// this machine's as `asked` says, by the rules of [`cards::pair`]; none
/// when the record does not carry its network settings.
///
/// # Errors
///
/// What [`cards::pair`] refuses.
pub fn pairs(
    record: &Record,
    here: &Here,
    asked: &[cards::Asked],
) -> Result<Vec<Pair>, crate::util::Fail> {
    let Some(text) = record.settings.as_ref().and_then(|s| s.network.as_deref()) else {
        if let Some(first) = asked.first() {
            return Err(crate::util::usage(format!(
                "--card {}: these backups' record does not carry the network settings, so its \
                 cards are not known before the restore; restore without --card and set the \
                 network after",
                first.old
            )));
        }
        return Ok(Vec::new());
    };
    let old = cards::old_cards(text, record.hardware.as_ref());
    cards::pair(text, &old, &here.hardware.cards, asked)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use crate::backup::record::{Card, Disk, Settings};

    fn card(name: &str, mac: &str) -> Card {
        Card { name: name.into(), mac: mac.into(), model: None }
    }

    /// The record of an old machine with two cards, net0 with a fixed
    /// address, and a mirror, as a round of format 2 writes it.
    fn record() -> Record {
        Record {
            format: 2,
            machine: "duke-nas".into(),
            time: "2026-11-14-013000".into(),
            places: BTreeMap::from([
                ("settings".to_string(), "/system/settings".to_string()),
                ("users".to_string(), "/users".to_string()),
                ("photos".to_string(), "/volumes/PHOTOS".to_string()),
            ]),
            state: BTreeMap::from([("bundled/installed".to_string(), String::new())]),
            bytes: BTreeMap::from([
                ("settings".to_string(), 1_000_000),
                ("users".to_string(), 412_000_000_000),
                ("photos".to_string(), 900_000_000_000),
            ]),
            hardware: Some(Hardware {
                cards: vec![card("net0", "52:54:00:00:00:01"), card("net1", "52:54:00:00:00:02")],
                disks: vec![Disk { model: Some("WD20EFZX".into()), size: 2_000_398_934_016 }; 2],
                mirror: true,
                serial: None,
                cpu: Some("Intel Core i5-6500".into()),
            }),
            settings: Some(Settings {
                network: Some(
                    "[52:54:00:00:00:01]\naddress = 192.168.1.12/24\nrouter = 192.168.1.1\n".into(),
                ),
                firewall: Some("allow tcp 80\ndeny ping on net1\nallow tcp 22 on wg0\n".into()),
            }),
            accounts: vec!["owner".into(), "helper".into()],
        }
    }

    fn here() -> Here {
        Here {
            hardware: Hardware {
                cards: vec![card("net0", "52:54:00:00:00:a1")],
                disks: vec![Disk { model: None, size: 1_000_000_000_000 }],
                mirror: false,
                serial: None,
                cpu: Some("AMD Ryzen 5 PRO 4650GE".into()),
            },
            free: Some(900_000_000_000),
            staged: false,
        }
    }

    #[test]
    fn a_plan_of_format_2_says_what_comes_across_and_what_differs() {
        let (record, here) = (record(), here());
        let pairs = pairs(&record, &here, &[]).unwrap();
        let plan = plan(&record, &here, &pairs);
        assert_eq!(plan["machine"], "duke-nas");
        assert_eq!(plan["at"], "2026-11-14-013000");
        assert!(plan["at_ms"].is_u64());
        assert_eq!((&plan["record_format"], &plan["staged"]), (&json!(2), &json!(false)));
        assert_eq!(plan["accounts"], json!(["owner", "helper"]));
        assert_eq!(
            plan["places"][2],
            json!({"path": "/users", "folder": "users", "bytes": 412_000_000_000u64})
        );
        // The root's places need their bytes; a disk's place goes to its disk.
        assert_eq!((&plan["needed"], &plan["fits"]), (&json!(412_001_000_000u64), &json!(true)));
        assert_eq!(plan["hardware"]["then"]["mirror"], true);
        assert_eq!(plan["hardware"]["now"]["cards"][0]["name"], "net0");
        assert_eq!(
            plan["cards"],
            json!([{
                "old": {"name": "net0", "mac": "52:54:00:00:00:01", "addresses": ["192.168.1.12/24"], "router": "192.168.1.1"},
                "new": {"name": "net0", "mac": "52:54:00:00:00:a1"},
                "choice": "automatic",
            }])
        );
        // net1 was the old machine's and this one lacks it; wg0 is no card.
        assert_eq!(plan["firewall_on_missing"], json!(["deny ping on net1"]));
    }

    #[test]
    fn a_choice_of_card_is_in_the_plan() {
        let (record, here) = (record(), here());
        let asked = [cards::Asked::parse("net0=52:54:00:00:00:a1").unwrap()];
        let plan = plan(&record, &here, &pairs(&record, &here, &asked).unwrap());
        assert_eq!(plan["cards"][0]["choice"], "fixed");
    }

    #[test]
    fn files_that_do_not_fit_say_so() {
        let mut here = here();
        here.free = Some(1_000);
        let record = record();
        assert_eq!(plan(&record, &here, &[])["fits"], false);
        here.free = None;
        assert_eq!(plan(&record, &here, &[])["fits"], Value::Null);
    }

    #[test]
    fn a_plan_of_format_1_has_no_hardware_bytes_accounts_or_cards() {
        let record = Record {
            format: 1,
            bytes: BTreeMap::new(),
            hardware: None,
            settings: None,
            accounts: Vec::new(),
            ..record()
        };
        let mut here = here();
        here.staged = true;
        let plan = plan(&record, &here, &pairs(&record, &here, &[]).unwrap());
        for empty in ["accounts", "needed", "fits", "hardware"] {
            assert_eq!(plan[empty], Value::Null, "{empty}");
        }
        assert_eq!((&plan["cards"], &plan["firewall_on_missing"]), (&json!([]), &json!([])));
        assert_eq!(plan["places"][0]["bytes"], Value::Null);
        assert_eq!(plan["staged"], true);
        // A card cannot be chosen when the record does not say the cards.
        let asked = [cards::Asked::parse("net0=automatic").unwrap()];
        assert!(pairs(&record, &here, &asked).unwrap_err().usage);
    }

    #[test]
    fn the_apps_are_the_registrys_each_once() {
        let mut record = record();
        let line = |place: &str, id: &str, version: &str| {
            format!(
                "{place}\t{id}\t{version}\tRWQf6LRCGA9i59SLOFxz6NxvASXDJeRtuZykwQepbDEGt87ig1BNpWaE\t{}\t1763000000\n",
                "0".repeat(64)
            )
        };
        let text = line("everyone", "org.nginx.nginx", "1.30.6")
            + &line("1000", "io.github.sharkdp.bat", "0.26.1")
            + &line("1001", "io.github.sharkdp.bat", "0.26.1");
        record.state.insert("bundled/installed".into(), text);
        let apps = apps(&record);
        assert_eq!(
            apps,
            [
                json!({"id": "io.github.sharkdp.bat", "version": "0.26.1"}),
                json!({"id": "org.nginx.nginx", "version": "1.30.6"}),
            ],
            "{apps:?}"
        );
        record.state.insert("bundled/installed".into(), "not a registry\n".into());
        assert_eq!(super::apps(&record), Vec::<Value>::new());
    }
}
