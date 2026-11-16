// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! What `update` and `update history` find, and the documents `--json`
//! prints of it (ADR 0541). The text form and the document are made from
//! the same values. update(8) describes the documents, and
//! `dukovo.system.update` reads them.

use serde::Serialize;
use system_release::rollout::Advice;

use super::FirmwareOffer;
use super::advisories::SystemFlaw;
use super::held::{Because, Held};
use crate::bundles::history::Event;
use crate::bundles::protocol::Available;
use crate::bundles::registry::Place;
use crate::json;
use crate::sysupdate::checked::Found;
use crate::sysupdate::online::Plan;
use crate::sysupdate::starts::{Outcome, Start};

/// The members of `update --json` after `format`.
#[derive(Serialize)]
pub(crate) struct Updates {
    /// What each part offers: the system, the firmware set, then each app
    /// whose catalog lists a newer version.
    pub(crate) offers: Vec<Offer>,
    /// The release this system runs, when its release keys withdrew it.
    pub(crate) running_withdrawn: Option<RunningWithdrawn>,
    /// The parts that could not be checked, and why.
    pub(crate) unchecked: Vec<Unchecked>,
    /// What the check says besides, for a person.
    pub(crate) notes: Vec<String>,
    /// What the security advisories say of this machine (ADR 0534).
    pub(crate) security: Security,
}

/// The security advisories that apply: those of the system in full, and
/// how many flaws the apps have, which `update advisories` lists.
#[derive(Serialize)]
pub(crate) struct Security {
    /// The advisories that apply to the system, the worst first.
    pub(crate) system: Vec<SystemFlaw>,
    /// How many flaws the apps installed have.
    pub(crate) app_flaws: usize,
}

/// What one part offers. Each kind has the members that mean something
/// for it, after `item`, `from` and `to`.
#[derive(Serialize)]
#[serde(untagged)]
pub(crate) enum Offer {
    System(SystemOffer),
    Firmware(FirmwareSet),
    App(App),
}

/// The system update the channel offers.
#[derive(Serialize)]
pub(crate) struct SystemOffer {
    /// Always `system`.
    item: &'static str,
    /// The release this system runs.
    from: String,
    /// The release the channel offers.
    to: String,
    state: Found,
    /// With `state` `waiting`: the day after the release on which this
    /// machine's wave opens.
    wave_day: Option<i64>,
    /// With `state` `waiting`: why no further wave opens, when the
    /// rollout holds them.
    waves_held: Option<String>,
    /// With `state` `withdrawn`: why.
    withdrawn: Option<String>,
    /// With `state` `update`: the release `update apply` stages first, a
    /// bridge, when `to` updates only from a later one than this system
    /// runs.
    bridge: Option<String>,
    /// With `state` `update`: what staging fetches.
    download: Option<Download>,
    /// With `state` `held`: `fell-back` when the health check held the
    /// release it fell back from, else `asked`.
    held_because: Option<&'static str>,
    /// With `state` `held` because it fell back: when, RFC 3339.
    held_at: Option<String>,
}

/// What staging a release fetches.
#[derive(Serialize)]
pub(crate) struct Download {
    pub(crate) files: u64,
    pub(crate) bytes: u64,
}

/// The firmware set the channel offers.
#[derive(Serialize)]
pub(crate) struct FirmwareSet {
    /// Always `firmware`.
    item: &'static str,
    /// The set the running slot names; none when it names none.
    from: Option<String>,
    /// The set the channel offers; none when it offers none.
    to: Option<String>,
    /// `current`, `update` or `held`.
    state: Found,
    /// With `state` `held`: `fell-back` or `asked`, as the system's.
    held_because: Option<&'static str>,
    /// With `state` `held` because it fell back: when, RFC 3339.
    held_at: Option<String>,
}

/// An app with a newer version in its catalog or its own feed.
#[derive(Serialize)]
pub(crate) struct App {
    /// The bundle's ID.
    item: String,
    from: String,
    to: String,
    /// `update`, or `held` when the person holds it at its version.
    state: Found,
    /// The catalog it updates from; none for its own feed.
    catalog: Option<String>,
}

/// A release that was withdrawn, which this system runs.
#[derive(Serialize)]
pub(crate) struct RunningWithdrawn {
    version: String,
    reason: String,
    /// `rollback`: go back to the version before it; `stay`: stay on it
    /// until the next release.
    advice: Advice,
}

/// A part that could not be checked.
#[derive(Debug, Serialize)]
pub(crate) struct Unchecked {
    /// `system`, `firmware`, `apps` or `security`.
    pub(crate) item: &'static str,
    pub(crate) why: String,
}

impl Offer {
    /// What `plan` says of the system running `running`; `download` is
    /// what staging the update fetches, and `held` the record of a
    /// release the health check held.
    pub(crate) fn system(
        plan: &Plan,
        running: &str,
        download: Option<Download>,
        held: Option<&Held>,
    ) -> Offer {
        let (to, state) = match plan {
            Plan::Current { offered } => (offered, Found::Current),
            Plan::Waiting { offered, .. } => (offered, Found::Waiting),
            Plan::Held { offered, .. } => (offered, Found::Held),
            Plan::Withdrawn { offered, .. } => (offered, Found::Withdrawn),
            Plan::Update { offered, .. } => (offered, Found::Update),
        };
        let (wave_day, waves_held) = match plan {
            Plan::Waiting { day, held, .. } => (Some(*day), held.clone()),
            _ => (None, None),
        };
        let withdrawn = match plan {
            Plan::Withdrawn { reason, .. } => Some(reason.clone()),
            _ => None,
        };
        let bridge = match plan {
            Plan::Update { offered, release, .. } if release.version != *offered => {
                Some(release.version.clone())
            }
            _ => None,
        };
        let why = match plan {
            Plan::Held { held: version, .. } => Some(Because::of(held, version)),
            _ => None,
        };
        Offer::System(SystemOffer {
            item: "system",
            from: running.to_string(),
            to: to.clone(),
            state,
            wave_day,
            waves_held,
            withdrawn,
            bridge,
            download,
            held_because: why.as_ref().map(|why| why.because),
            held_at: why.and_then(|why| why.at),
        })
    }

    /// What the channel offers of firmware; `held` is the record of a
    /// release the health check held, which is of the system, so that a
    /// firmware set is held because it was asked for.
    pub(crate) fn firmware(offer: &FirmwareOffer, held: Option<&Held>) -> Offer {
        let why = (offer.found == Found::Held)
            .then(|| Because::of(held, offer.held.as_deref().unwrap_or_default()));
        Offer::Firmware(FirmwareSet {
            item: "firmware",
            from: offer.running.clone(),
            to: offer.offered.clone(),
            state: offer.found,
            held_because: why.as_ref().map(|why| why.because),
            held_at: why.and_then(|why| why.at),
        })
    }

    /// An app's update.
    pub(crate) fn app(row: &Available) -> Offer {
        Offer::App(App {
            item: row.id.clone(),
            from: row.from.clone(),
            to: row.to.clone(),
            state: if row.held { Found::Held } else { Found::Update },
            catalog: row.catalog.clone(),
        })
    }
}

impl RunningWithdrawn {
    pub(crate) fn new(version: &str, reason: &str, advice: Advice) -> RunningWithdrawn {
        RunningWithdrawn { version: version.into(), reason: reason.into(), advice }
    }
}

impl Unchecked {
    pub(crate) fn new(item: &'static str, why: &str) -> Unchecked {
        Unchecked { item, why: why.into() }
    }
}

/// One change of the system or the firmware set, by the start that first
/// ran it.
#[derive(Serialize)]
pub(crate) struct Change {
    /// The number of the start.
    pub(crate) start: u64,
    /// When the start began, in seconds since 1970.
    #[serde(serialize_with = "json::serialize_time")]
    pub(crate) time: i64,
    /// `system` or `firmware`.
    pub(crate) item: &'static str,
    /// What ran before and after; none for no firmware set.
    pub(crate) from: Option<String>,
    pub(crate) to: Option<String>,
    pub(crate) outcome: Outcome,
    /// For a start that failed, the error ID that told why.
    pub(crate) error: Option<String>,
}

/// The changes of the system and the firmware set `records`, oldest
/// first, show: each start that ran another version of either than the
/// start before it.
pub(crate) fn changes(records: &[Start]) -> Vec<Change> {
    let value = |start: &Start, kind: &str| {
        start.items.iter().find(|item| item.kind == kind).map(|item| item.value.clone())
    };
    let mut changes = Vec::new();
    for pair in records.windows(2) {
        let (before, after) = (&pair[0], &pair[1]);
        for item in ["system", "firmware"] {
            let (from, to) = (value(before, item), value(after, item));
            if from != to {
                changes.push(Change {
                    start: after.number,
                    time: after.started,
                    item,
                    from,
                    to,
                    outcome: after.outcome,
                    error: if after.outcome == Outcome::Failed {
                        after.error.clone()
                    } else {
                        None
                    },
                });
            }
        }
    }
    changes
}

/// The members of `update history --json` after `format`.
#[derive(Serialize)]
pub(crate) struct History {
    /// Each change of the system and the firmware set, oldest first.
    pub(crate) changes: Vec<Change>,
    /// How many starts are kept, which the changes are among.
    pub(crate) starts_kept: usize,
    /// What bundled did to this person's apps and everyone's, oldest
    /// first.
    pub(crate) apps: Vec<AppEvent>,
}

/// Something bundled did.
#[derive(Serialize)]
pub(crate) struct AppEvent {
    #[serde(serialize_with = "json::serialize_time")]
    time: i64,
    id: String,
    for_everyone: bool,
    /// `installed`, `updated`, `rolled-back`, `removed`, `held`,
    /// `released`, `waiting`, `failed` or `flawed`.
    what: &'static str,
    /// The version before and after, as far as there was one.
    from: Option<String>,
    to: Option<String>,
    /// The user ID of who asked for it; none when bundled did it by
    /// itself.
    by_user: Option<u32>,
    /// Where it came from, or why: a few words, or none.
    note: String,
}

impl AppEvent {
    pub(crate) fn of(event: &Event) -> AppEvent {
        AppEvent {
            time: i64::try_from(event.when).unwrap_or(0),
            id: event.id.clone(),
            for_everyone: event.place == Place::Everyone,
            what: event.what.word(),
            from: event.from.clone(),
            to: event.to.clone(),
            by_user: event.by,
            note: event.note.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use system_release::list::Release;

    use super::*;
    use crate::bundles::history::What;
    use crate::sysupdate::starts::Item;
    use crate::update::advisories::Fix;

    fn release(version: &str) -> Release {
        Release {
            format: 1,
            version: version.into(),
            commit: "0123456789abcdef".into(),
            toolchain: "1.95.0".into(),
            built: "2026-11-06T00:00:00Z".into(),
            updates_from: "0.14.0".into(),
            files: Vec::new(),
        }
    }

    fn app(held: bool) -> Available {
        Available {
            place: Place::User(1000),
            id: "org.nginx.nginx".into(),
            from: "1.2.0".into(),
            to: "1.3.0".into(),
            catalog: Some("dukovo".into()),
            held,
        }
    }

    #[test]
    fn a_check_says_each_part_s_offer_and_what_it_could_not_check() {
        let updates = Updates {
            offers: vec![
                Offer::system(
                    &Plan::Update {
                        offered: "0.16.0".into(),
                        from: "0.15.0".into(),
                        release: release("0.15.1"),
                    },
                    "0.14.0",
                    Some(Download { files: 37, bytes: 19_322_880 }),
                    None,
                ),
                Offer::firmware(
                    &FirmwareOffer {
                        take: Some("2026.3".into()),
                        said: String::new(),
                        running: Some("2026.2".into()),
                        offered: Some("2026.3".into()),
                        found: Found::Update,
                        held: None,
                    },
                    None,
                ),
                Offer::app(&app(false)),
                Offer::app(&Available { id: "org.example.notes".into(), ..app(true) }),
            ],
            running_withdrawn: Some(RunningWithdrawn::new(
                "0.14.0",
                "it loses files",
                Advice::Rollback,
            )),
            unchecked: vec![Unchecked::new("apps", "bundled does not answer")],
            notes: vec!["Your bundles update by themselves, daily, since 2026-10-02".into()],
            security: Security {
                system: vec![SystemFlaw {
                    id: "DKSA-2026-0001".into(),
                    severity: dkv::flaws::Severity::High,
                    summary: "A local user can read another user's files".into(),
                    aliases: vec!["CVE-2026-12345".into()],
                    fixed_in: vec!["0.16.0".into()],
                    action: Some("Update with `update apply`, then restart.".into()),
                    fix: Fix::Offered,
                    release: Some("0.16.0".into()),
                }],
                app_flaws: 2,
            },
        };
        let line = json::line(&updates).unwrap();
        json::assert_golden(&line, include_str!("../../tests/json/update.json"));
    }

    #[test]
    fn the_system_offer_says_why_it_waits_is_held_or_withdrawn() {
        let said =
            |plan: Plan| serde_json::to_value(Offer::system(&plan, "0.15.0", None, None)).unwrap();
        let waiting = said(Plan::Waiting {
            offered: "0.16.0".into(),
            day: 3,
            held: Some("paused: a disk check".into()),
        });
        assert_eq!(waiting["state"], "waiting");
        assert_eq!(waiting["wave_day"], 3);
        assert_eq!(waiting["waves_held"], "paused: a disk check");
        let held = said(Plan::Held { offered: "0.16.0".into(), held: "0.15.1".into() });
        assert_eq!(held["state"], "held");
        assert_eq!(held["wave_day"], serde_json::Value::Null);
        let withdrawn =
            said(Plan::Withdrawn { offered: "0.16.0".into(), reason: "it loses files".into() });
        assert_eq!(withdrawn["state"], "withdrawn");
        assert_eq!(withdrawn["withdrawn"], "it loses files");
        let current = said(Plan::Current { offered: "0.15.0".into() });
        assert_eq!(current["state"], "current");
        assert_eq!(current["to"], "0.15.0");
        let direct = said(Plan::Update {
            offered: "0.16.0".into(),
            from: "0.15.0".into(),
            release: release("0.16.0"),
        });
        assert_eq!(direct["state"], "update");
        assert_eq!(direct["bridge"], serde_json::Value::Null);
    }

    #[test]
    fn a_held_offer_says_whether_it_fell_back_or_was_asked_for() {
        let record: Held = serde_json::from_str(
            r#"{"version":"0.15.1","why":"fell-back","at_ms":1794000000000,"start":14}"#,
        )
        .unwrap();
        let held = Plan::Held { offered: "0.16.0".into(), held: "0.15.1".into() };
        let said = |record: Option<&Held>| {
            serde_json::to_value(Offer::system(&held, "0.15.0", None, record)).unwrap()
        };
        let fell = said(Some(&record));
        assert_eq!(fell["held_because"], "fell-back");
        assert_eq!(fell["held_at"], "2026-11-06T21:20:00Z");
        // A record of another version, or none, is a hold that was asked for.
        let other = Held { version: "0.15.0".into(), ..record.clone() };
        for record in [Some(&other), None] {
            let asked = said(record);
            assert_eq!(asked["held_because"], "asked");
            assert_eq!(asked["held_at"], serde_json::Value::Null);
        }
        // What is not held says nothing of why.
        let current = Plan::Current { offered: "0.15.0".into() };
        let current =
            serde_json::to_value(Offer::system(&current, "0.15.0", None, Some(&record))).unwrap();
        assert_eq!(current["held_because"], serde_json::Value::Null);
        assert_eq!(current["held_at"], serde_json::Value::Null);
        // A firmware set is held because it was asked for.
        let firmware = FirmwareOffer {
            take: None,
            said: String::new(),
            running: Some("2026.2".into()),
            offered: Some("2026.3".into()),
            found: Found::Held,
            held: Some("2026.3".into()),
        };
        let held = serde_json::to_value(Offer::firmware(&firmware, Some(&record))).unwrap();
        assert_eq!(held["state"], "held");
        assert_eq!(held["held_because"], "asked");
    }

    fn start(number: u64, started: i64, outcome: Outcome, system: &str, firmware: &str) -> Start {
        let mut items = vec![Item::new("system", "", system, None)];
        if !firmware.is_empty() {
            items.push(Item::new("firmware", "", firmware, None));
        }
        Start {
            number,
            started,
            slot: "a".into(),
            stage: "services".into(),
            outcome,
            error: (outcome == Outcome::Failed).then(|| "UPD-101".to_string()),
            why: None,
            failing: None,
            trial: None,
            items,
            times: None,
        }
    }

    #[test]
    fn the_history_lists_each_change_by_the_start_that_first_ran_it_and_the_apps_events() {
        let records = [
            start(7, 1_793_000_000, Outcome::Good, "0.14.0", ""),
            start(8, 1_793_100_000, Outcome::Good, "0.14.0", ""),
            start(9, 1_793_200_000, Outcome::Failed, "0.15.0", "2026.2"),
            start(10, 1_793_300_000, Outcome::Good, "0.14.0", ""),
            start(11, 1_794_000_000, Outcome::Unfinished, "0.15.0", "2026.2"),
        ];
        let history = History {
            changes: changes(&records),
            starts_kept: records.len(),
            apps: vec![AppEvent::of(&Event {
                when: 1_793_500_000,
                place: Place::Everyone,
                id: "org.nginx.nginx".into(),
                what: What::Updated,
                from: Some("1.2.0".into()),
                to: Some("1.3.0".into()),
                by: None,
                note: "from catalog dukovo".into(),
            })],
        };
        let line = json::line(&history).unwrap();
        json::assert_golden(&line, include_str!("../../tests/json/update-history.json"));
    }

    #[test]
    fn starts_that_ran_the_same_things_make_no_change() {
        let records = [
            start(1, 1_793_000_000, Outcome::Good, "0.15.0", "2026.2"),
            start(2, 1_793_100_000, Outcome::Good, "0.15.0", "2026.2"),
        ];
        assert!(changes(&records).is_empty());
        assert!(changes(&records[..1]).is_empty());
    }
}
