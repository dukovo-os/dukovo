// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! What the receivers heard, in files the boot test reads with `grep`:
//! one block for each message or request, in the order they came.

use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};

/// The messages the mail server took.
pub const MAIL: &str = "mail.txt";

/// The requests the webhook receiver got.
pub const HOOKS: &str = "hooks.txt";

/// Made once every receiver listens, so that the test knows when to
/// begin.
pub const READY: &str = "ready";

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
            fs::write(folder.join(name), "")?;
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
        let mut file = OpenOptions::new().append(true).create(true).open(self.folder.join(name))?;
        file.write_all(block.as_bytes())
    }

    /// Says that every receiver listens.
    ///
    /// # Errors
    ///
    /// Why the file could not be made.
    pub fn ready(&self) -> io::Result<()> {
        fs::write(self.folder.join(READY), "ready\n")
    }
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
