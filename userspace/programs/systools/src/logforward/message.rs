// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The RFC 5424 message logforward(8) sends for a record of the log (ADR
//! 0482):
//!
//! ```text
//! <PRI>1 TIMESTAMP HOSTNAME APP-NAME PROCID MSGID STRUCTURED-DATA MSG
//! ```
//!
//! The priority is the facility times 8 plus the severity. The facility
//! is authpriv for a security event, whoever wrote it, else the kernel's
//! for the kernel's records, daemon for a service's lines and user for a
//! program's. The severity is the record's level as syslog(3) numbers it.
//! The message is the record's text, cut so that it fits what the
//! receiver reads: [`limit`]. The framing, the length that goes in front
//! over TCP and TLS, is the connection's.

use super::connection::Transport;
use crate::logd::Record;

/// The most octets of a message over TCP and TLS: rsyslog's default
/// maximum.
const STREAM_MAX: usize = 8192;
/// The most octets of a message in a datagram (RFC 5426 §3.2).
const DATAGRAM_MAX: usize = 2048;
/// The most octets of the hostname field (RFC 5424 §6.2.4).
const HOST_MAX: usize = 255;
/// The most octets of the application name field. RFC 5424 allows 48.
const APP_NAME_MAX: usize = 48;

const KERN: u8 = 0;
const USER: u8 = 1;
const DAEMON: u8 = 3;
const AUTHPRIV: u8 = 10;
/// syslog(3)'s severity of a notice.
const NOTICE: u8 = 5;

/// The most octets a message may have over `transport`.
pub fn limit(transport: Transport) -> usize {
    match transport {
        Transport::Tls | Transport::Tcp => STREAM_MAX,
        Transport::Udp => DATAGRAM_MAX,
    }
}

/// The machine's name as the hostname field holds it.
pub fn host_field(name: &str) -> String {
    field(name, HOST_MAX)
}

/// The message for `record` from machine `host` (as [`host_field`] gives it),
/// cut to `limit` octets at the end of a character.
pub fn record_message(record: &Record, host: &str, limit: usize) -> String {
    let facility = if record.audit {
        AUTHPRIV
    } else {
        match record.from.as_str() {
            "kernel" => KERN,
            "service" => DAEMON,
            _ => USER,
        }
    };
    let priority = facility * 8 + record.level_number();
    let process = record.process.map_or_else(|| "-".to_string(), |process| process.to_string());
    let message_id = if record.audit { "audit" } else { "-" };
    let mut message = format!(
        "<{priority}>1 {} {host} {} {process} {message_id} -",
        record.time,
        field(&record.name, APP_NAME_MAX)
    );
    // RFC 5424 leaves out the space before a message that is empty.
    if !record.text.is_empty() {
        message.push(' ');
        message.push_str(&record.text);
    }
    cut(&mut message, limit);
    message
}

/// The record `logforward test` sends to destination `destination`, made
/// at `time` (RFC 3339), on machine `host`.
pub fn test_message(host: &str, destination: &str, time: &str) -> String {
    let priority = USER * 8 + NOTICE;
    format!(
        "<{priority}>1 {time} {host} logforward - test - logforward: a test record from {host} \
         to {destination}"
    )
}

/// `text` as a header field: only the octets from 33 to 126, which RFC
/// 5424 allows, the others replaced by `_`, and at most `longest` of
/// them; `-` for a field with nothing.
fn field(text: &str, longest: usize) -> String {
    let field: String = text
        .bytes()
        .take(longest)
        .map(|byte| if (33..=126).contains(&byte) { char::from(byte) } else { '_' })
        .collect();
    if field.is_empty() { "-".to_string() } else { field }
}

/// Cuts `message` to `limit` octets, at the end of a character.
fn cut(message: &mut String, limit: usize) {
    if message.len() <= limit {
        return;
    }
    let mut end = limit;
    while !message.is_char_boundary(end) {
        end -= 1;
    }
    message.truncate(end);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(from: &str, name: &str, level: &str, text: &str) -> Record {
        Record {
            time: "2026-11-13T10:30:00.123456Z".into(),
            boot: "6c1f".into(),
            seq: 7,
            level: level.into(),
            from: from.into(),
            name: name.into(),
            text: text.into(),
            ..Record::default()
        }
    }

    #[test]
    fn a_kernel_record_is_the_kernels_facility_with_no_process() {
        let record = record("kernel", "kernel", "info", "Net: net0 is up");
        assert_eq!(
            record_message(&record, "dukovo", 8192),
            "<6>1 2026-11-13T10:30:00.123456Z dukovo kernel - - - Net: net0 is up"
        );
    }

    #[test]
    fn a_services_line_is_daemon_and_a_programs_is_user_with_its_process() {
        let service = record("service", "sshd", "warning", "refused a connection");
        assert_eq!(
            record_message(&service, "dukovo", 8192),
            "<28>1 2026-11-13T10:30:00.123456Z dukovo sshd - - - refused a connection"
        );
        let program =
            Record { process: Some(57), ..record("program", "forwardtest", "info", "first") };
        assert_eq!(
            record_message(&program, "dukovo", 8192),
            "<14>1 2026-11-13T10:30:00.123456Z dukovo forwardtest 57 - - first"
        );
    }

    #[test]
    fn a_security_event_is_authpriv_whatever_wrote_it() {
        for (from, name, severity) in
            [("kernel", "kernel", 6), ("service", "spark", 6), ("program", "x", 6)]
        {
            let event =
                Record { audit: true, ..record(from, name, "info", "asked to restart sshd") };
            let message = record_message(&event, "dukovo", 8192);
            assert!(message.starts_with(&format!("<{}>1 ", 10 * 8 + severity)), "{message}");
            assert!(
                message.contains(&format!(" {name} - audit - asked to restart sshd")),
                "{message}"
            );
        }
    }

    #[test]
    fn each_level_is_its_syslog_severity() {
        for (level, severity) in [("error", 3), ("warning", 4), ("info", 6), ("debug", 7)] {
            let message = record_message(&record("service", "x", level, "t"), "h", 8192);
            assert!(message.starts_with(&format!("<{}>1 ", 3 * 8 + severity)), "{message}");
        }
    }

    #[test]
    fn a_name_with_odd_octets_is_made_fit_and_cut_to_48() {
        let message = record_message(&record("program", "my prog \u{e9}", "info", "t"), "h", 8192);
        assert!(message.contains(" h my_prog___ - - - t"), "{message}");
        let long = "n".repeat(60);
        let message = record_message(&record("program", &long, "info", "t"), "h", 8192);
        assert!(message.contains(&format!(" h {} - - - t", "n".repeat(48))), "{message}");
        let message = record_message(&record("program", "", "info", "t"), "h", 8192);
        assert!(message.contains(" h - - - - t"), "{message}");
    }

    #[test]
    fn the_hostname_is_printable_ascii_at_most_255_and_a_dash_when_empty() {
        assert_eq!(host_field("dukovo-1"), "dukovo-1");
        assert_eq!(host_field("my host"), "my_host");
        assert_eq!(host_field(""), "-");
        assert_eq!(host_field(&"h".repeat(300)).len(), 255);
    }

    #[test]
    fn an_empty_text_leaves_out_the_space_before_it() {
        let message = record_message(&record("kernel", "kernel", "info", ""), "h", 8192);
        assert_eq!(message, "<6>1 2026-11-13T10:30:00.123456Z h kernel - - -");
    }

    #[test]
    fn a_text_is_sent_as_it_is_with_its_spaces_and_newlines() {
        let message =
            record_message(&record("kernel", "kernel", "info", "a  b\nc \u{e9}"), "h", 8192);
        assert!(message.ends_with(" - a  b\nc \u{e9}"), "{message}");
    }

    #[test]
    fn a_long_message_is_cut_at_the_limit_at_the_end_of_a_character() {
        // The header is an odd or even number of octets for one of these
        // texts, so the limit falls inside a character for the other.
        for (text, transport) in [
            ("\u{e9}".repeat(5000), Transport::Tls),
            (format!("a{}", "\u{e9}".repeat(5000)), Transport::Tcp),
            ("\u{e9}".repeat(5000), Transport::Udp),
            (format!("a{}", "\u{e9}".repeat(5000)), Transport::Udp),
        ] {
            let record = record("kernel", "kernel", "info", &text);
            let whole = record_message(&record, "h", usize::MAX);
            let message = record_message(&record, "h", limit(transport));
            assert!(whole.starts_with(&message));
            assert!(
                message.len() == limit(transport) || message.len() == limit(transport) - 1,
                "{} octets",
                message.len()
            );
        }
    }

    #[test]
    fn a_message_that_fits_is_not_cut() {
        let record = record("kernel", "kernel", "info", &"x".repeat(100));
        let message = record_message(&record, "h", 8192);
        assert!(message.ends_with(&"x".repeat(100)));
        assert_eq!(limit(Transport::Tls), 8192);
        assert_eq!(limit(Transport::Tcp), 8192);
        assert_eq!(limit(Transport::Udp), 2048);
    }

    #[test]
    fn the_test_record_is_a_notice_of_the_user_facility() {
        assert_eq!(
            test_message("dukovo", "siem", "2026-11-13T10:30:00.000000Z"),
            "<13>1 2026-11-13T10:30:00.000000Z dukovo logforward - test - logforward: a test \
             record from dukovo to siem"
        );
    }
}
