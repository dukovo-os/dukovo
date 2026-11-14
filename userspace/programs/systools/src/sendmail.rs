// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! sendmail(8): reads a message from standard input and sends it through
//! the submission server alerts.toml(5)'s `[email]` table names, as every
//! Linux program that mails (`cron`, `smartd`, scripts, `mail`) expects
//! `/usr/sbin/sendmail` to.
//!
//! The recipients are the arguments, and with `-t` the addresses in the
//! message's To, Cc and Bcc headers too, the Bcc header being removed
//! before it is sent. A message without a Date or a From header gets
//! them. Dukovo has no mail queue and no local delivery: the message goes
//! at once or the command fails, with sysexits(3)'s status for why
//! (`exit_status`) and a line in the log.
//!
//! The sending is `mail`, shared with alertd(8). The boot test
//! `admin-commands` covers the command's failures and, against a test
//! server, its success; the unit tests cover the headers and the statuses.

use std::fs::OpenOptions;
use std::io::{Read, Write};

use crate::args::{Arg, Args};
use crate::mail::{self, Kind};
use crate::util::{self, Fail, fail_with, usage};

const USAGE: &str = "Usage: sendmail [-t] [-i] [-f FROM] [-F NAME] [RECIPIENT...]\n\
                     Reads a message from standard input and sends it through the submission \
                     server\n/system/settings/alerts.toml names.";

/// The largest message sent, in bytes: submission servers refuse more.
const MESSAGE_MAX: u64 = 25 << 20;

/// Where a line for the log goes.
const KMSG: &str = "/devices/kmsg";

/// sysexits(3): a mistake in the command line, a message that is wrong,
/// a server that refuses for good, one to try again at, a failed read of
/// the input, and settings that are missing.
const EX_USAGE: i32 = 64;
const EX_DATAERR: i32 = 65;
const EX_UNAVAILABLE: i32 = 69;
const EX_IOERR: i32 = 74;
const EX_TEMPFAIL: i32 = 75;
const EX_CONFIG: i32 = 78;

/// What the command line asks for.
#[derive(Debug, Default, PartialEq, Eq)]
struct Options {
    /// `-t`: the recipients are in the headers too.
    from_headers: bool,
    /// `-f`: the envelope sender; the empty one is the null sender.
    sender: Option<String>,
    /// `-F`: the name before the address of a From header sendmail adds.
    full_name: Option<String>,
    recipients: Vec<String>,
}

/// sendmail(8).
pub fn main(args: &mut Args) -> Result<i32, Fail> {
    let options = parse(args).map_err(|mut fail| {
        if fail.usage {
            fail.code = EX_USAGE;
        }
        fail
    })?;
    let mut input = Vec::new();
    let read = std::io::stdin().lock().take(MESSAGE_MAX + 1).read_to_end(&mut input);
    if let Err(e) = read {
        return Err(fail_with(
            EX_IOERR,
            format!("cannot read the message: {}", util::strip_os_error(&e)),
        ));
    }
    if input.len() as u64 > MESSAGE_MAX {
        return Err(fail_with(
            EX_DATAERR,
            format!("the message is longer than {} MiB, which servers refuse", MESSAGE_MAX >> 20),
        ));
    }
    let host = util::hostname();
    let now = util::now_ns() / 1_000_000_000;
    let (recipients, message) =
        prepare(&options, &input, &host, &util::user_name(util::own_user()), now)
            .map_err(|why| fail_with(EX_USAGE, why))?;
    match mail::send(options.sender.as_deref(), &recipients, &message) {
        Ok(()) => Ok(0),
        Err(error) => {
            log_failure(&recipients, &error.message);
            Err(fail_with(exit_status(error.kind), error.message))
        }
    }
}

/// sysexits(3)'s status for a failed send: a server to try again later
/// or at all (`EX_TEMPFAIL`), one that refused for good
/// (`EX_UNAVAILABLE`), and settings that name none (`EX_CONFIG`).
fn exit_status(kind: Kind) -> i32 {
    match kind {
        Kind::Temporary => EX_TEMPFAIL,
        Kind::Refused => EX_UNAVAILABLE,
        Kind::NoServer => EX_CONFIG,
    }
}

/// Writes a line about the failure to the system's log, so that a job
/// whose mail was lost leaves a trace where its owner looks.
fn log_failure(recipients: &[String], why: &str) {
    let line = format!("<4>sendmail: not sent to {}: {why}\n", recipients.join(", "));
    // The log is a courtesy: the failure is reported in the status and on
    // the standard error whether or not this line is written.
    let _ = OpenOptions::new()
        .write(true)
        .open(KMSG)
        .and_then(|mut log| log.write_all(line.as_bytes()));
}

/// The options and recipients of the command line.
fn parse(args: &mut Args) -> Result<Options, Fail> {
    let mut options = Options::default();
    while let Some(arg) = args.next()? {
        match arg {
            Arg::Short('t') => options.from_headers = true,
            // The message ends at the end of the input, so a line of one
            // dot never ends it early, which is what these ask for.
            Arg::Short('i' | 'v' | 'm') => {}
            Arg::Short('f' | 'r') => options.sender = Some(args.text()?),
            Arg::Short('F') => options.full_name = Some(args.text()?),
            Arg::Short('o') => {
                // `-oi`, `-oem` and the other options sendmail reads as
                // `-o` and a letter, which change nothing here.
                args.text()?;
            }
            Arg::Short('b') => {
                let mode = args.text()?;
                if mode != "m" {
                    return Err(usage(format!(
                        "-b{mode}: Dukovo has no mail queue and no mail server to run; sendmail \
                         sends the message on its input through the submission server, at once"
                    )));
                }
            }
            Arg::Short('q') => {
                return Err(usage(
                    "-q: there is no mail queue; a message that cannot be sent is the status \
                     sendmail ends with"
                        .into(),
                ));
            }
            Arg::Long(l) if l == "help" => return Err(usage(USAGE.into())),
            Arg::Operand(recipient) => {
                options.recipients.push(recipient.to_string_lossy().into_owned());
            }
            other => return Err(args.unknown(&other)),
        }
    }
    Ok(options)
}

/// One header of a message: its name, its value with folds undone, and
/// the lines it takes as they were written.
#[derive(Debug, PartialEq, Eq)]
struct Header {
    name: String,
    value: String,
    lines: Vec<Vec<u8>>,
}

/// Splits `input` into its headers and its body, which keeps its line
/// ends. A message whose first line is no header is all body.
fn split(input: &[u8]) -> (Vec<Header>, &[u8]) {
    let mut headers: Vec<Header> = Vec::new();
    let mut at = 0;
    while at < input.len() {
        let end = input[at..].iter().position(|&b| b == b'\n').map_or(input.len(), |n| at + n);
        let next = (end + 1).min(input.len());
        let line = input[at..end].strip_suffix(b"\r").unwrap_or(&input[at..end]);
        if line.is_empty() {
            return (headers, &input[next..]);
        }
        if line[0] == b' ' || line[0] == b'\t' {
            let Some(last) = headers.last_mut() else { return (headers, &input[at..]) };
            last.value.push(' ');
            last.value.push_str(String::from_utf8_lossy(line).trim());
            last.lines.push(line.to_vec());
        } else {
            let Some(colon) = line.iter().position(|&b| b == b':').filter(|&n| n > 0) else {
                return (headers, &input[at..]);
            };
            let name = &line[..colon];
            if !name.iter().all(|&b| b.is_ascii_graphic()) {
                return (headers, &input[at..]);
            }
            headers.push(Header {
                name: String::from_utf8_lossy(name).into_owned(),
                value: String::from_utf8_lossy(&line[colon + 1..]).trim().to_string(),
                lines: vec![line.to_vec()],
            });
        }
        at = next;
    }
    (headers, &[])
}

/// The addresses in a header's `value`, as `name <address>`, `address
/// (comment)` and groups write them, split at the commas that are not in
/// a quoted name, a comment or an angle bracket. An address without a
/// domain gets `host`, as sendmail's masquerading does.
fn addresses(value: &str, host: &str) -> Result<Vec<String>, String> {
    let mut found = Vec::new();
    let (mut quoted, mut angle, mut comment) = (false, false, 0usize);
    let mut part = String::new();
    let mut parts = Vec::new();
    let mut chars = value.chars();
    while let Some(c) = chars.next() {
        match c {
            '\\' if quoted || comment > 0 => {
                part.push(c);
                part.extend(chars.next());
                continue;
            }
            '"' if comment == 0 => quoted = !quoted,
            '(' if !quoted => comment += 1,
            ')' if !quoted && comment > 0 => comment -= 1,
            '<' if !quoted && comment == 0 => angle = true,
            '>' if !quoted && comment == 0 => angle = false,
            ',' | ';' if !quoted && !angle && comment == 0 => {
                parts.push(std::mem::take(&mut part));
                continue;
            }
            _ => {}
        }
        part.push(c);
    }
    parts.push(part);
    for part in parts {
        let part = part.trim();
        // A group's name, `team: a@x, b@y`, is before its first colon.
        let part = part
            .rsplit_once(':')
            .filter(|_| !part.contains('<'))
            .map_or(part, |(_, rest)| rest.trim());
        if part.is_empty() {
            continue;
        }
        let address = match (part.find('<'), part.rfind('>')) {
            (Some(open), Some(close)) if open < close => part[open + 1..close].trim().to_string(),
            _ => strip_comments(part).trim().to_string(),
        };
        let address = address.trim_start_matches('<').trim_end_matches('>');
        if address.is_empty()
            || address.chars().any(|c| c.is_whitespace() || c.is_control() || c == '<' || c == '>')
        {
            return Err(format!("'{}' is not an address", part.trim()));
        }
        found.push(if address.contains('@') {
            address.to_string()
        } else {
            format!("{address}@{host}")
        });
    }
    Ok(found)
}

/// `text` without its `(comments)`.
fn strip_comments(text: &str) -> String {
    let mut depth = 0usize;
    let mut out = String::new();
    for c in text.chars() {
        match c {
            '(' => depth += 1,
            ')' if depth > 0 => depth -= 1,
            _ if depth == 0 => out.push(c),
            _ => {}
        }
    }
    out
}

/// The recipients and the message to send: the recipients of the command
/// line and, with `-t`, of the headers; the message with its Bcc headers
/// removed when `-t` was given, a Date and a From added when it has
/// none, and CRLF at each line's end.
///
/// `user` and `host` make the address of a From header that is added,
/// and `now` is the time of a Date header that is added.
fn prepare(
    options: &Options,
    input: &[u8],
    host: &str,
    user: &str,
    now: u64,
) -> Result<(Vec<String>, Vec<u8>), String> {
    let (mut headers, body) = split(input);
    let mut recipients = Vec::new();
    for given in &options.recipients {
        recipients.extend(addresses(given, host)?);
    }
    if options.from_headers {
        for header in &headers {
            if ["to", "cc", "bcc"].iter().any(|name| header.name.eq_ignore_ascii_case(name)) {
                recipients.extend(addresses(&header.value, host)?);
            }
        }
        headers.retain(|header| !header.name.eq_ignore_ascii_case("bcc"));
    }
    recipients.dedup();
    if recipients.is_empty() {
        return Err(if options.from_headers {
            "no recipients: the message has no To, Cc or Bcc header with an address, and none \
             were given"
                .into()
        } else {
            "no recipients: give them as arguments, or -t to read them from the message".into()
        });
    }
    let mut text = Vec::new();
    let mut put = |line: &[u8]| {
        text.extend_from_slice(line);
        text.extend_from_slice(b"\r\n");
    };
    for header in &headers {
        header.lines.iter().for_each(|line| put(line));
    }
    let has = |name: &str| headers.iter().any(|header| header.name.eq_ignore_ascii_case(name));
    if !has("date") {
        put(format!("Date: {}", mail::date(now)).as_bytes());
    }
    if !has("from") {
        let address = format!("{user}@{host}");
        put(match &options.full_name {
            Some(name) => format!("From: \"{}\" <{address}>", name.replace(['\\', '"'], "")),
            None => format!("From: {address}"),
        }
        .as_bytes());
    }
    put(b"");
    text.extend_from_slice(&crlf(body));
    Ok((recipients, text))
}

/// `text` with CRLF at each line's end, whatever each had.
fn crlf(text: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(text.len() + text.len() / 40);
    for (at, &byte) in text.iter().enumerate() {
        if byte == b'\n' && (at == 0 || text[at - 1] != b'\r') {
            out.push(b'\r');
        }
        out.push(byte);
    }
    if !out.is_empty() && !out.ends_with(b"\n") {
        out.extend_from_slice(b"\r\n");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;

    fn parsed(items: &[&str]) -> Result<Options, Fail> {
        parse(&mut Args::new("sendmail", items.iter().map(OsString::from).collect()))
    }

    fn text(options: &Options, input: &str) -> Result<(Vec<String>, String), String> {
        let (to, message) = prepare(options, input.as_bytes(), "box", "alice", 1_763_030_531)?;
        Ok((to, String::from_utf8(message).unwrap()))
    }

    #[test]
    fn the_command_line_is_sendmails() {
        let options = parsed(&["-t", "-i", "-oi", "-f", "cron@box", "ops@example.com"]).unwrap();
        assert_eq!(
            options,
            Options {
                from_headers: true,
                sender: Some("cron@box".into()),
                full_name: None,
                recipients: vec!["ops@example.com".into()],
            }
        );
        assert_eq!(
            parsed(&["-fcron@box", "-F", "Cron Daemon"]).unwrap().sender.as_deref(),
            Some("cron@box")
        );
        assert_eq!(parsed(&["-f", ""]).unwrap().sender.as_deref(), Some(""));
        assert_eq!(parsed(&["-bm", "-oem", "-v", "a@b"]).unwrap().recipients, ["a@b"]);
    }

    #[test]
    fn a_queue_or_a_server_is_pointed_away_from() {
        for items in [&["-bd"][..], &["-bp"], &["-q"]] {
            let why = parsed(items).unwrap_err();
            assert!(why.usage && why.message.contains("no mail queue"), "{}", why.message);
        }
        assert!(parsed(&["--bogus"]).unwrap_err().usage);
    }

    #[test]
    fn recipients_come_from_the_arguments_and_with_t_from_the_headers() {
        let options = Options {
            from_headers: true,
            recipients: vec!["extra@example.com".into()],
            ..Options::default()
        };
        let (to, sent) = text(
            &options,
            "To: Ops <ops@example.com>, \"Last, First\" <lf@example.com>\nCc: me@example.org (me)\nBcc: hidden@example.net\nSubject: hi\n\nbody\n",
        )
        .unwrap();
        assert_eq!(
            to,
            [
                "extra@example.com",
                "ops@example.com",
                "lf@example.com",
                "me@example.org",
                "hidden@example.net"
            ]
        );
        assert!(!sent.to_lowercase().contains("bcc"), "{sent}");
        assert!(sent.contains("To: Ops <ops@example.com>"), "{sent}");
    }

    #[test]
    fn without_t_the_headers_are_not_read_and_bcc_stays() {
        let options = Options { recipients: vec!["ops@example.com".into()], ..Options::default() };
        let (to, sent) =
            text(&options, "To: other@example.com\nBcc: x@example.com\n\nbody\n").unwrap();
        assert_eq!(to, ["ops@example.com"]);
        assert!(sent.contains("Bcc: x@example.com\r\n"), "{sent}");
    }

    #[test]
    fn a_date_and_a_from_are_added_only_when_missing() {
        let options = Options { recipients: vec!["ops@example.com".into()], ..Options::default() };
        let (_, sent) = text(&options, "Subject: report\n\nbody\n").unwrap();
        assert_eq!(
            sent,
            "Subject: report\r\nDate: Thu, 13 Nov 2025 10:42:11 +0000\r\nFrom: alice@box\r\n\r\nbody\r\n"
        );
        let (_, sent) =
            text(&options, "date: Mon, 1 Jan 2024 00:00:00 +0000\nfrom: cron@box\n\nbody\n")
                .unwrap();
        assert_eq!(sent, "date: Mon, 1 Jan 2024 00:00:00 +0000\r\nfrom: cron@box\r\n\r\nbody\r\n");
        let named = Options { full_name: Some("Cron \"Daemon\"".into()), ..options };
        let (_, sent) = text(&named, "Subject: x\n\nbody\n").unwrap();
        assert!(sent.contains("From: \"Cron Daemon\" <alice@box>\r\n"), "{sent}");
    }

    #[test]
    fn a_message_without_headers_is_all_body() {
        let options = Options { recipients: vec!["ops@example.com".into()], ..Options::default() };
        let (_, sent) = text(&options, "just a line\nand another").unwrap();
        assert!(sent.ends_with("\r\n\r\njust a line\r\nand another\r\n"), "{sent}");
        let (_, sent) = text(&options, "").unwrap();
        assert!(sent.ends_with("\r\n\r\n"), "{sent}");
    }

    #[test]
    fn folded_headers_stay_folded_and_are_read_whole() {
        let options = Options { from_headers: true, ..Options::default() };
        let (to, sent) =
            text(&options, "To: a@example.com,\n b@example.com\nSubject: s\n\nbody\r\nmore\r\n")
                .unwrap();
        assert_eq!(to, ["a@example.com", "b@example.com"]);
        assert!(
            sent.starts_with("To: a@example.com,\r\n b@example.com\r\nSubject: s\r\n"),
            "{sent}"
        );
        assert!(sent.ends_with("\r\n\r\nbody\r\nmore\r\n"), "{sent}");
    }

    #[test]
    fn an_address_without_a_domain_is_the_machines() {
        assert_eq!(addresses("root", "box").unwrap(), ["root@box"]);
        assert_eq!(addresses("root, Ann <ann>", "box").unwrap(), ["root@box", "ann@box"]);
        assert_eq!(
            addresses("team: a@x.test, b@x.test;", "box").unwrap(),
            ["a@x.test", "b@x.test"]
        );
        assert!(addresses("two words", "box").is_err());
        assert!(addresses("a@b.test c@d.test", "box").is_err());
    }

    #[test]
    fn no_recipient_is_said_by_how_to_give_one() {
        let why = text(&Options::default(), "Subject: x\n\nbody\n").unwrap_err();
        assert!(why.contains("give them as arguments, or -t"), "{why}");
        let why =
            text(&Options { from_headers: true, ..Options::default() }, "Subject: x\n\nbody\n")
                .unwrap_err();
        assert!(why.contains("no To, Cc or Bcc header"), "{why}");
    }

    #[test]
    fn a_message_goes_to_the_server_with_its_recipients_and_without_its_bcc() {
        use crate::mail::testing::{Script, client, smtp_server};
        let options = Options { from_headers: true, ..Options::default() };
        let input = "To: ops@example.com\nBcc: hidden@example.net\nSubject: report\n\n.dot\n";
        let (to, message) =
            prepare(&options, input.as_bytes(), "box", "alice", 1_763_030_531).unwrap();
        let (port, server) = smtp_server(Script::new(), true);
        mail::deliver_over(&client(port, None), "cron@box", &to, &message, "box", true).unwrap();
        let seen = server.join().unwrap();
        assert!(seen.commands.contains(&"MAIL FROM:<cron@box>".to_string()), "{:?}", seen.commands);
        for address in ["ops@example.com", "hidden@example.net"] {
            assert!(seen.commands.contains(&format!("RCPT TO:<{address}>")), "{:?}", seen.commands);
        }
        let sent = seen.message.unwrap();
        assert!(!sent.contains("hidden@example.net"), "{sent}");
        assert!(sent.ends_with("\r\n\r\n.dot\r\n"), "{sent}");
    }

    #[test]
    fn each_kind_of_failure_has_its_sysexits_status() {
        assert_eq!(exit_status(Kind::Temporary), 75);
        assert_eq!(exit_status(Kind::Refused), 69);
        assert_eq!(exit_status(Kind::NoServer), 78);
    }

    #[test]
    fn line_ends_become_crlf_once() {
        assert_eq!(crlf(b"a\nb\r\nc"), b"a\r\nb\r\nc\r\n");
        assert_eq!(crlf(b""), b"");
        assert_eq!(crlf(b"\n"), b"\r\n");
    }
}
