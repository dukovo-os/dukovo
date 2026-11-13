// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Delivering changes to the channels (ADR 0485): which changes go in a
//! message, when it is tried, and what is done when it fails.
//!
//! Changes for a channel are gathered for 30 seconds after the first, so
//! that a disk that fills and a backup that is late arrive as one
//! message, then sent. A message that fails stays in the outbox and is
//! tried again after 1, 5, 15 and 30 minutes, then every hour, until it
//! is 24 hours old, when it is dropped and the log says so. The outbox is
//! kept on disk, so a restart or a reboot loses no message. A message
//! keeps its id through every try, so that Matrix shows it once.
//!
//! The channels are independent: each has its own queue, in order, with
//! one message in flight at most, so a webhook that does not answer holds
//! up nothing but its own messages. The [`Outbox`] only decides; it takes
//! the time as an argument and sends nothing, and the thread that sends
//! ([`send`]) is alertd's, never a socket thread's.
//!
//! `alerts test` goes around the outbox: [`test_all`] sends one test
//! change to each channel at once and says how each went.

use std::collections::HashMap;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use super::settings::{Channel as Configured, Target};
use super::webhook::{self, Post};
use super::{email, text};
use crate::alerts::protocol::{Change, Channel as Standing, Delivery};
use crate::tls;

/// How long a message gathers changes after the first, in seconds.
pub const BATCH_WAIT: u64 = 30;

/// The most changes in one message; more start the next.
pub const BATCH_MAX: usize = 50;

/// The most messages one channel keeps waiting; past it the oldest is
/// dropped, so that a channel that is down for a day under a storm of
/// alerts cannot fill the memory.
pub const WAITING_MAX: usize = 100;

/// The wait after the first, second, third and fourth failure of a
/// message, in seconds; after that [`HOURLY`].
const RETRY: [u64; 4] = [60, 5 * 60, 15 * 60, 30 * 60];
const HOURLY: u64 = 60 * 60;

/// The age at which a message that has not gone is dropped, in seconds.
pub const GIVE_UP: u64 = 24 * 60 * 60;

/// How long `alerts test` waits for each channel.
const TEST_WAIT: Duration = Duration::from_secs(30);

/// Changes waiting to be sent to a channel as one message.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Message {
    /// The channel's name, which settings keep unique.
    pub channel: String,
    /// The same on every try of this message.
    pub id: String,
    /// When its first change came, and how many times it has failed.
    pub created: u64,
    pub tries: u32,
    /// When it is to be tried next.
    pub next: u64,
    pub changes: Vec<Change>,
    /// Whether a thread is sending it now; never kept.
    #[serde(skip)]
    sending: bool,
}

/// The messages waiting, for all the channels, oldest first, and how
/// each channel last fared.
#[derive(Debug, Default)]
pub struct Outbox {
    messages: Vec<Message>,
    last_ok: HashMap<String, u64>,
    last_error: HashMap<String, (u64, String)>,
    changed: bool,
}

/// What [`Outbox::due`] found.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Due {
    /// The messages to send now, one for each channel at most.
    pub send: Vec<Message>,
    /// Lines for the log: messages dropped for their age.
    pub dropped: Vec<String>,
}

impl Outbox {
    /// The outbox that `text`, the file's content, holds; and what is
    /// wrong with it, if anything: then it is empty.
    pub fn from_text(text: &str) -> (Outbox, Option<String>) {
        match serde_json::from_str::<Vec<Message>>(text) {
            Ok(messages) => (Outbox { messages, ..Outbox::default() }, None),
            Err(err) => (
                Outbox::default(),
                Some(format!("the outbox is not readable ({err}); its messages are lost")),
            ),
        }
    }

    /// Puts `change` in the message of each of `channels` it goes to:
    /// the one gathering, or a new one, which is due after
    /// [`BATCH_WAIT`]. Lines for the log: messages dropped for want of
    /// room.
    pub fn add(&mut self, channels: &[Configured], change: &Change, now: u64) -> Vec<String> {
        let mut dropped = Vec::new();
        for channel in channels.iter().filter(|channel| channel.filter.passes(change)) {
            match self.gathering(&channel.name, now) {
                Some(at) => self.messages[at].changes.push(change.clone()),
                None => self.messages.push(Message {
                    channel: channel.name.clone(),
                    id: webhook::new_id(now.saturating_mul(1000)),
                    created: now,
                    tries: 0,
                    next: now + BATCH_WAIT,
                    changes: vec![change.clone()],
                    sending: false,
                }),
            }
            dropped.extend(self.make_room(&channel.name));
            self.changed = true;
        }
        dropped
    }

    /// The message of `channel` still gathering changes at `now`.
    fn gathering(&self, channel: &str, now: u64) -> Option<usize> {
        self.messages.iter().position(|message| {
            message.channel == channel
                && message.tries == 0
                && !message.sending
                && message.next > now
                && message.changes.len() < BATCH_MAX
        })
    }

    /// Drops the oldest messages of `channel` that are not being sent
    /// while it has more than [`WAITING_MAX`].
    fn make_room(&mut self, channel: &str) -> Option<String> {
        let own = self.messages.iter().filter(|message| message.channel == channel).count();
        if own <= WAITING_MAX {
            return None;
        }
        let at = self.messages.iter().position(|m| m.channel == channel && !m.sending)?;
        let gone = self.messages.remove(at);
        Some(format!(
            "{channel}: more than {WAITING_MAX} messages are waiting; dropped the oldest, of {} \
             changes",
            gone.changes.len()
        ))
    }

    /// The messages to send at `now`: the oldest of each channel that is
    /// not being sent and is due, which are marked as being sent; and the
    /// messages too old to keep, which are dropped.
    pub fn due(&mut self, now: u64) -> Due {
        let mut due = Due::default();
        let mut old = Vec::new();
        for (at, message) in self.messages.iter().enumerate() {
            if !message.sending && now.saturating_sub(message.created) >= GIVE_UP {
                old.push(at);
            }
        }
        for at in old.into_iter().rev() {
            let gone = self.messages.remove(at);
            due.dropped.push(dropped_for_age(&gone, None));
            self.changed = true;
        }
        let mut seen: Vec<String> = Vec::new();
        for message in &mut self.messages {
            // Each channel's queue is in order: only its oldest message
            // goes, and when it is not due, nothing behind it does.
            if seen.contains(&message.channel) {
                continue;
            }
            seen.push(message.channel.clone());
            if !message.sending && message.next <= now {
                message.sending = true;
                due.send.push(message.clone());
            }
        }
        due
    }

    /// Takes the outcome of sending message `id`: it is gone when it was
    /// sent, tried again later when it was not, and dropped when it is
    /// too old. Lines for the log.
    pub fn finish(&mut self, id: &str, result: Result<(), String>, now: u64) -> Vec<String> {
        let Some(at) = self.messages.iter().position(|message| message.id == id) else {
            // Its channel was taken out of the settings meanwhile.
            return Vec::new();
        };
        self.changed = true;
        let channel = self.messages[at].channel.clone();
        let why = match result {
            Ok(()) => {
                self.messages.remove(at);
                self.last_ok.insert(channel, now);
                return Vec::new();
            }
            Err(why) => why,
        };
        self.last_error.insert(channel.clone(), (now, why.clone()));
        let message = &mut self.messages[at];
        message.sending = false;
        message.tries += 1;
        if now.saturating_sub(message.created) >= GIVE_UP {
            let gone = self.messages.remove(at);
            return vec![dropped_for_age(&gone, Some(&why))];
        }
        let wait = retry_after(message.tries);
        message.next = now + wait;
        vec![format!("{channel}: {why}; trying again in {}", minutes(wait))]
    }

    /// Drops the messages of channels not among `names`: those that were
    /// taken out of the settings. Lines for the log.
    pub fn keep_only(&mut self, names: &[String]) -> Vec<String> {
        let mut lines = Vec::new();
        let mut gone: HashMap<String, usize> = HashMap::new();
        self.messages.retain(|message| {
            let kept = names.contains(&message.channel);
            if !kept {
                *gone.entry(message.channel.clone()).or_default() += 1;
            }
            kept
        });
        self.last_ok.retain(|channel, _| names.contains(channel));
        self.last_error.retain(|channel, _| names.contains(channel));
        let mut gone: Vec<_> = gone.into_iter().collect();
        gone.sort();
        for (channel, count) in gone {
            lines.push(format!(
                "{channel}: is not in alerts.toml any more; dropped its {count} waiting messages"
            ));
            self.changed = true;
        }
        lines
    }

    /// Where each of `channels` stands, for `alerts status`.
    pub fn status(&self, channels: &[Configured]) -> Vec<Standing> {
        channels
            .iter()
            .map(|channel| Standing {
                channel: channel.name.clone(),
                last_ok: self.last_ok.get(&channel.name).copied(),
                last_error: self.last_error.get(&channel.name).cloned(),
                waiting: self.messages.iter().filter(|m| m.channel == channel.name).count(),
            })
            .collect()
    }

    /// The file's text, when the outbox changed since the last call.
    pub fn take_text(&mut self) -> Option<String> {
        std::mem::take(&mut self.changed).then(|| {
            // Plain data that serde_json can always write.
            serde_json::to_string(&self.messages).unwrap_or_else(|_| "[]".into())
        })
    }

    /// The messages waiting, for all the channels.
    #[cfg(test)]
    fn waiting(&self) -> usize {
        self.messages.len()
    }
}

/// The seconds to wait after a message failed for the `tries`th time.
fn retry_after(tries: u32) -> u64 {
    let at = usize::try_from(tries).unwrap_or(usize::MAX).saturating_sub(1);
    RETRY.get(at).copied().unwrap_or(HOURLY)
}

/// `seconds` as "1 minute", "5 minutes", "1 hour".
fn minutes(seconds: u64) -> String {
    let (count, unit) =
        if seconds >= HOURLY { (seconds / HOURLY, "hour") } else { (seconds / 60, "minute") };
    format!("{count} {unit}{}", if count == 1 { "" } else { "s" })
}

/// The log line for a message that is dropped for its age: its channel
/// and how many changes it held, and the last error when there was one.
fn dropped_for_age(message: &Message, last_error: Option<&str>) -> String {
    let count = message.changes.len();
    let why = last_error.map_or(String::new(), |why| format!(" (last error: {why})"));
    format!(
        "{}: dropped a message of {count} change{} that could not be sent in 24 hours{why}",
        message.channel,
        if count == 1 { "" } else { "s" }
    )
}

/// Sends `changes` to `channel` as the message `id`, telling it is from
/// machine `host`.
///
/// # Errors
///
/// Why it was not delivered, in the words of the server that refused it
/// where there was one.
pub fn send(channel: &Configured, id: &str, changes: &[Change], host: &str) -> Result<(), String> {
    let words = text::render(changes, host);
    match &channel.target {
        Target::Email(mail) => {
            let extra: Vec<String> =
                mail.ca.iter().filter_map(|path| path.to_str().map(str::to_string)).collect();
            let server = email::Server {
                host: mail.server.clone(),
                port: mail.port,
                user: mail.user.clone(),
                password: mail.password.clone(),
                roots: std::sync::Arc::new(tls::roots(&extra, "alertd")),
            };
            let message = email::Message {
                from: mail.from.clone(),
                to: mail.to.clone(),
                subject: words.subject.clone(),
                body: text::email_body(&words, host),
            };
            email::send(&server, &message, host)
        }
        Target::Webhook(hook) => {
            let json = text::json(changes, host);
            let post = Post {
                id,
                subject: &words.subject,
                body: &words.body,
                tone: words.tone,
                json: &json,
            };
            webhook::send(hook, &post)
        }
    }
}

/// Sends `change`, a test, to each of `channels` at once, without the
/// outbox, and says how each went. A channel that has not answered after
/// 30 seconds is said to have not.
pub fn test_all(channels: &[Configured], change: &Change, host: &str) -> Vec<Delivery> {
    let (tx, rx) = mpsc::channel();
    for (at, channel) in channels.iter().enumerate() {
        let (tx, channel, change, host) =
            (tx.clone(), channel.clone(), change.clone(), host.to_string());
        let id = webhook::new_id(change.at.saturating_mul(1000));
        // A thread of its own, so that a server that hangs holds up only
        // its own line; it ends when the server gives up.
        thread::spawn(move || {
            let result = send(&channel, &id, &[change], &host);
            let _ = tx.send((at, result));
        });
    }
    drop(tx);
    let deadline = Instant::now() + TEST_WAIT;
    let mut answers: Vec<Option<Result<(), String>>> = vec![None; channels.len()];
    while answers.iter().any(Option::is_none) {
        let Some(left) = deadline.checked_duration_since(Instant::now()) else { break };
        match rx.recv_timeout(left) {
            Ok((at, result)) => answers[at] = Some(result),
            Err(_) => break,
        }
    }
    let silent = format!("no answer within {} seconds", TEST_WAIT.as_secs());
    channels
        .iter()
        .zip(answers)
        .map(|(channel, answer)| {
            let (ok, error) = match answer {
                Some(Ok(())) => (true, None),
                Some(Err(why)) => (false, Some(why)),
                None => (false, Some(silent.clone())),
            };
            Delivery { channel: channel.name.clone(), ok, error }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::alerts::protocol::{Alert, Severity, State};

    use super::super::settings::{Filter, Kind, Webhook};
    use super::super::webhook::tests as hooks;

    fn hook(name: &str, severity: Severity, cleared: bool) -> Configured {
        Configured {
            name: name.into(),
            filter: Filter { severity, cleared },
            target: Target::Webhook(Webhook {
                kind: Kind::Json,
                url: "http://127.0.0.1:1/".into(),
                token: None,
                room: None,
            }),
        }
    }

    fn open(name: &str) -> Configured {
        hook(name, Severity::Warning, true)
    }

    fn change(state: State, severity: Severity, key: &str, at: u64) -> Change {
        Change {
            at,
            source: "metricsd".into(),
            state,
            alert: Alert {
                key: key.into(),
                severity,
                title: format!("{key} title"),
                detail: String::new(),
            },
        }
    }

    fn raised(key: &str, at: u64) -> Change {
        change(State::Raised, Severity::Warning, key, at)
    }

    /// The only message `due` gives.
    fn only(due: Due) -> Message {
        assert_eq!(due.send.len(), 1, "{due:?}");
        due.send.into_iter().next().unwrap()
    }

    #[test]
    fn a_message_waits_30_seconds_after_the_first_change() {
        let mut outbox = Outbox::default();
        outbox.add(&[open("a")], &raised("k", 100), 100);
        assert_eq!(outbox.due(100).send, []);
        assert_eq!(outbox.due(129).send, []);
        let message = only(outbox.due(130));
        assert_eq!((message.channel.as_str(), message.changes.len()), ("a", 1));
    }

    #[test]
    fn changes_inside_the_30_seconds_go_in_the_same_message() {
        let mut outbox = Outbox::default();
        let channels = [open("a")];
        outbox.add(&channels, &raised("one", 100), 100);
        outbox.add(&channels, &raised("two", 115), 115);
        outbox.add(&channels, &raised("three", 129), 129);
        // The window began with the first change and does not move.
        let message = only(outbox.due(130));
        let keys: Vec<&str> = message.changes.iter().map(|c| c.alert.key.as_str()).collect();
        assert_eq!(keys, ["one", "two", "three"]);
    }

    #[test]
    fn a_change_after_the_message_went_starts_the_next_and_waits_for_the_first() {
        let mut outbox = Outbox::default();
        let channels = [open("a")];
        outbox.add(&channels, &raised("one", 100), 100);
        let first = only(outbox.due(130));
        outbox.add(&channels, &raised("two", 131), 131);
        // The first is still being sent: the channel has one at a time.
        assert_eq!(outbox.due(170).send, []);
        outbox.finish(&first.id, Ok(()), 171);
        let second = only(outbox.due(171));
        assert_eq!(second.changes[0].alert.key, "two");
        assert_ne!(first.id, second.id);
    }

    #[test]
    fn a_message_holds_at_most_fifty_changes() {
        let mut outbox = Outbox::default();
        let channels = [open("a")];
        for n in 0..=BATCH_MAX {
            outbox.add(&channels, &raised(&format!("k{n}"), 100), 100);
        }
        assert_eq!(outbox.waiting(), 2);
        let first = only(outbox.due(130));
        assert_eq!(first.changes.len(), BATCH_MAX);
    }

    #[test]
    fn each_channel_gets_the_changes_its_filter_passes() {
        let mut outbox = Outbox::default();
        let channels = [
            hook("all", Severity::Warning, true),
            hook("critical", Severity::Critical, true),
            hook("no-clears", Severity::Warning, false),
        ];
        outbox.add(&channels, &raised("w", 100), 100);
        outbox.add(&channels, &change(State::Raised, Severity::Critical, "c", 100), 100);
        outbox.add(&channels, &change(State::Cleared, Severity::Warning, "w", 101), 101);
        let mut counts: Vec<(String, usize)> = outbox
            .due(130)
            .send
            .into_iter()
            .map(|message| (message.channel, message.changes.len()))
            .collect();
        counts.sort();
        assert_eq!(
            counts,
            [("all".to_string(), 3), ("critical".to_string(), 1), ("no-clears".to_string(), 2)]
        );
    }

    #[test]
    fn a_failed_message_is_tried_again_after_1_5_15_and_30_minutes_and_then_hourly() {
        let mut outbox = Outbox::default();
        outbox.add(&[open("a")], &raised("k", 0), 0);
        let mut now = 30;
        let mut waits = Vec::new();
        for _ in 0..7 {
            let message = only(outbox.due(now));
            let lines = outbox.finish(&message.id, Err("down".into()), now);
            assert_eq!(lines.len(), 1);
            let next = outbox.messages[0].next;
            assert_eq!(outbox.due(next - 1).send, [], "not before it is time");
            waits.push(next - now);
            now = next;
        }
        assert_eq!(waits, [60, 300, 900, 1800, 3600, 3600, 3600]);
    }

    #[test]
    fn the_log_says_why_and_when_the_next_try_is() {
        let mut outbox = Outbox::default();
        outbox.add(&[open("ntfy (ntfy.sh)")], &raised("k", 0), 0);
        let message = only(outbox.due(30));
        let lines = outbox.finish(&message.id, Err("ntfy answered 403 Forbidden".into()), 30);
        assert_eq!(
            lines,
            ["ntfy (ntfy.sh): ntfy answered 403 Forbidden; trying again in 1 minute"]
        );
        let message = only(outbox.due(90));
        let lines = outbox.finish(&message.id, Err("down".into()), 90);
        assert_eq!(lines, ["ntfy (ntfy.sh): down; trying again in 5 minutes"]);
        assert_eq!(minutes(3600), "1 hour");
    }

    #[test]
    fn a_message_whose_last_try_ends_after_24_hours_is_dropped_and_the_log_names_channel_and_count()
    {
        let mut outbox = Outbox::default();
        let channels = [open("email to ops@example.com")];
        outbox.add(&channels, &raised("one", 0), 0);
        outbox.add(&channels, &raised("two", 5), 5);
        let message = only(outbox.due(30));
        let lines = outbox.finish(&message.id, Err("refused".into()), GIVE_UP + 10);
        assert_eq!(
            lines,
            ["email to ops@example.com: dropped a message of 2 changes that could not be sent \
                 in 24 hours (last error: refused)"]
        );
        assert_eq!(outbox.waiting(), 0);
    }

    #[test]
    fn a_message_that_is_24_hours_old_when_its_next_try_comes_is_dropped_instead() {
        let mut outbox = Outbox::default();
        outbox.add(&[open("a")], &raised("one", 0), 0);
        let message = only(outbox.due(30));
        outbox.finish(&message.id, Err("refused".into()), GIVE_UP - 1);
        let next = outbox.messages[0].next;
        assert_eq!(outbox.due(GIVE_UP - 2).send, []);
        let due = outbox.due(next);
        assert_eq!(due.send, []);
        assert_eq!(
            due.dropped,
            ["a: dropped a message of 1 change that could not be sent in 24 hours"]
        );
        assert_eq!(outbox.waiting(), 0);
    }

    #[test]
    fn a_message_found_too_old_without_a_try_is_dropped() {
        let (mut outbox, problem) = Outbox::from_text(
            &serde_json::to_string(&[Message {
                channel: "a".into(),
                id: "1".into(),
                created: 100,
                tries: 0,
                next: 130,
                changes: vec![raised("k", 100)],
                sending: false,
            }])
            .unwrap(),
        );
        assert_eq!(problem, None);
        let due = outbox.due(100 + GIVE_UP);
        assert_eq!(due.send, []);
        assert_eq!(
            due.dropped,
            ["a: dropped a message of 1 change that could not be sent in 24 hours"]
        );
        assert_eq!(outbox.waiting(), 0);
    }

    #[test]
    fn a_dead_channel_does_not_hold_up_another() {
        let mut outbox = Outbox::default();
        let channels = [open("dead"), open("mail")];
        outbox.add(&channels, &raised("k", 0), 0);
        let both = outbox.due(30);
        assert_eq!(both.send.len(), 2);
        let dead = both.send.iter().find(|m| m.channel == "dead").unwrap().clone();
        let mail = both.send.iter().find(|m| m.channel == "mail").unwrap().clone();
        // The dead one is still being sent when the next change comes;
        // mail's message went and its next goes after 30 seconds.
        outbox.finish(&mail.id, Ok(()), 31);
        outbox.add(&channels, &raised("k2", 40), 40);
        let later = outbox.due(70);
        assert_eq!(only(later).channel, "mail");
        outbox.finish(&dead.id, Err("no answer".into()), 71);
        assert_eq!(outbox.due(72).send, []);
    }

    #[test]
    fn a_newer_message_waits_behind_an_older_one_that_is_to_be_tried_again() {
        let mut outbox = Outbox::default();
        let channels = [open("a")];
        outbox.add(&channels, &raised("one", 0), 0);
        let first = only(outbox.due(30));
        outbox.finish(&first.id, Err("down".into()), 30);
        outbox.add(&channels, &raised("two", 40), 40);
        // The second is due at 70 but the first is tried again at 90.
        assert_eq!(outbox.due(80).send, []);
        let again = only(outbox.due(90));
        assert_eq!((again.id.as_str(), again.tries), (first.id.as_str(), 1));
        outbox.finish(&again.id, Ok(()), 91);
        assert_eq!(only(outbox.due(91)).changes[0].alert.key, "two");
    }

    #[test]
    fn a_sent_message_is_gone_and_the_status_says_when_and_how_many_wait() {
        let mut outbox = Outbox::default();
        let channels = [open("a"), open("b")];
        outbox.add(&channels, &raised("k", 0), 0);
        let due = outbox.due(30);
        let a = due.send.iter().find(|m| m.channel == "a").unwrap().id.clone();
        let b = due.send.iter().find(|m| m.channel == "b").unwrap().id.clone();
        outbox.finish(&a, Ok(()), 33);
        outbox.finish(&b, Err("refused".into()), 34);
        let status = outbox.status(&channels);
        assert_eq!(
            status,
            [
                Standing { channel: "a".into(), last_ok: Some(33), last_error: None, waiting: 0 },
                Standing {
                    channel: "b".into(),
                    last_ok: None,
                    last_error: Some((34, "refused".into())),
                    waiting: 1
                },
            ]
        );
    }

    #[test]
    fn the_outbox_is_kept_and_a_message_being_sent_at_a_stop_is_sent_again() {
        let mut outbox = Outbox::default();
        outbox.add(&[open("a")], &raised("k", 0), 0);
        let message = only(outbox.due(30));
        let text = outbox.take_text().unwrap();
        assert_eq!(outbox.take_text(), None, "only when it changed");
        let (mut loaded, problem) = Outbox::from_text(&text);
        assert_eq!(problem, None);
        let again = only(loaded.due(31));
        assert_eq!(again.id, message.id);
        assert_eq!(again.changes, message.changes);
    }

    #[test]
    fn an_outbox_that_cannot_be_read_is_empty_and_said() {
        let (outbox, problem) = Outbox::from_text("not json");
        assert_eq!(outbox.waiting(), 0);
        assert!(problem.unwrap().starts_with("the outbox is not readable"));
    }

    #[test]
    fn the_messages_of_a_channel_taken_out_of_the_settings_are_dropped() {
        let mut outbox = Outbox::default();
        let channels = [open("a"), open("b")];
        outbox.add(&channels, &raised("k", 0), 0);
        outbox.add(&channels, &raised("j", 1), 1);
        let lines = outbox.keep_only(&["a".to_string()]);
        assert_eq!(lines, ["b: is not in alerts.toml any more; dropped its 1 waiting messages"]);
        assert_eq!(outbox.waiting(), 1);
        assert_eq!(outbox.due(30).send.len(), 1);
    }

    #[test]
    fn a_channel_that_is_down_keeps_at_most_a_hundred_messages() {
        let mut outbox = Outbox::default();
        let channels = [open("a")];
        let mut lines = Vec::new();
        for n in 0..=WAITING_MAX {
            // Each a minute apart, so that each is a message of its own.
            lines.extend(outbox.add(&channels, &raised(&format!("k{n}"), 0), n as u64 * 60));
        }
        assert_eq!(outbox.waiting(), WAITING_MAX);
        assert_eq!(lines.len(), 1);
        assert!(lines[0].starts_with("a: more than 100 messages are waiting"), "{lines:?}");
        assert_eq!(outbox.messages[0].changes[0].alert.key, "k1");
    }

    #[test]
    fn a_change_that_no_filter_passes_makes_no_message() {
        let mut outbox = Outbox::default();
        outbox.add(&[hook("critical", Severity::Critical, true)], &raised("w", 0), 0);
        assert_eq!(outbox.waiting(), 0);
        assert_eq!(outbox.take_text(), None);
    }

    fn json_channel(name: &str, port: u16) -> Configured {
        let mut channel = open(name);
        channel.target = Target::Webhook(Webhook {
            kind: Kind::Json,
            url: format!("http://127.0.0.1:{port}/alerts"),
            token: None,
            room: None,
        });
        channel
    }

    #[test]
    fn a_webhook_channel_is_sent_the_batch_as_json_with_the_hosts_name() {
        let (port, server) = hooks::server("200 OK", "");
        let changes = [raised("k", 1_763_030_531), raised("j", 1_763_030_532)];
        send(&json_channel("json", port), "id-1", &changes, "server").unwrap();
        let got = server.join().unwrap();
        let sent: serde_json::Value = serde_json::from_str(&got.body).unwrap();
        assert_eq!(sent["host"], "server");
        assert_eq!(sent["changes"].as_array().unwrap().len(), 2);
        assert_eq!(sent["changes"][1]["key"], "j");
    }

    #[test]
    fn a_test_goes_to_every_channel_at_once_and_each_answer_is_its_own() {
        let (good, good_server) = hooks::server("200 OK", "");
        let (bad, bad_server) = hooks::server("403 Forbidden", "no");
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let dead = listener.local_addr().unwrap().port();
        drop(listener);
        let channels =
            [json_channel("good", good), json_channel("bad", bad), json_channel("dead", dead)];
        let test = change(State::Test, Severity::Warning, "test", 1_763_030_531);
        let answers = test_all(&channels, &test, "server");
        good_server.join().unwrap();
        bad_server.join().unwrap();
        assert_eq!(
            answers,
            [
                Delivery { channel: "good".into(), ok: true, error: None },
                Delivery {
                    channel: "bad".into(),
                    ok: false,
                    error: Some("json answered 403 Forbidden: no".into())
                },
                Delivery {
                    channel: "dead".into(),
                    ok: false,
                    error: Some("cannot reach 127.0.0.1: Connection refused".into())
                },
            ]
        );
    }

    #[test]
    fn a_test_with_no_channel_answers_nothing() {
        let test = change(State::Test, Severity::Warning, "test", 1);
        assert_eq!(test_all(&[], &test, "server"), []);
    }
}
