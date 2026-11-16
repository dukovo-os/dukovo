// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The alerts actions (ADR 0485, ADR 0566): the alerts that are active, the
//! channels they go to and how each is doing, a test through every channel,
//! the email account, the webhooks, the browsers that get alerts by Web Push,
//! and which kinds of alert each channel is told of. The area is `alerts`,
//! the scope a token needs.
//!
//! alerts(8) needs root, so every one runs as an administrator, as `elevate
//! alerts ...` does at a terminal. `reliability.alerts` reads the
//! history and stays where it is.
//!
//! The changes write alerts.toml, which is root's and mode 0600 and so not
//! one of the files admind sees, or alertd's own browsers' file: they name
//! no file and have no probe, and each says it changed something when it
//! ended well. A password, a webhook's address (which for Slack and
//! Discord is a token itself), its token and a browser's subscription are
//! secrets (ADR 0561): the command reads them on its input and no word of
//! it, of the journal or of a task holds one. Their limits are what a
//! command's input takes (4,096 bytes), which is less than alertd would
//! take of a long address or token.
//!
//! The values are checked as the command and alertd check them, so that the
//! page, the API and the terminal refuse the same values with the same
//! words. What only the command can tell, an address that is not a webhook's
//! or a subscription that is not a browser's, it says when it runs, and
//! alerts.toml is then as it was.

use serde::{Deserialize, Serialize};
use serde_json::json;

use super::backups::or_list;
use super::{Action, Args, Bad, Examples, Field, Nothing, Runs, Secret, Tests, Undo};
use crate::alertd::settings::{is_address, is_host};
use crate::alerts::protocol::ABOUT;

/// The area of these actions: the scope a token needs, `alerts`.
const AREA: &str = "alerts";

/// The tests of these actions, which `lab/actions.toml` names.
const TESTS: Tests = Tests {
    checks: "admind::actions::alerts::tests",
    route: "admin-alerts",
    same_as_command: "admin-alerts",
    browser: "web-console/alerts",
    ansible: "web-console/ansible",
};

/// What a channel is told by severity: every alert, or the critical ones.
const SEVERITIES: &[&str] = &["warning", "critical"];

/// What a webhook speaks, as alertd names the kinds: written here as it is,
/// which a test holds to alertd's own list.
const KINDS: &[&str] = &["ntfy", "matrix", "slack", "discord", "json"];

/// The ports mail is submitted on: STARTTLS and TLS from the start.
const PORTS: &[&str] = &["587", "465"];

/// What text that is one line, with no NUL, matches.
const ONE_LINE: &str = "^[^\n\r\0]+$";

/// What a host name is made of: letters, digits, dots and hyphens.
const HOST_PATTERN: &str = "^[A-Za-z0-9.-]+$";

/// What text of lines, with no NUL, matches.
const LINES_PATTERN: &str = "^[^\0]+$";

/// What the kinds a channel is told of match: `all`, or one to three of
/// the words, as `alerts channel` takes them.
const ABOUT_PATTERN: &str = "^(all|(data|stopped|hardware)(,(data|stopped|hardware)){0,2})$";

/// The most characters of the kinds a channel is told of: three words of the
/// longest, hardware, and two commas.
const ABOUT_MAX: usize = 26;

/// What a channel is called to `alerts channel`: `email`, `webhook:` and a
/// number from 1 to 32, or `push:` and a browser's ID.
const CHANNEL_PATTERN: &str = "^(email|webhook:([1-9]|[12][0-9]|3[0-2])|push:[0-9a-f]{16})$";

/// What a browser's ID matches: 16 hexadecimal digits.
const ID_PATTERN: &str = "^[0-9a-f]{16}$";

/// What a Matrix room's ID matches: `!`, a part with no colon, a colon and
/// the server.
const ROOM_PATTERN: &str = "^![^:]+:.+$";

/// The most addresses of an email channel.
const TO_COUNT: usize = 8;

/// The most characters of an address: 64 before the `@` and a host name.
const ADDRESS_MAX: usize = 318;

/// The most characters of the addresses, one a line.
const TO_MAX: usize = TO_COUNT * (ADDRESS_MAX + 1);

/// The most characters of a host name.
const HOST_MAX: usize = 253;

/// The most characters of an account's name.
const USER_MAX: usize = 256;

/// The most characters of a password: what an input line of the command
/// holds with room for the rest.
const PASSWORD_CHARS: usize = 512;

/// The most characters of a webhook's address and of its token. Both are
/// lines of one input, and four bytes a character of each fit what a
/// command's input takes.
const WEBHOOK_CHARS: usize = 500;

/// The most bytes of a browser's subscription: its address, 1,024 bytes at
/// most, and two keys.
const SUBSCRIPTION_MAX: usize = 2048;

/// The most characters of a browser's name.
const NAME_MAX: usize = 64;

/// The most webhooks alerts.toml names.
const WEBHOOKS_MOST: i64 = 32;

/// `words`, as the owned words of a command.
fn argv_of(words: &[&str]) -> Vec<String> {
    words.iter().map(|word| (*word).to_string()).collect()
}

/// A refusal of the field `field`: `why`, and that nothing was done.
fn refuse(field: &'static str, why: &str) -> Bad {
    Bad { field, why: format!("{why} Nothing was done.") }
}

/// Whether `text` is one of `words`.
fn check_word(field: &'static str, text: &str, words: &[&str]) -> Result<(), Bad> {
    if words.contains(&text) {
        return Ok(());
    }
    Err(refuse(field, &format!("{field} is {}.", or_list(words))))
}

/// Whether `text` is a line of 1 to `max` characters with no control
/// character: a name, or an account.
fn check_line(field: &'static str, text: &str, max: usize) -> Result<(), Bad> {
    let length = text.chars().count();
    if length == 0 || length > max || text.chars().any(char::is_control) {
        return Err(refuse(field, &format!("It is 1 to {max} characters on one line.")));
    }
    Ok(())
}

/// Whether `about` is what `alerts channel --about` takes: `all`, or one to
/// three of the words of the kinds, with commas.
fn check_about(about: &str) -> Result<(), Bad> {
    let words: Vec<&str> = about.split(',').collect();
    if about == "all" || ((1..=3).contains(&words.len()) && words.iter().all(|w| ABOUT.contains(w)))
    {
        return Ok(());
    }
    Err(refuse("about", &format!("about is all, or {} with commas between them.", or_list(&ABOUT))))
}

/// What every channel is told: how severe, whether clears, which kinds.
fn check_filter(severity: Option<&str>, about: Option<&str>) -> Result<(), Bad> {
    if let Some(severity) = severity {
        check_word("severity", severity, SEVERITIES)?;
    }
    if let Some(about) = about {
        check_about(about)?;
    }
    Ok(())
}

/// The words `--severity`, `--cleared` and `--about` give a command, for
/// each of the three that is given.
fn filter_argv(
    argv: &mut Vec<String>,
    severity: Option<&str>,
    cleared: Option<bool>,
    about: Option<&str>,
) {
    if let Some(severity) = severity {
        argv.extend(["--severity".to_string(), severity.to_string()]);
    }
    if let Some(cleared) = cleared {
        argv.extend(["--cleared".to_string(), (if cleared { "yes" } else { "no" }).to_string()]);
    }
    if let Some(about) = about {
        argv.extend(["--about".to_string(), about.to_string()]);
    }
}

/// The field of the least severe alert a channel is told.
const SEVERITY: Field = Field::one_of(
    "severity",
    "The least severe alert it is told: warning, which is every alert, or critical. Warning if \
     left out.",
    SEVERITIES,
)
.optional();

/// The field of whether a channel is told when an alert clears.
const CLEARED: Field =
    Field::flag("cleared", "Whether it is also told when an alert clears. True if left out.")
        .optional();

/// The field of the kinds a channel is told of.
const ABOUT_FIELD: Field = Field::text(
    "about",
    "The kinds of alert it is told of: all, or data, stopped and hardware with commas between \
     them, as data,hardware. All if left out.",
    3,
    ABOUT_MAX,
    Some(ABOUT_PATTERN),
)
.optional();

/// `alerts list --json`: the alerts that are active, a line each.
pub struct List;

impl Action for List {
    const ID: &'static str = "alerts.list";
    const AREA: &'static str = AREA;
    const CHANGES: bool = false;
    const PATH: &'static str = "/api/1/alerts";
    const RUNS: Runs = Runs::AsAdministrator;
    const LINES: bool = true;
    const TESTS: Tests = TESTS;
    const TITLE: &'static str = "List the alerts";
    type Args = Nothing;

    fn argv(_: &Nothing) -> Vec<String> {
        argv_of(&["alerts", "list", "--json"])
    }

    fn examples() -> Examples<Nothing> {
        Examples {
            good: vec![Nothing {}],
            bad: vec![json!({ "json": true }), json!({ "key": "backup-late" })],
            undo: None,
        }
    }
}

/// `alerts status --json`: each channel alerts go to, a line each, with how
/// it is doing.
pub struct Channels;

impl Action for Channels {
    const ID: &'static str = "alerts.channels";
    const AREA: &'static str = AREA;
    const CHANGES: bool = false;
    const PATH: &'static str = "/api/1/alerts/channels";
    const RUNS: Runs = Runs::AsAdministrator;
    const LINES: bool = true;
    const TESTS: Tests = TESTS;
    const TITLE: &'static str = "Show where alerts go";
    type Args = Nothing;

    fn argv(_: &Nothing) -> Vec<String> {
        argv_of(&["alerts", "status", "--json"])
    }

    fn examples() -> Examples<Nothing> {
        Examples {
            good: vec![Nothing {}],
            bad: vec![json!({ "json": true }), json!({ "channel": "email" })],
            undo: None,
        }
    }
}

/// `alerts test --json`: sends a test alert through every channel and says
/// how each went.
///
/// It acts outside the machine, so it is a change that is journaled, and
/// says it changed something, with no file and no probe. Status 1 is a
/// channel that refused, which its line says: the output is the answer.
pub struct Test;

impl Action for Test {
    const ID: &'static str = "alerts.test";
    const AREA: &'static str = AREA;
    const CHANGES: bool = true;
    const PATH: &'static str = "/api/1/alerts/test";
    const RUNS: Runs = Runs::AsAdministrator;
    const ANSWERS: &'static [i32] = &[0, 1];
    const TESTS: Tests = TESTS;
    const TITLE: &'static str = "Send a test alert";
    type Args = Nothing;

    fn argv(_: &Nothing) -> Vec<String> {
        argv_of(&["alerts", "test", "--json"])
    }

    fn examples() -> Examples<Nothing> {
        Examples {
            good: vec![Nothing {}],
            bad: vec![json!({ "channel": "email" }), json!({ "json": true })],
            undo: None,
        }
    }
}

/// An email account for alerts, with the addresses they go to.
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct EmailSet {
    /// The addresses, one on each line: 1 to 8.
    pub to: String,
    /// The provider's submission server.
    pub server: String,
    /// 587, the default, or 465.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<String>,
    /// The account to sign in with.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user: Option<String>,
    /// The sender's address.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
    /// The least severe alert it is told.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub severity: Option<String>,
    /// Whether it is told when an alert clears.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cleared: Option<bool>,
    /// The kinds it is told of.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub about: Option<String>,
    /// The account's password, which the command reads on its input. Left
    /// out, the command keeps the one the file has.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub password: Option<Secret>,
}

/// The addresses of `to`, one on each line, checked as alertd checks them.
fn addresses(to: &str) -> Result<Vec<&str>, Bad> {
    let found: Vec<&str> = to.lines().map(str::trim).filter(|line| !line.is_empty()).collect();
    if found.is_empty() || found.len() > TO_COUNT {
        return Err(refuse("to", &format!("Give 1 to {TO_COUNT} addresses, one on each line.")));
    }
    if let Some(bad) = found.iter().find(|address| !is_address(address)) {
        return Err(refuse("to", &format!("'{bad}' is not an email address.")));
    }
    Ok(found)
}

impl Args for EmailSet {
    const FIELDS: &'static [Field] = &[
        Field::text(
            "to",
            "The addresses the alerts go to: 1 to 8, one on each line.",
            3,
            TO_MAX,
            Some(LINES_PATTERN),
        ),
        Field::text(
            "server",
            "The provider's submission server, a host name such as smtp.example.com. The port \
             goes in port.",
            1,
            HOST_MAX,
            Some(HOST_PATTERN),
        ),
        Field::one_of(
            "port",
            "587 starts TLS after connecting; 465 has it from the first byte. 587 if left out.",
            PORTS,
        )
        .optional(),
        Field::text(
            "user",
            "The account to sign in with, which most providers need. Left out, there is no \
             sign-in.",
            1,
            USER_MAX,
            Some(ONE_LINE),
        )
        .optional(),
        Field::text(
            "from",
            "The sender's address. Alerts@ and the machine's name if left out.",
            3,
            ADDRESS_MAX,
            None,
        )
        .optional(),
        SEVERITY,
        CLEARED,
        ABOUT_FIELD,
        Field::secret(
            "password",
            "The account's password, or an app password the provider made. Left out, the one \
             kept is used.",
            PASSWORD_CHARS,
        )
        .optional(),
    ];

    fn check(&self) -> Result<(), Bad> {
        addresses(&self.to)?;
        if !is_host(&self.server) {
            return Err(refuse(
                "server",
                "A server is a host name of letters, digits, dots and hyphens; the port goes in \
                 port.",
            ));
        }
        if let Some(port) = &self.port {
            check_word("port", port, PORTS)?;
        }
        if let Some(user) = &self.user {
            check_line("user", user, USER_MAX)?;
        }
        if let Some(from) = &self.from
            && !is_address(from)
        {
            return Err(refuse("from", &format!("'{from}' is not an email address.")));
        }
        check_filter(self.severity.as_deref(), self.about.as_deref())?;
        if self.password.is_some() && self.user.is_none() {
            return Err(refuse("user", "A password goes with the account to sign in as."));
        }
        Ok(())
    }
}

/// `alerts email set --to A... --server S ...`: sets up the email account.
pub struct Email;

impl Action for Email {
    const ID: &'static str = "alerts.email";
    const AREA: &'static str = AREA;
    const CHANGES: bool = true;
    const PATH: &'static str = "/api/1/alerts/email";
    const RUNS: Runs = Runs::AsAdministrator;
    const TESTS: Tests = TESTS;
    const TITLE: &'static str = "Set up email for alerts";
    type Args = EmailSet;

    fn argv(args: &EmailSet) -> Vec<String> {
        let mut argv = argv_of(&["alerts", "email", "set"]);
        for address in addresses(&args.to).unwrap_or_default() {
            argv.extend(["--to".to_string(), address.to_string()]);
        }
        argv.extend(["--server".to_string(), args.server.clone()]);
        for (option, value) in
            [("--port", &args.port), ("--user", &args.user), ("--from", &args.from)]
        {
            if let Some(value) = value {
                argv.extend([option.to_string(), value.clone()]);
            }
        }
        filter_argv(&mut argv, args.severity.as_deref(), args.cleared, args.about.as_deref());
        // A password goes with an account: one given is read from the input,
        // and with none the one the file has is kept.
        if args.user.is_some() {
            argv.push(
                if args.password.is_some() { "--password" } else { "--keep-password" }.into(),
            );
            if args.password.is_some() {
                argv.push("-".into());
            }
        }
        argv
    }

    fn input(args: &EmailSet) -> Option<Secret> {
        args.password.as_ref().map(|password| Secret::lines(&[password]))
    }

    fn examples() -> Examples<EmailSet> {
        let good = EmailSet {
            to: "ops@example.org".into(),
            server: "mail.example.org".into(),
            port: None,
            user: Some("alerts@example.org".into()),
            from: None,
            severity: None,
            cleared: None,
            about: None,
            password: Some(Secret::from("lab-password".to_string())),
        };
        Examples {
            good: vec![good],
            bad: vec![
                json!({}),
                json!({ "server": "mail.example.org" }),
                json!({ "to": "ops@example.org" }),
                json!({ "server": "mail.example.org", "to": "nobody" }),
                json!({ "server": "mail.example.org:587", "to": "ops@example.org" }),
                json!({ "port": "25", "server": "mail.example.org", "to": "ops@example.org" }),
                json!({ "from": "nobody", "server": "mail.example.org", "to": "ops@example.org" }),
                json!({ "password": "x", "server": "mail.example.org", "to": "ops@example.org" }),
                json!({
                    "password": "two\nlines",
                    "server": "mail.example.org",
                    "to": "ops@example.org",
                    "user": "u"
                }),
                json!({ "server": "mail.example.org", "severity": "high", "to": "ops@example.org" }),
                json!({ "cleared": "yes", "server": "mail.example.org", "to": "ops@example.org" }),
                json!({ "about": "data,sign-in", "server": "mail.example.org", "to": "a@b.test" }),
            ],
            // The old password is not known to admind, so nothing puts back
            // what this changes: the boot test sees the channel it made.
            undo: None,
        }
    }
}

/// `alerts email remove`: takes the email account away.
pub struct EmailRemove;

impl Action for EmailRemove {
    const ID: &'static str = "alerts.email-remove";
    const AREA: &'static str = AREA;
    const CHANGES: bool = true;
    const PATH: &'static str = "/api/1/alerts/email/remove";
    const RUNS: Runs = Runs::AsAdministrator;
    const TESTS: Tests = TESTS;
    const TITLE: &'static str = "Remove email for alerts";
    type Args = Nothing;

    fn argv(_: &Nothing) -> Vec<String> {
        argv_of(&["alerts", "email", "remove"])
    }

    fn examples() -> Examples<Nothing> {
        Examples {
            good: vec![Nothing {}],
            bad: vec![json!({ "to": "ops@example.org" }), json!({ "all": true })],
            undo: None,
        }
    }
}

/// A webhook to add, with its address and token on the command's input.
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WebhookAdd {
    /// What the address speaks: ntfy, matrix, slack, discord or json.
    pub kind: String,
    /// The address, which for Slack and Discord is a secret itself.
    pub url: Secret,
    /// The bearer token the server asks for: not for Slack and Discord.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token: Option<Secret>,
    /// Matrix only: the room's ID.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub room: Option<String>,
    /// The least severe alert it is told.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub severity: Option<String>,
    /// Whether it is told when an alert clears.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cleared: Option<bool>,
    /// The kinds it is told of.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub about: Option<String>,
}

impl Args for WebhookAdd {
    const FIELDS: &'static [Field] = &[
        Field::one_of(
            "kind",
            "What the address speaks: ntfy, matrix, slack, discord or json.",
            KINDS,
        ),
        Field::secret(
            "url",
            "The address: https, or http for json to a machine on the owner's own network. For \
             Slack and Discord it is a token itself.",
            WEBHOOK_CHARS,
        ),
        Field::secret(
            "token",
            "The bearer token the server asks for, for ntfy, matrix and json. Matrix needs \
             one. Left out, none.",
            WEBHOOK_CHARS,
        )
        .optional(),
        Field::text(
            "room",
            "Matrix only: the room's ID, such as !abc123:example.org.",
            4,
            255,
            Some(ROOM_PATTERN),
        )
        .optional(),
        SEVERITY,
        CLEARED,
        ABOUT_FIELD,
    ];

    fn check(&self) -> Result<(), Bad> {
        check_word("kind", &self.kind, KINDS)?;
        let matrix = self.kind == "matrix";
        match &self.room {
            Some(room) if matrix => {
                let valid = room.chars().count() <= 255
                    && room.starts_with('!')
                    && room[1..]
                        .split_once(':')
                        .is_some_and(|(left, right)| !left.is_empty() && !right.is_empty())
                    && !room.chars().any(char::is_control);
                if !valid {
                    return Err(refuse(
                        "room",
                        "A room's ID is ! and a name, a colon and the server, as \
                         !abc123:example.org.",
                    ));
                }
            }
            Some(_) => return Err(refuse("room", "A room is for a matrix webhook only.")),
            None if matrix => {
                return Err(refuse("room", "A matrix webhook needs its room's ID."));
            }
            None => {}
        }
        if self.token.is_some() && matches!(self.kind.as_str(), "slack" | "discord") {
            return Err(refuse(
                "token",
                &format!("{} has no token: its address holds the secret.", self.kind),
            ));
        }
        check_filter(self.severity.as_deref(), self.about.as_deref())
    }
}

/// `alerts webhook add KIND ...`: adds a webhook, whose address and token
/// are on the input, one line each.
pub struct WebhookAdded;

impl Action for WebhookAdded {
    const ID: &'static str = "alerts.webhook-add";
    const AREA: &'static str = AREA;
    const CHANGES: bool = true;
    const PATH: &'static str = "/api/1/alerts/webhooks";
    const RUNS: Runs = Runs::AsAdministrator;
    const TESTS: Tests = TESTS;
    const TITLE: &'static str = "Add a webhook for alerts";
    type Args = WebhookAdd;

    fn argv(args: &WebhookAdd) -> Vec<String> {
        let mut argv = argv_of(&["alerts", "webhook", "add"]);
        argv.push(args.kind.clone());
        if let Some(room) = &args.room {
            argv.extend(["--room".to_string(), room.clone()]);
        }
        filter_argv(&mut argv, args.severity.as_deref(), args.cleared, args.about.as_deref());
        argv
    }

    /// The address's line, then the token's when there is one.
    fn input(args: &WebhookAdd) -> Option<Secret> {
        Some(match &args.token {
            Some(token) => Secret::lines(&[&args.url, token]),
            None => Secret::lines(&[&args.url]),
        })
    }

    fn examples() -> Examples<WebhookAdd> {
        let good = WebhookAdd {
            kind: "json".into(),
            url: Secret::from("http://10.0.2.2:8080/alerts".to_string()),
            token: None,
            room: None,
            severity: None,
            cleared: None,
            about: None,
        };
        Examples {
            good: vec![good],
            bad: vec![
                json!({}),
                json!({ "kind": "json" }),
                json!({ "kind": "pager", "url": "https://x.example.org" }),
                json!({ "kind": "matrix", "url": "https://matrix.example.org" }),
                json!({ "kind": "json", "room": "!a:b.test", "url": "https://x.example.org" }),
                json!({ "kind": "slack", "token": "t", "url": "https://hooks.slack.com/x" }),
                json!({ "kind": "json", "severity": "urgent", "url": "https://x.example.org" }),
                json!({ "kind": "json", "url": "https://x.example.org", "url2": "x" }),
            ],
            // The webhook is the first on a machine that has none, which is
            // the one the examples are tried on; the boot test reads the
            // number from `alerts status --json`.
            undo: Some(Undo { action: "alerts.webhook-remove", args: json!({ "number": 1 }) }),
        }
    }
}

/// A webhook to take away, by its place among alerts.toml's webhooks.
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Numbered {
    /// Its number, as `alerts.channels` gives it, from 1.
    pub number: i64,
}

impl Args for Numbered {
    const FIELDS: &'static [Field] = &[Field::number(
        "number",
        "The webhook's number, as the list of channels gives it: 1 to 32.",
        1,
        WEBHOOKS_MOST,
    )];

    fn check(&self) -> Result<(), Bad> {
        if (1..=WEBHOOKS_MOST).contains(&self.number) {
            return Ok(());
        }
        Err(refuse("number", &format!("A webhook's number is 1 to {WEBHOOKS_MOST}.")))
    }
}

/// `alerts webhook remove NUMBER`: takes a webhook away. The ones after it
/// are numbered one less.
pub struct WebhookRemove;

impl Action for WebhookRemove {
    const ID: &'static str = "alerts.webhook-remove";
    const AREA: &'static str = AREA;
    const CHANGES: bool = true;
    const PATH: &'static str = "/api/1/alerts/webhooks/{number}/remove";
    const RUNS: Runs = Runs::AsAdministrator;
    const TESTS: Tests = TESTS;
    const TITLE: &'static str = "Remove a webhook for alerts";
    type Args = Numbered;

    fn argv(args: &Numbered) -> Vec<String> {
        vec!["alerts".into(), "webhook".into(), "remove".into(), args.number.to_string()]
    }

    fn subject(args: &Numbered) -> Option<String> {
        Some(args.number.to_string())
    }

    fn examples() -> Examples<Numbered> {
        Examples {
            good: vec![Numbered { number: 1 }],
            bad: vec![
                json!({}),
                json!({ "number": 0 }),
                json!({ "number": 33 }),
                json!({ "number": "1" }),
                json!({ "number": 1.5 }),
                json!({ "number": 1, "kind": "json" }),
            ],
            undo: None,
        }
    }
}

/// `alerts push key --json`: the machine's public key, which a browser
/// subscribes with.
pub struct PushKey;

impl Action for PushKey {
    const ID: &'static str = "alerts.push-key";
    const AREA: &'static str = AREA;
    const CHANGES: bool = false;
    const PATH: &'static str = "/api/1/alerts/push/key";
    const RUNS: Runs = Runs::AsAdministrator;
    const TESTS: Tests = TESTS;
    const TITLE: &'static str = "Read the key for browser alerts";
    type Args = Nothing;

    fn argv(_: &Nothing) -> Vec<String> {
        argv_of(&["alerts", "push", "key", "--json"])
    }

    fn examples() -> Examples<Nothing> {
        Examples {
            good: vec![Nothing {}],
            bad: vec![json!({ "json": true }), json!({ "id": "0123456789abcdef" })],
            undo: None,
        }
    }
}

/// A browser's subscription to alerts, which the command reads on its
/// input, with a name and what it is told.
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PushAdd {
    /// The browser's `PushSubscription` as JSON: its `endpoint` and `keys`.
    pub subscription: Secret,
    /// What the person calls it.
    pub name: String,
    /// The least severe alert it is told.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub severity: Option<String>,
    /// Whether it is told when an alert clears.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cleared: Option<bool>,
    /// The kinds it is told of.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub about: Option<String>,
}

impl Args for PushAdd {
    const FIELDS: &'static [Field] = &[
        Field::secret_document(
            "subscription",
            "The subscription the browser made: JSON with its endpoint and keys. Whoever has it \
             can send to the browser.",
            SUBSCRIPTION_MAX,
        ),
        Field::text(
            "name",
            "What to call this browser: 1 to 64 characters on one line.",
            1,
            NAME_MAX,
            Some(ONE_LINE),
        ),
        SEVERITY,
        CLEARED,
        ABOUT_FIELD,
    ];

    fn check(&self) -> Result<(), Bad> {
        check_line("name", &self.name, NAME_MAX)?;
        if self.name.trim().is_empty() {
            return Err(refuse("name", "A name is not all spaces."));
        }
        check_filter(self.severity.as_deref(), self.about.as_deref())
    }
}

/// `alerts push add --name NAME ...`: keeps a browser's subscription, which
/// is on the input, and answers its ID.
pub struct PushAdded;

impl Action for PushAdded {
    const ID: &'static str = "alerts.push-add";
    const AREA: &'static str = AREA;
    const CHANGES: bool = true;
    const PATH: &'static str = "/api/1/alerts/push";
    const RUNS: Runs = Runs::AsAdministrator;
    const TESTS: Tests = TESTS;
    const TITLE: &'static str = "Get alerts in a browser";
    type Args = PushAdd;

    fn argv(args: &PushAdd) -> Vec<String> {
        let mut argv = argv_of(&["alerts", "push", "add", "--name"]);
        argv.push(args.name.clone());
        filter_argv(&mut argv, args.severity.as_deref(), args.cleared, args.about.as_deref());
        argv
    }

    /// The subscription as it is.
    fn input(args: &PushAdd) -> Option<Secret> {
        Some(args.subscription.copied())
    }

    fn examples() -> Examples<PushAdd> {
        let subscription = json!({
            "endpoint": "https://push.example.net/send/lab",
            "keys": { "p256dh": "BLabLabLabLabLabLabLabLabLabLabLabLabLab", "auth": "LabLabLabLabLabLab" }
        });
        let good = PushAdd {
            subscription: Secret::from(subscription.to_string()),
            name: "Lab browser".into(),
            severity: None,
            cleared: None,
            about: None,
        };
        Examples {
            good: vec![good],
            bad: vec![
                json!({}),
                json!({ "name": "Lab browser" }),
                json!({ "name": "", "subscription": "{}" }),
                json!({ "name": "a\nb", "subscription": "{}" }),
                json!({ "name": "Lab browser", "severity": "high", "subscription": "{}" }),
                json!({ "name": "Lab browser", "subscription": "{}", "token": "x" }),
                json!({ "name": "   ", "subscription": "{}" }),
            ],
            // The ID that removes it is in the answer, which a boot test and
            // a page keep: this one is where the answer's would be.
            undo: Some(Undo {
                action: "alerts.push-remove",
                args: json!({ "id": "0000000000000000" }),
            }),
        }
    }
}

/// A browser to take away, by its ID.
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Identified {
    /// Its ID, as the answer to adding it and the list of channels give it.
    pub id: String,
}

/// Whether `id` is a browser's ID: 16 digits of 0 to 9 and a to f.
fn is_id(id: &str) -> bool {
    id.len() == 16 && id.bytes().all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

impl Args for Identified {
    const FIELDS: &'static [Field] = &[Field::text(
        "id",
        "The browser's ID: 16 digits, 0 to 9 and a to f, as the list of channels gives it.",
        16,
        16,
        Some(ID_PATTERN),
    )];

    fn check(&self) -> Result<(), Bad> {
        if is_id(&self.id) {
            return Ok(());
        }
        Err(refuse("id", "A browser's ID is 16 digits, 0 to 9 and a to f."))
    }
}

/// `alerts push remove ID`: stops alerts in a browser.
pub struct PushRemove;

impl Action for PushRemove {
    const ID: &'static str = "alerts.push-remove";
    const AREA: &'static str = AREA;
    const CHANGES: bool = true;
    const PATH: &'static str = "/api/1/alerts/push/{id}/remove";
    const RUNS: Runs = Runs::AsAdministrator;
    const TESTS: Tests = TESTS;
    const TITLE: &'static str = "Stop alerts in a browser";
    type Args = Identified;

    fn argv(args: &Identified) -> Vec<String> {
        vec!["alerts".into(), "push".into(), "remove".into(), args.id.clone()]
    }

    fn subject(args: &Identified) -> Option<String> {
        Some(args.id.clone())
    }

    fn examples() -> Examples<Identified> {
        Examples {
            good: vec![Identified { id: "0123456789abcdef".into() }],
            bad: vec![
                json!({}),
                json!({ "id": "" }),
                json!({ "id": "0123456789ABCDEF" }),
                json!({ "id": "0123456789abcde" }),
                json!({ "id": "../../etc/passwd" }),
                json!({ "id": 1 }),
            ],
            undo: None,
        }
    }
}

/// A channel and the kinds of alert it is told of.
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ChannelAbout {
    /// `email`, `webhook:NUMBER` or `push:ID`.
    pub channel: String,
    /// `all`, or some of `data`, `stopped` and `hardware`.
    pub about: String,
}

/// Whether `channel` is what `alerts channel` takes: `email`, `webhook:`
/// and a number from 1 to 32 with no leading zero, or `push:` and an ID.
fn is_channel(channel: &str) -> bool {
    match channel.split_once(':') {
        None => channel == "email",
        Some(("webhook", number)) => {
            number.bytes().all(|byte| byte.is_ascii_digit())
                && !number.starts_with('0')
                && number.parse::<i64>().is_ok_and(|number| (1..=WEBHOOKS_MOST).contains(&number))
        }
        Some(("push", id)) => is_id(id),
        Some(_) => false,
    }
}

impl Args for ChannelAbout {
    const FIELDS: &'static [Field] = &[
        Field::text(
            "channel",
            "The channel: email, webhook:NUMBER or push:ID, as the list of channels gives them.",
            5,
            21,
            Some(CHANNEL_PATTERN),
        ),
        Field::text(
            "about",
            "The kinds of alert it is told of: all, or data, stopped and hardware with commas \
             between them, as data,hardware.",
            3,
            ABOUT_MAX,
            Some(ABOUT_PATTERN),
        ),
    ];

    fn check(&self) -> Result<(), Bad> {
        if !is_channel(&self.channel) {
            return Err(refuse(
                "channel",
                "A channel is email, webhook: and its number from 1 to 32, or push: and a \
                 browser's ID.",
            ));
        }
        check_about(&self.about)
    }
}

/// `alerts channel CHANNEL --about WORDS`: says which kinds of alert a
/// channel is told of. A kind that is left out is never sent to it, and an
/// alert of no kind goes to every channel.
pub struct ChannelAboutSet;

impl Action for ChannelAboutSet {
    const ID: &'static str = "alerts.channel-about";
    const AREA: &'static str = AREA;
    const CHANGES: bool = true;
    const PATH: &'static str = "/api/1/alerts/channels/about";
    const RUNS: Runs = Runs::AsAdministrator;
    const TESTS: Tests = TESTS;
    const TITLE: &'static str = "Choose the kinds of alert a channel is told of";
    type Args = ChannelAbout;

    fn argv(args: &ChannelAbout) -> Vec<String> {
        vec![
            "alerts".into(),
            "channel".into(),
            args.channel.clone(),
            "--about".into(),
            args.about.clone(),
        ]
    }

    fn subject(args: &ChannelAbout) -> Option<String> {
        Some(args.channel.clone())
    }

    /// The same action puts it back: `all` is what a channel starts with.
    fn examples() -> Examples<ChannelAbout> {
        Examples {
            good: vec![ChannelAbout { channel: "email".into(), about: "data,stopped".into() }],
            bad: vec![
                json!({}),
                json!({ "channel": "email" }),
                json!({ "about": "data" }),
                json!({ "about": "data", "channel": "webhook:0" }),
                json!({ "about": "data", "channel": "webhook:33" }),
                json!({ "about": "data", "channel": "webhook:01" }),
                json!({ "about": "data", "channel": "push:0123456789ABCDEF" }),
                json!({ "about": "data", "channel": "sms" }),
                json!({ "about": "data,sign-in", "channel": "email" }),
                json!({ "about": "", "channel": "email" }),
                json!({ "about": "data,stopped,hardware,data", "channel": "email" }),
            ],
            undo: Some(Undo {
                action: "alerts.channel-about",
                args: json!({ "about": "all", "channel": "email" }),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::Value;

    use super::super::{Caller, Registered, authorize};
    use super::*;
    use crate::admind::actions::REGISTRY;
    use crate::admind::errors::Kind as Failed;
    use crate::admind::scopes::Scopes;
    use crate::alertd::settings::Kind;

    /// The registered action `id`.
    fn action(id: &str) -> &'static dyn Registered {
        *REGISTRY.iter().find(|action| action.id() == id).unwrap()
    }

    /// The failure of `args` for `id`: its kind, its field and its words.
    fn refused(id: &str, args: Value) -> (Failed, Option<&'static str>, String) {
        let failure = action(id).prepare(args).unwrap_err();
        (failure.kind, failure.field, failure.detail)
    }

    /// The words of a refusal of the field `field`, which must be the one.
    fn words(id: &str, args: Value, field: &str) -> String {
        let shown = args.to_string();
        let (kind, got, words) = refused(id, args);
        assert_eq!((kind, got), (Failed::BadValue, Some(field)), "{shown}: {words}");
        assert!(words.ends_with("Nothing was done."), "{words}");
        words
    }

    const ALL: [&str; 11] = [
        "alerts.list",
        "alerts.channels",
        "alerts.test",
        "alerts.email",
        "alerts.email-remove",
        "alerts.webhook-add",
        "alerts.webhook-remove",
        "alerts.push-key",
        "alerts.push-add",
        "alerts.push-remove",
        "alerts.channel-about",
    ];

    #[test]
    fn all_run_as_an_administrator_in_the_area_and_name_no_file_and_no_probe() {
        for id in ALL {
            let action = action(id);
            assert_eq!(action.runs(), Runs::AsAdministrator, "{id}");
            assert_eq!(action.area(), "alerts", "{id}");
            assert!(!action.long() && !action.secret_answer(), "{id}");
            assert!(!action.compares(), "{id}: admind sees neither alerts.toml nor push.json");
            assert_eq!(action.tests().checks, "admind::actions::alerts::tests", "{id}");
            assert_eq!(action.tests().browser, "web-console/alerts", "{id}");
            assert_eq!(action.tests().route, "admin-alerts", "{id}");
            let (good, _, _) = action.example_values();
            let prepared = action.prepare(good[0].clone()).unwrap();
            assert!(prepared.files.is_empty() && prepared.probe.is_none(), "{id}");
            assert!(prepared.ansible.become_root, "{id}");
        }
    }

    #[test]
    fn the_reads_are_gets_in_lines_or_one_answer_and_the_changes_are_posts() {
        for (id, changes, path, lines, argv) in [
            ("alerts.list", false, "/api/1/alerts", true, "alerts list --json"),
            ("alerts.channels", false, "/api/1/alerts/channels", true, "alerts status --json"),
            ("alerts.test", true, "/api/1/alerts/test", false, "alerts test --json"),
            (
                "alerts.email-remove",
                true,
                "/api/1/alerts/email/remove",
                false,
                "alerts email remove",
            ),
            ("alerts.push-key", false, "/api/1/alerts/push/key", false, "alerts push key --json"),
        ] {
            let action = action(id);
            assert_eq!((action.changes(), action.path(), action.lines()), (changes, path, lines));
            assert_eq!(action.posts(), changes, "{id}");
            assert_eq!(action.prepare(json!({})).unwrap().argv.join(" "), argv, "{id}");
        }
        for (id, path) in [
            ("alerts.email", "/api/1/alerts/email"),
            ("alerts.webhook-add", "/api/1/alerts/webhooks"),
            ("alerts.webhook-remove", "/api/1/alerts/webhooks/{number}/remove"),
            ("alerts.push-add", "/api/1/alerts/push"),
            ("alerts.push-remove", "/api/1/alerts/push/{id}/remove"),
            ("alerts.channel-about", "/api/1/alerts/channels/about"),
        ] {
            let action = action(id);
            assert!(action.changes() && action.posts(), "{id}");
            assert_eq!(action.path(), path, "{id}");
        }
        // A test is told by its output, which a channel that refused is part
        // of: status 1 answers it.
        assert_eq!(action("alerts.test").answers(), [0, 1]);
        assert_eq!(action("alerts.list").answers(), [0]);
        // The history's read keeps its address.
        assert_eq!(action("reliability.alerts").path(), "/api/1/alerts/history");
    }

    #[test]
    fn the_addresses_of_the_alerts_find_their_actions() {
        use super::super::{Found, find};
        for (post, path, id) in [
            (false, "/api/1/alerts", "alerts.list"),
            (false, "/api/1/alerts/history", "reliability.alerts"),
            (false, "/api/1/alerts/channels", "alerts.channels"),
            (true, "/api/1/alerts/channels/about", "alerts.channel-about"),
            (true, "/api/1/alerts/test", "alerts.test"),
            (true, "/api/1/alerts/email", "alerts.email"),
            (true, "/api/1/alerts/email/remove", "alerts.email-remove"),
            (true, "/api/1/alerts/webhooks", "alerts.webhook-add"),
            (true, "/api/1/alerts/webhooks/3/remove", "alerts.webhook-remove"),
            (false, "/api/1/alerts/push/key", "alerts.push-key"),
            (true, "/api/1/alerts/push", "alerts.push-add"),
            (true, "/api/1/alerts/push/0123456789abcdef/remove", "alerts.push-remove"),
        ] {
            let found = find(REGISTRY, post, path);
            let Found::Action(action, params) = found else { panic!("{path}: {found:?}") };
            assert_eq!(action.id(), id, "{path}");
            let wanted = match id {
                "alerts.webhook-remove" => Some(("number", "3")),
                "alerts.push-remove" => Some(("id", "0123456789abcdef")),
                _ => None,
            };
            assert_eq!(params.len(), usize::from(wanted.is_some()), "{path}");
            if let Some((name, value)) = wanted {
                assert_eq!(params.get(name).and_then(Value::as_str), Some(value), "{path}");
            }
        }
        for (post, path) in [
            (true, "/api/1/alerts"),
            (false, "/api/1/alerts/email"),
            (false, "/api/1/alerts/webhooks"),
            (false, "/api/1/alerts/push"),
            (true, "/api/1/alerts/push/key"),
        ] {
            assert!(
                matches!(find(REGISTRY, post, path), Found::WrongMethod),
                "{path}: asked for with the other method"
            );
        }
        for path in ["/api/1/alerts/", "/api/1/alerts/webhooks/3", "/api/1/alerts/push/x/y/remove"]
        {
            assert!(matches!(find(REGISTRY, true, path), Found::None), "{path}");
            assert!(matches!(find(REGISTRY, false, path), Found::None), "{path}");
        }
    }

    #[test]
    fn the_reads_and_the_test_take_nothing_and_refuse_anything_given() {
        for id in [
            "alerts.list",
            "alerts.channels",
            "alerts.test",
            "alerts.email-remove",
            "alerts.push-key",
        ] {
            for args in action(id).example_values().1 {
                assert_eq!(action(id).prepare(args.clone()).unwrap_err().kind, Failed::BadRequest);
            }
            assert!(action(id).fields().is_empty(), "{id}");
        }
    }

    /// The command and its input for `args` of the email action.
    fn email(args: Value) -> (String, Option<Vec<u8>>) {
        let prepared = action("alerts.email").prepare(args).unwrap();
        (prepared.argv.join(" "), prepared.input.map(|input| input.bytes().to_vec()))
    }

    #[test]
    fn email_gives_the_command_what_is_given_and_a_password_goes_on_the_input() {
        let (argv, input) = email(json!({
            "to": "ops@example.org", "server": "mail.example.org",
            "user": "alerts@example.org", "password": "app password"
        }));
        assert_eq!(
            argv,
            "alerts email set --to ops@example.org --server mail.example.org --user \
             alerts@example.org --password -"
        );
        assert_eq!(input.as_deref(), Some(&b"app password\n"[..]));
        assert!(!argv.contains("app password"));
        // Without a password, with an account: the one kept.
        let (argv, input) = email(json!({
            "to": "ops@example.org", "server": "mail.example.org", "user": "u"
        }));
        assert_eq!(
            argv,
            "alerts email set --to ops@example.org --server mail.example.org --user u \
             --keep-password"
        );
        assert_eq!(input, None);
        // Without an account, neither.
        let (argv, input) = email(json!({ "to": "a@b.test", "server": "s.test" }));
        assert_eq!(argv, "alerts email set --to a@b.test --server s.test");
        assert_eq!(input, None);
        // Everything: each address is an option, in the order of the lines.
        let (argv, _) = email(json!({
            "to": "one@example.org\r\ntwo@example.org\n\n  three@example.org \n",
            "server": "mail.example.org", "port": "465", "user": "u", "from": "alerts@example.org",
            "severity": "critical", "cleared": false, "about": "data,hardware", "password": "p"
        }));
        assert_eq!(
            argv,
            "alerts email set --to one@example.org --to two@example.org --to three@example.org \
             --server mail.example.org --port 465 --user u --from alerts@example.org --severity \
             critical --cleared no --about data,hardware --password -"
        );
        let (argv, _) = email(json!({
            "to": "a@b.test", "server": "s.test", "cleared": true, "about": "all"
        }));
        assert_eq!(
            argv,
            "alerts email set --to a@b.test --server s.test --cleared yes --about all"
        );
    }

    #[test]
    fn email_refuses_what_alertd_would_leave_out_with_the_field_named() {
        let base = |extra: Value| {
            let mut args = json!({ "to": "ops@example.org", "server": "mail.example.org" });
            for (name, value) in extra.as_object().unwrap() {
                args[name] = value.clone();
            }
            args
        };
        let nine: Vec<String> = (1..=9).map(|n| format!("a{n}@b.test")).collect();
        let eight = nine[..8].join("\n");
        assert!(action("alerts.email").prepare(base(json!({ "to": eight }))).is_ok());
        assert_eq!(
            words("alerts.email", base(json!({ "to": nine.join("\n") })), "to"),
            "Give 1 to 8 addresses, one on each line. Nothing was done."
        );
        for (to, bad) in [
            ("nobody", "nobody"),
            ("a@b.test\na b@c.test", "a b@c.test"),
            ("a@b.test\n<x>@c.test", "<x>@c.test"),
            ("a@-b.test", "a@-b.test"),
            ("a@b.test\n@c.test", "@c.test"),
        ] {
            assert_eq!(
                words("alerts.email", base(json!({ "to": to })), "to"),
                format!("'{bad}' is not an email address. Nothing was done.")
            );
        }
        assert_eq!(
            words("alerts.email", base(json!({ "to": "\n\n " })), "to"),
            "Give 1 to 8 addresses, one on each line. Nothing was done."
        );
        for server in ["mail.example.org:587", "-mail", "mail..", "a b", "https://mail", "mail_x"] {
            words("alerts.email", base(json!({ "server": server })), "server");
        }
        for port in ["25", "0587", " 587", "587 ", "587\n", "", "TLS"] {
            assert_eq!(
                words("alerts.email", base(json!({ "port": port })), "port"),
                "port is 587 or 465. Nothing was done."
            );
        }
        for user in ["", "a\nb", "a\tb"] {
            words("alerts.email", base(json!({ "user": user, "password": "x" })), "user");
        }
        let long = "u".repeat(257);
        words("alerts.email", base(json!({ "user": long, "password": "x" })), "user");
        words("alerts.email", base(json!({ "from": "nobody" })), "from");
        words("alerts.email", base(json!({ "severity": "High" })), "severity");
        words("alerts.email", base(json!({ "about": "data,sign-in" })), "about");
        // A password goes with an account.
        assert_eq!(
            words("alerts.email", base(json!({ "password": "x" })), "user"),
            "A password goes with the account to sign in as. Nothing was done."
        );
        for password in ["", "two\nlines", "a\rb"] {
            let (kind, field, _) =
                refused("alerts.email", base(json!({ "user": "u", "password": password })));
            assert_eq!((kind, field), (Failed::BadValue, Some("password")), "{password:?}");
        }
        let longest = "p".repeat(512);
        assert!(
            action("alerts.email")
                .prepare(base(json!({ "user": "u", "password": longest })))
                .is_ok()
        );
        let (kind, field, _) =
            refused("alerts.email", base(json!({ "user": "u", "password": "p".repeat(513) })));
        assert_eq!((kind, field), (Failed::BadValue, Some("password")));
        // What is not what a field takes.
        for args in [
            base(json!({ "port": 587 })),
            base(json!({ "cleared": "yes" })),
            base(json!({ "to": ["a@b.test"] })),
            base(json!({ "reach": "all" })),
        ] {
            assert_eq!(action("alerts.email").prepare(args).unwrap_err().kind, Failed::BadRequest);
        }
    }

    #[test]
    fn email_keeps_its_password_out_of_everything_it_shows() {
        let prepared = action("alerts.email")
            .prepare(json!({
                "to": "ops@example.org", "server": "mail.example.org", "user": "u",
                "password": "mk-marker-5f1e"
            }))
            .unwrap();
        for text in [
            prepared.argv.join(" "),
            prepared.args.to_string(),
            format!("{:?}", prepared.ansible),
            format!("{prepared:?}"),
        ] {
            assert!(!text.contains("mk-marker"), "{text}");
        }
        assert!(prepared.ansible.no_log);
        assert_eq!(prepared.ansible.params["stdin"], "{{ password }}");
        assert_eq!(prepared.ansible.name, "Set up email for alerts");
        // With no password given, the task reads none and hides nothing.
        let keep = action("alerts.email")
            .prepare(json!({ "to": "ops@example.org", "server": "mail.example.org", "user": "u" }))
            .unwrap();
        assert!(!keep.ansible.no_log && keep.ansible.params.get("stdin").is_none());
        assert!(keep.input.is_none());
        let (_, _, undo) = action("alerts.email").example_values();
        assert_eq!(undo, None, "the old password is not known to admind");
    }

    /// The command and its input for `args` of the webhook action.
    fn webhook(args: Value) -> (String, Option<Vec<u8>>) {
        let prepared = action("alerts.webhook-add").prepare(args).unwrap();
        (prepared.argv.join(" "), prepared.input.map(|input| input.bytes().to_vec()))
    }

    #[test]
    fn a_webhook_gives_its_address_then_its_token_on_the_input_and_the_rest_as_options() {
        let (argv, input) = webhook(json!({ "kind": "json", "url": "http://10.0.2.2:8080/a" }));
        assert_eq!(argv, "alerts webhook add json");
        assert_eq!(input.as_deref(), Some(&b"http://10.0.2.2:8080/a\n"[..]));
        let (argv, input) = webhook(json!({
            "kind": "ntfy", "url": "https://ntfy.sh/a-topic", "token": "tk_1",
            "severity": "critical", "cleared": false, "about": "data"
        }));
        assert_eq!(argv, "alerts webhook add ntfy --severity critical --cleared no --about data");
        assert_eq!(input.as_deref(), Some(&b"https://ntfy.sh/a-topic\ntk_1\n"[..]));
        let (argv, input) = webhook(json!({
            "kind": "matrix", "url": "https://matrix.example.org", "token": "syt_1",
            "room": "!abc123:example.org"
        }));
        assert_eq!(argv, "alerts webhook add matrix --room !abc123:example.org");
        assert_eq!(input.as_deref(), Some(&b"https://matrix.example.org\nsyt_1\n"[..]));
        let prepared = action("alerts.webhook-add")
            .prepare(json!({ "kind": "slack", "url": "mk-url-5f1e", "severity": "warning" }))
            .unwrap();
        assert_eq!(prepared.ansible.params["stdin"], "{{ url }}");
        assert!(prepared.ansible.no_log);
        assert!(!prepared.args.to_string().contains("mk-url"));
        let both = action("alerts.webhook-add")
            .prepare(json!({ "kind": "json", "url": "https://a.test", "token": "t" }))
            .unwrap();
        assert_eq!(both.ansible.params["stdin"], "{{ url }}\n{{ token }}");
    }

    #[test]
    fn a_webhook_refuses_a_kind_and_a_room_that_do_not_go_together() {
        let base = |extra: Value| {
            let mut args = json!({ "kind": "json", "url": "https://x.example.org" });
            for (name, value) in extra.as_object().unwrap() {
                args[name] = value.clone();
            }
            args
        };
        for kind in ["pager", "JSON", "json ", "", "ntfy\n"] {
            let words = words("alerts.webhook-add", base(json!({ "kind": kind })), "kind");
            assert_eq!(words, "kind is ntfy, matrix, slack, discord or json. Nothing was done.");
        }
        assert_eq!(
            words("alerts.webhook-add", base(json!({ "kind": "matrix", "token": "t" })), "room"),
            "A matrix webhook needs its room's ID. Nothing was done."
        );
        assert_eq!(
            words("alerts.webhook-add", base(json!({ "room": "!a:b.test" })), "room"),
            "A room is for a matrix webhook only. Nothing was done."
        );
        for room in ["a:b.test", "!ab.test", "!:b.test", "!a:", "!a:b\nc", ""] {
            let args = base(json!({ "kind": "matrix", "token": "t", "room": room }));
            let (kind, field, _) = refused("alerts.webhook-add", args);
            assert_eq!((kind, field), (Failed::BadValue, Some("room")), "{room:?}");
        }
        assert!(
            action("alerts.webhook-add")
                .prepare(base(json!({ "kind": "matrix", "token": "t", "room": "!a:b.test" })))
                .is_ok()
        );
        for kind in ["slack", "discord"] {
            let words =
                words("alerts.webhook-add", base(json!({ "kind": kind, "token": "t" })), "token");
            assert_eq!(
                words,
                format!("{kind} has no token: its address holds the secret. Nothing was done.")
            );
        }
        // A secret is one line of 1 to 500 characters.
        for url in ["", "a\nb", "u".repeat(501).as_str()] {
            let (kind, field, _) = refused("alerts.webhook-add", base(json!({ "url": url })));
            assert_eq!((kind, field), (Failed::BadValue, Some("url")), "{url:?}");
        }
        for token in ["", "a\nb", "t".repeat(501).as_str()] {
            let (kind, field, _) = refused("alerts.webhook-add", base(json!({ "token": token })));
            assert_eq!((kind, field), (Failed::BadValue, Some("token")), "{token:?}");
        }
        let widest = "\u{10ffff}".repeat(500);
        let args = base(json!({ "url": widest, "token": widest }));
        let prepared = action("alerts.webhook-add").prepare(args).unwrap();
        assert!(prepared.input.unwrap().bytes().len() <= 4096, "what a command's input takes");
        words("alerts.webhook-add", base(json!({ "severity": "urgent" })), "severity");
        words("alerts.webhook-add", base(json!({ "about": "colour" })), "about");
    }

    #[test]
    fn a_webhook_is_taken_away_by_its_number_and_the_new_one_by_the_undo() {
        let remove = |args: Value| action("alerts.webhook-remove").prepare(args);
        let prepared = remove(json!({ "number": 7 })).unwrap();
        assert_eq!(prepared.argv, ["alerts", "webhook", "remove", "7"]);
        assert_eq!(prepared.ansible.name, "Remove a webhook for alerts: 7");
        for number in [1, 32] {
            assert!(remove(json!({ "number": number })).is_ok(), "{number}");
        }
        for number in [0, 33, -1, 1000] {
            let (kind, field, words) =
                refused("alerts.webhook-remove", json!({ "number": number }));
            assert_eq!((kind, field), (Failed::BadValue, Some("number")), "{number}");
            assert_eq!(words, "A webhook's number is 1 to 32. Nothing was done.");
        }
        for number in [json!("1"), json!(1.5), json!(null), json!(true)] {
            assert_eq!(remove(json!({ "number": number })).unwrap_err().kind, Failed::BadRequest);
        }
        let (_, _, undo) = action("alerts.webhook-add").example_values();
        let undo = undo.unwrap();
        assert_eq!(undo.action, "alerts.webhook-remove");
        assert!(remove(undo.args).is_ok());
        assert_eq!(action("alerts.webhook-remove").example_values().2, None);
    }

    #[test]
    fn a_browser_is_added_with_its_subscription_on_the_input_as_it_is() {
        let subscription =
            r#"{"endpoint":"https://push.example.net/send/x","keys":{"p256dh":"a","auth":"b"}}"#;
        let prepared = action("alerts.push-add")
            .prepare(json!({ "subscription": subscription, "name": "Firefox on a phone" }))
            .unwrap();
        assert_eq!(prepared.argv, ["alerts", "push", "add", "--name", "Firefox on a phone"]);
        assert_eq!(prepared.input.as_ref().map(Secret::bytes), Some(subscription.as_bytes()));
        assert!(prepared.ansible.no_log);
        assert_eq!(prepared.ansible.params["stdin"], "{{ subscription }}");
        assert!(!prepared.args.to_string().contains("push.example.net"));
        assert!(!format!("{prepared:?}").contains("push.example.net"));
        let full = action("alerts.push-add")
            .prepare(json!({
                "subscription": subscription, "name": "n", "severity": "critical",
                "cleared": false, "about": "stopped"
            }))
            .unwrap();
        assert_eq!(
            full.argv.join(" "),
            "alerts push add --name n --severity critical --cleared no --about stopped"
        );
        let (_, _, undo) = action("alerts.push-add").example_values();
        assert_eq!(undo.unwrap().action, "alerts.push-remove");
        assert!(action("alerts.push-add").errors().is_empty(), "alertd's refusals are its words");
    }

    #[test]
    fn a_browsers_name_is_one_line_of_1_to_64_characters_and_what_is_told_is_checked() {
        let args = |name: &str| json!({ "subscription": "{}", "name": name });
        for name in ["a", "Lab browser", "caf\u{e9} \u{1f984}", "x".repeat(64).as_str()] {
            assert!(action("alerts.push-add").prepare(args(name)).is_ok(), "{name}");
        }
        for name in ["", " ", "a\nb", "a\tb", "x".repeat(65).as_str()] {
            let (kind, field, _) = refused("alerts.push-add", args(name));
            assert_eq!((kind, field), (Failed::BadValue, Some("name")), "{name:?}");
        }
        let mut with = args("n");
        with["about"] = json!("data,data");
        assert!(action("alerts.push-add").prepare(with).is_ok());
        let mut with = args("n");
        with["severity"] = json!("none");
        assert_eq!(refused("alerts.push-add", with).1, Some("severity"));
        // 2,048 bytes of subscription, which the command's input holds.
        let big = json!({ "subscription": "x".repeat(2048), "name": "n" });
        let prepared = action("alerts.push-add").prepare(big).unwrap();
        assert!(prepared.input.unwrap().bytes().len() <= 4096);
        let (kind, field, _) =
            refused("alerts.push-add", json!({ "subscription": "x".repeat(2049), "name": "n" }));
        assert_eq!((kind, field), (Failed::BadValue, Some("subscription")));
    }

    #[test]
    fn a_browser_is_removed_by_its_id_of_sixteen_lowercase_digits() {
        let remove = |id: &str| action("alerts.push-remove").prepare(json!({ "id": id }));
        let prepared = remove("0123456789abcdef").unwrap();
        assert_eq!(prepared.argv, ["alerts", "push", "remove", "0123456789abcdef"]);
        assert_eq!(prepared.ansible.name, "Stop alerts in a browser: 0123456789abcdef");
        for id in [
            "",
            "0123456789ABCDEF",
            "0123456789abcde",
            "0123456789abcdef0",
            "../etc",
            "g123456789abcdef",
            "0123456789abcde\n",
        ] {
            let (kind, field, words) = refused("alerts.push-remove", json!({ "id": id }));
            assert_eq!((kind, field), (Failed::BadValue, Some("id")), "{id:?}");
            assert_eq!(words, "A browser's ID is 16 digits, 0 to 9 and a to f. Nothing was done.");
        }
        assert_eq!(remove("0123456789abcdef").unwrap().args, json!({ "id": "0123456789abcdef" }));
        assert_eq!(
            action("alerts.push-remove").prepare(json!({ "id": 7 })).unwrap_err().kind,
            Failed::BadRequest
        );
    }

    #[test]
    fn the_kinds_of_a_channel_are_set_by_its_name_and_both_patterns_are_what_the_checks_hold() {
        let set = |args: Value| action("alerts.channel-about").prepare(args);
        let prepared = set(json!({ "channel": "webhook:2", "about": "data,hardware" })).unwrap();
        assert_eq!(prepared.argv, ["alerts", "channel", "webhook:2", "--about", "data,hardware"]);
        assert_eq!(
            prepared.ansible.name,
            "Choose the kinds of alert a channel is told of: webhook:2"
        );
        assert!(prepared.files.is_empty() && prepared.probe.is_none() && prepared.input.is_none());
        for channel in
            ["email", "webhook:1", "webhook:9", "webhook:10", "webhook:32", "push:0123456789abcdef"]
        {
            for about in [
                "all",
                "data",
                "stopped",
                "hardware",
                "data,stopped",
                "hardware,data",
                "data,stopped,hardware",
                "data,data,data",
            ] {
                assert!(
                    set(json!({ "channel": channel, "about": about })).is_ok(),
                    "{channel} {about}"
                );
            }
        }
        for channel in [
            "",
            "mail",
            "Email",
            "email ",
            "webhook",
            "webhook:",
            "webhook:0",
            "webhook:00",
            "webhook:01",
            "webhook:33",
            "webhook:100",
            "webhook:+5",
            "webhook:-1",
            "webhook:1.0",
            "webhook: 1",
            "push",
            "push:",
            "push:0123456789ABCDEF",
            "push:0123456789abcde",
            "push:0123456789abcdef0",
            "push:g123456789abcdef",
            "sms:1",
            "email:1",
            "webhook:1\n",
        ] {
            let (kind, field, words) =
                refused("alerts.channel-about", json!({ "channel": channel, "about": "all" }));
            assert_eq!((kind, field), (Failed::BadValue, Some("channel")), "{channel:?}");
            assert!(words.starts_with("A channel is email, webhook:"), "{words}");
        }
        for about in [
            "",
            "al",
            "ALL",
            "all,data",
            "data,",
            ",data",
            "data,,stopped",
            "data, stopped",
            " data",
            "data ",
            "Data",
            "sign-in",
            "data,sign-in",
            "data,stopped,hardware,data",
            "all\n",
            "data\n",
            "everything",
        ] {
            let (kind, field, words) =
                refused("alerts.channel-about", json!({ "channel": "email", "about": about }));
            assert_eq!((kind, field), (Failed::BadValue, Some("about")), "{about:?}");
            assert_eq!(
                words,
                "about is all, or data, stopped or hardware with commas between them. Nothing was done."
            );
        }
        // The schema and the checks say one thing.
        let schema = action("alerts.channel-about").fields();
        assert_eq!(schema.len(), 2);
        let (channel, about) = (&schema[0], &schema[1]);
        assert_eq!(
            (channel.kind, about.kind),
            (
                super::super::Type::Text { min: 5, max: 21, pattern: Some(CHANNEL_PATTERN) },
                super::super::Type::Text { min: 3, max: ABOUT_MAX, pattern: Some(ABOUT_PATTERN) },
            )
        );
        let (_, _, undo) = action("alerts.channel-about").example_values();
        let undo = undo.unwrap();
        assert!(set(undo.args.clone()).is_ok());
        assert_eq!(
            (undo.action, undo.args),
            ("alerts.channel-about", json!({ "about": "all", "channel": "email" }))
        );
        assert_eq!(
            set(json!({ "channel": "email", "about": "all", "x": 1 })).unwrap_err().kind,
            Failed::BadRequest
        );
    }

    #[test]
    fn the_kinds_the_pattern_names_are_the_kinds_alertd_knows() {
        for word in ["data", "stopped", "hardware"] {
            assert!(ABOUT.contains(&word));
            assert!(ABOUT_PATTERN.contains(word), "{word}");
        }
        assert_eq!(ABOUT.len(), 3);
        assert_eq!(KINDS, Kind::NAMES);
        for kind in Kind::NAMES {
            assert!(Kind::parse(kind).is_some(), "{kind}");
        }
    }

    #[test]
    fn a_token_needs_the_alerts_scope_even_to_read_since_the_command_is_roots() {
        let call = |list: &str, id: &str| {
            let scopes = Scopes::parse(list).unwrap();
            authorize(&Caller::Token { name: "ci", scopes: &scopes }, action(id))
        };
        for id in ALL {
            for list in ["alerts", "*"] {
                assert!(call(list, id).is_ok(), "{id} {list}");
            }
            for list in ["alerts:read", "reliability", "services"] {
                let refused = call(list, id).unwrap_err();
                assert_eq!(refused.kind, Failed::OutOfScope, "{id} {list}");
                assert_eq!(
                    refused.detail,
                    "the API token \"ci\" may not do this: it needs the scope alerts or *"
                );
            }
        }
    }

    #[test]
    fn a_person_who_is_no_administrator_is_refused_and_a_locked_one_is_asked_to_unlock() {
        for id in ALL {
            let action = action(id);
            let session = |admin, unlocked| Caller::Session { name: "bob", admin, unlocked };
            assert_eq!(
                authorize(&session(false, true), action).unwrap_err().kind,
                Failed::NotAdmin,
                "{id}"
            );
            assert_eq!(
                authorize(&session(true, false), action).unwrap_err().kind,
                Failed::Locked,
                "{id}"
            );
            assert!(authorize(&session(true, true), action).is_ok(), "{id}");
        }
    }

    #[test]
    fn the_changes_name_the_error_ids_of_none_and_every_example_is_tried_by_the_rules() {
        for id in ALL {
            assert!(action(id).errors().is_empty(), "{id}");
            assert!(!action(id).title().is_empty());
            let (good, bad, _) = action(id).example_values();
            assert!(!good.is_empty() && !bad.is_empty(), "{id}");
            for args in bad {
                assert!(action(id).prepare(args.clone()).is_err(), "{id}: {args}");
            }
        }
    }
}
