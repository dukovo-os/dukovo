// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! admind's store (ADR 0516): the one SQLite file, `/system/state/admind/
//! store.db`, that keeps what the console must not lose with a restart:
//! the journal of what changed ([`changes`]), the versions of the files it
//! watches ([`versions`]) and what they were when last looked at
//! ([`watched`]), and the tasks ([`tasks`]).
//!
//! The file uses SQLite's rollback journal, not WAL. WAL keeps its index
//! in a file that every connection of every process maps shared and
//! writable, which Dukovo does not give yet (ADR 0511); the rollback
//! journal works between processes by byte-range locks. admind holds the
//! only connection that writes. Root's `admin put-back` opens the same file
//! read-only ([`Store::open_read_only`]) and waits up to [`BUSY_TIME`] for
//! admind's write, which is a few milliseconds, to end.
//!
//! `PRAGMA user_version` numbers the schema. [`SCHEMA`] lists the
//! statements of each version, and [`Store::open`] applies the versions
//! the file lacks, each in one transaction that also sets the number, so
//! a file is always at one version whole. A file of a newer version than
//! this admind knows is refused, not changed.
//!
//! admind answers its requests on one thread, and a call here blocks it.
//! That is enough because each call is short: a row or a few, one file's
//! text of at most 1 MiB, on a file that is a few megabytes. No call holds
//! the connection across an `await`: the callers borrow the store, call
//! it and let go, so a connection is never in use by two requests, and a
//! request that waits for authd or a command waits with the store free. The
//! one wait in a call is the busy timeout, which only a root `admin
//! put-back` reading can make admind use, for as long as that read lasts.
//!
//! When the console says its journal or tasks are not kept, look in `log
//! admind` for the line that says why the file could not be opened, and
//! for SQLite's own words, which this module passes on. The host tests of
//! this module, `admind::store::tests`, make their files in the system's
//! temporary folder.

pub mod changes;
pub mod tasks;
pub mod versions;
pub mod watched;

use std::fmt;
use std::fs::OpenOptions;
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;
use std::time::Duration;

use rusqlite::{Connection, OpenFlags, TransactionBehavior};

/// Where the store is. Spark gives admind this folder, mode 0700.
pub const PATH: &str = "/system/state/admind/store.db";

/// How long a call waits for another connection's lock: the write of
/// admind that a root reader meets, or the reader that admind's write
/// meets, which are both short.
pub const BUSY_TIME: Duration = Duration::from_secs(5);

/// The schema, a list of statements for each version, the first being
/// version 1. A version is applied whole or not at all.
const SCHEMA: &[&[&str]] = &[&[
    // What changed. `how` is `console`, `token NAME` or `by hand`. `files`
    // is JSON: what each file was before and after, by its SHA-256.
    "CREATE TABLE changes (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        at INTEGER NOT NULL,
        who TEXT NOT NULL,
        how TEXT NOT NULL,
        action TEXT,
        argv TEXT,
        status INTEGER,
        task INTEGER,
        note TEXT,
        files TEXT NOT NULL,
        seen_by TEXT,
        seen_at INTEGER
    ) STRICT",
    "CREATE INDEX changes_at ON changes (at)",
    // The text of each version of a file, once however many files and
    // versions have it.
    "CREATE TABLE blobs (
        hash TEXT PRIMARY KEY,
        size INTEGER NOT NULL,
        bytes BLOB NOT NULL
    ) STRICT",
    "CREATE TABLE versions (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        path TEXT NOT NULL,
        hash TEXT NOT NULL,
        at INTEGER NOT NULL
    ) STRICT",
    "CREATE INDEX versions_path ON versions (path, id)",
    "CREATE INDEX versions_hash ON versions (hash)",
    // What each watched file was when the console last looked: its size and
    // modification time in nanoseconds, which tell it changed, and what it
    // was, as the journal's JSON for one side of a file.
    "CREATE TABLE watched (
        path TEXT PRIMARY KEY,
        size INTEGER NOT NULL,
        mtime INTEGER NOT NULL,
        seen TEXT NOT NULL
    ) STRICT",
    // Long actions. `log` is the last lines the command wrote, one to a
    // line.
    "CREATE TABLE tasks (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        action TEXT NOT NULL,
        args TEXT NOT NULL,
        argv TEXT NOT NULL,
        who TEXT NOT NULL,
        how TEXT NOT NULL,
        started INTEGER NOT NULL,
        state TEXT NOT NULL,
        step INTEGER NOT NULL DEFAULT 0,
        steps INTEGER NOT NULL DEFAULT 0,
        words TEXT NOT NULL DEFAULT '',
        percent INTEGER,
        ended INTEGER,
        status INTEGER,
        note TEXT,
        log TEXT NOT NULL DEFAULT '',
        changed INTEGER
    ) STRICT",
    "CREATE INDEX tasks_ended ON tasks (ended)",
]];

/// What went wrong with the store, in words.
#[derive(Debug, PartialEq, Eq)]
pub struct Error(String);

impl Error {
    /// An error that says `what`.
    pub fn new(what: impl Into<String>) -> Error {
        Error(what.into())
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Error {}

impl From<rusqlite::Error> for Error {
    fn from(err: rusqlite::Error) -> Error {
        Error(format!("the store: {err}"))
    }
}

/// What a call of the store gives.
pub type Result<T> = std::result::Result<T, Error>;

/// The store: admind's one connection, or a reader's.
pub struct Store {
    conn: Connection,
    /// What the versions of files are kept up to.
    pub limits: versions::Limits,
}

impl Store {
    /// Opens the store at `path`, makes it if it is not there (mode 0600),
    /// and brings its schema up to date.
    ///
    /// # Errors
    ///
    /// If the file cannot be made or opened, is not a database, or is of a
    /// newer schema than this admind knows.
    pub fn open(path: &Path) -> Result<Store> {
        // SQLite makes a new file with the mode of the umask, and its
        // journal with the file's: made here, they are 0600.
        OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .open(path)
            .map_err(|err| {
                Error::new(format!("{}: {}", path.display(), crate::util::strip_os_error(&err)))
            })?;
        let conn = Connection::open(path)?;
        Store::prepared(conn)
    }

    /// Opens the store at `path` to read it, as root's `admin put-back`
    /// does, while admind has it open.
    ///
    /// # Errors
    ///
    /// If it is not there, is not a database, or is not at the schema this
    /// program reads.
    pub fn open_read_only(path: &Path) -> Result<Store> {
        let flags = OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX;
        let conn = Connection::open_with_flags(path, flags)?;
        conn.busy_timeout(BUSY_TIME)?;
        let found = user_version(&conn)?;
        if found != SCHEMA.len() {
            return Err(Error::new(format!(
                "the store {} is of schema version {found}, and this program reads {}: start \
                 admind, which brings it up to date, or run the admin of the same release",
                path.display(),
                SCHEMA.len()
            )));
        }
        Ok(Store { conn, limits: versions::Limits::DEFAULT })
    }

    /// A store in memory, for the tests.
    #[cfg(test)]
    pub fn in_memory() -> Store {
        Store::prepared(Connection::open_in_memory().expect("a database in memory"))
            .expect("a schema that applies")
    }

    /// Sets `conn` up for admind and applies the schema versions it lacks.
    fn prepared(mut conn: Connection) -> Result<Store> {
        conn.busy_timeout(BUSY_TIME)?;
        // The default, said: WAL needs shared writable mappings (ADR 0511).
        conn.pragma_update(None, "journal_mode", "DELETE")?;
        migrate(&mut conn, SCHEMA)?;
        Ok(Store { conn, limits: versions::Limits::DEFAULT })
    }

    /// The schema version the file is at.
    ///
    /// # Errors
    ///
    /// If the file cannot be read.
    #[cfg(test)]
    pub fn schema_version(&self) -> Result<usize> {
        user_version(&self.conn)
    }

    /// Runs `work`, whose calls of this store all happen in one
    /// transaction: they are all kept or, if `work` fails, none. Inside
    /// another `atomically` it is part of that one.
    ///
    /// # Errors
    ///
    /// What `work` gives, or if the transaction cannot begin or end.
    pub fn atomically<T>(&self, work: impl FnOnce() -> Result<T>) -> Result<T> {
        if !self.conn.is_autocommit() {
            return work();
        }
        let tx = rusqlite::Transaction::new_unchecked(&self.conn, TransactionBehavior::Immediate)?;
        let done = work()?;
        tx.commit()?;
        Ok(done)
    }
}

/// Brings the file of `conn` to the last version of `schema`, one version
/// at a time, each in a transaction that also sets its number: a version
/// that fails leaves the file at the one before.
fn migrate(conn: &mut Connection, schema: &[&[&str]]) -> Result<()> {
    let found = user_version(conn)?;
    if found > schema.len() {
        return Err(Error::new(format!(
            "the store is of schema version {found}, which a newer admind made, and this one \
             knows up to {}",
            schema.len()
        )));
    }
    for (index, statements) in schema.iter().enumerate().skip(found) {
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        for statement in *statements {
            tx.execute(statement, [])?;
        }
        tx.pragma_update(None, "user_version", i64::try_from(index + 1).unwrap_or(i64::MAX))?;
        tx.commit()?;
    }
    Ok(())
}

/// `PRAGMA user_version` of `conn`.
fn user_version(conn: &Connection) -> Result<usize> {
    let number: i64 = conn.pragma_query_value(None, "user_version", |row| row.get(0))?;
    usize::try_from(number).map_err(|_| Error::new("the store's schema version is below 0"))
}

#[cfg(test)]
mod tests;
