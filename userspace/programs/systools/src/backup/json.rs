// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! What `backup list`, `show`, `key` and `ssh-key` say with `--json`: the
//! web console's interface to them, so that the page and the command
//! cannot drift apart. Times are milliseconds since 1970 under names that
//! end in `_ms`, as the console's other commands write them; a time that
//! is not known, a text that is empty and a count of nothing are null.
//!
//! Backups are named by the local time they were made, so their `at_ms`
//! is that time read in the machine's time zone.

use std::collections::BTreeMap;
use std::path::Path;

use serde_json::{Value, json};

use super::layout::{self, Listing};
use super::restic::Snapshot;
use super::store::Address;
use super::{Destination, Format, State, overdue_days, s3};

/// The shape of `backup list --json`, for the page to check what it reads.
const LIST_FORMAT: u32 = 1;

/// `ns` since 1970 as a time in milliseconds, null for none (0).
fn ms(ns: u64) -> Value {
    if ns == 0 { Value::Null } else { json!(ns / 1_000_000) }
}

/// `text`, or null when it is empty.
fn text(text: &str) -> Value {
    if text.is_empty() { Value::Null } else { json!(text) }
}

/// The milliseconds since 1970 of the backup time `name`
/// (`YYYY-MM-DD-HHMMSS`, local time); none for a name that is not one or
/// names a time that does not exist.
pub fn time_ms(name: &str) -> Option<u64> {
    if !layout::is_time(name) {
        return None;
    }
    let part = |from: usize, to: usize| name[from..to].parse::<u32>().ok();
    let (year, month, day) = (part(0, 4)?, part(5, 7)?, part(8, 10)?);
    let (hour, minute, second) = (part(11, 13)?, part(13, 15)?, part(15, 17)?);
    let asked = sysapi::time::Civil {
        year: i64::from(year),
        month,
        day,
        hour,
        minute,
        second,
        ..Default::default()
    };
    let (seconds, found) = sysapi::time::from_local(&asked).ok()?;
    // A field out of range comes back changed.
    let same = (asked.month, asked.day, asked.hour, asked.minute, asked.second)
        == (found.month, found.day, found.hour, found.minute, found.second);
    u64::try_from(seconds).ok().filter(|_| same).map(|seconds| seconds * 1000)
}

/// How a destination's `to` reaches it: `ssh`, `s3` or `folder`.
pub fn kind(to: &str) -> &'static str {
    if s3::Address::parse(to).is_ok_and(|address| address.is_some()) {
        "s3"
    } else if Address::parse(to).is_ok_and(|address| address.is_some()) {
        "ssh"
    } else {
        "folder"
    }
}

/// For a folder under a volume of `volumes`, whether the volume is
/// plugged in, if it is one that can be unplugged: a volume that is not
/// there is one that is not plugged in, and one that is there is when
/// `removable` says it is a drive's. Null for any other destination.
pub fn plugged_in(to: &str, volumes: &Path, removable: &dyn Fn(&Path) -> bool) -> Option<bool> {
    let first = Path::new(to).strip_prefix(volumes).ok()?.components().next()?;
    let volume = volumes.join(first);
    if !volume.is_dir() {
        return Some(false);
    }
    removable(&volume).then_some(true)
}

/// What `backup list` finds of a destination besides its settings and
/// state.
pub struct Outside {
    /// When it was added, in nanoseconds since 1970: its settings file's
    /// time.
    pub added: u64,
    /// When its key file last changed, in milliseconds; none without a
    /// key file.
    pub key_ms: Option<u64>,
    /// The places it backs up.
    pub places: Vec<String>,
    /// [`plugged_in`].
    pub plugged_in: Option<bool>,
}

/// One destination, as `backup list --json` has it, at `now` nanoseconds
/// since 1970. `running` is whether a round holds the lock.
pub fn destination(
    dest: &Destination,
    state: &State,
    outside: &Outside,
    running: bool,
    now: u64,
) -> Value {
    let every = match dest.every {
        super::Every::Day => "day",
        super::Every::Week => "week",
        super::Every::Hand => "hand",
    };
    let verify = match dest.verify {
        super::Verify::Week => "week",
        super::Verify::Month => "month",
        super::Verify::Never => "never",
    };
    let format = match dest.format {
        Format::Btrfs => "btrfs",
        Format::Restic => "restic",
    };
    json!({
        "name": dest.name,
        "to": dest.to,
        "kind": kind(&dest.to),
        "format": format,
        "every": every,
        "verify": verify,
        "keeps": {
            "chain": dest.chain,
            "chains": dest.chains,
            "daily": dest.keep.daily,
            "weekly": dest.keep.weekly,
            "monthly": dest.keep.monthly,
        },
        "places": outside.places,
        "append_only": dest.append_only,
        "recipients": dest.recipients.len(),
        "plugged_in": outside.plugged_in,
        "key_ms": outside.key_ms,
        "state": {
            "tried_ms": ms(state.tried),
            "done_ms": ms(state.done),
            "failed": text(&state.failed),
            "verified_ms": ms(state.verified),
            "verify_failed": text(&state.verify_failed),
        },
        "overdue_days": overdue_days(dest, state, outside.added, now),
        "running": running,
    })
}

/// `backup list --json`: the destinations `places`, as [`destination`]
/// makes them, and the whole-machine restore staged, `(machine, time)`,
/// when the caller may see it.
pub fn list(places: &[Value], staged: Option<(String, String)>) -> Value {
    let staged = staged.map(|(from, at)| json!({ "from": from, "at": at }));
    json!({ "format": LIST_FORMAT, "places": places, "staged": staged })
}

/// A backup, which is whole when it is the send that begins its chain.
fn backup(time: &str, whole: bool) -> Value {
    json!({ "time": time, "at_ms": time_ms(time), "whole": whole })
}

/// A record of the machine, by the time of its round.
fn record(time: &str) -> Value {
    json!({ "time": time, "at_ms": time_ms(time) })
}

/// `backup show NAME --json` for a destination of streams: what its
/// `listing` holds, at `place`. `paths` says each place's path on this
/// machine by its folder; a folder this machine no longer backs up has
/// none.
pub fn show_streams(
    name: &str,
    place: &str,
    listing: &Listing,
    paths: &BTreeMap<String, String>,
) -> Value {
    let places: Vec<Value> = listing
        .places
        .iter()
        .map(|(folder, chains)| {
            let backups: Vec<Value> = chains
                .iter()
                .flat_map(|chain| {
                    chain.backups.iter().map(|b| backup(&b.time, b.time == chain.name))
                })
                .collect();
            json!({ "path": paths.get(folder), "folder": folder, "backups": backups })
        })
        .collect();
    let records: Vec<Value> = listing.records.iter().map(|r| record(&r.time)).collect();
    json!({
        "name": name,
        "place": place,
        "format": "btrfs",
        "places": places,
        "records": records,
    })
}

/// `backup show NAME --json` for a restic repository at `place`: its
/// `snapshots`, each of which is whole.
pub fn show_restic(name: &str, place: &str, snapshots: &[Snapshot]) -> Value {
    let mut by_place: BTreeMap<(&str, &str), Vec<&str>> = BTreeMap::new();
    let mut records: Vec<&str> = Vec::new();
    for snapshot in snapshots {
        if snapshot.is_record() {
            records.push(&snapshot.time);
        } else {
            by_place.entry((&snapshot.path, &snapshot.folder)).or_default().push(&snapshot.time);
        }
    }
    let places: Vec<Value> = by_place
        .into_iter()
        .map(|((path, folder), mut times)| {
            times.sort_unstable();
            let backups: Vec<Value> = times.into_iter().map(|time| backup(time, true)).collect();
            json!({ "path": path, "folder": folder, "backups": backups })
        })
        .collect();
    records.sort_unstable();
    let records: Vec<Value> = records.into_iter().map(record).collect();
    json!({
        "name": name,
        "place": place,
        "format": "restic",
        "places": places,
        "records": records,
    })
}

/// `backup key NAME --json`: the destination's recovery key, the secret
/// line, and its public half.
pub fn key(name: &str, secret: &str, public: &str) -> Value {
    json!({ "name": name, "key": secret, "public": public })
}

/// `backup ssh-key --json`: the machine's public key for servers.
pub fn ssh_key(public: &str) -> Value {
    json!({ "public": public })
}

#[cfg(test)]
mod tests {
    use super::super::NANOS;
    use super::*;
    use crate::util;

    fn dest(text: &str) -> Destination {
        Destination::parse("nas", text).unwrap()
    }

    #[test]
    fn a_backup_time_is_milliseconds_in_the_local_zone_and_nothing_else_is() {
        let ms = time_ms("2026-11-10-020412").unwrap();
        assert_eq!(ms % 1000, 0);
        // Read back in the same zone, it is the time it was.
        assert_eq!(util::local_time((ms / 1000) as i64, "%Y-%m-%d-%H%M%S"), "2026-11-10-020412");
        for bad in ["", "2026-11-10-0204", "2026-13-10-020412", "2026-11-31-020412", "x"] {
            assert_eq!(time_ms(bad), None, "{bad}");
        }
        assert_eq!(time_ms("2026-11-10-250000"), None);
    }

    #[test]
    fn a_destination_says_how_it_is_reached() {
        assert_eq!(kind("/volumes/BACKUP/dukovo-backup"), "folder");
        assert_eq!(kind("ssh://backup@192.168.1.20/srv/dukovo"), "ssh");
        assert_eq!(kind("s3://bucket/dukovo"), "s3");
    }

    #[test]
    fn a_folder_on_a_volume_that_can_be_unplugged_says_whether_it_is_there() {
        let root = std::env::temp_dir().join(format!("backup-json-volumes-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("STICK")).unwrap();
        std::fs::create_dir_all(root.join("DATA")).unwrap();
        let removable = |volume: &Path| volume.ends_with("STICK");
        let to = |name: &str| root.join(name).join("dukovo-backup").display().to_string();
        assert_eq!(plugged_in(&to("STICK"), &root, &removable), Some(true));
        // A volume that stays in the machine is not one that is plugged in.
        assert_eq!(plugged_in(&to("DATA"), &root, &removable), None);
        // One that is not there is one that is not plugged in.
        assert_eq!(plugged_in(&to("GONE"), &root, &removable), Some(false));
        assert_eq!(plugged_in("/srv/backups", &root, &removable), None);
        assert_eq!(plugged_in("ssh://me@nas/srv", &root, &removable), None);
        assert_eq!(plugged_in(&root.display().to_string(), &root, &removable), None);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_destination_is_listed_with_what_it_last_did() {
        let day = 86_400 * NANOS;
        let dest = dest("to = \"ssh://backup@192.168.1.20/srv/dukovo\"\nchain = 7\nchains = 3\n");
        let state = State {
            tried: 1_763_082_240_000 * 1_000_000,
            done: 1_762_740_240_000 * 1_000_000,
            failed: "the server did not answer".into(),
            verified: 1_761_980_000_000 * 1_000_000,
            ..State::default()
        };
        let outside = Outside {
            added: 0,
            key_ms: Some(1_760_900_000_000),
            places: vec!["/users".into(), "/applications".into()],
            plugged_in: None,
        };
        let now = state.done + 3 * day;
        assert_eq!(
            destination(&dest, &state, &outside, false, now),
            json!({
                "name": "nas",
                "to": "ssh://backup@192.168.1.20/srv/dukovo",
                "kind": "ssh",
                "format": "btrfs",
                "every": "day",
                "verify": "month",
                "keeps": {"chain": 7, "chains": 3, "daily": 7, "weekly": 4, "monthly": 12},
                "places": ["/users", "/applications"],
                "append_only": false,
                "recipients": 0,
                "plugged_in": null,
                "key_ms": 1_760_900_000_000u64,
                "state": {
                    "tried_ms": 1_763_082_240_000u64,
                    "done_ms": 1_762_740_240_000u64,
                    "failed": "the server did not answer",
                    "verified_ms": 1_761_980_000_000u64,
                    "verify_failed": null,
                },
                "overdue_days": 3,
                "running": false,
            })
        );
    }

    #[test]
    fn a_destination_never_tried_has_null_times_and_texts_and_is_not_overdue_at_first() {
        let dest = dest("to = \"/volumes/BACKUP\"\nevery = \"hand\"\n");
        let outside = Outside { added: 0, key_ms: None, places: Vec::new(), plugged_in: None };
        let listed = destination(&dest, &State::default(), &outside, true, 5 * 86_400 * NANOS);
        assert_eq!(
            listed["state"],
            json!({"tried_ms": null, "done_ms": null, "failed": null,
                   "verified_ms": null, "verify_failed": null})
        );
        // A destination backed up by hand is never late.
        assert_eq!((&listed["overdue_days"], &listed["running"]), (&json!(null), &json!(true)));
        assert_eq!((&listed["every"], &listed["key_ms"]), (&json!("hand"), &json!(null)));
    }

    #[test]
    fn a_destination_that_was_never_backed_up_is_overdue_from_when_it_was_added() {
        let dest = dest("to = \"/volumes/BACKUP\"\n");
        let day = 86_400 * NANOS;
        let outside = Outside { added: day, key_ms: None, places: Vec::new(), plugged_in: None };
        let listed = |now| {
            destination(&dest, &State::default(), &outside, false, now)["overdue_days"].clone()
        };
        assert_eq!(listed(3 * day), json!(null));
        assert_eq!(listed(4 * day), json!(3));
    }

    #[test]
    fn the_list_has_its_format_and_the_staged_restore_only_when_it_is_given() {
        assert_eq!(list(&[], None), json!({"format": 1, "places": [], "staged": null}));
        assert_eq!(
            list(&[], Some(("duke-nas".into(), "2026-11-14-013000".into())))["staged"],
            json!({"from": "duke-nas", "at": "2026-11-14-013000"})
        );
    }

    fn listing() -> Listing {
        let names = [
            "machine/2026-11-09-020000.age",
            "machine/2026-11-10-020000.age",
            "machine/2026-11-10-020000.tag",
            "users/2026-11-09-020000/2026-11-09-020000.age",
            "users/2026-11-09-020000/2026-11-10-020000.age",
            "users/2026-11-11-020000/2026-11-11-020000.age",
            "old/2026-10-01-020000/2026-10-02-020000.age",
            "backup.toml",
        ];
        Listing::read(names)
    }

    #[test]
    fn what_a_destination_of_streams_holds_is_shown_by_place_with_its_whole_backups() {
        let paths = BTreeMap::from([("users".to_string(), "/users".to_string())]);
        let shown = show_streams("nas", "/volumes/BACKUP/dukovo-backup/duke", &listing(), &paths);
        assert_eq!((&shown["name"], &shown["format"]), (&json!("nas"), &json!("btrfs")));
        assert_eq!(shown["place"], "/volumes/BACKUP/dukovo-backup/duke");
        let places = shown["places"].as_array().unwrap();
        assert_eq!(places.len(), 2);
        // By folder: the one that this machine no longer backs up first,
        // and without a path.
        assert_eq!((&places[0]["folder"], &places[0]["path"]), (&json!("old"), &json!(null)));
        let wholes: Vec<(&str, bool)> = places[0]["backups"]
            .as_array()
            .unwrap()
            .iter()
            .map(|b| (b["time"].as_str().unwrap(), b["whole"].as_bool().unwrap()))
            .collect();
        // A chain that lost its whole send has no whole backup.
        assert_eq!(wholes, [("2026-10-02-020000", false)]);
        assert_eq!((&places[1]["folder"], &places[1]["path"]), (&json!("users"), &json!("/users")));
        let wholes: Vec<(&str, bool)> = places[1]["backups"]
            .as_array()
            .unwrap()
            .iter()
            .map(|b| (b["time"].as_str().unwrap(), b["whole"].as_bool().unwrap()))
            .collect();
        assert_eq!(
            wholes,
            [
                ("2026-11-09-020000", true),
                ("2026-11-10-020000", false),
                ("2026-11-11-020000", true)
            ]
        );
        assert_eq!(places[1]["backups"][0]["at_ms"], json!(time_ms("2026-11-09-020000")));
        let records = shown["records"].as_array().unwrap();
        assert_eq!(records.len(), 2);
        assert_eq!(
            records[1],
            json!({"time": "2026-11-10-020000", "at_ms": time_ms("2026-11-10-020000")})
        );
    }

    #[test]
    fn a_destination_with_nothing_yet_shows_empty_lists() {
        let shown = show_streams("nas", "there", &Listing::default(), &BTreeMap::new());
        assert_eq!((&shown["places"], &shown["records"]), (&json!([]), &json!([])));
    }

    #[test]
    fn the_keys_are_one_member_each_and_the_secret_is_only_in_the_key_that_is_one() {
        assert_eq!(
            key("nas", "AGE-SECRET-KEY-1ABC", "age1abc"),
            json!({"name": "nas", "key": "AGE-SECRET-KEY-1ABC", "public": "age1abc"})
        );
        assert_eq!(
            ssh_key("ssh-ed25519 AAAA backup@duke"),
            json!({"public": "ssh-ed25519 AAAA backup@duke"})
        );
    }
}
