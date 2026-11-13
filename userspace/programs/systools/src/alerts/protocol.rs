// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! What goes over alertd's socket (ADR 0485): a request, one line of
//! JSON, then the answer, one line of JSON for each item, after which
//! alertd closes the connection.
//!
//! | ask       | with      | answer                                          |
//! |-----------|-----------|-------------------------------------------------|
//! | `raise`   | `alert`   | [`Done`]                                        |
//! | `event`   | `alert`   | [`Done`]: told once, kept in the history, never active |
//! | `clear`   | `key`     | [`Done`], also when it was not active           |
//! | `set`     | `alerts`  | [`Done`]: the sender's active alerts not listed are cleared |
//! | `list`    |           | an [`Active`] a line, then [`End`]              |
//! | `history` | `count`   | a [`Change`] a line, oldest first, then [`End`] |
//! | `test`    |           | a [`Delivery`] a line, one per channel, then [`End`] |
//! | `status`  |           | a [`Channel`] a line, then [`End`]              |
//!
//! A request alertd does not take is answered with a [`Refusal`]. Only
//! root's processes may ask, and each alert belongs to the resource
//! domain of the process that raised it, its source: a service's is the
//! service's name. A sender clears and sets only its own.

use serde::{Deserialize, Serialize};

/// The socket alertd answers on.
pub const SOCKET: &str = "/system/live/alerts/socket";

/// The longest a key may be, and a title; a title is one line.
pub const KEY_MAX: usize = 128;
pub const TITLE_MAX: usize = 120;
/// The longest a detail may be, line breaks and all.
pub const DETAIL_MAX: usize = 2000;
/// The longest a request line may be.
pub const REQUEST_MAX: usize = 16 * 1024;

/// How bad it is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    /// Something to see to soon, before it costs data: a disk filling,
    /// a backup late.
    Warning,
    /// Something costing data or about to: a disk failing, a RAID
    /// volume without its second copy.
    Critical,
}

impl Severity {
    /// The word for it.
    pub fn name(self) -> &'static str {
        match self {
            Severity::Warning => "warning",
            Severity::Critical => "critical",
        }
    }
}

/// An alert as its source raises it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Alert {
    /// The source's own name for it, of `a-z`, `0-9` and `._:/-`, up to
    /// [`KEY_MAX`] long: `fs-full:/data`, `smart:wd-wcc4e1234567`.
    pub key: String,
    pub severity: Severity,
    /// What is wrong, in one line of up to [`TITLE_MAX`] characters:
    /// "/data is 93% full (1.2 TB of 1.3 TB)".
    pub title: String,
    /// What to do about it, up to [`DETAIL_MAX`] characters.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub detail: String,
}

impl Alert {
    /// An alert of `key` with `title` cut to one line of [`TITLE_MAX`]
    /// characters and `detail` to [`DETAIL_MAX`], so that nothing a
    /// producer reads from a disk or a log makes alertd refuse it. The
    /// key is the caller's to make right ([`key_part`]).
    pub fn new(key: String, severity: Severity, title: &str, detail: &str) -> Alert {
        let line = title.lines().next().unwrap_or_default().trim();
        Alert {
            key,
            severity,
            title: clip(line, TITLE_MAX),
            detail: clip(detail.trim_end(), DETAIL_MAX),
        }
    }

    /// What is wrong with it for alertd to refuse it, if anything.
    pub fn invalid(&self) -> Option<String> {
        let key_char =
            |c: char| c.is_ascii_lowercase() || c.is_ascii_digit() || "._:/-".contains(c);
        if self.key.is_empty() || self.key.len() > KEY_MAX || !self.key.chars().all(key_char) {
            return Some(format!(
                "the key {:?} is not 1 to {KEY_MAX} of a-z, 0-9 and ._:/-",
                self.key
            ));
        }
        if self.title.trim().is_empty() {
            return Some(format!("the alert {} has no title", self.key));
        }
        if self.title.chars().count() > TITLE_MAX || self.title.contains(['\n', '\r']) {
            return Some(format!(
                "the alert {}'s title is not one line of up to {TITLE_MAX} characters",
                self.key
            ));
        }
        if self.detail.chars().count() > DETAIL_MAX {
            return Some(format!(
                "the alert {}'s detail is longer than {DETAIL_MAX} characters",
                self.key
            ));
        }
        None
    }
}

/// `text`, cut to `most` characters, the last of them `…` when cut.
fn clip(text: &str, most: usize) -> String {
    if text.chars().count() <= most {
        return text.to_string();
    }
    let mut cut: String = text.chars().take(most - 1).collect();
    cut.push('…');
    cut
}

/// `text` as part of a key: lower case, with each character a key may
/// not have, a space or a `+`, made `-`. A disk's serial number or a
/// destination's name becomes part of a key this way.
pub fn key_part(text: &str) -> String {
    text.trim()
        .chars()
        .map(|c| match c.to_ascii_lowercase() {
            c @ ('a'..='z' | '0'..='9' | '.' | '_' | ':' | '/' | '-') => c,
            _ => '-',
        })
        .collect()
}

/// A request to alertd: a line of JSON.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Request {
    /// `raise`, `event`, `clear`, `set`, `list`, `history`, `test` or
    /// `status`.
    pub ask: String,
    /// raise and event: the alert.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub alert: Option<Alert>,
    /// clear: the key of the sender's alert to clear.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    /// set: every alert the sender has now; [`None`] for an ask that is
    /// not a set, so that an empty list clears them all.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub alerts: Option<Vec<Alert>>,
    /// history: the newest this many changes; 20 if not given.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub count: Option<usize>,
}

/// The answer to a raise, event, clear or set that alertd took.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Done {
    pub ok: bool,
}

/// The line after the last item of a list, history, test or status.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct End {
    pub end: bool,
}

/// Why alertd did not take a request.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Refusal {
    pub error: String,
}

/// An alert that is active: raised and not yet cleared.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Active {
    /// The resource domain of the process that raised it: a service's
    /// name.
    pub source: String,
    pub alert: Alert,
    /// When it was raised, and when its title, detail or severity last
    /// changed, in seconds since 1970.
    pub since: u64,
    pub changed: u64,
}

/// What a change did.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum State {
    /// Raised, or raised again from warning to critical.
    Raised,
    Cleared,
    /// Told once and never active: a fallback to the previous version.
    Event,
    /// `alerts test`'s.
    Test,
}

/// A change, as the history keeps it and the channels are told it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Change {
    /// When, in seconds since 1970.
    pub at: u64,
    pub source: String,
    pub state: State,
    pub alert: Alert,
}

/// What a channel did with `alerts test`'s alert.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Delivery {
    /// The channel, as `alerts status` names it: "email to
    /// ops@example.com", "ntfy (ntfy.sh)".
    pub channel: String,
    pub ok: bool,
    /// Why it was not sent, in the words of the server that refused it
    /// where there was one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Where a channel stands.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Channel {
    pub channel: String,
    /// When it last took a message, in seconds since 1970.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_ok: Option<u64>,
    /// When it last failed to, and why.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error: Option<(u64, String)>,
    /// The messages waiting to be tried again.
    pub waiting: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn alert(key: &str, title: &str) -> Alert {
        Alert {
            key: key.into(),
            severity: Severity::Warning,
            title: title.into(),
            detail: String::new(),
        }
    }

    #[test]
    fn keys_titles_and_details_are_checked() {
        assert_eq!(alert("fs-full:/data", "/data is 93% full").invalid(), None);
        assert!(alert("smart:WD-1", "x").invalid().is_some(), "upper case");
        assert!(alert("", "x").invalid().is_some());
        assert!(alert(&"k".repeat(KEY_MAX + 1), "x").invalid().is_some());
        assert!(alert("k", " ").invalid().is_some());
        assert!(alert("k", "two\nlines").invalid().is_some());
        assert!(alert("k", &"é".repeat(TITLE_MAX)).invalid().is_none(), "characters, not bytes");
        assert!(alert("k", &"t".repeat(TITLE_MAX + 1)).invalid().is_some());
        let mut long = alert("k", "x");
        long.detail = "d".repeat(DETAIL_MAX + 1);
        assert!(long.invalid().is_some());
    }

    #[test]
    fn requests_leave_out_what_they_do_not_carry() {
        let clear = Request { ask: "clear".into(), key: Some("k".into()), ..Request::default() };
        assert_eq!(serde_json::to_string(&clear).unwrap(), r#"{"ask":"clear","key":"k"}"#);
        let set = Request { ask: "set".into(), alerts: Some(Vec::new()), ..Request::default() };
        assert_eq!(serde_json::to_string(&set).unwrap(), r#"{"ask":"set","alerts":[]}"#);
        let raise: Request = serde_json::from_str(
            r#"{"ask":"raise","alert":{"key":"k","severity":"critical","title":"t"}}"#,
        )
        .unwrap();
        let alert = raise.alert.unwrap();
        assert_eq!((alert.severity, alert.detail.as_str()), (Severity::Critical, ""));
    }

    #[test]
    fn severities_order_from_warning_to_critical() {
        assert!(Severity::Warning < Severity::Critical);
        assert_eq!(serde_json::to_string(&State::Cleared).unwrap(), r#""cleared""#);
    }

    #[test]
    fn a_new_alert_is_cut_to_what_alertd_takes() {
        let long = "x".repeat(TITLE_MAX + 10);
        let made = Alert::new("k".into(), Severity::Warning, &format!("{long}\nmore"), "d\n");
        assert_eq!(made.title.chars().count(), TITLE_MAX);
        assert!(made.title.ends_with('…'));
        assert_eq!(made.detail, "d");
        assert_eq!(made.invalid(), None);
        let made = Alert::new("k".into(), Severity::Warning, "t", &"y".repeat(DETAIL_MAX * 2));
        assert_eq!(made.detail.chars().count(), DETAIL_MAX);
        assert_eq!(made.invalid(), None);
    }

    #[test]
    fn any_text_becomes_a_key_part() {
        assert_eq!(key_part("WD-WCC4E1234567"), "wd-wcc4e1234567");
        assert_eq!(key_part(" QM00001 "), "qm00001");
        assert_eq!(key_part("Off Site+2"), "off-site-2");
        let alert = Alert::new(format!("smart:{}", key_part("S/N ÄB")), Severity::Warning, "t", "");
        assert_eq!(alert.invalid(), None);
    }
}
