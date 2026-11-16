// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! `database copies`, and the copy that `list` shows of each database:
//! the file `NAME.dump` that PostgreSQL's backup hook writes before each
//! backup, in the folder in use and in each snapshot of `/shared`.
//!
//! A snapshot holds the copy as it was when the snapshot was made, so the
//! copies of a database are the ones a person can choose from to put it
//! back as it was at one time or another. A copy that several snapshots
//! hold is one copy, and is listed once.

use std::cmp::Reverse;
use std::fs;
use std::path::Path;
use std::time::UNIX_EPOCH;

use serde::Serialize;

use super::server::Host;
use crate::util;

/// What `from` says of the folder in use.
pub const LIVE: &str = "live";

/// A copy, as `list` says it: when it was written and how long it is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct Copy {
    /// When it was written, in milliseconds since 1970.
    pub at_ms: u64,
    /// Its size in bytes.
    pub bytes: u64,
}

/// A copy and where it is.
#[derive(Debug, PartialEq, Eq, Serialize)]
pub struct Found {
    /// When it was written, in milliseconds since 1970.
    pub at_ms: u64,
    /// Its size in bytes.
    pub bytes: u64,
    /// `live` for the folder in use, or the name of the snapshot of
    /// `/shared` that holds it.
    pub from: String,
    /// Its path, which `restore` takes.
    pub path: String,
}

/// What `copies` says: the document `--json` prints after its `format`.
#[derive(Debug, Serialize)]
pub struct Copies {
    /// The database.
    pub database: String,
    /// Its copies, the newest first.
    pub copies: Vec<Found>,
}

/// The copy of `name` in `folder`: a regular file called `NAME.dump`, which
/// is never followed from a link. None when there is none, or the
/// filesystem does not say when it was written.
#[must_use]
pub fn in_folder(folder: &Path, name: &str) -> Option<Copy> {
    let meta = fs::symlink_metadata(folder.join(format!("{name}.dump"))).ok()?;
    if !meta.is_file() {
        return None;
    }
    let since = meta.modified().ok()?.duration_since(UNIX_EPOCH).ok()?;
    Some(Copy { at_ms: u64::try_from(since.as_millis()).ok()?, bytes: meta.len() })
}

/// The copies of `name`, the newest first: the one in use, and each that a
/// snapshot of `/shared` holds that is not the same as another listed.
pub fn of(host: &mut dyn Host, name: &str) -> Copies {
    let file = format!("{name}.dump");
    let mut found = Vec::new();
    let live = host.dumps();
    if let Some(copy) = in_folder(&live, name) {
        found.push(Found {
            at_ms: copy.at_ms,
            bytes: copy.bytes,
            from: LIVE.into(),
            path: live.join(&file).to_string_lossy().into_owned(),
        });
    }
    for folder in host.snapshot_dumps() {
        if let Some(copy) = in_folder(&folder.path, name) {
            found.push(Found {
                at_ms: copy.at_ms,
                bytes: copy.bytes,
                from: folder.from,
                path: folder.path.join(&file).to_string_lossy().into_owned(),
            });
        }
    }
    // The folders come in use first and then the snapshots newest first,
    // and a sort that keeps the order of equals leaves them so.
    found.sort_by_key(|one| Reverse(one.at_ms));
    let mut seen: Vec<(u64, u64)> = Vec::new();
    found.retain(|one| {
        let key = (one.at_ms, one.bytes);
        let new = !seen.contains(&key);
        seen.push(key);
        new
    });
    Copies { database: name.to_string(), copies: found }
}

/// `copies` in words, for a person at a terminal: one line for each copy.
#[must_use]
pub fn text(copies: &Copies) -> String {
    if copies.copies.is_empty() {
        return format!("there is no copy of {}\n", copies.database);
    }
    let mut out = String::new();
    for one in &copies.copies {
        let seconds = i64::try_from(one.at_ms / 1000).unwrap_or(i64::MAX);
        out.push_str(&format!(
            "{}  {}  {}  {}\n",
            util::local_time(seconds, "%Y-%m-%d %H:%M"),
            util::human(one.bytes, false),
            one.from,
            one.path
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::super::fake::Fake;
    use super::*;
    use crate::json;

    const DAY: u64 = 86_400_000;
    const NOW: u64 = super::super::fake::START_MS;

    #[test]
    fn a_copy_is_the_size_and_time_of_its_file() {
        let fake = Fake::new().with_copy("shop", 12_345, NOW);
        let copy = in_folder(&fake.copies_folder(), "shop").unwrap();
        assert_eq!(copy, Copy { at_ms: NOW, bytes: 12_345 });
        assert_eq!(in_folder(&fake.copies_folder(), "blog"), None);
        assert_eq!(in_folder(&fake.copies_folder().join("none"), "shop"), None);
    }

    #[test]
    fn a_link_and_a_folder_are_no_copy() {
        let fake = Fake::new().with_copy("shop", 100, NOW);
        let folder = fake.copies_folder();
        std::os::unix::fs::symlink(folder.join("shop.dump"), folder.join("link.dump")).unwrap();
        std::fs::create_dir(folder.join("dir.dump")).unwrap();
        assert_eq!(in_folder(&folder, "link"), None);
        assert_eq!(in_folder(&folder, "dir"), None);
        assert!(in_folder(&folder, "shop").is_some());
    }

    #[test]
    fn the_copies_are_the_one_in_use_and_those_of_the_snapshots_newest_first() {
        let mut fake = Fake::new()
            .with_copy("shop", 3000, NOW)
            .with_copy("blog", 10, NOW)
            .with_snapshot_copy("2026-11-14-0200", "shop", 1000, NOW - 2 * DAY)
            .with_snapshot_copy("2026-11-15-0200", "shop", 2000, NOW - DAY)
            .with_snapshot_copy("2026-11-15-0200", "blog", 20, NOW - DAY);
        let copies = of(&mut fake, "shop");
        assert_eq!(copies.database, "shop");
        let at: Vec<(u64, &str)> =
            copies.copies.iter().map(|one| (one.bytes, one.from.as_str())).collect();
        assert_eq!(at, [(3000, "live"), (2000, "2026-11-15-0200"), (1000, "2026-11-14-0200")]);
        assert_eq!(copies.copies[0].at_ms, NOW);
        assert_eq!(
            copies.copies[1].path,
            fake.snapshot_folder("2026-11-15-0200").join("shop.dump").to_string_lossy()
        );
        assert!(copies.copies[0].path.ends_with("/dumps/shop.dump"));
        assert_eq!(of(&mut fake, "blog").copies.len(), 2);
        assert!(of(&mut fake, "none").copies.is_empty());
    }

    #[test]
    fn a_copy_that_several_snapshots_hold_is_listed_once_where_it_is_newest() {
        let mut fake = Fake::new()
            .with_copy("shop", 3000, NOW)
            .with_snapshot_copy("2026-11-15-0200", "shop", 2000, NOW - DAY)
            .with_snapshot_copy("2026-11-15-1200", "shop", 2000, NOW - DAY)
            // The snapshot of this morning holds the copy that is in use.
            .with_snapshot_copy("2026-11-16-0300", "shop", 3000, NOW);
        let copies = of(&mut fake, "shop");
        let from: Vec<&str> = copies.copies.iter().map(|one| one.from.as_str()).collect();
        assert_eq!(from, ["live", "2026-11-15-1200"]);
    }

    #[test]
    fn the_document_has_the_members_the_pages_read() {
        let mut fake = Fake::new().with_copy("shop", 12_345_678, NOW).with_snapshot_copy(
            "2026-11-15-0200",
            "shop",
            1_234,
            NOW - DAY,
        );
        let copies = of(&mut fake, "shop");
        let line = json::line(&copies).unwrap();
        let folder = fake.copies_folder().to_string_lossy().into_owned();
        let snapshot = fake.snapshot_folder("2026-11-15-0200").to_string_lossy().into_owned();
        let golden = include_str!("../../tests/json/database-copies.json")
            .replace("LIVE", &folder)
            .replace("SNAPSHOT", &snapshot);
        json::assert_golden(&line, &golden);
    }

    #[test]
    fn the_words_say_when_how_big_and_where() {
        let mut fake = Fake::new().with_copy("shop", 2048, NOW);
        let words = text(&of(&mut fake, "shop"));
        assert_eq!(words.lines().count(), 1);
        assert!(words.contains("  2.0K  live  "), "{words}");
        assert!(words.trim_end().ends_with("/dumps/shop.dump"), "{words}");
        assert_eq!(text(&of(&mut fake, "blog")), "there is no copy of blog\n");
    }
}
