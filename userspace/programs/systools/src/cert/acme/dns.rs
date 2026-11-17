// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! DNS-01 by hand (ADR 0513): the TXT records a person adds to prove they
//! hold a name, and the wait until the system's resolver shows them.
//!
//! The record for `example.com` and for `*.example.com` is the same name,
//! `_acme-challenge.example.com`, with one value each, so an order for
//! both needs both values there at once: every record is shown first, then
//! one Enter, then the wait for all of them.

use std::time::Duration;

use netproto::dns;

use crate::dig::{self, Asking};

/// How often the resolver is asked, and for how long.
pub const EVERY: Duration = Duration::from_secs(10);
pub const UP_TO: Duration = Duration::from_secs(600);

/// One record to add: the name it is asked for as, and its value.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Record {
    /// The DNS name the order is for, as given: `www.example.com` or
    /// `*.example.com`.
    pub name: String,
    /// The TXT record's value.
    pub value: String,
}

impl Record {
    /// The name the TXT record is made at, absolute: a wildcard's is its
    /// base name's (RFC 8555 section 8.4).
    pub fn at(&self) -> String {
        let base = self.name.strip_prefix("*.").unwrap_or(&self.name);
        format!("_acme-challenge.{}.", base.trim_end_matches('.'))
    }

    /// The name the record is made at, as a DNS provider's page takes it:
    /// [`at`](Record::at) without the dot at its end.
    pub fn label(&self) -> String {
        self.at().trim_end_matches('.').to_string()
    }

    /// The line `cert get --progress` gives for the record, for a page to
    /// show: `add: NAME TXT "VALUE"`.
    pub fn add_line(&self) -> String {
        format!("add: {} TXT \"{}\"", self.label(), self.value)
    }

    /// The record as a zone file shows it.
    pub fn line(&self) -> String {
        format!("{}  TXT  \"{}\"", self.at(), self.value)
    }
}

/// What is shown before the wait for the person: every record, then what
/// to do, which is to press Enter when `enter` asks for it.
pub fn instructions(records: &[Record], enter: bool) -> String {
    let mut text = String::new();
    let noun = if records.len() == 1 { "this TXT record" } else { "these TXT records" };
    let then = if enter { ", then press Enter" } else { "" };
    text.push_str(&format!("Add {noun}{then}:\n"));
    for record in records {
        text.push_str(&format!("  {}\n", record.line()));
    }
    text
}

/// The strings of the TXT records of `answer` that are for `at`, each
/// record's strings joined, as a resolver's answer message holds them.
pub fn txt_values(answer: &[u8]) -> Vec<String> {
    let Some(records) = dns::records(answer) else { return Vec::new() };
    let mut values = Vec::new();
    for record in records.iter().filter(|record| record.section == 0 && record.rtype == dns::TXT) {
        let mut data = &answer[record.data.0..record.data.1];
        let mut value = Vec::new();
        while let Some((&length, rest)) = data.split_first() {
            let length = usize::from(length).min(rest.len());
            value.extend_from_slice(&rest[..length]);
            data = &rest[length..];
        }
        values.push(String::from_utf8_lossy(&value).into_owned());
    }
    values
}

/// The TXT values the system's resolver gives for `at`, none when it
/// has none or does not answer.
pub fn lookup(at: &str) -> Vec<String> {
    let asking = Asking { tries: 1, time: Duration::from_secs(5), ..Asking::default() };
    let Ok(query) = dig::build(at, dns::TXT, dns::IN, &asking) else { return Vec::new() };
    match dig::exchange(&query, &asking, false) {
        Ok(reply) => txt_values(&reply.message),
        Err(_) => Vec::new(),
    }
}

/// How a wait for the records stands after a look.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Look {
    /// The look that was made, from 1.
    pub number: u64,
    /// How many looks the wait makes at most.
    pub of: u64,
    /// How many records have shown so far, and how many there are.
    pub found: usize,
    pub records: usize,
}

/// What a wait tells before and after each look.
pub struct Watch<'a> {
    /// Called before each look: it empties the resolver's cache, so that
    /// the answer of none from before the record was added is not kept.
    pub before: &'a mut dyn FnMut(),
    /// Called after each look, with how the wait stands.
    pub after: &'a mut dyn FnMut(&Look),
}

/// Waits until `ask` shows every record, looking each `every` for up to
/// `up_to`, `pause` being the wait between looks; `say` is told when it
/// waits, once for each name, and `watch` before and after each look.
///
/// # Errors
///
/// With the names whose records did not show in time.
pub fn wait_for(
    records: &[Record],
    every: Duration,
    up_to: Duration,
    ask: &mut dyn FnMut(&str) -> Vec<String>,
    pause: &mut dyn FnMut(Duration),
    say: &mut dyn FnMut(&str),
    watch: &mut Watch<'_>,
) -> Result<(), String> {
    let looks = (up_to.as_secs() / every.as_secs().max(1)).max(1);
    let mut missing: Vec<&Record> = records.iter().collect();
    let mut told = false;
    for look in 0..looks {
        (watch.before)();
        missing.retain(|record| !ask(&record.at()).contains(&record.value));
        (watch.after)(&Look {
            number: look + 1,
            of: looks,
            found: records.len() - missing.len(),
            records: records.len(),
        });
        if missing.is_empty() {
            return Ok(());
        }
        if !told {
            for record in &missing {
                say(&format!(
                    "waiting for the TXT record of {} to show (DNS can take minutes)",
                    record.name
                ));
            }
            told = true;
        }
        if look + 1 < looks {
            pause(every);
        }
    }
    let names: Vec<&str> = missing.iter().map(|record| record.name.as_str()).collect();
    Err(format!(
        "the TXT record of {} did not show within {} minutes: check the record, then run the \
         command again",
        names.join(", "),
        up_to.as_secs() / 60
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(name: &str, value: &str) -> Record {
        Record { name: name.into(), value: value.into() }
    }

    #[test]
    fn a_record_is_made_at_the_challenge_name_of_the_base_name() {
        assert_eq!(record("example.com", "abc").at(), "_acme-challenge.example.com.");
        assert_eq!(record("*.example.com", "abc").at(), "_acme-challenge.example.com.");
        assert_eq!(record("www.example.com.", "abc").at(), "_acme-challenge.www.example.com.");
        assert_eq!(
            record("example.com", "x-Y_9").line(),
            "_acme-challenge.example.com.  TXT  \"x-Y_9\""
        );
    }

    #[test]
    fn the_line_for_a_page_has_the_name_without_its_last_dot() {
        assert_eq!(record("shop.example.org", "v1").label(), "_acme-challenge.shop.example.org");
        assert_eq!(
            record("*.shop.example.org", "v2").add_line(),
            "add: _acme-challenge.shop.example.org TXT \"v2\""
        );
    }

    #[test]
    fn what_is_shown_names_every_record_and_what_to_do() {
        let one = instructions(&[record("example.com", "v1")], true);
        assert_eq!(
            one,
            "Add this TXT record, then press Enter:\n  _acme-challenge.example.com.  TXT  \"v1\"\n"
        );
        let two = instructions(&[record("example.com", "v1"), record("*.example.com", "v2")], true);
        assert!(two.starts_with("Add these TXT records, then press Enter:\n"));
        assert_eq!(two.lines().count(), 3);
        assert!(two.contains("TXT  \"v2\""));
        // A script is not asked to press Enter.
        assert_eq!(
            instructions(&[record("example.com", "v1")], false).lines().next(),
            Some("Add this TXT record:")
        );
    }

    #[test]
    fn the_strings_of_a_txt_record_are_joined() {
        // A message of one question and two TXT answers: "ab" "cd" and "ef".
        let mut message = vec![0, 1, 0x81, 0x80, 0, 1, 0, 2, 0, 0, 0, 0];
        message.extend_from_slice(b"\x01a\x00\x00\x10\x00\x01");
        for data in [&b"\x02ab\x02cd"[..], &b"\x02ef"[..]] {
            message.extend_from_slice(&[0xc0, 12, 0, 16, 0, 1, 0, 0, 0, 60]);
            message.extend_from_slice(&(data.len() as u16).to_be_bytes());
            message.extend_from_slice(data);
        }
        assert_eq!(txt_values(&message), ["abcd", "ef"]);
        assert!(txt_values(&message[..20]).is_empty());
    }

    #[test]
    fn it_waits_until_each_record_shows_and_says_so_once() {
        let records = [record("a.example", "va"), record("*.a.example", "vb")];
        let mut asked = 0;
        let mut ask = |_: &str| {
            asked += 1;
            // The first look shows nothing; the second, the first value;
            // from the fourth on, both.
            match asked {
                1 | 2 => Vec::new(),
                3..=4 => vec![String::from("other"), String::from("va")],
                _ => vec![String::from("va"), String::from("vb")],
            }
        };
        let (mut pauses, mut said) = (Vec::new(), Vec::new());
        let (mut flushed, mut looks) = (0, Vec::new());
        let result = wait_for(
            &records,
            Duration::from_secs(10),
            Duration::from_secs(600),
            &mut ask,
            &mut |time| pauses.push(time),
            &mut |text| said.push(text.to_string()),
            &mut Watch { before: &mut || flushed += 1, after: &mut |look| looks.push(*look) },
        );
        assert_eq!(result, Ok(()));
        assert_eq!(pauses, [Duration::from_secs(10); 2]);
        // The cache is emptied before each of the three looks, and each look
        // says how many records have shown.
        assert_eq!(flushed, 3);
        let stood: Vec<(u64, usize)> = looks.iter().map(|look| (look.number, look.found)).collect();
        assert_eq!(stood, [(1, 0), (2, 1), (3, 2)]);
        assert!(looks.iter().all(|look| look.of == 60 && look.records == 2));
        assert_eq!(
            said,
            [
                "waiting for the TXT record of a.example to show (DNS can take minutes)",
                "waiting for the TXT record of *.a.example to show (DNS can take minutes)"
            ]
        );
    }

    #[test]
    fn a_record_that_never_shows_is_named_after_the_time_is_up() {
        let records = [record("a.example", "va"), record("b.example", "vb")];
        let mut ask = |at: &str| {
            if at.contains("a.example") { vec![String::from("va")] } else { Vec::new() }
        };
        let mut pauses = 0;
        let result = wait_for(
            &records,
            Duration::from_secs(10),
            Duration::from_secs(600),
            &mut ask,
            &mut |_| pauses += 1,
            &mut |_| {},
            &mut Watch { before: &mut || {}, after: &mut |_| {} },
        );
        assert_eq!(
            result.unwrap_err(),
            "the TXT record of b.example did not show within 10 minutes: check the record, then \
             run the command again"
        );
        assert_eq!(pauses, 59);
    }
}
