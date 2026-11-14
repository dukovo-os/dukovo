// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Email through the provider's submission server (ADR 0485), as alertd
//! writes it: the message of an alert, and its sending through `mail`,
//! which holds the connection, STARTTLS or TLS from the first byte, and
//! the sign-in.
//!
//! A message follows RFC 5322: plain text in UTF-8, quoted-printable,
//! with `Auto-Submitted: auto-generated` (RFC 3834) so that vacation
//! responders do not answer, and a subject that is not ASCII as an RFC
//! 2047 word.

use std::fmt::Write as _;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;

use crate::mail::{self, Server, greeting_name};

/// A line of a message and of a header is folded to at most this many
/// columns (RFC 5322 §2.2.3 asks for 78).
const FOLD_AT: usize = 76;

/// How many bytes of a subject go in one RFC 2047 word: 36 bytes make 48
/// characters of base64 and 60 with the word's own marks, so that a word
/// and `Subject: ` fit a folded line.
const WORD_BYTES: usize = 36;

/// A message to send.
#[derive(Clone, Debug)]
pub struct Message {
    pub from: String,
    pub to: Vec<String>,
    pub subject: String,
    pub body: String,
}

/// Counts the messages of this run, so that their Message-IDs differ even
/// within a second.
static SENT: AtomicU64 = AtomicU64::new(0);

/// Sends `message` through `server`, saying `hostname` for itself in
/// EHLO.
///
/// # Errors
///
/// The step that failed and the server's words (`mail::deliver`).
pub fn send(server: &Server, message: &Message, hostname: &str) -> Result<(), String> {
    let count = SENT.fetch_add(1, Ordering::Relaxed);
    let now = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |since| since.as_secs());
    let text = format(message, hostname, now, &format!("{now}.{count}"));
    mail::deliver(server, &message.from, &message.to, &text, hostname).map_err(String::from)
}

/// The text of `message` as RFC 5322 has it, CRLF at each line's end:
/// sent at `now`, seconds since the epoch, from machine `hostname`, with
/// Message-ID `id`.
pub fn format(message: &Message, hostname: &str, now: u64, id: &str) -> Vec<u8> {
    let host = greeting_name(hostname);
    let mut text = String::new();
    let mut header = |name: &str, value: &str| {
        text.push_str(&fold(&format!("{name}: {value}")));
        text.push_str("\r\n");
    };
    header("Date", &mail::date(now));
    header("From", &message.from);
    header("To", &message.to.join(", "));
    header("Subject", &mime_words(&message.subject));
    header("Message-ID", &format!("<{id}@{host}>"));
    header("MIME-Version", "1.0");
    header("Content-Type", "text/plain; charset=utf-8");
    header("Content-Transfer-Encoding", "quoted-printable");
    header("Auto-Submitted", "auto-generated");
    text.push_str("\r\n");
    text.push_str(&quoted_printable(&message.body));
    text.into_bytes()
}

/// `text` as header text: unchanged when it is printable ASCII, else as
/// RFC 2047 words of base64, separated by spaces, which [`fold`] may put
/// on lines of their own.
pub fn mime_words(text: &str) -> String {
    let text = text.replace(['\r', '\n'], " ");
    if text.bytes().all(|byte| byte == b' ' || byte.is_ascii_graphic()) {
        return text;
    }
    let mut words = Vec::new();
    let mut rest = text.as_str();
    while !rest.is_empty() {
        let mut end = WORD_BYTES.min(rest.len());
        while !rest.is_char_boundary(end) {
            end -= 1;
        }
        let (word, after) = rest.split_at(end);
        words.push(format!("=?utf-8?B?{}?=", BASE64.encode(word)));
        rest = after;
    }
    words.join(" ")
}

/// `header` with a line break and a space (RFC 5322's folding) put at the
/// spaces that would take a line past [`FOLD_AT`] columns.
fn fold(header: &str) -> String {
    let mut out = String::new();
    let mut column = 0;
    for (at, word) in header.split(' ').enumerate() {
        if at > 0 && column + 1 + word.len() > FOLD_AT {
            out.push_str("\r\n ");
            column = 1;
        } else if at > 0 {
            out.push(' ');
            column += 1;
        }
        out.push_str(word);
        column += word.len();
    }
    out
}

/// `text` in quoted-printable (RFC 2045 §6.7), with CRLF at each line's
/// end and lines of at most [`FOLD_AT`] columns: each byte that is not
/// printable ASCII, and each `=`, as `=` and two capital hex digits, a
/// space or tab at a line's end the same, and a soft line break, `=` at a
/// line's end, where a line would be too long.
fn quoted_printable(text: &str) -> String {
    let mut out = String::new();
    if text.is_empty() {
        return out;
    }
    let lines: Vec<&str> = text.split('\n').collect();
    let last = if text.ends_with('\n') { lines.len() - 1 } else { lines.len() };
    for line in &lines[..last] {
        let bytes = line.trim_end_matches('\r').as_bytes();
        let mut column = 0;
        for (at, &byte) in bytes.iter().enumerate() {
            let at_end = at + 1 == bytes.len();
            let mut token = String::new();
            // A space or tab is itself unless it ends the line, where a
            // mail server may strip it.
            let plain = (byte.is_ascii_graphic() && byte != b'=')
                || ((byte == b' ' || byte == b'\t') && !at_end);
            if plain {
                token.push(char::from(byte));
            } else {
                let _ = write!(token, "={byte:02X}");
            }
            // The last token of a line has no soft break after it, so it
            // may take the column a soft break's `=` would.
            let limit = if at_end { FOLD_AT } else { FOLD_AT - 1 };
            if column + token.len() > limit {
                out.push_str("=\r\n");
                column = 0;
            }
            out.push_str(&token);
            column += token.len();
        }
        out.push_str("\r\n");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mail::testing::{Script, client, smtp_server};

    fn message() -> Message {
        Message {
            from: "alerts@server.example.com".into(),
            to: vec!["ops@example.com".into(), "me@example.org".into()],
            subject: "[server] CRITICAL: /data is 97% full".into(),
            body: "Raised, critical: /data is 97% full\n    Free space.\n".into(),
        }
    }

    #[test]
    fn an_alert_goes_through_the_submission_server_with_its_subject_and_body() {
        let (port, server) = smtp_server(Script::new(), true);
        let mail = client(port, None);
        let text = format(&message(), "server", 1_763_030_531, "17.3");
        mail::deliver_over(&mail, &message().from, &message().to, &text, "server", true).unwrap();
        let seen = server.join().unwrap();
        let sent = seen.message.unwrap();
        assert!(sent.contains("Subject: [server] CRITICAL: /data is 97% full\r\n"), "{sent}");
        assert!(sent.ends_with("Free space.\r\n"), "{sent}");
    }

    #[test]
    fn ehlo_names_the_machine_or_localhost() {
        assert_eq!(greeting_name("server.example.com"), "server.example.com");
        assert_eq!(greeting_name("my server"), "localhost");
        assert_eq!(greeting_name(""), "localhost");
        assert_eq!(greeting_name("a\\r\\nb"), "localhost");
    }

    #[test]
    fn a_message_has_the_headers_rfc_5322_asks_for() {
        let text = String::from_utf8(format(&message(), "server", 1_763_030_531, "17.3")).unwrap();
        let (head, body) = text.split_once("\r\n\r\n").unwrap();
        let lines: Vec<&str> = head.split("\r\n").collect();
        assert_eq!(
            lines,
            [
                "Date: Thu, 13 Nov 2025 10:42:11 +0000",
                "From: alerts@server.example.com",
                "To: ops@example.com, me@example.org",
                "Subject: [server] CRITICAL: /data is 97% full",
                "Message-ID: <17.3@server>",
                "MIME-Version: 1.0",
                "Content-Type: text/plain; charset=utf-8",
                "Content-Transfer-Encoding: quoted-printable",
                "Auto-Submitted: auto-generated",
            ]
        );
        assert_eq!(body, "Raised, critical: /data is 97% full\r\n    Free space.\r\n");
    }

    #[test]
    fn the_date_has_its_weekday_and_month() {
        assert_eq!(mail::date(0), "Thu, 1 Jan 1970 00:00:00 +0000");
        assert_eq!(mail::date(1_763_030_531), "Thu, 13 Nov 2025 10:42:11 +0000");
        assert_eq!(mail::date(951_782_400), "Tue, 29 Feb 2000 00:00:00 +0000");
        assert_eq!(mail::date(4_102_444_799), "Thu, 31 Dec 2099 23:59:59 +0000");
    }

    /// Reads quoted-printable back, as a mail program does.
    fn decoded(text: &str) -> Vec<u8> {
        let mut out = Vec::new();
        let bytes = text.as_bytes();
        let mut at = 0;
        while at < bytes.len() {
            match bytes[at] {
                b'=' if bytes[at + 1..].starts_with(b"\r\n") => at += 3,
                b'=' => {
                    let hex = std::str::from_utf8(&bytes[at + 1..at + 3]).unwrap();
                    out.push(u8::from_str_radix(hex, 16).unwrap());
                    at += 3;
                }
                byte => {
                    out.push(byte);
                    at += 1;
                }
            }
        }
        out
    }

    #[test]
    fn quoted_printable_keeps_ascii_and_encodes_the_rest() {
        assert_eq!(quoted_printable("plain text\n"), "plain text\r\n");
        assert_eq!(quoted_printable("a=b\n"), "a=3Db\r\n");
        assert_eq!(quoted_printable("é\n"), "=C3=A9\r\n");
        assert_eq!(quoted_printable("trailing space \n"), "trailing space=20\r\n");
        assert_eq!(quoted_printable("tab\t\n"), "tab=09\r\n");
        assert_eq!(quoted_printable("a b\tc\n"), "a b\tc\r\n");
        assert_eq!(quoted_printable("one\n\ntwo"), "one\r\n\r\ntwo\r\n");
        assert_eq!(quoted_printable("crlf\r\nlines\r\n"), "crlf\r\nlines\r\n");
        assert_eq!(quoted_printable(""), "");
    }

    #[test]
    fn quoted_printable_lines_are_at_most_76_characters_and_read_back() {
        let long = "word ".repeat(60);
        let accents = "é=ü ".repeat(40);
        let unbroken = "x".repeat(200);
        let edge = format!("{}é\n", "y".repeat(74));
        for text in [long, accents, unbroken, edge] {
            let encoded = quoted_printable(&text);
            for line in encoded.split("\r\n") {
                assert!(line.len() <= 76, "{} columns: {line}", line.len());
            }
            let mut wanted = text.replace('\n', "\r\n");
            if !text.ends_with('\n') {
                wanted.push_str("\r\n");
            }
            assert_eq!(String::from_utf8(decoded(&encoded)).unwrap(), wanted);
        }
    }

    #[test]
    fn a_subject_that_is_not_ascii_is_rfc_2047_words() {
        assert_eq!(mime_words("[server] Warning: disk"), "[server] Warning: disk");
        let words = mime_words("[serveur] Avertissement : le disque été plein");
        assert!(words.starts_with("=?utf-8?B?") && words.ends_with("?="), "{words}");
        let mut text = Vec::new();
        for word in words.split(' ') {
            let payload = word.strip_prefix("=?utf-8?B?").unwrap().strip_suffix("?=").unwrap();
            assert!(word.len() <= 75, "{word}");
            text.extend(BASE64.decode(payload).unwrap());
        }
        assert_eq!(
            String::from_utf8(text).unwrap(),
            "[serveur] Avertissement : le disque été plein"
        );
    }

    #[test]
    fn a_long_non_ascii_subject_is_split_at_characters_and_folded() {
        let subject = "é".repeat(60);
        let mut mail = message();
        mail.subject = subject.clone();
        let text = String::from_utf8(format(&mail, "server", 0, "1")).unwrap();
        let head = text.split("\r\n\r\n").next().unwrap();
        let folded: Vec<&str> =
            head.split("\r\n").skip_while(|line| !line.starts_with("Subject:")).collect();
        let mut got = String::new();
        for line in
            folded.iter().take_while(|line| line.starts_with("Subject:") || line.starts_with(' '))
        {
            assert!(line.len() <= 76, "{line}");
            for word in line.trim_start_matches("Subject:").split_whitespace() {
                let payload = word.strip_prefix("=?utf-8?B?").unwrap().strip_suffix("?=").unwrap();
                got.push_str(&String::from_utf8(BASE64.decode(payload).unwrap()).unwrap());
            }
        }
        assert_eq!(got, subject);
    }

    #[test]
    fn a_subject_cannot_start_another_header() {
        assert_eq!(mime_words("one\r\nBcc: x@y.test"), "one  Bcc: x@y.test");
    }
}
