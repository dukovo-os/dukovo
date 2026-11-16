// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! `lastlog --json`: each account's last sign-in and the wrong passwords
//! typed for it since, as the console's Accounts page reads them. A time is
//! Unix milliseconds. An address is null where the sign-in was at the
//! machine itself, and a sign-in or a wrong password is null where there
//! was none. `via` says how the person came, `ssh` or `console`, and is
//! null where the record does not say, as the Security page's SSH trial
//! lists only the sign-ins over SSH.

use accounts::signins::{Latest, Place};
use serde::Serialize;

use crate::util::{Fail, fail};

/// An account's row.
#[derive(Debug, PartialEq, Eq, Serialize)]
struct Row<'a> {
    name: &'a str,
    /// Its newest sign-in, null if it never signed in.
    sign_in: Option<Seen>,
    /// How many wrong passwords were typed for it since, or ever if it
    /// never signed in.
    failed: usize,
    /// The newest of them.
    last_failed: Option<Seen>,
}

/// When, where and where from.
#[derive(Debug, PartialEq, Eq, Serialize)]
struct Seen {
    at_ms: i64,
    terminal: String,
    from: Option<String>,
    via: Option<String>,
}

impl From<&Place> for Seen {
    fn from(place: &Place) -> Seen {
        Seen {
            at_ms: place.time.saturating_mul(1000),
            terminal: place.terminal.clone(),
            from: Some(place.from.clone()).filter(|from| !from.is_empty()),
            via: Some(place.via.clone()).filter(|via| !via.is_empty()),
        }
    }
}

/// The array `lastlog --json` writes for these `accounts`, by name.
pub(super) fn render(accounts: &[(&str, Latest)]) -> Result<String, Fail> {
    let rows: Vec<Row> = accounts
        .iter()
        .map(|(name, latest)| Row {
            name,
            sign_in: latest.sign_in.as_ref().map(Seen::from),
            failed: latest.failed,
            last_failed: latest.last_failed.as_ref().map(Seen::from),
        })
        .collect();
    serde_json::to_string(&rows).map_err(|e| fail(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};

    fn place(time: i64, terminal: &str, from: &str) -> Place {
        Place { time, terminal: terminal.into(), from: from.into(), via: String::new() }
    }

    #[test]
    fn each_account_has_its_last_sign_in_and_what_failed_since() {
        let accounts = [
            (
                "owner",
                Latest {
                    sign_in: Some(Place {
                        via: "console".into(),
                        ..place(1_763_110_000, "web", "192.168.1.25")
                    }),
                    failed: 0,
                    last_failed: None,
                },
            ),
            (
                "helper",
                Latest {
                    sign_in: Some(place(1_763_000_000, "tty1", "")),
                    failed: 2,
                    last_failed: Some(place(1_763_000_100, "pts/0", "10.8.0.2")),
                },
            ),
            ("new", Latest::default()),
        ];
        let text = render(&accounts).unwrap();
        let value: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(
            value,
            json!([
                {"name": "owner",
                 "sign_in": {"at_ms": 1_763_110_000_000_i64, "terminal": "web",
                             "from": "192.168.1.25", "via": "console"},
                 "failed": 0, "last_failed": null},
                {"name": "helper",
                 "sign_in": {"at_ms": 1_763_000_000_000_i64, "terminal": "tty1", "from": null,
                             "via": null},
                 "failed": 2,
                 "last_failed": {"at_ms": 1_763_000_100_000_i64, "terminal": "pts/0",
                                 "from": "10.8.0.2", "via": null}},
                {"name": "new", "sign_in": null, "failed": 0, "last_failed": null},
            ])
        );
        // The fields come in the order 11.7 gives.
        assert!(text.starts_with(r#"[{"name":"owner","sign_in":{"at_ms":"#), "{text}");
        assert_eq!(render(&[]).unwrap(), "[]");
    }
}
