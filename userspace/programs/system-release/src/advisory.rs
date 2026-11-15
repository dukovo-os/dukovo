// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Dukovo Security Advisories, DKSA (ADR 0534): one OSV 1.6 file for each
//! in the source tree's `security/advisories`, which the release keys sign
//! as targets of the update repository, so that a machine can match them
//! against its own version and apps, and nothing about it is sent.
//!
//! An advisory is `DKSA-YYYY-NNNN.json`, with `schema_version` "1.6.0",
//! an `id` that is its file's name, `modified` and `published` (RFC 3339
//! in UTC), a one-line `summary`, `details`, `affected`, and
//! `database_specific.severity`, one of critical, high, medium or low.
//! `aliases`, `references`, `severity` (CVSS vectors), `credits` and
//! `database_specific.action`, a sentence on what to do, are optional.
//!
//! An `affected` entry names a package of the ecosystem `Dukovo`: the
//! system, `dukovo`, or a catalog app by its ID. Its ranges are of type
//! `ECOSYSTEM`, whose events (`introduced`, `fixed` and `last_affected`)
//! are in Dukovo's version order (`slots::version::Version`, so 1.4.1 <
//! 1.4.1.1 < 1.4.2, and a pre-release comes before its release), and an
//! `introduced` of "0" is from the start. [`Range::contains`] evaluates
//! them as OSV does.
//!
//! [`parse`](Advisory::parse) reads one as a machine does, ignoring a field
//! it does not know; [`parse_source`](Advisory::parse_source) reads one of the
//! tree as `system-release advisories` and `./ob security check` do, and
//! also refuses a field Dukovo's advisories do not use, text that still
//! says TODO, and an id that is not the file's name. [`load_dir`] reads and
//! checks a folder of them, and [`Index`] is the list the update repository
//! signs beside them, which a machine reads first.

use std::borrow::Cow;
use std::cmp::Ordering;
use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use slots::version::Version;

use crate::list::hex;

/// The OSV schema version advisories use.
pub const SCHEMA_VERSION: &str = "1.6.0";

/// The ecosystem of every package an advisory names.
pub const ECOSYSTEM: &str = "Dukovo";

/// The system's package name; an app's is its ID.
pub const SYSTEM: &str = "dukovo";

/// Where the index is among the repository's targets.
pub const INDEX_TARGET: &str = "advisories/index.json";

/// The most bytes of an index a machine reads.
pub const INDEX_MAX: u64 = 1 << 20;

/// The most bytes of an advisory a machine reads.
pub const ADVISORY_MAX: u64 = 256 << 10;

/// The most characters of a summary, which is one line.
const SUMMARY_MAX: usize = 120;

/// The most characters of the sentence on what to do.
const ACTION_MAX: usize = 300;

/// The fields of an advisory of the tree, which are OSV's that Dukovo's
/// advisories use.
const FIELDS: &[&str] = &[
    "schema_version",
    "id",
    "modified",
    "published",
    "aliases",
    "summary",
    "details",
    "severity",
    "affected",
    "references",
    "credits",
    "database_specific",
];

/// The kinds of reference OSV has.
const REFERENCES: &[&str] = &[
    "ADVISORY",
    "ARTICLE",
    "DETECTION",
    "DISCUSSION",
    "REPORT",
    "FIX",
    "INTRODUCED",
    "GIT",
    "PACKAGE",
    "EVIDENCE",
    "WEB",
];

/// The kinds of credit OSV has.
const CREDITS: &[&str] = &[
    "FINDER",
    "REPORTER",
    "ANALYST",
    "COORDINATOR",
    "REMEDIATION_DEVELOPER",
    "REMEDIATION_REVIEWER",
    "REMEDIATION_VERIFIER",
    "TOOL",
    "SPONSOR",
    "OTHER",
];

/// The file of the advisory `id` among the targets.
#[must_use]
pub fn target_path(id: &str) -> String {
    format!("advisories/{id}.json")
}

/// The advisory an `advisories/` target's path names, if it names one and
/// is not the index.
#[must_use]
pub fn id_of_target(path: &str) -> Option<&str> {
    let id = path.strip_prefix("advisories/")?.strip_suffix(".json")?;
    valid_id(id).then_some(id)
}

/// An id's year and number, if it is `DKSA-YYYY-NNNN`: four digits of a
/// year that does not start with 0, and four of a number from 1.
#[must_use]
pub fn split_id(id: &str) -> Option<(u32, u32)> {
    let rest = id.strip_prefix("DKSA-")?;
    let (year, number) = rest.split_once('-')?;
    let digits = |text: &str| text.len() == 4 && text.bytes().all(|b| b.is_ascii_digit());
    if !digits(year) || !digits(number) || year.starts_with('0') {
        return None;
    }
    let (year, number) = (year.parse().ok()?, number.parse().ok()?);
    (number > 0).then_some((year, number))
}

/// Whether `id` is `DKSA-YYYY-NNNN`.
#[must_use]
pub fn valid_id(id: &str) -> bool {
    split_id(id).is_some()
}

/// How bad an advisory says its flaw is (ADR 0458's table), the worst
/// the greatest.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    /// Hard to reach, or minor: the next monthly release.
    Low,
    /// As low, a little worse: the next monthly release too.
    Medium,
    /// Local privilege escalation, or a remote crash of a service.
    High,
    /// Remote code execution from the network, or exploited in the wild.
    Critical,
}

impl Severity {
    /// Its word, as the advisory writes it.
    #[must_use]
    pub const fn word(self) -> &'static str {
        match self {
            Severity::Low => "low",
            Severity::Medium => "medium",
            Severity::High => "high",
            Severity::Critical => "critical",
        }
    }

    /// The severity a word names.
    #[must_use]
    pub fn parse(word: &str) -> Option<Severity> {
        [Severity::Low, Severity::Medium, Severity::High, Severity::Critical]
            .into_iter()
            .find(|severity| severity.word() == word)
    }
}

/// An event of a range.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// The first version with the flaw, or "0" for every version from the
    /// start.
    Introduced(String),
    /// The first version without it.
    Fixed(String),
    /// The last version with it.
    LastAffected(String),
}

impl Event {
    /// The version it names.
    #[must_use]
    pub fn version(&self) -> &str {
        match self {
            Event::Introduced(v) | Event::Fixed(v) | Event::LastAffected(v) => v,
        }
    }

    /// Its name in the advisory.
    #[must_use]
    pub const fn kind(&self) -> &'static str {
        match self {
            Event::Introduced(_) => "introduced",
            Event::Fixed(_) => "fixed",
            Event::LastAffected(_) => "last_affected",
        }
    }

    /// Which applies first among events of one version: the start of a
    /// range, then its last affected version, then its fix. OSV does not
    /// say; this keeps a range that starts and is fixed at one version
    /// empty, and one that starts and ends at one version that version.
    const fn rank(&self) -> u8 {
        match self {
            Event::Introduced(_) => 0,
            Event::LastAffected(_) => 1,
            Event::Fixed(_) => 2,
        }
    }
}

/// An OSV range of type `ECOSYSTEM`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Range {
    /// Its events, in the order they are written.
    pub events: Vec<Event>,
}

/// An event's version, where "0" is the lowest of all.
fn bound(text: &str) -> Option<Option<Version<'_>>> {
    if text == "0" { Some(None) } else { Version::parse(text).map(Some) }
}

impl Range {
    /// Whether `version` is in the range, as OSV evaluates one: the events
    /// are sorted by version, and then, for each in turn, an `introduced`
    /// at or before `version` makes it affected, a `fixed` at or before it
    /// does not, and a `last_affected` before it does not. It is in the
    /// range when it is affected after the last event. An event whose
    /// version is not one is left out.
    #[must_use]
    pub fn contains(&self, version: &Version<'_>) -> bool {
        let mut events: Vec<(Option<Version<'_>>, &Event)> = self
            .events
            .iter()
            .filter_map(|event| bound(event.version()).map(|at| (at, event)))
            .collect();
        events.sort_by(|(a, x), (b, y)| a.cmp(b).then(x.rank().cmp(&y.rank())));
        let mut affected = false;
        for (at, event) in &events {
            match event {
                Event::Introduced(_) => {
                    if at.as_ref().is_none_or(|at| at <= version) {
                        affected = true;
                    }
                }
                Event::Fixed(_) => {
                    if at.as_ref().is_some_and(|at| at <= version) {
                        affected = false;
                    }
                }
                Event::LastAffected(_) => {
                    if at.as_ref().is_some_and(|at| at < version) {
                        affected = false;
                    }
                }
            }
        }
        affected
    }
}

/// One package an advisory affects.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Affected {
    /// The system, [`SYSTEM`], or an app's ID.
    pub package: String,
    /// The ranges of versions with the flaw.
    pub ranges: Vec<Range>,
}

/// A CVSS vector.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cvss {
    /// `CVSS_V2`, `CVSS_V3` or `CVSS_V4`.
    pub kind: String,
    /// The vector string.
    pub score: String,
}

/// A link to more about the flaw.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reference {
    /// What it is: `ADVISORY`, `FIX`, `REPORT`...
    pub kind: String,
    /// Where it is, an https address.
    pub url: String,
}

/// Who found, reported or fixed the flaw.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Credit {
    /// Their name.
    pub name: String,
    /// What they did, if said: `FINDER`, `REPORTER`...
    pub kind: Option<String>,
}

/// An advisory, read and checked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Advisory {
    /// `DKSA-YYYY-NNNN`.
    pub id: String,
    /// When it last changed, RFC 3339 in UTC.
    pub modified: String,
    /// When it was first published, RFC 3339 in UTC.
    pub published: String,
    /// Its other IDs: CVEs, GHSAs.
    pub aliases: Vec<String>,
    /// What is wrong, in a line.
    pub summary: String,
    /// What it is, who is affected and what an attacker can do.
    pub details: String,
    /// Its CVSS vectors, if it has any.
    pub cvss: Vec<Cvss>,
    /// The packages and versions it affects.
    pub affected: Vec<Affected>,
    /// Where to read more.
    pub references: Vec<Reference>,
    /// Who found and fixed it.
    pub credits: Vec<Credit>,
    /// How bad it is.
    pub severity: Severity,
    /// What to do, in a sentence, if it says.
    pub action: Option<String>,
}

/// `what.key`, or `key` alone at the top.
fn at(what: &str, key: &str) -> String {
    if what.is_empty() { key.to_string() } else { format!("{what}.{key}") }
}

fn need<'a>(object: &'a Map<String, Value>, what: &str, key: &str) -> Result<&'a Value, String> {
    object.get(key).ok_or_else(|| format!("{} is needed", at(what, key)))
}

fn string<'a>(value: &'a Value, name: &str) -> Result<&'a str, String> {
    value.as_str().ok_or_else(|| format!("{name} must be text"))
}

fn array<'a>(value: &'a Value, name: &str) -> Result<&'a Vec<Value>, String> {
    value.as_array().ok_or_else(|| format!("{name} must be a list"))
}

fn object<'a>(value: &'a Value, name: &str) -> Result<&'a Map<String, Value>, String> {
    value.as_object().ok_or_else(|| format!("{name} must be an object"))
}

/// The text at `key` of `object`, which must be there.
fn needed_text<'a>(
    object: &'a Map<String, Value>,
    what: &str,
    key: &str,
) -> Result<&'a str, String> {
    string(need(object, what, key)?, &at(what, key))
}

/// The text at `key`, if it is there.
fn optional_text<'a>(
    object: &'a Map<String, Value>,
    what: &str,
    key: &str,
) -> Result<Option<&'a str>, String> {
    object.get(key).map(|value| string(value, &at(what, key))).transpose()
}

/// Refuses a key of `object` that is not one of `allowed`.
fn only(object: &Map<String, Value>, what: &str, allowed: &[&str]) -> Result<(), String> {
    match object.keys().find(|key| !allowed.contains(&key.as_str())) {
        Some(key) => Err(format!(
            "{} is not a field of Dukovo's advisories, which have {}",
            at(what, key),
            allowed.join(", ")
        )),
        None => Ok(()),
    }
}

/// The seconds since 1970 of an RFC 3339 time in UTC, `Z` after the
/// seconds, which may have a fraction.
fn time(text: &str, name: &str) -> Result<i64, String> {
    let plain = match text.strip_suffix('Z').and_then(|rest| rest.split_once('.')) {
        Some((whole, fraction))
            if !fraction.is_empty() && fraction.bytes().all(|b| b.is_ascii_digit()) =>
        {
            Cow::Owned(format!("{whole}Z"))
        }
        _ => Cow::Borrowed(text),
    };
    tuf::time::parse(&plain).ok_or_else(|| {
        format!("{name} is {text}, not an RFC 3339 time in UTC, such as 2026-11-15T09:30:00Z")
    })
}

/// A version of an event, which Dukovo's order reads: three or four
/// numbers, then a pre-release if it has one.
fn event_version(text: &str, name: &str, zero: bool) -> Result<(), String> {
    if zero && text == "0" {
        return Ok(());
    }
    if Version::parse(text).is_some() {
        return Ok(());
    }
    let from_the_start = if zero { ", or 0 for from the start" } else { "" };
    Err(format!(
        "{name} is {text}, not a version: use three or four numbers, such as 1.4.2 or \
         1.4.1.1, and a pre-release after - if it is one{from_the_start}"
    ))
}

/// A line of text: not empty, and without a line break.
fn one_line(text: &str, name: &str, most: usize) -> Result<(), String> {
    if text.trim().is_empty() {
        return Err(format!("{name} is empty"));
    }
    if text.contains(['\n', '\r']) {
        return Err(format!("{name} must be one line"));
    }
    if text.chars().count() > most {
        return Err(format!("{name} is longer than {most} characters"));
    }
    Ok(())
}

/// The ranges' events of one `affected` entry's range.
fn read_range(range: &Value, what: &str, strict: bool) -> Result<Range, String> {
    let range = object(range, what)?;
    if strict {
        only(range, what, &["type", "events"])?;
    }
    let kind = needed_text(range, what, "type")?;
    if kind != "ECOSYSTEM" {
        return Err(format!(
            "{} is {kind}: Dukovo's advisories use ECOSYSTEM ranges, in Dukovo's version order",
            at(what, "type")
        ));
    }
    let events_name = at(what, "events");
    let list = array(need(range, what, "events")?, &events_name)?;
    if list.is_empty() {
        return Err(format!("{events_name} is empty: it needs an introduced event"));
    }
    let mut events = Vec::new();
    for (n, event) in list.iter().enumerate() {
        let name = format!("{events_name}[{n}]");
        let event = object(event, &name)?;
        let found: Vec<(&String, &Value)> = event.iter().collect();
        let [(key, value)] = found.as_slice() else {
            return Err(format!(
                "{name} has one of introduced, fixed or last_affected, and no more"
            ));
        };
        let text = string(value, &at(&name, key))?;
        events.push(match key.as_str() {
            "introduced" => {
                event_version(text, &at(&name, key), true)?;
                Event::Introduced(text.to_string())
            }
            "fixed" => {
                event_version(text, &at(&name, key), false)?;
                Event::Fixed(text.to_string())
            }
            "last_affected" => {
                event_version(text, &at(&name, key), false)?;
                Event::LastAffected(text.to_string())
            }
            other => {
                return Err(format!(
                    "{name} has {other}: an event is introduced, fixed or last_affected"
                ));
            }
        });
    }
    let range = Range { events };
    check_order(&range, &events_name)?;
    Ok(range)
}

/// Checks that the events of `range`, sorted, open and close it in turn: an
/// `introduced`, then the `fixed` or `last_affected` that ends it, and so
/// on, the last range left open.
fn check_order(range: &Range, name: &str) -> Result<(), String> {
    if !range.events.iter().any(|event| matches!(event, Event::Introduced(_))) {
        return Err(format!("{name} has no introduced event: use \"0\" for from the start"));
    }
    let mut events: Vec<(Option<Version<'_>>, &Event)> = range
        .events
        .iter()
        .filter_map(|event| bound(event.version()).map(|at| (at, event)))
        .collect();
    events.sort_by(|(a, x), (b, y)| a.cmp(b).then(x.rank().cmp(&y.rank())));
    let mut open: Option<(&Option<Version<'_>>, &str)> = None;
    for (at, event) in &events {
        let version = event.version();
        match (event, open) {
            (Event::Introduced(_), Some((_, since))) => {
                return Err(format!(
                    "{name}: introduced {version} comes while the range is open since {since}: \
                     end it with fixed or last_affected first"
                ));
            }
            (Event::Introduced(_), None) => open = Some((at, version)),
            (_, None) => {
                return Err(format!(
                    "{name}: {} {version} ends nothing: it needs an introduced event before it",
                    event.kind()
                ));
            }
            (Event::Fixed(_), Some((from, since))) => {
                if from == at {
                    return Err(format!(
                        "{name}: fixed {version} is the version introduced {since}: the range is \
                         empty"
                    ));
                }
                open = None;
            }
            (_, Some(_)) => open = None,
        }
    }
    Ok(())
}

/// One entry of `affected`.
fn read_affected(entry: &Value, what: &str, strict: bool) -> Result<Affected, String> {
    let entry = object(entry, what)?;
    if strict {
        only(entry, what, &["package", "ranges"])?;
    }
    if entry.contains_key("versions") {
        return Err(format!(
            "{} is not used: give ranges, which are in Dukovo's version order",
            at(what, "versions")
        ));
    }
    let package_name = at(what, "package");
    let package = object(need(entry, what, "package")?, &package_name)?;
    if strict {
        only(package, &package_name, &["ecosystem", "name"])?;
    }
    let ecosystem = needed_text(package, &package_name, "ecosystem")?;
    if ecosystem != ECOSYSTEM {
        return Err(format!("{} is {ecosystem}, not {ECOSYSTEM}", at(&package_name, "ecosystem")));
    }
    let name = needed_text(package, &package_name, "name")?;
    let valid = !name.is_empty()
        && name.len() <= 128
        && name.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_'));
    if !valid {
        return Err(format!(
            "{} is {name:?}: it is dukovo, or an app's ID, which has letters, digits, . - and _",
            at(&package_name, "name")
        ));
    }
    let ranges_name = at(what, "ranges");
    let list = array(need(entry, what, "ranges")?, &ranges_name)?;
    if list.is_empty() {
        return Err(format!("{ranges_name} is empty: it needs a range"));
    }
    let mut ranges = Vec::new();
    for (n, range) in list.iter().enumerate() {
        ranges.push(read_range(range, &format!("{ranges_name}[{n}]"), strict)?);
    }
    Ok(Affected { package: name.to_string(), ranges })
}

/// A CVSS vector's kind and the start its score must have.
const VECTORS: &[(&str, &[&str])] =
    &[("CVSS_V2", &["AV:"]), ("CVSS_V3", &["CVSS:3.0/", "CVSS:3.1/"]), ("CVSS_V4", &["CVSS:4.0/"])];

fn read_cvss(list: &Value) -> Result<Vec<Cvss>, String> {
    let mut vectors = Vec::new();
    for (n, entry) in array(list, "severity")?.iter().enumerate() {
        let what = format!("severity[{n}]");
        let entry = object(entry, &what)?;
        let kind = needed_text(entry, &what, "type")?;
        let score = needed_text(entry, &what, "score")?;
        let Some((_, starts)) = VECTORS.iter().find(|(name, _)| *name == kind) else {
            return Err(format!(
                "{} is {kind}: say CVSS_V2, CVSS_V3 or CVSS_V4",
                at(&what, "type")
            ));
        };
        if !starts.iter().any(|start| score.starts_with(start)) {
            return Err(format!(
                "{} is {score}, which is not a {kind} vector: it starts with {}",
                at(&what, "score"),
                starts.join(" or ")
            ));
        }
        vectors.push(Cvss { kind: kind.to_string(), score: score.to_string() });
    }
    Ok(vectors)
}

fn read_references(list: &Value) -> Result<Vec<Reference>, String> {
    let mut references = Vec::new();
    for (n, entry) in array(list, "references")?.iter().enumerate() {
        let what = format!("references[{n}]");
        let entry = object(entry, &what)?;
        let kind = needed_text(entry, &what, "type")?;
        if !REFERENCES.contains(&kind) {
            return Err(format!(
                "{} is {kind}: it is one of {}",
                at(&what, "type"),
                REFERENCES.join(", ")
            ));
        }
        let url = needed_text(entry, &what, "url")?;
        if !url.starts_with("https://") || url.contains(char::is_whitespace) || url.len() <= 8 {
            return Err(format!("{} is {url}, which is not an https:// address", at(&what, "url")));
        }
        references.push(Reference { kind: kind.to_string(), url: url.to_string() });
    }
    Ok(references)
}

fn read_credits(list: &Value) -> Result<Vec<Credit>, String> {
    let mut credits = Vec::new();
    for (n, entry) in array(list, "credits")?.iter().enumerate() {
        let what = format!("credits[{n}]");
        let entry = object(entry, &what)?;
        let name = needed_text(entry, &what, "name")?;
        if name.trim().is_empty() {
            return Err(format!("{} is empty", at(&what, "name")));
        }
        let kind = optional_text(entry, &what, "type")?;
        if let Some(kind) = kind.filter(|kind| !CREDITS.contains(kind)) {
            return Err(format!(
                "{} is {kind}: it is one of {}",
                at(&what, "type"),
                CREDITS.join(", ")
            ));
        }
        credits.push(Credit { name: name.to_string(), kind: kind.map(str::to_string) });
    }
    Ok(credits)
}

fn read_aliases(list: &Value) -> Result<Vec<String>, String> {
    let mut aliases = Vec::new();
    for (n, alias) in array(list, "aliases")?.iter().enumerate() {
        let name = format!("aliases[{n}]");
        let alias = string(alias, &name)?;
        if alias.is_empty() || alias.contains(char::is_whitespace) {
            return Err(format!("{name} is {alias:?}: an alias is an ID such as CVE-2026-12345"));
        }
        aliases.push(alias.to_string());
    }
    Ok(aliases)
}

impl Advisory {
    /// Reads and checks the advisory in `bytes`, as a machine does: a
    /// field Dukovo's advisories do not use is left alone.
    ///
    /// # Errors
    ///
    /// What is wrong, in words for the person who wrote it: JSON that
    /// does not parse, a field missing or of the wrong kind, a time or
    /// version that is not one, a range whose events do not open and
    /// close in turn.
    pub fn parse(bytes: &[u8]) -> Result<Advisory, String> {
        Advisory::read(bytes, false)
    }

    /// Reads and checks the advisory of the tree in `bytes`, the file
    /// named `file`, as [`Advisory::parse`] does and more: its id is the
    /// file's name, it has no field Dukovo's advisories do not use, and
    /// none of its text still says TODO.
    ///
    /// # Errors
    ///
    /// As [`Advisory::parse`], or if one of those is not so.
    pub fn parse_source(bytes: &[u8], file: &str) -> Result<Advisory, String> {
        let advisory = Advisory::read(bytes, true)?;
        if file != format!("{}.json", advisory.id) {
            return Err(format!(
                "the id {} is not the file's name: the file is {}.json",
                advisory.id, advisory.id
            ));
        }
        Ok(advisory)
    }

    fn read(bytes: &[u8], strict: bool) -> Result<Advisory, String> {
        if bytes.len() as u64 > ADVISORY_MAX {
            return Err(format!(
                "it is {} bytes, and at most {ADVISORY_MAX} are read",
                bytes.len()
            ));
        }
        let value: Value =
            serde_json::from_slice(bytes).map_err(|err| format!("it is not JSON: {err}"))?;
        let top = value.as_object().ok_or("it is not a JSON object")?;
        if strict {
            only(top, "", FIELDS)?;
        }
        let schema = needed_text(top, "", "schema_version")?;
        if schema != SCHEMA_VERSION {
            return Err(format!(
                "schema_version is {schema}: Dukovo's advisories use OSV {SCHEMA_VERSION}"
            ));
        }
        let id = needed_text(top, "", "id")?;
        if !valid_id(id) {
            return Err(format!(
                "id is {id}, not DKSA-YYYY-NNNN: DKSA, the year, and a number of four digits"
            ));
        }
        let modified = needed_text(top, "", "modified")?;
        let published = needed_text(top, "", "published")?;
        let (changed, first) = (time(modified, "modified")?, time(published, "published")?);
        if changed < first {
            return Err(format!("modified is {modified}, before published, {published}"));
        }
        let summary = needed_text(top, "", "summary")?;
        one_line(summary, "summary", SUMMARY_MAX)?;
        let details = needed_text(top, "", "details")?;
        if details.trim().is_empty() {
            return Err("details is empty".into());
        }
        let specific = object(need(top, "", "database_specific")?, "database_specific")?;
        if strict {
            only(specific, "database_specific", &["severity", "action"])?;
        }
        let word = needed_text(specific, "database_specific", "severity")?;
        let severity = Severity::parse(word).ok_or_else(|| {
            format!("database_specific.severity is {word}: say critical, high, medium or low")
        })?;
        let action = optional_text(specific, "database_specific", "action")?;
        if let Some(action) = action {
            one_line(action, "database_specific.action", ACTION_MAX)?;
        }
        if strict {
            for (name, text) in [
                ("summary", Some(summary)),
                ("details", Some(details)),
                ("database_specific.action", action),
            ] {
                if text.is_some_and(|text| text.contains("TODO")) {
                    return Err(format!("{name} still says TODO: write it"));
                }
            }
        }
        let affected_list = array(need(top, "", "affected")?, "affected")?;
        if affected_list.is_empty() {
            return Err("affected is empty: it needs the system or an app, with its ranges".into());
        }
        let mut affected = Vec::new();
        for (n, entry) in affected_list.iter().enumerate() {
            affected.push(read_affected(entry, &format!("affected[{n}]"), strict)?);
        }
        let list = |key: &str| top.get(key);
        Ok(Advisory {
            id: id.to_string(),
            modified: modified.to_string(),
            published: published.to_string(),
            aliases: list("aliases").map(read_aliases).transpose()?.unwrap_or_default(),
            summary: summary.to_string(),
            details: details.to_string(),
            cvss: list("severity").map(read_cvss).transpose()?.unwrap_or_default(),
            affected,
            references: list("references").map(read_references).transpose()?.unwrap_or_default(),
            credits: list("credits").map(read_credits).transpose()?.unwrap_or_default(),
            severity,
            action: action.map(str::to_string),
        })
    }
}

/// `text` as a version of Dukovo's order: an app's version of one or two
/// numbers has zeros added, so that 668 is 668.0.0 and 5.8 is 5.8.0.
fn padded(text: &str) -> Cow<'_, str> {
    let end = text.find(['-', '+']).unwrap_or(text.len());
    let (core, rest) = text.split_at(end);
    let numbers = core.split('.').count();
    let plain =
        core.split('.').all(|part| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit()));
    if numbers < 3 && plain {
        let zeros = ".0".repeat(3 - numbers);
        Cow::Owned(format!("{core}{zeros}{rest}"))
    } else {
        Cow::Borrowed(text)
    }
}

/// Whether the advisory says version `version` of `package` has the
/// flaw: the system, [`SYSTEM`], or an app by its ID, in a range of one of
/// the advisory's entries for it. A version that is not one of Dukovo's
/// order is not in any range; an app's of one or two numbers is read with
/// zeros added.
#[must_use]
pub fn affects(advisory: &Advisory, package: &str, version: &str) -> bool {
    let text = padded(version);
    let Some(version) = Version::parse(&text) else { return false };
    advisory
        .affected
        .iter()
        .filter(|entry| entry.package == package)
        .any(|entry| entry.ranges.iter().any(|range| range.contains(&version)))
}

/// The versions of `package` the advisory says fix the flaw, the oldest
/// first, each once.
#[must_use]
pub fn fixed_versions(advisory: &Advisory, package: &str) -> Vec<String> {
    let mut fixed: Vec<&str> = advisory
        .affected
        .iter()
        .filter(|entry| entry.package == package)
        .flat_map(|entry| entry.ranges.iter())
        .flat_map(|range| range.events.iter())
        .filter_map(|event| match event {
            Event::Fixed(version) => Some(version.as_str()),
            _ => None,
        })
        .collect();
    fixed.sort_by(|a, b| match (Version::parse(a), Version::parse(b)) {
        (Some(a), Some(b)) => a.cmp(&b),
        _ => a.cmp(b),
    });
    fixed.dedup_by(|a, b| Version::parse(a).zip(Version::parse(b)).is_some_and(|(a, b)| a == b));
    fixed.into_iter().map(str::to_string).collect()
}

/// The packages an advisory affects, each once, in the order it names
/// them.
#[must_use]
pub fn packages(advisory: &Advisory) -> Vec<&str> {
    let mut names: Vec<&str> = Vec::new();
    for entry in &advisory.affected {
        if !names.contains(&entry.package.as_str()) {
            names.push(&entry.package);
        }
    }
    names
}

/// An advisory of the tree: its file, its bytes as signed and the SHA-256
/// of them.
#[derive(Debug, Clone)]
pub struct Source {
    /// The file's name, `DKSA-YYYY-NNNN.json`.
    pub file: String,
    /// What the file holds, which is what is signed.
    pub bytes: Vec<u8>,
    /// The SHA-256 of `bytes`, in hex.
    pub sha256: String,
    /// The advisory read from them.
    pub advisory: Advisory,
}

fn sha256(bytes: &[u8]) -> String {
    hex(ring::digest::digest(&ring::digest::SHA256, bytes).as_ref())
}

/// Reads every `DKSA-*.json` of the folder `dir`, as
/// [`Advisory::parse_source`] does, oldest id first. Other files, such as
/// its README.md, are left alone, but another `.json` is refused, and so
/// are two advisories of one id.
///
/// # Errors
///
/// Every problem, each with its file's name.
pub fn load_dir(dir: &Path) -> Result<Vec<Source>, Vec<String>> {
    let entries =
        std::fs::read_dir(dir).map_err(|err| vec![format!("{}: {err}", dir.display())])?;
    let mut problems = Vec::new();
    let mut sources = Vec::new();
    let mut names = Vec::new();
    for entry in entries {
        match entry {
            Ok(entry) => names.push(entry.file_name().to_string_lossy().into_owned()),
            Err(err) => problems.push(format!("{}: {err}", dir.display())),
        }
    }
    names.sort();
    for name in names {
        let path = dir.join(&name);
        let json = Path::new(&name).extension().is_some_and(|e| e.eq_ignore_ascii_case("json"));
        if !json || !path.is_file() {
            continue;
        }
        if !name.starts_with("DKSA-") {
            problems.push(format!("{name}: an advisory's file is named DKSA-YYYY-NNNN.json"));
            continue;
        }
        let bytes = match std::fs::read(&path) {
            Ok(bytes) => bytes,
            Err(err) => {
                problems.push(format!("{name}: {err}"));
                continue;
            }
        };
        match Advisory::parse_source(&bytes, &name) {
            Ok(advisory) => {
                sources.push(Source { sha256: sha256(&bytes), file: name, bytes, advisory });
            }
            Err(why) => problems.push(format!("{name}: {why}")),
        }
    }
    problems.extend(duplicates(&sources));
    if problems.is_empty() {
        sources.sort_by(|a, b| a.advisory.id.cmp(&b.advisory.id));
        Ok(sources)
    } else {
        Err(problems)
    }
}

/// One problem for each id that two of `sources` have.
fn duplicates(sources: &[Source]) -> Vec<String> {
    let mut seen: Vec<&Source> = Vec::new();
    let mut problems = Vec::new();
    for source in sources {
        match seen.iter().find(|other| other.advisory.id == source.advisory.id) {
            Some(other) => problems.push(format!(
                "{}: {} is also the id of {}: each advisory has a number of its own",
                source.file, source.advisory.id, other.file
            )),
            None => seen.push(source),
        }
    }
    problems
}

/// One advisory in the index: enough to list it, and the hash its file is
/// checked against.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    /// `DKSA-YYYY-NNNN`.
    pub id: String,
    /// When the advisory last changed.
    pub modified: String,
    /// How bad it is.
    pub severity: Severity,
    /// What is wrong, in a line.
    pub summary: String,
    /// The SHA-256 of the advisory's file, in hex.
    pub sha256: String,
}

/// The list of advisories the update repository signs as
/// [`INDEX_TARGET`]: a machine reads it first, and then each advisory.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Index {
    /// Every advisory, by id.
    pub advisories: Vec<Entry>,
}

impl Index {
    /// The index of `sources`, by id: the same for the same advisories,
    /// so that signing them again signs nothing new.
    #[must_use]
    pub fn of(sources: &[Source]) -> Index {
        let mut advisories: Vec<Entry> = sources
            .iter()
            .map(|source| Entry {
                id: source.advisory.id.clone(),
                modified: source.advisory.modified.clone(),
                severity: source.advisory.severity,
                summary: source.advisory.summary.clone(),
                sha256: source.sha256.clone(),
            })
            .collect();
        advisories.sort_by(|a, b| a.id.cmp(&b.id));
        Index { advisories }
    }

    /// Reads an index.
    ///
    /// # Errors
    ///
    /// If it is not JSON of the index's form, names an id that is not
    /// DKSA-YYYY-NNNN or a hash that is not a SHA-256, or lists an
    /// advisory twice.
    pub fn parse(bytes: &[u8]) -> Result<Index, String> {
        if bytes.len() as u64 > INDEX_MAX {
            return Err(format!(
                "the advisories' index is {} bytes, and at most {INDEX_MAX} are read",
                bytes.len()
            ));
        }
        let index: Index = serde_json::from_slice(bytes)
            .map_err(|err| format!("the advisories' index is malformed: {err}"))?;
        for (n, entry) in index.advisories.iter().enumerate() {
            if !valid_id(&entry.id) {
                return Err(format!("the advisories' index names {}, not a DKSA id", entry.id));
            }
            let hash = entry.sha256.len() == 64
                && entry.sha256.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
            if !hash {
                return Err(format!(
                    "the advisories' index gives {} a hash that is no SHA-256",
                    entry.id
                ));
            }
            if index.advisories[..n].iter().any(|other| other.id == entry.id) {
                return Err(format!("the advisories' index lists {} twice", entry.id));
            }
        }
        Ok(index)
    }

    /// The file as it is signed.
    ///
    /// # Panics
    ///
    /// Never: its fields are all JSON's.
    #[must_use]
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut bytes = serde_json::to_vec_pretty(self).expect("an index is JSON");
        bytes.push(b'\n');
        bytes
    }
}

/// Checks the advisories among a repository's `targets`, as the client
/// read them: each advisory's file is the advisory its path names, and
/// the index lists those and no others, each with the hash, time,
/// severity and summary of its file. Gives the index's entries, newest
/// id last; a repository with no advisory gives none.
///
/// # Errors
///
/// What does not agree, in words.
pub fn check_served(targets: &[(String, Vec<u8>)]) -> Result<Vec<Entry>, String> {
    let index = targets.iter().find(|(path, _)| path == INDEX_TARGET);
    let files: Vec<&(String, Vec<u8>)> =
        targets.iter().filter(|(path, _)| id_of_target(path).is_some()).collect();
    let Some((_, index)) = index else {
        return match files.first() {
            Some((path, _)) => Err(format!("{path} is signed, and {INDEX_TARGET} is not")),
            None => Ok(Vec::new()),
        };
    };
    let index = Index::parse(index)?;
    for (path, bytes) in &files {
        let advisory = Advisory::parse(bytes).map_err(|why| format!("{path}: {why}"))?;
        if target_path(&advisory.id) != *path {
            return Err(format!("{path} holds the advisory {}", advisory.id));
        }
        let Some(entry) = index.advisories.iter().find(|entry| entry.id == advisory.id) else {
            return Err(format!("{path} is signed, and the index does not list it"));
        };
        let same = entry.sha256 == sha256(bytes)
            && entry.modified == advisory.modified
            && entry.severity == advisory.severity
            && entry.summary == advisory.summary;
        if !same {
            return Err(format!("the index's entry for {} is not what {path} says", advisory.id));
        }
    }
    if let Some(entry) = index
        .advisories
        .iter()
        .find(|entry| !files.iter().any(|(path, _)| *path == target_path(&entry.id)))
    {
        return Err(format!("the index lists {}, whose file is not signed", entry.id));
    }
    Ok(index.advisories)
}

/// Orders two ids by year and then number.
#[must_use]
pub fn compare_ids(a: &str, b: &str) -> Ordering {
    match (split_id(a), split_id(b)) {
        (Some(a), Some(b)) => a.cmp(&b),
        _ => a.cmp(b),
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// The example advisory the tests read: a flaw in the system, from
    /// 1.4.0 to 1.4.1.1, and in an app, which only its last version has.
    pub(crate) const FIXTURE: &str = include_str!("../tests/fixtures/DKSA-2026-0001.json");

    fn fixture() -> Value {
        serde_json::from_str(FIXTURE).unwrap()
    }

    /// The fixture with `change` made to it, parsed as a machine does.
    fn changed(change: impl FnOnce(&mut Value)) -> Result<Advisory, String> {
        let mut value = fixture();
        change(&mut value);
        Advisory::parse(&serde_json::to_vec(&value).unwrap())
    }

    /// The same, read as the tree's file `DKSA-2026-0001.json`.
    fn changed_source(change: impl FnOnce(&mut Value)) -> Result<Advisory, String> {
        let mut value = fixture();
        change(&mut value);
        Advisory::parse_source(&serde_json::to_vec(&value).unwrap(), "DKSA-2026-0001.json")
    }

    fn refused(result: Result<Advisory, String>, words: &str) {
        match result {
            Ok(_) => panic!("it was taken; it should be refused with {words:?}"),
            Err(why) => assert!(why.contains(words), "{why:?} does not say {words:?}"),
        }
    }

    fn range(events: &[Event]) -> Range {
        Range { events: events.to_vec() }
    }

    fn introduced(version: &str) -> Event {
        Event::Introduced(version.into())
    }

    fn fixed(version: &str) -> Event {
        Event::Fixed(version.into())
    }

    fn last(version: &str) -> Event {
        Event::LastAffected(version.into())
    }

    fn inside(range: &Range, version: &str) -> bool {
        range.contains(&Version::parse(version).expect("a version"))
    }

    #[test]
    fn the_example_advisory_reads_whole() {
        let advisory = Advisory::parse(FIXTURE.as_bytes()).unwrap();
        assert_eq!(advisory.id, "DKSA-2026-0001");
        assert_eq!(advisory.severity, Severity::High);
        assert_eq!(advisory.aliases, ["CVE-2026-12345", "GHSA-xxxx-xxxx-xxxx"]);
        assert_eq!(advisory.cvss.len(), 1);
        assert_eq!(advisory.references.len(), 2);
        assert_eq!(advisory.credits[0].kind.as_deref(), Some("REPORTER"));
        assert!(advisory.action.as_deref().unwrap().starts_with("Update"));
        assert_eq!(packages(&advisory), ["dukovo", "org.dukovo.ripgrep"]);
        // The tree's reading takes it too: it is the file's name, and says no TODO.
        Advisory::parse_source(FIXTURE.as_bytes(), "DKSA-2026-0001.json").unwrap();
    }

    #[test]
    fn a_range_is_evaluated_by_sorting_its_events_and_applying_each() {
        // From a version to the one that fixes it: the fix is not affected.
        let r = range(&[introduced("1.4.0"), fixed("1.4.2")]);
        for (version, affected) in [
            ("1.3.9", false),
            ("1.4.0", true),
            ("1.4.1", true),
            ("1.4.1.1", true),
            ("1.4.2", false),
            ("1.5.0", false),
        ] {
            assert_eq!(inside(&r, version), affected, "{version}");
        }
        // "0" is from the start, and a range with no end is open.
        let r = range(&[introduced("0")]);
        assert!(inside(&r, "0.0.1") && inside(&r, "99.0.0"));
        // last_affected is affected itself; the next version is not.
        let r = range(&[introduced("1.0.0"), last("1.2.0")]);
        assert!(inside(&r, "1.2.0") && !inside(&r, "1.2.0.1") && !inside(&r, "0.9.9"));
        // The events are sorted first, whatever order they are written in.
        let r = range(&[fixed("1.4.2"), introduced("1.4.0")]);
        assert!(inside(&r, "1.4.1") && !inside(&r, "1.4.2") && !inside(&r, "1.3.0"));
        // Several intervals in one range, and a fix a security build later.
        let r =
            range(&[introduced("1.0.0"), fixed("1.2.0"), introduced("1.3.0"), fixed("1.4.1.1")]);
        for (version, affected) in [
            ("1.1.0", true),
            ("1.2.0", false),
            ("1.2.9", false),
            ("1.3.0", true),
            ("1.4.1", true),
            ("1.4.1.1", false),
        ] {
            assert_eq!(inside(&r, version), affected, "{version}");
        }
    }

    #[test]
    fn versions_are_in_dukovos_order_and_a_pre_release_comes_before_its_release() {
        let r = range(&[introduced("1.4.1"), fixed("1.4.2")]);
        assert!(inside(&r, "1.4.1") && inside(&r, "1.4.1.1") && !inside(&r, "1.4.2"));
        // The release before the fix is still affected; the fix's own
        // pre-release is before it, and so affected.
        assert!(inside(&r, "1.4.2-rc.1"));
        let r = range(&[introduced("1.0.0-alpha.1"), fixed("1.0.0")]);
        assert!(inside(&r, "1.0.0-alpha.1") && inside(&r, "1.0.0-rc.1") && !inside(&r, "1.0.0"));
        assert!(!inside(&r, "1.0.0-alpha"));
        // Build metadata does not count, and 1.4.1.0 is 1.4.1.
        let r = range(&[introduced("1.4.1"), fixed("1.4.2")]);
        assert!(inside(&r, "1.4.1.0") && !inside(&r, "1.4.2+good"));
    }

    #[test]
    fn events_at_one_version_apply_the_start_first() {
        // A single version, from last_affected at the version introduced.
        let r = range(&[last("1.2.0"), introduced("1.2.0")]);
        assert!(inside(&r, "1.2.0") && !inside(&r, "1.2.0.1") && !inside(&r, "1.1.9"));
        // An interval fixed where it starts is empty.
        let r = range(&[fixed("1.2.0"), introduced("1.2.0")]);
        assert!(!inside(&r, "1.2.0") && !inside(&r, "1.1.0") && !inside(&r, "1.3.0"));
    }

    #[test]
    fn affects_reads_the_package_and_any_range_of_it() {
        let advisory = Advisory::parse(FIXTURE.as_bytes()).unwrap();
        // The system: 1.4.0 up to the fix 1.4.2, and the older 1.3 line
        // from 1.3.5 to its fix 1.3.9.1.
        for (version, affected) in [
            ("1.3.4", false),
            ("1.3.5", true),
            ("1.3.9", true),
            ("1.3.9.1", false),
            ("1.4.0", true),
            ("1.4.1.1", true),
            ("1.4.2", false),
            ("not a version", false),
            ("", false),
        ] {
            assert_eq!(affects(&advisory, SYSTEM, version), affected, "{version}");
        }
        // Another package is another entry; one the advisory does not
        // name is never affected.
        assert!(affects(&advisory, "org.dukovo.ripgrep", "14.1.0"));
        assert!(!affects(&advisory, "org.dukovo.ripgrep", "14.1.1"));
        assert!(!affects(&advisory, "org.dukovo.bat", "14.1.0"));
        // An app's version of one or two numbers is read with zeros added.
        assert!(affects(&advisory, "org.dukovo.ripgrep", "14.1"));
        assert!(!affects(&advisory, "org.dukovo.ripgrep", "14.2"));
        let short = changed(|v| {
            v["affected"][1]["ranges"][0]["events"] =
                serde_json::json!([{"introduced": "0"}, {"fixed": "669.0.0"}]);
        })
        .unwrap();
        assert!(
            affects(&short, "org.dukovo.ripgrep", "668")
                && !affects(&short, "org.dukovo.ripgrep", "669")
        );
        assert!(
            affects(&short, "org.dukovo.ripgrep", "668.5")
                && !affects(&short, "org.dukovo.ripgrep", "669.0")
        );
        assert!(!affects(&short, "org.dukovo.ripgrep", "x.y"));
    }

    #[test]
    fn the_fixed_versions_of_a_package_come_oldest_first() {
        let advisory = Advisory::parse(FIXTURE.as_bytes()).unwrap();
        assert_eq!(fixed_versions(&advisory, SYSTEM), ["1.3.9.1", "1.4.2"]);
        assert_eq!(fixed_versions(&advisory, "org.dukovo.ripgrep"), ["14.1.1"]);
        assert!(fixed_versions(&advisory, "org.dukovo.bat").is_empty());
        // The same fix twice, written two ways, counts once.
        let twice = changed(|v| {
            v["affected"].as_array_mut().unwrap().push(serde_json::json!({
                "package": {"ecosystem": "Dukovo", "name": "dukovo"},
                "ranges": [{"type": "ECOSYSTEM", "events": [{"introduced": "1.4.0"}, {"fixed": "1.4.2.0"}]}]
            }));
        })
        .unwrap();
        assert_eq!(fixed_versions(&twice, SYSTEM), ["1.3.9.1", "1.4.2"]);
        let only_last = changed(|v| {
            v["affected"][1]["ranges"][0]["events"] =
                serde_json::json!([{"introduced": "0"}, {"last_affected": "14.1.0"}]);
        })
        .unwrap();
        assert!(fixed_versions(&only_last, "org.dukovo.ripgrep").is_empty());
    }

    #[test]
    fn what_is_not_json_or_not_an_advisory_is_refused() {
        refused(Advisory::parse(b"{"), "it is not JSON");
        refused(Advisory::parse(b"[]"), "it is not a JSON object");
        refused(Advisory::parse(b"{}"), "schema_version is needed");
        let big = vec![b' '; ADVISORY_MAX as usize + 1];
        refused(Advisory::parse(&big), "at most 262144 are read");
    }

    #[test]
    fn each_required_field_is_needed_and_of_its_kind() {
        for field in [
            "schema_version",
            "id",
            "modified",
            "published",
            "summary",
            "details",
            "affected",
            "database_specific",
        ] {
            refused(
                changed(|v| drop(v.as_object_mut().unwrap().remove(field))),
                &format!("{field} is needed"),
            );
        }
        refused(
            changed(|v| drop(v["database_specific"].as_object_mut().unwrap().remove("severity"))),
            "database_specific.severity is needed",
        );
        refused(changed(|v| v["summary"] = 3.into()), "summary must be text");
        refused(changed(|v| v["affected"] = "all".into()), "affected must be a list");
        refused(
            changed(|v| v["database_specific"] = "high".into()),
            "database_specific must be an object",
        );
    }

    #[test]
    fn the_schema_version_is_osvs_1_6() {
        refused(changed(|v| v["schema_version"] = "1.5.0".into()), "schema_version is 1.5.0");
        refused(changed(|v| v["schema_version"] = "1.6".into()), "schema_version is 1.6");
    }

    #[test]
    fn an_id_is_dksa_a_year_and_a_number_and_in_a_file_of_its_name() {
        for id in [
            "DSA-2026-0001",
            "DKSA-26-0001",
            "DKSA-2026-1",
            "DKSA-2026-0000",
            "DKSA-0126-0001",
            "dksa-2026-0001",
            "DKSA-2026-0001 ",
        ] {
            refused(changed(|v| v["id"] = id.into()), "not DKSA-YYYY-NNNN");
        }
        assert_eq!(split_id("DKSA-2026-0042"), Some((2026, 42)));
        assert!(valid_id("DKSA-2031-9999") && !valid_id("DKSA-2031-99999"));
        // In the tree, the id is the file's name.
        refused(
            Advisory::parse_source(FIXTURE.as_bytes(), "DKSA-2026-0002.json"),
            "the id DKSA-2026-0001 is not the file's name",
        );
        assert_eq!(id_of_target("advisories/DKSA-2026-0001.json"), Some("DKSA-2026-0001"));
        assert_eq!(id_of_target("advisories/index.json"), None);
        assert_eq!(id_of_target("channels/stable.json"), None);
        assert_eq!(target_path("DKSA-2026-0001"), "advisories/DKSA-2026-0001.json");
    }

    #[test]
    fn times_are_rfc_3339_in_utc_and_modified_is_not_before_published() {
        for bad in [
            "2026-11-15",
            "2026-11-15T09:30:00",
            "2026-11-15T09:30:00+01:00",
            "2026-13-15T09:30:00Z",
            "yesterday",
        ] {
            refused(changed(|v| v["published"] = bad.into()), "not an RFC 3339 time in UTC");
            refused(changed(|v| v["modified"] = bad.into()), "not an RFC 3339 time in UTC");
        }
        refused(
            changed(|v| v["modified"] = "2026-01-01T00:00:00Z".into()),
            "modified is 2026-01-01T00:00:00Z, before published",
        );
        // A fraction of a second is RFC 3339's, and the same times are fine.
        changed(|v| v["modified"] = "2026-11-15T09:30:00.250Z".into()).unwrap();
        changed(|v| v["modified"] = v["published"].clone()).unwrap();
    }

    #[test]
    fn the_summary_is_one_line_and_the_details_say_something() {
        refused(changed(|v| v["summary"] = "".into()), "summary is empty");
        refused(changed(|v| v["summary"] = "two\nlines".into()), "summary must be one line");
        refused(changed(|v| v["summary"] = "x".repeat(121).into()), "summary is longer than 120");
        changed(|v| v["summary"] = "x".repeat(120).into()).unwrap();
        refused(changed(|v| v["details"] = "  ".into()), "details is empty");
    }

    #[test]
    fn the_severity_is_one_of_four_words() {
        for word in ["critical", "high", "medium", "low"] {
            let advisory = changed(|v| v["database_specific"]["severity"] = word.into()).unwrap();
            assert_eq!(advisory.severity.word(), word);
        }
        refused(
            changed(|v| v["database_specific"]["severity"] = "urgent".into()),
            "database_specific.severity is urgent: say critical, high, medium or low",
        );
        refused(
            changed(|v| v["database_specific"]["severity"] = "High".into()),
            "say critical, high, medium or low",
        );
        assert!(
            Severity::Critical > Severity::High
                && Severity::High > Severity::Medium
                && Severity::Medium > Severity::Low
        );
    }

    #[test]
    fn the_action_is_a_line_if_there_is_one() {
        let none =
            changed(|v| drop(v["database_specific"].as_object_mut().unwrap().remove("action")))
                .unwrap();
        assert_eq!(none.action, None);
        refused(
            changed(|v| v["database_specific"]["action"] = "a\nb".into()),
            "database_specific.action must be one line",
        );
        refused(
            changed(|v| v["database_specific"]["action"] = "x".repeat(301).into()),
            "longer than 300",
        );
        refused(
            changed(|v| v["database_specific"]["action"] = 4.into()),
            "database_specific.action must be text",
        );
    }

    #[test]
    fn aliases_are_ids_without_blanks() {
        refused(changed(|v| v["aliases"] = serde_json::json!(["CVE 2026"])), "aliases[0]");
        refused(changed(|v| v["aliases"] = serde_json::json!([""])), "an alias is an ID");
        refused(changed(|v| v["aliases"] = serde_json::json!([5])), "aliases[0] must be text");
        assert!(
            changed(|v| drop(v.as_object_mut().unwrap().remove("aliases")))
                .unwrap()
                .aliases
                .is_empty()
        );
    }

    #[test]
    fn cvss_vectors_are_of_their_kind() {
        refused(
            changed(|v| v["severity"][0]["type"] = "CVSS_V5".into()),
            "say CVSS_V2, CVSS_V3 or CVSS_V4",
        );
        refused(
            changed(|v| v["severity"][0]["score"] = "7.5".into()),
            "which is not a CVSS_V3 vector",
        );
        refused(
            changed(|v| drop(v["severity"][0].as_object_mut().unwrap().remove("score"))),
            "severity[0].score is needed",
        );
        changed(|v| {
            v["severity"] = serde_json::json!([
                {"type": "CVSS_V4", "score": "CVSS:4.0/AV:N/AC:L/AT:N/PR:N/UI:N/VC:H/VI:H/VA:H/SC:N/SI:N/SA:N"},
                {"type": "CVSS_V2", "score": "AV:N/AC:L/Au:N/C:P/I:P/A:P"}
            ]);
        })
        .unwrap();
    }

    #[test]
    fn references_and_credits_are_of_a_known_kind_and_references_are_https() {
        refused(
            changed(|v| v["references"][0]["type"] = "BLOG".into()),
            "references[0].type is BLOG",
        );
        refused(
            changed(|v| v["references"][0]["url"] = "http://example.org/x".into()),
            "not an https:// address",
        );
        refused(
            changed(|v| v["references"][0]["url"] = "https://exa mple.org".into()),
            "not an https:// address",
        );
        refused(changed(|v| v["credits"][0]["name"] = " ".into()), "credits[0].name is empty");
        refused(changed(|v| v["credits"][0]["type"] = "BOSS".into()), "credits[0].type is BOSS");
        let plain =
            changed(|v| v["credits"] = serde_json::json!([{"name": "A reporter"}])).unwrap();
        assert_eq!(plain.credits[0].kind, None);
    }

    #[test]
    fn affected_needs_dukovos_packages_with_ranges_of_the_ecosystem_type() {
        refused(changed(|v| v["affected"] = serde_json::json!([])), "affected is empty");
        refused(
            changed(|v| v["affected"][0]["package"]["ecosystem"] = "Debian".into()),
            "affected[0].package.ecosystem is Debian, not Dukovo",
        );
        refused(
            changed(|v| v["affected"][0]["package"]["name"] = "".into()),
            "affected[0].package.name is",
        );
        refused(
            changed(|v| v["affected"][0]["package"]["name"] = "a b".into()),
            "it is dukovo, or an app's ID",
        );
        refused(
            changed(|v| drop(v["affected"][0].as_object_mut().unwrap().remove("package"))),
            "affected[0].package is needed",
        );
        refused(
            changed(|v| v["affected"][0]["ranges"] = serde_json::json!([])),
            "affected[0].ranges is empty",
        );
        refused(
            changed(|v| drop(v["affected"][0].as_object_mut().unwrap().remove("ranges"))),
            "affected[0].ranges is needed",
        );
        refused(
            changed(|v| v["affected"][0]["ranges"][0]["type"] = "SEMVER".into()),
            "ECOSYSTEM ranges",
        );
        refused(
            changed(|v| v["affected"][0]["ranges"][0]["type"] = "GIT".into()),
            "affected[0].ranges[0].type is GIT",
        );
        refused(
            changed(|v| v["affected"][0]["versions"] = serde_json::json!(["1.4.0"])),
            "affected[0].versions is not used",
        );
    }

    #[test]
    fn events_are_one_of_three_with_a_version() {
        let events = |value: serde_json::Value| {
            move |v: &mut Value| v["affected"][0]["ranges"][0]["events"] = value
        };
        refused(changed(events(serde_json::json!([]))), "affected[0].ranges[0].events is empty");
        refused(
            changed(events(serde_json::json!([{"fixed": "1.4.2", "introduced": "0"}]))),
            "has one of introduced, fixed or last_affected, and no more",
        );
        refused(
            changed(events(serde_json::json!([{}]))),
            "has one of introduced, fixed or last_affected",
        );
        refused(changed(events(serde_json::json!([{"limit": "2.0.0"}]))), "has limit");
        refused(changed(events(serde_json::json!(["0"]))), "events[0] must be an object");
        refused(changed(events(serde_json::json!([{"introduced": 0}]))), "introduced must be text");
        // Versions are Dukovo's: three or four numbers; 0 only starts.
        refused(
            changed(events(serde_json::json!([{"introduced": "1.4"}]))),
            "introduced is 1.4, not a version",
        );
        refused(
            changed(events(serde_json::json!([{"introduced": "0"}, {"fixed": "0"}]))),
            "fixed is 0, not a version",
        );
        refused(
            changed(events(serde_json::json!([{"introduced": "0"}, {"fixed": "v1.4.2"}]))),
            "fixed is v1.4.2",
        );
        refused(
            changed(events(serde_json::json!([{"introduced": "0"}, {"last_affected": "1.4.01"}]))),
            "last_affected is 1.4.01",
        );
    }

    #[test]
    fn the_events_of_a_range_open_it_and_close_it_in_turn() {
        let events = |value: serde_json::Value| {
            move |v: &mut Value| v["affected"][0]["ranges"][0]["events"] = value
        };
        refused(
            changed(events(serde_json::json!([{"fixed": "1.4.2"}]))),
            "has no introduced event",
        );
        refused(
            changed(events(serde_json::json!([{"introduced": "1.0.0"}, {"introduced": "1.1.0"}]))),
            "introduced 1.1.0 comes while the range is open since 1.0.0",
        );
        refused(
            changed(events(
                serde_json::json!([{"introduced": "1.0.0"}, {"fixed": "1.1.0"}, {"fixed": "1.2.0"}]),
            )),
            "fixed 1.2.0 ends nothing",
        );
        refused(
            changed(events(
                serde_json::json!([{"introduced": "1.0.0"}, {"fixed": "1.1.0"}, {"last_affected": "1.2.0"}]),
            )),
            "last_affected 1.2.0 ends nothing",
        );
        refused(
            changed(events(serde_json::json!([{"introduced": "1.2.0"}, {"fixed": "1.2.0"}]))),
            "fixed 1.2.0 is the version introduced 1.2.0",
        );
        // These are fine: one version, and a last range left open.
        changed(events(serde_json::json!([{"introduced": "1.2.0"}, {"last_affected": "1.2.0"}])))
            .unwrap();
        changed(events(
            serde_json::json!([{"introduced": "0"}, {"fixed": "1.0.0"}, {"introduced": "2.0.0"}]),
        ))
        .unwrap();
        // Written out of order, they are read in order.
        changed(events(serde_json::json!([{"fixed": "1.4.2"}, {"introduced": "1.4.0"}]))).unwrap();
    }

    #[test]
    fn the_trees_file_has_no_field_dukovo_does_not_use_and_no_todo() {
        refused(
            changed_source(|v| v["withdrawn"] = "2026-12-01T00:00:00Z".into()),
            "withdrawn is not a field of Dukovo's advisories",
        );
        refused(
            changed_source(|v| v["alias"] = serde_json::json!(["CVE-1"])),
            "alias is not a field",
        );
        refused(
            changed_source(|v| v["database_specific"]["why"] = "x".into()),
            "database_specific.why is not a field",
        );
        refused(
            changed_source(|v| v["affected"][0]["ecosystem_specific"] = serde_json::json!({})),
            "affected[0].ecosystem_specific is not a field",
        );
        refused(
            changed_source(|v| v["affected"][0]["ranges"][0]["repo"] = "x".into()),
            "affected[0].ranges[0].repo is not a field",
        );
        refused(
            changed_source(|v| v["affected"][0]["package"]["purl"] = "pkg:x".into()),
            "affected[0].package.purl is not a field",
        );
        // A machine reading one signed later with a field it does not know leaves it.
        changed(|v| v["related"] = serde_json::json!(["CVE-1"])).unwrap();
        for (field, change) in [
            (
                "summary",
                (|v: &mut Value| v["summary"] = "TODO: what is wrong".into()) as fn(&mut Value),
            ),
            ("details", |v: &mut Value| v["details"] = "Before. TODO: more.".into()),
            ("database_specific.action", |v: &mut Value| {
                v["database_specific"]["action"] = "TODO".into();
            }),
        ] {
            refused(changed_source(change), &format!("{field} still says TODO: write it"));
            // A machine does not look.
            let mut value = fixture();
            change(&mut value);
            Advisory::parse(&serde_json::to_vec(&value).unwrap()).unwrap();
        }
    }

    fn advisory_with_id(id: &str, modified: &str, severity: &str) -> Vec<u8> {
        let mut value = fixture();
        value["id"] = id.into();
        value["modified"] = modified.into();
        value["database_specific"]["severity"] = severity.into();
        serde_json::to_vec_pretty(&value).unwrap()
    }

    fn scratch(name: &str) -> std::path::PathBuf {
        crate::list::tests::scratch(name)
    }

    /// The folder `dir` holding exactly the advisories of `ids`, each the
    /// fixture with that id, time of change and severity, read as the
    /// tree's are.
    pub(crate) fn folder(dir: &Path, advisories: &[(&str, &str, &str)]) -> Vec<Source> {
        std::fs::create_dir_all(dir).unwrap();
        for entry in std::fs::read_dir(dir).unwrap() {
            std::fs::remove_file(entry.unwrap().path()).unwrap();
        }
        for (id, modified, severity) in advisories {
            let file = dir.join(format!("{id}.json"));
            std::fs::write(file, advisory_with_id(id, modified, severity)).unwrap();
        }
        load_dir(dir).unwrap()
    }

    #[test]
    fn a_folder_of_advisories_is_read_whole_and_each_problem_is_named() {
        let dir = scratch("advisory-dir");
        std::fs::write(dir.join("README.md"), "about").unwrap();
        std::fs::write(
            dir.join("DKSA-2026-0002.json"),
            advisory_with_id("DKSA-2026-0002", "2026-11-20T00:00:00Z", "low"),
        )
        .unwrap();
        std::fs::write(dir.join("DKSA-2026-0001.json"), FIXTURE).unwrap();
        let sources = load_dir(&dir).unwrap();
        assert_eq!(
            sources.iter().map(|s| s.file.as_str()).collect::<Vec<_>>(),
            ["DKSA-2026-0001.json", "DKSA-2026-0002.json"]
        );
        assert_eq!(sources[0].bytes, FIXTURE.as_bytes());
        assert_eq!(sources[0].sha256.len(), 64);

        // The id is the file's name; another .json is not an advisory's name.
        std::fs::write(
            dir.join("DKSA-2026-0003.json"),
            advisory_with_id("DKSA-2026-0004", "2026-11-20T00:00:00Z", "low"),
        )
        .unwrap();
        std::fs::write(dir.join("notes.json"), "{}").unwrap();
        std::fs::write(dir.join("DKSA-2026-0005.json"), "{").unwrap();
        let problems = load_dir(&dir).unwrap_err();
        assert_eq!(problems.len(), 3, "{problems:?}");
        assert!(problems.iter().any(|p| {
            p.starts_with("DKSA-2026-0003.json: the id DKSA-2026-0004 is not the file's name")
        }));
        assert!(
            problems
                .iter()
                .any(|p| p == "notes.json: an advisory's file is named DKSA-YYYY-NNNN.json")
        );
        assert!(problems.iter().any(|p| p.starts_with("DKSA-2026-0005.json: it is not JSON")));
        assert!(load_dir(&dir.join("nowhere")).is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn two_advisories_of_one_id_are_refused() {
        let source = |file: &str| {
            let bytes = FIXTURE.as_bytes().to_vec();
            Source {
                file: file.into(),
                sha256: sha256(&bytes),
                advisory: Advisory::parse(&bytes).unwrap(),
                bytes,
            }
        };
        let problems = duplicates(&[source("DKSA-2026-0001.json"), source("copy.json")]);
        assert_eq!(problems.len(), 1);
        assert!(
            problems[0]
                .starts_with("copy.json: DKSA-2026-0001 is also the id of DKSA-2026-0001.json"),
            "{problems:?}"
        );
        assert!(duplicates(&[source("a.json")]).is_empty());
    }

    fn sources() -> Vec<Source> {
        let dir = scratch("advisory-index");
        std::fs::write(
            dir.join("DKSA-2026-0002.json"),
            advisory_with_id("DKSA-2026-0002", "2026-11-20T00:00:00Z", "critical"),
        )
        .unwrap();
        std::fs::write(dir.join("DKSA-2026-0001.json"), FIXTURE).unwrap();
        let sources = load_dir(&dir).unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
        sources
    }

    #[test]
    fn the_index_lists_each_advisory_by_id_and_reads_back() {
        let sources = sources();
        let index = Index::of(&sources);
        assert_eq!(index.advisories[0].id, "DKSA-2026-0001");
        assert_eq!(index.advisories[0].severity, Severity::High);
        assert_eq!(index.advisories[1].severity, Severity::Critical);
        assert_eq!(index.advisories[1].modified, "2026-11-20T00:00:00Z");
        assert_eq!(index.advisories[0].sha256, sources[0].sha256);
        let bytes = index.to_bytes();
        assert!(bytes.ends_with(b"\n"));
        let json: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(json["advisories"][0]["severity"], "high");
        assert_eq!(Index::parse(&bytes).unwrap(), index);
        // The same advisories give the same bytes.
        let mut reversed = sources;
        reversed.reverse();
        assert_eq!(Index::of(&reversed).to_bytes(), bytes);
        assert_eq!(Index::of(&[]).to_bytes(), b"{\n  \"advisories\": []\n}\n");
    }

    #[test]
    fn a_bad_index_is_refused() {
        let good = Index::of(&sources());
        let entry = &good.advisories[0];
        let with = |change: &dyn Fn(&mut Entry)| {
            let mut index = good.clone();
            change(&mut index.advisories[0]);
            Index::parse(&index.to_bytes())
        };
        assert!(Index::parse(b"{").unwrap_err().contains("malformed"));
        assert!(
            Index::parse(b"{\"advisories\": [{\"id\": \"x\"}]}").unwrap_err().contains("malformed")
        );
        assert!(with(&|e| e.id = "DSA-1".into()).unwrap_err().contains("not a DKSA id"));
        assert!(with(&|e| e.sha256 = "abcd".into()).unwrap_err().contains("no SHA-256"));
        assert!(with(&|e| e.sha256 = e.sha256.to_uppercase()).unwrap_err().contains("no SHA-256"));
        let mut twice = good.clone();
        twice.advisories.push(entry.clone());
        assert!(
            Index::parse(&twice.to_bytes()).unwrap_err().contains("lists DKSA-2026-0001 twice")
        );
        let big = vec![b' '; INDEX_MAX as usize + 1];
        assert!(Index::parse(&big).unwrap_err().contains("at most 1048576 are read"));
    }

    fn served(sources: &[Source], index: &Index) -> Vec<(String, Vec<u8>)> {
        let mut targets = vec![(INDEX_TARGET.to_string(), index.to_bytes())];
        targets.extend(sources.iter().map(|s| (target_path(&s.advisory.id), s.bytes.clone())));
        targets.push(("channels/stable.json".into(), b"{}".to_vec()));
        targets
    }

    #[test]
    fn what_is_served_is_checked_against_the_index() {
        let sources = sources();
        let index = Index::of(&sources);
        assert_eq!(check_served(&served(&sources, &index)).unwrap(), index.advisories);
        assert!(check_served(&[("channels/stable.json".into(), vec![])]).unwrap().is_empty());
        assert!(check_served(&[]).unwrap().is_empty());
        // An empty index with no advisory is as good as none.
        assert!(check_served(&served(&[], &Index::default())).unwrap().is_empty());
        // No index.
        let mut no_index = served(&sources, &index);
        no_index.remove(0);
        assert!(check_served(&no_index).unwrap_err().contains("advisories/index.json is not"));
        // A file the index does not list, and one it lists with none.
        assert!(
            check_served(&served(&sources, &Index::of(&sources[..1])))
                .unwrap_err()
                .contains("the index does not list it")
        );
        assert!(
            check_served(&served(&sources[..1], &index))
                .unwrap_err()
                .contains("whose file is not signed")
        );
        // A file that is not the one the index hashed, and one of another id.
        let mut changed = served(&sources, &index);
        changed[1].1 = advisory_with_id("DKSA-2026-0001", "2026-11-16T00:00:00Z", "high");
        assert!(
            check_served(&changed)
                .unwrap_err()
                .contains("is not what advisories/DKSA-2026-0001.json says")
        );
        let mut moved = served(&sources, &index);
        moved[1].0 = "advisories/DKSA-2026-0009.json".into();
        assert!(check_served(&moved).unwrap_err().contains("holds the advisory DKSA-2026-0001"));
        let mut bad = served(&sources, &index);
        bad[1].1 = b"{}".to_vec();
        assert!(
            check_served(&bad)
                .unwrap_err()
                .contains("advisories/DKSA-2026-0001.json: schema_version is needed")
        );
    }

    #[test]
    fn ids_compare_by_year_and_number() {
        assert_eq!(compare_ids("DKSA-2026-0010", "DKSA-2026-0009"), Ordering::Greater);
        assert_eq!(compare_ids("DKSA-2026-0010", "DKSA-2027-0001"), Ordering::Less);
        assert_eq!(compare_ids("DKSA-2026-0001", "DKSA-2026-0001"), Ordering::Equal);
    }
}
