// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The destinations of logforward(8): what `/system/settings/logforward.toml`
//! says (logforward.toml(5)), checked, and which records each destination
//! gets (ADR 0482).
//!
//! A destination with a mistake in it is left out while the others go on.
//! Each mistake is a line that names the file, the destination and the
//! setting; the service logs them when it starts and at each reload
//! (`log logforward` shows them), and `logforward test` says them again.

use std::path::{Component, Path};

use toml::{Table, Value};

use super::connection::{TlsSettings, Transport};
use crate::logd::Record;
use crate::util;

/// The settings file.
pub const FILE: &str = "/system/settings/logforward.toml";
/// The folder of the system's certificates and keys, the only one a
/// destination's may be in: a settings file cannot have logforward read
/// any other file of the system's as a key.
const SSL: &str = "/system/settings/ssl";
/// The settings of a destination.
const KEYS: [&str; 10] = [
    "name",
    "address",
    "transport",
    "level",
    "names",
    "audit",
    "ca",
    "server_name",
    "certificate",
    "key",
];
/// The settings that only TLS has.
const TLS_KEYS: [&str; 4] = ["ca", "server_name", "certificate", "key"];
/// The most characters of a name, which the destination's position and
/// status files are named after.
const NAME_MAX: usize = 64;

/// One server the log goes to, and what it is sent.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Destination {
    /// What `logforward status` and the log call it.
    pub name: String,
    /// The server's name or address, with a port or without, as written.
    pub address: String,
    /// The address with its port, `host:port` or `[v6]:port`, for messages.
    pub endpoint: String,
    /// How messages reach it.
    pub transport: Transport,
    /// The least urgent level sent, as a syslog severity: 3, 4, 6 or 7.
    pub level: u8,
    /// The services and programs sent; all of them when it is `None`.
    pub names: Option<Vec<String>>,
    /// Whether only security events are sent.
    pub audit_only: bool,
    /// What TLS needs; `None` over TCP and UDP.
    pub tls: Option<TlsSettings>,
}

impl Destination {
    /// Whether `record` is one this destination is sent.
    pub fn wants(&self, record: &Record) -> bool {
        record.level_number() <= self.level
            && (!self.audit_only || record.audit)
            && self.names.as_ref().is_none_or(|names| names.contains(&record.name))
    }
}

/// The destinations the settings file text `text` holds, and what is wrong
/// in it, a line each.
pub fn read_settings(text: &str) -> (Vec<Destination>, Vec<String>) {
    let table = match text.parse::<Table>() {
        Ok(table) => table,
        Err(error) => {
            let line = error.span().map_or(1, |span| text[..span.start].matches('\n').count() + 1);
            return (Vec::new(), vec![format!("{FILE}:{line}: {}", error.message())]);
        }
    };
    let mut wrong: Vec<String> = table
        .keys()
        .filter(|key| *key != "destination")
        .map(|key| format!("{FILE}: no setting is called {key}; each server is a [[destination]]"))
        .collect();
    let mut destinations = Vec::new();
    let entries = match table.get("destination") {
        None => &Vec::new(),
        Some(Value::Array(entries)) => entries,
        Some(_) => {
            wrong.push(format!("{FILE}: destination is not a [[destination]] table"));
            return (destinations, wrong);
        }
    };
    for (at, entry) in entries.iter().enumerate() {
        match destination(entry, at + 1, &destinations) {
            Ok(destination) => destinations.push(destination),
            Err(error) => wrong.push(error),
        }
    }
    (destinations, wrong)
}

/// The destination `entry` makes, the `number`th of the file, or what is
/// wrong with it. `earlier` are the destinations before it.
fn destination(
    entry: &Value,
    number: usize,
    earlier: &[Destination],
) -> Result<Destination, String> {
    let Some(table) = entry.as_table() else {
        return Err(format!("{FILE}: destination {number} is not a table of settings"));
    };
    let label = match table.get("name").and_then(Value::as_str) {
        Some(name) if is_name(name) => name.to_string(),
        _ => number.to_string(),
    };
    build(table, earlier).map_err(|why| format!("{FILE}: destination {label}: {why}"))
}

fn build(table: &Table, earlier: &[Destination]) -> Result<Destination, String> {
    if let Some(key) = table.keys().find(|key| !KEYS.contains(&key.as_str())) {
        return Err(format!("no setting is called {key}"));
    }
    let name = name(table, earlier)?;
    let transport = transport(table)?;
    let (address, endpoint) = address(table, transport)?;
    Ok(Destination {
        name,
        address,
        endpoint,
        transport,
        level: level(table)?,
        names: names(table)?,
        audit_only: audit_only(table)?,
        tls: tls(table, transport)?,
    })
}

/// Whether `name` can be a destination's: it is in file names.
fn is_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= NAME_MAX
        && name.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
}

/// The text setting `key`, if there is one.
fn text<'a>(table: &'a Table, key: &str) -> Result<Option<&'a str>, String> {
    match table.get(key) {
        None => Ok(None),
        Some(Value::String(text)) => Ok(Some(text)),
        Some(_) => Err(format!("{key} is not text")),
    }
}

fn name(table: &Table, earlier: &[Destination]) -> Result<String, String> {
    let name = text(table, "name")?.ok_or("name is missing")?;
    if !is_name(name) {
        return Err(format!(
            "name '{name}' is not letters, digits, - and _, at most {NAME_MAX} of them"
        ));
    }
    if earlier.iter().any(|destination| destination.name == name) {
        return Err(format!("name {name} is used by an earlier destination"));
    }
    Ok(name.to_string())
}

fn transport(table: &Table) -> Result<Transport, String> {
    match text(table, "transport")? {
        None => Ok(Transport::Tls),
        Some(name) => Transport::parse(name)
            .ok_or_else(|| format!("transport is '{name}', not tls, tcp or udp")),
    }
}

/// The address as written and with its port.
fn address(table: &Table, transport: Transport) -> Result<(String, String), String> {
    let address = text(table, "address")?.ok_or("address is missing; it is the server's name")?;
    let (host, port) = util::split_port(address, transport.default_port())
        .map_err(|why| format!("address '{address}': {why}"))?;
    if host.is_empty() {
        return Err(format!("address '{address}' has no host"));
    }
    let endpoint =
        if host.contains(':') { format!("[{host}]:{port}") } else { format!("{host}:{port}") };
    Ok((address.to_string(), endpoint))
}

fn level(table: &Table) -> Result<u8, String> {
    match text(table, "level")? {
        Some("error") => Ok(3),
        Some("warning") => Ok(4),
        None | Some("info") => Ok(6),
        Some("debug") => Ok(7),
        Some(other) => Err(format!("level is '{other}', not error, warning, info or debug")),
    }
}

fn names(table: &Table) -> Result<Option<Vec<String>>, String> {
    let Some(value) = table.get("names") else { return Ok(None) };
    let list = value.as_array().ok_or("names is not a list of services and programs")?;
    let names: Vec<String> = list
        .iter()
        .map(|name| name.as_str().filter(|name| !name.is_empty()).map(str::to_string))
        .collect::<Option<_>>()
        .ok_or("names holds something that is not the name of a service or program")?;
    if names.is_empty() {
        return Err(
            "names is empty, so nothing would be sent; leave it out to send every record".into()
        );
    }
    Ok(Some(names))
}

fn audit_only(table: &Table) -> Result<bool, String> {
    match table.get("audit") {
        None => Ok(false),
        Some(Value::Boolean(only)) => Ok(*only),
        Some(_) => Err("audit is not true or false".into()),
    }
}

/// The TLS settings, which only `transport = "tls"` has.
fn tls(table: &Table, transport: Transport) -> Result<Option<TlsSettings>, String> {
    if transport != Transport::Tls {
        return match TLS_KEYS.into_iter().find(|key| table.contains_key(*key)) {
            Some(key) => Err(format!("{key} is for transport = \"tls\" only")),
            None => Ok(None),
        };
    }
    let server_name = match text(table, "server_name")? {
        Some("") => return Err("server_name is empty".into()),
        name => name.map(str::to_string),
    };
    let client = match (in_ssl_folder(table, "certificate")?, in_ssl_folder(table, "key")?) {
        (Some(certificate), Some(key)) => Some((certificate, key)),
        (None, None) => None,
        _ => return Err("certificate and key go together; one is missing".into()),
    };
    Ok(Some(TlsSettings { ca: in_ssl_folder(table, "ca")?, server_name, client }))
}

/// The file setting `key`, which must be in [`SSL`].
fn in_ssl_folder(table: &Table, key: &str) -> Result<Option<std::path::PathBuf>, String> {
    let Some(file) = text(table, key)? else { return Ok(None) };
    let path = Path::new(file);
    // A `..` could leave the folder the service's permissions name.
    let inside = path.starts_with(SSL)
        && path != Path::new(SSL)
        && path.components().all(|part| !matches!(part, Component::ParentDir | Component::CurDir));
    if !inside {
        return Err(format!(
            "{key} = '{file}' is not in {SSL}/, the only place the logforward service may read \
             certificates and keys from"
        ));
    }
    Ok(Some(path.to_path_buf()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(level: &str, name: &str, audit: bool) -> Record {
        Record {
            level: level.into(),
            name: name.into(),
            audit,
            text: "a line".into(),
            ..Record::default()
        }
    }

    /// The one destination in `text`, or what is wrong with it.
    fn one(text: &str) -> Result<Destination, String> {
        let (mut destinations, mut wrong) = read_settings(text);
        match (destinations.pop(), wrong.pop()) {
            (Some(destination), None) => Ok(destination),
            (None, Some(wrong)) => Err(wrong),
            other => panic!("not one destination or one mistake: {other:?}"),
        }
    }

    #[test]
    fn a_destination_with_every_setting_is_read() {
        let found = one(r#"
            [[destination]]
            name = "siem"
            address = "logs.example.com:7000"
            transport = "tls"
            level = "warning"
            names = ["sshd", "kernel"]
            audit = true
            ca = "/system/settings/ssl/siem-ca.pem"
            server_name = "logs.corp.example"
            certificate = "/system/settings/ssl/siem-client.pem"
            key = "/system/settings/ssl/siem-client.key"
        "#)
        .unwrap();
        assert_eq!(found.name, "siem");
        assert_eq!(found.address, "logs.example.com:7000");
        assert_eq!(found.endpoint, "logs.example.com:7000");
        assert_eq!(found.transport, Transport::Tls);
        assert_eq!(found.level, 4);
        assert_eq!(found.names, Some(vec!["sshd".to_string(), "kernel".to_string()]));
        assert!(found.audit_only);
        let tls = found.tls.unwrap();
        assert_eq!(tls.ca.as_deref(), Some(Path::new("/system/settings/ssl/siem-ca.pem")));
        assert_eq!(tls.server_name.as_deref(), Some("logs.corp.example"));
        assert_eq!(
            tls.client,
            Some((
                "/system/settings/ssl/siem-client.pem".into(),
                "/system/settings/ssl/siem-client.key".into()
            ))
        );
    }

    #[test]
    fn a_destination_with_only_a_name_and_an_address_sends_everything_over_tls() {
        let found = one("[[destination]]\nname = \"a\"\naddress = \"logs.example.com\"\n").unwrap();
        assert_eq!(found.transport, Transport::Tls);
        assert_eq!(found.level, 6);
        assert_eq!(found.names, None);
        assert!(!found.audit_only);
        assert_eq!(found.tls, Some(TlsSettings::default()));
    }

    #[test]
    fn the_port_an_address_leaves_out_is_the_transports() {
        for (transport, endpoint) in
            [("tls", "logs.test:6514"), ("tcp", "logs.test:514"), ("udp", "logs.test:514")]
        {
            let found = one(&format!(
                "[[destination]]\nname = \"a\"\naddress = \"logs.test\"\ntransport = \"{transport}\"\n"
            ))
            .unwrap();
            assert_eq!(found.endpoint, endpoint);
            assert_eq!(found.address, "logs.test");
        }
        let six =
            one("[[destination]]\nname = \"a\"\naddress = \"fd00::5\"\ntransport = \"udp\"\n");
        assert_eq!(six.unwrap().endpoint, "[fd00::5]:514");
        let six = one("[[destination]]\nname = \"a\"\naddress = \"[fd00::5]:9000\"\n");
        assert_eq!(six.unwrap().endpoint, "[fd00::5]:9000");
    }

    #[test]
    fn over_tcp_and_udp_there_are_no_tls_settings() {
        let found = one("[[destination]]\nname = \"a\"\naddress = \"x\"\ntransport = \"tcp\"\n");
        assert_eq!(found.unwrap().tls, None);
        for key in ["ca", "server_name", "certificate", "key"] {
            let refused = one(&format!(
                "[[destination]]\nname = \"a\"\naddress = \"x\"\ntransport = \"udp\"\n{key} = \"/system/settings/ssl/f\"\n"
            ))
            .unwrap_err();
            assert_eq!(
                refused,
                format!("{FILE}: destination a: {key} is for transport = \"tls\" only")
            );
        }
    }

    #[test]
    fn certificates_and_keys_must_be_in_the_ssl_folder() {
        for path in [
            "/etc/ssl/ca.pem",
            "/system/settings/ssl",
            "/system/settings/ssl/../logforward.toml",
            "/system/settings/sslx/ca.pem",
            "ca.pem",
        ] {
            let refused =
                one(&format!("[[destination]]\nname = \"a\"\naddress = \"x\"\nca = \"{path}\"\n"))
                    .unwrap_err();
            assert!(
                refused.starts_with(&format!(
                    "{FILE}: destination a: ca = '{path}' is not in /system/settings/ssl/"
                )),
                "{refused}"
            );
        }
        let refused = one(
            "[[destination]]\nname = \"a\"\naddress = \"x\"\ncertificate = \"/system/settings/ssl/c.pem\"\nkey = \"/root/k\"\n",
        )
        .unwrap_err();
        assert!(refused.contains("key = '/root/k' is not in /system/settings/ssl/"), "{refused}");
    }

    #[test]
    fn a_certificate_and_its_key_go_together() {
        for only in ["certificate", "key"] {
            let refused = one(&format!(
                "[[destination]]\nname = \"a\"\naddress = \"x\"\n{only} = \"/system/settings/ssl/f\"\n"
            ))
            .unwrap_err();
            assert_eq!(
                refused,
                format!("{FILE}: destination a: certificate and key go together; one is missing")
            );
        }
    }

    #[test]
    fn each_mistake_names_the_destination_and_the_setting() {
        let head = "[[destination]]\nname = \"siem\"\naddress = \"logs.test\"\n";
        for (extra, said) in [
            ("transport = \"relp\"\n", "transport is 'relp', not tls, tcp or udp"),
            ("transport = 6\n", "transport is not text"),
            ("level = \"loud\"\n", "level is 'loud', not error, warning, info or debug"),
            ("level = 6\n", "level is not text"),
            ("names = \"sshd\"\n", "names is not a list of services and programs"),
            (
                "names = [\"sshd\", 5]\n",
                "names holds something that is not the name of a service or program",
            ),
            (
                "names = [\"\"]\n",
                "names holds something that is not the name of a service or program",
            ),
            (
                "names = []\n",
                "names is empty, so nothing would be sent; leave it out to send every record",
            ),
            ("audit = \"yes\"\n", "audit is not true or false"),
            ("server_name = \"\"\n", "server_name is empty"),
            ("colour = \"red\"\n", "no setting is called colour"),
        ] {
            let refused = one(&format!("{head}{extra}")).unwrap_err();
            assert_eq!(refused, format!("{FILE}: destination siem: {said}"), "{extra}");
        }
    }

    #[test]
    fn a_name_and_an_address_are_needed_and_checked() {
        let refused = one("[[destination]]\naddress = \"x\"\n").unwrap_err();
        assert_eq!(refused, format!("{FILE}: destination 1: name is missing"));
        let refused = one("[[destination]]\nname = \"two words\"\naddress = \"x\"\n").unwrap_err();
        assert_eq!(
            refused,
            format!(
                "{FILE}: destination 1: name 'two words' is not letters, digits, - and _, at most 64 of them"
            )
        );
        let long = "n".repeat(65);
        let refused =
            one(&format!("[[destination]]\nname = \"{long}\"\naddress = \"x\"\n")).unwrap_err();
        assert!(refused.contains("at most 64"), "{refused}");
        let refused = one("[[destination]]\nname = \"a\"\n").unwrap_err();
        assert_eq!(
            refused,
            format!("{FILE}: destination a: address is missing; it is the server's name")
        );
        let refused = one("[[destination]]\nname = \"a\"\naddress = \"x:http\"\n").unwrap_err();
        assert_eq!(
            refused,
            format!("{FILE}: destination a: address 'x:http': 'http' is not a port")
        );
        let refused = one("[[destination]]\nname = \"a\"\naddress = \":514\"\n").unwrap_err();
        assert_eq!(refused, format!("{FILE}: destination a: address ':514' has no host"));
    }

    #[test]
    fn a_destination_with_a_mistake_is_left_out_and_the_others_stay() {
        let (destinations, wrong) = read_settings(
            "[[destination]]\nname = \"one\"\naddress = \"a.test\"\n\
             [[destination]]\nname = \"two\"\naddress = \"b.test\"\nlevel = \"loud\"\n\
             [[destination]]\nname = \"one\"\naddress = \"c.test\"\n\
             [[destination]]\nname = \"four\"\naddress = \"d.test\"\ntransport = \"udp\"\n",
        );
        let names: Vec<&str> = destinations.iter().map(|d| d.name.as_str()).collect();
        assert_eq!(names, ["one", "four"]);
        assert_eq!(
            wrong,
            [
                format!(
                    "{FILE}: destination two: level is 'loud', not error, warning, info or debug"
                ),
                format!("{FILE}: destination one: name one is used by an earlier destination"),
            ]
        );
    }

    #[test]
    fn a_file_that_is_not_toml_or_names_no_destination_gives_none() {
        let (destinations, wrong) = read_settings("[[destination]]\nname = \n");
        assert!(destinations.is_empty());
        assert_eq!(wrong.len(), 1);
        assert!(wrong[0].starts_with(&format!("{FILE}:2: ")), "{wrong:?}");
        assert_eq!(read_settings(""), (Vec::new(), Vec::new()));
        assert_eq!(read_settings("# nothing yet\n"), (Vec::new(), Vec::new()));
        let (destinations, wrong) = read_settings("[[destinations]]\nname = \"a\"\n");
        assert!(destinations.is_empty());
        assert_eq!(
            wrong,
            [format!(
                "{FILE}: no setting is called destinations; each server is a [[destination]]"
            )]
        );
        let (destinations, wrong) = read_settings("[destination]\nname = \"a\"\n");
        assert!(destinations.is_empty());
        assert_eq!(wrong, [format!("{FILE}: destination is not a [[destination]] table")]);
    }

    #[test]
    fn a_destination_is_sent_the_records_its_level_names_and_audit_setting_pick() {
        let destination = |extra: &str| {
            one(&format!("[[destination]]\nname = \"a\"\naddress = \"x\"\n{extra}")).unwrap()
        };
        let everything = destination("");
        assert!(everything.wants(&record("error", "kernel", false)));
        assert!(everything.wants(&record("info", "sshd", false)));
        assert!(!everything.wants(&record("debug", "sshd", false)));
        let debugging = destination("level = \"debug\"\n");
        assert!(debugging.wants(&record("debug", "sshd", false)));
        let errors = destination("level = \"error\"\n");
        assert!(errors.wants(&record("error", "sshd", false)));
        assert!(!errors.wants(&record("warning", "sshd", false)));
        let warnings = destination("level = \"warning\"\n");
        assert!(warnings.wants(&record("warning", "sshd", false)));
        assert!(!warnings.wants(&record("info", "sshd", false)));
        let named = destination("names = [\"sshd\", \"kernel\"]\n");
        assert!(named.wants(&record("info", "sshd", false)));
        assert!(named.wants(&record("info", "kernel", false)));
        assert!(!named.wants(&record("info", "sshdx", false)));
        assert!(!named.wants(&record("info", "authd", false)));
        let audit = destination("audit = true\nlevel = \"debug\"\n");
        assert!(audit.wants(&record("info", "authd", true)));
        assert!(!audit.wants(&record("info", "authd", false)));
        let all_three = destination("audit = true\nlevel = \"error\"\nnames = [\"authd\"]\n");
        assert!(all_three.wants(&record("error", "authd", true)));
        assert!(!all_three.wants(&record("info", "authd", true)));
        assert!(!all_three.wants(&record("error", "sshd", true)));
        assert!(!all_three.wants(&record("error", "authd", false)));
    }
}
