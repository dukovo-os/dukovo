// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The settings that let other machines in: the four that `ALTER SYSTEM`
//! writes to `postgresql.auto.conf`, and the certificates that PostgreSQL's
//! service file override names for Spark to copy.
//!
//! `allow` sets only what differs from what the files say now, and `deny`
//! resets only what `ALTER SYSTEM` set; a change that finds everything
//! as it wants it sends nothing, so that nothing restarts for nothing.
//! The same lists, taken the other way, are what puts a change back.

use std::collections::BTreeMap;

use manifest::service::{self, Override};

use super::sql;
use super::wire::Answer;
use crate::settings_edit;

/// Where the server listens: `*` for every address, `localhost` by
/// default.
pub const LISTEN: &str = "listen_addresses";

/// Whether the server takes TLS.
pub const SSL: &str = "ssl";

/// The file of the server's certificate and the chain after it.
pub const CERT_FILE: &str = "ssl_cert_file";

/// The file of the certificate's private key.
pub const KEY_FILE: &str = "ssl_key_file";

/// What `listen_addresses` is while other machines may connect.
pub const NETWORK: &str = "*";

/// What `listen_addresses` is when only this machine may.
pub const LOCAL: &str = "localhost";

/// The settings of this module, in the order they are set.
const KEYS: [&str; 4] = [LISTEN, SSL, CERT_FILE, KEY_FILE];

/// The folder where Spark puts the copies of PostgreSQL's certificates
/// at each start.
const LIVE: &str = "/system/live/certificates/postgresql";

/// Where the server finds the certificate `name`'s chain once Spark has
/// copied it.
#[must_use]
pub fn chain_file(name: &str) -> String {
    format!("{LIVE}/{name}/fullchain.pem")
}

/// Where the server finds the certificate `name`'s key.
#[must_use]
pub fn key_file(name: &str) -> String {
    format!("{LIVE}/{name}/key.pem")
}

/// The name of the certificate a `ssl_cert_file` is the copy of, when it
/// is one `database` set.
#[must_use]
pub fn certificate_of(path: &str) -> Option<&str> {
    let name = path.strip_prefix(LIVE)?.strip_prefix('/')?.strip_suffix("/fullchain.pem")?;
    (!name.is_empty() && !name.contains('/')).then_some(name)
}

/// One change to a setting: what it is to be, or none to reset it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Change {
    /// The setting.
    pub name: &'static str,
    /// Its new value, or none for `ALTER SYSTEM RESET`.
    pub to: Option<String>,
}

impl Change {
    /// The statement that makes it.
    ///
    /// # Errors
    ///
    /// When the value holds a NUL.
    pub fn statement(&self) -> Result<String, String> {
        match &self.to {
            Some(value) => sql::alter_system_set(self.name, value),
            None => Ok(sql::alter_system_reset(self.name)),
        }
    }
}

/// What the four settings are: as the files say them, which is what the
/// server is when it starts or reads them again, and the part of that
/// which `postgresql.auto.conf`, the file `ALTER SYSTEM` writes, says.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Settings {
    /// The value each setting has from the files: the last line that sets
    /// it, or the server's own default.
    effective: BTreeMap<String, String>,
    /// The value each setting has in `postgresql.auto.conf`, when it is
    /// there.
    auto: BTreeMap<String, String>,
}

impl Settings {
    /// The settings from the answers of [`sql::FILE_SETTINGS`], the lines
    /// of the files in the order they are read, and [`sql::SETTING_NOW`],
    /// which says what the server's default is.
    #[must_use]
    pub fn read(files: &Answer, now: &Answer) -> Settings {
        let mut settings = Settings::default();
        let column = |answer: &Answer, name: &str| answer.columns.iter().position(|c| c == name);
        if let (Some(name), Some(boot)) = (column(now, "name"), column(now, "boot_val")) {
            for row in &now.rows {
                if let (Some(Some(name)), Some(Some(boot))) = (row.get(name), row.get(boot)) {
                    settings.effective.insert(name.clone(), boot.clone());
                }
            }
        }
        let (name, value, file) =
            (column(files, "name"), column(files, "setting"), column(files, "sourcefile"));
        if let (Some(name), Some(value), Some(file)) = (name, value, file) {
            for row in &files.rows {
                let (Some(Some(name)), Some(Some(value))) = (row.get(name), row.get(value)) else {
                    continue;
                };
                settings.effective.insert(name.clone(), value.clone());
                match row.get(file) {
                    Some(Some(file)) if file.ends_with("postgresql.auto.conf") => {
                        settings.auto.insert(name.clone(), value.clone());
                    }
                    // A later line of another file is the one that counts.
                    _ => {
                        settings.auto.remove(name);
                    }
                }
            }
        }
        settings
    }

    /// What `name` is from the files.
    #[must_use]
    pub fn effective(&self, name: &str) -> Option<&str> {
        self.effective.get(name).map(String::as_str)
    }

    /// Whether the files turn TLS on, however they spell it.
    #[must_use]
    pub fn ssl_on(&self) -> bool {
        matches!(
            self.effective(SSL).map(str::to_ascii_lowercase).as_deref(),
            Some("on" | "true" | "yes" | "1" | "t" | "y")
        )
    }

    /// The value `postgresql.auto.conf` gives `name`.
    #[must_use]
    pub fn set_by_alter_system(&self, name: &str) -> Option<&str> {
        self.auto.get(name).map(String::as_str)
    }

    /// What it takes to let other machines in over TLS with the
    /// certificate `certificate`: only what the files do not say already.
    #[must_use]
    pub fn to_allow(&self, certificate: &str) -> Vec<Change> {
        let chain = chain_file(certificate);
        let key = key_file(certificate);
        let mut changes = Vec::new();
        if self.effective(LISTEN) != Some(NETWORK) {
            changes.push(Change { name: LISTEN, to: Some(NETWORK.into()) });
        }
        if !self.ssl_on() {
            changes.push(Change { name: SSL, to: Some("on".into()) });
        }
        if self.effective(CERT_FILE) != Some(chain.as_str()) {
            changes.push(Change { name: CERT_FILE, to: Some(chain) });
        }
        if self.effective(KEY_FILE) != Some(key.as_str()) {
            changes.push(Change { name: KEY_FILE, to: Some(key) });
        }
        changes
    }

    /// What it takes to have the server listen on this machine only
    /// again: `ALTER SYSTEM`'s own `listen_addresses` taken away. TLS and
    /// the certificate stay, as the server starts with them as long as the
    /// copies of the certificate are there.
    #[must_use]
    pub fn to_deny(&self) -> Vec<Change> {
        if self.set_by_alter_system(LISTEN).is_some() {
            vec![Change { name: LISTEN, to: None }]
        } else {
            Vec::new()
        }
    }

    /// What puts the settings of `changes` back as they were in `self`:
    /// each set again to the value `ALTER SYSTEM` had given it, or reset
    /// when it had none.
    #[must_use]
    pub fn to_take_back(&self, changes: &[Change]) -> Vec<Change> {
        changes
            .iter()
            .filter_map(|change| {
                let name = KEYS.iter().find(|key| **key == change.name)?;
                Some(Change { name, to: self.set_by_alter_system(name).map(String::from) })
            })
            .collect()
    }

    /// Whether `changes` need the server to start again to take effect,
    /// given where it listens now: the address it listens on, and the
    /// files its TLS reads, are read once, as it starts.
    #[must_use]
    pub fn needs_restart(&self, changes: &[Change], running_listen: &str) -> bool {
        changes.iter().any(|change| matches!(change.name, CERT_FILE | KEY_FILE))
            || self.effective(LISTEN).is_some_and(|listen| listen != running_listen)
    }
}

/// The certificates PostgreSQL's service names once `certificate` is one,
/// as the override that holds them would say: the certificates its file
/// and override name now, then `certificate`. None when it is named
/// already, and nothing is to be written.
///
/// An override's list replaces the file's whole (service.toml(5)), so a
/// new override starts from the file's. One that has a list keeps it, and
/// every item a person put in it.
///
/// # Errors
///
/// When the service file or the override does not read, or the override
/// that would be written does not.
pub fn override_with(
    file: &str,
    over: Option<&str>,
    certificate: &str,
) -> Result<Option<String>, String> {
    let parsed = match over {
        Some(text) => {
            Some(Override::from_toml(text).map_err(|why| format!("its override: {why}"))?)
        }
        None => None,
    };
    let read = service::from_toml_over(file, parsed.as_ref())
        .map_err(|why| format!("its service file: {why}"))?;
    let mut list = read.certificates;
    if !list.iter().any(|name| name == certificate) {
        list.push(certificate.to_string());
    }
    let new = settings_edit::edited(over.unwrap_or_default(), |document| {
        settings_edit::set_strings(document.as_table_mut(), "certificates", &list);
        Ok::<(), String>(())
    })
    .map_err(|why| match why {
        settings_edit::Unedited::Parse { message, .. } => format!("its override: {message}"),
        settings_edit::Unedited::Edit(why) => why,
    })?;
    let Some(new) = new else { return Ok(None) };
    // A new file says what it is above its one key.
    let new = if over.is_none() { format!("{HEADER}{new}") } else { new };
    let checked = Override::from_toml(&new)
        .and_then(|over| service::from_toml_over(file, Some(&over)))
        .map_err(|why| format!("the override would not read: {why}"))?;
    debug_assert!(checked.certificates.iter().any(|name| name == certificate));
    Ok(Some(new))
}

/// What a new override starts with.
const HEADER: &str = "\
# Keys database(8) keeps for PostgreSQL's service (ADR 0569). Your own keys
# here are kept; a list database changes keeps every item you added.
";

#[cfg(test)]
mod tests {
    use super::*;

    /// An answer with `columns` and `rows`, the cells text.
    fn answer(columns: &[&str], rows: &[&[Option<&str>]]) -> Answer {
        Answer {
            columns: columns.iter().map(|c| (*c).to_string()).collect(),
            rows: rows
                .iter()
                .map(|row| row.iter().map(|cell| cell.map(String::from)).collect())
                .collect(),
            tag: format!("SELECT {}", rows.len()),
        }
    }

    /// What a server that has never been told anything says.
    fn defaults() -> Answer {
        answer(
            &["name", "setting", "boot_val"],
            &[
                &[Some("listen_addresses"), Some("localhost"), Some("localhost")],
                &[Some("ssl"), Some("off"), Some("off")],
                &[Some("ssl_cert_file"), Some("server.crt"), Some("server.crt")],
                &[Some("ssl_key_file"), Some("server.key"), Some("server.key")],
            ],
        )
    }

    fn files(rows: &[(&str, &str, &str)]) -> Answer {
        let rows: Vec<Vec<Option<&str>>> = rows
            .iter()
            .map(|(name, value, file)| vec![Some(*name), Some(*value), Some(*file)])
            .collect();
        let rows: Vec<&[Option<&str>]> = rows.iter().map(Vec::as_slice).collect();
        answer(&["name", "setting", "sourcefile"], &rows)
    }

    const CONF: &str = "/shared/services/org.postgresql.postgresql/postgres/data/postgresql.conf";
    const AUTO: &str =
        "/shared/services/org.postgresql.postgresql/postgres/data/postgresql.auto.conf";

    fn fresh() -> Settings {
        Settings::read(&files(&[]), &defaults())
    }

    #[test]
    fn a_server_that_listens_on_this_machine_only_is_set_four_ways() {
        let changes = fresh().to_allow("this-machine");
        let statements: Vec<String> = changes.iter().map(|c| c.statement().unwrap()).collect();
        assert_eq!(
            statements,
            [
                "ALTER SYSTEM SET listen_addresses = '*'",
                "ALTER SYSTEM SET ssl = 'on'",
                "ALTER SYSTEM SET ssl_cert_file = \
                 '/system/live/certificates/postgresql/this-machine/fullchain.pem'",
                "ALTER SYSTEM SET ssl_key_file = \
                 '/system/live/certificates/postgresql/this-machine/key.pem'",
            ]
        );
    }

    #[test]
    fn only_what_differs_is_set() {
        let set = Settings::read(
            &files(&[
                ("listen_addresses", "*", AUTO),
                ("ssl", "true", CONF),
                (
                    "ssl_cert_file",
                    "/system/live/certificates/postgresql/this-machine/fullchain.pem",
                    AUTO,
                ),
                ("ssl_key_file", "/system/live/certificates/postgresql/other/key.pem", AUTO),
            ]),
            &defaults(),
        );
        assert!(set.ssl_on());
        let changes = set.to_allow("this-machine");
        assert_eq!(
            changes,
            vec![Change {
                name: KEY_FILE,
                to: Some("/system/live/certificates/postgresql/this-machine/key.pem".into())
            }]
        );
        // Everything as wanted: nothing to send.
        let done = Settings::read(
            &files(&[
                ("listen_addresses", "*", AUTO),
                ("ssl", "on", AUTO),
                (
                    "ssl_cert_file",
                    "/system/live/certificates/postgresql/this-machine/fullchain.pem",
                    AUTO,
                ),
                ("ssl_key_file", "/system/live/certificates/postgresql/this-machine/key.pem", AUTO),
            ]),
            &defaults(),
        );
        assert_eq!(done.to_allow("this-machine"), vec![]);
    }

    #[test]
    fn the_last_line_that_sets_a_value_is_the_one_that_counts() {
        let set = Settings::read(
            &files(&[("listen_addresses", "*", CONF), ("listen_addresses", "localhost", AUTO)]),
            &defaults(),
        );
        assert_eq!(set.effective(LISTEN), Some("localhost"));
        assert_eq!(set.set_by_alter_system(LISTEN), Some("localhost"));
        // A line of the conf file after ALTER SYSTEM's cannot be: the
        // server reads postgresql.auto.conf last. The reading still
        // follows the order it is given.
        let set = Settings::read(
            &files(&[("listen_addresses", "a", AUTO), ("listen_addresses", "b", CONF)]),
            &defaults(),
        );
        assert_eq!((set.effective(LISTEN), set.set_by_alter_system(LISTEN)), (Some("b"), None));
    }

    #[test]
    fn deny_resets_only_what_alter_system_set() {
        let set = Settings::read(&files(&[("listen_addresses", "*", AUTO)]), &defaults());
        let changes = set.to_deny();
        assert_eq!(changes, vec![Change { name: LISTEN, to: None }]);
        assert_eq!(changes[0].statement().unwrap(), "ALTER SYSTEM RESET listen_addresses");
        // Set by hand in the conf file: there is nothing of ALTER SYSTEM's to reset.
        let hand = Settings::read(&files(&[("listen_addresses", "*", CONF)]), &defaults());
        assert_eq!(hand.to_deny(), vec![]);
        assert_eq!(fresh().to_deny(), vec![]);
    }

    #[test]
    fn a_change_is_taken_back_to_what_alter_system_had() {
        let before = Settings::read(
            &files(&[(
                "ssl_cert_file",
                "/system/live/certificates/postgresql/old/fullchain.pem",
                AUTO,
            )]),
            &defaults(),
        );
        let changes = before.to_allow("this-machine");
        let back = before.to_take_back(&changes);
        let statements: Vec<String> = back.iter().map(|c| c.statement().unwrap()).collect();
        assert_eq!(
            statements,
            [
                "ALTER SYSTEM RESET listen_addresses",
                "ALTER SYSTEM RESET ssl",
                "ALTER SYSTEM SET ssl_cert_file = \
                 '/system/live/certificates/postgresql/old/fullchain.pem'",
                "ALTER SYSTEM RESET ssl_key_file",
            ]
        );
        assert_eq!(before.to_take_back(&[]), vec![]);
    }

    #[test]
    fn the_server_restarts_when_it_must_listen_or_read_other_files() {
        let fresh = fresh();
        let allow = fresh.to_allow("this-machine");
        // The setting is in postgresql.auto.conf by now, as the files say.
        let after = Settings::read(
            &files(&[
                ("listen_addresses", "*", AUTO),
                ("ssl", "on", AUTO),
                (
                    "ssl_cert_file",
                    "/system/live/certificates/postgresql/this-machine/fullchain.pem",
                    AUTO,
                ),
            ]),
            &defaults(),
        );
        assert!(after.needs_restart(&allow, "localhost"));
        // Listening as wanted and no other file named: a reload does.
        let ssl_only = [Change { name: SSL, to: Some("on".into()) }];
        assert!(!after.needs_restart(&ssl_only, "*"));
        assert!(after.needs_restart(&ssl_only, "localhost"));
        // Another certificate's files are read as the server starts.
        let cert = [Change { name: CERT_FILE, to: Some("x".into()) }];
        assert!(after.needs_restart(&cert, "*"));
        // Taking listen_addresses away is a restart when the server still
        // listens on the network.
        assert!(fresh.needs_restart(&[], "*"));
        assert!(!fresh.needs_restart(&[], "localhost"));
    }

    #[test]
    fn the_certificate_of_a_copy_is_named_by_its_folder() {
        assert_eq!(
            certificate_of("/system/live/certificates/postgresql/this-machine/fullchain.pem"),
            Some("this-machine")
        );
        assert_eq!(certificate_of("/system/live/certificates/postgresql/a/b/fullchain.pem"), None);
        assert_eq!(certificate_of("/etc/ssl/server.crt"), None);
        assert_eq!(certificate_of("server.crt"), None);
        assert_eq!(certificate_of("/system/live/certificates/postgresql//fullchain.pem"), None);
    }

    /// PostgreSQL's service file, as far as the tests need it.
    const SERVICE: &str = "service_version = 1\n\
        description = \"PostgreSQL, the database server\"\n\
        program = \"/applications/org.postgresql.postgresql/bin/postgresql-service\"\n";

    #[test]
    fn a_new_override_names_the_certificate_and_says_what_it_is() {
        let text = override_with(SERVICE, None, "this-machine").unwrap().unwrap();
        assert_eq!(
            text,
            "# Keys database(8) keeps for PostgreSQL's service (ADR 0569). Your own keys\n\
             # here are kept; a list database changes keeps every item you added.\n\
             certificates = [\"this-machine\"]\n"
        );
        // Named already: nothing to write.
        assert_eq!(override_with(SERVICE, Some(&text), "this-machine").unwrap(), None);
    }

    #[test]
    fn an_override_keeps_every_item_and_comment_and_gains_the_certificate() {
        let over = "# mine\ncertificates = [\"mine.example\"] # for my own clients\n\n[permissions]\nfiles.read = [\"/x\"]\n";
        let text = override_with(SERVICE, Some(over), "this-machine").unwrap().unwrap();
        assert!(
            text.starts_with(
                "# mine\ncertificates = [\"mine.example\", \"this-machine\"] # for my own clients\n"
            ),
            "{text}"
        );
        assert!(text.ends_with("[permissions]\nfiles.read = [\"/x\"]\n"), "{text}");
        // An override that sets other things and no list starts the list
        // from the service file's, which it would otherwise replace.
        let with_list = format!("{SERVICE}certificates = [\"file.example\"]\n");
        let text = override_with(
            &with_list,
            Some("[permissions]\nfiles.read = [\"/x\"]\n"),
            "this-machine",
        )
        .unwrap()
        .unwrap();
        assert!(text.contains("certificates = [\"file.example\", \"this-machine\"]"), "{text}");
    }

    #[test]
    fn a_file_or_an_override_that_does_not_read_is_said() {
        assert!(
            override_with("not = toml = at all", None, "a")
                .unwrap_err()
                .starts_with("its service file")
        );
        assert!(
            override_with(SERVICE, Some("certificates = "), "a")
                .unwrap_err()
                .starts_with("its override")
        );
        assert!(override_with(SERVICE, Some("certificates = 5\n"), "a").is_err());
    }
}
