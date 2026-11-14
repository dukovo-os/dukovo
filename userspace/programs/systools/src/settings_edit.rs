// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Writing a settings file whole, the way the web console changes one
//! (ADR 0516): the new text goes into a new file beside it, which gets the
//! old file's owner and mode and is made durable, and then takes the old
//! file's name in one rename, so that a reader sees the old file or the
//! new one and never a part of either, and a power cut leaves one of the
//! two. A write that would leave the file as it was writes nothing, so
//! that the journal and a second run of the same change see no change.

use std::ffi::OsString;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::os::unix::ffi::OsStringExt;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

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
