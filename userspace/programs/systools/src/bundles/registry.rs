// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! What bundled(8) keeps in `/system/state/bundled` (ADR 0223), which only
//! it writes: the bundles it installed, the key each ID is bound to, each
//! person's developer key, the catalog each ID installed from one is
//! bound to (ADR 0383), and the version each new one replaced, the IDs
//! held at their version, and the places that update by themselves (ADR
//! 0384), and the line of each ID that runs unless a project asks for
//! another (ADR 0387). Each is a file of lines, tab-separated, read
//! strictly: a line that does not read is an error, never skipped.
//!
//! A bundle with a line is kept by its full ID, `ID@LINE`; the key and
//! catalog an ID is bound to bind all its lines, so bindings are kept by
//! the ID alone.

use std::fmt;

use super::minisign::PublicKey;

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
/// The files, in the order [`Registry::read`] takes them.
pub const FILES: [&str; 8] =
    [INSTALLED, BOUND, DEVELOPER_KEYS, CATALOG_BOUND, PREVIOUS, HELD, AUTOMATIC, DEFAULTS];

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
    pub fn read(texts: &[String; 8]) -> Result<Registry, String> {
        let [installed, bound, developer_keys, catalog_bound, previous, held, automatic, defaults] =
            texts;
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
        let registry = Registry {
            installed,
            bound,
            developer_keys,
            catalog_bound,
            previous,
            held,
            automatic,
            defaults,
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
        Ok(())
    }

    /// The files' texts, in the order of [`FILES`], sorted.
    pub fn texts(&self) -> [String; 8] {
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
        [
            installed_text(&self.installed),
            bound.concat(),
            keys.concat(),
            catalogs.concat(),
            installed_text(&self.previous),
            held.concat(),
            automatic.concat(),
            defaults.concat(),
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
}

impl Registry {
    /// Whether a file of `id` signed by `key`, which the catalogs
    /// `listed` list, may install in `place` for user `asker`: its trust,
    /// or why not.
    ///
    /// - An ID bound to a catalog, for everyone or in the place, installs
    ///   only from a file that catalog lists, whatever key signed it.
    /// - Otherwise an ID bound to a key installs only as that key signs.
    /// - A file a catalog lists is trusted as that catalog's, the first of
    ///   `listed`; then the asker's developer key; then nothing.
    pub fn trust(
        &self,
        place: Place,
        asker: u32,
        id: &str,
        key: &PublicKey,
        listed: &[String],
    ) -> Result<Trust, String> {
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
        for (binding, whose) in [(everyone, "for everyone"), (own, "here")] {
            if let Some(b) = binding
                && b.key != *key
            {
                return Err(format!(
                    "{id} is signed by key {}, but it is bound to key {} {whose}, since its first \
                     install: only its publisher's key may install it",
                    key.encoded(),
                    b.key.encoded()
                ));
            }
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
        Ok(Trust::Unknown)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bundles::minisign::SecretKey;

    fn key(n: u8) -> PublicKey {
        let mut random = [n; 40];
        random[0] = n.wrapping_add(1);
        SecretKey::generate(&random).unwrap().public()
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
        assert_eq!(registry.trust(alice, 1000, id, &key(1), &[]), Ok(Trust::Developer));
        assert_eq!(registry.trust(bob, 1001, id, &key(1), &[]), Ok(Trust::Unknown));
        assert_eq!(registry.trust(alice, 1000, id, &key(2), &[]), Ok(Trust::Unknown));
        // Bound for her: only that key, for her; bob is free.
        registry.record(installed(alice, id, 1), None);
        assert_eq!(
            registry.trust(alice, 1000, id, &key(1), &[]),
            Ok(Trust::Bound { since: 1_793_000_000 })
        );
        assert!(
            registry.trust(alice, 1000, id, &key(2), &[]).unwrap_err().contains("bound to key")
        );
        assert_eq!(registry.trust(bob, 1001, id, &key(2), &[]), Ok(Trust::Unknown));
        // Bound for everyone: that key, for everyone and each person.
        registry.record(installed(Place::Everyone, "org.example.tea", 3), None);
        let tea = "org.example.tea";
        assert!(registry.trust(bob, 1001, tea, &key(2), &[]).is_err());
        assert_eq!(
            registry.trust(bob, 1001, tea, &key(3), &[]),
            Ok(Trust::BoundForEveryone { since: 1_793_000_000 })
        );
        assert_eq!(
            registry.trust(Place::Everyone, 0, tea, &key(3), &[]),
            Ok(Trust::Bound { since: 1_793_000_000 })
        );
        // Forgetting keeps the binding.
        assert!(registry.forget(alice, id).is_some());
        assert!(registry.installed(alice, id).is_none());
        assert!(registry.trust(alice, 1000, id, &key(2), &[]).is_err());
    }

    #[test]
    fn an_id_from_a_catalog_installs_only_from_it() {
        let mut registry = Registry::default();
        let alice = Place::User(1000);
        let id = "io.github.burntsushi.ripgrep";
        let dukovo = vec!["dukovo".to_string()];
        // Listed: the catalog's, whatever key signed it.
        assert_eq!(
            registry.trust(alice, 1000, id, &key(1), &dukovo),
            Ok(Trust::Catalog { name: "dukovo".into(), since: None })
        );
        registry.record(installed(alice, id, 1), Some("dukovo"));
        assert_eq!(registry.catalog_bound(alice, id).unwrap().catalog, "dukovo");
        // From then on, a file it lists, signed by any key; nothing else,
        // not even one signed by the same key.
        assert_eq!(
            registry.trust(alice, 1000, id, &key(2), &dukovo),
            Ok(Trust::Catalog { name: "dukovo".into(), since: Some(1_793_000_000) })
        );
        let other = vec!["feed".to_string()];
        assert!(
            registry.trust(alice, 1000, id, &key(1), &[]).unwrap_err().contains("catalog dukovo")
        );
        assert!(registry.trust(alice, 1000, id, &key(1), &other).is_err());
        // An ID bound to a key stays that key's, though a catalog lists it.
        let notes = "org.example.notes";
        registry.record(installed(alice, notes, 3), None);
        assert!(registry.trust(alice, 1000, notes, &key(4), &dukovo).is_err());
        assert_eq!(
            registry.trust(alice, 1000, notes, &key(3), &dukovo),
            Ok(Trust::Catalog { name: "dukovo".into(), since: None })
        );
        // The bindings read back.
        let again = Registry::read(&registry.texts()).unwrap();
        assert_eq!(again, registry);
        assert!(registry.texts()[3].starts_with("1000\tio.github.burntsushi.ripgrep\tdukovo\t"));
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
}
