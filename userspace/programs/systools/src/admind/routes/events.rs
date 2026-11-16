// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! A tab's stream of events (ADR 0514), `GET /api/1/events`, as
//! server-sent events: `unlock` with the seconds left when the session's
//! unlock changes, `session` with why it ended when it does, `task` for
//! each change of a task's state, step or progress (ADR 0516), `change` for
//! each row of the journal that ends, a console change's or a hand edit's,
//! `trial` with the trials that run, `power` with the restart or shut down
//! that is set, and a comment every 25 seconds so that nothing between the
//! browser and admind closes a stream that says nothing.
//!
//! A tab that opens is told where each task is that waits, runs or ended
//! in the last minute, so that it has no gap between what it asked for and
//! what came after. Later a task is told at most four times a second: what
//! comes sooner waits, and only the latest is sent. A row is told once to
//! every open tab of every session, as the journal is everyone's to read,
//! and says its number, how it was made and when, with no file's name and
//! no person's; a tab that opens is told of the rows after it opened.
//!
//! The trials that run are told to every open tab of every session, as a
//! trial is the machine's and names no one: to a tab that opens while one
//! runs, whenever [`Shared::tell_trials`] has been called since the stream
//! last told them (a change that begins or ends a trial, and the round of
//! admind's upkeep, which tells them every time while one runs and once
//! more, with none, when the last has ended). Each tells the time left of
//! each trial as of when it is sent, so that a tab corrects its countdown.
//!
//! The restart or shut down that is set is told the same way (ADR 0565): to
//! a tab that opens while one is set, and to every tab each time what is
//! set changes, which admind sees when it reads Spark (a `power` change
//! reads it as soon as it answers, and the upkeep every round). Its
//! seconds left are as of when it is sent.
//!
//! An open stream is a tab: while a session has one, it is not idle, and
//! once its last stream has closed and 60 seconds have passed, an
//! unlocked session locks itself, as the design says closing the console
//! does. The stream is a task that sends what it finds into a channel its
//! answer's body reads; when the browser closes the connection the body
//! goes, the channel closes, and the task ends and counts the tab out.

use std::convert::Infallible;
use std::pin::{Pin, pin};
use std::rc::Rc;
use std::task::{self, Poll};
use std::time::{Duration, Instant};

use accounts::console::Console;
use bytes::Bytes;
use http::StatusCode;
use http_body::Frame;
use serde_json::json;
use tokio::sync::{mpsc, watch};

use super::{Content, Live, Outcome, Reply};
use crate::admind::authd;
use crate::admind::journal::Row;
use crate::admind::log;
use crate::admind::shared::{Shared, seconds_left, unix_now};
use crate::admind::tasks::Told;
use crate::admind::trials::Running;
use crate::power::Pending;

/// How long a stream may say nothing.
const KEEP_TIME: Duration = Duration::from_secs(25);

/// How long a tab may leave what it was sent unread before its stream is
/// closed, which is far longer than a browser that reads takes.
const SEND_TIME: Duration = Duration::from_secs(30);

/// How long a session stays unlocked once its last tab has closed.
const LOCK_AFTER: Duration = Duration::from_secs(60);

/// The body of the answer: what the stream's task sends.
pub struct Events {
    receiver: mpsc::Receiver<Bytes>,
}

impl http_body::Body for Events {
    type Data = Bytes;
    type Error = Infallible;

    fn poll_frame(
        mut self: Pin<&mut Self>,
        cx: &mut task::Context<'_>,
    ) -> Poll<Option<Result<Frame<Bytes>, Infallible>>> {
        self.receiver.poll_recv(cx).map(|sent| sent.map(|bytes| Ok(Frame::data(bytes))))
    }
}

/// The event that says the session is unlocked until the Unix time
/// `until`, or locked, at the Unix time `now`.
fn unlock_event(until: Option<i64>, now: i64) -> Bytes {
    let data = json!({ "unlock_seconds": seconds_left(until, now) });
    Bytes::from(format!("event: unlock\ndata: {data}\n\n"))
}

/// The event that says the session ended, and why.
fn session_event(reason: &str) -> Bytes {
    let data = json!({ "ended": reason });
    Bytes::from(format!("event: session\ndata: {data}\n\n"))
}

/// The event that says the journal has the row `row`, made as it says and
/// ended at its time.
fn change_event(row: &Row) -> Bytes {
    let data = json!({ "id": row.id, "how": row.how, "at": row.at });
    Bytes::from(format!("event: change\ndata: {data}\n\n"))
}

/// The event that tells the trials that run, `[]` when none does.
fn trial_event(running: &[Running]) -> Bytes {
    let data = json!({ "trials": running });
    Bytes::from(format!("event: trial\ndata: {data}\n\n"))
}

/// The event that tells the restart or shut down that is set, with the
/// seconds left at `now_ms` (milliseconds since 1970), or that none is.
fn power_event(set: Option<&Pending>, now_ms: u64) -> Bytes {
    let shutdown = set.map(|set| {
        json!({
            "end": set.end,
            "in_seconds": set.at_ms.saturating_sub(now_ms) / 1000,
            "at_ms": set.at_ms,
        })
    });
    let data = json!({ "shutdown": shutdown });
    Bytes::from(format!("event: power\ndata: {data}\n\n"))
}

/// `GET /api/1/events`: counts the tab in and starts its stream.
///
/// # Panics
///
/// Outside a `LocalSet`.
pub fn open(cx: &super::Context<'_>, live: &Live) -> Outcome {
    let (sender, receiver) = mpsc::channel(8);
    cx.shared.with_session(&live.cookie, |session| {
        session.tabs += 1;
        session.seen = Instant::now();
        session.closed = None;
    });
    tokio::task::spawn_local(stream(
        Rc::clone(cx.shared),
        live.cookie.clone(),
        live.authd.clone(),
        sender,
    ));
    Ok(Reply {
        status: StatusCode::OK,
        content: Content::Events(Events { receiver }),
        cookies: Vec::new(),
        retry_after: None,
    })
}

/// Sends `event` to the tab; `false` when the tab has gone or does not read.
async fn tell(sender: &mpsc::Sender<Bytes>, event: Bytes) -> bool {
    matches!(tokio::time::timeout(SEND_TIME, sender.send(event)).await, Ok(Ok(())))
}

/// What woke a stream.
enum Woken {
    /// A session ended or an unlock changed.
    Changed,
    /// A task changed.
    Tasks,
    /// A row of the journal ended.
    Rows,
    /// The tab has gone, or admind is stopping.
    Gone,
}

/// Waits until `changes`, `tasks` or `rows` has news or `sender`'s tab has
/// gone.
async fn woken(
    changes: &mut watch::Receiver<()>,
    tasks: &mut watch::Receiver<()>,
    rows: &mut watch::Receiver<()>,
    sender: &mpsc::Sender<Bytes>,
) -> Woken {
    let mut changed = pin!(changes.changed());
    let mut task_news = pin!(tasks.changed());
    let mut row_news = pin!(rows.changed());
    let mut closed = pin!(sender.closed());
    std::future::poll_fn(|cx| {
        if closed.as_mut().poll(cx).is_ready() {
            return Poll::Ready(Woken::Gone);
        }
        match changed.as_mut().poll(cx) {
            Poll::Ready(Ok(())) => return Poll::Ready(Woken::Changed),
            // Every sender has gone: admind is stopping.
            Poll::Ready(Err(_)) => return Poll::Ready(Woken::Gone),
            Poll::Pending => {}
        }
        match task_news.as_mut().poll(cx) {
            Poll::Ready(Ok(())) => return Poll::Ready(Woken::Tasks),
            Poll::Ready(Err(_)) => return Poll::Ready(Woken::Gone),
            Poll::Pending => {}
        }
        match row_news.as_mut().poll(cx) {
            Poll::Ready(Ok(())) => Poll::Ready(Woken::Rows),
            Poll::Ready(Err(_)) => Poll::Ready(Woken::Gone),
            Poll::Pending => Poll::Pending,
        }
    })
    .await
}

/// Tells a tab of its session until the session or the tab ends, then
/// counts the tab out.
async fn stream(shared: Rc<Shared>, cookie: String, authd: String, sender: mpsc::Sender<Bytes>) {
    let mut changes = shared.changes.subscribe();
    let mut task_news = shared.tasks.subscribe();
    let mut row_news = shared.journal.news.subscribe();
    // The unlock last told, so that only a change is, what of each task,
    // where in the journal's news the tab is, and how often the trials had
    // been told, and what is set to end the machine had changed, when this
    // stream last sent them: none, until it has.
    let mut told = None;
    let mut told_tasks = Told::default();
    let mut told_rows = shared.journal.news.position();
    let mut told_trials = None;
    let mut told_power = None;
    loop {
        let Some(until) = shared.with_session(&cookie, |session| session.unlocked_until) else {
            let reason = shared.endings.borrow().reason(&authd);
            // A tab that has gone cannot be told, and the stream ends
            // either way.
            tell(&sender, session_event(reason)).await;
            return;
        };
        if told != Some(until) {
            told = Some(until);
            if !tell(&sender, unlock_event(until, unix_now())).await {
                break;
            }
        }
        let (times, running) = {
            let trials = shared.trials.borrow();
            (trials.times_told(), trials.running(Instant::now()))
        };
        // A tab that opens is told only if a trial runs; after that, each time
        // they have been told.
        let trials_due = told_trials.map_or(!running.is_empty(), |told| told != times);
        told_trials = Some(times);
        if trials_due && !tell(&sender, trial_event(&running)).await {
            break;
        }
        // Likewise what is set to end the machine: to a tab that opens only
        // if something is.
        let (times, set) = shared.power.borrow().told();
        let power_due = told_power.map_or(set.is_some(), |told| told != times);
        told_power = Some(times);
        let now_ms = crate::util::now_ns() / 1_000_000;
        if power_due && !tell(&sender, power_event(set.as_ref(), now_ms)).await {
            break;
        }
        let (due, held) = shared.tasks.events(&mut told_tasks, tokio::time::Instant::now());
        let mut gone = false;
        for event in due {
            if !tell(&sender, event).await {
                gone = true;
                break;
            }
        }
        if gone {
            break;
        }
        let (position, rows) = shared.journal.news.since(told_rows);
        told_rows = position;
        for row in &rows {
            if !tell(&sender, change_event(row)).await {
                gone = true;
                break;
            }
        }
        if gone {
            break;
        }
        let wait = held.map_or(KEEP_TIME, |held| held.min(KEEP_TIME));
        match tokio::time::timeout(
            wait,
            woken(&mut changes, &mut task_news, &mut row_news, &sender),
        )
        .await
        {
            Ok(Woken::Changed | Woken::Tasks | Woken::Rows) => {}
            Ok(Woken::Gone) => break,
            // A task's event was held back, and its time has come.
            Err(_) if held.is_some() => {}
            Err(_) => {
                if !tell(&sender, Bytes::from_static(b": keep\n\n")).await {
                    break;
                }
            }
        }
    }
    left(&shared, &cookie, &authd, LOCK_AFTER).await;
}

/// Counts a tab of the session out, and locks the session if it was the
/// last and the session was unlocked, once `after` has passed with no tab
/// open again. Requests in that time, as a script with the session's
/// cookie makes, keep it signed in but do not keep it unlocked: only an
/// open page does.
pub(super) async fn left(shared: &Rc<Shared>, cookie: &str, authd: &str, after: Duration) {
    let last = shared.with_session(cookie, |session| {
        session.tabs = session.tabs.saturating_sub(1);
        session.seen = Instant::now();
        if session.tabs == 0 {
            session.closed = Some(session.seen);
        }
        session.tabs == 0 && session.unlocked_until.is_some()
    });
    if last != Some(true) {
        return;
    }
    tokio::time::sleep(after).await;
    // Another tab that opened and closed in the meantime has its own wait,
    // from when it closed.
    let due = shared.with_session(cookie, |session| {
        session.tabs == 0
            && session.unlocked_until.is_some()
            && session.closed.is_some_and(|closed| closed.elapsed() >= after)
    });
    if due != Some(true) {
        return;
    }
    // A follow of the whole log ends as locked, before authd ends its command.
    shared.follows.borrow_mut().lock(cookie);
    match authd::ask(&shared.authd, Console::Lock { session: authd.to_string() }, &[]).await {
        Ok(_) => {
            // authd may refuse a session it has forgotten; either way it
            // is not unlocked.
            shared.set_unlocked(cookie, None);
            log(6, "a console session was locked: no page of it has been open for a minute");
        }
        Err(why) => log(4, &why),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unlock_is_told_as_the_seconds_left_and_a_lock_as_none() {
        assert_eq!(
            unlock_event(Some(1_000 + 600), 1_000),
            Bytes::from("event: unlock\ndata: {\"unlock_seconds\":600}\n\n")
        );
        assert_eq!(
            unlock_event(None, 1_000),
            Bytes::from("event: unlock\ndata: {\"unlock_seconds\":null}\n\n")
        );
        // An unlock that ran out is a lock, whether or not authd was told.
        assert_eq!(unlock_event(Some(999), 1_000), unlock_event(None, 1_000));
    }

    #[test]
    fn a_row_is_told_as_its_number_how_it_was_made_and_when_and_nothing_else() {
        let row = Row { id: 41, how: "token ci".into(), at: 1_800_000_000 };
        assert_eq!(
            change_event(&row),
            Bytes::from(
                "event: change\ndata: {\"at\":1800000000,\"how\":\"token ci\",\"id\":41}\n\n"
            )
        );
        // The words of a person's name are text in JSON, not the event's.
        let odd = Row { id: 1, how: "by hand\nevent: unlock".into(), at: 2 };
        let told = String::from_utf8(change_event(&odd).to_vec()).unwrap();
        assert_eq!(told.matches('\n').count(), 3, "{told:?}");
    }

    #[test]
    fn the_trials_that_run_are_told_with_their_time_left_page_and_words_and_none_as_an_empty_list()
    {
        let running = [
            Running {
                id: "firewall".into(),
                left_ms: 41_000,
                page: "/firewall".into(),
                what: "The firewall's rules".into(),
            },
            Running {
                id: "service.web".into(),
                left_ms: 5_000,
                page: "/services/web".into(),
                what: "web's service file".into(),
            },
        ];
        assert_eq!(
            trial_event(&running),
            Bytes::from(
                "event: trial\ndata: {\"trials\":[\
                 {\"id\":\"firewall\",\"left_ms\":41000,\"page\":\"/firewall\",\
                 \"what\":\"The firewall's rules\"},\
                 {\"id\":\"service.web\",\"left_ms\":5000,\"page\":\"/services/web\",\
                 \"what\":\"web's service file\"}]}\n\n"
            )
        );
        assert_eq!(trial_event(&[]), Bytes::from("event: trial\ndata: {\"trials\":[]}\n\n"));
    }

    #[test]
    fn what_is_set_to_end_the_machine_is_told_with_its_seconds_left_as_it_is_sent() {
        let set = Pending { end: "reboot", in_seconds: 60, at_ms: 1_800_000_060_000 };
        assert_eq!(
            power_event(Some(&set), 1_800_000_018_500),
            Bytes::from(
                "event: power\ndata: {\"shutdown\":{\"at_ms\":1800000060000,\"end\":\"reboot\",\
                 \"in_seconds\":41}}\n\n"
            )
        );
        // Its time gone by is no time left, never a time that wraps.
        let late = power_event(Some(&set), 1_800_000_090_000);
        assert!(String::from_utf8(late.to_vec()).unwrap().contains("\"in_seconds\":0"));
        assert_eq!(
            power_event(None, 0),
            Bytes::from("event: power\ndata: {\"shutdown\":null}\n\n")
        );
    }

    #[test]
    fn the_end_of_a_session_is_told_with_its_reason_in_one_event() {
        assert_eq!(
            session_event("idle"),
            Bytes::from("event: session\ndata: {\"ended\":\"idle\"}\n\n")
        );
    }
}
