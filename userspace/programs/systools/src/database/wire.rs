// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! PostgreSQL's protocol, as much of it as `database` speaks: a session
//! over the server's Unix socket as the role postgres, statements by the
//! simple query protocol with their answers as text, and a check that
//! the server takes TLS with a certificate a client trusts.
//!
//! `database` signs in by the server's peer authentication, which takes
//! root to the role postgres (the bundle's `pg_hba.conf` and
//! `pg_ident.conf`): no password is ever sent, and a server that asks
//! for one is refused. There is no extended protocol and no COPY. Every
//! name a statement holds is quoted as an identifier ([`ident`]) and
//! every text as a string ([`literal`]); a session starts only when the
//! server reads strings as the standard says (`standard_conforming_strings`
//! on), so that a backslash in a string is a backslash.
//!
//! A message over 1 MiB, more than 100,000 rows to one statement, or 30
//! seconds with no byte end the session, as does anything the server
//! sends that this module does not read.

use std::fmt;
use std::io::{self, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use rustls::RootCertStore;

use crate::tls;

/// The protocol's version 3.0, as the startup message names it.
const VERSION: i32 = 3 << 16;

/// The code an SSLRequest is sent with in place of a version.
const SSL_REQUEST: i32 = 80_877_103;

/// The longest message read: longer ends the session.
pub const MESSAGE_MAX: usize = 1 << 20;

/// The most rows one call of [`Session::simple`] reads.
pub const ROWS_MAX: usize = 100_000;

/// How long the server may send nothing before the session ends.
pub const QUIET_MAX: Duration = Duration::from_secs(30);

/// The SQLSTATEs of a role or a database that is there already.
pub const TAKEN: [&str; 2] = ["42710", "42P04"];

/// What PostgreSQL said when it refused a statement: the fields of its
/// ErrorResponse that `database` shows.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PgError {
    /// `ERROR`, `FATAL` or `PANIC`, in English whatever the server's
    /// language.
    pub severity: String,
    /// The SQLSTATE, such as `42P04`.
    pub code: String,
    /// The server's words.
    pub message: String,
    /// More of them, when the server gave more.
    pub detail: Option<String>,
    /// What the server suggests.
    pub hint: Option<String>,
    /// Where in the statement, a character count from 1.
    pub position: Option<String>,
}

impl PgError {
    /// Whether it says that the role or the database is there already.
    #[must_use]
    pub fn is_taken(&self) -> bool {
        TAKEN.contains(&self.code.as_str())
    }
}

impl fmt::Display for PgError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "PostgreSQL said: {} ({})", self.message, self.code)?;
        if let Some(detail) = &self.detail {
            write!(f, ": {detail}")?;
        }
        if let Some(hint) = &self.hint {
            write!(f, " {hint}")?;
        }
        Ok(())
    }
}

/// Why a session could not do what it was asked.
#[derive(Debug)]
pub enum Error {
    /// The server asked for a password or another proof in place of
    /// letting root in by its account; the number is the way it asked
    /// for. The session never started.
    NotLetIn(i32),
    /// The server refused the statement, or the start; the session goes
    /// on after a statement. Boxed, as it is the largest of the three
    /// and the rarest.
    Server(Box<PgError>),
    /// The session is over: the socket failed, the server went quiet,
    /// sent too much, or sent what this module does not read.
    Broken(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::NotLetIn(way) => write!(
                f,
                "PostgreSQL did not let root in by its account: its pg_hba.conf's first line \
                 was changed (it asked for a proof of kind {way})"
            ),
            Error::Server(error) => error.fmt(f),
            Error::Broken(why) => write!(f, "the session with PostgreSQL ended: {why}"),
        }
    }
}

/// What one statement answered: the names of its columns, its rows with
/// each value as text or null, and its tag, such as `CREATE ROLE` or
/// `SELECT 2`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Answer {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<Option<String>>>,
    pub tag: String,
}

impl Answer {
    /// The value of `column` in the first row, when the answer has both
    /// and it is not null.
    #[must_use]
    pub fn first(&self, column: &str) -> Option<&str> {
        let at = self.columns.iter().position(|name| name == column)?;
        self.rows.first()?.get(at)?.as_deref()
    }
}

/// A session with the server, signed in as the role postgres.
pub struct Session<S: Read + Write> {
    stream: S,
    /// What the server said of itself as the session started and since:
    /// `server_version`, `standard_conforming_strings` and the rest.
    parameters: Vec<(String, String)>,
    /// Set when the session is over: every call after fails.
    broken: Option<String>,
}

impl Session<UnixStream> {
    /// Signs in over the server's Unix socket `socket` as the role
    /// postgres, to the database postgres.
    ///
    /// # Errors
    ///
    /// When the socket cannot be reached, when the server asks for a
    /// proof ([`Error::NotLetIn`]), refuses the start, or reads strings
    /// otherwise than the standard says.
    pub fn connect(socket: &Path) -> Result<Session<UnixStream>, Error> {
        let stream = UnixStream::connect(socket)
            .map_err(|err| Error::Broken(format!("{}: {err}", socket.display())))?;
        let timeouts = stream
            .set_read_timeout(Some(QUIET_MAX))
            .and_then(|()| stream.set_write_timeout(Some(QUIET_MAX)));
        timeouts.map_err(|err| Error::Broken(format!("{}: {err}", socket.display())))?;
        Session::start(stream)
    }
}

impl<S: Read + Write> Session<S> {
    /// Sends the startup message over `stream` and reads the server's
    /// answers up to the first ReadyForQuery.
    ///
    /// # Errors
    ///
    /// As [`Session::connect`].
    pub fn start(stream: S) -> Result<Session<S>, Error> {
        let mut session = Session { stream, parameters: Vec::new(), broken: None };
        let mut body = Vec::new();
        body.extend_from_slice(&VERSION.to_be_bytes());
        for (key, value) in [
            ("user", "postgres"),
            ("database", "postgres"),
            ("application_name", "database"),
            ("client_encoding", "UTF8"),
        ] {
            body.extend_from_slice(key.as_bytes());
            body.push(0);
            body.extend_from_slice(value.as_bytes());
            body.push(0);
        }
        body.push(0);
        session.send(None, &body)?;
        let mut signed_in = false;
        loop {
            let (kind, body) = session.receive()?;
            let mut reading = Body::new(kind, &body);
            match kind {
                b'R' => {
                    let way = reading.i32()?;
                    if way != 0 {
                        // Nothing is answered: the server ends it.
                        session.broken = Some("it asked for a proof".into());
                        return Err(Error::NotLetIn(way));
                    }
                    signed_in = true;
                }
                b'S' => session.parameter(&mut reading)?,
                // The key a cancel would need, which `database` never
                // sends, and notices.
                b'K' | b'N' => {}
                b'E' => {
                    session.broken = Some("it refused the start".into());
                    return Err(Error::Server(Box::new(error_of(&mut reading)?)));
                }
                b'Z' if signed_in => break,
                other => {
                    return Err(
                        session.end(format!("it sent {} as the session started", shown(other)))
                    );
                }
            }
        }
        match session.parameter_of("standard_conforming_strings") {
            Some("on") => Ok(session),
            said => Err(session.end(format!(
                "standard_conforming_strings is {}, and a string is only written as one when \
                 it is on",
                said.unwrap_or("not said")
            ))),
        }
    }

    /// What the server said `name` is, such as `server_version`.
    #[must_use]
    pub fn parameter_of(&self, name: &str) -> Option<&str> {
        self.parameters.iter().rev().find(|(key, _)| key == name).map(|(_, value)| value.as_str())
    }

    /// Runs `sql`, one statement or several, by the simple query
    /// protocol: an answer for each statement, in order.
    ///
    /// # Errors
    ///
    /// [`Error::Server`] when the server refused a statement: those
    /// before it have run, none after it has, and the session goes on.
    /// [`Error::Broken`] when the session is over, as the module's text
    /// says, or when the server asks to copy (no COPY is spoken).
    pub fn simple(&mut self, sql: &str) -> Result<Vec<Answer>, Error> {
        // Never sent, so the session goes on.
        if sql.as_bytes().contains(&0) {
            return Err(Error::Broken("a statement held a NUL, which it cannot".into()));
        }
        let answered = self.query(sql);
        // A message that did not read leaves the rest out of step.
        if let Err(Error::Broken(why)) = &answered {
            self.broken.get_or_insert_with(|| why.clone());
        }
        answered
    }

    /// [`Session::simple`], but for marking the session over.
    fn query(&mut self, sql: &str) -> Result<Vec<Answer>, Error> {
        let mut body = Vec::with_capacity(sql.len() + 1);
        body.extend_from_slice(sql.as_bytes());
        body.push(0);
        self.send(Some(b'Q'), &body)?;
        let mut answers = Vec::new();
        let mut current = Answer::default();
        let mut refused = None;
        let mut rows = 0usize;
        loop {
            let (kind, body) = self.receive()?;
            let mut reading = Body::new(kind, &body);
            match kind {
                b'T' => current.columns = self.columns(&mut reading)?,
                b'D' => {
                    rows += 1;
                    if rows > ROWS_MAX {
                        return Err(self.end(format!("a statement gave more than {ROWS_MAX} rows")));
                    }
                    let row = self.row(&mut reading, current.columns.len())?;
                    current.rows.push(row);
                }
                b'C' => {
                    current.tag = self.text(reading.string()?)?;
                    answers.push(std::mem::take(&mut current));
                }
                b'I' => answers.push(std::mem::take(&mut current)),
                b'E' => refused = Some(error_of(&mut reading)?),
                b'S' => self.parameter(&mut reading)?,
                b'N' | b'A' => {}
                b'G' | b'H' | b'W' => {
                    return Err(self.end("it asked to copy, which is not spoken here".into()));
                }
                b'Z' => break,
                other => return Err(self.end(format!("it sent {} to a statement", shown(other)))),
            }
        }
        match refused {
            Some(error) => Err(Error::Server(Box::new(error))),
            None => Ok(answers),
        }
    }

    /// Writes one message: `kind` and its length before `body`, or the
    /// length alone for the startup message.
    fn send(&mut self, kind: Option<u8>, body: &[u8]) -> Result<(), Error> {
        if let Some(why) = &self.broken {
            return Err(Error::Broken(why.clone()));
        }
        let length = i32::try_from(body.len() + 4)
            .ok()
            .filter(|_| body.len() <= MESSAGE_MAX)
            .ok_or_else(|| Error::Broken("a statement was longer than 1 MiB".into()))?;
        let mut message = Vec::with_capacity(body.len() + 5);
        message.extend(kind);
        message.extend_from_slice(&length.to_be_bytes());
        message.extend_from_slice(body);
        if let Err(err) = self.stream.write_all(&message).and_then(|()| self.stream.flush()) {
            return Err(self.end(format!("it could not be written to: {err}")));
        }
        Ok(())
    }

    /// Reads one message: its kind and its body.
    fn receive(&mut self) -> Result<(u8, Vec<u8>), Error> {
        if let Some(why) = &self.broken {
            return Err(Error::Broken(why.clone()));
        }
        let mut head = [0u8; 5];
        if let Err(err) = self.stream.read_exact(&mut head) {
            return Err(self.end(quiet_or(&err)));
        }
        let length = i32::from_be_bytes([head[1], head[2], head[3], head[4]]);
        let Some(size) = usize::try_from(length).ok().and_then(|length| length.checked_sub(4))
        else {
            return Err(self.end(format!("a message's length was {length}")));
        };
        if size > MESSAGE_MAX {
            return Err(self.end(format!("a message was {size} bytes, more than 1 MiB")));
        }
        let mut body = vec![0u8; size];
        if let Err(err) = self.stream.read_exact(&mut body) {
            return Err(self.end(quiet_or(&err)));
        }
        Ok((head[0], body))
    }

    /// Ends the session for `why`: every call after fails with it.
    fn end(&mut self, why: String) -> Error {
        self.broken = Some(why.clone());
        Error::Broken(why)
    }

    /// Keeps a ParameterStatus.
    fn parameter(&mut self, reading: &mut Body<'_>) -> Result<(), Error> {
        let name = self.text(reading.string()?)?;
        let value = self.text(reading.string()?)?;
        self.parameters.push((name, value));
        Ok(())
    }

    /// The names of a RowDescription's columns; each must be sent as text.
    fn columns(&mut self, reading: &mut Body<'_>) -> Result<Vec<String>, Error> {
        let count = reading.i16()?;
        let mut columns = Vec::new();
        for _ in 0..count {
            let name = self.text(reading.string()?)?;
            // The table, the column's number, its type, size and modifier.
            reading.skip(4 + 2 + 4 + 2 + 4)?;
            if reading.i16()? != 0 {
                return Err(self.end(format!("the column {name} was sent as binary")));
            }
            columns.push(name);
        }
        Ok(columns)
    }

    /// A DataRow's values, as text or null; it must have `columns` of
    /// them.
    fn row(
        &mut self,
        reading: &mut Body<'_>,
        columns: usize,
    ) -> Result<Vec<Option<String>>, Error> {
        let count = reading.i16()?;
        if usize::try_from(count).ok() != Some(columns) {
            return Err(self.end(format!("a row had {count} values for {columns} columns")));
        }
        let mut row = Vec::with_capacity(columns);
        for _ in 0..count {
            let length = reading.i32()?;
            if length == -1 {
                row.push(None);
                continue;
            }
            let Ok(length) = usize::try_from(length) else {
                return Err(self.end(format!("a value's length was {length}")));
            };
            let value = reading.take(length)?;
            row.push(Some(self.text(value)?));
        }
        Ok(row)
    }

    /// `bytes` as text, which UTF8 as the client's encoding makes them.
    fn text(&mut self, bytes: &[u8]) -> Result<String, Error> {
        match std::str::from_utf8(bytes) {
            Ok(text) => Ok(text.to_string()),
            Err(_) => Err(self.end("it sent text that is not UTF-8".into())),
        }
    }
}

impl<S: Read + Write> Drop for Session<S> {
    /// Says Terminate, so that the server ends the session at once.
    fn drop(&mut self) {
        if self.broken.is_none() {
            let _ = self.stream.write_all(&[b'X', 0, 0, 0, 4]);
            let _ = self.stream.flush();
        }
    }
}

/// A message's body, read from the front.
struct Body<'a> {
    kind: u8,
    bytes: &'a [u8],
}

impl<'a> Body<'a> {
    fn new(kind: u8, bytes: &'a [u8]) -> Body<'a> {
        Body { kind, bytes }
    }

    /// The next `count` bytes.
    fn take(&mut self, count: usize) -> Result<&'a [u8], Error> {
        if self.bytes.len() < count {
            return Err(Error::Broken(format!("its message {} was cut short", shown(self.kind))));
        }
        let (taken, rest) = self.bytes.split_at(count);
        self.bytes = rest;
        Ok(taken)
    }

    fn skip(&mut self, count: usize) -> Result<(), Error> {
        self.take(count).map(|_| ())
    }

    fn i16(&mut self) -> Result<i16, Error> {
        let bytes = self.take(2)?;
        Ok(i16::from_be_bytes([bytes[0], bytes[1]]))
    }

    fn i32(&mut self) -> Result<i32, Error> {
        let bytes = self.take(4)?;
        Ok(i32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
    }

    /// The bytes up to the next NUL, which is passed.
    fn string(&mut self) -> Result<&'a [u8], Error> {
        let Some(end) = self.bytes.iter().position(|&byte| byte == 0) else {
            return Err(Error::Broken(format!(
                "its message {} held an unended string",
                shown(self.kind)
            )));
        };
        let text = &self.bytes[..end];
        self.bytes = &self.bytes[end + 1..];
        Ok(text)
    }
}

/// The fields of an ErrorResponse.
fn error_of(reading: &mut Body<'_>) -> Result<PgError, Error> {
    let mut error = PgError::default();
    let mut localized = String::new();
    loop {
        let field = reading.take(1)?[0];
        if field == 0 {
            break;
        }
        let value = String::from_utf8_lossy(reading.string()?).into_owned();
        match field {
            b'S' => localized = value,
            b'V' => error.severity = value,
            b'C' => error.code = value,
            b'M' => error.message = value,
            b'D' => error.detail = Some(value),
            b'H' => error.hint = Some(value),
            b'P' => error.position = Some(value),
            _ => {}
        }
    }
    if error.severity.is_empty() {
        error.severity = localized;
    }
    Ok(error)
}

/// A message's kind as a person reads it: its letter, or its number.
fn shown(kind: u8) -> String {
    if kind.is_ascii_graphic() { format!("'{}'", char::from(kind)) } else { format!("byte {kind}") }
}

/// Why a read failed: the server's silence, or the error.
fn quiet_or(err: &io::Error) -> String {
    match err.kind() {
        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut => {
            format!("it sent nothing for {} seconds", QUIET_MAX.as_secs())
        }
        io::ErrorKind::UnexpectedEof => "it closed the connection".into(),
        _ => format!("it could not be read: {err}"),
    }
}

/// `name` quoted as an identifier: in double quotes, each double quote
/// doubled, so that it is only ever a name.
///
/// # Errors
///
/// When it is empty or holds a NUL, which no name can.
pub fn ident(name: &str) -> Result<String, String> {
    if name.is_empty() || name.contains('\0') {
        return Err("a name must have characters and no NUL".into());
    }
    Ok(format!("\"{}\"", name.replace('"', "\"\"")))
}

/// `text` quoted as a string: in single quotes, each single quote
/// doubled. A session reads it so only with `standard_conforming_strings`
/// on, which [`Session::start`] holds it to.
///
/// # Errors
///
/// When it holds a NUL, which no string can.
pub fn literal(text: &str) -> Result<String, String> {
    if text.contains('\0') {
        return Err("a string cannot hold a NUL".into());
    }
    Ok(format!("'{}'", text.replace('\'', "''")))
}

/// Checks that the server at `address` takes TLS with a certificate for
/// `name` that `roots` lead to: PostgreSQL's SSLRequest, its `S`, then
/// the handshake. Nothing is sent after it.
///
/// # Errors
///
/// When the server cannot be reached, answers `N` (it does not take
/// TLS), or the handshake or the certificate fails.
pub fn tls_probe(address: SocketAddr, name: &str, roots: RootCertStore) -> Result<(), String> {
    let mut tcp = TcpStream::connect_timeout(&address, QUIET_MAX)
        .map_err(|err| format!("{address} could not be reached: {err}"))?;
    let set =
        tcp.set_read_timeout(Some(QUIET_MAX)).and_then(|()| tcp.set_write_timeout(Some(QUIET_MAX)));
    set.map_err(|err| format!("{address}: {err}"))?;
    let mut request = Vec::with_capacity(8);
    request.extend_from_slice(&8i32.to_be_bytes());
    request.extend_from_slice(&SSL_REQUEST.to_be_bytes());
    tcp.write_all(&request).map_err(|err| format!("{address} could not be written to: {err}"))?;
    let mut answer = [0u8; 1];
    tcp.read_exact(&mut answer).map_err(|err| format!("{address} did not answer: {err}"))?;
    match answer[0] {
        b'S' => {}
        b'N' => return Err(format!("PostgreSQL at {address} does not take TLS")),
        other => return Err(format!("PostgreSQL at {address} answered {} to TLS", shown(other))),
    }
    let server = tls::server_name(Some(name), address).map_err(|err| format!("{name}: {err}"))?;
    let config: Arc<_> = tls::verified(roots, &[]);
    tls::handshake(tcp, server, &config)
        .map(|_| ())
        .map_err(|err| format!("TLS with PostgreSQL at {address} for {name} failed: {err}"))
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::io::Cursor;
    use std::rc::Rc;

    use super::*;

    /// A server's side, written beforehand: what it sends, and what the
    /// session wrote to it.
    struct Recorded {
        sent: Cursor<Vec<u8>>,
        written: Vec<u8>,
    }

    impl Read for Recorded {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            self.sent.read(buf)
        }
    }

    impl Write for Recorded {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.written.extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    /// One message of the server's.
    fn message(kind: u8, body: &[u8]) -> Vec<u8> {
        let mut out = vec![kind];
        out.extend_from_slice(&i32::try_from(body.len() + 4).unwrap().to_be_bytes());
        out.extend_from_slice(body);
        out
    }

    fn parameter(name: &str, value: &str) -> Vec<u8> {
        message(b'S', format!("{name}\0{value}\0").as_bytes())
    }

    /// What a server sends as a session starts: let in, its parameters,
    /// its key, ready.
    fn welcome() -> Vec<u8> {
        [
            message(b'R', &0i32.to_be_bytes()),
            parameter("server_version", "19rc2"),
            parameter("standard_conforming_strings", "on"),
            message(b'K', &[0, 0, 0, 7, 1, 2, 3, 4]),
            message(b'Z', b"I"),
        ]
        .concat()
    }

    fn columns(names: &[&str]) -> Vec<u8> {
        let mut body = i16::try_from(names.len()).unwrap().to_be_bytes().to_vec();
        for name in names {
            body.extend_from_slice(name.as_bytes());
            body.push(0);
            body.extend_from_slice(&[0; 4 + 2 + 4 + 2 + 4]);
            body.extend_from_slice(&0i16.to_be_bytes());
        }
        message(b'T', &body)
    }

    fn row(values: &[Option<&str>]) -> Vec<u8> {
        let mut body = i16::try_from(values.len()).unwrap().to_be_bytes().to_vec();
        for value in values {
            match value {
                None => body.extend_from_slice(&(-1i32).to_be_bytes()),
                Some(text) => {
                    body.extend_from_slice(&i32::try_from(text.len()).unwrap().to_be_bytes());
                    body.extend_from_slice(text.as_bytes());
                }
            }
        }
        message(b'D', &body)
    }

    fn done(tag: &str) -> Vec<u8> {
        message(b'C', format!("{tag}\0").as_bytes())
    }

    fn error(fields: &[(u8, &str)]) -> Vec<u8> {
        let mut body = Vec::new();
        for (field, value) in fields {
            body.push(*field);
            body.extend_from_slice(value.as_bytes());
            body.push(0);
        }
        body.push(0);
        message(b'E', &body)
    }

    fn session(sent: Vec<u8>) -> Result<Session<Recorded>, Error> {
        Session::start(Recorded { sent: Cursor::new(sent), written: Vec::new() })
    }

    #[test]
    fn the_start_asks_for_postgres_as_postgres_and_keeps_what_the_server_says() {
        let session = session(welcome()).unwrap();
        let written = &session.stream.written;
        let length = usize::try_from(i32::from_be_bytes(written[..4].try_into().unwrap())).unwrap();
        assert_eq!(length, written.len());
        assert_eq!(&written[4..8], &(3i32 << 16).to_be_bytes());
        assert_eq!(
            &written[8..],
            b"user\0postgres\0database\0postgres\0application_name\0database\0client_encoding\0UTF8\0\0"
        );
        assert_eq!(session.parameter_of("server_version"), Some("19rc2"));
        assert_eq!(session.parameter_of("standard_conforming_strings"), Some("on"));
    }

    #[test]
    fn a_server_that_asks_for_a_password_or_reads_strings_otherwise_is_refused() {
        for way in [3, 5, 10] {
            let sent = message(b'R', &i32::to_be_bytes(way));
            assert!(matches!(session(sent), Err(Error::NotLetIn(asked)) if asked == way));
        }
        let sent = [
            message(b'R', &0i32.to_be_bytes()),
            parameter("standard_conforming_strings", "off"),
            message(b'Z', b"I"),
        ]
        .concat();
        let Err(Error::Broken(why)) = session(sent) else { panic!("a session started") };
        assert!(why.contains("standard_conforming_strings is off"), "{why}");
        // A refused start gives the server's words.
        let sent = error(&[
            (b'S', "FATAL"),
            (b'V', "FATAL"),
            (b'C', "28000"),
            (b'M', "no pg_hba.conf entry"),
        ]);
        let Err(Error::Server(refused)) = session(sent) else { panic!("a session started") };
        assert_eq!((refused.severity.as_str(), refused.code.as_str()), ("FATAL", "28000"));
        // Ready before being let in is not a start.
        assert!(matches!(session(message(b'Z', b"I")), Err(Error::Broken(_))));
    }

    #[test]
    fn a_recorded_exchange_reads_into_answers() {
        let sent = [
            welcome(),
            columns(&["datname", "size"]),
            row(&[Some("shop"), Some("7")]),
            row(&[Some("blog"), None]),
            done("SELECT 2"),
            message(b'N', b"SNOTICE\0Mnothing\0\0"),
            done("CREATE ROLE"),
            message(b'I', b""),
            message(b'Z', b"I"),
        ]
        .concat();
        let mut session = session(sent).unwrap();
        let answers = session.simple("SELECT 1; CREATE ROLE x; ").unwrap();
        assert_eq!(answers.len(), 3);
        assert_eq!(answers[0].columns, ["datname", "size"]);
        assert_eq!(
            answers[0].rows,
            [vec![Some("shop".into()), Some("7".into())], vec![Some("blog".into()), None]]
        );
        assert_eq!(answers[0].tag, "SELECT 2");
        assert_eq!(answers[0].first("datname"), Some("shop"));
        assert_eq!(answers[1], Answer { columns: vec![], rows: vec![], tag: "CREATE ROLE".into() });
        assert_eq!(answers[2], Answer::default());
        // The query went as one Q message, its text ended by a NUL.
        let written = &session.stream.written;
        let query = b"SELECT 1; CREATE ROLE x; \0";
        assert!(written.ends_with(query));
        let at = written.len() - query.len() - 5;
        assert_eq!(written[at], b'Q');
    }

    #[test]
    fn an_error_s_fields_are_read_and_the_session_goes_on() {
        let sent = [
            welcome(),
            error(&[
                (b'S', "ERREUR"),
                (b'V', "ERROR"),
                (b'C', "42P04"),
                (b'M', "database \"shop\" already exists"),
                (b'D', "more"),
                (b'H', "pick another"),
                (b'P', "17"),
                (b'F', "dbcommands.c"),
            ]),
            message(b'Z', b"I"),
            done("SELECT 1"),
            message(b'Z', b"I"),
        ]
        .concat();
        let mut session = session(sent).unwrap();
        let Err(Error::Server(refused)) = session.simple("CREATE DATABASE shop") else {
            panic!("taken")
        };
        assert_eq!(
            *refused,
            PgError {
                severity: "ERROR".into(),
                code: "42P04".into(),
                message: "database \"shop\" already exists".into(),
                detail: Some("more".into()),
                hint: Some("pick another".into()),
                position: Some("17".into()),
            }
        );
        assert!(refused.is_taken());
        assert_eq!(
            refused.to_string(),
            "PostgreSQL said: database \"shop\" already exists (42P04): more pick another"
        );
        assert_eq!(session.simple("SELECT 1").unwrap()[0].tag, "SELECT 1");
    }

    #[test]
    fn each_bound_ends_the_session() {
        // A message over 1 MiB.
        let mut sent = welcome();
        sent.push(b'D');
        sent.extend_from_slice(&i32::try_from(MESSAGE_MAX + 5).unwrap().to_be_bytes());
        let mut over = session(sent).unwrap();
        let Err(Error::Broken(why)) = over.simple("SELECT") else { panic!("read") };
        assert!(why.contains("more than 1 MiB"), "{why}");
        // Every call after fails with the same words.
        let Err(Error::Broken(again)) = over.simple("SELECT") else { panic!("read") };
        assert_eq!(again, why);
        // More rows than allowed.
        let mut sent = welcome();
        sent.extend(columns(&["n"]));
        for _ in 0..=ROWS_MAX {
            sent.extend(row(&[Some("1")]));
        }
        let Err(Error::Broken(why)) = session(sent).unwrap().simple("SELECT") else {
            panic!("read")
        };
        assert!(why.contains("more than 100000 rows"), "{why}");
        // A length that cannot be.
        let mut sent = welcome();
        sent.extend_from_slice(&[b'C', 0, 0, 0, 2]);
        assert!(matches!(session(sent).unwrap().simple("SELECT"), Err(Error::Broken(_))));
        // The server closing, or going quiet, which a socket's timeout says.
        let Err(Error::Broken(why)) = session(welcome()).unwrap().simple("SELECT") else {
            panic!()
        };
        assert_eq!(why, "it closed the connection");
        assert_eq!(quiet_or(&io::ErrorKind::WouldBlock.into()), "it sent nothing for 30 seconds");
    }

    #[test]
    fn copying_binary_columns_and_short_rows_are_refused() {
        for kind in *b"GHW" {
            let sent = [welcome(), message(kind, &[0, 0, 0])].concat();
            let Err(Error::Broken(why)) = session(sent).unwrap().simple("COPY x FROM STDIN") else {
                panic!()
            };
            assert!(why.contains("copy"), "{why}");
        }
        let mut binary = columns(&["n"]);
        let at = binary.len() - 1;
        binary[at] = 1;
        let sent = [welcome(), binary].concat();
        assert!(matches!(session(sent).unwrap().simple("SELECT"), Err(Error::Broken(_))));
        let sent = [welcome(), columns(&["a", "b"]), row(&[Some("1")])].concat();
        assert!(matches!(session(sent).unwrap().simple("SELECT"), Err(Error::Broken(_))));
        let sent = [welcome(), columns(&["a"]), message(b'D', &[0, 1, 0, 0, 0, 9, b'x'])].concat();
        let mut cut = session(sent).unwrap();
        let Err(Error::Broken(why)) = cut.simple("SELECT") else { panic!("read") };
        assert!(why.contains("cut short"), "{why}");
        // What follows is out of step: the session is over.
        assert!(matches!(cut.simple("SELECT"), Err(Error::Broken(again)) if again == why));
    }

    #[test]
    fn a_statement_with_a_nul_is_not_sent() {
        let mut session = session(welcome()).unwrap();
        let before = session.stream.written.len();
        assert!(matches!(session.simple("SELECT '\0'"), Err(Error::Broken(_))));
        assert_eq!(session.stream.written.len(), before);
        assert!(session.broken.is_none());
    }

    #[test]
    fn names_and_strings_are_quoted_so_that_they_stay_one_name_and_one_string() {
        assert_eq!(ident("shop").unwrap(), "\"shop\"");
        assert_eq!(ident("a\"b").unwrap(), "\"a\"\"b\"");
        assert_eq!(literal("it's").unwrap(), "'it''s'");
        assert_eq!(literal("a\\'; DROP ROLE x; --").unwrap(), "'a\\''; DROP ROLE x; --'");
        assert_eq!(literal("").unwrap(), "''");
        assert!(ident("a\0b").is_err());
        assert!(ident("").is_err());
        assert!(literal("a\0b").is_err());
    }

    /// A stream whose written bytes outlive it.
    struct Kept(Cursor<Vec<u8>>, Rc<RefCell<Vec<u8>>>);

    impl Read for Kept {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            self.0.read(buf)
        }
    }

    impl Write for Kept {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.1.borrow_mut().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn a_dropped_session_says_terminate_and_one_that_ended_says_nothing() {
        let written = Rc::new(RefCell::new(Vec::new()));
        let session = Session::start(Kept(Cursor::new(welcome()), written.clone())).unwrap();
        drop(session);
        assert!(written.borrow().ends_with(&[b'X', 0, 0, 0, 4]));
        let written = Rc::new(RefCell::new(Vec::new()));
        let mut session = Session::start(Kept(Cursor::new(welcome()), written.clone())).unwrap();
        // The server has closed: the session is over.
        assert!(session.simple("SELECT").is_err());
        let before = written.borrow().len();
        drop(session);
        assert_eq!(written.borrow().len(), before);
    }
}
