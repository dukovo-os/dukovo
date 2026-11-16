// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The SSH actions (ADR 0562): sshd's settings and whether it is on, with
//! a window to undo a change in. The area is `ssh`, the scope a token
//! needs.
//!
//! `ssh.show` runs `ssh-server show --json` as the person, who may read
//! what it prints: sshd's file is readable by all, and whether sshd runs
//! is Spark's to say to anyone. The changes run `ssh-server set`, `on`,
//! `off`, `keep` and `undo` as an administrator, as `elevate ssh-server
//! ...` does at a terminal: the file, the service file and Spark's control
//! socket are root's.
//!
//! `ssh.set` builds the words `ssh-server set` takes after `set` and
//! checks them with the command's own reader ([`set::change`]), so that
//! the page, the API and the terminal refuse the same words. A name in a
//! list is an account's or a group's, by [`accounts::check_name`]'s rule;
//! whether the machine has such an account or group is the command's to
//! say when it runs, as it has the lists. With `try` a change is tried:
//! its argv ends with `--try SECONDS`, and Spark undoes it after that many
//! seconds unless `ssh.keep` keeps it (the trial `ssh`). While it is
//! tried every `ssh-server` change refuses with NET-182, and a file with a
//! mistake in it refuses `set` with NET-183.
//!
//! A change says whether it changed anything by reading `ssh-server show
//! --json` before and after, and comparing what the change moves: the
//! settings the file gives, or whether sshd is on.

use accounts::sshd_config::PATH as SSHD_CONFIG;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use super::trial::{TRY, check_try, refusal, try_words};
use super::{Action, Args, Bad, Examples, Field, Nothing, Runs, Tests, Trial, Undo};
use crate::servicefiles;
use crate::ssh_server::TRIAL;
use crate::ssh_server::edit::{Change, Names};
use crate::ssh_server::set;

/// The area of these actions: the scope a token needs, `ssh`.
const AREA: &str = "ssh";

/// The tests of these actions, which `lab/actions.toml` names.
const TESTS: Tests = Tests {
    checks: "admind::actions::ssh::tests",
    route: "admin-security",
    same_as_command: "admin-security",
    browser: "web-console/security",
    ansible: "web-console/ansible",
};

/// The words `passwords` and `keys` are.
const YES_NO: [&str; 2] = ["yes", "no"];

/// The word that stands for no list, and for everyone.
const EVERYONE: &str = "everyone";

/// The most names of a list: the command takes as many as are asked, and
/// a page of accounts is not a longer list.
const NAMES_MOST: usize = 32;

/// The most characters of a list of names: the most names, with one space
/// between them.
const LIST_MAX: usize = NAMES_MOST * (accounts::NAME_MAX + 1) - 1;

/// What a list of names matches: names of the shape `accounts::check_name`
/// takes, with one space between them. The check is the exact rule.
const LIST_PATTERN: &str = "^[a-z_][a-z0-9_.-]*( [a-z_][a-z0-9_.-]*)*$";

/// What of the settings `ssh-server show --json` gives a change of the
/// file moves: every setting of the file, and what the file reads wrong.
const SETTINGS: [&str; 9] = [
    "passwords",
    "keys",
    "allow_users",
    "allow_groups",
    "deny_users",
    "deny_groups",
    "forwarding",
    "gateway_ports",
    "mistakes",
];

/// The command that reads what a change moves.
fn show_argv() -> Vec<String> {
    ["ssh-server", "show", "--json"].map(String::from).to_vec()
}

/// The members `keys` of the document `output` holds, as text to compare:
/// none when it is no document or lacks one, or when a member is `null`
/// where it is a state the command could not tell (`on`).
fn picked(output: &str, keys: &[&str], null_says_nothing: bool) -> Option<String> {
    let document: Value = serde_json::from_str(output).ok()?;
    let mut picked = Map::new();
    for key in keys {
        let value = document.get(*key)?;
        if null_says_nothing && value.is_null() {
            return None;
        }
        picked.insert((*key).to_string(), value.clone());
    }
    Some(Value::Object(picked).to_string())
}

/// `ssh-server show --json`: whether sshd is on and runs, and what its
/// file says.
pub struct Show;

impl Action for Show {
    const ID: &'static str = "ssh.show";
    const AREA: &'static str = AREA;
    const CHANGES: bool = false;
    const PATH: &'static str = "/api/1/ssh";
    const RUNS: Runs = Runs::AsThePerson;
    const TESTS: Tests = TESTS;
    const TITLE: &'static str = "Show SSH's settings";
    type Args = Nothing;

    fn argv(_: &Nothing) -> Vec<String> {
        show_argv()
    }

    fn examples() -> Examples<Nothing> {
        Examples {
            good: vec![Nothing {}],
            bad: vec![json!({ "json": true }), json!({ "passwords": "no" })],
            undo: None,
        }
    }
}

/// What to set in sshd's file: the settings given, and the rest left as
/// they are.
#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Setting {
    /// `yes` or `no`: whether a password signs a person in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub passwords: Option<String>,
    /// `yes` or `no`: whether a key signs a person in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keys: Option<String>,
    /// `everyone`, or the accounts that may sign in, with a space between
    /// them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allow_users: Option<String>,
    /// `everyone`, or the groups that may sign in, with a space between
    /// them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allow_groups: Option<String>,
    /// How long the change is tried for, in seconds.
    #[serde(default, rename = "try", skip_serializing_if = "Option::is_none")]
    pub seconds: Option<i64>,
}

impl Setting {
    /// The words `ssh-server set` takes after `set`, in the order its
    /// usage gives: passwords, keys, and the two lists.
    fn words(&self) -> Vec<String> {
        let mut words = Vec::new();
        for (keyword, value) in [("passwords", &self.passwords), ("keys", &self.keys)] {
            if let Some(value) = value {
                words.extend([keyword.to_string(), value.clone()]);
            }
        }
        for (keyword, value) in
            [("allow-users", &self.allow_users), ("allow-groups", &self.allow_groups)]
        {
            if let Some(value) = value {
                words.push(keyword.to_string());
                words.extend(value.split(' ').map(String::from));
            }
        }
        words
    }
}

/// The refusal of `field` for a value of `passwords` or `keys` that is not
/// yes or no.
fn check_flag(field: &'static str, value: Option<&String>) -> Result<(), Bad> {
    match value {
        Some(value) if !YES_NO.contains(&value.as_str()) => {
            Err(Bad { field, why: refusal(&format!("The value of {field} is yes or no")) })
        }
        _ => Ok(()),
    }
}

/// Checks the list `value` of `field`, which `keyword` begins in the
/// command: `everyone`, or at most [`NAMES_MOST`] names that are
/// [`accounts::check_name`]'s, each once; `of` makes the change that sets
/// it. What the command reads from the words is what was asked.
fn check_list(
    field: &'static str,
    keyword: &str,
    value: &str,
    of: fn(Names) -> Change,
) -> Result<(), Bad> {
    let refused = |why: &str| Bad { field, why: refusal(why) };
    let names: Vec<String> = value.split(' ').map(String::from).collect();
    if value != EVERYONE {
        if names.len() > NAMES_MOST {
            return Err(refused(&format!("At most {NAMES_MOST} names go in {field}")));
        }
        for name in &names {
            accounts::check_name(name).map_err(|why| refused(&why))?;
        }
    }
    let mut words = vec![keyword.to_string()];
    words.extend(names.iter().cloned());
    let asked = if value == EVERYONE { Names::Everyone } else { Names::Only(names) };
    match set::change(&words) {
        Ok(change) if change == of(asked) => Ok(()),
        Ok(_) => Err(refused(&format!("{field} has a name that the command reads as a word"))),
        Err(why) => Err(refused(&why)),
    }
}

impl Args for Setting {
    const FIELDS: &'static [Field] = &[
        Field::one_of("passwords", "Whether a password signs a person in: yes or no.", &YES_NO)
            .optional(),
        Field::one_of("keys", "Whether an SSH key signs a person in: yes or no.", &YES_NO)
            .optional(),
        Field::text(
            "allow_users",
            "Who may sign in, by account: everyone, or up to 32 account names with a space \
             between them.",
            1,
            LIST_MAX,
            Some(LIST_PATTERN),
        )
        .optional(),
        Field::text(
            "allow_groups",
            "Who may sign in, by group: everyone, or up to 32 group names with a space between \
             them.",
            1,
            LIST_MAX,
            Some(LIST_PATTERN),
        )
        .optional(),
        TRY,
    ];

    fn check(&self) -> Result<(), Bad> {
        if self.passwords.is_none()
            && self.keys.is_none()
            && self.allow_users.is_none()
            && self.allow_groups.is_none()
        {
            let why = "Nothing to change: say at least one of passwords, keys, allow_users and \
                       allow_groups.";
            return Err(Bad { field: "passwords", why: why.into() });
        }
        check_flag("passwords", self.passwords.as_ref())?;
        check_flag("keys", self.keys.as_ref())?;
        if self.passwords.as_deref() == Some("no") && self.keys.as_deref() == Some("no") {
            let why = "Nobody could sign in over SSH: leave passwords or keys on, or turn SSH off";
            return Err(Bad { field: "keys", why: refusal(why) });
        }
        if let Some(users) = &self.allow_users {
            check_list("allow_users", "allow-users", users, |names| Change {
                allow_users: Some(names),
                ..Change::default()
            })?;
        }
        if let Some(groups) = &self.allow_groups {
            check_list("allow_groups", "allow-groups", groups, |names| Change {
                allow_groups: Some(names),
                ..Change::default()
            })?;
        }
        check_try(self.seconds)?;
        Ok(())
    }
}

/// `ssh-server set [passwords yes|no] [keys yes|no] [allow-users ...]
/// [allow-groups ...] [--try SECONDS]`: changes sshd's file, and has sshd
/// read it, at once or tried.
pub struct SetThem;

impl Action for SetThem {
    const ID: &'static str = "ssh.set";
    const AREA: &'static str = AREA;
    const CHANGES: bool = true;
    const PATH: &'static str = "/api/1/ssh";
    const RUNS: Runs = Runs::AsAdministrator;
    const ERRORS: &'static [&'static str] = &["NET-182", "NET-183"];
    const TESTS: Tests = TESTS;
    const TITLE: &'static str = "Set SSH's settings";
    type Args = Setting;

    fn argv(args: &Setting) -> Vec<String> {
        let mut argv = vec!["ssh-server".to_string(), "set".into()];
        argv.extend(args.words());
        argv.extend(try_words(args.seconds));
        argv
    }

    fn files(_: &Setting) -> Vec<String> {
        vec![SSHD_CONFIG.into()]
    }

    fn probe(_: &Setting) -> Option<Vec<String>> {
        Some(show_argv())
    }

    /// What the file says: every setting of it, and what does not read.
    fn probed(_: &Setting, output: &str) -> Option<String> {
        picked(output, &SETTINGS, false)
    }

    fn trial(args: &Setting) -> Option<Trial> {
        args.seconds.map(|_| Trial::Begins(TRIAL))
    }

    fn examples() -> Examples<Setting> {
        Examples {
            good: vec![Setting {
                passwords: Some("no".into()),
                seconds: Some(30),
                ..Setting::default()
            }],
            bad: vec![
                json!({}),
                json!({ "passwords": "maybe" }),
                json!({ "passwords": "no", "keys": "no" }),
                json!({ "allow_users": "../x" }),
                json!({ "allow_users": "everyone alice" }),
                json!({ "allow_users": "alice alice" }),
                json!({ "allow_users": "alice keys yes" }),
                json!({ "allow_groups": "Admins" }),
                json!({ "passwords": "no", "try": 5 }),
            ],
            undo: Some(Undo { action: "ssh.undo", args: json!({}) }),
        }
    }
}

/// `ssh-server on`: turns SSH on, as `service sshd enable` does.
pub struct On;

/// sshd's service file, which `on` and `off` write.
fn service_file() -> String {
    format!("{}/sshd.toml", servicefiles::DIR)
}

impl Action for On {
    const ID: &'static str = "ssh.on";
    const AREA: &'static str = AREA;
    const CHANGES: bool = true;
    const PATH: &'static str = "/api/1/ssh/on";
    const RUNS: Runs = Runs::AsAdministrator;
    const ERRORS: &'static [&'static str] = &["NET-182"];
    const TESTS: Tests = TESTS;
    const TITLE: &'static str = "Turn SSH on";
    type Args = Nothing;

    fn argv(_: &Nothing) -> Vec<String> {
        ["ssh-server", "on"].map(String::from).to_vec()
    }

    fn files(_: &Nothing) -> Vec<String> {
        vec![service_file()]
    }

    fn probe(_: &Nothing) -> Option<Vec<String>> {
        Some(show_argv())
    }

    /// Whether SSH is on: what `on` and `off` move.
    fn probed(_: &Nothing, output: &str) -> Option<String> {
        picked(output, &["on"], true)
    }

    fn examples() -> Examples<Nothing> {
        Examples {
            good: vec![Nothing {}],
            bad: vec![json!({ "try": 30 }), json!({ "on": true })],
            undo: Some(Undo { action: "ssh.off", args: json!({}) }),
        }
    }
}

/// What `off` takes: how long to try it for, if it is tried.
#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Trying {
    /// How long the change is tried for, in seconds.
    #[serde(default, rename = "try", skip_serializing_if = "Option::is_none")]
    pub seconds: Option<i64>,
}

impl Args for Trying {
    const FIELDS: &'static [Field] = &[TRY];

    fn check(&self) -> Result<(), Bad> {
        check_try(self.seconds)?;
        Ok(())
    }
}

/// `ssh-server off [--try SECONDS]`: turns SSH off, at once or tried.
pub struct Off;

impl Action for Off {
    const ID: &'static str = "ssh.off";
    const AREA: &'static str = AREA;
    const CHANGES: bool = true;
    const PATH: &'static str = "/api/1/ssh/off";
    const RUNS: Runs = Runs::AsAdministrator;
    const ERRORS: &'static [&'static str] = &["NET-182"];
    const TESTS: Tests = TESTS;
    const TITLE: &'static str = "Turn SSH off";
    type Args = Trying;

    fn argv(args: &Trying) -> Vec<String> {
        let mut argv = ["ssh-server", "off"].map(String::from).to_vec();
        argv.extend(try_words(args.seconds));
        argv
    }

    fn files(_: &Trying) -> Vec<String> {
        vec![service_file()]
    }

    fn probe(_: &Trying) -> Option<Vec<String>> {
        Some(show_argv())
    }

    fn probed(_: &Trying, output: &str) -> Option<String> {
        picked(output, &["on"], true)
    }

    fn trial(args: &Trying) -> Option<Trial> {
        args.seconds.map(|_| Trial::Begins(TRIAL))
    }

    fn examples() -> Examples<Trying> {
        Examples {
            good: vec![Trying::default()],
            bad: vec![
                json!({ "try": 5 }),
                json!({ "try": 3601 }),
                json!({ "try": "30" }),
                json!({ "on": false }),
            ],
            undo: Some(Undo { action: "ssh.on", args: json!({}) }),
        }
    }
}

/// `ssh-server keep`: keeps the change of SSH's settings that is being
/// tried.
pub struct Keep;

impl Action for Keep {
    const ID: &'static str = "ssh.keep";
    const AREA: &'static str = AREA;
    const CHANGES: bool = true;
    const PATH: &'static str = "/api/1/ssh/trial/keep";
    const RUNS: Runs = Runs::AsAdministrator;
    const ERRORS: &'static [&'static str] = &["NET-179"];
    const TESTS: Tests = TESTS;
    const TITLE: &'static str = "Keep SSH's settings";
    type Args = Nothing;

    fn argv(_: &Nothing) -> Vec<String> {
        ["ssh-server", "keep"].map(String::from).to_vec()
    }

    fn trial(_: &Nothing) -> Option<Trial> {
        Some(Trial::Ends(TRIAL))
    }

    fn examples() -> Examples<Nothing> {
        // What puts it back is the change that was tried, which Spark
        // undoes, or `ssh.undo`.
        Examples { good: vec![Nothing {}], bad: vec![json!({ "id": "x" })], undo: None }
    }
}

/// `ssh-server undo`: undoes the change of SSH's settings that is being
/// tried.
pub struct Revert;

impl Action for Revert {
    const ID: &'static str = "ssh.undo";
    const AREA: &'static str = AREA;
    const CHANGES: bool = true;
    const PATH: &'static str = "/api/1/ssh/trial/undo";
    const RUNS: Runs = Runs::AsAdministrator;
    const ERRORS: &'static [&'static str] = &["NET-179"];
    const TESTS: Tests = TESTS;
    const TITLE: &'static str = "Undo SSH's settings";
    type Args = Nothing;

    fn argv(_: &Nothing) -> Vec<String> {
        ["ssh-server", "undo"].map(String::from).to_vec()
    }

    /// The trial's file: sshd_config for a tried `set`, sshd's service
    /// file for a tried `off`.
    fn files(_: &Nothing) -> Vec<String> {
        vec![SSHD_CONFIG.into(), service_file()]
    }

    fn probe(_: &Nothing) -> Option<Vec<String>> {
        Some(show_argv())
    }

    /// What a trial of SSH may have moved: the settings, and whether SSH is
    /// on.
    fn probed(_: &Nothing, output: &str) -> Option<String> {
        let keys: Vec<&str> = SETTINGS.iter().copied().chain(["on"]).collect();
        picked(output, &keys, false)
    }

    fn trial(_: &Nothing) -> Option<Trial> {
        Some(Trial::Ends(TRIAL))
    }

    fn examples() -> Examples<Nothing> {
        Examples { good: vec![Nothing {}], bad: vec![json!({ "id": "x" })], undo: None }
    }
}

#[cfg(test)]
mod tests {
    use super::super::{Caller, REGISTRY, Registered, authorize};
    use super::*;
    use crate::admind::errors::Kind;
    use crate::admind::scopes::Scopes;

    /// The registered action `id`.
    fn action(id: &str) -> &'static dyn Registered {
        *REGISTRY.iter().find(|action| action.id() == id).unwrap()
    }

    /// The failure of `args` for `id`: its kind, its field and its words.
    fn refused(args: Value) -> (Kind, Option<&'static str>, String) {
        let failure = action("ssh.set").prepare(args).unwrap_err();
        (failure.kind, failure.field, failure.detail)
    }

    #[test]
    fn show_is_a_read_of_the_person_that_asks_for_json() {
        let action = action("ssh.show");
        assert_eq!(
            (action.changes(), action.runs(), action.area(), action.long()),
            (false, Runs::AsThePerson, "ssh", false)
        );
        assert_eq!(
            (action.path(), action.title(), action.posts()),
            ("/api/1/ssh", "Show SSH's settings", false)
        );
        assert!(action.errors().is_empty() && !action.compares());
        let prepared = action.prepare(json!({})).unwrap();
        assert_eq!(prepared.argv, ["ssh-server", "show", "--json"]);
        assert_eq!((prepared.probe, prepared.files.len(), prepared.trial), (None, 0, None));
        assert_eq!(prepared.ansible.changed_when.as_deref(), Some("false"));
        assert!(!prepared.ansible.become_root);
        let (good, bad, undo) = action.example_values();
        assert_eq!((good, undo), (vec![json!({})], None));
        for args in bad {
            assert_eq!(action.prepare(args).unwrap_err().kind, Kind::BadRequest);
        }
    }

    #[test]
    fn set_gives_the_command_its_words_in_the_order_of_its_usage_and_the_window_last() {
        let set = |args: Value| action("ssh.set").prepare(args).unwrap().argv;
        assert_eq!(set(json!({ "passwords": "no" })), ["ssh-server", "set", "passwords", "no"]);
        assert_eq!(
            set(json!({
                "allow_groups": "staff ops", "keys": "yes", "allow_users": "alice bob",
                "passwords": "no", "try": 30
            })),
            [
                "ssh-server",
                "set",
                "passwords",
                "no",
                "keys",
                "yes",
                "allow-users",
                "alice",
                "bob",
                "allow-groups",
                "staff",
                "ops",
                "--try",
                "30"
            ]
        );
        assert_eq!(
            set(json!({ "allow_users": "everyone" })),
            ["ssh-server", "set", "allow-users", "everyone"]
        );
        // What is built is what the command reads.
        let words: Vec<String> =
            ["passwords", "no", "allow-users", "alice", "bob"].map(String::from).to_vec();
        let change = set::change(&words).unwrap();
        assert_eq!(
            (change.passwords, change.allow_users),
            (Some(false), Some(Names::Only(vec!["alice".into(), "bob".into()])))
        );
        let prepared = action("ssh.set").prepare(json!({ "keys": "yes" })).unwrap();
        assert_eq!(prepared.args, json!({ "keys": "yes" }));
        assert_eq!(prepared.files, ["/system/settings/ssh/sshd_config"]);
        assert_eq!(prepared.ansible.name, "Set SSH's settings");
        assert!(prepared.ansible.become_root);
    }

    #[test]
    fn set_is_tried_with_a_window_and_at_once_without() {
        let action = action("ssh.set");
        assert_eq!(action.errors(), ["NET-182", "NET-183"]);
        let tried = action.prepare(json!({ "passwords": "no", "try": 30 })).unwrap();
        assert_eq!(tried.trial, Some(Trial::Begins("ssh")));
        let at_once = action.prepare(json!({ "passwords": "no" })).unwrap();
        assert_eq!(at_once.trial, None);
        let range = spark_api::trial::SECONDS;
        for seconds in [*range.start(), *range.end()] {
            assert!(action.prepare(json!({ "passwords": "no", "try": seconds })).is_ok());
        }
        for seconds in [json!(5), json!(9), json!(3601), json!(0), json!(-30)] {
            let (kind, field, words) = refused(json!({ "passwords": "no", "try": seconds }));
            assert_eq!((kind, field), (Kind::BadValue, Some("try")), "{seconds}");
            assert_eq!(words, "A change is tried for 10 to 3600 seconds. Nothing was done.");
        }
    }

    #[test]
    fn set_refuses_what_the_command_would_and_names_the_field() {
        let nothing = "Nothing to change: say at least one of passwords, keys, allow_users and \
                       allow_groups.";
        assert_eq!(refused(json!({})), (Kind::BadValue, Some("passwords"), nothing.into()));
        assert_eq!(refused(json!({ "try": 30 })).2, nothing);
        let yes_no = |field: &str| format!("The value of {field} is yes or no. Nothing was done.");
        for value in ["maybe", "", "Yes", "yes ", "true"] {
            assert_eq!(
                refused(json!({ "passwords": value })),
                (Kind::BadValue, Some("passwords"), yes_no("passwords")),
                "{value:?}"
            );
            assert_eq!(
                refused(json!({ "keys": value })),
                (Kind::BadValue, Some("keys"), yes_no("keys")),
                "{value:?}"
            );
        }
        assert_eq!(
            refused(json!({ "passwords": "no", "keys": "no" })),
            (
                Kind::BadValue,
                Some("keys"),
                "Nobody could sign in over SSH: leave passwords or keys on, or turn SSH off. \
                 Nothing was done."
                    .into()
            )
        );
        let long = "a".repeat(33);
        for (field, value, why) in [
            ("allow_users", "../x", "`../x` cannot be a name"),
            ("allow_users", "Alice", "`Alice` cannot be a name"),
            ("allow_users", "alice  bob", "a name needs a letter at least"),
            ("allow_users", " alice", "a name needs a letter at least"),
            ("allow_users", "alice ", "a name needs a letter at least"),
            ("allow_users", "everyone alice", "everyone stands alone: it takes no names beside it"),
            ("allow_users", "alice everyone", "everyone stands alone: it takes no names beside it"),
            ("allow_users", "alice alice", "alice is named twice"),
            ("allow_users", "alice keys yes", "has a name that the command reads as a word"),
            ("allow_groups", "staff passwords", "passwords takes yes or no"),
            ("allow_groups", "Staff", "`Staff` cannot be a name"),
            ("allow_groups", long.as_str(), "a name has at most 32 characters"),
            ("allow_groups", "", "a name needs a letter at least"),
        ] {
            let (kind, got, words) = refused(json!({ field: value }));
            assert_eq!((kind, got), (Kind::BadValue, Some(field)), "{field} {value:?}");
            // A refusal starts with a capital.
            assert!(
                words.to_lowercase().contains(&why.to_lowercase()),
                "{field} {value:?}: {words}"
            );
            assert!(words.ends_with(". Nothing was done."), "{words}");
        }
        let many: Vec<String> = (0..=NAMES_MOST).map(|n| format!("u{n}")).collect();
        let (kind, field, words) = refused(json!({ "allow_users": many.join(" ") }));
        assert_eq!((kind, field), (Kind::BadValue, Some("allow_users")));
        assert_eq!(words, "At most 32 names go in allow_users. Nothing was done.");
        let most: Vec<String> = (0..NAMES_MOST).map(|n| format!("u{n}")).collect();
        assert!(action("ssh.set").prepare(json!({ "allow_users": most.join(" ") })).is_ok());
        let names: Vec<String> = (10..10 + NAMES_MOST)
            .map(|n| format!("{}{n}", "a".repeat(accounts::NAME_MAX - 2)))
            .collect();
        let longest = names.join(" ");
        assert_eq!(longest.len(), LIST_MAX);
        assert!(action("ssh.set").prepare(json!({ "allow_groups": longest })).is_ok());
        // A value of the wrong type is the types' refusal.
        for args in [
            json!({ "passwords": true }),
            json!({ "allow_users": ["alice"] }),
            json!({ "keys": null, "allow_users": 7 }),
            json!({ "passwords": "no", "extra": 1 }),
            json!({ "passwords": "no", "try": "30" }),
        ] {
            assert_eq!(
                action("ssh.set").prepare(args.clone()).unwrap_err().kind,
                Kind::BadRequest,
                "{args}"
            );
        }
    }

    #[test]
    fn set_compares_what_the_file_says_and_the_pattern_is_the_names_rule() {
        let action = action("ssh.set");
        assert!(action.compares());
        let prepared = action.prepare(json!({ "passwords": "no" })).unwrap();
        assert_eq!(
            prepared.probe.as_deref(),
            Some(&["ssh-server".to_string(), "show".into(), "--json".into()][..])
        );
        let printed = |passwords: bool, running: bool, trial: &str| {
            format!(
                r#"{{"format":1,"on":true,"running":{running},"port":22,"passwords":{passwords},"keys":true,"allow_users":[],"allow_groups":[],"deny_users":[],"deny_groups":[],"forwarding":"remote","gateway_ports":"no","mistakes":[],"host_key":"SHA256:x","trial":{trial}}}"#
            )
        };
        let picked = |output: &str| action.probed(&prepared.args, output);
        assert!(picked(&printed(true, true, "null")).is_some());
        assert_ne!(picked(&printed(true, true, "null")), picked(&printed(false, true, "null")));
        // Whether sshd runs, and the time left of a trial, are not settings.
        assert_eq!(
            picked(&printed(true, true, "null")),
            picked(&printed(true, false, r#"{"left_ms":4000}"#))
        );
        for output in ["", "{}", "[]", "not json", r#"{"passwords":true}"#] {
            assert_eq!(picked(output), None, "{output:?}");
        }
        // The pattern lets through what `check_name` takes and no more of
        // its shape: a name made of its characters, with one space between.
        let pattern = |text: &str| {
            let first = |c: char| c.is_ascii_lowercase() || c == '_';
            text.split(' ').all(|name| {
                name.starts_with(first)
                    && name.chars().all(|c| matches!(c, 'a'..='z' | '0'..='9' | '_' | '-' | '.'))
            })
        };
        for good in ["alice", "a_b.c-d", "_x", "alice bob"] {
            assert!(pattern(good) && accounts::check_name(good.split(' ').next().unwrap()).is_ok());
        }
        assert_eq!(LIST_PATTERN, "^[a-z_][a-z0-9_.-]*( [a-z_][a-z0-9_.-]*)*$");
    }

    #[test]
    fn on_and_off_write_the_service_file_and_compare_whether_ssh_is_on() {
        for (id, path, title, word) in [
            ("ssh.on", "/api/1/ssh/on", "Turn SSH on", "on"),
            ("ssh.off", "/api/1/ssh/off", "Turn SSH off", "off"),
        ] {
            let action = action(id);
            assert_eq!(
                (action.changes(), action.runs(), action.area(), action.long()),
                (true, Runs::AsAdministrator, "ssh", false),
                "{id}"
            );
            assert_eq!((action.path(), action.title(), action.posts()), (path, title, true));
            assert_eq!(action.errors(), ["NET-182"], "{id}");
            assert!(action.compares(), "{id}");
            let prepared = action.prepare(json!({})).unwrap();
            assert_eq!(prepared.argv, ["ssh-server", word], "{id}");
            assert_eq!(prepared.files, ["/system/settings/spark/services/sshd.toml"]);
            assert_eq!(prepared.trial, None);
            let picked = |output: &str| action.probed(&prepared.args, output);
            assert_eq!(
                picked(r#"{"on":true,"passwords":false}"#).as_deref(),
                Some(r#"{"on":true}"#)
            );
            assert_ne!(picked(r#"{"on":true}"#), picked(r#"{"on":false}"#));
            // Not knowing whether it is on is no state to compare.
            for output in [r#"{"on":null}"#, "{}", "", "x"] {
                assert_eq!(picked(output), None, "{id} {output:?}");
            }
        }
        let (_, _, undo) = action("ssh.on").example_values();
        assert_eq!(undo, Some(Undo { action: "ssh.off", args: json!({}) }));
        let (good, _, undo) = action("ssh.off").example_values();
        assert_eq!(
            (good, undo),
            (vec![json!({})], Some(Undo { action: "ssh.on", args: json!({}) }))
        );
        // `on` takes nothing, and `off` takes the window.
        assert_eq!(
            action("ssh.on").prepare(json!({ "try": 30 })).unwrap_err().kind,
            Kind::BadRequest
        );
        let tried = action("ssh.off").prepare(json!({ "try": 30 })).unwrap();
        assert_eq!(tried.argv, ["ssh-server", "off", "--try", "30"]);
        assert_eq!(tried.trial, Some(Trial::Begins("ssh")));
        for seconds in [5, 9, 3601] {
            let failure = action("ssh.off").prepare(json!({ "try": seconds })).unwrap_err();
            assert_eq!((failure.kind, failure.field), (Kind::BadValue, Some("try")), "{seconds}");
        }
    }

    #[test]
    fn keep_and_undo_end_the_trial_as_an_administrator_and_undo_compares_the_file() {
        for (id, path, word, title) in [
            ("ssh.keep", "/api/1/ssh/trial/keep", "keep", "Keep SSH's settings"),
            ("ssh.undo", "/api/1/ssh/trial/undo", "undo", "Undo SSH's settings"),
        ] {
            let action = action(id);
            assert_eq!(
                (action.changes(), action.runs(), action.area(), action.long()),
                (true, Runs::AsAdministrator, "ssh", false),
                "{id}"
            );
            assert_eq!((action.path(), action.title(), action.posts()), (path, title, true));
            assert_eq!(action.errors(), ["NET-179"], "{id}");
            assert_eq!(action.fields().len(), 0, "{id}");
            let prepared = action.prepare(json!({})).unwrap();
            assert_eq!(prepared.argv, ["ssh-server", word], "{id}");
            assert_eq!(prepared.trial, Some(Trial::Ends("ssh")), "{id}");
            assert_eq!(action.compares(), id == "ssh.undo", "{id}");
            let (good, bad, undo) = action.example_values();
            assert_eq!((good, undo), (vec![json!({})], None), "{id}");
            for args in bad {
                assert_eq!(action.prepare(args).unwrap_err().kind, Kind::BadRequest, "{id}");
            }
        }
        // The trial holds sshd_config for a tried set, and sshd's service
        // file for a tried off.
        let prepared = action("ssh.undo").prepare(json!({})).unwrap();
        assert_eq!(
            prepared.files,
            ["/system/settings/ssh/sshd_config", "/system/settings/spark/services/sshd.toml"]
        );
        // What the trial may have moved: the settings and whether SSH is
        // on, which the settings alone do not say.
        let printed = |on: bool, passwords: bool| {
            format!(
                r#"{{"format":1,"on":{on},"running":true,"port":22,"passwords":{passwords},"keys":true,"allow_users":[],"allow_groups":[],"deny_users":[],"deny_groups":[],"forwarding":"remote","gateway_ports":"no","mistakes":[],"host_key":null,"trial":null}}"#
            )
        };
        let picked = |output: &str| action("ssh.undo").probed(&prepared.args, output);
        assert_ne!(picked(&printed(true, true)), picked(&printed(false, true)));
        assert_ne!(picked(&printed(true, true)), picked(&printed(true, false)));
        assert_eq!(picked(&printed(true, true)), picked(&printed(true, true)));
        assert_eq!(picked("{}"), None);
    }

    #[test]
    fn a_token_needs_the_ssh_scope_to_change_and_the_read_scope_to_read() {
        let call = |list: &str, id: &str| {
            let scopes = Scopes::parse(list).unwrap();
            authorize(&Caller::Token { name: "ci", scopes: &scopes }, action(id))
        };
        for list in ["ssh", "ssh:read", "*"] {
            assert!(call(list, "ssh.show").is_ok(), "{list}");
        }
        for id in ["ssh.set", "ssh.on", "ssh.off", "ssh.keep", "ssh.undo"] {
            for list in ["ssh", "*"] {
                assert!(call(list, id).is_ok(), "{id} {list}");
            }
            for list in ["ssh:read", "security", "console"] {
                let refused = call(list, id).unwrap_err();
                assert_eq!(refused.kind, Kind::OutOfScope, "{id} {list}");
                assert_eq!(
                    refused.detail,
                    "the API token \"ci\" may not do this: it needs the scope ssh or *"
                );
            }
        }
    }
}
