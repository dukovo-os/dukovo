// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! What each watched file was when the console last looked at it (ADR
//! 0516): the size and modification time that tell a round of the
//! journal that it changed, and what it held then.

use std::collections::BTreeMap;

use rusqlite::params;

use super::changes::{Seen, from_json, to_json};
use super::{Result, Store};

/// What a file was at the last look.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Watched {
    /// Its size in bytes.
    pub size: u64,
    /// Its modification time, in nanoseconds since 1970.
    pub mtime: i64,
    /// What it held.
    pub seen: Seen,
}

impl Store {
    /// Every file watched, by path.
    ///
    /// # Errors
    ///
    /// If the store fails, or a row does not read.
    pub fn watched(&self) -> Result<BTreeMap<String, Watched>> {
        let mut query = self.conn.prepare("SELECT path, size, mtime, seen FROM watched")?;
        let rows = query.query_map([], |row| {
            let size: i64 = row.get(1)?;
            Ok((
                row.get::<_, String>(0)?,
                Watched {
                    size: u64::try_from(size).unwrap_or(0),
                    mtime: row.get(2)?,
                    seen: from_json(3, &row.get::<_, String>(3)?)?,
                },
            ))
        })?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    /// Notes that `path` was `watched` at the last look.
    ///
    /// # Errors
    ///
    /// If the store fails.
    pub fn set_watched(&self, path: &str, watched: &Watched) -> Result<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO watched (path, size, mtime, seen) VALUES (?1, ?2, ?3, ?4)",
            params![path, watched.size as i64, watched.mtime, to_json(&watched.seen)?],
        )?;
        Ok(())
    }

    /// Forgets `path`: it is not a file any more.
    ///
    /// # Errors
    ///
    /// If the store fails.
    pub fn forget_watched(&self, path: &str) -> Result<()> {
        self.conn.execute("DELETE FROM watched WHERE path = ?1", [path])?;
        Ok(())
    }
}
