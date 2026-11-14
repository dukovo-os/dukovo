// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The versions of the files the console watches (ADR 0516): their text,
//! kept by SHA-256 so that a text held by several versions or files is
//! stored once, and the order the versions came in.
//!
//! The limits keep this a small part of the store: at most 1 MiB of a
//! file, the last 20 versions of a file, and 64 MiB of text in all, the
//! oldest out first. A version that is out of the store leaves its hash
//! in the journal's rows, which then say the text is not kept.

use ring::digest;
use rusqlite::{OptionalExtension, params};

use super::{Error, Result, Store};

/// What the store keeps of files' versions, at most.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    /// The longest text of one version, in bytes: a longer file has its
    /// hash noted and no text kept.
    pub file: u64,
    /// The versions of one file.
    pub per_file: usize,
    /// The bytes of all the texts together.
    pub total: u64,
}

impl Limits {
    /// The limits admind keeps: 1 MiB, 20 versions, 64 MiB.
    pub const DEFAULT: Limits = Limits { file: 1 << 20, per_file: 20, total: 64 << 20 };
}

/// The SHA-256 of `bytes`, in lower-case hexadecimal.
pub fn sha256(bytes: &[u8]) -> String {
    accounts::request::hex(digest::digest(&digest::SHA256, bytes).as_ref())
}

impl Store {
    /// Keeps `bytes` as the text of the version of `path` that was seen at
    /// Unix time `at`, and gives its hash. A text the file's newest
    /// version already has adds no version. Versions past the limits go.
    ///
    /// # Errors
    ///
    /// If `bytes` is longer than [`Limits::file`], or the store fails.
    pub fn keep_version(&self, path: &str, bytes: &[u8], at: i64) -> Result<String> {
        let limits = self.limits;
        if bytes.len() as u64 > limits.file {
            return Err(Error::new(format!(
                "the store keeps at most {} bytes of a file, and {path} has {}",
                limits.file,
                bytes.len()
            )));
        }
        let hash = sha256(bytes);
        self.atomically(|| {
            self.conn.execute(
                "INSERT OR IGNORE INTO blobs (hash, size, bytes) VALUES (?1, ?2, ?3)",
                params![hash, bytes.len() as i64, bytes],
            )?;
            let newest: Option<String> = self
                .conn
                .query_row(
                    "SELECT hash FROM versions WHERE path = ?1 ORDER BY id DESC LIMIT 1",
                    [path],
                    |row| row.get(0),
                )
                .optional()?;
            if newest.as_deref() != Some(hash.as_str()) {
                self.conn.execute(
                    "INSERT INTO versions (path, hash, at) VALUES (?1, ?2, ?3)",
                    params![path, hash, at],
                )?;
            }
            self.trim_versions(path)
        })?;
        Ok(hash)
    }

    /// The text kept for `hash`, if a version still has it.
    ///
    /// # Errors
    ///
    /// If the store fails.
    pub fn version_text(&self, hash: &str) -> Result<Option<Vec<u8>>> {
        Ok(self
            .conn
            .query_row("SELECT bytes FROM blobs WHERE hash = ?1", [hash], |row| row.get(0))
            .optional()?)
    }

    /// The hashes of the versions kept of `path`, newest first.
    ///
    /// # Errors
    ///
    /// If the store fails.
    #[cfg(test)]
    pub fn versions_of(&self, path: &str) -> Result<Vec<String>> {
        let mut query =
            self.conn.prepare("SELECT hash FROM versions WHERE path = ?1 ORDER BY id DESC")?;
        let rows = query.query_map([path], |row| row.get(0))?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    /// The bytes of text the versions hold, each text once.
    ///
    /// # Errors
    ///
    /// If the store fails.
    pub fn text_size(&self) -> Result<u64> {
        let size: i64 =
            self.conn
                .query_row("SELECT COALESCE(SUM(size), 0) FROM blobs", [], |row| row.get(0))?;
        Ok(u64::try_from(size).unwrap_or(0))
    }

    /// Drops the versions that are past the limits, after one of `path`
    /// was added: those of the file beyond its last
    /// [`Limits::per_file`], then the oldest of all while the texts are
    /// more than [`Limits::total`]. A file's newest version stays as long
    /// as another can go in its place, so that what a file was is known
    /// for the longest time.
    fn trim_versions(&self, path: &str) -> Result<()> {
        let limits = self.limits;
        self.conn.execute(
            "DELETE FROM versions WHERE path = ?1 AND id NOT IN
                 (SELECT id FROM versions WHERE path = ?1 ORDER BY id DESC LIMIT ?2)",
            params![path, limits.per_file as i64],
        )?;
        while self.text_size_after_orphans()? > limits.total {
            let victim: Option<i64> = self.conn.query_row(
                "SELECT COALESCE(
                         (SELECT MIN(id) FROM versions
                          WHERE id NOT IN (SELECT MAX(id) FROM versions GROUP BY path)),
                         (SELECT MIN(id) FROM versions))",
                [],
                |row| row.get(0),
            )?;
            let Some(victim) = victim else { break };
            self.conn.execute("DELETE FROM versions WHERE id = ?1", [victim])?;
        }
        Ok(())
    }

    /// Removes the texts no version has, and gives what the rest weigh.
    fn text_size_after_orphans(&self) -> Result<u64> {
        self.conn.execute("DELETE FROM blobs WHERE hash NOT IN (SELECT hash FROM versions)", [])?;
        self.text_size()
    }
}
