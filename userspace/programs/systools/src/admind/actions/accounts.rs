// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The accounts' actions (ADR 0515, ADR 0561, ADR 0562): who has an
//! account and what each can do, when each last signed in, one account
//! whole, and an account made, changed, given a new password, disabled,
//! enabled, removed, given or taken a key, taken a token, taken its second
//! steps and taken a session at the machine. The area is `accounts`, the
//! scope a token needs.
//!
//! The list is the person's own: `user list --json` says only what the
//! account files say to everyone. The rest run as an administrator, as root
//! reads the password file, another account's keys, second steps, tokens
//! and sessions, and as root changes them.
//!
//! A password is a secret (ADR 0561): it is never one of a command's words,
//! and the command reads it on its standard input, as `passwd` and `user
//! add` read it from a pipe.
//!
//! The person's own account, and who may reach it, is [`super::account`].

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::{Action, Ansible, Args, Bad, Examples, Field, Nothing, Runs, Secret, Tests, Undo};
use crate::account::capitalized;
use crate::account::keys::{check_line, fingerprint_ok};
use crate::account::session::parse_id;
use crate::authd::console::token_name_ok;

/// The area of these actions: the scope a token needs, `accounts`.
const AREA: &str = "accounts";

/// The tests of these actions, which `lab/actions.toml` names.
const TESTS: Tests = Tests {
    checks: "admind::actions::accounts::tests",
    route: "admin-accounts",
    same_as_command: "admin-accounts",
    browser: "web-console/accounts",
    ansible: "web-console/ansible",
};

/// What an account's name matches: `accounts::check_name`'s rule, which the
/// pattern says as far as a pattern can: lowercase letters, digits, `_`, `-`
/// and `.`, starting with a letter or `_`, and not `help`, which the helpers
/// of help sessions sign in as.
pub(super) const NAME_PATTERN: &str = "^(?!help$)[a-z_][a-z0-9_.-]*$";

/// An account's name field, for each action that takes one.
const NAME: Field = Field::text(
    "name",
    "The account's name, as user list shows it.",
    1,
    accounts::NAME_MAX,
    Some(NAME_PATTERN),
);

/// What a full name matches: no colon and no line break, which end the
/// account file's fields and lines.
pub(super) const FULL_NAME_PATTERN: &str = "^[^:\n\r]*$";

/// The most bytes of a full name.
const FULL_NAME_MAX: usize = 256;

/// A full name field, for each action that takes one. A schema counts
/// characters and the rule bytes, so the action checks the bytes.
pub(super) const FULL_NAME: Field = Field::text(
    "full_name",
    "The full name: at most 256 bytes, with no colon or line break.",
    0,
    FULL_NAME_MAX,
    Some(FULL_NAME_PATTERN),
);

/// The most characters of a password the actions take. The command takes
/// 1024 bytes ([`accounts::PASSWORD_MAX`]), which the action checks as
/// bytes; the characters are what fits, with a line's room at four bytes
/// each, in a pipe beside the others (`secret::INPUT_MAX`).
pub(super) const PASSWORD_CHARS: usize = 1023;

/// The most characters of each of the two passwords of a person's own
/// change, which share a pipe.
pub(super) const TWO_PASSWORDS_CHARS: usize = 511;

/// The most bytes of a key's line, as a file of keys has them in a line.
pub(super) const LINE_MAX: usize = 16 * 1024;

/// The key field, for each action that adds one.
pub(super) const LINE: Field = Field::text(
    "line",
    "The key, as ssh-keygen prints it in a .pub file: one line, Ed25519 or ECDSA (P-256 or \
     P-384), with no options before it.",
    1,
    LINE_MAX,
    None,
);

/// What a key's fingerprint matches: `SHA256:` and 43 characters of
/// base64, which a text of 50 characters says exactly.
pub(super) const FINGERPRINT_PATTERN: &str = "^SHA256:[A-Za-z0-9+/]*$";

/// The fingerprint field, for each action that takes a key away.
pub(super) const FINGERPRINT: Field = Field::text(
    "fingerprint",
    "The key's fingerprint, as user show gives it: SHA256: and 43 characters.",
    50,
    50,
    Some(FINGERPRINT_PATTERN),
);

/// An account's name, which `accounts::check_name` takes.
fn check_name(name: &str) -> Result<(), Bad> {
    accounts::check_name(name).map_err(|_| Bad {
        field: "name",
        why: format!(
            "An account's name is 1 to {} lowercase letters, digits, _, - and ., starting with a \
             letter or _, and is not help. Nothing was done.",
            accounts::NAME_MAX
        ),
    })
}

/// A full name, as `accounts::check_full_name` takes it.
pub(super) fn check_full_name(full_name: &str) -> Result<(), Bad> {
    accounts::check_full_name(full_name).map_err(|_| Bad {
        field: "full_name",
        why: format!(
            "A full name has at most {FULL_NAME_MAX} bytes, and no colon or line break. Nothing \
             was done."
        ),
    })
}

/// A password, which `passwd` and `user add` take: at most
/// [`accounts::PASSWORD_MAX`] bytes. `field` names it.
pub(super) fn check_password(field: &'static str, password: &Secret) -> Result<(), Bad> {
    if password.bytes().len() > accounts::PASSWORD_MAX {
        return Err(Bad {
            field,
            why: format!(
                "A password is at most {} bytes. Nothing was done.",
                accounts::PASSWORD_MAX
            ),
        });
    }
    Ok(())
}

/// A key's line, which `user key add` takes: the parser authd reads keys
/// with finds a key of a kind it signs in with, and no options before it.
pub(super) fn check_key_line(line: &str) -> Result<(), Bad> {
    if line.len() > LINE_MAX {
        return Err(Bad {
            field: "line",
            why: format!("A key is a line of at most {LINE_MAX} bytes. Nothing was added."),
        });
    }
    check_line(line).map(|_| ()).map_err(|why| Bad {
        field: "line",
        why: format!(
            "{}. Dukovo takes Ed25519 and ECDSA (P-256 and P-384) keys, as one line the way \
             ssh-keygen writes it, with no options before the key. Nothing was added.",
            capitalized(&why)
        ),
    })
}

/// A fingerprint, as `ssh-keygen -l` and `user show` give it.
pub(super) fn check_fingerprint(fingerprint: &str) -> Result<(), Bad> {
    if fingerprint_ok(fingerprint) {
        return Ok(());
    }
    Err(Bad {
        field: "fingerprint",
        why: "A fingerprint is SHA256: and 43 characters, as user show gives it. Nothing was \
              removed."
            .into(),
    })
}

/// The two account files, which making, changing and removing an account
/// write.
fn account_files() -> Vec<String> {
    [accounts::PASSWD, accounts::GROUP].map(String::from).to_vec()
}

/// The command that reads what making, changing or removing an account
/// moves: the list. It runs as the person, who is not root, so what it says
/// of whether an account is disabled or has a password is null, and neither
/// change is one of those.
fn list_argv() -> Vec<String> {
    ["user", "list", "--json"].map(String::from).to_vec()
}

/// What `user list --json` says of account `name`: its row as text, or
/// words for a list that has no such account, so that a change compares
/// before and after. None for output that is not the list.
fn picked(name: &str, output: &str) -> Option<String> {
    let rows: Vec<Value> = serde_json::from_str(output).ok()?;
    let row = rows.iter().find(|row| row["name"] == name);
    Some(row.map_or_else(|| "no such account".to_string(), Value::to_string))
}

/// An account, by its name.
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Named {
    pub name: String,
}

impl Args for Named {
    const FIELDS: &'static [Field] = &[NAME];

    fn check(&self) -> Result<(), Bad> {
        check_name(&self.name)
    }
}

/// Examples for an action that takes an account's name: `lab-made`, which
/// the boot test makes.
fn named_examples() -> Examples<Named> {
    Examples {
        good: vec![Named { name: "lab-made".into() }],
        bad: vec![json!({ "name": "" }), json!({ "name": "a b" }), json!({ "all": true })],
        undo: None,
    }
}

/// `user list --json` as the person: the accounts and what each can do.
pub struct List;

impl Action for List {
    const ID: &'static str = "accounts.list";
    const AREA: &'static str = AREA;
    const CHANGES: bool = false;
    const PATH: &'static str = "/api/1/accounts";
    const RUNS: Runs = Runs::AsThePerson;
    const TESTS: Tests = TESTS;
    const TITLE: &'static str = "List the accounts";
    type Args = Nothing;

    fn argv(_: &Nothing) -> Vec<String> {
        list_argv()
    }

    fn examples() -> Examples<Nothing> {
        Examples { good: vec![Nothing {}], bad: vec![json!({ "all": true })], undo: None }
    }
}

/// `user list --json` as an administrator: the list, with whether each
/// account is disabled and has a password, which only root reads. The
/// list's page reads it for an administrator who has unlocked, so that a
/// disabled account, and one that signs in with a key alone, say so.
///
/// Its address is not one of an account's: `/api/1/accounts/all` would be
/// [`Show`]'s for an account called `all`.
pub struct ListAll;

impl Action for ListAll {
    const ID: &'static str = "accounts.list-all";
    const AREA: &'static str = AREA;
    const CHANGES: bool = false;
    const PATH: &'static str = "/api/1/accounts/list/all";
    const RUNS: Runs = Runs::AsAdministrator;
    const TESTS: Tests = TESTS;
    const TITLE: &'static str = "List the accounts as an administrator";
    type Args = Nothing;

    fn argv(_: &Nothing) -> Vec<String> {
        list_argv()
    }

    fn examples() -> Examples<Nothing> {
        Examples { good: vec![Nothing {}], bad: vec![json!({ "all": true })], undo: None }
    }
}

/// `lastlog --json`: when each account last signed in, and the wrong
/// passwords typed for it since.
///
/// It is before [`Show`] in the list, since its address is also one of an
/// account called `last`.
pub struct Last;

impl Action for Last {
    const ID: &'static str = "accounts.last";
    const AREA: &'static str = AREA;
    const CHANGES: bool = false;
    const PATH: &'static str = "/api/1/sign-ins/last";
    const RUNS: Runs = Runs::AsAdministrator;
    const TESTS: Tests = TESTS;
    const TITLE: &'static str = "Show when each account last signed in";
    type Args = Nothing;

    fn argv(_: &Nothing) -> Vec<String> {
        ["lastlog", "--json"].map(String::from).to_vec()
    }

    fn examples() -> Examples<Nothing> {
        Examples { good: vec![Nothing {}], bad: vec![json!({ "all": true })], undo: None }
    }
}

/// `user show NAME --json`: an account whole, with its keys, second steps,
/// tokens and sessions at the machine.
pub struct Show;

impl Action for Show {
    const ID: &'static str = "accounts.show";
    const AREA: &'static str = AREA;
    const CHANGES: bool = false;
    const PATH: &'static str = "/api/1/accounts/{name}";
    const RUNS: Runs = Runs::AsAdministrator;
    const ERRORS: &'static [&'static str] = &["ACC-101"];
    const TESTS: Tests = TESTS;
    const TITLE: &'static str = "Show an account";
    type Args = Named;

    fn argv(args: &Named) -> Vec<String> {
        vec!["user".into(), "show".into(), args.name.clone(), "--json".into()]
    }

    fn subject(args: &Named) -> Option<String> {
        Some(args.name.clone())
    }

    fn examples() -> Examples<Named> {
        named_examples()
    }
}

/// An account to make.
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Added {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub full_name: Option<String>,
    /// Whether it is an administrator's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub admin: Option<bool>,
    /// Its password, which the command reads on its input; none yet when it
    /// is left out, and the account signs in with none until one is set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub password: Option<Secret>,
}

impl Args for Added {
    const FIELDS: &'static [Field] = &[
        NAME,
        FULL_NAME.optional(),
        Field::flag("admin", "Whether it is an administrator's account.").optional(),
        Field::secret("password", "Its password. Left out, it has none yet.", PASSWORD_CHARS)
            .optional(),
    ];

    fn check(&self) -> Result<(), Bad> {
        check_name(&self.name)?;
        if let Some(full_name) = &self.full_name {
            check_full_name(full_name)?;
        }
        if let Some(password) = &self.password {
            check_password("password", password)?;
        }
        Ok(())
    }
}

/// `user add [--admin] [--full-name TEXT] NAME`: makes an account, with its
/// password on the input.
pub struct Add;

impl Action for Add {
    const ID: &'static str = "accounts.add";
    const AREA: &'static str = AREA;
    const CHANGES: bool = true;
    const PATH: &'static str = "/api/1/accounts/add";
    const RUNS: Runs = Runs::AsAdministrator;
    const ERRORS: &'static [&'static str] = &["ACC-105"];
    const TESTS: Tests = TESTS;
    const TITLE: &'static str = "Add an account";
    type Args = Added;

    fn argv(args: &Added) -> Vec<String> {
        let mut argv = vec!["user".to_string(), "add".to_string()];
        if args.admin == Some(true) {
            argv.push("--admin".into());
        }
        if let Some(full_name) = &args.full_name {
            argv.extend(["--full-name".to_string(), full_name.clone()]);
        }
        argv.push(args.name.clone());
        argv
    }

    /// The password's line, or an empty one for an account with none yet:
    /// `user add` asks for a password always, and an empty one is the answer
    /// that says so.
    fn input(args: &Added) -> Option<Secret> {
        let none = Secret::from(String::new());
        Some(Secret::lines(&[args.password.as_ref().unwrap_or(&none)]))
    }

    /// The task. One that has no password gives the command an empty line:
    /// Ansible adds the line's end to what it is given, and gives a command
    /// no input at all for an empty text.
    fn ansible(args: &Added) -> Ansible {
        let name = format!("{}: {}", Self::TITLE, args.name);
        let mut task = Ansible::command(name, &Self::argv(args), true, true);
        if args.password.is_none() {
            task.params.insert("stdin".into(), "\n".into());
        }
        task
    }

    fn subject(args: &Added) -> Option<String> {
        Some(args.name.clone())
    }

    fn files(_: &Added) -> Vec<String> {
        account_files()
    }

    fn probe(_: &Added) -> Option<Vec<String>> {
        Some(list_argv())
    }

    fn probed(args: &Added, output: &str) -> Option<String> {
        picked(&args.name, output)
    }

    fn examples() -> Examples<Added> {
        let good = Added {
            name: "lab-made".into(),
            full_name: Some("Made by the test".into()),
            admin: None,
            password: Some(Secret::from("lab-pass-1".to_string())),
        };
        Examples {
            good: vec![good],
            // A name that is taken is the command's to refuse (ACC-105), not
            // the check's, so it is no bad example: those the check refuses.
            bad: vec![json!({ "name": "Bad Name" })],
            undo: Some(Undo {
                action: "accounts.remove",
                args: json!({ "name": "lab-made", "home": "delete" }),
            }),
        }
    }
}

/// What to change of an account.
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Setting {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub full_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub admin: Option<bool>,
    /// Whether it signs in to the console without being an administrator.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub console: Option<bool>,
}

impl Args for Setting {
    const FIELDS: &'static [Field] = &[
        NAME,
        FULL_NAME.optional(),
        Field::flag("admin", "Whether it is an administrator's account.").optional(),
        Field::flag(
            "console",
            "Whether it signs in to Dukovo Admin without being an administrator.",
        )
        .optional(),
    ];

    fn check(&self) -> Result<(), Bad> {
        check_name(&self.name)?;
        if let Some(full_name) = &self.full_name {
            check_full_name(full_name)?;
        }
        if self.full_name.is_none() && self.admin.is_none() && self.console.is_none() {
            return Err(Bad {
                field: "full_name",
                why: "Say what to change: full_name, admin or console. Nothing was changed.".into(),
            });
        }
        Ok(())
    }
}

/// `user set [--full-name TEXT] [--admin | --no-admin] [--console |
/// --no-console] --yes NAME`: changes an account.
pub struct Set;

impl Action for Set {
    const ID: &'static str = "accounts.set";
    const AREA: &'static str = AREA;
    const CHANGES: bool = true;
    const PATH: &'static str = "/api/1/accounts/{name}/set";
    const RUNS: Runs = Runs::AsAdministrator;
    const ERRORS: &'static [&'static str] = &["ACC-101", "ACC-102"];
    const NOT_SELF: Option<&'static str> = Some("name");
    const TESTS: Tests = TESTS;
    const TITLE: &'static str = "Change an account";
    type Args = Setting;

    fn argv(args: &Setting) -> Vec<String> {
        let mut argv = vec!["user".to_string(), "set".to_string()];
        if let Some(full_name) = &args.full_name {
            argv.extend(["--full-name".to_string(), full_name.clone()]);
        }
        match args.admin {
            Some(true) => argv.push("--admin".into()),
            Some(false) => argv.push("--no-admin".into()),
            None => {}
        }
        match args.console {
            Some(true) => argv.push("--console".into()),
            Some(false) => argv.push("--no-console".into()),
            None => {}
        }
        argv.extend(["--yes".to_string(), args.name.clone()]);
        argv
    }

    /// Only taking the administrator right away could lock the caller
    /// out: a full name, or the console's right, which an administrator
    /// always has, may be changed on one's own account.
    fn not_self_applies(args: &Setting) -> bool {
        args.admin == Some(false)
    }

    fn subject(args: &Setting) -> Option<String> {
        Some(args.name.clone())
    }

    fn files(_: &Setting) -> Vec<String> {
        account_files()
    }

    fn probe(_: &Setting) -> Option<Vec<String>> {
        Some(list_argv())
    }

    fn probed(args: &Setting, output: &str) -> Option<String> {
        picked(&args.name, output)
    }

    fn examples() -> Examples<Setting> {
        Examples {
            good: vec![Setting {
                name: "lab-made".into(),
                full_name: None,
                admin: None,
                console: Some(false),
            }],
            bad: vec![
                json!({ "name": "lab-made" }),
                json!({ "admin": "yes", "name": "lab-made" }),
                json!({ "full_name": "a:b", "name": "lab-made" }),
            ],
            undo: Some(Undo {
                action: "accounts.set",
                args: json!({ "name": "lab-made", "console": true }),
            }),
        }
    }
}

/// An account and the new password it is given.
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct NewPassword {
    pub name: String,
    pub password: Secret,
}

impl Args for NewPassword {
    const FIELDS: &'static [Field] =
        &[NAME, Field::secret("password", "The new password.", PASSWORD_CHARS)];

    fn check(&self) -> Result<(), Bad> {
        check_name(&self.name)?;
        check_password("password", &self.password)
    }
}

/// `passwd NAME`: gives an account a new password, read from the input. An
/// administrator changes anyone's without the old one.
pub struct Password;

impl Action for Password {
    const ID: &'static str = "accounts.password";
    const AREA: &'static str = AREA;
    const CHANGES: bool = true;
    const PATH: &'static str = "/api/1/accounts/{name}/password";
    const RUNS: Runs = Runs::AsAdministrator;
    const ERRORS: &'static [&'static str] = &["ACC-101"];
    const TESTS: Tests = TESTS;
    const TITLE: &'static str = "Change an account's password";
    type Args = NewPassword;

    fn argv(args: &NewPassword) -> Vec<String> {
        vec!["passwd".into(), args.name.clone()]
    }

    fn input(args: &NewPassword) -> Option<Secret> {
        Some(Secret::lines(&[&args.password]))
    }

    fn subject(args: &NewPassword) -> Option<String> {
        Some(args.name.clone())
    }

    fn examples() -> Examples<NewPassword> {
        Examples {
            good: vec![NewPassword {
                name: "lab-made".into(),
                password: Secret::from("lab-pass-2".to_string()),
            }],
            bad: vec![
                json!({ "name": "lab-made" }),
                json!({ "name": "lab-made", "password": "" }),
                json!({ "name": "lab-made", "password": "two\nlines" }),
            ],
            undo: None,
        }
    }
}

/// `user disable --yes NAME`: nothing signs in to the account, and what it
/// runs ends.
pub struct Disable;

impl Action for Disable {
    const ID: &'static str = "accounts.disable";
    const AREA: &'static str = AREA;
    const CHANGES: bool = true;
    const PATH: &'static str = "/api/1/accounts/{name}/disable";
    const RUNS: Runs = Runs::AsAdministrator;
    const ERRORS: &'static [&'static str] = &["ACC-101", "ACC-102"];
    const NOT_SELF: Option<&'static str> = Some("name");
    const TESTS: Tests = TESTS;
    const TITLE: &'static str = "Disable an account";
    type Args = Named;

    fn argv(args: &Named) -> Vec<String> {
        vec!["user".into(), "disable".into(), "--yes".into(), args.name.clone()]
    }

    fn subject(args: &Named) -> Option<String> {
        Some(args.name.clone())
    }

    fn examples() -> Examples<Named> {
        Examples {
            undo: Some(Undo { action: "accounts.enable", args: json!({ "name": "lab-made" }) }),
            ..named_examples()
        }
    }
}

/// `user enable NAME`: the account signs in again, with its password.
pub struct Enable;

impl Action for Enable {
    const ID: &'static str = "accounts.enable";
    const AREA: &'static str = AREA;
    const CHANGES: bool = true;
    const PATH: &'static str = "/api/1/accounts/{name}/enable";
    const RUNS: Runs = Runs::AsAdministrator;
    const ERRORS: &'static [&'static str] = &["ACC-101"];
    const TESTS: Tests = TESTS;
    const TITLE: &'static str = "Enable an account";
    type Args = Named;

    fn argv(args: &Named) -> Vec<String> {
        vec!["user".into(), "enable".into(), args.name.clone()]
    }

    fn subject(args: &Named) -> Option<String> {
        Some(args.name.clone())
    }

    fn examples() -> Examples<Named> {
        Examples {
            undo: Some(Undo { action: "accounts.disable", args: json!({ "name": "lab-made" }) }),
            ..named_examples()
        }
    }
}

/// What becomes of a removed account's home.
const HOME: &[&str] = &["keep", "archive", "delete"];

/// An account to remove, and what becomes of its home.
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Removal {
    pub name: String,
    pub home: String,
}

impl Args for Removal {
    const FIELDS: &'static [Field] = &[
        NAME,
        Field::one_of(
            "home",
            "What becomes of its home folder: keep it where it is, archive it, or delete it.",
            HOME,
        ),
    ];

    fn check(&self) -> Result<(), Bad> {
        check_name(&self.name)?;
        if !HOME.contains(&self.home.as_str()) {
            return Err(Bad {
                field: "home",
                why: "home is keep, archive or delete. Nothing was removed.".into(),
            });
        }
        Ok(())
    }
}

/// `user remove --keep-home | --archive-home | --delete-home --yes NAME`:
/// takes an account away, with what it runs; its home is kept, archived or
/// deleted.
pub struct Remove;

impl Action for Remove {
    const ID: &'static str = "accounts.remove";
    const AREA: &'static str = AREA;
    const CHANGES: bool = true;
    const PATH: &'static str = "/api/1/accounts/{name}/remove";
    const RUNS: Runs = Runs::AsAdministrator;
    const ERRORS: &'static [&'static str] = &["ACC-101", "ACC-102"];
    const NOT_SELF: Option<&'static str> = Some("name");
    const TESTS: Tests = TESTS;
    const TITLE: &'static str = "Remove an account";
    type Args = Removal;

    fn argv(args: &Removal) -> Vec<String> {
        vec![
            "user".into(),
            "remove".into(),
            format!("--{}-home", args.home),
            "--yes".into(),
            args.name.clone(),
        ]
    }

    fn subject(args: &Removal) -> Option<String> {
        Some(args.name.clone())
    }

    fn files(_: &Removal) -> Vec<String> {
        account_files()
    }

    fn probe(_: &Removal) -> Option<Vec<String>> {
        Some(list_argv())
    }

    fn probed(args: &Removal, output: &str) -> Option<String> {
        picked(&args.name, output)
    }

    fn examples() -> Examples<Removal> {
        Examples {
            good: vec![Removal { name: "lab-made".into(), home: "delete".into() }],
            bad: vec![
                json!({ "name": "lab-made" }),
                json!({ "home": "burn", "name": "lab-made" }),
                json!({ "home": "delete", "name": "" }),
            ],
            undo: None,
        }
    }
}

/// A key to add to an account.
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct KeyLine {
    pub name: String,
    pub line: String,
}

impl Args for KeyLine {
    const FIELDS: &'static [Field] = &[NAME, LINE];

    fn check(&self) -> Result<(), Bad> {
        check_name(&self.name)?;
        check_key_line(&self.line)
    }
}

/// The key the examples add: an Ed25519 key of 32 counted bytes, as the
/// unit tests of `sshkeys.rs` use.
pub(super) const EXAMPLE_KEY: &str =
    "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIAABAgMEBQYHCAkKCwwNDg8QERITFBUWFxgZGhscHR4f lab";

/// The fingerprint of [`EXAMPLE_KEY`], which takes it away. A unit test
/// holds it to what `sshkeys.rs` makes of the key.
pub(super) const EXAMPLE_FINGERPRINT: &str = "SHA256:ZkAslGjFiUHdGf/WUL8rQvkib4PTvQatUV0OUQSncCA";

/// `user key add NAME LINE`: adds a key to an account's `authorized_keys`.
///
/// It names no file and has no probe, so it always did something. admind's
/// service sees no home folder, so the journal would find the file absent
/// before the change and after it; and a probe runs as the person, who may
/// not read another account's keys. The same is true of [`KeyRemove`].
pub struct KeyAdd;

impl Action for KeyAdd {
    const ID: &'static str = "accounts.key-add";
    const AREA: &'static str = AREA;
    const CHANGES: bool = true;
    const PATH: &'static str = "/api/1/accounts/{name}/keys/add";
    const RUNS: Runs = Runs::AsAdministrator;
    const ERRORS: &'static [&'static str] = &["ACC-101", "ACC-103"];
    const TESTS: Tests = TESTS;
    const TITLE: &'static str = "Add an SSH key to an account";
    type Args = KeyLine;

    fn argv(args: &KeyLine) -> Vec<String> {
        vec!["user".into(), "key".into(), "add".into(), args.name.clone(), args.line.clone()]
    }

    fn subject(args: &KeyLine) -> Option<String> {
        Some(args.name.clone())
    }

    fn examples() -> Examples<KeyLine> {
        let key = |line: &str| json!({ "line": line, "name": "lab-made" });
        Examples {
            good: vec![KeyLine { name: "lab-made".into(), line: EXAMPLE_KEY.into() }],
            bad: vec![
                key("ssh-dss AAAAB3NzaC1kc3MAAACBAP"),
                key(&format!("command=\"x\" {EXAMPLE_KEY}")),
            ],
            undo: Some(Undo {
                action: "accounts.key-remove",
                args: json!({ "fingerprint": EXAMPLE_FINGERPRINT, "name": "lab-made" }),
            }),
        }
    }
}

/// A key to take from an account.
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct KeyPrint {
    pub name: String,
    pub fingerprint: String,
}

impl Args for KeyPrint {
    const FIELDS: &'static [Field] = &[NAME, FINGERPRINT];

    fn check(&self) -> Result<(), Bad> {
        check_name(&self.name)?;
        check_fingerprint(&self.fingerprint)
    }
}

/// `user key remove NAME FINGERPRINT`: takes a key from an account's
/// `authorized_keys`.
pub struct KeyRemove;

impl Action for KeyRemove {
    const ID: &'static str = "accounts.key-remove";
    const AREA: &'static str = AREA;
    const CHANGES: bool = true;
    const PATH: &'static str = "/api/1/accounts/{name}/keys/remove";
    const RUNS: Runs = Runs::AsAdministrator;
    const ERRORS: &'static [&'static str] = &["ACC-101", "ACC-104"];
    const TESTS: Tests = TESTS;
    const TITLE: &'static str = "Remove an SSH key from an account";
    type Args = KeyPrint;

    fn argv(args: &KeyPrint) -> Vec<String> {
        vec![
            "user".into(),
            "key".into(),
            "remove".into(),
            args.name.clone(),
            args.fingerprint.clone(),
        ]
    }

    fn subject(args: &KeyPrint) -> Option<String> {
        Some(args.name.clone())
    }

    fn examples() -> Examples<KeyPrint> {
        Examples {
            good: vec![KeyPrint {
                name: "lab-made".into(),
                fingerprint: EXAMPLE_FINGERPRINT.into(),
            }],
            bad: vec![
                json!({ "fingerprint": "SHA256:short", "name": "lab-made" }),
                json!({ "fingerprint": "MD5:00", "name": "lab-made" }),
                json!({ "name": "lab-made" }),
            ],
            undo: None,
        }
    }
}

/// An API token to take from an account.
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TokenOf {
    pub name: String,
    pub token: String,
}

impl Args for TokenOf {
    const FIELDS: &'static [Field] = &[
        NAME,
        Field::text(
            "token",
            "The token's name, as user show gives it: 1 to 32 of a to z, 0 to 9 and -.",
            1,
            32,
            Some("^[a-z0-9-]+$"),
        ),
    ];

    fn check(&self) -> Result<(), Bad> {
        check_name(&self.name)?;
        if !token_name_ok(&self.token) {
            return Err(Bad {
                field: "token",
                why: "A token's name is 1 to 32 of a to z, 0 to 9 and -. Nothing was removed."
                    .into(),
            });
        }
        Ok(())
    }
}

/// `user token remove NAME TOKEN`: takes an API token from an account, and
/// whatever uses it stops working. authd keeps the state.
pub struct TokenRevoke;

impl Action for TokenRevoke {
    const ID: &'static str = "accounts.token-revoke";
    const AREA: &'static str = AREA;
    const CHANGES: bool = true;
    const PATH: &'static str = "/api/1/accounts/{name}/tokens/remove";
    const RUNS: Runs = Runs::AsAdministrator;
    const ERRORS: &'static [&'static str] = &["ACC-101", "ACC-104"];
    const TESTS: Tests = TESTS;
    const TITLE: &'static str = "Revoke an API token of an account";
    type Args = TokenOf;

    fn argv(args: &TokenOf) -> Vec<String> {
        vec!["user".into(), "token".into(), "remove".into(), args.name.clone(), args.token.clone()]
    }

    fn subject(args: &TokenOf) -> Option<String> {
        Some(format!("{} of {}", args.token, args.name))
    }

    fn examples() -> Examples<TokenOf> {
        Examples {
            good: vec![TokenOf { name: "lab-made".into(), token: "lab-token".into() }],
            bad: vec![
                json!({ "name": "lab-made", "token": "" }),
                json!({ "name": "lab-made", "token": "Lab Token" }),
                json!({ "name": "lab-made" }),
            ],
            undo: None,
        }
    }
}

/// `user steps reset NAME`: takes an account's authenticator app, passkeys,
/// recovery codes and trusted browsers away; the person is told at their
/// next sign-in. authd keeps the state.
pub struct StepsReset;

impl Action for StepsReset {
    const ID: &'static str = "accounts.steps-reset";
    const AREA: &'static str = AREA;
    const CHANGES: bool = true;
    const PATH: &'static str = "/api/1/accounts/{name}/second-step/reset";
    const RUNS: Runs = Runs::AsAdministrator;
    const ERRORS: &'static [&'static str] = &["ACC-101"];
    const TESTS: Tests = TESTS;
    const TITLE: &'static str = "Remove an account's second steps";
    type Args = Named;

    fn argv(args: &Named) -> Vec<String> {
        vec!["user".into(), "steps".into(), "reset".into(), args.name.clone()]
    }

    fn subject(args: &Named) -> Option<String> {
        Some(args.name.clone())
    }

    fn examples() -> Examples<Named> {
        named_examples()
    }
}

/// A session at the machine to end.
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SessionOf {
    pub name: String,
    pub session: String,
}

impl Args for SessionOf {
    const FIELDS: &'static [Field] = &[
        NAME,
        Field::text(
            "session",
            "The session, as user show gives it: its leader's number and the second it began, \
             with a dash between.",
            3,
            41,
            Some("^[0-9]+-[0-9]+$"),
        ),
    ];

    fn check(&self) -> Result<(), Bad> {
        check_name(&self.name)?;
        if parse_id(&self.session).is_none() {
            return Err(Bad {
                field: "session",
                why: "A session is two numbers with a dash between, as user show gives it. \
                      Nothing was ended."
                    .into(),
            });
        }
        Ok(())
    }
}

/// `user end NAME SESSION`: hangs up an account's session at the machine,
/// and kills what is left of it five seconds later.
pub struct End;

impl Action for End {
    const ID: &'static str = "accounts.end";
    const AREA: &'static str = AREA;
    const CHANGES: bool = true;
    const PATH: &'static str = "/api/1/accounts/{name}/end";
    const RUNS: Runs = Runs::AsAdministrator;
    const ERRORS: &'static [&'static str] = &["ACC-101", "ACC-104"];
    const NOT_SELF: Option<&'static str> = Some("name");
    const TESTS: Tests = TESTS;
    const TITLE: &'static str = "End an account's session at the machine";
    type Args = SessionOf;

    fn argv(args: &SessionOf) -> Vec<String> {
        vec!["user".into(), "end".into(), args.name.clone(), args.session.clone()]
    }

    fn subject(args: &SessionOf) -> Option<String> {
        Some(format!("{} of {}", args.session, args.name))
    }

    fn examples() -> Examples<SessionOf> {
        Examples {
            good: vec![SessionOf { name: "lab-made".into(), session: "4123-1763110000".into() }],
            bad: vec![
                json!({ "name": "lab-made", "session": "x" }),
                json!({ "name": "lab-made", "session": "4123-" }),
                json!({ "name": "lab-made" }),
            ],
            undo: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::{Caller, Found, REGISTRY, Registered, authorize, find};
    use super::*;
    use crate::admind::errors::Kind;
    use crate::admind::scopes::Scopes;
    use crate::sshkeys;

    fn action(id: &str) -> &'static dyn Registered {
        *REGISTRY.iter().find(|action| action.id() == id).unwrap()
    }

    /// Why `id` refuses `args`: the kind, the field and the words.
    fn refused(id: &str, args: Value) -> (Kind, Option<&'static str>, String) {
        let failure = action(id).prepare(args).unwrap_err();
        (failure.kind, failure.field, failure.detail)
    }

    /// What `user list --json` prints for a person: the two fields that are
    /// root's are null.
    const LIST: &str = r#"[
        {"name": "ann", "uid": 1000, "full_name": "Ann Lee", "home": "/users/ann",
         "shell": "/system/commands/nukesh", "admin": true, "console": true,
         "groups": ["admins", "ann"], "disabled": null, "password": null},
        {"name": "bob", "uid": 1001, "full_name": "", "home": "/users/bob",
         "shell": "/system/commands/nukesh", "admin": false, "console": false,
         "groups": ["bob"], "disabled": null, "password": null}
    ]"#;

    /// The rule of an account's name, as the actions say it.
    const NAME_WORDS: &str = "An account's name is 1 to 32 lowercase letters, digits, _, - and ., \
                              starting with a letter or _, and is not help. Nothing was done.";

    #[test]
    fn what_could_lock_the_caller_out_names_the_account_it_is_not_done_to() {
        let not_self = |id: &str, args: Value| action(id).prepare(args).unwrap().not_self;
        let ann = || Some("ann".to_string());
        assert_eq!(not_self("accounts.disable", json!({ "name": "ann" })), ann());
        let remove = json!({ "name": "ann", "home": "keep" });
        assert_eq!(not_self("accounts.remove", remove), ann());
        assert_eq!(not_self("accounts.end", json!({ "name": "ann", "session": "41-2100" })), ann());
        // accounts.set only when it takes the administrator right away.
        assert_eq!(not_self("accounts.set", json!({ "name": "ann", "admin": false })), ann());
        for kept in
            [json!({ "admin": true }), json!({ "full_name": "Ann" }), json!({ "console": false })]
        {
            let mut args = kept.clone();
            args["name"] = json!("ann");
            assert_eq!(not_self("accounts.set", args), None, "{kept}");
        }
        // What cannot lock the caller out is done to their own account too.
        for id in ["accounts.enable", "accounts.password", "accounts.steps-reset"] {
            assert_eq!(action(id).not_self(), None, "{id}");
        }
    }

    #[test]
    fn the_list_is_the_persons_and_the_other_reads_are_an_administrator_s() {
        for (id, runs, path, argv) in [
            ("accounts.list", Runs::AsThePerson, "/api/1/accounts", vec!["user", "list", "--json"]),
            (
                "accounts.list-all",
                Runs::AsAdministrator,
                "/api/1/accounts/list/all",
                vec!["user", "list", "--json"],
            ),
            (
                "accounts.last",
                Runs::AsAdministrator,
                "/api/1/sign-ins/last",
                vec!["lastlog", "--json"],
            ),
        ] {
            let read = action(id);
            assert_eq!(
                (read.changes(), read.runs(), read.path(), read.long()),
                (false, runs, path, false),
                "{id}"
            );
            assert!(!read.secret_answer() && !read.posts() && !read.compares(), "{id}");
            assert!(read.errors().is_empty(), "{id}");
            assert_eq!(read.prepare(json!({})).unwrap().argv, argv, "{id}");
            assert_eq!(
                read.prepare(json!({ "all": true })).unwrap_err().kind,
                Kind::BadRequest,
                "{id}"
            );
        }
        let show = action("accounts.show");
        assert_eq!(
            (show.changes(), show.runs(), show.path(), show.errors()),
            (false, Runs::AsAdministrator, "/api/1/accounts/{name}", ["ACC-101"].as_slice())
        );
        let prepared = show.prepare(json!({ "name": "ann" })).unwrap();
        assert_eq!(prepared.argv, ["user", "show", "ann", "--json"]);
        assert!(prepared.ansible.become_root && prepared.files.is_empty());
    }

    #[test]
    fn a_name_is_the_rule_of_accounts_and_help_is_not_one() {
        for name in [
            "",
            "Ann",
            "a b",
            "a/b",
            "1ann",
            "-x",
            ".x",
            "a:b",
            "help",
            "\u{e9}",
            "a\n",
            &"a".repeat(33),
        ] {
            let (kind, field, words) = refused("accounts.show", json!({ "name": name }));
            assert_eq!((kind, field), (Kind::BadValue, Some("name")), "{name:?}");
            assert_eq!(words, NAME_WORDS, "{name:?}");
        }
        for name in ["ann", "a", "_x", "a-b", "a.b", "a_1", "help2", "helper", &"a".repeat(32)] {
            assert!(action("accounts.show").prepare(json!({ "name": name })).is_ok(), "{name:?}");
        }
        assert_eq!(refused("accounts.show", json!({})).0, Kind::BadRequest);
        assert_eq!(
            refused("accounts.show", json!({ "name": "ann", "all": true })).0,
            Kind::BadRequest
        );
        // Each of those that takes a name takes it the same way.
        for id in [
            "accounts.add",
            "accounts.disable",
            "accounts.enable",
            "accounts.steps-reset",
            "accounts.password",
            "accounts.set",
            "accounts.remove",
            "accounts.key-add",
            "accounts.key-remove",
            "accounts.token-revoke",
            "accounts.end",
        ] {
            let good = action(id).example_values().0.remove(0);
            let mut args = good;
            args["name"] = json!("../x");
            assert_eq!(refused(id, args).1, Some("name"), "{id}");
        }
    }

    #[test]
    fn a_full_name_has_no_colon_or_line_break_and_at_most_256_bytes() {
        let with = |full_name: &str| json!({ "name": "ann", "full_name": full_name });
        let words = "A full name has at most 256 bytes, and no colon or line break. Nothing was \
                     done.";
        for full_name in ["a:b", "a\nb", "a\rb", &"\u{e9}".repeat(129), &"a".repeat(257)] {
            for id in ["accounts.add", "accounts.set"] {
                let (kind, field, why) = refused(id, with(full_name));
                assert_eq!((kind, field, why.as_str()), (Kind::BadValue, Some("full_name"), words));
            }
        }
        for full_name in ["", "Ann Lee", &"a".repeat(256), &"\u{e9}".repeat(128), "O'Neil, J."] {
            for id in ["accounts.add", "accounts.set"] {
                let mut args = with(full_name);
                if id == "accounts.set" {
                    args["console"] = json!(true);
                }
                assert!(action(id).prepare(args).is_ok(), "{id}: {full_name:?}");
            }
        }
    }

    #[test]
    fn an_account_is_added_with_its_two_files_and_its_password_on_the_input() {
        let add = action("accounts.add");
        assert_eq!(
            (add.changes(), add.runs(), add.long(), add.secret_answer(), add.path()),
            (true, Runs::AsAdministrator, false, false, "/api/1/accounts/add")
        );
        assert!(add.posts() && add.compares());
        assert_eq!(add.errors(), ["ACC-105"]);
        let plain = add.prepare(json!({ "name": "ann", "password": "pass word" })).unwrap();
        assert_eq!(plain.argv, ["user", "add", "ann"]);
        assert_eq!(plain.input.as_ref().map(Secret::bytes), Some(&b"pass word\n"[..]));
        assert_eq!(plain.files, ["/system/settings/passwd", "/system/settings/group"]);
        assert_eq!(plain.probe.as_deref(), Some(&["user", "list", "--json"].map(String::from)[..]));
        // The playbook gives the password from its variable and shows nothing.
        assert_eq!(plain.ansible.params["stdin"], "{{ password }}");
        assert!(plain.ansible.no_log && plain.ansible.become_root);
        let whole = add
            .prepare(json!({
                "name": "ann", "full_name": "Ann Lee", "admin": true, "password": "x"
            }))
            .unwrap();
        assert_eq!(whole.argv, ["user", "add", "--admin", "--full-name", "Ann Lee", "ann"]);
        // `admin: false` is what the command does without a flag.
        let not = add.prepare(json!({ "name": "ann", "admin": false })).unwrap();
        assert_eq!(not.argv, ["user", "add", "ann"]);
    }

    #[test]
    fn an_account_added_with_no_password_is_given_an_empty_line() {
        let add = action("accounts.add");
        let none = add.prepare(json!({ "name": "ann" })).unwrap();
        // The command asks for a password always, and an empty line is the
        // answer that says none yet.
        assert_eq!(none.input.as_ref().map(Secret::bytes), Some(&b"\n"[..]));
        // Ansible adds a line end to what it gives, and gives nothing for
        // an empty text: one line end makes the empty line.
        assert_eq!(none.ansible.params["stdin"], "\n");
        assert!(!none.ansible.no_log && none.ansible.become_root);
        assert_eq!(none.argv, ["user", "add", "ann"]);
        // A password that is empty text is the page's way to give one it has
        // not typed yet, and is refused.
        let (kind, field, _) = refused("accounts.add", json!({ "name": "ann", "password": "" }));
        assert_eq!((kind, field), (Kind::BadValue, Some("password")));
    }

    #[test]
    fn a_password_is_one_line_of_at_most_1024_bytes() {
        let words = "A password is at most 1024 bytes. Nothing was done.";
        for id in ["accounts.add", "accounts.password"] {
            let with = |password: &str| json!({ "name": "ann", "password": password });
            // 512 characters of two bytes are 1024 bytes, and 513 are not.
            assert!(action(id).prepare(with(&"\u{e9}".repeat(512))).is_ok(), "{id}");
            let (kind, field, why) = refused(id, with(&"\u{e9}".repeat(513)));
            assert_eq!((kind, field, why.as_str()), (Kind::BadValue, Some("password"), words));
            assert!(action(id).prepare(with(&"a".repeat(1023))).is_ok(), "{id}");
            // 1024 bytes of one byte each do not fit a pipe with the others.
            assert_eq!(refused(id, with(&"a".repeat(1024))).1, Some("password"), "{id}");
            for bad in ["", "two\nlines", "a\0b"] {
                assert_eq!(refused(id, with(bad)).1, Some("password"), "{id}: {bad:?}");
            }
        }
    }

    #[test]
    fn a_password_is_changed_from_the_input_and_names_the_file_of_passwords() {
        let password = action("accounts.password");
        assert_eq!(
            (password.changes(), password.runs(), password.path(), password.errors()),
            (
                true,
                Runs::AsAdministrator,
                "/api/1/accounts/{name}/password",
                ["ACC-101"].as_slice()
            )
        );
        assert!(password.posts() && !password.compares() && !password.secret_answer());
        let prepared = password.prepare(json!({ "name": "ann", "password": "new one" })).unwrap();
        assert_eq!(prepared.argv, ["passwd", "ann"]);
        assert_eq!(prepared.input.as_ref().map(Secret::bytes), Some(&b"new one\n"[..]));
        // authd's file of passwords is not in admind's view, and nothing
        // reads it to compare.
        assert!(prepared.files.is_empty() && prepared.probe.is_none());
        assert!(prepared.ansible.no_log && prepared.ansible.become_root);
        assert_eq!(password.example_values().2, None);
        assert_eq!(refused("accounts.password", json!({ "name": "ann" })).0, Kind::BadRequest);
    }

    #[test]
    fn an_account_is_changed_by_what_is_given_and_not_by_nothing() {
        let set = action("accounts.set");
        assert_eq!(
            (set.changes(), set.runs(), set.path(), set.errors()),
            (
                true,
                Runs::AsAdministrator,
                "/api/1/accounts/{name}/set",
                ["ACC-101", "ACC-102"].as_slice()
            )
        );
        assert!(set.compares());
        let argv =
            |args: Value| set.prepare(args).map(|prepared| prepared.argv).map_err(|f| f.detail);
        assert_eq!(
            argv(json!({ "name": "ann", "admin": false })),
            Ok(["user", "set", "--no-admin", "--yes", "ann"].map(String::from).to_vec())
        );
        assert_eq!(
            argv(json!({
                "name": "ann", "full_name": "Ann", "admin": true, "console": false
            })),
            Ok(["user", "set", "--full-name", "Ann", "--admin", "--no-console", "--yes", "ann"]
                .map(String::from)
                .to_vec())
        );
        assert_eq!(
            argv(json!({ "name": "ann", "console": true, "full_name": "" })),
            Ok(["user", "set", "--full-name", "", "--console", "--yes", "ann"]
                .map(String::from)
                .to_vec())
        );
        let (kind, field, why) = refused("accounts.set", json!({ "name": "ann" }));
        assert_eq!((kind, field), (Kind::BadValue, Some("full_name")));
        assert_eq!(why, "Say what to change: full_name, admin or console. Nothing was changed.");
        let prepared = set.prepare(json!({ "name": "ann", "admin": true })).unwrap();
        assert_eq!(prepared.files, ["/system/settings/passwd", "/system/settings/group"]);
        assert_eq!(
            prepared.probe.as_deref(),
            Some(&["user", "list", "--json"].map(String::from)[..])
        );
        for bad in [json!("yes"), json!(1), json!("true")] {
            let args = json!({ "name": "ann", "admin": bad });
            assert_eq!(refused("accounts.set", args).0, Kind::BadRequest);
        }
    }

    #[test]
    fn disabling_enabling_and_removing_say_what_they_run_and_the_home_is_chosen() {
        let take = |id: &str, args: Value| action(id).prepare(args).unwrap();
        assert_eq!(
            take("accounts.disable", json!({ "name": "ann" })).argv,
            ["user", "disable", "--yes", "ann"]
        );
        assert_eq!(
            take("accounts.enable", json!({ "name": "ann" })).argv,
            ["user", "enable", "ann"]
        );
        for (home, flag) in
            [("keep", "--keep-home"), ("archive", "--archive-home"), ("delete", "--delete-home")]
        {
            let prepared = take("accounts.remove", json!({ "name": "ann", "home": home }));
            assert_eq!(prepared.argv, ["user", "remove", flag, "--yes", "ann"], "{home}");
        }
        for home in ["burn", "Keep", "", "keep "] {
            let (kind, field, why) =
                refused("accounts.remove", json!({ "name": "ann", "home": home }));
            assert_eq!((kind, field), (Kind::BadValue, Some("home")), "{home:?}");
            assert_eq!(why, "home is keep, archive or delete. Nothing was removed.");
        }
        // The home is chosen by whoever asks: the command has no default.
        assert_eq!(refused("accounts.remove", json!({ "name": "ann" })).0, Kind::BadRequest);
        for (id, errors) in [
            ("accounts.disable", ["ACC-101", "ACC-102"].as_slice()),
            ("accounts.enable", ["ACC-101"].as_slice()),
            ("accounts.remove", ["ACC-101", "ACC-102"].as_slice()),
        ] {
            let action = action(id);
            assert_eq!(
                (action.changes(), action.runs(), action.errors()),
                (true, Runs::AsAdministrator, errors)
            );
        }
        // What an account had is gone with it, and nothing puts it back.
        assert_eq!(action("accounts.remove").example_values().2, None);
        let remove = take("accounts.remove", json!({ "name": "ann", "home": "keep" }));
        assert_eq!(remove.files, ["/system/settings/passwd", "/system/settings/group"]);
        assert!(remove.probe.is_some());
    }

    #[test]
    fn what_is_kept_by_authd_names_no_file_and_compares_nothing() {
        for (id, args) in [
            ("accounts.disable", json!({ "name": "ann" })),
            ("accounts.enable", json!({ "name": "ann" })),
            ("accounts.token-revoke", json!({ "name": "ann", "token": "ci" })),
            ("accounts.steps-reset", json!({ "name": "ann" })),
            ("accounts.end", json!({ "name": "ann", "session": "41-1763110000" })),
        ] {
            let prepared = action(id).prepare(args).unwrap();
            assert!(prepared.files.is_empty() && prepared.probe.is_none(), "{id}");
            assert!(!action(id).compares(), "{id}");
            assert!(prepared.ansible.become_root && !prepared.ansible.no_log, "{id}");
            assert!(prepared.input.is_none(), "{id}");
        }
    }

    #[test]
    fn a_key_is_one_line_of_a_kind_dukovo_takes_and_goes_by_its_fingerprint() {
        let add = action("accounts.key-add");
        assert_eq!(
            (add.changes(), add.runs(), add.path(), add.errors()),
            (
                true,
                Runs::AsAdministrator,
                "/api/1/accounts/{name}/keys/add",
                ["ACC-101", "ACC-103"].as_slice()
            )
        );
        let prepared = add.prepare(json!({ "name": "ann", "line": EXAMPLE_KEY })).unwrap();
        assert_eq!(prepared.argv, ["user", "key", "add", "ann", EXAMPLE_KEY]);
        // admind sees no home folder, and its probe would run as the
        // person, who may not read another account's keys: it compares
        // nothing, and a key added always did something.
        assert!(prepared.files.is_empty() && prepared.probe.is_none() && !add.compares());
        let remove = action("accounts.key-remove");
        assert_eq!(
            (remove.changes(), remove.runs(), remove.path(), remove.errors()),
            (
                true,
                Runs::AsAdministrator,
                "/api/1/accounts/{name}/keys/remove",
                ["ACC-101", "ACC-104"].as_slice()
            )
        );
        let prepared =
            remove.prepare(json!({ "name": "ann", "fingerprint": EXAMPLE_FINGERPRINT })).unwrap();
        assert_eq!(prepared.argv, ["user", "key", "remove", "ann", EXAMPLE_FINGERPRINT]);
        assert!(prepared.files.is_empty() && prepared.probe.is_none() && !remove.compares());
        let key = sshkeys::public_line(EXAMPLE_KEY).unwrap();
        assert_eq!(sshkeys::fingerprint(&key), EXAMPLE_FINGERPRINT);
        let (_, _, undo) = add.example_values();
        assert_eq!(
            undo,
            Some(Undo {
                action: "accounts.key-remove",
                args: json!({ "fingerprint": EXAMPLE_FINGERPRINT, "name": "lab-made" })
            })
        );
        assert_eq!(remove.example_values().2, None);
    }

    #[test]
    fn a_line_that_is_no_key_dukovo_takes_is_refused_with_the_words_of_acc_103() {
        let with = |line: &str| json!({ "name": "ann", "line": line });
        let ends = "Dukovo takes Ed25519 and ECDSA (P-256 and P-384) keys, as one line the way \
                    ssh-keygen writes it, with no options before the key. Nothing was added.";
        for line in [
            "",
            "   ",
            "not a key",
            "ssh-dss AAAAB3NzaC1kc3M=",
            "ssh-ed25519 !!!!",
            &format!("command=\"x\" {EXAMPLE_KEY}"),
            &format!("{EXAMPLE_KEY}\nssh-ed25519 AAAA"),
            &format!("{EXAMPLE_KEY}\u{7}"),
        ] {
            let (kind, field, why) = refused("accounts.key-add", with(line));
            assert_eq!((kind, field), (Kind::BadValue, Some("line")), "{line:?}");
            assert!(why.ends_with(ends), "{line:?}: {why}");
        }
        let (_, _, why) = refused("accounts.key-add", with(""));
        assert!(why.starts_with("That is not a public key. "), "{why}");
        let (_, _, why) =
            refused("accounts.key-add", with(&format!("command=\"x\" {EXAMPLE_KEY}")));
        assert!(why.starts_with("That line has options before its key. "), "{why}");
        let (_, _, why) = refused("accounts.key-add", with("ssh-dss AAAAB3NzaC1kc3M="));
        assert!(why.starts_with("Keys of the kind ssh-dss are not taken. "), "{why}");
        // Too long is told by its size.
        let long = format!("{EXAMPLE_KEY} {}", "x".repeat(LINE_MAX));
        let (kind, field, why) = refused("accounts.key-add", with(&long));
        assert_eq!((kind, field), (Kind::BadValue, Some("line")));
        assert_eq!(why, "A key is a line of at most 16384 bytes. Nothing was added.");
        for line in [
            EXAMPLE_KEY,
            "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIAABAgMEBQYHCAkKCwwNDg8QERITFBUWFxgZGhscHR4f",
            &format!("  {EXAMPLE_KEY}  "),
        ] {
            assert!(action("accounts.key-add").prepare(with(line)).is_ok(), "{line:?}");
        }
    }

    #[test]
    fn a_fingerprint_is_sha256_and_43_characters() {
        let words = "A fingerprint is SHA256: and 43 characters, as user show gives it. Nothing \
                     was removed.";
        let with = |fingerprint: &str| json!({ "name": "ann", "fingerprint": fingerprint });
        for bad in [
            String::new(),
            "SHA256:short".to_string(),
            format!("SHA256:{}", "A".repeat(44)),
            format!("SHA256:{}!", "A".repeat(42)),
            format!("sha256:{}", "A".repeat(43)),
            format!("MD5:{}", "A".repeat(43)),
            format!("SHA256:{}\n", "A".repeat(42)),
        ] {
            let (kind, field, why) = refused("accounts.key-remove", with(&bad));
            assert_eq!((kind, field, why.as_str()), (Kind::BadValue, Some("fingerprint"), words));
        }
        for good in [EXAMPLE_FINGERPRINT.to_string(), format!("SHA256:{}", "a+/".repeat(14) + "A")]
        {
            assert!(action("accounts.key-remove").prepare(with(&good)).is_ok(), "{good}");
        }
    }

    #[test]
    fn a_token_is_one_authd_names_and_a_session_is_two_numbers() {
        let token = |token: &str| json!({ "name": "ann", "token": token });
        let words = "A token's name is 1 to 32 of a to z, 0 to 9 and -. Nothing was removed.";
        for bad in ["", "Ansible", "a_b", "a b", "a.b", &"a".repeat(33)] {
            let (kind, field, why) = refused("accounts.token-revoke", token(bad));
            assert_eq!((kind, field, why.as_str()), (Kind::BadValue, Some("token"), words));
        }
        for good in ["ansible", "ci-2", "a", "-", &"a".repeat(32)] {
            let prepared = action("accounts.token-revoke").prepare(token(good)).unwrap();
            assert_eq!(prepared.argv, ["user", "token", "remove", "ann", good]);
        }
        assert_eq!(action("accounts.token-revoke").errors(), ["ACC-101", "ACC-104"]);
        assert_eq!(action("accounts.token-revoke").path(), "/api/1/accounts/{name}/tokens/remove");
        assert_eq!(action("accounts.steps-reset").errors(), ["ACC-101"]);
        assert_eq!(
            action("accounts.steps-reset").path(),
            "/api/1/accounts/{name}/second-step/reset"
        );
        let session = |session: &str| json!({ "name": "ann", "session": session });
        let words = "A session is two numbers with a dash between, as user show gives it. \
                     Nothing was ended.";
        for bad in [
            "x",
            "41",
            "41-",
            "-5",
            "+41-5",
            "41--5",
            "41-5-6",
            "4 1-5",
            " 41-5",
            "41-5\n",
            "99999999999999999999-1",
        ] {
            let (kind, field, why) = refused("accounts.end", session(bad));
            assert_eq!((kind, field, why.as_str()), (Kind::BadValue, Some("session"), words));
        }
        for good in ["41-1763110000", "1-1", "4123-1763110000"] {
            let prepared = action("accounts.end").prepare(session(good)).unwrap();
            assert_eq!(prepared.argv, ["user", "end", "ann", good]);
        }
        assert_eq!(action("accounts.end").errors(), ["ACC-101", "ACC-104"]);
    }

    #[test]
    fn the_probe_picks_the_account_s_row_and_an_account_that_is_not_there_is_none() {
        let add = action("accounts.add");
        let ann = json!({ "name": "ann", "password": "x" });
        let picked = add.probed(&ann, LIST).unwrap();
        let row: Value = serde_json::from_str(&picked).unwrap();
        assert_eq!(row["name"], "ann");
        assert_eq!(row["admin"], true);
        assert_eq!(add.probed(&json!({ "name": "zed" }), LIST), Some("no such account".into()));
        // Another account's change is not this one's.
        let other = LIST.replacen("\"full_name\": \"\"", "\"full_name\": \"Bob\"", 1);
        assert_ne!(other, LIST);
        assert_eq!(add.probed(&ann, &other), Some(picked.clone()));
        // A change to the account moves what is picked.
        let demoted = LIST.replacen("\"admin\": true", "\"admin\": false", 1);
        assert_ne!(add.probed(&ann, &demoted), Some(picked.clone()));
        let set = action("accounts.set");
        assert_eq!(
            set.probed(&json!({ "name": "ann", "console": true }), LIST),
            Some(picked.clone())
        );
        assert_eq!(
            action("accounts.remove").probed(&json!({ "name": "ann", "home": "keep" }), LIST),
            Some(picked)
        );
        assert_eq!(
            action("accounts.remove").probed(&json!({ "name": "ann", "home": "keep" }), "[]"),
            Some("no such account".into())
        );
        // What does not read as the list says nothing.
        for output in ["", "not json", "{}", r#"{"name": "ann"}"#, "3"] {
            assert_eq!(add.probed(&ann, output), None, "{output:?}");
        }
        assert_eq!(add.probed(&json!({ "name": 3 }), LIST), None);
    }

    #[test]
    fn the_routes_are_found_by_their_methods_and_every_name_is_an_account_s() {
        for (post, path, id) in [
            (false, "/api/1/accounts", "accounts.list"),
            (false, "/api/1/accounts/list/all", "accounts.list-all"),
            (false, "/api/1/sign-ins/last", "accounts.last"),
            (false, "/api/1/accounts/ann", "accounts.show"),
            (true, "/api/1/accounts/add", "accounts.add"),
            (true, "/api/1/accounts/ann/set", "accounts.set"),
            (true, "/api/1/accounts/ann/password", "accounts.password"),
            (true, "/api/1/accounts/ann/disable", "accounts.disable"),
            (true, "/api/1/accounts/ann/enable", "accounts.enable"),
            (true, "/api/1/accounts/ann/remove", "accounts.remove"),
            (true, "/api/1/accounts/ann/keys/add", "accounts.key-add"),
            (true, "/api/1/accounts/ann/keys/remove", "accounts.key-remove"),
            (true, "/api/1/accounts/ann/tokens/remove", "accounts.token-revoke"),
            (true, "/api/1/accounts/ann/second-step/reset", "accounts.steps-reset"),
            (true, "/api/1/accounts/ann/end", "accounts.end"),
        ] {
            let found = find(REGISTRY, post, path);
            assert!(
                matches!(&found, Found::Action(action, _) if action.id() == id),
                "{path}: {found:?}"
            );
        }
        let Found::Action(_, params) = find(REGISTRY, false, "/api/1/accounts/ann") else {
            panic!()
        };
        assert_eq!(Value::Object(params), json!({ "name": "ann" }));
        // The last sign-ins have an address of their own, so that every
        // name, last and add too, is an account's.
        for name in ["last", "add"] {
            let path = format!("/api/1/accounts/{name}");
            let Found::Action(action, params) = find(REGISTRY, false, &path) else { panic!() };
            assert_eq!(
                (action.id(), Value::Object(params)),
                ("accounts.show", json!({ "name": name }))
            );
        }
        assert!(matches!(find(REGISTRY, true, "/api/1/accounts"), Found::WrongMethod));
        assert!(matches!(find(REGISTRY, false, "/api/1/accounts/ann/set"), Found::WrongMethod));
    }

    #[test]
    fn a_token_needs_the_area_and_the_reads_of_an_administrator_need_all_of_it() {
        let call = |list: &str, id: &str| {
            let scopes = Scopes::parse(list).unwrap();
            authorize(&Caller::Token { name: "ci", scopes: &scopes }, action(id))
        };
        assert!(call("accounts:read", "accounts.list").is_ok());
        for id in ["accounts.list-all", "accounts.last", "accounts.show"] {
            assert_eq!(call("accounts:read", id).unwrap_err().kind, Kind::OutOfScope, "{id}");
            assert!(call("accounts", id).is_ok(), "{id}");
        }
        for id in [
            "accounts.add",
            "accounts.set",
            "accounts.password",
            "accounts.disable",
            "accounts.enable",
            "accounts.remove",
            "accounts.key-add",
            "accounts.key-remove",
            "accounts.token-revoke",
            "accounts.steps-reset",
            "accounts.end",
        ] {
            assert_eq!(call("accounts:read", id).unwrap_err().kind, Kind::OutOfScope, "{id}");
            assert_eq!(call("account", id).unwrap_err().kind, Kind::OutOfScope, "{id}");
            assert!(call("accounts", id).is_ok(), "{id}");
            assert!(call("*", id).is_ok(), "{id}");
        }
    }

    #[test]
    fn an_administrator_runs_what_runs_as_one_and_a_person_the_list() {
        let session = |admin: bool, unlocked: bool, id: &str| {
            authorize(&Caller::Session { name: "ann", admin, unlocked }, action(id))
                .map_err(|f| f.kind)
        };
        assert_eq!(session(false, false, "accounts.list"), Ok(()));
        for id in [
            "accounts.list-all",
            "accounts.last",
            "accounts.show",
            "accounts.add",
            "accounts.set",
            "accounts.password",
            "accounts.disable",
            "accounts.enable",
            "accounts.remove",
            "accounts.key-add",
            "accounts.key-remove",
            "accounts.token-revoke",
            "accounts.steps-reset",
            "accounts.end",
        ] {
            assert_eq!(session(false, true, id), Err(Kind::NotAdmin), "{id}");
            assert_eq!(session(true, false, id), Err(Kind::Locked), "{id}");
            assert_eq!(session(true, true, id), Ok(()), "{id}");
        }
    }

    #[test]
    fn the_examples_are_what_the_boot_and_lab_tests_run() {
        let (good, bad, undo) = action("accounts.add").example_values();
        assert_eq!(
            good,
            [
                json!({ "name": "lab-made", "full_name": "Made by the test", "password": "(secret)" })
            ]
        );
        assert_eq!(
            undo,
            Some(Undo {
                action: "accounts.remove",
                args: json!({ "name": "lab-made", "home": "delete" })
            })
        );
        assert_eq!(bad, [json!({ "name": "Bad Name" })]);
        assert!(action("accounts.add").prepare(bad[0].clone()).is_err());
        let (good, bad, undo) = action("accounts.set").example_values();
        assert_eq!(good, [json!({ "name": "lab-made", "console": false })]);
        assert_eq!(
            undo,
            Some(Undo {
                action: "accounts.set",
                args: json!({ "name": "lab-made", "console": true })
            })
        );
        for args in bad {
            assert!(action("accounts.set").prepare(args.clone()).is_err(), "{args}");
        }
        for (id, put_back) in
            [("accounts.disable", "accounts.enable"), ("accounts.enable", "accounts.disable")]
        {
            let (good, bad, undo) = action(id).example_values();
            assert_eq!(good, [json!({ "name": "lab-made" })], "{id}");
            assert_eq!(
                undo,
                Some(Undo { action: put_back, args: json!({ "name": "lab-made" }) }),
                "{id}"
            );
            for args in bad {
                assert!(action(id).prepare(args.clone()).is_err(), "{id}: {args}");
            }
        }
        for id in ["accounts.show", "accounts.steps-reset"] {
            let (good, bad, undo) = action(id).example_values();
            assert_eq!((good, undo), (vec![json!({ "name": "lab-made" })], None), "{id}");
            for args in bad {
                assert!(action(id).prepare(args.clone()).is_err(), "{id}: {args}");
            }
        }
        // The key and the token and the session are the boot test's to make.
        for id in [
            "accounts.key-add",
            "accounts.key-remove",
            "accounts.token-revoke",
            "accounts.end",
            "accounts.remove",
            "accounts.password",
        ] {
            let (good, bad, _) = action(id).example_values();
            assert_eq!(good.len(), 1, "{id}");
            assert_eq!(good[0]["name"], "lab-made", "{id}");
            for args in bad {
                // The two keys the schema cannot refuse are the code's.
                assert!(action(id).prepare(args.clone()).is_err(), "{id}: {args}");
            }
        }
    }
}
