// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The run's private folder: the program's `/tmp`, the `/etc` files
//! `linux run` writes, and the mounts file linuxd reads, in a folder of
//! the person's own `/tmp` that no one else may enter, made for the run
//! and removed with it. The program's `/dev/shm` is a memory filesystem
//! of the run's own (`view.rs`).
//!
//! Beneath it are `tmp`, `etc` and `mounts`; the view attaches the first
//! as a folder and each file of `etc` by itself, so the program finds
//! none of the others in its `/tmp`. When a run ends with its folder
//! still in `/tmp` (`linux` was killed), it is the person's to remove: its
//! name has the number of the run's process.

use std::fs;
use std::io;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use super::etc;
use crate::bundles::folder::Dir;
use crate::bundles::run::{folder_of, own_folder};

/// Where the person's `/tmp` is, which the private folder is made in.
pub const TMP: &str = "/tmp";

/// A folder every program of the person may write in and none may remove
/// from another's, as Linux's `/tmp` is.
const OPEN_FOLDER: u32 = 0o1777;

/// The folder made for a run, which is removed when this is dropped.
pub struct Private {
    path: String,
}

impl Private {
    /// Makes the folder for the run of process `number` in `parent`, with
    /// the folder of the program's `/tmp`, and the one that holds the
    /// `/etc` files.
    ///
    /// # Errors
    ///
    /// If the folder is there already and not the person's, or any of them
    /// cannot be made.
    pub fn make(parent: &str, uid: u32, number: u32) -> Result<Private, String> {
        let path = format!("{}/.linux-{number}", parent.trim_end_matches('/'));
        // Removed by whatever left it, if it is a stale one of the same
        // number: a number is reused, and nothing of a run is kept.
        if fs::symlink_metadata(&path).is_ok() {
            remove(&path);
        }
        own_folder(&path, uid)?;
        // The value is built before the folders so that a failure on the
        // way removes what was made.
        let private = Private { path };
        for name in ["etc", "tmp"] {
            own_folder(&private.at(name), uid)?;
        }
        let tmp = private.at("tmp");
        fs::set_permissions(&tmp, fs::Permissions::from_mode(OPEN_FOLDER))
            .map_err(|e| format!("{tmp}: {e}"))?;
        Ok(private)
    }

    /// The folder's path.
    pub fn path(&self) -> &str {
        &self.path
    }

    /// The path of `name` in it.
    pub fn at(&self, name: &str) -> String {
        format!("{}/{name}", self.path)
    }

    /// Writes `text` as the file `name` of the program's `/etc`, which it
    /// may read and not change.
    ///
    /// # Errors
    ///
    /// If it cannot be written.
    pub fn write_etc(&self, name: &str, text: &str) -> Result<(), String> {
        self.write(&format!("etc/{name}"), text, 0o444)
    }

    /// Writes `text` as `name` in the folder, with `mode`.
    ///
    /// # Errors
    ///
    /// If it cannot be written.
    pub fn write(&self, name: &str, text: &str, mode: u32) -> Result<(), String> {
        let file = self.at(name);
        fs::write(&file, text).map_err(|e| format!("{file}: {e}"))?;
        fs::set_permissions(&file, fs::Permissions::from_mode(mode))
            .map_err(|e| format!("{file}: {e}"))
    }
}

impl Drop for Private {
    fn drop(&mut self) {
        remove(&self.path);
    }
}

/// Removes the folder `path` and all that is in it, as far as it can: the
/// person who ran the program owns all of it.
fn remove(path: &str) {
    let path = Path::new(path);
    let (Some(parent), Some(name)) = (path.parent(), path.file_name().and_then(|n| n.to_str()))
    else {
        return;
    };
    let Some(parent) = parent.to_str().and_then(|p| Dir::open(p).ok()) else { return };
    // A link or a file of the same name is only unlinked.
    if parent.remove_tree(name, 0).is_err() {
        let _ = parent.unlink(name);
    }
}

/// The machine ID kept in `store`: the file's text if it is one, else a
/// new one made of `random` and kept there, so that it stays the same from
/// one run to the next and one program to another (`machine-id(5)`).
///
/// # Errors
///
/// If there is none and a new one cannot be made or kept.
pub fn machine_id(
    store: &Path,
    random: impl FnOnce() -> io::Result<[u8; 16]>,
) -> Result<String, String> {
    if let Ok(text) = fs::read_to_string(store)
        && etc::is_machine_id(&text)
    {
        return Ok(text);
    }
    let text = etc::machine_id(&random().map_err(|e| format!("no random numbers: {e}"))?);
    crate::util::replace_whole(store, &text, 0o600)
        .map_err(|e| format!("{}: {}", store.display(), crate::util::strip_os_error(&e)))?;
    Ok(text)
}

/// The folder of the person's Linux programs' data, made with its parent
/// beneath `home`: `app-data/linux`, which keeps the machine ID, and each
/// program's own folder when `linux run` runs it.
///
/// # Errors
///
/// If a folder is there and not the person's, or cannot be made.
pub fn linux_folder(home: &str, uid: u32) -> Result<PathBuf, String> {
    let mut path = PathBuf::from(home);
    for part in [crate::bundled::APP_DATA, "linux"] {
        path.push(part);
        own_data_folder(&path, uid)?;
    }
    Ok(path)
}

/// The folder of the program's own data, made with its parents beneath
/// `home`: `app-data/linux/NAME`.
///
/// # Errors
///
/// If a folder is there and not the person's, or cannot be made.
pub fn data_folder(home: &str, uid: u32, name: &str) -> Result<PathBuf, String> {
    let path = linux_folder(home, uid)?.join(name);
    own_data_folder(&path, uid)?;
    Ok(path)
}

/// Makes the folder `path` the person `uid`'s, as [`folder_of`] does.
fn own_data_folder(path: &Path, uid: u32) -> Result<(), String> {
    let text = path.to_str().ok_or_else(|| format!("{}: not text", path.display()))?;
    folder_of(text, uid, 0o700)
}

/// The name of a program's data folder: the file's name, with what is not
/// a letter, a digit or one of `._-+` made `_`, and no leading dot, so that
/// the folder is never `.` or `..`, hidden, or the machine ID's file beside
/// the programs' folders.
pub fn data_name(file_name: &str) -> String {
    let name: String = file_name
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || "._-+".contains(c) { c } else { '_' })
        .collect();
    let name = name.trim_start_matches('.');
    if name.is_empty() { "program".into() } else { name.chars().take(64).collect() }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::MetadataExt;
    use sysapi::process::uid;

    /// A folder of the test's own, which the test removes.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new(what: &str) -> Scratch {
            let path =
                std::env::temp_dir().join(format!("linux-run-{what}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&path);
            fs::create_dir_all(&path).unwrap();
            Scratch(path)
        }

        fn text(&self) -> &str {
            self.0.to_str().unwrap()
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn the_private_folder_holds_tmp_and_etc_and_is_gone_when_dropped() {
        let scratch = Scratch::new("private");
        let private = Private::make(scratch.text(), uid(), 4242).unwrap();
        let path = PathBuf::from(private.path());
        assert_eq!(path, scratch.0.join(".linux-4242"));
        for (name, mode) in [("", 0o700), ("etc", 0o700), ("tmp", 0o1777)] {
            let meta = fs::metadata(path.join(name)).unwrap();
            assert!(meta.is_dir(), "{name}");
            assert_eq!(meta.mode() & 0o7777, mode, "{name}");
        }
        private.write_etc("passwd", "root:x:0:0:root:/root:/bin/sh\n").unwrap();
        private.write("mounts", "proc /proc proc rw 0 0\n", 0o600).unwrap();
        fs::write(path.join("tmp/left-by-the-program"), "x").unwrap();
        let passwd = fs::metadata(path.join("etc/passwd")).unwrap();
        assert_eq!(passwd.mode() & 0o7777, 0o444);
        assert_eq!(fs::read_to_string(path.join("mounts")).unwrap(), "proc /proc proc rw 0 0\n");
        drop(private);
        assert!(!path.exists());
    }

    #[test]
    fn a_stale_folder_of_the_same_number_is_replaced() {
        let scratch = Scratch::new("stale");
        fs::create_dir_all(scratch.0.join(".linux-77/tmp")).unwrap();
        fs::write(scratch.0.join(".linux-77/tmp/old"), "x").unwrap();
        let private = Private::make(scratch.text(), uid(), 77).unwrap();
        assert!(!scratch.0.join(".linux-77/tmp/old").exists());
        drop(private);
        assert!(!scratch.0.join(".linux-77").exists());
    }

    #[test]
    fn a_machine_id_is_made_once_and_kept() {
        let scratch = Scratch::new("machine-id");
        let store = scratch.0.join(".machine-id");
        let fresh = || Ok([0xab; 16]);
        let first = machine_id(&store, fresh).unwrap();
        assert_eq!(first, "abababababababababababababababab\n");
        assert_eq!(fs::read_to_string(&store).unwrap(), first);
        // The next run reads it and asks for no random numbers.
        let again = machine_id(&store, || panic!("asked for random numbers")).unwrap();
        assert_eq!(again, first);
        // Something else in its place is replaced.
        fs::write(&store, "not an id").unwrap();
        assert_eq!(
            machine_id(&store, || Ok([0x01; 16])).unwrap(),
            "01010101010101010101010101010101\n"
        );
    }

    #[test]
    fn no_random_numbers_is_a_failure_that_says_so() {
        let scratch = Scratch::new("no-random");
        let store = scratch.0.join(".machine-id");
        let why = machine_id(&store, || Err(io::Error::other("nothing"))).unwrap_err();
        assert_eq!(why, "no random numbers: nothing");
        assert!(!store.exists());
    }

    #[test]
    fn the_data_folder_and_its_parents_are_made_for_the_person() {
        let scratch = Scratch::new("data");
        let folder = data_folder(scratch.text(), uid(), "hello").unwrap();
        assert_eq!(folder, scratch.0.join("app-data/linux/hello"));
        assert!(folder.is_dir());
        // Again, with the folders there.
        assert_eq!(data_folder(scratch.text(), uid(), "hello").unwrap(), folder);
    }

    #[test]
    fn the_machine_id_has_its_folder_before_any_program_has_one() {
        // A bundle's program keeps its data in the bundle's folder: the
        // machine ID may be the first thing in app-data/linux.
        let scratch = Scratch::new("linux-folder");
        let folder = linux_folder(scratch.text(), uid()).unwrap();
        assert_eq!(folder, scratch.0.join("app-data/linux"));
        let id = machine_id(&folder.join(".machine-id"), || Ok([0x5e; 16])).unwrap();
        assert_eq!(fs::read_to_string(folder.join(".machine-id")).unwrap(), id);
        // Again, with the folder there.
        assert_eq!(linux_folder(scratch.text(), uid()).unwrap(), folder);
    }

    #[test]
    fn a_data_folder_that_is_a_file_is_refused() {
        let scratch = Scratch::new("data-file");
        fs::create_dir_all(scratch.0.join("app-data/linux")).unwrap();
        fs::write(scratch.0.join("app-data/linux/hello"), "x").unwrap();
        assert!(data_folder(scratch.text(), uid(), "hello").is_err());
    }

    #[test]
    fn a_data_name_is_a_plain_name() {
        for (file, name) in [
            ("hello", "hello"),
            ("python3.12", "python3.12"),
            ("my tool", "my_tool"),
            ("a/b", "a_b"),
            (".hidden", "hidden"),
            ("..", "program"),
            (".", "program"),
            ("", "program"),
            ("naïve", "na_ve"),
        ] {
            assert_eq!(data_name(file), name, "{file:?}");
        }
        assert_eq!(data_name(&"x".repeat(100)).len(), 64);
    }
}
