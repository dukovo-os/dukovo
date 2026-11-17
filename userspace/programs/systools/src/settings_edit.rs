// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Changing a settings file the way the web console does (ADR 0516).
//!
//! [`change`] reads the file as a TOML document, lets the caller edit it,
//! and writes it back with [`write`], so that every comment, blank line and
//! order of keys the file had stays, and a change that leaves the text as
//! it was writes nothing: the journal and a second run of the same change
//! see no change. [`edited`] is its part that works on text, without a
//! file. [`table`], [`set_string`], [`set_integer`] and [`set_bool`] are the
//! edits the console makes, which keep the comment after a value they replace and
//! put a new key where a person would. [`set_strings`], [`push_string`],
//! [`remove_string`] and [`remove_key`] edit a list of words, and
//! [`push_table`] and [`remove_table`] the `[[tables]]` of one, and
//! [`take_table`] takes out a `[table]`.
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

use toml_edit::{Array, ArrayOfTables, DocumentMut, Item, Table, TableLike, Value};

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
    let made = edited(existing.as_deref().unwrap_or_default(), edit).map_err(|error| {
        let path = path.to_path_buf();
        match error {
            Unedited::Parse { line, message } => Changed::Parse { path, line, message },
            Unedited::Edit(error) => Changed::Edit { path, error },
        }
    })?;
    let Some(new) = made else { return Ok(false) };
    write(path, new.as_bytes(), new_mode)
        .map_err(|error| Changed::Write { path: path.to_path_buf(), error })
}

/// Why [`edited`] made no text.
#[derive(Debug)]
pub enum Unedited<E> {
    /// The text is not TOML.
    Parse {
        /// The line the parser stopped at, when it says.
        line: Option<usize>,
        /// What the parser said was wrong.
        message: String,
    },
    /// The edit refused: its own error.
    Edit(E),
}

/// The part of [`change`] that works on text: reads `text` as a TOML
/// document, lets `edit` change it, and gives the text the document makes,
/// or None when the edit changed nothing.
///
/// # Errors
///
/// [`Unedited`]: the text is not TOML, or the edit refused.
pub fn edited<E>(
    text: &str,
    edit: impl FnOnce(&mut DocumentMut) -> Result<(), E>,
) -> Result<Option<String>, Unedited<E>> {
    let mut document: DocumentMut = text.parse().map_err(|error: toml_edit::TomlError| {
        let line = error.span().map(|span| line_at(text, span.start));
        Unedited::Parse { line, message: error.message().to_string() }
    })?;
    // The text the document makes before the edit, which is not always the
    // file's: the parser drops a carriage return that follows a comment.
    // An edit that changes nothing is told by it, so that it writes
    // nothing, and makes no file of one that is not there.
    let unedited = document.to_string();
    edit(&mut document).map_err(Unedited::Edit)?;
    let new = document.to_string();
    Ok((new != unedited).then_some(new))
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

/// [`set_string`] for a switch: `true` or `false`.
pub fn set_bool(table: &mut dyn TableLike, key: &str, value: bool) {
    if table.get(key).and_then(Item::as_bool) != Some(value) {
        put(table, key, Value::from(value));
    }
}

/// Makes `key` of `table` the list of strings `values`, keeping the comment
/// after the list it replaces. A list that holds them already is left as
/// the file wrote it.
pub fn set_strings(table: &mut dyn TableLike, key: &str, values: &[String]) {
    let same = table.get(key).and_then(Item::as_array).is_some_and(|array| {
        array.len() == values.len()
            && array.iter().zip(values).all(|(old, new)| old.as_str() == Some(new))
    });
    if !same {
        put(table, key, Value::Array(values.iter().map(String::as_str).collect()));
    }
}

/// Adds the string `value` to the end of the list `key` of `table`,
/// making the list when there is none. Returns false, having changed
/// nothing, when the list holds it already, and when `key` holds
/// something that is not a list of strings.
pub fn push_string(table: &mut dyn TableLike, key: &str, value: &str) -> bool {
    if table.get(key).is_none() {
        table.insert(key, Item::Value(Value::Array(Array::new())));
    }
    let Some(array) = table.get_mut(key).and_then(Item::as_array_mut) else { return false };
    if array.iter().any(|old| old.as_str() == Some(value)) {
        return false;
    }
    array.push(value);
    true
}

/// Takes the string `value` out of the list `key` of `table`. Returns
/// false, having changed nothing, when the list does not hold it.
pub fn remove_string(table: &mut dyn TableLike, key: &str, value: &str) -> bool {
    let Some(array) = table.get_mut(key).and_then(Item::as_array_mut) else { return false };
    let Some(at) = array.iter().position(|old| old.as_str() == Some(value)) else { return false };
    array.remove(at);
    // The one that comes first keeps no space before it, as a list written
    // by hand has none after its bracket; a long list keeps its lines.
    if at == 0
        && let Some(first) = array.get_mut(0)
        && first.decor().prefix().and_then(|prefix| prefix.as_str()) == Some(" ")
    {
        first.decor_mut().set_prefix("");
    }
    true
}

/// Takes `key` out of `table`, with the comment that is on its line.
/// Returns false when it is not there.
pub fn remove_key(table: &mut dyn TableLike, key: &str) -> bool {
    table.remove(key).is_some()
}

/// Adds a table to the `[[name]]` tables of `document`, after the others,
/// and gives it. The comments that end the file go above the first one,
/// as [`table`] puts them above a table of its own. None when `name` is
/// a key that holds something other than `[[tables]]`.
pub fn push_table<'a>(document: &'a mut DocumentMut, name: &str) -> Option<&'a mut Table> {
    let first = !document.contains_key(name);
    let mut made = Table::new();
    let before = document.trailing().as_str().unwrap_or_default().to_string();
    let anything = !document.as_table().is_empty() || !before.is_empty();
    if first {
        let mut prefix = before;
        if !prefix.is_empty() {
            document.set_trailing("");
            if !prefix.ends_with("\n\n") {
                prefix.push_str(if prefix.ends_with('\n') { "\n" } else { "\n\n" });
            }
        } else if anything {
            prefix.push('\n');
        }
        made.decor_mut().set_prefix(prefix);
        document.insert(name, Item::ArrayOfTables(ArrayOfTables::new()));
    } else {
        made.decor_mut().set_prefix("\n");
    }
    let tables = document.get_mut(name)?.as_array_of_tables_mut()?;
    tables.push(made);
    let last = tables.len() - 1;
    tables.get_mut(last)
}

/// Takes out the first of the `[[name]]` tables of `document` that
/// `which` picks, with the comments above it. When it was the last one,
/// those comments stay at the end of the file, where they were before the
/// first was added. When it was the first of several, and the first thing
/// in the file, the comments above it are the file's own (the ones
/// [`push_table`] put above the first it made), and stay above the table
/// that is first now. Returns false when none is picked.
pub fn remove_table(
    document: &mut DocumentMut,
    name: &str,
    which: impl Fn(&Table) -> bool,
) -> bool {
    let first_in_file = document.iter().next().is_some_and(|(key, _)| key == name);
    let Some(tables) = document.get_mut(name).and_then(Item::as_array_of_tables_mut) else {
        return false;
    };
    let Some(at) = tables.iter().position(which) else { return false };
    let prefix = tables
        .get(at)
        .and_then(|table| table.decor().prefix())
        .and_then(|prefix| prefix.as_str())
        .unwrap_or_default()
        .to_string();
    tables.remove(at);
    if !tables.is_empty() {
        if first_in_file
            && at == 0
            && !prefix.trim().is_empty()
            && let Some(next) = tables.get_mut(0)
        {
            let own = next.decor().prefix().and_then(|own| own.as_str()).unwrap_or_default();
            let kept = format!("{}\n\n", prefix.trim_end_matches('\n'));
            let above = if own.trim().is_empty() { kept } else { format!("{kept}{own}") };
            next.decor_mut().set_prefix(above);
        }
        return true;
    }
    document.remove(name);
    // What `push_table` put above the table: the comments, and the
    // blank line it adds before the table that follows them.
    if !prefix.trim().is_empty() {
        let after = document.trailing().as_str().unwrap_or_default().to_string();
        document.set_trailing(format!("{}\n{after}", prefix.trim_end_matches('\n')));
    }
    true
}

/// Takes out the table `name` of `document`, with the comments above it.
/// When it is the first table, the comments above it are the file's own
/// (the ones [`table`] left above the first table it made), and they stay,
/// above the next table or at the end of the file. Returns false when
/// there is no table of that name.
pub fn take_table(document: &mut DocumentMut, name: &str) -> bool {
    let Some(table) = document.get(name).and_then(Item::as_table) else { return false };
    let prefix =
        table.decor().prefix().and_then(|prefix| prefix.as_str()).unwrap_or_default().to_string();
    let first = document.iter().next().is_some_and(|(key, _)| key == name);
    document.remove(name);
    if !first || prefix.trim().is_empty() {
        return true;
    }
    let kept = format!("{}\n\n", prefix.trim_end_matches('\n'));
    let next = document.iter_mut().next().map(|(_, item)| item);
    let above = match next {
        Some(Item::Table(table)) => Some(table.decor_mut()),
        Some(Item::ArrayOfTables(tables)) => tables.get_mut(0).map(Table::decor_mut),
        _ => None,
    };
    match above {
        // Where the table had a blank line above it, this one has the
        // file's comments and one.
        Some(decor)
            if decor.prefix().and_then(|p| p.as_str()).is_none_or(|p| p.trim().is_empty()) =>
        {
            decor.set_prefix(kept);
        }
        Some(decor) => {
            let own = decor.prefix().and_then(|p| p.as_str()).unwrap_or_default().to_string();
            decor.set_prefix(format!("{kept}{own}"));
        }
        None => {
            let after = document.trailing().as_str().unwrap_or_default().to_string();
            document.set_trailing(format!("{}\n{after}", prefix.trim_end_matches('\n')));
        }
    }
    true
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
    fn a_switch_is_set_in_place_and_one_that_is_set_already_writes_nothing() {
        let scratch = Scratch::new("switch");
        let text = "[terminal]\nannounce = false # off\nshortcuts = true\n";
        let (wrote, after) = changed(&scratch, text, |document| {
            set_bool(table(document, "terminal").unwrap(), "announce", true);
        });
        assert!(wrote);
        assert_eq!(after, text.replace("announce = false", "announce = true"));
        let (wrote, after) = changed(&scratch, text, |document| {
            let terminal = table(document, "terminal").unwrap();
            set_bool(terminal, "announce", false);
            set_bool(terminal, "shortcuts", true);
        });
        assert!(!wrote);
        assert_eq!(after, text);
        // A key that is not there goes to the end of its table, and one of
        // another type is replaced by the switch.
        let (wrote, after) = changed(&scratch, "[terminal]\nannounce = \"yes\"\n", |document| {
            let terminal = table(document, "terminal").unwrap();
            set_bool(terminal, "announce", false);
            set_bool(terminal, "copy-on-select", true);
        });
        assert!(wrote);
        assert_eq!(after, "[terminal]\nannounce = false\ncopy-on-select = true\n");
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
    fn a_text_is_edited_in_memory_as_a_file_is() {
        let none = |_: &mut DocumentMut| Ok::<(), std::convert::Infallible>(());
        let edited_text = edited(KEPT, |document| {
            set_string(console(document), "reach", "any");
            Ok::<(), std::convert::Infallible>(())
        })
        .unwrap();
        let wanted = KEPT.replace("reach = \"private\" # for now", "reach = \"any\" # for now");
        assert_eq!(edited_text.as_deref(), Some(wanted.as_str()));
        // An edit that changes nothing gives no text, and neither does the
        // empty text that nothing is added to.
        assert_eq!(edited(KEPT, none).unwrap(), None);
        assert_eq!(edited("", none).unwrap(), None);
        // A text that is not a document is not handed to the edit.
        let broken = edited("[console]\nlisten = \n", |_| -> Result<(), &str> {
            panic!("the edit is asked")
        });
        assert!(matches!(broken, Err(Unedited::Parse { line: Some(2), .. })));
        let refused = edited(KEPT, |_| Err("no"));
        assert!(matches!(refused, Err(Unedited::Edit("no"))));
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

    /// What `edit` makes of `text`, which it changes and cannot fail.
    fn made(text: &str, edit: impl FnOnce(&mut DocumentMut)) -> String {
        let made = edited(text, |document| {
            edit(document);
            Ok::<(), std::convert::Infallible>(())
        })
        .unwrap_or_else(|_| unreachable!());
        made.unwrap_or_else(|| text.to_string())
    }

    #[test]
    fn a_list_of_words_gains_and_loses_one_and_keeps_what_is_around_it() {
        let text = "# Whom.\nessential = [\"logd\", \"sshd\"] # the minimum\nstable = 120\n";
        let more = made(text, |d| assert!(push_string(d.as_table_mut(), "essential", "nginx")));
        assert_eq!(
            more,
            "# Whom.\nessential = [\"logd\", \"sshd\", \"nginx\"] # the minimum\nstable = 120\n"
        );
        let less = made(&more, |d| assert!(remove_string(d.as_table_mut(), "essential", "logd")));
        assert_eq!(
            less,
            "# Whom.\nessential = [\"sshd\", \"nginx\"] # the minimum\nstable = 120\n"
        );
        let last = made(&less, |d| assert!(remove_string(d.as_table_mut(), "essential", "nginx")));
        assert_eq!(last, "# Whom.\nessential = [\"sshd\"] # the minimum\nstable = 120\n");
        // One that is there already, or not there, changes nothing.
        let same = made(text, |d| {
            assert!(!push_string(d.as_table_mut(), "essential", "sshd"));
            assert!(!remove_string(d.as_table_mut(), "essential", "nginx"));
            assert!(!remove_string(d.as_table_mut(), "other", "nginx"));
        });
        assert_eq!(same, text);
        // A list that is not there is made, and a key that is no list is left alone.
        assert_eq!(
            made("", |d| assert!(push_string(d.as_table_mut(), "essential", "logd"))),
            "essential = [\"logd\"]\n"
        );
        made("essential = 3\n", |d| assert!(!push_string(d.as_table_mut(), "essential", "x")));
    }

    #[test]
    fn a_list_is_set_whole_and_a_key_is_taken_out_with_its_comment() {
        let words = ["a".to_string(), "b".to_string()];
        let set = made("x = 1\n", |d| set_strings(d.as_table_mut(), "list", &words));
        assert_eq!(set, "x = 1\nlist = [\"a\", \"b\"]\n");
        // The same list, as the file wrote it, is not written again.
        let spaced = "list = [ \"a\",\"b\" ]\n";
        assert_eq!(made(spaced, |d| set_strings(d.as_table_mut(), "list", &words)), spaced);
        let text = "a = 1 # one\nb = 2 # two\n";
        let less = made(text, |d| assert!(remove_key(d.as_table_mut(), "a")));
        assert_eq!(less, "b = 2 # two\n");
        made(text, |d| assert!(!remove_key(d.as_table_mut(), "c")));
    }

    #[test]
    fn a_table_of_a_list_comes_after_the_comments_and_goes_with_the_ones_above_it() {
        let shipped = "stable = 120\n\n# Checks of your own.\n# [[check]]\n# name = \"x\"\n";
        let one = made(shipped, |d| {
            push_table(d, "check").unwrap().insert("name", Item::Value(Value::from("a")));
        });
        assert_eq!(
            one,
            "stable = 120\n\n# Checks of your own.\n# [[check]]\n# name = \"x\"\n\n[[check]]\nname = \"a\"\n"
        );
        let two = made(&one, |d| {
            push_table(d, "check").unwrap().insert("name", Item::Value(Value::from("b")));
        });
        assert!(two.ends_with("name = \"a\"\n\n[[check]]\nname = \"b\"\n"), "{two}");
        // The last takes the comments back to the end of the file.
        let back = made(&two, |d| {
            assert!(remove_table(d, "check", |t| t.get("name").and_then(Item::as_str) == Some("a")));
        });
        assert!(back.contains("[[check]]\nname = \"b\"") && !back.contains("name = \"a\""));
        let gone = made(&one, |d| assert!(remove_table(d, "check", |_| true)));
        assert_eq!(gone, shipped);
        // The first of two takes the file's comments to the second.
        let header = "# The header.\n\n# Checks of your own.\n";
        let pair = made(header, |d| {
            push_table(d, "check").unwrap().insert("name", Item::Value(Value::from("a")));
            push_table(d, "check").unwrap().insert("name", Item::Value(Value::from("b")));
        });
        assert_eq!(pair, format!("{header}\n[[check]]\nname = \"a\"\n\n[[check]]\nname = \"b\"\n"));
        let after = made(&pair, |d| {
            assert!(remove_table(d, "check", |t| t.get("name").and_then(Item::as_str) == Some("a")));
        });
        assert_eq!(after, format!("{header}\n[[check]]\nname = \"b\"\n"));
        let last = made(&after, |d| assert!(remove_table(d, "check", |_| true)));
        assert_eq!(last, header);
        made(&one, |d| assert!(!remove_table(d, "check", |_| false)));
        made(&one, |d| assert!(!remove_table(d, "other", |_| true)));
        // A table in a file with nothing else, and a key that holds a value.
        let alone = made("", |d| {
            push_table(d, "check").unwrap().insert("name", Item::Value(Value::from("a")));
        });
        assert_eq!(alone, "[[check]]\nname = \"a\"\n");
        made("check = 3\n", |d| assert!(push_table(d, "check").is_none()));
    }

    #[test]
    fn a_table_goes_with_its_comments_and_the_files_own_comments_stay() {
        let shipped = "# SPDX header\n\n# Where alerts go.\n# [email]\n# to = []\n";
        let set = |d: &mut DocumentMut| {
            set_string(table(d, "email").unwrap(), "server", "smtp.example.com");
        };
        let one = made(shipped, set);
        assert_eq!(one, format!("{shipped}\n[email]\nserver = \"smtp.example.com\"\n"));
        // The table alone: the file is as it was.
        assert_eq!(made(&one, |d| assert!(take_table(d, "email"))), shipped);
        // With a table of a list after it, the comments go above that.
        let two = made(&one, |d| {
            push_table(d, "webhook").unwrap().insert("kind", Item::Value(Value::from("ntfy")));
        });
        let gone = made(&two, |d| assert!(take_table(d, "email")));
        assert_eq!(gone, format!("{shipped}\n[[webhook]]\nkind = \"ntfy\"\n"));
        // A table that is not the first takes its own comments with it.
        let late = "[[webhook]]\nkind = \"ntfy\"\n\n# The mail.\n[email]\nserver = \"s\"\n";
        assert_eq!(
            made(late, |d| assert!(take_table(d, "email"))),
            "[[webhook]]\nkind = \"ntfy\"\n"
        );
        // Nothing to take, and a key that is not a table.
        made(&one, |d| assert!(!take_table(d, "console")));
        made("email = 3\n", |d| assert!(!take_table(d, "email")));
    }
}
