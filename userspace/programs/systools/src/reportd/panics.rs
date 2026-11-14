// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Panics of services (ADR 0531). A program built on Rust's standard
//! library unwinds on a panic and ends with status 101, with no signal,
//! so the kernel keeps no record of it; what it leaves is its standard
//! error's lines in the log:
//!
//! ```text
//! thread 'main' panicked at src/main.rs:12:5:
//! index out of bounds: the len is 3 but the index is 7
//! note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace
//! ```
//!
//! reportd follows the log for services' `panicked at` lines, and reads
//! the lines after each from the same process for its message. A panic
//! of a thread that the program lives through is a crash as well: a bug
//! the program's makers want to hear of.

use crate::logd::{self, Record, Request};

/// The most lines of a panic's message kept.
const MESSAGE_LINES: usize = 10;

/// A panic in the log.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Panic {
    /// The service.
    pub service: String,
    pub process: Option<u64>,
    pub user: u32,
    /// When, in nanoseconds since 1970.
    pub at_ns: u64,
    /// `src/main.rs:12:5`.
    pub location: String,
    /// The message's lines.
    pub message: Vec<String>,
}

/// The location in a `thread 'main' panicked at src/main.rs:12:5:` line,
/// and what follows it on the line in Rust before 1.73, where the message
/// came after the location's comma.
pub fn location(text: &str) -> Option<(String, Option<String>)> {
    let rest = text.split_once(" panicked at ")?.1.trim();
    // Rust 1.73 on: `at LOCATION:` and the message on the lines after.
    if let Some(location) = rest.strip_suffix(':') {
        return Some((location.to_string(), None));
    }
    // Earlier: `at 'MESSAGE', LOCATION`.
    if let Some((message, location)) = rest.rsplit_once("', ") {
        let message = message.strip_prefix('\'').unwrap_or(message);
        return Some((location.to_string(), Some(message.to_string())));
    }
    Some((rest.to_string(), None))
}

/// The message after a panic's line: the records of the same process
/// after it, to the note about RUST_BACKTRACE.
pub fn message(after: &[Record], process: Option<u64>) -> Vec<String> {
    after
        .iter()
        .filter(|r| r.process == process)
        .map(|r| r.text.clone())
        .take_while(|text| !text.starts_with("note: run with `RUST_BACKTRACE"))
        .take(MESSAGE_LINES)
        .collect()
}

/// The panic `record` tells of, if it is a service's `panicked at` line,
/// with the records after it.
pub fn of(record: &Record, after: &[Record]) -> Option<Panic> {
    if record.from != "service" {
        return None;
    }
    let (location, inline) = location(&record.text)?;
    let message = match inline {
        Some(message) => vec![message],
        None => message(after, record.process),
    };
    Some(Panic {
        service: record.name.clone(),
        process: record.process,
        user: record.user.unwrap_or(0),
        at_ns: logd::parse_rfc3339(&record.time).unwrap_or(0),
        location,
        message,
    })
}

/// Follows the log for services' panics, calling `each` with each, until
/// logd closes the connection: from `since` on, to go on after a restart
/// of logd without a panic told twice or missed.
///
/// # Errors
///
/// When logd cannot be asked.
pub fn follow(since: Option<u64>, mut each: impl FnMut(Panic)) -> Result<(), String> {
    let request = Request {
        grep: Some(" panicked at ".into()),
        follow: true,
        since,
        lines: since.is_none().then_some(0),
        ..Request::default()
    };
    logd::ask(&request, |line| {
        let Ok(record) = serde_json::from_str::<Record>(line) else { return Ok(true) };
        if record.from != "service" {
            return Ok(true);
        }
        // The message's lines are written after the panic's own; a moment
        // lets them reach the log.
        std::thread::sleep(std::time::Duration::from_millis(500));
        let after = lines_after(&record);
        if let Some(panic) = of(&record, &after) {
            each(panic);
        }
        Ok(true)
    })
    .map_err(|why| why.message)
}

/// The service's records in the two seconds from `record` on, after it.
fn lines_after(record: &Record) -> Vec<Record> {
    let Some(at) = logd::parse_rfc3339(&record.time) else { return Vec::new() };
    let request = Request {
        services: vec![record.name.clone()],
        since: Some(at),
        until: Some(at + 2_000_000_000),
        ..Request::default()
    };
    let mut records = Vec::new();
    let _ = logd::ask(&request, |line| {
        if let Ok(r) = serde_json::from_str::<Record>(line) {
            records.push(r);
        }
        Ok(records.len() < 200)
    });
    // Those after the panic's own line.
    match records.iter().position(|r| r.seq == record.seq && r.boot == record.boot) {
        Some(at) => records.split_off(at + 1),
        None => records,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(seq: u64, process: u64, text: &str) -> Record {
        Record {
            time: "2026-11-13T10:02:11.000000Z".into(),
            boot: "b".into(),
            seq,
            level: "error".into(),
            from: "service".into(),
            name: "crash-test".into(),
            process: Some(process),
            user: Some(0),
            audit: false,
            text: text.into(),
        }
    }

    #[test]
    fn a_panic_has_its_location_and_the_message_after_it() {
        let line = record(1, 40, "thread 'main' panicked at src/main.rs:12:5:");
        let after = [
            record(2, 40, "index out of bounds: the len is 3 but the index is 7"),
            record(3, 41, "another process's line"),
            record(
                4,
                40,
                "note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace",
            ),
            record(5, 40, "a later line"),
        ];
        let panic = of(&line, &after).unwrap();
        assert_eq!(panic.location, "src/main.rs:12:5");
        assert_eq!(panic.message, ["index out of bounds: the len is 3 but the index is 7"]);
        assert_eq!(panic.service, "crash-test");
        assert_eq!(panic.at_ns, 1_794_564_131_000_000_000);
    }

    #[test]
    fn the_old_form_has_the_message_on_its_line() {
        let (location, message) =
            location("thread 'main' panicked at 'boom', src/main.rs:3:5").unwrap();
        assert_eq!((location.as_str(), message.as_deref()), ("src/main.rs:3:5", Some("boom")));
    }

    #[test]
    fn a_programs_line_is_no_services_panic() {
        let mut line = record(1, 40, "thread 'main' panicked at src/main.rs:12:5:");
        line.from = "program".into();
        assert!(of(&line, &[]).is_none());
        assert!(of(&record(1, 40, "all is well"), &[]).is_none());
    }
}
