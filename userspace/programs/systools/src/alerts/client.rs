// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! How a service raises, clears and sets its alerts (ADR 0485).
//!
//! Each call connects to alertd, sends one request and reads its answer,
//! waiting at most [`PATIENCE`] in all. A service that raises alerts
//! does not depend on alertd: when alertd is not running, or does not
//! answer in time, the call says why, and the caller logs it and carries
//! on. A source that knows its whole list of problems at once, as
//! metricsd does after each step, sends it with [`set`], which also
//! clears what has gone, after a restart of either side too.

use std::io::{self, BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::thread;
use std::time::{Duration, Instant};

use super::protocol::{Alert, Done, Refusal, Request, SOCKET};
use crate::deadline::Deadline;

/// The longest a call waits for alertd to take a request.
pub const PATIENCE: Duration = Duration::from_secs(2);

/// What a call says when nothing answers on alertd's socket.
const NOT_RUNNING: &str = "alertd is not running";

/// Raises `alert`: alertd tells the owner when it is new, or when it
/// goes from warning to critical, and keeps it active until it is
/// cleared.
///
/// # Errors
///
/// Why alertd did not take it, as a line for the caller's log.
pub fn raise(alert: &Alert) -> Result<(), String> {
    send(&Request { ask: "raise".into(), alert: Some(alert.clone()), ..Request::default() })
}

/// Whether the sender of an event shows it on the consoles itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Screen {
    /// alertd tells the consoles of it, as of every change.
    Tell,
    /// The sender has shown it there in its own words, which alertd's
    /// notification would replace on the screen's bottom line: alertd
    /// sends it to the channels and keeps it in the history only.
    Shown,
}

/// Tells the owner of `alert` once, as an event that is never active: a
/// fallback to the previous version, a restart after a kernel panic.
///
/// # Errors
///
/// As [`raise`].
pub fn event(alert: &Alert, screen: Screen) -> Result<(), String> {
    send(&event_request(alert, screen))
}

/// Tells of `alert` as [`event`] does, from a program that runs as the
/// machine starts, perhaps before alertd: while alertd is not running,
/// it tries again each second for up to `within`.
///
/// # Errors
///
/// As [`raise`], once `within` has passed.
pub fn event_at_start(alert: &Alert, screen: Screen, within: Duration) -> Result<(), String> {
    patiently(Path::new(SOCKET), &event_request(alert, screen), within)
}

fn event_request(alert: &Alert, screen: Screen) -> Request {
    Request {
        ask: "event".into(),
        alert: Some(alert.clone()),
        shown: screen == Screen::Shown,
        ..Request::default()
    }
}

/// Sends `request` as [`send_to`] does, again each second for up to
/// `within` while alertd is not running.
fn patiently(socket: &Path, request: &Request, within: Duration) -> Result<(), String> {
    let until = Instant::now() + within;
    loop {
        match send_to(socket, request) {
            Err(why) if why == NOT_RUNNING && Instant::now() < until => {
                thread::sleep(Duration::from_secs(1));
            }
            done => return done,
        }
    }
}

/// Clears the caller's alert `key`, if it is active.
///
/// # Errors
///
/// As [`raise`].
pub fn clear(key: &str) -> Result<(), String> {
    send(&Request { ask: "clear".into(), key: Some(key.into()), ..Request::default() })
}

/// Sets the caller's alerts to `alerts`: each is raised as [`raise`]
/// does, and each of the caller's active alerts not among them is
/// cleared. An empty list clears them all.
///
/// # Errors
///
/// As [`raise`].
pub fn set(alerts: &[Alert]) -> Result<(), String> {
    send(&Request { ask: "set".into(), alerts: Some(alerts.to_vec()), ..Request::default() })
}

fn send(request: &Request) -> Result<(), String> {
    send_to(Path::new(SOCKET), request)
}

/// Sends `request` to the alertd answering at `socket`.
fn send_to(socket: &Path, request: &Request) -> Result<(), String> {
    let answer = exchange(socket, request).map_err(|err| match err.kind() {
        io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused => NOT_RUNNING.to_string(),
        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut => {
            format!("alertd did not answer within {} seconds", PATIENCE.as_secs())
        }
        _ => format!("cannot reach alertd at {}: {err}", socket.display()),
    })?;
    if let Ok(refusal) = serde_json::from_str::<Refusal>(&answer) {
        return Err(format!("alertd refused it: {}", refusal.error));
    }
    match serde_json::from_str::<Done>(&answer) {
        Ok(Done { ok: true }) => Ok(()),
        _ => Err(format!("alertd answered {:?}", answer.trim_end())),
    }
}

/// Sends `request` and reads the one line that answers it.
fn exchange(socket: &Path, request: &Request) -> io::Result<String> {
    let stream = UnixStream::connect(socket)?;
    let mut timed = Deadline::new(&stream, PATIENCE)?;
    let mut line = serde_json::to_string(request).map_err(io::Error::other)?;
    line.push('\n');
    timed.write_all(line.as_bytes())?;
    let mut answer = String::new();
    BufReader::new(timed).read_line(&mut answer)?;
    if answer.is_empty() {
        return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "alertd closed the connection"));
    }
    Ok(answer)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::alerts::protocol::Severity;
    use std::os::unix::net::UnixListener;
    use std::path::PathBuf;
    use std::thread::{self, JoinHandle};

    /// A socket path of the test's own.
    fn socket(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("alerts-client-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("socket")
    }

    /// A one-request alertd that answers `answer`, or nothing when it is
    /// [`None`], and gives back the request it read.
    fn alertd(path: &Path, answer: Option<&'static str>) -> JoinHandle<String> {
        let listener = UnixListener::bind(path).unwrap();
        thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut request = String::new();
            BufReader::new(stream.try_clone().unwrap()).read_line(&mut request).unwrap();
            match answer {
                Some(answer) => (&stream).write_all(answer.as_bytes()).unwrap(),
                None => thread::sleep(PATIENCE + Duration::from_millis(500)),
            }
            request
        })
    }

    fn full() -> Alert {
        Alert {
            key: "fs-full:/data".into(),
            severity: Severity::Critical,
            title: "/data is 97% full".into(),
            detail: String::new(),
        }
    }

    #[test]
    fn a_raise_is_one_line_and_ok_is_taken() {
        let path = socket("raise");
        let server = alertd(&path, Some("{\"ok\":true}\n"));
        let request = Request { ask: "raise".into(), alert: Some(full()), ..Request::default() };
        assert_eq!(send_to(&path, &request), Ok(()));
        let sent: Request = serde_json::from_str(&server.join().unwrap()).unwrap();
        assert_eq!(sent, request);
    }

    #[test]
    fn a_refusal_is_given_in_alertds_words() {
        let path = socket("refused");
        let server = alertd(
            &path,
            Some("{\"error\":\"only root and the system's services raise and read alerts\"}\n"),
        );
        let request = Request { ask: "clear".into(), key: Some("k".into()), ..Request::default() };
        assert_eq!(
            send_to(&path, &request),
            Err("alertd refused it: only root and the system's services raise and read alerts"
                .into())
        );
        server.join().unwrap();
    }

    #[test]
    fn no_alertd_is_said_plainly() {
        let path = socket("none");
        let request = Request { ask: "set".into(), alerts: Some(Vec::new()), ..Request::default() };
        assert_eq!(send_to(&path, &request), Err("alertd is not running".into()));
    }

    #[test]
    fn an_alertd_that_does_not_answer_is_waited_for_two_seconds() {
        let path = socket("silent");
        let server = alertd(&path, None);
        let request = Request { ask: "set".into(), alerts: Some(Vec::new()), ..Request::default() };
        let started = std::time::Instant::now();
        assert_eq!(send_to(&path, &request), Err("alertd did not answer within 2 seconds".into()));
        assert!(started.elapsed() < PATIENCE + Duration::from_millis(400));
        server.join().unwrap();
    }

    #[test]
    fn an_answer_that_is_not_ok_is_quoted() {
        let path = socket("odd");
        let server = alertd(&path, Some("{\"ok\":false}\n"));
        let request = Request { ask: "clear".into(), key: Some("k".into()), ..Request::default() };
        assert_eq!(send_to(&path, &request), Err("alertd answered \"{\\\"ok\\\":false}\"".into()));
        server.join().unwrap();
    }

    #[test]
    fn an_event_at_start_waits_for_alertd_to_come() {
        let path = socket("later");
        let request = event_request(&full(), Screen::Shown);
        // Nothing comes: it gives up once the time has passed.
        let started = std::time::Instant::now();
        assert_eq!(
            patiently(&path, &request, Duration::from_millis(1500)),
            Err("alertd is not running".into())
        );
        assert!(started.elapsed() >= Duration::from_millis(1500));
        // alertd comes a moment later: the event reaches it.
        let later = path.clone();
        let server = thread::spawn(move || {
            thread::sleep(Duration::from_millis(1200));
            alertd(&later, Some("{\"ok\":true}\n")).join().unwrap()
        });
        assert_eq!(patiently(&path, &request, Duration::from_secs(5)), Ok(()));
        let sent = server.join().unwrap();
        assert!(sent.contains("\"shown\":true"), "{sent}");
        assert_eq!(serde_json::from_str::<Request>(&sent).unwrap(), request);
    }

    #[test]
    fn an_event_alertd_tells_the_consoles_of_says_nothing_of_the_screen() {
        let told = serde_json::to_string(&event_request(&full(), Screen::Tell)).unwrap();
        assert!(!told.contains("shown"), "{told}");
        let request: Request = serde_json::from_str(&told).unwrap();
        assert!(!request.shown);
    }
}
