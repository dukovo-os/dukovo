// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Asking reportd (ADR 0531): connect to its socket, send one request as
//! a line of JSON, read the lines of the answer until reportd closes the
//! connection, all within a time the caller names.
//!
//! reportd is a service that starts with the system, so a missing socket
//! is the common failure and has its own words, with the command that
//! starts it. A line that is a refusal, `{"error": ...}` and nothing
//! else, ends the question with reportd's own reason: a status line may
//! carry an `error` of its own beside its other fields, and is not one.

use std::io::{self, BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::time::Duration;

use serde::de::DeserializeOwned;
use serde_json::Value;

use super::protocol::{Done, End, REQUEST_MAX, Refusal, Request, SOCKET};
use crate::deadline::Deadline;
use crate::util::{Fail, fail};

/// The time for what reportd answers from its own records: crashes, the
/// queue, a draft, a redaction, a delete.
pub const QUICK: Duration = Duration::from_secs(20);

/// The time for what reportd answers after asking the reports server, as
/// `send`, `retry`, `status` and `delete` do. reportd gives up on the
/// server for one report after about 30 seconds, queues what it could not
/// send, and answers within 120 seconds at the latest; the client waits a
/// little longer, so that it hears reportd's answer and not its own
/// silence.
pub const ONLINE: Duration = Duration::from_secs(150);

/// The longest line read from reportd: a drafted text of up to
/// [`super::protocol::TEXT_MAX`] bytes with every one escaped.
const LINE_MAX: usize = REQUEST_MAX * 3;

/// What is said when nothing answers on reportd's socket.
const NOT_RUNNING: &str = "reportd is not running: an administrator starts it with \
                           elevate service reportd start";

/// A way to reportd's socket.
pub struct Client {
    socket: PathBuf,
}

impl Client {
    /// The reportd of this system.
    pub fn new() -> Client {
        Client { socket: PathBuf::from(SOCKET) }
    }

    /// The reportd that answers at `socket`, as a test's own does.
    #[cfg(test)]
    pub fn at(socket: &std::path::Path) -> Client {
        Client { socket: socket.to_path_buf() }
    }

    /// Asks `request`, whose answer is one line: its `T`.
    ///
    /// # Errors
    ///
    /// Why reportd could not be asked, did not answer in `wait`, or
    /// refused: in its words.
    pub fn one<T: DeserializeOwned>(&self, request: &Request, wait: Duration) -> Result<T, Fail> {
        let lines = self.lines(request, wait, false)?;
        match lines.into_iter().next() {
            Some(value) => from_line(value),
            None => Err(fail("reportd closed the connection without an answer".into())),
        }
    }

    /// Asks `request`, whose answer is a line for each item and then the
    /// end line: the items.
    ///
    /// # Errors
    ///
    /// As [`Client::one`], and if the answer stops before its end.
    pub fn list<T: DeserializeOwned>(
        &self,
        request: &Request,
        wait: Duration,
    ) -> Result<Vec<T>, Fail> {
        self.lines(request, wait, true)?.into_iter().map(from_line).collect()
    }

    /// Asks `request`, whose answer is that it was done.
    ///
    /// # Errors
    ///
    /// As [`Client::one`], and if the answer is not that it was done.
    pub fn done(&self, request: &Request, wait: Duration) -> Result<(), Fail> {
        let done: Done = self.one(request, wait)?;
        if done.ok { Ok(()) } else { Err(fail("reportd did not do it".into())) }
    }

    /// The lines of the answer to `request`, without the end line of a
    /// list, which `list` says it wants.
    fn lines(&self, request: &Request, wait: Duration, list: bool) -> Result<Vec<Value>, Fail> {
        self.exchange(request, wait, list).map_err(|failure| match failure {
            Trouble::Refused(why) => fail(why),
            Trouble::Io(err) => match err.kind() {
                io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused => {
                    fail(NOT_RUNNING.into())
                }
                io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut => {
                    fail(format!("reportd did not answer within {} seconds", wait.as_secs()))
                }
                _ => fail(format!("cannot ask reportd at {}: {err}", self.socket.display())),
            },
        })
    }

    fn exchange(
        &self,
        request: &Request,
        wait: Duration,
        list: bool,
    ) -> Result<Vec<Value>, Trouble> {
        let stream = UnixStream::connect(&self.socket)?;
        let mut timed = Deadline::new(&stream, wait)?;
        let mut line = serde_json::to_string(request).map_err(io::Error::other)?;
        line.push('\n');
        timed.write_all(line.as_bytes())?;
        let mut reader = BufReader::new(timed);
        let mut answer = Vec::new();
        loop {
            let mut text = String::new();
            let length = reader.by_ref().take(LINE_MAX as u64).read_line(&mut text)?;
            if length == 0 {
                break;
            }
            if !text.ends_with('\n') && length >= LINE_MAX {
                return Err(io::Error::new(io::ErrorKind::InvalidData, "an answer too long").into());
            }
            let value: Value = serde_json::from_str(&text).map_err(|err| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("an answer that is no JSON: {err}"),
                )
            })?;
            if let Some(why) = refusal(&value) {
                return Err(Trouble::Refused(why));
            }
            if list && is_end(&value) {
                return Ok(answer);
            }
            answer.push(value);
            if !list {
                return Ok(answer);
            }
        }
        if list {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "reportd closed the connection before the end of its answer",
            )
            .into());
        }
        Ok(answer)
    }
}

/// Why a question was not answered.
enum Trouble {
    /// reportd refused it, for the reason it gave.
    Refused(String),
    /// The connection failed.
    Io(io::Error),
}

impl From<io::Error> for Trouble {
    fn from(err: io::Error) -> Trouble {
        Trouble::Io(err)
    }
}

impl Default for Client {
    fn default() -> Client {
        Client::new()
    }
}

/// Whether `value` is an object of one field, as a refusal and the end
/// line are.
fn has_one_field(value: &Value) -> bool {
    value.as_object().is_some_and(|object| object.len() == 1)
}

/// The reason in a refusal, `{"error": "..."}` and no more.
fn refusal(value: &Value) -> Option<String> {
    if !has_one_field(value) {
        return None;
    }
    serde_json::from_value::<Refusal>(value.clone()).ok().map(|refusal| refusal.error)
}

/// Whether `value` is the line after a list's last item, `{"end": true}`.
fn is_end(value: &Value) -> bool {
    has_one_field(value) && serde_json::from_value::<End>(value.clone()).is_ok_and(|end| end.end)
}

/// An answer line as the `T` it should be.
fn from_line<T: DeserializeOwned>(value: Value) -> Result<T, Fail> {
    serde_json::from_value(value)
        .map_err(|err| fail(format!("reportd gave an answer that is not what was asked: {err}")))
}

#[cfg(test)]
mod tests {
    use super::super::protocol::{Crash, Drafted, Kind, Status};
    use super::*;
    use std::os::unix::net::UnixListener;
    use std::thread::{self, JoinHandle};

    /// A reportd that answers one request with `lines`, then closes, and
    /// the request it was sent.
    fn scripted(name: &str, lines: &'static [&'static str]) -> (PathBuf, JoinHandle<String>) {
        let dir = std::env::temp_dir().join(format!("report-client-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let socket = dir.join("socket");
        let listener = UnixListener::bind(&socket).unwrap();
        let served = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = String::new();
            BufReader::new(&stream).read_line(&mut request).unwrap();
            for line in lines {
                stream.write_all(format!("{line}\n").as_bytes()).unwrap();
            }
            request
        });
        (socket, served)
    }

    fn ask(command: &str) -> Request {
        Request { ask: command.into(), ..Request::default() }
    }

    const CRASH: &str = r#"{"id":"20261113T100211-4012","kind":"signal","program":"nginx","user":0,"at":1,"what":"SIGSEGV","signature":"3fa2c4d19e0b7a61","seen":1}"#;

    #[test]
    fn a_request_is_one_line_and_one_line_answers_it() {
        let (socket, served) =
            scripted("one", &[r#"{"text":"Dukovo problem report\n","cuts":2,"has_logs":true}"#]);
        let request =
            Request { ask: "redact".into(), text: Some("hello".into()), ..Request::default() };
        let drafted: Drafted = Client::at(&socket).one(&request, QUICK).unwrap();
        assert_eq!(
            drafted,
            Drafted { text: "Dukovo problem report\n".into(), cuts: 2, has_logs: true }
        );
        assert_eq!(served.join().unwrap(), "{\"ask\":\"redact\",\"text\":\"hello\"}\n");
    }

    #[test]
    fn a_list_is_its_lines_up_to_the_end_line() {
        let lines: &'static [&'static str] = Box::leak(Box::new([CRASH, CRASH, r#"{"end":true}"#]));
        let (socket, served) = scripted("list", lines);
        let crashes: Vec<Crash> = Client::at(&socket).list(&ask("crashes"), QUICK).unwrap();
        assert_eq!(crashes.len(), 2);
        assert_eq!((crashes[0].kind, crashes[0].program.as_str()), (Kind::Signal, "nginx"));
        served.join().unwrap();
    }

    #[test]
    fn an_empty_list_is_the_end_line_alone() {
        let (socket, served) = scripted("empty", &[r#"{"end":true}"#]);
        let crashes: Vec<Crash> = Client::at(&socket).list(&ask("crashes"), QUICK).unwrap();
        assert!(crashes.is_empty());
        served.join().unwrap();
    }

    #[test]
    fn a_refusal_is_reportds_reason_whether_it_comes_first_or_later() {
        let (socket, served) =
            scripted("refused", &[r#"{"error":"only an administrator reports a failed start"}"#]);
        let err = Client::at(&socket).one::<Drafted>(&ask("draft"), QUICK).unwrap_err();
        assert_eq!(err.message, "only an administrator reports a failed start");
        served.join().unwrap();
        let lines: &'static [&'static str] =
            Box::leak(Box::new([CRASH, r#"{"error":"the list broke"}"#]));
        let (socket, served) = scripted("late", lines);
        let err = Client::at(&socket).list::<Crash>(&ask("crashes"), QUICK).unwrap_err();
        assert_eq!(err.message, "the list broke");
        served.join().unwrap();
    }

    #[test]
    fn a_status_with_an_error_of_its_own_is_not_a_refusal() {
        let line = r#"{"number":7,"title":"Disk","sent":1,"error":"the server did not answer","security":false}"#;
        let lines: &'static [&'static str] = Box::leak(Box::new([line, r#"{"end":true}"#]));
        let (socket, served) = scripted("status", lines);
        let all: Vec<Status> = Client::at(&socket).list(&ask("status"), QUICK).unwrap();
        assert_eq!(all[0].error.as_deref(), Some("the server did not answer"));
        served.join().unwrap();
    }

    #[test]
    fn done_is_a_line_that_says_so() {
        let (socket, served) = scripted("done", &[r#"{"ok":true}"#]);
        Client::at(&socket).done(&ask("delete"), QUICK).unwrap();
        served.join().unwrap();
        let (socket, served) = scripted("not-done", &[r#"{"ok":false}"#]);
        assert!(Client::at(&socket).done(&ask("delete"), QUICK).is_err());
        served.join().unwrap();
        assert_eq!(serde_json::from_str::<Done>(r#"{"ok":true}"#).unwrap(), Done { ok: true });
    }

    #[test]
    fn a_list_that_stops_before_its_end_is_an_error() {
        let lines: &'static [&'static str] = Box::leak(Box::new([CRASH]));
        let (socket, served) = scripted("cut", lines);
        let err = Client::at(&socket).list::<Crash>(&ask("crashes"), QUICK).unwrap_err();
        assert!(err.message.contains("before the end"), "{}", err.message);
        served.join().unwrap();
    }

    #[test]
    fn an_answer_of_the_wrong_shape_is_said_so() {
        let (socket, served) = scripted("shape", &[r#"{"unexpected":1}"#]);
        let err = Client::at(&socket).one::<Drafted>(&ask("draft"), QUICK).unwrap_err();
        assert!(err.message.contains("not what was asked"), "{}", err.message);
        served.join().unwrap();
    }

    #[test]
    fn no_socket_says_reportd_is_not_running() {
        let missing =
            std::env::temp_dir().join(format!("report-client-{}-none", std::process::id()));
        let err =
            Client::at(&missing.join("socket")).one::<Drafted>(&ask("draft"), QUICK).unwrap_err();
        assert!(err.message.starts_with("reportd is not running"), "{}", err.message);
        assert!(err.message.contains("elevate service reportd start"));
    }

    #[test]
    fn a_reportd_that_does_not_answer_is_given_up_on() {
        let dir = std::env::temp_dir().join(format!("report-client-{}-silent", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let socket = dir.join("socket");
        let listener = UnixListener::bind(&socket).unwrap();
        let silent = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            // Held open until the client has given up, and says nothing.
            let _ = stream.read(&mut [0u8; 1024]);
            let _ = stream.read(&mut [0u8; 1]);
        });
        let err = Client::at(&socket)
            .one::<Drafted>(&ask("draft"), Duration::from_millis(300))
            .unwrap_err();
        assert!(err.message.starts_with("reportd did not answer within"), "{}", err.message);
        silent.join().unwrap();
    }
}
