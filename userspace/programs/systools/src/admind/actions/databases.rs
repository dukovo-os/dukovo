// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The databases' actions (ADR 0569): what PostgreSQL has, a database
//! with the role that owns it, a new password for that role, letting
//! other machines in and shutting them out, and removing a database, as
//! `database` does them. The area is `databases`, the scope a token
//! needs.
//!
//! PostgreSQL lets only root in, as the role postgres, so every action
//! runs as an administrator, the list too: it is a read that needs one.
//!
//! A database is named by the one rule `database` names databases and
//! roles by, so no route takes a one-part `{name}`: `add` is a word of
//! its own, and a database called `add` is changed at
//! `/api/1/databases/add/remove`.
//!
//! `add` and `password` print the role's password, once, in their JSON.
//! Their routes are `POST`s that keep nothing of the answer
//! ([`Action::SECRET_ANSWER`]), and they are never tasks. The password is
//! in no argument, no command and no file: the answer holds it, and a
//! playbook's `register`.
//!
//! The service's override, which names the certificates PostgreSQL
//! serves, and the firewall's rules, which hold the block that opens
//! port 5432 to the networks a database is opened to, are the two files of
//! `allow` and `deny` that admind sees. `pg_hba.conf` and the server's own
//! settings are under PostgreSQL's data folder, which it does not.
//!
//! Each change compares from a read of the list, `database list --json`,
//! which PostgreSQL gives only to root. The probe runs as the person, so
//! it cannot read it, and the change says that it changed something: a
//! probe that fails is no reason to fail the change.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::{
    Action, Ansible, Args, Bad, Examples, Field, Nothing, RESULT, Runs, Tests, Undo, command_task,
};
use crate::database::names::{self, Network};
use crate::firewall::SETTINGS as FIREWALL;
use manifest::service::{CERTIFICATE_NAME_MAX, check_certificate};

/// The area of these actions: the scope a token needs, `databases`.
const AREA: &str = "databases";

/// The tests of these actions, which `lab/actions.toml` names.
const TESTS: Tests = Tests {
    checks: "admind::actions::databases::tests",
    route: "admin-websites",
    same_as_command: "admin-websites",
    browser: "web-console/databases",
    ansible: "web-console/ansible",
};

/// What a database's or a role's name matches, as `database` checks it:
/// a lowercase letter, then up to 62 lowercase letters, digits and `_`,
/// and not a name PostgreSQL keeps for itself: one that starts with `pg_`,
/// `postgres`, `template0` or `template1`.
const DATABASE_PATTERN: &str = "^(?!pg_)(?!(postgres|template0|template1)$)[a-z][a-z0-9_]{0,62}$";

/// What the name of an account that owns a database matches: it is also
/// the name of the role, so the rule of roles holds, up to the 32 characters
/// an account's name has, and not `help`, which no account is called.
const PEER_PATTERN: &str = "^(?!pg_)(?!(postgres|template0|template1|help)$)[a-z][a-z0-9_]{0,31}$";

/// What a certificate's name matches, as `cert` checks it apart from its
/// length: a lowercase letter or a digit, then more of them, and hyphens
/// and dots, with no two dots in a row.
const CERTIFICATE_PATTERN: &str = "^(?!.*\\.\\.)[a-z0-9][a-z0-9.-]*$";

/// What a website's name matches, as `cert` checks a name apart from its
/// length: labels of lowercase letters, digits and hyphens, none starting
/// or ending with a hyphen or over 63 characters, with dots between them.
const SITE_PATTERN: &str =
    "^[a-z0-9]([a-z0-9-]{0,61}[a-z0-9])?(\\.[a-z0-9]([a-z0-9-]{0,61}[a-z0-9])?)*$";

/// What the networks of a list match: one a line, 1 to 20, each an
/// address, a slash and a prefix length. The addresses are checked as
/// addresses by [`check_from`], which a pattern cannot.
const FROM_PATTERN: &str = "^[0-9a-fA-F:.]+/[0-9]{1,3}(\n[0-9a-fA-F:.]+/[0-9]{1,3}){0,19}$";

/// The most characters of a list of 20 networks: 20 IPv6 networks of 43
/// characters and the 19 line ends between them.
const FROM_MAX: usize = 900;

/// The sortings a new database may have: `database`'s own list, which a
/// test holds this to.
const LOCALES: &[&str] = &["en_US.UTF-8", "C.UTF-8"];

/// The service file that names the certificates PostgreSQL serves, in the
/// settings' folder, which admind sees.
const OVERRIDE: &str = "/system/settings/spark/services/settings.d/postgresql.toml";

/// The field of a database's name, for each action that takes one.
const NAME: Field = Field::text(
    "name",
    "The database's name: a lowercase letter, then lowercase letters, digits and _, up to 63 \
     characters, and not one that starts with pg_.",
    1,
    63,
    Some(DATABASE_PATTERN),
);

/// The field of the role that owns a new database.
const OWNER: Field = Field::text(
    "owner",
    "The role that owns the database, which signs in with a password: the database's name when it \
     is left out. A name by the same rule.",
    1,
    63,
    Some(DATABASE_PATTERN),
)
.optional();

/// The field of the account that owns a new database and signs in by the
/// socket, with no password.
const PEER: Field = Field::text(
    "peer",
    "An account that owns the database and signs in as itself on this machine, with no password: \
     its name, as user list shows it, which is also the role's: a lowercase letter, then \
     lowercase letters, digits and _.",
    1,
    accounts::NAME_MAX,
    Some(PEER_PATTERN),
)
.optional();

/// The field of a new database's sorting.
const LOCALE: Field = Field::one_of(
    "locale",
    "How the database sorts text: en_US.UTF-8, or C.UTF-8, which sorts by character code.",
    LOCALES,
)
.optional();

/// The field of the website a database is made for.
const SITE: Field = Field::text(
    "site",
    "The website the database is for, by its name, which the list shows beside it.",
    1,
    253,
    Some(SITE_PATTERN),
)
.optional();

/// The field of the networks to let in or shut out.
const FROM: Field = Field::text(
    "from",
    "The networks, one on each line, up to 20, each an address and its prefix length, as \
     192.168.1.0/24.",
    1,
    FROM_MAX,
    Some(FROM_PATTERN),
);

/// The field of the certificate PostgreSQL serves to the other machines.
const CERTIFICATE: Field = Field::text(
    "certificate",
    "The certificate PostgreSQL shows to the machines that connect, by its name as cert list shows \
     it: this-machine when it is left out.",
    1,
    CERTIFICATE_NAME_MAX,
    Some(CERTIFICATE_PATTERN),
)
.optional();

/// The field that asks for the database's role to go too.
const ROLE: Field = Field::flag(
    "role",
    "Whether to remove the role that owns the database too, if the database made it and it owns \
     nothing else.",
)
.optional();

/// The field that asks for the open sessions to be ended.
const FORCE: Field = Field::flag(
    "force",
    "Whether to end the sessions that are open on the database, which stop at once, and remove it \
     anyway.",
)
.optional();

/// Why the argument `field` cannot be used, in the words the command
/// would say, and what was not done.
fn bad(field: &'static str, why: &str, done: &str) -> Bad {
    Bad { field, why: format!("{why}. {done}") }
}

/// Checks `name` as the name of a database or a role, for `field`.
fn check_name(field: &'static str, name: &str, done: &str) -> Result<(), Bad> {
    names::check_name(name).map_err(|why| bad(field, &why, done))
}

/// Checks the text of networks, one on each line: 1 to 20 of them, each
/// an address with its prefix, none twice. `done` says what was not done.
fn check_from(from: &str, done: &str) -> Result<Vec<Network>, Bad> {
    let mut found = Vec::new();
    for line in from.split('\n') {
        found.push(Network::parse(line).map_err(|why| bad("from", &why, done))?);
    }
    names::check_networks(&found).map_err(|why| bad("from", &why, done))?;
    Ok(found)
}

/// The networks of `from`, one on each line, as words of a command: each
/// after `--from`.
fn from_words(from: &str) -> Vec<String> {
    from.split('\n').flat_map(|network| ["--from".to_string(), network.to_string()]).collect()
}

/// `database list --json`, which says what the changes move.
fn list_argv() -> Vec<String> {
    ["database", "list", "--json"].map(String::from).to_vec()
}

/// What the list `output` says of the database `name` that a change
/// moves, as text to compare: its owner, encoding, provider, sorting and
/// site; `none` for a database the list does not have. The sessions and
/// the size change with what is done to it, not with the change.
fn entry_of(name: &str, output: &str) -> Option<String> {
    let document: Value = serde_json::from_str(output).ok()?;
    let found = document
        .get("databases")?
        .as_array()?
        .iter()
        .find(|row| row.get("name").and_then(Value::as_str) == Some(name));
    Some(match found {
        Some(row) => {
            let member = |key: &str| row.get(key).cloned().unwrap_or(Value::Null);
            json!({
                "owner": member("owner"),
                "encoding": member("encoding"),
                "provider": member("provider"),
                "locale": member("locale"),
                "site": member("site"),
            })
            .to_string()
        }
        None => "none".into(),
    })
}

/// What the list `output` says of who may connect to the database
/// `name` from other machines that a change moves, as text to compare:
/// the addresses the server listens on and each rule of `pg_hba.conf`
/// that names the database for a connection over TLS.
fn access_of(name: &str, output: &str) -> Option<String> {
    let document: Value = serde_json::from_str(output).ok()?;
    let listen = document.get("server")?.as_object()?.get("listen").cloned()?;
    let rules: Vec<Value> = document
        .get("access")?
        .as_array()?
        .iter()
        .filter(|row| {
            row.get("type").and_then(Value::as_str) == Some("hostssl")
                && row
                    .get("databases")
                    .and_then(Value::as_array)
                    .is_some_and(|names| names.iter().any(|one| one.as_str() == Some(name)))
        })
        .map(|row| {
            let member = |key: &str| row.get(key).cloned().unwrap_or(Value::Null);
            json!({
                "users": member("users"),
                "address": member("address"),
                "method": member("method"),
            })
        })
        .collect();
    Some(json!({ "listen": listen, "access": rules }).to_string())
}

/// A database, by its name.
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Named {
    /// The database's name.
    pub name: String,
}

impl Args for Named {
    const FIELDS: &'static [Field] = &[NAME];

    fn check(&self) -> Result<(), Bad> {
        check_name("name", &self.name, "Nothing was changed.")
    }
}

/// `database list --json`: the server, its databases, its roles and who
/// may connect, which PostgreSQL tells only root.
pub struct List;

impl Action for List {
    const ID: &'static str = "databases.list";
    const AREA: &'static str = AREA;
    const CHANGES: bool = false;
    const PATH: &'static str = "/api/1/databases";
    const RUNS: Runs = Runs::AsAdministrator;
    const ERRORS: &'static [&'static str] = &["DB-101"];
    const TESTS: Tests = TESTS;
    const TITLE: &'static str = "List the databases";
    type Args = Nothing;

    fn argv(_: &Nothing) -> Vec<String> {
        list_argv()
    }

    fn examples() -> Examples<Nothing> {
        Examples { good: vec![Nothing {}], bad: vec![json!({ "all": true })], undo: None }
    }
}

/// A database to make, and who owns it.
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Adding {
    /// The database's name, and its owner's when no owner is named.
    pub name: String,
    /// The role that owns it, with a password.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,
    /// The account that owns it and signs in by the socket.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub peer: Option<String>,
    /// How it sorts text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub locale: Option<String>,
    /// The website it is for.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub site: Option<String>,
}

impl Args for Adding {
    const FIELDS: &'static [Field] = &[NAME, OWNER, PEER, LOCALE, SITE];

    fn check(&self) -> Result<(), Bad> {
        const DONE: &str = "Nothing was made.";
        check_name("name", &self.name, DONE)?;
        if let Some(owner) = &self.owner {
            check_name("owner", owner, DONE)?;
        }
        if let Some(peer) = &self.peer {
            let account = accounts::check_name(peer).is_ok() && names::check_name(peer).is_ok();
            if !account {
                return Err(Bad {
                    field: "peer",
                    why: format!(
                        "An account that owns a database is also its role, so its name is a \
                         lowercase letter, then lowercase letters, digits and _, up to {} \
                         characters, not help, and not a name PostgreSQL keeps for itself. {DONE}",
                        accounts::NAME_MAX
                    ),
                });
            }
        }
        if let Some(locale) = &self.locale {
            names::check_locale(locale).map_err(|why| bad("locale", &why, DONE))?;
        }
        if let Some(site) = &self.site {
            names::check_site(site).map_err(|_| Bad {
                field: "site",
                why: format!(
                    "A website's name has lowercase letters, digits and hyphens in labels of up \
                     to 63 characters, with dots between them. {DONE}"
                ),
            })?;
        }
        Ok(())
    }
}

/// `database add NAME ... --json`: makes the database, and the role that
/// owns it with a password or, for an account, none. The answer holds the
/// password, once: the route is a `POST` that keeps nothing of it.
pub struct Add;

impl Action for Add {
    const ID: &'static str = "databases.add";
    const AREA: &'static str = AREA;
    const CHANGES: bool = true;
    const PATH: &'static str = "/api/1/databases/add";
    const RUNS: Runs = Runs::AsAdministrator;
    const SECRET_ANSWER: bool = true;
    const ERRORS: &'static [&'static str] = &["DB-101", "DB-102", "DB-106"];
    const TESTS: Tests = TESTS;
    const TITLE: &'static str = "Add a database";
    type Args = Adding;

    fn argv(args: &Adding) -> Vec<String> {
        let mut argv: Vec<String> = ["database", "add", &args.name].map(String::from).to_vec();
        for (option, value) in [
            ("--owner", &args.owner),
            ("--peer", &args.peer),
            ("--locale", &args.locale),
            ("--site", &args.site),
        ] {
            if let Some(value) = value {
                argv.extend([option.to_string(), value.clone()]);
            }
        }
        argv.push("--json".into());
        argv
    }

    fn subject(args: &Adding) -> Option<String> {
        Some(args.name.clone())
    }

    /// The command, run so that a second run with the database there
    /// changes nothing: it ends with DB-102 on its error output, which is
    /// no failure of the play. The password is in the registered answer
    /// (`answer`, which `no_log` hides) and nowhere else: the command
    /// puts it only in its JSON, on its output, and no argument, file or
    /// log line has it.
    fn ansible(args: &Adding) -> Ansible {
        let mut task = command_task::<Self>(args);
        task.changed_when = Some(format!("{RESULT}.rc == 0"));
        task.failed_when = Some(format!("{RESULT}.rc != 0 and 'DB-102' not in {RESULT}.stderr"));
        task
    }

    fn probe(_: &Adding) -> Option<Vec<String>> {
        Some(list_argv())
    }

    fn probed(args: &Adding, output: &str) -> Option<String> {
        entry_of(&args.name, output)
    }

    fn examples() -> Examples<Adding> {
        Examples {
            good: vec![Adding {
                name: "lab".into(),
                owner: Some("lab".into()),
                peer: None,
                locale: None,
                site: None,
            }],
            bad: vec![
                json!({ "name": "pg_x" }),
                json!({ "name": "Shop" }),
                json!({ "name": "postgres" }),
                json!({ "name": "a;b" }),
                json!({ "name": "x", "locale": "de_DE.UTF-8" }),
            ],
            undo: Some(Undo {
                action: "databases.remove",
                args: json!({ "name": "lab", "role": true }),
            }),
        }
    }
}

/// `database password NAME --json`: a new password for the role that owns
/// the database, shown once in the answer, which keeps nothing of it.
pub struct Password;

impl Action for Password {
    const ID: &'static str = "databases.password";
    const AREA: &'static str = AREA;
    const CHANGES: bool = true;
    const PATH: &'static str = "/api/1/databases/{name}/password";
    const RUNS: Runs = Runs::AsAdministrator;
    const SECRET_ANSWER: bool = true;
    const ERRORS: &'static [&'static str] = &["DB-101"];
    const TESTS: Tests = TESTS;
    const TITLE: &'static str = "Make a new password for a database's role";
    type Args = Named;

    fn argv(args: &Named) -> Vec<String> {
        ["database", "password", &args.name, "--json"].map(String::from).to_vec()
    }

    fn subject(args: &Named) -> Option<String> {
        Some(args.name.clone())
    }

    fn examples() -> Examples<Named> {
        Examples {
            good: vec![Named { name: "lab".into() }],
            bad: vec![json!({ "name": "Shop" }), json!({ "name": "a;b" }), json!({})],
            // The old password is gone; the page's dialog says so.
            undo: None,
        }
    }
}

/// Where to let in, and with which certificate.
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Allowing {
    /// The database's name.
    pub name: String,
    /// The networks, one on each line.
    pub from: String,
    /// The certificate PostgreSQL serves; `this-machine` without one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub certificate: Option<String>,
}

impl Args for Allowing {
    const FIELDS: &'static [Field] = &[NAME, FROM, CERTIFICATE];

    fn check(&self) -> Result<(), Bad> {
        const DONE: &str = "Nothing was changed.";
        check_name("name", &self.name, DONE)?;
        check_from(&self.from, DONE)?;
        if let Some(certificate) = &self.certificate {
            check_certificate(certificate).map_err(|_| Bad {
                field: "certificate",
                why: format!(
                    "A certificate's name has 1 to {CERTIFICATE_NAME_MAX} lowercase letters, \
                     digits, hyphens and dots, starts with a letter or a digit, and has no two \
                     dots in a row. {DONE}"
                ),
            })?;
        }
        Ok(())
    }
}

/// `database allow NAME --from NETWORK... --progress`: lets the networks
/// sign in to the database from other machines, over TLS, and opens port
/// 5432 in the firewall to them, as a task: PostgreSQL restarts when it
/// has to listen on the network or serve another certificate.
pub struct Allow;

impl Action for Allow {
    const ID: &'static str = "databases.allow";
    const AREA: &'static str = AREA;
    const CHANGES: bool = true;
    const PATH: &'static str = "/api/1/databases/{name}/allow";
    const RUNS: Runs = Runs::AsAdministrator;
    const LONG: bool = true;
    const ERRORS: &'static [&'static str] = &["DB-101", "DB-104", "DB-105", "DB-107", "DB-108"];
    const TESTS: Tests = TESTS;
    const TITLE: &'static str = "Let other machines into a database";
    type Args = Allowing;

    fn argv(args: &Allowing) -> Vec<String> {
        let mut argv: Vec<String> = ["database", "allow", &args.name].map(String::from).to_vec();
        argv.extend(from_words(&args.from));
        if let Some(certificate) = &args.certificate {
            argv.extend(["--certificate".to_string(), certificate.clone()]);
        }
        argv.push("--progress".into());
        argv
    }

    fn subject(args: &Allowing) -> Option<String> {
        Some(args.name.clone())
    }

    fn files(_: &Allowing) -> Vec<String> {
        vec![OVERRIDE.into(), FIREWALL.into()]
    }

    fn probe(_: &Allowing) -> Option<Vec<String>> {
        Some(list_argv())
    }

    fn probed(args: &Allowing, output: &str) -> Option<String> {
        access_of(&args.name, output)
    }

    fn examples() -> Examples<Allowing> {
        Examples {
            good: vec![Allowing {
                name: "lab".into(),
                from: "10.0.2.0/24".into(),
                certificate: None,
            }],
            bad: vec![
                json!({ "name": "lab", "from": "10.0.2.0" }),
                json!({ "name": "lab", "from": "anywhere" }),
            ],
            undo: Some(Undo {
                action: "databases.deny",
                args: json!({ "name": "lab", "from": "10.0.2.0/24" }),
            }),
        }
    }
}

/// Where to shut out.
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Denying {
    /// The database's name.
    pub name: String,
    /// The networks, one on each line; every one without.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
}

impl Args for Denying {
    const FIELDS: &'static [Field] = &[NAME, FROM.optional()];

    fn check(&self) -> Result<(), Bad> {
        const DONE: &str = "Nothing was changed.";
        check_name("name", &self.name, DONE)?;
        if let Some(from) = &self.from {
            check_from(from, DONE)?;
        }
        Ok(())
    }
}

/// `database deny NAME [--from NETWORK]... --progress`: shuts the networks
/// out of the database again, or every one, and shuts port 5432 to those
/// that no database names, as a task: PostgreSQL listens on this machine
/// only again when none is left.
pub struct Deny;

impl Action for Deny {
    const ID: &'static str = "databases.deny";
    const AREA: &'static str = AREA;
    const CHANGES: bool = true;
    const PATH: &'static str = "/api/1/databases/{name}/deny";
    const RUNS: Runs = Runs::AsAdministrator;
    const LONG: bool = true;
    const ERRORS: &'static [&'static str] = &["DB-101", "DB-104", "DB-107", "DB-108"];
    const TESTS: Tests = TESTS;
    const TITLE: &'static str = "Shut other machines out of a database";
    type Args = Denying;

    fn argv(args: &Denying) -> Vec<String> {
        let mut argv: Vec<String> = ["database", "deny", &args.name].map(String::from).to_vec();
        if let Some(from) = &args.from {
            argv.extend(from_words(from));
        }
        argv.push("--progress".into());
        argv
    }

    fn subject(args: &Denying) -> Option<String> {
        Some(args.name.clone())
    }

    fn files(_: &Denying) -> Vec<String> {
        vec![OVERRIDE.into(), FIREWALL.into()]
    }

    fn probe(_: &Denying) -> Option<Vec<String>> {
        Some(list_argv())
    }

    fn probed(args: &Denying, output: &str) -> Option<String> {
        access_of(&args.name, output)
    }

    fn examples() -> Examples<Denying> {
        Examples {
            good: vec![Denying { name: "lab".into(), from: Some("10.0.2.0/24".into()) }],
            bad: vec![
                json!({ "name": "lab", "from": "10.0.2.0" }),
                json!({ "name": "lab", "from": "" }),
            ],
            undo: Some(Undo {
                action: "databases.allow",
                args: json!({ "name": "lab", "from": "10.0.2.0/24" }),
            }),
        }
    }
}

/// A database to remove, and what goes with it.
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Removing {
    /// The database's name.
    pub name: String,
    /// Whether its role goes too.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<bool>,
    /// Whether the sessions open on it are ended.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub force: Option<bool>,
}

impl Args for Removing {
    const FIELDS: &'static [Field] = &[NAME, ROLE, FORCE];

    fn check(&self) -> Result<(), Bad> {
        check_name("name", &self.name, "Nothing was removed.")
    }
}

/// `database remove NAME [--role] [--force]`: removes the database, and
/// with `role` the role that owns it, and shuts port 5432 in the firewall
/// to the networks that no database names any more. It is the machine's
/// data that goes: the playbook has to say it is meant too, in
/// `dukovo_confirm_purge`.
pub struct Remove;

impl Action for Remove {
    const ID: &'static str = "databases.remove";
    const AREA: &'static str = AREA;
    const CHANGES: bool = true;
    const PATH: &'static str = "/api/1/databases/{name}/remove";
    const RUNS: Runs = Runs::AsAdministrator;
    const ERRORS: &'static [&'static str] = &["DB-101", "DB-103", "DB-107", "DB-108"];
    const TESTS: Tests = TESTS;
    const TITLE: &'static str = "Remove a database";
    type Args = Removing;

    fn argv(args: &Removing) -> Vec<String> {
        let mut argv: Vec<String> = ["database", "remove", &args.name].map(String::from).to_vec();
        for (option, given) in [("--role", args.role), ("--force", args.force)] {
            if given == Some(true) {
                argv.push(option.into());
            }
        }
        argv
    }

    fn subject(args: &Removing) -> Option<String> {
        Some(args.name.clone())
    }

    fn ansible(args: &Removing) -> Ansible {
        command_task::<Self>(args).when("dukovo_confirm_purge | bool")
    }

    fn files(_: &Removing) -> Vec<String> {
        vec![FIREWALL.into()]
    }

    fn probe(_: &Removing) -> Option<Vec<String>> {
        Some(list_argv())
    }

    fn probed(args: &Removing, output: &str) -> Option<String> {
        entry_of(&args.name, output)
    }

    fn examples() -> Examples<Removing> {
        Examples {
            good: vec![Removing { name: "lab".into(), role: Some(true), force: None }],
            bad: vec![
                json!({ "name": "pg_x" }),
                json!({ "name": "Shop", "role": true }),
                json!({ "name": "lab", "role": "yes" }),
            ],
            // What was removed is made again by `databases.add`, with a
            // new password and no data.
            undo: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::show::yaml;
    use super::super::{Caller, Found, REGISTRY, Registered, authorize, find};
    use super::*;
    use crate::admind::errors::Kind;
    use crate::admind::scopes::Scopes;

    fn action(id: &str) -> &'static dyn Registered {
        *REGISTRY.iter().find(|action| action.id() == id).unwrap()
    }

    /// Why `id` refuses `args`: the kind, the field and the words.
    fn refused(id: &str, args: Value) -> (Kind, Option<&'static str>, String) {
        let failure = action(id).prepare(args).unwrap_err();
        (failure.kind, failure.field, failure.detail)
    }

    /// The words of a command that a good example makes.
    fn argv(id: &str, args: Value) -> Vec<String> {
        action(id).prepare(args).unwrap().argv
    }

    /// What `database list --json` prints, as far as the actions read it:
    /// a database with a rule for another network, and the rules the
    /// server has of its own.
    const LIST: &str = r#"{
        "format": 1,
        "server": {"installed": true, "enabled": true, "running": true, "version": "19.0",
                   "listen": "*", "ssl": true, "certificate": "this-machine",
                   "firewall": {"networks": ["192.0.2.0/24", "198.51.100.0/24"],
                                "earlier": null}},
        "databases": [
            {"name": "shop", "owner": "shop", "encoding": "UTF8", "provider": "libc",
             "locale": "en_US.UTF-8", "bytes": 8192, "sessions": 1, "site": "shop.example.org",
             "made_by_database": true, "collation_mismatch": false}
        ],
        "roles": [],
        "access": [
            {"line": 5, "type": "host", "databases": ["all"], "users": ["all"],
             "address": "127.0.0.1/32", "method": "scram-sha-256", "by_hand": true},
            {"line": 9, "type": "hostssl", "databases": ["shop"], "users": ["shop"],
             "address": "192.0.2.0/24", "method": "scram-sha-256", "by_hand": false},
            {"line": 10, "type": "hostssl", "databases": ["blog"], "users": ["blog"],
             "address": "198.51.100.0/24", "method": "scram-sha-256", "by_hand": false}
        ],
        "problems": []
    }"#;

    #[test]
    fn the_list_is_the_one_read_and_every_change_is_an_administrator_s_post() {
        for (id, post, path, long, secret, errors) in [
            ("databases.list", false, "/api/1/databases", false, false, vec!["DB-101"]),
            (
                "databases.add",
                true,
                "/api/1/databases/add",
                false,
                true,
                vec!["DB-101", "DB-102", "DB-106"],
            ),
            (
                "databases.password",
                true,
                "/api/1/databases/{name}/password",
                false,
                true,
                vec!["DB-101"],
            ),
            (
                "databases.allow",
                true,
                "/api/1/databases/{name}/allow",
                true,
                false,
                vec!["DB-101", "DB-104", "DB-105", "DB-107", "DB-108"],
            ),
            (
                "databases.deny",
                true,
                "/api/1/databases/{name}/deny",
                true,
                false,
                vec!["DB-101", "DB-104", "DB-107", "DB-108"],
            ),
            (
                "databases.remove",
                true,
                "/api/1/databases/{name}/remove",
                false,
                false,
                vec!["DB-101", "DB-103", "DB-107", "DB-108"],
            ),
        ] {
            let one = action(id);
            assert_eq!(one.area(), "databases", "{id}");
            assert_eq!(one.changes(), post, "{id}");
            assert_eq!(one.posts(), post, "{id}");
            assert_eq!(one.path(), path, "{id}");
            assert_eq!(one.runs(), Runs::AsAdministrator, "{id}");
            assert_eq!((one.long(), one.secret_answer()), (long, secret), "{id}");
            assert_eq!(one.errors(), errors, "{id}");
            assert_eq!(one.answers(), [0], "{id}");
            assert!(!one.lines(), "{id}");
            assert!(one.restarts().is_none() && one.cuts_off_on().is_none(), "{id}");
            assert_eq!(one.tests().checks, "admind::actions::databases::tests", "{id}");
            assert_eq!(one.tests().browser, "web-console/databases", "{id}");
            assert_eq!(one.tests().route, "admin-websites", "{id}");
        }
    }

    #[test]
    fn the_list_is_a_read_of_the_commands_json_and_takes_nothing() {
        let list = action("databases.list");
        let prepared = list.prepare(json!({})).unwrap();
        assert_eq!(prepared.argv, ["database", "list", "--json"]);
        assert!(prepared.files.is_empty() && prepared.probe.is_none());
        assert!(prepared.ansible.become_root);
        assert_eq!(prepared.ansible.changed_when.as_deref(), Some("false"));
        assert!(!list.compares());
        assert_eq!(refused("databases.list", json!({ "all": true })).0, Kind::BadRequest);
    }

    #[test]
    fn a_name_is_the_rule_of_database_for_every_action_that_takes_one() {
        let good =
            ["lab", "shop_2", "a", "x9", "pg", "pgx", "postgres2", "template", "add", "list"];
        for name in good.iter().copied().chain([&*"a".repeat(63)]) {
            for (id, args) in [
                ("databases.password", json!({ "name": name })),
                ("databases.remove", json!({ "name": name })),
                ("databases.allow", json!({ "name": name, "from": "10.0.0.0/8" })),
                ("databases.deny", json!({ "name": name })),
                ("databases.add", json!({ "name": name, "owner": name })),
            ] {
                assert!(action(id).prepare(args).is_ok(), "{id}: {name}");
            }
        }
        let long = "a".repeat(64);
        let bad = [
            "",
            "pg_x",
            "pg_",
            "postgres",
            "template0",
            "template1",
            "Shop",
            "1x",
            "_x",
            "a-b",
            "a b",
            "a;b",
            "a.b",
            "a\"b",
            "caf\u{e9}",
            "x\n",
            "\nx",
            "x\0",
            &long,
        ];
        for name in bad {
            for (id, args) in [
                ("databases.password", json!({ "name": name })),
                ("databases.remove", json!({ "name": name })),
                ("databases.allow", json!({ "name": name, "from": "10.0.0.0/8" })),
                ("databases.deny", json!({ "name": name })),
                ("databases.add", json!({ "name": name })),
            ] {
                let (kind, field, _) = refused(id, args);
                assert_eq!((kind, field), (Kind::BadValue, Some("name")), "{id}: {name:?}");
            }
        }
        // The owner is named by the same rule, and says it is the owner.
        let (kind, field, words) =
            refused("databases.add", json!({ "name": "lab", "owner": "Pg" }));
        assert_eq!((kind, field), (Kind::BadValue, Some("owner")));
        assert!(words.ends_with("Nothing was made."), "{words}");
        // The words are the command's, and say what was not done.
        let (_, _, words) = refused("databases.remove", json!({ "name": "pg_x" }));
        assert_eq!(
            words,
            "`pg_x` is a name PostgreSQL keeps for itself: choose another, not one that starts \
             with pg_, and not postgres, template0 or template1. Nothing was removed."
        );
    }

    #[test]
    fn a_database_is_added_with_the_options_it_is_asked_for_and_json() {
        assert_eq!(
            argv("databases.add", json!({ "name": "lab" })),
            ["database", "add", "lab", "--json"]
        );
        assert_eq!(
            argv(
                "databases.add",
                json!({
                    "name": "shop",
                    "owner": "shop_owner",
                    "locale": "C.UTF-8",
                    "site": "shop.example.org",
                })
            ),
            [
                "database",
                "add",
                "shop",
                "--owner",
                "shop_owner",
                "--locale",
                "C.UTF-8",
                "--site",
                "shop.example.org",
                "--json"
            ]
        );
        assert_eq!(
            argv("databases.add", json!({ "name": "lab", "peer": "ann", "locale": "en_US.UTF-8" })),
            ["database", "add", "lab", "--peer", "ann", "--locale", "en_US.UTF-8", "--json"]
        );
        for (args, field) in [
            (json!({ "name": "lab", "locale": "de_DE.UTF-8" }), "locale"),
            (json!({ "name": "lab", "locale": "c.utf-8" }), "locale"),
            (json!({ "name": "lab", "peer": "Ann" }), "peer"),
            (json!({ "name": "lab", "peer": "help" }), "peer"),
            (json!({ "name": "lab", "peer": "" }), "peer"),
            // An account's name that is not a role's, and a role's that no account has.
            (json!({ "name": "lab", "peer": "ann-b" }), "peer"),
            (json!({ "name": "lab", "peer": "_ann" }), "peer"),
            (json!({ "name": "lab", "peer": "a.b" }), "peer"),
            (json!({ "name": "lab", "peer": "pg_ann" }), "peer"),
            (json!({ "name": "lab", "peer": "postgres" }), "peer"),
            (json!({ "name": "lab", "peer": "a".repeat(33) }), "peer"),
            (json!({ "name": "lab", "site": "a b" }), "site"),
            (json!({ "name": "lab", "site": "*.example.org" }), "site"),
            (json!({ "name": "lab", "site": "Shop.example.org" }), "site"),
            (json!({ "name": "lab", "site": "shop..example.org" }), "site"),
            (json!({ "name": "lab", "site": "-x.example.org" }), "site"),
        ] {
            let (kind, found, words) = refused("databases.add", args.clone());
            assert_eq!((kind, found), (Kind::BadValue, Some(field)), "{args}");
            assert!(words.ends_with("Nothing was made."), "{args}: {words}");
        }
        assert_eq!(refused("databases.add", json!({})).0, Kind::BadRequest);
        assert_eq!(
            refused("databases.add", json!({ "name": "lab", "password": "x" })).0,
            Kind::BadRequest
        );
    }

    #[test]
    fn the_locales_are_the_ones_database_makes() {
        assert_eq!(LOCALES, names::LOCALES);
        for locale in LOCALES {
            assert!(names::check_locale(locale).is_ok(), "{locale}");
        }
    }

    #[test]
    fn a_password_is_in_the_answer_alone_and_the_task_keeps_it_from_the_play() {
        for id in ["databases.add", "databases.password"] {
            let one = action(id);
            assert!(one.posts() && one.secret_answer() && !one.long(), "{id}");
            // The password is made by the command: nothing is sent to it.
            assert!(one.fields().iter().all(|field| !field.is_input()), "{id}");
            let (good, _, _) = one.example_values();
            let prepared = one.prepare(good[0].clone()).unwrap();
            assert!(prepared.input.is_none(), "{id}");
            assert!(prepared.argv.contains(&"--json".to_string()), "{id}");
            assert!(prepared.ansible.no_log, "{id}");
            assert_eq!(prepared.ansible.register, Some("answer"), "{id}");
            assert!(prepared.ansible.become_root, "{id}");
            assert!(prepared.ansible.params.get("stdin").is_none(), "{id}");
            let text = yaml(&prepared.ansible);
            assert!(text.contains("  no_log: true\n  register: answer\n"), "{id}: {text}");
        }
        let add = action("databases.add").prepare(json!({ "name": "lab" })).unwrap();
        assert_eq!(add.ansible.name, "Add a database: lab");
        assert_eq!(add.ansible.changed_when.as_deref(), Some("answer.rc == 0"));
        assert_eq!(
            add.ansible.failed_when.as_deref(),
            Some("answer.rc != 0 and 'DB-102' not in answer.stderr")
        );
        assert!(yaml(&add.ansible).ends_with(
            "  no_log: true\n  register: answer\n  changed_when: \"answer.rc == 0\"\n  \
             failed_when: \"answer.rc != 0 and 'DB-102' not in answer.stderr\"\n"
        ));
        let password = action("databases.password").prepare(json!({ "name": "lab" })).unwrap();
        assert_eq!(password.argv, ["database", "password", "lab", "--json"]);
        assert_eq!(password.ansible.name, "Make a new password for a database's role: lab");
        assert!(password.ansible.changed_when.is_none() && password.ansible.failed_when.is_none());
        // Nothing but the name goes to `password`.
        assert_eq!(
            refused("databases.password", json!({ "name": "lab", "password": "x" })).0,
            Kind::BadRequest
        );
    }

    #[test]
    fn allow_names_each_network_after_from_and_the_certificate_when_there_is_one() {
        assert_eq!(
            argv("databases.allow", json!({ "name": "lab", "from": "192.168.1.0/24\n10.0.0.0/8" })),
            [
                "database",
                "allow",
                "lab",
                "--from",
                "192.168.1.0/24",
                "--from",
                "10.0.0.0/8",
                "--progress"
            ]
        );
        assert_eq!(
            argv(
                "databases.allow",
                json!({ "name": "lab", "from": "2001:db8::/32", "certificate": "this-machine" })
            ),
            [
                "database",
                "allow",
                "lab",
                "--from",
                "2001:db8::/32",
                "--certificate",
                "this-machine",
                "--progress"
            ]
        );
        let prepared = action("databases.allow")
            .prepare(json!({ "name": "lab", "from": "10.0.2.0/24" }))
            .unwrap();
        assert_eq!(prepared.files, [OVERRIDE, FIREWALL]);
        assert!(prepared.probe.is_some() && prepared.ansible.become_root);
        assert_eq!(prepared.ansible.name, "Let other machines into a database: lab");
        assert!(action("databases.allow").long() && action("databases.allow").compares());
        let twenty: Vec<String> = (0..20).map(|n| format!("10.0.{n}.0/24")).collect();
        assert!(
            action("databases.allow")
                .prepare(json!({ "name": "lab", "from": twenty.join("\n") }))
                .is_ok()
        );
        let twenty_one: Vec<String> = (0..21).map(|n| format!("10.0.{n}.0/24")).collect();
        for from in [
            "",
            "10.0.2.0",
            "anywhere",
            "10.0.2.0/33",
            "::1/129",
            "10.0.2.0/",
            "10.0.2.0/24 ",
            "10.0.2.0/24\n",
            "\n10.0.2.0/24",
            "10.0.2.0/24\n\n10.0.3.0/24",
            "10.0.2.0/24\n10.0.2.0/24",
            "10.0.2.0/24,10.0.3.0/24",
            "999.0.0.0/8",
            "example.org/24",
            &twenty_one.join("\n"),
        ] {
            let (kind, field, words) =
                refused("databases.allow", json!({ "name": "lab", "from": from }));
            assert_eq!((kind, field), (Kind::BadValue, Some("from")), "{from:?}");
            assert!(words.ends_with("Nothing was changed."), "{from:?}: {words}");
        }
        for certificate in ["", "This-Machine", "a..b", "../x", "-x", "this machine"] {
            let (kind, field, _) = refused(
                "databases.allow",
                json!({ "name": "lab", "from": "10.0.2.0/24", "certificate": certificate }),
            );
            assert_eq!((kind, field), (Kind::BadValue, Some("certificate")), "{certificate:?}");
        }
        assert_eq!(refused("databases.allow", json!({ "name": "lab" })).0, Kind::BadRequest);
    }

    #[test]
    fn deny_names_each_network_or_none_for_every_one() {
        assert_eq!(
            argv("databases.deny", json!({ "name": "lab" })),
            ["database", "deny", "lab", "--progress"]
        );
        assert_eq!(
            argv("databases.deny", json!({ "name": "lab", "from": "10.0.2.0/24\n10.0.3.0/24" })),
            [
                "database",
                "deny",
                "lab",
                "--from",
                "10.0.2.0/24",
                "--from",
                "10.0.3.0/24",
                "--progress"
            ]
        );
        let deny = action("databases.deny");
        let prepared = deny.prepare(json!({ "name": "lab" })).unwrap();
        assert_eq!(prepared.files, [OVERRIDE, FIREWALL]);
        assert!(deny.long() && deny.compares() && prepared.probe.is_some());
        for from in ["", "10.0.2.0", "anywhere", "10.0.2.0/24\n10.0.2.0/24"] {
            let (kind, field, words) =
                refused("databases.deny", json!({ "name": "lab", "from": from }));
            assert_eq!((kind, field), (Kind::BadValue, Some("from")), "{from:?}");
            assert!(words.ends_with("Nothing was changed."), "{from:?}: {words}");
        }
    }

    #[test]
    fn a_removal_names_its_flags_and_the_playbook_confirms_it() {
        assert_eq!(
            argv("databases.remove", json!({ "name": "lab" })),
            ["database", "remove", "lab"]
        );
        assert_eq!(
            argv("databases.remove", json!({ "name": "lab", "role": true })),
            ["database", "remove", "lab", "--role"]
        );
        assert_eq!(
            argv("databases.remove", json!({ "name": "lab", "role": true, "force": true })),
            ["database", "remove", "lab", "--role", "--force"]
        );
        // A flag that is false is a flag left out.
        assert_eq!(
            argv("databases.remove", json!({ "name": "lab", "role": false, "force": false })),
            ["database", "remove", "lab"]
        );
        let remove = action("databases.remove");
        assert!(remove.changes() && !remove.long() && remove.compares());
        let prepared = remove.prepare(json!({ "name": "lab" })).unwrap();
        assert_eq!(prepared.ansible.when.as_deref(), Some("dukovo_confirm_purge | bool"));
        assert!(yaml(&prepared.ansible).ends_with("  when: \"dukovo_confirm_purge | bool\"\n"));
        assert!(prepared.ansible.become_root && !prepared.ansible.no_log);
        // The firewall's rules, which lose the networks only it named.
        assert_eq!(prepared.files, [FIREWALL]);
        for (args, field) in [
            (json!({ "name": "lab", "role": "yes" }), None),
            (json!({ "name": "lab", "force": 1 }), None),
            (json!({ "name": "Lab" }), Some("name")),
        ] {
            let (kind, found, _) = refused("databases.remove", args.clone());
            assert_eq!(found, field, "{args}");
            assert_eq!(kind, if field.is_some() { Kind::BadValue } else { Kind::BadRequest });
        }
        // Nothing puts back the data that went.
        assert_eq!(remove.example_values().2, None);
        assert_eq!(action("databases.password").example_values().2, None);
    }

    #[test]
    fn each_change_but_a_password_reads_the_list_and_picks_what_it_moves() {
        let probe = ["database", "list", "--json"].map(String::from).to_vec();
        for (id, args) in [
            ("databases.add", json!({ "name": "shop" })),
            ("databases.remove", json!({ "name": "shop" })),
            ("databases.allow", json!({ "name": "shop", "from": "192.0.2.0/24" })),
            ("databases.deny", json!({ "name": "shop" })),
        ] {
            let prepared = action(id).prepare(args).unwrap();
            assert_eq!(prepared.probe, Some(probe.clone()), "{id}");
            assert!(action(id).compares(), "{id}");
        }
        assert!(!action("databases.password").compares());
        // The database's entry is its owner, encoding, sorting and site;
        // what it holds and who is signed in to it are not the change's.
        let shop = json!({ "name": "shop" });
        let picked = action("databases.add").probed(&shop, LIST).unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(&picked).unwrap(),
            json!({
                "owner": "shop",
                "encoding": "UTF8",
                "provider": "libc",
                "locale": "en_US.UTF-8",
                "site": "shop.example.org",
            })
        );
        let busier = LIST.replace("\"sessions\": 1", "\"sessions\": 7").replace("8192", "16384");
        assert_ne!(busier, LIST);
        assert_eq!(action("databases.add").probed(&shop, &busier), Some(picked.clone()));
        assert_eq!(action("databases.remove").probed(&shop, LIST), Some(picked));
        assert_eq!(
            action("databases.add").probed(&json!({ "name": "other" }), LIST),
            Some("none".into())
        );
        // The rules that name the database for TLS, and where the server
        // listens: not the rules of every database, nor the server's own.
        let allowed = action("databases.allow")
            .probed(&json!({ "name": "shop", "from": "192.0.2.0/24" }), LIST)
            .unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(&allowed).unwrap(),
            json!({
                "listen": "*",
                "access": [{
                    "users": ["shop"],
                    "address": "192.0.2.0/24",
                    "method": "scram-sha-256",
                }],
            })
        );
        assert_eq!(action("databases.deny").probed(&shop, LIST), Some(allowed));
        let none = action("databases.deny").probed(&json!({ "name": "lab" }), LIST).unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(&none).unwrap(),
            json!({ "listen": "*", "access": [] })
        );
        for output in ["", "not json", "[]", r#"{"databases": 3}"#, r#"{"format": 1}"#] {
            assert_eq!(action("databases.add").probed(&shop, output), None, "{output:?}");
            assert_eq!(action("databases.deny").probed(&shop, output), None, "{output:?}");
        }
        assert_eq!(action("databases.add").probed(&json!({ "name": 3 }), LIST), None);
    }

    #[test]
    fn an_example_is_put_back_by_the_action_that_undoes_it() {
        let undo = |id: &str| action(id).example_values().2;
        assert_eq!(
            undo("databases.add"),
            Some(Undo { action: "databases.remove", args: json!({ "name": "lab", "role": true }) })
        );
        assert_eq!(
            undo("databases.allow"),
            Some(Undo {
                action: "databases.deny",
                args: json!({ "name": "lab", "from": "10.0.2.0/24" })
            })
        );
        assert_eq!(
            undo("databases.deny"),
            Some(Undo {
                action: "databases.allow",
                args: json!({ "name": "lab", "from": "10.0.2.0/24" })
            })
        );
        let (good, _, _) = action("databases.add").example_values();
        assert_eq!(good, [json!({ "name": "lab", "owner": "lab" })]);
        let (good, _, _) = action("databases.allow").example_values();
        assert_eq!(good, [json!({ "name": "lab", "from": "10.0.2.0/24" })]);
    }

    #[test]
    fn each_bad_example_is_refused_by_the_action_that_has_it() {
        for id in [
            "databases.list",
            "databases.add",
            "databases.password",
            "databases.allow",
            "databases.deny",
            "databases.remove",
        ] {
            let (good, bad, _) = action(id).example_values();
            assert!(!good.is_empty() && !bad.is_empty(), "{id}");
            for args in bad {
                assert!(action(id).prepare(args.clone()).is_err(), "{id}: {args}");
            }
        }
    }

    #[test]
    fn the_routes_are_found_by_their_methods_and_add_is_a_word_of_its_own() {
        for (post, path, id) in [
            (false, "/api/1/databases", "databases.list"),
            (true, "/api/1/databases/add", "databases.add"),
            (true, "/api/1/databases/lab/password", "databases.password"),
            (true, "/api/1/databases/lab/allow", "databases.allow"),
            (true, "/api/1/databases/lab/deny", "databases.deny"),
            (true, "/api/1/databases/lab/remove", "databases.remove"),
            // A database called `add` is changed by two parts, as any.
            (true, "/api/1/databases/add/remove", "databases.remove"),
        ] {
            let found = find(REGISTRY, post, path);
            assert!(
                matches!(&found, Found::Action(action, _) if action.id() == id),
                "{path}: {found:?}"
            );
        }
        let Found::Action(_, params) = find(REGISTRY, true, "/api/1/databases/add/remove") else {
            panic!()
        };
        assert_eq!(Value::Object(params), json!({ "name": "add" }));
        assert!(matches!(find(REGISTRY, false, "/api/1/databases/add"), Found::WrongMethod));
        assert!(matches!(find(REGISTRY, true, "/api/1/databases"), Found::WrongMethod));
        assert!(matches!(
            find(REGISTRY, false, "/api/1/databases/lab/password"),
            Found::WrongMethod
        ));
        // No route takes a one-part name.
        assert!(matches!(find(REGISTRY, false, "/api/1/databases/lab"), Found::None));
        assert!(matches!(find(REGISTRY, true, "/api/1/databases/lab"), Found::None));
    }

    #[test]
    fn a_token_needs_the_whole_area_and_a_session_an_unlocked_administrator() {
        let ids = [
            "databases.list",
            "databases.add",
            "databases.password",
            "databases.allow",
            "databases.deny",
            "databases.remove",
        ];
        let call = |list: &str, id: &str| {
            let scopes = Scopes::parse(list).unwrap();
            authorize(&Caller::Token { name: "ci", scopes: &scopes }, action(id))
        };
        for id in ids {
            // PostgreSQL lets only root in, so even the list is run as an
            // administrator, which a token may with the area's full scope.
            assert!(call("databases", id).is_ok(), "{id}");
            assert!(call("*", id).is_ok(), "{id}");
            assert_eq!(call("databases:read", id).unwrap_err().kind, Kind::OutOfScope, "{id}");
            assert_eq!(call("apps", id).unwrap_err().kind, Kind::OutOfScope, "{id}");
        }
        let session = |admin: bool, unlocked: bool, id: &str| {
            authorize(&Caller::Session { name: "ann", admin, unlocked }, action(id))
                .map_err(|f| f.kind)
        };
        for id in ids {
            assert_eq!(session(false, true, id), Err(Kind::NotAdmin), "{id}");
            assert_eq!(session(true, false, id), Err(Kind::Locked), "{id}");
            assert_eq!(session(true, true, id), Ok(()), "{id}");
        }
        // The list is the only read.
        let reads: Vec<&str> = ids.iter().copied().filter(|id| !action(id).changes()).collect();
        assert_eq!(reads, ["databases.list"]);
    }
}
