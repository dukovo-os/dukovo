// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! How bundle versions order (ADR 0595): by precedence, as semantic
//! versioning orders releases, then by build number.
//!
//! A build number, `_2` to `_9999` at the very end of a version, marks a
//! version packed again from the same upstream source, after a change to
//! what it was built with. Build 1 is the version as upstream writes it,
//! with no number, so that one build never has two spellings. Neither a
//! pre-release nor build metadata may hold `_`, so the last `_` splits a
//! version without doubt.
//!
//! bundled, bundle(1), the catalog's tools and the console all order
//! versions with [`compare_versions`]; when two of them disagree on which
//! version is newest, the cause is a copy of this order somewhere else.

use core::cmp::Ordering;

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

/// The highest build number a version may end in.
pub const BUILD_MAX: u32 = 9999;

/// The version without its build number, and the build: `("15.2.0", 2)`
/// for `15.2.0_2`; the whole of `version` and 1 when it ends in no build
/// number. An ending after `_` that is no build number (`_1`, `_01`,
/// `_x`) stays part of the version, which [`check_version`] then refuses.
///
/// [`check_version`]: super::check_version
#[must_use]
pub fn split_build(version: &str) -> (&str, u32) {
    let Some((release, build)) = version.rsplit_once('_') else {
        return (version, 1);
    };
    let is_number = !build.is_empty() && build.bytes().all(|b| b.is_ascii_digit());
    match build.parse::<u32>() {
        Ok(number)
            if is_number
                && !release.is_empty()
                && !build.starts_with('0')
                && (2..=BUILD_MAX).contains(&number) =>
        {
            (release, number)
        }
        _ => (version, 1),
    }
}

/// `version`, which ends in no build number, as build `build`: `version`
/// itself for build 1, else `VERSION_BUILD`.
#[must_use]
pub fn with_build(version: &str, build: u32) -> String {
    if build <= 1 { version.to_string() } else { format!("{version}_{build}") }
}

/// Orders two bundle versions: by precedence, then by build, a missing
/// build being 1.
///
/// Precedence is semantic versioning's: the numbers, a missing one 0,
/// then a pre-release before its release, pre-releases by their
/// dot-separated parts, numbers before words; build metadata after `+`
/// is left out. A number that does not read counts as 0, so the order is
/// total over whatever a catalog lists, checked or not.
#[must_use]
pub fn compare_versions(a: &str, b: &str) -> Ordering {
    let (a, a_build) = split_build(a);
    let (b, b_build) = split_build(b);
    compare_releases(a, b).then(a_build.cmp(&b_build))
}

fn compare_releases(a: &str, b: &str) -> Ordering {
    fn split(version: &str) -> (Vec<u64>, Option<&str>) {
        let version = version.split('+').next().unwrap_or_default();
        let (numbers, pre) = match version.split_once('-') {
            Some((numbers, pre)) => (numbers, Some(pre)),
            None => (version, None),
        };
        (numbers.split('.').map(|n| n.parse().unwrap_or(0)).collect(), pre)
    }
    let (an, ap) = split(a);
    let (bn, bp) = split(b);
    for i in 0..an.len().max(bn.len()) {
        let order = an.get(i).copied().unwrap_or(0).cmp(&bn.get(i).copied().unwrap_or(0));
        if order.is_ne() {
            return order;
        }
    }
    match (ap, bp) {
        (None, None) => Ordering::Equal,
        (None, Some(_)) => Ordering::Greater,
        (Some(_), None) => Ordering::Less,
        (Some(ap), Some(bp)) => compare_pre_releases(ap, bp),
    }
}

fn compare_pre_releases(a: &str, b: &str) -> Ordering {
    let mut ai = a.split('.');
    let mut bi = b.split('.');
    loop {
        let order = match (ai.next(), bi.next()) {
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(x), Some(y)) => match (x.parse::<u64>(), y.parse::<u64>()) {
                (Ok(x), Ok(y)) => x.cmp(&y),
                (Ok(_), Err(_)) => Ordering::Less,
                (Err(_), Ok(_)) => Ordering::Greater,
                (Err(_), Err(_)) => x.cmp(y),
            },
        };
        if order.is_ne() {
            return order;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_order_as_semantic_versioning_orders_them() {
        let ordered = [
            "0.9",
            "1.0.0-alpha",
            "1.0.0-alpha.1",
            "1.0.0-alpha.beta",
            "1.0.0-beta",
            "1.0.0-beta.2",
            "1.0.0-beta.11",
            "1.0.0-rc.1",
            "1.0.0",
            "1.0.1",
            "1.2",
            "1.10",
            "2",
        ];
        for pair in ordered.windows(2) {
            assert_eq!(compare_versions(pair[0], pair[1]), Ordering::Less, "{pair:?}");
            assert_eq!(compare_versions(pair[1], pair[0]), Ordering::Greater, "{pair:?}");
        }
        assert_eq!(compare_versions("1.0", "1.0.0"), Ordering::Equal);
        assert_eq!(compare_versions("1.0.0+build.5", "1.0.0"), Ordering::Equal);
    }

    #[test]
    fn a_rebuild_comes_after_its_version_and_before_the_next() {
        let cases = [
            ("15.2.0", "15.2.0_2", Ordering::Less),
            ("15.2.0_2", "15.2.0_10", Ordering::Less),
            ("15.2.0_10", "15.2.1-rc.1", Ordering::Less),
            ("19.0-rc.2_2", "19.0", Ordering::Less),
            ("1.0_2", "1.0.0_2", Ordering::Equal),
            ("1.0.0+a_2", "1.0.0+b_2", Ordering::Equal),
        ];
        for (a, b, order) in cases {
            assert_eq!(compare_versions(a, b), order, "{a} against {b}");
            assert_eq!(compare_versions(b, a), order.reverse(), "{b} against {a}");
        }
    }

    #[test]
    fn the_order_is_total_over_builds_and_releases() {
        let ordered = [
            "1.0.0-rc.1",
            "1.0.0-rc.1_2",
            "1.0.0",
            "1.0.0_2",
            "1.0.0_9",
            "1.0.0_10",
            "1.0.0_9999",
            "1.0.1",
            "1.0.1_2",
            "1.1",
            "1.1_3",
            "2",
        ];
        let mut shuffled = ordered.to_vec();
        shuffled.reverse();
        shuffled.swap(0, 7);
        shuffled.swap(3, 10);
        shuffled.sort_by(|a, b| compare_versions(a, b));
        assert_eq!(shuffled, ordered);
        for a in ordered {
            for b in ordered {
                assert_eq!(compare_versions(a, b), compare_versions(b, a).reverse(), "{a}, {b}");
                for c in ordered {
                    if compare_versions(a, b).is_lt() && compare_versions(b, c).is_lt() {
                        assert!(compare_versions(a, c).is_lt(), "{a} < {b} < {c}");
                    }
                }
            }
        }
    }

    #[test]
    fn only_a_build_number_is_split_off() {
        assert_eq!(split_build("15.2.0_2"), ("15.2.0", 2));
        assert_eq!(split_build("15.2.0_9999"), ("15.2.0", 9999));
        assert_eq!(split_build("19.0-rc.2_2"), ("19.0-rc.2", 2));
        assert_eq!(split_build("15.2.0"), ("15.2.0", 1));
        for whole in ["15.2.0_1", "15.2.0_0", "15.2.0_01", "15.2.0_10000", "15.2.0_", "15.2.0_x"] {
            assert_eq!(split_build(whole), (whole, 1), "{whole}");
        }
        assert_eq!(split_build("_2"), ("_2", 1));
        assert_eq!(split_build("15.2.0__2"), ("15.2.0_", 2));
        assert_eq!(split_build("15.2.0_+2"), ("15.2.0_+2", 1));
    }

    #[test]
    fn a_build_is_written_after_its_version_and_read_back() {
        assert_eq!(with_build("15.2.0", 1), "15.2.0");
        assert_eq!(with_build("15.2.0", 2), "15.2.0_2");
        for version in ["15.2.0", "15.2.0_2", "19.0-rc.2_2", "1.0.0+meta_3", "3.13.1_9999"] {
            let (release, build) = split_build(version);
            assert_eq!(with_build(release, build), version);
        }
    }
}
