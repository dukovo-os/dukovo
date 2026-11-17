// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Setting up the channels of alerts.toml(5) from the command line (ADR
//! 0485): `alerts email set` and `remove`, `alerts webhook add` and
//! `remove`, and `alerts channel`, which says which kinds of alert a
//! channel is told of.
//!
//! The file is changed with the writer the web console uses
//! ([`crate::settings_edit`]): its comments and the order of its keys stay,
//! and it is written whole or not at all. It holds passwords and tokens, so
//! it is made root's and 0600 before anything is written, and what would be
//! written is read as alertd reads it first: a table that alertd would leave
//! out is refused with alertd's own words, and the file is not changed. Then
//! Spark is asked to have alertd read it again.

use std::cell::Cell;
use std::io::{self, BufRead, Read};
use std::os::unix::fs::MetadataExt;
use std::path::Path;

use toml_edit::{DocumentMut, Item, TableLike};

use super::command::{self, nothing_more};
use super::protocol::{Kinds, Severity};
use crate::alertd::settings::{self, Channel, FILE, Kind, Settings, Target};
use crate::args::{Arg, Args};
use crate::settings_edit::{self, Unedited};
use crate::util::{self, Fail, fail, usage};
use crate::{prompt, service};

/// The ports mail is submitted on.
const PORTS: [u16; 2] = [587, 465];

/// The most bytes of the standard input read for an address and a token.
const INPUT_MOST: u64 = 8192;

/// The most webhooks alerts.toml may name: what the console's action takes.
const WEBHOOKS_MOST: u32 = 32;

/// Which password `alerts email set` gives the table.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Password {
    /// None: the account is not signed in to.
    None,
    /// `--password -`: the first line of the standard input.
    Input,
    /// `--keep-password`: the one the file has.
    Keep,
}

/// What `alerts email set` writes of the `[email]` table: each setting not
/// given is left out of the table, so that it is its default.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct EmailSet {
    pub(super) to: Vec<String>,
    pub(super) server: String,
    pub(super) port: Option<u16>,
    pub(super) user: Option<String>,
    pub(super) password: Password,
    pub(super) from: Option<String>,
    pub(super) severity: Option<Severity>,
    pub(super) cleared: Option<bool>,
    pub(super) about: Kinds,
}

/// `alerts email`.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Email {
    Set(EmailSet),
    Remove,
}

/// What `alerts webhook add` writes: a `[[webhook]]` table, after the
/// others. Its address and token come from the standard input.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct WebhookAdd {
    pub(super) kind: String,
    pub(super) room: Option<String>,
    pub(super) severity: Option<Severity>,
    pub(super) cleared: Option<bool>,
    pub(super) about: Kinds,
}

/// `alerts webhook`.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Webhook {
    Add(WebhookAdd),
    /// Remove the one in this place among the `[[webhook]]` tables.
    Remove(u32),
}

/// A channel, as `alerts channel` and the console name it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Which {
    Email,
    /// The `[[webhook]]` table in this place, from 1.
    Webhook(u32),
    /// The browser with this ID.
    Push(String),
}

/// `alerts channel CHANNEL --about WORDS`.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct Told {
    pub(super) channel: Which,
    pub(super) about: Kinds,
}

/// The words for the kinds, for the usage lines.
const ABOUT_WORDS: &str = "all, or data, stopped, hardware, separated by commas";

/// `alerts email`: `set` or `remove`.
pub(super) fn parse_email(args: &mut Args) -> Result<Email, Fail> {
    match args.subcommand().as_deref().and_then(|name| name.to_str()) {
        Some("set") => parse_email_set(args).map(Email::Set),
        Some("remove") => {
            nothing_more(args)?;
            Ok(Email::Remove)
        }
        _ => Err(usage("alerts email takes set or remove; `alerts --help` says how".into())),
    }
}

/// The options of `alerts email set`.
fn parse_email_set(args: &mut Args) -> Result<EmailSet, Fail> {
    let mut set = EmailSet {
        to: Vec::new(),
        server: String::new(),
        port: None,
        user: None,
        password: Password::None,
        from: None,
        severity: None,
        cleared: None,
        about: Kinds::ALL,
    };
    let mut server = None;
    let mut passwords = 0;
    while let Some(arg) = args.next()? {
        let Arg::Long(name) = &arg else { return Err(args.unknown(&arg)) };
        match name.as_str() {
            "to" => set.to.extend(addresses(&args.text()?)),
            "server" => server = Some(args.text()?),
            "port" => set.port = Some(port_of(&args.text()?)?),
            "user" => set.user = Some(args.text()?),
            "from" => set.from = Some(args.text()?),
            "severity" => set.severity = Some(severity_of(&args.text()?)?),
            "cleared" => set.cleared = Some(yes_or_no(&args.text()?)?),
            "about" => set.about = kinds_of(&args.text()?)?,
            "password" => {
                if args.text()? != "-" {
                    return Err(usage(
                        "--password takes -: the password is the first line of the standard \
                         input"
                            .into(),
                    ));
                }
                set.password = Password::Input;
                passwords += 1;
            }
            "keep-password" => {
                set.password = Password::Keep;
                passwords += 1;
            }
            _ => return Err(args.unknown(&arg)),
        }
    }
    if set.to.is_empty() {
        return Err(usage("--to ADDRESS is needed: the address the alerts go to".into()));
    }
    set.server = server.ok_or_else(|| usage("--server HOST is needed".into()))?;
    if passwords > 1 {
        return Err(usage("--password - and --keep-password are two ways to say one thing".into()));
    }
    match (&set.user, set.password) {
        (Some(_), Password::None) => {
            return Err(usage(
                "--user needs its password: --password - to give it on the standard input, or \
                 --keep-password for the one the file has"
                    .into(),
            ));
        }
        (None, Password::Input | Password::Keep) => {
            return Err(usage("a password goes with --user: the account to sign in as".into()));
        }
        _ => {}
    }
    Ok(set)
}

/// `alerts webhook`: `add` or `remove`.
pub(super) fn parse_webhook(args: &mut Args) -> Result<Webhook, Fail> {
    match args.subcommand().as_deref().and_then(|name| name.to_str()) {
        Some("add") => parse_webhook_add(args).map(Webhook::Add),
        Some("remove") => {
            let number = webhook_number(&operand(args, "NUMBER")?)?;
            Ok(Webhook::Remove(number))
        }
        _ => Err(usage("alerts webhook takes add or remove; `alerts --help` says how".into())),
    }
}

/// The options and the kind of `alerts webhook add`.
fn parse_webhook_add(args: &mut Args) -> Result<WebhookAdd, Fail> {
    let mut add = WebhookAdd {
        kind: String::new(),
        room: None,
        severity: None,
        cleared: None,
        about: Kinds::ALL,
    };
    let mut kind = None;
    while let Some(arg) = args.next()? {
        match &arg {
            Arg::Long(name) if name == "room" => add.room = Some(args.text()?),
            Arg::Long(name) if name == "severity" => {
                add.severity = Some(severity_of(&args.text()?)?);
            }
            Arg::Long(name) if name == "cleared" => add.cleared = Some(yes_or_no(&args.text()?)?),
            Arg::Long(name) if name == "about" => add.about = kinds_of(&args.text()?)?,
            Arg::Operand(operand) if kind.is_none() => {
                kind = Some(command::text_of(operand.clone())?);
            }
            _ => return Err(args.unknown(&arg)),
        }
    }
    let kind =
        kind.ok_or_else(|| usage(format!("a KIND is needed: {}", Kind::NAMES.join(", "))))?;
    if Kind::parse(&kind).is_none() {
        return Err(usage(format!(
            "'{kind}' is not a kind of webhook: {}",
            Kind::NAMES.join(", ")
        )));
    }
    add.kind = kind;
    Ok(add)
}

/// `alerts channel CHANNEL --about WORDS`.
pub(super) fn parse_channel(args: &mut Args) -> Result<Told, Fail> {
    let (mut channel, mut about) = (None, None);
    while let Some(arg) = args.next()? {
        match &arg {
            Arg::Long(name) if name == "about" => about = Some(kinds_of(&args.text()?)?),
            Arg::Operand(operand) if channel.is_none() => {
                channel = Some(which_of(&command::text_of(operand.clone())?)?);
            }
            _ => return Err(args.unknown(&arg)),
        }
    }
    let channel = channel.ok_or_else(|| {
        usage("a CHANNEL is needed: email, webhook:NUMBER or push:ID, as alerts status says".into())
    })?;
    let about = about.ok_or_else(|| usage(format!("--about WORDS is needed: {ABOUT_WORDS}")))?;
    Ok(Told { channel, about })
}

/// The one operand left, called `name`.
fn operand(args: &mut Args, name: &str) -> Result<String, Fail> {
    let mut found = None;
    while let Some(arg) = args.next()? {
        match arg {
            Arg::Operand(operand) if found.is_none() => found = Some(command::text_of(operand)?),
            other => return Err(args.unknown(&other)),
        }
    }
    found.ok_or_else(|| usage(format!("a {name} is needed")))
}

/// The addresses in `text`: one, or several with commas.
fn addresses(text: &str) -> impl Iterator<Item = String> + '_ {
    text.split(',').map(str::trim).filter(|address| !address.is_empty()).map(str::to_string)
}

fn port_of(text: &str) -> Result<u16, Fail> {
    text.trim()
        .parse()
        .ok()
        .filter(|port| PORTS.contains(port))
        .ok_or_else(|| usage("--port is 587 (STARTTLS) or 465 (TLS from the start)".into()))
}

pub(super) fn severity_of(text: &str) -> Result<Severity, Fail> {
    match text {
        "warning" => Ok(Severity::Warning),
        "critical" => Ok(Severity::Critical),
        other => Err(usage(format!("--severity is warning or critical, not '{other}'"))),
    }
}

pub(super) fn yes_or_no(text: &str) -> Result<bool, Fail> {
    match text {
        "yes" => Ok(true),
        "no" => Ok(false),
        other => Err(usage(format!("--cleared is yes or no, not '{other}'"))),
    }
}

pub(super) fn kinds_of(text: &str) -> Result<Kinds, Fail> {
    Kinds::parse(text).map_err(usage)
}

/// The number of a webhook, from 1.
fn webhook_number(text: &str) -> Result<u32, Fail> {
    text.parse().ok().filter(|number| (1..=WEBHOOKS_MOST).contains(number)).ok_or_else(|| {
        usage(format!("'{text}' is not the number of a webhook, 1 to {WEBHOOKS_MOST}"))
    })
}

/// The channel `text` names: `email`, `webhook:N` or `push:ID`.
fn which_of(text: &str) -> Result<Which, Fail> {
    let mistake = || {
        usage(format!(
            "'{text}' is not a channel: email, webhook:NUMBER or push:ID, as `alerts status \
             --json` gives them"
        ))
    };
    if text == "email" {
        return Ok(Which::Email);
    }
    match text.split_once(':') {
        Some(("webhook", number)) => {
            Ok(Which::Webhook(webhook_number(number).map_err(|_| mistake())?))
        }
        Some(("push", id)) if id.len() == 16 && id.bytes().all(|b| b.is_ascii_hexdigit()) => {
            Ok(Which::Push(id.to_ascii_lowercase()))
        }
        _ => Err(mistake()),
    }
}

/// How asking Spark to have alertd read alerts.toml again came out.
#[derive(Debug, PartialEq, Eq)]
enum Reloaded {
    Asked,
    /// alertd is not running, so it reads the file when it starts.
    NotRunning,
}

/// Asks Spark to have alertd read its settings again.
///
/// # Errors
///
/// When Spark cannot be reached or refuses.
fn reload() -> Result<Reloaded, Fail> {
    match service::ask("reload alertd") {
        Ok(_) => Ok(Reloaded::Asked),
        Err(refused) if refused.message == "alertd is not running" => Ok(Reloaded::NotRunning),
        Err(refused) => Err(refused),
    }
}

/// Says that alerts.toml is written, and has alertd read it.
fn finish(said: &str) -> Result<i32, Fail> {
    println!("alerts: {said}");
    match reload() {
        Ok(Reloaded::Asked) => Ok(0),
        Ok(Reloaded::NotRunning) => {
            println!("alerts: alertd is not running; it reads alerts.toml when it starts");
            Ok(0)
        }
        Err(refused) => Err(fail(format!(
            "{FILE} is written, but alertd was not asked to read it: {}; `service alertd reload` does",
            refused.message
        ))),
    }
}

/// Lines of `input`, up to `count`: each without its line end, and empty
/// when the input has ended.
fn lines_of(input: &mut impl BufRead, count: usize) -> io::Result<Vec<String>> {
    let mut lines = Vec::new();
    for _ in 0..count {
        let mut line = String::new();
        input.read_line(&mut line)?;
        lines.push(line.trim_end_matches(['\n', '\r']).to_string());
    }
    Ok(lines)
}

/// What the standard input says, or what the terminal is asked for with
/// `prompts`, with the echo off: a line for each.
fn input_lines(prompts: &[&str]) -> Result<Vec<String>, Fail> {
    if !prompt::interactive() {
        let mut input = io::stdin().lock().take(INPUT_MOST);
        let lines = lines_of(&mut io::BufReader::new(&mut input), prompts.len())
            .map_err(|error| util::failed(&"standard input", error))?;
        if lines.iter().any(|line| line.len() as u64 >= INPUT_MOST) {
            return Err(fail(format!(
                "a line of the standard input is longer than {INPUT_MOST} bytes"
            )));
        }
        return Ok(lines);
    }
    prompts
        .iter()
        .map(|asked| match prompt::ask(asked, false) {
            Ok(prompt::Answer::Line(line)) => String::from_utf8(line)
                .map_err(|_| fail("what was typed is not UTF-8; nothing changed".into())),
            Ok(prompt::Answer::Stopped) => Err(fail("stopped; nothing changed".into())),
            Err(error) => Err(util::failed(&"the terminal", error)),
        })
        .collect()
}

/// Why `lines` of alertd's reading were not a channel: the first line about
/// `table` (`[email]` or `webhook 2`), as alertd would log it, without the
/// file's name.
fn why_left_out(wrong: &[String], table: &str) -> String {
    let own = format!("{FILE}: {table}:");
    match wrong.iter().find(|line| line.starts_with(&own)) {
        Some(line) => {
            let said = line.strip_prefix(&format!("{FILE}: ")).unwrap_or(line);
            format!("{said}; {FILE} is as it was")
        }
        None => format!("{table} is not a channel alertd would set up; {FILE} is as it was"),
    }
}

/// Changes alerts.toml at `path` as `edit` says, if `check` accepts what
/// alertd reads of the result, and gives what `check` gives. `owner` is who
/// the file belongs to: root.
///
/// # Errors
///
/// A file that cannot be read, is not TOML or cannot be written, what `edit`
/// refuses, and what `check` refuses. The file is as it was.
fn change_at<T>(
    path: &Path,
    owner: u32,
    edit: impl FnOnce(&mut DocumentMut) -> Result<(), String>,
    check: impl FnOnce(&Settings, &[String]) -> Result<T, String>,
) -> Result<T, Fail> {
    let old = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == io::ErrorKind::NotFound => String::new(),
        Err(error) => return Err(util::failed(&path.display(), error)),
    };
    let new = settings_edit::edited(&old, edit).map_err(|unedited| match unedited {
        Unedited::Parse { line, message } => {
            let at = line.map_or(String::new(), |line| format!(", at line {line}"));
            fail(format!("{} is not valid TOML{at}: {message}; it is as it was", path.display()))
        }
        Unedited::Edit(why) => fail(why),
    })?;
    let (settings, wrong) =
        settings::read_settings(new.as_deref().unwrap_or(&old), &util::hostname());
    let found = check(&settings, &wrong).map_err(fail)?;
    // Made safe to hold a secret before any is in it, which `write` then keeps.
    tighten(path, owner).map_err(|error| util::failed(&path.display(), error))?;
    if let Some(text) = new {
        settings_edit::write(path, text.as_bytes(), 0o600)
            .map_err(|error| util::failed(&path.display(), error))?;
    }
    Ok(found)
}

/// Makes the file at `path`, if there is one, `owner`'s and no one else's
/// to read, as alertd reads a file that names a channel.
fn tighten(path: &Path, owner: u32) -> io::Result<()> {
    let meta = match std::fs::symlink_metadata(path) {
        // The writer refuses a link and says so.
        Ok(meta) if meta.file_type().is_symlink() => return Ok(()),
        Ok(meta) => meta,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    if meta.uid() != owner {
        std::os::unix::fs::chown(path, Some(owner), Some(owner))?;
    }
    if meta.mode() & 0o077 != 0 {
        std::fs::set_permissions(path, std::os::unix::fs::PermissionsExt::from_mode(0o600))?;
    }
    Ok(())
}

/// [`change_at`] for alerts.toml, which is root's.
fn change<T>(
    edit: impl FnOnce(&mut DocumentMut) -> Result<(), String>,
    check: impl FnOnce(&Settings, &[String]) -> Result<T, String>,
) -> Result<T, Fail> {
    change_at(Path::new(FILE), 0, edit, check)
}

/// Makes `about` of `table` the kinds `kinds`: the list, or no key for every
/// kind.
fn put_about(table: &mut dyn TableLike, kinds: Kinds) {
    match kinds.words() {
        Some(words) => settings_edit::set_strings(table, "about", &words),
        None => {
            settings_edit::remove_key(table, "about");
        }
    }
}

/// Makes `key` of `table` the string `value`, or takes the key out.
fn put_text(table: &mut dyn TableLike, key: &str, value: Option<&str>) {
    match value {
        Some(value) => settings_edit::set_string(table, key, value),
        None => {
            settings_edit::remove_key(table, key);
        }
    }
}

/// The settings every channel's table takes: how severe, whether clears are
/// told, and of which kinds. Each not given is left to its default.
fn put_filter(
    table: &mut dyn TableLike,
    severity: Option<Severity>,
    cleared: Option<bool>,
    about: Kinds,
) {
    put_text(table, "severity", severity.map(Severity::name));
    match cleared {
        Some(cleared) => settings_edit::set_bool(table, "cleared", cleared),
        None => {
            settings_edit::remove_key(table, "cleared");
        }
    }
    put_about(table, about);
}

/// Writes the `[email]` table as `set` says, with `password` as the one
/// the standard input gave.
fn email_table(
    document: &mut DocumentMut,
    set: &EmailSet,
    password: Option<&str>,
) -> Result<(), String> {
    let kept = document
        .get("email")
        .and_then(Item::as_table_like)
        .and_then(|table| table.get("password"))
        .and_then(Item::as_str)
        .map(str::to_string);
    let table = settings_edit::table(document, "email").ok_or("email is not a table")?;
    settings_edit::set_strings(table, "to", &set.to);
    settings_edit::set_string(table, "server", &set.server);
    match set.port {
        Some(port) => settings_edit::set_integer(table, "port", i64::from(port)),
        None => {
            settings_edit::remove_key(table, "port");
        }
    }
    put_text(table, "from", set.from.as_deref());
    put_text(table, "user", set.user.as_deref());
    let secret = match set.password {
        Password::Input => password.map(str::to_string),
        Password::Keep => Some(kept.ok_or(
            "no password is kept: the file has none, so give it with --password - on the \
             standard input",
        )?),
        Password::None => None,
    };
    put_text(table, "password", secret.as_deref());
    put_filter(table, set.severity, set.cleared, set.about);
    Ok(())
}

/// Adds a `[[webhook]]` table as `add` says, with the address and the token
/// of the standard input; gives its place among them.
fn webhook_table(
    document: &mut DocumentMut,
    add: &WebhookAdd,
    url: &str,
    token: Option<&str>,
) -> Result<u32, String> {
    let table =
        settings_edit::push_table(document, "webhook").ok_or("webhook is not a list of tables")?;
    settings_edit::set_string(table, "kind", &add.kind);
    settings_edit::set_string(table, "url", url);
    put_text(table, "token", token);
    put_text(table, "room", add.room.as_deref());
    put_filter(table, add.severity, add.cleared, add.about);
    Ok(webhooks_in(document))
}

/// How many `[[webhook]]` tables the document has.
fn webhooks_in(document: &DocumentMut) -> u32 {
    let count = document
        .get("webhook")
        .and_then(Item::as_array_of_tables)
        .map_or(0, toml_edit::ArrayOfTables::len);
    u32::try_from(count).unwrap_or(u32::MAX)
}

/// The `email` channel of `settings`.
fn email_channel(settings: &Settings) -> Option<&Channel> {
    settings.channels.iter().find(|channel| matches!(channel.target, Target::Email(_)))
}

/// The channel of the `[[webhook]]` table in place `number` of `settings`.
fn webhook_channel(settings: &Settings, number: u32) -> Option<&Channel> {
    settings
        .channels
        .iter()
        .find(|channel| matches!(&channel.target, Target::Webhook(hook) if hook.place == number))
}

/// `alerts email set` and `alerts email remove`.
pub(super) fn run_email(email: &Email) -> Result<i32, Fail> {
    match email {
        Email::Set(set) => {
            let password = match set.password {
                Password::Input => {
                    let mut lines = input_lines(&["Password: "])?;
                    let line = lines.pop().unwrap_or_default();
                    if line.is_empty() {
                        return Err(fail(
                            "the standard input has no password on its first line".into(),
                        ));
                    }
                    Some(line)
                }
                Password::Keep | Password::None => None,
            };
            change(
                |document| email_table(document, set, password.as_deref()),
                |settings, wrong| {
                    email_channel(settings)
                        .map(|_| ())
                        .ok_or_else(|| why_left_out(wrong, "[email]"))
                },
            )?;
            finish("email set")
        }
        Email::Remove => {
            change(
                |document| {
                    if settings_edit::take_table(document, "email") {
                        Ok(())
                    } else {
                        Err("there is no email channel; `alerts status` lists them".into())
                    }
                },
                |_, _| Ok(()),
            )?;
            finish("email removed")
        }
    }
}

/// `alerts webhook add` and `alerts webhook remove`.
pub(super) fn run_webhook(webhook: &Webhook) -> Result<i32, Fail> {
    match webhook {
        Webhook::Add(add) => {
            let mut lines = input_lines(&["Address: ", "Token (nothing if there is none): "])?;
            let token = lines.pop().filter(|token| !token.is_empty());
            let url = lines.pop().unwrap_or_default();
            if url.is_empty() {
                return Err(fail(
                    "the first line of the standard input is the webhook's address, and it is empty"
                        .into(),
                ));
            }
            let number = Cell::new(0);
            let name = change(
                |document| {
                    number.set(webhook_table(document, add, &url, token.as_deref())?);
                    Ok(())
                },
                |settings, wrong| {
                    let at = number.get();
                    webhook_channel(settings, at)
                        .map(|channel| channel.name.clone())
                        .ok_or_else(|| why_left_out(wrong, &format!("webhook {at}")))
                },
            )?;
            finish(&format!("webhook {} added ({name})", number.get()))
        }
        Webhook::Remove(number) => {
            let seen = Cell::new(0);
            change(
                |document| {
                    let found = settings_edit::remove_table(document, "webhook", |_| {
                        seen.set(seen.get() + 1);
                        seen.get() == *number
                    });
                    if found {
                        Ok(())
                    } else {
                        Err(format!("there is no webhook {number}; `alerts status` lists them"))
                    }
                },
                |_, _| Ok(()),
            )?;
            finish(&format!("webhook {number} removed"))
        }
    }
}

/// `alerts channel`: the kinds a channel is told of.
pub(super) fn run_channel(told: &Told) -> Result<i32, Fail> {
    let said = match &told.channel {
        Which::Email => {
            let name = change(
                |document| {
                    let table = document
                        .get_mut("email")
                        .and_then(Item::as_table_like_mut)
                        .ok_or("there is no email channel; `alerts status` lists them")?;
                    put_about(table, told.about);
                    Ok(())
                },
                |settings, _| Ok(email_channel(settings).map(|channel| channel.name.clone())),
            )?;
            format!("email{}", in_brackets(name.as_deref()))
        }
        Which::Webhook(number) => {
            let name = change(
                |document| {
                    let table = document
                        .get_mut("webhook")
                        .and_then(Item::as_array_of_tables_mut)
                        .and_then(|tables| {
                            tables.get_mut(usize::try_from(number - 1).unwrap_or(usize::MAX))
                        })
                        .ok_or_else(|| {
                            format!("there is no webhook {number}; `alerts status` lists them")
                        })?;
                    put_about(table, told.about);
                    Ok(())
                },
                |settings, _| {
                    Ok(webhook_channel(settings, *number).map(|channel| channel.name.clone()))
                },
            )?;
            format!("webhook {number}{}", in_brackets(name.as_deref()))
        }
        Which::Push(id) => {
            let name = super::browsers::set_about(id, told.about)?;
            format!("push {id}{}", in_brackets(Some(&name)))
        }
    };
    println!("alerts: {said} is told of: {}", told.about.phrases());
    // A browser is changed by alertd itself; the others by the file.
    if matches!(told.channel, Which::Push(_)) {
        return Ok(0);
    }
    match reload()? {
        Reloaded::Asked => Ok(0),
        Reloaded::NotRunning => {
            println!("alerts: alertd is not running; it reads alerts.toml when it starts");
            Ok(0)
        }
    }
}

/// ` (name)`, or nothing.
fn in_brackets(name: Option<&str>) -> String {
    name.map_or(String::new(), |name| format!(" ({name})"))
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;

    use super::*;
    use crate::alerts::protocol::About;

    fn args(words: &[&str]) -> Args {
        let mut args = Args::new("alerts", words.iter().map(OsString::from).collect());
        // `alerts` has been read: what is left is the subcommand.
        assert!(args.subcommand().is_some() || words.is_empty());
        args
    }

    fn email_set(words: &[&str]) -> Result<EmailSet, Fail> {
        match parse_email(&mut args(&[&["email"], words].concat()))? {
            Email::Set(set) => Ok(set),
            Email::Remove => panic!("not set"),
        }
    }

    fn says(result: Result<impl std::fmt::Debug, Fail>) -> String {
        let fail = result.unwrap_err();
        assert!(fail.usage, "{}", fail.message);
        fail.message
    }

    #[test]
    fn email_set_takes_what_the_table_has_and_each_not_given_is_its_default() {
        let set =
            email_set(&["set", "--to", "ops@example.com", "--server", "smtp.example.com"]).unwrap();
        assert_eq!(
            set,
            EmailSet {
                to: vec!["ops@example.com".into()],
                server: "smtp.example.com".into(),
                port: None,
                user: None,
                password: Password::None,
                from: None,
                severity: None,
                cleared: None,
                about: Kinds::ALL,
            }
        );
        let full = email_set(&[
            "set",
            "--to=ops@example.com,me@example.org",
            "--to",
            "third@example.net",
            "--server",
            "smtp.example.com",
            "--port",
            "465",
            "--user",
            "alerts@example.com",
            "--password",
            "-",
            "--from",
            "alerts@server.example.com",
            "--severity",
            "critical",
            "--cleared",
            "no",
            "--about",
            "data,hardware",
        ])
        .unwrap();
        assert_eq!(full.to, ["ops@example.com", "me@example.org", "third@example.net"]);
        assert_eq!((full.port, full.password), (Some(465), Password::Input));
        assert_eq!(
            (full.severity, full.cleared, full.about),
            (Some(Severity::Critical), Some(false), Kinds::of(&[About::Data, About::Hardware]))
        );
        let kept = email_set(&[
            "set",
            "--to",
            "a@b.test",
            "--server",
            "s.test",
            "--user",
            "u",
            "--keep-password",
        ])
        .unwrap();
        assert_eq!(kept.password, Password::Keep);
    }

    #[test]
    fn a_mistake_in_email_set_is_a_usage_error_that_says_what_to_give() {
        let to = ["--to", "a@b.test"];
        let server = ["--server", "s.test"];
        let with = |extra: &[&str]| email_set(&[&["set"], &to[..], &server[..], extra].concat());
        assert_eq!(
            says(email_set(&["set", "--server", "s.test"])),
            "--to ADDRESS is needed: the address the alerts go to"
        );
        assert_eq!(says(email_set(&["set", "--to", "a@b.test"])), "--server HOST is needed");
        assert!(says(with(&["--port", "25"])).starts_with("--port is 587"));
        assert!(says(with(&["--severity", "high"])).contains("not 'high'"));
        assert!(says(with(&["--cleared", "true"])).contains("yes or no"));
        assert!(says(with(&["--about", "data,sign-in"])).contains("'sign-in' is no kind of alert"));
        assert!(says(with(&["--password", "secret"])).starts_with("--password takes -"));
        assert!(says(with(&["--user", "u"])).starts_with("--user needs its password"));
        assert!(says(with(&["--password", "-"])).starts_with("a password goes with --user"));
        assert!(says(with(&["--keep-password"])).starts_with("a password goes with --user"));
        let both = with(&["--user", "u", "--password", "-", "--keep-password"]);
        assert!(says(both).contains("two ways to say one thing"));
        assert_eq!(says(with(&["--nope"])), "unrecognized option '--nope'");
        assert_eq!(says(with(&["extra"])), "unexpected argument 'extra'");
        assert!(
            says(parse_email(&mut args(&["email", "change"]))).starts_with("alerts email takes")
        );
        assert!(
            says(parse_email(&mut args(&["email", "remove", "now"])))
                .contains("unexpected argument")
        );
    }

    #[test]
    fn webhook_add_and_remove_take_a_kind_or_a_number() {
        let add = |words: &[&str]| parse_webhook(&mut args(&[&["webhook", "add"], words].concat()));
        let Ok(Webhook::Add(ntfy)) = add(&["ntfy"]) else { panic!() };
        assert_eq!(
            ntfy,
            WebhookAdd {
                kind: "ntfy".into(),
                room: None,
                severity: None,
                cleared: None,
                about: Kinds::ALL
            }
        );
        let Ok(Webhook::Add(matrix)) = add(&[
            "--room",
            "!abc:example.org",
            "matrix",
            "--severity",
            "critical",
            "--cleared",
            "no",
            "--about",
            "stopped",
        ]) else {
            panic!()
        };
        assert_eq!(matrix.room.as_deref(), Some("!abc:example.org"));
        assert_eq!(matrix.about, Kinds::of(&[About::Stopped]));
        assert!(says(add(&[])).starts_with("a KIND is needed: ntfy, matrix"));
        assert!(says(add(&["pager"])).starts_with("'pager' is not a kind of webhook"));
        assert!(says(add(&["ntfy", "json"])).contains("unexpected argument 'json'"));
        let remove =
            |words: &[&str]| parse_webhook(&mut args(&[&["webhook", "remove"], words].concat()));
        assert_eq!(remove(&["3"]).unwrap(), Webhook::Remove(3));
        assert!(says(remove(&["0"])).contains("is not the number of a webhook"));
        assert!(says(remove(&["33"])).contains("is not the number of a webhook"));
        assert!(says(remove(&["two"])).contains("is not the number of a webhook"));
        assert_eq!(says(remove(&[])), "a NUMBER is needed");
    }

    #[test]
    fn channel_takes_email_a_webhooks_number_or_a_browsers_id_and_the_kinds() {
        let channel = |words: &[&str]| parse_channel(&mut args(&[&["channel"], words].concat()));
        let told = |which, about| Told { channel: which, about };
        assert_eq!(channel(&["email", "--about", "all"]).unwrap(), told(Which::Email, Kinds::ALL));
        assert_eq!(
            channel(&["webhook:2", "--about", "data,stopped"]).unwrap(),
            told(Which::Webhook(2), Kinds::of(&[About::Data, About::Stopped]))
        );
        assert_eq!(
            channel(&["--about=hardware", "push:0123456789ABCDEF"]).unwrap(),
            told(Which::Push("0123456789abcdef".into()), Kinds::of(&[About::Hardware]))
        );
        assert!(says(channel(&["email"])).starts_with("--about WORDS is needed"));
        assert!(says(channel(&["--about", "all"])).starts_with("a CHANNEL is needed"));
        for wrong in
            ["webhook", "webhook:0", "webhook:x", "push:12", "push:zzzzzzzzzzzzzzzz", "sms"]
        {
            let said = says(channel(&[wrong, "--about", "all"]));
            assert!(said.contains(&format!("'{wrong}' is not a channel")), "{said}");
        }
        assert!(says(channel(&["email", "--about", "all", "x"])).contains("unexpected argument"));
    }

    /// A folder of the test's own with an alerts.toml of `text` in it, or
    /// none, taken away at the end.
    struct Folder(PathBuf);

    impl Folder {
        fn new(name: &str, text: Option<&str>) -> Folder {
            let dir =
                std::env::temp_dir().join(format!("alerts-file-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            let folder = Folder(dir);
            if let Some(text) = text {
                std::fs::write(folder.file(), text).unwrap();
                std::fs::set_permissions(folder.file(), std::fs::Permissions::from_mode(0o644))
                    .unwrap();
            }
            folder
        }

        fn file(&self) -> PathBuf {
            self.0.join("alerts.toml")
        }

        fn text(&self) -> String {
            std::fs::read_to_string(self.file()).unwrap()
        }

        fn mode(&self) -> u32 {
            std::fs::metadata(self.file()).unwrap().mode() & 0o7777
        }
    }

    impl Drop for Folder {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn me() -> u32 {
        util::own_user()
    }

    const SHIPPED: &str =
        "# SPDX header\n#\n# Where alerts go.\n#\n#   [email]\n#   to = [\"ops@example.com\"]\n";

    fn set_email(folder: &Folder, set: &EmailSet, password: Option<&str>) -> Result<(), Fail> {
        change_at(
            &folder.file(),
            me(),
            |document| email_table(document, set, password),
            |settings, wrong| {
                email_channel(settings).map(|_| ()).ok_or_else(|| why_left_out(wrong, "[email]"))
            },
        )
    }

    fn good_set() -> EmailSet {
        email_set(&[
            "set",
            "--to",
            "ops@example.com",
            "--server",
            "smtp.example.com",
            "--user",
            "alerts@example.com",
            "--password",
            "-",
        ])
        .unwrap()
    }

    #[test]
    fn email_set_writes_the_table_keeps_the_comments_and_makes_the_file_private() {
        let folder = Folder::new("email", Some(SHIPPED));
        set_email(&folder, &good_set(), Some("app-password")).unwrap();
        assert_eq!(
            folder.text(),
            format!(
                "{SHIPPED}\n[email]\nto = [\"ops@example.com\"]\nserver = \"smtp.example.com\"\nuser = \"alerts@example.com\"\npassword = \"app-password\"\n"
            )
        );
        assert_eq!(folder.mode(), 0o600, "the shipped file was 0644");
        // The same again changes nothing; another password changes it, and
        // a comment the person added stays.
        let hand = folder.text().replace("server =", "# the provider\nserver =");
        std::fs::write(folder.file(), hand).unwrap();
        set_email(&folder, &good_set(), Some("other")).unwrap();
        let text = folder.text();
        assert!(text.contains("# the provider\nserver = \"smtp.example.com\""), "{text}");
        assert!(text.contains("password = \"other\""), "{text}");
        assert_eq!(folder.mode(), 0o600);
    }

    #[test]
    fn email_set_makes_a_file_that_is_not_there_and_keeps_a_ca_and_the_password_asked_for() {
        let folder = Folder::new("email-new", None);
        let plain = email_set(&["set", "--to", "a@b.test", "--server", "s.test"]).unwrap();
        set_email(&folder, &plain, None).unwrap();
        assert_eq!(folder.text(), "[email]\nto = [\"a@b.test\"]\nserver = \"s.test\"\n");
        assert_eq!(folder.mode(), 0o600);
        // A setting the command does not take stays; one not given goes.
        std::fs::write(
            folder.file(),
            "[email]\nto = [\"a@b.test\"]\nserver = \"s.test\"\nport = 465\nca = \"/system/settings/ssl/c.pem\"\nseverity = \"critical\"\nuser = \"u\"\npassword = \"kept\"\n",
        )
        .unwrap();
        let keep = email_set(&[
            "set",
            "--to",
            "a@b.test",
            "--server",
            "s.test",
            "--user",
            "u2",
            "--keep-password",
        ])
        .unwrap();
        set_email(&folder, &keep, None).unwrap();
        assert_eq!(
            folder.text(),
            "[email]\nto = [\"a@b.test\"]\nserver = \"s.test\"\nca = \"/system/settings/ssl/c.pem\"\nuser = \"u2\"\npassword = \"kept\"\n"
        );
        // No user takes the account and its password out together.
        set_email(&folder, &plain, None).unwrap();
        assert!(!folder.text().contains("password") && !folder.text().contains("user"));
    }

    #[test]
    fn email_set_that_alertd_would_leave_out_is_refused_with_alertds_words_and_changes_nothing() {
        let folder = Folder::new("email-bad", Some(SHIPPED));
        let bad = email_set(&["set", "--to", "nobody", "--server", "s.test"]).unwrap();
        let refused = set_email(&folder, &bad, None).unwrap_err();
        assert_eq!(
            refused.message,
            format!(
                "[email]: to holds 'nobody', which is not an email address; {FILE} is as it was"
            )
        );
        assert!(!refused.usage);
        assert_eq!(folder.text(), SHIPPED);
        assert_eq!(folder.mode(), 0o644, "nothing was touched");
        let host =
            email_set(&["set", "--to", "a@b.test", "--server", "smtp.example.com:587"]).unwrap();
        assert!(
            set_email(&folder, &host, None).unwrap_err().message.contains("is not a host name")
        );
        // A kept password the file does not have.
        let keep = email_set(&[
            "set",
            "--to",
            "a@b.test",
            "--server",
            "s.test",
            "--user",
            "u",
            "--keep-password",
        ])
        .unwrap();
        let refused = set_email(&folder, &keep, None).unwrap_err();
        assert!(refused.message.starts_with("no password is kept"), "{}", refused.message);
        assert_eq!(folder.text(), SHIPPED);
    }

    #[test]
    fn a_file_that_is_not_toml_is_refused_and_not_touched() {
        let folder = Folder::new("not-toml", Some("[email\nto ="));
        let refused = set_email(&folder, &good_set(), Some("x")).unwrap_err();
        assert!(refused.message.contains("is not valid TOML, at line 1"), "{}", refused.message);
        assert!(refused.message.ends_with("it is as it was"));
        assert_eq!(folder.text(), "[email\nto =");
    }

    fn add_webhook(
        folder: &Folder,
        words: &[&str],
        url: &str,
        token: Option<&str>,
    ) -> Result<(u32, String), Fail> {
        let Ok(Webhook::Add(add)) =
            parse_webhook(&mut args(&[&["webhook", "add"], words].concat()))
        else {
            panic!("{words:?}")
        };
        let number = Cell::new(0);
        let name = change_at(
            &folder.file(),
            me(),
            |document| {
                number.set(webhook_table(document, &add, url, token)?);
                Ok(())
            },
            |settings, wrong| {
                let at = number.get();
                webhook_channel(settings, at)
                    .map(|channel| channel.name.clone())
                    .ok_or_else(|| why_left_out(wrong, &format!("webhook {at}")))
            },
        )?;
        Ok((number.get(), name))
    }

    fn remove_webhook(folder: &Folder, number: u32) -> Result<(), Fail> {
        let seen = Cell::new(0);
        change_at(
            &folder.file(),
            me(),
            |document| {
                let found = settings_edit::remove_table(document, "webhook", |_| {
                    seen.set(seen.get() + 1);
                    seen.get() == number
                });
                if found {
                    Ok(())
                } else {
                    Err(format!("there is no webhook {number}; `alerts status` lists them"))
                }
            },
            |_, _| Ok(()),
        )
    }

    #[test]
    fn webhooks_are_added_after_each_other_numbered_and_removed_by_their_number() {
        let folder = Folder::new("hooks", Some(SHIPPED));
        let (one, name) =
            add_webhook(&folder, &["ntfy"], "https://ntfy.sh/a-long-topic", Some("tk_1")).unwrap();
        assert_eq!((one, name.as_str()), (1, "ntfy (ntfy.sh)"));
        let (two, name) = add_webhook(
            &folder,
            &["json", "--severity", "critical", "--cleared", "no", "--about", "hardware"],
            "http://10.0.2.2:8080/alerts",
            None,
        )
        .unwrap();
        assert_eq!((two, name.as_str()), (2, "json (10.0.2.2)"));
        assert_eq!(
            folder.text(),
            format!(
                "{SHIPPED}\n[[webhook]]\nkind = \"ntfy\"\nurl = \"https://ntfy.sh/a-long-topic\"\ntoken = \"tk_1\"\n\n[[webhook]]\nkind = \"json\"\nurl = \"http://10.0.2.2:8080/alerts\"\nseverity = \"critical\"\ncleared = false\nabout = [\"hardware\"]\n"
            )
        );
        assert_eq!(folder.mode(), 0o600);
        // The first goes, and the second is the first.
        remove_webhook(&folder, 1).unwrap();
        assert!(folder.text().starts_with(SHIPPED) && !folder.text().contains("ntfy"));
        let refused = remove_webhook(&folder, 2).unwrap_err();
        assert_eq!(refused.message, "there is no webhook 2; `alerts status` lists them");
        remove_webhook(&folder, 1).unwrap();
        assert_eq!(folder.text(), SHIPPED, "the file is as it was before the first was added");
    }

    #[test]
    fn a_webhook_alertd_would_leave_out_is_refused_and_a_place_counts_the_ones_left_out() {
        let folder = Folder::new("hook-bad", Some(SHIPPED));
        let refused =
            add_webhook(&folder, &["slack"], "http://hooks.slack.com/x", None).unwrap_err();
        assert_eq!(
            refused.message,
            format!("webhook 1: url is http; slack is sent over https only; {FILE} is as it was")
        );
        let refused =
            add_webhook(&folder, &["matrix"], "https://matrix.example.org", Some("t")).unwrap_err();
        assert!(refused.message.starts_with("webhook 1: room is missing"), "{}", refused.message);
        let refused =
            add_webhook(&folder, &["slack"], "https://hooks.slack.com/x", Some("t")).unwrap_err();
        assert!(refused.message.contains("token is not used by slack"), "{}", refused.message);
        assert_eq!(folder.text(), SHIPPED);
        // A table with a mistake that is there already is one of the places.
        std::fs::write(folder.file(), "[[webhook]]\nkind = \"pager\"\nurl = \"https://x.test\"\n")
            .unwrap();
        let (number, _) = add_webhook(&folder, &["ntfy"], "https://ntfy.sh/topic-x", None).unwrap();
        assert_eq!(number, 2);
    }

    #[test]
    fn email_removed_takes_its_table_and_the_comments_above_it_and_none_is_refused() {
        let folder = Folder::new("email-gone", Some(SHIPPED));
        set_email(&folder, &good_set(), Some("p")).unwrap();
        let take = |folder: &Folder| {
            change_at(
                &folder.file(),
                me(),
                |document| {
                    if settings_edit::take_table(document, "email") {
                        Ok(())
                    } else {
                        Err("there is no email channel; `alerts status` lists them".to_string())
                    }
                },
                |_, _| Ok(()),
            )
        };
        take(&folder).unwrap();
        assert_eq!(folder.text(), SHIPPED);
        assert_eq!(
            take(&folder).unwrap_err().message,
            "there is no email channel; `alerts status` lists them"
        );
    }

    fn about_of(folder: &Folder, which: &Which, kinds: Kinds) -> Result<Option<String>, Fail> {
        change_at(
            &folder.file(),
            me(),
            |document| match which {
                Which::Email => {
                    let table = document
                        .get_mut("email")
                        .and_then(Item::as_table_like_mut)
                        .ok_or("there is no email channel; `alerts status` lists them")?;
                    put_about(table, kinds);
                    Ok(())
                }
                Which::Webhook(number) => {
                    let table = document
                        .get_mut("webhook")
                        .and_then(Item::as_array_of_tables_mut)
                        .and_then(|tables| tables.get_mut(usize::try_from(number - 1).unwrap()))
                        .ok_or_else(|| {
                            format!("there is no webhook {number}; `alerts status` lists them")
                        })?;
                    put_about(table, kinds);
                    Ok(())
                }
                Which::Push(_) => unreachable!(),
            },
            |settings, _| {
                Ok(match which {
                    Which::Email => email_channel(settings),
                    Which::Webhook(number) => webhook_channel(settings, *number),
                    Which::Push(_) => None,
                }
                .map(|channel| channel.name.clone()))
            },
        )
    }

    #[test]
    fn the_kinds_a_channel_is_told_of_are_written_and_all_takes_the_key_out() {
        let folder = Folder::new("about", Some(SHIPPED));
        set_email(&folder, &good_set(), Some("p")).unwrap();
        add_webhook(&folder, &["ntfy"], "https://ntfy.sh/a-long-topic", None).unwrap();
        let narrow = Kinds::of(&[About::Data, About::Hardware]);
        let name = about_of(&folder, &Which::Webhook(1), narrow).unwrap();
        assert_eq!(name.as_deref(), Some("ntfy (ntfy.sh)"));
        assert!(
            folder.text().contains(
                "url = \"https://ntfy.sh/a-long-topic\"\nabout = [\"data\", \"hardware\"]\n"
            ),
            "{}",
            folder.text()
        );
        let name = about_of(&folder, &Which::Email, Kinds::of(&[About::Stopped])).unwrap();
        assert_eq!(name.as_deref(), Some("email to ops@example.com"));
        assert!(folder.text().contains("about = [\"stopped\"]"));
        // All, and all three, are no key.
        about_of(&folder, &Which::Webhook(1), Kinds::ALL).unwrap();
        about_of(&folder, &Which::Email, Kinds::of(&About::ALL)).unwrap();
        assert!(!folder.text().contains("about"), "{}", folder.text());
        // A channel there is not.
        let none = about_of(&folder, &Which::Webhook(2), narrow).unwrap_err();
        assert_eq!(none.message, "there is no webhook 2; `alerts status` lists them");
        let folder = Folder::new("about-none", Some(SHIPPED));
        let none = about_of(&folder, &Which::Email, narrow).unwrap_err();
        assert_eq!(none.message, "there is no email channel; `alerts status` lists them");
        assert_eq!(folder.text(), SHIPPED);
    }

    #[test]
    fn a_file_others_can_read_is_made_private_even_when_nothing_changes() {
        let folder = Folder::new("tighten", Some(SHIPPED));
        let plain = email_set(&["set", "--to", "a@b.test", "--server", "s.test"]).unwrap();
        set_email(&folder, &plain, None).unwrap();
        std::fs::set_permissions(folder.file(), std::fs::Permissions::from_mode(0o664)).unwrap();
        set_email(&folder, &plain, None).unwrap();
        assert_eq!(folder.mode(), 0o600);
        // A file that is not there is not made by a change that writes nothing.
        let gone = Folder::new("tighten-none", None);
        assert!(about_of(&gone, &Which::Email, Kinds::ALL).is_err());
        assert!(!gone.file().exists());
    }

    #[test]
    fn lines_are_read_one_each_and_a_missing_line_is_empty() {
        let mut input: &[u8] = b"https://ntfy.sh/topic\r\ntoken\n";
        assert_eq!(lines_of(&mut input, 2).unwrap(), ["https://ntfy.sh/topic", "token"]);
        let mut input: &[u8] = b"https://ntfy.sh/topic\n";
        assert_eq!(lines_of(&mut input, 2).unwrap(), ["https://ntfy.sh/topic", ""]);
        let mut input: &[u8] = b"";
        assert_eq!(lines_of(&mut input, 1).unwrap(), [""]);
    }
}
