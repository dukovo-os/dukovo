// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The reports server's protocol (ADR 0531), over HTTPS with the
//! system's root certificates and those report.toml adds:
//!
//! | request | headers | answer |
//! |---|---|---|
//! | `POST /v1/reports`, the text | `Content-Type`, `Report-Key-Hash`, `Report-Box: security` for the security box | 201 `{"number": N, "status": "received"}` |
//! | `GET /v1/reports/N` | `Report-Key` | 200 `{"number", "status", "duplicate_of", "fixed_in", "reason"}` |
//! | `DELETE /v1/reports/N` | `Report-Key` | 204 |
//!
//! Each report has a key of its own, 32 random bytes made when it is
//! queued: the server keeps only its SHA-256, and only the key's holder
//! may ask after the report or delete it. Nothing else is sent: no
//! cookie, no account, no address of the machine but the one the
//! connection comes from, which the server does not keep.

use std::io::{self, Read, Write};
use std::net::{SocketAddr, ToSocketAddrs};
use std::sync::Arc;
use std::time::Duration;

use rustls::ClientConfig;
use serde::Deserialize;

use super::settings::Settings;
use crate::tls;

/// How long a connection, or each read and write, may take.
const TIMEOUT: Duration = Duration::from_secs(20);
/// The most of an answer read: a status is a few hundred bytes.
const ANSWER_MAX: u64 = 64 * 1024;

/// Why the server was not reached, or what it said instead.
#[derive(Debug, PartialEq, Eq)]
pub enum Failure {
    /// The network or the server did not answer: worth trying again.
    Unreachable(String),
    /// The server answered with a status this did not ask for, and its
    /// words: a 5xx is worth trying again, a 4xx is not.
    Refused(u16, String),
}

impl Failure {
    /// Whether trying again later may work.
    pub fn passing(&self) -> bool {
        match self {
            Failure::Unreachable(_) => true,
            Failure::Refused(status, _) => *status >= 500 || *status == 429,
        }
    }
}

impl std::fmt::Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Failure::Unreachable(why) => f.write_str(why),
            Failure::Refused(status, words) if words.is_empty() => {
                write!(f, "the server answered {status}")
            }
            Failure::Refused(status, words) => write!(f, "the server answered {status}: {words}"),
        }
    }
}

/// What the server says of a report.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct Standing {
    pub number: u64,
    pub status: String,
    pub duplicate_of: Option<u64>,
    pub fixed_in: Option<String>,
    pub reason: Option<String>,
}

/// A connection's settings for the server report.toml names.
pub struct Server {
    host: String,
    port: u16,
    config: Arc<ClientConfig>,
    version: String,
}

impl Server {
    /// The server `settings` name, trusting the system's roots and its `ca`.
    pub fn new(settings: &Settings) -> Server {
        let url = tuf::Url::parse(&settings.server).ok();
        let host = url.as_ref().map_or_else(String::new, |url| url.host().to_string());
        let port = url.as_ref().and_then(tuf::Url::port).unwrap_or(443);
        let config = tls::verified(tls::roots(&settings.ca, "reportd"), &[b"http/1.1"]);
        Server { host, port, config, version: env!("CARGO_PKG_VERSION").into() }
    }

    /// Sends a report's exact `bytes`, encrypted to the security team when
    /// `security`, with the hash of its key: the number the server gives.
    ///
    /// # Errors
    ///
    /// [`Failure`], and whether trying again may work.
    pub fn send(&self, bytes: &[u8], security: bool, key_hash: &str) -> Result<u64, Failure> {
        let kind =
            if security { "application/age-encryption" } else { "text/plain; charset=utf-8" };
        let mut headers = vec![("Content-Type", kind), ("Report-Key-Hash", key_hash)];
        if security {
            headers.push(("Report-Box", "security"));
        }
        let (status, body) = self.exchange("POST", "/v1/reports", &headers, bytes)?;
        if status != 201 {
            return Err(Failure::Refused(status, words(&body)));
        }
        let standing: Standing = serde_json::from_slice(&body)
            .map_err(|_| Failure::Refused(status, "an answer that is not a report's".into()))?;
        if standing.number == 0 {
            return Err(Failure::Refused(status, "an answer with no report number".into()));
        }
        Ok(standing.number)
    }

    /// What the server says of report `number`, asked with its `key`.
    ///
    /// # Errors
    ///
    /// [`Failure`].
    pub fn status(&self, number: u64, key: &str) -> Result<Standing, Failure> {
        let path = format!("/v1/reports/{number}");
        let (status, body) = self.exchange("GET", &path, &[("Report-Key", key)], &[])?;
        if status != 200 {
            return Err(Failure::Refused(status, words(&body)));
        }
        serde_json::from_slice(&body)
            .map_err(|_| Failure::Refused(status, "an answer that is not a report's".into()))
    }

    /// Deletes report `number` from the server, with its `key`.
    ///
    /// # Errors
    ///
    /// [`Failure`]; a report the server no longer has is deleted already.
    pub fn delete(&self, number: u64, key: &str) -> Result<(), Failure> {
        let path = format!("/v1/reports/{number}");
        let (status, body) = self.exchange("DELETE", &path, &[("Report-Key", key)], &[])?;
        match status {
            200 | 204 | 404 | 410 => Ok(()),
            _ => Err(Failure::Refused(status, words(&body))),
        }
    }

    /// Sends one request and reads its whole answer: its status and body.
    fn exchange(
        &self,
        method: &str,
        path: &str,
        headers: &[(&str, &str)],
        body: &[u8],
    ) -> Result<(u16, Vec<u8>), Failure> {
        let unreachable = |err: io::Error| Failure::Unreachable(crate::util::strip_os_error(&err));
        let mut stream = self.connect().map_err(unreachable)?;
        let request = request(method, path, &self.authority(), &self.version, headers, body.len());
        stream.write_all(request.as_bytes()).map_err(unreachable)?;
        stream.write_all(body).map_err(unreachable)?;
        stream.flush().map_err(unreachable)?;
        let mut answer = Vec::new();
        // A server that closes without TLS's close_notify still sent all of
        // a Content-Length answer, which `parse` checks.
        match Read::by_ref(&mut stream).take(ANSWER_MAX).read_to_end(&mut answer) {
            Ok(_) => {}
            Err(err) if err.kind() == io::ErrorKind::UnexpectedEof && !answer.is_empty() => {}
            Err(err) => return Err(unreachable(err)),
        }
        parse(&answer).ok_or_else(|| Failure::Unreachable("the server's answer is cut off".into()))
    }

    /// The Host header: the host, and the port when it is not 443.
    fn authority(&self) -> String {
        if self.port == 443 { self.host.clone() } else { format!("{}:{}", self.host, self.port) }
    }

    /// A TLS connection to the first of the server's addresses that takes
    /// one.
    fn connect(&self) -> io::Result<tls::Stream> {
        if self.host.is_empty() {
            return Err(io::Error::other("report.toml names no server"));
        }
        let bare = self.host.trim_start_matches('[').trim_end_matches(']');
        let addresses: Vec<SocketAddr> = (bare, self.port).to_socket_addrs()?.collect();
        let literal = bare.parse::<std::net::IpAddr>().is_ok();
        let mut last = io::Error::other(format!("{bare} has no address"));
        for address in addresses {
            let name = tls::server_name((!literal).then_some(bare), address)?;
            match tls::connect(address, name, &self.config, TIMEOUT) {
                Ok(stream) => return Ok(stream),
                Err(err) => last = err,
            }
        }
        Err(last)
    }
}

/// A request's head.
fn request(
    method: &str,
    path: &str,
    host: &str,
    version: &str,
    headers: &[(&str, &str)],
    length: usize,
) -> String {
    let mut head = format!(
        "{method} {path} HTTP/1.1\r\nHost: {host}\r\nUser-Agent: dukovo-report/{version}\r\n\
         Accept: application/json\r\nConnection: close\r\nContent-Length: {length}\r\n"
    );
    for (name, value) in headers {
        head.push_str(&format!("{name}: {value}\r\n"));
    }
    head.push_str("\r\n");
    head
}

/// An answer's status and body: by its Content-Length, or chunked, or to
/// its end; none when it is cut off or not HTTP.
fn parse(answer: &[u8]) -> Option<(u16, Vec<u8>)> {
    let end = answer.windows(4).position(|w| w == b"\r\n\r\n")?;
    let head = std::str::from_utf8(&answer[..end]).ok()?;
    let rest = &answer[end + 4..];
    let mut lines = head.split("\r\n");
    let status: u16 = lines.next()?.strip_prefix("HTTP/1.")?.get(2..5)?.parse().ok()?;
    let mut length = None;
    let mut chunked = false;
    for line in lines {
        let (name, value) = line.split_once(':')?;
        let value = value.trim();
        if name.eq_ignore_ascii_case("content-length") {
            length = Some(value.parse::<usize>().ok()?);
        } else if name.eq_ignore_ascii_case("transfer-encoding") {
            chunked = value.eq_ignore_ascii_case("chunked");
        }
    }
    let body = if chunked {
        dechunk(rest)?
    } else if let Some(length) = length {
        rest.get(..length)?.to_vec()
    } else {
        rest.to_vec()
    };
    Some((status, body))
}

/// A chunked body's bytes, if it is whole.
fn dechunk(mut rest: &[u8]) -> Option<Vec<u8>> {
    let mut body = Vec::new();
    loop {
        let line = rest.windows(2).position(|w| w == b"\r\n")?;
        let size = std::str::from_utf8(&rest[..line]).ok()?;
        let size = usize::from_str_radix(size.split(';').next()?.trim(), 16).ok()?;
        rest = &rest[line + 2..];
        if size == 0 {
            return Some(body);
        }
        body.extend_from_slice(rest.get(..size)?);
        rest = rest.get(size + 2..)?;
    }
}

/// The server's words in an answer's body, for a line of the log: its
/// `error` when it is JSON with one, else its start.
fn words(body: &[u8]) -> String {
    #[derive(Deserialize)]
    struct Said {
        error: String,
    }
    if let Ok(said) = serde_json::from_slice::<Said>(body) {
        return crate::notifyd::clean(&said.error, 200, false);
    }
    crate::notifyd::clean(&String::from_utf8_lossy(&body[..body.len().min(200)]), 200, false)
}

/// The hex SHA-256 of `key`, which the server keeps in its place.
pub fn key_hash(key: &[u8]) -> String {
    hex(ring::digest::digest(&ring::digest::SHA256, key).as_ref())
}

/// `bytes` in lowercase hex.
pub fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    bytes.iter().fold(String::with_capacity(bytes.len() * 2), |mut out, b| {
        let _ = write!(out, "{b:02x}");
        out
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_answer_is_read_by_its_length_or_its_chunks() {
        let answer = b"HTTP/1.1 201 Created\r\nContent-Type: application/json\r\n\
                       Content-Length: 35\r\n\r\n{\"number\": 7, \"status\": \"received\"}";
        let (status, body) = parse(answer).unwrap();
        assert_eq!(status, 201);
        let standing: Standing = serde_json::from_slice(&body).unwrap();
        assert_eq!((standing.number, standing.status.as_str()), (7, "received"));
        let chunked = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n4\r\nabcd\r\n2\r\nef\r\n0\r\n\r\n";
        assert_eq!(parse(chunked).unwrap(), (200, b"abcdef".to_vec()));
        // Shorter than it says: cut off.
        assert!(parse(b"HTTP/1.1 200 OK\r\nContent-Length: 10\r\n\r\nabc").is_none());
        assert!(parse(b"garbage").is_none());
    }

    #[test]
    fn a_request_names_the_client_and_nothing_of_the_machine() {
        let head = request(
            "POST",
            "/v1/reports",
            "reports.dukovo.org",
            "0.16.0",
            &[("Content-Type", "text/plain; charset=utf-8"), ("Report-Key-Hash", "ab")],
            12,
        );
        assert_eq!(
            head,
            "POST /v1/reports HTTP/1.1\r\nHost: reports.dukovo.org\r\n\
             User-Agent: dukovo-report/0.16.0\r\nAccept: application/json\r\nConnection: close\r\n\
             Content-Length: 12\r\nContent-Type: text/plain; charset=utf-8\r\nReport-Key-Hash: ab\r\n\r\n"
        );
    }

    #[test]
    fn only_a_server_fault_or_no_answer_is_worth_another_try() {
        assert!(Failure::Unreachable("connection refused".into()).passing());
        assert!(Failure::Refused(503, String::new()).passing());
        assert!(!Failure::Refused(413, "too long".into()).passing());
        assert_eq!(words(br#"{"error": "the report is too long"}"#), "the report is too long");
    }

    #[test]
    fn the_key_hash_is_sha256_in_hex() {
        assert_eq!(
            key_hash(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }
}
