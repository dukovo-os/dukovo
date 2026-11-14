// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! What `sysupdate status` reads, and the document `--json` says of it
//! (ADR 0541). The text form and the document are made from the same
//! [`Facts`], so that they cannot differ in what they say. sysupdate(8)
//! describes the document, and `dukovo.system.update` reads it.

use serde::Serialize;
use slots::{Slot, State};

use super::online::Settings;
use crate::json;

/// What one slot holds.
pub(crate) struct SlotFacts {
    pub(crate) slot: Slot,
    /// Whether its subvolume is there.
    pub(crate) exists: bool,
    /// The release it says it is, when it says.
    pub(crate) version: Option<String>,
    /// The firmware set it names, when it names one.
    pub(crate) firmware: Option<String>,
}

/// The health check's last result, from its file.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Health {
    /// When it ran, in seconds since 1970, when the file says.
    pub(crate) time: Option<i64>,
    pub(crate) passed: bool,
    /// What it found, in words.
    pub(crate) detail: String,
}

impl Health {
    /// What the health check's file says in `text`: a time, `passed` or
    /// `failed`, and the words that follow; nothing when it says no
    /// result.
    pub(crate) fn parse(text: &str) -> Option<Health> {
        let mut words = text.trim().splitn(3, ' ');
        let time = words.next()?;
        let passed = match words.next()? {
            "passed" => true,
            "failed" => false,
            _ => return None,
        };
        Some(Health {
            time: time.parse().ok(),
            passed,
            detail: words.next().unwrap_or_default().to_string(),
        })
    }
}

/// The last update, as the about file of the update's folder says.
pub(crate) struct Update {
    pub(crate) from: Option<String>,
    pub(crate) to: Option<String>,
    /// The slot it went to, `a` or `b`.
    pub(crate) slot: Option<String>,
    /// Whether a fallback put the settings from before it back.
    pub(crate) restored: bool,
}

/// One EFI system partition that holds the slot state.
pub(crate) struct Partition {
    /// Its name in `/devices`.
    pub(crate) name: String,
    /// The generation of its newest whole copy, when it has one.
    pub(crate) generation: Option<u8>,
}

/// Everything `sysupdate status` says, read.
pub(crate) struct Facts {
    pub(crate) running: Slot,
    pub(crate) state: State,
    /// Slot A, then slot B.
    pub(crate) slots: [SlotFacts; 2],
    pub(crate) health: Option<Health>,
    pub(crate) update: Option<Update>,
    pub(crate) partitions: Vec<Partition>,
}

/// The members of `sysupdate status --json` after `format`.
#[derive(Serialize)]
pub(crate) struct Status {
    /// The slot that runs, `a` or `b`.
    running: &'static str,
    /// The slot that starts unless an update is being tried: the version
    /// that last proved itself.
    default: &'static str,
    /// The slot holding the version before the default, which a rollback
    /// goes back to.
    previous: Option<&'static str>,
    /// An update that has not proved itself.
    staged: Option<Staged>,
    /// The slot the next start runs.
    next: &'static str,
    /// The slot whose start was not seen through: the boot menu shows at
    /// the next start.
    unfinished_start: Option<&'static str>,
    slots: Vec<Contents>,
    /// The channel this system follows.
    channel: String,
    canary: bool,
    /// This machine's number from 0 to 999, which places it in a wave;
    /// none until the machine has drawn it.
    wave: Option<u16>,
    held: Held,
    last_health_check: Option<LastHealth>,
    last_update: Option<LastUpdate>,
    slot_state: Vec<StatePartition>,
}

/// An update that has not proved itself.
#[derive(Serialize)]
struct Staged {
    slot: &'static str,
    /// The starts it has left to prove itself in; with none, the next
    /// start runs the default.
    tries_left: u8,
}

/// What a slot holds.
#[derive(Serialize)]
struct Contents {
    slot: &'static str,
    /// The release the slot says it is; none when it is empty or says
    /// nothing.
    version: Option<String>,
    /// The firmware set it names, when it names one.
    firmware: Option<String>,
}

/// The versions held back, as `update hold` sets them.
#[derive(Serialize)]
struct Held {
    system: Option<String>,
    firmware: Option<String>,
}

/// The health check's last result.
#[derive(Serialize)]
struct LastHealth {
    /// RFC 3339; none when the file does not say.
    time: Option<String>,
    passed: bool,
    detail: String,
}

/// The last update.
#[derive(Serialize)]
struct LastUpdate {
    from: Option<String>,
    to: Option<String>,
    slot: Option<String>,
    /// Whether a fallback put the settings from before it back.
    settings_restored: bool,
}

/// A partition that holds the slot state.
#[derive(Serialize)]
struct StatePartition {
    partition: String,
    /// The generation of its newest whole copy; none when it has none.
    generation: Option<u8>,
}

impl Status {
    /// The document for `facts`, with the channel and the holds `settings`
    /// say and the machine's `wave` number.
    pub(crate) fn of(facts: &Facts, settings: &Settings, wave: Option<u16>) -> Status {
        let state = &facts.state;
        let (next, _) = state.starts();
        Status {
            running: facts.running.letter(),
            default: state.default.letter(),
            previous: state.previous.map(Slot::letter),
            staged: state
                .trying
                .map(|slot| Staged { slot: slot.letter(), tries_left: state.tries }),
            next: next.letter(),
            unfinished_start: state.boot.map(Slot::letter),
            slots: facts
                .slots
                .iter()
                .map(|held| Contents {
                    slot: held.slot.letter(),
                    version: held.version.clone(),
                    firmware: held.firmware.clone(),
                })
                .collect(),
            channel: settings.channel.clone(),
            canary: settings.canary,
            wave,
            held: Held {
                system: settings.hold.system.clone(),
                firmware: settings.hold.firmware.clone(),
            },
            last_health_check: facts.health.as_ref().map(|health| LastHealth {
                time: health.time.map(json::time),
                passed: health.passed,
                detail: health.detail.clone(),
            }),
            last_update: facts.update.as_ref().map(|update| LastUpdate {
                from: update.from.clone(),
                to: update.to.clone(),
                slot: update.slot.clone(),
                settings_restored: update.restored,
            }),
            slot_state: facts
                .partitions
                .iter()
                .map(|part| StatePartition {
                    partition: part.name.clone(),
                    generation: part.generation,
                })
                .collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sysupdate::online::Hold;

    fn slot_with(slot: Slot, version: Option<&str>, firmware: Option<&str>) -> SlotFacts {
        SlotFacts {
            slot,
            exists: version.is_some(),
            version: version.map(str::to_string),
            firmware: firmware.map(str::to_string),
        }
    }

    /// Slot A started with the default, slot B holds an update two tries
    /// into its three.
    fn updating() -> Facts {
        Facts {
            running: Slot::A,
            state: State {
                generation: 1,
                default: Slot::A,
                previous: None,
                trying: Some(Slot::B),
                tries: 2,
                boot: None,
            },
            slots: [
                slot_with(Slot::A, Some("0.15.0"), Some("2026.2")),
                slot_with(Slot::B, Some("0.16.0"), Some("2026.3")),
            ],
            health: Health::parse("1794452400 passed 14 services up"),
            update: Some(Update {
                from: Some("0.14.0".into()),
                to: Some("0.15.0".into()),
                slot: Some("a".into()),
                restored: false,
            }),
            partitions: vec![
                Partition { name: "vda1".into(), generation: Some(1) },
                Partition { name: "vda2".into(), generation: None },
            ],
        }
    }

    #[test]
    fn the_health_check_s_file_is_read_by_its_words() {
        let health = Health::parse("1794452400 failed essential service logd is down\n").unwrap();
        assert_eq!(
            health,
            Health {
                time: Some(1_794_452_400),
                passed: false,
                detail: "essential service logd is down".into()
            }
        );
        assert_eq!(Health::parse("1794452400 passed").unwrap().detail, "");
        assert_eq!(Health::parse("soon passed x").unwrap().time, None);
        assert_eq!(Health::parse("1794452400 maybe x"), None);
        assert_eq!(Health::parse(""), None);
    }

    #[test]
    fn the_document_says_the_slots_the_update_in_trial_and_the_settings() {
        let settings = Settings {
            channel: "beta".into(),
            canary: true,
            hold: Hold { system: Some("0.16.0".into()), firmware: None },
            ..Settings::default()
        };
        let line = json::line(&Status::of(&updating(), &settings, Some(412))).unwrap();
        json::assert_golden(&line, include_str!("../../tests/json/sysupdate-status.json"));
    }

    #[test]
    fn a_system_with_nothing_staged_says_none_and_a_machine_without_a_wave_says_none() {
        let facts = Facts {
            running: Slot::A,
            state: State {
                generation: 0,
                default: Slot::A,
                previous: None,
                trying: None,
                tries: 0,
                boot: None,
            },
            slots: [slot_with(Slot::A, Some("0.15.0"), None), slot_with(Slot::B, None, None)],
            health: None,
            update: None,
            partitions: vec![Partition { name: "vda1".into(), generation: Some(0) }],
        };
        let line = json::line(&Status::of(&facts, &Settings::default(), None)).unwrap();
        json::assert_golden(&line, include_str!("../../tests/json/sysupdate-status-idle.json"));
    }

    #[test]
    fn an_update_that_ran_out_of_tries_leaves_the_next_start_to_the_default() {
        let mut facts = updating();
        facts.state.tries = 0;
        facts.state.boot = Some(Slot::B);
        let document =
            serde_json::to_value(Status::of(&facts, &Settings::default(), None)).unwrap();
        assert_eq!(document["staged"]["tries_left"], 0);
        assert_eq!(document["staged"]["slot"], "b");
        assert_eq!(document["next"], "a");
        assert_eq!(document["unfinished_start"], "b");
    }
}
