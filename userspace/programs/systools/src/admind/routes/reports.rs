// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The one-time link to a report's text (ADR 0568): the Report a problem
//! page's Download by QR code.
//!
//! `POST /api/1/reports/drafts/{draft}/phone`, with `{sha256}`, has the
//! person's worker run `report text DRAFT --sha256 HEX`, which reportd
//! answers only for the person whose draft it is and only while its text
//! is the one the page read, and keeps what it printed as a link
//! ([`super::super::once`]). The answer is the link's address, by an
//! address of this machine a phone on its network reaches (a phone may not
//! resolve the machine's name), when it ends, and the fingerprint of the
//! certificate the console serves, which the phone's warning shows.
//!
//! `GET /once/{token}/report.txt` needs no session: the phone has none.
//! The first request gets the text as a file to keep; a later one is told
//! in plain words that the link was used (410) and a token that is not
//! kept that there is nothing (404), both with ADM-140, which says why
//! either can be. Each counts against the address's tries, as a sign-in
//! does, and the log names the address, never the token.

use std::net::IpAddr;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use http::StatusCode;
use serde::Deserialize;
use serde_json::{Value, json};

use super::{Content, Context, Live, Outcome, Reply, Text};
use crate::admind::errors::{Failure, Kind};
use crate::admind::log;
use crate::admind::once::{self, Fetched, Refused};
use crate::admind::reach::{self, Network};
use crate::admind::run;

/// The error IDs `report text` gives, which the answer carries as its own.
const REPORT_ERRORS: [&str; 2] = ["RPT-101", "RPT-102"];

/// What the phone is told for a link that is not there, or was used.
pub const GONE_WORDS: &str = "This one-time link was used or has ended (ADM-140).

A link to a report works once, for 10 minutes. This one was opened \
already (a program that only looks at a link, as a chat program does to \
show a preview, opens it too), or its 10 minutes have passed, or the \
console restarted since it was made.

To make another, choose Download by QR code on the Report a problem page \
again. The report was not changed.
";

/// What the phone is told when its address has asked too often.
pub const TRIES_WORDS: &str = "Too many tries from this address (ADM-105): wait a minute, then \
open the link again.\n";

/// The body of a request for a link.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Asked {
    sha256: Text,
}

/// Whether `text` is `length` lowercase hex digits.
fn is_hex(text: &str, length: usize) -> bool {
    text.len() == length && text.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// `POST /api/1/reports/drafts/{draft}/phone`.
pub(super) async fn phone(cx: &Context<'_>, live: &Live) -> Outcome {
    let draft = cx.params.get("draft").and_then(Value::as_str).unwrap_or_default();
    if !is_hex(draft, 32) {
        let why = "a draft's ID is the 32 hex digits reportd gave it";
        return Err(Failure::bad_value("draft", why));
    }
    let asked: Asked = cx.json()?;
    if !is_hex(&asked.sha256, 64) {
        let why = "the hash is the 64 hex digits of the draft's SHA-256, as reportd gave it";
        return Err(Failure::bad_value("sha256", why));
    }
    let argv: Vec<String> =
        ["report", "text", draft, "--sha256", &asked.sha256].map(String::from).to_vec();
    let ran = run::as_the_person(&live.link, &argv, None).await?;
    if ran.status != 0 {
        return Err(run::command_failed(&argv, &REPORT_ERRORS, &ran));
    }
    if ran.output.is_empty() || ran.output.len() >= run::OUTPUT_MAX {
        let why = "report text printed no report, or more than a report may hold";
        return Err(Failure::new(Kind::Internal, why));
    }
    let now = Instant::now();
    let made = match cx.shared.once.add(&live.cookie, ran.output.into_bytes().into(), now) {
        Ok(made) => made,
        Err(Refused::Session(seconds)) => {
            let why = format!(
                "this session has {} one-time links already: one ends in {seconds} s",
                once::EACH
            );
            return Err(Failure::wait(why, seconds));
        }
        Err(Refused::All(seconds)) => {
            let why = format!("the console keeps {} one-time links already", once::ALL);
            return Err(Failure::wait(why, seconds));
        }
        Err(Refused::NoRandom) => {
            return Err(Failure::new(Kind::Internal, "no random token could be made"));
        }
    };
    let host = phone_host(cx.host.as_str(), &cx.shared.identity.borrow().networks);
    let url = format!("https://{host}:{}/once/{}/report.txt", cx.shared.port, made.token);
    let expires_ms = SystemTime::now()
        .checked_add(made.until.saturating_duration_since(now))
        .and_then(|at| at.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |at| u64::try_from(at.as_millis()).unwrap_or(u64::MAX));
    let fingerprint = cx.shared.certificate.borrow().fingerprint.clone();
    log(6, &format!("{} made a one-time link to a report's text", live.name));
    Ok(Reply::json(json!({
        "url": url,
        "expires_ms": expires_ms,
        "fingerprint_sha256": fingerprint,
    })))
}

/// The host a phone is sent to: the address the browser used, when it is
/// one a phone on its network can reach too; else the first private
/// address of the machine's cards, IPv4 first; else the name the browser
/// used.
fn phone_host(browser: &str, networks: &[Network]) -> String {
    let usable = |ip: IpAddr| {
        let ip = reach::plain(ip);
        let link_local = match ip {
            IpAddr::V4(v4) => v4.is_link_local(),
            IpAddr::V6(v6) => v6.segments()[0] & 0xffc0 == 0xfe80,
        };
        !ip.is_loopback() && !ip.is_unspecified() && !link_local && reach::private_range(ip)
    };
    let bare = browser.trim_start_matches('[').trim_end_matches(']');
    if let Ok(ip) = bare.parse::<IpAddr>()
        && usable(ip)
    {
        return shown(reach::plain(ip));
    }
    let cards = || networks.iter().map(|network| reach::plain(network.ip)).filter(|ip| usable(*ip));
    cards()
        .find(IpAddr::is_ipv4)
        .or_else(|| cards().next())
        .map_or_else(|| browser.to_string(), shown)
}

/// `ip` as an address's host: IPv6 in brackets.
fn shown(ip: IpAddr) -> String {
    match ip {
        IpAddr::V4(v4) => v4.to_string(),
        IpAddr::V6(v6) => format!("[{v6}]"),
    }
}

/// `GET` or, with `head`, `HEAD /once/{token}/report.txt`.
pub(super) fn fetch(cx: &Context<'_>, head: bool) -> Outcome {
    if let Err(seconds) = cx.shared.tries.borrow_mut().take(reach::counted(cx.peer), Instant::now())
    {
        let mut reply = plain(StatusCode::TOO_MANY_REQUESTS, TRIES_WORDS);
        reply.retry_after = Some(seconds);
        return Ok(reply);
    }
    let token = cx.params.get("token").and_then(Value::as_str).unwrap_or_default();
    let fetched = if once::is_token(token) {
        cx.shared.once.fetch(token, Instant::now())
    } else {
        Fetched::Unknown
    };
    match fetched {
        Fetched::Text(delivery) => {
            log(6, &format!("a one-time link to a report's text was opened from {}", cx.peer));
            if head {
                let length = delivery.bytes();
                delivery.taken();
                return Ok(Reply {
                    status: StatusCode::OK,
                    content: Content::OnceHead(length),
                    cookies: Vec::new(),
                    retry_after: None,
                });
            }
            Ok(Reply {
                status: StatusCode::OK,
                content: Content::Once(delivery.body()),
                cookies: Vec::new(),
                retry_after: None,
            })
        }
        Fetched::Used => {
            log(6, &format!("a used one-time link was asked for again from {}", cx.peer));
            Ok(plain(StatusCode::GONE, GONE_WORDS))
        }
        Fetched::Unknown => Ok(plain(StatusCode::NOT_FOUND, GONE_WORDS)),
    }
}

/// A plain answer of `words`, with `status`.
fn plain(status: StatusCode, words: &'static str) -> Reply {
    Reply { status, content: Content::Plain(words), cookies: Vec::new(), retry_after: None }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{Ipv4Addr, Ipv6Addr};

    fn card(ip: IpAddr) -> Network {
        Network { ip, prefix: 24 }
    }

    #[test]
    fn a_phone_is_sent_to_the_browser_s_address_or_the_first_private_card_ipv4_first() {
        let lan = IpAddr::V4(Ipv4Addr::new(192, 168, 1, 20));
        let ula = IpAddr::V6("fd00::20".parse::<Ipv6Addr>().unwrap());
        let cards = [card(IpAddr::V4(Ipv4Addr::LOCALHOST)), card(ula), card(lan)];
        assert_eq!(phone_host("10.0.0.5", &cards), "10.0.0.5");
        assert_eq!(phone_host("[fd00::7]", &cards), "[fd00::7]");
        assert_eq!(phone_host("nas.local", &cards), "192.168.1.20");
        assert_eq!(phone_host("127.0.0.1", &cards), "192.168.1.20");
        // A public address may be behind a router the phone is not.
        assert_eq!(phone_host("203.0.113.9", &cards), "192.168.1.20");
        assert_eq!(phone_host("nas.local", &[card(ula)]), "[fd00::20]");
        let only_link_local = [card(IpAddr::V4(Ipv4Addr::new(169, 254, 3, 4)))];
        assert_eq!(phone_host("nas.local", &only_link_local), "nas.local");
    }

    #[test]
    fn a_draft_and_a_hash_are_small_hex_of_their_length() {
        assert!(is_hex(&"0a".repeat(16), 32));
        assert!(!is_hex(&"0A".repeat(16), 32));
        assert!(!is_hex(&"0a".repeat(16), 64));
    }

    #[test]
    fn the_words_for_a_used_link_name_its_error() {
        assert!(GONE_WORDS.starts_with("This one-time link was used or has ended (ADM-140)."));
        assert!(GONE_WORDS.contains(&format!("for {} minutes", once::MINUTES)));
    }
}
