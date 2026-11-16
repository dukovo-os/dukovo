// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! `database add`, `password` and `remove`: a database and the role that
//! owns it made, its role's password replaced, and both taken away again.
//!
//! A role is given only the SCRAM verifier of its password, made here:
//! the password reaches PostgreSQL and its log never, and lives in
//! [`Added`] until it is shown, which is once, as JSON, as the person
//! asked. Nothing here writes it to a log or to an error.

use ring::rand::SecureRandom;
use serde::Serialize;

use super::scram::{self, Password};
use super::server::{Host, PORT};
use super::wire::{Answer, Error};
use super::{allow, errors, sql};
use crate::json;
use crate::util::{Fail, fail, usage};

/// What `add` is asked to make.
#[derive(Debug, PartialEq, Eq)]
pub struct Add {
    /// The database.
    pub name: String,
    /// The role that owns it.
    pub owner: String,
    /// The account the role signs in as, by the socket, in place of a
    /// password.
    pub peer: Option<String>,
    /// How it sorts text.
    pub locale: String,
    /// The website it is for.
    pub site: Option<String>,
}

/// What `add` and `password` made: where to connect and, for a role with
/// a password, the password, which is told once.
#[derive(Debug)]
pub struct Added {
    /// The database.
    pub database: String,
    /// The role that owns it.
    pub role: String,
    /// The role's password, none for a role that signs in by the socket.
    pub password: Option<Password>,
    /// The host programs on this machine connect to.
    pub host: Option<&'static str>,
    /// The port.
    pub port: u16,
}

impl Added {
    /// The line `--json` prints: the database, the role, the password,
    /// the host and the port. It holds the password, and is the one
    /// place that does.
    ///
    /// # Errors
    ///
    /// When the document cannot be written.
    pub fn line(&self) -> Result<String, Fail> {
        /// The members after `format`.
        #[derive(Serialize)]
        struct Document<'a> {
            /// The database.
            database: &'a str,
            /// The role that owns it.
            role: &'a str,
            /// The password, none for a role that signs in by its account.
            password: Option<&'a str>,
            /// Where programs on this machine connect.
            host: Option<&'a str>,
            /// The port.
            port: u16,
        }
        json::line(&Document {
            database: &self.database,
            role: &self.role,
            password: self.password.as_ref().map(Password::as_str),
            host: self.host,
            port: self.port,
        })
    }
}

/// The address programs on this machine connect to a database at.
pub const HOST: &str = "127.0.0.1";

/// The failure for a statement of `add` that PostgreSQL refused: a name
/// that is taken is DB-102, `what` saying which, and anything else is
/// PostgreSQL's own words.
fn refused(error: &Error, what: &str, name: &str) -> Fail {
    match error {
        Error::Server(pg) if pg.is_taken() => errors::taken(what, name),
        other => errors::said(other),
    }
}

/// Fails with DB-101 unless PostgreSQL runs and lets root in.
///
/// # Errors
///
/// DB-101, in the words of what is wrong.
pub fn ready(host: &mut dyn Host) -> Result<(), Fail> {
    let service = host.service()?;
    if !service.installed {
        return Err(errors::not_installed());
    }
    if !service.running {
        return Err(errors::not_running(service.enabled));
    }
    host.sql("SELECT 1").map(|_| ()).map_err(|error| errors::not_let_in(&error))
}

/// A statement from a builder that refuses a NUL: no name that passed the
/// rule has one.
pub fn built(statement: Result<String, String>) -> Result<String, Fail> {
    statement.map_err(fail)
}

/// Runs `sql` and says what it answered, as a failure when PostgreSQL
/// refuses it.
pub fn run(host: &mut dyn Host, sql: &str) -> Result<Vec<Answer>, Fail> {
    host.sql(sql).map_err(|error| errors::said(&error))
}

/// Whether the lookup `sql` finds a row.
fn found(host: &mut dyn Host, sql: &str) -> Result<bool, Fail> {
    Ok(run(host, sql)?.first().is_some_and(|answer| !answer.rows.is_empty()))
}

/// The verifier PostgreSQL is given for a new password, and the password.
fn new_password(random: &dyn SecureRandom) -> Result<(Password, String), Fail> {
    let password = Password::new(random).map_err(fail)?;
    let salt = scram::salt(random).map_err(fail)?;
    let verifier = scram::verifier(password.bytes(), &salt, scram::ITERATIONS);
    Ok((password, verifier))
}

/// `database add`: makes the role, then the database it owns, each
/// statement alone, and takes the role and the database away again when a
/// statement after the role is refused.
///
/// # Errors
///
/// DB-101, DB-102 or DB-106 as the command says; PostgreSQL's words for
/// anything else.
pub fn add(host: &mut dyn Host, ask: &Add, random: &dyn SecureRandom) -> Result<Added, Fail> {
    ready(host)?;
    if let Some(account) = &ask.peer
        && !host.is_account(account)
    {
        return Err(errors::no_account(account));
    }
    if found(host, &built(sql::database_exists(&ask.name))?)? {
        return Err(errors::taken("database", &ask.name));
    }
    if found(host, &built(sql::role_exists(&ask.owner))?)? {
        return Err(errors::taken("role", &ask.owner));
    }
    let (password, verifier) = match &ask.peer {
        Some(_) => (None, None),
        None => {
            let (password, verifier) = new_password(random)?;
            (Some(password), Some(verifier))
        }
    };
    let mut made = Made::default();
    let result = make(host, ask, verifier.as_deref(), &mut made);
    if let Err(failure) = result {
        return Err(made.undo(host, ask, failure));
    }
    let way = if ask.peer.is_some() { "signs in by its account" } else { "has a password" };
    host.log(&format!("added {} (owner {}, {}; the role {way})", ask.name, ask.owner, ask.locale));
    Ok(Added {
        database: ask.name.clone(),
        role: ask.owner.clone(),
        host: password.is_some().then_some(HOST),
        password,
        port: PORT,
    })
}

/// What `add` has made so far.
#[derive(Default)]
struct Made {
    /// The role is there.
    role: bool,
    /// The database is there.
    database: bool,
}

impl Made {
    /// Takes back what was made, after `failure`, and gives the failure
    /// with how that went, when it did not.
    fn undo(&self, host: &mut dyn Host, ask: &Add, failure: Fail) -> Fail {
        let mut left = Vec::new();
        if self.database {
            let dropped = sql::drop_database(&ask.name, false)
                .map_err(fail)
                .and_then(|statement| run(host, &statement));
            if let Err(why) = dropped {
                left.push(format!("the database {} stays ({})", ask.name, why.message));
            }
        }
        if self.role && left.is_empty() {
            let dropped = sql::drop_role(&ask.owner)
                .map_err(fail)
                .and_then(|statement| run(host, &statement));
            if let Err(why) = dropped {
                left.push(format!("the role {} stays ({})", ask.owner, why.message));
            }
        } else if self.role {
            left.push(format!("the role {} stays as the database does", ask.owner));
        }
        if left.is_empty() {
            return failure;
        }
        let mut failure = failure;
        failure.message = format!("{}; {}", failure.message, left.join(", "));
        failure
    }
}

/// The statements of `add`, each alone, noting in `made` what is there.
fn make(
    host: &mut dyn Host,
    ask: &Add,
    verifier: Option<&str>,
    made: &mut Made,
) -> Result<(), Fail> {
    let role = built(sql::create_role(&ask.owner, verifier))?;
    host.sql(&role).map_err(|error| refused(&error, "role", &ask.owner))?;
    made.role = true;
    let comment = built(sql::comment_on_role(&ask.owner, &ask.name))?;
    run(host, &comment)?;
    let database = built(sql::create_database(&ask.name, &ask.owner, &ask.locale))?;
    host.sql(&database).map_err(|error| refused(&error, "database", &ask.name))?;
    made.database = true;
    if let Some(site) = &ask.site {
        run(host, &built(sql::comment_on_database(&ask.name, site))?)?;
    }
    run(host, &built(sql::revoke_connect(&ask.name))?)?;
    Ok(())
}

/// `database password`: a new password for the role that owns `name`.
///
/// # Errors
///
/// DB-101; a usage failure for a role that signs in by its account and
/// has no password to change; PostgreSQL's words for anything else.
pub fn password(host: &mut dyn Host, name: &str, random: &dyn SecureRandom) -> Result<Added, Fail> {
    ready(host)?;
    let found = run(host, &built(sql::the_database(name))?)?;
    let Some(answer) = found.first().filter(|answer| !answer.rows.is_empty()) else {
        return Err(no_database(name));
    };
    let role = answer.first("owner").unwrap_or_default().to_string();
    if role == "postgres" || role.is_empty() {
        return Err(usage(format!(
            "{name} belongs to the role {role}, whose password is not for database to change"
        )));
    }
    if answer.first("password") != Some("t") {
        return Err(usage(format!(
            "the role {role} signs in as the account of that name and has no password to change"
        )));
    }
    let (password, verifier) = new_password(random)?;
    run(host, &built(sql::alter_role_password(&role, &verifier))?)?;
    host.log(&format!("changed the password of the role {role}, which owns {name}"));
    Ok(Added {
        database: name.to_string(),
        role,
        password: Some(password),
        host: Some(HOST),
        port: PORT,
    })
}

/// The failure for a database that is not there.
fn no_database(name: &str) -> Fail {
    fail(format!("there is no database called {name}; `database list` shows them"))
}

/// What `remove` is asked to do.
#[derive(Debug, PartialEq, Eq)]
pub struct Remove {
    /// The database.
    pub name: String,
    /// Whether its role goes too, when `database` made it and it owns
    /// nothing else.
    pub role: bool,
    /// Whether the sessions that are open end.
    pub force: bool,
}

/// `database remove`: the database dropped, its lines of `pg_hba.conf`
/// taken out, and with `--role` the role it was owned by. Gives the
/// words to say.
///
/// # Errors
///
/// DB-101, DB-103 for open sessions unless they are forced to end,
/// DB-104 when the block of `pg_hba.conf` is not whole.
pub fn remove(host: &mut dyn Host, ask: &Remove) -> Result<Vec<String>, Fail> {
    ready(host)?;
    let found = run(host, &built(sql::the_database(&ask.name))?)?;
    let Some(answer) = found.first().filter(|answer| !answer.rows.is_empty()) else {
        return Err(no_database(&ask.name));
    };
    let owner = answer.first("owner").unwrap_or_default().to_string();
    let made_by_database =
        answer.first("comment").is_some_and(|comment| comment.starts_with(sql::ROLE_COMMENT));
    if !ask.force {
        let open = sessions(host, &ask.name)?;
        if !open.is_empty() {
            return Err(errors::sessions(&ask.name, &open));
        }
    }
    // The file is read before anything is dropped: a block that is not
    // whole is found while nothing is changed.
    let lines = allow::hba_without(host, &ask.name)?;
    let dropped = built(sql::drop_database(&ask.name, ask.force))?;
    if let Err(error) = host.sql(&dropped) {
        return Err(match &error {
            Error::Server(pg) if errors::is_in_use(pg) => {
                errors::sessions(&ask.name, &sessions(host, &ask.name).unwrap_or_default())
            }
            other => errors::said(other),
        });
    }
    host.log(&format!("removed {}", ask.name));
    let mut words = vec![format!("removed the database {}", ask.name)];
    if let Some(lines) = lines {
        words.extend(allow::write_without(host, &lines)?);
    }
    if ask.role {
        words.push(drop_role(host, &owner, made_by_database));
    }
    Ok(words)
}

/// The programs with sessions open to `name`, and how many each has.
fn sessions(host: &mut dyn Host, name: &str) -> Result<Vec<(String, u64)>, Fail> {
    let answers = run(host, &built(sql::sessions_of(name))?)?;
    let Some(answer) = answers.first() else { return Ok(Vec::new()) };
    let (Some(program), Some(count)) = (
        answer.columns.iter().position(|column| column == "application_name"),
        answer.columns.iter().position(|column| column == "sessions"),
    ) else {
        return Ok(Vec::new());
    };
    Ok(answer
        .rows
        .iter()
        .map(|row| {
            let program = row.get(program).cloned().flatten().unwrap_or_default();
            let count = row.get(count).cloned().flatten().and_then(|n| n.parse().ok());
            (program, count.unwrap_or(1))
        })
        .collect())
}

/// Drops `role`, which owned the database that is gone, when `database`
/// made it and nothing else is its own, and says what came of it.
fn drop_role(host: &mut dyn Host, role: &str, made_by_database: bool) -> String {
    if !made_by_database {
        return format!("the role {role} stays: it was not made by database");
    }
    let dropped = sql::drop_role(role).map_err(fail).and_then(|statement| {
        host.sql(&statement).map_err(|error| match &error {
            // The role still owns, or may use, something else.
            Error::Server(pg) if pg.code == "2BP01" => {
                fail(format!("it still owns or may use something else ({})", pg.message))
            }
            other => errors::said(other),
        })
    });
    match dropped {
        Ok(_) => {
            host.log(&format!("removed the role {role}"));
            format!("removed the role {role}")
        }
        Err(why) => format!("the role {role} stays: {}", why.message),
    }
}

#[cfg(test)]
mod tests {
    use super::super::fake::Fake;
    use super::super::hba;
    use super::*;
    use ring::rand::SystemRandom;

    fn ask(name: &str) -> Add {
        Add {
            name: name.into(),
            owner: name.into(),
            peer: None,
            locale: "en_US.UTF-8".into(),
            site: None,
        }
    }

    #[test]
    fn a_database_and_its_role_are_made_by_the_statements_of_the_command() {
        let mut fake = Fake::new();
        let ask = Add { site: Some("shop.example.org".into()), ..ask("shop") };
        let added = add(&mut fake, &ask, &SystemRandom::new()).unwrap();
        assert_eq!(added.database, "shop");
        assert_eq!(added.role, "shop");
        assert_eq!(added.host, Some("127.0.0.1"));
        assert_eq!(added.port, 5432);
        let password = added.password.as_ref().unwrap();
        assert_eq!(password.as_str().len(), 32);
        let changes = fake.changes();
        assert!(changes[0].starts_with("CREATE ROLE \"shop\" LOGIN PASSWORD 'SCRAM-SHA-256$4096:"));
        assert_eq!(
            changes[1..],
            [
                "COMMENT ON ROLE \"shop\" IS 'database(8): owner of shop'",
                "CREATE DATABASE \"shop\" OWNER \"shop\" TEMPLATE template0 ENCODING 'UTF8' \
                 LOCALE_PROVIDER libc LOCALE 'en_US.UTF-8'",
                "COMMENT ON DATABASE \"shop\" IS 'database(8): site shop.example.org'",
                "REVOKE ALL ON DATABASE \"shop\" FROM PUBLIC",
            ]
        );
        assert_eq!(
            fake.logged(),
            ["added shop (owner shop, en_US.UTF-8; the role has a password)"]
        );
    }

    #[test]
    fn the_builtin_sorting_and_no_site_change_the_statements() {
        let mut fake = Fake::new();
        let ask = Add { locale: "C.UTF-8".into(), ..ask("shop") };
        add(&mut fake, &ask, &SystemRandom::new()).unwrap();
        let changes = fake.changes();
        assert_eq!(changes.len(), 4, "{changes:?}");
        assert!(changes[2].ends_with("LOCALE_PROVIDER builtin BUILTIN_LOCALE 'C.UTF-8'"));
        assert!(!changes.iter().any(|statement| statement.starts_with("COMMENT ON DATABASE")));
    }

    #[test]
    fn a_role_of_an_account_has_no_password_and_no_host() {
        let mut fake = Fake::new().with_account("webdev");
        let ask = Add { peer: Some("webdev".into()), ..ask("webdev") };
        let added = add(&mut fake, &ask, &SystemRandom::new()).unwrap();
        assert!(added.password.is_none());
        assert_eq!(added.host, None);
        assert_eq!(fake.changes()[0], "CREATE ROLE \"webdev\" LOGIN");
        assert!(fake.logged()[0].contains("signs in by its account"));
    }

    #[test]
    fn an_account_that_is_not_one_is_db_106_and_nothing_is_made() {
        let mut fake = Fake::new();
        let ask = Add { peer: Some("ghost".into()), ..ask("ghost") };
        let failure = add(&mut fake, &ask, &SystemRandom::new()).unwrap_err();
        assert!(failure.message.ends_with("(DB-106)"), "{}", failure.message);
        assert!(fake.changes().is_empty());
    }

    #[test]
    fn a_name_that_is_taken_is_db_102_and_nothing_is_made() {
        let mut fake = Fake::new().with_database("shop", "shop");
        let failure = add(&mut fake, &ask("shop"), &SystemRandom::new()).unwrap_err();
        assert!(failure.message.contains("database called shop"), "{}", failure.message);
        assert!(failure.message.ends_with("(DB-102)"));
        assert!(fake.changes().is_empty());
        let mut fake = Fake::new().with_role("owner");
        let ask = Add { owner: "owner".into(), ..ask("shop") };
        let failure = add(&mut fake, &ask, &SystemRandom::new()).unwrap_err();
        assert!(failure.message.contains("role called owner"), "{}", failure.message);
        assert!(failure.message.ends_with("(DB-102)"));
        assert!(fake.changes().is_empty());
    }

    #[test]
    fn a_name_that_is_taken_between_the_look_and_the_statement_is_db_102_too() {
        let mut fake = Fake::new().refusing("CREATE DATABASE", "42P04", "database already exists");
        let failure = add(&mut fake, &ask("shop"), &SystemRandom::new()).unwrap_err();
        assert!(failure.message.ends_with("(DB-102)"), "{}", failure.message);
        // The role it made is taken away again.
        let changes = fake.changes();
        assert_eq!(changes.last().unwrap(), "DROP ROLE \"shop\"");
    }

    #[test]
    fn a_statement_that_fails_takes_back_the_role_and_the_database() {
        let mut fake = Fake::new().refusing("REVOKE", "XX000", "an internal error");
        let failure = add(&mut fake, &ask("shop"), &SystemRandom::new()).unwrap_err();
        assert_eq!(failure.message, "PostgreSQL said: an internal error (XX000)");
        let changes = fake.changes();
        assert_eq!(changes[changes.len() - 2..], ["DROP DATABASE \"shop\"", "DROP ROLE \"shop\""]);
        assert!(!fake.has_role("shop") && !fake.has_database("shop"));
        // Nothing is logged as added.
        assert!(fake.logged().is_empty());
    }

    #[test]
    fn a_failure_of_the_role_leaves_nothing_to_take_back() {
        let mut fake = Fake::new().refusing("COMMENT ON ROLE", "XX000", "no");
        add(&mut fake, &ask("shop"), &SystemRandom::new()).unwrap_err();
        let changes = fake.changes();
        assert_eq!(changes.last().unwrap(), "DROP ROLE \"shop\"");
        assert!(!changes.iter().any(|statement| statement.starts_with("DROP DATABASE")));
        let mut fake = Fake::new().refusing("CREATE ROLE", "XX000", "no");
        add(&mut fake, &ask("shop"), &SystemRandom::new()).unwrap_err();
        assert_eq!(fake.changes().len(), 1, "no statement follows the one refused");
    }

    #[test]
    fn a_role_that_cannot_be_taken_back_is_said() {
        let mut fake = Fake::new().refusing("REVOKE", "XX000", "an internal error").refusing(
            "DROP DATABASE",
            "XX001",
            "cannot",
        );
        let failure = add(&mut fake, &ask("shop"), &SystemRandom::new()).unwrap_err();
        assert!(failure.message.contains("the database shop stays"), "{}", failure.message);
        assert!(failure.message.contains("the role shop stays as the database does"));
    }

    #[test]
    fn a_server_that_is_not_there_is_db_101() {
        let mut fake = Fake::new().not_installed();
        assert!(
            add(&mut fake, &ask("shop"), &SystemRandom::new())
                .unwrap_err()
                .message
                .contains("not installed")
        );
        let mut fake = Fake::new().off();
        let failure = add(&mut fake, &ask("shop"), &SystemRandom::new()).unwrap_err();
        assert!(failure.message.contains("service postgresql enable"), "{}", failure.message);
        let mut fake = Fake::new().not_let_in();
        let failure = add(&mut fake, &ask("shop"), &SystemRandom::new()).unwrap_err();
        assert!(failure.message.ends_with("(DB-101)"), "{}", failure.message);
        assert!(fake.changes().is_empty());
    }

    #[test]
    fn a_new_password_is_for_the_owner_and_shown_in_the_answer_alone() {
        let mut fake = Fake::new().with_database("shop", "shop");
        let added = password(&mut fake, "shop", &SystemRandom::new()).unwrap();
        assert_eq!(added.role, "shop");
        assert_eq!(added.host, Some("127.0.0.1"));
        let changes = fake.changes();
        assert_eq!(changes.len(), 1);
        assert!(changes[0].starts_with("ALTER ROLE \"shop\" PASSWORD 'SCRAM-SHA-256$4096:"));
        assert_eq!(fake.logged(), ["changed the password of the role shop, which owns shop"]);
    }

    #[test]
    fn a_password_is_not_changed_for_a_role_that_has_none_or_for_postgres() {
        let mut fake = Fake::new().with_database("webdev", "webdev").without_password("webdev");
        let failure = password(&mut fake, "webdev", &SystemRandom::new()).unwrap_err();
        assert!(failure.usage, "{}", failure.message);
        assert!(failure.message.contains("no password to change"));
        let mut fake = Fake::new().with_database("misc", "postgres");
        let failure = password(&mut fake, "misc", &SystemRandom::new()).unwrap_err();
        assert!(failure.usage);
        assert!(fake.changes().is_empty());
        let mut fake = Fake::new();
        let failure = password(&mut fake, "gone", &SystemRandom::new()).unwrap_err();
        assert!(failure.message.contains("no database called gone"));
        assert!(!failure.usage);
    }

    #[test]
    fn open_sessions_refuse_a_remove_unless_it_is_forced() {
        let mut fake = Fake::new().with_database("shop", "shop").with_sessions("shop", "php", 2);
        let failure = remove(&mut fake, &Remove { name: "shop".into(), role: false, force: false })
            .unwrap_err();
        assert!(
            failure.message.contains("shop has 2 open sessions, from php (2)"),
            "{}",
            failure.message
        );
        assert!(failure.message.ends_with("(DB-103)"));
        assert!(fake.changes().is_empty());
        let words =
            remove(&mut fake, &Remove { name: "shop".into(), role: false, force: true }).unwrap();
        assert_eq!(fake.changes(), ["DROP DATABASE \"shop\" WITH (FORCE)"]);
        assert_eq!(words, ["removed the database shop"]);
    }

    #[test]
    fn a_session_that_opens_after_the_look_is_db_103_too() {
        let mut fake = Fake::new().with_database("shop", "shop").refusing(
            "DROP DATABASE",
            "55006",
            "database \"shop\" is being accessed by other users",
        );
        let failure = remove(&mut fake, &Remove { name: "shop".into(), role: false, force: false })
            .unwrap_err();
        assert!(failure.message.ends_with("(DB-103)"), "{}", failure.message);
    }

    #[test]
    fn a_role_goes_with_the_database_when_database_made_it() {
        let mut fake = Fake::new().with_database("shop", "shop").made_by_database("shop", "shop");
        let words =
            remove(&mut fake, &Remove { name: "shop".into(), role: true, force: false }).unwrap();
        assert_eq!(fake.changes(), ["DROP DATABASE \"shop\"", "DROP ROLE \"shop\""]);
        assert_eq!(words, ["removed the database shop", "removed the role shop"]);
        assert_eq!(fake.logged(), ["removed shop", "removed the role shop"]);
    }

    #[test]
    fn a_role_that_database_did_not_make_or_that_owns_more_stays_with_the_reason() {
        let mut fake = Fake::new().with_database("shop", "shop");
        let words =
            remove(&mut fake, &Remove { name: "shop".into(), role: true, force: false }).unwrap();
        assert_eq!(words[1], "the role shop stays: it was not made by database");
        assert_eq!(fake.changes(), ["DROP DATABASE \"shop\""]);
        let mut fake = Fake::new()
            .with_database("shop", "shop")
            .made_by_database("shop", "shop")
            .refusing("DROP ROLE", "2BP01", "role \"shop\" cannot be dropped");
        let words =
            remove(&mut fake, &Remove { name: "shop".into(), role: true, force: false }).unwrap();
        assert!(
            words[1].starts_with("the role shop stays: it still owns or may use something else"),
            "{words:?}"
        );
    }

    #[test]
    fn a_database_that_is_not_there_is_said_and_nothing_is_dropped() {
        let mut fake = Fake::new();
        let failure = remove(&mut fake, &Remove { name: "shop".into(), role: true, force: true })
            .unwrap_err();
        assert!(failure.message.contains("no database called shop"));
        assert!(fake.changes().is_empty());
    }

    #[test]
    fn the_lines_of_the_database_leave_pg_hba_conf_when_it_goes() {
        let mut fake = Fake::new()
            .with_database("shop", "shop")
            .with_hba_lines(&[("shop", "shop", "192.168.1.0/24"), ("blog", "blog", "10.0.0.0/8")]);
        let words =
            remove(&mut fake, &Remove { name: "shop".into(), role: false, force: false }).unwrap();
        assert_eq!(fake.hba_rules(), ["hostssl \"blog\" \"blog\" 10.0.0.0/8 scram-sha-256"]);
        assert_eq!(words[1], "took shop's lines out of pg_hba.conf, and PostgreSQL reads it again");
        assert_eq!(fake.reloads(), 1);
        // The file keeps the other lines, and it was read before anything
        // was dropped.
        assert!(fake.hba_text().contains("local   all          all"));
    }

    #[test]
    fn a_block_that_is_not_whole_is_found_before_the_database_goes() {
        let mut fake = Fake::new().with_database("shop", "shop");
        fake.write_hba(&format!("{}\n{}\n", "local all all peer", hba::BEGIN));
        let failure = remove(&mut fake, &Remove { name: "shop".into(), role: false, force: false })
            .unwrap_err();
        assert!(failure.message.ends_with("(DB-104)"), "{}", failure.message);
        assert!(fake.changes().is_empty(), "{:?}", fake.changes());
    }

    #[test]
    fn no_answer_error_or_log_line_holds_the_password() {
        let mut fake = Fake::new();
        let added = add(&mut fake, &ask("shop"), &SystemRandom::new()).unwrap();
        let secret = added.password.as_ref().unwrap().as_str().to_string();
        // Not in what was sent, logged, or put in words.
        assert!(!fake.sent().iter().any(|statement| statement.contains(&secret)));
        assert!(!fake.logged().iter().any(|line| line.contains(&secret)));
        assert!(!format!("{:?}", added.password).contains(&secret));
        // The verifier is what was sent, and it holds neither the password
        // nor its base64.
        let verifier = fake.changes()[0].clone();
        assert!(verifier.contains("SCRAM-SHA-256$4096:"));

        // A failure of a later statement, and its taking back.
        let mut failing = Fake::new().refusing("REVOKE", "XX000", "an internal error");
        let failure = add(&mut failing, &ask("shop"), &SystemRandom::new()).unwrap_err();
        assert!(!failure.message.contains("SCRAM-SHA-256$4096:"));
        // A new password the same way.
        let mut fake = Fake::new().with_database("shop", "shop");
        let added = password(&mut fake, "shop", &SystemRandom::new()).unwrap();
        let secret = added.password.as_ref().unwrap().as_str().to_string();
        assert!(!fake.sent().iter().any(|statement| statement.contains(&secret)));
        assert!(!fake.logged().iter().any(|line| line.contains(&secret)));
    }

    #[test]
    fn the_json_of_an_add_has_the_members_the_pages_read() {
        let mut fake = Fake::new();
        let added = add(&mut fake, &ask("shop"), &SystemRandom::new()).unwrap();
        let line = added.line().unwrap();
        let secret = added.password.as_ref().unwrap().as_str();
        // The password is the one thing that is not the same each time.
        let masked = line.replace(secret, "A-PASSWORD-OF-32-CHARACTERS-LONG");
        assert_ne!(masked, line);
        json::assert_golden(&masked, include_str!("../../tests/json/database-add.json"));
        // A role that signs in by its account has no password and no host.
        let mut fake = Fake::new().with_account("webdev");
        let peer = Add { peer: Some("webdev".into()), ..ask("webdev") };
        let added = add(&mut fake, &peer, &SystemRandom::new()).unwrap();
        assert_eq!(
            added.line().unwrap(),
            "{\"format\":1,\"database\":\"webdev\",\"role\":\"webdev\",\"password\":null,\
             \"host\":null,\"port\":5432}"
        );
    }

    #[test]
    fn the_password_is_a_fresh_one_each_time() {
        let mut fake = Fake::new().with_database("shop", "shop");
        let one = password(&mut fake, "shop", &SystemRandom::new()).unwrap();
        let two = password(&mut fake, "shop", &SystemRandom::new()).unwrap();
        assert_ne!(one.password.unwrap().as_str(), two.password.unwrap().as_str());
        let changes = fake.changes();
        assert_ne!(changes[0], changes[1], "each has its own salt too");
    }
}
