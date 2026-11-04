// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The install sheet (ADR 0223): what bundled tells about a bundle before
//! it is installed, and `bundle info` after. The permissions are in the
//! system's own words, which a bundle cannot reword; what the bundle says
//! of itself (its name, its publisher, why it asks) is shown as its own,
//! with any control character taken out, so it draws nothing on the
//! terminal.

use std::fmt::Write as _;

use manifest::bundle::permissions;
use manifest::bundle::{Manifest, RunsWith};

use dkv::flaws::List;

use super::minisign::PublicKey;
use super::registry::{Place, Trust};
use crate::util;

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

#[cfg(test)]
mod tests {
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
