// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! `snap browse`: a folder as a snapshot has it, each entry compared with
//! the same path now, so that a person can see what a restore would bring
//! back.
//!
//! The folders come first, then the rest, each by name; at most
//! [`ENTRIES_MOST`] are given, and the answer says when there are more. A
//! folder of very many entries is read only as far as [`READ_MOST`] names,
//! so that a browse of one does not take the memory it would take to sort
//! them all; its answer says there are more as well.

use std::cmp::Reverse;
use std::ffi::OsString;
use std::fs::{self, FileType, Metadata};
use std::io;
use std::os::unix::fs::MetadataExt;
use std::path::Path;

use serde_json::{Value, json};

use crate::util::{self, Fail};

/// The most entries a browse gives.
pub const ENTRIES_MOST: usize = 2000;

/// The most names of one folder a browse reads to choose them from.
const READ_MOST: usize = 100_000;

/// What an entry is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    File,
    Folder,
    Link,
    /// A device, a pipe or a socket.
    Other,
}

impl Kind {
    fn of(kind: FileType) -> Kind {
        if kind.is_dir() {
            Kind::Folder
        } else if kind.is_symlink() {
            Kind::Link
        } else if kind.is_file() {
            Kind::File
        } else {
            Kind::Other
        }
    }

    fn word(self) -> &'static str {
        match self {
            Kind::File => "file",
            Kind::Folder => "folder",
            Kind::Link => "link",
            Kind::Other => "other",
        }
    }
}

/// How an entry of the snapshot compares with the same path now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Then {
    /// It is not there now.
    Deleted,
    /// Its kind, size or modification time differs. A folder's size is
    /// not compared: a folder has none to speak of.
    Changed,
    Same,
}

impl Then {
    fn word(self) -> &'static str {
        match self {
            Then::Deleted => "deleted",
            Then::Changed => "changed",
            Then::Same => "same",
        }
    }
}

/// One entry of a folder in a snapshot.
#[derive(Debug, PartialEq, Eq)]
pub struct Entry {
    pub name: String,
    pub kind: Kind,
    /// In bytes; none for a folder.
    pub size: Option<u64>,
    /// When it was last changed, in milliseconds since 1970; none before
    /// 1970.
    pub modified_ms: Option<u64>,
    pub then: Then,
}

/// What a browse found.
#[derive(Debug, PartialEq, Eq)]
pub struct Browse {
    pub entries: Vec<Entry>,
    /// Whether the folder has entries that are not given.
    pub more: bool,
}

/// The modification time of `meta`, as seconds and nanoseconds.
fn modified(meta: &Metadata) -> (i64, i64) {
    (meta.mtime(), meta.mtime_nsec())
}

/// How the entry `was` of the snapshot compares with what `now` says of
/// the same path: its `lstat`.
fn compare(was: &Metadata, now: io::Result<Metadata>) -> Then {
    let now = match now {
        Ok(now) => now,
        Err(err)
            if matches!(err.kind(), io::ErrorKind::NotFound | io::ErrorKind::NotADirectory) =>
        {
            return Then::Deleted;
        }
        // Not to be looked at now, which is not what it was.
        Err(_) => return Then::Changed,
    };
    let kind = Kind::of(was.file_type());
    let same = kind == Kind::of(now.file_type())
        && modified(was) == modified(&now)
        && (kind == Kind::Folder || was.len() == now.len());
    if same { Then::Same } else { Then::Changed }
}

/// The entries of the folder `snapshot`, which is in a snapshot, each
/// compared with its path in the folder `now`, at most `most` of them.
///
/// # Errors
///
/// A folder that cannot be read, or an entry of it that cannot be looked
/// at, with the system's reason.
pub fn entries(snapshot: &Path, now: &Path, most: usize) -> io::Result<Browse> {
    let mut names: Vec<(bool, OsString)> = Vec::new();
    let mut more = false;
    for entry in fs::read_dir(snapshot)? {
        if names.len() == READ_MOST {
            more = true;
            break;
        }
        let entry = entry?;
        let folder = entry.file_type()?.is_dir();
        names.push((folder, entry.file_name()));
    }
    // Folders first: a folder sorts before anything that is not one.
    names.sort_by(|a, b| (Reverse(a.0), &a.1).cmp(&(Reverse(b.0), &b.1)));
    more |= names.len() > most;
    names.truncate(most);
    let mut found = Vec::with_capacity(names.len());
    for (_, name) in names {
        let was = fs::symlink_metadata(snapshot.join(&name))?;
        let kind = Kind::of(was.file_type());
        found.push(Entry {
            name: name.to_string_lossy().into_owned(),
            kind,
            size: (kind != Kind::Folder).then_some(was.len()),
            modified_ms: u64::try_from(was.mtime()).ok().map(|seconds| {
                seconds * 1000 + u64::try_from(was.mtime_nsec()).unwrap_or(0) / 1_000_000
            }),
            then: compare(&was, fs::symlink_metadata(now.join(&name))),
        });
    }
    Ok(Browse { entries: found, more })
}

/// `folder` as a path from the place `place`, both absolute and without
/// `.` or `..`: none when `folder` is not the place or within it.
pub fn within(place: &Path, folder: &Path) -> Option<std::path::PathBuf> {
    folder.strip_prefix(place).ok().map(Path::to_path_buf)
}

/// Checks that `inside`, a path from the snapshot's folder `root`, is
/// reached without a link: a link in a snapshot can lead out of it, and a
/// browse shows what the snapshot holds.
///
/// # Errors
///
/// A part that cannot be looked at, with the system's reason, or that is a
/// link, each with its path.
pub fn no_links(root: &Path, inside: &Path) -> Result<(), Fail> {
    let mut at = root.to_path_buf();
    for part in inside.components() {
        at.push(part);
        let meta = fs::symlink_metadata(&at).map_err(|e| util::failed(&at.display(), e))?;
        if meta.file_type().is_symlink() {
            return Err(util::fail(format!(
                "{}: a link, which a browse does not follow",
                at.display()
            )));
        }
    }
    Ok(())
}

/// `snap browse --json`.
pub fn json(place: &str, snapshot: &str, folder: &str, found: &Browse) -> Value {
    let entries: Vec<Value> = found
        .entries
        .iter()
        .map(|entry| {
            json!({
                "name": entry.name,
                "kind": entry.kind.word(),
                "size": entry.size,
                "modified_ms": entry.modified_ms,
                "then": entry.then.word(),
            })
        })
        .collect();
    json!({
        "place": place,
        "snapshot": snapshot,
        "folder": folder,
        "entries": entries,
        "more": found.more,
    })
}

/// `snap browse` for a person: a row for each entry, as `snap list` shows
/// its snapshots.
pub fn rows(found: &Browse) -> Vec<Vec<String>> {
    let mut rows = vec![["NAME", "SIZE", "MODIFIED", "NOW"].map(String::from).to_vec()];
    for entry in &found.entries {
        let name = if entry.kind == Kind::Folder {
            format!("{}/", entry.name)
        } else {
            entry.name.clone()
        };
        let size = entry.size.map_or("-".into(), |size| util::human(size, false));
        let modified = entry.modified_ms.map_or("-".into(), |ms| {
            util::local_time(i64::try_from(ms / 1000).unwrap_or(0), "%Y-%m-%d %H:%M")
        });
        rows.push(vec![name, size, modified, entry.then.word().into()]);
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;
    use std::time::{Duration, SystemTime};

    /// A snapshot's folder, and the folder it was taken of, as they are
    /// now.
    struct Pair {
        root: std::path::PathBuf,
    }

    impl Pair {
        fn new(name: &str) -> Pair {
            let root =
                std::env::temp_dir().join(format!("snap-browse-{name}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&root);
            fs::create_dir_all(root.join("then")).unwrap();
            fs::create_dir_all(root.join("now")).unwrap();
            Pair { root }
        }

        fn then(&self) -> std::path::PathBuf {
            self.root.join("then")
        }

        fn now(&self) -> std::path::PathBuf {
            self.root.join("now")
        }

        /// A file in both, as it was.
        fn both(&self, name: &str, text: &str) {
            for side in ["then", "now"] {
                fs::write(self.root.join(side).join(name), text).unwrap();
                set_time(&self.root.join(side).join(name), 1_700_000_000);
            }
        }
    }

    impl Drop for Pair {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    fn set_time(path: &Path, seconds: u64) {
        // Read only: a folder cannot be opened for writing, and the owner
        // may set a time through either.
        let file = fs::File::open(path).unwrap();
        file.set_modified(SystemTime::UNIX_EPOCH + Duration::from_secs(seconds)).unwrap();
    }

    fn names(found: &Browse) -> Vec<&str> {
        found.entries.iter().map(|e| e.name.as_str()).collect()
    }

    #[test]
    fn entries_are_folders_first_then_by_name() {
        let pair = Pair::new("order");
        for name in ["b.txt", "A.txt", "c", "a.txt"] {
            pair.both(name, "x");
        }
        fs::create_dir(pair.then().join("zeta")).unwrap();
        fs::create_dir(pair.then().join("alpha")).unwrap();
        fs::create_dir(pair.then().join("Mid")).unwrap();
        let found = entries(&pair.then(), &pair.now(), ENTRIES_MOST).unwrap();
        assert_eq!(names(&found), ["Mid", "alpha", "zeta", "A.txt", "a.txt", "b.txt", "c"]);
        assert!(!found.more);
        assert_eq!(found.entries[0].kind, Kind::Folder);
        assert_eq!(found.entries[0].size, None);
    }

    #[test]
    fn an_entry_is_compared_with_the_same_path_now() {
        let pair = Pair::new("then");
        pair.both("same.txt", "unchanged");
        pair.both("grown.txt", "short");
        fs::write(pair.now().join("grown.txt"), "a good deal longer").unwrap();
        set_time(&pair.now().join("grown.txt"), 1_700_000_000);
        pair.both("touched.txt", "same size");
        set_time(&pair.now().join("touched.txt"), 1_700_000_500);
        pair.both("gone.txt", "removed since");
        fs::remove_file(pair.now().join("gone.txt")).unwrap();
        // A file that is a folder now, and a folder that is a file.
        pair.both("was-a-file", "x");
        fs::remove_file(pair.now().join("was-a-file")).unwrap();
        fs::create_dir(pair.now().join("was-a-file")).unwrap();
        fs::create_dir(pair.then().join("was-a-folder")).unwrap();
        fs::write(pair.now().join("was-a-folder"), "x").unwrap();
        // A folder that is as it was.
        fs::create_dir(pair.then().join("same-folder")).unwrap();
        fs::create_dir(pair.now().join("same-folder")).unwrap();
        set_time(&pair.then().join("same-folder"), 1_700_000_000);
        set_time(&pair.now().join("same-folder"), 1_700_000_000);
        let found = entries(&pair.then(), &pair.now(), ENTRIES_MOST).unwrap();
        let then: Vec<(&str, Then)> =
            found.entries.iter().map(|e| (e.name.as_str(), e.then)).collect();
        assert_eq!(
            then,
            [
                ("same-folder", Then::Same),
                ("was-a-folder", Then::Changed),
                ("gone.txt", Then::Deleted),
                ("grown.txt", Then::Changed),
                ("same.txt", Then::Same),
                ("touched.txt", Then::Changed),
                ("was-a-file", Then::Changed),
            ]
        );
    }

    #[test]
    fn a_folder_that_is_not_there_now_deletes_what_was_in_it() {
        let pair = Pair::new("deleted");
        pair.both("keep", "x");
        let found = entries(&pair.then(), &pair.root.join("not-there-now"), ENTRIES_MOST).unwrap();
        assert_eq!(found.entries[0].then, Then::Deleted);
        // Under a file, which is not a folder, there is nothing either.
        let found = entries(&pair.then(), &pair.now().join("keep"), ENTRIES_MOST).unwrap();
        assert_eq!(found.entries[0].then, Then::Deleted);
    }

    #[test]
    fn kinds_and_sizes_and_times_are_the_snapshots() {
        let pair = Pair::new("kinds");
        pair.both("photo.heic", "0123456789");
        symlink("photo.heic", pair.then().join("link")).unwrap();
        sysapi::fs::make_fifo(&pair.then().join("pipe"), 0o600).unwrap();
        let found = entries(&pair.then(), &pair.now(), ENTRIES_MOST).unwrap();
        let by = |name: &str| found.entries.iter().find(|e| e.name == name).unwrap();
        assert_eq!(by("photo.heic").kind, Kind::File);
        assert_eq!(
            (by("photo.heic").size, by("photo.heic").modified_ms),
            (Some(10), Some(1_700_000_000_000))
        );
        // A link is the link, not what it points at: its size is its target's length.
        assert_eq!((by("link").kind, by("link").size), (Kind::Link, Some(10)));
        assert_eq!(by("link").then, Then::Deleted);
        assert_eq!(by("pipe").kind, Kind::Other);
    }

    #[test]
    fn no_more_than_the_most_are_given_and_it_says_there_are_others() {
        let pair = Pair::new("most");
        for i in 0..7 {
            pair.both(&format!("f{i}"), "x");
        }
        fs::create_dir(pair.then().join("zz-folder")).unwrap();
        let found = entries(&pair.then(), &pair.now(), 5).unwrap();
        // The folder first, so that it is not what is cut.
        assert_eq!(names(&found), ["zz-folder", "f0", "f1", "f2", "f3"]);
        assert!(found.more);
        let found = entries(&pair.then(), &pair.now(), 8).unwrap();
        assert_eq!((found.entries.len(), found.more), (8, false));
        let found = entries(&pair.then(), &pair.now(), 7).unwrap();
        assert_eq!((found.entries.len(), found.more), (7, true));
    }

    #[test]
    fn a_folder_that_cannot_be_read_is_the_systems_error() {
        let pair = Pair::new("missing");
        let err = entries(&pair.root.join("none"), &pair.now(), ENTRIES_MOST).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::NotFound);
        let err = entries(&pair.now().join("nothing"), &pair.now(), ENTRIES_MOST).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::NotFound);
    }

    #[test]
    fn a_link_on_the_way_is_not_followed() {
        let pair = Pair::new("links");
        fs::create_dir_all(pair.then().join("owner/photos")).unwrap();
        symlink("/etc", pair.then().join("owner/out")).unwrap();
        symlink("owner", pair.then().join("in")).unwrap();
        assert!(no_links(&pair.then(), Path::new("")).is_ok());
        assert!(no_links(&pair.then(), Path::new("owner/photos")).is_ok());
        for (inside, link) in [("owner/out", "owner/out"), ("in/photos", "in")] {
            let words = no_links(&pair.then(), Path::new(inside)).unwrap_err().message;
            let at = pair.then().join(link);
            assert_eq!(words, format!("{}: a link, which a browse does not follow", at.display()));
        }
        let words = no_links(&pair.then(), Path::new("owner/none")).unwrap_err().message;
        assert!(words.ends_with("none: No such file or directory"), "{words}");
    }

    #[test]
    fn a_folder_is_within_a_place_by_its_path() {
        let place = Path::new("/users");
        assert_eq!(within(place, Path::new("/users")).as_deref(), Some(Path::new("")));
        assert_eq!(
            within(place, Path::new("/users/owner/photos")).as_deref(),
            Some(Path::new("owner/photos"))
        );
        assert_eq!(within(place, Path::new("/usersx")), None);
        assert_eq!(within(place, Path::new("/shared")), None);
        assert_eq!(
            within(Path::new("/"), Path::new("/users")).as_deref(),
            Some(Path::new("users"))
        );
    }

    #[test]
    fn the_json_has_the_members_the_page_reads() {
        let found = Browse {
            entries: vec![
                Entry {
                    name: "photos".into(),
                    kind: Kind::Folder,
                    size: None,
                    modified_ms: Some(5),
                    then: Then::Same,
                },
                Entry {
                    name: "IMG_4410.heic".into(),
                    kind: Kind::File,
                    size: Some(3_100_000),
                    modified_ms: Some(1_763_000_000_000),
                    then: Then::Deleted,
                },
            ],
            more: false,
        };
        assert_eq!(
            json("/users", "2026-11-13-0302", "/users/owner", &found),
            json!({
                "place": "/users",
                "snapshot": "2026-11-13-0302",
                "folder": "/users/owner",
                "entries": [
                    {"name": "photos", "kind": "folder", "size": null, "modified_ms": 5, "then": "same"},
                    {"name": "IMG_4410.heic", "kind": "file", "size": 3_100_000, "modified_ms": 1_763_000_000_000u64, "then": "deleted"},
                ],
                "more": false,
            })
        );
        let said = rows(&found);
        assert_eq!(said[0], ["NAME", "SIZE", "MODIFIED", "NOW"]);
        assert_eq!(
            (said[1][0].as_str(), said[1][1].as_str(), said[1][3].as_str()),
            ("photos/", "-", "same")
        );
        assert_eq!((said[2][0].as_str(), said[2][3].as_str()), ("IMG_4410.heic", "deleted"));
        // Sizes as df(1) and ls -h give them.
        assert_eq!(said[2][1], "3.0M");
    }
}
