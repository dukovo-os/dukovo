// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! What the receivers heard, in files the boot test reads with `grep`:
//! one block for each message or request, in the order they came.

use std::fs::{self, File, OpenOptions, Permissions};
use std::io::{self, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};

/// The messages the mail server took.
pub const MAIL: &str = "mail.txt";

/// The requests the webhook receiver got.
pub const HOOKS: &str = "hooks.txt";

/// Made once every receiver listens, so that the test knows when to
/// begin.
pub const READY: &str = "ready";

/// Who may read the files: everyone, whatever umask the service runs
/// with, as the test reads them as a user.
const MODE: u32 = 0o644;

/// The files in a folder.
pub struct Records {
    folder: PathBuf,
    /// One block is written whole before the next one starts.
    writing: Mutex<()>,
}

impl Records {
    /// Records in `folder`, which has [`MAIL`] and [`HOOKS`] made empty
    /// and no [`READY`].
    ///
    /// # Errors
    ///
    /// Why a file could not be made: the folder is not there, or not
    /// writable.
    pub fn start(folder: &Path) -> io::Result<Records> {
        for name in [MAIL, HOOKS] {
            create(&folder.join(name), true)?;
        }
        match fs::remove_file(folder.join(READY)) {
            Err(err) if err.kind() != io::ErrorKind::NotFound => return Err(err),
            _ => {}
        }
        Ok(Records { folder: folder.to_path_buf(), writing: Mutex::new(()) })
    }

    /// Adds `block` to the end of the file `name`.
    ///
    /// # Errors
    ///
    /// Why it could not be written.
    pub fn add(&self, name: &str, block: &str) -> io::Result<()> {
        let _turn = self.writing.lock().unwrap_or_else(PoisonError::into_inner);
        let mut file = create(&self.folder.join(name), false)?;
        file.write_all(block.as_bytes())
    }

    /// Says that every receiver listens.
    ///
    /// # Errors
    ///
    /// Why the file could not be made.
    pub fn ready(&self) -> io::Result<()> {
        let mut file = create(&self.folder.join(READY), true)?;
        file.write_all(b"ready\n")
    }
}

/// Opens `path` to be written at its end, made if it is not there and
/// emptied first when `empty`, readable by everyone.
fn create(path: &Path, empty: bool) -> io::Result<File> {
    let file =
        OpenOptions::new().append(!empty).write(empty).truncate(empty).create(true).open(path)?;
    file.set_permissions(Permissions::from_mode(MODE))?;
    Ok(file)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn folder(name: &str) -> PathBuf {
        let path =
            std::env::temp_dir().join(format!("monitoring-test-{name}-{}", std::process::id()));
        fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn blocks_are_added_in_order_to_files_made_empty() {
        let dir = folder("records");
        fs::write(dir.join(MAIL), "old\n").unwrap();
        let records = Records::start(&dir).unwrap();
        assert_eq!(fs::read_to_string(dir.join(MAIL)).unwrap(), "");
        records.add(MAIL, "one\n").unwrap();
        records.add(MAIL, "two\n").unwrap();
        assert_eq!(fs::read_to_string(dir.join(MAIL)).unwrap(), "one\ntwo\n");
        assert_eq!(fs::read_to_string(dir.join(HOOKS)).unwrap(), "");
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn the_files_are_readable_by_everyone_for_the_test_reads_them_as_a_user() {
        use std::os::unix::fs::MetadataExt;
        let dir = folder("mode");
        let records = Records::start(&dir).unwrap();
        records.add(HOOKS, "x\n").unwrap();
        records.ready().unwrap();
        for name in [MAIL, HOOKS, READY] {
            assert_eq!(fs::metadata(dir.join(name)).unwrap().mode() & 0o777, 0o644, "{name}");
        }
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn ready_is_made_when_said_and_not_before() {
        let dir = folder("ready");
        let records = Records::start(&dir).unwrap();
        assert!(!dir.join(READY).exists());
        records.ready().unwrap();
        assert!(dir.join(READY).exists());
        // A new start forgets it.
        Records::start(&dir).unwrap();
        assert!(!dir.join(READY).exists());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_folder_that_is_not_there_is_an_error() {
        assert!(Records::start(Path::new("/nonexistent/monitoring")).is_err());
    }
}
