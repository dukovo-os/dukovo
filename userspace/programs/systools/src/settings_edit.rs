// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Changing a settings file the way the web console does (ADR 0516).
//!
//! [`change`] reads the file as a TOML document, lets the caller edit it,
//! and writes it back with [`write`], so that every comment, blank line and
//! order of keys the file had stays, and a change that leaves the text as
//! it was writes nothing: the journal and a second run of the same change
//! see no change. [`table`], [`set_string`] and [`set_integer`] are the
//! edits the console makes, which keep the comment after a value they
//! replace and put a new key where a person would.
//!
//! [`write`] puts the new text into a new file beside the old one, which
//! gets the old file's owner and mode and is made durable, and then takes
//! the old file's name in one rename, so that a reader sees the old file or
//! the new one and never a part of either, and a power cut leaves one of
//! the two.
//!
//! When a change fails, [`Changed`] says which step did and names the file;
//! the console's answer and `log admind` carry those words, and `admin
//! reach` prints them. The host tests below cover both parts.

use std::ffi::OsString;
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::os::unix::ffi::OsStringExt;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use toml_edit::{DocumentMut, Item, Table, TableLike, Value};

use crate::util::strip_os_error;

/// Who owns a file and its mode, which the new file keeps.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Owner {
    uid: u32,
    gid: u32,
    mode: u32,
}

/// Makes `path` hold `bytes`, as the module says. Returns false, having
/// written nothing, when the file holds them already. A file that is not
/// there yet is made with `new_mode` and the writer's own owner. A link
/// is refused rather than followed or replaced: the console names files,
/// never what a link would make of them.
pub fn write(path: &Path, bytes: &[u8], new_mode: u32) -> io::Result<bool> {
    let owner = match fs::symlink_metadata(path) {
        Ok(found) if found.file_type().is_symlink() => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "it is a link, which is not written through",
            ));
        }
        Ok(found) if !found.is_file() => {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "it is not a file"));
        }
        Ok(found) => {
            if fs::read(path)? == bytes {
                return Ok(false);
            }
            Some(Owner { uid: found.uid(), gid: found.gid(), mode: found.mode() & 0o7777 })
        }
        Err(err) if err.kind() == io::ErrorKind::NotFound => None,
        Err(err) => return Err(err),
    };
    let folder = folder_of(path)?;
    let new = new_name(path)?;
    let written = write_new(&new, bytes, owner, new_mode)
        .and_then(|()| fs::rename(&new, path))
        .and_then(|()| File::open(&folder)?.sync_all());
    if written.is_err() {
        // What is left of the new file is nobody's; the old one is as it was.
        let _ = fs::remove_file(&new);
    }
    written.map(|()| true)
}

/// Why [`change`] did not make its change, and the file it was about.
#[derive(Debug)]
pub enum Changed<E> {
    /// The file could not be read.
    Read {
        /// The file.
        path: PathBuf,
        /// What the system said.
        error: io::Error,
    },
    /// The file is not TOML.
    Parse {
        /// The file.
        path: PathBuf,
        /// The line the parser stopped at, when it says.
        line: Option<usize>,
        /// What the parser said was wrong.
        message: String,
    },
    /// The edit refused: its own error.
    Edit {
        /// The file.
        path: PathBuf,
        /// What the edit said.
        error: E,
    },
    /// The new text could not be written.
    Write {
        /// The file.
        path: PathBuf,
        /// What the system said.
        error: io::Error,
    },
}

impl<E: fmt::Display> fmt::Display for Changed<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Changed::Read { path, error } => {
                write!(f, "cannot read {}: {}", path.display(), strip_os_error(error))
            }
            Changed::Parse { path, line: Some(line), message } => {
                write!(f, "{} is not valid TOML, at line {line}: {message}", path.display())
            }
            Changed::Parse { path, line: None, message } => {
                write!(f, "{} is not valid TOML: {message}", path.display())
            }
            Changed::Edit { path, error } => write!(f, "{}: {error}", path.display()),
            Changed::Write { path, error } => {
                write!(f, "cannot write {}: {}", path.display(), strip_os_error(error))
            }
        }
    }
}

/// The line of `text` that the byte `offset` is in, counted from 1.
pub fn line_at(text: &str, offset: usize) -> usize {
    let before = &text.as_bytes()[..offset.min(text.len())];
    before.iter().filter(|&&byte| byte == b'\n').count() + 1
}

/// Reads the TOML file `path`, lets `edit` change it, and writes it back
/// with `write`, a file that is not there being a document with nothing in
/// it, made with `new_mode`. Everything the edit leaves alone is written
/// as it was: comments, blank lines and the order of the keys. Returns
/// false, having written nothing, when the edit changed nothing.
///
/// The read and the write are not one step: what another program writes
/// between them is lost.
///
/// # Errors
///
/// Which step failed, with the file, as [`Changed`] says. The file is as
/// it was.
pub fn change<E>(
    path: &Path,
    new_mode: u32,
    edit: impl FnOnce(&mut DocumentMut) -> Result<(), E>,
) -> Result<bool, Changed<E>> {
    let existing = match fs::read_to_string(path) {
        Ok(text) => Some(text),
        Err(error) if error.kind() == io::ErrorKind::NotFound => None,
        Err(error) => return Err(Changed::Read { path: path.to_path_buf(), error }),
    };
    let text = existing.as_deref().unwrap_or_default();
    let mut document: DocumentMut = text.parse().map_err(|error: toml_edit::TomlError| {
        let line = error.span().map(|span| line_at(text, span.start));
        Changed::Parse { path: path.to_path_buf(), line, message: error.message().to_string() }
    })?;
    // The text the document makes before the edit, which is not always the
    // file's: the parser drops a carriage return that follows a comment.
    // An edit that changes nothing is told by it, so that it writes
    // nothing, and makes no file of one that is not there.
    let unedited = document.to_string();
    edit(&mut document).map_err(|error| Changed::Edit { path: path.to_path_buf(), error })?;
    let new = document.to_string();
    if new == unedited {
        return Ok(false);
    }
    write(path, new.as_bytes(), new_mode)
        .map_err(|error| Changed::Write { path: path.to_path_buf(), error })
}

/// The table `name` of `document`, which is made after everything the
/// file holds, the comments that end it included, when there is none: the
/// file that ships with only comments in it keeps them above the table.
/// None when `name` is a key that holds something other than a table.
pub fn table<'a>(document: &'a mut DocumentMut, name: &str) -> Option<&'a mut dyn TableLike> {
    if !document.contains_key(name) {
        let mut made = Table::new();
        let mut before = document.trailing().as_str().unwrap_or_default().to_string();
        if !before.is_empty() {
            // A blank line between the comments and the table, which is
            // what the parser puts before a table that follows another.
            if !before.ends_with("\n\n") {
                before.push_str(if before.ends_with('\n') { "\n" } else { "\n\n" });
            }
            made.decor_mut().set_prefix(before);
            document.set_trailing("");
        }
        document.insert(name, Item::Table(made));
    }
    document.get_mut(name)?.as_table_like_mut()
}

/// Makes `key` of `table` the string `value`, keeping the comment that
/// follows the value it replaces, and adding the key after the others when
/// there is none. A key that holds `value` already is left as the file
/// wrote it, whatever its quotes.
pub fn set_string(table: &mut dyn TableLike, key: &str, value: &str) {
    if table.get(key).and_then(Item::as_str) != Some(value) {
        put(table, key, Value::from(value));
    }
}

/// [`set_string`] for an integer.
pub fn set_integer(table: &mut dyn TableLike, key: &str, value: i64) {
    if table.get(key).and_then(Item::as_integer) != Some(value) {
        put(table, key, Value::from(value));
    }
}

/// Puts `value` at `key`, in the place and with the comment of the value
/// there, if there is one.
fn put(table: &mut dyn TableLike, key: &str, mut value: Value) {
    match table.get_mut(key).and_then(Item::as_value_mut) {
        Some(old) => {
            *value.decor_mut() = old.decor().clone();
            *old = value;
        }
        None => {
            table.insert(key, Item::Value(value));
        }
    }
}

/// The folder `path` is in, which the rename changes.
fn folder_of(path: &Path) -> io::Result<PathBuf> {
    match path.parent() {
        Some(folder) if folder.as_os_str().is_empty() => Ok(PathBuf::from(".")),
        Some(folder) => Ok(folder.to_path_buf()),
        None => Err(io::Error::new(io::ErrorKind::InvalidInput, "it names no file")),
    }
}

/// `.NAME.new-PID` beside `path`: hidden, and this process's alone.
fn new_name(path: &Path) -> io::Result<PathBuf> {
    let name = path
        .file_name()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "it names no file"))?;
    let mut hidden = OsString::from(".");
    hidden.push(name);
    hidden.push(format!(".new-{}", std::process::id()));
    Ok(path.with_file_name(OsString::from_vec(hidden.into_vec())))
}

/// Writes `bytes` into the new file `new`, owned and with the mode of
/// `owner` when there is one and `new_mode` when not, and makes it
/// durable. It is made 0600 and given its owner and mode before anything
/// is in it, so that no one reads it who may not read the old file. A new
/// file left by a write that died is taken away first.
fn write_new(new: &Path, bytes: &[u8], owner: Option<Owner>, new_mode: u32) -> io::Result<()> {
    match fs::remove_file(new) {
        Err(err) if err.kind() != io::ErrorKind::NotFound => return Err(err),
        _ => {}
    }
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(new)?;
    if let Some(owner) = owner {
        let made = file.metadata()?;
        if (made.uid(), made.gid()) != (owner.uid, owner.gid) {
            std::os::unix::fs::fchown(&file, Some(owner.uid), Some(owner.gid))?;
        }
    }
    let mode = owner.map_or(new_mode, |owner| owner.mode);
    file.set_permissions(fs::Permissions::from_mode(mode))?;
    file.write_all(bytes)?;
    file.sync_all()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A folder of its own under the system's temporary folder, taken
    /// away at the end.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str) -> Scratch {
            let path =
                std::env::temp_dir().join(format!("settings-edit-{name}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&path);
            fs::create_dir_all(&path).unwrap();
            Scratch(path)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn mode_of(path: &Path) -> u32 {
        fs::metadata(path).unwrap().mode() & 0o7777
    }

    #[test]
    fn a_write_replaces_the_text_and_keeps_the_mode() {
        let scratch = Scratch::new("keeps");
        let path = scratch.0.join("admin.toml");
        fs::write(&path, "reach = \"private\"\n").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o640)).unwrap();
        assert!(write(&path, b"reach = \"any\"\n", 0o600).unwrap());
        assert_eq!(fs::read_to_string(&path).unwrap(), "reach = \"any\"\n");
        assert_eq!(mode_of(&path), 0o640);
        // Nothing is left beside it.
        let names: Vec<_> =
            fs::read_dir(&scratch.0).unwrap().map(|entry| entry.unwrap().file_name()).collect();
        assert_eq!(names, ["admin.toml"]);
    }

    #[test]
    fn the_same_text_writes_nothing() {
        let scratch = Scratch::new("same");
        let path = scratch.0.join("a.toml");
        fs::write(&path, "a = 1\n").unwrap();
        let before = fs::metadata(&path).unwrap().ino();
        assert!(!write(&path, b"a = 1\n", 0o644).unwrap());
        assert_eq!(fs::metadata(&path).unwrap().ino(), before);
    }

    #[test]
    fn a_new_file_gets_the_mode_it_is_given() {
        let scratch = Scratch::new("new");
        let path = scratch.0.join("settings.toml");
        assert!(write(&path, b"theme = \"dark\"\n", 0o600).unwrap());
        assert_eq!(fs::read_to_string(&path).unwrap(), "theme = \"dark\"\n");
        assert_eq!(mode_of(&path), 0o600);
    }

    #[test]
    fn a_link_or_a_folder_is_refused_and_left_as_it_is() {
        let scratch = Scratch::new("link");
        let target = scratch.0.join("target.toml");
        fs::write(&target, "a = 1\n").unwrap();
        let link = scratch.0.join("link.toml");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        let err = write(&link, b"a = 2\n", 0o644).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
        assert_eq!(fs::read_to_string(&target).unwrap(), "a = 1\n");
        assert!(fs::symlink_metadata(&link).unwrap().file_type().is_symlink());
        assert_eq!(write(&scratch.0, b"x", 0o644).unwrap_err().kind(), io::ErrorKind::InvalidInput);
    }

    #[test]
    fn a_new_file_left_by_a_write_that_died_is_taken_away() {
        let scratch = Scratch::new("left");
        let path = scratch.0.join("b.toml");
        fs::write(&path, "b = 1\n").unwrap();
        let left = new_name(&path).unwrap();
        fs::write(&left, "half").unwrap();
        assert!(write(&path, b"b = 2\n", 0o644).unwrap());
        assert_eq!(fs::read_to_string(&path).unwrap(), "b = 2\n");
        assert!(!left.exists());
    }

    #[test]
    fn a_folder_that_is_not_there_fails_and_leaves_nothing() {
        let scratch = Scratch::new("none");
        let path = scratch.0.join("no-such-folder").join("c.toml");
        assert_eq!(write(&path, b"c = 1\n", 0o644).unwrap_err().kind(), io::ErrorKind::NotFound);
    }

    /// A settings file as a person keeps it: a header, a blank line, a
    /// comment for each key, and a comment after a value.
    const KEPT: &str = "\
# SPDX-FileCopyrightText: 2026 The Dukovo contributors

# Where the console answers, and whom.
[console]
# The port.
listen = 9090 # not 443

# Whom: private or any.
reach = \"private\" # for now
certificate = \"this-machine\"

[sign-in]
session-hours = 12
";

    /// Writes `text` to a file of the scratch folder and changes it with
    /// `edit`, which cannot fail.
    fn changed(
        scratch: &Scratch,
        text: &str,
        edit: impl FnOnce(&mut DocumentMut),
    ) -> (bool, String) {
        let path = scratch.0.join("settings.toml");
        fs::write(&path, text).unwrap();
        let wrote = change(&path, 0o600, |document| {
            edit(document);
            Ok::<(), std::convert::Infallible>(())
        })
        .unwrap();
        (wrote, fs::read_to_string(&path).unwrap())
    }

    fn console(document: &mut DocumentMut) -> &mut dyn TableLike {
        table(document, "console").unwrap()
    }

    #[test]
    fn a_changed_value_keeps_every_comment_blank_line_and_the_order_around_it() {
        let scratch = Scratch::new("kept");
        let (wrote, text) =
            changed(&scratch, KEPT, |document| set_string(console(document), "reach", "any"));
        assert!(wrote);
        assert_eq!(
            text,
            KEPT.replace("reach = \"private\" # for now", "reach = \"any\" # for now")
        );
        let (wrote, text) =
            changed(&scratch, KEPT, |document| set_integer(console(document), "listen", 443));
        assert!(wrote);
        assert_eq!(text, KEPT.replace("listen = 9090 # not 443", "listen = 443 # not 443"));
    }

    #[test]
    fn a_new_key_goes_to_the_end_of_its_table_and_not_to_the_end_of_the_file() {
        let scratch = Scratch::new("key");
        let text = "[console]\nlisten = 9090 # port\n\n[sign-in]\n# Hours.\nsession-hours = 3\n";
        let (wrote, text) =
            changed(&scratch, text, |document| set_string(console(document), "reach", "any"));
        assert!(wrote);
        assert_eq!(
            text,
            "[console]\nlisten = 9090 # port\nreach = \"any\"\n\n[sign-in]\n# Hours.\n\
             session-hours = 3\n"
        );
        // A file with no newline at its end gets one before the new key.
        let (_, text) = changed(&scratch, "[console]\nlisten = 9090", |document| {
            set_string(console(document), "reach", "any");
        });
        assert_eq!(text, "[console]\nlisten = 9090\nreach = \"any\"\n");
    }

    #[test]
    fn a_new_table_goes_after_the_comments_that_end_the_file() {
        let scratch = Scratch::new("table");
        let shipped = "# SPDX header\n\n# [console]\n# reach = \"private\"\n";
        let (wrote, text) =
            changed(&scratch, shipped, |document| set_string(console(document), "reach", "any"));
        assert!(wrote);
        assert_eq!(text, format!("{shipped}\n[console]\nreach = \"any\"\n"));
        // After tables that are there, and the comments that end the file.
        let after_tables = "[sign-in]\nsession-hours = 3\n\n# [console]\n# reach = \"private\"\n";
        let (_, text) = changed(&scratch, after_tables, |document| {
            set_string(console(document), "reach", "any");
        });
        assert_eq!(text, format!("{after_tables}\n[console]\nreach = \"any\"\n"));
        // A file with no comments at its end has the blank line a table has.
        let (_, text) = changed(&scratch, "[sign-in]\nsession-hours = 3\n", |document| {
            set_string(console(document), "reach", "any");
        });
        assert_eq!(text, "[sign-in]\nsession-hours = 3\n\n[console]\nreach = \"any\"\n");
        // A file with nothing in it.
        let (_, text) =
            changed(&scratch, "", |document| set_string(console(document), "reach", "any"));
        assert_eq!(text, "[console]\nreach = \"any\"\n");
    }

    #[test]
    fn a_key_that_is_not_a_table_is_no_table() {
        let mut document: DocumentMut =
            "console = 3\nsign-in = { session-hours = 3 }\n".parse().unwrap();
        assert!(table(&mut document, "console").is_none());
        // An inline table is a table, whose keys the edits set as well.
        set_integer(table(&mut document, "sign-in").unwrap(), "session-hours", 5);
        assert_eq!(document.to_string(), "console = 3\nsign-in = { session-hours = 5 }\n");
    }

    #[test]
    fn a_value_that_is_there_already_writes_nothing_however_the_file_quotes_it() {
        let scratch = Scratch::new("same-value");
        let text = "[console]\nreach = 'any' # quoted\nlisten = 0x2382\n";
        let (wrote, after) = changed(&scratch, text, |document| {
            set_string(console(document), "reach", "any");
            set_integer(console(document), "listen", 9090);
        });
        assert!(!wrote);
        assert_eq!(after, text);
        let (wrote, after) = changed(&scratch, text, |document| {
            set_string(console(document), "reach", "private");
        });
        assert!(wrote);
        assert_eq!(after, text.replace("'any'", "\"private\""));
    }

    #[test]
    fn a_change_that_edits_nothing_writes_nothing_and_makes_no_file() {
        let scratch = Scratch::new("nothing");
        let path = scratch.0.join("settings.toml");
        let odd = "\n\n# top\n  a = 1\t# tab\r\n[ t . u ]\nb=[ 1,2 ,\n 3 ]\n\n\n# end";
        fs::write(&path, odd).unwrap();
        let before = fs::metadata(&path).unwrap().ino();
        let nothing = |_: &mut DocumentMut| Ok::<(), std::convert::Infallible>(());
        assert!(!change(&path, 0o600, nothing).unwrap());
        assert_eq!(fs::read_to_string(&path).unwrap(), odd);
        assert_eq!(fs::metadata(&path).unwrap().ino(), before);
        // A file that is not there stays not there.
        let missing = scratch.0.join("missing.toml");
        assert!(!change(&missing, 0o600, nothing).unwrap());
        assert!(!missing.exists());
        // An empty one stays empty.
        let empty = scratch.0.join("empty.toml");
        fs::write(&empty, "").unwrap();
        assert!(!change(&empty, 0o600, nothing).unwrap());
    }

    #[test]
    fn a_file_that_is_not_there_is_made_with_the_mode_given() {
        let scratch = Scratch::new("made");
        let path = scratch.0.join("settings.toml");
        let wrote = change(&path, 0o600, |document| {
            set_string(document.as_table_mut(), "theme", "light");
            Ok::<(), std::convert::Infallible>(())
        });
        assert!(wrote.unwrap());
        assert_eq!(fs::read_to_string(&path).unwrap(), "theme = \"light\"\n");
        assert_eq!(mode_of(&path), 0o600);
    }

    #[test]
    fn owner_and_mode_are_kept() {
        let scratch = Scratch::new("owner");
        let path = scratch.0.join("admin.toml");
        fs::write(&path, KEPT).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o640)).unwrap();
        let before = fs::metadata(&path).unwrap();
        let wrote = change(&path, 0o600, |document| {
            set_string(console(document), "reach", "any");
            Ok::<(), std::convert::Infallible>(())
        });
        assert!(wrote.unwrap());
        let after = fs::metadata(&path).unwrap();
        assert_eq!(after.mode() & 0o7777, 0o640);
        assert_eq!((after.uid(), after.gid()), (before.uid(), before.gid()));
        assert_ne!(after.ino(), before.ino(), "the new text came by a rename");
    }

    #[test]
    fn a_file_that_does_not_parse_is_refused_and_left_as_it_is() {
        let scratch = Scratch::new("broken");
        let path = scratch.0.join("settings.toml");
        let text = "[console]\nreach = \"private\"\nlisten = \n";
        fs::write(&path, text).unwrap();
        let mut asked = false;
        let refused = change(&path, 0o600, |_| {
            asked = true;
            Ok::<(), std::convert::Infallible>(())
        })
        .unwrap_err();
        assert!(!asked, "the edit is not given a file that is not a document");
        assert_eq!(fs::read_to_string(&path).unwrap(), text);
        let Changed::Parse { line, .. } = &refused else { panic!("{refused:?}") };
        assert_eq!(*line, Some(3));
        let said = refused.to_string();
        assert!(said.starts_with(&format!("{} is not valid TOML, at line 3: ", path.display())));
    }

    #[test]
    fn an_edit_that_refuses_leaves_the_file_as_it_was_and_its_words_name_the_file() {
        let scratch = Scratch::new("refused");
        let path = scratch.0.join("settings.toml");
        fs::write(&path, KEPT).unwrap();
        let refused = change(&path, 0o600, |document| {
            set_string(console(document), "reach", "any");
            Err("the console is not for that")
        })
        .unwrap_err();
        assert_eq!(fs::read_to_string(&path).unwrap(), KEPT);
        assert!(matches!(refused, Changed::Edit { .. }));
        assert_eq!(refused.to_string(), format!("{}: the console is not for that", path.display()));
    }

    #[test]
    fn a_file_that_cannot_be_read_or_written_is_told_in_words_with_its_name() {
        let scratch = Scratch::new("told");
        let nothing = |_: &mut DocumentMut| Ok::<(), std::convert::Infallible>(());
        let read = change(&scratch.0, 0o600, nothing).unwrap_err();
        assert!(matches!(read, Changed::Read { .. }));
        assert_eq!(
            read.to_string(),
            format!("cannot read {}: Is a directory", scratch.0.display())
        );
        let target = scratch.0.join("target.toml");
        fs::write(&target, "a = 1\n").unwrap();
        let link = scratch.0.join("link.toml");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        let write = change(&link, 0o600, |document| {
            document.insert("a", toml_edit::value(2));
            Ok::<(), std::convert::Infallible>(())
        })
        .unwrap_err();
        assert!(matches!(write, Changed::Write { .. }));
        assert_eq!(
            write.to_string(),
            format!("cannot write {}: it is a link, which is not written through", link.display())
        );
        assert_eq!(fs::read_to_string(&target).unwrap(), "a = 1\n");
    }

    #[test]
    fn the_line_of_an_offset_counts_from_one() {
        assert_eq!(line_at("a\nb\nc", 0), 1);
        assert_eq!(line_at("a\nb\nc", 2), 2);
        assert_eq!(line_at("a\nb\nc", 4), 3);
        assert_eq!(line_at("a\nb\nc", 99), 3);
        assert_eq!(line_at("", 0), 1);
    }

    #[test]
    fn the_new_name_is_hidden_beside_the_file() {
        let name = new_name(Path::new("/system/settings/admin.toml")).unwrap();
        assert_eq!(
            name,
            PathBuf::from(format!("/system/settings/.admin.toml.new-{}", std::process::id()))
        );
        assert!(new_name(Path::new("/")).is_err());
    }
}
