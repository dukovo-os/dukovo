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

use super::minisign::PublicKey;
use super::registry::{Place, Trust};
use crate::util;

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
}

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
    let services: Vec<String> = m.services.iter().map(|s| s.name.to_string()).collect();
    if !services.is_empty() {
        let _ = writeln!(o, "  Services: {}", services.join(", "));
    }
    o
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bundles::minisign::SecretKey;

    #[test]
    fn the_sheet_says_what_it_asks_for() {
        let text = manifest::bundle::template("org.example.notes", "Notes", "Example, Inc.", "MIT")
            .replace(
                "\"permissions\": {}",
                "\"permissions\": {\"network.outbound\": {\"need\": \"required\", \"why\": \"To \
             sync.\"}, \"network.listen\": {\"need\": \"optional\", \"ports\": [8080]}}",
            );
        let manifest = manifest::bundle::from_json(&text).unwrap();
        let mut random = [3u8; 40];
        random[0] = 4;
        let key = SecretKey::generate(&random).unwrap().public();
        let sheet = |trust| {
            write(&Facts {
                manifest: &manifest,
                key: &key,
                trust: &trust,
                place: Place::User(1000),
                files: 3,
                bytes: 12_345,
                replaces: None,
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
        });
        assert!(update.starts_with("Notes 0.1.0, for everyone, in place of 0.0.9\n"), "{update}");
        assert!(update.contains("(at install, optional), new in this version\n"), "{update}");
        assert!(update.contains("this computer (at install)\n"), "{update}");
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
