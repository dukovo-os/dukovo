// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! alertd's socket (ADR 0485): a connection sends one request, a line
//! of JSON, and reads the answer ([`crate::alerts::protocol`]), after
//! which alertd closes it.
//!
//! Each connection has a thread of its own, at most 16 at a time; a 17th
//! is told alertd is busy. Those of programs that may not ask have two
//! threads of their own, past which they are refused unread, so that they
//! never take the threads of those that may. A request has 5 seconds in all to arrive,
//! whatever it does, and is at most 16 KiB, so a program that sends it
//! slowly or without end holds one thread for a few seconds and nothing
//! else.
//!
//! Only root's programs may ask, as the kernel says who they act for
//! (`socket_state`, which is POSIX's SO_PEERCRED), never as the request
//! says. An alert's source is the resource domain of the process that
//! sent it, as /processes names it: a service's is the service's name.
//! The thread applies the change, queues what is to be delivered and
//! answers; it never sends a message itself, but for `test`, which
//! waits for the threads that do.

use std::io::{self, BufRead, BufReader, Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::net::UnixStream;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;
use std::time::Duration;

use serde::Serialize;

use super::{Alertd, CONNECTIONS_MAX};
use crate::alerts::protocol::{Alert, Done, End, REQUEST_MAX, Refusal, Request};
use crate::deadline::Deadline;
use crate::{notifyd, proc, sys};

/// How long a request has to arrive in all, and an answer to be read.
const REQUEST_TIME: Duration = Duration::from_secs(5);

/// The most connections of those who may not ask that are read before
/// they are refused; past it, one is refused at once.
const REFUSING_MAX: usize = 2;

/// The history's newest changes `history` gives without a count, and the
/// most it gives.
const HISTORY_DEFAULT: usize = 20;
const HISTORY_MAX: usize = 1000;

const NOT_ROOT: &str = "only root and the system's services raise and read alerts; try elevate";
const BUSY: &str = "alertd is busy with other requests; try again in a moment";

/// Who is at the other end: the source of what they raise.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Who {
    pub source: String,
}

/// Takes `stream`: serves it on a thread of its own, or says alertd is
/// busy.
pub fn accept(alertd: &Arc<Alertd>, stream: UnixStream) {
    // Asked at once: the peer waits for its answer, so it is still there.
    let who = who_is(&stream);
    admit(alertd, stream, who);
}

/// Serves `stream`, from `who`, on a thread of its own, or turns it away
/// at once. Those who may not ask are counted apart and kept to a few
/// threads, so that a person's program that connects again and again
/// cannot take the threads the system's services raise alerts on.
fn admit(alertd: &Arc<Alertd>, stream: UnixStream, who: Result<Who, String>) {
    let root = who.is_ok();
    let max = if root { CONNECTIONS_MAX } else { REFUSING_MAX };
    if count(alertd, root).fetch_add(1, Ordering::AcqRel) >= max {
        count(alertd, root).fetch_sub(1, Ordering::AcqRel);
        return refuse(&stream, who.err().as_deref().unwrap_or(BUSY));
    }
    let this = Arc::clone(alertd);
    let own = stream.try_clone();
    let started = thread::Builder::new().name("alertd-request".into()).spawn(move || {
        serve(&this, &stream, who);
        count(&this, root).fetch_sub(1, Ordering::AcqRel);
    });
    if started.is_err() {
        count(alertd, root).fetch_sub(1, Ordering::AcqRel);
        if let Ok(stream) = own {
            refuse(&stream, BUSY);
        }
    }
}

/// The count of connections being served of those who may ask, when
/// `root`, else of those who may not.
fn count(alertd: &Alertd, root: bool) -> &AtomicUsize {
    if root { &alertd.serving } else { &alertd.refusing }
}

/// Says `why` to a connection that is turned away.
fn refuse(stream: &UnixStream, why: &str) {
    if let Ok(mut out) = Deadline::new(stream, Duration::from_secs(1)) {
        let _ = line(&mut out, &Refusal { error: why.into() });
    }
}

/// Who is at the other end of `stream`, or why they may not ask.
fn who_is(stream: &UnixStream) -> Result<Who, String> {
    let slot = sys::slot_of(stream.as_raw_fd());
    let state = radix_abi::file::socket_state(&sys::SYS, slot)
        .map_err(|_| "alertd cannot tell who is asking".to_string())?;
    if state.user != 0 {
        return Err(NOT_ROOT.into());
    }
    let domain = proc::process(state.peer).map_or_default(|process| process.domain);
    let source = notifyd::clean(&domain, 64, false);
    Ok(Who { source: if source.is_empty() { "unknown".into() } else { source } })
}

/// Reads the request on `stream` and answers it.
pub fn serve(alertd: &Alertd, stream: &UnixStream, who: Result<Who, String>) {
    let (Ok(request_in), Ok(answer_out)) =
        (Deadline::new(stream, REQUEST_TIME), Deadline::each(stream, REQUEST_TIME))
    else {
        return;
    };
    let mut out = io::BufWriter::new(answer_out);
    // The request is read first, even from one who may not ask, so that
    // the refusal is not lost to a connection closed on unread bytes.
    let request = read_request(request_in);
    let answered = match request.and_then(|request| who.map(|who| (who, request))) {
        Ok((who, request)) => answer(alertd, &who, &request, &mut out),
        Err(why) => Err(why),
    };
    if let Err(why) = answered {
        let _ = line(&mut out, &Refusal { error: why });
    }
    let _ = out.flush();
}

/// The request that `stream` sends before its deadline.
fn read_request(stream: Deadline<'_>) -> Result<Request, String> {
    let mut buffered = BufReader::new(stream);
    let mut text = Vec::new();
    let limit = (REQUEST_MAX + 1) as u64;
    match (&mut buffered).take(limit).read_until(b'\n', &mut text) {
        Ok(0) => return Err("the request is empty".into()),
        Ok(_) if text.len() > REQUEST_MAX => {
            return Err(format!("the request is longer than {REQUEST_MAX} bytes"));
        }
        Ok(_) if !text.ends_with(b"\n") => return Err("the request has no end of line".into()),
        Ok(_) => {}
        Err(err) if matches!(err.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut) => {
            return Err(format!("no request within {} seconds", REQUEST_TIME.as_secs()));
        }
        Err(err) => return Err(format!("cannot read the request: {err}")),
    }
    serde_json::from_slice(&text).map_err(|err| format!("not a request: {err}"))
}

/// Writes `item` as a line of JSON.
fn line<W: Write + ?Sized>(out: &mut W, item: &impl Serialize) -> io::Result<()> {
    serde_json::to_writer(&mut *out, item).map_err(io::Error::other)?;
    out.write_all(b"\n")
}

/// Does what `request` asks, for `who`, and writes the answer; or says
/// why not.
fn answer(
    alertd: &Alertd,
    who: &Who,
    request: &Request,
    out: &mut impl Write,
) -> Result<(), String> {
    let source = who.source.as_str();
    let ok = |out: &mut dyn Write| {
        line(out, &Done { ok: true }).map_err(|err| format!("cannot answer: {err}"))
    };
    match request.ask.as_str() {
        "raise" => {
            alertd.raise(source, checked(request.alert.as_ref(), "raise")?)?;
            ok(out)
        }
        "event" => {
            alertd.event(source, checked(request.alert.as_ref(), "event")?)?;
            ok(out)
        }
        "clear" => {
            let key = request.key.as_deref().ok_or("clear needs a key")?;
            alertd.clear(source, key)?;
            ok(out)
        }
        "set" => {
            let alerts = request.alerts.as_ref().ok_or("set needs alerts, even an empty list")?;
            let valid: Result<Vec<Alert>, String> =
                alerts.iter().map(|alert| checked(Some(alert), "set")).collect();
            alertd.set(source, valid?)?;
            ok(out)
        }
        "list" => lines(out, &alertd.active()),
        "history" => {
            let count = request.count.unwrap_or(HISTORY_DEFAULT).min(HISTORY_MAX);
            lines(out, &alertd.history(count))
        }
        "test" => lines(out, &alertd.test(source)),
        "status" => lines(out, &alertd.status()),
        other => Err(format!("alertd does not know '{other}'")),
    }
}

/// `alert`, which a request must carry, and which must be valid.
fn checked(alert: Option<&Alert>, ask: &str) -> Result<Alert, String> {
    let alert = alert.ok_or_else(|| format!("{ask} needs an alert"))?;
    match alert.invalid() {
        Some(why) => Err(why),
        None => Ok(alert.clone()),
    }
}

/// Writes each of `items` as a line, then the end.
fn lines(out: &mut impl Write, items: &[impl Serialize]) -> Result<(), String> {
    let written = items
        .iter()
        .try_for_each(|item| line(out, item))
        .and_then(|()| line(out, &End { end: true }));
    written.map_err(|err| format!("cannot answer: {err}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread::JoinHandle;
    use std::time::Instant;

    use super::super::tests::{fixture, warning};
    use crate::alerts::protocol::{Active, Change, Channel, Severity, State};

    fn root() -> Result<Who, String> {
        Ok(Who { source: "backup".into() })
    }

    /// Serves one request, sent as `text`, for `who` on a thread, and
    /// gives the lines of the answer.
    fn ask(alertd: &Arc<Alertd>, who: Result<Who, String>, text: &[u8]) -> Vec<String> {
        let (client, server) = UnixStream::pair().unwrap();
        let alertd = Arc::clone(alertd);
        let thread: JoinHandle<()> = thread::spawn(move || serve(&alertd, &server, who));
        (&client).write_all(text).unwrap();
        let mut answer = String::new();
        BufReader::new(&client).read_to_string(&mut answer).unwrap();
        thread.join().unwrap();
        answer.lines().map(str::to_string).collect()
    }

    fn ask_json(alertd: &Arc<Alertd>, request: &Request) -> Vec<String> {
        let mut text = serde_json::to_string(request).unwrap();
        text.push('\n');
        ask(alertd, root(), text.as_bytes())
    }

    fn request(ask: &str) -> Request {
        Request { ask: ask.into(), ..Request::default() }
    }

    fn raise(key: &str, severity: Severity) -> Request {
        let alert = Alert { severity, ..warning(key) };
        Request { alert: Some(alert), ..request("raise") }
    }

    #[test]
    fn a_raise_is_answered_ok_and_then_listed_with_its_source() {
        let (alertd, _screen, _dir) = fixture("socket-raise", Vec::new());
        assert_eq!(
            ask_json(&alertd, &raise("backup-late", Severity::Critical)),
            [r#"{"ok":true}"#]
        );
        let listed = ask_json(&alertd, &request("list"));
        assert_eq!(listed.len(), 2);
        let active: Active = serde_json::from_str(&listed[0]).unwrap();
        assert_eq!((active.source.as_str(), active.alert.key.as_str()), ("backup", "backup-late"));
        assert_eq!(listed[1], r#"{"end":true}"#);
    }

    #[test]
    fn an_event_is_told_and_is_not_active_and_a_set_clears_only_the_senders_others() {
        let (alertd, _screen, _dir) = fixture("socket-set", Vec::new());
        let event = Request { alert: Some(warning("fell-back")), ..request("event") };
        assert_eq!(ask_json(&alertd, &event), [r#"{"ok":true}"#]);
        assert_eq!(ask_json(&alertd, &request("list")), [r#"{"end":true}"#]);
        alertd.raise("other", warning("theirs")).unwrap();
        ask_json(&alertd, &raise("mine", Severity::Warning));
        let set = Request { alerts: Some(vec![warning("new")]), ..request("set") };
        assert_eq!(ask_json(&alertd, &set), [r#"{"ok":true}"#]);
        let keys: Vec<String> = alertd
            .active()
            .into_iter()
            .map(|held| format!("{}:{}", held.source, held.alert.key))
            .collect();
        assert_eq!(keys.len(), 2);
        assert!(
            keys.contains(&"other:theirs".to_string()) && keys.contains(&"backup:new".to_string())
        );
    }

    #[test]
    fn a_clear_of_an_alert_that_is_not_active_is_ok() {
        let (alertd, _screen, _dir) = fixture("socket-clear", Vec::new());
        let clear = Request { key: Some("not-there".into()), ..request("clear") };
        assert_eq!(ask_json(&alertd, &clear), [r#"{"ok":true}"#]);
    }

    #[test]
    fn the_history_is_the_newest_changes_oldest_first_and_twenty_without_a_count() {
        let (alertd, _screen, _dir) = fixture("socket-history", Vec::new());
        for n in 0..25 {
            alertd.event("s", warning(&format!("k{n}"))).unwrap();
        }
        let all = ask_json(&alertd, &request("history"));
        assert_eq!(all.len(), 21);
        let first: Change = serde_json::from_str(&all[0]).unwrap();
        assert_eq!(first.alert.key, "k5");
        let two = ask_json(&alertd, &Request { count: Some(2), ..request("history") });
        let last: Change = serde_json::from_str(&two[1]).unwrap();
        assert_eq!((two.len(), last.alert.key.as_str(), last.state), (3, "k24", State::Event));
    }

    #[test]
    fn status_and_test_with_no_channels_have_only_the_end() {
        let (alertd, _screen, _dir) = fixture("socket-none", Vec::new());
        assert_eq!(ask_json(&alertd, &request("status")), [r#"{"end":true}"#]);
        assert_eq!(ask_json(&alertd, &request("test")), [r#"{"end":true}"#]);
    }

    #[test]
    fn status_has_a_line_for_each_channel() {
        let channels = vec![super::super::tests::json_channel(1)];
        let (alertd, _screen, _dir) = fixture("socket-status", channels);
        let lines = ask_json(&alertd, &request("status"));
        let channel: Channel = serde_json::from_str(&lines[0]).unwrap();
        assert_eq!((channel.channel.as_str(), channel.waiting), ("json (127.0.0.1)", 0));
    }

    fn refusal(lines: &[String]) -> String {
        assert_eq!(lines.len(), 1, "{lines:?}");
        serde_json::from_str::<Refusal>(&lines[0]).unwrap().error
    }

    #[test]
    fn someone_who_is_not_root_is_refused_and_nothing_is_changed() {
        let (alertd, _screen, _dir) = fixture("socket-user", Vec::new());
        let text = serde_json::to_string(&raise("k", Severity::Warning)).unwrap() + "\n";
        let lines = ask(&alertd, Err(NOT_ROOT.into()), text.as_bytes());
        assert_eq!(refusal(&lines), NOT_ROOT);
        assert!(alertd.active().is_empty());
    }

    #[test]
    fn a_request_that_cannot_be_taken_is_refused_in_plain_words() {
        let (alertd, _screen, _dir) = fixture("socket-bad", Vec::new());
        let says = |text: &[u8]| refusal(&ask(&alertd, root(), text));
        assert!(says(b"not json\n").starts_with("not a request: "));
        assert_eq!(says(b"{\"ask\":\"frobnicate\"}\n"), "alertd does not know 'frobnicate'");
        assert_eq!(says(b"{\"ask\":\"raise\"}\n"), "raise needs an alert");
        assert_eq!(says(b"{\"ask\":\"clear\"}\n"), "clear needs a key");
        assert_eq!(says(b"{\"ask\":\"set\"}\n"), "set needs alerts, even an empty list");
        assert!(says(b"\n").starts_with("not a request: "));
        let upper = Request { alert: Some(warning("Upper")), ..request("raise") };
        let text = serde_json::to_string(&upper).unwrap() + "\n";
        assert!(says(text.as_bytes()).contains("is not 1 to 128 of a-z"));
        // One bad alert in a set refuses the whole set.
        let set = Request { alerts: Some(vec![warning("fine"), warning("Bad")]), ..request("set") };
        let text = serde_json::to_string(&set).unwrap() + "\n";
        assert!(says(text.as_bytes()).contains("is not 1 to 128 of a-z"));
        assert!(alertd.active().is_empty());
    }

    #[test]
    fn a_request_longer_than_16_kib_is_refused() {
        let (alertd, _screen, _dir) = fixture("socket-long", Vec::new());
        let mut text = vec![b'x'; REQUEST_MAX + 100];
        text.push(b'\n');
        assert_eq!(
            refusal(&ask(&alertd, root(), &text)),
            format!("the request is longer than {REQUEST_MAX} bytes")
        );
    }

    #[test]
    fn a_request_without_an_end_of_line_is_refused_when_the_other_end_stops() {
        let (alertd, _screen, _dir) = fixture("socket-noeol", Vec::new());
        let (client, server) = UnixStream::pair().unwrap();
        let alertd2 = Arc::clone(&alertd);
        let thread = thread::spawn(move || serve(&alertd2, &server, root()));
        (&client).write_all(b"{\"ask\":\"list\"}").unwrap();
        client.shutdown(std::net::Shutdown::Write).unwrap();
        let mut answer = String::new();
        BufReader::new(&client).read_to_string(&mut answer).unwrap();
        thread.join().unwrap();
        assert!(answer.contains("the request has no end of line"), "{answer}");
    }

    #[test]
    fn a_request_that_does_not_come_is_given_up_on_after_five_seconds() {
        let (alertd, _screen, _dir) = fixture("socket-slow", Vec::new());
        let (client, server) = UnixStream::pair().unwrap();
        let started = Instant::now();
        let alertd2 = Arc::clone(&alertd);
        let thread = thread::spawn(move || serve(&alertd2, &server, root()));
        let mut answer = String::new();
        BufReader::new(&client).read_to_string(&mut answer).unwrap();
        thread.join().unwrap();
        assert!(answer.contains("no request within 5 seconds"), "{answer}");
        assert!(started.elapsed() < REQUEST_TIME + Duration::from_secs(2));
    }

    #[test]
    fn a_seventeenth_connection_is_told_alertd_is_busy() {
        let (alertd, _screen, _dir) = fixture("socket-busy", Vec::new());
        alertd.serving.store(CONNECTIONS_MAX, Ordering::Release);
        let (client, server) = UnixStream::pair().unwrap();
        admit(&alertd, server, root());
        let mut answer = String::new();
        BufReader::new(&client).read_to_string(&mut answer).unwrap();
        assert_eq!(refusal(&answer.lines().map(str::to_string).collect::<Vec<_>>()), BUSY);
        assert_eq!(alertd.serving.load(Ordering::Acquire), CONNECTIONS_MAX);
    }

    #[test]
    fn those_who_may_not_ask_never_take_the_threads_of_those_who_may() {
        let (alertd, _screen, _dir) = fixture("socket-not-root", Vec::new());
        // Their own few are taken: the next is refused at once, unread,
        // and root's are all still free.
        alertd.refusing.store(REFUSING_MAX, Ordering::Release);
        let (client, server) = UnixStream::pair().unwrap();
        let started = Instant::now();
        admit(&alertd, server, Err(NOT_ROOT.into()));
        let mut answer = String::new();
        BufReader::new(&client).read_to_string(&mut answer).unwrap();
        assert_eq!(refusal(&answer.lines().map(str::to_string).collect::<Vec<_>>()), NOT_ROOT);
        assert!(started.elapsed() < Duration::from_secs(2));
        assert_eq!(alertd.refusing.load(Ordering::Acquire), REFUSING_MAX);
        assert_eq!(alertd.serving.load(Ordering::Acquire), 0);
        // One of them while there is room is read, then refused, on a
        // thread counted with theirs.
        alertd.refusing.store(0, Ordering::Release);
        let (mut client, server) = UnixStream::pair().unwrap();
        admit(&alertd, server, Err(NOT_ROOT.into()));
        client.write_all(b"{\"ask\":\"list\"}\n").unwrap();
        let mut answer = String::new();
        BufReader::new(&client).read_to_string(&mut answer).unwrap();
        assert_eq!(refusal(&answer.lines().map(str::to_string).collect::<Vec<_>>()), NOT_ROOT);
        assert_eq!(alertd.serving.load(Ordering::Acquire), 0);
    }
}
