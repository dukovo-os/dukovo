// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The owner's screen (ADR 0485): each change is told at once as a
//! notification, which notifyd(8) shows on the consoles, and the next
//! sign-in shows what is active as a notice with a line for each alert.
//!
//! One thread does both, so that a socket thread never waits for notifyd,
//! and tells of nothing twice: it takes what the socket threads queue.
//! notifyd refuses a program more than ten notifications in ten seconds,
//! so a burst of changes is told as one notification, and the rest are
//! paced.

use std::collections::VecDeque;
use std::sync::mpsc::{Receiver, SyncSender, sync_channel};
use std::thread;
use std::time::{Duration, Instant};

use super::text::{self, Notice};
use crate::alerts::protocol::{Active, Change};
use crate::notifyd::{self, Urgency};
use crate::sign_in_notices;

/// The name of the notice at sign-in, and the app of the notifications.
const NAME: &str = "alerts";

/// How many tellings wait for the thread; more are dropped, as the
/// sign-in notice still says what is active.
const QUEUE: usize = 256;

/// More changes than this at once are told as one notification.
const SEPARATE_MAX: usize = 3;

/// notifyd takes [`notifyd::BURST`] notifications in [`notifyd::WINDOW`];
/// this stays under it.
const BURST: usize = notifyd::BURST - 2;

/// What happened, for the owner's screen: the changes just made, and
/// every alert that is active now.
#[derive(Debug)]
pub struct Tell {
    pub changes: Vec<Change>,
    pub active: Vec<Active>,
}

/// Starts the thread that tells the screen, after it sets the sign-in
/// notice for `active`; the sender queues what to tell.
pub fn start(active: &[Active]) -> SyncSender<Tell> {
    let (tx, rx) = sync_channel(QUEUE);
    let lines = text::sign_in_lines(active);
    thread::spawn(move || {
        sign_in(&lines);
        run(&rx);
    });
    tx
}

fn run(rx: &Receiver<Tell>) {
    let mut sent: VecDeque<Instant> = VecDeque::new();
    let mut failing = false;
    for tell in rx {
        sign_in(&text::sign_in_lines(&tell.active));
        for notice in notices(&tell.changes) {
            if let Some(wait) = pause(&mut sent, Instant::now()) {
                thread::sleep(wait);
            }
            sent.push_back(Instant::now());
            let urgency = if notice.critical { Urgency::Critical } else { Urgency::Normal };
            match notifyd::send(NAME, &notice.title, &notice.body, urgency) {
                Ok(()) => failing = false,
                // Said once for as long as it keeps failing.
                Err(why) if !failing => {
                    failing = true;
                    eprintln!("<4>alertd: cannot send a notification: {}", why.message);
                }
                Err(_) => {}
            }
        }
    }
}

/// Writes the notice for the next sign-in, or removes it when no alert
/// is active.
fn sign_in(lines: &[String]) {
    let _ = sign_in_notices::put(NAME, lines);
}

/// The notifications that tell of `changes`: one each, or one for all of
/// them when there are many.
fn notices(changes: &[Change]) -> Vec<Notice> {
    if changes.len() <= SEPARATE_MAX {
        return changes.iter().map(text::notice).collect();
    }
    let each: Vec<Notice> = changes.iter().map(text::notice).collect();
    vec![Notice {
        title: format!("{} alerts changed", changes.len()),
        body: "alerts list shows what is active".into(),
        critical: each.iter().any(|notice| notice.critical),
    }]
}

/// How long to wait before the next notification at `now`, if
/// [`BURST`] went within the window; forgets those the window has passed.
fn pause(sent: &mut VecDeque<Instant>, now: Instant) -> Option<Duration> {
    while sent.front().is_some_and(|at| now.duration_since(*at) >= notifyd::WINDOW) {
        sent.pop_front();
    }
    if sent.len() < BURST {
        return None;
    }
    let oldest = *sent.front()?;
    Some((oldest + notifyd::WINDOW).saturating_duration_since(now) + Duration::from_millis(100))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::alerts::protocol::{Alert, Severity, State};

    fn change(severity: Severity, title: &str) -> Change {
        Change {
            at: 1,
            source: "metricsd".into(),
            state: State::Raised,
            alert: Alert { key: "k".into(), severity, title: title.into(), detail: String::new() },
        }
    }

    #[test]
    fn a_few_changes_are_told_one_each() {
        let changes = [change(Severity::Warning, "a"), change(Severity::Critical, "b")];
        let told = notices(&changes);
        assert_eq!(told.len(), 2);
        assert_eq!((told[0].critical, told[1].critical), (false, true));
    }

    #[test]
    fn many_changes_are_told_as_one_and_it_is_critical_if_any_is() {
        let mut changes: Vec<Change> =
            (0..=SEPARATE_MAX).map(|n| change(Severity::Warning, &format!("a{n}"))).collect();
        let told = notices(&changes);
        assert_eq!(told.len(), 1);
        assert_eq!(told[0].title, "4 alerts changed");
        assert!(!told[0].critical);
        changes[2] = change(Severity::Critical, "c");
        assert!(notices(&changes)[0].critical);
        assert!(notices(&[]).is_empty());
    }

    #[test]
    fn notifications_are_paced_to_stay_under_notifyds_limit() {
        let start = Instant::now();
        let mut sent: VecDeque<Instant> = VecDeque::new();
        for n in 0..BURST {
            assert_eq!(pause(&mut sent, start + Duration::from_millis(n as u64)), None);
            sent.push_back(start + Duration::from_millis(n as u64));
        }
        // The next waits until the first has left the window.
        let wait = pause(&mut sent, start + Duration::from_secs(1)).unwrap();
        assert!(wait > Duration::from_secs(8) && wait < Duration::from_secs(10), "{wait:?}");
        // After the window it does not.
        let later = start + notifyd::WINDOW + Duration::from_secs(1);
        assert_eq!(pause(&mut sent, later), None);
        assert!(sent.is_empty());
    }
}
