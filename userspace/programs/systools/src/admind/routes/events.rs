// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! A tab's stream of events (ADR 0514), `GET /api/1/events`, as
//! server-sent events: `unlock` with the seconds left when the session's
//! unlock changes, `session` with why it ended when it does, `task` for
//! each change of a task's state, step or progress (ADR 0516), and a
//! comment every 25 seconds so that nothing between the browser and admind
//! closes a stream that says nothing.
//!
//! A tab that opens is told where each task is that waits, runs or ended
//! in the last minute, so that it has no gap between what it asked for and
//! what came after. Later a task is told at most four times a second: what
//! comes sooner waits, and only the latest is sent.
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
use crate::admind::log;
use crate::admind::shared::{Shared, seconds_left, unix_now};
use crate::admind::tasks::Told;

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
    /// The tab has gone, or admind is stopping.
    Gone,
}

/// Waits until `changes` or `tasks` has news or `sender`'s tab has gone.
async fn woken(
    changes: &mut watch::Receiver<()>,
    tasks: &mut watch::Receiver<()>,
    sender: &mpsc::Sender<Bytes>,
) -> Woken {
    let mut changed = pin!(changes.changed());
    let mut task_news = pin!(tasks.changed());
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
            Poll::Ready(Ok(())) => Poll::Ready(Woken::Tasks),
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
    // The unlock last told, so that only a change is, and what of each
    // task.
    let mut told = None;
    let mut told_tasks = Told::default();
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
        let wait = held.map_or(KEEP_TIME, |held| held.min(KEEP_TIME));
        match tokio::time::timeout(wait, woken(&mut changes, &mut task_news, &sender)).await {
            Ok(Woken::Changed | Woken::Tasks) => {}
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
    fn the_end_of_a_session_is_told_with_its_reason_in_one_event() {
        assert_eq!(
            session_event("idle"),
            Bytes::from("event: session\ndata: {\"ended\":\"idle\"}\n\n")
        );
    }
}
