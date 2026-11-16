// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The install sheet (ADR 0223): what bundled tells about a bundle before
//! it is installed, and `bundle info` after. The permissions are in the
//! system's own words, which a bundle cannot reword; what the bundle says
//! of itself (its name, its publisher, why it asks) is shown as its own,
//! with any control character taken out, so it draws nothing on the
//! terminal.
//!
//! `bundle info --json` says the same facts as one typed document
//! ([`details`]), in the manifest's own words, which serde escapes.

use std::fmt::Write as _;

use manifest::bundle::permissions;
use manifest::bundle::{Manifest, RunsWith};

use dkv::flaws::List;
use serde::Serialize;

use super::grants::Grant;
use super::minisign::PublicKey;
use super::registry::{Place, Trust};
use super::sizes::DataSize;
use crate::{json, util};

/// What [`Facts::replaces`] names when a version runs a service the one
/// it replaces did not.
pub const SERVICES: &str = "services";

/// What the sheet tells of a bundle besides its manifest.
pub struct Facts<'a> {
    pub manifest: &'a Manifest,
    pub key: &'a PublicKey,
    pub trust: &'a Trust,
    pub place: Place,
    /// Its files, and the bytes they take.
    pub files: usize,
    pub bytes: u64,
    /// The version it would replace, and the permissions it asks for
    /// that that version did not (ADR 0384).
    pub replaces: Option<(&'a str, &'a [String])>,
    /// The list of known flaws of the catalog it is from, if one is kept
    /// (ADR 0389).
    pub flaws: Option<&'a List>,
    /// Whether to name every flaw, as `bundle info` does, or the worst
    /// few, as the install sheet does.
    pub all_flaws: bool,
}

/// The most flaws the sheet names; `bundle info` names them all.
const FLAWS_SHOWN: usize = 5;

/// `text` as the bundle wrote it, on one line, without control
/// characters, at most `max` characters.
pub fn shown(text: &str, max: usize) -> String {
    let mut out: String =
        text.chars().map(|c| if c.is_control() { ' ' } else { c }).take(max + 1).collect();
    if out.chars().count() > max {
        out = out.chars().take(max.saturating_sub(3)).collect::<String>() + "...";
    }
    out
}

/// A size as people read it: bytes, kB, MB or GB, by thousands.
pub fn size(bytes: u64) -> String {
    const UNITS: [&str; 3] = ["kB", "MB", "GB"];
    if bytes < 1000 {
        return format!("{bytes} bytes");
    }
    let mut value = bytes as f64 / 1000.0;
    let mut unit = 0;
    while value >= 1000.0 && unit < UNITS.len() - 1 {
        value /= 1000.0;
        unit += 1;
    }
    format!("{value:.1} {}", UNITS[unit])
}

fn day(seconds: u64) -> String {
    util::time_text(i64::try_from(seconds).unwrap_or(i64::MAX), "%Y-%m-%d", true)
}

/// Whose a place is, for the sheet's first line.
fn for_whom(place: Place) -> &'static str {
    match place {
        Place::Everyone => "for everyone",
        Place::User(_) => "for yourself",
    }
}

/// What catalog `catalog`'s list of flaws says of version `version` of
/// bundle `id`: every flaw when `all`, else the worst few.
fn flaws(o: &mut String, catalog: &str, list: Option<&List>, id: &str, version: &str, all: bool) {
    let Some(list) = list.filter(|l| l.checked(id, version)) else {
        let _ = writeln!(o, "  Known flaws: catalog {catalog} does not say.");
        return;
    };
    let made = list.made.get(..10).unwrap_or(&list.made);
    let found: Vec<_> = list.of(id, version).collect();
    if found.is_empty() {
        let _ = writeln!(o, "  Known flaws: none, by catalog {catalog}'s list of {made}.");
        return;
    }
    let count = found.len();
    let flaws = if count == 1 { "flaw" } else { "flaws" };
    let _ = writeln!(
        o,
        "  Warning: catalog {catalog}'s list of {made} knows of {count} {flaws}\n  in the \
         components it holds:"
    );
    let shown_count = if all { count } else { count.min(FLAWS_SHOWN) };
    for flaw in &found[..shown_count] {
        let fixed = flaw.fixed.as_ref().map_or_else(
            || ", which no version fixes yet".to_string(),
            |f| format!(", fixed in {}", shown(f, 20)),
        );
        let _ = writeln!(
            o,
            "    {} ({}) in {}{fixed}",
            shown(&flaw.id, 30),
            flaw.severity.word(),
            shown(&flaw.component, 40)
        );
        if !flaw.summary.is_empty() {
            let _ = writeln!(o, "      {}", shown(&flaw.summary, 70));
        }
    }
    if shown_count < count {
        let _ = writeln!(o, "    and {} more, which bundle info names", count - shown_count);
    }
}

/// The sheet: lines, each ending in a newline.
pub fn write(facts: &Facts<'_>) -> String {
    let m = facts.manifest;
    let mut o = String::new();
    let over = facts.replaces.map_or_default(|(version, _)| format!(", in place of {version}"));
    let new: &[String] = facts.replaces.map_or(&[], |(_, new)| new);
    let _ = writeln!(o, "{} {}, {}{over}", shown(&m.name.0, 60), m.version, for_whom(facts.place));
    let _ = writeln!(o, "  {}, by {}", m.id, shown(&m.publisher.name.0, 60));
    let _ = writeln!(o, "  {}", shown(&m.summary.0, 100));
    let _ = writeln!(o, "  Signed by key {}", facts.key.encoded());
    match facts.trust {
        Trust::Catalog { name, since: None } => {
            let _ = writeln!(
                o,
                "  From catalog {name}, which this computer trusts: it lists this very file."
            );
        }
        Trust::Catalog { name, since: Some(since) } => {
            let _ = writeln!(
                o,
                "  From catalog {name}, which this computer trusts: it lists this very file.\n  \
                 The bundle has come from there since {}.",
                day(*since)
            );
        }
        Trust::Developer => {
            let _ = writeln!(o, "  Your developer key, which this computer trusts for you alone.");
        }
        Trust::Bound { since } => {
            let _ = writeln!(o, "  Its key since its first install here, on {}.", day(*since));
        }
        Trust::BoundForEveryone { since } => {
            let _ = writeln!(o, "  Its key for everyone, since {}.", day(*since));
        }
        Trust::Unknown => {
            let _ = writeln!(
                o,
                "  Warning: this computer does not know this key.\n  No catalog it trusts lists \
                 this file. That the bundle comes from\n  {} is its own word, which nothing has \
                 checked: install it\n  only if you trust where you got it. From now on only \
                 this key\n  may install {}.",
                shown(&m.publisher.name.0, 40),
                m.id
            );
        }
    }
    let _ = writeln!(o, "  {} files, {}", facts.files, size(facts.bytes));
    if let Some(support) = &m.support {
        let _ = writeln!(o, "  Its publisher takes problem reports at {}", shown(&support.0, 100));
    }
    if let Some(update) = &m.update
        && !matches!(facts.trust, Trust::Catalog { .. })
    {
        let _ = writeln!(o, "  It updates from its own feed at {}", shown(&update.feed.0, 100));
    }
    if let Trust::Catalog { name, .. } = facts.trust {
        flaws(&mut o, name, facts.flaws, &m.full_id(), &m.version.0, facts.all_flaws);
    }
    if m.permissions.0.is_empty() {
        let _ = writeln!(o, "  It asks for no permission.");
    } else {
        let _ = writeln!(o, "  It asks for:");
        for (name, request) in m.permissions.iter() {
            let Some(permission) = permissions::find(name) else { continue };
            let mut what = permission.lets.to_string();
            let objects: Vec<String> = request
                .folders
                .iter()
                .map(|(folder, access)| format!("{} ({access})", shown(folder, 40)))
                .chain(request.ports.iter().map(ToString::to_string))
                .chain(request.apps.iter().map(ToString::to_string))
                .collect();
            if !objects.is_empty() {
                let _ = write!(what, ": {}", objects.join(", "));
            }
            let need = if request.need.word() == "optional" { ", optional" } else { "" };
            let fresh = if new.iter().any(|n| n == name) { ", new in this version" } else { "" };
            let _ = writeln!(o, "    {what} ({}{need}){fresh}", permission.tier.words());
            if let Some(why) = &request.why {
                let _ = writeln!(o, "      In its words: \"{}\"", shown(&why.0, 100));
            }
        }
    }
    let commands = |runs_with: RunsWith| -> Vec<String> {
        m.commands.iter().filter(|c| c.runs_with == runs_with).map(|c| c.name.to_string()).collect()
    };
    let sandboxed = commands(RunsWith::Sandbox);
    if !sandboxed.is_empty() {
        let _ = writeln!(o, "  Commands: {}", sandboxed.join(", "));
    }
    // Plainly, as the security design asks: these are not confined.
    let yours = commands(RunsWith::YourAccess);
    if !yours.is_empty() {
        let _ = writeln!(
            o,
            "  Commands that run with your access, outside its sandbox: {}",
            yours.join(", ")
        );
        if new.iter().any(|n| n == super::grants::YOUR_ACCESS) {
            let _ = writeln!(o, "  New in this version: a command that runs with your access.");
        }
    }
    // Each runs for no one, as an account of its own (ADR 0385).
    let services = super::services::sheet_lines(m);
    if !services.is_empty() {
        let _ = writeln!(o, "  Services, which Spark runs, each as an account of its own:");
        for line in services {
            let _ = writeln!(o, "    {line}");
        }
        if new.iter().any(|n| n == SERVICES) {
            let _ = writeln!(o, "  New in this version: a service.");
        }
    }
    o
}

/// Where an installed bundle is and what is known of how it got there,
/// which `bundle info` adds to the sheet's facts.
pub struct Installation<'a> {
    /// When it was installed, in seconds since 1970.
    pub installed: u64,
    /// Its folder.
    pub folder: &'a str,
    /// Where its data is, in words.
    pub data: &'a str,
    /// What its data takes, as the last walk found, if one did.
    pub data_size: Option<DataSize>,
    /// The version it replaced, if it is kept.
    pub previous: Option<&'a str>,
    /// Whether it is held at its version.
    pub held: bool,
    /// The names of its services that Spark starts at each boot.
    pub started: &'a [String],
    /// The bundles of the same place that use what it provides.
    pub used_by: &'a [Dependent],
}

/// A bundle that uses an interface or a socket another provides: its full
/// ID and name, and what it uses, as its manifest names it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Dependent {
    id: String,
    name: String,
    what: String,
}

/// The bundles among `others`, other than `provider`, whose manifests
/// `uses` name an interface or a socket that `provider` `provides`: a row
/// for each use, by ID and then by what it uses. A socket is named by its
/// provider's ID and its own name, as the provider's ID is written with
/// or without its line.
pub fn dependents<'m>(
    provider: &Manifest,
    others: impl IntoIterator<Item = &'m Manifest>,
) -> Vec<Dependent> {
    let interfaces: Vec<&str> = provider
        .provides
        .iter()
        .filter_map(|p| p.interface.as_ref().map(|i| i.0.as_str()))
        .collect();
    let mut sockets = Vec::new();
    for socket in provider.provides.iter().filter_map(|p| p.socket.as_ref()) {
        sockets.push(format!("{}/{}", provider.id.0, socket.0));
        if provider.full_id() != provider.id.0 {
            sockets.push(format!("{}/{}", provider.full_id(), socket.0));
        }
    }
    let mut found = Vec::new();
    for other in others {
        if other.full_id() == provider.full_id() {
            continue;
        }
        for used in &other.uses {
            let named = match (&used.interface, &used.socket) {
                (Some(interface), _) => interfaces.contains(&interface.0.as_str()),
                (None, Some(socket)) => sockets.contains(&socket.0),
                (None, None) => false,
            };
            if named {
                found.push(Dependent {
                    id: other.full_id(),
                    name: other.name.0.clone(),
                    what: used.what(),
                });
            }
        }
    }
    found.sort_by(|a, b| (&a.id, &a.what).cmp(&(&b.id, &b.what)));
    found.dedup();
    found
}

/// What `bundle info --json` says of an installed bundle: what
/// [`write`] shows of it, and what the person said of its permissions.
#[derive(Serialize)]
pub struct Details {
    id: String,
    name: String,
    version: String,
    publisher: String,
    summary: String,
    for_everyone: bool,
    key: String,
    trust: TrustDetails,
    files: usize,
    bytes: u64,
    support: Option<String>,
    update_feed: Option<String>,
    flaws: FlawsDetails,
    permissions: Vec<PermissionDetails>,
    commands: Vec<String>,
    commands_with_your_access: Vec<String>,
    services: Vec<String>,
    grants: Vec<GrantDetails>,
    installed: String,
    folder: String,
    data: String,
    data_size: Option<DataSize>,
    previous: Option<String>,
    held: bool,
    license: String,
    service_list: Vec<ServiceDetails>,
    provides: Vec<Offered>,
    uses: Vec<Offered>,
    used_by: Vec<Dependent>,
}

/// A service of the bundle: its name, the account it runs as, and whether
/// Spark starts it at each boot (`on`) or not (`off`).
#[derive(Serialize)]
struct ServiceDetails {
    name: String,
    account: String,
    start: &'static str,
}

/// An interface or a socket, as the manifest names it: one of the two, the
/// other `null`.
#[derive(Serialize)]
struct Offered {
    interface: Option<String>,
    socket: Option<String>,
}

/// Why the bundle's key is trusted: `catalog`, `developer`, `bound`,
/// `bound_for_everyone` or `unknown`, with the catalog and since when
/// where there are any.
#[derive(Serialize)]
struct TrustDetails {
    kind: &'static str,
    catalog: Option<String>,
    since: Option<String>,
}

/// What the catalog's list of flaws says: `unknown` when it says nothing
/// of this version, `none`, or `listed` with each flaw in `known`.
#[derive(Serialize)]
struct FlawsDetails {
    state: &'static str,
    list_made: Option<String>,
    known: Vec<FlawDetails>,
}

#[derive(Serialize)]
struct FlawDetails {
    id: String,
    severity: &'static str,
    component: String,
    fixed: Option<String>,
    summary: String,
}

/// A permission the bundle asks for.
#[derive(Serialize)]
struct PermissionDetails {
    name: String,
    lets: String,
    tier: String,
    need: String,
    folders: Vec<FolderDetails>,
    ports: Vec<String>,
    apps: Vec<String>,
    why: Option<String>,
}

#[derive(Serialize)]
struct FolderDetails {
    folder: String,
    access: String,
}

/// What the person said of a permission: `allow` or `never`.
#[derive(Serialize)]
struct GrantDetails {
    permission: String,
    said: &'static str,
    since: String,
    given: Vec<String>,
}

/// `seconds` since 1970 as RFC 3339.
fn at(seconds: u64) -> String {
    json::time(i64::try_from(seconds).unwrap_or(i64::MAX))
}

fn trust_details(trust: &Trust) -> TrustDetails {
    let (kind, catalog, since) = match trust {
        Trust::Catalog { name, since } => ("catalog", Some(name.clone()), *since),
        Trust::Developer => ("developer", None, None),
        Trust::Bound { since } => ("bound", None, Some(*since)),
        Trust::BoundForEveryone { since } => ("bound_for_everyone", None, Some(*since)),
        Trust::Unknown => ("unknown", None, None),
    };
    TrustDetails { kind, catalog, since: since.map(at) }
}

/// The flaws the catalog's list knows of the bundle, as [`flaws`] tells
/// them.
fn flaws_details(facts: &Facts<'_>) -> FlawsDetails {
    let m = facts.manifest;
    let unknown = FlawsDetails { state: "unknown", list_made: None, known: Vec::new() };
    let (Trust::Catalog { .. }, Some(list)) = (facts.trust, facts.flaws) else {
        return unknown;
    };
    if !list.checked(&m.full_id(), &m.version.0) {
        return unknown;
    }
    let known: Vec<FlawDetails> = list
        .of(&m.full_id(), &m.version.0)
        .map(|flaw| FlawDetails {
            id: flaw.id.clone(),
            severity: flaw.severity.word(),
            component: flaw.component.clone(),
            fixed: flaw.fixed.clone(),
            summary: flaw.summary.clone(),
        })
        .collect();
    let state = if known.is_empty() { "none" } else { "listed" };
    FlawsDetails { state, list_made: Some(list.made.clone()), known }
}

fn permission_details(manifest: &Manifest) -> Vec<PermissionDetails> {
    manifest
        .permissions
        .iter()
        .filter_map(|(name, request)| {
            let permission = permissions::find(name)?;
            Some(PermissionDetails {
                name: name.to_string(),
                lets: permission.lets.to_string(),
                tier: permission.tier.words().to_string(),
                need: request.need.word().to_string(),
                folders: request
                    .folders
                    .iter()
                    .map(|(folder, access)| FolderDetails {
                        folder: folder.clone(),
                        access: access.to_string(),
                    })
                    .collect(),
                ports: request.ports.iter().map(ToString::to_string).collect(),
                apps: request.apps.iter().map(ToString::to_string).collect(),
                why: request.why.as_ref().map(|why| why.0.clone()),
            })
        })
        .collect()
}

/// The facts of an installed bundle as `bundle info --json` says them:
/// [`write`]'s, the person's `grants` and where the bundle is.
pub fn details(facts: &Facts<'_>, grants: &[Grant], installation: &Installation<'_>) -> Details {
    let m = facts.manifest;
    let commands = |runs_with: RunsWith| -> Vec<String> {
        m.commands.iter().filter(|c| c.runs_with == runs_with).map(|c| c.name.to_string()).collect()
    };
    let mut grants: Vec<&Grant> = grants.iter().collect();
    grants.sort_by(|a, b| a.permission.cmp(&b.permission));
    Details {
        id: m.full_id(),
        name: m.name.0.clone(),
        version: m.version.0.clone(),
        publisher: m.publisher.name.0.clone(),
        summary: m.summary.0.clone(),
        for_everyone: facts.place == Place::Everyone,
        key: facts.key.encoded(),
        trust: trust_details(facts.trust),
        files: facts.files,
        bytes: facts.bytes,
        support: m.support.as_ref().map(|support| support.0.clone()),
        update_feed: m
            .update
            .as_ref()
            .filter(|_| !matches!(facts.trust, Trust::Catalog { .. }))
            .map(|update| update.feed.0.clone()),
        flaws: flaws_details(facts),
        permissions: permission_details(m),
        commands: commands(RunsWith::Sandbox),
        commands_with_your_access: commands(RunsWith::YourAccess),
        services: super::services::sheet_lines(m),
        grants: grants
            .into_iter()
            .map(|grant| GrantDetails {
                permission: grant.permission.clone(),
                said: grant.said.word(),
                since: at(grant.since),
                given: grant.given.clone(),
            })
            .collect(),
        installed: at(installation.installed),
        folder: installation.folder.to_string(),
        data: installation.data.to_string(),
        data_size: installation.data_size,
        previous: installation.previous.map(String::from),
        held: installation.held,
        license: m.license.0.clone(),
        service_list: m
            .services
            .iter()
            .map(|service| ServiceDetails {
                name: service.name.0.clone(),
                account: super::services::account(service),
                start: if installation.started.contains(&service.name.0) { "on" } else { "off" },
            })
            .collect(),
        provides: m
            .provides
            .iter()
            .map(|provide| Offered {
                interface: provide.interface.as_ref().map(|interface| interface.0.clone()),
                socket: provide.socket.as_ref().map(|socket| socket.0.clone()),
            })
            .collect(),
        uses: m
            .uses
            .iter()
            .map(|used| Offered {
                interface: used.interface.as_ref().map(|interface| interface.0.clone()),
                socket: used.socket.as_ref().map(|socket| socket.0.clone()),
            })
            .collect(),
        used_by: installation.used_by.to_vec(),
    }
}

#[cfg(test)]
mod tests {
    use super::super::grants::Said;
    use super::*;
    use crate::bundles::minisign::SecretKey;
    use dkv::flaws::Severity;

    #[test]
    fn the_sheet_says_what_it_asks_for() {
        let text = manifest::bundle::template("org.example.notes", "Notes", "Example, Inc.", "MIT")
            .replace(
                "\"permissions\": {}",
                "\"permissions\": {\"network.outbound\": {\"need\": \"required\", \"why\": \"To \
             sync.\"}, \"network.listen\": {\"need\": \"optional\", \"ports\": [8080]}}",
            );
        let mut random = [3u8; 40];
        random[0] = 4;
        let key = SecretKey::generate(&random).unwrap().public();
        let text = text.replace(
            "\"permissions\":",
            &format!(
                "\"update\": {{\"feed\": \"https://example.org/notes/feed.json\", \"key\": \"{}\"}}, \
                 \"permissions\":",
                key.encoded()
            ),
        );
        let manifest = manifest::bundle::from_json(&text).unwrap();
        let sheet = |trust| {
            write(&Facts {
                manifest: &manifest,
                key: &key,
                trust: &trust,
                place: Place::User(1000),
                files: 3,
                bytes: 12_345,
                replaces: None,
                flaws: None,
                all_flaws: false,
            })
        };
        let unknown = sheet(Trust::Unknown);
        for line in [
            "Notes 0.1.0, for yourself\n",
            "  org.example.notes, by Example, Inc.\n",
            &format!("  Signed by key {}\n", key.encoded()),
            "  Warning: this computer does not know this key.",
            "  3 files, 12.3 kB\n",
            "    Connect to the internet and to services on this computer (at install)\n",
            "      In its words: \"To sync.\"\n",
            "    Accept connections on these ports: 8080 (at install, optional)\n",
        ] {
            assert!(unknown.contains(line), "{line}\n{unknown}");
        }
        assert!(unknown.contains("  Commands: notes\n"), "{unknown}");
        assert!(!unknown.contains("your access"), "{unknown}");
        assert!(sheet(Trust::Developer).contains("Your developer key"));
        assert!(!sheet(Trust::Developer).contains("Warning"));
        assert!(sheet(Trust::Bound { since: 1_793_000_000 }).contains("here, on 2026-10-26."));
        assert!(unknown.contains("  No catalog it trusts lists this file."), "{unknown}");
        let listed = sheet(Trust::Catalog { name: "dukovo".into(), since: None });
        assert!(listed.contains("  From catalog dukovo, which this computer trusts: it lists"));
        assert!(!listed.contains("Warning"));
        // Its own feed, where no catalog lists it.
        let feed = "  It updates from its own feed at https://example.org/notes/feed.json\n";
        assert!(unknown.contains(feed), "{unknown}");
        assert!(!listed.contains("its own feed"), "{listed}");
        let bound = sheet(Trust::Catalog { name: "dukovo".into(), since: Some(1_793_000_000) });
        assert!(
            bound.contains("this very file.\n  The bundle has come from there since 2026-10-26.\n"),
            "{bound}"
        );
        // An update: what it replaces, and what it asks for anew.
        let new = ["network.listen".to_string()];
        let update = write(&Facts {
            manifest: &manifest,
            key: &key,
            trust: &Trust::Developer,
            place: Place::Everyone,
            files: 3,
            bytes: 12_345,
            replaces: Some(("0.0.9", &new)),
            flaws: None,
            all_flaws: false,
        });
        assert!(update.starts_with("Notes 0.1.0, for everyone, in place of 0.0.9\n"), "{update}");
        assert!(update.contains("(at install, optional), new in this version\n"), "{update}");
        assert!(update.contains("this computer (at install)\n"), "{update}");
        assert!(!update.contains("Known flaws"), "{update}");
        assert!(!update.contains("problem reports"), "{update}");
    }

    #[test]
    fn the_sheet_tells_of_known_flaws() {
        let text = manifest::bundle::template("org.example.notes", "Notes", "Example, Inc.", "MIT")
            .replace(
                "\"permissions\": {}",
                "\"permissions\": {}, \"support\": \"bugs@example.org\"",
            );
        let manifest = manifest::bundle::from_json(&text).unwrap();
        let key = SecretKey::generate(&[5u8; 40]).unwrap().public();
        let trust = Trust::Catalog { name: "dukovo".into(), since: None };
        let flaw = |id: &str, severity, fixed: Option<&str>| dkv::flaws::Flaw {
            bundle: "org.example.notes".into(),
            version: "0.1.0".into(),
            id: id.into(),
            aliases: Vec::new(),
            component: "regex 1.10.2".into(),
            severity,
            summary: format!("{id}'s summary"),
            fixed: fixed.map(String::from),
        };
        let checked = vec![("org.example.notes".to_string(), "0.1.0".to_string())];
        let none = List { made: "2026-11-03T03:00:00Z".into(), checked, flaws: Vec::new() };
        let mut some = none.clone();
        some.flaws = (1..=7)
            .map(|n| flaw(&format!("RUSTSEC-2026-000{n}"), Severity::High, Some("1.10.3")))
            .collect();
        some.flaws[0] = flaw("RUSTSEC-2026-0001", Severity::Critical, None);
        let sheet = |flaws: Option<&List>, all_flaws| {
            write(&Facts {
                manifest: &manifest,
                key: &key,
                trust: &trust,
                place: Place::User(1000),
                files: 3,
                bytes: 12_345,
                replaces: None,
                flaws,
                all_flaws,
            })
        };
        assert!(
            sheet(None, false)
                .contains("  Its publisher takes problem reports at bugs@example.org\n")
        );
        assert!(sheet(None, false).contains("  Known flaws: catalog dukovo does not say.\n"));
        let mut other = none.clone();
        other.checked[0].1 = "0.0.9".into();
        assert!(sheet(Some(&other), false).contains("does not say"));
        assert!(
            sheet(Some(&none), false)
                .contains("  Known flaws: none, by catalog dukovo's list of 2026-11-03.\n")
        );
        let few = sheet(Some(&some), false);
        for line in [
            "  Warning: catalog dukovo's list of 2026-11-03 knows of 7 flaws\n",
            "    RUSTSEC-2026-0001 (critical) in regex 1.10.2, which no version fixes yet\n",
            "      RUSTSEC-2026-0001's summary\n",
            "    RUSTSEC-2026-0005 (high) in regex 1.10.2, fixed in 1.10.3\n",
            "    and 2 more, which bundle info names\n",
        ] {
            assert!(few.contains(line), "{line}\n{few}");
        }
        assert!(!few.contains("RUSTSEC-2026-0006"), "{few}");
        let all = sheet(Some(&some), true);
        assert!(all.contains("RUSTSEC-2026-0007 (high)") && !all.contains("more"), "{all}");
        assert!(few.lines().all(|l| l.chars().count() <= 80), "{few}");
        // A bundle from no catalog: its flaws are not the sheet's to say.
        let developer = write(&Facts {
            manifest: &manifest,
            key: &key,
            trust: &Trust::Developer,
            place: Place::User(1000),
            files: 3,
            bytes: 12_345,
            replaces: None,
            flaws: Some(&some),
            all_flaws: true,
        });
        assert!(!developer.contains("flaws"), "{developer}");
    }

    #[test]
    fn the_details_say_the_sheets_facts_as_a_document() {
        let text = manifest::bundle::template("org.example.notes", "Notes", "Example, Inc.", "MIT")
            .replace(
                "\"permissions\": {}",
                "\"services\": [{\"name\": \"notes-sync\", \"program\": \"notes\", \"account\": \
             \"notes\", \"sockets\": [{\"path\": \"api\"}]}, {\"name\": \"notes-mail\", \
             \"program\": \"notes\"}], \"provides\": [{\"socket\": \"api\", \"to\": \"anyone\"}, \
             {\"interface\": \"org.example.notes/1\", \"to\": \"anyone\"}], \"uses\": \
             [{\"interface\": \"dukovo.certificates/1\", \"need\": \"optional\"}, {\"socket\": \
             \"org.example.mail/smtp\", \"need\": \"optional\"}], \"permissions\": \
             {\"network.outbound\": {\"need\": \"required\", \"why\": \"To sync.\"}, \
             \"network.listen\": {\"need\": \"optional\", \"ports\": [8080]}}, \"support\": \
             \"bugs@example.org\"",
            );
        let manifest = manifest::bundle::from_json(&text).unwrap();
        let key = SecretKey::generate(&[5u8; 40]).unwrap().public();
        let trust = Trust::Catalog { name: "dukovo".into(), since: Some(1_793_000_000) };
        let flaw = dkv::flaws::Flaw {
            bundle: "org.example.notes".into(),
            version: "0.1.0".into(),
            id: "RUSTSEC-2026-0001".into(),
            aliases: Vec::new(),
            component: "regex 1.10.2".into(),
            severity: Severity::High,
            summary: "a summary".into(),
            fixed: Some("1.10.3".into()),
        };
        let checked = vec![("org.example.notes".to_string(), "0.1.0".to_string())];
        let list = List { made: "2026-11-03T03:00:00Z".into(), checked, flaws: vec![flaw] };
        let facts = Facts {
            manifest: &manifest,
            key: &key,
            trust: &trust,
            place: Place::Everyone,
            files: 3,
            bytes: 12_345,
            replaces: None,
            flaws: Some(&list),
            all_flaws: true,
        };
        let grants = [
            Grant {
                permission: "network.outbound".into(),
                said: Said::Allow,
                since: 1_793_000_000,
                given: Vec::new(),
                asked: Vec::new(),
            },
            Grant {
                permission: "network.listen".into(),
                said: Said::Never,
                since: 1_793_100_000,
                given: Vec::new(),
                asked: Vec::new(),
            },
        ];
        let user = user_of("org.example.reader", "org.example.notes/api", None);
        let used_by = dependents(&manifest, [&user]);
        let installation = Installation {
            installed: 1_793_000_000,
            folder: "/applications/org.example.notes",
            data: "each person's data",
            data_size: Some(DataSize {
                bytes: 1_234_567_890,
                files: 3412,
                complete: true,
                measured_ms: 1_793_000_100_000,
            }),
            previous: Some("0.0.9"),
            held: true,
            started: &["notes-sync".to_string()],
            used_by: &used_by,
        };
        let line = json::line(&details(&facts, &grants, &installation)).unwrap();
        json::assert_golden(&line, include_str!("../../tests/json/bundle-info.json"));
    }

    /// A bundle called `id` that uses the socket `socket`, or the interface
    /// `interface`.
    fn user_of(id: &str, socket: &str, interface: Option<&str>) -> Manifest {
        let uses = match interface {
            Some(interface) => {
                format!("{{\"interface\": \"{interface}\", \"need\": \"optional\"}}")
            }
            None => format!("{{\"socket\": \"{socket}\", \"need\": \"optional\"}}"),
        };
        // The template's one program is named for the last part of the id.
        let program = id.rsplit('.').next().unwrap_or(id);
        let text = manifest::bundle::template(id, "A user", "Example, Inc.", "MIT").replace(
            "\"permissions\": {}",
            &format!(
                "\"services\": [{{\"name\": \"{program}\", \"program\": \"{program}\"}}], \
                 \"uses\": [{uses}], \"permissions\": {{}}"
            ),
        );
        manifest::bundle::from_json(&text).unwrap()
    }

    /// A provider of the socket `api` and the interface
    /// `org.example.notes/1`.
    fn provider() -> Manifest {
        let text = manifest::bundle::template("org.example.notes", "Notes", "Example, Inc.", "MIT")
            .replace(
                "\"permissions\": {}",
                "\"services\": [{\"name\": \"notes\", \"program\": \"notes\", \"sockets\": \
                 [{\"path\": \"api\"}]}], \"provides\": [{\"socket\": \"api\", \"to\": \"anyone\"}, \
                 {\"interface\": \"org.example.notes/1\", \"to\": \"anyone\"}], \"permissions\": {}",
            );
        manifest::bundle::from_json(&text).unwrap()
    }

    #[test]
    fn a_provider_is_used_by_the_bundles_of_its_place_that_name_what_it_provides() {
        let provider = provider();
        let by_socket = user_of("org.example.reader", "org.example.notes/api", None);
        let by_interface = user_of("org.example.writer", "", Some("org.example.notes/1"));
        let other_socket = user_of("org.example.other", "org.example.notes/web", None);
        let other_interface = user_of("org.example.newer", "", Some("org.example.notes/2"));
        let unrelated = user_of("org.example.mail", "org.example.smtp/out", None);
        let all = [&by_socket, &by_interface, &other_socket, &other_interface, &unrelated];
        let rows: Vec<(String, String)> =
            dependents(&provider, all).into_iter().map(|d| (d.id, d.what)).collect();
        assert_eq!(
            rows,
            [
                ("org.example.reader".to_string(), "org.example.notes/api".to_string()),
                ("org.example.writer".to_string(), "org.example.notes/1".to_string()),
            ]
        );
        // None of the others use it, and it does not use itself.
        assert!(dependents(&provider, [&other_socket, &unrelated, &provider]).is_empty());
        assert!(dependents(&provider, std::iter::empty()).is_empty());
        // A bundle that names two things it provides has a row for each.
        let both = user_of("org.example.reader", "org.example.notes/api", None);
        let again = user_of("org.example.reader", "", Some("org.example.notes/1"));
        let mut twice = both.clone();
        twice.uses.extend(again.uses.clone());
        let rows = dependents(&provider, [&twice]);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].what, "org.example.notes/1");
        assert_eq!(rows[1].what, "org.example.notes/api");
        assert_eq!(rows[1].name, "A user");
    }

    #[test]
    fn a_service_starts_when_its_file_says_so_and_a_bundle_with_none_has_none() {
        let provider = provider();
        let say = |started: &[String]| {
            let installation = Installation {
                installed: 1,
                folder: "/applications/org.example.notes",
                data: "data",
                data_size: None,
                previous: None,
                held: false,
                started,
                used_by: &[],
            };
            let facts = Facts {
                manifest: &provider,
                key: &SecretKey::generate(&[5u8; 40]).unwrap().public(),
                trust: &Trust::Unknown,
                place: Place::Everyone,
                files: 1,
                bytes: 1,
                replaces: None,
                flaws: None,
                all_flaws: true,
            };
            serde_json::to_value(details(&facts, &[], &installation)).unwrap()
        };
        let off = say(&[]);
        assert_eq!(
            off["service_list"][0],
            serde_json::json!({"name": "notes", "account": "notes", "start": "off"})
        );
        let on = say(&["notes".to_string()]);
        assert_eq!(on["service_list"][0]["start"], "on");
        assert_eq!(on["license"], "MIT");
        assert_eq!(on["provides"][0], serde_json::json!({"interface": null, "socket": "api"}));
        assert_eq!(on["used_by"], serde_json::json!([]));
    }

    #[test]
    fn sizes_and_shown_text() {
        assert_eq!(size(999), "999 bytes");
        assert_eq!(size(1000), "1.0 kB");
        assert_eq!(size(2_500_000), "2.5 MB");
        assert_eq!(size(7_000_000_000_000), "7000.0 GB");
        assert_eq!(shown("a\tb\u{7}c", 10), "a b c");
        assert_eq!(shown("abcdefghijk", 8), "abcde...");
        assert_eq!(shown("abcdefgh", 8), "abcdefgh");
    }
}
