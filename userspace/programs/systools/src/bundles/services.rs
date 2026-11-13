// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! A bundle's services (ADR 0385): what bundled(8) gives Spark to run
//! for a bundle installed for everyone.
//!
//! Each service runs as an account of its own, the one its manifest
//! names or its service's name, which authd(8) makes as the bundle is
//! installed: a number from 100, a group of its own, no password and no
//! shell. Its data folder, and its home, is `/shared/services/ID/ACCOUNT`,
//! the account's alone. bundled writes the service's file,
//! `/system/settings/spark/services/NAME.toml`, as a service file of the
//! system's is written (service.toml(5)), and keeps a record of it in
//! `/system/state/bundled/services/NAME`: the bundle's ID, the account,
//! and a sum of the file it wrote last, which tells an administrator's
//! changes apart (ADR 0386). A file there that no record names is not
//! bundled's, and no bundle's service takes its name.
//!
//! A service makes its sockets in `/system/live/services/ACCOUNT`, which
//! `RUNTIME_DIRECTORY` names, as systemd's does: the health check of an
//! update connects to the one its manifest names (ADR 0386).
//!
//! The file holds what the service may do, from what the bundle asks for
//! and its program keeps, as far as a service may hold it: the network,
//! to connect out and to listen on its ports, and folders of `/shared`.
//! What is a person's to give (their folders, their home, their drives,
//! other apps' data, their terminal's serial ports, their access) never
//! reaches a service, which runs for no one.

use std::fmt::Write as _;
use std::path::Path;

use manifest::bundle::{Access, Manifest, Service, ServiceName, Start};
use manifest::service::{self as service_file, Ports};

use super::run;

/// Where Spark reads the service files.
pub const SPARK: &str = crate::servicefiles::DIR;

/// The folder of the services' data: a folder for each bundle, and in it
/// one for each of its accounts.
pub const DATA: &str = "/shared/services";

/// The folder of bundled's records of the service files it wrote, in its
/// state folder.
pub const RECORDS: &str = "services";

/// Where a bundle's service starts among the others: after the system's
/// own, which take 5 to 50, and those `service new` makes, 60.
pub const ORDER: u32 = 70;

/// The account `service` runs as: the one it names, or its name.
pub fn account(service: &Service) -> String {
    service.account.as_ref().map_or_else(|| service.name.0.clone(), |a| a.0.clone())
}

/// The accounts of `manifest`'s services, each once, in order.
pub fn accounts(manifest: &Manifest) -> Vec<String> {
    let mut all: Vec<String> = Vec::new();
    for service in &manifest.services {
        let name = account(service);
        if !all.contains(&name) {
            all.push(name);
        }
    }
    all
}

/// The data folder of bundle `id`'s account `account`: its home.
pub fn data_folder(id: &str, account: &str) -> String {
    format!("{DATA}/{id}/{account}")
}

/// The folder of the sockets the services of account `account` make,
/// which Spark makes as one starts, the account's (ADR 0386).
pub fn live_folder(account: &str) -> String {
    format!("/system/live/services/{account}")
}

/// What tells the service file `text` from another: a sum of it with its
/// `enabled` line set one way, as `service enable` changes only that.
pub fn fingerprint(text: &str) -> String {
    let text = crate::servicefiles::with_enabled(text, false);
    super::sums::hex(&super::sums::sha256(text.as_bytes()))
}

/// The path of service `name`'s file.
pub fn file_path(name: &str) -> String {
    format!("{SPARK}/{name}.toml")
}

/// A record of a service file bundled wrote: whose service it is, the
/// account it runs as, and the [`fingerprint`] of the file bundled wrote
/// last.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Record {
    pub id: String,
    pub account: String,
    pub written: String,
}

impl Record {
    /// The record as its file holds it: its fields on a line, with tabs
    /// between them.
    pub fn text(&self) -> String {
        format!("{}\t{}\t{}\n", self.id, self.account, self.written)
    }

    /// The record its file's `text` holds, if it reads.
    pub fn parse(text: &str) -> Option<Record> {
        let mut fields = text.trim_end_matches('\n').split('\t');
        let (id, account, written) = (fields.next()?, fields.next()?, fields.next()?);
        if id.is_empty() || account.is_empty() || fields.next().is_some() || text.contains('\r') {
            return None;
        }
        Some(Record { id: id.into(), account: account.into(), written: written.into() })
    }

    /// Whether `text`, the service's file as it is, is not the one
    /// bundled wrote last: an administrator changed it.
    pub fn changed(&self, text: &str) -> bool {
        fingerprint(text) != self.written
    }
}

/// What a service holds of what its bundle asks for.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Holds {
    /// To connect out: `network.outbound` or `network.local`.
    pub outbound: bool,
    /// The ports it listens on: `network.listen`.
    pub listen: Vec<Ports>,
    /// Folders of `/shared`, by name, each with whether it may change it:
    /// `files.shared`.
    pub shared: Vec<(String, bool)>,
}

/// What `service` of `manifest` holds: what the bundle asks for, as much
/// as its program keeps, of what a service may hold. An administrator's
/// yes to the install sheet gave it.
pub fn holds(manifest: &Manifest, service: &Service) -> Holds {
    let mut holds = Holds::default();
    for (name, request) in run::kept(manifest, &service.program.0) {
        match name {
            "network.outbound" | "network.local" => holds.outbound = true,
            "network.listen" => holds.listen.extend(request.ports.iter().copied()),
            "files.shared" => holds.shared.extend(
                request
                    .folders
                    .iter()
                    .map(|(folder, access)| (folder.clone(), *access == Access::ReadWrite)),
            ),
            _ => {}
        }
    }
    holds
}

/// The folders of `/shared` that `manifest`'s services read and none
/// may change, each once. bundled makes those that are not there as it
/// installs the bundle, the root's and everyone's to read, so that a
/// service starts before anything is put in them: Spark refuses to start
/// a service a place it only reads is missing from, and makes one it may
/// change itself, the account's.
pub fn read_folders(manifest: &Manifest) -> Vec<String> {
    let shared: Vec<(String, bool)> =
        manifest.services.iter().flat_map(|service| holds(manifest, service).shared).collect();
    let mut folders: Vec<String> = Vec::new();
    for (folder, write) in &shared {
        let path = format!("/shared/{folder}");
        if !write && !shared.iter().any(|(f, w)| f == folder && *w) && !folders.contains(&path) {
            folders.push(path);
        }
    }
    folders
}

/// `text` as a TOML string, quoted, with what it cannot hold as is
/// escaped.
fn quoted(text: &str) -> String {
    let mut out = String::from("\"");
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            c if c.is_control() => {
                let _ = write!(out, "\\u{:04X}", u32::from(c));
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// `items` as a TOML array of strings.
fn list<'a>(items: impl IntoIterator<Item = &'a str>) -> String {
    let items: Vec<String> = items.into_iter().map(quoted).collect();
    format!("[{}]", items.join(", "))
}

/// Where the system's own service files are: a bundle's service may name
/// one of them in `requires`, `wants` or `after` (ADR 0470).
const SYSTEM_SERVICES: &str = "/system/resources/default-settings/spark/services";

/// The path in the bundle of `manifest`'s program `program`, for this
/// computer, which its service `service` runs.
fn program_path<'a>(
    manifest: &'a Manifest,
    program: &str,
    service: &str,
) -> Result<&'a str, String> {
    manifest
        .programs
        .get(program)
        .and_then(|found| found.path.get(&run::ARCH))
        .map(|path| path.0.as_str())
        .ok_or_else(|| {
            format!("its service {service} runs {program}, which it has none of for this computer")
        })
}

/// Checks that `name`, which `manifest`'s service `service` names in
/// `key`, is a service of the bundle's or one the system brings.
fn linked(manifest: &Manifest, name: &ServiceName, key: &str, service: &str) -> Result<(), String> {
    let own = manifest.services.iter().any(|other| other.name == *name);
    if own || Path::new(&format!("{SYSTEM_SERVICES}/{name}.toml")).exists() {
        return Ok(());
    }
    Err(format!(
        "its service {service} names {name} in {key}, which is neither one of its services nor \
         one the system brings"
    ))
}

/// The service file bundled writes for `service` of `manifest`,
/// installed in `folder`; `enabled`, whether it starts at boot, when an
/// administrator turned it on or off already. Checked as Spark reads it.
pub fn file(
    manifest: &Manifest,
    service: &Service,
    folder: &str,
    enabled: Option<bool>,
) -> Result<String, String> {
    let id = &manifest.id.0;
    let name = &service.name.0;
    let program = program_path(manifest, &service.program.0, name)?;
    let account = account(service);
    let data = data_folder(id, &account);
    let live = live_folder(&account);
    let holds = holds(manifest, service);
    let enabled = enabled.unwrap_or(service.start != Start::WhenEnabled);
    let mut o = String::new();
    let _ = writeln!(
        o,
        "# The service {name} of the bundle {id} {}, which bundled(8) wrote as it\n# \
         installed it (ADR 0385); removing the bundle removes it. service.toml(5)\n# explains \
         each setting.\n",
        manifest.version
    );
    // Version 2 only when it uses its keys, so that a release before 1.0
    // reads the files of the bundles that do not (ADR 0454).
    let version = if service.uses_version_2() { 2 } else { 1 };
    let _ = writeln!(o, "service_version = {version}");
    let description =
        format!("{name}, a service of {}", crate::bundles::sheet::shown(&manifest.name.0, 60));
    let _ = writeln!(o, "description = {}", quoted(&description));
    let _ = writeln!(o, "program = {}", quoted(&format!("{folder}/{program}")));
    let _ = writeln!(o, "arguments = {}", list(service.arguments.iter().map(String::as_str)));
    let _ = writeln!(o, "account = {}", quoted(&account));
    let _ = writeln!(o, "order = {ORDER}");
    let _ = writeln!(o, "enabled = {enabled}");
    let _ = writeln!(o, "restart = {}", quoted(service.restart.name()));
    let _ = writeln!(o, "secrets = {}", list(service.secrets.iter().map(String::as_str)));
    let links =
        [("requires", &service.requires), ("wants", &service.wants), ("after", &service.after)];
    for (key, names) in links {
        for name in names {
            linked(manifest, name, key, &service.name.0)?;
        }
        if !names.is_empty() {
            let _ = writeln!(o, "{key} = {}", list(names.iter().map(|n| n.0.as_str())));
        }
    }
    if service.give_up_after > 0 {
        let _ = writeln!(o, "give_up_after = {}", service.give_up_after);
    }
    let _ = writeln!(o, "\n[environment]");
    // As a person's session has them: the account's, whatever the
    // manifest says.
    let _ = writeln!(o, "HOME = {}", quoted(&data));
    let _ = writeln!(o, "USER = {}", quoted(&account));
    let _ = writeln!(o, "LOGNAME = {}", quoted(&account));
    let _ = writeln!(o, "RUNTIME_DIRECTORY = {}", quoted(&live));
    for (key, value) in &service.environment.0 {
        if !["HOME", "USER", "LOGNAME", "RUNTIME_DIRECTORY"].contains(&key.as_str()) {
            let _ = writeln!(o, "{} = {}", quoted(key), quoted(value));
        }
    }
    let limits = &service.limits;
    let _ = writeln!(o, "\n[limits]");
    let _ = writeln!(o, "cpu_weight = {}", limits.cpu_weight);
    match limits.memory {
        Some(bytes) => {
            let _ = writeln!(o, "memory = {bytes}");
        }
        None => {
            let _ = writeln!(o, "memory = \"none\"");
        }
    }
    let _ = writeln!(o, "oom = {}", quoted(limits.oom.name()));
    let _ = writeln!(o, "\n[permissions]");
    let _ = writeln!(o, "network.outbound = {}", holds.outbound);
    let ports: Vec<String> = holds.listen.iter().map(|p| quoted(&p.to_string())).collect();
    let _ = writeln!(o, "network.listen = [{}]", ports.join(", "));
    let shared = |write: bool| {
        holds.shared.iter().filter(move |(_, w)| *w == write).map(|(f, _)| format!("/shared/{f}"))
    };
    let read: Vec<String> = std::iter::once(folder.to_string()).chain(shared(false)).collect();
    let read_write: Vec<String> = [data, live.clone()].into_iter().chain(shared(true)).collect();
    let _ = writeln!(o, "files.read = {}", list(read.iter().map(String::as_str)));
    let _ = writeln!(o, "files.read-write = {}", list(read_write.iter().map(String::as_str)));
    if let Some(ready) = &service.ready {
        let _ = writeln!(o, "\n[ready]");
        let _ = writeln!(o, "when = {}", quoted(ready.when.name()));
        if let Some(port) = ready.port {
            let _ = writeln!(o, "port = {port}");
        }
        if let Some(path) = &ready.path {
            let _ = writeln!(o, "path = {}", quoted(&format!("{live}/{path}")));
        }
        if let Some(timeout) = ready.timeout {
            let _ = writeln!(o, "timeout = {timeout}");
        }
    }
    if let Some(command) = service.health.as_ref().and_then(|health| health.command.as_ref()) {
        let program = program_path(manifest, &command.program.0, name)?;
        let words = std::iter::once(format!("{folder}/{program}")).chain(command.arguments.clone());
        let words: Vec<String> = words.collect();
        let _ = writeln!(o, "\n[health]");
        let _ = writeln!(o, "command = {}", list(words.iter().map(String::as_str)));
        for (key, value) in
            [("every", command.every), ("timeout", command.timeout), ("failures", command.failures)]
        {
            if let Some(value) = value {
                let _ = writeln!(o, "{key} = {value}");
            }
        }
    }
    service_file::from_toml(&o)
        .map_err(|err| format!("its service {name}'s file does not read: {err}"))?;
    Ok(o)
}

/// Whether the service file `text` says it starts at boot, if it reads.
pub fn enabled(text: &str) -> Option<bool> {
    service_file::from_toml(text).ok().map(|service| service.enabled)
}

/// What the sheet says of `manifest`'s services, a line each.
pub fn sheet_lines(manifest: &Manifest) -> Vec<String> {
    manifest
        .services
        .iter()
        .map(|service| {
            let when = match service.start {
                Start::WhenEnabled => "once an administrator enables it",
                Start::AtBoot | Start::OnConnection => "now and at each boot",
            };
            format!("{}, as the account {}, starting {when}", service.name, account(service))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest(services: &str, permissions: &str) -> Manifest {
        let text = manifest::bundle::template("org.example.web", "Web", "Example, Inc.", "MIT")
            .replace(
                "\"permissions\": {}",
                &format!("\"services\": [{services}],\n  \"permissions\": {{{permissions}}}"),
            );
        manifest::bundle::from_json(&text).unwrap()
    }

    #[test]
    fn a_service_file_holds_what_a_service_may() {
        let m = manifest(
            r#"{"name": "web", "program": "web", "arguments": ["--port", "8080 \"x\""],
                "environment": {"MODE": "fast"}, "start": "at-boot", "secrets": ["key"],
                "limits": {"memory": "64M", "cpu_weight": 50}}"#,
            r#""network.listen": {"need": "required", "ports": [8080, "9000-9001"]},
               "network.local": {"need": "optional"},
               "files.shared": {"need": "required", "folders": {"photos": "read", "music": "read-write"}},
               "files.home": {"need": "required"}"#,
        );
        let service = &m.services[0];
        let text = file(&m, service, "/applications/org.example.web", None).unwrap();
        let read = service_file::from_toml(&text).unwrap();
        assert_eq!(read.account.as_deref(), Some("web"));
        assert!(read.enabled);
        assert_eq!(read.order, ORDER);
        assert_eq!(read.arguments, ["--port", "8080 \"x\""]);
        assert_eq!(read.secrets, ["key"]);
        assert_eq!(read.limits.memory, Some(64 << 20));
        assert_eq!(read.limits.cpu_weight, 50);
        assert!(read.permissions.network.outbound);
        assert_eq!(read.network_grants(), 3);
        let places: Vec<(String, bool)> =
            read.permissions.places().into_iter().map(|p| (p.path, p.write)).collect();
        assert_eq!(
            places,
            [
                ("/applications/org.example.web".to_string(), false),
                ("/shared/photos".into(), false),
                ("/shared/services/org.example.web/web".into(), true),
                ("/system/live/services/web".into(), true),
                ("/shared/music".into(), true),
            ]
        );
        assert!(
            read.environment
                .iter()
                .any(|(k, v)| k == "HOME" && v == "/shared/services/org.example.web/web")
        );
        assert!(read.environment.iter().any(|(k, v)| k == "MODE" && v == "fast"));
        for key in ["USER", "LOGNAME"] {
            assert!(read.environment.iter().any(|(k, v)| k == key && v == "web"), "{key}");
        }
        assert!(
            read.environment
                .iter()
                .any(|(k, v)| k == "RUNTIME_DIRECTORY" && v == "/system/live/services/web")
        );
        assert!(text.starts_with("# The service web of the bundle org.example.web 0.1.0"));
        assert_eq!(enabled(&text), Some(true));
    }

    #[test]
    fn a_service_that_says_what_it_needs_gets_a_version_2_file() {
        let m = manifest(
            r#"{"name": "web", "program": "web", "requires": ["db"], "after": ["netd-like"],
                "give_up_after": 5, "ready": {"when": "listening", "port": 8080, "timeout": 30},
                "health": {"command": {"program": "web", "arguments": ["--check"], "every": 20}}},
               {"name": "db", "program": "web", "account": "web-db",
                "ready": {"when": "socket", "path": "db.sock"}},
               {"name": "netd-like", "program": "web", "account": "web-net"}"#,
            r#""network.listen": {"need": "required", "ports": [8080]}"#,
        );
        let folder = "/applications/org.example.web";
        let text = file(&m, &m.services[0], folder, None).unwrap();
        assert!(text.contains("service_version = 2\n"), "{text}");
        let read = service_file::from_toml(&text).unwrap();
        assert_eq!(
            (read.requires, read.after, read.give_up_after),
            (vec!["db".into()], vec!["netd-like".into()], 5)
        );
        assert_eq!(read.ready.when, service_file::ReadyWhen::Listening(Some(8080)));
        assert_eq!(read.ready.timeout, 30);
        let health = read.health.unwrap();
        assert_eq!(health.command, [format!("{folder}/bin/web"), "--check".into()]);
        assert_eq!((health.every, health.timeout, health.failures), (20, 10, 3));
        // A socket's name is in the service's own live folder.
        let db = service_file::from_toml(&file(&m, &m.services[1], folder, None).unwrap()).unwrap();
        let socket = "/system/live/services/web-db/db.sock".to_string();
        assert_eq!(db.ready.when, service_file::ReadyWhen::Socket(socket));
        // One that says none of it stays at version 1, which releases
        // before 1.0 read.
        let plain = file(&m, &m.services[2], folder, None).unwrap();
        assert!(plain.contains("service_version = 1\n"), "{plain}");
    }

    #[test]
    fn a_service_may_name_only_its_bundle_s_and_the_system_s() {
        let m = manifest(r#"{"name": "web", "program": "web", "wants": ["nosuch"]}"#, "");
        let err = file(&m, &m.services[0], "/applications/org.example.web", None).unwrap_err();
        assert_eq!(
            err,
            "its service web names nosuch in wants, which is neither one of its services nor one \
             the system brings"
        );
    }

    #[test]
    fn one_enabled_later_keeps_what_an_administrator_said() {
        let m = manifest(r#"{"name": "web", "program": "web", "account": "web-data"}"#, "");
        let service = &m.services[0];
        let text = file(&m, service, "/applications/org.example.web", None).unwrap();
        let read = service_file::from_toml(&text).unwrap();
        assert!(!read.enabled, "when-enabled is the default");
        assert_eq!(read.account.as_deref(), Some("web-data"));
        assert!(!read.permissions.network.outbound);
        let on = file(&m, service, "/applications/org.example.web", Some(true)).unwrap();
        assert_eq!(enabled(&on), Some(true));
        assert_eq!(accounts(&m), ["web-data"]);
        assert_eq!(
            sheet_lines(&m),
            ["web, as the account web-data, starting once an administrator enables it"]
        );
    }

    #[test]
    fn the_folders_only_read_are_made() {
        let m = manifest(
            r#"{"name": "web", "program": "web"}, {"name": "sync", "program": "web"}"#,
            r#""files.shared": {"need": "required",
                                "folders": {"www": "read", "photos": "read", "music": "read-write"}}"#,
        );
        assert_eq!(read_folders(&m), ["/shared/www", "/shared/photos"]);
        let none = manifest(r#"{"name": "web", "program": "web"}"#, "");
        assert!(read_folders(&none).is_empty());
    }

    #[test]
    fn records_read_back_and_tell_changes_apart() {
        let m = manifest(r#"{"name": "web", "program": "web"}"#, "");
        let text = file(&m, &m.services[0], "/applications/org.example.web", None).unwrap();
        let record = Record {
            id: "org.example.web".into(),
            account: "web".into(),
            written: fingerprint(&text),
        };
        assert_eq!(Record::parse(&record.text()), Some(record.clone()));
        assert_eq!(Record::parse("org.example.web\tweb\n"), None);
        assert_eq!(Record::parse("\tweb\tsum\n"), None);
        assert_eq!(Record::parse("a\tweb\tsum\tmore\n"), None);
        // Enabled or not, it is the file bundled wrote; another change is
        // an administrator's.
        assert!(!record.changed(&text));
        let on = file(&m, &m.services[0], "/applications/org.example.web", Some(true)).unwrap();
        assert!(!record.changed(&on));
        assert!(record.changed(&text.replace("cpu_weight = 100", "cpu_weight = 50")));
    }

    #[test]
    fn quoting_holds_any_text() {
        let text = "a \"b\" \\ c\nd\te\u{1}";
        let toml =
            format!("service_version = 1\nprogram = \"/x\"\ndescription = {}\n", quoted(text));
        assert_eq!(service_file::from_toml(&toml).unwrap().description, text);
    }
}
