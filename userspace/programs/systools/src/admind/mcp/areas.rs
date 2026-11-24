// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The console's areas as an MCP client's owner chooses among them
//! (ADR 0690): each area's title, and how much of it there is to offer.
//!
//! The titles are the pages' own words for the areas. The counts come
//! from the registry through [`offer`], so the levels the page shows
//! for an area (off, read, read all and ask) are the ones the tools
//! give, and an area with nothing at a level does not show it.

use serde_json::{Value, json};

use super::super::actions::Registered;
use super::offer::{Offer, WHO_GETS_IN, offer};

/// Every area and its title, in the order the page lists them: what the
/// machine runs and keeps first, the areas that decide who gets in
/// last.
pub const AREAS: [(&str, &str); 26] = [
    ("services", "Services"),
    ("logs", "Logs"),
    ("metrics", "Usage charts"),
    ("reliability", "Reliability"),
    ("storage", "Storage"),
    ("snapshots", "Snapshots"),
    ("backups", "Backups"),
    ("network", "Networking"),
    ("firewall", "Firewall"),
    ("certificates", "Certificates"),
    ("apps", "Apps"),
    ("websites", "Websites"),
    ("databases", "Databases"),
    ("updates", "Updates"),
    ("hardware", "Hardware"),
    ("alerts", "Alerts"),
    ("power", "Restart and shut down"),
    ("machine", "This server's name"),
    ("reports", "Problem reports"),
    ("move", "Move to new hardware"),
    ("accounts", "Accounts"),
    ("account", "Your account"),
    ("security", "Security"),
    ("ssh", "SSH"),
    ("private", "Private network"),
    ("console", "The console's settings"),
];

/// How much of an area there is to offer.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Counts {
    /// Reads that run as the client's owner.
    pub reads: usize,
    /// Reads that run as an administrator.
    pub reads_all: usize,
    /// Changes a client may ask for.
    pub asks: usize,
    /// Actions no client is offered.
    pub never: usize,
}

/// How much of `area` there is to offer, of `actions`.
pub fn counts(actions: &[&dyn Registered], area: &str) -> Counts {
    let mut counts = Counts::default();
    for action in actions.iter().filter(|action| action.area() == area) {
        match offer(*action) {
            Ok(Offer::Read) => counts.reads += 1,
            Ok(Offer::ReadAll) => counts.reads_all += 1,
            Ok(Offer::Ask) => counts.asks += 1,
            Err(_) => counts.never += 1,
        }
    }
    counts
}

/// What `GET /api/1/clients/areas` answers: every area with its title,
/// its counts, and whether it decides who gets in, which is why it has
/// no change to ask for.
pub fn listed(actions: &[&dyn Registered]) -> Value {
    let areas: Vec<Value> = AREAS
        .iter()
        .map(|(area, title)| {
            let Counts { reads, reads_all, asks, never } = counts(actions, area);
            json!({
                "area": area,
                "title": title,
                "reads": reads,
                "reads_all": reads_all,
                "asks": asks,
                "never": never,
                "who_gets_in": WHO_GETS_IN.contains(area),
            })
        })
        .collect();
    json!({ "areas": areas })
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;
    use crate::admind::actions::REGISTRY;

    #[test]
    fn every_area_of_the_registry_has_a_title_and_every_title_an_area() {
        let registry: BTreeSet<&str> = REGISTRY.iter().map(|action| action.area()).collect();
        let titled: BTreeSet<&str> = AREAS.iter().map(|(area, _)| *area).collect();
        assert_eq!(registry, titled);
        assert_eq!(titled.len(), AREAS.len(), "an area is listed twice");
        let titles: BTreeSet<&str> = AREAS.iter().map(|(_, title)| *title).collect();
        assert_eq!(titles.len(), AREAS.len(), "two areas have one title");
    }

    #[test]
    fn the_areas_that_decide_who_gets_in_have_no_change_to_ask_for() {
        for area in WHO_GETS_IN {
            let counts = counts(REGISTRY, area);
            assert_eq!(counts.asks, 0, "{area}");
            assert!(counts.reads + counts.reads_all > 0, "{area} has something to read");
        }
        // And the others each have something to offer.
        for (area, _) in AREAS {
            let counts = counts(REGISTRY, area);
            assert!(counts.reads + counts.reads_all + counts.asks > 0, "{area}");
        }
    }

    #[test]
    fn the_counts_of_an_area_are_all_of_its_actions() {
        for (area, _) in AREAS {
            let Counts { reads, reads_all, asks, never } = counts(REGISTRY, area);
            let all = REGISTRY.iter().filter(|action| action.area() == area).count();
            assert_eq!(reads + reads_all + asks + never, all, "{area}");
        }
        let listed = listed(REGISTRY);
        let areas = listed["areas"].as_array().expect("a list");
        assert_eq!(areas.len(), AREAS.len());
        assert_eq!(areas[0]["area"], "services");
        assert_eq!(areas[0]["title"], "Services");
        assert_eq!(areas[0]["who_gets_in"], false);
        let console = areas.iter().find(|area| area["area"] == "console").expect("console");
        assert_eq!((&console["who_gets_in"], &console["asks"]), (&json!(true), &json!(0)));
    }
}
