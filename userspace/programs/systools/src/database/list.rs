// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! `database list`: the server, its databases and roles, and who may
//! connect, as PostgreSQL says them and as `--json` writes them.
//!
//! A server that is not installed or does not run is told as such, with
//! empty lists, and is no failure. A verifier is never read: whether a
//! role has a password is. The server's `firewall` is what the block in
//! the firewall's rules says of its port ([`super::port`]).

use std::fs;
use std::path::Path;

use serde::Serialize;

use super::add::{ready, run};
use super::copies::{self, Copy};
use super::names::Network;
use super::port::{self, Firewall};
use super::server::{Host, value};
use super::wire::Answer;
use super::{hba, settings, sql};
use crate::util::{Fail, fail};

/// What `list` says: the document `--json` prints after its `format`.
#[derive(Debug, Serialize)]
pub struct List {
    /// The server.
    pub server: Server,
    /// Its databases, except `postgres` and the templates.
    pub databases: Vec<Database>,
    /// Its roles, except PostgreSQL's own `pg_` ones.
    pub roles: Vec<Role>,
    /// The lines of `pg_hba.conf` the server read.
    pub access: Vec<Access>,
    /// The mistakes it found in that file.
    pub problems: Vec<Problem>,
}

/// The server.
#[derive(Debug, Serialize)]
pub struct Server {
    /// Whether the bundle with PostgreSQL's service is installed.
    pub installed: bool,
    /// Whether the service is turned on.
    pub enabled: bool,
    /// Whether it runs and answers.
    pub running: bool,
    /// Its version, such as `19.0`; none while it does not run.
    pub version: Option<String>,
    /// What it listens on: `localhost`, or `*` for every address.
    pub listen: Option<String>,
    /// Whether it takes TLS.
    pub ssl: bool,
    /// The certificate it takes TLS with, when `database` set it.
    pub certificate: Option<String>,
    /// What the firewall's rules say of its port. While the server does
    /// not run, they are not read, and say nothing.
    pub firewall: Firewall,
    /// Whether the backup hook of its service copies each database, as
    /// the service file and its override say. A machine that has no
    /// backup set up has the hook and makes no copy.
    pub copies: bool,
}

/// A database.
#[derive(Debug, Serialize)]
pub struct Database {
    /// Its name.
    pub name: String,
    /// The role that owns it.
    pub owner: String,
    /// Its text encoding, such as `UTF8`.
    pub encoding: String,
    /// What sorts its text: `builtin`, `libc` or `icu`.
    pub provider: String,
    /// The locale the provider sorts by.
    pub locale: Option<String>,
    /// Its size in bytes.
    pub bytes: u64,
    /// The sessions open to it.
    pub sessions: u64,
    /// The website it was made for.
    pub site: Option<String>,
    /// Whether `database add` made it.
    pub made_by_database: bool,
    /// Whether the sorting data changed since it was made: its indexes
    /// are to be rebuilt.
    pub collation_mismatch: bool,
    /// The newest copy of it that the backup hook made: when, and how
    /// long. None before the first backup.
    pub copy: Option<Copy>,
}

/// A role.
#[derive(Debug, Serialize)]
pub struct Role {
    /// Its name.
    pub name: String,
    /// Whether it may sign in.
    pub login: bool,
    /// Whether it has a password.
    pub password: bool,
    /// Whether `database add` made it.
    pub made_by_database: bool,
}

/// A line of `pg_hba.conf`.
#[derive(Debug, Serialize)]
pub struct Access {
    /// Its line number in the file.
    pub line: u64,
    /// `local`, `host`, `hostssl` and the others.
    pub r#type: String,
    /// The databases it is for.
    pub databases: Vec<String>,
    /// The roles it is for.
    pub users: Vec<String>,
    /// The addresses, as `192.168.1.0/24`; none for a line of `local`.
    pub address: Option<String>,
    /// How they prove who they are.
    pub method: String,
    /// Whether it is written outside the block `database` keeps.
    pub by_hand: bool,
}

/// A mistake in `pg_hba.conf`, or DB-108: the firewall's rules have a
/// block of `database`'s that is not whole.
#[derive(Debug, Serialize)]
pub struct Problem {
    /// The ID of the page that explains it.
    pub id: &'static str,
    /// PostgreSQL's words, or the firewall's file and what is wrong with
    /// it.
    pub words: String,
    /// The line of `pg_hba.conf`; none for the firewall's rules.
    pub line: Option<u64>,
}

/// The cell `name` of `row` in `answer`, when it is not null.
fn cell<'a>(answer: &Answer, row: &'a [Option<String>], name: &str) -> Option<&'a str> {
    let at = answer.columns.iter().position(|column| column == name)?;
    row.get(at)?.as_deref()
}

/// A number in a cell, or 0.
fn number(answer: &Answer, row: &[Option<String>], name: &str) -> u64 {
    cell(answer, row, name).and_then(|text| text.parse().ok()).unwrap_or(0)
}

/// A true or false in a cell: PostgreSQL writes `t` and `f`.
fn truth(answer: &Answer, row: &[Option<String>], name: &str) -> bool {
    cell(answer, row, name) == Some("t")
}

/// The words PostgreSQL's one-letter provider stands for.
fn provider(letter: &str) -> String {
    match letter {
        "b" => "builtin",
        "c" => "libc",
        "i" => "icu",
        other => other,
    }
    .to_string()
}

/// Whether the backup hook of the service is the one that copies each
/// database, by the service file and its override as they are now.
fn copying(host: &dyn Host) -> bool {
    let files = host.files();
    let Ok(file) = fs::read_to_string(&files.service) else { return false };
    let over = fs::read_to_string(&files.over).ok();
    settings::copies(&file, over.as_deref())
}

/// What `list` says of a server that is not there to ask.
fn silent(installed: bool, enabled: bool, copies: bool) -> List {
    List {
        server: Server {
            installed,
            enabled,
            running: false,
            version: None,
            listen: None,
            ssl: false,
            certificate: None,
            firewall: Firewall::default(),
            copies,
        },
        databases: Vec::new(),
        roles: Vec::new(),
        access: Vec::new(),
        problems: Vec::new(),
    }
}

/// `database list`.
///
/// # Errors
///
/// DB-101 when the server runs and does not let root in.
pub fn list(host: &mut dyn Host) -> Result<List, Fail> {
    let service = host.service()?;
    if !service.installed || !service.running {
        return Ok(silent(service.installed, service.enabled, service.installed && copying(host)));
    }
    ready(host)?;
    let version = run(host, "SHOW server_version")?;
    let listen = run(host, "SHOW listen_addresses")?;
    let ssl = run(host, "SHOW ssl")?;
    let cert_file = run(host, "SHOW ssl_cert_file")?;
    let hba_file = run(host, "SHOW hba_file")?;
    let mut server = Server {
        installed: true,
        enabled: service.enabled,
        running: true,
        version: value(&version).map(String::from),
        listen: value(&listen).map(String::from),
        ssl: value(&ssl) == Some("on"),
        certificate: value(&cert_file).and_then(settings::certificate_of).map(String::from),
        firewall: Firewall::default(),
        copies: copying(host),
    };
    let sessions = run(host, sql::LIST_SESSIONS)?;
    let databases = run(host, sql::LIST_DATABASES)?;
    let roles = run(host, sql::LIST_ROLES)?;
    let access = run(host, sql::LIST_ACCESS)?;
    let hba_text = value(&hba_file).and_then(|path| fs::read_to_string(Path::new(path)).ok());
    let (access, mut problems) = access_of(first(&access)?, hba_text.as_deref())?;
    match port::read(&host.files().firewall) {
        Ok(firewall) => server.firewall = firewall,
        Err(words) => problems.push(Problem { id: "DB-108", words, line: None }),
    }
    Ok(List {
        server,
        databases: databases_of(first(&databases)?, first(&sessions)?, &host.dumps()),
        roles: roles_of(first(&roles)?),
        access,
        problems,
    })
}

/// The answer to one statement.
fn first(answers: &[Answer]) -> Result<&Answer, Fail> {
    answers.first().ok_or_else(|| fail("PostgreSQL did not answer".into()))
}

/// The databases of the answer of [`sql::LIST_DATABASES`], with their
/// sessions from [`sql::LIST_SESSIONS`] and their copies from `dumps`, the
/// folder of the copies.
fn databases_of(databases: &Answer, sessions: &Answer, dumps: &Path) -> Vec<Database> {
    databases
        .rows
        .iter()
        .map(|row| {
            let name = cell(databases, row, "name").unwrap_or_default().to_string();
            let comment = cell(databases, row, "comment");
            let open = sessions
                .rows
                .iter()
                .find(|one| cell(sessions, one, "name") == Some(name.as_str()))
                .map_or(0, |one| number(sessions, one, "sessions"));
            let owner_made_it = cell(databases, row, "owner_comment")
                == Some(format!("{}{name}", sql::ROLE_COMMENT).as_str());
            let mismatch = match (cell(databases, row, "recorded"), cell(databases, row, "actual"))
            {
                (Some(recorded), Some(actual)) => recorded != actual,
                _ => false,
            };
            Database {
                owner: cell(databases, row, "owner").unwrap_or_default().to_string(),
                encoding: cell(databases, row, "encoding").unwrap_or_default().to_string(),
                provider: provider(cell(databases, row, "provider").unwrap_or_default()),
                locale: cell(databases, row, "locale").map(String::from),
                bytes: number(databases, row, "bytes"),
                sessions: open,
                site: comment
                    .and_then(|text| text.strip_prefix(sql::SITE_COMMENT))
                    .map(String::from),
                made_by_database: comment.is_some_and(|text| text.starts_with(sql::COMMENT))
                    || owner_made_it,
                collation_mismatch: mismatch,
                copy: copies::in_folder(dumps, &name),
                name,
            }
        })
        .collect()
}

/// The roles of the answer of [`sql::LIST_ROLES`].
fn roles_of(roles: &Answer) -> Vec<Role> {
    roles
        .rows
        .iter()
        .map(|row| Role {
            name: cell(roles, row, "name").unwrap_or_default().to_string(),
            login: truth(roles, row, "login"),
            password: truth(roles, row, "password"),
            made_by_database: cell(roles, row, "comment")
                .is_some_and(|text| text.starts_with(sql::ROLE_COMMENT)),
        })
        .collect()
}

/// A list of words that PostgreSQL's `to_json` wrote.
fn words_of(json: Option<&str>) -> Vec<String> {
    json.and_then(|json| serde_json::from_str(json).ok()).unwrap_or_default()
}

/// The lines and the mistakes of the answer of [`sql::LIST_ACCESS`];
/// `text` is `pg_hba.conf` itself, which says what is in the block
/// `database` keeps.
fn access_of(answer: &Answer, text: Option<&str>) -> Result<(Vec<Access>, Vec<Problem>), Fail> {
    let block = text.and_then(hba::block_lines);
    let mut access = Vec::new();
    let mut problems = Vec::new();
    for row in &answer.rows {
        let line = number(answer, row, "line_number");
        if let Some(error) = cell(answer, row, "error") {
            problems.push(Problem {
                id: "DB-104",
                words: error.to_string(),
                line: cell(answer, row, "line_number").map(|_| line),
            });
            continue;
        }
        let Some(kind) = cell(answer, row, "type") else { continue };
        let usize_line = usize::try_from(line).unwrap_or(usize::MAX);
        let inside = block.is_some_and(|(first, last)| (first..=last).contains(&usize_line));
        let address = match (cell(answer, row, "address"), cell(answer, row, "netmask")) {
            (Some(address), Some(mask)) => Some(
                Network::of_netmask(address, mask)
                    .map_or_else(|| format!("{address} {mask}"), |network| network.to_string()),
            ),
            (Some(address), None) => Some(address.to_string()),
            (None, _) => None,
        };
        access.push(Access {
            line,
            r#type: kind.to_string(),
            databases: words_of(cell(answer, row, "databases")),
            users: words_of(cell(answer, row, "users")),
            address,
            method: cell(answer, row, "auth_method").unwrap_or_default().to_string(),
            by_hand: !inside,
        });
    }
    Ok((access, problems))
}

/// `list` in words, for a person at a terminal.
#[must_use]
pub fn text(list: &List) -> String {
    let server = &list.server;
    let mut out = String::new();
    if !server.installed {
        out.push_str("PostgreSQL is not installed.\n");
        return out;
    }
    if !server.running {
        let why = if server.enabled { "not running" } else { "off" };
        out.push_str(&format!("PostgreSQL is {why}.\n"));
        return out;
    }
    let listens = match server.listen.as_deref() {
        Some("localhost") | None => "this machine only".to_string(),
        Some(other) => {
            format!("{other}, over TLS: {}", server.certificate.as_deref().unwrap_or("?"))
        }
    };
    out.push_str(&format!(
        "PostgreSQL {}, running, listens on {listens}\n",
        server.version.as_deref().unwrap_or("")
    ));
    for database in &list.databases {
        let site = database.site.as_ref().map_or(String::new(), |site| format!(", for {site}"));
        out.push_str(&format!(
            "database {}: owned by {}, {} bytes, {} open session(s){site}\n",
            database.name, database.owner, database.bytes, database.sessions
        ));
    }
    for line in &list.access {
        let who = if line.by_hand { "by hand" } else { "database" };
        out.push_str(&format!(
            "pg_hba.conf line {}: {} {} {} {} {} ({who})\n",
            line.line,
            line.r#type,
            line.databases.join(","),
            line.users.join(","),
            line.address.as_deref().unwrap_or("-"),
            line.method
        ));
    }
    for problem in &list.problems {
        if problem.id == "DB-108" {
            out.push_str(&format!("{} ({})\n", problem.words, problem.id));
            continue;
        }
        let line = problem.line.map_or(String::new(), |line| format!(" at line {line}"));
        out.push_str(&format!(
            "a mistake in pg_hba.conf{line}: {} ({})\n",
            problem.words, problem.id
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::super::fake::{Fake, START_MS};
    use super::*;
    use crate::json;

    #[test]
    fn a_server_that_is_not_installed_or_not_running_is_told_with_empty_lists() {
        let mut fake = Fake::new().not_installed();
        let list = list(&mut fake).unwrap();
        assert!(!list.server.installed && !list.server.running);
        assert!(list.databases.is_empty() && list.roles.is_empty() && list.access.is_empty());
        assert!(list.problems.is_empty());
        let mut fake = Fake::new().off();
        let list = list_of(&mut fake);
        assert!(list.server.installed && !list.server.running && !list.server.enabled);
        assert_eq!(list.server.version, None);
        assert!(fake.sent().is_empty(), "no statement is sent to a server that does not run");
    }

    fn list_of(fake: &mut Fake) -> List {
        list(fake).unwrap()
    }

    #[test]
    fn a_server_that_runs_says_its_version_and_where_it_listens() {
        let mut fake = Fake::new();
        let list = list_of(&mut fake);
        assert_eq!(
            (list.server.installed, list.server.enabled, list.server.running),
            (true, true, true)
        );
        assert_eq!(list.server.version.as_deref(), Some("19.0"));
        assert_eq!(list.server.listen.as_deref(), Some("localhost"));
        assert!(!list.server.ssl);
        assert_eq!(list.server.certificate, None);
    }

    #[test]
    fn the_databases_roles_and_lines_are_read_from_the_server() {
        let mut fake = Fake::new()
            .with_database("shop", "shop")
            .made_by_database("shop", "shop")
            .with_site("shop", "shop.example.org")
            .with_sessions("shop", "php", 2)
            .with_database("notes", "webdev")
            .without_password("webdev")
            .with_hba_lines(&[("shop", "shop", "192.168.1.0/24")]);
        let list = list_of(&mut fake);
        let shop = &list.databases[1];
        assert_eq!(shop.name, "shop");
        assert_eq!(shop.owner, "shop");
        assert_eq!(shop.provider, "libc");
        assert_eq!(shop.locale.as_deref(), Some("en_US.UTF-8"));
        assert_eq!(shop.sessions, 2);
        assert_eq!(shop.site.as_deref(), Some("shop.example.org"));
        assert!(shop.made_by_database);
        assert!(!shop.collation_mismatch);
        let notes = &list.databases[0];
        assert_eq!(
            (notes.name.as_str(), notes.sessions, notes.made_by_database),
            ("notes", 0, false)
        );
        let webdev = list.roles.iter().find(|role| role.name == "webdev").unwrap();
        assert!(webdev.login && !webdev.password && !webdev.made_by_database);
        let shop_role = list.roles.iter().find(|role| role.name == "shop").unwrap();
        assert!(shop_role.password && shop_role.made_by_database);
        // The lines the server read: the bundle's by hand, ours not.
        let ours: Vec<&Access> = list.access.iter().filter(|line| !line.by_hand).collect();
        assert_eq!(ours.len(), 1);
        assert_eq!(ours[0].r#type, "hostssl");
        assert_eq!(ours[0].databases, ["shop"]);
        assert_eq!(ours[0].users, ["shop"]);
        assert_eq!(ours[0].address.as_deref(), Some("192.168.1.0/24"));
        assert_eq!(ours[0].method, "scram-sha-256");
        let by_hand = list.access.iter().filter(|line| line.by_hand).count();
        assert_eq!(by_hand, 4, "{:?}", list.access);
        let local = list.access.iter().find(|line| line.r#type == "local").unwrap();
        assert_eq!(local.address, None);
        let host = list.access.iter().find(|line| line.r#type == "host").unwrap();
        assert_eq!(host.address.as_deref(), Some("127.0.0.1/32"));
    }

    #[test]
    fn a_mistake_of_pg_hba_conf_is_a_problem_with_its_line() {
        let mut fake = Fake::new();
        fake.write_hba("local all all peer\nhost all all BROKEN scram-sha-256\n");
        let list = list_of(&mut fake);
        assert_eq!(list.access.len(), 1);
        assert_eq!(list.problems.len(), 1);
        assert_eq!(list.problems[0].id, "DB-104");
        assert_eq!(list.problems[0].line, Some(2));
        assert!(list.problems[0].words.contains("invalid"), "{}", list.problems[0].words);
    }

    #[test]
    fn a_server_that_does_not_let_root_in_is_db_101() {
        let mut fake = Fake::new().not_let_in();
        let failure = list(&mut fake).err().unwrap();
        assert!(failure.message.ends_with("(DB-101)"));
    }

    #[test]
    fn the_collation_version_that_changed_is_told() {
        let mut fake =
            Fake::new().with_database("shop", "shop").with_collation("shop", "2.39", "2.40");
        let list = list_of(&mut fake);
        assert!(list.databases[0].collation_mismatch);
        let mut fake =
            Fake::new().with_database("shop", "shop").with_collation("shop", "2.39", "2.39");
        assert!(!list_of(&mut fake).databases[0].collation_mismatch);
    }

    #[test]
    fn the_certificate_is_named_from_the_file_the_server_reads() {
        let mut fake = Fake::new().listening_with("this-machine");
        let list = list_of(&mut fake);
        assert_eq!(list.server.listen.as_deref(), Some("*"));
        assert!(list.server.ssl);
        assert_eq!(list.server.certificate.as_deref(), Some("this-machine"));
    }

    #[test]
    fn the_document_has_the_members_the_pages_read() {
        let mut fake = Fake::new()
            .with_database("shop", "shop")
            .made_by_database("shop", "shop")
            .with_site("shop", "shop.example.org")
            .with_sessions("shop", "php", 1)
            .listening_with("this-machine")
            .with_hba_lines(&[("shop", "shop", "192.0.2.0/24")])
            .with_copying_hook()
            .with_copy("shop", 12345, START_MS)
            .with_firewall_rules(&format!(
                "allow tcp 5000-6000\n\n{}",
                block_of("shop", &["192.0.2.0/24"])
            ));
        let list = list_of(&mut fake);
        let line = json::line(&list).unwrap();
        json::assert_golden(&line, include_str!("../../tests/json/database-list.json"));
    }

    /// The firewall's block when `database` is let in from `networks`.
    fn block_of(database: &str, networks: &[&str]) -> String {
        let rules: Vec<hba::Rule> = networks
            .iter()
            .map(|network| hba::Rule {
                database: database.into(),
                role: database.into(),
                network: Network::parse(network).unwrap(),
            })
            .collect();
        port::wanted("", &rules).unwrap().text
    }

    #[test]
    fn the_firewall_is_what_its_block_says_of_the_port() {
        let mut fake = Fake::new();
        let list = list_of(&mut fake);
        assert_eq!(list.server.firewall, Firewall::default(), "no file is no block");
        let block = block_of("shop", &["192.168.1.0/24", "10.0.0.0/8"]);
        let mut fake = Fake::new().with_firewall_rules(&format!("deny udp 5353\n{block}"));
        let list = list_of(&mut fake);
        assert_eq!(list.server.firewall.networks, ["192.168.1.0/24", "10.0.0.0/8"]);
        assert_eq!(list.server.firewall.earlier, None);
        assert!(list.problems.is_empty());
        let mut fake = Fake::new().with_firewall_rules(&format!("allow tcp\n{block}"));
        let list = list_of(&mut fake);
        assert_eq!(list.server.firewall.earlier.as_deref(), Some("allow tcp"));
    }

    #[test]
    fn a_firewall_block_that_is_not_whole_is_a_problem_and_no_failure() {
        let mut fake = Fake::new().with_firewall_rules("allow tcp 22\n# database(8): end\n");
        let list = list_of(&mut fake);
        assert_eq!(list.server.firewall, Firewall::default());
        assert_eq!(list.problems.len(), 1);
        assert_eq!(list.problems[0].id, "DB-108");
        assert_eq!(list.problems[0].line, None);
        assert!(
            list.problems[0].words.contains("line 2 ends a block that no line starts"),
            "{}",
            list.problems[0].words
        );
        let said = text(&list);
        assert!(said.contains("(DB-108)") && !said.contains("a mistake in pg_hba.conf"), "{said}");
    }

    #[test]
    fn a_server_that_does_not_run_says_nothing_of_the_firewall() {
        let block = block_of("shop", &["10.0.0.0/8"]);
        let mut fake = Fake::new().off().with_firewall_rules(&block);
        let list = list_of(&mut fake);
        assert_eq!(list.server.firewall, Firewall::default());
    }

    #[test]
    fn a_database_has_the_copy_the_hook_made_of_it_or_none() {
        let mut fake = Fake::new()
            .with_database("shop", "shop")
            .with_database("blog", "blog")
            .with_copy("shop", 2048, START_MS - 3_600_000)
            // A copy of a database that is gone is not any database's.
            .with_copy("gone", 10, START_MS);
        let list = list_of(&mut fake);
        let copy = |name: &str| list.databases.iter().find(|one| one.name == name).unwrap().copy;
        assert_eq!(copy("shop"), Some(Copy { at_ms: START_MS - 3_600_000, bytes: 2048 }));
        assert_eq!(copy("blog"), None);
        // The text of `list` says nothing of it.
        assert!(!text(&list).contains("copy"), "{}", text(&list));
    }

    #[test]
    fn the_server_says_whether_its_backup_hook_copies_the_databases() {
        let says = |fake: Fake| {
            let mut fake = fake;
            list_of(&mut fake).server.copies
        };
        assert!(says(Fake::new().with_copying_hook()));
        // The hook before the bundle copied, and none.
        assert!(!says(Fake::new().with_checkpoint_hook()));
        assert!(!says(Fake::new()));
        // The override is read as Spark reads it: its list replaces the file's.
        let fake = Fake::new().with_copying_hook();
        let over = fake.files().over.clone();
        std::fs::create_dir_all(over.parent().unwrap()).unwrap();
        std::fs::write(&over, "[backup]\nbefore = []\n").unwrap();
        assert!(!says(fake));
        let fake = Fake::new().with_checkpoint_hook();
        let over = fake.files().over.clone();
        std::fs::create_dir_all(over.parent().unwrap()).unwrap();
        std::fs::write(
            &over,
            "[backup]\nbefore = [\"/applications/org.postgresql.postgresql/bin/postgresql-backup\"]\n",
        )
        .unwrap();
        assert!(says(fake));
        // A server that does not run says it too, as far as the file says.
        let mut off = Fake::new().off().with_copying_hook();
        assert!(list(&mut off).unwrap().server.copies);
        let mut none = Fake::new().not_installed().with_copying_hook();
        assert!(!list(&mut none).unwrap().server.copies);
    }

    #[test]
    fn no_verifier_is_selected_or_shown() {
        let mut fake = Fake::new().with_database("shop", "shop");
        let list = list_of(&mut fake);
        let line = json::line(&list).unwrap();
        assert!(!line.contains("SCRAM"), "{line}");
        assert!(fake.sent().iter().all(|statement| !statement.contains("rolpassword,")));
    }
}
