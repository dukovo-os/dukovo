// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The words service(8) and [`explain`](crate::explain) use for what the
//! `show` document holds: a length of time, a list of names, what a
//! service is ready once. They live here so that the console says them as
//! the command line does.
//!
//! If a sentence reads wrong, the rule is here and its test is below;
//! `./ob test unit` runs it.

use alloc::format;
use alloc::string::String;

/// A length of time in words, rounded down: "4 s", "2 min", "3 h 5 min",
/// "2 days".
pub fn span(ms: u64) -> String {
    let seconds = ms / 1000;
    match seconds {
        0..60 => format!("{seconds} s"),
        60..3600 => format!("{} min", seconds / 60),
        3600..86_400 if seconds % 3600 >= 60 => {
            format!("{} h {} min", seconds / 3600, seconds % 3600 / 60)
        }
        3600..86_400 => format!("{} h", seconds / 3600),
        _ if seconds < 2 * 86_400 => String::from("1 day"),
        _ => format!("{} days", seconds / 86_400),
    }
}

/// A duration in milliseconds as Spark's lines write one: "10.4 s".
pub fn tenths(ms: u64) -> String {
    format!("{}.{} s", ms / 1000, ms % 1000 / 100)
}

/// `names` in words: "a", "a and b", "a, b and c".
pub fn and_list(names: &[String]) -> String {
    match names {
        [] => String::new(),
        [one] => one.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

/// What a service waits for to be ready, after "ready once": the words
/// for a service file's `ready.when`.
pub fn ready_once(when: &str) -> &str {
    match when {
        "listening" => "it listens",
        "socket" => "its socket is made",
        "healthy" => "its health check passes",
        "ended" => "it has ended",
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spans_read_as_words() {
        assert_eq!(span(4_999), "4 s");
        assert_eq!(span(120_000), "2 min");
        assert_eq!(span(3_600_000), "1 h");
        assert_eq!(span(11_100_000), "3 h 5 min");
        assert_eq!(span(90_000_000), "1 day");
        assert_eq!(span(200_000_000), "2 days");
        assert_eq!(tenths(10_449), "10.4 s");
    }

    #[test]
    fn names_read_as_a_list() {
        assert_eq!(and_list(&[]), "");
        assert_eq!(and_list(&["a".into()]), "a");
        assert_eq!(and_list(&["a".into(), "b".into()]), "a and b");
        assert_eq!(and_list(&["a".into(), "b".into(), "c".into()]), "a, b and c");
    }

    #[test]
    fn a_service_is_ready_once_by_the_words_of_its_file() {
        assert_eq!(ready_once("listening"), "it listens");
        assert_eq!(ready_once("socket"), "its socket is made");
        assert_eq!(ready_once("healthy"), "its health check passes");
        assert_eq!(ready_once("ended"), "it has ended");
        assert_eq!(ready_once("started"), "started");
    }
}
