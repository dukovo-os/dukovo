// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Whose account a console session is, from the account files (ADR 0514).
//! The worker says it for the session it serves, by its own user number,
//! and admind finds out for itself, by the name authd signed in: what a
//! person is, an administrator or not, is never taken from a worker's
//! word, which any other program running as the same person could give.

use serde_json::{Value, json};

/// The person's account, as the files say it now.
#[derive(Debug, PartialEq, Eq)]
pub struct Who {
    /// The name they sign in with.
    pub name: String,
    /// Their full name, which may be empty.
    pub full_name: String,
    /// Whether they are an administrator.
    pub admin: bool,
    /// The groups they are in, their own first.
    pub groups: Vec<String>,
    /// Whether they have a shell a terminal can start: one is named, and
    /// it is not the one that refuses every sign-in.
    pub shell: bool,
}

/// Which account to find.
#[derive(Clone, Copy)]
pub enum Account<'a> {
    /// The account with this user number.
    Number(u32),
    /// The account with this name.
    Name(&'a str),
}

impl Who {
    /// What the worker answers to `whoami`.
    pub fn json(&self) -> Value {
        json!({
            "name": self.name,
            "full_name": self.full_name,
            "admin": self.admin,
            "groups": self.groups,
        })
    }
}

/// The account `account` in the texts of the files `passwd` and `group`.
///
/// # Errors
///
/// In plain words, if no account is the one asked for.
pub fn find(passwd: &str, group: &str, account: Account) -> Result<Who, String> {
    let users = accounts::users(passwd);
    let groups = accounts::groups(group);
    let found = users.entries().find(|user| match account {
        Account::Number(uid) => user.uid == uid,
        Account::Name(name) => user.name == name,
    });
    let Some(user) = found else {
        return Err(match account {
            Account::Number(uid) => format!("no account has the number {uid}"),
            Account::Name(name) => format!("there is no account {name}"),
        });
    };
    let theirs = groups
        .entries()
        .filter(|g| g.gid == user.gid || g.members.contains(&user.name))
        .map(|g| g.name.clone())
        .collect();
    // The fifth field is a GECOS one: the full name, then the room and the
    // phone, after commas.
    let full_name = user.full_name.split(',').next().unwrap_or_default().trim();
    Ok(Who {
        name: user.name.clone(),
        full_name: full_name.to_string(),
        admin: accounts::is_admin(user, &groups),
        groups: theirs,
        shell: !user.shell.is_empty() && user.shell != accounts::NO_SHELL,
    })
}

/// Where the account files are.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Sources {
    /// The file of the accounts.
    pub passwd: String,
    /// The file of the groups.
    pub group: String,
}

impl Default for Sources {
    fn default() -> Sources {
        Sources { passwd: accounts::PASSWD.into(), group: accounts::GROUP.into() }
    }
}

/// The account `account`, read from the files `sources` names.
///
/// # Errors
///
/// In plain words, if a file cannot be read or no account is the one asked
/// for.
pub fn read(sources: &Sources, account: Account) -> Result<Who, String> {
    let text = |path: &str| {
        std::fs::read_to_string(path)
            .map_err(|err| format!("cannot read {path}: {}", crate::util::strip_os_error(&err)))
    };
    find(&text(&sources.passwd)?, &text(&sources.group)?, account)
}

#[cfg(test)]
mod tests {
    use super::*;

    const PASSWD: &str = "root:x:0:0:root:/system/root:/system/commands/nukesh\n\
                          chase:x:1000:1000:Chase Y,Room 4,555:/users/chase:/system/commands/nukesh\n\
                          bob:x:1001:1001::/users/bob:/system/commands/nukesh\n";
    const GROUP: &str =
        "root:x:0:\nchase:x:1000:\nbob:x:1001:\nadmins:x:10:chase\nconsole:x:12:bob\n";

    #[test]
    fn an_account_is_found_by_its_number_or_its_name_with_its_groups() {
        let chase = find(PASSWD, GROUP, Account::Number(1000)).unwrap();
        assert_eq!(chase, find(PASSWD, GROUP, Account::Name("chase")).unwrap());
        assert_eq!(chase.name, "chase");
        assert_eq!(chase.full_name, "Chase Y");
        assert!(chase.admin);
        assert_eq!(chase.groups, ["chase", "admins"]);
        let bob = find(PASSWD, GROUP, Account::Name("bob")).unwrap();
        assert!(!bob.admin);
        assert_eq!(bob.full_name, "");
        assert_eq!(bob.groups, ["bob", "console"]);
        assert!(chase.shell && bob.shell);
        let nobody = "svc:x:900:900::/:/system/commands/false\nnone:x:901:901::/:\n";
        assert!(!find(nobody, GROUP, Account::Name("svc")).unwrap().shell);
        assert!(!find(nobody, GROUP, Account::Name("none")).unwrap().shell);
        assert_eq!(
            chase.json(),
            json!({"name": "chase", "full_name": "Chase Y", "admin": true, "groups": ["chase", "admins"]})
        );
    }

    #[test]
    fn an_account_that_is_not_there_is_said_in_words() {
        assert_eq!(
            find(PASSWD, GROUP, Account::Number(4242)),
            Err("no account has the number 4242".to_string())
        );
        assert_eq!(
            find(PASSWD, GROUP, Account::Name("mallory")),
            Err("there is no account mallory".to_string())
        );
        // A name that only looks like an account's is not it.
        assert!(find(PASSWD, GROUP, Account::Name("Chase")).is_err());
        assert!(find(PASSWD, GROUP, Account::Name("chase ")).is_err());
    }

    #[test]
    fn the_files_are_read_from_where_the_sources_say() {
        let folder = std::env::temp_dir().join(format!("admind-who-{}", std::process::id()));
        std::fs::create_dir_all(&folder).unwrap();
        let sources = Sources {
            passwd: folder.join("passwd").to_string_lossy().into_owned(),
            group: folder.join("group").to_string_lossy().into_owned(),
        };
        let why = read(&sources, Account::Name("chase")).unwrap_err();
        assert!(why.starts_with("cannot read "), "{why}");
        std::fs::write(&sources.passwd, PASSWD).unwrap();
        std::fs::write(&sources.group, GROUP).unwrap();
        assert_eq!(read(&sources, Account::Name("chase")).unwrap().full_name, "Chase Y");
        std::fs::remove_dir_all(&folder).unwrap();
        assert_eq!(Sources::default().passwd, "/system/settings/passwd");
    }

    #[test]
    fn root_is_an_administrator_by_its_number() {
        assert!(find(PASSWD, GROUP, Account::Number(0)).unwrap().admin);
    }
}
