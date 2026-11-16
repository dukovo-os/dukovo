// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The updates' actions (ADR 0515): what the last check found, a check
//! now, the slots, the history and the advisories; how the machine
//! updates and the checks it makes after an update; and an update taken,
//! unstaged, gone back on, held back or let go. The area is `updates`, the
//! scope a token needs.
//!
//! The reads are the person's own, as the commands print them, but the
//! slots, which are the administrator's to read. A change is an
//! administrator's.
//!
//! What the settings are is in two files of `/system/settings/sysupdate`,
//! which admind sees: `updates.toml` for how the machine updates and
//! `health.toml` for what it checks after an update. Each change of them
//! names its file and has a probe, a read of what it moves, so that the
//! answer says whether it changed anything. A slot is not a setting:
//! applying an update, unstaging it and going back change none of the
//! files, and the last two are compared through `sysupdate status`.
//!
//! The search for a bad change is [`super::bisect`]'s.

use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{Map, Value, json};

use super::apps::{check_id, id_body, list_argv as apps_argv, picked as app_picked};
use super::{Action, Args, Bad, Examples, Field, Nothing, Runs, Tests, Undo};
use crate::sysupdate::{health, online};
use crate::update::checks;

/// The area of these actions: the scope a token needs, `updates`.
const AREA: &str = "updates";

/// The tests of these actions, which `lab/actions.toml` names.
const TESTS: Tests = Tests {
    checks: "admind::actions::updates::tests",
    route: "admin-updates",
    same_as_command: "admin-updates",
    browser: "web-console/updates",
    ansible: "web-console/ansible",
};

/// How the machine updates: the file `update set`, `update hold` and
/// `sysupdate channel` write.
const SETTINGS: &str = online::SETTINGS;

/// What the machine checks after an update: the file `update checks`
/// writes.
const HEALTH: &str = health::CONFIG;

/// The channels a machine follows, as `sysupdate channel` takes them.
const CHANNELS: &[&str] = &["stable", "beta", "nightly"];

/// How often the machine looks for updates, and what it does with one.
const CHECK: &[&str] = &["notify", "download", "off"];

/// What the machine does with the apps' updates.
const APPS: &[&str] = &["notify", "auto"];

/// What the machine does with updates on a connection that is metered.
const METERED: &[&str] = &["wait", "download"];

/// What the time of a restart matches: `HH:MM`, from 00:00 to 23:59.
const TIME_PATTERN: &str = "^([01][0-9]|2[0-3]):[0-5][0-9]$";

/// What a check's name matches, as `update checks` takes one.
const CHECK_NAME_PATTERN: &str = "^[A-Za-z0-9][A-Za-z0-9 ._-]*$";

/// What a service's name matches, as `manifest` checks one.
const SERVICE_PATTERN: &str = "^[A-Za-z0-9_][A-Za-z0-9_.-]*$";

/// What the web address of a check matches, as far as a schema can say:
/// no space of any kind is the code's.
const URL_PATTERN: &str = "^https?://[^ ]+$";

/// What the item of an update to apply matches: the system, the firmware,
/// the apps, or one app.
const APPLY_PATTERN: &str = concat!("^(system|firmware|apps|", id_body!(), ")$");

/// What the item of a hold matches: the system, the firmware, or one app.
const HOLD_PATTERN: &str = concat!("^(system|firmware|", id_body!(), ")$");

/// The most characters of an item that is an app's ID with its line.
const ITEM_MAX: usize = super::apps::ID_MAX;

/// The most characters of a service's name: a resource domain's.
const SERVICE_MAX: usize = radix_abi::resource_domain::NAME_MAX as usize;

/// `words` as "a, b or c".
fn or_list(words: &[&str]) -> String {
    super::backups::or_list(words)
}

/// A read of one of the update commands, which takes nothing and says
/// what it prints as JSON.
macro_rules! read {
    ($(#[$meta:meta])* $name:ident, $id:literal, $path:literal, $runs:expr, $title:literal,
     $answers:expr, [$($word:literal),+]) => {
        $(#[$meta])*
        pub struct $name;

        impl Action for $name {
            const ID: &'static str = $id;
            const AREA: &'static str = AREA;
            const CHANGES: bool = false;
            const PATH: &'static str = $path;
            const RUNS: Runs = $runs;
            const ANSWERS: &'static [i32] = $answers;
            const TESTS: Tests = TESTS;
            const TITLE: &'static str = $title;
            type Args = Nothing;

            fn argv(_: &Nothing) -> Vec<String> {
                [$($word),+].map(String::from).to_vec()
            }

            fn examples() -> Examples<Nothing> {
                Examples { good: vec![Nothing {}], bad: vec![json!({ "all": true })], undo: None }
            }
        }
    };
}

read! {
    /// `update last --json`: the last check that ran as root, which the
    /// daily one of updated keeps, or none, which is status 3.
    Last, "updates.last", "/api/1/updates", Runs::AsThePerson, "Show the last check for updates",
    &[0, 3], ["update", "last", "--json"]
}

read! {
    /// `update check --json`: a check now, of the system, its firmware and
    /// the apps, with what could not be checked said in the answer.
    Check, "updates.check", "/api/1/updates/check", Runs::AsThePerson, "Check for updates", &[0],
    ["update", "check", "--json"]
}

read! {
    /// `sysupdate status --json`: the slots, which version each holds and
    /// which starts next. The slots are an administrator's to read.
    Slots, "updates.slots", "/api/1/updates/slots", Runs::AsAdministrator,
    "Show the slots of the system", &[0], ["sysupdate", "status", "--json"]
}

read! {
    /// `update history --json`: each change of the system and the apps, by
    /// the start that first ran it.
    History, "updates.history", "/api/1/updates/history", Runs::AsThePerson,
    "Show what was updated", &[0], ["update", "history", "--json"]
}

read! {
    /// `update advisories --json`: the security advisories that apply to
    /// what the machine runs. 1 is one that does; 3 that none could be
    /// read.
    Advisories, "updates.advisories", "/api/1/updates/advisories", Runs::AsThePerson,
    "Show the security advisories", &[0, 1, 3], ["update", "advisories", "--json"]
}

read! {
    /// `update settings --json`: how the machine updates, with what is not
    /// written in the file filled in.
    Settings, "updates.settings", "/api/1/updates/settings", Runs::AsThePerson,
    "Show how the machine updates", &[0], ["update", "settings", "--json"]
}

read! {
    /// `update checks --json`: what the machine checks after an update.
    Checks, "updates.checks", "/api/1/updates/checks", Runs::AsThePerson,
    "Show the checks made after an update", &[0], ["update", "checks", "--json"]
}

read! {
    /// `changes --failed --json`: the changes since the last good start
    /// to the newest that failed, and the search for the bad one.
    Failed, "updates.failed", "/api/1/updates/failed", Runs::AsThePerson,
    "Show what changed before a start failed", &[0], ["changes", "--failed", "--json"]
}

/// `update settings --json`, which says what the settings changes move.
fn settings_argv() -> Vec<String> {
    ["update", "settings", "--json"].map(String::from).to_vec()
}

/// `update checks --json`, which says what the checks' changes move.
fn checks_argv() -> Vec<String> {
    ["update", "checks", "--json"].map(String::from).to_vec()
}

/// `sysupdate status --json`, which says what the slots' changes move.
fn status_argv() -> Vec<String> {
    ["sysupdate", "status", "--json"].map(String::from).to_vec()
}

/// The members `keys` of the object `output`, which a command printed as
/// JSON, as text to compare. None when it is not an object that has them
/// all.
fn pick(output: &str, keys: &[&str]) -> Option<String> {
    let document: Value = serde_json::from_str(output.trim()).ok()?;
    let document = document.as_object()?;
    let mut picked = Map::new();
    for key in keys {
        picked.insert((*key).to_string(), document.get(*key)?.clone());
    }
    Some(Value::Object(picked).to_string())
}

/// What `now`, a text [`pick`] made, says of `key`.
fn said(now: &str, key: &str) -> Option<Value> {
    serde_json::from_str::<Value>(now).ok()?.get(key).cloned()
}

/// Reads a time that may be said as `null`: a `null` is `--no-restart`,
/// and a member that is left out is no change of it.
///
/// # Errors
///
/// What serde says of a value that is neither text nor `null`.
fn told<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<Option<String>>, D::Error> {
    Option::<String>::deserialize(deserializer).map(Some)
}

/// Whether `time` is `HH:MM`, the hours from 00 to 23 and the minutes from
/// 00 to 59.
fn is_time(time: &str) -> bool {
    let digit =
        |at: usize| time.as_bytes().get(at).filter(|b| b.is_ascii_digit()).map(|b| b - b'0');
    let (Some(h1), Some(h2), Some(m1), Some(m2)) = (digit(0), digit(1), digit(3), digit(4)) else {
        return false;
    };
    time.len() == 5 && time.as_bytes()[2] == b':' && h1 * 10 + h2 < 24 && m1 < 6 && m2 < 10
}

/// Why a word of `words` is wanted for `field`.
fn word_in(field: &'static str, value: &str, words: &[&str], done: &str) -> Result<(), Bad> {
    if words.contains(&value) {
        return Ok(());
    }
    Err(Bad { field, why: format!("{field} is {}. {done}", or_list(words)) })
}

/// What changes of the settings of how the machine updates: each member
/// that is there is set, and at least one is.
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Setting {
    /// `notify`, `download` or `off`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub check: Option<String>,
    /// `notify` or `auto`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub apps: Option<String>,
    /// Whether the machine takes updates and restarts by itself.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unattended: Option<bool>,
    /// `HH:MM`; `null` for no time of day, which is `--no-restart`. Left
    /// out, the time is as it was.
    #[serde(default, deserialize_with = "told", skip_serializing_if = "Option::is_none")]
    pub restart: Option<Option<String>>,
    /// `wait` or `download`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metered: Option<String>,
}

impl Args for Setting {
    const FIELDS: &'static [Field] = &[
        Field::one_of(
            "check",
            "What the machine does when it looks for updates: notify, download or off.",
            CHECK,
        )
        .optional(),
        Field::one_of(
            "apps",
            "What the machine does with the apps' updates: notify, or auto, which takes them \
             by itself.",
            APPS,
        )
        .optional(),
        Field::flag("unattended", "Whether the machine takes updates and restarts by itself.")
            .optional(),
        Field::text(
            "restart",
            "The time of day the machine restarts to start an update, as 03:30; null for none. \
             It needs unattended updates.",
            5,
            5,
            Some(TIME_PATTERN),
        )
        .optional(),
        Field::one_of(
            "metered",
            "What the machine does with updates on a metered connection: wait, or download.",
            METERED,
        )
        .optional(),
    ];

    fn check(&self) -> Result<(), Bad> {
        const DONE: &str = "Nothing was changed.";
        if self.check.is_none()
            && self.apps.is_none()
            && self.unattended.is_none()
            && self.restart.is_none()
            && self.metered.is_none()
        {
            return Err(Bad {
                field: "check",
                why: "Say what to change: check, apps, unattended, restart or metered. Nothing \
                      was changed."
                    .into(),
            });
        }
        if let Some(check) = &self.check {
            word_in("check", check, CHECK, DONE)?;
        }
        if let Some(apps) = &self.apps {
            word_in("apps", apps, APPS, DONE)?;
        }
        if let Some(metered) = &self.metered {
            word_in("metered", metered, METERED, DONE)?;
        }
        if let Some(Some(time)) = &self.restart {
            if !is_time(time) {
                return Err(Bad {
                    field: "restart",
                    why: "A time of day is written HH:MM, as 03:30, from 00:00 to 23:59. Nothing \
                          was changed."
                        .into(),
                });
            }
            if self.unattended == Some(false) {
                return Err(Bad {
                    field: "restart",
                    why: "A restart time is for a machine that updates by itself, and unattended \
                          is off. Nothing was changed."
                        .into(),
                });
            }
        }
        Ok(())
    }
}

/// What `update settings --json` says that `update set` moves: the five
/// settings, and whether bundled updates by itself, which `apps` also
/// sets.
const SET_KEYS: [&str; 6] = ["check", "apps", "unattended", "restart", "metered", "apps_automatic"];

/// `update set [--check C] [--apps A] [--unattended on|off] [--restart T |
/// --no-restart] [--metered M]`: writes the settings given into
/// updates.toml, and with `apps` tells bundled too, so that the two agree.
pub struct Set;

impl Action for Set {
    const ID: &'static str = "updates.set";
    const AREA: &'static str = AREA;
    const CHANGES: bool = true;
    const PATH: &'static str = "/api/1/updates/settings";
    const RUNS: Runs = Runs::AsAdministrator;
    const TESTS: Tests = TESTS;
    const TITLE: &'static str = "Change how the machine updates";
    type Args = Setting;

    fn argv(args: &Setting) -> Vec<String> {
        let mut argv = vec!["update".to_string(), "set".to_string()];
        if let Some(check) = &args.check {
            argv.extend(["--check".to_string(), check.clone()]);
        }
        if let Some(apps) = &args.apps {
            argv.extend(["--apps".to_string(), apps.clone()]);
        }
        if let Some(unattended) = args.unattended {
            argv.extend(["--unattended".to_string(), if unattended { "on" } else { "off" }.into()]);
        }
        match &args.restart {
            Some(Some(time)) => argv.extend(["--restart".to_string(), time.clone()]),
            Some(None) => argv.push("--no-restart".into()),
            None => {}
        }
        if let Some(metered) = &args.metered {
            argv.extend(["--metered".to_string(), metered.clone()]);
        }
        argv
    }

    fn files(_: &Setting) -> Vec<String> {
        vec![SETTINGS.to_string()]
    }

    fn probe(_: &Setting) -> Option<Vec<String>> {
        Some(settings_argv())
    }

    fn probed(_: &Setting, output: &str) -> Option<String> {
        pick(output, &SET_KEYS)
    }

    /// Each setting that is given is what the settings say already.
    fn settled(args: &Setting, now: &str) -> Option<bool> {
        let now: Value = serde_json::from_str(now).ok()?;
        let is = |key: &str, wanted: Value| now.get(key) == Some(&wanted);
        let mut settled = true;
        if let Some(check) = &args.check {
            settled &= is("check", json!(check));
        }
        if let Some(apps) = &args.apps {
            // `apps` is also bundled's say, which the settings tell.
            settled &= is("apps", json!(apps)) && is("apps_automatic", json!(apps == "auto"));
        }
        if let Some(unattended) = args.unattended {
            settled &= is("unattended", json!(unattended));
            // Off takes the time of the restart with it.
            settled &= unattended || is("restart", Value::Null);
        }
        match &args.restart {
            Some(Some(time)) => settled &= is("restart", json!(time)),
            Some(None) => settled &= is("restart", Value::Null),
            None => {}
        }
        if let Some(metered) = &args.metered {
            settled &= is("metered", json!(metered));
        }
        Some(settled)
    }

    fn examples() -> Examples<Setting> {
        let only = |check: &str| Setting {
            check: Some(check.into()),
            apps: None,
            unattended: None,
            restart: None,
            metered: None,
        };
        Examples {
            good: vec![only("download")],
            bad: vec![
                json!({}),
                json!({ "restart": "25:00" }),
                json!({ "apps": "sometimes" }),
                json!({ "restart": "3:30" }),
                json!({ "unattended": false, "restart": "03:30" }),
            ],
            // The machine starts with the check that notifies.
            undo: Some(Undo { action: Self::ID, args: json!({ "check": "notify" }) }),
        }
    }
}

/// A channel to follow.
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Channel {
    pub channel: String,
}

impl Args for Channel {
    const FIELDS: &'static [Field] = &[Field::one_of(
        "channel",
        "The channel to follow: stable, beta or nightly. An older release than the one the \
         machine runs waits.",
        CHANNELS,
    )];

    fn check(&self) -> Result<(), Bad> {
        word_in("channel", &self.channel, CHANNELS, "Nothing was changed.")
    }
}

/// `sysupdate channel NAME`: the machine follows that channel from now on.
/// It never goes back to an older release: it waits until the channel
/// passes the one it runs.
pub struct SetChannel;

impl Action for SetChannel {
    const ID: &'static str = "updates.channel";
    const AREA: &'static str = AREA;
    const CHANGES: bool = true;
    const PATH: &'static str = "/api/1/updates/channel";
    const RUNS: Runs = Runs::AsAdministrator;
    const TESTS: Tests = TESTS;
    const TITLE: &'static str = "Change the update channel";
    type Args = Channel;

    fn argv(args: &Channel) -> Vec<String> {
        vec!["sysupdate".into(), "channel".into(), args.channel.clone()]
    }

    fn subject(args: &Channel) -> Option<String> {
        Some(args.channel.clone())
    }

    fn files(_: &Channel) -> Vec<String> {
        vec![SETTINGS.to_string()]
    }

    fn probe(_: &Channel) -> Option<Vec<String>> {
        Some(settings_argv())
    }

    fn probed(_: &Channel, output: &str) -> Option<String> {
        pick(output, &["channel"])
    }

    fn settled(args: &Channel, now: &str) -> Option<bool> {
        Some(said(now, "channel")? == json!(args.channel))
    }

    fn examples() -> Examples<Channel> {
        Examples {
            good: vec![Channel { channel: "beta".into() }],
            bad: vec![json!({ "channel": "alpha" }), json!({}), json!({ "channel": "Beta" })],
            undo: Some(Undo { action: Self::ID, args: json!({ "channel": "stable" }) }),
        }
    }
}

/// The field of a check's name.
const CHECK_NAME: Field = Field::text(
    "name",
    "The check's name, as update checks shows it.",
    1,
    checks::NAME_MAX,
    Some(CHECK_NAME_PATTERN),
);

/// The field of a service's name, for the checks.
const SERVICE: Field = Field::text(
    "service",
    "The service's name, as service list shows it.",
    1,
    SERVICE_MAX,
    Some(SERVICE_PATTERN),
);

/// Checks that `name` is a check's.
fn check_name(name: &str, done: &str) -> Result<(), Bad> {
    checks::check_name(name).map_err(|_| Bad {
        field: "name",
        why: format!(
            "A check's name is 1 to {} letters, digits, spaces, dots, hyphens and underscores, \
             and starts with a letter or a digit. {done}",
            checks::NAME_MAX
        ),
    })
}

/// Checks that `service` can be a service's name.
fn check_service(service: &str, done: &str) -> Result<(), Bad> {
    if manifest::service::check_name(service).is_ok() {
        return Ok(());
    }
    Err(Bad {
        field: "service",
        why: format!(
            "A service's name has 1 to {SERVICE_MAX} letters, digits, hyphens, underscores and \
             dots, and starts with a letter, a digit or an underscore. {done}"
        ),
    })
}

/// The check `name` of `output`, which `update checks --json` printed: its
/// row, if there is one; None when `output` does not read.
fn check_row(name: &str, output: &str) -> Option<Option<Value>> {
    let document: Value = serde_json::from_str(output).ok()?;
    let found = document
        .get("checks")?
        .as_array()?
        .iter()
        .find(|row| row.get("name").and_then(Value::as_str) == Some(name));
    Some(found.cloned())
}

/// What the list says of the check `name` that a change of it moves: its
/// command, whether it is required and its time, as text to compare; `none`
/// for a list that has no such check.
fn check_picked(name: &str, output: &str) -> Option<String> {
    Some(match check_row(name, output)? {
        Some(row) => {
            let member = |key: &str| row.get(key).cloned().unwrap_or(Value::Null);
            json!({
                "command": member("command"),
                "required": member("required"),
                "timeout": member("timeout"),
            })
            .to_string()
        }
        None => "none".into(),
    })
}

/// A check to add, which asks for a web page or a service.
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CheckNew {
    pub name: String,
    /// The page to ask for; this or `service`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// The service that must run; this or `url`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub service: Option<String>,
    /// Whether a certificate that does not verify is no failure.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub insecure: Option<bool>,
    /// Whether a failing check holds the update back.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub required: Option<bool>,
    /// The seconds the check may take, from 1 to 600; 30 without it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout: Option<i64>,
}

impl Args for CheckNew {
    const FIELDS: &'static [Field] = &[
        CHECK_NAME,
        Field::text(
            "url",
            "The page to ask for: an http:// or https:// address. This or service.",
            8,
            checks::URL_MAX,
            Some(URL_PATTERN),
        )
        .optional(),
        SERVICE.optional(),
        Field::flag(
            "insecure",
            "Whether a certificate that does not verify is no failure. Only with url.",
        )
        .optional(),
        Field::flag("required", "Whether a check that fails holds the update back.").optional(),
        Field::number(
            "timeout",
            "The seconds the check may take, from 1 to 600. 30 without it.",
            1,
            checks::TIMEOUT_MAX as i64,
        )
        .optional(),
    ];

    fn check(&self) -> Result<(), Bad> {
        const DONE: &str = "Nothing was added.";
        check_name(&self.name, DONE)?;
        let neither = |field: &'static str| Bad {
            field,
            why: format!("Say what to check: a url, or a service, one of the two. {DONE}"),
        };
        match (&self.url, &self.service) {
            (Some(url), None) => {
                checks::check_url(url).map_err(|_| Bad {
                    field: "url",
                    why: format!(
                        "A check asks for an address that starts with http:// or https://, has \
                         no spaces, and is at most {} characters. {DONE}",
                        checks::URL_MAX
                    ),
                })?;
            }
            (None, Some(service)) => {
                check_service(service, DONE)?;
                if self.insecure == Some(true) {
                    return Err(Bad {
                        field: "insecure",
                        why: format!("insecure is for a url, not a service. {DONE}"),
                    });
                }
            }
            (Some(_), Some(_)) => return Err(neither("service")),
            (None, None) => return Err(neither("url")),
        }
        if let Some(timeout) = self.timeout
            && !(1..=checks::TIMEOUT_MAX as i64).contains(&timeout)
        {
            return Err(Bad {
                field: "timeout",
                why: format!("A check may take from 1 to {} seconds. {DONE}", checks::TIMEOUT_MAX),
            });
        }
        Ok(())
    }
}

/// `update checks add NAME (--url URL [--insecure] | --service S)
/// [--required] [--timeout T]`: a check the machine makes after an update.
pub struct CheckAdd;

impl Action for CheckAdd {
    const ID: &'static str = "updates.check-add";
    const AREA: &'static str = AREA;
    const CHANGES: bool = true;
    const PATH: &'static str = "/api/1/updates/checks";
    const RUNS: Runs = Runs::AsAdministrator;
    const TESTS: Tests = TESTS;
    const TITLE: &'static str = "Add a check made after an update";
    type Args = CheckNew;

    fn argv(args: &CheckNew) -> Vec<String> {
        let mut argv = ["update", "checks", "add"].map(String::from).to_vec();
        argv.push(args.name.clone());
        if let Some(url) = &args.url {
            argv.extend(["--url".to_string(), url.clone()]);
            if args.insecure == Some(true) {
                argv.push("--insecure".into());
            }
        }
        if let Some(service) = &args.service {
            argv.extend(["--service".to_string(), service.clone()]);
        }
        if args.required == Some(true) {
            argv.push("--required".into());
        }
        if let Some(timeout) = args.timeout {
            argv.extend(["--timeout".to_string(), timeout.to_string()]);
        }
        argv
    }

    fn subject(args: &CheckNew) -> Option<String> {
        Some(args.name.clone())
    }

    fn files(_: &CheckNew) -> Vec<String> {
        vec![HEALTH.to_string()]
    }

    fn probe(_: &CheckNew) -> Option<Vec<String>> {
        Some(checks_argv())
    }

    fn probed(args: &CheckNew, output: &str) -> Option<String> {
        check_picked(&args.name, output)
    }

    /// A name that is in use is refused, so a check that is there is as
    /// good as made.
    fn settled(_: &CheckNew, now: &str) -> Option<bool> {
        Some(now != "none")
    }

    fn examples() -> Examples<CheckNew> {
        let good = CheckNew {
            name: "web site".into(),
            url: Some("https://localhost/".into()),
            service: None,
            insecure: None,
            required: Some(true),
            timeout: None,
        };
        Examples {
            good: vec![good],
            bad: vec![
                json!({ "name": "x" }),
                json!({ "name": "x", "url": "ftp://a" }),
                json!({ "name": "x", "url": "https://a/", "service": "nginx" }),
                json!({ "name": "x", "service": "nginx", "insecure": true }),
                json!({ "name": "x", "url": "https://a/", "timeout": 601 }),
                json!({ "name": "-x", "url": "https://a/" }),
            ],
            undo: Some(Undo {
                action: "updates.check-remove",
                args: json!({ "name": "web site" }),
            }),
        }
    }
}

/// A check, by its name.
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CheckNamed {
    pub name: String,
}

impl Args for CheckNamed {
    const FIELDS: &'static [Field] = &[CHECK_NAME];

    fn check(&self) -> Result<(), Bad> {
        check_name(&self.name, "Nothing was removed.")
    }
}

/// `update checks remove NAME`: the machine stops making a check after an
/// update. A check that is not there is the command's own refusal.
pub struct CheckRemove;

impl Action for CheckRemove {
    const ID: &'static str = "updates.check-remove";
    const AREA: &'static str = AREA;
    const CHANGES: bool = true;
    const PATH: &'static str = "/api/1/updates/checks/{name}/remove";
    const RUNS: Runs = Runs::AsAdministrator;
    const TESTS: Tests = TESTS;
    const TITLE: &'static str = "Remove a check made after an update";
    type Args = CheckNamed;

    fn argv(args: &CheckNamed) -> Vec<String> {
        ["update", "checks", "remove"]
            .map(String::from)
            .into_iter()
            .chain([args.name.clone()])
            .collect()
    }

    fn subject(args: &CheckNamed) -> Option<String> {
        Some(args.name.clone())
    }

    fn files(_: &CheckNamed) -> Vec<String> {
        vec![HEALTH.to_string()]
    }

    fn probe(_: &CheckNamed) -> Option<Vec<String>> {
        Some(checks_argv())
    }

    fn probed(args: &CheckNamed, output: &str) -> Option<String> {
        check_picked(&args.name, output)
    }

    fn settled(_: &CheckNamed, now: &str) -> Option<bool> {
        Some(now == "none")
    }

    fn examples() -> Examples<CheckNamed> {
        Examples {
            good: vec![CheckNamed { name: "web site".into() }],
            bad: vec![json!({ "name": "../x" }), json!({ "name": "" }), json!({})],
            // The check the example's own add makes, which is a url check:
            // one with a command of its own has no way back from here.
            undo: Some(Undo {
                action: "updates.check-add",
                args: json!({ "name": "web site", "url": "https://localhost/", "required": true }),
            }),
        }
    }
}

/// A service, and whether it must run after an update.
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Essential {
    pub service: String,
    pub on: bool,
}

impl Args for Essential {
    const FIELDS: &'static [Field] =
        &[SERVICE, Field::flag("on", "Whether the service must run after an update.")];

    fn check(&self) -> Result<(), Bad> {
        check_service(&self.service, "Nothing was changed.")
    }
}

/// `update checks essential SERVICE on|off`: whether the service must run
/// for an update to prove itself.
pub struct SetEssential;

impl Action for SetEssential {
    const ID: &'static str = "updates.essential";
    const AREA: &'static str = AREA;
    const CHANGES: bool = true;
    const PATH: &'static str = "/api/1/updates/essential";
    const RUNS: Runs = Runs::AsAdministrator;
    const TESTS: Tests = TESTS;
    const TITLE: &'static str = "Change which services must run after an update";
    type Args = Essential;

    fn argv(args: &Essential) -> Vec<String> {
        vec![
            "update".into(),
            "checks".into(),
            "essential".into(),
            args.service.clone(),
            if args.on { "on" } else { "off" }.into(),
        ]
    }

    fn subject(args: &Essential) -> Option<String> {
        Some(args.service.clone())
    }

    fn files(_: &Essential) -> Vec<String> {
        vec![HEALTH.to_string()]
    }

    fn probe(_: &Essential) -> Option<Vec<String>> {
        Some(checks_argv())
    }

    /// Whether the service is one the health check goes by.
    fn probed(args: &Essential, output: &str) -> Option<String> {
        let document: Value = serde_json::from_str(output).ok()?;
        let listed = document.get("essential")?.as_array()?;
        let on = listed.iter().any(|name| name.as_str() == Some(args.service.as_str()));
        Some(json!({ "essential": on }).to_string())
    }

    fn settled(args: &Essential, now: &str) -> Option<bool> {
        Some(said(now, "essential")? == json!(args.on))
    }

    fn examples() -> Examples<Essential> {
        Examples {
            good: vec![Essential { service: "nginx".into(), on: true }],
            bad: vec![
                json!({ "service": "../x", "on": true }),
                json!({ "service": "nginx" }),
                json!({ "service": "nginx", "on": "yes" }),
            ],
            undo: Some(Undo { action: Self::ID, args: json!({ "service": "nginx", "on": false }) }),
        }
    }
}

/// An update's item, which the address gives: the system, the firmware,
/// the apps, or one app.
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Item {
    pub item: String,
}

/// Checks `item`, which is `system`, `firmware`, and `apps` when
/// `with_apps`, or an app's ID.
fn check_item(item: &str, with_apps: bool, done: &str) -> Result<(), Bad> {
    if matches!(item, "system" | "firmware") || (with_apps && item == "apps") {
        return Ok(());
    }
    check_id("item", item).map_err(|_| Bad {
        field: "item",
        why: format!(
            "An item is system, firmware, {}or an app's ID, as org.example.app. {done}",
            if with_apps { "apps, " } else { "" }
        ),
    })
}

impl Args for Item {
    const FIELDS: &'static [Field] = &[Field::text(
        "item",
        "What to update: system, firmware, apps, or an app's ID.",
        3,
        ITEM_MAX,
        Some(APPLY_PATTERN),
    )];

    fn check(&self) -> Result<(), Bad> {
        check_item(&self.item, true, "Nothing was done.")
    }
}

/// `update apply --only ITEM --yes`: stages the system or the firmware, or
/// updates the apps, or one, as a task. A stage that fails its verify is
/// UPD-101.
pub struct Apply;

impl Action for Apply {
    const ID: &'static str = "updates.apply";
    const AREA: &'static str = AREA;
    const CHANGES: bool = true;
    const PATH: &'static str = "/api/1/updates/{item}/apply";
    const RUNS: Runs = Runs::AsAdministrator;
    const LONG: bool = true;
    const ERRORS: &'static [&'static str] = &["UPD-101"];
    const TESTS: Tests = TESTS;
    const TITLE: &'static str = "Take an update";
    type Args = Item;

    fn argv(args: &Item) -> Vec<String> {
        ["update", "apply", "--only"]
            .map(String::from)
            .into_iter()
            .chain([args.item.clone(), "--yes".to_string()])
            .collect()
    }

    fn subject(args: &Item) -> Option<String> {
        Some(args.item.clone())
    }

    fn examples() -> Examples<Item> {
        Examples {
            good: vec![Item { item: "system".into() }],
            bad: vec![json!({ "item": "everything" }), json!({ "item": "../x" }), json!({})],
            // The page's way back is updates.unstage, for the system and
            // the firmware; an app's is apps.rollback.
            undo: None,
        }
    }
}

/// `sysupdate unstage`: the update that waits for the next start is not
/// tried; it stays in its slot until the next stage.
pub struct Unstage;

impl Action for Unstage {
    const ID: &'static str = "updates.unstage";
    const AREA: &'static str = AREA;
    const CHANGES: bool = true;
    const PATH: &'static str = "/api/1/updates/unstage";
    const RUNS: Runs = Runs::AsAdministrator;
    const TESTS: Tests = TESTS;
    const TITLE: &'static str = "Do not try the staged update";
    type Args = Nothing;

    fn argv(_: &Nothing) -> Vec<String> {
        ["sysupdate", "unstage"].map(String::from).to_vec()
    }

    fn probe(_: &Nothing) -> Option<Vec<String>> {
        Some(status_argv())
    }

    /// What waits to start.
    fn probed(_: &Nothing, output: &str) -> Option<String> {
        pick(output, &["staged"])
    }

    fn settled(_: &Nothing, now: &str) -> Option<bool> {
        Some(said(now, "staged")?.is_null())
    }

    fn examples() -> Examples<Nothing> {
        Examples {
            good: vec![Nothing {}],
            bad: vec![json!({ "item": "system" })],
            undo: Some(Undo { action: "updates.apply", args: json!({ "item": "system" }) }),
        }
    }
}

/// What an undo does with the settings.
const BEFORE: &[&str] = &["before", "keep"];

/// What goes back, and with which settings.
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Back {
    /// `system` or `firmware`.
    pub item: String,
    /// `before`, the settings from before the update, or `keep`, today's.
    pub settings: String,
}

impl Args for Back {
    const FIELDS: &'static [Field] = &[
        Field::one_of("item", "What goes back: system or firmware.", &["system", "firmware"]),
        Field::one_of(
            "settings",
            "Which settings it starts with: before, those from before the update, or keep, \
             today's.",
            BEFORE,
        ),
    ];

    fn check(&self) -> Result<(), Bad> {
        word_in("item", &self.item, &["system", "firmware"], "Nothing was done.")?;
        word_in("settings", &self.settings, BEFORE, "Nothing was done.")
    }
}

/// `update undo ITEM (--settings-from-before | --keep-settings)`: the
/// previous version is what the next start runs. It does it again the
/// other way, since sysupdate keeps the version it left.
pub struct Undoing;

impl Action for Undoing {
    const ID: &'static str = "updates.undo";
    const AREA: &'static str = AREA;
    const CHANGES: bool = true;
    const PATH: &'static str = "/api/1/updates/{item}/undo";
    const RUNS: Runs = Runs::AsAdministrator;
    const TESTS: Tests = TESTS;
    const TITLE: &'static str = "Go back to the version before";
    type Args = Back;

    fn argv(args: &Back) -> Vec<String> {
        let settings =
            if args.settings == "before" { "--settings-from-before" } else { "--keep-settings" };
        ["update", "undo"]
            .map(String::from)
            .into_iter()
            .chain([args.item.clone(), settings.to_string()])
            .collect()
    }

    fn subject(args: &Back) -> Option<String> {
        Some(args.item.clone())
    }

    fn probe(_: &Back) -> Option<Vec<String>> {
        Some(status_argv())
    }

    /// What starts next, and what it goes back to.
    fn probed(_: &Back, output: &str) -> Option<String> {
        pick(output, &["default", "previous"])
    }

    fn examples() -> Examples<Back> {
        Examples {
            good: vec![Back { item: "system".into(), settings: "keep".into() }],
            bad: vec![
                json!({ "item": "apps", "settings": "keep" }),
                json!({ "item": "system" }),
                json!({ "item": "system", "settings": "sometimes" }),
            ],
            undo: Some(Undo {
                action: Self::ID,
                args: json!({ "item": "system", "settings": "keep" }),
            }),
        }
    }
}

/// An item to hold back or let go: the system, the firmware or one app.
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Held {
    pub item: String,
}

impl Args for Held {
    const FIELDS: &'static [Field] = &[Field::text(
        "item",
        "What to hold back: system, firmware, or an app's ID.",
        3,
        ITEM_MAX,
        Some(HOLD_PATTERN),
    )];

    fn check(&self) -> Result<(), Bad> {
        check_item(&self.item, false, "Nothing was done.")
    }
}

/// Whether `item` is held in the settings (the system's and the
/// firmware's) and not by bundled.
fn in_settings(item: &str) -> bool {
    matches!(item, "system" | "firmware")
}

/// `update hold ITEM` or `update unhold ITEM`: a newer version is taken as
/// usual, and the held one is not, or the hold is lifted.
macro_rules! holding {
    ($(#[$meta:meta])* $name:ident, $id:literal, $path:literal, $title:literal, $word:literal,
     $on:expr, $other:literal) => {
        $(#[$meta])*
        pub struct $name;

        impl Action for $name {
            const ID: &'static str = $id;
            const AREA: &'static str = AREA;
            const CHANGES: bool = true;
            const PATH: &'static str = $path;
            const RUNS: Runs = Runs::AsAdministrator;
            const TESTS: Tests = TESTS;
            const TITLE: &'static str = $title;
            type Args = Held;

            fn argv(args: &Held) -> Vec<String> {
                vec!["update".into(), $word.into(), args.item.clone()]
            }

            fn subject(args: &Held) -> Option<String> {
                Some(args.item.clone())
            }

            /// The system's and the firmware's holds are in updates.toml;
            /// an app's is bundled's, whose state admind does not see.
            fn files(args: &Held) -> Vec<String> {
                if in_settings(&args.item) { vec![SETTINGS.to_string()] } else { Vec::new() }
            }

            fn probe(args: &Held) -> Option<Vec<String>> {
                Some(if in_settings(&args.item) { settings_argv() } else { apps_argv() })
            }

            fn probed(args: &Held, output: &str) -> Option<String> {
                if !in_settings(&args.item) {
                    return app_picked(&args.item, output);
                }
                let document: Value = serde_json::from_str(output).ok()?;
                let held = document.get("hold")?.get(&args.item)?.clone();
                Some(json!({ "held": held }).to_string())
            }

            /// Held when the version, or for an app the flag, says so.
            fn settled(args: &Held, now: &str) -> Option<bool> {
                let held = if in_settings(&args.item) {
                    !said(now, "held")?.is_null()
                } else {
                    said(now, "held")?.as_bool()?
                };
                Some(held == $on)
            }

            fn examples() -> Examples<Held> {
                Examples {
                    good: vec![Held { item: "system".into() }],
                    bad: vec![json!({ "item": "everything" }), json!({ "item": "apps" }), json!({})],
                    undo: Some(Undo { action: $other, args: json!({ "item": "system" }) }),
                }
            }
        }
    };
}

holding! {
    /// `update hold ITEM`.
    Hold, "updates.hold", "/api/1/updates/{item}/hold", "Hold an update back", "hold", true,
    "updates.unhold"
}

holding! {
    /// `update unhold ITEM`.
    Unhold, "updates.unhold", "/api/1/updates/{item}/unhold", "Stop holding an update back",
    "unhold", false, "updates.hold"
}

#[cfg(test)]
mod tests {
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

    /// `update settings --json` of a machine on the beta channel that
    /// updates by itself, with the system held back.
    const SETTINGS_JSON: &str = r#"{
        "format": 1, "channel": "beta", "check": "notify", "apps": "notify",
        "metered": "wait", "unattended": true, "restart": "03:30",
        "hold": {"system": "1.0.0-alpha.2", "firmware": null},
        "held": null, "apps_automatic": false, "apps_disagree": false,
        "server": "https://updates.dukovo.org/system/", "canary": false
    }"#;

    /// `update checks --json` of a machine with one check of its own.
    const CHECKS_JSON: &str = r#"{
        "format": 1, "essential": ["logd", "authd", "nginx"], "stable": 10, "deadline": 120,
        "checks": [
            {"name": "web site", "command": ["curl", "-fsS", "--max-time", "30", "https://localhost/"],
             "required": true, "timeout": 30, "kind": "url", "target": "https://localhost/"}
        ]
    }"#;

    /// `sysupdate status --json` with an update that waits to start.
    const STATUS_JSON: &str = r#"{
        "format": 1, "running": "a", "default": "a", "previous": "b",
        "staged": {"slot": "b", "tries_left": 3}, "next": "b"
    }"#;

    /// The members of `text`, a JSON object, as a value to compare.
    fn value(text: &str) -> Value {
        serde_json::from_str(text).unwrap()
    }

    #[test]
    fn the_reads_are_the_commands_json_and_only_the_slots_are_an_administrator_s() {
        let reads: [(&str, &str, Runs, &[i32], &[&str]); 8] = [
            (
                "updates.last",
                "/api/1/updates",
                Runs::AsThePerson,
                &[0, 3],
                &["update", "last", "--json"],
            ),
            (
                "updates.check",
                "/api/1/updates/check",
                Runs::AsThePerson,
                &[0],
                &["update", "check", "--json"],
            ),
            (
                "updates.slots",
                "/api/1/updates/slots",
                Runs::AsAdministrator,
                &[0],
                &["sysupdate", "status", "--json"],
            ),
            (
                "updates.history",
                "/api/1/updates/history",
                Runs::AsThePerson,
                &[0],
                &["update", "history", "--json"],
            ),
            (
                "updates.advisories",
                "/api/1/updates/advisories",
                Runs::AsThePerson,
                &[0, 1, 3],
                &["update", "advisories", "--json"],
            ),
            (
                "updates.settings",
                "/api/1/updates/settings",
                Runs::AsThePerson,
                &[0],
                &["update", "settings", "--json"],
            ),
            (
                "updates.checks",
                "/api/1/updates/checks",
                Runs::AsThePerson,
                &[0],
                &["update", "checks", "--json"],
            ),
            (
                "updates.failed",
                "/api/1/updates/failed",
                Runs::AsThePerson,
                &[0],
                &["changes", "--failed", "--json"],
            ),
        ];
        for (id, path, runs, answers, argv) in reads {
            let read = action(id);
            assert_eq!(
                (read.changes(), read.runs(), read.path(), read.long(), read.area()),
                (false, runs, path, false, "updates"),
                "{id}"
            );
            assert!(!read.posts() && !read.compares() && read.errors().is_empty(), "{id}");
            assert_eq!(read.answers(), answers, "{id}");
            let prepared = read.prepare(json!({})).unwrap();
            assert_eq!(prepared.argv, argv, "{id}");
            assert!(prepared.files.is_empty() && prepared.probe.is_none(), "{id}");
            assert_eq!(prepared.ansible.become_root, runs == Runs::AsAdministrator, "{id}");
            assert_eq!(refused(id, json!({ "all": true })).0, Kind::BadRequest, "{id}");
        }
    }

    #[test]
    fn a_setting_is_a_flag_of_update_set_and_a_null_restart_is_no_restart() {
        let set = action("updates.set");
        assert_eq!(
            (set.changes(), set.runs(), set.long(), set.path(), set.compares()),
            (true, Runs::AsAdministrator, false, "/api/1/updates/settings", true)
        );
        let all = set
            .prepare(json!({
                "check": "download", "apps": "auto", "unattended": true, "restart": "03:30",
                "metered": "download"
            }))
            .unwrap();
        assert_eq!(
            all.argv,
            [
                "update",
                "set",
                "--check",
                "download",
                "--apps",
                "auto",
                "--unattended",
                "on",
                "--restart",
                "03:30",
                "--metered",
                "download"
            ]
        );
        assert_eq!(all.files, [SETTINGS]);
        assert_eq!(all.probe.as_deref(), Some(&settings_argv()[..]));
        assert!(all.ansible.become_root);
        let off = set.prepare(json!({ "unattended": false, "restart": null })).unwrap();
        assert_eq!(off.argv, ["update", "set", "--unattended", "off", "--no-restart"]);
        // The null is kept, so that the journal says what was asked.
        assert_eq!(off.args, json!({ "unattended": false, "restart": null }));
        let one = set.prepare(json!({ "metered": "wait" })).unwrap();
        assert_eq!(one.argv, ["update", "set", "--metered", "wait"]);
        assert_eq!(one.args, json!({ "metered": "wait" }));
        // Left out is no change, which a null is not.
        let setting: Setting = serde_json::from_value(json!({ "check": "off" })).unwrap();
        assert_eq!(setting.restart, None);
        let setting: Setting = serde_json::from_value(json!({ "restart": null })).unwrap();
        assert_eq!(setting.restart, Some(None));
    }

    #[test]
    fn a_setting_that_is_not_one_is_refused_with_what_was_not_done() {
        let (kind, field, words) = refused("updates.set", json!({}));
        assert_eq!((kind, field), (Kind::BadValue, Some("check")));
        assert_eq!(
            words,
            "Say what to change: check, apps, unattended, restart or metered. Nothing was changed."
        );
        let (_, field, words) = refused("updates.set", json!({ "apps": "sometimes" }));
        assert_eq!(field, Some("apps"));
        assert_eq!(words, "apps is notify or auto. Nothing was changed.");
        for (args, field) in [
            (json!({ "check": "daily" }), "check"),
            (json!({ "check": "" }), "check"),
            (json!({ "metered": "never" }), "metered"),
            (json!({ "restart": "25:00" }), "restart"),
            (json!({ "restart": "3:30" }), "restart"),
            (json!({ "restart": "03:60" }), "restart"),
            (json!({ "restart": "+3:30" }), "restart"),
            (json!({ "restart": "003:3" }), "restart"),
            (json!({ "unattended": false, "restart": "03:30" }), "restart"),
        ] {
            let (kind, found, _) = refused("updates.set", args.clone());
            assert_eq!((kind, found), (Kind::BadValue, Some(field)), "{args}");
        }
        for args in [
            json!({ "check": 3 }),
            json!({ "restart": 330 }),
            json!({ "unattended": "on" }),
            json!({ "unattended": 1 }),
            json!({ "channel": "beta" }),
        ] {
            assert_eq!(refused("updates.set", args.clone()).0, Kind::BadRequest, "{args}");
        }
        let (_, _, words) =
            refused("updates.set", json!({ "unattended": false, "restart": "03:30" }));
        assert!(words.ends_with("Nothing was changed."), "{words}");
    }

    #[test]
    fn a_time_is_hours_and_minutes_with_two_digits_each() {
        for time in ["00:00", "03:30", "09:05", "19:59", "23:59"] {
            assert!(is_time(time), "{time}");
        }
        for time in [
            "",
            "24:00",
            "23:60",
            "3:30",
            "03:3",
            "0330",
            "03-30",
            "03:30 ",
            " 3:30",
            "+3:30",
            "003:3",
            "03:300",
            "ab:cd",
            "０３:３０",
            "03:30\n",
        ] {
            assert!(!is_time(time), "{time:?}");
        }
    }

    #[test]
    fn the_settings_probe_picks_what_set_moves_and_check_mode_reads_it() {
        let set = action("updates.set");
        let args = json!({ "check": "download" });
        let picked = set.probed(&args, SETTINGS_JSON).unwrap();
        assert_eq!(
            value(&picked),
            json!({
                "check": "notify", "apps": "notify", "unattended": true, "restart": "03:30",
                "metered": "wait", "apps_automatic": false
            })
        );
        // What the change does not move is not part of it.
        let elsewhere = SETTINGS_JSON.replace("\"canary\": false", "\"canary\": true");
        assert_eq!(set.probed(&args, &elsewhere), Some(picked.clone()));
        for output in ["", "not json", "[]", r#"{"check": "notify"}"#] {
            assert_eq!(set.probed(&args, output), None, "{output:?}");
        }
        let settled = |args: Value, edit: &dyn Fn(&str) -> String| {
            let picked = set.probed(&args, &edit(SETTINGS_JSON)).unwrap();
            set.settled(&args, &picked)
        };
        let same = |text: &str| text.to_string();
        assert_eq!(settled(json!({ "check": "notify" }), &same), Some(true));
        assert_eq!(settled(json!({ "check": "download" }), &same), Some(false));
        assert_eq!(settled(json!({ "metered": "wait", "unattended": true }), &same), Some(true));
        assert_eq!(settled(json!({ "metered": "download" }), &same), Some(false));
        assert_eq!(settled(json!({ "restart": "03:30" }), &same), Some(true));
        assert_eq!(settled(json!({ "restart": "04:00" }), &same), Some(false));
        assert_eq!(settled(json!({ "restart": null }), &same), Some(false));
        let none = |text: &str| text.replace("\"restart\": \"03:30\"", "\"restart\": null");
        assert_eq!(settled(json!({ "restart": null }), &none), Some(true));
        assert_eq!(settled(json!({ "unattended": true }), &none), Some(true));
        // Off takes the time with it, so it is settled only without one.
        assert_eq!(settled(json!({ "unattended": false }), &same), Some(false));
        let off = |text: &str| none(text).replace("\"unattended\": true", "\"unattended\": false");
        assert_eq!(settled(json!({ "unattended": false }), &off), Some(true));
        // `apps` is also what bundled does: both must say it.
        assert_eq!(settled(json!({ "apps": "notify" }), &same), Some(true));
        assert_eq!(settled(json!({ "apps": "auto" }), &same), Some(false));
        let half = |text: &str| text.replace("\"apps\": \"notify\"", "\"apps\": \"auto\"");
        assert_eq!(settled(json!({ "apps": "auto" }), &half), Some(false));
        let both = |text: &str| {
            half(text).replace("\"apps_automatic\": false", "\"apps_automatic\": true")
        };
        assert_eq!(settled(json!({ "apps": "auto" }), &both), Some(true));
        assert_eq!(settled(json!({ "apps": "notify" }), &both), Some(false));
        assert_eq!(set.settled(&args, "not json"), None);
    }

    #[test]
    fn a_channel_is_changed_by_sysupdate_and_read_from_the_settings() {
        let channel = action("updates.channel");
        assert_eq!(
            (channel.changes(), channel.runs(), channel.long(), channel.path()),
            (true, Runs::AsAdministrator, false, "/api/1/updates/channel")
        );
        let prepared = channel.prepare(json!({ "channel": "nightly" })).unwrap();
        assert_eq!(prepared.argv, ["sysupdate", "channel", "nightly"]);
        assert_eq!(prepared.files, [SETTINGS]);
        assert_eq!(prepared.probe.as_deref(), Some(&settings_argv()[..]));
        for bad in ["alpha", "Beta", "", "stable "] {
            let (kind, field, words) = refused("updates.channel", json!({ "channel": bad }));
            assert_eq!((kind, field), (Kind::BadValue, Some("channel")), "{bad:?}");
            assert_eq!(words, "channel is stable, beta or nightly. Nothing was changed.");
        }
        assert_eq!(refused("updates.channel", json!({})).0, Kind::BadRequest);
        let args = json!({ "channel": "beta" });
        let picked = channel.probed(&args, SETTINGS_JSON).unwrap();
        assert_eq!(value(&picked), json!({ "channel": "beta" }));
        assert_eq!(channel.settled(&args, &picked), Some(true));
        assert_eq!(channel.settled(&json!({ "channel": "stable" }), &picked), Some(false));
        assert_eq!(channel.probed(&args, r#"{"check": "notify"}"#), None);
    }

    #[test]
    fn a_check_is_added_by_a_page_or_a_service_and_by_one_of_them() {
        let add = action("updates.check-add");
        assert_eq!(
            (add.changes(), add.runs(), add.long(), add.path(), add.compares()),
            (true, Runs::AsAdministrator, false, "/api/1/updates/checks", true)
        );
        assert!(add.posts());
        let page = add
            .prepare(json!({ "name": "web site", "url": "https://localhost/", "required": true }))
            .unwrap();
        assert_eq!(
            page.argv,
            ["update", "checks", "add", "web site", "--url", "https://localhost/", "--required"]
        );
        assert_eq!(page.files, [HEALTH]);
        assert_eq!(page.probe.as_deref(), Some(&checks_argv()[..]));
        let all = add
            .prepare(json!({
                "name": "x", "url": "https://a/", "insecure": true, "required": true,
                "timeout": 5
            }))
            .unwrap();
        assert_eq!(
            all.argv,
            [
                "update",
                "checks",
                "add",
                "x",
                "--url",
                "https://a/",
                "--insecure",
                "--required",
                "--timeout",
                "5"
            ]
        );
        let service = add.prepare(json!({ "name": "db", "service": "postgres" })).unwrap();
        assert_eq!(service.argv, ["update", "checks", "add", "db", "--service", "postgres"]);
        // A flag that is off is not given.
        let off = add
            .prepare(
                json!({ "name": "x", "url": "https://a/", "insecure": false, "required": false }),
            )
            .unwrap();
        assert_eq!(off.argv, ["update", "checks", "add", "x", "--url", "https://a/"]);
        assert_eq!(page.ansible.name, "Add a check made after an update: web site");
    }

    #[test]
    fn a_check_that_is_not_one_is_refused_with_what_was_not_done() {
        let url = "https://a/";
        for (args, field) in [
            (json!({ "name": "x" }), "url"),
            (json!({ "name": "x", "url": url, "service": "nginx" }), "service"),
            (json!({ "name": "x", "service": "nginx", "insecure": true }), "insecure"),
            (json!({ "name": "x", "url": "ftp://a" }), "url"),
            (json!({ "name": "x", "url": "https://" }), "url"),
            (json!({ "name": "x", "url": "https://a b" }), "url"),
            (json!({ "name": "x", "url": "https://a\t" }), "url"),
            (json!({ "name": "x", "service": "../x" }), "service"),
            (json!({ "name": "x", "url": url, "timeout": 0 }), "timeout"),
            (json!({ "name": "x", "url": url, "timeout": 601 }), "timeout"),
            (json!({ "name": "-x", "url": url }), "name"),
            (json!({ "name": "", "url": url }), "name"),
            (json!({ "name": "a/b", "url": url }), "name"),
            (json!({ "name": "x".repeat(65), "url": url }), "name"),
        ] {
            let (kind, found, words) = refused("updates.check-add", args.clone());
            assert_eq!((kind, found), (Kind::BadValue, Some(field)), "{args}");
            assert!(words.ends_with("Nothing was added."), "{args}: {words}");
        }
        for args in [
            json!({ "url": url }),
            json!({ "name": "x", "url": 3 }),
            json!({ "name": "x", "url": url, "timeout": 1.5 }),
            json!({ "name": "x", "url": url, "required": "yes" }),
            json!({ "name": "x", "url": url, "extra": 1 }),
        ] {
            assert_eq!(refused("updates.check-add", args.clone()).0, Kind::BadRequest, "{args}");
        }
        for name in ["x", "web site", "a.b_c-d", &"x".repeat(64)] {
            assert!(
                action("updates.check-add").prepare(json!({ "name": name, "url": url })).is_ok(),
                "{name}"
            );
        }
        let (_, _, words) = refused("updates.check-remove", json!({ "name": "../x" }));
        assert_eq!(
            words,
            "A check's name is 1 to 64 letters, digits, spaces, dots, hyphens and underscores, \
             and starts with a letter or a digit. Nothing was removed."
        );
    }

    #[test]
    fn the_probe_picks_what_a_check_is_and_none_for_one_that_is_not_there() {
        let add = action("updates.check-add");
        let there = json!({ "name": "web site", "url": "https://localhost/" });
        let picked = add.probed(&there, CHECKS_JSON).unwrap();
        assert_eq!(
            value(&picked),
            json!({
                "command": ["curl", "-fsS", "--max-time", "30", "https://localhost/"],
                "required": true, "timeout": 30
            })
        );
        // The kind and the target are the command said another way.
        let moved = CHECKS_JSON.replace("\"kind\": \"url\"", "\"kind\": \"command\"");
        assert_eq!(add.probed(&there, &moved), Some(picked.clone()));
        let other = json!({ "name": "db", "service": "postgres" });
        assert_eq!(add.probed(&other, CHECKS_JSON), Some("none".into()));
        for output in ["", "not json", "[]", r#"{"checks": 3}"#, r#"{"essential": []}"#] {
            assert_eq!(add.probed(&there, output), None, "{output:?}");
        }
        // A name in use is refused, so a check that is there is as good as
        // made, and one that is not is to be.
        assert_eq!(add.settled(&there, &picked), Some(true));
        assert_eq!(add.settled(&other, "none"), Some(false));
        let remove = action("updates.check-remove");
        let named = json!({ "name": "web site" });
        assert_eq!(remove.probed(&named, CHECKS_JSON), Some(picked.clone()));
        assert_eq!(remove.settled(&named, &picked), Some(false));
        assert_eq!(remove.settled(&named, "none"), Some(true));
    }

    #[test]
    fn a_check_is_removed_by_its_name_and_the_page_can_put_it_back() {
        let remove = action("updates.check-remove");
        assert_eq!(
            (remove.changes(), remove.runs(), remove.long(), remove.path()),
            (true, Runs::AsAdministrator, false, "/api/1/updates/checks/{name}/remove")
        );
        let prepared = remove.prepare(json!({ "name": "web site" })).unwrap();
        assert_eq!(prepared.argv, ["update", "checks", "remove", "web site"]);
        assert_eq!(prepared.files, [HEALTH]);
        assert_eq!(prepared.probe.as_deref(), Some(&checks_argv()[..]));
        let (good, _, undo) = remove.example_values();
        assert_eq!(good, [json!({ "name": "web site" })]);
        let undo = undo.unwrap();
        assert_eq!(undo.action, "updates.check-add");
        assert!(action(undo.action).prepare(undo.args).is_ok());
        // What the example's add makes, the example's remove takes away.
        let (good, _, undo) = action("updates.check-add").example_values();
        assert_eq!(good[0]["name"], "web site");
        assert_eq!(
            undo,
            Some(Undo { action: "updates.check-remove", args: json!({ "name": "web site" }) })
        );
    }

    #[test]
    fn a_service_is_essential_or_not_by_the_list_the_health_check_goes_by() {
        let essential = action("updates.essential");
        assert_eq!(
            (essential.changes(), essential.runs(), essential.path()),
            (true, Runs::AsAdministrator, "/api/1/updates/essential")
        );
        let on = json!({ "service": "nginx", "on": true });
        let off = json!({ "service": "nginx", "on": false });
        assert_eq!(
            essential.prepare(on.clone()).unwrap().argv,
            ["update", "checks", "essential", "nginx", "on"]
        );
        let prepared = essential.prepare(off.clone()).unwrap();
        assert_eq!(prepared.argv, ["update", "checks", "essential", "nginx", "off"]);
        assert_eq!(prepared.files, [HEALTH]);
        assert_eq!(prepared.probe.as_deref(), Some(&checks_argv()[..]));
        for (args, kind, field) in [
            (json!({ "service": "../x", "on": true }), Kind::BadValue, Some("service")),
            (json!({ "service": ".x", "on": true }), Kind::BadValue, Some("service")),
            (json!({ "service": "-x", "on": true }), Kind::BadValue, Some("service")),
            (json!({ "service": "", "on": true }), Kind::BadValue, Some("service")),
            (json!({ "service": "nginx" }), Kind::BadRequest, None),
            (json!({ "service": "nginx", "on": "yes" }), Kind::BadRequest, None),
        ] {
            let (found, field_found, _) = refused("updates.essential", args.clone());
            assert_eq!((found, field_found), (kind, field), "{args}");
        }
        let there = essential.probed(&on, CHECKS_JSON).unwrap();
        assert_eq!(value(&there), json!({ "essential": true }));
        let not = essential.probed(&json!({ "service": "sshd", "on": true }), CHECKS_JSON).unwrap();
        assert_eq!(value(&not), json!({ "essential": false }));
        assert_eq!(essential.settled(&on, &there), Some(true));
        assert_eq!(essential.settled(&off, &there), Some(false));
        assert_eq!(essential.settled(&on, &not), Some(false));
        assert_eq!(essential.settled(&off, &not), Some(true));
        assert_eq!(essential.probed(&on, r#"{"checks": []}"#), None);
    }

    #[test]
    fn an_update_is_taken_as_a_task_and_its_item_is_the_system_the_firmware_or_an_app() {
        let apply = action("updates.apply");
        assert_eq!(
            (apply.changes(), apply.runs(), apply.long(), apply.path(), apply.errors()),
            (
                true,
                Runs::AsAdministrator,
                true,
                "/api/1/updates/{item}/apply",
                ["UPD-101"].as_slice()
            )
        );
        // Slots and apps are not files of the settings, and a task runs no
        // probe: there is nothing to compare.
        assert!(!apply.compares());
        for (item, argv) in [
            ("system", ["update", "apply", "--only", "system", "--yes"]),
            ("firmware", ["update", "apply", "--only", "firmware", "--yes"]),
            ("apps", ["update", "apply", "--only", "apps", "--yes"]),
            ("org.nodejs.node@22", ["update", "apply", "--only", "org.nodejs.node@22", "--yes"]),
        ] {
            let prepared = apply.prepare(json!({ "item": item })).unwrap();
            assert_eq!(prepared.argv, argv, "{item}");
            assert!(prepared.files.is_empty() && prepared.probe.is_none(), "{item}");
            assert!(prepared.ansible.become_root, "{item}");
        }
        for item in [
            "everything",
            "",
            "System",
            "../x",
            "update",
            "org.example.app@",
            "org.Example.app",
            "apps ",
            "org.example.app\n",
        ] {
            let (kind, field, words) = refused("updates.apply", json!({ "item": item }));
            assert_eq!((kind, field), (Kind::BadValue, Some("item")), "{item:?}");
            assert_eq!(
                words,
                "An item is system, firmware, apps, or an app's ID, as org.example.app. Nothing \
                 was done.",
                "{item:?}"
            );
        }
        assert_eq!(refused("updates.apply", json!({})).0, Kind::BadRequest);
    }

    #[test]
    fn the_staged_update_is_unstaged_and_the_slots_say_whether_it_is() {
        let unstage = action("updates.unstage");
        assert_eq!(
            (unstage.changes(), unstage.runs(), unstage.long(), unstage.path(), unstage.compares()),
            (true, Runs::AsAdministrator, false, "/api/1/updates/unstage", true)
        );
        let prepared = unstage.prepare(json!({})).unwrap();
        assert_eq!(prepared.argv, ["sysupdate", "unstage"]);
        assert!(prepared.files.is_empty());
        assert_eq!(prepared.probe.as_deref(), Some(&status_argv()[..]));
        assert_eq!(refused("updates.unstage", json!({ "item": "system" })).0, Kind::BadRequest);
        let waiting = unstage.probed(&json!({}), STATUS_JSON).unwrap();
        assert_eq!(value(&waiting), json!({ "staged": { "slot": "b", "tries_left": 3 } }));
        assert_eq!(unstage.settled(&json!({}), &waiting), Some(false));
        let gone = STATUS_JSON.replace(r#"{"slot": "b", "tries_left": 3}"#, "null");
        let none = unstage.probed(&json!({}), &gone).unwrap();
        assert_eq!(unstage.settled(&json!({}), &none), Some(true));
        assert_eq!(unstage.probed(&json!({}), r#"{"running": "a"}"#), None);
        // What the example's way back takes again is the system.
        let (_, _, undo) = unstage.example_values();
        assert_eq!(undo, Some(Undo { action: "updates.apply", args: json!({ "item": "system" }) }));
    }

    #[test]
    fn going_back_names_the_item_and_what_becomes_of_the_settings() {
        let back = action("updates.undo");
        assert_eq!(
            (back.changes(), back.runs(), back.long(), back.path()),
            (true, Runs::AsAdministrator, false, "/api/1/updates/{item}/undo")
        );
        assert_eq!(
            back.prepare(json!({ "item": "system", "settings": "before" })).unwrap().argv,
            ["update", "undo", "system", "--settings-from-before"]
        );
        let kept = back.prepare(json!({ "item": "firmware", "settings": "keep" })).unwrap();
        assert_eq!(kept.argv, ["update", "undo", "firmware", "--keep-settings"]);
        assert!(kept.files.is_empty());
        assert_eq!(kept.probe.as_deref(), Some(&status_argv()[..]));
        for (args, field) in [
            (json!({ "item": "apps", "settings": "keep" }), "item"),
            (json!({ "item": "org.example.app", "settings": "keep" }), "item"),
            (json!({ "item": "system", "settings": "sometimes" }), "settings"),
        ] {
            let (kind, found, words) = refused("updates.undo", args.clone());
            assert_eq!((kind, found), (Kind::BadValue, Some(field)), "{args}");
            assert!(words.ends_with("Nothing was done."), "{words}");
        }
        assert_eq!(refused("updates.undo", json!({ "item": "system" })).0, Kind::BadRequest);
        let args = json!({ "item": "system", "settings": "keep" });
        let picked = back.probed(&args, STATUS_JSON).unwrap();
        assert_eq!(value(&picked), json!({ "default": "a", "previous": "b" }));
        assert_eq!(back.settled(&args, &picked), None);
    }

    #[test]
    fn a_hold_is_the_settings_for_the_system_and_the_firmware_and_bundled_s_for_an_app() {
        for (id, word, other, path, on) in [
            ("updates.hold", "hold", "updates.unhold", "/api/1/updates/{item}/hold", true),
            ("updates.unhold", "unhold", "updates.hold", "/api/1/updates/{item}/unhold", false),
        ] {
            let held = action(id);
            assert_eq!(
                (held.changes(), held.runs(), held.long(), held.path(), held.compares()),
                (true, Runs::AsAdministrator, false, path, true),
                "{id}"
            );
            for item in ["system", "firmware"] {
                let prepared = held.prepare(json!({ "item": item })).unwrap();
                assert_eq!(prepared.argv, ["update", word, item], "{id} {item}");
                assert_eq!(prepared.files, [SETTINGS], "{id} {item}");
                assert_eq!(prepared.probe.as_deref(), Some(&settings_argv()[..]), "{id} {item}");
            }
            // An app's hold is in what bundled keeps: no file of the
            // settings, and the list of the apps says it.
            let app = held.prepare(json!({ "item": "org.example.notes" })).unwrap();
            assert_eq!(app.argv, ["update", word, "org.example.notes"], "{id}");
            assert!(app.files.is_empty(), "{id}");
            assert_eq!(app.probe.as_deref(), Some(&apps_argv()[..]), "{id}");
            let (good, _, undo) = held.example_values();
            assert_eq!(good, [json!({ "item": "system" })], "{id}");
            assert_eq!(
                undo,
                Some(Undo { action: other, args: json!({ "item": "system" }) }),
                "{id}"
            );
            for item in ["apps", "everything", "", "Firmware", "../x", "update"] {
                let (kind, field, words) = refused(id, json!({ "item": item }));
                assert_eq!((kind, field), (Kind::BadValue, Some("item")), "{id} {item:?}");
                assert_eq!(
                    words,
                    "An item is system, firmware, or an app's ID, as org.example.app. Nothing \
                     was done."
                );
            }
            // What the settings say of each, and what the apps' list does.
            let system = json!({ "item": "system" });
            let firmware = json!({ "item": "firmware" });
            let said_system = held.probed(&system, SETTINGS_JSON).unwrap();
            assert_eq!(value(&said_system), json!({ "held": "1.0.0-alpha.2" }), "{id}");
            let said_firmware = held.probed(&firmware, SETTINGS_JSON).unwrap();
            assert_eq!(value(&said_firmware), json!({ "held": null }), "{id}");
            assert_eq!(held.settled(&system, &said_system), Some(on), "{id}");
            assert_eq!(held.settled(&firmware, &said_firmware), Some(!on), "{id}");
            let notes = json!({ "item": "org.nginx.nginx" });
            let list = r#"{"format": 1, "bundles": [
                {"id": "org.nginx.nginx", "version": "1.2.0", "for_everyone": true,
                 "held": true, "update": null}]}"#;
            let said_app = held.probed(&notes, list).unwrap();
            assert_eq!(held.settled(&notes, &said_app), Some(on), "{id}");
            assert_eq!(held.probed(&notes, "[]"), None, "{id}");
            assert_eq!(held.probed(&system, "not json"), None, "{id}");
            // An app that everyone has not got cannot be held or released.
            assert_eq!(held.settled(&notes, "none"), None, "{id}");
        }
    }

    #[test]
    fn the_routes_are_found_by_their_methods_and_a_check_s_name_is_decoded() {
        for (post, path, id) in [
            (false, "/api/1/updates", "updates.last"),
            (false, "/api/1/updates/check", "updates.check"),
            (false, "/api/1/updates/slots", "updates.slots"),
            (false, "/api/1/updates/history", "updates.history"),
            (false, "/api/1/updates/advisories", "updates.advisories"),
            (false, "/api/1/updates/settings", "updates.settings"),
            (false, "/api/1/updates/checks", "updates.checks"),
            (false, "/api/1/updates/failed", "updates.failed"),
            (true, "/api/1/updates/settings", "updates.set"),
            (true, "/api/1/updates/channel", "updates.channel"),
            (true, "/api/1/updates/checks", "updates.check-add"),
            (true, "/api/1/updates/checks/web%20site/remove", "updates.check-remove"),
            (true, "/api/1/updates/essential", "updates.essential"),
            (true, "/api/1/updates/unstage", "updates.unstage"),
            (true, "/api/1/updates/system/apply", "updates.apply"),
            (true, "/api/1/updates/org.nodejs.node@22/apply", "updates.apply"),
            (true, "/api/1/updates/firmware/undo", "updates.undo"),
            (true, "/api/1/updates/system/hold", "updates.hold"),
            (true, "/api/1/updates/org.example.notes/unhold", "updates.unhold"),
        ] {
            let found = find(REGISTRY, post, path);
            assert!(
                matches!(&found, Found::Action(action, _) if action.id() == id),
                "{path}: {found:?}"
            );
        }
        let Found::Action(_, params) =
            find(REGISTRY, true, "/api/1/updates/checks/web%20site/remove")
        else {
            panic!()
        };
        assert_eq!(Value::Object(params), json!({ "name": "web site" }));
        let Found::Action(_, params) =
            find(REGISTRY, true, "/api/1/updates/org.nodejs.node@22/apply")
        else {
            panic!()
        };
        assert_eq!(Value::Object(params), json!({ "item": "org.nodejs.node@22" }));
        assert!(matches!(find(REGISTRY, true, "/api/1/updates/check"), Found::WrongMethod));
        assert!(matches!(find(REGISTRY, false, "/api/1/updates/system/apply"), Found::WrongMethod));
        // The word that names a read is no item of a change.
        assert_eq!(refused("updates.apply", json!({ "item": "check" })).1, Some("item"));
    }

    #[test]
    fn a_token_needs_the_area_and_the_slots_and_the_changes_are_an_unlocked_administrator_s() {
        let call = |list: &str, id: &str| {
            let scopes = Scopes::parse(list).unwrap();
            authorize(&Caller::Token { name: "ci", scopes: &scopes }, action(id))
        };
        for id in [
            "updates.last",
            "updates.check",
            "updates.history",
            "updates.advisories",
            "updates.settings",
            "updates.checks",
            "updates.failed",
        ] {
            assert!(call("updates:read", id).is_ok(), "{id}");
            assert_eq!(call("apps", id).unwrap_err().kind, Kind::OutOfScope, "{id}");
        }
        for id in [
            "updates.slots",
            "updates.set",
            "updates.channel",
            "updates.check-add",
            "updates.check-remove",
            "updates.essential",
            "updates.apply",
            "updates.unstage",
            "updates.undo",
            "updates.hold",
            "updates.unhold",
        ] {
            assert_eq!(call("updates:read", id).unwrap_err().kind, Kind::OutOfScope, "{id}");
            assert_eq!(call("apps", id).unwrap_err().kind, Kind::OutOfScope, "{id}");
            assert!(call("updates", id).is_ok(), "{id}");
            assert!(call("*", id).is_ok(), "{id}");
        }
        let session = |admin: bool, unlocked: bool, id: &str| {
            authorize(&Caller::Session { name: "ann", admin, unlocked }, action(id))
                .map_err(|f| f.kind)
        };
        for id in ["updates.last", "updates.settings", "updates.checks", "updates.failed"] {
            assert_eq!(session(false, false, id), Ok(()), "{id}");
        }
        for id in ["updates.slots", "updates.set", "updates.apply", "updates.hold", "updates.undo"]
        {
            assert_eq!(session(false, true, id), Err(Kind::NotAdmin), "{id}");
            assert_eq!(session(true, false, id), Err(Kind::Locked), "{id}");
            assert_eq!(session(true, true, id), Ok(()), "{id}");
        }
    }

    #[test]
    fn the_examples_are_what_the_boot_and_lab_tests_run() {
        for id in [
            "updates.set",
            "updates.channel",
            "updates.check-add",
            "updates.check-remove",
            "updates.essential",
            "updates.apply",
            "updates.unstage",
            "updates.undo",
            "updates.hold",
            "updates.unhold",
        ] {
            let (good, bad, undo) = action(id).example_values();
            assert_eq!(good.len(), 1, "{id}");
            assert!(action(id).prepare(good[0].clone()).is_ok(), "{id}");
            assert!(!bad.is_empty(), "{id}");
            for args in bad {
                assert!(action(id).prepare(args.clone()).is_err(), "{id}: {args}");
            }
            // The undo is a change of the list, which takes what it is given.
            if let Some(undo) = undo {
                let back = action(undo.action);
                assert!(back.changes(), "{id}");
                assert!(back.prepare(undo.args.clone()).is_ok(), "{id}: {}", undo.args);
            }
        }
        // `apply` is not put back by one action, since what it takes is the
        // system's, the firmware's or an app's.
        assert_eq!(action("updates.apply").example_values().2, None);
        let (good, _, undo) = action("updates.set").example_values();
        assert_eq!(good, [json!({ "check": "download" })]);
        assert_eq!(undo, Some(Undo { action: "updates.set", args: json!({ "check": "notify" }) }));
        let (good, _, undo) = action("updates.channel").example_values();
        assert_eq!(good, [json!({ "channel": "beta" })]);
        assert_eq!(
            undo,
            Some(Undo { action: "updates.channel", args: json!({ "channel": "stable" }) })
        );
        for id in [
            "updates.last",
            "updates.check",
            "updates.slots",
            "updates.history",
            "updates.advisories",
            "updates.settings",
            "updates.checks",
            "updates.failed",
        ] {
            let (good, bad, undo) = action(id).example_values();
            assert_eq!(good, [json!({})], "{id}");
            assert_eq!(undo, None, "{id}");
            for args in bad {
                assert!(action(id).prepare(args.clone()).is_err(), "{id}: {args}");
            }
        }
    }

    #[test]
    fn a_list_of_words_reads_as_a_sentence() {
        assert_eq!(or_list(&["notify", "auto"]), "notify or auto");
        assert_eq!(or_list(&["stable", "beta", "nightly"]), "stable, beta or nightly");
    }
}
