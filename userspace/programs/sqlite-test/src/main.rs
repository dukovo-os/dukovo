// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The test of SQLite on Radix (the `sqlite` boot test, ADR 0511): the
//! library the sqlite port builds, linked through rusqlite and
//! libsqlite3-sys as the system's services link it, on a folder of a disk
//! filesystem. It checks the library's version and options, a rollback
//! journal, a reader beside a writer, a second writer's busy timeout,
//! another process taking the database's locks, WAL in exclusive locking
//! mode and SQLite's FTS5 and JSON functions. Like the C tests, it prints
//! "sqlite-test: <check>" for each check that passed, a line for each
//! thing that fails, and "sqlite-test: every check passed"; it exits 1 if
//! a check failed.
//!
//! WAL between connections is not among the checks: its index is a shared
//! writable mapping of a file, which Radix refuses (docs/deferred.md).

use std::fmt::Debug;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use rusqlite::{Connection, ErrorCode};

/// The test's name, which every line it prints starts with.
const NAME: &str = "sqlite-test";

/// The version of the sqlite port.
const VERSION: &str = "3.54.1";

/// How many rows the first table gets.
const ROWS: i64 = 1000;

/// The database the rollback journal checks use, in the folder given.
const DATABASE: &str = "test.db";

/// The database of the WAL check.
const WAL_DATABASE: &str = "wal.db";

/// The files of an earlier run, which a run starts by taking away.
const LEFTOVERS: [&str; 7] = [
    "test.db",
    "test.db-journal",
    "test.db-wal",
    "test.db-shm",
    "wal.db",
    "wal.db-wal",
    "wal.db-shm",
];

/// The exit code of the child process that got SQLITE_BUSY.
const CHILD_BUSY: u8 = 10;

/// How long the child process may take, as the parent waits for it: its
/// own busy timeout is a fifth of a second.
const CHILD_LIMIT: Duration = Duration::from_secs(60);

/// How long a thread of the test may take to answer.
const THREAD_LIMIT: Duration = Duration::from_secs(30);

/// How long a connection waits for a lock that is taken, in the checks
/// that must not wait for it.
const PATIENCE: Duration = Duration::from_secs(5);

/// What the checks found.
#[derive(Default)]
struct Report {
    /// How many things failed.
    failures: usize,
}

impl Report {
    /// Says that something failed, and counts it.
    fn fail(&mut self, what: &str, expected: &dyn Debug, got: &dyn Debug) {
        println!("{NAME}: FAILED: {what}, {expected:?} but {got:?}");
        self.failures += 1;
    }

    /// Checks that a value is the one expected.
    fn expect<T: PartialEq + Debug + Copy>(&mut self, what: &str, expected: T, got: T) {
        if expected != got {
            self.fail(what, &expected, &got);
        }
    }

    /// The value of a result, or a failure and `None`.
    fn ok<T>(&mut self, what: &str, result: rusqlite::Result<T>) -> Option<T> {
        match result {
            Ok(value) => Some(value),
            Err(err) => {
                self.fail(what, &"no error", &err.to_string());
                None
            }
        }
    }

    /// Runs a check, and says it passed if nothing in it failed.
    fn check(&mut self, name: &str, body: impl FnOnce(&mut Self)) {
        let before = self.failures;
        body(self);
        if self.failures == before {
            println!("{NAME}: {name}");
        }
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    match args.as_slice() {
        [_, mode, dir] if mode == "child" => child(Path::new(dir)),
        [program, dir] => run(program, Path::new(dir)),
        _ => {
            eprintln!("usage: {NAME} FOLDER");
            ExitCode::from(2)
        }
    }
}

/// Runs the checks in `dir`, which `program` (the name the test was started
/// by) starts again as the second process.
fn run(program: &str, dir: &Path) -> ExitCode {
    if let Err(err) = fs::create_dir_all(dir) {
        println!("{NAME}: FAILED: making {}, a folder but {err}", dir.display());
        return ExitCode::FAILURE;
    }
    for name in LEFTOVERS {
        // Not there at the first run.
        let _ = fs::remove_file(dir.join(name));
    }
    let mut report = Report::default();
    linked(&mut report);
    rollback_journal(&mut report, dir);
    reader_beside_writer(&mut report, dir);
    second_writer(&mut report, dir);
    other_process(&mut report, dir, program);
    exclusive_wal(&mut report, dir);
    integrity_fts5_json(&mut report, dir);
    if report.failures == 0 {
        println!("{NAME}: every check passed");
        ExitCode::SUCCESS
    } else {
        println!("{NAME}: {} checks failed", report.failures);
        ExitCode::FAILURE
    }
}

/// The rows of the first table.
fn rows(conn: &Connection) -> rusqlite::Result<i64> {
    conn.query_row("SELECT count(*) FROM t", [], |row| row.get(0))
}

/// The first row of a query that gives one text.
fn text(conn: &Connection, sql: &str) -> rusqlite::Result<String> {
    conn.query_row(sql, [], |row| row.get(0))
}

/// Makes the first table, and fills it with `ROWS` rows in one transaction.
fn fill(conn: &mut Connection) -> rusqlite::Result<()> {
    conn.execute_batch("CREATE TABLE t(id INTEGER PRIMARY KEY, v TEXT NOT NULL)")?;
    let tx = conn.transaction()?;
    {
        let mut insert = tx.prepare("INSERT INTO t(v) VALUES (?1)")?;
        for n in 0..ROWS {
            insert.execute([format!("row {n}")])?;
        }
    }
    tx.commit()
}

/// Whether an error is SQLITE_BUSY.
fn is_busy(err: &rusqlite::Error) -> bool {
    matches!(err, rusqlite::Error::SqliteFailure(failure, _) if failure.code == ErrorCode::DatabaseBusy)
}

/// The kind of filesystem that holds `dir`, as `/processes/mounts` names
/// it (radix-proc(5)), or `unknown`.
fn filesystem(dir: &Path) -> String {
    let (Ok(path), Ok(mounts)) = (fs::canonicalize(dir), fs::read_to_string("/processes/mounts"))
    else {
        return "unknown".to_string();
    };
    let mut best: Option<(usize, &str)> = None;
    for line in mounts.lines() {
        let mut fields = line.split(' ');
        let (Some(_), Some(at), Some(kind)) = (fields.next(), fields.next(), fields.next()) else {
            continue;
        };
        let at = PathBuf::from(unescape(at));
        if path.starts_with(&at) && best.is_none_or(|(length, _)| at.as_os_str().len() >= length) {
            best = Some((at.as_os_str().len(), kind));
        }
    }
    best.map_or("unknown", |(_, kind)| kind).to_string()
}

/// A mount's path with the escapes of `/processes/mounts` undone: a
/// backslash and three octal digits stand for one byte.
fn unescape(field: &str) -> String {
    let bytes = field.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut at = 0;
    while at < bytes.len() {
        let octal = bytes
            .get(at + 1..at + 4)
            .filter(|_| bytes[at] == b'\\')
            .and_then(|digits| std::str::from_utf8(digits).ok())
            .and_then(|digits| u8::from_str_radix(digits, 8).ok());
        if let Some(byte) = octal {
            out.push(byte);
            at += 4;
        } else {
            out.push(bytes[at]);
            at += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The compile options the library lists.
fn compile_options(conn: &Connection) -> rusqlite::Result<Vec<String>> {
    let mut query = conn.prepare("SELECT compile_options FROM pragma_compile_options")?;
    query.query_map([], |row| row.get::<_, String>(0))?.collect()
}

/// That the library is the port's, built with the options the port gives.
fn linked(report: &mut Report) {
    report.check("linked SQLite 3.54.1, threadsafe, without extension loading", |r| {
        r.expect("the linked SQLite's version", VERSION, rusqlite::version());
        let Some(conn) = r.ok("opening a database", Connection::open_in_memory()) else {
            return;
        };
        let Some(options) = r.ok("reading pragma_compile_options", compile_options(&conn)) else {
            return;
        };
        for wanted in ["THREADSAFE=1", "OMIT_LOAD_EXTENSION", "ENABLE_FTS5", "MAX_MMAP_SIZE=0"] {
            if !options.iter().any(|option| option == wanted) {
                r.fail("pragma_compile_options", &format!("{wanted} listed"), &"missing");
            }
        }
    });
}

/// A rollback journal, SQLite's default, on the disk filesystem: a
/// transaction of many rows commits, and another connection sees it.
fn rollback_journal(report: &mut Report, dir: &Path) {
    let name = format!("a rollback journal on {}", filesystem(dir));
    report.check(&name, |r| {
        let Some(mut conn) = r.ok("opening test.db", Connection::open(dir.join(DATABASE))) else {
            return;
        };
        if let Some(mode) = r.ok("PRAGMA journal_mode", text(&conn, "PRAGMA journal_mode")) {
            r.expect("the default journal mode", "delete", mode.as_str());
        }
        if r.ok("filling test.db", fill(&mut conn)).is_none() {
            return;
        }
        let Some(other) = r.ok("opening test.db again", Connection::open(dir.join(DATABASE)))
        else {
            return;
        };
        if let Some(count) = r.ok("counting the rows", rows(&other)) {
            r.expect("the rows a new connection counts", ROWS, count);
        }
    });
}

/// A writer holds the database's reserved lock, which lets a reader in:
/// the reader counts at once, and counts the writer's row once it commits.
fn reader_beside_writer(report: &mut Report, dir: &Path) {
    report.check("a reader is not held up by a writer", |r| {
        let path = dir.join(DATABASE);
        let Some(writer) = r.ok("opening the writer", Connection::open(&path)) else {
            return;
        };
        let begun = writer.execute_batch("BEGIN IMMEDIATE; INSERT INTO t(v) VALUES ('writer');");
        if r.ok("the writer's BEGIN IMMEDIATE", begun).is_none() {
            return;
        }
        let (counted, counts) = mpsc::channel();
        let (commit, committed) = mpsc::channel::<()>();
        let reader = thread::spawn(move || {
            let start = Instant::now();
            let conn = Connection::open(&path).and_then(|conn| {
                conn.busy_timeout(PATIENCE)?;
                Ok(conn)
            });
            let count = |conn: &rusqlite::Result<Connection>| match conn {
                Ok(conn) => rows(conn).map_err(|err| err.to_string()),
                Err(err) => Err(err.to_string()),
            };
            let first = count(&conn);
            // The test has stopped listening, and has said why, if a send
            // fails.
            let _ = counted.send((first, start.elapsed()));
            if committed.recv().is_ok() {
                let _ = counted.send((count(&conn), Duration::ZERO));
            }
        });
        match counts.recv_timeout(THREAD_LIMIT) {
            Ok((Ok(count), took)) => {
                r.expect("the rows a reader counts beside a writer", ROWS, count);
                if took >= Duration::from_secs(1) {
                    r.fail("how long the reader took", &"under 1 s", &took);
                }
            }
            Ok((Err(err), _)) => r.fail("the reader's count", &"no error", &err),
            Err(err) => {
                r.fail("the reader's count", &"an answer", &err.to_string());
                return;
            }
        }
        if r.ok("the writer's COMMIT", writer.execute_batch("COMMIT")).is_none() {
            return;
        }
        // Telling the reader to go on needs its receiver to be there still.
        if commit.send(()).is_err() {
            r.fail("the reader", &"waiting for the commit", &"gone");
            return;
        }
        match counts.recv_timeout(THREAD_LIMIT) {
            Ok((Ok(count), _)) => {
                r.expect("the rows a reader counts after the commit", ROWS + 1, count);
            }
            Ok((Err(err), _)) => r.fail("the reader's second count", &"no error", &err),
            Err(err) => r.fail("the reader's second count", &"an answer", &err.to_string()),
        }
        if reader.join().is_err() {
            r.fail("the reader's thread", &"no panic", &"a panic");
        }
    });
}

/// A second writer waits as long as its busy timeout, then gets
/// SQLITE_BUSY.
fn second_writer(report: &mut Report, dir: &Path) {
    report.check("a second writer waits, then gets SQLITE_BUSY", |r| {
        let path = dir.join(DATABASE);
        let Some(first) = r.ok("opening the first writer", Connection::open(&path)) else {
            return;
        };
        if r.ok("the first writer's BEGIN IMMEDIATE", first.execute_batch("BEGIN IMMEDIATE"))
            .is_none()
        {
            return;
        }
        let Some(second) = r.ok("opening the second writer", Connection::open(&path)) else {
            return;
        };
        if r.ok("setting the busy timeout", second.busy_timeout(Duration::from_millis(200)))
            .is_none()
        {
            return;
        }
        let start = Instant::now();
        let outcome = second.execute_batch("BEGIN IMMEDIATE");
        let took = start.elapsed();
        match outcome {
            Err(err) if is_busy(&err) => {}
            Err(err) => {
                r.fail("the second writer's BEGIN IMMEDIATE", &"SQLITE_BUSY", &err.to_string());
            }
            Ok(()) => r.fail("the second writer's BEGIN IMMEDIATE", &"SQLITE_BUSY", &"success"),
        }
        if took < Duration::from_millis(150) || took >= PATIENCE {
            r.fail("how long the second writer waited", &"150 ms to 5 s", &took);
        }
        r.ok("the first writer's ROLLBACK", first.execute_batch("ROLLBACK"));
    });
}

/// Another process takes the database's locks as this one does, through
/// fcntl's byte-range locks: it waits for this process's write lock and
/// gets SQLITE_BUSY, and writes once this process lets go.
fn other_process(report: &mut Report, dir: &Path, program: &str) {
    report.check("another process waits for a writer, then sees its commit", |r| {
        let Some(parent) = r.ok("opening test.db", Connection::open(dir.join(DATABASE))) else {
            return;
        };
        if r.ok("BEGIN IMMEDIATE", parent.execute_batch("BEGIN IMMEDIATE")).is_none() {
            return;
        }
        let start = Instant::now();
        match run_child(program, dir) {
            Ok(code) => r.expect(
                "the exit code of a process that wrote while this one held the lock",
                i32::from(CHILD_BUSY),
                code,
            ),
            Err(err) => {
                r.fail("a process that wrote while this one held the lock", &"SQLITE_BUSY", &err);
            }
        }
        let waited = start.elapsed();
        if waited < Duration::from_millis(150) {
            r.fail("how long the other process waited", &"at least 150 ms", &waited);
        }
        if r.ok("COMMIT", parent.execute_batch("COMMIT")).is_none() {
            return;
        }
        match run_child(program, dir) {
            Ok(code) => r.expect("the exit code of a process that wrote after the commit", 0, code),
            Err(err) => r.fail("a process that wrote after the commit", &"success", &err),
        }
        if let Some(count) = r.ok("counting the rows", rows(&parent)) {
            r.expect("the rows counted after the other process's commit", ROWS + 2, count);
        }
    });
}

/// Starts this program again, as `program child DIR`, and waits for it:
/// its exit code, or what went wrong.
fn run_child(program: &str, dir: &Path) -> Result<i32, String> {
    let mut child = Command::new(program)
        .arg("child")
        .arg(dir)
        .spawn()
        .map_err(|err| format!("starting {program}: {err}"))?;
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return status.code().ok_or_else(|| format!("ended by {status}")),
            Ok(None) if start.elapsed() < CHILD_LIMIT => thread::sleep(Duration::from_millis(20)),
            Ok(None) => {
                // The child may have ended since the last look: the point
                // is that none is left running, and the failure is the
                // limit's.
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("still running after {} s", CHILD_LIMIT.as_secs()));
            }
            Err(err) => return Err(format!("waiting for it: {err}")),
        }
    }
}

/// The second process: inserts a row into the database in `dir` and
/// commits, with a busy timeout of 200 ms. It exits with `CHILD_BUSY` when
/// the database stayed locked, 1 for any other error.
fn child(dir: &Path) -> ExitCode {
    let wrote = Connection::open(dir.join(DATABASE)).and_then(|mut conn| {
        conn.busy_timeout(Duration::from_millis(200))?;
        let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        tx.execute("INSERT INTO t(v) VALUES ('child')", [])?;
        tx.commit()
    });
    match wrote {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) if is_busy(&err) => ExitCode::from(CHILD_BUSY),
        Err(err) => {
            eprintln!("{NAME}: child: {err}");
            ExitCode::FAILURE
        }
    }
}

/// WAL in exclusive locking mode, the one WAL Radix has: SQLite keeps the
/// WAL's index in memory and makes no -shm file, which would need a shared
/// writable mapping of a file. The checkpoint empties the log.
fn exclusive_wal(report: &mut Report, dir: &Path) {
    report.check("WAL in exclusive locking mode", |r| {
        let Some(mut conn) = r.ok("opening wal.db", Connection::open(dir.join(WAL_DATABASE)))
        else {
            return;
        };
        // The locking mode comes first: it is what keeps the index in memory.
        r.ok("PRAGMA locking_mode=EXCLUSIVE", text(&conn, "PRAGMA locking_mode=EXCLUSIVE"));
        if let Some(mode) = r.ok("PRAGMA journal_mode=WAL", text(&conn, "PRAGMA journal_mode=WAL"))
        {
            r.expect("the journal mode", "wal", mode.as_str());
        }
        if r.ok("filling wal.db", fill(&mut conn)).is_none() {
            return;
        }
        let checkpoint = conn.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?, row.get::<_, i64>(2)?))
        });
        if let Some(result) = r.ok("PRAGMA wal_checkpoint(TRUNCATE)", checkpoint) {
            r.expect("the checkpoint's busy, log and checkpointed frames", (0, 0, 0), result);
        }
        match fs::metadata(dir.join("wal.db-wal")) {
            Ok(wal) => r.expect("the size of wal.db-wal", 0, wal.len()),
            Err(err) => r.fail("wal.db-wal", &"a file of 0 bytes", &err.to_string()),
        }
        if dir.join("wal.db-shm").exists() {
            r.fail("wal.db-shm", &"no file", &"a file");
        }
        if let Some(count) = r.ok("counting the rows", rows(&conn)) {
            r.expect("the rows in wal.db", ROWS, count);
        }
    });
}

/// The integrity check, an FTS5 table and a JSON function, on the
/// database the earlier checks wrote.
fn integrity_fts5_json(report: &mut Report, dir: &Path) {
    report.check("integrity, FTS5 and JSON", |r| {
        let Some(conn) = r.ok("opening test.db", Connection::open(dir.join(DATABASE))) else {
            return;
        };
        let made = conn.execute_batch(
            "CREATE VIRTUAL TABLE docs USING fts5(body);\
             INSERT INTO docs(body) VALUES ('the quick brown fox'), ('a slow green turtle'), \
             ('an idle grey heron');",
        );
        if r.ok("making the FTS5 table", made).is_some() {
            let found =
                text(&conn, "SELECT group_concat(body) FROM docs WHERE docs MATCH 'turtle'");
            if let Some(found) = r.ok("an FTS5 query", found) {
                r.expect("the rows that match turtle", "a slow green turtle", found.as_str());
            }
        }
        let json = conn.query_row("SELECT json_extract('{\"a\":[1,2]}', '$.a[1]')", [], |row| {
            row.get::<_, i64>(0)
        });
        if let Some(value) = r.ok("json_extract", json) {
            r.expect("json_extract of $.a[1]", 2, value);
        }
        if let Some(verdict) = r.ok("PRAGMA integrity_check", text(&conn, "PRAGMA integrity_check"))
        {
            r.expect("the integrity check", "ok", verdict.as_str());
        }
    });
}
