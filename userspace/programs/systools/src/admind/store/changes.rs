// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The journal's rows (ADR 0516): what changed, who changed it and how,
//! and what each file was before and after, by its SHA-256. The text of
//! those versions is in [`versions`](super::versions).

use rusqlite::{OptionalExtension, Row, params};
use serde::{Deserialize, Serialize};

use super::{Error, Result, Store};

/// The most rows the journal keeps: the newest.
pub const ROWS_MAX: i64 = 10_000;

/// What a file was before a change or after it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "kebab-case")]
pub enum Seen {
    /// There was no such file.
    Absent,
    /// A text: its hash, and its size in bytes. The store keeps the text
    /// while the file's last versions include it.
    Text { sha256: String, size: u64 },
    /// A file longer than the store keeps: its hash and size, and no text.
    Large { sha256: String, size: u64 },
    /// A secret, or a file that not everyone may read: only that it is
    /// there, since everyone who is signed in reads the journal.
    Secret,
    /// A file admind's account could not read.
    Unreadable,
}

impl Seen {
    /// The hash of the text, for a file that has one.
    pub fn hash(&self) -> Option<&str> {
        match self {
            Seen::Text { sha256, .. } | Seen::Large { sha256, .. } => Some(sha256),
            Seen::Absent | Seen::Secret | Seen::Unreadable => None,
        }
    }

    /// Whether the two are the same file as far as they tell: the same
    /// hash, or both absent. A secret or an unreadable file tells nothing,
    /// so it is the same as none.
    pub fn same_as(&self, other: &Seen) -> bool {
        match (self, other) {
            (Seen::Absent, Seen::Absent) => true,
            (a, b) => a.hash().is_some() && a.hash() == b.hash(),
        }
    }
}

/// One file of a change.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileChange {
    /// Its path.
    pub path: String,
    /// What it was before.
    pub before: Seen,
    /// What it was after.
    pub after: Seen,
}

/// A row to add to the journal.
#[derive(Debug)]
pub struct NewChange<'a> {
    /// The Unix time it ended.
    pub at: i64,
    /// The person, or "outside the console".
    pub who: &'a str,
    /// `console`, `token NAME` or `by hand`.
    pub how: &'a str,
    /// The action's ID, for a console change.
    pub action: Option<&'a str>,
    /// The command it ran.
    pub argv: Option<&'a [String]>,
    /// The command's exit status, if it ran to its end.
    pub status: Option<i32>,
    /// The task it ran as.
    pub task: Option<i64>,
    /// What else there is to say: why a command did not end, what was
    /// put back.
    pub note: Option<&'a str>,
    /// The files it named or changed.
    pub files: &'a [FileChange],
}

/// A row of the journal.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Change {
    /// Its number, which grows with each row.
    pub id: i64,
    /// The Unix time it ended, or was found.
    pub at: i64,
    /// The person, or "outside the console".
    pub who: String,
    /// `console`, `token NAME` or `by hand`.
    pub how: String,
    /// The action's ID, for a console change.
    pub action: Option<String>,
    /// The command it ran.
    pub argv: Option<Vec<String>>,
    /// The command's exit status, if it ran to its end.
    pub status: Option<i32>,
    /// The task it ran as, for a long action.
    pub task: Option<i64>,
    /// What else there is to say: why a command did not end, what was put
    /// back.
    pub note: Option<String>,
    /// The files it named or changed, each as it was before and after.
    pub files: Vec<FileChange>,
    /// Who marked a hand edit seen, and when.
    pub seen: Option<Marked>,
}

/// Who marked a row seen, and when.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Marked {
    /// The person.
    pub by: String,
    /// The Unix time.
    pub at: i64,
}

/// What marking a row seen did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Marking {
    /// The row is marked now.
    Done,
    /// It was marked before, and stays as it was.
    Already,
    /// It is not a hand edit, which is all that is marked.
    NotByHand,
    /// There is no such row.
    Missing,
}

/// The columns of a row, in the order [`Change::read`] takes them.
const COLUMNS: &str = "id, at, who, how, action, argv, status, task, note, files, seen_by, seen_at";

impl Change {
    /// Whether it is a change made by hand.
    #[cfg(test)]
    pub fn by_hand(&self) -> bool {
        self.how == "by hand"
    }

    /// The row `row` of a query of [`COLUMNS`].
    fn read(row: &Row<'_>) -> rusqlite::Result<Change> {
        let argv = match row.get::<_, Option<String>>(5)? {
            Some(text) => Some(from_json(5, &text)?),
            None => None,
        };
        let files = from_json(9, &row.get::<_, String>(9)?)?;
        let seen = match (row.get::<_, Option<String>>(10)?, row.get::<_, Option<i64>>(11)?) {
            (Some(by), Some(at)) => Some(Marked { by, at }),
            _ => None,
        };
        Ok(Change {
            id: row.get(0)?,
            at: row.get(1)?,
            who: row.get(2)?,
            how: row.get(3)?,
            action: row.get(4)?,
            argv,
            status: row.get(6)?,
            task: row.get(7)?,
            note: row.get(8)?,
            files,
            seen,
        })
    }
}

/// `value` as JSON text.
pub fn to_json<T: Serialize + ?Sized>(value: &T) -> Result<String> {
    serde_json::to_string(value).map_err(|err| Error::new(format!("the store: {err}")))
}

/// `text` read as the JSON `T`, for column `at`.
pub fn from_json<T: serde::de::DeserializeOwned>(at: usize, text: &str) -> rusqlite::Result<T> {
    serde_json::from_str(text).map_err(|err| {
        rusqlite::Error::FromSqlConversionFailure(at, rusqlite::types::Type::Text, Box::new(err))
    })
}

impl Store {
    /// Adds `change` to the journal, and gives its number. Rows past the
    /// newest [`ROWS_MAX`] are forgotten.
    ///
    /// # Errors
    ///
    /// If the store fails.
    pub fn add_change(&self, change: &NewChange<'_>) -> Result<i64> {
        let argv = change.argv.map(to_json).transpose()?;
        let files = to_json(&change.files)?;
        self.atomically(|| {
            self.conn.execute(
                "INSERT INTO changes (at, who, how, action, argv, status, task, note, files)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![
                    change.at,
                    change.who,
                    change.how,
                    change.action,
                    argv,
                    change.status,
                    change.task,
                    change.note,
                    files
                ],
            )?;
            let id = self.conn.last_insert_rowid();
            self.conn.execute("DELETE FROM changes WHERE id <= ?1", [id - ROWS_MAX])?;
            Ok(id)
        })
    }

    /// The rows made at Unix time `since` or later, newest first, at most
    /// `limit` of them.
    ///
    /// # Errors
    ///
    /// If the store fails, or a row does not read.
    pub fn changes(&self, since: i64, limit: usize) -> Result<Vec<Change>> {
        let mut query = self.conn.prepare(&format!(
            "SELECT {COLUMNS} FROM changes WHERE at >= ?1 ORDER BY id DESC LIMIT ?2"
        ))?;
        let rows = query.query_map(params![since, limit as i64], Change::read)?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    /// The row numbered `id`.
    ///
    /// # Errors
    ///
    /// If the store fails, or the row does not read.
    pub fn change(&self, id: i64) -> Result<Option<Change>> {
        Ok(self
            .conn
            .query_row(&format!("SELECT {COLUMNS} FROM changes WHERE id = ?1"), [id], Change::read)
            .optional()?)
    }

    /// Marks the hand edit `id` seen by `by` at Unix time `at`.
    ///
    /// # Errors
    ///
    /// If the store fails.
    pub fn mark_seen(&self, id: i64, by: &str, at: i64) -> Result<Marking> {
        let found: Option<(String, Option<i64>)> = self
            .conn
            .query_row("SELECT how, seen_at FROM changes WHERE id = ?1", [id], |row| {
                Ok((row.get(0)?, row.get(1)?))
            })
            .optional()?;
        Ok(match found {
            None => Marking::Missing,
            Some((how, _)) if how != "by hand" => Marking::NotByHand,
            Some((_, Some(_))) => Marking::Already,
            Some(_) => {
                self.conn.execute(
                    "UPDATE changes SET seen_by = ?2, seen_at = ?3 WHERE id = ?1",
                    params![id, by, at],
                )?;
                Marking::Done
            }
        })
    }
}
