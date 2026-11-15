// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! What the canaries make of the waves (ADR 0533), in words and JSON, for
//! the commands that show it and act on it: `system-release canaries
//! show` and `canaries changed`, and `rollout ... canaries`, and
//! catalog-repo's `canaries changed` and `rollout ... canaries`, which
//! share this. The plan's `red` is the names of the canaries that are
//! not green, as [`canary::evaluate`] finds them at a time: the update
//! server recomputes it and signs the rollouts again only when the set
//! changes.

use std::path::Path;

use serde_json::{Value, json};

use crate::canary::{self, Canaries, Offer, Standing};

/// The exit status of `canaries changed` when the canaries not green are
/// the plan's own: 0 is that they are not, and 1 that it failed.
pub const UNCHANGED: i32 = 3;

/// [`UNCHANGED`] as a process exit status.
pub const UNCHANGED_STATUS: u8 = 3;

/// What the old `rollout ... canary NAME red|green` is answered with.
pub const BY_HAND: &str = "a canary is no longer marked by rollout: `./ob release canary NAME red \
     REASON`, `NAME green REASON` or `NAME auto` changes the list that both the system's \
     releases and the catalog follow";

/// Each canary of the list at `canaries`, as its newest report in
/// `reports` and the releases `offered` show it at `now`.
///
/// # Errors
///
/// If the list cannot be read.
pub fn standings(
    canaries: &Path,
    reports: &Path,
    offered: &[Offer],
    now: i64,
) -> Result<Vec<Standing>, String> {
    Ok(canary::evaluate(&Canaries::load(canaries)?, reports, offered, now))
}

/// Whether the canaries `red` are not those `held`, the plan's: a set,
/// whatever the order.
#[must_use]
pub fn differs(held: &[String], red: &[String]) -> bool {
    let sorted = |names: &[String]| {
        let mut names = names.to_vec();
        names.sort();
        names.dedup();
        names
    };
    sorted(held) != sorted(red)
}

/// `names`, set apart by commas, or `none`.
#[must_use]
pub fn listed(names: &[String]) -> String {
    if names.is_empty() { "none".into() } else { names.join(", ") }
}

/// What the canaries not green, `red`, make of the waves, in words.
#[must_use]
pub fn said(red: &[String]) -> String {
    if red.is_empty() {
        "no canary holds the waves".into()
    } else {
        format!("the canaries not green are {}", red.join(", "))
    }
}

/// The line that says the plan's canaries, `held`, are not those that
/// are not green, `red`.
#[must_use]
pub fn change_line(held: &[String], red: &[String]) -> String {
    format!(
        "the canaries not green are {}, and the plan holds the waves for {}",
        listed(red),
        listed(held)
    )
}

/// One line for a canary: its colour and why, when it last reported and
/// what it runs, and a colour set by hand.
#[must_use]
pub fn line(standing: &Standing) -> String {
    let colour = if standing.green { "green" } else { "not green" };
    let mut parts = vec![format!("{}: {colour}", standing.name)];
    match (&standing.over, standing.reasons.is_empty()) {
        (Some(over), _) => {
            let colour = if over.colour == canary::Colour::Green { "green" } else { "red" };
            parts.push(format!(
                "set {colour} by hand on {}: {}",
                tuf::time::format(over.set),
                over.reason
            ));
        }
        (None, false) => parts.push(standing.reasons.join("; ")),
        (None, true) => {}
    }
    parts.push(
        standing.reported.map_or_else(
            || "no report".into(),
            |at| format!("last report {}", tuf::time::format(at)),
        ),
    );
    if let Some(report) = &standing.report {
        parts.push(format!("running {} on {}", report.running, report.channel));
    }
    parts.join("; ")
}

/// What `canaries show --json` prints: each canary's standing, the names
/// of those not green, and, when `held` is the plan's, whether the plan
/// holds the waves for exactly those.
#[must_use]
pub fn json(standings: &[Standing], held: Option<&[String]>) -> Value {
    let red = canary::red(standings);
    let mut shown = json!({ "canaries": standings, "red": red });
    if let Some(held) = held {
        shown["in_step"] = json!(!differs(held, &red));
    }
    shown
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::canary::{Colour, Override};

    fn standing(name: &str, green: bool, reasons: &[&str]) -> Standing {
        Standing {
            name: name.into(),
            green,
            reasons: reasons.iter().map(|r| (*r).to_string()).collect(),
            reported: None,
            report: None,
            over: None,
            added: 0,
        }
    }

    fn names(words: &[&str]) -> Vec<String> {
        words.iter().map(|w| (*w).to_string()).collect()
    }

    #[test]
    fn the_plans_canaries_are_compared_as_a_set() {
        assert!(!differs(&names(&["a", "b"]), &names(&["b", "a"])));
        assert!(!differs(&[], &[]));
        assert!(differs(&names(&["a"]), &[]));
        assert!(differs(&[], &names(&["a"])));
        assert!(differs(&names(&["a"]), &names(&["a", "b"])));
        assert_eq!(said(&[]), "no canary holds the waves");
        assert_eq!(said(&names(&["a", "b"])), "the canaries not green are a, b");
        assert_eq!(
            change_line(&names(&["a"]), &[]),
            "the canaries not green are none, and the plan holds the waves for a"
        );
    }

    #[test]
    fn a_canary_is_one_line_with_why_and_when() {
        assert_eq!(line(&standing("ref-box", true, &[])), "ref-box: green; no report");
        let mut sick = standing("do-nyc", false, &["silent since 2026-11-15T09:12:00Z", "x"]);
        sick.reported = Some(1_800_000_000);
        sick.report = Some(canary::Report::new("do-nyc", 1_800_000_000, "0.16.0", "beta"));
        assert_eq!(
            line(&sick),
            "do-nyc: not green; silent since 2026-11-15T09:12:00Z; x; \
             last report 2027-01-15T08:00:00Z; running 0.16.0 on beta"
        );
        let mut held = standing("ref-box", true, &["green by hand: why"]);
        held.over = Some(Override { colour: Colour::Green, reason: "why".into(), set: 0 });
        assert_eq!(
            line(&held),
            "ref-box: green; set green by hand on 1970-01-01T00:00:00Z: why; no report"
        );
    }

    #[test]
    fn json_says_whether_the_plan_is_in_step() {
        let standings = vec![standing("a", true, &[]), standing("b", false, &["x"])];
        let plain = json(&standings, None);
        assert_eq!(plain["red"], json!(["b"]));
        assert!(plain.get("in_step").is_none());
        assert_eq!(plain["canaries"][1]["name"], "b");
        assert_eq!(json(&standings, Some(&names(&["b"])))["in_step"], json!(true));
        assert_eq!(json(&standings, Some(&[]))["in_step"], json!(false));
    }
}
