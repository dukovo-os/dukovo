// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! A machine in memory for the tests of `database`: the part of
//! PostgreSQL's answers that the commands read, with the files it reads
//! in a folder of their own, and every statement, log line and restart
//! noted.

use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::Seek;
use std::os::unix::fs::FileExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, UNIX_EPOCH};

use super::server::{Clock, Files, Folder, Host, Service};
use super::wire::{Answer, Error, PgError};
use super::{hba, names::Network, sql};
use crate::util::Fail;

/// A folder for each fake, so that tests that run together do not meet.
static FOLDERS: AtomicU64 = AtomicU64::new(0);

/// The bundle's `pg_hba.conf`, as far as the tests need it.
const BUNDLE_HBA: &str = "\
# TYPE  DATABASE     USER      ADDRESS         METHOD

local   all          postgres                  peer map=administrators
local   all          all                       peer
host    all          all       127.0.0.1/32    scram-sha-256
host    all          all       ::1/128         scram-sha-256
";

/// What a bundle's service file says, as far as the tests need it.
const SERVICE_FILE: &str = "\
service_version = 1
description = \"PostgreSQL, the database server\"
program = \"/applications/org.postgresql.postgresql/bin/postgresql-service\"
";

/// What a backup's hook is, as the service file says.
const COPYING_HOOK: &str = "\
[backup]
before = [\"/applications/org.postgresql.postgresql/bin/postgresql-backup\"]
time_limit = 3600
";

/// What the hook was before it copied: a checkpoint.
const CHECKPOINT_HOOK: &str = "\
[backup]
before = [\"/applications/org.postgresql.postgresql/bin/psql\", \"-X\", \"-q\", \"-c\", \"CHECKPOINT\"]
";

/// 2026-11-16 02:00 UTC, in milliseconds since 1970, which is what the
/// fake's clock says at first.
pub const START_MS: u64 = 1_794_794_400_000;

/// A role.
#[derive(Clone, Debug)]
struct Role {
    /// Whether it has a password.
    password: bool,
    /// Its comment.
    comment: Option<String>,
}

/// A database.
#[derive(Clone, Debug)]
struct Database {
    /// The role that owns it.
    owner: String,
    /// Its comment.
    comment: Option<String>,
    /// `c` or `b`.
    provider: char,
    /// The locale.
    locale: String,
    /// The collation version it recorded, and the system's now.
    versions: Option<(String, String)>,
    /// Whether it takes connections.
    open: bool,
    /// How many connections it takes besides the superuser's: -1 for any.
    limit: i32,
}

/// A call of `pg_restore`, as the fake saw it.
#[derive(Clone, Debug)]
pub struct RestoreCall {
    /// What it was given as arguments.
    pub argv: Vec<String>,
    /// How long the copy it was given to read is.
    pub length: u64,
    /// Where in the copy it was to start reading.
    pub offset: u64,
    /// Whether the database it put the copy into took no connection but
    /// the superuser's.
    pub closed: bool,
}

/// A statement the machine refuses.
struct Refusal {
    /// What the statement starts with.
    prefix: String,
    /// The SQLSTATE.
    code: String,
    /// The words.
    message: String,
}

/// The machine.
pub struct Fake {
    /// Where its files are.
    folder: PathBuf,
    /// The files of the service.
    files: Files,
    /// Whether the service is installed, turned on, and running.
    service: Service,
    /// Whether the server lets root in.
    let_in: bool,
    /// The accounts.
    accounts: Vec<String>,
    /// The certificates certd keeps.
    certificates: Vec<String>,
    /// The roles.
    roles: BTreeMap<String, Role>,
    /// The databases.
    databases: BTreeMap<String, Database>,
    /// The sessions of each database: program and count.
    sessions: BTreeMap<String, Vec<(String, u64)>>,
    /// What `postgresql.auto.conf` sets.
    auto: BTreeMap<String, String>,
    /// What the server runs with: listen, ssl and the certificate file.
    running: BTreeMap<String, String>,
    /// The statements the server refuses.
    refusals: Vec<Refusal>,
    /// A mistake in the settings that PostgreSQL finds.
    settings_error: Option<String>,
    /// Every statement sent.
    sent: Vec<String>,
    /// Every line logged.
    logged: Vec<String>,
    /// How many times the server read its files again, and started.
    reloads: usize,
    /// The restarts that are still to fail.
    restarts_to_fail: usize,
    /// How many times it was restarted.
    restarts: usize,
    /// What the TLS check says, when it fails.
    probe_error: Option<String>,
    /// Whether the server never says it has read its files again.
    deaf: bool,
    /// When it last read them.
    loaded: u64,
    /// The time now, and how much each look at it moves it on.
    clock: Clock,
    step: u64,
    /// The snapshots that hold copies, oldest first.
    snapshots: Vec<String>,
    /// Every call of `pg_restore`.
    restores: Vec<RestoreCall>,
    /// What `pg_restore` says when it fails.
    restore_error: Option<String>,
    /// The databases a copy was put into, and how long the copy was.
    restored: BTreeMap<String, u64>,
}

impl Fake {
    /// A machine with PostgreSQL running, no database and the bundle's
    /// `pg_hba.conf`.
    ///
    /// # Panics
    ///
    /// When its folder cannot be made.
    pub fn new() -> Fake {
        let folder = std::env::temp_dir().join(format!(
            "database-fake-{}-{}",
            std::process::id(),
            FOLDERS.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&folder);
        fs::create_dir_all(folder.join("data")).unwrap();
        fs::create_dir_all(folder.join("services")).unwrap();
        fs::write(folder.join("data/pg_hba.conf"), BUNDLE_HBA).unwrap();
        fs::write(folder.join("services/postgresql.toml"), SERVICE_FILE).unwrap();
        let files = Files {
            service: folder.join("services/postgresql.toml"),
            over: folder.join("services/settings.d/postgresql.toml"),
        };
        let mut roles = BTreeMap::new();
        roles.insert("postgres".to_string(), Role { password: false, comment: None });
        let running =
            [("listen_addresses", "localhost"), ("ssl", "off"), ("ssl_cert_file", "server.crt")]
                .iter()
                .map(|(name, value)| ((*name).to_string(), (*value).to_string()))
                .collect();
        Fake {
            folder,
            files,
            service: Service { installed: true, enabled: true, running: true },
            let_in: true,
            accounts: Vec::new(),
            certificates: vec!["this-machine".into()],
            roles,
            databases: BTreeMap::new(),
            sessions: BTreeMap::new(),
            auto: BTreeMap::new(),
            running,
            refusals: Vec::new(),
            settings_error: None,
            sent: Vec::new(),
            logged: Vec::new(),
            reloads: 0,
            restarts_to_fail: 0,
            restarts: 0,
            probe_error: None,
            deaf: false,
            loaded: 1,
            clock: Clock { ms: START_MS, local: (2026, 11, 16, 2, 0) },
            step: 0,
            snapshots: Vec::new(),
            restores: Vec::new(),
            restore_error: None,
            restored: BTreeMap::new(),
        }
    }

    /// PostgreSQL is not installed.
    #[must_use]
    pub fn not_installed(mut self) -> Fake {
        self.service = Service { installed: false, enabled: false, running: false };
        self
    }

    /// PostgreSQL is installed and turned off.
    #[must_use]
    pub fn off(mut self) -> Fake {
        self.service = Service { installed: true, enabled: false, running: false };
        self
    }

    /// The server asks root for a password.
    #[must_use]
    pub fn not_let_in(mut self) -> Fake {
        self.let_in = false;
        self
    }

    /// The server has an account called `name`.
    #[must_use]
    pub fn with_account(mut self, name: &str) -> Fake {
        self.accounts.push(name.into());
        self
    }

    /// The server has a role called `name`, with a password.
    #[must_use]
    pub fn with_role(mut self, name: &str) -> Fake {
        self.roles.insert(name.into(), Role { password: true, comment: None });
        self
    }

    /// The server has a database called `name`, owned by `owner`, which
    /// is made if it is not there, with a password.
    #[must_use]
    pub fn with_database(mut self, name: &str, owner: &str) -> Fake {
        self.roles
            .entry(owner.to_string())
            .or_insert(Role { password: owner != "postgres", comment: None });
        self.databases.insert(
            name.into(),
            Database {
                owner: owner.into(),
                comment: None,
                provider: 'c',
                locale: "en_US.UTF-8".into(),
                versions: None,
                open: true,
                limit: -1,
            },
        );
        self
    }

    /// The copy of `name` that the hook made is `bytes` long and was
    /// written at `at_ms`.
    ///
    /// # Panics
    ///
    /// When the file cannot be written.
    #[must_use]
    pub fn with_copy(self, name: &str, bytes: usize, at_ms: u64) -> Fake {
        write_copy(&self.folder.join("dumps"), name, bytes, at_ms);
        self
    }

    /// The snapshot `snapshot`, which is newer than those before it, has a
    /// copy of `name` of `bytes` that was written at `at_ms`.
    ///
    /// # Panics
    ///
    /// When the file cannot be written.
    #[must_use]
    pub fn with_snapshot_copy(
        mut self,
        snapshot: &str,
        name: &str,
        bytes: usize,
        at_ms: u64,
    ) -> Fake {
        write_copy(&self.snapshot_folder(snapshot), name, bytes, at_ms);
        if !self.snapshots.iter().any(|one| one == snapshot) {
            self.snapshots.push(snapshot.into());
        }
        self
    }

    /// The folder of the copies in the snapshot `snapshot`.
    pub fn snapshot_folder(&self, snapshot: &str) -> PathBuf {
        self.folder.join("snapshots").join(snapshot).join("dumps")
    }

    /// The folder of the copies now.
    pub fn copies_folder(&self) -> PathBuf {
        self.folder.join("dumps")
    }

    /// The service's backup hook is the one that copies the databases.
    ///
    /// # Panics
    ///
    /// When the file cannot be written.
    #[must_use]
    pub fn with_copying_hook(self) -> Fake {
        self.with_hook(COPYING_HOOK)
    }

    /// The service's backup hook only writes a checkpoint.
    ///
    /// # Panics
    ///
    /// When the file cannot be written.
    #[must_use]
    pub fn with_checkpoint_hook(self) -> Fake {
        self.with_hook(CHECKPOINT_HOOK)
    }

    /// The service file ends with `hook`.
    fn with_hook(self, hook: &str) -> Fake {
        fs::write(&self.files.service, format!("{SERVICE_FILE}\n{hook}")).unwrap();
        self
    }

    /// The clock says `local` and `ms`, and moves `step` milliseconds on
    /// each time it is looked at.
    #[must_use]
    pub fn at(mut self, local: (i64, u32, u32, u32, u32), ms: u64, step: u64) -> Fake {
        self.clock = Clock { ms, local };
        self.step = step;
        self
    }

    /// `pg_restore` ends with `words` on its error output.
    #[must_use]
    pub fn failing_restore(mut self, words: &str) -> Fake {
        self.restore_error = Some(words.into());
        self
    }

    /// Every call of `pg_restore`.
    pub fn restores(&self) -> &[RestoreCall] {
        &self.restores
    }

    /// How long the copy was that was put into `name`, if one was.
    pub fn restored(&self, name: &str) -> Option<u64> {
        self.restored.get(name).copied()
    }

    /// The sessions open to `name`: how many, of every program.
    pub fn open_sessions(&self, name: &str) -> u64 {
        self.sessions.get(name).map_or(0, |programs| programs.iter().map(|(_, n)| n).sum())
    }

    /// Whether `name` takes connections, and how many besides the
    /// superuser's: the fake's own record of what was asked.
    pub fn takes(&self, name: &str) -> Option<(bool, i32)> {
        self.databases.get(name).map(|db| (db.open, db.limit))
    }

    /// The comment of the database `name`.
    pub fn comment_of(&self, name: &str) -> Option<String> {
        self.databases.get(name)?.comment.clone()
    }

    /// The role `name` has no password.
    #[must_use]
    pub fn without_password(mut self, name: &str) -> Fake {
        if let Some(role) = self.roles.get_mut(name) {
            role.password = false;
        }
        self
    }

    /// `role` was made by `database add` for `database`.
    #[must_use]
    pub fn made_by_database(mut self, database: &str, role: &str) -> Fake {
        if let Some(role) = self.roles.get_mut(role) {
            role.comment = Some(format!("{}{database}", sql::ROLE_COMMENT));
        }
        self
    }

    /// `database` was made for the website `site`.
    #[must_use]
    pub fn with_site(mut self, database: &str, site: &str) -> Fake {
        if let Some(one) = self.databases.get_mut(database) {
            one.comment = Some(format!("{}{site}", sql::SITE_COMMENT));
        }
        self
    }

    /// `program` has `count` sessions open to `database`.
    #[must_use]
    pub fn with_sessions(mut self, database: &str, program: &str, count: u64) -> Fake {
        self.sessions.entry(database.into()).or_default().push((program.into(), count));
        self
    }

    /// `database` recorded the collation version `recorded`, and the
    /// system says `actual` now.
    #[must_use]
    pub fn with_collation(mut self, database: &str, recorded: &str, actual: &str) -> Fake {
        if let Some(one) = self.databases.get_mut(database) {
            one.versions = Some((recorded.into(), actual.into()));
        }
        self
    }

    /// Refuses each statement that starts with `prefix`.
    #[must_use]
    pub fn refusing(mut self, prefix: &str, code: &str, message: &str) -> Fake {
        self.refusals.push(Refusal {
            prefix: prefix.into(),
            code: code.into(),
            message: message.into(),
        });
        self
    }

    /// The server already listens on the network with `certificate`.
    #[must_use]
    pub fn listening_with(mut self, certificate: &str) -> Fake {
        for change in super::settings::Settings::default().to_allow(certificate) {
            if let Some(value) = change.to {
                self.auto.insert(change.name.to_string(), value);
            }
        }
        self.write_auto();
        self.apply_files();
        self
    }

    /// `pg_hba.conf` has the block with these lines: database, role and
    /// network.
    #[must_use]
    pub fn with_hba_lines(self, lines: &[(&str, &str, &str)]) -> Fake {
        let rules: Vec<hba::Rule> = lines
            .iter()
            .map(|(database, role, network)| hba::Rule {
                database: (*database).into(),
                role: (*role).into(),
                network: Network::parse(network).unwrap(),
            })
            .collect();
        let text = hba::with_rules(&self.hba_text(), &rules).unwrap();
        self.write_hba(&text);
        self
    }

    /// The server restarts `times` without coming back first.
    #[must_use]
    pub fn failing_to_restart(mut self, times: usize) -> Fake {
        self.restarts_to_fail = times;
        self
    }

    /// The TLS check says `why`.
    #[must_use]
    pub fn failing_the_tls_check(mut self, why: &str) -> Fake {
        self.probe_error = Some(why.into());
        self
    }

    /// PostgreSQL finds a mistake in its settings, in `words`.
    #[must_use]
    pub fn with_settings_error(mut self, words: &str) -> Fake {
        self.settings_error = Some(words.into());
        self
    }

    /// The server never says it has read its files again.
    #[must_use]
    pub fn deaf(mut self) -> Fake {
        self.deaf = true;
        self
    }

    /// Every statement sent.
    pub fn sent(&self) -> &[String] {
        &self.sent
    }

    /// The statements that change something.
    pub fn changes(&self) -> Vec<String> {
        self.sent
            .iter()
            .filter(|statement| {
                ["CREATE ", "COMMENT ", "REVOKE ", "DROP ", "ALTER "]
                    .iter()
                    .any(|start| statement.starts_with(start))
            })
            .cloned()
            .collect()
    }

    /// Every line logged.
    pub fn logged(&self) -> &[String] {
        &self.logged
    }

    /// How many times the server read its files again.
    pub fn reloads(&self) -> usize {
        self.reloads
    }

    /// How many times it was restarted.
    pub fn restarts(&self) -> usize {
        self.restarts
    }

    /// Whether the server has the role.
    pub fn has_role(&self, name: &str) -> bool {
        self.roles.contains_key(name)
    }

    /// Whether the server has the database.
    pub fn has_database(&self, name: &str) -> bool {
        self.databases.contains_key(name)
    }

    /// What `pg_hba.conf` says.
    ///
    /// # Panics
    ///
    /// When the file is gone.
    pub fn hba_text(&self) -> String {
        fs::read_to_string(self.folder.join("data/pg_hba.conf")).unwrap()
    }

    /// Writes `pg_hba.conf`.
    ///
    /// # Panics
    ///
    /// When it cannot be written.
    pub fn write_hba(&self, text: &str) {
        fs::write(self.folder.join("data/pg_hba.conf"), text).unwrap();
    }

    /// The lines of `database`'s block.
    pub fn hba_rules(&self) -> Vec<String> {
        hba::rules(&self.hba_text()).unwrap().iter().map(hba::Rule::line).collect()
    }

    /// What the override of the service says, none for no file.
    pub fn override_text(&self) -> Option<String> {
        fs::read_to_string(&self.files.over).ok()
    }

    /// What `postgresql.auto.conf` sets.
    pub fn auto(&self) -> &BTreeMap<String, String> {
        &self.auto
    }

    /// What `postgresql.auto.conf` holds as a file.
    pub fn auto_file(&self) -> Option<String> {
        fs::read_to_string(self.folder.join("data/postgresql.auto.conf")).ok()
    }

    /// What the server runs with now for `name`.
    pub fn running(&self, name: &str) -> &str {
        self.running.get(name).map_or("", String::as_str)
    }

    /// Writes `postgresql.auto.conf` from what is set.
    fn write_auto(&self) {
        let mut text = String::new();
        for (name, value) in &self.auto {
            text.push_str(&format!("{name} = '{value}'\n"));
        }
        fs::write(self.folder.join("data/postgresql.auto.conf"), text).unwrap();
    }

    /// Reads `postgresql.auto.conf` into what is set, as the server
    /// reads its files.
    fn read_auto(&mut self) {
        self.auto.clear();
        let Ok(text) = fs::read_to_string(self.folder.join("data/postgresql.auto.conf")) else {
            return;
        };
        for line in text.lines() {
            if let Some((name, value)) = line.split_once(" = ") {
                self.auto.insert(name.into(), value.trim_matches('\'').into());
            }
        }
    }

    /// What the server runs with after it reads the files: the part
    /// that a restart or a reload takes.
    fn apply_files(&mut self) {
        self.read_auto();
        for (name, value) in self.auto.clone() {
            self.running.insert(name, value);
        }
        // What is not in the file is the default.
        for (name, default) in [("listen_addresses", "localhost"), ("ssl", "off")] {
            if !self.auto.contains_key(name) {
                self.running.insert(name.into(), default.into());
            }
        }
    }

    /// An answer of `columns` and `rows`.
    fn answer(columns: &[&str], rows: Vec<Vec<Option<String>>>) -> Vec<Answer> {
        vec![Answer {
            columns: columns.iter().map(|c| (*c).to_string()).collect(),
            tag: format!("SELECT {}", rows.len()),
            rows,
        }]
    }

    /// One value, as a `SHOW` answers.
    fn shown(name: &str, value: &str) -> Vec<Answer> {
        Fake::answer(&[name], vec![vec![Some(value.into())]])
    }

    /// The statement `sql` answers, as the server would.
    #[allow(clippy::too_many_lines)]
    fn answer_of(&mut self, sql: &str) -> Result<Vec<Answer>, Error> {
        let ok = |tag: &str| vec![Answer { tag: tag.into(), ..Answer::default() }];
        if sql == "SELECT 1" {
            return Ok(Fake::shown("?column?", "1"));
        }
        if let Some(name) = sql.strip_prefix("SHOW ") {
            return Ok(match name {
                "server_version" => Fake::shown(name, "19.0"),
                "hba_file" => {
                    Fake::shown(name, &self.folder.join("data/pg_hba.conf").display().to_string())
                }
                "data_directory" => {
                    Fake::shown(name, &self.folder.join("data").display().to_string())
                }
                other => Fake::shown(other, self.running(other)),
            });
        }
        if let Some(rest) =
            sql.strip_prefix("SELECT 1 FROM pg_catalog.pg_database WHERE datname = ")
        {
            let found = self.databases.contains_key(&literal_of(rest));
            return Ok(Fake::answer(
                &["?column?"],
                if found { vec![vec![Some("1".into())]] } else { vec![] },
            ));
        }
        if let Some(rest) = sql.strip_prefix("SELECT 1 FROM pg_catalog.pg_roles WHERE rolname = ") {
            let found = self.roles.contains_key(&literal_of(rest));
            return Ok(Fake::answer(
                &["?column?"],
                if found { vec![vec![Some("1".into())]] } else { vec![] },
            ));
        }
        if sql.starts_with("SELECT pg_catalog.pg_get_userbyid(d.datdba) AS owner, r.rolpassword") {
            let name = literal_of(sql.rsplit("WHERE d.datname = ").next().unwrap_or(""));
            let rows = self.databases.get(&name).map(|db| {
                let role = &self.roles[&db.owner];
                vec![
                    Some(db.owner.clone()),
                    Some(if role.password { "t" } else { "f" }.into()),
                    role.comment.clone(),
                ]
            });
            return Ok(Fake::answer(&["owner", "password", "comment"], rows.into_iter().collect()));
        }
        if sql.starts_with(
            "SELECT pg_catalog.pg_get_userbyid(d.datdba) AS owner, pg_catalog.pg_encoding",
        ) {
            let name = literal_of(sql.rsplit("WHERE d.datname = ").next().unwrap_or(""));
            let rows = self.databases.get(&name).map(|db| {
                let libc = db.provider == 'c';
                vec![
                    Some(db.owner.clone()),
                    Some("UTF8".into()),
                    Some(db.provider.to_string()),
                    libc.then(|| db.locale.clone()),
                    libc.then(|| db.locale.clone()),
                    Some(db.locale.clone()),
                    db.comment.clone(),
                ]
            });
            return Ok(Fake::answer(
                &["owner", "encoding", "provider", "collate", "ctype", "locale", "comment"],
                rows.into_iter().collect(),
            ));
        }
        if let Some(rest) =
            sql.strip_prefix("SELECT pg_catalog.pg_terminate_backend(pid) FROM pg_catalog.pg_stat_activity WHERE datname = ")
        {
            let name = literal_of(rest);
            let ended = self.sessions.remove(&name).unwrap_or_default();
            let rows = ended
                .iter()
                .flat_map(|(_, count)| (0..*count).map(|_| vec![Some("t".to_string())]))
                .collect();
            return Ok(Fake::answer(&["pg_terminate_backend"], rows));
        }
        if sql.starts_with("SELECT application_name, count(*) AS sessions") {
            let name = literal_of(sql.split("WHERE datname = ").nth(1).unwrap_or(""));
            let rows = self.sessions.get(&name).map_or_default(|programs| {
                programs
                    .iter()
                    .map(|(program, count)| vec![Some(program.clone()), Some(count.to_string())])
                    .collect()
            });
            return Ok(Fake::answer(&["application_name", "sessions"], rows));
        }
        if sql == super::sql::LIST_SESSIONS {
            let rows = self
                .sessions
                .iter()
                .map(|(name, programs)| {
                    let total: u64 = programs.iter().map(|(_, count)| count).sum();
                    vec![Some(name.clone()), Some(total.to_string())]
                })
                .collect();
            return Ok(Fake::answer(&["name", "sessions"], rows));
        }
        if sql == super::sql::LIST_DATABASES {
            let rows = self
                .databases
                .iter()
                .map(|(name, db)| {
                    let (recorded, actual) = db.versions.clone().unzip();
                    vec![
                        Some(name.clone()),
                        Some(db.owner.clone()),
                        Some("UTF8".into()),
                        Some(db.provider.to_string()),
                        Some(db.locale.clone()),
                        Some("8192".into()),
                        db.comment.clone(),
                        self.roles[&db.owner].comment.clone(),
                        recorded,
                        actual,
                    ]
                })
                .collect();
            return Ok(Fake::answer(
                &[
                    "name",
                    "owner",
                    "encoding",
                    "provider",
                    "locale",
                    "bytes",
                    "comment",
                    "owner_comment",
                    "recorded",
                    "actual",
                ],
                rows,
            ));
        }
        if sql == super::sql::LIST_ROLES {
            let rows = self
                .roles
                .iter()
                .map(|(name, role)| {
                    vec![
                        Some(name.clone()),
                        Some("t".into()),
                        Some(if role.password { "t" } else { "f" }.into()),
                        role.comment.clone(),
                    ]
                })
                .collect();
            return Ok(Fake::answer(&["name", "login", "password", "comment"], rows));
        }
        if sql == super::sql::LIST_ACCESS {
            return Ok(self.access());
        }
        if sql == super::sql::HBA_ERRORS {
            let rows = self
                .hba_text()
                .lines()
                .enumerate()
                .filter(|(_, line)| line.contains("BROKEN"))
                .map(|(at, _)| {
                    vec![Some((at + 1).to_string()), Some("invalid IP address \"BROKEN\"".into())]
                })
                .collect();
            return Ok(Fake::answer(&["line_number", "error"], rows));
        }
        if sql == super::sql::SETTINGS_ERRORS {
            let mut rows = Vec::new();
            if let Some(words) = &self.settings_error {
                rows.push(vec![
                    Some("/data/postgresql.conf".into()),
                    Some("7".into()),
                    Some("max_connections".into()),
                    Some(words.clone()),
                ]);
            }
            // What a setting that waits for a restart looks like.
            if self
                .auto
                .get("listen_addresses")
                .is_some_and(|wanted| wanted != self.running("listen_addresses"))
            {
                rows.push(vec![
                    Some("/data/postgresql.auto.conf".into()),
                    Some("3".into()),
                    Some("listen_addresses".into()),
                    Some("setting could not be applied".into()),
                ]);
            }
            return Ok(Fake::answer(&["sourcefile", "sourceline", "name", "error"], rows));
        }
        if sql == super::sql::FILE_SETTINGS {
            self.read_auto();
            let file = self.folder.join("data/postgresql.auto.conf").display().to_string();
            let rows = self
                .auto
                .iter()
                .map(|(name, value)| {
                    vec![Some(name.clone()), Some(value.clone()), Some(file.clone())]
                })
                .collect();
            return Ok(Fake::answer(&["name", "setting", "sourcefile"], rows));
        }
        if sql == super::sql::SETTING_NOW {
            let rows = [
                ("listen_addresses", "localhost"),
                ("ssl", "off"),
                ("ssl_cert_file", "server.crt"),
                ("ssl_key_file", "server.key"),
            ]
            .iter()
            .map(|(name, boot)| {
                vec![Some((*name).into()), Some((*boot).into()), Some((*boot).into())]
            })
            .collect();
            return Ok(Fake::answer(&["name", "setting", "boot_val"], rows));
        }
        if sql == super::sql::LOADED_AT {
            if !self.deaf {
                // The server reads its files as soon as it is asked.
            }
            return Ok(Fake::shown("pg_conf_load_time", &self.loaded.to_string()));
        }
        if sql == super::sql::RELOAD {
            self.reloads += 1;
            if !self.deaf {
                self.loaded += 1;
                self.apply_files();
            }
            return Ok(Fake::shown("pg_reload_conf", "t"));
        }
        self.change(sql).map(|tag| ok(&tag))
    }

    /// The rows of `pg_hba_file_rules`.
    fn access(&self) -> Vec<Answer> {
        let mut rows = Vec::new();
        for (at, line) in self.hba_text().lines().enumerate() {
            let number = Some((at + 1).to_string());
            let text = line.split('#').next().unwrap_or("");
            let words: Vec<String> =
                text.split_whitespace().map(|word| word.trim_matches('"').to_string()).collect();
            if words.is_empty() {
                continue;
            }
            if line.contains("BROKEN") {
                rows.push(vec![
                    number,
                    None,
                    None,
                    None,
                    None,
                    None,
                    None,
                    Some("invalid IP address \"BROKEN\"".into()),
                ]);
                continue;
            }
            let json = |word: &str| Some(format!("[\"{word}\"]"));
            let (address, netmask) = match words.get(3).filter(|_| words[0] != "local") {
                Some(cidr) => match cidr.split_once('/') {
                    Some((address, prefix)) => {
                        (Some(address.to_string()), Some(mask_of(address, prefix)))
                    }
                    None => (Some(cidr.clone()), None),
                },
                None => (None, None),
            };
            let method = if words[0] == "local" { words.get(3) } else { words.get(4) };
            rows.push(vec![
                number,
                Some(words[0].clone()),
                json(&words[1]),
                json(&words[2]),
                address,
                netmask,
                method.cloned(),
                None,
            ]);
        }
        Fake::answer(
            &[
                "line_number",
                "type",
                "databases",
                "users",
                "address",
                "netmask",
                "auth_method",
                "error",
            ],
            rows,
        )
    }

    /// Makes the change `sql` says, and gives its tag.
    fn change(&mut self, sql: &str) -> Result<String, Error> {
        let broken = |why: String| Error::Broken(why);
        if let Some(rest) = sql.strip_prefix("CREATE ROLE ") {
            let name = ident_of(rest);
            if self.roles.contains_key(&name) {
                return Err(server("42710", "role already exists"));
            }
            let password = rest.contains(" PASSWORD ");
            self.roles.insert(name, Role { password, comment: None });
            return Ok("CREATE ROLE".into());
        }
        if let Some(rest) = sql.strip_prefix("COMMENT ON ROLE ") {
            let name = ident_of(rest);
            let comment = literal_of(rest.split(" IS ").nth(1).unwrap_or(""));
            self.roles.get_mut(&name).ok_or_else(|| server("42704", "no such role"))?.comment =
                Some(comment);
            return Ok("COMMENT".into());
        }
        if let Some(rest) = sql.strip_prefix("CREATE DATABASE ") {
            let name = ident_of(rest);
            if self.databases.contains_key(&name) {
                return Err(server("42P04", "database already exists"));
            }
            let owner = ident_of(rest.split(" OWNER ").nth(1).unwrap_or(""));
            let builtin = rest.contains("LOCALE_PROVIDER builtin");
            let after = |key: &str| rest.split(key).nth(1).map(literal_of);
            let locale = after("BUILTIN_LOCALE ")
                .or_else(|| after("LC_COLLATE "))
                .or_else(|| after("LOCALE "))
                .unwrap_or_default();
            let limit = if rest.contains(" CONNECTION LIMIT 0") { 0 } else { -1 };
            self.databases.insert(
                name,
                Database {
                    owner,
                    comment: None,
                    provider: if builtin { 'b' } else { 'c' },
                    locale,
                    versions: None,
                    open: true,
                    limit,
                },
            );
            return Ok("CREATE DATABASE".into());
        }
        if let Some(rest) = sql.strip_prefix("COMMENT ON DATABASE ") {
            let name = ident_of(rest);
            let comment = literal_of(rest.split(" IS ").nth(1).unwrap_or(""));
            self.databases
                .get_mut(&name)
                .ok_or_else(|| server("3D000", "no such database"))?
                .comment = Some(comment);
            return Ok("COMMENT".into());
        }
        if sql.starts_with("REVOKE ALL ON DATABASE ") {
            return Ok("REVOKE".into());
        }
        if let Some(rest) = sql.strip_prefix("DROP DATABASE ") {
            let name = ident_of(rest);
            self.databases.remove(&name).ok_or_else(|| server("3D000", "no such database"))?;
            self.sessions.remove(&name);
            return Ok("DROP DATABASE".into());
        }
        if let Some(rest) = sql.strip_prefix("DROP ROLE ") {
            let name = ident_of(rest);
            self.roles.remove(&name).ok_or_else(|| server("42704", "no such role"))?;
            return Ok("DROP ROLE".into());
        }
        if let Some(rest) = sql.strip_prefix("ALTER DATABASE ") {
            let name = ident_of(rest);
            if let Some(to) = rest.split(" RENAME TO ").nth(1) {
                let to = ident_of(to);
                if !self.databases.contains_key(&name) {
                    return Err(server("3D000", "no such database"));
                }
                if self.databases.contains_key(&to) {
                    return Err(server("42P04", "database already exists"));
                }
                if self.open_sessions(&name) > 0 {
                    return Err(server("55006", "database is being accessed by other users"));
                }
                if let Some(db) = self.databases.remove(&name) {
                    self.databases.insert(to, db);
                }
                return Ok("ALTER DATABASE".into());
            }
            let db =
                self.databases.get_mut(&name).ok_or_else(|| server("3D000", "no such database"))?;
            if let Some(allow) = rest.split(" WITH ALLOW_CONNECTIONS ").nth(1) {
                db.open = allow == "true";
            } else if let Some(limit) = rest.split(" WITH CONNECTION LIMIT ").nth(1) {
                db.limit = limit.parse().map_err(|_| broken(format!("fake: {sql}")))?;
            } else {
                return Err(broken(format!("the fake does not know the statement {sql}")));
            }
            return Ok("ALTER DATABASE".into());
        }
        if let Some(rest) = sql.strip_prefix("ALTER ROLE ") {
            let name = ident_of(rest);
            self.roles.get_mut(&name).ok_or_else(|| server("42704", "no such role"))?.password =
                true;
            return Ok("ALTER ROLE".into());
        }
        if let Some(rest) = sql.strip_prefix("ALTER SYSTEM SET ") {
            let (name, value) =
                rest.split_once(" = ").ok_or_else(|| broken(format!("fake: {sql}")))?;
            self.read_auto();
            self.auto.insert(name.into(), literal_of(value));
            self.write_auto();
            return Ok("ALTER SYSTEM".into());
        }
        if let Some(name) = sql.strip_prefix("ALTER SYSTEM RESET ") {
            self.read_auto();
            self.auto.remove(name);
            self.write_auto();
            return Ok("ALTER SYSTEM".into());
        }
        Err(broken(format!("the fake does not know the statement {sql}")))
    }
}

/// Writes the copy of `name` in `folder`: `bytes` long, starting as a
/// copy made with `pg_dump -Fc` does, and last written at `at_ms`.
fn write_copy(folder: &Path, name: &str, bytes: usize, at_ms: u64) {
    fs::create_dir_all(folder).unwrap();
    let path = folder.join(format!("{name}.dump"));
    let mut content = b"PGDMP".to_vec();
    content.resize(bytes.max(content.len()), 0);
    fs::write(&path, content).unwrap();
    let file = OpenOptions::new().write(true).open(&path).unwrap();
    file.set_modified(UNIX_EPOCH + Duration::from_millis(at_ms)).unwrap();
}

/// The quoted identifier at the start of `text`.
fn ident_of(text: &str) -> String {
    let inner = text.trim_start().strip_prefix('"').unwrap_or("");
    let mut name = String::new();
    let mut chars = inner.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '"' {
            if chars.peek() == Some(&'"') {
                chars.next();
                name.push('"');
                continue;
            }
            break;
        }
        name.push(c);
    }
    name
}

/// The quoted string at the start of `text`.
fn literal_of(text: &str) -> String {
    let inner = text.trim_start().strip_prefix('\'').unwrap_or("");
    let mut out = String::new();
    let mut chars = inner.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\'' {
            if chars.peek() == Some(&'\'') {
                chars.next();
                out.push('\'');
                continue;
            }
            break;
        }
        out.push(c);
    }
    out
}

/// A refusal of the server.
fn server(code: &str, message: &str) -> Error {
    Error::Server(Box::new(PgError {
        severity: "ERROR".into(),
        code: code.into(),
        message: message.into(),
        ..PgError::default()
    }))
}

/// The netmask `pg_hba_file_rules` gives for a prefix.
fn mask_of(address: &str, prefix: &str) -> String {
    let prefix: u32 = prefix.parse().unwrap_or(0);
    if address.contains(':') {
        let mask = u128::MAX.checked_shl(128 - prefix).unwrap_or(0);
        std::net::Ipv6Addr::from(mask).to_string()
    } else {
        let mask = u32::MAX.checked_shl(32 - prefix).unwrap_or(0);
        std::net::Ipv4Addr::from(mask).to_string()
    }
}

impl Drop for Fake {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.folder);
    }
}

impl Host for Fake {
    fn sql(&mut self, sql: &str) -> Result<Vec<Answer>, Error> {
        if !self.let_in {
            return Err(Error::NotLetIn(3));
        }
        self.sent.push(sql.to_string());
        for refusal in &self.refusals {
            if sql.starts_with(&refusal.prefix) {
                return Err(server(&refusal.code, &refusal.message));
            }
        }
        self.answer_of(sql)
    }

    fn service(&mut self) -> Result<Service, Fail> {
        Ok(self.service)
    }

    fn restart(&mut self) -> Result<(), String> {
        self.restarts += 1;
        if self.restarts_to_fail > 0 {
            self.restarts_to_fail -= 1;
            return Err("PostgreSQL did not come back within 60 seconds: it did not answer".into());
        }
        self.loaded += 1;
        self.apply_files();
        Ok(())
    }

    fn has_certificate(&mut self, name: &str) -> Result<bool, String> {
        Ok(self.certificates.iter().any(|one| one == name))
    }

    fn is_account(&mut self, name: &str) -> bool {
        self.accounts.iter().any(|one| one == name)
    }

    fn probe(&mut self, name: &str) -> Result<(), String> {
        self.sent.push(format!("(a TLS check with {name})"));
        match &self.probe_error {
            Some(why) => Err(why.clone()),
            None => Ok(()),
        }
    }

    fn files(&self) -> &Files {
        &self.files
    }

    fn dumps(&self) -> PathBuf {
        self.copies_folder()
    }

    fn snapshot_dumps(&mut self) -> Vec<Folder> {
        self.snapshots
            .iter()
            .rev()
            .map(|snapshot| Folder { from: snapshot.clone(), path: self.snapshot_folder(snapshot) })
            .collect()
    }

    fn now(&mut self) -> Clock {
        let now = self.clock;
        self.clock.ms += self.step;
        now
    }

    fn pg_restore(&mut self, argv: &[String], mut copy: File) -> Result<(), String> {
        let length = copy.metadata().map_or(0, |meta| meta.len());
        let offset = copy.stream_position().unwrap_or(u64::MAX);
        let target = argv
            .iter()
            .position(|word| word == "-d")
            .and_then(|at| argv.get(at + 1))
            .cloned()
            .unwrap_or_default();
        let closed = self.databases.get(&target).is_some_and(|db| db.limit == 0);
        self.restores.push(RestoreCall { argv: argv.to_vec(), length, offset, closed });
        let mut magic = [0u8; 5];
        if copy.read_exact_at(&mut magic, 0).is_err() || &magic != b"PGDMP" {
            return Err(
                "pg_restore: error: input file does not appear to be a valid archive".into()
            );
        }
        if let Some(words) = &self.restore_error {
            return Err(words.clone());
        }
        if !self.databases.contains_key(&target) {
            return Err(format!("pg_restore: error: database \"{target}\" does not exist"));
        }
        self.restored.insert(target, length);
        Ok(())
    }

    fn pause(&mut self, _time: Duration) {}

    fn log(&mut self, message: &str) {
        self.logged.push(message.to_string());
    }
}
