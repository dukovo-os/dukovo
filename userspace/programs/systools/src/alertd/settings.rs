// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The channels of alertd(8): what `/system/settings/alerts.toml` says
//! (alerts.toml(5)), checked, and which changes each channel is told (ADR
//! 0485).
//!
//! A channel with a mistake in it is left out while the others go on. Each
//! mistake is a line that names the file, the channel and the setting;
//! alertd logs them when it starts and at each reload (`log alertd` shows
//! them).
//!
//! The file holds secrets: a password, tokens, and webhook addresses that
//! are tokens themselves. A file that names a channel is read only when it
//! is root's and nobody else may read it; otherwise no channel is set up,
//! and the log says what to do. Nothing here keeps a secret in a form
//! that prints: [`Secret`] shows only that there is one.

use std::fmt;
use std::net::IpAddr;
use std::os::unix::fs::MetadataExt;
use std::path::{Component, Path, PathBuf};

use toml::{Table, Value};
use tuf::Url;

use crate::alerts::protocol::{Change, Severity, State};

/// The settings file.
pub const FILE: &str = "/system/settings/alerts.toml";

/// The folder of the system's certificates and keys, the only one a
/// channel's `ca` may be in.
const SSL: &str = "/system/settings/ssl";

/// The settings of the email channel.
const EMAIL_KEYS: [&str; 9] =
    ["to", "from", "server", "port", "user", "password", "ca", "severity", "cleared"];
/// The settings of a webhook.
const WEBHOOK_KEYS: [&str; 6] = ["kind", "url", "token", "room", "severity", "cleared"];

/// A password or a token: it prints as nothing but its presence.
#[derive(Clone, PartialEq, Eq)]
pub struct Secret(String);

impl Secret {
    /// A secret of `text`, for the tests of what uses one.
    #[cfg(test)]
    pub fn new(text: &str) -> Secret {
        Secret(text.to_string())
    }

    /// The secret itself, for the request that needs it.
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Secret(..)")
    }
}

/// Which changes a channel is told: those at least as severe as
/// `severity`, and clears only when `cleared`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Filter {
    pub severity: Severity,
    pub cleared: bool,
}

impl Filter {
    /// Whether `change` goes to a channel with this filter. A test always
    /// goes; an event goes when its severity passes.
    pub fn passes(self, change: &Change) -> bool {
        match change.state {
            State::Test => true,
            State::Cleared => self.cleared && change.alert.severity >= self.severity,
            State::Raised | State::Event => change.alert.severity >= self.severity,
        }
    }
}

/// One way alerts reach the owner beyond the screen.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Channel {
    /// What `alerts status` and the log call it: "email to
    /// ops@example.com", "ntfy (ntfy.sh)". Unique among the channels, as
    /// the outbox is kept by it.
    pub name: String,
    pub filter: Filter,
    pub target: Target,
}

/// Where a channel sends.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    Email(Email),
    Webhook(Webhook),
}

/// The submission server and the addresses of the email channel.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Email {
    pub to: Vec<String>,
    pub from: String,
    pub server: String,
    pub port: u16,
    pub user: Option<String>,
    pub password: Option<Secret>,
    /// The PEM file of authorities to trust besides the system's.
    pub ca: Option<PathBuf>,
}

/// What a webhook speaks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Ntfy,
    Matrix,
    Slack,
    Discord,
    Json,
}

impl Kind {
    fn parse(name: &str) -> Option<Kind> {
        match name {
            "ntfy" => Some(Kind::Ntfy),
            "matrix" => Some(Kind::Matrix),
            "slack" => Some(Kind::Slack),
            "discord" => Some(Kind::Discord),
            "json" => Some(Kind::Json),
            _ => None,
        }
    }

    /// Its name in the settings and in `alerts status`.
    pub fn name(self) -> &'static str {
        match self {
            Kind::Ntfy => "ntfy",
            Kind::Matrix => "matrix",
            Kind::Slack => "slack",
            Kind::Discord => "discord",
            Kind::Json => "json",
        }
    }
}

/// An address alerts are posted to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Webhook {
    pub kind: Kind,
    /// The address as written: for Matrix the homeserver's.
    pub url: String,
    pub token: Option<Secret>,
    /// Matrix only: the room's ID.
    pub room: Option<String>,
}

/// The channels the settings name.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Settings {
    pub channels: Vec<Channel>,
}

/// The settings in [`FILE`], and what is wrong in it, a line each; no
/// channel at all when the file names one and others may read it.
pub fn load(host: &str) -> (Settings, Vec<String>) {
    let text = match std::fs::read_to_string(FILE) {
        Ok(text) => text,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            return (Settings::default(), Vec::new());
        }
        Err(err) => return (Settings::default(), vec![unreadable(&err)]),
    };
    let (settings, wrong) = read_settings(&text, host);
    if !names_a_channel(&text) {
        return (settings, wrong);
    }
    match std::fs::metadata(FILE) {
        Ok(meta) => match unsafe_to_read(meta.uid(), meta.mode()) {
            Some(why) => (Settings::default(), vec![why]),
            None => (settings, wrong),
        },
        Err(err) => (Settings::default(), vec![unreadable(&err)]),
    }
}

fn unreadable(err: &std::io::Error) -> String {
    format!("{FILE}: {}", crate::util::strip_os_error(err))
}

/// Why a file of owner `uid` and `mode` may not hold secrets, if it
/// may not: it is another user's, or others can read it.
pub fn unsafe_to_read(uid: u32, mode: u32) -> Option<String> {
    if mode & 0o077 != 0 {
        return Some(format!("{FILE} can be read by others; chmod 600 {FILE}"));
    }
    (uid != 0).then(|| format!("{FILE} is not root's; chown root {FILE}"))
}

/// Whether `text` has an `[email]` or a `[[webhook]]`, even one with
/// mistakes in it.
fn names_a_channel(text: &str) -> bool {
    text.parse::<Table>()
        .is_ok_and(|table| table.contains_key("email") || table.contains_key("webhook"))
}

/// The channels the settings text `text` names, and what is wrong in it,
/// a line each. `host` is the machine's name, for the default sender.
pub fn read_settings(text: &str, host: &str) -> (Settings, Vec<String>) {
    let table = match text.parse::<Table>() {
        Ok(table) => table,
        Err(error) => {
            let line = error.span().map_or(1, |span| text[..span.start].matches('\n').count() + 1);
            return (Settings::default(), vec![format!("{FILE}:{line}: {}", error.message())]);
        }
    };
    let mut wrong: Vec<String> = table
        .keys()
        .filter(|key| !["email", "webhook"].contains(&key.as_str()))
        .map(|key| {
            format!("{FILE}: no setting is called {key}; there is [email] and each address is a [[webhook]]")
        })
        .collect();
    let mut channels = Vec::new();
    if let Some(email) = table.get("email") {
        match email_channel(email, host) {
            Ok(channel) => channels.push(channel),
            Err(why) => wrong.push(format!("{FILE}: [email]: {why}")),
        }
    }
    match table.get("webhook") {
        None => {}
        Some(Value::Array(entries)) => {
            for (at, entry) in entries.iter().enumerate() {
                match webhook_channel(entry) {
                    Ok(channel) => channels.push(channel),
                    Err(why) => wrong.push(format!("{FILE}: webhook {}: {why}", at + 1)),
                }
            }
        }
        Some(_) => wrong.push(format!("{FILE}: webhook is not a [[webhook]] table")),
    }
    name_uniquely(&mut channels);
    (Settings { channels }, wrong)
}

/// Gives a channel whose name an earlier one has a number after it.
fn name_uniquely(channels: &mut [Channel]) {
    for at in 1..channels.len() {
        let name = channels[at].name.clone();
        let mut number = 1;
        while channels[..at].iter().any(|earlier| earlier.name == channels[at].name) {
            number += 1;
            channels[at].name = format!("{name} {number}");
        }
    }
}

/// The text setting `key`, if there is one.
fn text<'a>(table: &'a Table, key: &str) -> Result<Option<&'a str>, String> {
    match table.get(key) {
        None => Ok(None),
        Some(Value::String(text)) => Ok(Some(text)),
        Some(_) => Err(format!("{key} is not text")),
    }
}

/// The filter of a channel's table.
fn filter(table: &Table) -> Result<Filter, String> {
    let severity = match text(table, "severity")? {
        None | Some("warning") => Severity::Warning,
        Some("critical") => Severity::Critical,
        Some(other) => return Err(format!("severity is '{other}', not warning or critical")),
    };
    let cleared = match table.get("cleared") {
        None => true,
        Some(Value::Boolean(cleared)) => *cleared,
        Some(_) => return Err("cleared is not true or false".into()),
    };
    Ok(Filter { severity, cleared })
}

fn unknown_key(table: &Table, known: &[&str]) -> Result<(), String> {
    match table.keys().find(|key| !known.contains(&key.as_str())) {
        Some(key) => Err(format!("no setting is called {key}")),
        None => Ok(()),
    }
}

fn email_channel(entry: &Value, host: &str) -> Result<Channel, String> {
    let table = entry.as_table().ok_or("email is not a table of settings")?;
    unknown_key(table, &EMAIL_KEYS)?;
    let to = recipients(table)?;
    let from = match text(table, "from")? {
        Some(from) => from.to_string(),
        None => format!("alerts@{host}"),
    };
    if !is_address(&from) {
        return Err(format!("from '{from}' is not an email address"));
    }
    let server = text(table, "server")?
        .ok_or("server is missing; it is the provider's submission server")?;
    if !is_host(server) {
        return Err(format!("server '{server}' is not a host name; the port goes in port"));
    }
    let port = match table.get("port") {
        None => 587,
        Some(Value::Integer(465)) => 465,
        Some(Value::Integer(587)) => 587,
        Some(_) => return Err("port is not 587 (STARTTLS) or 465 (TLS)".into()),
    };
    let (user, password) = (text(table, "user")?, text(table, "password")?);
    if user.is_some() != password.is_some() {
        return Err("user and password go together; one is missing".into());
    }
    let ca = text(table, "ca")?.map(in_ssl_folder).transpose()?;
    let Some((first, more)) = to.split_first() else {
        return Err("to is empty, so nothing would be sent".into());
    };
    let name = if more.is_empty() {
        format!("email to {first}")
    } else {
        format!("email to {first} and {} more", more.len())
    };
    let email = Email {
        to,
        from,
        server: server.to_string(),
        port,
        user: user.map(str::to_string),
        password: password.map(|password| Secret(password.to_string())),
        ca,
    };
    Ok(Channel { name, filter: filter(table)?, target: Target::Email(email) })
}

/// The addresses of `to`.
fn recipients(table: &Table) -> Result<Vec<String>, String> {
    let list = table
        .get("to")
        .ok_or("to is missing; it is the list of addresses the alerts go to")?
        .as_array()
        .ok_or("to is not a list of addresses")?;
    let to: Vec<String> = list
        .iter()
        .map(|address| address.as_str().map(str::to_string))
        .collect::<Option<_>>()
        .ok_or("to holds something that is not an address")?;
    match to.iter().find(|address| !is_address(address)) {
        Some(bad) => Err(format!("to holds '{bad}', which is not an email address")),
        None => Ok(to),
    }
}

/// Whether `address` is `local@domain` in the plain form mail is sent to:
/// no spaces, quotes, angle brackets or line breaks, which could end the
/// address in SMTP's commands and start another.
fn is_address(address: &str) -> bool {
    let Some((local, domain)) = address.split_once('@') else { return false };
    let local_char = |c: char| c.is_ascii_graphic() && !"<>()[]\\,;:\"@".contains(c);
    !local.is_empty() && local.len() <= 64 && local.chars().all(local_char) && is_host(domain)
}

/// Whether `host` is a host name or an IPv4 address: letters, digits,
/// dots and hyphens.
fn is_host(host: &str) -> bool {
    let edge = |c: char| c == '.' || c == '-';
    !host.is_empty()
        && host.len() <= 253
        && !host.starts_with(edge)
        && !host.ends_with(edge)
        && host.chars().all(|c| c.is_ascii_alphanumeric() || edge(c))
}

/// `file`, which must be in [`SSL`].
fn in_ssl_folder(file: &str) -> Result<PathBuf, String> {
    let path = Path::new(file);
    // A `..` could leave the folder.
    let inside = path.starts_with(SSL)
        && path != Path::new(SSL)
        && path.components().all(|part| !matches!(part, Component::ParentDir | Component::CurDir));
    if inside {
        Ok(path.to_path_buf())
    } else {
        Err(format!("ca = '{file}' is not in {SSL}/, the only place certificates are read from"))
    }
}

fn webhook_channel(entry: &Value) -> Result<Channel, String> {
    let table = entry.as_table().ok_or("it is not a table of settings")?;
    unknown_key(table, &WEBHOOK_KEYS)?;
    let name =
        text(table, "kind")?.ok_or("kind is missing: ntfy, matrix, slack, discord or json")?;
    let kind = Kind::parse(name)
        .ok_or_else(|| format!("kind is '{name}', not ntfy, matrix, slack, discord or json"))?;
    let url = text(table, "url")?.ok_or("url is missing")?;
    let parsed = Url::parse(url).map_err(|_| "url is not an address with a host".to_string())?;
    check_scheme(kind, &parsed)?;
    let token = text(table, "token")?;
    if token.is_some_and(|token| token.chars().any(|c| c.is_whitespace() || c.is_control())) {
        return Err("token has a space or a line break in it".into());
    }
    let room = text(table, "room")?;
    match (kind, room, token) {
        (Kind::Matrix, None, _) => {
            return Err("room is missing; it is the room's ID, such as !abc:example.org".into());
        }
        (Kind::Matrix, _, None) => {
            return Err("token is missing; Matrix needs an access token".into());
        }
        (Kind::Matrix, _, _) => {}
        (_, Some(_), _) => return Err("room is for matrix only".into()),
        (Kind::Slack | Kind::Discord, _, Some(_)) => {
            return Err(format!(
                "token is not used by {}; its address holds the secret",
                kind.name()
            ));
        }
        _ => {}
    }
    let channel_name = format!("{} ({})", kind.name(), parsed.host());
    let target = Webhook {
        kind,
        url: url.to_string(),
        token: token.map(|token| Secret(token.to_string())),
        room: room.map(str::to_string),
    };
    Ok(Channel { name: channel_name, filter: filter(table)?, target: Target::Webhook(target) })
}

/// Alerts are sent over TLS only; the json kind may use plain HTTP to a
/// machine on the owner's own network.
fn check_scheme(kind: Kind, url: &Url) -> Result<(), String> {
    match (url.scheme(), kind) {
        ("https", _) => Ok(()),
        ("http", Kind::Json) if is_private(url.host()) => Ok(()),
        ("http", Kind::Json) => Err("url is http to an address that is not on a private network; \
             use https, or the address of a machine on this network"
            .into()),
        ("http", _) => Err(format!("url is http; {} is sent over https only", kind.name())),
        (other, _) => Err(format!("url is {other}, not https")),
    }
}

/// Whether `host` is localhost or an address on a private network: a
/// loopback, link-local or unique local address, or a private IPv4 one.
fn is_private(host: &str) -> bool {
    let bare = host.trim_start_matches('[').trim_end_matches(']');
    if bare == "localhost" {
        return true;
    }
    match bare.parse::<IpAddr>() {
        Ok(IpAddr::V4(address)) => {
            address.is_loopback() || address.is_private() || address.is_link_local()
        }
        Ok(IpAddr::V6(address)) => {
            let first = address.segments()[0];
            address.is_loopback() || first & 0xfe00 == 0xfc00 || first & 0xffc0 == 0xfe80
        }
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const HOST: &str = "server";

    /// The one channel in `text`, or what is wrong with it.
    fn one(text: &str) -> Result<Channel, String> {
        let (mut settings, mut wrong) = read_settings(text, HOST);
        match (settings.channels.pop(), wrong.pop()) {
            (Some(channel), None) => Ok(channel),
            (None, Some(wrong)) => Err(wrong),
            other => panic!("not one channel or one mistake: {other:?}"),
        }
    }

    fn email(extra: &str) -> String {
        format!("[email]\nto = [\"ops@example.com\"]\nserver = \"smtp.example.com\"\n{extra}")
    }

    fn hook(extra: &str) -> String {
        format!("[[webhook]]\n{extra}")
    }

    fn target_email(channel: Channel) -> Email {
        match channel.target {
            Target::Email(email) => email,
            other => panic!("not email: {other:?}"),
        }
    }

    fn target_webhook(channel: Channel) -> Webhook {
        match channel.target {
            Target::Webhook(hook) => hook,
            other => panic!("not a webhook: {other:?}"),
        }
    }

    #[test]
    fn email_with_every_setting_is_read() {
        let channel = one(r#"
            [email]
            to = ["ops@example.com", "me@example.org"]
            from = "alerts@server.example.com"
            server = "smtp.example.com"
            port = 465
            user = "alerts@example.com"
            password = "app-password"
            ca = "/system/settings/ssl/mail-ca.pem"
            severity = "critical"
            cleared = false
            "#)
        .unwrap();
        assert_eq!(channel.name, "email to ops@example.com and 1 more");
        assert_eq!(channel.filter, Filter { severity: Severity::Critical, cleared: false });
        let email = target_email(channel);
        assert_eq!(email.to, ["ops@example.com", "me@example.org"]);
        assert_eq!(
            (email.from.as_str(), email.server.as_str(), email.port),
            ("alerts@server.example.com", "smtp.example.com", 465)
        );
        assert_eq!(email.user.as_deref(), Some("alerts@example.com"));
        assert_eq!(email.password.unwrap().expose(), "app-password");
        assert_eq!(email.ca.as_deref(), Some(Path::new("/system/settings/ssl/mail-ca.pem")));
    }

    #[test]
    fn email_with_only_what_it_needs_goes_through_starttls_from_the_machine() {
        let channel = one(&email("")).unwrap();
        assert_eq!(channel.name, "email to ops@example.com");
        assert_eq!(channel.filter, Filter { severity: Severity::Warning, cleared: true });
        let email = target_email(channel);
        assert_eq!((email.from.as_str(), email.port), ("alerts@server", 587));
        assert_eq!((email.user, email.password, email.ca), (None, None, None));
    }

    #[test]
    fn a_webhook_of_each_kind_is_read() {
        for (kind, extra) in [
            ("ntfy", "token = \"tk_abc\"\n"),
            ("slack", ""),
            ("discord", ""),
            ("json", "token = \"t\"\n"),
            ("matrix", "room = \"!abc:example.org\"\ntoken = \"syt\"\n"),
        ] {
            let url = if kind == "matrix" {
                "https://matrix.example.org"
            } else {
                "https://hooks.example.com/x/y"
            };
            let channel =
                one(&hook(&format!("kind = \"{kind}\"\nurl = \"{url}\"\n{extra}"))).unwrap();
            let host = Url::parse(url).unwrap().host().to_string();
            assert_eq!(channel.name, format!("{kind} ({host})"));
            let hook = target_webhook(channel);
            assert_eq!(hook.kind.name(), kind);
            assert_eq!(hook.url, url);
        }
    }

    #[test]
    fn json_may_use_http_to_a_private_address_only() {
        let json = |url: &str| one(&hook(&format!("kind = \"json\"\nurl = \"{url}\"\n")));
        for url in [
            "http://127.0.0.1:8080/",
            "http://192.168.1.5/hook",
            "http://10.0.0.1/",
            "http://[fd00::1]/",
            "http://localhost/x",
        ] {
            assert!(json(url).is_ok(), "{url}");
        }
        for url in ["http://8.8.8.8/", "http://hooks.example.com/", "http://[2001:db8::1]/"] {
            let refused = json(url).unwrap_err();
            assert!(
                refused.contains("http to an address that is not on a private network"),
                "{refused}"
            );
        }
        assert!(json("https://hooks.example.com/").is_ok());
    }

    #[test]
    fn each_mistake_names_the_channel_and_the_setting() {
        let ntfy =
            |extra: &str| hook(&format!("kind = \"ntfy\"\nurl = \"https://ntfy.sh/t\"\n{extra}"));
        for (text, said) in [
            (
                hook("kind = \"pager\"\nurl = \"https://x.test/\"\n"),
                "webhook 1: kind is 'pager', not ntfy, matrix, slack, discord or json",
            ),
            (
                hook("url = \"https://x.test/\"\n"),
                "webhook 1: kind is missing: ntfy, matrix, slack, discord or json",
            ),
            (hook("kind = \"ntfy\"\n"), "webhook 1: url is missing"),
            (
                hook("kind = \"ntfy\"\nurl = \"ntfy.sh/topic\"\n"),
                "webhook 1: url is not an address with a host",
            ),
            (
                hook("kind = \"ntfy\"\nurl = \"http://ntfy.sh/t\"\n"),
                "webhook 1: url is http; ntfy is sent over https only",
            ),
            (
                hook("kind = \"ntfy\"\nurl = \"ftp://ntfy.sh/t\"\n"),
                "webhook 1: url is ftp, not https",
            ),
            (
                hook("kind = \"matrix\"\nurl = \"https://m.test\"\ntoken = \"t\"\n"),
                "webhook 1: room is missing; it is the room's ID, such as !abc:example.org",
            ),
            (
                hook("kind = \"matrix\"\nurl = \"https://m.test\"\nroom = \"!a:b\"\n"),
                "webhook 1: token is missing; Matrix needs an access token",
            ),
            (ntfy("room = \"!a:b\"\n"), "webhook 1: room is for matrix only"),
            (
                hook("kind = \"slack\"\nurl = \"https://hooks.slack.test/T/B/x\"\ntoken = \"t\"\n"),
                "webhook 1: token is not used by slack; its address holds the secret",
            ),
            (ntfy("token = \"two words\"\n"), "webhook 1: token has a space or a line break in it"),
            (
                ntfy("severity = \"loud\"\n"),
                "webhook 1: severity is 'loud', not warning or critical",
            ),
            (ntfy("cleared = \"yes\"\n"), "webhook 1: cleared is not true or false"),
            (ntfy("colour = \"red\"\n"), "webhook 1: no setting is called colour"),
            (hook("kind = 5\nurl = \"https://x.test/\"\n"), "webhook 1: kind is not text"),
            (email("port = 25\n"), "[email]: port is not 587 (STARTTLS) or 465 (TLS)"),
            (email("port = \"587\"\n"), "[email]: port is not 587 (STARTTLS) or 465 (TLS)"),
            (email("password = \"x\"\n"), "[email]: user and password go together; one is missing"),
            (email("user = \"x\"\n"), "[email]: user and password go together; one is missing"),
            (
                email("ca = \"/etc/ssl/ca.pem\"\n"),
                "[email]: ca = '/etc/ssl/ca.pem' is not in /system/settings/ssl/, the only place certificates are read from",
            ),
            (
                email("ca = \"/system/settings/ssl/../alerts.toml\"\n"),
                "[email]: ca = '/system/settings/ssl/../alerts.toml' is not in /system/settings/ssl/, the only place certificates are read from",
            ),
            (email("from = \"nobody\"\n"), "[email]: from 'nobody' is not an email address"),
            (
                email("severity = \"info\"\n"),
                "[email]: severity is 'info', not warning or critical",
            ),
            (
                "[email]\nserver = \"s.test\"\n".into(),
                "[email]: to is missing; it is the list of addresses the alerts go to",
            ),
            (
                "[email]\nto = \"ops@example.com\"\nserver = \"s.test\"\n".into(),
                "[email]: to is not a list of addresses",
            ),
            (
                "[email]\nto = []\nserver = \"s.test\"\n".into(),
                "[email]: to is empty, so nothing would be sent",
            ),
            (
                "[email]\nto = [\"a b@example.com\"]\nserver = \"s.test\"\n".into(),
                "[email]: to holds 'a b@example.com', which is not an email address",
            ),
            (
                "[email]\nto = [5]\nserver = \"s.test\"\n".into(),
                "[email]: to holds something that is not an address",
            ),
            (
                "[email]\nto = [\"a@b.test\"]\n".into(),
                "[email]: server is missing; it is the provider's submission server",
            ),
            (
                "[email]\nto = [\"a@b.test\"]\nserver = \"s.test:587\"\n".into(),
                "[email]: server 's.test:587' is not a host name; the port goes in port",
            ),
            ("email = 5\n".into(), "[email]: email is not a table of settings"),
            ("webhook = 5\n".into(), "webhook is not a [[webhook]] table"),
            ("webhook = [5]\n".into(), "webhook 1: it is not a table of settings"),
            (
                "colour = 5\n".into(),
                "no setting is called colour; there is [email] and each address is a [[webhook]]",
            ),
        ] {
            let refused = one(&text).unwrap_err();
            assert_eq!(refused, format!("{FILE}: {said}"), "{text}");
        }
    }

    #[test]
    fn a_channel_with_a_mistake_is_left_out_and_the_others_stay() {
        let (settings, wrong) = read_settings(
            &[
                email("port = 25\n"),
                hook("kind = \"slack\"\nurl = \"https://hooks.slack.test/a\"\n"),
                hook("kind = \"pager\"\nurl = \"https://x.test/\"\n"),
                hook("kind = \"discord\"\nurl = \"https://discord.test/a\"\n"),
            ]
            .join("\n"),
            HOST,
        );
        let names: Vec<&str> = settings.channels.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["slack (hooks.slack.test)", "discord (discord.test)"]);
        assert_eq!(wrong.len(), 2);
        assert!(wrong[0].contains("[email]") && wrong[1].contains("webhook 2"), "{wrong:?}");
    }

    #[test]
    fn two_channels_with_one_name_are_told_apart() {
        let (settings, wrong) = read_settings(
            &[
                hook("kind = \"ntfy\"\nurl = \"https://ntfy.sh/one\"\n"),
                hook("kind = \"ntfy\"\nurl = \"https://ntfy.sh/two\"\n"),
                hook("kind = \"ntfy\"\nurl = \"https://ntfy.sh/three\"\n"),
            ]
            .join("\n"),
            HOST,
        );
        assert!(wrong.is_empty());
        let names: Vec<&str> = settings.channels.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["ntfy (ntfy.sh)", "ntfy (ntfy.sh) 2", "ntfy (ntfy.sh) 3"]);
    }

    #[test]
    fn a_file_that_is_not_toml_or_holds_only_comments_gives_none() {
        let (settings, wrong) = read_settings("[email]\nto = \n", HOST);
        assert!(settings.channels.is_empty());
        assert_eq!(wrong.len(), 1);
        assert!(wrong[0].starts_with(&format!("{FILE}:2: ")), "{wrong:?}");
        assert_eq!(read_settings("", HOST), (Settings::default(), Vec::new()));
        assert_eq!(read_settings("# Email goes here.\n", HOST), (Settings::default(), Vec::new()));
    }

    #[test]
    fn a_file_others_can_read_is_not_used() {
        assert_eq!(unsafe_to_read(0, 0o600), None);
        assert_eq!(unsafe_to_read(0, 0o400), None);
        for mode in [0o644, 0o640, 0o604, 0o660, 0o666] {
            assert_eq!(
                unsafe_to_read(0, mode).as_deref(),
                Some(
                    "/system/settings/alerts.toml can be read by others; chmod 600 /system/settings/alerts.toml"
                ),
                "{mode:o}"
            );
        }
        assert_eq!(
            unsafe_to_read(1000, 0o600).as_deref(),
            Some(
                "/system/settings/alerts.toml is not root's; chown root /system/settings/alerts.toml"
            )
        );
    }

    #[test]
    fn only_a_file_that_names_a_channel_needs_its_mode_checked() {
        assert!(!names_a_channel("# Email goes here.\n"));
        assert!(!names_a_channel("[email\n"));
        assert!(names_a_channel("[email]\nto = []\n"));
        assert!(names_a_channel("[[webhook]]\nkind = \"pager\"\n"));
    }

    #[test]
    fn a_secret_prints_as_nothing() {
        let channel = one(&email("user = \"u\"\npassword = \"hunter2\"\n")).unwrap();
        let printed = format!("{channel:?}");
        assert!(!printed.contains("hunter2"), "{printed}");
        assert!(printed.contains("Secret(..)"), "{printed}");
    }

    #[test]
    fn the_filter_picks_by_severity_and_clearing_and_a_test_always_goes() {
        use crate::alerts::protocol::Alert;
        let change = |state, severity| Change {
            at: 0,
            source: "s".into(),
            state,
            alert: Alert { key: "k".into(), severity, title: "t".into(), detail: String::new() },
        };
        let all = Filter { severity: Severity::Warning, cleared: true };
        let critical = Filter { severity: Severity::Critical, cleared: true };
        let quiet = Filter { severity: Severity::Warning, cleared: false };
        assert!(all.passes(&change(State::Raised, Severity::Warning)));
        assert!(!critical.passes(&change(State::Raised, Severity::Warning)));
        assert!(critical.passes(&change(State::Raised, Severity::Critical)));
        assert!(critical.passes(&change(State::Event, Severity::Critical)));
        assert!(!critical.passes(&change(State::Event, Severity::Warning)));
        assert!(all.passes(&change(State::Cleared, Severity::Warning)));
        assert!(!quiet.passes(&change(State::Cleared, Severity::Critical)));
        assert!(!critical.passes(&change(State::Cleared, Severity::Warning)));
        assert!(quiet.passes(&change(State::Event, Severity::Warning)));
        for filter in [all, critical, quiet] {
            assert!(filter.passes(&change(State::Test, Severity::Warning)));
        }
    }

    #[test]
    fn addresses_with_what_could_end_an_smtp_command_are_refused() {
        for good in ["ops@example.com", "a.b+c@sub.example.org", "root@localhost", "x_y@10.0.0.1"] {
            assert!(is_address(good), "{good}");
        }
        for bad in [
            "",
            "ops",
            "@example.com",
            "ops@",
            "a@b@c",
            "a b@c.test",
            "a<b>@c.test",
            "a@c.test>",
            "a@c.test\r\nRCPT TO:<x@y>",
            "a@-c.test",
            "a@c..",
            "a;b@c.test",
            "\"a\"@c.test",
        ] {
            assert!(!is_address(bad), "{bad:?}");
        }
    }

    /// The file that ships.
    const SHIPPED: &str = include_str!("../../../../defaults/alerts.toml");

    #[test]
    fn the_file_that_ships_names_no_channel_and_so_its_mode_does_not_matter() {
        assert!(!names_a_channel(SHIPPED));
        let (settings, wrong) = read_settings(SHIPPED, "server");
        assert_eq!((settings, wrong), (Settings::default(), Vec::new()));
    }

    #[test]
    fn the_examples_in_the_file_that_ships_are_channels_when_they_are_uncommented() {
        // The indented comment lines are the examples; `# room` stays a
        // comment, as the file shows it.
        let uncommented: String = SHIPPED
            .lines()
            .filter_map(|line| line.strip_prefix("#   "))
            .filter(|line| {
                !line.starts_with("chmod")
                    && !line.starts_with("service ")
                    && !line.starts_with("alerts ")
            })
            .map(|line| format!("{line}\n"))
            .collect();
        let (settings, wrong) = read_settings(&uncommented, "server");
        assert_eq!(wrong, Vec::<String>::new(), "{uncommented}");
        let names: Vec<&str> = settings.channels.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["email to ops@example.com", "ntfy (ntfy.sh)"]);
    }
}
