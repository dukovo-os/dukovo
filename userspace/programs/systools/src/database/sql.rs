// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The statements `database` sends PostgreSQL, each made here and nowhere
//! else, so that a test holds every one as a string.
//!
//! Every name goes in as an identifier ([`wire::ident`]) and the texts
//! that are not names, a verifier, a locale and a comment, as strings
//! ([`wire::literal`]). The statements that change anything are sent one
//! at a time: `CREATE DATABASE` cannot run in a transaction, and a failed
//! statement then leaves the ones before it as they were.
//!
//! Nothing here takes a password. The only secret a statement holds is
//! the SCRAM verifier, which PostgreSQL would have made of the password
//! itself.

use super::wire::{ident, literal};

/// What a role's comment says when `database` made it: followed by the
/// name of the database it owns.
pub const ROLE_COMMENT: &str = "database(8): owner of ";

/// What a database's comment says when `database` made it for a website:
/// followed by the website's name.
pub const SITE_COMMENT: &str = "database(8): site ";

/// What every comment `database` writes starts with.
pub const COMMENT: &str = "database(8): ";

/// `SELECT pg_reload_conf()`: reads the configuration files again.
pub const RELOAD: &str = "SELECT pg_catalog.pg_reload_conf()";

/// When the server last read its configuration files, to see that a
/// reload happened.
pub const LOADED_AT: &str = "SELECT pg_catalog.pg_conf_load_time()::text";

/// Whether the `pg_hba.conf` the server would read has a line it cannot.
pub const HBA_ERRORS: &str =
    "SELECT line_number, error FROM pg_hba_file_rules WHERE error IS NOT NULL";

/// Whether a setting of a configuration file has a mistake in it.
pub const SETTINGS_ERRORS: &str =
    "SELECT sourcefile, sourceline, name, error FROM pg_file_settings WHERE error IS NOT NULL";

/// The role's `CREATE ROLE`: it signs in with the password the
/// `verifier` is of, or, with none, by the socket as the account of the
/// same name.
///
/// # Errors
///
/// When a text holds a NUL.
pub fn create_role(role: &str, verifier: Option<&str>) -> Result<String, String> {
    let role = ident(role)?;
    Ok(match verifier {
        Some(verifier) => format!("CREATE ROLE {role} LOGIN PASSWORD {}", literal(verifier)?),
        None => format!("CREATE ROLE {role} LOGIN"),
    })
}

/// The comment that says `database` made `role` to own `database`.
///
/// # Errors
///
/// When a name holds a NUL.
pub fn comment_on_role(role: &str, database: &str) -> Result<String, String> {
    let comment = literal(&format!("{ROLE_COMMENT}{database}"))?;
    Ok(format!("COMMENT ON ROLE {} IS {comment}", ident(role)?))
}

/// `CREATE DATABASE` for `database`, owned by `owner`, from `template0`
/// in UTF-8, sorted as `locale` says: the C library's for a language, the
/// builtin provider's for `C.UTF-8`.
///
/// # Errors
///
/// When a text holds a NUL.
pub fn create_database(database: &str, owner: &str, locale: &str) -> Result<String, String> {
    let provider = if locale == "C.UTF-8" {
        format!("LOCALE_PROVIDER builtin BUILTIN_LOCALE {}", literal(locale)?)
    } else {
        format!("LOCALE_PROVIDER libc LOCALE {}", literal(locale)?)
    };
    Ok(format!(
        "CREATE DATABASE {} OWNER {} TEMPLATE template0 ENCODING 'UTF8' {provider}",
        ident(database)?,
        ident(owner)?
    ))
}

/// The comment that says `database` was made for the website `site`.
///
/// # Errors
///
/// When a text holds a NUL.
pub fn comment_on_database(database: &str, site: &str) -> Result<String, String> {
    let comment = literal(&format!("{SITE_COMMENT}{site}"))?;
    Ok(format!("COMMENT ON DATABASE {} IS {comment}", ident(database)?))
}

/// Takes the right to connect from everyone, so that only the database's
/// owner and the superuser connect to it.
///
/// # Errors
///
/// When the name holds a NUL.
pub fn revoke_connect(database: &str) -> Result<String, String> {
    Ok(format!("REVOKE ALL ON DATABASE {} FROM PUBLIC", ident(database)?))
}

/// `DROP DATABASE`, which with `force` ends the sessions that are open.
///
/// # Errors
///
/// When the name holds a NUL.
pub fn drop_database(database: &str, force: bool) -> Result<String, String> {
    let database = ident(database)?;
    Ok(if force {
        format!("DROP DATABASE {database} WITH (FORCE)")
    } else {
        format!("DROP DATABASE {database}")
    })
}

/// `DROP ROLE` for a role that owns nothing any more.
///
/// # Errors
///
/// When the name holds a NUL.
pub fn drop_role(role: &str) -> Result<String, String> {
    Ok(format!("DROP ROLE {}", ident(role)?))
}

/// A new password for `role`, as the `verifier` of it.
///
/// # Errors
///
/// When a text holds a NUL.
pub fn alter_role_password(role: &str, verifier: &str) -> Result<String, String> {
    Ok(format!("ALTER ROLE {} PASSWORD {}", ident(role)?, literal(verifier)?))
}

/// `ALTER SYSTEM SET`, which writes `postgresql.auto.conf`. The name is
/// one of [`settings`](super::settings)' own.
///
/// # Errors
///
/// When the value holds a NUL.
pub fn alter_system_set(name: &str, value: &str) -> Result<String, String> {
    Ok(format!("ALTER SYSTEM SET {name} = {}", literal(value)?))
}

/// `ALTER SYSTEM RESET`, which takes the setting out of
/// `postgresql.auto.conf` again.
#[must_use]
pub fn alter_system_reset(name: &str) -> String {
    format!("ALTER SYSTEM RESET {name}")
}

/// The database `name`, its owner, and whether the owner has a password:
/// no row when there is no such database.
///
/// # Errors
///
/// When the name holds a NUL.
pub fn the_database(name: &str) -> Result<String, String> {
    Ok(format!(
        "SELECT pg_catalog.pg_get_userbyid(d.datdba) AS owner, \
         r.rolpassword IS NOT NULL AS password, \
         pg_catalog.shobj_description(r.oid, 'pg_authid') AS comment \
         FROM pg_catalog.pg_database d JOIN pg_catalog.pg_authid r ON r.oid = d.datdba \
         WHERE d.datname = {}",
        literal(name)?
    ))
}

/// The sessions other than this one that are open to `database`: the
/// program each is, and how many.
///
/// # Errors
///
/// When the name holds a NUL.
pub fn sessions_of(database: &str) -> Result<String, String> {
    Ok(format!(
        "SELECT application_name, count(*) AS sessions FROM pg_catalog.pg_stat_activity \
         WHERE datname = {} AND pid <> pg_catalog.pg_backend_pid() \
         GROUP BY application_name ORDER BY application_name",
        literal(database)?
    ))
}

/// Whether a database called `name` is there.
///
/// # Errors
///
/// When the name holds a NUL.
pub fn database_exists(name: &str) -> Result<String, String> {
    Ok(format!("SELECT 1 FROM pg_catalog.pg_database WHERE datname = {}", literal(name)?))
}

/// Whether a role called `name` is there.
///
/// # Errors
///
/// When the name holds a NUL.
pub fn role_exists(name: &str) -> Result<String, String> {
    Ok(format!("SELECT 1 FROM pg_catalog.pg_roles WHERE rolname = {}", literal(name)?))
}

/// The databases that `list` shows: not `postgres` and the templates.
pub const LIST_DATABASES: &str = "SELECT d.datname AS name, \
     pg_catalog.pg_get_userbyid(d.datdba) AS owner, \
     pg_catalog.pg_encoding_to_char(d.encoding) AS encoding, \
     d.datlocprovider AS provider, \
     CASE d.datlocprovider WHEN 'c' THEN d.datcollate ELSE d.datlocale END AS locale, \
     pg_catalog.pg_database_size(d.oid) AS bytes, \
     pg_catalog.shobj_description(d.oid, 'pg_database') AS comment, \
     (SELECT pg_catalog.shobj_description(r.oid, 'pg_authid') FROM pg_catalog.pg_roles r \
      WHERE r.oid = d.datdba) AS owner_comment, \
     d.datcollversion AS recorded, \
     pg_catalog.pg_database_collation_actual_version(d.oid) AS actual \
     FROM pg_catalog.pg_database d \
     WHERE d.datname NOT IN ('postgres', 'template0', 'template1') \
     ORDER BY d.datname";

/// How many sessions each database has, this one not counted.
pub const LIST_SESSIONS: &str = "SELECT datname AS name, count(*) AS sessions \
     FROM pg_catalog.pg_stat_activity \
     WHERE datname IS NOT NULL AND pid <> pg_catalog.pg_backend_pid() GROUP BY datname";

/// The roles that `list` shows: not PostgreSQL's own `pg_` roles. The
/// verifier is never selected, only whether there is one.
pub const LIST_ROLES: &str = "SELECT rolname AS name, rolcanlogin AS login, \
     rolpassword IS NOT NULL AS password, \
     pg_catalog.shobj_description(oid, 'pg_authid') AS comment \
     FROM pg_catalog.pg_authid WHERE rolname !~ '^pg_' ORDER BY rolname";

/// The lines of `pg_hba.conf` the server read, and the mistakes in it,
/// with the people and databases of a line as JSON arrays.
pub const LIST_ACCESS: &str = "SELECT line_number, type, \
     pg_catalog.to_json(database)::text AS databases, \
     pg_catalog.to_json(user_name)::text AS users, \
     address, netmask, auth_method, error \
     FROM pg_catalog.pg_hba_file_rules ORDER BY line_number";

/// What each setting `allow` and `deny` change is: set in a file, where,
/// and as what.
pub const FILE_SETTINGS: &str = "SELECT name, setting, sourcefile FROM pg_catalog.pg_file_settings \
     WHERE name IN ('listen_addresses', 'ssl', 'ssl_cert_file', 'ssl_key_file') \
     AND error IS NULL ORDER BY seqno";

/// What each setting `allow` and `deny` change is when no file sets it,
/// and what it is now.
pub const SETTING_NOW: &str = "SELECT name, setting, boot_val FROM pg_catalog.pg_settings \
     WHERE name IN ('listen_addresses', 'ssl', 'ssl_cert_file', 'ssl_key_file')";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_statements_of_an_add_are_these() {
        let verifier = "SCRAM-SHA-256$4096:c2FsdA==$c3RvcmVk:c2VydmVy";
        assert_eq!(
            create_role("shop", Some(verifier)).unwrap(),
            "CREATE ROLE \"shop\" LOGIN PASSWORD 'SCRAM-SHA-256$4096:c2FsdA==$c3RvcmVk:c2VydmVy'"
        );
        assert_eq!(create_role("webdev", None).unwrap(), "CREATE ROLE \"webdev\" LOGIN");
        assert_eq!(
            comment_on_role("shop", "shop").unwrap(),
            "COMMENT ON ROLE \"shop\" IS 'database(8): owner of shop'"
        );
        assert_eq!(
            create_database("shop", "shop", "en_US.UTF-8").unwrap(),
            "CREATE DATABASE \"shop\" OWNER \"shop\" TEMPLATE template0 ENCODING 'UTF8' \
             LOCALE_PROVIDER libc LOCALE 'en_US.UTF-8'"
        );
        assert_eq!(
            create_database("shop", "shop", "C.UTF-8").unwrap(),
            "CREATE DATABASE \"shop\" OWNER \"shop\" TEMPLATE template0 ENCODING 'UTF8' \
             LOCALE_PROVIDER builtin BUILTIN_LOCALE 'C.UTF-8'"
        );
        assert_eq!(
            comment_on_database("shop", "shop.example.org").unwrap(),
            "COMMENT ON DATABASE \"shop\" IS 'database(8): site shop.example.org'"
        );
        assert_eq!(revoke_connect("shop").unwrap(), "REVOKE ALL ON DATABASE \"shop\" FROM PUBLIC");
    }

    #[test]
    fn the_statements_of_a_password_and_a_remove_are_these() {
        assert_eq!(
            alter_role_password("shop", "SCRAM-SHA-256$4096:a$b:c").unwrap(),
            "ALTER ROLE \"shop\" PASSWORD 'SCRAM-SHA-256$4096:a$b:c'"
        );
        assert_eq!(drop_database("shop", false).unwrap(), "DROP DATABASE \"shop\"");
        assert_eq!(drop_database("shop", true).unwrap(), "DROP DATABASE \"shop\" WITH (FORCE)");
        assert_eq!(drop_role("shop").unwrap(), "DROP ROLE \"shop\"");
    }

    #[test]
    fn the_checks_are_the_two_selects_that_read_the_files_as_they_are_on_disk() {
        assert_eq!(
            HBA_ERRORS,
            "SELECT line_number, error FROM pg_hba_file_rules WHERE error IS NOT NULL"
        );
        assert_eq!(
            SETTINGS_ERRORS,
            "SELECT sourcefile, sourceline, name, error FROM pg_file_settings \
             WHERE error IS NOT NULL"
        );
    }

    #[test]
    fn the_settings_of_a_network_are_set_and_reset_by_alter_system() {
        assert_eq!(
            alter_system_set("listen_addresses", "*").unwrap(),
            "ALTER SYSTEM SET listen_addresses = '*'"
        );
        assert_eq!(alter_system_set("ssl", "on").unwrap(), "ALTER SYSTEM SET ssl = 'on'");
        assert_eq!(alter_system_reset("listen_addresses"), "ALTER SYSTEM RESET listen_addresses");
    }

    #[test]
    fn a_name_is_quoted_as_a_name_and_a_text_as_a_text() {
        // The rule keeps these out long before, and the quoting would
        // still make each one a single name or a single string.
        assert_eq!(
            create_role("a\"b", Some("it's")).unwrap(),
            "CREATE ROLE \"a\"\"b\" LOGIN PASSWORD 'it''s'"
        );
        assert_eq!(
            comment_on_database("a\"; DROP DATABASE x; --", "it's").unwrap(),
            "COMMENT ON DATABASE \"a\"\"; DROP DATABASE x; --\" IS 'database(8): site it''s'"
        );
        assert_eq!(
            drop_database("x\" WITH (FORCE); --", false).unwrap(),
            "DROP DATABASE \"x\"\" WITH (FORCE); --\""
        );
        assert!(the_database("o'x").unwrap().ends_with("WHERE d.datname = 'o''x'"));
    }

    #[test]
    fn a_nul_is_refused_in_every_statement() {
        assert!(create_role("a\0b", None).is_err());
        assert!(create_role("a", Some("v\0")).is_err());
        assert!(comment_on_role("a", "b\0").is_err());
        assert!(create_database("a", "b\0", "C.UTF-8").is_err());
        assert!(comment_on_database("a", "b\0").is_err());
        assert!(revoke_connect("\0").is_err());
        assert!(drop_database("\0", true).is_err());
        assert!(drop_role("\0").is_err());
        assert!(alter_role_password("a", "\0").is_err());
        assert!(alter_system_set("ssl", "\0").is_err());
        assert!(the_database("\0").is_err());
        assert!(sessions_of("\0").is_err());
        assert!(database_exists("\0").is_err());
        assert!(role_exists("\0").is_err());
    }

    #[test]
    fn the_comments_database_writes_start_the_same_way() {
        assert!(ROLE_COMMENT.starts_with(COMMENT));
        assert!(SITE_COMMENT.starts_with(COMMENT));
    }

    #[test]
    fn the_queries_that_list_never_select_a_verifier() {
        for query in [LIST_DATABASES, LIST_ROLES, LIST_ACCESS, FILE_SETTINGS] {
            assert!(!query.contains("rolpassword,"), "{query}");
            assert!(!query.to_lowercase().contains("select rolpassword"), "{query}");
        }
        // The only mention is whether there is one.
        assert_eq!(LIST_ROLES.matches("rolpassword").count(), 1);
        assert!(LIST_ROLES.contains("rolpassword IS NOT NULL"));
    }
}
