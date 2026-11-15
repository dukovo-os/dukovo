// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! What the networking and firewall actions share (ADR 0518): the window
//! that a change may be tried for, and the sentence of a refusal.
//!
//! A change with a window is tried: its command ends with `--try SECONDS`,
//! and Spark undoes it after that many seconds unless it is kept
//! (`network keep`, `firewall keep`). The seconds are checked as Spark
//! checks them, by [`spark_api::trial::SECONDS`].

use super::{Bad, Field};

/// The field of the window to try a change for.
pub(super) const TRY: Field = Field::number(
    "try",
    "Try the change for this many seconds, 10 to 3600: Spark undoes it unless it is kept.",
    10,
    3600,
)
.optional();

/// `text`, which a command says in a few words, as the sentence of a
/// refusal: its first letter a capital, and what was not done after it.
pub(super) fn refusal(text: &str) -> String {
    let mut letters = text.trim_end_matches('.').chars();
    let first: String = letters.next().map(|c| c.to_ascii_uppercase()).into_iter().collect();
    format!("{first}{}. Nothing was done.", letters.as_str())
}

/// `seconds`, a window to try a change for, checked.
///
/// # Errors
///
/// A refusal of the field `try` for a window Spark would not take.
pub(super) fn check_try(seconds: Option<i64>) -> Result<Option<u32>, Bad> {
    let Some(seconds) = seconds else { return Ok(None) };
    let range = spark_api::trial::SECONDS;
    match u32::try_from(seconds).ok().filter(|seconds| range.contains(seconds)) {
        Some(seconds) => Ok(Some(seconds)),
        None => {
            let why = format!("A change is tried for {} to {} seconds", range.start(), range.end());
            Err(Bad { field: "try", why: refusal(&why) })
        }
    }
}

/// The words that end a command with a window: `--try SECONDS`, or none
/// when there is no window.
pub(super) fn try_words(seconds: Option<i64>) -> Vec<String> {
    match check_try(seconds) {
        Ok(Some(seconds)) => vec!["--try".into(), seconds.to_string()],
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_refusal_starts_with_a_capital_and_says_that_nothing_was_done() {
        assert_eq!(refusal("gateway is not here."), "Gateway is not here. Nothing was done.");
        assert_eq!(refusal("Already a capital"), "Already a capital. Nothing was done.");
        assert_eq!(
            refusal("10.0.2.3 is given twice."),
            "10.0.2.3 is given twice. Nothing was done."
        );
        assert_eq!(refusal(""), ". Nothing was done.");
    }

    #[test]
    fn a_window_is_the_seconds_spark_takes_and_nothing_else() {
        assert_eq!(check_try(None), Ok(None));
        let range = spark_api::trial::SECONDS;
        for seconds in [i64::from(*range.start()), 30, i64::from(*range.end())] {
            assert_eq!(check_try(Some(seconds)).unwrap(), u32::try_from(seconds).ok());
        }
        for seconds in [i64::MIN, -10, 0, 9, 3601, i64::from(u32::MAX) + 10, i64::MAX] {
            let bad = check_try(Some(seconds)).unwrap_err();
            assert_eq!(bad.field, "try", "{seconds}");
            assert_eq!(bad.why, "A change is tried for 10 to 3600 seconds. Nothing was done.");
        }
    }

    #[test]
    fn the_window_ends_the_command_as_two_words_and_no_window_adds_none() {
        assert_eq!(try_words(Some(30)), ["--try", "30"]);
        assert_eq!(try_words(Some(3600)), ["--try", "3600"]);
        assert!(try_words(None).is_empty());
        assert!(try_words(Some(5)).is_empty(), "a window that is refused is not passed on");
    }

    #[test]
    fn the_field_is_optional_and_says_the_same_limits() {
        assert_eq!((TRY.name, TRY.required), ("try", false));
        let super::super::Type::Number { min, max } = TRY.kind else { panic!("{:?}", TRY.kind) };
        assert_eq!((min, max), (10, 3600));
        assert_eq!(
            (min, max),
            (
                i64::from(*spark_api::trial::SECONDS.start()),
                i64::from(*spark_api::trial::SECONDS.end())
            )
        );
    }
}
