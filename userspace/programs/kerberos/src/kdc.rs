// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Talking to a KDC (RFC 4120 7.2): over TCP, each message after its
//! length in 4 bytes, and over UDP where a KDC takes no TCP. Each KDC of
//! the realm is tried in krb5.conf's order until one answers; MIT's
//! `tcp/` and `udp/` before a KDC's name keep to the one.
//!
//! When no KDC answers, the error names each and why: a refused
//! connection means no KDC listens there, a timeout that a firewall is
//! in the way. `ss -t` shows the connection while it is open.

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream, ToSocketAddrs, UdpSocket};
use std::time::Duration;

use crate::{Error, Result};

/// Kerberos's port.
const PORT: u16 = 88;
/// How long a connection or an answer may take.
const WAIT: Duration = Duration::from_secs(5);
/// The longest answer taken over TCP: far more than any ticket needs.
const MOST: usize = 1 << 20;

/// How to reach one KDC.
#[derive(Debug, PartialEq, Eq)]
struct Kdc<'a> {
    /// Its host's name or address.
    host: &'a str,
    /// Its port.
    port: u16,
    /// Whether to try TCP.
    tcp: bool,
    /// Whether to try UDP.
    udp: bool,
}

/// A KDC as krb5.conf names it: `[tcp/|udp/]host[:port]`, where an IPv6
/// address with a port is in brackets.
fn parse(text: &str) -> Kdc<'_> {
    let (tcp, udp, rest) = if let Some(rest) = text.strip_prefix("tcp/") {
        (true, false, rest)
    } else if let Some(rest) = text.strip_prefix("udp/") {
        (false, true, rest)
    } else {
        (true, true, text)
    };
    let (host, port) = if let Some(inner) = rest.strip_prefix('[') {
        match inner.split_once(']') {
            Some((host, after)) => (host, after.strip_prefix(':').and_then(|p| p.parse().ok())),
            None => (inner, None),
        }
    } else if rest.matches(':').count() == 1 {
        let (host, port) = rest.split_once(':').expect("one colon");
        (host, port.parse().ok())
    } else {
        (rest, None)
    };
    Kdc { host, port: port.unwrap_or(PORT), tcp, udp }
}

/// `request` sent to the first of `kdcs` that answers: its answer.
pub fn send(kdcs: &[String], request: &[u8]) -> Result<Vec<u8>> {
    let mut failures = Vec::new();
    for text in kdcs {
        let kdc = parse(text);
        let addresses: Vec<SocketAddr> = match (kdc.host, kdc.port).to_socket_addrs() {
            Ok(found) => found.collect(),
            Err(err) => {
                failures.push(format!("{text}: {err}"));
                continue;
            }
        };
        for address in addresses {
            if kdc.tcp {
                match over_tcp(address, request) {
                    Ok(answer) => return Ok(answer),
                    Err(err) => failures.push(format!("{text} ({address}) over TCP: {err}")),
                }
            }
            if kdc.udp {
                match over_udp(address, request) {
                    Ok(answer) => return Ok(answer),
                    Err(err) => failures.push(format!("{text} ({address}) over UDP: {err}")),
                }
            }
        }
    }
    Err(Error::Other(format!("no KDC answered: {}", failures.join("; "))))
}

/// `request` over TCP to `address`.
fn over_tcp(address: SocketAddr, request: &[u8]) -> std::io::Result<Vec<u8>> {
    let mut stream = TcpStream::connect_timeout(&address, WAIT)?;
    stream.set_read_timeout(Some(WAIT))?;
    stream.set_write_timeout(Some(WAIT))?;
    let mut message = (request.len() as u32).to_be_bytes().to_vec();
    message.extend_from_slice(request);
    stream.write_all(&message)?;
    let mut length = [0u8; 4];
    stream.read_exact(&mut length)?;
    let length = u32::from_be_bytes(length) as usize;
    // The top bit would say the KDC wants a later version of the
    // protocol, which none does.
    if length > MOST {
        return Err(std::io::Error::other(format!("an answer of {length} bytes")));
    }
    let mut answer = vec![0; length];
    stream.read_exact(&mut answer)?;
    Ok(answer)
}

/// `request` over UDP to `address`: one datagram each way, asked twice
/// before giving up, since a datagram may be lost.
fn over_udp(address: SocketAddr, request: &[u8]) -> std::io::Result<Vec<u8>> {
    let local: SocketAddr =
        if address.is_ipv4() { "0.0.0.0:0".parse() } else { "[::]:0".parse() }.expect("an address");
    let socket = UdpSocket::bind(local)?;
    socket.connect(address)?;
    socket.set_read_timeout(Some(WAIT / 2))?;
    let mut answer = vec![0; 65_536];
    let mut last = std::io::Error::other("no answer");
    for _ in 0..2 {
        socket.send(request)?;
        match socket.recv(&mut answer) {
            Ok(got) => {
                answer.truncate(got);
                return Ok(answer);
            }
            Err(err) => last = err,
        }
    }
    Err(last)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kdcs_read_as_krb5_conf_names_them() {
        assert_eq!(
            parse("kdc.example.org"),
            Kdc { host: "kdc.example.org", port: 88, tcp: true, udp: true }
        );
        assert_eq!(parse("kdc:750"), Kdc { host: "kdc", port: 750, tcp: true, udp: true });
        assert_eq!(
            parse("tcp/10.0.2.2:8888"),
            Kdc { host: "10.0.2.2", port: 8888, tcp: true, udp: false }
        );
        assert_eq!(parse("udp/kdc"), Kdc { host: "kdc", port: 88, tcp: false, udp: true });
        assert_eq!(parse("[fd00::1]:89"), Kdc { host: "fd00::1", port: 89, tcp: true, udp: true });
        assert_eq!(parse("fd00::1"), Kdc { host: "fd00::1", port: 88, tcp: true, udp: true });
    }

    #[test]
    fn tcp_frames_each_message_with_its_length() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut length = [0u8; 4];
            stream.read_exact(&mut length).unwrap();
            let mut request = vec![0; u32::from_be_bytes(length) as usize];
            stream.read_exact(&mut request).unwrap();
            assert_eq!(request, b"as-req");
            stream.write_all(&[0, 0, 0, 6]).unwrap();
            stream.write_all(b"as-rep").unwrap();
        });
        let answer = send(&[address.to_string()], b"as-req").unwrap();
        assert_eq!(answer, b"as-rep");
        server.join().unwrap();
    }

    #[test]
    fn a_kdc_that_is_not_there_is_named_in_the_error() {
        // A port no one listens on: bound, then closed.
        let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        let err = send(&[format!("tcp/127.0.0.1:{port}")], b"x").unwrap_err();
        assert!(err.to_string().contains(&format!("127.0.0.1:{port}")), "{err}");
    }
}
