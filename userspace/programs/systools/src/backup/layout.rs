// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The files of a destination (ADR 0463), read from a list of their
//! paths: under the machine's folder, a folder for each place, in it a
//! folder for each chain, named by the time of its whole send, and in it
//! a file for each backup, named by its time, `<time>.age`, or cut into
//! parts `<time>.age.000`, `<time>.age.001` and on. A file still being
//! written ends `.partial`.
//!
//! Which backup comes next, and which chains go, is decided here too, from
//! the list alone, so that the host tests check it.

use std::collections::BTreeMap;

/// The destination's own file, beside the places' folders.
pub const MARK: &str = "backup.toml";

/// The ending of an encrypted stream's file.
pub const ENDING: &str = ".age";

/// The ending of a file not yet whole.
pub const PARTIAL: &str = ".partial";

/// Whether `name` is a time as backups are named:
/// `YYYY-MM-DD-HHMMSS`.
pub fn is_time(name: &str) -> bool {
    let bytes = name.as_bytes();
    bytes.len() == 17
        && bytes.iter().enumerate().all(|(i, &b)| match i {
            4 | 7 | 10 => b == b'-',
            _ => b.is_ascii_digit(),
        })
}

/// The time name of a local date and time.
pub fn time_name(year: i64, month: u32, day: u32, hour: u32, minute: u32, second: u32) -> String {
    format!("{year:04}-{month:02}-{day:02}-{hour:02}{minute:02}{second:02}")
}

/// The name of part `index` of a backup made at `time` that is cut into
/// parts.
pub fn part_name(time: &str, index: usize) -> String {
    format!("{time}{ENDING}.{index:03}")
}

/// The name of a backup made at `time` in one file.
pub fn whole_name(time: &str) -> String {
    format!("{time}{ENDING}")
}

/// One backup: a stream, whole or of the changes from the backup before
/// it in its chain.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Backup {
    /// When its snapshot was made, as named.
    pub time: String,
    /// Its files' paths, from the machine's folder, in order.
    pub parts: Vec<String>,
}

/// A chain: a whole send, then the changes, each from the one before.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Chain {
    /// The time of its whole send, which names its folder.
    pub name: String,
    /// Its backups, oldest first.
    pub backups: Vec<Backup>,
}

impl Chain {
    /// Whether it starts with its whole send, so that its backups can be
    /// restored.
    pub fn whole(&self) -> bool {
        self.backups.first().is_some_and(|b| b.time == self.name)
    }

    /// Its newest backup.
    pub fn last(&self) -> Option<&Backup> {
        self.backups.last()
    }
}

/// What a destination holds.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Listing {
    /// Whether the destination's own file is there.
    pub marked: bool,
    /// Each place's chains, oldest first, by the place's folder name.
    pub places: BTreeMap<String, Vec<Chain>>,
    /// Files a backup left unfinished, which can go: those still
    /// `.partial`, and parts without their first.
    pub stray: Vec<String>,
}

/// A part of a backup found: its number (none for a backup in one
/// file) and its path.
type Part = (Option<usize>, String);

impl Listing {
    /// Reads the list of every file's path under the machine's folder,
    /// as `users/2026-11-11-030000/2026-11-11-030000.age`. A file that is
    /// not a backup's is passed over, and never removed.
    pub fn read<'a>(paths: impl IntoIterator<Item = &'a str>) -> Listing {
        let mut listing = Listing::default();
        // Place, chain and time, to the parts found by their numbers
        // (None for a backup in one file).
        let mut found: BTreeMap<(String, String, String), Vec<Part>> = BTreeMap::new();
        for path in paths {
            let path = path.strip_prefix("./").unwrap_or(path);
            if path == MARK {
                listing.marked = true;
                continue;
            }
            let names: Vec<&str> = path.split('/').collect();
            let [place, chain, file] = names[..] else { continue };
            if !is_place(place) || !is_time(chain) {
                continue;
            }
            if let Some(unfinished) = file.strip_suffix(PARTIAL) {
                if backup_file(unfinished).is_some() {
                    listing.stray.push(path.to_string());
                }
                continue;
            }
            let Some((time, part)) = backup_file(file) else { continue };
            found
                .entry((place.to_string(), chain.to_string(), time.to_string()))
                .or_default()
                .push((part, path.to_string()));
        }
        for ((place, chain, time), mut parts) in found {
            parts.sort();
            // One file, or parts numbered from 0 without a gap; the rest
            // are what a backup cut short left.
            let complete = match parts.as_slice() {
                [(None, _)] => true,
                _ => parts.iter().enumerate().all(|(i, (part, _))| *part == Some(i)),
            };
            if !complete {
                listing.stray.extend(parts.into_iter().map(|(_, path)| path));
                continue;
            }
            let chains = listing.places.entry(place).or_default();
            if chains.last().is_none_or(|c| c.name != chain) {
                chains.push(Chain { name: chain, backups: Vec::new() });
            }
            let parts = parts.into_iter().map(|(_, path)| path).collect();
            if let Some(last) = chains.last_mut() {
                last.backups.push(Backup { time, parts });
            }
        }
        listing
    }

    /// The chains of the place whose folder is `place`, oldest first.
    pub fn chains(&self, place: &str) -> &[Chain] {
        self.places.get(place).map_or(&[], Vec::as_slice)
    }
}

/// Whether `name` can be a place's folder: a name of letters, digits,
/// `-`, `_` and `.`, not starting with a dot.
pub fn is_place(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('.')
        && name.bytes().all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
}

/// The time of the backup whose file is named `name`, and its part's
/// number if it is a part.
fn backup_file(name: &str) -> Option<(&str, Option<usize>)> {
    let (time, rest) = name.split_at_checked(17)?;
    if !is_time(time) {
        return None;
    }
    let rest = rest.strip_prefix(ENDING)?;
    if rest.is_empty() {
        return Some((time, None));
    }
    let number = rest.strip_prefix('.')?;
    if number.len() < 3 || !number.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    Some((time, Some(number.parse().ok()?)))
}

/// What the next backup of a place is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Next {
    /// A whole send, which starts a new chain.
    Whole,
    /// The changes from the newest backup, added to its chain.
    Changes {
        /// The chain's name.
        chain: String,
        /// The time of the backup the changes are from, which names the
        /// local snapshot sent before.
        from: String,
    },
}

/// The next backup of a place whose chains are `chains`: the changes
/// from the newest backup, if its chain is whole and holds fewer than
/// `length` backups, and the destination last took the snapshot of time
/// `sent`, which is still here (`have`); otherwise a whole send.
pub fn next(chains: &[Chain], sent: Option<&str>, have: bool, length: usize) -> Next {
    let Some(chain) = chains.last().filter(|c| c.whole() && c.backups.len() < length) else {
        return Next::Whole;
    };
    match (chain.last(), sent) {
        (Some(last), Some(sent)) if last.time == sent && have => {
            Next::Changes { chain: chain.name.clone(), from: last.time.clone() }
        }
        _ => Next::Whole,
    }
}

/// The chains of `chains`, oldest first, that go once the newest `keep`
/// whole ones stay (at least one): the older ones, and every one without
/// its whole send, which nothing can restore.
pub fn old_chains(chains: &[Chain], keep: usize) -> Vec<&Chain> {
    let mut kept = keep.max(1);
    let mut going = Vec::new();
    for chain in chains.iter().rev() {
        if chain.whole() && kept > 0 {
            kept -= 1;
        } else {
            going.push(chain);
        }
    }
    going.reverse();
    going
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: &str = "2026-11-10-030000";
    const B: &str = "2026-11-11-030000";
    const C: &str = "2026-11-12-030000";
    const D: &str = "2026-11-13-030000";

    fn listing(paths: &[String]) -> Listing {
        Listing::read(paths.iter().map(String::as_str))
    }

    #[test]
    fn times_name_backups() {
        assert!(is_time(A));
        assert_eq!(time_name(2026, 11, 10, 3, 0, 0), A);
        assert!(!is_time("2026-11-10-0300"));
        assert!(!is_time("2026-11-10T030000"));
        assert_eq!(backup_file(&whole_name(A)), Some((A, None)));
        assert_eq!(backup_file(&part_name(A, 12)), Some((A, Some(12))));
        assert_eq!(backup_file(&part_name(A, 1000)), Some((A, Some(1000))));
        assert_eq!(backup_file(&format!("{A}.age.1")), None);
        assert_eq!(backup_file(&format!("{A}.tar")), None);
    }

    #[test]
    fn a_listing_reads_chains_and_their_backups() {
        let paths = vec![
            "./backup.toml".to_string(),
            format!("users/{A}/{}", whole_name(A)),
            format!("users/{A}/{}", whole_name(B)),
            format!("users/{C}/{}", part_name(C, 1)),
            format!("users/{C}/{}", part_name(C, 0)),
            format!("users/{C}/{}", whole_name(D)),
            format!("settings/{A}/{}", whole_name(A)),
            "users/notes.txt".to_string(),
            format!("users/{A}/readme"),
        ];
        let found = listing(&paths);
        assert!(found.marked);
        assert!(found.stray.is_empty());
        let users = found.chains("users");
        assert_eq!(users.len(), 2);
        assert_eq!(users[0].name, A);
        assert_eq!(users[0].backups.iter().map(|b| b.time.as_str()).collect::<Vec<_>>(), [A, B]);
        assert_eq!(
            users[1].backups[0].parts,
            [format!("users/{C}/{}", part_name(C, 0)), format!("users/{C}/{}", part_name(C, 1))]
        );
        assert!(users.iter().all(Chain::whole));
        assert_eq!(found.chains("settings").len(), 1);
        assert!(found.chains("shared").is_empty());
    }

    #[test]
    fn what_a_cut_off_backup_left_is_stray() {
        let paths = vec![
            format!("users/{A}/{}", whole_name(A)),
            format!("users/{A}/{}{PARTIAL}", whole_name(B)),
            // Parts renamed from the last: the first is still partial.
            format!("users/{A}/{}{PARTIAL}", part_name(C, 0)),
            format!("users/{A}/{}", part_name(C, 1)),
            format!("users/{A}/{}", part_name(D, 0)),
            format!("users/{A}/{}", part_name(D, 2)),
        ];
        let found = listing(&paths);
        assert_eq!(found.chains("users")[0].backups.len(), 1);
        assert_eq!(found.stray.len(), 5);
    }

    #[test]
    fn a_chain_without_its_whole_send_is_not_whole() {
        let found = listing(&[format!("users/{A}/{}", whole_name(B))]);
        assert!(!found.chains("users")[0].whole());
        assert_eq!(next(found.chains("users"), Some(B), true, 30), Next::Whole);
    }

    #[test]
    fn changes_follow_the_snapshot_the_destination_took() {
        let found = listing(&[
            format!("users/{A}/{}", whole_name(A)),
            format!("users/{A}/{}", whole_name(B)),
        ]);
        let chains = found.chains("users");
        let changes = Next::Changes { chain: A.into(), from: B.into() };
        assert_eq!(next(chains, Some(B), true, 30), changes);
        // The local snapshot gone, another one last taken, none taken, or
        // the chain full: a whole send.
        assert_eq!(next(chains, Some(B), false, 30), Next::Whole);
        assert_eq!(next(chains, Some(A), true, 30), Next::Whole);
        assert_eq!(next(chains, None, true, 30), Next::Whole);
        assert_eq!(next(chains, Some(B), true, 2), Next::Whole);
        assert_eq!(next(&[], Some(B), true, 30), Next::Whole);
    }

    #[test]
    fn old_chains_go_once_newer_whole_ones_are_kept() {
        let paths = vec![
            format!("users/{A}/{}", whole_name(A)),
            format!("users/{B}/{}", whole_name(B)),
            format!("users/{C}/{}", whole_name(D)),
            format!("users/{D}/{}", whole_name(D)),
        ];
        let found = listing(&paths);
        let chains = found.chains("users");
        let names =
            |keep| old_chains(chains, keep).iter().map(|c| c.name.clone()).collect::<Vec<_>>();
        // C has no whole send: nothing restores it, so it goes too.
        assert_eq!(names(2), [A, C].map(String::from));
        assert_eq!(names(1), [A, B, C].map(String::from));
        assert_eq!(names(3), [C.to_string()]);
        assert_eq!(names(0), names(1));
    }
}
