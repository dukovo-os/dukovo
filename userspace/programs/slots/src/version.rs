// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Dukovo's version numbers, compared as an update compares them: three
//! numbers, or four for a security fix's build number (`1.4.1.1`, the
//! updates design's section 9), then a pre-release after `-`
//! (`1.0.0-alpha.1`), then build metadata after `+`, which says how a
//! release was built and does not make it newer, as Semantic Versioning
//! 2.0.0 has it. A pre-release comes before its release; its
//! dot-separated parts compare as numbers when they are numbers, a
//! number before a word, and fewer parts before more.

use alloc::vec::Vec;
use core::cmp::Ordering;

/// A version, parsed. Two versions are equal when they have the same
/// precedence: build metadata does not count.
#[derive(Clone, Debug)]
pub struct Version<'a> {
    numbers: [u64; 4],
    pre: Vec<&'a str>,
    build: &'a str,
}

impl<'a> Version<'a> {
    /// `text` as a version: `None` if it is not one.
    #[must_use]
    pub fn parse(text: &'a str) -> Option<Version<'a>> {
        let (rest, build) = text.split_once('+').unwrap_or((text, ""));
        let (core, pre) = match rest.split_once('-') {
            Some((core, pre)) => (core, Some(pre)),
            None => (rest, None),
        };
        let mut numbers = [0u64; 4];
        let mut count = 0;
        for part in core.split('.') {
            if count == numbers.len() || !number(part) {
                return None;
            }
            numbers[count] = part.parse().ok()?;
            count += 1;
        }
        if count < 3 {
            return None;
        }
        let pre: Vec<&str> = match pre {
            Some(pre) => pre.split('.').collect(),
            None => Vec::new(),
        };
        if pre.iter().any(|part| !identifier(part) || (numeric(part) && !number(part))) {
            return None;
        }
        if (!build.is_empty() || text.ends_with('+'))
            && build.split('.').any(|part| !identifier(part))
        {
            return None;
        }
        Some(Version { numbers, pre, build })
    }

    /// The build metadata, after `+`, or nothing.
    #[must_use]
    pub const fn build(&self) -> &'a str {
        self.build
    }
}

/// Whether `part` is digits alone.
fn numeric(part: &str) -> bool {
    !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit())
}

/// Whether `part` is a number as a version writes one: digits, without a
/// leading zero.
fn number(part: &str) -> bool {
    numeric(part) && (part == "0" || !part.starts_with('0')) && part.len() <= 19
}

/// Whether `part` is a pre-release's or build's identifier: letters,
/// digits and `-`, at least one.
fn identifier(part: &str) -> bool {
    !part.is_empty() && part.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
}

impl PartialEq for Version<'_> {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl Eq for Version<'_> {}

impl PartialOrd for Version<'_> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Version<'_> {
    /// Precedence: build metadata does not count, so two versions that
    /// differ only in it are equal, though not the same release.
    fn cmp(&self, other: &Self) -> Ordering {
        self.numbers.cmp(&other.numbers).then_with(|| match (self.pre.len(), other.pre.len()) {
            (0, 0) => Ordering::Equal,
            (0, _) => Ordering::Greater,
            (_, 0) => Ordering::Less,
            _ => {
                for (a, b) in self.pre.iter().zip(&other.pre) {
                    let order = match (numeric(a), numeric(b)) {
                        (true, true) => {
                            a.parse::<u64>().unwrap_or(0).cmp(&b.parse::<u64>().unwrap_or(0))
                        }
                        (true, false) => Ordering::Less,
                        (false, true) => Ordering::Greater,
                        (false, false) => a.cmp(b),
                    };
                    if order != Ordering::Equal {
                        return order;
                    }
                }
                self.pre.len().cmp(&other.pre.len())
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(text: &str) -> Version<'_> {
        Version::parse(text).unwrap_or_else(|| panic!("{text} is a version"))
    }

    #[test]
    fn orders_releases_fixes_and_pre_releases() {
        // Semantic Versioning's own example, then Dukovo's ladder.
        let ordered = [
            "1.0.0-alpha",
            "1.0.0-alpha.1",
            "1.0.0-alpha.beta",
            "1.0.0-beta",
            "1.0.0-beta.2",
            "1.0.0-beta.11",
            "1.0.0-rc.1",
            "1.0.0",
            "1.0.1",
            "1.4.1",
            "1.4.1.1",
            "1.4.1.2",
            "1.4.2",
            "1.10.0",
            "2.0.0",
        ];
        for pair in ordered.windows(2) {
            assert!(v(pair[0]) < v(pair[1]), "{} < {}", pair[0], pair[1]);
        }
        assert_eq!(v("0.15.0+good").cmp(&v("0.15.0")), Ordering::Equal);
        assert_eq!(v("0.15.0+good").build(), "good");
        assert_eq!(v("1.4.1.0"), v("1.4.1"));
        assert_eq!(v("0.15.0+good"), v("0.15.0"));
    }

    #[test]
    fn refuses_what_is_not_a_version() {
        for text in [
            "",
            "1",
            "1.2",
            "1.2.3.4.5",
            "1.02.3",
            "a.b.c",
            "1.2.3-",
            "1.2.3+",
            "1.2.3-01",
            "1.2.3-a..b",
            "1.2.3+a_b",
            "v1.2.3",
            "1.2.3 ",
        ] {
            assert!(Version::parse(text).is_none(), "{text:?}");
        }
    }
}
