// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The refusals of database(8) that have an ID of their own, DB-101 to
//! DB-108, each as the failure the command exits with. The ID ends the
//! last line in parentheses, as the other tools' IDs do (`ACC-105`): the
//! web console reads it from the last line of the command's error output
//! when its action names it, and the support center's page for it is found
//! by it.
//!
//! The words do not start with the command's name: `main` puts it before
//! the message. They never hold a password, nor the verifier of one.

use super::server::PORT;
use super::wire::{self, PgError};
use crate::firewall::SETTINGS;
use crate::util::{Fail, fail};

/// DB-101: PostgreSQL is not installed.
pub fn not_installed() -> Fail {
    fail(
        "PostgreSQL is not installed: add it from the apps first (the bundle \
         org.postgresql.postgresql) (DB-101)"
            .into(),
    )
}

/// DB-101: PostgreSQL's service is installed and does not run. `enabled`
/// says whether it is turned on.
pub fn not_running(enabled: bool) -> Fail {
    let what = if enabled {
        "PostgreSQL is not running: `service postgresql status` says why"
    } else {
        "PostgreSQL is off: turn it on with `service postgresql enable`"
    };
    fail(format!("{what} (DB-101)"))
}

/// DB-101: PostgreSQL did not take the sign-in, or the socket did not
/// answer; `why` says which.
pub fn not_let_in(why: &wire::Error) -> Fail {
    fail(format!("{why} (DB-101)"))
}

/// DB-102: a database or a role called `name` is there already. `what`
/// says which: `database` or `role`.
pub fn taken(what: &str, name: &str) -> Fail {
    fail(format!(
        "there is a {what} called {name} already: choose another name, or remove it first (DB-102)"
    ))
}

/// DB-103: `database` has open sessions. `programs` are the
/// `application_name` of each program and how many sessions it has.
pub fn sessions(database: &str, programs: &[(String, u64)]) -> Fail {
    let count: u64 = programs.iter().map(|(_, count)| count).sum();
    let who: Vec<String> = programs
        .iter()
        .map(|(name, count)| {
            let name = if name.is_empty() { "a program that gave no name" } else { name };
            if *count > 1 { format!("{name} ({count})") } else { name.to_string() }
        })
        .collect();
    let sessions =
        if count == 1 { "1 open session".to_string() } else { format!("{count} open sessions") };
    fail(format!(
        "{database} has {sessions}, from {}: stop those programs, or use --force to end them \
         (DB-103)",
        who.join(", ")
    ))
}

/// DB-104: PostgreSQL's own check of its files found `words`, or a file
/// of the change did not read, so the change was not made. `back` says
/// what could not be put back, in words that follow, and is empty when
/// everything was.
pub fn not_passed(words: &str, back: &str) -> Fail {
    if back.is_empty() {
        fail(format!("{words}: nothing was changed (DB-104)"))
    } else {
        fail(format!("{words}{back} (DB-104)"))
    }
}

/// DB-104: the change was made and the server then did not answer as it
/// should, so it was taken back; `why` says what was wrong, and `back`
/// how taking it back went.
pub fn taken_back(why: &str, back: &str) -> Fail {
    fail(format!("{why}; the change was taken back{back} (DB-104)"))
}

/// DB-105: certd has no certificate called `name`.
pub fn no_certificate(name: &str) -> Fail {
    fail(format!("there is no certificate called {name}; `cert list` shows them (DB-105)"))
}

/// DB-106: the server has no account called `name`.
pub fn no_account(name: &str) -> Fail {
    fail(format!("there is no account called {name}; `user list` shows the accounts (DB-106)"))
}

/// DB-107: the firewall's rules are being tried, so the block for the
/// port could not be written.
pub fn firewall_is_tried() -> Fail {
    fail(format!(
        "the firewall's rules are being tried, and a block for port {PORT} written now would be \
         lost when the trial ends: keep or undo that change first, with `elevate firewall keep` \
         or `elevate firewall undo`; nothing was changed (DB-107)"
    ))
}

/// DB-108: the firewall's rules cannot take the block for the port; `why`
/// says what is wrong with the file, as the code that reads the block
/// does.
pub fn firewall_block(why: &str) -> Fail {
    fail(format!(
        "the firewall's rules in {SETTINGS} cannot take the block for port {PORT}, and nothing \
         was changed: {why} (DB-108)"
    ))
}

/// The failure for what a session said after it started: PostgreSQL's own
/// words, which hold no password, and no ID.
pub fn said(error: &wire::Error) -> Fail {
    match error {
        wire::Error::NotLetIn(_) => not_let_in(error),
        other => fail(other.to_string()),
    }
}

/// How PostgreSQL's mistakes in its files read: its words and the line
/// of the file where it has them.
pub fn problem_words(file: &str, line: Option<&str>, name: Option<&str>, error: &str) -> String {
    let at = match (line, name) {
        (Some(line), Some(name)) => format!("{file} line {line} ({name})"),
        (Some(line), None) => format!("{file} line {line}"),
        (None, _) => file.to_string(),
    };
    format!("{at}: {error}")
}

/// Whether PostgreSQL said that `database` is being used, which is how
/// `DROP DATABASE` refuses while a session is open.
#[must_use]
pub fn is_in_use(error: &PgError) -> bool {
    error.code == "55006"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_refusal_ends_with_its_id_and_exits_with_status_1() {
        let refusals = [
            (not_installed(), "DB-101"),
            (not_running(true), "DB-101"),
            (not_running(false), "DB-101"),
            (not_let_in(&wire::Error::NotLetIn(3)), "DB-101"),
            (taken("database", "shop"), "DB-102"),
            (sessions("shop", &[("php".into(), 2), ("psql".into(), 1)]), "DB-103"),
            (not_passed("pg_hba.conf line 12: invalid address", ""), "DB-104"),
            (not_passed("a file", ", but it could not be put back"), "DB-104"),
            (taken_back("it did not answer", ", and PostgreSQL runs again"), "DB-104"),
            (no_certificate("nas"), "DB-105"),
            (no_account("ann"), "DB-106"),
            (firewall_is_tried(), "DB-107"),
            (firewall_block("line 3 starts a block that no line ends"), "DB-108"),
        ];
        for (refusal, id) in refusals {
            assert!(refusal.message.ends_with(&format!(" ({id})")), "{}", refusal.message);
            assert_eq!(refusal.message.lines().count(), 1, "{id}");
            assert_eq!((refusal.code, refusal.usage), (1, false), "{id}");
        }
    }

    #[test]
    fn the_sessions_are_counted_and_the_programs_named() {
        let words =
            sessions("shop", &[("php".into(), 2), ("psql".into(), 1), (String::new(), 1)]).message;
        assert_eq!(
            words,
            "shop has 4 open sessions, from php (2), psql, a program that gave no name: stop \
             those programs, or use --force to end them (DB-103)"
        );
        assert!(
            sessions("shop", &[("psql".into(), 1)]).message.contains("shop has 1 open session,")
        );
    }

    #[test]
    fn a_session_that_started_and_then_failed_has_no_id_unless_it_was_not_let_in() {
        let broken = said(&wire::Error::Broken("it closed the connection".into()));
        assert!(!broken.message.contains("DB-1"), "{}", broken.message);
        let server = said(&wire::Error::Server(Box::new(PgError {
            severity: "ERROR".into(),
            code: "42501".into(),
            message: "permission denied".into(),
            ..PgError::default()
        })));
        assert_eq!(server.message, "PostgreSQL said: permission denied (42501)");
        assert!(said(&wire::Error::NotLetIn(3)).message.ends_with("(DB-101)"));
    }

    #[test]
    fn postgresql_s_mistakes_read_as_a_file_and_a_line() {
        assert_eq!(
            problem_words("pg_hba.conf", Some("12"), None, "invalid IP address \"x\""),
            "pg_hba.conf line 12: invalid IP address \"x\""
        );
        assert_eq!(
            problem_words("postgresql.auto.conf", Some("3"), Some("ssl"), "bad value"),
            "postgresql.auto.conf line 3 (ssl): bad value"
        );
        assert_eq!(problem_words("pg_hba.conf", None, None, "gone"), "pg_hba.conf: gone");
    }

    #[test]
    fn a_drop_that_was_refused_for_a_session_is_told() {
        let error = PgError { code: "55006".into(), ..PgError::default() };
        assert!(is_in_use(&error));
        assert!(!is_in_use(&PgError { code: "42P04".into(), ..PgError::default() }));
    }
}
