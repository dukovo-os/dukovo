// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! whois(1): who holds a domain, an address block or an AS number, from
//! the registries' WHOIS servers (RFC 3912, ADR 0341).
//!
//! A question is a line sent over TCP to port 43, and the answer is what
//! the server writes until it closes the connection. Without `-h`, whois
//! asks IANA's server, whose answer names the registry that holds the
//! object (`refer:`), and follows the referrals from there: a registry's
//! `whois:` or `ReferralServer:`, and a thin registry's `Registrar WHOIS
//! Server:`, up to [`HOPS`] servers, never one twice. It shows every
//! answer, each after a line naming its server, as FreeBSD's whois does;
//! `-h` asks the one server named, and follows nothing.

use std::fmt::Write as _;
use std::io::{self, Read, Write};
use std::net::{IpAddr, TcpStream, ToSocketAddrs};
use std::time::Duration;

use crate::args::{Arg, Args};
use crate::util::{Fail, fail_with, usage};

/// WHOIS's port.
const PORT: u16 = 43;

/// The server asked first: IANA's, which knows every registry.
const IANA: &str = "whois.iana.org";

/// The most servers one question goes to.
const HOPS: usize = 4;

/// How long to wait for a server to connect and to answer.
const TIMEOUT: Duration = Duration::from_secs(15);

/// The most of an answer kept: more is a server that does not stop.
const ANSWER_MAX: u64 = 1 << 20;

/// Asks `server` at `port` about `query`, and gives what it answered.
fn ask(server: &str, port: u16, query: &str) -> io::Result<String> {
    let mut last = io::Error::new(io::ErrorKind::NotFound, "no address");
    for address in (server, port).to_socket_addrs()? {
        match TcpStream::connect_timeout(&address, TIMEOUT) {
            Ok(mut stream) => {
                stream.set_read_timeout(Some(TIMEOUT))?;
                stream.set_write_timeout(Some(TIMEOUT))?;
                stream.write_all(format!("{query}\r\n").as_bytes())?;
                let mut answer = Vec::new();
                stream.take(ANSWER_MAX).read_to_end(&mut answer)?;
                return Ok(String::from_utf8_lossy(&answer).replace("\r\n", "\n"));
            }
            Err(e) => last = e,
        }
    }
    Err(last)
}

/// The server an answer refers the question to, if it names one.
fn referral(answer: &str) -> Option<String> {
    for line in answer.lines() {
        let Some((key, value)) = line.trim_start().split_once(':') else { continue };
        let key = key.trim().to_ascii_lowercase();
        if !matches!(key.as_str(), "refer" | "whois" | "referralserver" | "registrar whois server")
        {
            continue;
        }
        // ARIN's are URLs: whois://whois.ripe.net, or rwhois:// (not
        // followed); the others bare names.
        let value = value.trim();
        let host = match value.split_once("://") {
            Some(("whois", rest)) => rest,
            Some(_) => continue,
            None => value,
        };
        let host = host.trim_end_matches('/');
        // A port after the name is the server's own, not WHOIS's.
        let host = host
            .rsplit_once(':')
            .filter(|(_, p)| p.parse::<u16>().is_ok())
            .map_or(host, |(h, _)| h);
        if !host.is_empty()
            && host.bytes().all(|b| b.is_ascii_alphanumeric() || b"-._".contains(&b))
        {
            return Some(host.to_ascii_lowercase());
        }
    }
    None
}

/// The question as `server` takes it: ARIN's wants `n + ` before an
/// address, for the networks that hold it rather than a summary.
fn query_for(server: &str, object: &str) -> String {
    if server == "whois.arin.net" && object.parse::<IpAddr>().is_ok() {
        format!("n + {object}")
    } else {
        object.to_string()
    }
}

/// Runs whois.
pub fn main(args: &mut Args) -> Result<i32, Fail> {
    let mut server: Option<String> = None;
    let mut port = PORT;
    let mut verbose = false;
    let mut objects = Vec::new();
    while let Some(arg) = args.next()? {
        match arg {
            Arg::Short('h') => server = Some(args.text()?),
            Arg::Short('p') => port = args.number()?,
            Arg::Long(name) if name == "verbose" => verbose = true,
            Arg::Long(name) if name == "help" => {
                print!("{HELP}");
                return Ok(0);
            }
            Arg::Long(name) if name == "host" => server = Some(args.text()?),
            Arg::Long(name) if name == "port" => port = args.number()?,
            Arg::Operand(object) => objects.push(object.to_string_lossy().into_owned()),
            other => return Err(args.unknown(&other)),
        }
    }
    if objects.is_empty() {
        return Err(usage("missing operand: what to look up".into()));
    }
    // Several words are one question, as whois(1) joins them.
    let object = objects.join(" ");
    let mut stdout = io::stdout().lock();
    let mut asked: Vec<String> = Vec::new();
    let mut next = Some(server.clone().unwrap_or_else(|| IANA.to_string()));
    while let Some(host) = next.take() {
        let query = query_for(&host, &object);
        if verbose {
            eprintln!("whois: asking {host} for '{query}'");
        }
        let answer = match ask(&host, port, &query) {
            Ok(answer) => answer,
            Err(e) if asked.is_empty() => {
                return Err(fail_with(2, format!("{host}: {}", crate::util::strip_os_error(&e))));
            }
            Err(e) => {
                // A registrar's server that does not answer leaves what the
                // registry said.
                eprintln!("whois: {host}: {}", crate::util::strip_os_error(&e));
                break;
            }
        };
        let mut out = String::new();
        if !asked.is_empty() {
            out.push('\n');
        }
        if server.is_none() {
            let _ = writeln!(out, "% {host}:\n");
        }
        out.push_str(&answer);
        if !answer.ends_with('\n') {
            out.push('\n');
        }
        let _ = stdout.write_all(out.as_bytes());
        asked.push(host);
        if server.is_some() || asked.len() >= HOPS {
            break;
        }
        next = referral(&answer).filter(|r| !asked.contains(r));
    }
    Ok(0)
}

const HELP: &str = "\
Usage: whois [-h HOST] [-p PORT] [--verbose] OBJECT...
Asks the WHOIS servers who holds a domain, an address or an AS number,
from IANA's server on, following each server's referral.
  -h, --host HOST  ask this server alone, and follow nothing
  -p, --port PORT  ask at this port, not 43
  --verbose        say which server is asked, on standard error
";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn referrals_are_found_in_each_registry_form() {
        assert_eq!(
            referral("% IANA WHOIS server\nrefer:        whois.verisign-grs.com\n").as_deref(),
            Some("whois.verisign-grs.com")
        );
        assert_eq!(
            referral("   Registrar WHOIS Server: whois.markmonitor.com\n").as_deref(),
            Some("whois.markmonitor.com")
        );
        assert_eq!(
            referral("ReferralServer:  whois://whois.ripe.net\n").as_deref(),
            Some("whois.ripe.net")
        );
        assert_eq!(referral("ReferralServer: rwhois://rwhois.example.net:4321\n"), None);
        assert_eq!(referral("whois: whois.nic.example:43\n").as_deref(), Some("whois.nic.example"));
        assert_eq!(referral("domain: example.com\nrefer: \n"), None);
        assert_eq!(referral("refer: bad host!\n"), None);
    }

    #[test]
    fn arin_is_asked_for_networks() {
        assert_eq!(query_for("whois.arin.net", "192.0.2.1"), "n + 192.0.2.1");
        assert_eq!(query_for("whois.arin.net", "AS64496"), "AS64496");
        assert_eq!(query_for("whois.ripe.net", "192.0.2.1"), "192.0.2.1");
    }
}
