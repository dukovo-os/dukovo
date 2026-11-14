// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The services a try of changes bisect leaves out (ADR 0475). Each try
//! has a list in `/system/state/bisect`, `try-N`, a service name on each
//! line, which changes bisect writes before the try starts. When the
//! kernel's command line says `trial=N`, Spark reads the list of try N
//! and holds those services off as meant, with [`why`] as the reason
//! `show` gives, so that `service list --failed` and the health check do
//! not count them. Without `trial=N` the lists are ignored.
//!
//! Spark has no host tests of its own; the rules are here so that
//! `./ob test unit` runs them. The `bisect` boot test covers Spark's side.

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

/// The file of try `number`'s list, in `/system/state/bisect`.
pub fn file(number: u8) -> String {
    format!("try-{number}")
}

/// The service names a list holds: one on each line, without the white
/// space around it. Blank lines hold none.
pub fn names(list: &str) -> Vec<String> {
    list.lines().map(str::trim).filter(|line| !line.is_empty()).map(ToString::to_string).collect()
}

/// Why try `number` leaves a service out, as `show` gives it.
pub fn why(number: u8) -> String {
    format!("left out for try {number} of changes bisect")
}

/// Whether `reason` is [`why`] of some try.
pub fn is_why(reason: &str) -> bool {
    reason
        .strip_prefix("left out for try ")
        .and_then(|rest| rest.strip_suffix(" of changes bisect"))
        .is_some_and(|number| number.parse::<u8>().is_ok())
}

#[cfg(test)]
mod tests {
    use alloc::string::String;
    use alloc::vec;

    use super::*;

    #[test]
    fn a_list_holds_the_names_of_its_lines() {
        assert_eq!(names("beacon\nbeacon-web\n"), ["beacon", "beacon-web"]);
        assert_eq!(names("  beacon \r\n\n\t\nweb"), ["beacon", "web"]);
        assert_eq!(names(""), Vec::<String>::new());
        assert_eq!(names("\n \n"), Vec::<String>::new());
    }

    #[test]
    fn a_try_has_a_file_and_a_reason_that_name_its_number() {
        assert_eq!(file(1), "try-1");
        assert_eq!(file(99), "try-99");
        assert_eq!(why(2), "left out for try 2 of changes bisect");
    }

    #[test]
    fn only_a_reason_of_a_try_is_one() {
        for number in [1, 2, 99] {
            assert!(is_why(&why(number)));
        }
        let others = vec![
            "Rescue leaves it off until it is started",
            "left out for try of changes bisect",
            "left out for try x of changes bisect",
            "left out for try 300 of changes bisect",
            "left out for try 2 of changes bisect, and more",
            "",
        ];
        for other in others {
            assert!(!is_why(other), "{other}");
        }
    }
}
