// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The accounts checks of dukovo-check(8): whether a person's account has
//! no password, and whether anyone can administer the computer. The files
//! are authd(8)'s, which root reads, as Rescue does when it asks for an
//! administrator.

use accounts::{Group, Password, Table, User};
use spark_api::words::and_list;

use super::context::Context;
use super::line::{Outcome, Problem};

/// The account files, read.
struct Files {
    users: Table<User>,
    groups: Table<Group>,
    passwords: Table<Password>,
}

impl Files {
    /// Reads the files, or says which cannot be, so that the check is
    /// skipped with that reason.
    fn read(context: &Context) -> Result<Files, String> {
        let file = |path: &str| {
            context.read(path)?.ok_or_else(|| format!("{path} is not there: no account is made"))
        };
        Ok(Files {
            users: accounts::users(&file(accounts::PASSWD)?),
            groups: accounts::groups(&file(accounts::GROUP)?),
            passwords: accounts::passwords(&file(accounts::PASSWORDS)?),
        })
    }

    /// The people's accounts: those numbered from 1000, not the system's.
    fn people(&self) -> impl Iterator<Item = &User> {
        self.users.entries().filter(|user| user.is_person())
    }

    /// Whether the account `name` is disabled, which nothing signs in to.
    fn disabled(&self, name: &str) -> bool {
        self.passwords.entries().any(|password| password.name == name && password.is_disabled())
    }
}

/// Warns of each person's account that is enabled and has an empty
/// password, which anyone who reaches a sign-in can use. An account with
/// no line in the password file cannot be signed in to, and a disabled
/// one is no risk.
pub(super) fn empty_passwords(context: &Context) -> Vec<Outcome> {
    let files = match Files::read(context) {
        Ok(files) => files,
        Err(why) => return vec![Outcome::skipped(why)],
    };
    let empty: Vec<String> = files
        .passwords
        .entries()
        .filter(|password| !password.is_disabled() && password.own_hash().is_empty())
        .filter(|password| files.people().any(|person| person.name == password.name))
        .map(|password| password.name.clone())
        .collect();
    if empty.is_empty() {
        return vec![Outcome::passed("every person's account has a password")];
    }
    let have = if empty.len() == 1 { "has" } else { "have" };
    vec![Outcome::warning(
        Problem::EmptyPassword,
        format!("{} {have} no password", and_list(&empty)),
    )]
}

/// Warns when no person's enabled account is an administrator: nothing
/// can be elevated, and Rescue asks for no password.
pub(super) fn administrators(context: &Context) -> Vec<Outcome> {
    let files = match Files::read(context) {
        Ok(files) => files,
        Err(why) => return vec![Outcome::skipped(why)],
    };
    let admins: Vec<String> = files
        .people()
        .filter(|person| accounts::is_admin(person, &files.groups) && !files.disabled(&person.name))
        .map(|person| person.name.clone())
        .collect();
    if admins.is_empty() {
        return vec![Outcome::warning(
            Problem::NoAdministrator,
            "no account is an administrator, so nothing can be elevated and Rescue asks for no \
             password",
        )];
    }
    let noun = if admins.len() == 1 { "administrator" } else { "administrators" };
    vec![Outcome::passed(format!("{} {noun}: {}", admins.len(), and_list(&admins)))]
}

#[cfg(test)]
mod tests {
    use super::super::context::planted::System;
    use super::super::line::Verdict;
    use super::*;

    /// A system with alice, bob and a service, in the groups and with the
    /// password lines the arguments say.
    fn system(groups: &str, passwords: &str) -> System {
        let system = System::new(Vec::new());
        system.plant(
            accounts::PASSWD,
            "root:x:0:0:root:/root:/system/commands/nukesh\n\
             alice:x:1000:1000:Alice:/users/alice:/system/commands/nukesh\n\
             bob:x:1001:1001:Bob:/users/bob:/system/commands/nukesh\n\
             web:x:100:100:Web:/system/state/web:/system/commands/false\n",
        );
        system.plant(accounts::GROUP, groups);
        system.plant(accounts::PASSWORDS, passwords);
        system
    }

    const GROUPS: &str = "admins:x:10:alice\nalice:x:1000:\nbob:x:1001:\n";
    const HASH: &str = "$argon2id$v=19$m=19456,t=2,p=1$c2FsdHNhbHQ$aGFzaGhhc2g";

    #[test]
    fn every_person_with_a_password_passes() {
        let system = system(GROUPS, &format!("alice:{HASH}:\nbob:{HASH}:\nweb::\n"));
        let outcome = &empty_passwords(&system.context)[0];
        assert_eq!(outcome.verdict, Verdict::Passed);
        assert_eq!(outcome.detail, "every person's account has a password");
    }

    #[test]
    fn a_person_with_an_empty_password_warns_by_name_and_a_service_does_not() {
        let system = system(GROUPS, &format!("alice:{HASH}:\nbob::\nweb::\n"));
        let outcome = &empty_passwords(&system.context)[0];
        assert_eq!(outcome.verdict, Verdict::Warning);
        assert_eq!(outcome.id.as_deref(), Some("SEC-120"));
        assert_eq!(outcome.detail, "bob has no password");
        let both = self::system(GROUPS, "alice::\nbob::\n");
        assert_eq!(empty_passwords(&both.context)[0].detail, "alice and bob have no password");
    }

    #[test]
    fn a_disabled_account_with_an_empty_password_is_no_risk() {
        let system = system(GROUPS, &format!("alice:{HASH}:\nbob:!:\n"));
        assert_eq!(empty_passwords(&system.context)[0].verdict, Verdict::Passed);
    }

    #[test]
    fn a_person_with_no_password_line_cannot_sign_in_and_is_not_empty() {
        let system = system(GROUPS, &format!("alice:{HASH}:\n"));
        assert_eq!(empty_passwords(&system.context)[0].verdict, Verdict::Passed);
    }

    #[test]
    fn password_files_that_cannot_be_read_skip_the_checks_with_the_reason() {
        let none = System::new(Vec::new());
        let outcome = &empty_passwords(&none.context)[0];
        assert_eq!(outcome.verdict, Verdict::Skipped);
        assert_eq!(outcome.detail, "/system/settings/passwd is not there: no account is made");
        assert_eq!(administrators(&none.context)[0].verdict, Verdict::Skipped);
        let folder = System::new(Vec::new());
        std::fs::create_dir_all(folder.context.root.join("system/state/authd/passwords")).unwrap();
        folder.plant(accounts::PASSWD, "alice:x:1000:1000:Alice:/users/alice:/x\n");
        folder.plant(accounts::GROUP, GROUPS);
        let outcome = &empty_passwords(&folder.context)[0];
        assert!(outcome.detail.starts_with("cannot read /system/state/authd/passwords: "));
    }

    #[test]
    fn an_administrator_is_a_person_in_admins() {
        let system = system(GROUPS, &format!("alice:{HASH}:\nbob:{HASH}:\n"));
        let outcome = &administrators(&system.context)[0];
        assert_eq!(outcome.verdict, Verdict::Passed);
        assert_eq!(outcome.detail, "1 administrator: alice");
        let two = self::system("admins:x:10:alice,bob\n", &format!("alice:{HASH}:\nbob:{HASH}:\n"));
        assert_eq!(administrators(&two.context)[0].detail, "2 administrators: alice and bob");
    }

    #[test]
    fn without_a_person_in_admins_nothing_can_be_elevated() {
        // Root is an administrator by its number, but is no person's account.
        let system = system("admins:x:10:\n", &format!("alice:{HASH}:\n"));
        let outcome = &administrators(&system.context)[0];
        assert_eq!((outcome.verdict, outcome.id.as_deref()), (Verdict::Warning, Some("SEC-121")));
        let nobody = self::system("admins:x:10:ghost\n", &format!("alice:{HASH}:\n"));
        assert_eq!(administrators(&nobody.context)[0].verdict, Verdict::Warning);
    }

    #[test]
    fn the_only_administrator_being_disabled_is_no_administrator() {
        let system = system(GROUPS, &format!("alice:!{HASH}:\nbob:{HASH}:\n"));
        assert_eq!(administrators(&system.context)[0].verdict, Verdict::Warning);
    }
}
