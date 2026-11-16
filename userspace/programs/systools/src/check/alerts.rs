// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The alerts check of dukovo-check(8): whether the channels alerts are
//! sent to (email, webhooks, browsers) have taken a message lately. A
//! machine with no channel is not warned: that is how every machine is
//! installed, and its alerts reach its own screens. A warning is for a
//! machine that has set a channel up and where none of them has taken a
//! message for a day while each has failed in that time.

use super::context::Context;
use super::line::{Outcome, Problem};
use crate::alerts::protocol::Channel;

/// A day, in seconds.
const DAY: u64 = 86_400;

/// Whether `at`, in seconds since 1970, is within the last day.
fn lately(at: u64, now: u64) -> bool {
    now.saturating_sub(at) < DAY
}

/// Whether the channel took a message in the last day.
fn took_lately(channel: &Channel, now: u64) -> bool {
    channel.last_ok.is_some_and(|at| lately(at, now))
}

/// Whether the channel failed in the last day, and has not taken a message
/// since.
fn failed_lately(channel: &Channel, now: u64) -> bool {
    channel
        .last_error
        .as_ref()
        .is_some_and(|(at, _)| lately(*at, now) && channel.last_ok.is_none_or(|ok| *at > ok))
}

/// What a channel is called in the line: an email account as `email`,
/// since its address is no part of what to say, and the others as `alerts
/// status` names them.
fn called(channel: &Channel) -> &str {
    if channel.kind == "email" { "email" } else { &channel.channel }
}

/// `names` as a sentence's list: "a", "a and b", "a, b and c".
fn listed(names: &[&str]) -> String {
    match names {
        [] => String::new(),
        [only] => (*only).to_string(),
        [first @ .., last] => format!("{} and {last}", first.join(", ")),
    }
}

/// "1 channel", "3 channels".
fn counted(number: usize) -> String {
    format!("{number} channel{}", if number == 1 { "" } else { "s" })
}

/// Says whether alerts have reached anyone beyond the screen in the last
/// day: it passes when a channel took a message, or when no channel is set
/// up, or when what is set up had nothing to send; it warns, as SEC-170,
/// when each channel failed and none took a message.
pub(super) fn reach(context: &Context) -> Vec<Outcome> {
    let Ok(channels) = &context.channels else {
        return vec![Outcome::skipped("alertd did not answer")];
    };
    if channels.is_empty() {
        return vec![Outcome::passed("no channel is set up: alerts reach this server's screens")];
    }
    let now = context.now;
    let took: Vec<&str> =
        channels.iter().filter(|channel| took_lately(channel, now)).map(called).collect();
    if !took.is_empty() {
        let detail = format!("{} took a message in the last day", listed(&took));
        return vec![Outcome::passed(detail)];
    }
    if channels.iter().all(|channel| failed_lately(channel, now)) {
        let names: Vec<&str> = channels.iter().map(|channel| channel.channel.as_str()).collect();
        let detail = format!(
            "{} took no message in the last day and each failed: {}",
            counted(channels.len()),
            names.join(", ")
        );
        return vec![Outcome::warning(Problem::NoAlertReached, detail)];
    }
    vec![Outcome::passed(format!("{}, nothing to send in the last day", counted(channels.len())))]
}

#[cfg(test)]
mod tests {
    use super::super::context::planted::{NOW, System};
    use super::super::line::Verdict;
    use super::*;
    use crate::alerts::protocol::Severity;

    /// A channel that has taken and failed nothing.
    fn channel(name: &str, kind: &str) -> Channel {
        Channel {
            channel: name.into(),
            last_ok: None,
            last_error: None,
            waiting: 0,
            kind: kind.into(),
            host: None,
            severity: Severity::Warning,
            cleared: true,
            number: None,
            id: None,
            name: None,
            error_id: None,
            about: None,
        }
    }

    fn ok(mut channel: Channel, ago: u64) -> Channel {
        channel.last_ok = Some(NOW - ago);
        channel
    }

    fn failed(mut channel: Channel, ago: u64) -> Channel {
        channel.last_error = Some((NOW - ago, "refused".into()));
        channel
    }

    fn reached(channels: Vec<Channel>) -> Outcome {
        let mut system = System::new(Vec::new());
        system.context.channels = Ok(channels);
        let mut outcomes = reach(&system.context);
        assert_eq!(outcomes.len(), 1);
        outcomes.remove(0)
    }

    #[test]
    fn no_channel_is_not_a_warning_and_says_where_alerts_go() {
        let outcome = reached(Vec::new());
        assert_eq!(outcome.verdict, Verdict::Passed);
        assert_eq!(outcome.detail, "no channel is set up: alerts reach this server's screens");
        assert_eq!(outcome.id, None);
    }

    #[test]
    fn the_channels_that_took_a_message_in_the_last_day_are_named() {
        let both = reached(vec![
            ok(channel("email to ops@example.com", "email"), 600),
            ok(channel("ntfy (ntfy.sh)", "ntfy"), DAY - 1),
            channel("push to Phone", "push"),
        ]);
        assert_eq!(both.verdict, Verdict::Passed);
        assert_eq!(both.detail, "email and ntfy (ntfy.sh) took a message in the last day");
        let one = reached(vec![ok(channel("ntfy (ntfy.sh)", "ntfy"), 60)]);
        assert_eq!(one.detail, "ntfy (ntfy.sh) took a message in the last day");
        let three = reached(vec![
            ok(channel("email to a@b.test", "email"), 60),
            ok(channel("ntfy (ntfy.sh)", "ntfy"), 60),
            ok(channel("push to Phone", "push"), 60),
        ]);
        assert_eq!(
            three.detail,
            "email, ntfy (ntfy.sh) and push to Phone took a message in the last day"
        );
        // One that failed after it took a message has still taken one lately.
        let later = failed(ok(channel("ntfy (ntfy.sh)", "ntfy"), 3600), 60);
        assert_eq!(reached(vec![later]).verdict, Verdict::Passed);
    }

    #[test]
    fn channels_with_nothing_to_send_pass_and_say_how_many() {
        let quiet = reached(vec![channel("ntfy (ntfy.sh)", "ntfy")]);
        assert_eq!(quiet.verdict, Verdict::Passed);
        assert_eq!(quiet.detail, "1 channel, nothing to send in the last day");
        // A message taken two days ago, and a failure long past.
        let old = reached(vec![
            ok(channel("email to a@b.test", "email"), 2 * DAY),
            failed(channel("ntfy (ntfy.sh)", "ntfy"), 3 * DAY),
        ]);
        assert_eq!(old.detail, "2 channels, nothing to send in the last day");
        // One failed and the other has not: the other may have nothing to send.
        let half = reached(vec![
            failed(channel("email to a@b.test", "email"), 600),
            channel("ntfy (ntfy.sh)", "ntfy"),
        ]);
        assert_eq!((half.verdict, half.id), (Verdict::Passed, None));
        // A failure that came before the last success says it works.
        let mended = reached(vec![failed(ok(channel("ntfy (ntfy.sh)", "ntfy"), 2 * DAY), 3 * DAY)]);
        assert_eq!(mended.verdict, Verdict::Passed);
    }

    #[test]
    fn every_channel_failing_for_a_day_is_sec_170() {
        let outcome = reached(vec![
            failed(channel("email to ops@example.com", "email"), 600),
            failed(ok(channel("ntfy (ntfy.sh)", "ntfy"), 2 * DAY), DAY - 60),
            failed(ok(channel("push to Phone", "push"), 3 * DAY), 2 * DAY - 1),
        ]);
        // The last failed two days ago: it is not a failure of the last day.
        assert_eq!(outcome.verdict, Verdict::Passed);
        let outcome = reached(vec![
            failed(channel("email to ops@example.com", "email"), 600),
            failed(ok(channel("ntfy (ntfy.sh)", "ntfy"), 2 * DAY), DAY - 60),
        ]);
        assert_eq!(outcome.verdict, Verdict::Warning);
        assert_eq!(outcome.id.as_deref(), Some("SEC-170"));
        assert_eq!(
            outcome.detail,
            "2 channels took no message in the last day and each failed: email to \
             ops@example.com, ntfy (ntfy.sh)"
        );
        assert!(outcome.fix.as_deref().is_some_and(|fix| fix.contains("alerts status")));
        let one = reached(vec![failed(channel("ntfy (ntfy.sh)", "ntfy"), 5)]);
        assert_eq!(one.id.as_deref(), Some("SEC-170"));
        assert!(one.detail.starts_with("1 channel took no message in the last day and each"));
    }

    #[test]
    fn alertd_that_did_not_answer_skips_the_check() {
        let mut system = System::new(Vec::new());
        system.context.channels = Err("alertd is not running; try service alertd start".into());
        let outcomes = reach(&system.context);
        assert_eq!(outcomes.len(), 1);
        assert_eq!(outcomes[0].verdict, Verdict::Skipped);
        assert_eq!(outcomes[0].detail, "alertd did not answer");
    }

    #[test]
    fn a_day_is_counted_back_from_now() {
        assert!(lately(NOW - (DAY - 1), NOW));
        assert!(!lately(NOW - DAY, NOW));
        // A time after now, as when the clock was set back, is lately.
        assert!(lately(NOW + 10, NOW));
    }
}
