// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! What alertd says about changes (ADR 0485): the subject and body of a
//! message to a channel, the JSON of the json kind, the notice on the
//! screen and the lines of the next sign-in.
//!
//! One change reads `[server] CRITICAL: /data is 97% full`, several
//! `[server] 3 alerts: /data is 97% full, and 2 more`. The body has a
//! line for each change with its state, severity and title, the detail
//! below it, indented, and where and when it was raised. The text is
//! plain, so that a phone, a mail program and a chat room show it alike.
//! Times are UTC, written out, so that a message read a day later or in
//! another zone still says when.

use serde::Serialize;

use super::webhook::Tone;
use crate::alerts::protocol::{Active, Change, Severity, State};
use crate::logd;

/// The most alerts a sign-in notice names; the rest are counted.
const SIGN_IN_MAX: usize = 8;

/// A message's words, before a channel puts them in its own form.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Text {
    pub subject: String,
    pub body: String,
    pub tone: Tone,
}

/// The text for `changes`, told by the machine `host`.
pub fn render(changes: &[Change], host: &str) -> Text {
    let sections: Vec<String> = changes.iter().map(section).collect();
    Text { subject: subject(changes, host), body: sections.join("\n"), tone: tone(changes) }
}

/// The body of an email: [`Text::body`] and what to do next.
pub fn email_body(text: &Text, host: &str) -> String {
    format!("{}\nRun \"alerts list\" on {host} to see what is active.\n", text.body)
}

/// The subject: for one change by its state and severity, for several
/// the count and the first title.
fn subject(changes: &[Change], host: &str) -> String {
    match changes {
        [] => format!("[{host}] No alerts"),
        [one] => {
            let title = &one.alert.title;
            match (one.state, one.alert.severity) {
                (State::Test, _) => format!("[{host}] Test alert"),
                (State::Cleared, _) => format!("[{host}] Cleared: {title}"),
                (_, Severity::Critical) => format!("[{host}] CRITICAL: {title}"),
                (_, Severity::Warning) => format!("[{host}] Warning: {title}"),
            }
        }
        [first, rest @ ..] => {
            format!(
                "[{host}] {} alerts: {}, and {} more",
                changes.len(),
                first.alert.title,
                rest.len()
            )
        }
    }
}

/// How urgent `changes` are together: the worst of those that raise
/// something; a clearing or a test is calm.
fn tone(changes: &[Change]) -> Tone {
    let raising =
        changes.iter().filter(|change| matches!(change.state, State::Raised | State::Event));
    match raising.map(|change| change.alert.severity).max() {
        Some(Severity::Critical) => Tone::Critical,
        Some(Severity::Warning) => Tone::Warning,
        None => Tone::Calm,
    }
}

/// A change's lines: its state, severity and title, the detail indented,
/// and where it is from.
fn section(change: &Change) -> String {
    let alert = &change.alert;
    let mut text = match change.state {
        State::Test => format!("Test: {}\n", alert.title),
        state => format!("{}, {}: {}\n", state_word(state), alert.severity.name(), alert.title),
    };
    for line in alert.detail.lines() {
        if line.is_empty() {
            text.push('\n');
        } else {
            text.push_str("  ");
            text.push_str(line);
            text.push('\n');
        }
    }
    text.push_str(&format!("  From {} at {}.\n", change.source, utc(change.at)));
    text
}

/// The word for what a change did.
fn state_word(state: State) -> &'static str {
    match state {
        State::Raised => "Raised",
        State::Cleared => "Cleared",
        State::Event => "Event",
        State::Test => "Test",
    }
}

/// `seconds` since the epoch as `2026-11-13 10:02:11 UTC`.
pub fn utc(seconds: u64) -> String {
    format!("{} UTC", date_time(seconds, true))
}

/// `seconds` since the epoch as `2026-11-13 10:02`, with the seconds when
/// `seconds_too`.
pub fn date_time(seconds: u64, seconds_too: bool) -> String {
    let (year, month, day) = logd::civil(i64::try_from(seconds / 86_400).unwrap_or(0));
    let rest = seconds % 86_400;
    let (hour, minute) = (rest / 3600, rest % 3600 / 60);
    let date = format!("{year:04}-{month:02}-{day:02} {hour:02}:{minute:02}");
    if seconds_too { format!("{date}:{:02}", rest % 60) } else { date }
}

#[derive(Serialize)]
struct JsonBatch<'a> {
    version: u32,
    host: &'a str,
    changes: Vec<JsonChange<'a>>,
}

/// A change as the json kind has it; the fields in this order.
#[derive(Serialize)]
struct JsonChange<'a> {
    state: State,
    source: &'a str,
    key: &'a str,
    severity: Severity,
    title: &'a str,
    detail: &'a str,
    at: u64,
}

/// The json kind's text: `{"version":1,"host":"...","changes":[...]}`.
pub fn json(changes: &[Change], host: &str) -> String {
    let changes = changes
        .iter()
        .map(|change| JsonChange {
            state: change.state,
            source: &change.source,
            key: &change.alert.key,
            severity: change.alert.severity,
            title: &change.alert.title,
            detail: &change.alert.detail,
            at: change.at,
        })
        .collect();
    // Plain data that serde_json can always write.
    serde_json::to_string(&JsonBatch { version: 1, host, changes }).unwrap_or_default()
}

/// The notice for the screen: its title and body, and whether it is
/// critical.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Notice {
    pub title: String,
    pub body: String,
    pub critical: bool,
}

/// The notice that tells of `change`: the title, and the source and then
/// the detail as the body.
pub fn notice(change: &Change) -> Notice {
    let alert = &change.alert;
    let title = match change.state {
        State::Cleared => format!("Cleared: {}", alert.title),
        _ => alert.title.clone(),
    };
    let body = format!("{}\n{}", change.source, alert.detail);
    Notice {
        title,
        body: body.trim_end().to_string(),
        critical: change.state != State::Cleared && alert.severity == Severity::Critical,
    }
}

/// The lines the next sign-in shows, one for each active alert,
/// critical first: `Alert: <title> (since 2026-11-13 10:02, alerts
/// list)`. Past eight the rest are counted, so a sign-in does not scroll
/// away.
pub fn sign_in_lines(active: &[Active]) -> Vec<String> {
    let mut lines: Vec<String> = active
        .iter()
        .take(SIGN_IN_MAX)
        .map(|held| {
            format!(
                "Alert: {} (since {}, alerts list)",
                held.alert.title,
                date_time(held.since, false)
            )
        })
        .collect();
    if active.len() > SIGN_IN_MAX {
        lines.push(format!("Alert: {} more (alerts list)", active.len() - SIGN_IN_MAX));
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::alerts::protocol::Alert;

    fn change(state: State, severity: Severity, title: &str, detail: &str) -> Change {
        Change {
            at: 1_763_030_531,
            source: "metricsd".into(),
            state,
            alert: Alert {
                key: "fs-full:/data".into(),
                severity,
                title: title.into(),
                detail: detail.into(),
            },
        }
    }

    fn raised(title: &str) -> Change {
        change(State::Raised, Severity::Warning, title, "")
    }

    #[test]
    fn one_change_has_its_state_and_severity_in_the_subject() {
        let subject = |state, severity| {
            render(&[change(state, severity, "/data is 97% full", "")], "server").subject
        };
        assert_eq!(
            subject(State::Raised, Severity::Critical),
            "[server] CRITICAL: /data is 97% full"
        );
        assert_eq!(
            subject(State::Raised, Severity::Warning),
            "[server] Warning: /data is 97% full"
        );
        assert_eq!(
            subject(State::Event, Severity::Critical),
            "[server] CRITICAL: /data is 97% full"
        );
        assert_eq!(
            subject(State::Cleared, Severity::Critical),
            "[server] Cleared: /data is 97% full"
        );
        assert_eq!(subject(State::Test, Severity::Warning), "[server] Test alert");
    }

    #[test]
    fn several_changes_have_the_count_and_the_first_title() {
        let changes = [raised("first one"), raised("second"), raised("third")];
        assert_eq!(render(&changes, "server").subject, "[server] 3 alerts: first one, and 2 more");
        assert_eq!(
            render(&changes[..2], "server").subject,
            "[server] 2 alerts: first one, and 1 more"
        );
    }

    #[test]
    fn the_body_has_a_section_a_change_with_the_detail_indented_and_where_from() {
        let changes = [
            change(
                State::Raised,
                Severity::Critical,
                "/data is 97% full",
                "Free space.\nOr add a disk.",
            ),
            change(State::Cleared, Severity::Warning, "/tmp was full", ""),
        ];
        assert_eq!(
            render(&changes, "server").body,
            "Raised, critical: /data is 97% full\n  Free space.\n  Or add a disk.\n  From metricsd at \
             2025-11-13 10:42:11 UTC.\n\nCleared, warning: /tmp was full\n  From metricsd at \
             2025-11-13 10:42:11 UTC.\n"
        );
    }

    #[test]
    fn a_test_says_it_is_one_and_an_empty_line_in_a_detail_stays_empty() {
        let test = change(State::Test, Severity::Warning, "Test alert from alertd", "a\n\nb");
        assert_eq!(
            render(&[test], "server").body,
            "Test: Test alert from alertd\n  a\n\n  b\n  From metricsd at 2025-11-13 10:42:11 UTC.\n"
        );
    }

    #[test]
    fn an_email_closes_with_how_to_see_what_is_active() {
        let text = render(&[raised("x")], "server");
        let body = email_body(&text, "server");
        assert!(body.starts_with(&text.body));
        assert!(
            body.ends_with("\nRun \"alerts list\" on server to see what is active.\n"),
            "{body}"
        );
    }

    #[test]
    fn the_tone_is_the_worst_of_what_is_raised_and_calm_for_a_clearing() {
        let tone_of = |changes: &[Change]| render(changes, "h").tone;
        let critical = change(State::Raised, Severity::Critical, "c", "");
        let cleared = change(State::Cleared, Severity::Critical, "c", "");
        let event = change(State::Event, Severity::Warning, "e", "");
        assert_eq!(tone_of(&[raised("w"), critical.clone()]), Tone::Critical);
        assert_eq!(tone_of(&[cleared.clone(), event]), Tone::Warning);
        assert_eq!(tone_of(&[cleared]), Tone::Calm);
        assert_eq!(tone_of(&[change(State::Test, Severity::Warning, "t", "")]), Tone::Calm);
    }

    #[test]
    fn the_json_has_the_fields_in_the_order_the_documentation_shows() {
        let changes = [change(State::Raised, Severity::Warning, "/data is 93% full", "Free some.")];
        assert_eq!(
            json(&changes, "server"),
            r#"{"version":1,"host":"server","changes":[{"state":"raised","source":"metricsd","key":"fs-full:/data","severity":"warning","title":"/data is 93% full","detail":"Free some.","at":1763030531}]}"#
        );
    }

    #[test]
    fn the_json_escapes_quotes_and_keeps_an_empty_detail() {
        let changes = [change(State::Cleared, Severity::Critical, "a \"quoted\"\ttitle", "")];
        let text = json(&changes, "my \"host\"");
        let value: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(value["host"], "my \"host\"");
        assert_eq!(value["changes"][0]["title"], "a \"quoted\"\ttitle");
        assert_eq!(value["changes"][0]["detail"], "");
        assert_eq!(value["changes"][0]["state"], "cleared");
    }

    #[test]
    fn the_time_is_written_out_in_utc() {
        assert_eq!(utc(0), "1970-01-01 00:00:00 UTC");
        assert_eq!(utc(1_763_030_531), "2025-11-13 10:42:11 UTC");
        assert_eq!(utc(951_782_400), "2000-02-29 00:00:00 UTC");
    }

    #[test]
    fn a_notice_has_the_title_and_the_source_with_the_detail_and_only_critical_is_urgent() {
        let notice_of = |change| notice(&change);
        let critical =
            notice_of(change(State::Raised, Severity::Critical, "Disk failing", "Back up."));
        assert_eq!(
            critical,
            Notice {
                title: "Disk failing".into(),
                body: "metricsd\nBack up.".into(),
                critical: true
            }
        );
        let warning = notice_of(change(State::Raised, Severity::Warning, "Disk filling", ""));
        assert_eq!((warning.critical, warning.body.as_str()), (false, "metricsd"));
        let cleared = notice_of(change(State::Cleared, Severity::Critical, "Disk failing", ""));
        assert_eq!((cleared.title.as_str(), cleared.critical), ("Cleared: Disk failing", false));
    }

    fn held(title: &str, since: u64) -> Active {
        Active { source: "metricsd".into(), alert: raised(title).alert, since, changed: since }
    }

    #[test]
    fn the_sign_in_has_a_line_for_each_alert_in_the_order_given() {
        let lines = sign_in_lines(&[held("Disk failing", 1_763_030_531), held("Backup late", 0)]);
        assert_eq!(
            lines,
            [
                "Alert: Disk failing (since 2025-11-13 10:42, alerts list)",
                "Alert: Backup late (since 1970-01-01 00:00, alerts list)"
            ]
        );
        assert!(sign_in_lines(&[]).is_empty());
    }

    #[test]
    fn a_long_list_is_cut_to_eight_lines_and_counted() {
        let active: Vec<Active> = (0..20).map(|n| held(&format!("a{n}"), 60)).collect();
        let lines = sign_in_lines(&active);
        assert_eq!(lines.len(), SIGN_IN_MAX + 1);
        assert_eq!(lines.last().unwrap(), "Alert: 12 more (alerts list)");
    }
}
