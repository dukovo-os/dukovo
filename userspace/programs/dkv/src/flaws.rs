// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Known flaws (ADR 0389): which components a bundle's SBOM lists by
//! their package URLs, which of them an advisory in the OSV format
//! (<https://ossf.github.io/osv-schema/>) says are affected, and the list
//! of a catalog's flaws that its update server signs each day.
//!
//! An advisory names a component by its ecosystem and name, and the
//! versions it affects as a list, or as ranges of events: `introduced`,
//! `fixed` and `last_affected`. Ranges are read where versions order as
//! semantic versioning orders them, which `SEMVER` ranges and the
//! `crates.io` and `npm` ecosystems do; a list of versions is read in
//! every ecosystem. An advisory that is withdrawn, or only informational
//! (a crate that is unmaintained), is no flaw; one of malicious code
//! (OSV's `MAL-` IDs) is critical.

use std::cmp::Ordering;
use std::fmt;

use serde_json::{Value, json};

/// The most flaws a catalog's list holds.
pub const FLAWS_MAX: usize = 100_000;

/// The role a catalog's targets delegate its list of flaws to, which the
/// update server's online key signs each day: it may list `flaws/` alone,
/// so the key that signs it cannot list a bundle.
pub const ROLE: &str = "flaws";

/// The list's path among a catalog's targets.
pub const TARGET: &str = "flaws/flaws.json";

/// A component an SBOM lists, by its package URL.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Component {
    /// Its ecosystem, as OSV names it: `crates.io`, `npm`, `PyPI`...
    pub ecosystem: String,
    /// Its name there, and its version.
    pub name: String,
    /// Its version.
    pub version: String,
}

/// How bad a flaw is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    /// The advisory does not say.
    Unknown,
    /// CVSS below 4.
    Low,
    /// CVSS from 4 to below 7.
    Medium,
    /// CVSS from 7 to below 9.
    High,
    /// CVSS 9 and above.
    Critical,
}

impl Severity {
    /// Its word.
    #[must_use]
    pub fn word(self) -> &'static str {
        match self {
            Severity::Unknown => "unknown",
            Severity::Low => "low",
            Severity::Medium => "medium",
            Severity::High => "high",
            Severity::Critical => "critical",
        }
    }

    /// The severity a word names; none for another word.
    #[must_use]
    pub fn from_word(word: &str) -> Option<Severity> {
        match word.to_ascii_lowercase().as_str() {
            "unknown" => Some(Severity::Unknown),
            "low" => Some(Severity::Low),
            "medium" | "moderate" => Some(Severity::Medium),
            "high" => Some(Severity::High),
            "critical" => Some(Severity::Critical),
            _ => None,
        }
    }

    /// The severity of a CVSS base score, as CVSS rates it.
    #[must_use]
    pub fn of_score(score: f64) -> Severity {
        match score {
            s if s >= 9.0 => Severity::Critical,
            s if s >= 7.0 => Severity::High,
            s if s >= 4.0 => Severity::Medium,
            s if s > 0.0 => Severity::Low,
            _ => Severity::Unknown,
        }
    }
}

impl fmt::Display for Severity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.word())
    }
}

/// A known flaw of a version of a bundle.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Flaw {
    /// The bundle's full ID.
    pub bundle: String,
    /// The bundle's version.
    pub version: String,
    /// The advisory's ID, such as `RUSTSEC-2026-0001`.
    pub id: String,
    /// The other IDs it is known by, such as a CVE's.
    pub aliases: Vec<String>,
    /// The component it is in, as `NAME VERSION`.
    pub component: String,
    /// How bad it is.
    pub severity: Severity,
    /// What it is, in a line.
    pub summary: String,
    /// The first version of the component without it, if there is one.
    pub fixed: Option<String>,
}

/// `text` on one line, without control characters, at most `max`
/// characters.
fn one_line(text: &str, max: usize) -> String {
    let line: String = text
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .filter(|c| !c.is_control())
        .collect();
    if line.chars().count() > max {
        line.chars().take(max.saturating_sub(3)).collect::<String>() + "..."
    } else {
        line
    }
}

/// Text without `%` escapes, as a package URL writes it.
fn unescaped(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = text.get(i + 1..i + 3)?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// The component a package URL, `pkg:TYPE/NAMESPACE/NAME@VERSION`, names,
/// in the ecosystem OSV calls its type by; none for a type OSV has no
/// ecosystem of, or a URL without a version.
#[must_use]
pub fn purl(url: &str) -> Option<Component> {
    let rest = url.strip_prefix("pkg:")?;
    let rest = rest.split(['?', '#']).next()?;
    let (path, version) = rest.rsplit_once('@')?;
    let (kind, path) = path.split_once('/')?;
    let parts: Vec<String> = path.split('/').map(unescaped).collect::<Option<_>>()?;
    let (namespace, name) = match parts.as_slice() {
        [name] => (None, name.clone()),
        [namespace @ .., name] => (Some(namespace.join("/")), name.clone()),
        [] => return None,
    };
    let version = unescaped(version)?;
    let joined = |sep: &str| match &namespace {
        Some(ns) => format!("{ns}{sep}{name}"),
        None => name.clone(),
    };
    let (ecosystem, name) = match kind.to_ascii_lowercase().as_str() {
        "cargo" => ("crates.io", name.clone()),
        "npm" => ("npm", joined("/")),
        "pypi" => ("PyPI", name.to_ascii_lowercase().replace('_', "-")),
        "gem" => ("RubyGems", name.clone()),
        "golang" => ("Go", joined("/")),
        "maven" => ("Maven", joined(":")),
        "nuget" => ("NuGet", name.clone()),
        "composer" => ("Packagist", joined("/")),
        "hex" => ("Hex", name.clone()),
        "pub" => ("Pub", name.clone()),
        _ => return None,
    };
    let fits = |t: &str| !t.is_empty() && t.len() <= 256 && !t.chars().any(char::is_control);
    (fits(&name) && fits(&version)).then(|| Component {
        ecosystem: ecosystem.into(),
        name,
        version,
    })
}

/// The components the SPDX tag-value document `sbom` lists by package
/// URL, as `ExternalRef: PACKAGE-MANAGER purl URL` lines.
#[must_use]
pub fn components(sbom: &str) -> Vec<Component> {
    let mut found: Vec<Component> = sbom
        .lines()
        .filter_map(|line| line.strip_prefix("ExternalRef:"))
        .filter_map(|rest| {
            let words: Vec<&str> = rest.split_whitespace().collect();
            match words.as_slice() {
                [category, "purl", url]
                    if matches!(*category, "PACKAGE-MANAGER" | "PACKAGE_MANAGER") =>
                {
                    purl(url)
                }
                _ => None,
            }
        })
        .collect();
    found.sort();
    found.dedup();
    found
}

/// Orders two versions as semantic versioning does: the numbers of each
/// part, then a pre-release below its release, with build metadata left
/// out. A part that is no number orders as text.
#[must_use]
pub fn compare(a: &str, b: &str) -> Ordering {
    fn split(version: &str) -> (&str, Option<&str>) {
        let version = version.split('+').next().unwrap_or_default();
        match version.split_once('-') {
            Some((core, pre)) => (core, Some(pre)),
            None => (version, None),
        }
    }
    fn parts(text: &str) -> Vec<&str> {
        text.split('.').collect()
    }
    fn part(a: &str, b: &str) -> Ordering {
        match (a.parse::<u64>(), b.parse::<u64>()) {
            (Ok(x), Ok(y)) => x.cmp(&y),
            (Ok(_), Err(_)) => Ordering::Less,
            (Err(_), Ok(_)) => Ordering::Greater,
            (Err(_), Err(_)) => a.cmp(b),
        }
    }
    let (core_a, pre_a) = split(a);
    let (core_b, pre_b) = split(b);
    let (pa, pb) = (parts(core_a), parts(core_b));
    for i in 0..pa.len().max(pb.len()) {
        let order = part(pa.get(i).unwrap_or(&"0"), pb.get(i).unwrap_or(&"0"));
        if order != Ordering::Equal {
            return order;
        }
    }
    match (pre_a, pre_b) {
        (None, None) => Ordering::Equal,
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (Some(x), Some(y)) => {
            let (px, py) = (parts(x), parts(y));
            for (x, y) in px.iter().zip(&py) {
                let order = part(x, y);
                if order != Ordering::Equal {
                    return order;
                }
            }
            px.len().cmp(&py.len())
        }
    }
}

/// The CVSS 3.0 or 3.1 base score of `vector`, such as
/// `CVSS:3.1/AV:N/AC:L/PR:N/UI:N/S:U/C:H/I:H/A:H`; none if it is not one.
#[must_use]
pub fn cvss3(vector: &str) -> Option<f64> {
    let mut parts = vector.split('/');
    if !matches!(parts.next(), Some("CVSS:3.0" | "CVSS:3.1")) {
        return None;
    }
    let mut metric = std::collections::BTreeMap::new();
    for part in parts {
        let (name, value) = part.split_once(':')?;
        metric.insert(name, value);
    }
    let changed = match *metric.get("S")? {
        "U" => false,
        "C" => true,
        _ => return None,
    };
    let av = match *metric.get("AV")? {
        "N" => 0.85,
        "A" => 0.62,
        "L" => 0.55,
        "P" => 0.2,
        _ => return None,
    };
    let ac = match *metric.get("AC")? {
        "L" => 0.77,
        "H" => 0.44,
        _ => return None,
    };
    let pr = match (*metric.get("PR")?, changed) {
        ("N", _) => 0.85,
        ("L", false) => 0.62,
        ("L", true) => 0.68,
        ("H", false) => 0.27,
        ("H", true) => 0.5,
        _ => return None,
    };
    let ui = match *metric.get("UI")? {
        "N" => 0.85,
        "R" => 0.62,
        _ => return None,
    };
    let cia = |name: &str| -> Option<f64> {
        match *metric.get(name)? {
            "H" => Some(0.56),
            "L" => Some(0.22),
            "N" => Some(0.0),
            _ => None,
        }
    };
    let (c, i, a) = (cia("C")?, cia("I")?, cia("A")?);
    let iss = 1.0 - (1.0 - c) * (1.0 - i) * (1.0 - a);
    let impact =
        if changed { 7.52 * (iss - 0.029) - 3.25 * (iss - 0.02).powi(15) } else { 6.42 * iss };
    if impact <= 0.0 {
        return Some(0.0);
    }
    let exploitability = 8.22 * av * ac * pr * ui;
    let base = if changed {
        (1.08 * (impact + exploitability)).min(10.0)
    } else {
        (impact + exploitability).min(10.0)
    };
    // CVSS 3.1's round-up, to one decimal, kept from floating point's
    // error as the specification's appendix does.
    #[allow(clippy::cast_possible_truncation)] // A score is at most 10.
    let whole = (base * 100_000.0).round() as i64;
    #[allow(clippy::cast_precision_loss)] // As small as the score.
    Some(if whole % 10_000 == 0 {
        whole as f64 / 100_000.0
    } else {
        (whole / 10_000 + 1) as f64 / 10.0
    })
}

/// An event of a range of affected versions.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Event {
    Introduced(String),
    Fixed(String),
    LastAffected(String),
}

impl Event {
    fn version(&self) -> &str {
        match self {
            Event::Introduced(v) | Event::Fixed(v) | Event::LastAffected(v) => v,
        }
    }
}

/// What an advisory says one component's affected versions are.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Affected {
    ecosystem: String,
    name: String,
    versions: Vec<String>,
    /// Ranges that order as semantic versioning does, each its events.
    ranges: Vec<Vec<Event>>,
}

/// An advisory, as much of it as a flaw needs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Advisory {
    /// Its ID.
    pub id: String,
    /// The other IDs it is known by.
    pub aliases: Vec<String>,
    /// What it is, in a line.
    pub summary: String,
    /// How bad it is: its CVSS 3 score's rating, else its database's.
    pub severity: Severity,
    affected: Vec<Affected>,
}

/// Ecosystems whose versions order as semantic versioning does.
const SEMANTIC: &[&str] = &["crates.io", "npm"];

impl Advisory {
    /// Reads one advisory in the OSV format; none for one that is
    /// withdrawn or only informational.
    ///
    /// # Errors
    ///
    /// If it is not an OSV advisory.
    pub fn read(entry: &Value) -> Result<Option<Advisory>, String> {
        let text = |value: &Value| value.as_str().map(str::to_string);
        let id = text(&entry["id"]).ok_or("an advisory without an id")?;
        if entry.get("withdrawn").is_some_and(|w| !w.is_null())
            || entry["database_specific"].get("informational").is_some_and(|i| !i.is_null())
        {
            return Ok(None);
        }
        let aliases: Vec<String> =
            entry["aliases"].as_array().into_iter().flatten().filter_map(text).take(4).collect();
        let summary = text(&entry["summary"])
            .or_else(|| text(&entry["details"]))
            .map_or_else(String::new, |s| one_line(&s, 200));
        let scored = entry["severity"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|s| s["type"].as_str() == Some("CVSS_V3"))
            .filter_map(|s| s["score"].as_str().and_then(cvss3))
            .fold(None, |best: Option<f64>, s| Some(best.map_or(s, |b| b.max(s))));
        let severity = match scored {
            Some(score) => Severity::of_score(score),
            // Malicious code, as OpenSSF's database lists it: the worst.
            None if id.starts_with("MAL-") => Severity::Critical,
            None => entry["database_specific"]["severity"]
                .as_str()
                .and_then(Severity::from_word)
                .unwrap_or(Severity::Unknown),
        };
        let mut affected = Vec::new();
        for a in entry["affected"].as_array().into_iter().flatten() {
            let (Some(ecosystem), Some(name)) =
                (text(&a["package"]["ecosystem"]), text(&a["package"]["name"]))
            else {
                continue;
            };
            let versions =
                a["versions"].as_array().into_iter().flatten().filter_map(text).collect();
            let semantic = SEMANTIC.contains(&ecosystem.as_str());
            let ranges = a["ranges"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|r| match r["type"].as_str() {
                    Some("SEMVER") => true,
                    Some("ECOSYSTEM") => semantic,
                    _ => false,
                })
                .map(|r| {
                    r["events"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(|e| {
                            let event = e.as_object()?;
                            let (kind, version) = event.iter().next()?;
                            let version = version.as_str()?.to_string();
                            match kind.as_str() {
                                "introduced" => Some(Event::Introduced(version)),
                                "fixed" => Some(Event::Fixed(version)),
                                "last_affected" => Some(Event::LastAffected(version)),
                                _ => None,
                            }
                        })
                        .collect()
                })
                .collect();
            affected.push(Affected { ecosystem, name, versions, ranges });
        }
        Ok(Some(Advisory { id: one_line(&id, 64), aliases, summary, severity, affected }))
    }

    /// Whether it affects `component`: if so, the first version that
    /// fixes it, if one does.
    #[must_use]
    pub fn affects(&self, component: &Component) -> Option<Option<String>> {
        let mut hit = None;
        for a in &self.affected {
            if a.ecosystem != component.ecosystem || a.name != component.name {
                continue;
            }
            if a.versions.contains(&component.version) {
                hit = Some(hit.flatten());
            }
            for events in &a.ranges {
                if let Some(fixed) = in_range(events, &component.version) {
                    hit = Some(hit.flatten().or(fixed));
                }
            }
        }
        hit
    }
}

/// Whether `version` is in the range `events` give: if so, the version
/// that fixes it, if one does.
fn in_range(events: &[Event], version: &str) -> Option<Option<String>> {
    let zero = |v: &str| v == "0";
    let mut sorted: Vec<&Event> = events.iter().collect();
    sorted.sort_by(|a, b| match (zero(a.version()), zero(b.version())) {
        (true, true) => Ordering::Equal,
        (true, false) => Ordering::Less,
        (false, true) => Ordering::Greater,
        (false, false) => compare(a.version(), b.version()),
    });
    let mut affected = false;
    for event in &sorted {
        match event {
            Event::Introduced(v) if zero(v) || compare(v, version) != Ordering::Greater => {
                affected = true;
            }
            Event::Fixed(v) if compare(v, version) != Ordering::Greater => affected = false,
            Event::LastAffected(v) if compare(v, version) == Ordering::Less => affected = false,
            _ => {}
        }
    }
    affected.then(|| {
        sorted.iter().find_map(|e| match e {
            Event::Fixed(v) if compare(v, version) == Ordering::Greater => Some(v.clone()),
            _ => None,
        })
    })
}

/// Reads advisories in the OSV format from `data`: a zip of them, as
/// OSV's exports are, a JSON array, or one.
///
/// # Errors
///
/// If `data` is none of these.
pub fn read_osv(data: &[u8]) -> Result<Vec<Advisory>, String> {
    let mut found = Vec::new();
    if data.starts_with(b"PK") {
        let archive = crate::zip::Archive::read(data)?;
        for entry in archive
            .entries
            .iter()
            .filter(|e| !e.folder && e.path.to_ascii_lowercase().ends_with(".json"))
        {
            let bytes = archive.read_small(entry, 16 << 20)?;
            let value: Value =
                serde_json::from_slice(&bytes).map_err(|e| format!("{}: {e}", entry.path))?;
            found.extend(Advisory::read(&value).map_err(|e| format!("{}: {e}", entry.path))?);
        }
        return Ok(found);
    }
    let value: Value = serde_json::from_slice(data).map_err(|e| e.to_string())?;
    match value {
        Value::Array(entries) => {
            for entry in &entries {
                found.extend(Advisory::read(entry)?);
            }
        }
        entry => found.extend(Advisory::read(&entry)?),
    }
    Ok(found)
}

/// The flaws of version `version` of bundle `bundle`, whose SBOM is
/// `sbom`, that `advisories` know of: the worst first.
#[must_use]
pub fn find(bundle: &str, version: &str, sbom: &str, advisories: &[Advisory]) -> Vec<Flaw> {
    let mut flaws = Vec::new();
    for component in components(sbom) {
        for advisory in advisories {
            if let Some(fixed) = advisory.affects(&component) {
                flaws.push(Flaw {
                    bundle: bundle.into(),
                    version: version.into(),
                    id: advisory.id.clone(),
                    aliases: advisory.aliases.clone(),
                    component: format!("{} {}", component.name, component.version),
                    severity: advisory.severity,
                    summary: advisory.summary.clone(),
                    fixed,
                });
            }
        }
    }
    flaws.sort_by(|a, b| b.severity.cmp(&a.severity).then_with(|| a.id.cmp(&b.id)));
    flaws.dedup_by(|a, b| a.id == b.id && a.component == b.component);
    flaws
}

/// A catalog's list of flaws, as its update server signs it each day.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct List {
    /// When it was made, an RFC 3339 time.
    pub made: String,
    /// The bundles it looked at, by full ID and version, each with an
    /// SBOM: of a bundle not here, it cannot tell.
    pub checked: Vec<(String, String)>,
    /// What it found, the worst of each bundle first.
    pub flaws: Vec<Flaw>,
}

impl List {
    /// Whether it looked at version `version` of bundle `bundle`.
    #[must_use]
    pub fn checked(&self, bundle: &str, version: &str) -> bool {
        self.checked.iter().any(|(b, v)| b == bundle && v == version)
    }

    /// The flaws of version `version` of bundle `bundle`, the worst first.
    pub fn of<'a>(&'a self, bundle: &'a str, version: &'a str) -> impl Iterator<Item = &'a Flaw> {
        self.flaws.iter().filter(move |f| f.bundle == bundle && f.version == version)
    }

    /// It as JSON.
    #[must_use]
    pub fn write(&self) -> Vec<u8> {
        let checked: Vec<Value> = self
            .checked
            .iter()
            .map(|(bundle, version)| json!({ "bundle": bundle, "version": version }))
            .collect();
        let flaws: Vec<Value> = self
            .flaws
            .iter()
            .map(|f| {
                json!({
                    "bundle": f.bundle,
                    "version": f.version,
                    "id": f.id,
                    "aliases": f.aliases,
                    "component": f.component,
                    "severity": f.severity.word(),
                    "summary": f.summary,
                    "fixed": f.fixed,
                })
            })
            .collect();
        let list = json!({ "made": self.made, "checked": checked, "flaws": flaws });
        let mut out = serde_json::to_vec_pretty(&list).unwrap_or_default();
        out.push(b'\n');
        out
    }

    /// Reads a catalog's list of flaws: each text on one line.
    ///
    /// # Errors
    ///
    /// If it is not one, or lists more than [`FLAWS_MAX`] bundles or
    /// flaws.
    pub fn read(data: &[u8]) -> Result<List, String> {
        let value: Value = serde_json::from_slice(data).map_err(|e| format!("its flaws: {e}"))?;
        let text = |v: &Value, max: usize| v.as_str().map(|s| one_line(s, max));
        let array = |key: &str| {
            let listed = value[key].as_array().ok_or(format!("its flaws: no {key}"))?;
            if listed.len() > FLAWS_MAX {
                return Err(format!("its flaws: {key} lists too many"));
            }
            Ok(listed)
        };
        let bundle = |v: &Value| -> Result<(String, String), String> {
            let bundle = text(&v["bundle"], 200).ok_or("a bundle without its ID")?;
            manifest::bundle::check_full_id(&bundle)?;
            let version = text(&v["version"], 64).ok_or("a bundle without its version")?;
            Ok((bundle, version))
        };
        let checked = array("checked")?.iter().map(bundle).collect::<Result<_, _>>()?;
        let flaws = array("flaws")?
            .iter()
            .map(|f| {
                let (bundle, version) = bundle(f)?;
                Ok(Flaw {
                    bundle,
                    version,
                    id: text(&f["id"], 64)
                        .filter(|i| !i.is_empty())
                        .ok_or("a flaw without its id")?,
                    aliases: f["aliases"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(|a| text(a, 64))
                        .take(4)
                        .collect(),
                    component: text(&f["component"], 200).unwrap_or_default(),
                    severity: f["severity"]
                        .as_str()
                        .and_then(Severity::from_word)
                        .unwrap_or(Severity::Unknown),
                    summary: text(&f["summary"], 200).unwrap_or_default(),
                    fixed: text(&f["fixed"], 64),
                })
            })
            .collect::<Result<_, String>>()?;
        Ok(List { made: text(&value["made"], 64).unwrap_or_default(), checked, flaws })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn package_urls_name_components() {
        let c = |e: &str, n: &str, v: &str| Component {
            ecosystem: e.into(),
            name: n.into(),
            version: v.into(),
        };
        assert_eq!(purl("pkg:cargo/regex@1.10.2"), Some(c("crates.io", "regex", "1.10.2")));
        assert_eq!(purl("pkg:npm/%40babel/core@7.0.0"), Some(c("npm", "@babel/core", "7.0.0")));
        assert_eq!(purl("pkg:pypi/Jinja2@3.1.0"), Some(c("PyPI", "jinja2", "3.1.0")));
        assert_eq!(
            purl("pkg:golang/golang.org/x/net@v0.17.0?type=module"),
            Some(c("Go", "golang.org/x/net", "v0.17.0"))
        );
        assert_eq!(
            purl("pkg:maven/org.apache.logging.log4j/log4j-core@2.14.1"),
            Some(c("Maven", "org.apache.logging.log4j:log4j-core", "2.14.1"))
        );
        for bad in ["pkg:cargo/regex", "cargo/regex@1", "pkg:deb/debian/curl@7", "pkg:cargo/@1"] {
            assert_eq!(purl(bad), None, "{bad}");
        }
        let sbom = "PackageName: regex\nExternalRef: PACKAGE-MANAGER purl pkg:cargo/regex@1.10.2\n\
                    ExternalRef: SECURITY cpe23Type cpe:2.3:a:x\n\
                    ExternalRef: PACKAGE_MANAGER purl pkg:cargo/memchr@2.7.0\n";
        assert_eq!(
            components(sbom),
            [c("crates.io", "memchr", "2.7.0"), c("crates.io", "regex", "1.10.2")]
        );
    }

    #[test]
    fn versions_order_as_semantic_versioning() {
        assert_eq!(compare("1.10.0", "1.9.9"), Ordering::Greater);
        assert_eq!(compare("1.0.0-alpha", "1.0.0"), Ordering::Less);
        assert_eq!(compare("1.0.0-alpha.1", "1.0.0-alpha.beta"), Ordering::Less);
        assert_eq!(compare("1.0.0-rc.1", "1.0.0-beta.11"), Ordering::Greater);
        assert_eq!(compare("1.0", "1.0.0"), Ordering::Equal);
        assert_eq!(compare("1.0.0+build", "1.0.0"), Ordering::Equal);
    }

    #[test]
    fn cvss_scores_as_the_calculator_does() {
        let score = |v: &str| cvss3(v).unwrap();
        assert!((score("CVSS:3.1/AV:N/AC:L/PR:N/UI:N/S:U/C:H/I:H/A:H") - 9.8).abs() < 1e-9);
        assert!((score("CVSS:3.1/AV:N/AC:L/PR:N/UI:N/S:C/C:H/I:H/A:H") - 10.0).abs() < 1e-9);
        assert!((score("CVSS:3.1/AV:N/AC:L/PR:N/UI:N/S:U/C:N/I:N/A:H") - 7.5).abs() < 1e-9);
        assert!((score("CVSS:3.1/AV:L/AC:H/PR:L/UI:R/S:U/C:L/I:N/A:N") - 2.2).abs() < 1e-9);
        assert!((score("CVSS:3.0/AV:N/AC:L/PR:L/UI:R/S:C/C:L/I:L/A:N") - 5.4).abs() < 1e-9);
        assert!((score("CVSS:3.1/AV:N/AC:L/PR:N/UI:N/S:U/C:N/I:N/A:N")).abs() < 1e-9);
        assert_eq!(cvss3("CVSS:4.0/AV:N/AC:L/AT:N/PR:N/UI:N/VC:H/VI:H/VA:H/SC:N/SI:N/SA:N"), None);
        assert_eq!(cvss3("CVSS:3.1/AV:N/AC:L"), None);
        assert_eq!(Severity::of_score(9.8), Severity::Critical);
        assert_eq!(Severity::of_score(7.5), Severity::High);
        assert_eq!(Severity::of_score(5.4), Severity::Medium);
        assert_eq!(Severity::of_score(2.0), Severity::Low);
    }

    fn advisory(text: &str) -> Option<Advisory> {
        Advisory::read(&serde_json::from_str(text).unwrap()).unwrap()
    }

    #[test]
    fn advisories_say_which_versions_they_affect() {
        let a = advisory(
            r#"{"id": "RUSTSEC-2026-0001", "aliases": ["CVE-2026-1234"],
                "summary": "Out of bounds\n read",
                "severity": [{"type": "CVSS_V3", "score": "CVSS:3.1/AV:N/AC:L/PR:N/UI:N/S:U/C:N/I:N/A:H"}],
                "affected": [{"package": {"ecosystem": "crates.io", "name": "regex"},
                  "ranges": [{"type": "SEMVER", "events": [{"introduced": "0"}, {"fixed": "1.5.5"},
                    {"introduced": "1.6.0"}, {"fixed": "1.10.3"}]}]}]}"#,
        )
        .unwrap();
        assert_eq!(a.summary, "Out of bounds read");
        assert_eq!(a.severity, Severity::High);
        let regex = |v: &str| Component {
            ecosystem: "crates.io".into(),
            name: "regex".into(),
            version: v.into(),
        };
        assert_eq!(a.affects(&regex("1.0.0")), Some(Some("1.5.5".into())));
        assert_eq!(a.affects(&regex("1.5.5")), None);
        assert_eq!(a.affects(&regex("1.5.9")), None);
        assert_eq!(a.affects(&regex("1.6.0")), Some(Some("1.10.3".into())));
        assert_eq!(a.affects(&regex("1.10.2")), Some(Some("1.10.3".into())));
        assert_eq!(a.affects(&regex("1.10.3")), None);
        let other = Component { name: "regex-lite".into(), ..regex("1.0.0") };
        assert_eq!(a.affects(&other), None);
        // last_affected, and a list of versions.
        let b = advisory(
            r#"{"id": "GHSA-xxxx", "database_specific": {"severity": "MODERATE"},
                "affected": [{"package": {"ecosystem": "crates.io", "name": "regex"},
                  "versions": ["0.1.0"],
                  "ranges": [{"type": "ECOSYSTEM", "events": [{"introduced": "1.0.0"}, {"last_affected": "1.2.0"}]}]}]}"#,
        )
        .unwrap();
        assert_eq!(b.severity, Severity::Medium);
        assert_eq!(b.affects(&regex("1.2.0")), Some(None));
        assert_eq!(b.affects(&regex("1.2.1")), None);
        assert_eq!(b.affects(&regex("0.1.0")), Some(None));
        // Malicious code, which has no score.
        let c = advisory(r#"{"id": "MAL-2026-1", "affected": []}"#).unwrap();
        assert_eq!(c.severity, Severity::Critical);
        // Withdrawn, and only informational: no flaw.
        assert_eq!(advisory(r#"{"id": "X", "withdrawn": "2026-01-01T00:00:00Z"}"#), None);
        assert_eq!(
            advisory(r#"{"id": "Y", "database_specific": {"informational": "unmaintained"}}"#),
            None
        );
    }

    #[test]
    fn a_bundles_flaws_are_found_listed_and_read_back() {
        let advisories = read_osv(
            br#"[{"id": "RUSTSEC-2026-0002", "summary": "Panic", "affected": [{"package":
                {"ecosystem": "crates.io", "name": "memchr"}, "ranges": [{"type": "SEMVER",
                "events": [{"introduced": "2.0.0"}, {"fixed": "2.7.1"}]}]}]},
               {"id": "RUSTSEC-2026-0001", "severity": [{"type": "CVSS_V3", "score":
                "CVSS:3.1/AV:N/AC:L/PR:N/UI:N/S:U/C:H/I:H/A:H"}], "summary": "Bad", "affected":
                [{"package": {"ecosystem": "crates.io", "name": "regex"}, "versions": ["1.10.2"]}]}]"#,
        )
        .unwrap();
        let sbom = "ExternalRef: PACKAGE-MANAGER purl pkg:cargo/regex@1.10.2\n\
                    ExternalRef: PACKAGE-MANAGER purl pkg:cargo/memchr@2.7.0\n";
        let flaws = find("org.example.tool", "1.0.0", sbom, &advisories);
        let ids: Vec<&str> = flaws.iter().map(|f| f.id.as_str()).collect();
        assert_eq!(ids, ["RUSTSEC-2026-0001", "RUSTSEC-2026-0002"]);
        assert_eq!(flaws[0].severity, Severity::Critical);
        assert_eq!(flaws[1].fixed.as_deref(), Some("2.7.1"));
        assert_eq!(flaws[1].component, "memchr 2.7.0");
        let list = List {
            made: "2026-11-03T00:00:00Z".into(),
            checked: vec![("org.example.tool".into(), "1.0.0".into())],
            flaws: flaws.clone(),
        };
        let read = List::read(&list.write()).unwrap();
        assert_eq!(read, list);
        assert!(read.checked("org.example.tool", "1.0.0"));
        assert!(!read.checked("org.example.tool", "1.0.1"));
        assert_eq!(read.of("org.example.tool", "1.0.0").count(), 2);
        let bad = br#"{"checked": [], "flaws": [{"bundle": "../x", "version": "1", "id": "A"}]}"#;
        assert!(List::read(bad).is_err());
        assert!(List::read(br#"{"flaws": []}"#).is_err());
        assert!(find("org.example.tool", "1.0.0", "PackageName: regex\n", &advisories).is_empty());
    }
}
