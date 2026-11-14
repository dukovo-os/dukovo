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

use manifest::bundle::{Access, Manifest, Need, Service, ServiceName, Start, To};
use manifest::service::{self as service_file, Ports, SocketAccess};

use super::minisign::PublicKey;
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

/// What becomes of a bundle's use of a socket another bundle offers (ADR
/// 0477).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Grant {
    /// It is given as the bundle is installed.
    Given,
    /// It is given if the person who installs the bundle approves it.
    Ask,
    /// It is never given to a bundle.
    Never,
}

/// What a provider that offers a socket `to` gives a bundle that uses it,
/// `same_publisher` being whether both are signed by one key.
pub fn socket_grant(to: To, same_publisher: bool) -> Grant {
    match to {
        To::Anyone => Grant::Given,
        To::Publisher if same_publisher => Grant::Given,
        To::Publisher | To::System => Grant::Never,
        To::Approved => Grant::Ask,
    }
}

/// The group of the socket a service of account `account` offers to its
/// publisher's bundles or the programs people approve: authd makes it as
/// bundled installs the provider.
pub fn socket_group(account: &str) -> String {
    format!("{account}{}", accounts::SOCKET_GROUP_SUFFIX)
}

/// Whether offering a socket `to` has it a group of its own.
fn has_group(to: To) -> bool {
    matches!(to, To::Publisher | To::Approved)
}

/// The groups `manifest` has made for the sockets it offers, each once.
pub fn socket_groups(manifest: &Manifest) -> Vec<String> {
    let mut groups: Vec<String> = Vec::new();
    for provide in &manifest.provides {
        if let Some(name) = &provide.socket
            && has_group(provide.to)
            && let Some(offer) = offer(manifest, &name.0)
        {
            let group = socket_group(&offer.account);
            if !groups.contains(&group) {
                groups.push(group);
            }
        }
    }
    groups
}

/// A socket a bundle offers: the account of the service that makes it,
/// whose live folder it is in, and who may connect.
#[derive(Debug, PartialEq, Eq)]
pub struct Offer {
    pub account: String,
    pub to: To,
}

/// The socket `name` that `manifest` offers, if it does: named in
/// `provides` and a `path` of one of its services' sockets.
pub fn offer(manifest: &Manifest, name: &str) -> Option<Offer> {
    let provide =
        manifest.provides.iter().find(|p| p.socket.as_ref().is_some_and(|s| s.0 == name))?;
    let service = manifest
        .services
        .iter()
        .find(|s| s.sockets.iter().any(|socket| socket.path.as_deref() == Some(name)))?;
    Some(Offer { account: account(service), to: provide.to })
}

/// A socket of another bundle that a service reaches: its place and,
/// for one with a group, the group.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Reach {
    /// The account of the service that makes it.
    pub account: String,
    /// Its name.
    pub name: String,
    /// The group whose members connect to it, if it has one.
    pub group: Option<String>,
}

impl Reach {
    /// Where it is.
    pub fn path(&self) -> String {
        format!("{}/{}", live_folder(&self.account), self.name)
    }
}

/// What came of one `uses` of a socket.
#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Given as the bundle is installed.
    Given(Reach),
    /// Given if the person approves it at the install.
    Ask(Reach),
    /// Not given, and why.
    Refused(String),
}

/// One `uses` of a socket of another bundle, and what came of it.
#[derive(Debug, PartialEq, Eq)]
pub struct Wanted {
    /// The provider's ID.
    pub provider: String,
    /// The socket's name.
    pub socket: String,
    /// Whether the bundle works without it.
    pub need: Need,
    pub outcome: Outcome,
}

impl Wanted {
    /// What it uses, as the manifest writes it: `PROVIDER/NAME`.
    pub fn what(&self) -> String {
        format!("{}/{}", self.provider, self.socket)
    }
}

/// What each socket that `manifest` uses comes to: `find` gives the
/// manifest and the signing key of the installed bundle with an ID, if
/// there is one, and `key` is the key `manifest` is signed by.
pub fn wanted(
    manifest: &Manifest,
    key: &PublicKey,
    find: impl Fn(&str) -> Option<(Manifest, PublicKey)>,
) -> Vec<Wanted> {
    let mut all = Vec::new();
    for used in &manifest.uses {
        let Some(socket) = &used.socket else { continue };
        let Some((provider, name)) = socket.0.split_once('/') else { continue };
        let outcome = match find(provider) {
            None => Outcome::Refused(format!(
                "{provider} is not installed for everyone: install it first, with elevate \
                 bundle install --for-everyone {provider}"
            )),
            Some((other, other_key)) => match offer(&other, name) {
                None => Outcome::Refused(format!("{provider} does not offer a socket {name}")),
                Some(offer) => {
                    let reach = Reach {
                        group: has_group(offer.to).then(|| socket_group(&offer.account)),
                        account: offer.account,
                        name: name.to_string(),
                    };
                    match socket_grant(offer.to, other_key == *key) {
                        Grant::Given => Outcome::Given(reach),
                        Grant::Ask => Outcome::Ask(reach),
                        Grant::Never if offer.to == To::System => Outcome::Refused(format!(
                            "{provider} offers {name} to the system's own programs alone, which \
                             no bundle is"
                        )),
                        Grant::Never => Outcome::Refused(format!(
                            "{provider} offers {name} only to bundles of its own publisher, and \
                             this one is signed by another key"
                        )),
                    }
                }
            },
        };
        all.push(Wanted {
            provider: provider.to_string(),
            socket: name.to_string(),
            need: used.need,
            outcome,
        });
    }
    all
}

/// Why `wanted` stops the install, if it does: a socket the bundle
/// requires that it is not given.
pub fn refusal(wanted: &[Wanted]) -> Option<String> {
    wanted.iter().find_map(|w| match (&w.outcome, w.need) {
        (Outcome::Refused(why), Need::Required) => Some(format!(
            "it requires the socket {} of {}, which it cannot have: {why}",
            w.socket, w.provider
        )),
        _ => None,
    })
}

/// What the install says of the sockets it does not give, for the ones
/// the bundle works without.
pub fn skipped(wanted: &[Wanted]) -> String {
    let mut said = String::new();
    for w in wanted {
        if let (Outcome::Refused(why), Need::Optional) = (&w.outcome, w.need) {
            let _ = write!(said, "; it has no socket {} of {}: {why}", w.socket, w.provider);
        }
    }
    said
}

/// The sheet's lines about the sockets of other bundles that the install
/// would give, for the person to approve.
pub fn socket_sheet(wanted: &[Wanted]) -> String {
    let mut o = String::new();
    for w in wanted {
        match &w.outcome {
            Outcome::Ask(_) => {
                let _ = writeln!(
                    o,
                    "  Uses the socket {} of {}, which it offers to the programs you approve: \
                     installing approves it.",
                    w.socket, w.provider
                );
            }
            Outcome::Given(_) => {
                let _ = writeln!(o, "  Uses the socket {} of {}.", w.socket, w.provider);
            }
            Outcome::Refused(_) => {}
        }
    }
    o
}

/// The places of the given sockets, which a service of the bundle reaches.
pub fn reaches(wanted: &[Wanted]) -> Vec<Reach> {
    wanted
        .iter()
        .filter_map(|w| match &w.outcome {
            Outcome::Given(reach) | Outcome::Ask(reach) => Some(reach.clone()),
            Outcome::Refused(_) => None,
        })
        .collect()
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
#[cfg(test)]
pub fn file(
    manifest: &Manifest,
    service: &Service,
    folder: &str,
    enabled: Option<bool>,
) -> Result<String, String> {
    file_with(manifest, service, folder, enabled, &[])
}

/// [`file`], for a service whose bundle reaches the sockets `reach` of
/// others (ADR 0477): their places are its to read and write, and the
/// groups of those that have one are its process's.
pub fn file_with(
    manifest: &Manifest,
    service: &Service,
    folder: &str,
    enabled: Option<bool>,
    reach: &[Reach],
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
    if service.start == Start::OnConnection {
        let _ = writeln!(o, "start = {}", quoted(service_file::Start::OnConnection.name()));
    }
    let _ = writeln!(o, "secrets = {}", list(service.secrets.iter().map(String::as_str)));
    let mut groups: Vec<&str> = Vec::new();
    for group in reach.iter().filter_map(|r| r.group.as_deref()) {
        if !groups.contains(&group) {
            groups.push(group);
        }
    }
    if !groups.is_empty() {
        let _ = writeln!(o, "groups = {}", list(groups));
    }
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
    if let Some(files) = limits.open_files {
        let _ = writeln!(o, "open_files = {files}");
    }
    let _ = writeln!(o, "\n[permissions]");
    let _ = writeln!(o, "network.outbound = {}", holds.outbound);
    let ports: Vec<String> = holds.listen.iter().map(|p| quoted(&p.to_string())).collect();
    let _ = writeln!(o, "network.listen = [{}]", ports.join(", "));
    let shared = |write: bool| {
        holds.shared.iter().filter(move |(_, w)| *w == write).map(|(f, _)| format!("/shared/{f}"))
    };
    let read: Vec<String> = std::iter::once(folder.to_string()).chain(shared(false)).collect();
    let reached: Vec<String> = reach.iter().map(Reach::path).collect();
    let read_write: Vec<String> =
        [data, live.clone()].into_iter().chain(shared(true)).chain(reached).collect();
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
    write_sockets(&mut o, &offered(manifest, service));
    if let Some(stop) = &service.stop {
        write_stop(&mut o, stop);
    }
    if let Some(backup) = &service.backup {
        write_backup(&mut o, backup, |program| program_path(manifest, program, name), folder)?;
    }
    if let Some(timer) = &service.timer {
        write_timer(&mut o, timer);
    }
    if let Some(logs) = &service.logs {
        write_logs(&mut o, logs);
    }
    service_file::from_toml(&o)
        .map_err(|err| format!("its service {name}'s file does not read: {err}"))?;
    Ok(o)
}

/// The sockets of `service`, those its bundle offers having who connects
/// to them (ADR 0477): `to = "anyone"` or `"system"`, or the group of
/// the socket for the publisher's bundles and the programs people
/// approve.
fn offered(manifest: &Manifest, service: &Service) -> Vec<service_file::SocketFile> {
    let mut sockets = service.sockets.clone();
    for socket in &mut sockets {
        let Some(offer) = socket.path.as_deref().and_then(|path| self::offer(manifest, path))
        else {
            continue;
        };
        match offer.to {
            To::Anyone => socket.to = Some(SocketAccess::Anyone),
            To::System => socket.to = Some(SocketAccess::System),
            To::Publisher | To::Approved => socket.group = Some(socket_group(&offer.account)),
        }
    }
    sockets
}

/// `sockets` as service.toml(5)'s `[[sockets]]` tables. A `path` stays a
/// plain name: Spark makes the socket in the service's live folder.
fn write_sockets(o: &mut String, sockets: &[service_file::SocketFile]) {
    for socket in sockets {
        let _ = writeln!(o, "\n[[sockets]]");
        if let Some(port) = socket.port {
            let _ = writeln!(o, "port = {port}");
        }
        if let Some(address) = socket.address.and_then(service_file::SocketAddress::text) {
            let _ = writeln!(o, "address = {}", quoted(&address));
        }
        if let Some(udp) = socket.udp {
            let _ = writeln!(o, "udp = {udp}");
        }
        if let Some(path) = &socket.path {
            let _ = writeln!(o, "path = {}", quoted(path));
        }
        if let Some(name) = &socket.name {
            let _ = writeln!(o, "name = {}", quoted(name));
        }
        if let Some(backlog) = socket.backlog {
            let _ = writeln!(o, "backlog = {backlog}");
        }
        match &socket.to {
            Some(SocketAccess::Anyone) => {
                let _ = writeln!(o, "to = \"anyone\"");
            }
            Some(SocketAccess::System) => {
                let _ = writeln!(o, "to = \"system\"");
            }
            _ => {}
        }
        if let Some(group) = &socket.group {
            let _ = writeln!(o, "group = {}", quoted(group));
        }
    }
}

/// `stop` as service.toml(5)'s `[stop]`.
fn write_stop(o: &mut String, stop: &manifest::bundle::Stop) {
    let _ = writeln!(o, "\n[stop]");
    if let Some(signal) = stop.signal {
        let _ = writeln!(o, "signal = {}", quoted(signal.name()));
    }
    if let Some(timeout) = stop.timeout {
        let _ = writeln!(o, "timeout = {timeout}");
    }
}

/// `backup` as service.toml(5)'s `[backup]`, each hook's program at its
/// path in the installed bundle `folder`; `program_path` finds a
/// program's path in the bundle.
fn write_backup<'a>(
    o: &mut String,
    backup: &manifest::bundle::BackupHooks,
    program_path: impl Fn(&str) -> Result<&'a str, String>,
    folder: &str,
) -> Result<(), String> {
    let _ = writeln!(o, "\n[backup]");
    for (key, hook) in [("before", &backup.before), ("after", &backup.after)] {
        let Some(hook) = hook else { continue };
        let path = program_path(&hook.program.0)?;
        let words: Vec<String> =
            std::iter::once(format!("{folder}/{path}")).chain(hook.arguments.clone()).collect();
        let _ = writeln!(o, "{key} = {}", list(words.iter().map(String::as_str)));
    }
    if let Some(limit) = backup.time_limit {
        let _ = writeln!(o, "time_limit = {limit}");
    }
    Ok(())
}

/// `timer` as service.toml(5)'s `[timer]`, with what it says, as it was
/// read.
fn write_timer(o: &mut String, timer: &service_file::TimerFile) {
    let _ = writeln!(o, "\n[timer]");
    if !timer.at.is_empty() {
        let _ = writeln!(o, "at = {}", list(timer.at.iter().map(String::as_str)));
    }
    let durations = [
        ("every", &timer.every),
        ("after_boot", &timer.after_boot),
        ("spread", &timer.spread),
        ("time_zone", &timer.time_zone),
    ];
    for (key, value) in durations {
        if let Some(value) = value {
            let _ = writeln!(o, "{key} = {}", quoted(value));
        }
    }
    if let Some(catch_up) = timer.catch_up {
        let _ = writeln!(o, "catch_up = {catch_up}");
    }
}

/// `logs` as service.toml(5)'s `[logs]`.
fn write_logs(o: &mut String, logs: &service_file::LogsFile) {
    let _ = writeln!(o, "\n[logs]");
    let _ = writeln!(o, "files = {}", list(logs.files.iter().map(String::as_str)));
    match logs.turn_over {
        service_file::TurnOver::Size(bytes) => {
            let _ = writeln!(o, "turn_over = {bytes}");
        }
        other => {
            let _ = writeln!(o, "turn_over = {}", quoted(&other.word()));
        }
    }
    let _ = writeln!(o, "keep = {}", logs.keep);
    let _ = writeln!(o, "compress = {}", logs.compress);
    let _ = writeln!(o, "reopen = {}", quoted(logs.reopen.word()));
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
                "limits": {"memory": "64M", "cpu_weight": 50, "open_files": 65536}}"#,
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
        assert_eq!(read.limits.open_files, Some(65536));
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
    fn a_timer_and_logs_are_written_through() {
        let m = manifest(
            r#"{"name": "web", "program": "web",
                "timer": {"at": ["daily", "30 2 * * 1-5"], "every": "6h", "after_boot": "10m",
                          "spread": "1h", "catch_up": false, "time_zone": "UTC"},
                "logs": {"files": ["logs/*.log"], "turn_over": "100M", "keep": 7,
                         "compress": false, "reopen": "SIGUSR1"}}"#,
            "",
        );
        let text = file(&m, &m.services[0], "/applications/org.example.web", None).unwrap();
        let read = service_file::from_toml(&text).unwrap();
        let timer = read.timer.unwrap();
        assert_eq!(timer.at.len(), 2);
        assert_eq!((timer.every, timer.after_boot, timer.spread), (Some(21_600), Some(600), 3600));
        assert!(!timer.catch_up);
        assert_eq!(timer.time_zone.as_deref(), Some("UTC"));
        let logs = read.logs.unwrap();
        assert_eq!(logs.files, ["logs/*.log"]);
        assert_eq!(logs.turn_over, service_file::TurnOver::Size(100 << 20));
        assert_eq!((logs.keep, logs.compress), (7, false));
        assert_eq!(logs.reopen, service_file::Reopen::Signal(service_file::Signal::Usr1));
    }

    #[test]
    fn sockets_and_starting_on_a_connection_are_written_through() {
        let m = manifest(
            r#"{"name": "web", "program": "web", "start": "on-connection",
                "sockets": [{"port": 8080, "address": "0.0.0.0"},
                            {"port": 8080, "address": "::", "name": "web6", "backlog": 64},
                            {"port": 9000, "udp": true}, {"path": "echo"}]}"#,
            r#""network.listen": {"need": "required", "ports": [8080, 9000]}"#,
        );
        let text = file(&m, &m.services[0], "/applications/org.example.web", None).unwrap();
        assert!(text.contains("\nstart = \"on-connection\"\n"), "{text}");
        assert_eq!(enabled(&text), Some(true));
        let read = service_file::from_toml(&text).unwrap();
        assert_eq!(read.start, service_file::Start::OnConnection);
        let names: Vec<&str> = read.sockets.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, ["8080", "web6", "9000", "echo"]);
        assert_eq!(read.sockets[1].backlog, 64);
        assert_eq!(
            read.sockets[1].kind,
            service_file::SocketKind::Stream {
                port: 8080,
                address: service_file::SocketAddress::AnyV6
            }
        );
        assert_eq!(
            read.sockets[2].kind,
            service_file::SocketKind::Datagram {
                port: 9000,
                address: service_file::SocketAddress::Any
            }
        );
        assert_eq!(read.sockets[3].kind, service_file::SocketKind::Unix { path: "echo".into() });
        // The offered sockets' groups and a user's place are written
        // apart, by the grants.
        assert!(read.groups.is_empty());
    }

    #[test]
    fn a_service_without_sockets_starts_at_boot_with_none() {
        let m = manifest(r#"{"name": "web", "program": "web", "start": "at-boot"}"#, "");
        let text = file(&m, &m.services[0], "/applications/org.example.web", None).unwrap();
        assert!(!text.contains("[[sockets]]") && !text.contains("on-connection"), "{text}");
        let read = service_file::from_toml(&text).unwrap();
        assert!(read.sockets.is_empty() && read.start == service_file::Start::AtBoot);
        assert_eq!(read.stop, service_file::Stop::default());
    }

    #[test]
    fn a_stop_and_backup_hooks_are_written_through() {
        let m = manifest(
            r#"{"name": "web", "program": "web",
                "stop": {"signal": "SIGQUIT", "timeout": 30},
                "backup": {"before": {"program": "web", "arguments": ["checkpoint", "-q"]},
                           "time_limit": 120}}"#,
            "",
        );
        let folder = "/applications/org.example.web";
        let text = file(&m, &m.services[0], folder, None).unwrap();
        assert!(text.contains("service_version = 2\n"), "{text}");
        let read = service_file::from_toml(&text).unwrap();
        assert_eq!(
            read.stop,
            service_file::Stop { signal: service_file::Signal::Quit, timeout: 30 }
        );
        assert_eq!(
            read.backup.before,
            [format!("{folder}/bin/web"), "checkpoint".into(), "-q".into()]
        );
        assert!(read.backup.after.is_empty());
        assert_eq!(read.backup.time_limit, 120);
        // A timeout alone leaves the signal at SIGTERM; backup alone
        // keeps the file at version 1.
        let m = manifest(
            r#"{"name": "web", "program": "web", "stop": {"timeout": 9},
                "backup": {"after": {"program": "web"}}}"#,
            "",
        );
        let text = file(&m, &m.services[0], folder, None).unwrap();
        let read = service_file::from_toml(&text).unwrap();
        assert_eq!(read.stop, service_file::Stop { timeout: 9, ..Default::default() });
        assert_eq!(read.backup.after, [format!("{folder}/bin/web")]);
        let m = manifest(r#"{"name": "web", "program": "web", "backup": {"time_limit": 5}}"#, "");
        let text = file(&m, &m.services[0], folder, None).unwrap();
        assert!(text.contains("service_version = 1\n"), "{text}");
    }

    #[test]
    fn a_service_without_a_timer_or_logs_writes_none() {
        let m = manifest(r#"{"name": "web", "program": "web"}"#, "");
        let text = file(&m, &m.services[0], "/applications/org.example.web", None).unwrap();
        assert!(!text.contains("[timer]") && !text.contains("[logs]"));
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

    /// A provider of org.example.web whose service `web` makes the socket
    /// `echo`, offered `to`.
    fn provider(to: &str) -> Manifest {
        let text = manifest::bundle::template("org.example.web", "Web", "Example, Inc.", "MIT")
            .replace(
                "\"permissions\": {}",
                &format!(
                    "\"services\": [{{\"name\": \"web\", \"program\": \"web\", \"sockets\": \
                     [{{\"path\": \"echo\"}}]}}],\n  \"provides\": [{{\"socket\": \"echo\", \
                     \"to\": \"{to}\"}}],\n  \"permissions\": {{}}"
                ),
            );
        manifest::bundle::from_json(&text).unwrap()
    }

    /// A consumer whose service `app` uses `org.example.web/echo`.
    fn consumer(need: &str) -> Manifest {
        let text = manifest::bundle::template("org.example.app", "App", "Example, Inc.", "MIT")
            .replace(
                "\"permissions\": {}",
                &format!(
                    "\"services\": [{{\"name\": \"app\", \"program\": \"app\"}}],\n  \
                     \"uses\": [{{\"socket\": \"org.example.web/echo\", \"need\": \"{need}\"}}],\n  \
                     \"permissions\": {{}}"
                ),
            );
        manifest::bundle::from_json(&text).unwrap()
    }

    fn key(n: u8) -> PublicKey {
        super::super::minisign::SecretKey::generate(&[n; 40]).unwrap().public()
    }

    #[test]
    fn who_is_given_a_socket_follows_who_it_is_offered_to() {
        assert_eq!(socket_grant(To::Anyone, false), Grant::Given);
        assert_eq!(socket_grant(To::Publisher, true), Grant::Given);
        assert_eq!(socket_grant(To::Publisher, false), Grant::Never);
        assert_eq!(socket_grant(To::Approved, true), Grant::Ask);
        assert_eq!(socket_grant(To::Approved, false), Grant::Ask);
        assert_eq!(socket_grant(To::System, true), Grant::Never);
        assert_eq!(socket_grant(To::System, false), Grant::Never);
    }

    #[test]
    fn a_provider_writes_who_connects_to_its_socket() {
        let read = |to: &str| {
            let m = provider(to);
            let text = file(&m, &m.services[0], "/applications/org.example.web", None).unwrap();
            service_file::from_toml(&text).unwrap().sockets[0].access.clone()
        };
        assert_eq!(read("anyone"), SocketAccess::Anyone);
        assert_eq!(read("system"), SocketAccess::System);
        assert_eq!(read("approved"), SocketAccess::Group("web-socket".into()));
        assert_eq!(read("publisher"), SocketAccess::Group("web-socket".into()));
        assert_eq!(socket_groups(&provider("approved")), ["web-socket"]);
        assert_eq!(socket_groups(&provider("publisher")), ["web-socket"]);
        assert!(socket_groups(&provider("anyone")).is_empty());
        assert!(socket_groups(&provider("system")).is_empty());
        // A socket that is not offered stays the service's own.
        let m = manifest(r#"{"name": "web", "program": "web", "sockets": [{"path": "db"}]}"#, "");
        let text = file(&m, &m.services[0], "/applications/org.example.web", None).unwrap();
        assert_eq!(service_file::from_toml(&text).unwrap().sockets[0].access, SocketAccess::Own);
        assert!(!text.contains("group ="));
    }

    #[test]
    fn a_consumer_reaches_the_socket_it_is_given() {
        let (web, app) = (provider("approved"), consumer("required"));
        let find = |id: &str| (id == "org.example.web").then(|| (provider("approved"), key(1)));
        let wanted = wanted(&app, &key(2), find);
        assert_eq!(wanted.len(), 1);
        let Outcome::Ask(reach) = &wanted[0].outcome else { panic!("{:?}", wanted[0].outcome) };
        assert_eq!(reach.path(), "/system/live/services/web/echo");
        assert_eq!(reach.group.as_deref(), Some("web-socket"));
        assert_eq!(refusal(&wanted), None);
        assert!(socket_sheet(&wanted).contains("echo of org.example.web"));
        let text = file_with(
            &app,
            &app.services[0],
            "/applications/org.example.app",
            None,
            &reaches(&wanted),
        )
        .unwrap();
        let read = service_file::from_toml(&text).unwrap();
        assert_eq!(read.groups, ["web-socket"]);
        let places: Vec<String> =
            read.permissions.places().into_iter().filter(|p| p.write).map(|p| p.path).collect();
        assert!(places.contains(&"/system/live/services/web/echo".to_string()), "{places:?}");
        // Without it, the file has neither.
        let plain = file(&app, &app.services[0], "/applications/org.example.app", None).unwrap();
        assert!(service_file::from_toml(&plain).unwrap().groups.is_empty());
        assert!(!plain.contains("web/echo"));
        drop(web);
    }

    #[test]
    fn a_socket_offered_to_anyone_is_a_place_without_a_group() {
        let app = consumer("required");
        let find = |_: &str| Some((provider("anyone"), key(1)));
        let wanted = wanted(&app, &key(2), find);
        let Outcome::Given(reach) = &wanted[0].outcome else { panic!() };
        assert_eq!(reach.group, None);
        let text = file_with(&app, &app.services[0], "/x/app", None, &reaches(&wanted)).unwrap();
        assert!(service_file::from_toml(&text).unwrap().groups.is_empty());
        assert!(text.contains("/system/live/services/web/echo"));
    }

    #[test]
    fn a_socket_that_cannot_be_had_stops_a_bundle_that_requires_it() {
        let required = consumer("required");
        let optional = consumer("optional");
        let absent = |_: &str| None;
        let w = wanted(&required, &key(2), absent);
        assert!(refusal(&w).unwrap().contains("org.example.web is not installed"));
        assert!(refusal(&w).unwrap().contains("elevate bundle install"));
        let w = wanted(&optional, &key(2), absent);
        assert_eq!(refusal(&w), None);
        assert!(skipped(&w).contains("echo of org.example.web"));
        assert!(reaches(&w).is_empty());
        // Offered to its publisher alone, and the keys differ; or the same.
        let find = |_: &str| Some((provider("publisher"), key(1)));
        assert!(refusal(&wanted(&required, &key(2), find)).unwrap().contains("another key"));
        assert_eq!(refusal(&wanted(&required, &key(1), find)), None);
        // To the system alone, never a bundle.
        let find = |_: &str| Some((provider("system"), key(1)));
        assert!(refusal(&wanted(&required, &key(1), find)).unwrap().contains("system's"));
        // A provider with no such socket.
        let find = |_: &str| Some((manifest(r#"{"name": "web", "program": "web"}"#, ""), key(1)));
        assert!(refusal(&wanted(&required, &key(1), find)).unwrap().contains("does not offer"));
    }
}
