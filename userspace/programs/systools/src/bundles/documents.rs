// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The document `bundle list --json` prints (ADR 0541), made from what
//! bundled lists: the bundles this person can run, theirs and everyone's.
//! `bundle info --json` is bundled's own document ([`super::sheet::Details`]),
//! since only bundled holds its facts. bundle(1) describes both, and
//! `dukovo.system.bundle` reads them.

use serde::Serialize;

use super::protocol::{Flags, Listed};
use super::registry::Place;

/// The members of `bundle list --json` after `format`.
#[derive(Serialize)]
pub(crate) struct List {
    bundles: Vec<Bundle>,
}

/// An installed bundle.
#[derive(Serialize)]
struct Bundle {
    id: String,
    name: String,
    version: String,
    for_everyone: bool,
    /// The catalog it is bound to; none when it updates from its own
    /// feed, or from none.
    catalog: Option<String>,
    held: bool,
    /// While it is held, the reason kept with the hold, if one was.
    held_reason: Option<String>,
    /// While it is held, the user ID of who held it.
    held_by: Option<u32>,
    /// While it is held, since when, in milliseconds since 1970.
    held_since_ms: Option<u64>,
    /// The newest version the catalog listed when bundled last read it,
    /// when that is newer than `version`.
    update: Option<String>,
    /// While its update is asked for tonight's window, what was asked.
    tonight: Option<Tonight>,
    flaws: Flaws,
}

/// An update asked for tonight, as `bundle list --json` says it.
#[derive(Serialize)]
struct Tonight {
    /// The version it installs.
    to: String,
    asked_ms: u64,
    /// The user ID of who asked.
    by: u32,
    /// When a window first passed without it, if one did.
    missed_ms: Option<u64>,
    /// When the window it installs in starts, in milliseconds since 1970.
    starts_ms: u64,
}

/// What the catalog's list of known flaws says of the version: `unknown`
/// when it says nothing, `none`, or `known` with the count and the
/// worst severity.
#[derive(Serialize)]
struct Flaws {
    state: &'static str,
    count: u32,
    worst: Option<&'static str>,
}

impl Flaws {
    fn of(flags: Flags) -> Flaws {
        match flags {
            Flags::Unknown => Flaws { state: "unknown", count: 0, worst: None },
            Flags::None => Flaws { state: "none", count: 0, worst: None },
            Flags::Known { count, worst } => {
                Flaws { state: "known", count, worst: Some(worst.word()) }
            }
        }
    }
}

impl List {
    /// The bundles `rows` list, by ID and then place, as `bundle list`
    /// shows them.
    pub(crate) fn of(mut rows: Vec<Listed>) -> List {
        rows.sort_by(|a, b| a.id.cmp(&b.id).then(a.place.cmp(&b.place)));
        let bundles = rows
            .into_iter()
            .map(|row| Bundle {
                id: row.id,
                name: row.name,
                version: row.version,
                for_everyone: row.place == Place::Everyone,
                catalog: row.catalog,
                held: row.held,
                held_reason: row.held_reason.filter(|_| row.held),
                held_by: row.held_by.filter(|_| row.held),
                held_since_ms: row.held_since.filter(|_| row.held).map(|s| s.saturating_mul(1000)),
                update: row.update,
                // Everyone's apps alone are asked for tonight.
                tonight: row.tonight.filter(|_| row.place == Place::Everyone).map(|t| Tonight {
                    to: t.to,
                    asked_ms: t.asked_ms,
                    by: t.by,
                    missed_ms: t.missed_ms,
                    starts_ms: t.starts_ms,
                }),
                flaws: Flaws::of(row.flags),
            })
            .collect();
        List { bundles }
    }
}

#[cfg(test)]
mod tests {
    use dkv::flaws::Severity;

    use super::*;
    use crate::bundles::minisign::SecretKey;

    fn row(place: Place, id: &str, version: &str, flags: Flags) -> Listed {
        let mut random = [9u8; 40];
        random[0] = 1;
        Listed {
            place,
            id: id.into(),
            version: version.into(),
            key: SecretKey::generate(&random).unwrap().public(),
            when: 1_793_000_000,
            name: id.rsplit('.').next().unwrap().into(),
            flags,
            catalog: None,
            held: false,
            held_since: None,
            held_by: None,
            held_reason: None,
            update: None,
            tonight: None,
        }
    }

    #[test]
    fn the_document_holds_each_bundle_with_its_catalog_hold_update_and_flaws() {
        let rows = vec![
            Listed {
                catalog: Some("dukovo".into()),
                update: Some("1.3.0".into()),
                tonight: Some(crate::bundles::protocol::Tonight {
                    to: "1.3.0".into(),
                    asked_ms: 1_793_200_000_000,
                    by: 1000,
                    missed_ms: None,
                    starts_ms: 1_793_300_000_000,
                }),
                ..row(
                    Place::Everyone,
                    "org.nginx.nginx",
                    "1.2.0",
                    Flags::Known { count: 2, worst: Severity::High },
                )
            },
            row(Place::User(1000), "org.example.notes", "0.1.0", Flags::Unknown),
            Listed {
                catalog: Some("dukovo".into()),
                held: true,
                held_since: Some(1_793_100_000),
                held_by: Some(1000),
                held_reason: Some("app tests use this version".into()),
                ..row(Place::User(1000), "org.nginx.nginx", "1.1.0", Flags::None)
            },
        ];
        let line = crate::json::line(&List::of(rows)).unwrap();
        crate::json::assert_golden(&line, include_str!("../../tests/json/bundle-list.json"));
    }

    #[test]
    fn no_bundles_are_an_empty_list() {
        assert_eq!(
            crate::json::line(&List::of(Vec::new())).unwrap(),
            r#"{"format":1,"bundles":[]}"#
        );
    }
}
