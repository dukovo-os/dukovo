// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! What `database` needs of the machine around PostgreSQL's statements:
//! the session, Spark's word on the service, a restart, the certificates
//! certd keeps, the accounts, the TLS check, the firewall's trial and its
//! loading of rules, and the log. The commands work through [`Host`], so
//! that a test can stand a machine in memory in their place.

use std::fs::TryLockError;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use manifest::service::OVERRIDES;
use spark_api::show::State;

use super::wire::{self, Answer, Error, Session};
use crate::cert::inspect;
use crate::cert::store::Store;
use crate::deadline::Quiet;
use crate::tls;
use crate::util::{Fail, fail};
use crate::{firewall, logger, service, servicefiles, trial};

/// PostgreSQL's socket, in the service's live folder.
pub const SOCKET: &str = "/system/live/services/postgres/.s.PGSQL.5432";

/// Where `database` keeps its lock.
pub const STATE: &str = "/system/state/databases";

/// The service's name, in Spark and in the files of the settings.
pub const SERVICE: &str = "postgresql";

/// The port PostgreSQL listens on.
pub const PORT: u16 = 5432;

/// The bundle PostgreSQL's service and its commands are in.
pub const BUNDLE: &str = "org.postgresql.postgresql";

/// The top of `/shared`, which snapshots and backups hold.
pub const SHARED: &str = "/shared";

/// Where the snapshots of `/shared` are read.
pub const SNAPSHOTS_OF_SHARED: &str = "/snapshots/shared";

/// The folder of the copies the bundle's backup hook makes, from the top
/// of `/shared`: in the home of the account postgres.
pub const DUMPS_IN_SHARED: &str = "services/org.postgresql.postgresql/postgres/dumps";

/// The folder of those copies now.
pub const DUMPS: &str = "/shared/services/org.postgresql.postgresql/postgres/dumps";

/// The folder of PostgreSQL's socket, which `pg_restore` is told with `-h`.
pub const SOCKET_DIR: &str = "/system/live/services/postgres";

/// The command that runs a bundle's command.
const BUNDLE_COMMAND: &str = "/system/commands/bundle";

/// How much of what `pg_restore` says is kept.
pub const SAID_MAX: usize = 16 * 1024;

/// How long a restart is waited for: the server must answer again.
pub const RESTART_WAIT: Duration = Duration::from_secs(60);

/// How long a change of the files is waited for: the server must say it
/// has read them again.
pub const RELOAD_WAIT: Duration = Duration::from_secs(10);

/// How long to wait between looks at a server that is starting or
/// reading its files.
const POLL: Duration = Duration::from_millis(250);

/// How long a change waits for another to finish.
const LOCK_WAIT: Duration = Duration::from_secs(60);

/// What Spark says of PostgreSQL's service.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Service {
    /// Whether the bundle that has the service is installed.
    pub installed: bool,
    /// Whether it starts at boot, and now.
    pub enabled: bool,
    /// Whether it runs and is ready.
    pub running: bool,
}

/// The files of PostgreSQL's service and of the firewall that `database`
/// changes.
#[derive(Clone, Debug)]
pub struct Files {
    /// The service file Spark reads.
    pub service: PathBuf,
    /// Its override, which names the certificates.
    pub over: PathBuf,
    /// The firewall's rules, which hold the block for the port.
    pub firewall: PathBuf,
}

impl Files {
    /// Where the system keeps them.
    #[must_use]
    pub fn system() -> Files {
        let folder = Path::new(servicefiles::DIR);
        Files {
            service: folder.join(format!("{SERVICE}.toml")),
            over: folder.join(OVERRIDES).join(format!("{SERVICE}.toml")),
            firewall: PathBuf::from(firewall::SETTINGS),
        }
    }
}

/// The time now, as a person at the machine reads it and as a document
/// writes it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Clock {
    /// Milliseconds since 1970.
    pub ms: u64,
    /// The local year, month, day, hour and minute.
    pub local: (i64, u32, u32, u32, u32),
}

impl Clock {
    /// The time on the machine's clock.
    #[must_use]
    pub fn now() -> Clock {
        let ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |since| u64::try_from(since.as_millis()).unwrap_or(u64::MAX));
        Clock { ms, local: crate::snap::local(i64::try_from(ms / 1000).unwrap_or(i64::MAX)) }
    }

    /// The local day as a name has it: `20261116`.
    #[must_use]
    pub fn day(&self) -> String {
        let (year, month, day, _, _) = self.local;
        format!("{year:04}{month:02}{day:02}")
    }

    /// The local day and minute as a name has them: `20261116_0200`.
    #[must_use]
    pub fn minute(&self) -> String {
        let (_, _, _, hour, minute) = self.local;
        format!("{}_{hour:02}{minute:02}", self.day())
    }
}

/// A folder that holds the copies of the databases that a backup made.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Folder {
    /// `live` for the folder in use, or the name of the snapshot of
    /// `/shared` the folder is in.
    pub from: String,
    /// The folder.
    pub path: PathBuf,
}

/// The machine a command works on.
pub trait Host {
    /// Runs `sql` as the role postgres, signing in first when it has not.
    ///
    /// # Errors
    ///
    /// As [`Session::simple`] says, and the sign-in's.
    fn sql(&mut self, sql: &str) -> Result<Vec<Answer>, Error>;

    /// What Spark says of the service.
    ///
    /// # Errors
    ///
    /// When Spark does not answer.
    fn service(&mut self) -> Result<Service, Fail>;

    /// Restarts the server and waits until it answers again, a new one
    /// that Spark says runs: the open sessions end, and the next statement
    /// signs in again.
    ///
    /// # Errors
    ///
    /// Why it did not come back, in words.
    fn restart(&mut self) -> Result<(), String>;

    /// Whether certd keeps the certificate `name`.
    ///
    /// # Errors
    ///
    /// When the store cannot be read.
    fn has_certificate(&mut self, name: &str) -> Result<bool, String>;

    /// Whether the server has an account called `name`.
    fn is_account(&mut self, name: &str) -> bool;

    /// Checks that the server takes TLS on this machine's address with
    /// the certificate `name`, by a name the certificate holds.
    ///
    /// # Errors
    ///
    /// What the handshake or the certificate said.
    fn probe(&mut self, name: &str) -> Result<(), String>;

    /// Whether the firewall's rules are being tried (ADR 0472): Spark puts
    /// the file back as it was when the trial ends, so a change of it now
    /// would be lost. Spark that does not answer runs no trial, as
    /// `firewall` takes it.
    fn firewall_is_tried(&mut self) -> bool;

    /// Has the firewall give the kernel the rules of its file, as
    /// `firewall load` does.
    ///
    /// # Errors
    ///
    /// Why the kernel did not take them, in words.
    fn load_firewall(&mut self) -> Result<(), String>;

    /// The files of the service.
    fn files(&self) -> &Files;

    /// The folder of the copies that the backup hook makes.
    fn dumps(&self) -> PathBuf;

    /// The folders of those copies in the snapshots of `/shared`, newest
    /// snapshot first. Empty when the snapshots cannot be read, which
    /// is said on standard error.
    fn snapshot_dumps(&mut self) -> Vec<Folder>;

    /// The time now.
    fn now(&mut self) -> Clock;

    /// Runs the bundle's `pg_restore` as root with `argv`, which reads the
    /// copy from `copy`, an open file, as its standard input.
    ///
    /// # Errors
    ///
    /// What `pg_restore` said, the first [`SAID_MAX`] bytes of it, or why
    /// it did not run.
    fn pg_restore(&mut self, argv: &[String], copy: File) -> Result<(), String>;

    /// Waits `time`, as the server takes it to do something.
    fn pause(&mut self, time: Duration);

    /// Writes a line to the log: what was done, never a password.
    fn log(&mut self, message: &str);
}

/// The first value of the first answer: what a `SHOW` or a lookup of one
/// value says.
#[must_use]
pub fn value(answers: &[Answer]) -> Option<&str> {
    answers.first()?.rows.first()?.first()?.as_deref()
}

/// The machine this runs on, with PostgreSQL's socket.
pub struct Machine {
    /// The session, once there is one.
    session: Option<Session<Quiet>>,
    /// The files of the service.
    files: Files,
}

impl Machine {
    /// This machine, with no session yet.
    #[must_use]
    pub fn new() -> Machine {
        Machine { session: None, files: Files::system() }
    }

    /// Signs in, unless a session is open.
    fn session(&mut self) -> Result<&mut Session<Quiet>, Error> {
        if self.session.is_none() {
            self.session = Some(Session::connect(Path::new(SOCKET))?);
        }
        self.session.as_mut().ok_or_else(|| Error::Broken("there is no session".into()))
    }
}

impl Default for Machine {
    fn default() -> Machine {
        Machine::new()
    }
}

impl Host for Machine {
    fn sql(&mut self, sql: &str) -> Result<Vec<Answer>, Error> {
        let answered = self.session()?.simple(sql);
        // A session that ended is not kept: the next statement signs in
        // again.
        if matches!(answered, Err(Error::Broken(_))) {
            self.session = None;
        }
        answered
    }

    fn service(&mut self) -> Result<Service, Fail> {
        match service::shown(Some(SERVICE)) {
            Ok(document) => Ok(document.services.iter().find(|one| one.name == SERVICE).map_or(
                Service { installed: false, enabled: false, running: false },
                |one| Service {
                    installed: true,
                    enabled: one.enabled,
                    running: one.state == State::Running,
                },
            )),
            // LSB's "unrecognized service".
            Err(failed) if failed.code == 4 => {
                Ok(Service { installed: false, enabled: false, running: false })
            }
            Err(failed) => Err(failed),
        }
    }

    fn restart(&mut self) -> Result<(), String> {
        // The start of the server that runs now, to tell the new one
        // from it: while the old one is shutting down it still answers.
        let before =
            self.sql(START_TIME).ok().and_then(|answers| value(&answers).map(String::from));
        self.session = None;
        service::change("restart", SERVICE, &["--no-wait".to_string(), "-q".to_string()])
            .map_err(|failed| failed.message)?;
        let deadline = Instant::now() + RESTART_WAIT;
        let mut last = String::from("it did not answer");
        while Instant::now() < deadline {
            thread::sleep(POLL);
            match self.sql(START_TIME) {
                Ok(answers) => {
                    let now = value(&answers).map(String::from);
                    if now.is_none() || now == before {
                        last = String::from("it had not restarted");
                    } else if self.service().is_ok_and(|service| service.running) {
                        return Ok(());
                    } else {
                        // Spark says that the service runs once its check of
                        // readiness passes, a moment after the server answers,
                        // and `database list` asks Spark first: a list read
                        // as the change ends would find no server.
                        last = String::from("Spark did not say that it runs");
                    }
                }
                Err(error) => last = error.to_string(),
            }
            self.session = None;
        }
        Err(format!(
            "PostgreSQL did not come back within {} seconds: {last}",
            RESTART_WAIT.as_secs()
        ))
    }

    fn has_certificate(&mut self, name: &str) -> Result<bool, String> {
        Ok(Store::system().generation(name)?.is_some())
    }

    fn is_account(&mut self, name: &str) -> bool {
        fs::read_to_string(accounts::PASSWD)
            .is_ok_and(|text| accounts::users(&text).entries().any(|user| user.name == name))
    }

    fn probe(&mut self, name: &str) -> Result<(), String> {
        let stored = Store::system().get(name)?;
        let chain = inspect::chain_of(&String::from_utf8_lossy(&stored.chain))?;
        let leaf = chain.first().ok_or_else(|| format!("{name} holds no certificate"))?;
        let details = inspect::details(leaf)?;
        let host = details
            .names
            .iter()
            .find(|name| !name.starts_with("*.") && name.parse::<IpAddr>().is_err())
            .ok_or_else(|| format!("{name} names no host a client could check it for"))?;
        // The certificate's own chain is trusted as well as the system's
        // roots: `this-machine` is its own authority.
        let mut roots = tls::roots(&[], "database");
        roots.add_parsable_certificates(chain.iter().cloned());
        let address = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), PORT);
        wire::tls_probe(address, host, roots)
    }

    fn firewall_is_tried(&mut self) -> bool {
        trial::running(firewall::TRIAL).is_ok_and(|running| running.is_some())
    }

    fn load_firewall(&mut self) -> Result<(), String> {
        firewall::load(&mut io::sink(), false).map(|_| ()).map_err(|failed| failed.message)
    }

    fn files(&self) -> &Files {
        &self.files
    }

    fn dumps(&self) -> PathBuf {
        PathBuf::from(DUMPS)
    }

    fn snapshot_dumps(&mut self) -> Vec<Folder> {
        let shared = Path::new(SHARED);
        let found = crate::snap::place(shared).and_then(|place| {
            let snapshots = crate::snap::snapshots(&place)?;
            Ok((place, snapshots))
        });
        match found {
            // A snapshot whose name could not be in the path of a copy that
            // is put back is left out: it would be listed and refused.
            Ok((place, snapshots)) => snapshots
                .iter()
                .rev()
                .filter(|snapshot| super::names::snapshot_name_ok(&snapshot.name))
                .map(|snapshot| Folder {
                    from: snapshot.name.clone(),
                    path: place.shown.join(&snapshot.name).join(DUMPS_IN_SHARED),
                })
                .collect(),
            Err(failed) => {
                eprintln!("database: the snapshots of {SHARED} are not read: {}", failed.message);
                Vec::new()
            }
        }
    }

    fn now(&mut self) -> Clock {
        Clock::now()
    }

    fn pg_restore(&mut self, argv: &[String], copy: File) -> Result<(), String> {
        let mut child = Command::new(BUNDLE_COMMAND)
            .args(["run", "--command", "pg_restore", BUNDLE])
            .args(argv)
            .stdin(Stdio::from(copy))
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| format!("{BUNDLE_COMMAND} did not start: {error}"))?;
        let said = child.stderr.take().map_or_default(kept_words);
        let status = child.wait().map_err(|error| format!("pg_restore: {error}"))?;
        if status.success() {
            Ok(())
        } else {
            let how = status.code().map_or_else(
                || "was ended by a signal".to_string(),
                |code| format!("ended with status {code}"),
            );
            Err(if said.is_empty() { format!("pg_restore {how}") } else { said })
        }
    }

    fn pause(&mut self, time: Duration) {
        thread::sleep(time);
    }

    fn log(&mut self, message: &str) {
        if let Err(failed) = logger::log("database", 6, message) {
            eprintln!("database: cannot write to the log: {}", failed.message);
        }
    }
}

/// The first [`SAID_MAX`] bytes of what a program wrote to `stream`, as
/// text, with a line that says so when there was more. The rest is read
/// and left, so that the program does not stop on a full pipe.
fn kept_words(mut stream: impl Read) -> String {
    let mut kept = Vec::new();
    let _ = (&mut stream).take(SAID_MAX as u64).read_to_end(&mut kept);
    let more = io::copy(&mut stream, &mut io::sink()).is_ok_and(|rest| rest > 0);
    let mut words = String::from_utf8_lossy(&kept).trim_end().to_string();
    if more {
        // Not in the middle of a line.
        if let Some(end) = words.rfind('\n') {
            words.truncate(end);
        }
        words.push_str("\n(more of what pg_restore said is not kept)");
    }
    words
}

/// When the server started, to see that a restart made a new one.
const START_TIME: &str = "SELECT pg_catalog.pg_postmaster_start_time()::text";

/// The lock a change of `database` holds: only one at a time, so that two
/// changes of the files and of the server's settings do not mix.
pub struct Lock {
    /// The file, locked while it is open.
    _file: File,
}

impl Lock {
    /// Takes the lock in `folder`, which is made if it is not there,
    /// waiting for whoever has it.
    ///
    /// # Errors
    ///
    /// When the folder or the file cannot be made, or the lock stays
    /// taken.
    pub fn take(folder: &Path, wait: Duration) -> Result<Lock, Fail> {
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(folder)
            .map_err(|error| crate::util::failed(&folder.display(), error))?;
        let path = folder.join("lock");
        let file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .open(&path)
            .map_err(|error| crate::util::failed(&path.display(), error))?;
        let deadline = Instant::now() + wait;
        loop {
            match file.try_lock() {
                Ok(()) => return Ok(Lock { _file: file }),
                Err(TryLockError::WouldBlock) if Instant::now() < deadline => {
                    thread::sleep(Duration::from_millis(100));
                }
                Err(TryLockError::WouldBlock) => {
                    return Err(fail(
                        "another database command is changing PostgreSQL; try again".into(),
                    ));
                }
                Err(TryLockError::Error(error)) => {
                    return Err(crate::util::failed(&path.display(), error));
                }
            }
        }
    }

    /// The lock of the system's.
    ///
    /// # Errors
    ///
    /// As [`Lock::take`].
    pub fn system() -> Result<Lock, Fail> {
        Lock::take(Path::new(STATE), LOCK_WAIT)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_first_value_is_the_first_cell() {
        let answer = Answer {
            columns: vec!["listen_addresses".into()],
            rows: vec![vec![Some("*".into())], vec![Some("x".into())]],
            tag: "SHOW".into(),
        };
        assert_eq!(value(std::slice::from_ref(&answer)), Some("*"));
        assert_eq!(value(&[]), None);
        let empty = Answer { rows: vec![vec![None]], ..Answer::default() };
        assert_eq!(value(&[empty]), None);
        assert_eq!(value(&[Answer::default()]), None);
    }

    #[test]
    fn the_files_are_the_service_its_override_and_the_firewall_s_rules() {
        let files = Files::system();
        assert_eq!(files.service, Path::new("/system/settings/spark/services/postgresql.toml"));
        assert_eq!(
            files.over,
            Path::new("/system/settings/spark/services/settings.d/postgresql.toml")
        );
        assert_eq!(files.firewall, Path::new("/system/settings/firewall"));
    }

    #[test]
    fn a_second_change_waits_and_then_gives_up() {
        let dir = std::env::temp_dir().join(format!("database-lock-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let first = Lock::take(&dir, Duration::from_millis(50)).unwrap();
        let second = Lock::take(&dir, Duration::from_millis(250));
        assert!(second.is_err_and(|failed| failed.message.contains("try again")));
        drop(first);
        assert!(Lock::take(&dir, Duration::from_millis(50)).is_ok());
        let _ = fs::remove_dir_all(&dir);
    }
}
