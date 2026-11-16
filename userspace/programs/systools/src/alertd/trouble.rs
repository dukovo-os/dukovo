// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! What went wrong with a connection to a mail or webhook server, in
//! words for `alerts test` and the log (ADR 0485): the certificate a
//! server showed, the silence of one, and what a server said, cut so that
//! one long answer cannot fill a line.

use std::io;
use std::time::Duration;

use rustls::CertificateError as Certificate;

use crate::util;

/// How long a connection may be silent before alertd gives up.
pub const SILENCE: Duration = Duration::from_secs(30);

/// A server's words cut to `max` characters, with "..." when they were
/// longer, and on one line.
pub fn cut(text: &str, max: usize) -> String {
    let one_line = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut cut: String = one_line.chars().take(max).collect();
    if cut.len() < one_line.len() {
        cut.push_str("...");
    }
    cut
}

/// The ID of the page in the support center for a channel's failure, when
/// its `words` say what kind of failure it was: a mail server that refused
/// the sign-in (ALR-101) or offers no TLS (ALR-102), a webhook that
/// answered with a refusal (ALR-103), a browser that ended its subscription
/// (ALR-104) or a push service that refused the message (ALR-105). Another
/// failure has none.
pub fn error_id(words: &str) -> Option<&'static str> {
    let ends = |id: &str| words.contains(&format!("({id})"));
    if ends("ALR-104") {
        return Some("ALR-104");
    }
    if ends("ALR-105") {
        return Some("ALR-105");
    }
    if words.starts_with("the server refused the password")
        || words.starts_with("the server could not sign in")
    {
        return Some("ALR-101");
    }
    if words.contains("does not offer STARTTLS") {
        return Some("ALR-102");
    }
    let hooks = ["ntfy", "matrix", "slack", "discord", "json"];
    let answered = hooks.iter().any(|kind| {
        words.strip_prefix(kind).is_some_and(|rest| {
            rest.strip_prefix(" answered ")
                .is_some_and(|code| code.starts_with(|c: char| c.is_ascii_digit()))
        })
    });
    answered.then_some("ALR-103")
}

/// Why connecting to `host`, or talking to it, failed with `err`.
/// `trusted` says whose certificates count, as the end of "its
/// certificate is not from an authority ...".
pub fn connection(err: &io::Error, host: &str, trusted: &str) -> String {
    let Some(error) = err.get_ref().and_then(|inner| inner.downcast_ref::<rustls::Error>()) else {
        return match err.kind() {
            io::ErrorKind::UnexpectedEof => "the server closed the connection".to_string(),
            io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut => {
                format!("no answer within {} seconds", SILENCE.as_secs())
            }
            _ => util::strip_os_error(err),
        };
    };
    match error {
        rustls::Error::InvalidCertificate(Certificate::UnknownIssuer) => {
            format!("its certificate is not from an authority {trusted}")
        }
        rustls::Error::InvalidCertificate(
            Certificate::NotValidForName | Certificate::NotValidForNameContext { .. },
        ) => format!("its certificate is not for {host}"),
        rustls::Error::InvalidCertificate(
            Certificate::Expired | Certificate::ExpiredContext { .. },
        ) => "its certificate has expired".into(),
        rustls::Error::InvalidCertificate(
            Certificate::NotValidYet | Certificate::NotValidYetContext { .. },
        ) => "its certificate is not valid yet".into(),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rustls_error(error: rustls::Error) -> io::Error {
        io::Error::new(io::ErrorKind::InvalidData, error)
    }

    #[test]
    fn words_are_cut_to_the_most_and_put_on_one_line() {
        assert_eq!(cut("short", 200), "short");
        assert_eq!(cut("two\n  lines\r\nhere", 200), "two lines here");
        assert_eq!(cut(&"x".repeat(500), 200), format!("{}...", "x".repeat(200)));
        assert_eq!(cut("", 200), "");
        // At characters, not bytes.
        assert_eq!(cut("ééé", 2), "éé...");
    }

    #[test]
    fn a_certificate_that_is_refused_is_said_in_plain_words() {
        let said = |error| connection(&rustls_error(error), "mail.test", "the system trusts");
        let invalid = rustls::Error::InvalidCertificate;
        assert_eq!(
            said(invalid(Certificate::UnknownIssuer)),
            "its certificate is not from an authority the system trusts"
        );
        assert_eq!(
            said(invalid(Certificate::NotValidForName)),
            "its certificate is not for mail.test"
        );
        assert_eq!(said(invalid(Certificate::Expired)), "its certificate has expired");
        assert_eq!(said(invalid(Certificate::NotValidYet)), "its certificate is not valid yet");
    }

    #[test]
    fn a_failure_has_the_id_of_its_page_when_its_words_say_what_it_was() {
        let id = |words: &str| error_id(words);
        assert_eq!(
            id("the server refused the password (535 5.7.8 authentication failed)"),
            Some("ALR-101")
        );
        assert_eq!(id("the server could not sign in (454 try later)"), Some("ALR-101"));
        assert_eq!(
            id("the server does not offer STARTTLS, so nothing was sent; mail is never sent \
                without TLS (it offers: 8BITMIME)"),
            Some("ALR-102")
        );
        assert_eq!(id("ntfy answered 403 Forbidden: forbidden"), Some("ALR-103"));
        assert_eq!(id("json answered 502"), Some("ALR-103"));
        assert_eq!(id("the push service answered 400: Unauthorized (ALR-105)"), Some("ALR-105"));
        assert_eq!(
            id("the browser ended the subscription: the push service answered 410, so it was \
                removed (ALR-104)"),
            Some("ALR-104")
        );
        // What no page is about.
        assert_eq!(id("no answer within 30 seconds"), None);
        assert_eq!(id("cannot reach ntfy.sh: refused"), None);
        assert_eq!(id("ntfy answered nothing"), None);
        assert_eq!(id(""), None);
    }

    #[test]
    fn a_silent_or_closed_connection_is_said_and_an_os_error_loses_its_number() {
        let said = |kind| connection(&io::Error::from(kind), "h", "");
        assert_eq!(said(io::ErrorKind::UnexpectedEof), "the server closed the connection");
        assert_eq!(said(io::ErrorKind::TimedOut), "no answer within 30 seconds");
        assert_eq!(said(io::ErrorKind::WouldBlock), "no answer within 30 seconds");
        let refused = io::Error::from_raw_os_error(111);
        assert!(!connection(&refused, "h", "").contains("os error"));
    }
}
