// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The follows of the log (ADR 0518): what admind keeps of each, and the
//! task that reads one and tells its tab. The route,
//! `GET /api/1/logs/follow`, is `routes::follow`'s.
//!
//! A follow is one command for as long as the page follows: authd starts
//! `log --output json --follow --lines 0` on two pipes whose read ends
//! admind keeps, its output and its errors. Each record is a line of its
//! output, which the task sends as a server-sent event:
//!
//! - `command`, once: `{"shown": "elevate log --output json ..."}`;
//! - `record`, a record each, as log prints it;
//! - `skipped`, `{"count": N}`, when more than [`PER_SECOND`] records came
//!   in a second: the first are sent, then the count;
//! - `ended`, `{"why": W}`, before the stream closes, with `error`, log's
//!   last line of errors, when it is `failed`.
//!
//! [`Why`] says each reason. A session has one follow: a second ends the
//! first (`replaced`). admind runs [`FOLLOWS_MAX`] at once, each for
//! [`FOLLOW_TIME`] at most, and ends one whose tab leaves [`UNSENT_MAX`]
//! unread. When admind ends a follow, it asks authd to end its process,
//! which authd reaps; authd also ends it by itself when the session ends,
//! and, for the whole log, when the session locks.

use std::cell::Cell;
use std::collections::HashMap;
use std::convert::Infallible;
use std::future::{Future, poll_fn};
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::pin::{Pin, pin};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::task::{self, Poll};
use std::time::Duration;

use accounts::console::Console;
use bytes::Bytes;
use http_body::Frame;
use serde_json::{Value, json};
use tokio::io::{AsyncBufRead, AsyncBufReadExt, BufReader};
use tokio::net::unix::pipe;
use tokio::sync::{Notify, mpsc};
use tokio::time::{Instant, sleep_until};

use accounts::request::Answer;

use super::authd;
use super::errors::{self, Attempt, Failure, Kind};
use super::shared::{Shared, unix_now};

/// The most follows admind runs at once, of every session together.
pub const FOLLOWS_MAX: usize = 8;

/// How long a follow lasts: an hour, or a minute in the lab's build, for
/// the boot test.
pub const FOLLOW_TIME: Duration =
    if cfg!(feature = "lab") { Duration::from_secs(60) } else { Duration::from_secs(3600) };

/// How long a refused follow is told to wait before it asks again.
pub const RETRY_AFTER: u64 = 30;

/// The most records sent in a second; the rest are counted.
const PER_SECOND: u32 = 100;

/// The most bytes a tab may leave unread before its follow ends.
const UNSENT_MAX: usize = 256 * 1024;

/// The most events waiting for the tab: far more than a second's.
const QUEUED_MAX: usize = 1024;

/// The longest record read: a longer one is skipped.
const LINE_MAX: usize = 64 * 1024;

/// The most of log's last line of errors kept.
const ERROR_MAX: usize = 1024;

/// How long a stream may say nothing.
const KEEP_TIME: Duration = Duration::from_secs(25);

/// Why a follow ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Why {
    /// The session locked, and the follow read the whole log.
    Locked,
    /// The session's unlock ran out, likewise.
    UnlockEnded,
    /// The session ended.
    SessionEnded,
    /// The session began another follow.
    Replaced,
    /// [`FOLLOW_TIME`] passed.
    Time,
    /// The tab left [`UNSENT_MAX`] unread.
    Slow,
    /// admind is stopping.
    Stopped,
    /// log ended by itself.
    Failed,
    /// The tab went: no one is told.
    Gone,
}

impl Why {
    /// The word the `ended` event says.
    pub fn word(self) -> &'static str {
        match self {
            Why::Locked => "locked",
            Why::UnlockEnded => "unlock-ended",
            Why::SessionEnded => "session-ended",
            Why::Replaced => "replaced",
            Why::Time => "time",
            Why::Slow => "slow",
            Why::Stopped => "stopped",
            Why::Failed => "failed",
            Why::Gone => "gone",
        }
    }
}

/// What ends a follow from outside its task: the reason, then a wake.
#[derive(Default)]
pub struct Stop {
    why: Cell<Option<Why>>,
    notify: Notify,
}

impl Stop {
    /// Ends the follow for `why`, unless it was ended already.
    fn end(&self, why: Why) {
        if self.why.get().is_none() {
            self.why.set(Some(why));
        }
        self.notify.notify_one();
    }
}

/// A follow that runs: its number, whether it reads the whole log, and
/// what ends it.
struct Running {
    id: u64,
    all: bool,
    stop: Rc<Stop>,
}

/// The follows that run, one for each session at most.
#[derive(Default)]
pub struct Follows {
    running: HashMap<String, Running>,
    next: u64,
}

impl Follows {
    /// Takes a place for a follow of the session `cookie`, of the whole
    /// log when `all`, whose follow before, if it had one, ends as
    /// replaced. None when [`FOLLOWS_MAX`] other sessions follow.
    pub fn begin(&mut self, cookie: &str, all: bool) -> Option<(u64, Rc<Stop>)> {
        if !self.running.contains_key(cookie) && self.running.len() >= FOLLOWS_MAX {
            return None;
        }
        self.next += 1;
        let stop = Rc::new(Stop::default());
        let running = Running { id: self.next, all, stop: Rc::clone(&stop) };
        if let Some(before) = self.running.insert(cookie.to_string(), running) {
            before.stop.end(Why::Replaced);
        }
        Some((self.next, stop))
    }

    /// Gives back the place of the follow `id` of the session `cookie`, if
    /// it still has it.
    pub fn end(&mut self, cookie: &str, id: u64) {
        if self.running.get(cookie).is_some_and(|running| running.id == id) {
            self.running.remove(cookie);
        }
    }

    /// Ends the session `cookie`'s follow of the whole log as locked,
    /// before authd locks the session and so ends its command: the follow
    /// then tells the lock, not that the command ended. A follow of the
    /// signed-in person's own records goes on.
    pub fn lock(&mut self, cookie: &str) {
        if let Some(running) = self.running.get(cookie).filter(|running| running.all) {
            running.stop.end(Why::Locked);
        }
    }

    /// Ends every follow, as admind stops.
    pub fn stop_all(&mut self) {
        for running in self.running.values() {
            running.stop.end(Why::Stopped);
        }
    }
}

/// The body of the answer: what the follow's task sends, with a count of
/// the bytes it holds that the tab has not taken.
pub struct Followed {
    receiver: mpsc::Receiver<Bytes>,
    unsent: Arc<AtomicUsize>,
}

impl http_body::Body for Followed {
    type Data = Bytes;
    type Error = Infallible;

    fn poll_frame(
        mut self: Pin<&mut Self>,
        cx: &mut task::Context<'_>,
    ) -> Poll<Option<Result<Frame<Bytes>, Infallible>>> {
        let polled = self.receiver.poll_recv(cx);
        if let Poll::Ready(Some(bytes)) = &polled {
            self.unsent.fetch_sub(bytes.len(), Ordering::Relaxed);
        }
        polled.map(|sent| sent.map(|bytes| Ok(Frame::data(bytes))))
    }
}

/// What the task sends to the tab through.
struct Tab {
    sender: mpsc::Sender<Bytes>,
    unsent: Arc<AtomicUsize>,
}

impl Tab {
    /// A tab's body and what sends to it.
    fn new() -> (Tab, Followed) {
        let (sender, receiver) = mpsc::channel(QUEUED_MAX);
        let unsent = Arc::new(AtomicUsize::new(0));
        (Tab { sender, unsent: Arc::clone(&unsent) }, Followed { receiver, unsent })
    }

    /// Queues `event` for the tab, without waiting: Err when the tab has
    /// gone, or has left too much unread.
    fn tell(&self, event: Bytes) -> Result<(), Why> {
        let size = event.len();
        if self.unsent.load(Ordering::Relaxed) + size > UNSENT_MAX {
            return Err(Why::Slow);
        }
        self.unsent.fetch_add(size, Ordering::Relaxed);
        self.sender.try_send(event).map_err(|err| {
            self.unsent.fetch_sub(size, Ordering::Relaxed);
            match err {
                mpsc::error::TrySendError::Full(_) => Why::Slow,
                mpsc::error::TrySendError::Closed(_) => Why::Gone,
            }
        })
    }

    /// Queues the last event, past the bound on what is unread: it is
    /// small, and the stream closes after it.
    fn tell_last(&self, event: Bytes) {
        self.unsent.fetch_add(event.len(), Ordering::Relaxed);
        if self.sender.try_send(event).is_err() {
            // A tab that has gone or is full is not told; the stream ends.
        }
    }
}

/// The `command` event: the command the follow runs, as a person types it.
pub fn command_event(shown: &str) -> Bytes {
    Bytes::from(format!("event: command\ndata: {}\n\n", json!({ "shown": shown })))
}

/// The `record` event of a line of log's output, if it is a JSON value:
/// written again, so that it is one line, whatever spaces it had.
fn record_event(line: &[u8]) -> Option<Bytes> {
    let value: Value = serde_json::from_slice(line).ok()?;
    Some(Bytes::from(format!("event: record\ndata: {value}\n\n")))
}

/// The `skipped` event: `count` records were not sent.
fn skipped_event(count: u64) -> Bytes {
    Bytes::from(format!("event: skipped\ndata: {}\n\n", json!({ "count": count })))
}

/// The `ended` event, with log's last line of errors when it failed.
fn ended_event(why: Why, error: &str) -> Bytes {
    let data = if why == Why::Failed {
        json!({ "why": why.word(), "error": error })
    } else {
        json!({ "why": why.word() })
    };
    Bytes::from(format!("event: ended\ndata: {data}\n\n"))
}

/// A pipe: its read end, which admind keeps and reads without blocking,
/// and its write end, which authd gives log.
fn pipe() -> io::Result<(pipe::Receiver, OwnedFd)> {
    let mut ends = [0; 2];
    // SAFETY: `ends` has room for the two descriptors pipe2 writes.
    if unsafe { libc::pipe2(ends.as_mut_ptr(), libc::O_CLOEXEC) } != 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: pipe2 opened both, and nothing else owns them.
    let [read, write] = ends.map(|fd| unsafe { OwnedFd::from_raw_fd(fd) });
    // SAFETY: fcntl on an open descriptor touches no memory.
    let flags = unsafe { libc::fcntl(read.as_raw_fd(), libc::F_GETFL) };
    if flags < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: as above.
    if unsafe { libc::fcntl(read.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok((pipe::Receiver::from_owned_fd_unchecked(read)?, write))
}

/// What a follow is: whose, what it runs, and what it reads.
pub struct Asked {
    /// The session's cookie, which admind knows it by.
    pub cookie: String,
    /// The session's ID at authd.
    pub authd: String,
    /// Whether it reads the whole log.
    pub all: bool,
    /// The request authd starts it with.
    pub request: Console,
    /// The command, as a person types it.
    pub shown: String,
}

/// Starts the follow `asked`, in the place `id` that [`Follows::begin`]
/// gave with `stop`: authd starts log on two new pipes, and a task reads
/// them and tells the tab. Answers the tab's body.
///
/// # Errors
///
/// ADM-121 when a pipe cannot be opened, ADM-111 when authd does not
/// answer, and authd's refusal, which may carry an ID of the console's
/// (ADM-106, ADM-122, ADM-125, ADM-134). The place is given back.
///
/// # Panics
///
/// Outside a `LocalSet`.
pub async fn start(
    shared: &Rc<Shared>,
    asked: Asked,
    id: u64,
    stop: Rc<Stop>,
) -> Result<Followed, Failure> {
    let started = async {
        let failed =
            |err: io::Error| Failure::new(Kind::Internal, format!("cannot open a pipe: {err}"));
        let (output, output_end) = pipe().map_err(failed)?;
        let (errors, errors_end) = pipe().map_err(failed)?;
        let fds = [output_end.as_raw_fd(), errors_end.as_raw_fd()];
        let (answer, _) = authd::ask(&shared.authd, asked.request.clone(), &fds)
            .await
            .map_err(|why| Failure::new(Kind::AuthdSilent, why))?;
        // log holds the write ends now, if it started; the reads end when
        // it does.
        drop((output_end, errors_end));
        match answer {
            Answer::Done(text) => {
                let process = text.trim().parse::<i32>().map_err(|_| {
                    Failure::new(Kind::AuthdSilent, "authd answered a process that does not read")
                })?;
                Ok((process, output, errors))
            }
            Answer::Refused(why) => Err(errors::refusal(Attempt::Other, &why)),
        }
    };
    let (process, output, errors) = match started.await {
        Ok(started) => started,
        Err(why) => {
            shared.follows.borrow_mut().end(&asked.cookie, id);
            return Err(why);
        }
    };
    let (tab, body) = Tab::new();
    let reading = Reading { shared: Rc::clone(shared), asked, id, stop, process, tab };
    tokio::task::spawn_local(reading.run(output, errors));
    Ok(body)
}

/// A follow's task.
struct Reading {
    shared: Rc<Shared>,
    asked: Asked,
    id: u64,
    stop: Rc<Stop>,
    process: i32,
    tab: Tab,
}

/// How many records were sent in the second that began at `began`, and
/// how many skipped.
struct Second {
    began: Instant,
    sent: u32,
    skipped: u64,
}

impl Second {
    /// Whether a record that came at `now` is sent: the second begins
    /// again once it has passed.
    fn take(&mut self, now: Instant) -> bool {
        if now.duration_since(self.began) >= Duration::from_secs(1) {
            *self = Second { began: now, sent: 0, skipped: self.skipped };
        }
        if self.sent < PER_SECOND {
            self.sent += 1;
            true
        } else {
            self.skipped += 1;
            false
        }
    }
}

/// What woke a follow's task.
enum Woke {
    /// Something ended it from outside, as its [`Stop`] says.
    Stopped,
    /// The tab went.
    Gone,
    /// The session's state may have changed, or its unlock run out.
    Look,
    /// admind is stopping.
    Stopping,
    /// [`FOLLOW_TIME`] passed.
    Time,
    /// The second whose records were counted has passed.
    Count,
    /// The stream has said nothing for [`KEEP_TIME`].
    Keep,
    /// A line of log's output was read, or its end.
    Read(io::Result<bool>),
}

impl Reading {
    /// Why the session's state ends the follow now, if it does.
    fn session_ends_it(&self) -> Option<Why> {
        let until = self.shared.with_session(&self.asked.cookie, |session| session.unlocked_until);
        match until {
            None => Some(Why::SessionEnded),
            Some(None) if self.asked.all => Some(Why::Locked),
            Some(Some(until)) if self.asked.all && until <= unix_now() => Some(Why::UnlockEnded),
            Some(_) => None,
        }
    }

    /// When the session's unlock runs out, for a follow of the whole log.
    fn unlock_ends(&self) -> Option<Instant> {
        if !self.asked.all {
            return None;
        }
        let until = self.shared.with_session(&self.asked.cookie, |s| s.unlocked_until)??;
        let left = u64::try_from(until.saturating_sub(unix_now())).unwrap_or(0);
        Some(Instant::now() + Duration::from_secs(left))
    }

    /// Reads log's records from `output` and tells the tab, until the
    /// follow ends; then tells why, and ends log if it runs.
    async fn run(self, output: pipe::Receiver, errors: pipe::Receiver) {
        let last_error = tokio::task::spawn_local(last_line(errors));
        let why = self.follow(output).await;
        let error = if why == Why::Failed {
            last_error.await.unwrap_or_default()
        } else {
            last_error.abort();
            String::new()
        };
        if why != Why::Gone {
            self.tab.tell_last(ended_event(why, &error));
        }
        // log ended by itself, or authd ends it: the session ended or
        // locked. Else authd ends it now.
        if !matches!(why, Why::Failed | Why::SessionEnded | Why::Locked | Why::UnlockEnded) {
            let end =
                Console::FollowEnd { session: self.asked.authd.clone(), process: self.process };
            if let Err(why) = authd::ask(&self.shared.authd, end, &[]).await {
                super::log(4, &format!("a follow of the log was not ended: {why}"));
            }
        }
        self.shared.follows.borrow_mut().end(&self.asked.cookie, self.id);
    }

    /// Tells the tab each record until the follow ends, and says why.
    async fn follow(&self, output: pipe::Receiver) -> Why {
        if let Err(why) = self.tab.tell(command_event(&self.asked.shown)) {
            return why;
        }
        let mut changes = self.shared.changes.subscribe();
        let mut reader = BufReader::new(output);
        let mut line = Vec::new();
        let mut cut = false;
        let ends = Instant::now() + FOLLOW_TIME;
        let mut second = Second { began: Instant::now(), sent: 0, skipped: 0 };
        let mut told = Instant::now();
        loop {
            if let Some(why) = self.session_ends_it() {
                return why;
            }
            if let Some(why) = self.stop.why.get() {
                return why;
            }
            let unlock_ends = self.unlock_ends().unwrap_or(ends);
            let count_due = second.began + Duration::from_secs(1);
            let counting = second.skipped > 0;
            let woke = {
                let mut stopped = pin!(self.stop.notify.notified());
                let mut gone = pin!(self.tab.sender.closed());
                let mut changed = pin!(changes.changed());
                let mut time = pin!(sleep_until(ends));
                let mut unlock = pin!(sleep_until(unlock_ends));
                let mut count = pin!(sleep_until(count_due));
                let mut keep = pin!(sleep_until(told + KEEP_TIME));
                let mut read = pin!(next_line(&mut reader, &mut line, &mut cut));
                // In this order, so that an end comes before a record, and
                // the count of a second before the next second's records.
                poll_fn(|cx| {
                    if stopped.as_mut().poll(cx).is_ready() {
                        return Poll::Ready(Woke::Stopped);
                    }
                    if gone.as_mut().poll(cx).is_ready() {
                        return Poll::Ready(Woke::Gone);
                    }
                    if let Poll::Ready(changed) = changed.as_mut().poll(cx) {
                        // Every sender has gone: admind is stopping.
                        return Poll::Ready(if changed.is_ok() {
                            Woke::Look
                        } else {
                            Woke::Stopping
                        });
                    }
                    if time.as_mut().poll(cx).is_ready() {
                        return Poll::Ready(Woke::Time);
                    }
                    if unlock.as_mut().poll(cx).is_ready() {
                        return Poll::Ready(Woke::Look);
                    }
                    if counting && count.as_mut().poll(cx).is_ready() {
                        return Poll::Ready(Woke::Count);
                    }
                    if keep.as_mut().poll(cx).is_ready() {
                        return Poll::Ready(Woke::Keep);
                    }
                    read.as_mut().poll(cx).map(Woke::Read)
                })
                .await
            };
            match woke {
                Woke::Stopped | Woke::Look => {}
                Woke::Gone => return Why::Gone,
                Woke::Stopping => return Why::Stopped,
                Woke::Time => return Why::Time,
                Woke::Count => {
                    let skipped = std::mem::take(&mut second.skipped);
                    if let Err(why) = self.tab.tell(skipped_event(skipped)) {
                        return why;
                    }
                    second = Second { began: Instant::now(), sent: 0, skipped: 0 };
                    told = Instant::now();
                }
                Woke::Keep => {
                    if let Err(why) = self.tab.tell(Bytes::from_static(b": keep\n\n")) {
                        return why;
                    }
                    told = Instant::now();
                }
                Woke::Read(Ok(true)) => {
                    let event = record_event(&line);
                    line.clear();
                    if let Some(event) = event
                        && second.take(Instant::now())
                    {
                        if let Err(why) = self.tab.tell(event) {
                            return why;
                        }
                        told = Instant::now();
                    }
                }
                // log ended: by itself, or authd ended it, as the
                // session's state or the stop says.
                Woke::Read(Ok(false) | Err(_)) => {
                    tokio::task::yield_now().await;
                    return self
                        .session_ends_it()
                        .or_else(|| self.stop.why.get())
                        .unwrap_or(Why::Failed);
                }
            }
        }
    }
}

/// Reads the next line of `reader` into `line`, without its newline: true
/// for a line, false at the end. A line longer than [`LINE_MAX`] is
/// skipped up to its newline, and `cut` says that one is being skipped.
/// What was read stays in `line` and `cut` when the read is given up, so
/// that the next goes on from there.
async fn next_line(
    reader: &mut (impl AsyncBufRead + Unpin),
    line: &mut Vec<u8>,
    cut: &mut bool,
) -> io::Result<bool> {
    loop {
        let available = reader.fill_buf().await?;
        if available.is_empty() {
            // A last line with no newline is not a whole record.
            return Ok(false);
        }
        let (taken, ended) = match available.iter().position(|&byte| byte == b'\n') {
            Some(at) => (at + 1, true),
            None => (available.len(), false),
        };
        let part = &available[..if ended { taken - 1 } else { taken }];
        if !*cut {
            if line.len() + part.len() > LINE_MAX {
                *cut = true;
                line.clear();
            } else {
                line.extend_from_slice(part);
            }
        }
        reader.consume(taken);
        if ended {
            let whole = !std::mem::take(cut);
            if whole {
                return Ok(true);
            }
        }
    }
}

/// The last line `errors` says before it ends, at most [`ERROR_MAX`]
/// bytes of it.
async fn last_line(errors: pipe::Receiver) -> String {
    let mut reader = BufReader::new(errors);
    let mut line = Vec::new();
    let mut last = Vec::new();
    let mut cut = false;
    while let Ok(true) = next_line(&mut reader, &mut line, &mut cut).await {
        if !line.iter().all(u8::is_ascii_whitespace) {
            last = std::mem::take(&mut line);
        }
        line.clear();
    }
    if !line.is_empty() {
        last = line;
    }
    last.truncate(ERROR_MAX);
    String::from_utf8_lossy(&last).trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_session_has_one_follow_and_admind_eight() {
        let mut follows = Follows::default();
        let (first, stop) = follows.begin("a", true).unwrap();
        let (second, _) = follows.begin("a", true).unwrap();
        assert_ne!(first, second);
        assert_eq!(stop.why.get(), Some(Why::Replaced));
        assert_eq!(follows.running.len(), 1);
        // The first's end does not take the second's place.
        follows.end("a", first);
        assert_eq!(follows.running.len(), 1);
        for name in ["b", "c", "d", "e", "f", "g", "h"] {
            assert!(follows.begin(name, true).is_some(), "{name}");
        }
        assert_eq!(follows.running.len(), FOLLOWS_MAX);
        assert!(follows.begin("i", true).is_none());
        // A session that follows already may begin again.
        assert!(follows.begin("h", true).is_some());
        follows.end("a", second);
        assert!(follows.begin("i", true).is_some());
        let stops: Vec<Rc<Stop>> =
            follows.running.values().map(|running| Rc::clone(&running.stop)).collect();
        follows.stop_all();
        assert!(stops.iter().all(|stop| stop.why.get() == Some(Why::Stopped)));
    }

    #[test]
    fn a_lock_ends_only_a_follow_of_the_whole_log() {
        let mut follows = Follows::default();
        let (_, all) = follows.begin("a", true).unwrap();
        let (_, own) = follows.begin("b", false).unwrap();
        follows.lock("a");
        follows.lock("b");
        follows.lock("c");
        assert_eq!(all.why.get(), Some(Why::Locked));
        assert_eq!(own.why.get(), None);
        // The place is given back when the follow's task ends.
        assert_eq!(follows.running.len(), 2);
    }

    #[test]
    fn a_stop_keeps_its_first_reason() {
        let stop = Stop::default();
        stop.end(Why::Replaced);
        stop.end(Why::Stopped);
        assert_eq!(stop.why.get(), Some(Why::Replaced));
    }

    #[test]
    fn each_event_is_one_event_of_one_data_line() {
        assert_eq!(
            command_event("elevate log --output json"),
            Bytes::from("event: command\ndata: {\"shown\":\"elevate log --output json\"}\n\n")
        );
        // A record spread over a line's spaces is written again whole.
        let record = record_event(b"{ \"text\": \"a\\nevent: x\",\r\t\"level\": 3 }").unwrap();
        let text = String::from_utf8(record.to_vec()).unwrap();
        assert_eq!(text, "event: record\ndata: {\"level\":3,\"text\":\"a\\nevent: x\"}\n\n");
        assert_eq!(record_event(b"not json"), None);
        assert_eq!(record_event(b""), None);
        assert_eq!(skipped_event(7), Bytes::from("event: skipped\ndata: {\"count\":7}\n\n"));
        assert_eq!(
            ended_event(Why::Failed, "log: logd is not running"),
            Bytes::from(
                "event: ended\ndata: {\"error\":\"log: logd is not running\",\"why\":\"failed\"}\n\n"
            )
        );
        assert_eq!(
            ended_event(Why::UnlockEnded, "ignored"),
            Bytes::from("event: ended\ndata: {\"why\":\"unlock-ended\"}\n\n")
        );
    }

    #[test]
    fn a_hundred_records_go_each_second_and_the_rest_are_counted() {
        let start = Instant::now();
        let mut second = Second { began: start, sent: 0, skipped: 0 };
        let sent = (0..150).filter(|_| second.take(start)).count();
        assert_eq!((sent, second.skipped), (100, 50));
        assert!(second.take(start + Duration::from_secs(1)));
        assert_eq!((second.sent, second.skipped), (1, 50));
    }

    #[test]
    fn lines_are_read_whole_and_one_too_long_is_skipped() {
        let long = vec![b'x'; LINE_MAX + 1];
        let mut text = b"{\"a\":1}\n".to_vec();
        text.extend_from_slice(&long);
        text.extend_from_slice(b"\n{\"b\":2}\npartial");
        // A small buffer, so that lines come in pieces.
        let mut reader = BufReader::with_capacity(7, text.as_slice());
        let mut line = Vec::new();
        let mut cut = false;
        let mut lines = Vec::new();
        let runtime = tokio::runtime::Builder::new_current_thread().build().unwrap();
        runtime.block_on(async {
            while next_line(&mut reader, &mut line, &mut cut).await.unwrap() {
                lines.push(String::from_utf8(std::mem::take(&mut line)).unwrap());
            }
        });
        assert_eq!(lines, ["{\"a\":1}", "{\"b\":2}"]);
    }

    #[test]
    fn a_tab_that_leaves_too_much_unread_is_slow_and_one_that_went_is_gone() {
        let (tab, body) = Tab::new();
        let big = Bytes::from(vec![b'x'; UNSENT_MAX / 2]);
        assert_eq!(tab.tell(big.clone()), Ok(()));
        assert_eq!(tab.tell(big.clone()), Ok(()));
        assert_eq!(tab.tell(Bytes::from_static(b"x")), Err(Why::Slow));
        drop(body);
        let (tab, body) = Tab::new();
        drop(body);
        assert_eq!(tab.tell(Bytes::from_static(b"x")), Err(Why::Gone));
        assert_eq!(tab.unsent.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn every_reason_has_its_word() {
        let words: Vec<&str> = [
            Why::Locked,
            Why::UnlockEnded,
            Why::SessionEnded,
            Why::Replaced,
            Why::Time,
            Why::Slow,
            Why::Stopped,
            Why::Failed,
        ]
        .map(Why::word)
        .to_vec();
        assert_eq!(
            words,
            [
                "locked",
                "unlock-ended",
                "session-ended",
                "replaced",
                "time",
                "slow",
                "stopped",
                "failed"
            ]
        );
    }
}
