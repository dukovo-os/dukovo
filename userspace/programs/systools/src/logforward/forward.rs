// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! What one destination of logforward(8) does, in a thread of its own so
//! that a server that is down holds up no other (ADR 0482): connect, read
//! the log from the destination's position, send it, and keep the
//! position and the status.
//!
//! A connection that fails is tried again after 1, 2, 4 ... 60 seconds,
//! and the log waits in logd's day files meanwhile. After a failure
//! sending starts again from the last position that counts as delivered
//! ([`Progress`]), so that a server may get the last seconds twice and
//! never misses any. Each failure is said in the log once, until the error
//! changes.
//!
//! When something misbehaves, `logforward status` says where each
//! destination stands and why, and the service's lines in the log
//! (`log logforward`) say what failed. The `log-forward` boot test covers
//! the thread against TLS, TCP and UDP servers.

use std::io;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use super::connection::Connection;
use super::message;
use super::position::{self, Position, Progress, SETTLE};
use super::reader::{self, Batch, Reader};
use super::settings::Destination;
use super::status::{self, State, Status};
use crate::logd;
use crate::util;

/// How long the thread waits when the log has nothing new.
const IDLE: Duration = Duration::from_secs(1);
/// How often at least the connection is looked at, the position saved and
/// the status written.
const TICK: Duration = Duration::from_secs(1);
/// How long a wait lasts before it looks again at whether the thread is to
/// end.
const STEP: Duration = Duration::from_millis(100);
/// The wait after the first failed attempt.
const BACKOFF_FIRST: Duration = Duration::from_secs(1);
/// The longest wait between attempts.
const BACKOFF_LAST: Duration = Duration::from_secs(60);

/// The folders a destination's thread works in.
#[derive(Clone, Debug)]
pub struct Places {
    /// The log's day files, which it reads.
    pub log: PathBuf,
    /// Where the positions are saved.
    pub positions: PathBuf,
    /// Where the status files are written.
    pub status: PathBuf,
}

impl Places {
    /// The folders the service uses.
    pub fn system() -> Places {
        Places {
            log: PathBuf::from(logd::DIRECTORY),
            positions: PathBuf::from(position::DIRECTORY),
            status: PathBuf::from(status::DIRECTORY),
        }
    }
}

/// Sends the log to `destination` until `stop` is set, then saves the
/// position that counts as delivered and ends.
pub fn run(destination: &Destination, places: &Places, stop: &AtomicBool) {
    let mut forwarder = Forwarder::new(destination, places, stop);
    forwarder.serve();
    forwarder.finish();
}

/// What a failure last said, so that each change of it is said once and
/// not at each try.
#[derive(Default)]
struct Said(Option<String>);

impl Said {
    /// Whether `error` is not what was said last; it is what was said
    /// from now on.
    fn is_new(&mut self, error: &str) -> bool {
        if self.0.as_deref() == Some(error) {
            return false;
        }
        self.0 = Some(error.to_string());
        true
    }

    /// The failure is over: a later one is said again.
    fn clear(&mut self) {
        self.0 = None;
    }
}

/// How long to wait before the next attempt to connect.
struct Backoff {
    next: Duration,
}

impl Backoff {
    fn new() -> Backoff {
        Backoff { next: BACKOFF_FIRST }
    }

    /// How long to wait now; the wait after it doubles, up to a minute.
    fn wait(&mut self) -> Duration {
        let now = self.next;
        self.next = (self.next * 2).min(BACKOFF_LAST);
        now
    }

    fn reset(&mut self) {
        self.next = BACKOFF_FIRST;
    }
}

/// Sleeps for `total`, or until `stop` is set: whether it slept all of it.
fn pause(stop: &AtomicBool, total: Duration) -> bool {
    let end = Instant::now() + total;
    while !stop.load(Ordering::Relaxed) {
        let left = end.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return true;
        }
        thread::sleep(left.min(STEP));
    }
    false
}

/// Where a destination with no saved position starts: after the log as it
/// is, so that it is sent the log from now on. The position is saved at
/// once, so that a restart before anything is delivered does not move it.
fn begin(name: &str, places: &Places) -> Position {
    let start = reader::end_of_log(&places.log);
    if let Err(error) = start.save(&places.positions, name) {
        eprintln!("<3>logforward: {name}: {}", cannot_save(places, &error));
    }
    start
}

fn cannot_save(places: &Places, error: &io::Error) -> String {
    format!(
        "cannot save its position in {}: {}",
        places.positions.display(),
        util::strip_os_error(error)
    )
}

/// One destination's thread.
struct Forwarder<'a> {
    destination: &'a Destination,
    places: &'a Places,
    stop: &'a AtomicBool,
    reader: Reader,
    progress: Progress,
    status: Status,
    /// A position that was due to be saved and could not be: tried again
    /// at each tick.
    unsaved: Option<Position>,
    said_connection: Said,
    said_read: Said,
    said_save: Said,
    said_status: Said,
}

impl<'a> Forwarder<'a> {
    fn new(destination: &'a Destination, places: &'a Places, stop: &'a AtomicBool) -> Self {
        let name = &destination.name;
        let start = Position::load(&places.positions, name).unwrap_or_else(|| begin(name, places));
        Forwarder {
            destination,
            places,
            stop,
            reader: Reader::new(&places.log, name, start.clone()),
            progress: Progress::new(start, destination.transport),
            status: Status {
                name: name.clone(),
                address: destination.endpoint.clone(),
                transport: destination.transport.name().to_string(),
                state: State::Connecting,
                since: logd::rfc3339(util::now_ns()),
                error: None,
                sent: 0,
                behind: 0,
                tls: None,
            },
            unsaved: None,
            said_connection: Said::default(),
            said_read: Said::default(),
            said_save: Said::default(),
            said_status: Said::default(),
        }
    }

    fn stopped(&self) -> bool {
        self.stop.load(Ordering::Relaxed)
    }

    /// Connects and sends until the thread is to end.
    fn serve(&mut self) {
        let destination = self.destination;
        let mut backoff = Backoff::new();
        while !self.stopped() {
            self.set_state(State::Connecting);
            let connected = Connection::connect(
                &destination.address,
                destination.transport,
                destination.tls.as_ref(),
            );
            let failure = match connected {
                Ok(mut connection) => {
                    let opened = Instant::now();
                    match self.session(&mut connection) {
                        Ok(()) => {
                            connection.close();
                            return;
                        }
                        Err(error) => {
                            // The server may have taken what was flushed
                            // since the last position that counts as
                            // delivered without keeping it.
                            self.reader.seek(self.progress.rewind());
                            // A connection that stayed up is no sign of a
                            // server that only accepts and closes.
                            if opened.elapsed() >= SETTLE {
                                backoff.reset();
                                self.said_connection.clear();
                            }
                            error
                        }
                    }
                }
                Err(error) => error,
            };
            self.failed(&failure);
            if !self.wait(backoff.wait()) {
                return;
            }
        }
    }

    /// Waits `total` before the next attempt to connect, or until the
    /// thread is to end: whether it waited all of it. The status stays
    /// current meanwhile, for the log goes on growing.
    fn wait(&mut self, total: Duration) -> bool {
        let end = Instant::now() + total;
        loop {
            let left = end.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return true;
            }
            if !pause(self.stop, left.min(TICK)) {
                return false;
            }
            self.write_status();
        }
    }

    /// Says `failure` of the connection, if it is new, and shows it in the
    /// status.
    fn failed(&mut self, failure: &str) {
        if self.said_connection.is_new(failure) {
            eprintln!("<4>logforward: {}: {failure}; trying again", self.destination.name);
        }
        self.status.error = Some(failure.to_string());
        self.status.tls = None;
        self.set_state(State::Waiting);
    }

    /// Sends the log over `connection` until the thread is to end, or the
    /// connection fails.
    fn session(&mut self, connection: &mut Connection) -> Result<(), String> {
        let destination = self.destination;
        let found = connection.describe().map_or_default(|found| format!(": {found}"));
        println!("logforward: {}: connected to {}{found}", destination.name, destination.endpoint);
        self.status.error = None;
        self.status.tls = connection.describe().map(str::to_string);
        self.set_state(State::Connected);
        let mut ticked = Instant::now();
        while !self.stopped() {
            let batch = self.read();
            for note in &batch.notes {
                eprintln!("<4>logforward: {note}");
            }
            self.send(connection, &batch)?;
            if ticked.elapsed() >= TICK {
                self.tick(connection)?;
                ticked = Instant::now();
            }
            if batch.lines == 0 {
                pause(self.stop, IDLE);
            }
        }
        Ok(())
    }

    /// The next batch of the log; an empty one if the log cannot be read,
    /// which is said once.
    fn read(&mut self) -> Batch {
        match self.reader.read_batch() {
            Ok(batch) => {
                self.said_read.clear();
                batch
            }
            Err(error) => {
                let why = format!(
                    "cannot read the log in {}: {}",
                    self.places.log.display(),
                    util::strip_os_error(&error)
                );
                if self.said_read.is_new(&why) {
                    eprintln!("<3>logforward: {}: {why}", self.destination.name);
                }
                Batch {
                    records: Vec::new(),
                    lines: 0,
                    position: self.reader.position().clone(),
                    notes: Vec::new(),
                }
            }
        }
    }

    /// Sends the records of `batch` the destination is sent, flushes, and
    /// notes that the log up to the batch's end is flushed.
    fn send(&mut self, connection: &mut Connection, batch: &Batch) -> Result<(), String> {
        let destination = self.destination;
        let host = message::host_field(&util::hostname());
        let limit = message::limit(destination.transport);
        let mut sent = 0;
        for record in batch.records.iter().filter(|record| destination.wants(record)) {
            let message = message::record_message(record, &host, limit);
            connection
                .send(message.as_bytes())
                .map_err(|error| cannot_send(&destination.endpoint, &error))?;
            sent += 1;
        }
        if sent > 0 {
            connection.flush().map_err(|error| cannot_send(&destination.endpoint, &error))?;
        }
        self.status.sent += sent;
        if batch.lines > 0 {
            self.progress.sent(batch.position.clone(), Instant::now());
        }
        Ok(())
    }

    /// What is done at least once a second: the connection is looked at,
    /// what has stayed flushed long enough counts as delivered, and the
    /// position and the status are saved.
    fn tick(&mut self, connection: &mut Connection) -> Result<(), String> {
        if !connection.open() {
            return Err(format!("{} closed the connection", self.destination.endpoint));
        }
        let now = Instant::now();
        self.progress.still_open(now);
        if let Some(position) = self.progress.save_due(now) {
            self.unsaved = Some(position.clone());
        }
        self.save_position();
        self.write_status();
        Ok(())
    }

    /// Saves the position that is due, if there is one; a failure is said
    /// once, and the save is tried again at the next tick.
    fn save_position(&mut self) {
        let Some(position) = self.unsaved.take() else { return };
        match position.save(&self.places.positions, &self.destination.name) {
            Ok(()) => self.said_save.clear(),
            Err(error) => {
                let why = cannot_save(self.places, &error);
                if self.said_save.is_new(&why) {
                    eprintln!("<3>logforward: {}: {why}", self.destination.name);
                }
                self.unsaved = Some(position);
            }
        }
    }

    /// Notes the destination's state, and since when it has been in it.
    fn set_state(&mut self, state: State) {
        if self.status.state != state {
            self.status.since = logd::rfc3339(util::now_ns());
            self.status.state = state;
        }
        self.write_status();
    }

    /// Writes the status file; a failure is said once.
    fn write_status(&mut self) {
        self.status.behind = reader::behind(&self.places.log, self.reader.position());
        match self.status.write(&self.places.status) {
            Ok(()) => self.said_status.clear(),
            Err(error) => {
                let why = format!(
                    "cannot write its status in {}: {}",
                    self.places.status.display(),
                    util::strip_os_error(&error)
                );
                if self.said_status.is_new(&why) {
                    eprintln!("<4>logforward: {}: {why}", self.destination.name);
                }
            }
        }
    }

    /// Saves the position that counts as delivered, where sending goes on
    /// when logforward starts again, and takes the status away: the
    /// destination is not being served.
    fn finish(&mut self) {
        let name = &self.destination.name;
        if let Err(error) = self.progress.delivered().save(&self.places.positions, name) {
            eprintln!("<3>logforward: {name}: {}", cannot_save(self.places, &error));
        }
        status::remove(&self.places.status, name);
    }
}

/// Why a write to the server at `endpoint` failed, for the status.
fn cannot_send(endpoint: &str, error: &io::Error) -> String {
    match error.kind() {
        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut => {
            format!("cannot send to {endpoint}: the server stopped taking messages")
        }
        _ => format!("cannot send to {endpoint}: {}", util::strip_os_error(error)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream, UdpSocket};
    use std::path::Path;

    use super::super::connection::Transport;
    use crate::logd::Record;

    /// A folder of its own for each test, with the log, positions and
    /// status folders in it and a first day file of history.
    fn places(test: &str) -> Places {
        let dir =
            std::env::temp_dir().join(format!("logforward-forward-{test}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let places =
            Places { log: dir.join("log"), positions: dir.join("pos"), status: dir.join("live") };
        fs::create_dir_all(&places.log).unwrap();
        append(&places.log, &line(0, "info", "history", "before the destination"));
        places
    }

    fn line(seq: u64, level: &str, name: &str, text: &str) -> String {
        let record = Record {
            time: "2026-11-13T10:30:00.000000Z".into(),
            boot: "6c1f".into(),
            seq,
            level: level.into(),
            from: "service".into(),
            name: name.into(),
            text: text.into(),
            ..Record::default()
        };
        format!("{}\n", serde_json::to_string(&record).unwrap())
    }

    const DAY: &str = "2026-11-13.jsonl";

    fn append(log: &Path, text: &str) {
        let mut file =
            fs::OpenOptions::new().create(true).append(true).open(log.join(DAY)).unwrap();
        file.write_all(text.as_bytes()).unwrap();
    }

    fn destination(endpoint: &str, transport: Transport) -> Destination {
        Destination {
            name: "siem".into(),
            address: endpoint.into(),
            endpoint: endpoint.into(),
            transport,
            level: 6,
            names: None,
            audit_only: false,
            tls: None,
        }
    }

    /// The message logforward sends for a service's `text` at `level`
    /// from `name`, without its length.
    fn message_of(severity: u8, name: &str, text: &str) -> String {
        format!(
            "<{}>1 2026-11-13T10:30:00.000000Z {} {name} - - - {text}",
            3 * 8 + severity,
            message::host_field(&util::hostname())
        )
    }

    /// Reads `count` messages with their lengths in front from `stream`.
    fn read_counted(stream: &mut TcpStream, count: usize) -> Vec<String> {
        stream.set_read_timeout(Some(Duration::from_secs(20))).unwrap();
        let (mut buffer, mut messages) = (Vec::new(), Vec::new());
        let mut chunk = [0u8; 4096];
        while messages.len() < count {
            let Some(space) = buffer.iter().position(|byte| *byte == b' ') else {
                let read = stream.read(&mut chunk).unwrap();
                assert!(read > 0, "the connection ended after {} messages", messages.len());
                buffer.extend_from_slice(&chunk[..read]);
                continue;
            };
            let length: usize = std::str::from_utf8(&buffer[..space]).unwrap().parse().unwrap();
            if buffer.len() < space + 1 + length {
                let read = stream.read(&mut chunk).unwrap();
                assert!(read > 0, "the connection ended inside a message");
                buffer.extend_from_slice(&chunk[..read]);
                continue;
            }
            let body: Vec<u8> = buffer.drain(..space + 1 + length).skip(space + 1).collect();
            messages.push(String::from_utf8(body).unwrap());
        }
        messages
    }

    fn wait_until(what: &str, mut done: impl FnMut() -> bool) {
        let end = Instant::now() + Duration::from_secs(20);
        while !done() {
            assert!(Instant::now() < end, "never {what}");
            thread::sleep(Duration::from_millis(50));
        }
    }

    fn status_of(places: &Places) -> Option<Status> {
        status::read_all(&places.status).pop()
    }

    /// Stops a destination's thread when a test ends, also by a failed
    /// assertion, which would leave the thread scope waiting for it.
    struct Stopper<'a>(&'a AtomicBool);

    impl Drop for Stopper<'_> {
        fn drop(&mut self) {
            self.0.store(true, Ordering::Relaxed);
        }
    }

    #[test]
    fn a_new_destination_is_sent_the_log_from_now_on_counted_and_in_order() {
        let places = places("tcp");
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = listener.local_addr().unwrap().to_string();
        let destination = destination(&endpoint, Transport::Tcp);
        let stop = AtomicBool::new(false);
        thread::scope(|scope| {
            scope.spawn(|| run(&destination, &places, &stop));
            let _stopper = Stopper(&stop);
            let (mut server, _) = listener.accept().unwrap();
            wait_until("connected", || {
                status_of(&places).is_some_and(|s| s.state == State::Connected)
            });
            append(&places.log, &line(1, "info", "sshd", "first"));
            append(&places.log, &line(2, "warning", "sshd", "second"));
            append(&places.log, &line(3, "debug", "sshd", "not sent: debug"));
            append(&places.log, &line(4, "error", "kernel", "third é"));
            let got = read_counted(&mut server, 3);
            assert_eq!(
                got,
                [
                    message_of(6, "sshd", "first"),
                    message_of(4, "sshd", "second"),
                    message_of(3, "kernel", "third é")
                ]
            );
            wait_until("sent 3", || status_of(&places).is_some_and(|s| s.sent == 3));
            let status = status_of(&places).unwrap();
            assert_eq!(
                (status.name.as_str(), status.address.as_str(), status.transport.as_str()),
                ("siem", endpoint.as_str(), "tcp")
            );
        });
        // Nothing counts as delivered over TCP until the connection has
        // stayed up for ten seconds: the position is where the destination
        // began, after the history.
        let history = line(0, "info", "history", "before the destination");
        assert_eq!(
            Position::load(&places.positions, "siem"),
            Some(Position::of(DAY, history.len() as u64))
        );
        assert_eq!(status::read_all(&places.status), []);
    }

    #[test]
    fn over_udp_each_record_is_a_datagram_and_a_restart_goes_on_after_the_last() {
        let places = places("udp");
        let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
        socket.set_read_timeout(Some(Duration::from_secs(20))).unwrap();
        let endpoint = socket.local_addr().unwrap().to_string();
        let destination = destination(&endpoint, Transport::Udp);
        let stop = AtomicBool::new(false);
        let mut text = String::new();
        let mut datagram = [0u8; 2048];
        thread::scope(|scope| {
            scope.spawn(|| run(&destination, &places, &stop));
            let _stopper = Stopper(&stop);
            wait_until("connected", || {
                status_of(&places).is_some_and(|s| s.state == State::Connected)
            });
            text.push_str(&line(1, "info", "sshd", "first"));
            text.push_str(&line(2, "info", "sshd", "second"));
            append(&places.log, &text);
            for want in ["first", "second"] {
                let read = socket.recv(&mut datagram).unwrap();
                assert_eq!(
                    std::str::from_utf8(&datagram[..read]).unwrap(),
                    message_of(6, "sshd", want)
                );
            }
        });
        let history = line(0, "info", "history", "before the destination");
        assert_eq!(
            Position::load(&places.positions, "siem"),
            Some(Position::of(DAY, (history.len() + text.len()) as u64))
        );
        // Started again, it is sent what came after and nothing twice.
        let stop = AtomicBool::new(false);
        thread::scope(|scope| {
            scope.spawn(|| run(&destination, &places, &stop));
            let _stopper = Stopper(&stop);
            wait_until("connected", || {
                status_of(&places).is_some_and(|s| s.state == State::Connected)
            });
            append(&places.log, &line(3, "info", "sshd", "third"));
            let read = socket.recv(&mut datagram).unwrap();
            assert_eq!(
                std::str::from_utf8(&datagram[..read]).unwrap(),
                message_of(6, "sshd", "third")
            );
        });
        socket.set_read_timeout(Some(Duration::from_millis(300))).unwrap();
        assert!(socket.recv(&mut datagram).is_err(), "a record was sent twice");
    }

    #[test]
    fn records_the_destination_is_not_sent_still_move_its_position() {
        let places = places("filter");
        let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
        socket.set_read_timeout(Some(Duration::from_secs(20))).unwrap();
        let endpoint = socket.local_addr().unwrap().to_string();
        let destination =
            Destination { audit_only: true, level: 7, ..destination(&endpoint, Transport::Udp) };
        let stop = AtomicBool::new(false);
        let mut text = String::new();
        thread::scope(|scope| {
            scope.spawn(|| run(&destination, &places, &stop));
            let _stopper = Stopper(&stop);
            wait_until("connected", || {
                status_of(&places).is_some_and(|s| s.state == State::Connected)
            });
            text.push_str(&line(1, "info", "sshd", "not an event"));
            let event = Record {
                audit: true,
                ..serde_json::from_str(line(2, "info", "authd", "signed in").trim()).unwrap()
            };
            text.push_str(&format!("{}\n", serde_json::to_string(&event).unwrap()));
            text.push_str(&line(3, "info", "sshd", "not an event either"));
            append(&places.log, &text);
            let mut datagram = [0u8; 2048];
            let read = socket.recv(&mut datagram).unwrap();
            let got = std::str::from_utf8(&datagram[..read]).unwrap();
            assert!(got.starts_with("<86>1 "), "{got}");
            assert!(got.ends_with(" authd - audit - signed in"), "{got}");
            wait_until("moved past the log", || {
                let history = line(0, "info", "history", "before the destination");
                reader::behind(&places.log, &Position::of(DAY, (history.len() + text.len()) as u64))
                    == 0
                    && status_of(&places).is_some_and(|s| s.behind == 0)
            });
        });
        socket.set_read_timeout(Some(Duration::from_millis(300))).unwrap();
        let mut datagram = [0u8; 2048];
        assert!(socket.recv(&mut datagram).is_err(), "a record that is no event was sent");
    }

    #[test]
    fn while_the_server_is_away_the_log_waits_and_is_sent_in_order_when_it_is_back() {
        let places = places("queue");
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = listener.local_addr().unwrap().to_string();
        drop(listener);
        let destination = destination(&endpoint, Transport::Tcp);
        let stop = AtomicBool::new(false);
        thread::scope(|scope| {
            scope.spawn(|| run(&destination, &places, &stop));
            let _stopper = Stopper(&stop);
            wait_until("waiting", || status_of(&places).is_some_and(|s| s.state == State::Waiting));
            let status = status_of(&places).unwrap();
            assert_eq!(
                status.error,
                Some(format!("cannot connect to {endpoint}: Connection refused"))
            );
            append(&places.log, &line(1, "info", "sshd", "second"));
            append(&places.log, &line(2, "info", "sshd", "third"));
            wait_until("behind", || status_of(&places).is_some_and(|s| s.behind > 0));
            let listener = TcpListener::bind(&endpoint).unwrap();
            let (mut server, _) = listener.accept().unwrap();
            let got = read_counted(&mut server, 2);
            assert_eq!(got, [message_of(6, "sshd", "second"), message_of(6, "sshd", "third")]);
            wait_until("connected", || {
                status_of(&places).is_some_and(|s| s.state == State::Connected && s.error.is_none())
            });
        });
    }

    #[test]
    fn a_server_that_closes_the_connection_is_sent_what_it_may_have_lost_again() {
        let places = places("closed");
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = listener.local_addr().unwrap().to_string();
        let destination = destination(&endpoint, Transport::Tcp);
        let stop = AtomicBool::new(false);
        thread::scope(|scope| {
            scope.spawn(|| run(&destination, &places, &stop));
            let _stopper = Stopper(&stop);
            let (mut first, _) = listener.accept().unwrap();
            wait_until("connected", || {
                status_of(&places).is_some_and(|s| s.state == State::Connected)
            });
            append(&places.log, &line(1, "info", "sshd", "once"));
            assert_eq!(read_counted(&mut first, 1), [message_of(6, "sshd", "once")]);
            drop(first);
            // The connection did not last ten seconds, so the server may
            // not have kept it: it comes again on the next connection.
            let (mut second, _) = listener.accept().unwrap();
            assert_eq!(read_counted(&mut second, 1), [message_of(6, "sshd", "once")]);
        });
    }

    #[test]
    fn a_position_that_cannot_be_saved_is_tried_again_each_second() {
        let places = places("save");
        let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
        socket.set_read_timeout(Some(Duration::from_secs(20))).unwrap();
        let endpoint = socket.local_addr().unwrap().to_string();
        let destination = destination(&endpoint, Transport::Udp);
        let stop = AtomicBool::new(false);
        let mut text = String::new();
        thread::scope(|scope| {
            scope.spawn(|| run(&destination, &places, &stop));
            let _stopper = Stopper(&stop);
            wait_until("connected", || {
                status_of(&places).is_some_and(|s| s.state == State::Connected)
            });
            // A file where the positions' folder goes cannot be made one.
            fs::remove_dir_all(&places.positions).unwrap();
            fs::write(&places.positions, "in the way").unwrap();
            text.push_str(&line(1, "info", "sshd", "moves the position"));
            append(&places.log, &text);
            let mut datagram = [0u8; 2048];
            socket.recv(&mut datagram).unwrap();
            thread::sleep(Duration::from_millis(2500));
            fs::remove_file(&places.positions).unwrap();
            let history = line(0, "info", "history", "before the destination");
            let want = Position::of(DAY, (history.len() + text.len()) as u64);
            wait_until("saved", || {
                Position::load(&places.positions, "siem").as_ref() == Some(&want)
            });
        });
    }

    #[test]
    fn the_wait_between_attempts_doubles_from_a_second_to_a_minute_and_starts_over() {
        let mut backoff = Backoff::new();
        let waits: Vec<u64> = (0..9).map(|_| backoff.wait().as_secs()).collect();
        assert_eq!(waits, [1, 2, 4, 8, 16, 32, 60, 60, 60]);
        backoff.reset();
        assert_eq!(backoff.wait(), Duration::from_secs(1));
    }

    #[test]
    fn an_error_is_said_once_until_it_changes_or_ends() {
        let mut said = Said::default();
        assert!(said.is_new("refused"));
        assert!(!said.is_new("refused"));
        assert!(said.is_new("timed out"));
        assert!(said.is_new("refused"));
        said.clear();
        assert!(said.is_new("refused"));
    }

    #[test]
    fn a_pause_ends_early_when_the_thread_is_to_stop() {
        let stop = AtomicBool::new(false);
        let started = Instant::now();
        assert!(pause(&stop, Duration::from_millis(150)));
        assert!(started.elapsed() >= Duration::from_millis(150));
        stop.store(true, Ordering::Relaxed);
        let started = Instant::now();
        assert!(!pause(&stop, Duration::from_secs(30)));
        assert!(started.elapsed() < Duration::from_secs(1));
    }
}
