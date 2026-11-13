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
    fn a_silent_or_closed_connection_is_said_and_an_os_error_loses_its_number() {
        let said = |kind| connection(&io::Error::from(kind), "h", "");
        assert_eq!(said(io::ErrorKind::UnexpectedEof), "the server closed the connection");
        assert_eq!(said(io::ErrorKind::TimedOut), "no answer within 30 seconds");
        assert_eq!(said(io::ErrorKind::WouldBlock), "no answer within 30 seconds");
        let refused = io::Error::from_raw_os_error(111);
        assert!(!connection(&refused, "h", "").contains("os error"));
    }
}
