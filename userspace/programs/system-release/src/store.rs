// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! A store of a release's files, each kept once under its SHA-256 as
//! `files/<sha256>`, as the update server keeps them (the updates
//! design's section 4, ADR 0308). Two releases share every file they have
//! in common, and a file is found by what it holds, never by its name, so
//! a list from one release can be checked against a store another filled.
//!
//! A file whose bytes are not what its name says is a problem, named by
//! [`Store::check`] with the path the release gives it: that is where to
//! look when a stick or a download is damaged.

use std::io::Read as _;
use std::path::{Path, PathBuf};

use crate::list::{Release, hex};

/// A store, at a directory of `<sha256>` files.
pub struct Store {
    dir: PathBuf,
}

/// What [`Store::check`] found in a store that holds a release whole.
#[derive(Debug, PartialEq, Eq)]
pub struct Checked {
    /// How many files the release has.
    pub files: usize,
    /// Their total size in bytes.
    pub bytes: u64,
}

impl Store {
    /// The store at `dir`.
    #[must_use]
    pub fn at(dir: &Path) -> Store {
        Store { dir: dir.to_path_buf() }
    }

    /// Where the file with the SHA-256 `sha256` is kept.
    #[must_use]
    pub fn path(&self, sha256: &str) -> PathBuf {
        self.dir.join(sha256)
    }

    /// Copies each file of `release` from the tree at `tree` into the
    /// store, unless it holds one of that hash already, each checked
    /// against the list as it is copied. Returns how many files were
    /// added.
    ///
    /// # Errors
    ///
    /// If a file cannot be read or written, or the tree's file is not
    /// what the list says.
    pub fn add_tree(&self, tree: &Path, release: &Release) -> Result<usize, String> {
        std::fs::create_dir_all(&self.dir)
            .map_err(|err| format!("{}: {err}", self.dir.display()))?;
        let mut added = 0;
        for entry in &release.files {
            let Some((sha256, size)) = entry.file() else { continue };
            let to = self.path(sha256);
            if to.exists() {
                continue;
            }
            let from = tree.join(&entry.path);
            let bytes = std::fs::read(&from).map_err(|err| format!("{}: {err}", from.display()))?;
            let actual = hex(ring::digest::digest(&ring::digest::SHA256, &bytes).as_ref());
            if bytes.len() as u64 != size || actual != sha256 {
                return Err(format!("{}: changed since it was listed", from.display()));
            }
            // Written whole, then renamed into place, so that a store a
            // cut-short copy left holds no file of the wrong bytes.
            let partial = self.dir.join(format!(".{sha256}.partial"));
            std::fs::write(&partial, &bytes)
                .map_err(|err| format!("{}: {err}", partial.display()))?;
            std::fs::rename(&partial, &to).map_err(|err| format!("{}: {err}", to.display()))?;
            added += 1;
        }
        Ok(added)
    }

    /// Reads every file of `release` from the store and checks its size
    /// and SHA-256.
    ///
    /// # Errors
    ///
    /// Each file that is missing or not what the list says, by its path
    /// in the release, one a line.
    pub fn check(&self, release: &Release) -> Result<Checked, Vec<String>> {
        let mut problems = Vec::new();
        let mut checked = Checked { files: 0, bytes: 0 };
        let mut buffer = vec![0u8; 1 << 16];
        for entry in &release.files {
            let Some((sha256, size)) = entry.file() else { continue };
            checked.files += 1;
            checked.bytes += size;
            if let Err(why) = self.check_file(sha256, size, &mut buffer) {
                problems.push(format!("{}: {why}", entry.path));
            }
        }
        if problems.is_empty() { Ok(checked) } else { Err(problems) }
    }

    /// Checks the store's file `sha256` is `size` bytes of that hash.
    fn check_file(&self, sha256: &str, size: u64, buffer: &mut [u8]) -> Result<(), String> {
        let path = self.path(sha256);
        let mut file = std::fs::File::open(&path).map_err(|err| match err.kind() {
            std::io::ErrorKind::NotFound => "missing from the store".to_string(),
            _ => format!("cannot read {}: {err}", path.display()),
        })?;
        let mut context = ring::digest::Context::new(&ring::digest::SHA256);
        let mut read = 0u64;
        loop {
            let got = file
                .read(buffer)
                .map_err(|err| format!("cannot read {}: {err}", path.display()))?;
            if got == 0 {
                break;
            }
            read += got as u64;
            // A file longer than the list says is wrong already; reading
            // the rest would only take time.
            if read > size {
                return Err(format!("longer than the {size} bytes the signed list gives"));
            }
            context.update(&buffer[..got]);
        }
        if read != size {
            return Err(format!("{read} bytes, not the {size} the signed list gives"));
        }
        let actual = hex(context.finish().as_ref());
        if actual != sha256 {
            return Err(format!("its SHA-256 is {actual}, not the signed list's"));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::list::{
        About,
        tests::{scratch, tree},
    };

    fn release_of(dir: &Path) -> Release {
        let about = About {
            version: "0.14.0".into(),
            commit: "01d66b7".into(),
            toolchain: "nightly-2026-10-15".into(),
            built: 0,
            updates_from: "0.14.0".into(),
        };
        Release::of_tree(dir, about).unwrap()
    }

    #[test]
    fn a_store_holds_each_file_once_and_finds_damage() {
        let dir = scratch("store");
        let slot = dir.join("slot");
        tree(&slot);
        // The same bytes twice are kept once.
        std::fs::write(slot.join("commands/again"), b"spark's bytes").unwrap();
        let release = release_of(&slot);
        let store = Store::at(&dir.join("files"));
        assert_eq!(store.add_tree(&slot, &release).unwrap(), 2);
        assert_eq!(store.add_tree(&slot, &release).unwrap(), 0);
        assert_eq!(store.check(&release).unwrap(), Checked { files: 3, bytes: 26 });

        let spark = store.path(
            release
                .files
                .iter()
                .find(|e| e.path == "commands/spark")
                .unwrap()
                .sha256
                .as_deref()
                .unwrap(),
        );
        std::fs::write(&spark, b"spark's bytez").unwrap();
        let problems = store.check(&release).unwrap_err();
        assert_eq!(problems.len(), 2);
        assert!(problems[0].starts_with("commands/again: its SHA-256 is "), "{problems:?}");
        assert!(problems[1].starts_with("commands/spark: its SHA-256 is "), "{problems:?}");
        std::fs::write(&spark, b"spark's bytes, and more").unwrap();
        assert!(store.check(&release).unwrap_err()[0].contains("longer than the 13 bytes"));
        std::fs::remove_file(&spark).unwrap();
        assert!(store.check(&release).unwrap_err()[0].ends_with("missing from the store"));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
