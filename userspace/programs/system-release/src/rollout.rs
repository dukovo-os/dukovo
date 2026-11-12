// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Waves, pauses and withdrawals (ADRs 0456 and 0469). A release a
//! channel offers reaches machines in waves: each machine keeps a number
//! from 0 to 999, and the update server's rollout role signs, for each
//! release, the highest wave open. Wave 0 is the canaries, on release
//! day; wave 1 opens numbers 0 to 9 on day 1, wave 2 to 99 on day 3,
//! wave 3 to 499 on day 5, and wave 4 every machine on day 7. A wave
//! opens on schedule only while every canary is green and no one has
//! paused the release; a pause closes no wave already open. A
//! withdrawal, signed by the release keys, takes a release out of every
//! channel, and a machine that staged it unstages it.

use serde::{Deserialize, Serialize};

/// The waves: the day after release each opens, and the machine numbers
/// below which it reaches; wave 0, the canaries, reaches none by number.
pub const WAVES: [(i64, u16); 5] = [(0, 0), (1, 10), (3, 100), (5, 500), (7, 1000)];

/// The highest wave.
pub const LAST: u8 = 4;

/// Seconds in a day.
const DAY: i64 = 24 * 60 * 60;

/// The path of release `version`'s rollout among the rollout role's
/// targets.
#[must_use]
pub fn target_path(version: &str) -> String {
    format!("rollout/{version}.json")
}

/// The path of the withdrawal of release `version` among the targets.
#[must_use]
pub fn withdrawal_path(version: &str) -> String {
    format!("withdrawn/{version}.json")
}

/// What the rollout role signs for a release: the highest wave open, and
/// why no further one opens, if one is held.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rollout {
    /// The release.
    pub release: String,
    /// The highest wave open, 0 to [`LAST`].
    pub wave: u8,
    /// Why no further wave opens: a pause's reason, or a canary that is
    /// not green.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub held: Option<String>,
}

/// The delegated role that signs the rollouts, with the online key.
pub const ROLE: &str = "rollout";

/// What the update server knows of a release, from which it signs its
/// [`Rollout`] each time it refreshes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Plan {
    /// When a channel first offered the release, in seconds since 1970.
    pub released: i64,
    /// The wave open at the last refresh.
    pub open: u8,
    /// The reason of a pause, if one was asked for.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub paused: Option<String>,
}

/// The update server's plan of every release a channel offered, and the
/// canaries that are not green, kept beside the repository, never served.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Plans {
    /// Each release's plan, by version.
    #[serde(default)]
    pub releases: std::collections::BTreeMap<String, Plan>,
    /// The canaries that are not green, by name.
    #[serde(default)]
    pub red: Vec<String>,
}

impl Plans {
    /// The plans kept at `path`, or none if there is no file yet.
    ///
    /// # Errors
    ///
    /// If the file cannot be read, or is not a plan.
    pub fn load(path: &std::path::Path) -> Result<Plans, String> {
        match std::fs::read(path) {
            Ok(bytes) => {
                serde_json::from_slice(&bytes).map_err(|err| format!("{}: {err}", path.display()))
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(Plans::default()),
            Err(err) => Err(format!("{}: {err}", path.display())),
        }
    }

    /// Keeps the plans at `path`, whole or not at all.
    ///
    /// # Errors
    ///
    /// If it cannot be written.
    ///
    /// # Panics
    ///
    /// Never: plans always serialise.
    pub fn save(&self, path: &std::path::Path) -> Result<(), String> {
        let mut bytes = serde_json::to_vec_pretty(self).expect("plans serialise");
        bytes.push(b'\n');
        let partial = path.with_extension("partial");
        std::fs::write(&partial, &bytes)
            .and_then(|()| std::fs::rename(&partial, path))
            .map_err(|err| format!("{}: {err}", path.display()))
    }
}

/// The targets' delegation of the rollouts to the online key, the key
/// the root `root` gives timestamp: `rollout/` alone, and no further.
///
/// # Errors
///
/// If `root` is not a root, or names no key for timestamp.
pub fn delegation(root: &[u8]) -> Result<serde_json::Value, String> {
    let root = tuf::metadata::Signed::<tuf::metadata::Root>::parse(root)
        .map_err(|err| format!("the root: {err}"))?;
    let ids = &root.signed.roles.get("timestamp").ok_or("the root names no timestamp key")?.keyids;
    let mut keys = serde_json::Map::new();
    for id in ids {
        let key = root.signed.keys.get(id).ok_or("the root lacks its timestamp key")?;
        keys.insert(id.clone(), serde_json::to_value(key).map_err(|err| err.to_string())?);
    }
    Ok(serde_json::json!({
        "keys": keys,
        "roles": [{
            "name": ROLE,
            "keyids": ids,
            "threshold": 1,
            "paths": ["rollout/*"],
            "terminating": true,
        }],
    }))
}

/// The wave the schedule opens `now`, for a release added at `released`.
#[must_use]
pub fn scheduled(released: i64, now: i64) -> u8 {
    let days = now.saturating_sub(released).max(0) / DAY;
    let open = WAVES.iter().take_while(|(day, _)| *day <= days).count();
    u8::try_from(open.saturating_sub(1)).unwrap_or(LAST)
}

/// The rollout to sign `now`: the scheduled wave, unless a pause or a
/// canary of `red`, not green, holds it where it is.
#[must_use]
pub fn next(release: &str, plan: &Plan, red: &[String], now: i64) -> Rollout {
    let held = match (&plan.paused, red.is_empty()) {
        (Some(reason), _) => Some(format!("paused: {reason}")),
        (None, false) => Some(format!("canaries not green: {}", red.join(", "))),
        (None, true) => None,
    };
    let wave =
        if held.is_some() { plan.open } else { plan.open.max(scheduled(plan.released, now)) };
    Rollout { release: release.to_string(), wave: wave.min(LAST), held }
}

/// Whether a machine with `number`, or a canary, is in a wave open in
/// `rollout`.
#[must_use]
pub fn reaches(rollout: &Rollout, number: u16, canary: bool) -> bool {
    canary || WAVES.get(usize::from(rollout.wave)).is_some_and(|(_, below)| number < *below)
}

/// The day after release the wave that reaches `number` opens.
#[must_use]
pub fn day_for(number: u16) -> i64 {
    WAVES.iter().find(|(_, below)| number < *below).map_or(7, |(day, _)| *day)
}

/// What a machine running a withdrawn release is told to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Advice {
    /// Go back to the version before it.
    Rollback,
    /// Stay on it until the next release.
    Stay,
}

/// A withdrawal, signed by the release keys: the release leaves every
/// channel, and its version is never used again.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Withdrawal {
    /// The release withdrawn.
    pub release: String,
    /// Why, in words the owner reads.
    pub reason: String,
    /// What a machine running it should do.
    pub advice: Advice,
}

impl Withdrawal {
    /// The withdrawal's bytes, as signed.
    ///
    /// # Panics
    ///
    /// Never: a withdrawal always serialises.
    #[must_use]
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut bytes = serde_json::to_vec_pretty(self).expect("a withdrawal serialises");
        bytes.push(b'\n');
        bytes
    }

    /// Reads a withdrawal.
    ///
    /// # Errors
    ///
    /// If it is not one, or has no reason.
    pub fn parse(bytes: &[u8]) -> Result<Withdrawal, String> {
        let withdrawal: Withdrawal =
            serde_json::from_slice(bytes).map_err(|err| format!("a withdrawal: {err}"))?;
        if withdrawal.reason.trim().is_empty() {
            return Err(format!("the withdrawal of {} gives no reason", withdrawal.release));
        }
        Ok(withdrawal)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn waves_open_on_their_days() {
        let day = |n: i64| 1_000_000 + n * DAY;
        assert_eq!(scheduled(day(0), day(0)), 0);
        assert_eq!(scheduled(day(0), day(1) - 1), 0);
        assert_eq!(scheduled(day(0), day(1)), 1);
        assert_eq!(scheduled(day(0), day(4)), 2);
        assert_eq!(scheduled(day(0), day(5)), 3);
        assert_eq!(scheduled(day(0), day(30)), LAST);
        // A clock before the release opens nothing past the canaries.
        assert_eq!(scheduled(day(2), day(0)), 0);
        assert_eq!((day_for(9), day_for(10), day_for(499), day_for(999)), (1, 3, 5, 7));
    }

    #[test]
    fn canaries_and_pauses_hold_the_waves_open() {
        let plan = Plan { released: 0, open: 1, paused: None };
        let now = 5 * DAY;
        assert_eq!(
            next("0.15.1", &plan, &[], now),
            Rollout { release: "0.15.1".into(), wave: 3, held: None }
        );
        let held = next("0.15.1", &plan, &["dell".into()], now);
        assert_eq!((held.wave, held.held.as_deref()), (1, Some("canaries not green: dell")));
        let paused = Plan { paused: Some("disk errors on NVMe".into()), ..plan };
        assert_eq!(
            next("0.15.1", &paused, &[], now).held.as_deref(),
            Some("paused: disk errors on NVMe")
        );
        assert_eq!(next("0.15.1", &paused, &[], now).wave, 1);
        let plans =
            Plans { releases: [("0.15.1".into(), paused)].into(), red: vec!["dell".into()] };
        let path = std::env::temp_dir().join(format!("rollout-plans-{}", std::process::id()));
        plans.save(&path).unwrap();
        assert_eq!(Plans::load(&path), Ok(plans));
        std::fs::remove_file(&path).unwrap();
        assert_eq!(Plans::load(&path), Ok(Plans::default()));
    }

    #[test]
    fn a_machine_takes_a_release_once_its_wave_is_open() {
        let at = |wave| Rollout { release: "0.15.1".into(), wave, held: None };
        assert!(reaches(&at(0), 999, true));
        assert!(!reaches(&at(0), 0, false));
        assert!(reaches(&at(1), 9, false) && !reaches(&at(1), 10, false));
        assert!(reaches(&at(3), 499, false) && !reaches(&at(3), 500, false));
        assert!(reaches(&at(LAST), 999, false));
        let withdrawal = Withdrawal {
            release: "0.15.1".into(),
            reason: "corrupts".into(),
            advice: Advice::Rollback,
        };
        assert_eq!(Withdrawal::parse(&withdrawal.to_bytes()), Ok(withdrawal));
        assert!(Withdrawal::parse(br#"{"release":"1","reason":" ","advice":"stay"}"#).is_err());
    }
}
