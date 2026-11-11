// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The files of a destination (ADR 0463), read from a list of their
//! paths: under the machine's folder, a folder for each place, in it a
//! folder for each chain, named by the time of its whole send, and in it
//! a file for each backup, named by its time, `<time>.age`, or cut into
//! parts `<time>.age.000`, `<time>.age.001` and on, with its tag,
//! `<time>.tag` (ADR 0464). The machine's records are in the folder
//! `machine`, one a round, `<time>.age` with its tag. A file still being
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

/// The ending of a tag's file (ADR 0464).
pub const TAG: &str = super::tag::ENDING;

/// The folder of the machine's records, beside the places' (ADR 0464).
pub const RECORDS: &str = "machine";

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

/// The name of the tag of the backup or record made at `time`.
pub fn tag_name(time: &str) -> String {
    format!("{time}{TAG}")
}

/// The time of the tag whose file is named `name`.
fn tag_file(name: &str) -> Option<&str> {
    name.strip_suffix(TAG).filter(|time| is_time(time))
}

/// One backup: a stream, whole or of the changes from the backup before
/// it in its chain.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Backup {
    /// When its snapshot was made, as named.
    pub time: String,
    /// Its files' paths, from the machine's folder, in order.
    pub parts: Vec<String>,
    /// Its tag's path, if it has one.
    pub tag: Option<String>,
}

/// The path a tag is made over, of the backup or record whose first file
/// is `path`: its path from the machine's folder without its ending, as
/// `users/CHAIN/TIME`.
pub fn tagged_path(path: &str) -> String {
    let (folder, file) = path.rsplit_once('/').unwrap_or(("", path));
    let time = file.get(..17).unwrap_or(file);
    if folder.is_empty() { time.to_string() } else { format!("{folder}/{time}") }
}

/// One of the machine's records (ADR 0464).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Record {
    /// The time of the round that wrote it.
    pub time: String,
    /// Its file's path, from the machine's folder.
    pub path: String,
    /// Its tag's path, if it has one.
    pub tag: Option<String>,
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
    /// The machine's records, oldest first.
    pub records: Vec<Record>,
    /// Files a backup left unfinished, which can go: those still
    /// `.partial`, parts without their first, and tags without their
    /// file.
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
        // The tags, by the place, chain and time of what they tag.
        let mut tags: BTreeMap<(String, String, String), String> = BTreeMap::new();
        // The records and their tags, by time.
        let mut records: BTreeMap<String, String> = BTreeMap::new();
        let mut record_tags: BTreeMap<String, String> = BTreeMap::new();
        for path in paths {
            let path = path.strip_prefix("./").unwrap_or(path);
            if path == MARK {
                listing.marked = true;
                continue;
            }
            let names: Vec<&str> = path.split('/').collect();
            if let [RECORDS, file] = names[..] {
                if let Some(unfinished) = file.strip_suffix(PARTIAL) {
                    if backup_file(unfinished).is_some() || tag_file(unfinished).is_some() {
                        listing.stray.push(path.to_string());
                    }
                } else if let Some((time, None)) = backup_file(file) {
                    records.insert(time.to_string(), path.to_string());
                } else if let Some(time) = tag_file(file) {
                    record_tags.insert(time.to_string(), path.to_string());
                }
                continue;
            }
            let [place, chain, file] = names[..] else { continue };
            if !is_place(place) || place == RECORDS || !is_time(chain) {
                continue;
            }
            if let Some(unfinished) = file.strip_suffix(PARTIAL) {
                if backup_file(unfinished).is_some() || tag_file(unfinished).is_some() {
                    listing.stray.push(path.to_string());
                }
                continue;
            }
            if let Some(time) = tag_file(file) {
                tags.insert((place.into(), chain.into(), time.into()), path.to_string());
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
            let key = (place, chain, time);
            if !complete {
                listing.stray.extend(parts.into_iter().map(|(_, path)| path));
                continue;
            }
            let tag = tags.remove(&key);
            let (place, chain, time) = key;
            let chains = listing.places.entry(place).or_default();
            if chains.last().is_none_or(|c| c.name != chain) {
                chains.push(Chain { name: chain, backups: Vec::new() });
            }
            let parts = parts.into_iter().map(|(_, path)| path).collect();
            if let Some(last) = chains.last_mut() {
                last.backups.push(Backup { time, parts, tag });
            }
        }
        // A tag is written before its backup is named, so one without its
        // backup is what a backup cut short left.
        listing.stray.extend(tags.into_values());
        for (time, path) in records {
            let tag = record_tags.remove(&time);
            listing.records.push(Record { time, path, tag });
        }
        listing.stray.extend(record_tags.into_values());
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

/// The records of `listing` that go once the chains `going` have gone:
/// those from before the oldest backup a kept chain restores, except
/// the newest of them, which a restore of that backup may need.
pub fn old_records<'l>(listing: &'l Listing, going: &[&Chain]) -> Vec<&'l Record> {
    let oldest = listing
        .places
        .values()
        .flatten()
        .filter(|chain| chain.whole() && !going.iter().any(|g| std::ptr::eq(*g, *chain)))
        .map(|chain| chain.name.as_str())
        .min();
    let Some(oldest) = oldest else { return Vec::new() };
    let before: Vec<&Record> =
        listing.records.iter().filter(|r| r.time.as_str() < oldest).collect();
    before.split_last().map_or_else(Vec::new, |(_, older)| older.to_vec())
}

/// The newest record of `listing` from no later than `at`, or the newest.
pub fn record_at<'l>(listing: &'l Listing, at: Option<&str>) -> Option<&'l Record> {
    listing.records.iter().rev().find(|r| at.is_none_or(|at| r.time.as_str() <= at))
}

/// The backups of the place `place` up to its newest from no later than
/// `at` (the newest of all without it): its chain's from the whole send.
pub fn chain_upto<'l>(listing: &'l Listing, place: &str, at: Option<&str>) -> Option<&'l [Backup]> {
    let mut best: Option<&[Backup]> = None;
    for chain in listing.chains(place).iter().filter(|c| c.whole()) {
        let end =
            chain.backups.iter().filter(|b| at.is_none_or(|at| b.time.as_str() <= at)).count();
        let candidate = &chain.backups[..end];
        let newer = match (candidate.last(), best.and_then(<[Backup]>::last)) {
            (Some(c), Some(b)) => c.time > b.time,
            (Some(_), None) => true,
            (None, _) => false,
        };
        if newer {
            best = Some(candidate);
        }
    }
    best
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
    fn tags_go_with_their_backups_and_records_are_read_beside_the_places() {
        let paths = vec![
            format!("users/{A}/{}", whole_name(A)),
            format!("users/{A}/{}", tag_name(A)),
            format!("users/{A}/{}", part_name(B, 0)),
            format!("users/{A}/{}", part_name(B, 1)),
            format!("users/{A}/{}", tag_name(B)),
            format!("users/{A}/{}", whole_name(C)),
            // A tag whose backup was cut short, and one being written.
            format!("users/{A}/{}", tag_name(D)),
            format!("users/{A}/{}{PARTIAL}", tag_name(D)),
            format!("{RECORDS}/{}", whole_name(A)),
            format!("{RECORDS}/{}", tag_name(A)),
            format!("{RECORDS}/{}", whole_name(B)),
            format!("{RECORDS}/{}", tag_name(C)),
            format!("{RECORDS}/{}{PARTIAL}", whole_name(D)),
            format!("{RECORDS}/notes.txt"),
            // Not a place's chain: a record's folder holds no chains.
            format!("{RECORDS}/{A}/{}", whole_name(A)),
        ];
        let found = listing(&paths);
        let users = &found.chains("users")[0].backups;
        assert_eq!(users[0].tag, Some(format!("users/{A}/{}", tag_name(A))));
        assert_eq!(tagged_path(&users[0].parts[0]), format!("users/{A}/{A}"));
        assert_eq!(users[1].tag, Some(format!("users/{A}/{}", tag_name(B))));
        assert_eq!(tagged_path(&users[1].parts[0]), format!("users/{A}/{B}"));
        assert_eq!(users[2].tag, None);
        assert!(found.chains(RECORDS).is_empty());
        assert_eq!(
            found.records,
            [
                Record {
                    time: A.into(),
                    path: format!("{RECORDS}/{}", whole_name(A)),
                    tag: Some(format!("{RECORDS}/{}", tag_name(A))),
                },
                Record { time: B.into(), path: format!("{RECORDS}/{}", whole_name(B)), tag: None },
            ]
        );
        assert_eq!(tagged_path(&found.records[0].path), format!("{RECORDS}/{A}"));
        let mut stray = found.stray.clone();
        stray.sort();
        let mut expected = vec![
            format!("users/{A}/{}", tag_name(D)),
            format!("users/{A}/{}{PARTIAL}", tag_name(D)),
            format!("{RECORDS}/{}", tag_name(C)),
            format!("{RECORDS}/{}{PARTIAL}", whole_name(D)),
        ];
        expected.sort();
        assert_eq!(stray, expected);
    }

    #[test]
    fn a_restore_finds_the_record_and_backups_of_its_time() {
        let paths = vec![
            format!("users/{A}/{}", whole_name(A)),
            format!("users/{A}/{}", whole_name(B)),
            format!("users/{C}/{}", whole_name(C)),
            format!("users/{C}/{}", whole_name(D)),
            // shared missed the rounds of B and D.
            format!("shared/{A}/{}", whole_name(A)),
            format!("shared/{A}/{}", whole_name(C)),
            format!("{RECORDS}/{}", whole_name(A)),
            format!("{RECORDS}/{}", whole_name(C)),
        ];
        let found = listing(&paths);
        let times = |backups: Option<&[Backup]>| {
            backups.unwrap().iter().map(|b| b.time.clone()).collect::<Vec<_>>()
        };
        assert_eq!(times(chain_upto(&found, "users", None)), [C, D]);
        assert_eq!(times(chain_upto(&found, "users", Some(B))), [A, B]);
        assert_eq!(times(chain_upto(&found, "users", Some(C))), [C]);
        assert_eq!(times(chain_upto(&found, "shared", Some(B))), [A]);
        assert_eq!(times(chain_upto(&found, "shared", Some(D))), [A, C]);
        assert_eq!(chain_upto(&found, "users", Some("2026-01-01-000000")), None);
        assert_eq!(chain_upto(&found, "settings", None), None);
        assert_eq!(record_at(&found, None).map(|r| r.time.as_str()), Some(C));
        assert_eq!(record_at(&found, Some(B)).map(|r| r.time.as_str()), Some(A));
        assert_eq!(record_at(&found, Some(D)).map(|r| r.time.as_str()), Some(C));
        assert_eq!(record_at(&found, Some("2026-01-01-000000")), None);
    }

    #[test]
    fn records_go_with_the_chains_that_needed_them() {
        let paths = vec![
            format!("users/{A}/{}", whole_name(A)),
            format!("users/{C}/{}", whole_name(C)),
            format!("users/{C}/{}", whole_name(D)),
            format!("{RECORDS}/{}", whole_name(A)),
            format!("{RECORDS}/{}", whole_name(B)),
            format!("{RECORDS}/{}", whole_name(C)),
            format!("{RECORDS}/{}", whole_name(D)),
        ];
        let found = listing(&paths);
        let chains = found.chains("users");
        let times =
            |records: Vec<&Record>| records.iter().map(|r| r.time.clone()).collect::<Vec<_>>();
        // Both chains kept: every record stays.
        assert!(old_records(&found, &[]).is_empty());
        // A's chain goes: A's record goes; B's stays, the newest before C.
        let going = old_chains(chains, 1);
        assert_eq!(times(old_records(&found, &going)), [A]);
        // Without a backup, nothing goes.
        assert!(old_records(&listing(&[format!("{RECORDS}/{}", whole_name(A))]), &[]).is_empty());
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
