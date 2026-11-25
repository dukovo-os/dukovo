// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! `snap restore --from SNAPSHOT --beside LABEL PATH...`: each path's copy
//! from the snapshot, put beside it under a name of its own, so that what
//! is there now is never written over.
//!
//! The web console restores as the person (`snapshots.restore`), or as an
//! administrator (`snapshots.restore-all`) into folders that other
//! accounts own, where a person could have put a link at the very name
//! the copy will make, or in place of a folder on the way. A process that
//! acts for root cannot become another account on Dukovo (only authd
//! starts one, with the names capability), so the copy does not change who
//! it is. Instead nothing is reached by a path that another account could
//! change:
//!
//! - each folder on the way, in the snapshot and in the place, is opened
//!   beneath the one before it, one name at a time, never through a link;
//!   the snapshot is read-only, so what is checked there stays so;
//! - the copy is made in a folder of its own beside the new name, which
//!   this process makes, checks it made, and alone may enter, and each
//!   file, folder and link in it is made with `O_EXCL`, never through a
//!   link;
//! - the new name is taken first, with `O_EXCL | O_NOFOLLOW`, and the copy
//!   moved onto it in one rename, which replaces a name and never writes
//!   through one; a rename that fails gives the name back only while it is
//!   still what was taken, which stays open until then so that nothing
//!   else can be given its number;
//! - a link in the snapshot is copied as a link, never followed.
//!
//! Each copy keeps the mode and the times of what it copies, and its owner
//! and group when root restores (a person's own copies are theirs). An
//! access list is not copied: the copy of an entry that had one gets the
//! group permissions the list let the file's group have, never more, and
//! snap says so.

use std::fs::{File, FileTimes};
use std::io::{self, Write};
use std::os::fd::OwnedFd;
use std::path::{Component, Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use acl::{NO_ID, Tag};

use super::{absolute, place, snapshots, top_of};
use crate::backup_errors;
use crate::bundles::folder::{self, DEPTH_MAX, Dir, c_name, fd_stat};
use crate::util::{self, Fail};

/// The most characters a label has.
pub const LABEL_MAX: usize = 40;

/// The most bytes a name has.
const NAME_MAX: usize = 255;

/// The start of the name of the folder a copy is made in.
const STAGING: &str = ".snap-restore-";

/// How many names for that folder are tried.
const STAGING_TRIES: u32 = 16;

/// Whether `label` can name a copy: 1 to [`LABEL_MAX`] characters, none
/// of them a control character or `/`, and not `.` or `..`.
pub fn label_ok(label: &str) -> bool {
    let count = label.chars().count();
    (1..=LABEL_MAX).contains(&count)
        && label != "."
        && label != ".."
        && !label.contains('/')
        && !label.chars().any(char::is_control)
}

/// The name of `name`'s copy labelled `label`: `STEM (LABEL).EXT` for a
/// file with an extension, `NAME (LABEL)` for a folder or a name with
/// none.
pub fn beside_name(name: &str, label: &str, folder: bool) -> String {
    let path = Path::new(name);
    let stem = path.file_stem().and_then(|stem| stem.to_str());
    let extension = path.extension().and_then(|ext| ext.to_str()).filter(|ext| !ext.is_empty());
    match (folder, stem, extension) {
        (false, Some(stem), Some(extension)) => format!("{stem} ({label}).{extension}"),
        _ => format!("{name} ({label})"),
    }
}

/// `snap restore --from SNAPSHOT --beside LABEL PATH...`: each path in
/// turn, saying each copy as it is made. A path whose copy's name is
/// taken stops it with BAK-106, after the copies before it.
///
/// # Errors
///
/// A label that cannot be one (a usage error); BAK-105 for a snapshot the
/// place does not have; BAK-106 for a name that is taken; a path that is
/// not in the snapshot, a folder on the way that is a link or is not there
/// now, or what the copy met.
pub(super) fn restore(snapshot: &str, label: &str, paths: &[PathBuf]) -> Result<(), Fail> {
    if !label_ok(label) {
        return Err(util::usage(format!(
            "'{label}' cannot label a copy: a label is 1 to {LABEL_MAX} characters, with no / or \
             control character, and is not . or .."
        )));
    }
    for path in paths {
        let (made, notes) = one(snapshot, label, path)?;
        for note in notes {
            eprintln!("snap: {note}");
        }
        let _ = writeln!(io::stdout(), "Restored {} from snapshot {snapshot}", made.display());
    }
    Ok(())
}

/// Restores `path` beside itself from `snapshot`: the copy's path, and
/// what snap has to say of it.
fn one(snapshot: &str, label: &str, path: &Path) -> Result<(PathBuf, Vec<String>), Fail> {
    let path = absolute(path)?;
    let (top, inner) = top_of(&path)?;
    // top_of follows links to find the subvolume: the path given must be
    // the one it found, or a link is on the way.
    if top.join(&inner) != path {
        return Err(util::fail(format!(
            "{}: a link is on the way to it; give the path it leads to",
            path.display()
        )));
    }
    let mut parts = Vec::new();
    for part in inner.components() {
        let Component::Normal(part) = part else {
            return Err(util::fail(format!("{}: not a path to restore", path.display())));
        };
        let part = part
            .to_str()
            .ok_or_else(|| util::fail(format!("{}: a name in it is not text", path.display())))?;
        parts.push(part);
    }
    let Some((name, folders)) = parts.split_last() else {
        return Err(util::fail(format!(
            "{}: the top of a place; restore what is in it",
            path.display()
        )));
    };
    let place = place(&top)?;
    if !snapshots(&place)?.iter().any(|snap| snap.name == snapshot) {
        return Err(backup_errors::no_such_snapshot(snapshot, &place.top.display().to_string()));
    }
    let parent = path.parent().unwrap_or(Path::new("/"));
    let shown = place.shown.join(snapshot);
    let from = walk(&shown, folders).map_err(|err| on_the_way(parent, err))?;
    let into = walk(&top, folders).map_err(|err| on_the_way(parent, err))?;
    let mut notes = Vec::new();
    let made = restore_into(&from, name, &shown.join(&inner), &into, label, &mut notes).map_err(
        |why| match why {
            Why::Taken(beside) => {
                backup_errors::already_there(&parent.join(beside).display().to_string())
            }
            Why::NotThere => util::fail(format!("{}: not in snapshot {snapshot}", path.display())),
            Why::Failed(err) => util::failed(&path.display(), err),
        },
    )?;
    Ok((parent.join(made), notes))
}

/// The folder `folders` beneath `start`, each opened beneath the one
/// before it, never through a link.
fn walk(start: &Path, folders: &[&str]) -> io::Result<Dir> {
    let start = start.to_str().ok_or_else(|| io::Error::from(io::ErrorKind::InvalidInput))?;
    let mut dir = Dir::open(start)?;
    for folder in folders {
        dir = dir.dir_at(folder)?;
    }
    Ok(dir)
}

/// The failure of opening the folders on the way to `parent`.
fn on_the_way(parent: &Path, err: io::Error) -> Fail {
    let why = match err.raw_os_error() {
        Some(libc::ELOOP | libc::ENOTDIR) => "a folder on the way is a link, or not a folder",
        Some(libc::ENOENT) => "it is not there now: restore the folder first",
        _ => return util::failed(&parent.display(), err),
    };
    util::fail(format!("{}: {why}", parent.display()))
}

/// Why a copy was not made.
#[derive(Debug)]
enum Why {
    /// Its name, which is taken.
    Taken(String),
    /// The snapshot has nothing of that name.
    NotThere,
    /// What failed.
    Failed(io::Error),
}

impl From<io::Error> for Why {
    fn from(err: io::Error) -> Why {
        Why::Failed(err)
    }
}

/// Copies `name` of `from`, a folder of a snapshot whose path is `at`,
/// into `into` beside what is called `name` there now, labelled `label`:
/// the copy's name. What snap says of the copy goes in `notes`.
fn restore_into(
    from: &Dir,
    name: &str,
    at: &Path,
    into: &Dir,
    label: &str,
    notes: &mut Vec<String>,
) -> Result<String, Why> {
    let Some(source) = from.stat_at(name)? else { return Err(Why::NotThere) };
    let folder = kind(&source) == libc::S_IFDIR;
    let beside = beside_name(name, label, folder);
    if beside.len() > NAME_MAX {
        return Err(Why::Failed(io::Error::other(format!(
            "{beside}: longer than the {NAME_MAX} bytes a name has"
        ))));
    }
    c_name(&beside)?;
    if into.stat_at(&beside)?.is_some() {
        return Err(Why::Taken(beside));
    }
    let staging = Staging::make(into)?;
    copy(from, name, &staging.dir, &source, at, 0, notes)?;
    let claimed = claim(into, &beside, folder).map_err(|err| match err.raw_os_error() {
        Some(libc::EEXIST) => Why::Taken(beside.clone()),
        _ => Why::Failed(err),
    })?;
    let moved = staging.dir.move_to(name, into, &beside);
    if let Err(err) = moved {
        unclaim(into, &beside, &claimed);
        return Err(Why::Failed(err));
    }
    Ok(beside)
}

/// The kind of what `st` describes: its `S_IFMT` bits.
fn kind(st: &libc::stat) -> libc::mode_t {
    st.st_mode & libc::S_IFMT
}

/// Whether this process acts for root, and so gives copies their owners.
fn is_root() -> bool {
    sysapi::process::euid() == 0
}

/// The folder a copy is made in: made by this process in the folder the
/// copy goes to, so that the copy can be moved to its name in one rename;
/// removed with what is left in it when it goes.
struct Staging<'a> {
    into: &'a Dir,
    name: String,
    dir: Dir,
}

impl<'a> Staging<'a> {
    /// Makes one in `into`, under a name no one has, open, which only this
    /// process's account may enter.
    fn make(into: &'a Dir) -> io::Result<Staging<'a>> {
        let me = sysapi::process::euid();
        for n in 0..STAGING_TRIES {
            let name = format!("{STAGING}{}-{n}", std::process::id());
            match into.make(&name, 0o700) {
                Ok(()) => {}
                Err(err) if err.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(err) => return Err(err),
            }
            let dir = into.dir_at(&name)?;
            let made = dir.stat()?;
            // The folder at that name must be the one made: another account
            // that may write here could have put its own in its place.
            if made.st_uid != me || kind(&made) != libc::S_IFDIR {
                return Err(io::Error::other(format!("{name}: not the folder snap made")));
            }
            // The mode leaves out a default access list's entries too: it
            // sets the mask to nothing.
            sysapi::fs::set_mode(&dir, 0o700)?;
            return Ok(Staging { into, name, dir });
        }
        Err(io::Error::new(io::ErrorKind::AlreadyExists, "no free name to make the copy in"))
    }
}

impl Drop for Staging<'_> {
    fn drop(&mut self) {
        // What is left in it, through the descriptor, then the folder
        // itself if its name is still the one made.
        if let Ok(names) = self.dir.names() {
            for name in names {
                let _ = self.dir.remove_tree(&name, 0);
            }
        }
        let (Ok(made), Ok(Some(named))) = (self.dir.stat(), self.into.stat_at(&self.name)) else {
            return;
        };
        if folder::same(&made, &named) {
            let _ = self.into.remove_dir(&self.name);
        }
    }
}

/// Takes the name `beside` in `into` for the copy: an empty folder, or an
/// empty file, made only if nothing has the name, never through a link.
/// What was made, held open: while it is, no other file can be given its
/// number, so [`unclaim`] can tell it from one put at the name since.
fn claim(into: &Dir, beside: &str, folder: bool) -> io::Result<OwnedFd> {
    if folder {
        into.make(beside, 0o700)?;
        return Ok(into.dir_at(beside)?.into());
    }
    create(into, beside)
}

/// Removes what [`claim`] made, if it is still at the name.
fn unclaim(into: &Dir, beside: &str, claimed: &OwnedFd) {
    let (Ok(made), Ok(Some(named))) = (fd_stat(claimed), into.stat_at(beside)) else {
        return;
    };
    if folder::same(&made, &named) {
        let _ = if kind(&named) == libc::S_IFDIR {
            into.remove_dir(beside)
        } else {
            into.unlink(beside)
        };
    }
}

/// Makes the file `name` in `into` to write, if nothing has the name,
/// not through a link, which only its owner may read for now.
fn create(into: &Dir, name: &str) -> io::Result<OwnedFd> {
    into.create_new(name, 0o600)
}

/// Copies `name` of `from`, which `source` describes and the snapshot
/// holds at `at`, into `into` under the same name, `depth` folders down.
fn copy(
    from: &Dir,
    name: &str,
    into: &Dir,
    source: &libc::stat,
    at: &Path,
    depth: usize,
    notes: &mut Vec<String>,
) -> io::Result<()> {
    match kind(source) {
        libc::S_IFDIR => {
            if depth > DEPTH_MAX {
                return Err(io::Error::other(format!(
                    "{}: more than {DEPTH_MAX} folders deep",
                    at.display()
                )));
            }
            let inner = from.dir_at(name)?;
            same_as(&inner.stat()?, source, at)?;
            into.make(name, 0o700)?;
            let made = into.dir_at(name)?;
            for entry in inner.names()? {
                match inner.stat_at(&entry) {
                    Ok(Some(st)) => {
                        copy(&inner, &entry, &made, &st, &at.join(&entry), depth + 1, notes)?;
                    }
                    _ => notes.push(format!(
                        "{}: left out: its name could not be read",
                        at.join(&entry).display()
                    )),
                }
            }
            finish(made.into(), source, at, notes)
        }
        libc::S_IFREG => {
            let read = from.open_at(name, false)?;
            same_as(&fd_stat(&read)?, source, at)?;
            let written = create(into, name)?;
            clone_or_copy(read, &written)?;
            finish(written, source, at, notes)
        }
        libc::S_IFLNK => {
            let target = from
                .read_link(name)?
                .ok_or_else(|| io::Error::other(format!("{}: not a link now", at.display())))?;
            into.symlink(&target, name)?;
            if is_root() {
                // The link itself, and not its target.
                into.set_owner_at(name, source.st_uid, source.st_gid)?;
            }
            Ok(())
        }
        _ => {
            notes.push(format!("{}: left out: not a file, a folder or a link", at.display()));
            Ok(())
        }
    }
}

/// That what was opened is what was looked at: a read-only snapshot does
/// not change, so anything else is a fault.
fn same_as(opened: &libc::stat, looked: &libc::stat, at: &Path) -> io::Result<()> {
    if folder::same(opened, looked) {
        Ok(())
    } else {
        Err(io::Error::other(format!("{}: changed while it was read", at.display())))
    }
}

/// Gives `written` the data of `read`: a clone that shares it, where the
/// filesystem makes one, else a copy.
fn clone_or_copy(read: OwnedFd, written: &OwnedFd) -> io::Result<()> {
    if sysapi::fs::clone_data(written, &read).is_ok() {
        return Ok(());
    }
    let mut read = File::from(read);
    let mut written = File::from(written.try_clone()?);
    io::copy(&mut read, &mut written)?;
    Ok(())
}

/// Gives the copy `fd` the owner (when root restores), the mode and the
/// times of `source`, at `at` in the snapshot.
fn finish(fd: OwnedFd, source: &libc::stat, at: &Path, notes: &mut Vec<String>) -> io::Result<()> {
    if is_root() {
        // The owner goes first, since changing it clears a set-user-ID bit.
        std::os::unix::fs::fchown(&fd, Some(source.st_uid), Some(source.st_gid))?;
    }
    let mode = mode_of(source, at, notes);
    sysapi::fs::set_mode(&fd, mode)?;
    let times = FileTimes::new()
        .set_accessed(time(source.st_atime, source.st_atime_nsec))
        .set_modified(time(source.st_mtime, source.st_mtime_nsec));
    // A copy whose times cannot be set is still the copy.
    let _ = File::from(fd).set_times(times);
    Ok(())
}

/// The mode a copy of `source` gets: its own, but for one that has an
/// access list (read at `at`, which a read-only snapshot holds as it was
/// looked at), whose group permissions are those the list let the group
/// have. Said in `notes`.
fn mode_of(source: &libc::stat, at: &Path, notes: &mut Vec<String>) -> libc::mode_t {
    let mode = source.st_mode & 0o7777;
    let Ok(list) = acl::access_list(at, mode, true) else { return mode };
    let Some(mask) = list.get(Tag::Mask, NO_ID).map(|entry| entry.perm) else { return mode };
    let group = list.get(Tag::GroupObj, NO_ID).map_or(0, |entry| entry.perm);
    notes.push(format!(
        "{}: its access list was not copied; give it again with setfacl",
        at.display()
    ));
    (mode & !0o070) | (libc::mode_t::from(mask & group) << 3)
}

/// The time `seconds` and `nanoseconds` after 1970 say, or 1970 itself for
/// one before it.
fn time(seconds: libc::time_t, nanoseconds: libc::c_long) -> SystemTime {
    let seconds = u64::try_from(seconds).unwrap_or(0);
    let nanoseconds = u32::try_from(nanoseconds).unwrap_or(0);
    UNIX_EPOCH + Duration::new(seconds, nanoseconds)
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::os::unix::fs::{MetadataExt, PermissionsExt, symlink};

    use super::*;

    /// A snapshot's folder and a place's, made anew for one test.
    struct Trees {
        top: PathBuf,
    }

    impl Trees {
        fn new(test: &str) -> Trees {
            let top =
                std::env::temp_dir().join(format!("snap-beside-{test}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&top);
            fs::create_dir_all(top.join("snapshot/ann")).unwrap();
            fs::create_dir_all(top.join("place/ann")).unwrap();
            Trees { top }
        }

        fn snapshot(&self) -> PathBuf {
            self.top.join("snapshot/ann")
        }

        fn place(&self) -> PathBuf {
            self.top.join("place/ann")
        }

        /// Restores `name` of the snapshot's folder into the place's.
        fn restore(&self, name: &str, label: &str) -> (Result<String, Why>, Vec<String>) {
            let from = Dir::open(self.snapshot().to_str().unwrap()).unwrap();
            let into = Dir::open(self.place().to_str().unwrap()).unwrap();
            let mut notes = Vec::new();
            let made =
                restore_into(&from, name, &self.snapshot().join(name), &into, label, &mut notes);
            (made, notes)
        }

        /// The names in the place's folder.
        fn names(&self) -> Vec<String> {
            let mut names: Vec<String> = fs::read_dir(self.place())
                .unwrap()
                .map(|entry| entry.unwrap().file_name().into_string().unwrap())
                .collect();
            names.sort();
            names
        }
    }

    impl Drop for Trees {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.top);
        }
    }

    #[test]
    fn a_label_is_one_short_line_with_no_slash_that_is_not_a_dot_or_two() {
        for good in ["lab", "Nov 13", "before the update", &"x".repeat(LABEL_MAX), "é"] {
            assert!(label_ok(good), "{good}");
        }
        for bad in ["", ".", "..", "a/b", "/", "a\nb", "tab\there", &"x".repeat(LABEL_MAX + 1)] {
            assert!(!label_ok(bad), "{bad:?}");
        }
    }

    #[test]
    fn the_copy_s_name_keeps_a_file_s_extension_last() {
        assert_eq!(beside_name("notes.txt", "lab", false), "notes (lab).txt");
        assert_eq!(beside_name("archive.tar.gz", "lab", false), "archive.tar (lab).gz");
        assert_eq!(beside_name("README", "lab", false), "README (lab)");
        assert_eq!(beside_name(".profile", "lab", false), ".profile (lab)");
        assert_eq!(beside_name("notes.", "lab", false), "notes. (lab)");
        assert_eq!(beside_name("photos.2026", "lab", true), "photos.2026 (lab)");
    }

    #[test]
    fn a_file_is_restored_beside_itself_with_its_mode_and_times_and_none_is_left_over() {
        let trees = Trees::new("file");
        let old = trees.snapshot().join("notes.txt");
        fs::write(&old, "as it was\n").unwrap();
        fs::set_permissions(&old, fs::Permissions::from_mode(0o640)).unwrap();
        let then = UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        File::options().write(true).open(&old).unwrap().set_modified(then).unwrap();
        fs::write(trees.place().join("notes.txt"), "as it is\n").unwrap();
        let (made, notes) = trees.restore("notes.txt", "lab");
        assert_eq!(made.unwrap(), "notes (lab).txt");
        assert!(notes.is_empty(), "{notes:?}");
        let copy = trees.place().join("notes (lab).txt");
        assert_eq!(fs::read_to_string(&copy).unwrap(), "as it was\n");
        assert_eq!(fs::read_to_string(trees.place().join("notes.txt")).unwrap(), "as it is\n");
        let meta = fs::metadata(&copy).unwrap();
        assert_eq!(meta.mode() & 0o7777, 0o640);
        assert_eq!(meta.modified().unwrap(), then);
        assert_eq!(meta.uid(), fs::metadata(&old).unwrap().uid());
        // The folder it was made in is gone.
        assert_eq!(trees.names(), ["notes (lab).txt", "notes.txt"]);
    }

    #[test]
    fn a_folder_is_restored_whole_and_its_links_stay_links() {
        let trees = Trees::new("folder");
        let photos = trees.snapshot().join("photos");
        fs::create_dir_all(photos.join("2026")).unwrap();
        fs::write(photos.join("2026/a.jpg"), "a").unwrap();
        symlink("/system/settings", photos.join("settings")).unwrap();
        fs::set_permissions(&photos, fs::Permissions::from_mode(0o750)).unwrap();
        let (made, _) = trees.restore("photos", "Nov 13");
        assert_eq!(made.unwrap(), "photos (Nov 13)");
        let copy = trees.place().join("photos (Nov 13)");
        assert_eq!(fs::read_to_string(copy.join("2026/a.jpg")).unwrap(), "a");
        let link = fs::symlink_metadata(copy.join("settings")).unwrap();
        assert!(link.file_type().is_symlink());
        assert_eq!(fs::read_link(copy.join("settings")).unwrap(), Path::new("/system/settings"));
        assert_eq!(fs::metadata(&copy).unwrap().mode() & 0o7777, 0o750);
        assert_eq!(trees.names(), ["photos (Nov 13)"]);
    }

    #[test]
    fn a_link_at_the_copy_s_name_is_bak_106_and_nothing_is_written_through_it() {
        let trees = Trees::new("planted");
        fs::write(trees.snapshot().join("notes.txt"), "as it was\n").unwrap();
        let elsewhere = trees.top.join("elsewhere");
        fs::write(&elsewhere, "not ann's\n").unwrap();
        symlink(&elsewhere, trees.place().join("notes (lab).txt")).unwrap();
        let (made, _) = trees.restore("notes.txt", "lab");
        assert!(matches!(made, Err(Why::Taken(name)) if name == "notes (lab).txt"));
        assert_eq!(fs::read_to_string(&elsewhere).unwrap(), "not ann's\n");
        // A link that leads nowhere is a name taken too.
        fs::remove_file(trees.place().join("notes (lab).txt")).unwrap();
        symlink(trees.top.join("nothing"), trees.place().join("notes (lab).txt")).unwrap();
        let (made, _) = trees.restore("notes.txt", "lab");
        assert!(matches!(made, Err(Why::Taken(_))));
        assert!(!trees.top.join("nothing").exists());
        assert_eq!(trees.names(), ["notes (lab).txt"]);
    }

    #[test]
    fn a_name_taken_after_it_was_looked_at_is_never_written_over() {
        // The claim of the name is what decides: a folder made at the name
        // after the look and before the claim stops the copy.
        let trees = Trees::new("claimed");
        let into = Dir::open(trees.place().to_str().unwrap()).unwrap();
        fs::create_dir(trees.place().join("photos (lab)")).unwrap();
        let err = claim(&into, "photos (lab)", true).unwrap_err();
        assert_eq!(err.raw_os_error(), Some(libc::EEXIST));
        symlink(trees.top.join("nothing"), trees.place().join("notes (lab).txt")).unwrap();
        let err = claim(&into, "notes (lab).txt", false).unwrap_err();
        assert_eq!(err.raw_os_error(), Some(libc::EEXIST));
        assert!(!trees.top.join("nothing").exists());
    }

    #[test]
    fn a_link_in_place_of_a_folder_on_the_way_is_never_followed() {
        let trees = Trees::new("on-the-way");
        fs::create_dir_all(trees.snapshot().join("docs")).unwrap();
        fs::write(trees.snapshot().join("docs/notes.txt"), "as it was\n").unwrap();
        // In the place, ann's docs is now a link to a folder that is not hers.
        let elsewhere = trees.top.join("elsewhere");
        fs::create_dir(&elsewhere).unwrap();
        symlink(&elsewhere, trees.place().join("docs")).unwrap();
        let err = walk(&trees.top.join("place"), &["ann", "docs"]).err().unwrap();
        assert!(matches!(err.raw_os_error(), Some(libc::ELOOP | libc::ENOTDIR)), "{err}");
        let fail = on_the_way(&trees.place().join("docs"), err);
        assert!(fail.message.contains("a folder on the way is a link"), "{}", fail.message);
        // In the snapshot as well.
        let snapshot_docs = trees.snapshot().join("docs");
        fs::remove_dir_all(&snapshot_docs).unwrap();
        symlink(&elsewhere, &snapshot_docs).unwrap();
        assert!(walk(&trees.top.join("snapshot"), &["ann", "docs"]).is_err());
        assert_eq!(fs::read_dir(&elsewhere).unwrap().count(), 0);
    }

    #[test]
    fn a_link_in_the_snapshot_is_restored_as_a_link_and_not_what_it_leads_to() {
        let trees = Trees::new("link");
        let secret = trees.top.join("secret");
        fs::write(&secret, "root's\n").unwrap();
        symlink(&secret, trees.snapshot().join("shortcut")).unwrap();
        let (made, _) = trees.restore("shortcut", "lab");
        assert_eq!(made.unwrap(), "shortcut (lab)");
        let copy = trees.place().join("shortcut (lab)");
        assert!(fs::symlink_metadata(&copy).unwrap().file_type().is_symlink());
        assert_eq!(fs::read_link(&copy).unwrap(), secret);
    }

    #[test]
    fn what_the_snapshot_does_not_have_is_said_and_nothing_is_made() {
        let trees = Trees::new("missing");
        let (made, _) = trees.restore("notes.txt", "lab");
        assert!(matches!(made, Err(Why::NotThere)));
        assert!(trees.names().is_empty());
    }

    #[test]
    fn a_copy_keeps_its_owner_when_root_restores() {
        if !is_root() {
            return;
        }
        let trees = Trees::new("owner");
        let old = trees.snapshot().join("notes.txt");
        fs::write(&old, "ann's\n").unwrap();
        std::os::unix::fs::chown(&old, Some(1000), Some(1000)).unwrap();
        symlink("notes.txt", trees.snapshot().join("link")).unwrap();
        std::os::unix::fs::lchown(trees.snapshot().join("link"), Some(1000), Some(1000)).unwrap();
        trees.restore("notes.txt", "lab").0.unwrap();
        trees.restore("link", "lab").0.unwrap();
        let copy = fs::metadata(trees.place().join("notes (lab).txt")).unwrap();
        assert_eq!((copy.uid(), copy.gid()), (1000, 1000));
        let link = fs::symlink_metadata(trees.place().join("link (lab)")).unwrap();
        assert_eq!((link.uid(), link.gid()), (1000, 1000));
    }

    #[test]
    fn the_folder_the_copy_is_made_in_is_this_process_s_alone_and_goes_after() {
        let trees = Trees::new("staging");
        let into = Dir::open(trees.place().to_str().unwrap()).unwrap();
        // A name another account took first is passed over.
        let first = format!("{STAGING}{}-0", std::process::id());
        fs::create_dir(trees.place().join(&first)).unwrap();
        {
            let staging = Staging::make(&into).unwrap();
            assert_eq!(staging.name, format!("{STAGING}{}-1", std::process::id()));
            let st = staging.dir.stat().unwrap();
            assert_eq!(st.st_mode & 0o777, 0o700);
            fs::write(trees.place().join(&staging.name).join("half"), "made").unwrap();
        }
        assert_eq!(trees.names(), [first]);
    }

    #[test]
    fn a_failed_move_removes_its_claim_only_while_it_is_its_own() {
        let trees = Trees::new("unclaim");
        let into = Dir::open(trees.place().to_str().unwrap()).unwrap();
        let claimed = claim(&into, "notes (lab).txt", false).unwrap();
        unclaim(&into, "notes (lab).txt", &claimed);
        assert!(trees.names().is_empty());
        // Another's file at the name after the claim stays.
        let claimed = claim(&into, "notes (lab).txt", false).unwrap();
        fs::remove_file(trees.place().join("notes (lab).txt")).unwrap();
        fs::write(trees.place().join("notes (lab).txt"), "another's").unwrap();
        unclaim(&into, "notes (lab).txt", &claimed);
        assert_eq!(fs::read_to_string(trees.place().join("notes (lab).txt")).unwrap(), "another's");
    }
}
