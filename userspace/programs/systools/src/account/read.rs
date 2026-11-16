// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! `user list --json` and `user show [NAME]`: what the console's Accounts
//! and Your account pages read, and what a person at a terminal reads to
//! see an account whole.
//!
//! A field the machine does not give is null, never a value made up for
//! it: whether an account is disabled and whether it has a password are
//! root's to read, so they are null for anyone else; the second steps and
//! tokens are null where authd does not answer; the keys where the file
//! cannot be read safely; the sessions where `/processes` cannot be read;
//! and when the password last changed, which the account files do not
//! keep, is always null. A session's address is null where the sign-in was
//! at the machine itself, or no record of it is kept.

use std::fmt::Write as _;
use std::fs;

use accounts::console::Console;
use accounts::request::{Answer, Request};
use accounts::signins;
use accounts::{Password, Table, User};
use serde::Serialize;

use super::session::{self, Found};
use super::steps_json::{self, Steps, Token};
use super::{Accounts, ask, own_name};
use crate::args::{Arg, Args};
use crate::sshkeys;
use crate::util::{self, Fail, fail};
use crate::{last, proc, sessions};

/// An account as `user list` and `user show` give it.
#[derive(Debug, PartialEq, Eq, Serialize)]
struct Row {
    name: String,
    uid: u32,
    full_name: String,
    home: String,
    shell: String,
    /// Whether it is in the administrators' group, or is root.
    admin: bool,
    /// Whether it may sign in to the console.
    console: bool,
    /// Every group it is in, by name, its own too.
    groups: Vec<String>,
    /// Whether it is disabled, and whether it has a password (`set` or
    /// `none`): in the password file, which only root reads.
    disabled: Option<bool>,
    password: Option<&'static str>,
}

/// `user`'s row. `passwords` is the password file, if the caller may read
/// it.
fn row(user: &User, accounts: &Accounts, passwords: Option<&Table<Password>>) -> Row {
    let mut groups: Vec<String> = accounts
        .groups
        .entries()
        .filter(|g| g.gid == user.gid || g.members.contains(&user.name))
        .map(|g| g.name.clone())
        .collect();
    groups.sort();
    groups.dedup();
    fn of<'a>(all: &'a Table<Password>, name: &str) -> Option<&'a Password> {
        all.entries().find(|p| p.name == name)
    }
    Row {
        name: user.name.clone(),
        uid: user.uid,
        full_name: user.full_name.clone(),
        home: user.home.clone(),
        shell: user.shell.clone(),
        admin: accounts::is_admin(user, &accounts.groups),
        console: accounts::may_use_console(user, &accounts.groups),
        groups,
        disabled: passwords.map(|all| of(all, &user.name).is_some_and(Password::is_disabled)),
        password: passwords.map(|all| match of(all, &user.name) {
            Some(p) if !p.own_hash().is_empty() => "set",
            _ => "none",
        }),
    }
}

/// The password file, if the caller may read it: root alone does.
fn passwords() -> Option<Table<Password>> {
    fs::read_to_string(accounts::PASSWORDS).ok().map(|text| accounts::passwords(&text))
}

/// `user list --json`: the people's accounts, in the order of the account
/// file.
pub(super) fn list(accounts: &Accounts) -> Result<(), Fail> {
    let passwords = passwords();
    let rows: Vec<Row> = accounts
        .users
        .entries()
        .filter(|u| u.is_person())
        .map(|u| row(u, accounts, passwords.as_ref()))
        .collect();
    println!("{}", serde_json::to_string(&rows).map_err(|e| fail(e.to_string()))?);
    Ok(())
}

/// What `user show --json` writes.
#[derive(Debug, PartialEq, Eq, Serialize)]
struct Show {
    /// The shape of this object: 1.
    format: u8,
    account: Detail,
    /// The keys of `~/.ssh/authorized_keys`, null where the file cannot be
    /// read safely.
    keys: Option<Vec<Key>>,
    /// What authd answers of the second steps, and of the tokens: null for
    /// both where it gives none, as for an account that is no person's.
    steps: Option<Steps>,
    tokens: Option<Vec<Token>>,
    /// The sessions at the machine, null where `/processes` cannot be read.
    sessions: Option<Vec<Session>>,
}

/// The account, and what only `show` tells of it.
#[derive(Debug, PartialEq, Eq, Serialize)]
struct Detail {
    #[serde(flatten)]
    row: Row,
    /// When its home folder was made, if its filesystem keeps that.
    since_ms: Option<i64>,
    /// When its password last changed, which the account files do not keep.
    password_changed_ms: Option<i64>,
}

/// A key of `authorized_keys`.
#[derive(Debug, PartialEq, Eq, Serialize)]
struct Key {
    /// Its kind, as SSH names it.
    #[serde(rename = "type")]
    algorithm: String,
    comment: String,
    fingerprint: String,
    /// Its line in the file.
    line: usize,
    /// Whether options come before it: authd passes such a line over.
    options: bool,
}

/// A session at the machine.
#[derive(Debug, PartialEq, Eq, Serialize)]
struct Session {
    /// Its leader's number and start, which `user end` takes.
    id: String,
    /// `ssh`, `screen`, `serial` or `other`.
    kind: &'static str,
    terminal: String,
    from: Option<String>,
    since_ms: i64,
    /// How many processes it holds that have not ended.
    processes: usize,
}

/// The keys the file `text` holds.
fn keys_of(text: &str) -> Vec<Key> {
    sshkeys::entries(text)
        .into_iter()
        .map(|e| Key {
            fingerprint: sshkeys::fingerprint(&e.key),
            algorithm: e.algorithm,
            comment: e.comment,
            line: e.line,
            options: e.options,
        })
        .collect()
}

/// `found` as `show` lists it.
fn sessions_of(found: &[Found]) -> Vec<Session> {
    found
        .iter()
        .map(|f| Session {
            id: f.id(),
            kind: f.kind(),
            terminal: f.terminal.clone(),
            from: f.from.clone(),
            since_ms: f.start.saturating_mul(1000),
            processes: f.members.len(),
        })
        .collect()
}

/// When `user`'s home folder was made, if its filesystem keeps that.
fn since_ms(user: &User) -> Option<i64> {
    let made = fs::metadata(&user.home).and_then(|m| m.created()).ok()?;
    let since = made.duration_since(std::time::UNIX_EPOCH).ok()?;
    i64::try_from(since.as_millis()).ok()
}

/// What authd says of `user`'s second steps and tokens: its own to the
/// person, `StepsOf` to root for another's. Nothing for an account that no
/// one signs in to the console with; a word on the standard error when
/// authd says none.
fn steps_of(user: &User, asker: u32) -> Option<(Steps, Vec<Token>)> {
    if !user.is_person() {
        return None;
    }
    let request = if user.uid == asker {
        Request::Console(Console::Steps)
    } else {
        Request::StepsOf { name: user.name.clone() }
    };
    match ask(&request) {
        Ok(Answer::Done(text)) => {
            let read = steps_json::parse(&text);
            if read.is_none() {
                eprintln!("user: authd's answer about {}'s second steps is not read", user.name);
            }
            read
        }
        Ok(Answer::Refused(why)) => {
            eprintln!("user: the second steps and tokens of {} are not given: {why}", user.name);
            None
        }
        Err(why) => {
            eprintln!("user: {}", why.message);
            None
        }
    }
}

/// `user show [--json] [NAME]`: an account whole. Root shows any; anyone
/// shows their own, with no name.
pub(super) fn show(args: &mut Args) -> Result<i32, Fail> {
    let (mut json, mut name) = (false, None);
    while let Some(arg) = args.next()? {
        match arg {
            Arg::Long(l) if l == "json" => json = true,
            Arg::Operand(operand) if name.is_none() => {
                name = Some(operand.into_string().map_err(|_| fail("a name is text".into()))?);
            }
            other => return Err(args.unknown(&other)),
        }
    }
    let name = match name {
        Some(name) => name,
        None => own_name()?,
    };
    let accounts = Accounts::read()?;
    let user = accounts.user(&name)?.clone();
    let asker = util::own_user();
    if asker != 0 && user.uid != asker {
        return Err(fail(format!(
            "only {name}, or an administrator inside elevate, sees {name}'s keys, second steps, \
             tokens and sessions"
        )));
    }
    let keys = match sshkeys::read_authorized(&user) {
        Ok(text) => Some(keys_of(text.as_deref().unwrap_or(""))),
        Err(why) => {
            eprintln!("user: the keys of {name} are not read: {why}");
            None
        }
    };
    let (steps, tokens) = match steps_of(&user, asker) {
        Some((steps, tokens)) => (Some(steps), Some(tokens)),
        None => (None, None),
    };
    let records = last::read(&signins::of(user.uid)).ok().flatten().unwrap_or_default();
    let sessions = proc::processes().ok().map(|processes| {
        sessions_of(&session::of(&processes, user.uid, sessions::boot_seconds(), &records))
    });
    let show = Show {
        format: 1,
        account: Detail {
            row: row(&user, &accounts, passwords().as_ref()),
            since_ms: since_ms(&user),
            password_changed_ms: None,
        },
        keys,
        steps,
        tokens,
        sessions,
    };
    if json {
        println!("{}", serde_json::to_string(&show).map_err(|e| fail(e.to_string()))?);
    } else {
        print!("{}", words(&show, &util::local_time));
    }
    Ok(0)
}

/// `show` as a person reads it. `when` writes a time, in seconds since the
/// epoch, by a strftime(3) format.
fn words(show: &Show, when: &dyn Fn(i64, &str) -> String) -> String {
    let Detail { row, since_ms, .. } = &show.account;
    let day = |ms: i64| when(ms / 1000, "%Y-%m-%d %H:%M");
    let yes = |yes: bool| if yes { "yes" } else { "no" };
    let mut out = format!("{} (account {})\n", row.name, row.uid);
    let mut line = |name: &str, text: &str| {
        let _ = writeln!(out, "  {name:<12}{text}");
    };
    line("Full name", &row.full_name);
    line("Kind", if row.admin { "administrator" } else { "user" });
    line("Console", yes(row.console));
    line("Groups", &row.groups.join(", "));
    line("Home", &row.home);
    line("Shell", &row.shell);
    if let Some(disabled) = row.disabled {
        line("Disabled", yes(disabled));
    }
    if let Some(password) = row.password {
        line("Password", password);
    }
    if let Some(ms) = since_ms {
        line("Since", &day(*ms));
    }
    match &show.steps {
        Some(steps) => {
            let mut parts = Vec::new();
            if steps.app {
                parts.push("an authenticator app".to_string());
            }
            if !steps.passkeys.is_empty() {
                let names: Vec<&str> = steps.passkeys.iter().map(|p| p.name.as_str()).collect();
                parts.push(format!("passkeys {}", names.join(", ")));
            }
            if steps.recovery_left > 0 {
                parts.push(format!("{} recovery codes left", steps.recovery_left));
            }
            if steps.trusted_browsers > 0 {
                parts.push(format!("{} trusted browsers", steps.trusted_browsers));
            }
            let said = if parts.is_empty() { "none".to_string() } else { parts.join(", ") };
            line("Second step", &said);
        }
        None => line("Second step", "not given"),
    }
    let _ = writeln!(out, "Keys:");
    match &show.keys {
        Some(keys) if keys.is_empty() => out.push_str("  none\n"),
        Some(keys) => {
            for key in keys {
                let options = if key.options { "  (with options: not taken)" } else { "" };
                let _ = writeln!(
                    out,
                    "  {}  {}  {}{options}",
                    key.fingerprint, key.algorithm, key.comment
                );
            }
        }
        None => out.push_str("  not read\n"),
    }
    let _ = writeln!(out, "Tokens:");
    match &show.tokens {
        Some(tokens) if tokens.is_empty() => out.push_str("  none\n"),
        Some(tokens) => {
            for token in tokens {
                let _ = writeln!(
                    out,
                    "  {}  {}  until {}",
                    token.name,
                    token.scopes.join(" "),
                    day(token.until_ms)
                );
            }
        }
        None => out.push_str("  not given\n"),
    }
    let _ = writeln!(out, "Sessions:");
    match &show.sessions {
        Some(sessions) if sessions.is_empty() => out.push_str("  none\n"),
        Some(sessions) => {
            for s in sessions {
                let from = s.from.as_deref().map_or(String::new(), |from| format!(" from {from}"));
                let _ = writeln!(
                    out,
                    "  {}  {} on {}{from}, since {}, {} processes",
                    s.id,
                    s.kind,
                    s.terminal,
                    day(s.since_ms),
                    s.processes
                );
            }
        }
        None => out.push_str("  not read\n"),
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};

    const PASSWD: &str = "root:x:0:0:root:/root:/system/commands/nukesh\n\
         owner:x:1000:1000:Ann Owner:/users/owner:/system/commands/nukesh\n\
         helper:x:1001:1001::/users/helper:/system/commands/nukesh\n\
         sshd:x:900:900::/nonexistent:/bin/false\n";
    const GROUP: &str = "admins:x:200:owner\nowner:x:1000:\nhelper:x:1001:\nconsole:x:201:helper\n\
         photos:x:300:helper,owner\n";
    const PASSWORDS: &str = "owner:$argon2id$x$y:\nhelper:!$argon2id$x$y:\n";

    fn accounts() -> Accounts {
        Accounts { users: accounts::users(PASSWD), groups: accounts::groups(GROUP), disabled: None }
    }

    fn rows(passwords: Option<&Table<Password>>) -> Vec<Row> {
        let accounts = accounts();
        accounts
            .users
            .entries()
            .filter(|u| u.is_person())
            .map(|u| row(u, &accounts, passwords))
            .collect()
    }

    #[test]
    fn the_list_is_the_people_with_what_the_files_say() {
        let passwords = accounts::passwords(PASSWORDS);
        let shown = serde_json::to_value(rows(Some(&passwords))).unwrap();
        assert_eq!(
            shown,
            json!([
                {"name": "owner", "uid": 1000, "full_name": "Ann Owner",
                 "home": "/users/owner", "shell": "/system/commands/nukesh", "admin": true,
                 "console": true, "groups": ["admins", "owner", "photos"],
                 "disabled": false, "password": "set"},
                {"name": "helper", "uid": 1001, "full_name": "", "home": "/users/helper",
                 "shell": "/system/commands/nukesh", "admin": false, "console": true,
                 "groups": ["console", "helper", "photos"], "disabled": true,
                 "password": "set"},
            ])
        );
    }

    #[test]
    fn what_is_root_s_to_read_is_null_for_anyone_else() {
        let shown = serde_json::to_value(rows(None)).unwrap();
        for account in shown.as_array().unwrap() {
            assert_eq!(account["disabled"], Value::Null);
            assert_eq!(account["password"], Value::Null);
        }
        // An account with no line in the password file has none.
        let passwords = accounts::passwords("owner:$argon2id$x$y:\nhelper::\n");
        let shown = serde_json::to_value(rows(Some(&passwords))).unwrap();
        assert_eq!(shown[1]["password"], "none");
        let none = accounts::passwords("");
        assert_eq!(serde_json::to_value(rows(Some(&none))).unwrap()[0]["password"], "none");
    }

    #[test]
    fn an_account_with_no_console_right_says_so() {
        let accounts = Accounts {
            users: accounts::users(PASSWD),
            groups: accounts::groups("owner:x:1000:\nhelper:x:1001:\n"),
            disabled: None,
        };
        let helper = accounts.users.entries().find(|u| u.name == "helper").unwrap();
        let row = row(helper, &accounts, None);
        assert!(!row.console && !row.admin);
        assert_eq!(row.groups, ["helper"]);
    }

    /// The key of the line, and one with options before it.
    const KEYS: &str = "# keys\n\
        ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIAABAgMEBQYHCAkKCwwNDg8QERITFBUWFxgZGhscHR4f laptop\n\
        command=\"date\" ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIAABAgMEBQYHCAkKCwwNDg8QERITFBUWFxgZGhscHR4g a job\n";

    /// What `user show --json owner` writes for a machine of the fixtures
    /// above, with a golden file to hold its fields.
    fn shown() -> Show {
        let passwords = accounts::passwords(PASSWORDS);
        let accounts = accounts();
        let owner = accounts.users.entries().find(|u| u.name == "owner").unwrap();
        let (steps, tokens) = steps_json::parse(
            r#"{"app":{"added":1},"passkeys":[{"id":"AbC-1","relying-party":"lab",
                "name":"Security key","added":1,"used":2}],"recovery":{"left":8,"made":3},
                "trusted":2,"tokens":[{"name":"ansible","scopes":["*"],"until":1765600000,
                "made":1,"used":0,"expired":false}],"needed":true}"#,
        )
        .unwrap();
        let found = [Found {
            leader: 4123,
            terminal: "pts/3".into(),
            start: 1_763_110_000,
            from: Some("10.8.0.2".into()),
            members: vec![(4123, 1), (4124, 2), (4130, 3), (4131, 4), (4140, 5)],
        }];
        Show {
            format: 1,
            account: Detail {
                row: row(owner, &accounts, Some(&passwords)),
                since_ms: Some(1_760_900_000_000),
                password_changed_ms: None,
            },
            keys: Some(keys_of(KEYS)),
            steps: Some(steps),
            tokens: Some(tokens),
            sessions: Some(sessions_of(&found)),
        }
    }

    #[test]
    fn the_object_is_the_golden_file() {
        let golden: Value = serde_json::from_str(include_str!("read.golden.json")).unwrap();
        assert_eq!(serde_json::to_value(shown()).unwrap(), golden);
    }

    #[test]
    fn what_the_machine_does_not_give_is_null() {
        let show = Show {
            keys: None,
            steps: None,
            tokens: None,
            sessions: None,
            account: Detail { since_ms: None, ..shown().account },
            ..shown()
        };
        let value = serde_json::to_value(show).unwrap();
        for field in ["keys", "steps", "tokens", "sessions"] {
            assert_eq!(value[field], Value::Null, "{field}");
        }
        assert_eq!(value["account"]["since_ms"], Value::Null);
        assert_eq!(value["account"]["password_changed_ms"], Value::Null);
    }

    #[test]
    fn a_person_reads_the_same() {
        let utc = |seconds: i64, format: &str| util::time_text(seconds, format, true);
        let job = keys_of(KEYS)[1].fingerprint.clone();
        assert_eq!(
            words(&shown(), &utc),
            format!(
                "owner (account 1000)\n\
                 \x20 Full name   Ann Owner\n\
                 \x20 Kind        administrator\n\
                 \x20 Console     yes\n\
                 \x20 Groups      admins, owner, photos\n\
                 \x20 Home        /users/owner\n\
                 \x20 Shell       /system/commands/nukesh\n\
                 \x20 Disabled    no\n\
                 \x20 Password    set\n\
                 \x20 Since       2025-10-19 18:53\n\
                 \x20 Second step an authenticator app, passkeys Security key, 8 recovery codes \
                 left, 2 trusted browsers\n\
                 Keys:\n\
                 \x20 SHA256:ZkAslGjFiUHdGf/WUL8rQvkib4PTvQatUV0OUQSncCA  ssh-ed25519  laptop\n\
                 \x20 {job}  ssh-ed25519  a job  (with options: not taken)\n\
                 Tokens:\n\
                 \x20 ansible  *  until 2025-12-13 04:26\n\
                 Sessions:\n\
                 \x20 4123-1763110000  ssh on pts/3 from 10.8.0.2, since 2025-11-14 08:46, 5 \
                 processes\n"
            )
        );
    }

    #[test]
    fn what_is_not_given_is_said_not_given() {
        let utc = |seconds: i64, format: &str| util::time_text(seconds, format, true);
        let show = Show {
            keys: None,
            steps: None,
            tokens: None,
            sessions: None,
            account: Detail {
                row: Row { disabled: None, password: None, ..shown().account.row },
                since_ms: None,
                password_changed_ms: None,
            },
            ..shown()
        };
        let text = words(&show, &utc);
        assert!(text.contains("Second step not given\n"), "{text}");
        assert!(text.contains("Keys:\n  not read\n"), "{text}");
        assert!(text.contains("Tokens:\n  not given\n"), "{text}");
        assert!(text.ends_with("Sessions:\n  not read\n"), "{text}");
        assert!(!text.contains("Disabled") && !text.contains("Password"));
    }
}
