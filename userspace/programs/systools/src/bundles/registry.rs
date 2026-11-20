// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! What bundled(8) keeps in `/system/state/bundled` (ADR 0223), which only
//! it writes: the bundles it installed, the key each ID is bound to, each
//! person's developer key, the catalog each ID installed from one is
//! bound to (ADR 0383), and the version each new one replaced, the IDs
//! held at their version, and the places that update by themselves (ADR
//! 0384), the line of each ID that runs unless a project asks for
//! another (ADR 0387), and the versions catalogs and feeds withdrew (ADR
//! 0593). Each is a file of lines, tab-separated, read strictly: a line
//! that does not read is an error, never skipped.
//!
//! A bundle with a line is kept by its full ID, `ID@LINE`; the key and
//! catalog an ID is bound to bind all its lines, so bindings are kept by
//! the ID alone.

use std::fmt;

use super::catalogs::Kind;
use super::minisign::PublicKey;
use super::rollover::{self, Link};

/// Where bundled keeps them.
pub const STATE: &str = "/system/state/bundled";
/// The bundles it installed.
pub const INSTALLED: &str = "installed";
/// The key each ID is bound to, in each place.
pub const BOUND: &str = "bound";
/// Each person's developer key.
pub const DEVELOPER_KEYS: &str = "developer-keys";
/// The catalog each ID is bound to, in each place.
pub const CATALOG_BOUND: &str = "catalog-bound";
/// The version each bundle replaced, kept to go back to.
pub const PREVIOUS: &str = "previous";
/// The IDs held at their version, in each place.
pub const HELD: &str = "held";
/// The places whose bundles update by themselves.
pub const AUTOMATIC: &str = "automatic";
/// The default line of each ID with lines, in each place.
pub const DEFAULTS: &str = "defaults";
/// The versions catalogs and feeds withdrew, kept for good.
pub const WITHDRAWN: &str = "withdrawn";
/// The files, in the order [`Registry::read`] takes them.
pub const FILES: [&str; 9] = [
    INSTALLED,
    BOUND,
    DEVELOPER_KEYS,
    CATALOG_BOUND,
    PREVIOUS,
    HELD,
    AUTOMATIC,
    DEFAULTS,
    WITHDRAWN,
];
/// The most lines [`WITHDRAWN`] keeps, unless more are of versions
/// installed or kept.
pub const WITHDRAWN_MAX: usize = 4096;

/// The ID of the full ID `id`: what is before its `@`, if it has a line.
#[must_use]
pub fn base_id(id: &str) -> &str {
    id.split_once('@').map_or(id, |(base, _)| base)
}

/// Where a bundle is installed: for everyone, or for one person.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Place {
    /// `/applications`, for everyone.
    Everyone,
    /// The `applications` folder of the home of the user with this number.
    User(u32),
}

impl fmt::Display for Place {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Place::Everyone => f.write_str("everyone"),
            Place::User(uid) => write!(f, "{uid}"),
        }
    }
}

impl Place {
    pub fn parse(text: &str) -> Result<Place, String> {
        if text == "everyone" {
            return Ok(Place::Everyone);
        }
        number(text).map(Place::User)
    }
}

/// A number as the files write it: decimal, no sign, no leading zero.
fn number<T: std::str::FromStr>(text: &str) -> Result<T, String> {
    let plain = !text.is_empty()
        && text.bytes().all(|b| b.is_ascii_digit())
        && (text == "0" || !text.starts_with('0'));
    plain.then(|| text.parse().ok()).flatten().ok_or_else(|| format!("{text:?} is not a number"))
}

/// A field: printable, without tabs or newlines.
fn field(text: &str) -> Result<&str, String> {
    if text.is_empty() || text.chars().any(char::is_control) {
        return Err(format!("{text:?} is not a field"));
    }
    Ok(text)
}

/// A bundle bundled installed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Installed {
    pub place: Place,
    pub id: String,
    pub version: String,
    pub key: PublicKey,
    /// The SHA-256 of its signed list, `files.sha256`, in hex.
    pub list: String,
    /// When, in seconds since 1970.
    pub when: u64,
}

/// An ID bound to a key in a place, from its first install there.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Bound {
    pub place: Place,
    pub id: String,
    pub key: PublicKey,
    /// Since when, in seconds since 1970.
    pub since: u64,
}

/// An ID bound to a catalog in a place, from its first install from it
/// there: from then on, only a file that catalog lists installs it, as
/// whatever key signed it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CatalogBound {
    pub place: Place,
    pub id: String,
    pub catalog: String,
    /// Since when, in seconds since 1970.
    pub since: u64,
}

/// An ID held at its version in a place: updates leave it as it is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Held {
    pub place: Place,
    pub id: String,
    /// Since when, in seconds since 1970.
    pub since: u64,
}

/// The line of an ID that runs in a place unless a project asks for
/// another.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LineDefault {
    pub place: Place,
    pub id: String,
    pub line: String,
}

/// Who withdrew a version.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Source {
    /// A catalog, by its name.
    Catalog(String),
    /// The feed of a bundle installed in this place (ADR 0390), which
    /// withdraws only versions of that bundle's own ID.
    Feed(Place),
}

impl fmt::Display for Source {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Source::Catalog(name) => write!(f, "catalog:{name}"),
            Source::Feed(place) => write!(f, "feed:{place}"),
        }
    }
}

impl Source {
    fn parse(text: &str) -> Result<Source, String> {
        match text.split_once(':') {
            Some(("catalog", name)) => {
                super::catalogs::check_name(name)?;
                Ok(Source::Catalog(name.into()))
            }
            Some(("feed", place)) => Ok(Source::Feed(Place::parse(place)?)),
            _ => Err(format!("{text:?} is neither a catalog nor a feed")),
        }
    }

    /// Who it is, in words.
    #[must_use]
    pub fn words(&self) -> String {
        match self {
            Source::Catalog(name) => format!("catalog {name}"),
            Source::Feed(_) => "its publisher's feed".into(),
        }
    }
}

/// A version a catalog or feed withdrew (ADR 0593): bundled installs it
/// nowhere and goes back from it, and stops it at once if it is
/// malicious.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Withdrawn {
    pub source: Source,
    /// The full ID of the bundle.
    pub id: String,
    pub version: String,
    /// The SHA-256 of the `.dkv` file the source listed, in hex.
    pub sha256: String,
    pub kind: Kind,
    /// Since when, as the source says, in seconds since 1970.
    pub since: u64,
    /// Why, as the source says: one line.
    pub reason: String,
}

impl Withdrawn {
    /// Whether it may be kept: each field as [`WITHDRAWN`] reads it back.
    fn check(&self) -> Result<(), String> {
        if let Source::Catalog(name) = &self.source {
            super::catalogs::check_name(name)?;
        }
        checked_full_id(&self.id)?;
        field(&self.version)?;
        checked_hex(&self.sha256)?;
        field(&self.reason)?;
        Ok(())
    }

    /// Its key: a line of the same, of a kind as bad, adds nothing.
    fn same(&self, other: &Withdrawn) -> bool {
        self.source == other.source
            && self.id == other.id
            && self.version == other.version
            && self.sha256 == other.sha256
    }
}

/// What bundled keeps.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Registry {
    pub installed: Vec<Installed>,
    pub bound: Vec<Bound>,
    /// Each person's developer key, by user number.
    pub developer_keys: Vec<(u32, PublicKey)>,
    pub catalog_bound: Vec<CatalogBound>,
    /// The version each installed bundle replaced, if it is kept.
    pub previous: Vec<Installed>,
    pub held: Vec<Held>,
    /// The places that update by themselves, and since when.
    pub automatic: Vec<(Place, u64)>,
    pub defaults: Vec<LineDefault>,
    pub withdrawn: Vec<Withdrawn>,
}

/// Splits `line` into exactly `N` tab-separated fields.
fn fields<const N: usize>(line: &str) -> Result<[&str; N], String> {
    let parts: Vec<&str> = line.split('\t').collect();
    let parts: [&str; N] = parts
        .try_into()
        .map_err(|parts: Vec<&str>| format!("{} fields where {N} belong", parts.len()))?;
    for part in parts {
        field(part)?;
    }
    Ok(parts)
}

/// Reads each line of `text`, a file's, with `read`; says which line is
/// wrong.
fn lines<T>(
    name: &str,
    text: &str,
    read: impl Fn(&str) -> Result<T, String>,
) -> Result<Vec<T>, String> {
    if !text.is_empty() && !text.ends_with('\n') {
        return Err(format!("{name}: its last line has no end"));
    }
    text.lines()
        .enumerate()
        .map(|(n, line)| read(line).map_err(|why| format!("{name}:{}: {why}", n + 1)))
        .collect()
}

fn checked_id(id: &str) -> Result<String, String> {
    manifest::bundle::check_id(id)?;
    Ok(id.into())
}

fn checked_full_id(id: &str) -> Result<String, String> {
    manifest::bundle::check_full_id(id)?;
    Ok(id.into())
}

fn checked_hex(text: &str) -> Result<String, String> {
    let hex = text.len() == 64 && text.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'));
    hex.then(|| text.into()).ok_or_else(|| format!("{text:?} is not a SHA-256"))
}

/// A line of [`INSTALLED`] or [`PREVIOUS`].
fn read_installed(line: &str) -> Result<Installed, String> {
    let [place, id, version, key, list, when] = fields(line)?;
    Ok(Installed {
        place: Place::parse(place)?,
        id: checked_full_id(id)?,
        version: version.into(),
        key: PublicKey::decode(key)?,
        list: checked_hex(list)?,
        when: number(when)?,
    })
}

/// The bundles the text of [`INSTALLED`] lists.
///
/// # Errors
///
/// If a line cannot be read.
pub fn installed(text: &str) -> Result<Vec<Installed>, String> {
    lines(INSTALLED, text, read_installed)
}

/// The lines of [`INSTALLED`] or [`PREVIOUS`], sorted.
fn installed_text(all: &[Installed]) -> String {
    let mut lines: Vec<String> = all
        .iter()
        .map(|i| {
            let key = i.key.encoded();
            format!("{}\t{}\t{}\t{key}\t{}\t{}\n", i.place, i.id, i.version, i.list, i.when)
        })
        .collect();
    lines.sort();
    lines.concat()
}

impl Registry {
    /// Reads the files' texts, in the order of [`FILES`].
    pub fn read(texts: &[String; 9]) -> Result<Registry, String> {
        let [
            installed,
            bound,
            developer_keys,
            catalog_bound,
            previous,
            held,
            automatic,
            defaults,
            withdrawn,
        ] = texts;
        let installed = lines(INSTALLED, installed, read_installed)?;
        let previous = lines(PREVIOUS, previous, read_installed)?;
        let bound = lines(BOUND, bound, |line| {
            let [place, id, key, since] = fields(line)?;
            Ok(Bound {
                place: Place::parse(place)?,
                id: checked_id(id)?,
                key: PublicKey::decode(key)?,
                since: number(since)?,
            })
        })?;
        let developer_keys = lines(DEVELOPER_KEYS, developer_keys, |line| {
            let [user, key] = fields(line)?;
            Ok((number(user)?, PublicKey::decode(key)?))
        })?;
        let catalog_bound = lines(CATALOG_BOUND, catalog_bound, |line| {
            let [place, id, catalog, since] = fields(line)?;
            super::catalogs::check_name(catalog)?;
            Ok(CatalogBound {
                place: Place::parse(place)?,
                id: checked_id(id)?,
                catalog: catalog.into(),
                since: number(since)?,
            })
        })?;
        let held = lines(HELD, held, |line| {
            let [place, id, since] = fields(line)?;
            Ok(Held {
                place: Place::parse(place)?,
                id: checked_full_id(id)?,
                since: number(since)?,
            })
        })?;
        let automatic = lines(AUTOMATIC, automatic, |line| {
            let [place, since] = fields(line)?;
            Ok((Place::parse(place)?, number(since)?))
        })?;
        let defaults = lines(DEFAULTS, defaults, |line| {
            let [place, id, line] = fields(line)?;
            manifest::bundle::check_version_line(line)?;
            Ok(LineDefault { place: Place::parse(place)?, id: checked_id(id)?, line: line.into() })
        })?;
        let withdrawn = lines(WITHDRAWN, withdrawn, |line| {
            let [source, id, version, sha256, kind, since, reason] = fields(line)?;
            Ok(Withdrawn {
                source: Source::parse(source)?,
                id: checked_full_id(id)?,
                version: version.into(),
                sha256: checked_hex(sha256)?,
                kind: Kind::read(kind)?,
                since: number(since)?,
                reason: reason.into(),
            })
        })?;
        let registry = Registry {
            installed,
            bound,
            developer_keys,
            catalog_bound,
            previous,
            held,
            automatic,
            defaults,
            withdrawn,
        };
        registry.check()?;
        Ok(registry)
    }

    /// Each kind at most once: a bundle per place and ID, a binding per
    /// place and ID, a developer key per person.
    fn check(&self) -> Result<(), String> {
        let mut seen = std::collections::BTreeSet::new();
        for i in &self.installed {
            if !seen.insert((0, i.place, i.id.as_str())) {
                return Err(format!("{INSTALLED}: {} is there twice for {}", i.id, i.place));
            }
        }
        for b in &self.bound {
            if !seen.insert((1, b.place, b.id.as_str())) {
                return Err(format!("{BOUND}: {} is there twice for {}", b.id, b.place));
            }
        }
        for (user, _) in &self.developer_keys {
            if !seen.insert((2, Place::User(*user), "")) {
                return Err(format!("{DEVELOPER_KEYS}: user {user} is there twice"));
            }
        }
        for b in &self.catalog_bound {
            if !seen.insert((3, b.place, b.id.as_str())) {
                return Err(format!("{CATALOG_BOUND}: {} is there twice for {}", b.id, b.place));
            }
        }
        for p in &self.previous {
            if !seen.insert((4, p.place, p.id.as_str())) {
                return Err(format!("{PREVIOUS}: {} is there twice for {}", p.id, p.place));
            }
        }
        for h in &self.held {
            if !seen.insert((5, h.place, h.id.as_str())) {
                return Err(format!("{HELD}: {} is there twice for {}", h.id, h.place));
            }
        }
        for (place, _) in &self.automatic {
            if !seen.insert((6, *place, "")) {
                return Err(format!("{AUTOMATIC}: {place} is there twice"));
            }
        }
        for d in &self.defaults {
            if !seen.insert((7, d.place, d.id.as_str())) {
                return Err(format!("{DEFAULTS}: {} is there twice for {}", d.id, d.place));
            }
        }
        for (n, w) in self.withdrawn.iter().enumerate() {
            if self.withdrawn[..n].iter().any(|before| before.same(w) && before.kind == w.kind) {
                return Err(format!("{WITHDRAWN}: {} {} is there twice", w.id, w.version));
            }
        }
        Ok(())
    }

    /// The files' texts, in the order of [`FILES`], sorted.
    pub fn texts(&self) -> [String; 9] {
        let mut bound: Vec<String> = self
            .bound
            .iter()
            .map(|b| format!("{}\t{}\t{}\t{}\n", b.place, b.id, b.key.encoded(), b.since))
            .collect();
        bound.sort();
        let mut keys: Vec<String> = self
            .developer_keys
            .iter()
            .map(|(user, key)| format!("{user}\t{}\n", key.encoded()))
            .collect();
        keys.sort();
        let mut catalogs: Vec<String> = self
            .catalog_bound
            .iter()
            .map(|b| format!("{}\t{}\t{}\t{}\n", b.place, b.id, b.catalog, b.since))
            .collect();
        catalogs.sort();
        let mut held: Vec<String> =
            self.held.iter().map(|h| format!("{}\t{}\t{}\n", h.place, h.id, h.since)).collect();
        held.sort();
        let mut automatic: Vec<String> =
            self.automatic.iter().map(|(place, since)| format!("{place}\t{since}\n")).collect();
        automatic.sort();
        let mut defaults: Vec<String> =
            self.defaults.iter().map(|d| format!("{}\t{}\t{}\n", d.place, d.id, d.line)).collect();
        defaults.sort();
        let mut withdrawn: Vec<String> = self
            .withdrawn
            .iter()
            .map(|w| {
                format!(
                    "{}\t{}\t{}\t{}\t{}\t{}\t{}\n",
                    w.source,
                    w.id,
                    w.version,
                    w.sha256,
                    w.kind.word(),
                    w.since,
                    w.reason
                )
            })
            .collect();
        withdrawn.sort();
        [
            installed_text(&self.installed),
            bound.concat(),
            keys.concat(),
            catalogs.concat(),
            installed_text(&self.previous),
            held.concat(),
            automatic.concat(),
            defaults.concat(),
            withdrawn.concat(),
        ]
    }

    /// The bundle `id` installed in `place`.
    pub fn installed(&self, place: Place, id: &str) -> Option<&Installed> {
        self.installed.iter().find(|i| i.place == place && i.id == id)
    }

    /// The key `id`, or the ID of its line, is bound to in `place`.
    pub fn bound(&self, place: Place, id: &str) -> Option<&Bound> {
        self.bound.iter().find(|b| b.place == place && b.id == base_id(id))
    }

    /// The catalog `id`, or the ID of its line, is bound to in `place`.
    pub fn catalog_bound(&self, place: Place, id: &str) -> Option<&CatalogBound> {
        self.catalog_bound.iter().find(|b| b.place == place && b.id == base_id(id))
    }

    /// The default line of the ID `id` in `place`, if one is set.
    pub fn default_line(&self, place: Place, id: &str) -> Option<&str> {
        self.defaults.iter().find(|d| d.place == place && d.id == id).map(|d| d.line.as_str())
    }

    /// Sets the default line of the ID `id` in `place`, or forgets it.
    pub fn set_default_line(&mut self, place: Place, id: &str, line: Option<&str>) {
        self.defaults.retain(|d| d.place != place || d.id != id);
        if let Some(line) = line {
            self.defaults.push(LineDefault { place, id: id.into(), line: line.into() });
        }
    }

    /// The developer key of the user `user`.
    pub fn developer_key(&self, user: u32) -> Option<PublicKey> {
        self.developer_keys.iter().find(|(u, _)| *u == user).map(|(_, key)| *key)
    }

    /// The version the bundle `id` in `place` replaced, if it is kept.
    pub fn previous(&self, place: Place, id: &str) -> Option<&Installed> {
        self.previous.iter().find(|i| i.place == place && i.id == id)
    }

    /// Whether `id` is held in `place`, and since when.
    pub fn held(&self, place: Place, id: &str) -> Option<&Held> {
        self.held.iter().find(|h| h.place == place && h.id == id)
    }

    /// Holds `id` in `place` from `since`, or lets it go; whether that
    /// changed anything.
    pub fn set_held(&mut self, place: Place, id: &str, on: bool, since: u64) -> bool {
        let was = self.held(place, id).is_some();
        if was != on {
            self.held.retain(|h| h.place != place || h.id != id);
            if on {
                self.held.push(Held { place, id: id.into(), since });
            }
        }
        was != on
    }

    /// Since when the bundles of `place` update by themselves, if they do.
    pub fn automatic(&self, place: Place) -> Option<u64> {
        self.automatic.iter().find(|(p, _)| *p == place).map(|(_, since)| *since)
    }

    /// Turns updating by themselves on or off for `place`; whether that
    /// changed anything.
    pub fn set_automatic(&mut self, place: Place, on: bool, since: u64) -> bool {
        let was = self.automatic(place).is_some();
        if was != on {
            self.automatic.retain(|(p, _)| *p != place);
            if on {
                self.automatic.push((place, since));
            }
        }
        was != on
    }

    /// Goes back to the version `id` replaced in `place`, which then is the
    /// one kept: the version now installed, and the one it replaced.
    pub fn roll_back(&mut self, place: Place, id: &str) -> Option<(Installed, Installed)> {
        let at = self.previous.iter().position(|i| i.place == place && i.id == id)?;
        let now = self.installed.iter().position(|i| i.place == place && i.id == id)?;
        std::mem::swap(&mut self.previous[at], &mut self.installed[now]);
        Some((self.installed[now].clone(), self.previous[at].clone()))
    }

    /// Forgets the version kept of `id` in `place`.
    pub fn forget_previous(&mut self, place: Place, id: &str) -> Option<Installed> {
        let at = self.previous.iter().position(|i| i.place == place && i.id == id)?;
        Some(self.previous.remove(at))
    }

    /// Records `installed`, in place of what was there for its place and
    /// ID, which is kept as the version it replaced, and binds its ID to
    /// its key there if it is not bound yet, and to `catalog`, the one that
    /// listed it, if it is not bound to one.
    pub fn record(&mut self, installed: Installed, catalog: Option<&str>) {
        if let Some(catalog) = catalog
            && self.catalog_bound(installed.place, &installed.id).is_none()
        {
            self.catalog_bound.push(CatalogBound {
                place: installed.place,
                id: base_id(&installed.id).into(),
                catalog: catalog.into(),
                since: installed.when,
            });
        }
        if self.bound(installed.place, &installed.id).is_none() {
            self.bound.push(Bound {
                place: installed.place,
                id: base_id(&installed.id).into(),
                key: installed.key,
                since: installed.when,
            });
        }
        if let Some(old) = self.forget(installed.place, &installed.id) {
            self.previous.push(old);
        }
        self.installed.push(installed);
    }

    /// Binds `id` to `key` from `since`, in `place` and for everyone,
    /// wherever it is bound to another key: [`Registry::trust`] said, as
    /// [`Trust::NewKey`] or [`Trust::Takes`], that each such binding moves
    /// to it (ADR 0592). Returns what each was before.
    pub fn rebind(&mut self, place: Place, id: &str, key: &PublicKey, since: u64) -> Vec<Bound> {
        let mut before = Vec::new();
        for b in &mut self.bound {
            let here = b.place == place || b.place == Place::Everyone;
            if here && b.id == base_id(id) && b.key != *key {
                before.push(b.clone());
                b.key = *key;
                b.since = since;
            }
        }
        before
    }

    /// Forgets the bundle `id` installed in `place`, and the version it
    /// replaced; its binding stays.
    pub fn forget(&mut self, place: Place, id: &str) -> Option<Installed> {
        self.forget_previous(place, id);
        let at = self.installed.iter().position(|i| i.place == place && i.id == id)?;
        Some(self.installed.remove(at))
    }

    /// Keeps `key` as the developer key of `user`.
    pub fn set_developer_key(&mut self, user: u32, key: PublicKey) {
        self.developer_keys.retain(|(u, _)| *u != user);
        self.developer_keys.push((user, key));
    }

    /// Keeps `withdrawn` for good, unless a line of the same, of a kind as
    /// bad, is kept: a malicious line is added beside a broken one, and
    /// nothing a source says later takes a line back. Past
    /// [`WITHDRAWN_MAX`] lines, the oldest of a version installed and kept
    /// nowhere goes. Whether it was new. One whose fields would not read
    /// back is not kept: it names no version that could install.
    pub fn withdraw(&mut self, withdrawn: Withdrawn) -> bool {
        let known = self.withdrawn.iter().any(|w| w.same(&withdrawn) && w.kind >= withdrawn.kind);
        if known || withdrawn.check().is_err() {
            return false;
        }
        self.withdrawn.push(withdrawn);
        while self.withdrawn.len() > WITHDRAWN_MAX {
            let in_use = |w: &Withdrawn| {
                self.installed
                    .iter()
                    .chain(&self.previous)
                    .any(|i| i.id == w.id && i.version == w.version)
            };
            let oldest = (0..self.withdrawn.len())
                .filter(|&n| !in_use(&self.withdrawn[n]))
                .min_by_key(|&n| self.withdrawn[n].since);
            let Some(oldest) = oldest else { break };
            self.withdrawn.remove(oldest);
        }
        true
    }

    /// The worst of `lines`: malicious before broken, then the first
    /// withdrawn.
    fn worst<'a>(lines: impl Iterator<Item = &'a Withdrawn>) -> Option<&'a Withdrawn> {
        lines.min_by_key(|w| (std::cmp::Reverse(w.kind), w.since))
    }

    /// Whether `source` may withdraw a version of what is installed in
    /// `place`, of the ID `id`: the catalog the ID is bound to there or
    /// for everyone, or the feed of a bundle of the ID there or for
    /// everyone.
    fn speaks_for(&self, source: &Source, place: Place, id: &str) -> bool {
        match source {
            Source::Catalog(name) => [place, Place::Everyone]
                .into_iter()
                .any(|at| self.catalog_bound(at, id).is_some_and(|b| b.catalog == *name)),
            Source::Feed(at) => *at == place || *at == Place::Everyone,
        }
    }

    /// The withdrawal of `id` at `version` in `place`, if one applies
    /// (ADR 0593): the worst of a catalog the ID is bound to there or for
    /// everyone, and of its feed there or for everyone.
    #[must_use]
    pub fn withdrawal(&self, place: Place, id: &str, version: &str) -> Option<&Withdrawn> {
        Registry::worst(self.withdrawn.iter().filter(|w| {
            w.id == id && w.version == version && self.speaks_for(&w.source, place, id)
        }))
    }

    /// The withdrawal of a file of `id` whose SHA-256 is `sha256`, for an
    /// install of it in `place`, whatever its version: the worst of any
    /// catalog's, and of its feed's there or for everyone. A feed of
    /// another place is a bundle another person installed, and refuses
    /// nothing here.
    #[must_use]
    pub fn withdrawn_file(&self, place: Place, id: &str, sha256: &str) -> Option<&Withdrawn> {
        Registry::worst(self.withdrawn.iter().filter(|w| {
            let source = match &w.source {
                Source::Catalog(_) => true,
                Source::Feed(at) => *at == place || *at == Place::Everyone,
            };
            w.id == id && w.sha256 == sha256 && source
        }))
    }

    /// The malicious withdrawal of the version of `id` installed in
    /// `place`, which stops it: it stays installed, but cannot run.
    #[must_use]
    pub fn stopped(&self, place: Place, id: &str) -> Option<&Withdrawn> {
        let installed = self.installed(place, id)?;
        self.withdrawal(place, id, &installed.version).filter(|w| w.kind == Kind::Malicious)
    }

    /// Forgets what the feed of `place` withdrew of `id`, or of its
    /// lines: the ID was taken from its publisher (ADR 0592), and what
    /// that publisher said of its versions is not said of the new one's.
    pub fn forget_feed_withdrawals(&mut self, place: Place, id: &str) -> usize {
        let before = self.withdrawn.len();
        self.withdrawn.retain(|w| w.source != Source::Feed(place) || base_id(&w.id) != base_id(id));
        before - self.withdrawn.len()
    }
}

/// Why a bundle's key is trusted, or that it is not known.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Trust {
    /// A catalog this computer trusts lists this very file, and its ID
    /// is bound to that catalog in the place since then, or will be.
    Catalog { name: String, since: Option<u64> },
    /// Its ID is bound to this key in the place, since then.
    Bound { since: u64 },
    /// Its ID is bound to this key for everyone, since then.
    BoundForEveryone { since: u64 },
    /// It is the asker's own developer key.
    Developer,
    /// No ID is bound here yet, and the key is not one this computer
    /// knows: it installs after a warning.
    Unknown,
    /// Its ID is bound to the key `old`, since then, and the rollover file
    /// the bundle carries moves it from that key to this one, from
    /// `moved` (ADR 0592): installing it binds the ID to this key, and
    /// `old` is refused from then on. `catalog` lists the file, if one
    /// does.
    NewKey { old: PublicKey, since: u64, moved: u64, catalog: Option<String> },
    /// Its ID is bound to the key `old`, since then, but `catalog` lists
    /// this file and vouches for the ID's publisher (ADR 0592): installing
    /// it removes the app of the old key, sets its data aside, and binds
    /// the ID to this key and to the catalog.
    Takes { catalog: String, old: PublicKey, since: u64 },
}

impl Trust {
    /// The catalog the file installs from, which its ID is bound to from
    /// then on if it is not bound to one.
    pub fn catalog(&self) -> Option<&str> {
        match self {
            Trust::Catalog { name, .. } | Trust::Takes { catalog: name, .. } => Some(name),
            Trust::NewKey { catalog, .. } => catalog.as_deref(),
            Trust::Bound { .. } | Trust::BoundForEveryone { .. } => None,
            Trust::Developer | Trust::Unknown => None,
        }
    }
}

/// What a bundle's file says of itself, and the catalogs of it, for
/// [`Registry::trust`].
#[derive(Clone, Copy, Debug)]
pub struct Claim<'a> {
    /// Its full ID.
    pub id: &'a str,
    /// The key that signed it.
    pub key: &'a PublicKey,
    /// The catalogs that list this very file, the one it was fetched from
    /// first.
    pub listed: &'a [String],
    /// The catalogs that vouch for the publisher of its ID (ADR 0592).
    pub owners: &'a [String],
    /// The rollover links it carries, read and their signatures checked.
    pub links: &'a [Link],
    /// When it was signed, if its signature says.
    pub signed_at: Option<i64>,
}

impl Registry {
    /// Whether the file `claim` tells of may install in `place` for user
    /// `asker`: its trust, or why not.
    ///
    /// - An ID bound to a catalog, for everyone or in the place, installs
    ///   only from a file that catalog lists, whatever key signed it.
    /// - Otherwise an ID bound to a key installs as that key signs; as
    ///   another key signs only if the bundle's rollover file moves the ID
    ///   from the bound key to it, or, for the binding of the place it
    ///   installs in, if the first catalog that lists the file vouches for
    ///   the ID's publisher (ADR 0592).
    /// - A file a catalog lists is trusted as that catalog's, the first of
    ///   `listed`; then the asker's developer key; then, if no catalog
    ///   vouches for the ID's publisher, nothing.
    pub fn trust(&self, place: Place, asker: u32, claim: &Claim<'_>) -> Result<Trust, String> {
        let Claim { id, key, listed, owners, .. } = *claim;
        let everyone = self.catalog_bound(Place::Everyone, id);
        let own = if place == Place::Everyone { None } else { self.catalog_bound(place, id) };
        if let Some(b) = own.or(everyone) {
            if !listed.contains(&b.catalog) {
                return Err(format!(
                    "{id} comes from catalog {}, since its first install from it, and that \
                     catalog does not list this file: only a file it lists may install it",
                    b.catalog
                ));
            }
            let since = own.map_or(b.since, |own| own.since);
            return Ok(Trust::Catalog { name: b.catalog.clone(), since: Some(since) });
        }
        let everyone = self.bound(Place::Everyone, id);
        let own = if place == Place::Everyone { None } else { self.bound(place, id) };
        // The catalog that lists the file and vouches for its publisher.
        let vouching = listed.first().filter(|c| owners.contains(c));
        let mut moves = None;
        for (binding, whose) in [(everyone, "for everyone"), (own, "here")] {
            let Some(b) = binding.filter(|b| b.key != *key) else { continue };
            let chain = rollover::chain(claim.links, base_id(id), &b.key, key, claim.signed_at);
            moves = Some(match (chain, vouching) {
                (Ok(moved), _) => Trust::NewKey {
                    old: b.key,
                    since: b.since,
                    moved: u64::try_from(moved).unwrap_or_default(),
                    catalog: listed.first().cloned(),
                },
                // Only the binding of the place it installs in: a person's
                // install takes nothing from everyone's.
                (Err(_), Some(catalog)) if b.place == place => {
                    Trust::Takes { catalog: catalog.clone(), old: b.key, since: b.since }
                }
                (Err(why), _) => {
                    let mut text = format!(
                        "{id} is signed by key {}, but it is bound to key {} {whose}, since its \
                         first install: only its publisher's key may install it",
                        key.encoded(),
                        b.key.encoded()
                    );
                    if !claim.links.is_empty() {
                        text.push_str(&format!("; its rollover file does not move it: {why}"));
                    }
                    if let Some(catalog) = vouching {
                        text.push_str(&format!(
                            "; catalog {catalog} vouches for its publisher, and an administrator \
                             installing it for everyone from {catalog} replaces the app of key {}",
                            b.key.encoded()
                        ));
                    } else if let Some(catalog) = owners.first() {
                        text.push_str(&format!(
                            "; catalog {catalog} vouches for its publisher: install it from \
                             {catalog}"
                        ));
                    }
                    return Err(text);
                }
            });
        }
        // Taking the ID over is the one to warn of, if both are asked.
        if let Some(trust) = moves {
            return Ok(trust);
        }
        if let Some(name) = listed.first() {
            return Ok(Trust::Catalog { name: name.clone(), since: None });
        }
        if let Some(b) = own {
            return Ok(Trust::Bound { since: b.since });
        }
        if let Some(b) = everyone {
            return Ok(if place == Place::Everyone {
                Trust::Bound { since: b.since }
            } else {
                Trust::BoundForEveryone { since: b.since }
            });
        }
        if self.developer_key(asker) == Some(*key) {
            return Ok(Trust::Developer);
        }
        if let Some(catalog) = owners.first() {
            return Err(format!(
                "{id} belongs to the publisher catalog {catalog} vouches for: install it from \
                 {catalog}"
            ));
        }
        Ok(Trust::Unknown)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bundles::minisign::SecretKey;

    fn secret(n: u8) -> SecretKey {
        let mut random = [n; 40];
        random[0] = n.wrapping_add(1);
        SecretKey::generate(&random).unwrap()
    }

    fn key(n: u8) -> PublicKey {
        secret(n).public()
    }

    /// A file of `id` signed by `key`, which `listed` list, of an ID no
    /// catalog vouches for, with no rollover file.
    fn claim<'a>(id: &'a str, key: &'a PublicKey, listed: &'a [String]) -> Claim<'a> {
        Claim { id, key, listed, owners: &[], links: &[], signed_at: None }
    }

    fn installed(place: Place, id: &str, n: u8) -> Installed {
        Installed {
            place,
            id: id.into(),
            version: "1.0.0".into(),
            key: key(n),
            list: "ab".repeat(32),
            when: 1_793_000_000,
        }
    }

    #[test]
    fn the_files_read_back() {
        let mut registry = Registry::default();
        registry.record(installed(Place::User(1000), "org.example.notes", 1), None);
        registry.record(installed(Place::Everyone, "org.example.tea", 2), None);
        registry.set_developer_key(1000, key(1));
        let texts = registry.texts();
        let again = Registry::read(&texts).unwrap();
        let [a, b, ..] = texts;
        assert_eq!(again.texts(), registry.texts());
        assert_eq!(again.installed.len(), 2);
        assert_eq!(again.bound.len(), 2);
        assert!(a.starts_with("1000\torg.example.notes\t1.0.0\tRW"));
        assert!(b.contains("everyone\torg.example.tea\tRW"));
        assert_eq!(again.developer_key(1000), Some(key(1)));
        assert_eq!(Registry::read(&Default::default()).unwrap(), Registry::default());
    }

    #[test]
    fn lines_are_kept_by_full_id_and_bound_by_id() {
        let mut registry = Registry::default();
        registry.record(installed(Place::Everyone, "org.nodejs.node@20", 1), Some("dukovo"));
        registry.record(installed(Place::Everyone, "org.nodejs.node@22", 1), Some("dukovo"));
        assert_eq!(registry.installed.len(), 2);
        assert!(registry.previous.is_empty());
        // One binding for the ID, which binds every line.
        assert_eq!(registry.bound.len(), 1);
        assert_eq!(registry.bound[0].id, "org.nodejs.node");
        assert!(registry.bound(Place::Everyone, "org.nodejs.node@24").is_some());
        assert!(registry.catalog_bound(Place::Everyone, "org.nodejs.node@24").is_some());
        registry.set_default_line(Place::User(1000), "org.nodejs.node", Some("20"));
        registry.set_default_line(Place::Everyone, "org.nodejs.node", Some("22"));
        registry.set_held(Place::Everyone, "org.nodejs.node@22", true, 5);
        let again = Registry::read(&registry.texts()).unwrap();
        assert_eq!(again, registry);
        assert_eq!(again.default_line(Place::User(1000), "org.nodejs.node"), Some("20"));
        assert_eq!(again.default_line(Place::Everyone, "org.nodejs.node"), Some("22"));
        assert_eq!(again.texts()[7], "1000\torg.nodejs.node\t20\neveryone\torg.nodejs.node\t22\n");
        registry.set_default_line(Place::User(1000), "org.nodejs.node", None);
        assert_eq!(registry.default_line(Place::User(1000), "org.nodejs.node"), None);
        assert_eq!(base_id("org.nodejs.node@22"), "org.nodejs.node");
        assert_eq!(base_id("org.nodejs.node"), "org.nodejs.node");
        // A default names an ID and a line; a binding, the ID alone.
        let mut texts = Registry::default().texts();
        texts[7] = "everyone\torg.nodejs.node@22\t22\n".into();
        assert!(Registry::read(&texts).is_err());
        texts[7] = "everyone\torg.nodejs.node\tv22\n".into();
        assert!(Registry::read(&texts).is_err());
    }

    #[test]
    fn a_wrong_line_is_refused() {
        let good = Registry::default().texts();
        let line = "1000\torg.example.notes\t1.0.0\t".to_string() + &key(1).encoded();
        let list = "ab".repeat(32);
        for bad in [
            format!("{line}\t{list}\t17"),
            format!("{line}\t{list}\t17\n{line}\t{list}\t18\n"),
            format!("{line}\t{list}\n"),
            format!("{line}\t{list}\t017\n"),
            format!("{line}\t{}\t17\n", "AB".repeat(32)),
            format!("01000{}\t{list}\t17\n", &line[4..]),
            format!("{}\t{list}\t17\n", line.replace("org.example.notes", "Notes")),
            format!("{}\t{list}\t17\n", line.replace("1.0.0", "1.0.0\u{1b}")),
            format!("all{}\t{list}\t17\n", &line[4..]),
        ] {
            let mut texts = good.clone();
            texts[0] = bad.clone();
            assert!(Registry::read(&texts).is_err(), "{bad:?}");
        }
        let two = format!("5\t{}\n5\t{}\n", key(1).encoded(), key(2).encoded());
        let mut texts = good.clone();
        texts[2] = two;
        assert!(Registry::read(&texts).unwrap_err().contains("twice"));
        let mut texts = good;
        texts[6] = "1000\t5\n1000\t6\n".into();
        assert!(Registry::read(&texts).unwrap_err().contains("twice"));
    }

    #[test]
    fn keys_are_trusted_as_bound() {
        let mut registry = Registry::default();
        let alice = Place::User(1000);
        let bob = Place::User(1001);
        let id = "org.example.notes";
        // Nothing bound: her developer key is hers alone to trust.
        registry.set_developer_key(1000, key(1));
        assert_eq!(registry.trust(alice, 1000, &claim(id, &key(1), &[])), Ok(Trust::Developer));
        assert_eq!(registry.trust(bob, 1001, &claim(id, &key(1), &[])), Ok(Trust::Unknown));
        assert_eq!(registry.trust(alice, 1000, &claim(id, &key(2), &[])), Ok(Trust::Unknown));
        // Bound for her: only that key, for her; bob is free.
        registry.record(installed(alice, id, 1), None);
        assert_eq!(
            registry.trust(alice, 1000, &claim(id, &key(1), &[])),
            Ok(Trust::Bound { since: 1_793_000_000 })
        );
        assert!(
            registry
                .trust(alice, 1000, &claim(id, &key(2), &[]))
                .unwrap_err()
                .contains("bound to key")
        );
        assert_eq!(registry.trust(bob, 1001, &claim(id, &key(2), &[])), Ok(Trust::Unknown));
        // Bound for everyone: that key, for everyone and each person.
        registry.record(installed(Place::Everyone, "org.example.tea", 3), None);
        let tea = "org.example.tea";
        assert!(registry.trust(bob, 1001, &claim(tea, &key(2), &[])).is_err());
        assert_eq!(
            registry.trust(bob, 1001, &claim(tea, &key(3), &[])),
            Ok(Trust::BoundForEveryone { since: 1_793_000_000 })
        );
        assert_eq!(
            registry.trust(Place::Everyone, 0, &claim(tea, &key(3), &[])),
            Ok(Trust::Bound { since: 1_793_000_000 })
        );
        // Forgetting keeps the binding.
        assert!(registry.forget(alice, id).is_some());
        assert!(registry.installed(alice, id).is_none());
        assert!(registry.trust(alice, 1000, &claim(id, &key(2), &[])).is_err());
    }

    #[test]
    fn an_id_from_a_catalog_installs_only_from_it() {
        let mut registry = Registry::default();
        let alice = Place::User(1000);
        let id = "io.github.burntsushi.ripgrep";
        let dukovo = vec!["dukovo".to_string()];
        // Listed: the catalog's, whatever key signed it.
        assert_eq!(
            registry.trust(alice, 1000, &claim(id, &key(1), &dukovo)),
            Ok(Trust::Catalog { name: "dukovo".into(), since: None })
        );
        registry.record(installed(alice, id, 1), Some("dukovo"));
        assert_eq!(registry.catalog_bound(alice, id).unwrap().catalog, "dukovo");
        // From then on, a file it lists, signed by any key; nothing else,
        // not even one signed by the same key.
        assert_eq!(
            registry.trust(alice, 1000, &claim(id, &key(2), &dukovo)),
            Ok(Trust::Catalog { name: "dukovo".into(), since: Some(1_793_000_000) })
        );
        let other = vec!["feed".to_string()];
        assert!(
            registry
                .trust(alice, 1000, &claim(id, &key(1), &[]))
                .unwrap_err()
                .contains("catalog dukovo")
        );
        assert!(registry.trust(alice, 1000, &claim(id, &key(1), &other)).is_err());
        // An ID bound to a key stays that key's, though a catalog lists it.
        let notes = "org.example.notes";
        registry.record(installed(alice, notes, 3), None);
        assert!(registry.trust(alice, 1000, &claim(notes, &key(4), &dukovo)).is_err());
        assert_eq!(
            registry.trust(alice, 1000, &claim(notes, &key(3), &dukovo)),
            Ok(Trust::Catalog { name: "dukovo".into(), since: None })
        );
        // The bindings read back.
        let again = Registry::read(&registry.texts()).unwrap();
        assert_eq!(again, registry);
        assert!(registry.texts()[3].starts_with("1000\tio.github.burntsushi.ripgrep\tdukovo\t"));
    }

    #[test]
    fn a_rollover_moves_an_id_to_a_new_key_for_good() {
        let mut registry = Registry::default();
        let alice = Place::User(1000);
        let id = "org.example.notes";
        registry.record(installed(alice, id, 1), None);
        let text = rollover::link(&secret(1), id, &key(2), 1_794_000_000);
        let links = rollover::read(&text).unwrap();
        let new_key = key(2);
        let moving =
            Claim { links: &links, signed_at: Some(1_795_000_000), ..claim(id, &new_key, &[]) };
        // Without the links, refused; with them, a move from the bound key.
        assert!(registry.trust(alice, 1000, &claim(id, &new_key, &[])).is_err());
        assert_eq!(
            registry.trust(alice, 1000, &moving),
            Ok(Trust::NewKey {
                old: key(1),
                since: 1_793_000_000,
                moved: 1_794_000_000,
                catalog: None
            })
        );
        // Not when the bundle was signed before the move, nor for another
        // ID, and saying why.
        let early = Claim { signed_at: Some(1_793_500_000), ..moving };
        let error = registry.trust(alice, 1000, &early).unwrap_err();
        assert!(error.contains("rollover file does not move it"), "{error}");
        assert!(registry.trust(alice, 1000, &Claim { id: "org.example.tea", ..moving }).is_ok());
        // Rebound: the new key as bound, and never the old one again.
        let before = registry.rebind(alice, id, &new_key, 1_795_000_000);
        assert_eq!(before.iter().map(|b| b.key).collect::<Vec<_>>(), [key(1)]);
        assert!(registry.rebind(alice, id, &new_key, 1_795_000_000).is_empty());
        assert_eq!(
            registry.trust(alice, 1000, &claim(id, &new_key, &[])),
            Ok(Trust::Bound { since: 1_795_000_000 })
        );
        assert!(registry.trust(alice, 1000, &claim(id, &key(1), &[])).is_err());
        let back = rollover::read(&rollover::link(&secret(2), id, &key(1), 1_796_000_000)).unwrap();
        let old_key = key(1);
        let backwards = Claim { links: &back, ..claim(id, &old_key, &[]) };
        assert!(registry.trust(alice, 1000, &backwards).is_ok());
        assert_eq!(Registry::read(&registry.texts()).unwrap(), registry);
    }

    /// [`claim`], of an ID the catalogs `owners` vouch for.
    fn vouched<'a>(
        id: &'a str,
        key: &'a PublicKey,
        listed: &'a [String],
        owners: &'a [String],
    ) -> Claim<'a> {
        Claim { owners, ..claim(id, key, listed) }
    }

    #[test]
    fn a_catalog_that_vouches_for_a_publisher_takes_its_id() {
        let mut registry = Registry::default();
        let (alice, bob) = (Place::User(1000), Place::User(1001));
        let (id, tea) = ("org.example.notes", "org.example.tea");
        let dukovo = ["dukovo".to_string()];
        let other = ["other".to_string()];
        let (real, stranger) = (key(2), key(3));
        // An impostor bound first, for Alice: the catalog takes it.
        registry.record(installed(alice, id, 1), None);
        let takes = Trust::Takes { catalog: "dukovo".into(), old: key(1), since: 1_793_000_000 };
        assert_eq!(
            registry.trust(alice, 1000, &vouched(id, &real, &dukovo, &dukovo)),
            Ok(takes.clone())
        );
        // Not a catalog that does not vouch: the refusal names the one that does.
        let error = registry.trust(alice, 1000, &vouched(id, &real, &other, &dukovo)).unwrap_err();
        assert!(error.contains("catalog dukovo vouches for its publisher: install it from dukovo"));
        // Nothing bound for Bob: only from the catalog, or his own key.
        let error = registry.trust(bob, 1001, &vouched(id, &stranger, &[], &dukovo)).unwrap_err();
        assert!(error.contains("belongs to the publisher catalog dukovo vouches for"), "{error}");
        registry.set_developer_key(1001, key(3));
        assert_eq!(
            registry.trust(bob, 1001, &vouched(id, &stranger, &[], &dukovo)),
            Ok(Trust::Developer)
        );
        // Bound for everyone: a person's install takes nothing from it; one
        // for everyone does.
        registry.record(installed(Place::Everyone, tea, 1), None);
        let error = registry.trust(bob, 1001, &vouched(tea, &real, &dukovo, &dukovo)).unwrap_err();
        assert!(error.contains("an administrator installing it for everyone"), "{error}");
        assert_eq!(
            registry.trust(Place::Everyone, 0, &vouched(tea, &real, &dukovo, &dukovo)),
            Ok(takes)
        );
        // Taken: bound to the new key and to the catalog, the old key
        // refused.
        let mut taken = installed(alice, id, 2);
        taken.when = 1_795_000_000;
        assert_eq!(registry.rebind(alice, id, &real, taken.when).len(), 1);
        registry.record(taken, Some("dukovo"));
        assert_eq!(
            registry.trust(alice, 1000, &vouched(id, &real, &dukovo, &dukovo)),
            Ok(Trust::Catalog { name: "dukovo".into(), since: Some(1_795_000_000) })
        );
        assert!(registry.trust(alice, 1000, &claim(id, &key(1), &[])).is_err());
    }

    #[test]
    fn a_replaced_version_is_kept_to_go_back_to() {
        let mut registry = Registry::default();
        let alice = Place::User(1000);
        let id = "org.example.notes";
        registry.record(installed(alice, id, 1), None);
        assert_eq!(registry.previous(alice, id), None);
        let mut newer = installed(alice, id, 1);
        newer.version = "1.1.0".into();
        newer.list = "cd".repeat(32);
        registry.record(newer.clone(), None);
        assert_eq!(registry.installed(alice, id), Some(&newer));
        assert_eq!(registry.previous(alice, id).map(|p| p.version.as_str()), Some("1.0.0"));
        // Back, and forward again: each time the other is kept.
        let (now, kept) = registry.roll_back(alice, id).unwrap();
        assert_eq!((now.version.as_str(), kept.version.as_str()), ("1.0.0", "1.1.0"));
        let (now, kept) = registry.roll_back(alice, id).unwrap();
        assert_eq!((now.version.as_str(), kept.version.as_str()), ("1.1.0", "1.0.0"));
        assert_eq!(registry.roll_back(Place::Everyone, id), None);
        // Held, automatic, and all of it read back.
        assert!(registry.set_held(alice, id, true, 5));
        assert!(!registry.set_held(alice, id, true, 6));
        assert_eq!(registry.held(alice, id).map(|h| h.since), Some(5));
        assert!(registry.set_automatic(Place::Everyone, true, 7));
        assert!(!registry.set_automatic(Place::Everyone, true, 8));
        assert_eq!(registry.automatic(Place::Everyone), Some(7));
        assert_eq!(registry.automatic(alice), None);
        let again = Registry::read(&registry.texts()).unwrap();
        assert_eq!(again, registry);
        assert!(registry.texts()[4].starts_with("1000\torg.example.notes\t1.0.0\tRW"));
        assert_eq!(registry.texts()[5], "1000\torg.example.notes\t5\n");
        assert_eq!(registry.texts()[6], "everyone\t7\n");
        assert!(registry.set_held(alice, id, false, 9));
        assert!(registry.set_automatic(Place::Everyone, false, 9));
        // Removing forgets the kept version too.
        registry.forget(alice, id);
        assert_eq!(registry.previous(alice, id), None);
    }

    fn withdrawn(source: Source, version: &str, sha: char, kind: Kind, since: u64) -> Withdrawn {
        Withdrawn {
            source,
            id: "org.example.notes".into(),
            version: version.into(),
            sha256: sha.to_string().repeat(64),
            kind,
            since,
            reason: "it loses notes".into(),
        }
    }

    fn catalog(name: &str) -> Source {
        Source::Catalog(name.into())
    }

    #[test]
    fn a_withdrawal_is_kept_for_good_and_only_made_worse() {
        let mut registry = Registry::default();
        let broken = withdrawn(catalog("dukovo"), "1.0.0", 'a', Kind::Broken, 5);
        assert!(registry.withdraw(broken.clone()));
        assert!(!registry.withdraw(broken.clone()));
        // A later reason or time is the same line.
        assert!(!registry.withdraw(Withdrawn {
            reason: "other".into(),
            since: 9,
            ..broken.clone()
        }));
        // Malicious is added beside it; broken again adds nothing.
        let malicious = Withdrawn { kind: Kind::Malicious, ..broken.clone() };
        assert!(registry.withdraw(malicious.clone()));
        assert!(!registry.withdraw(broken.clone()));
        assert_eq!(registry.withdrawn.len(), 2);
        // One that would not read back is not kept.
        assert!(!registry.withdraw(Withdrawn { version: String::new(), ..broken.clone() }));
        assert!(!registry.withdraw(Withdrawn { id: "Not An ID".into(), ..broken.clone() }));
        assert!(!registry.withdraw(Withdrawn { sha256: "ab".into(), ..malicious.clone() }));
        // Another source, file or version is a line of its own.
        assert!(registry.withdraw(withdrawn(catalog("other"), "1.0.0", 'a', Kind::Broken, 5)));
        assert!(registry.withdraw(withdrawn(catalog("dukovo"), "1.0.0", 'b', Kind::Broken, 5)));
        let feed = withdrawn(Source::Feed(Place::User(1000)), "2.0.0", 'c', Kind::Broken, 6);
        assert!(registry.withdraw(feed));
        let texts = registry.texts();
        assert!(texts[8].starts_with("catalog:dukovo\torg.example.notes\t1.0.0\t"), "{}", texts[8]);
        assert!(texts[8].contains("\nfeed:1000\torg.example.notes\t2.0.0\t"), "{}", texts[8]);
        let again = Registry::read(&texts).unwrap();
        assert_eq!(again.texts(), texts);
        // A line twice, or one that does not read, is refused.
        let mut twice = texts.clone();
        let first = texts[8].lines().next().unwrap();
        twice[8] = format!("{first}\n{}", texts[8]);
        assert!(Registry::read(&twice).unwrap_err().contains("twice"));
        let good = format!(
            "catalog:dukovo\torg.example.notes\t1.0.0\t{}\tbroken\t5\twhy\n",
            "a".repeat(64)
        );
        let mut texts = Registry::default().texts();
        texts[8] = good.clone();
        assert!(Registry::read(&texts).is_ok());
        for bad in [
            good.replace("catalog:dukovo", "catalog:Bad"),
            good.replace("catalog:dukovo", "mirror:dukovo"),
            good.replace("catalog:dukovo", "feed:me"),
            good.replace("broken", "bad"),
            good.replace("\t5\t", "\t05\t"),
            good.replace("\twhy", "\t"),
            good.replace("\twhy", ""),
            good.replace(&"a".repeat(64), "A".repeat(64).as_str()),
        ] {
            texts[8] = bad.clone();
            assert!(Registry::read(&texts).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn a_withdrawal_applies_where_its_source_speaks_for_the_id() {
        let alice = Place::User(1000);
        let bob = Place::User(1001);
        let id = "org.example.notes";
        let mut registry = Registry::default();
        for line in [
            withdrawn(catalog("dukovo"), "1.0.0", 'a', Kind::Broken, 5),
            withdrawn(catalog("dukovo"), "1.0.0", 'a', Kind::Malicious, 7),
            withdrawn(catalog("other"), "2.0.0", 'b', Kind::Malicious, 5),
            withdrawn(Source::Feed(alice), "3.0.0", 'c', Kind::Broken, 5),
        ] {
            registry.withdraw(line);
        }
        // Bound to no catalog: no catalog's withdrawal of a version
        // applies; the feed's of the place does.
        assert_eq!(registry.withdrawal(alice, id, "1.0.0"), None);
        assert_eq!(registry.withdrawal(alice, id, "3.0.0").map(|w| w.kind), Some(Kind::Broken));
        assert_eq!(registry.withdrawal(bob, id, "3.0.0"), None);
        assert_eq!(registry.withdrawal(Place::Everyone, id, "3.0.0"), None);
        // Bound to dukovo for everyone: its withdrawals apply in every
        // place, the worst first; another catalog's do not.
        registry.record(installed(Place::Everyone, id, 1), Some("dukovo"));
        for place in [Place::Everyone, alice, bob] {
            let w = registry.withdrawal(place, id, "1.0.0").unwrap();
            assert_eq!((w.kind, w.since), (Kind::Malicious, 7));
            assert_eq!(registry.withdrawal(place, id, "2.0.0"), None);
        }
        assert_eq!(registry.stopped(Place::Everyone, id).map(|w| w.since), Some(7));
        assert_eq!(registry.stopped(alice, id), None);
        // Bound to other in Bob's place: other's apply there too.
        registry.catalog_bound.push(CatalogBound {
            place: bob,
            id: id.into(),
            catalog: "other".into(),
            since: 1,
        });
        assert!(registry.withdrawal(bob, id, "2.0.0").is_some());
        assert_eq!(registry.withdrawal(alice, id, "2.0.0"), None);
        // A broken withdrawal does not stop it.
        let mut registry = Registry::default();
        registry.record(installed(Place::Everyone, id, 1), Some("dukovo"));
        registry.withdraw(withdrawn(catalog("dukovo"), "1.0.0", 'a', Kind::Broken, 5));
        assert!(registry.withdrawal(Place::Everyone, id, "1.0.0").is_some());
        assert_eq!(registry.stopped(Place::Everyone, id), None);
    }

    #[test]
    fn a_withdrawn_file_is_refused_by_its_sha256() {
        let alice = Place::User(1000);
        let id = "org.example.notes";
        let mut registry = Registry::default();
        registry.withdraw(withdrawn(catalog("other"), "1.0.0", 'a', Kind::Broken, 5));
        registry.withdraw(withdrawn(Source::Feed(alice), "2.0.0", 'b', Kind::Malicious, 5));
        // Any catalog's, wherever it installs, of that ID alone.
        for place in [Place::Everyone, alice, Place::User(1001)] {
            assert!(registry.withdrawn_file(place, id, &"a".repeat(64)).is_some());
        }
        assert!(registry.withdrawn_file(alice, "org.example.other", &"a".repeat(64)).is_none());
        assert!(registry.withdrawn_file(alice, id, &"c".repeat(64)).is_none());
        // A person's feed, in that person's place alone.
        assert!(registry.withdrawn_file(alice, id, &"b".repeat(64)).is_some());
        assert!(registry.withdrawn_file(Place::User(1001), id, &"b".repeat(64)).is_none());
        assert!(registry.withdrawn_file(Place::Everyone, id, &"b".repeat(64)).is_none());
        // Taken from its publisher, what its feed said goes.
        assert_eq!(registry.forget_feed_withdrawals(Place::Everyone, id), 0);
        assert_eq!(registry.forget_feed_withdrawals(alice, "org.example.notes@2"), 1);
        assert!(registry.withdrawn_file(alice, id, &"b".repeat(64)).is_none());
        assert_eq!(registry.withdrawn.len(), 1);
    }

    #[test]
    fn past_its_most_the_oldest_unused_withdrawal_goes() {
        let mut registry = Registry::default();
        let id = "org.example.notes";
        registry.record(installed(Place::Everyone, id, 1), None);
        // The installed version's line is the oldest, and stays.
        registry.withdraw(withdrawn(Source::Feed(Place::Everyone), "1.0.0", 'a', Kind::Broken, 0));
        for n in 1..=WITHDRAWN_MAX as u64 {
            let version = format!("0.{n}.0");
            registry.withdraw(withdrawn(catalog("dukovo"), &version, 'b', Kind::Broken, n));
        }
        assert_eq!(registry.withdrawn.len(), WITHDRAWN_MAX);
        assert!(registry.withdrawn.iter().any(|w| w.version == "1.0.0"));
        assert!(!registry.withdrawn.iter().any(|w| w.version == "0.1.0"));
        assert!(registry.withdrawn.iter().any(|w| w.version == "0.2.0"));
    }
}
