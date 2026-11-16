// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The refusal of admin(8) that has an ID of its own, ADM-138, as the
//! failure a command exits with. The ID ends the line in parentheses, as
//! the other tools' IDs do (`NET-181`): the web console reads it from the
//! last line of the command's error output when its action names it, and
//! the support center's page for it is found by it.
//!
//! The console's own errors, ADM-101 to ADM-137 and ADM-139, are
//! `admind::errors`'s. The words do not start with the command's name:
//! `main` puts it before the message.

use crate::util::{Fail, fail};

/// ADM-138: `admin reach`, `admin certificate` or `admin sign-in` while
/// the `console` trial runs. Each writes `admin.toml`, which Spark puts
/// back as it was before the trial, so a change made now would be lost.
pub fn console_is_tried() -> Fail {
    fail(
        "the console's address or certificate is being tried, and its settings file is in that \
         trial: keep or undo that change first, with `admin keep` or `admin undo` (ADM-138)"
            .into(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_refusal_ends_with_its_id_and_exits_with_status_1() {
        let refusal = console_is_tried();
        assert!(refusal.message.ends_with(" (ADM-138)"), "{}", refusal.message);
        assert_eq!(refusal.message.lines().count(), 1);
        assert_eq!((refusal.code, refusal.usage), (1, false));
    }

    #[test]
    fn the_words_say_what_to_run() {
        let words = console_is_tried().message;
        assert!(words.starts_with("the console's address or certificate is being tried"));
        assert!(words.contains("`admin keep` or `admin undo`"));
    }
}
