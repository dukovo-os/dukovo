// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Canaries that report (ADRs 0456 and 0533). A canary is a machine the
//! project owns that takes every release first. Each hour it signs a
//! [`Report`] of its own state with a key of its own and sends it to the
//! update server, whose receiver keeps the newest report of each
//! canary on the [`Canaries`] list and nothing else. The update server
//! then works out which canaries are not green ([`evaluate`]) and holds
//! the waves while any is not: a canary that failed a check, fell back,
//! fell silent, or did not take a new release in time.
//!
//! A report is the exact bytes the canary signed, carried as a string
//! in an [`Envelope`], so no step of canonical JSON stands between what
//! was signed and what is checked. The receiver checks a report before
//! it keeps it, and the update server checks it again against the list
//! before it trusts it, so a receiver that someone took over can only
//! withhold or replay reports, which ends in a hold, and never forge a
//! green one.

use std::collections::BTreeMap;
use std::io::Write as _;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::list::hex;

/// What a report's `type` says.
pub const REPORT_TYPE: &str = "dukovo-canary-report";

/// The version of the report's format.
pub const REPORT_VERSION: u32 = 1;

/// Where on the update server's host a canary sends its report.
pub const REPORT_PATH: &str = "/canary/v1/report";

/// The largest envelope taken, in bytes.
pub const REPORT_MAX: usize = 16 * 1024;

/// How long after its newest report a canary counts as silent, in
/// seconds: two missed hours.
pub const SILENT_AFTER: i64 = 2 * 60 * 60;

/// How long a canary may run an older release than its channel offers,
/// in seconds, before it counts as stuck.
pub const BEHIND_AFTER: i64 = 6 * 60 * 60;

/// How far ahead of the server's clock a report's time may be, in
/// seconds.
pub const AHEAD_LIMIT: i64 = 5 * 60;

/// The longest text a report's field may hold, in bytes.
pub const FIELD_MAX: usize = 512;

/// The most items a report's list may hold.
pub const LIST_MAX: usize = 32;

/// Whether `name` may name a canary: 1 to 32 of `a-z`, `0-9` and `-`,
/// not starting with `-`. It names the report's file, so nothing else
/// is allowed.
#[must_use]
pub fn valid_name(name: &str) -> bool {
    (1..=32).contains(&name.len())
        && !name.starts_with('-')
        && name.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// The ID of a public key, as an envelope names it: the SHA-256 of its
/// 32 bytes, in hex.
#[must_use]
pub fn key_id(public: &[u8]) -> String {
    hex(ring::digest::digest(&ring::digest::SHA256, public).as_ref())
}

/// `text`, an even number of hex digits, as bytes.
fn unhex(text: &str) -> Option<Vec<u8>> {
    if !text.len().is_multiple_of(2) {
        return None;
    }
    (0..text.len())
        .step_by(2)
        .map(|at| text.get(at..at + 2).and_then(|pair| u8::from_str_radix(pair, 16).ok()))
        .collect()
}

/// A canary's signing key, kept by the canary alone.
pub struct Key {
    pair: ring::signature::Ed25519KeyPair,
}

impl Key {
    /// A new key, as the PKCS #8 bytes that [`Key::from_pkcs8`] reads.
    ///
    /// # Errors
    ///
    /// If the system gives no random bytes.
    pub fn generate() -> Result<Vec<u8>, String> {
        ring::signature::Ed25519KeyPair::generate_pkcs8(&ring::rand::SystemRandom::new())
            .map(|doc| doc.as_ref().to_vec())
            .map_err(|_| "no random bytes for a new key".to_string())
    }

    /// The key kept as `bytes`.
    ///
    /// # Errors
    ///
    /// If they are not an Ed25519 key.
    pub fn from_pkcs8(bytes: &[u8]) -> Result<Key, String> {
        ring::signature::Ed25519KeyPair::from_pkcs8(bytes)
            .map(|pair| Key { pair })
            .map_err(|_| "not an Ed25519 key".to_string())
    }

    /// Its public half, in hex, as the canary list keeps it.
    #[must_use]
    pub fn public_hex(&self) -> String {
        use ring::signature::KeyPair as _;
        hex(self.pair.public_key().as_ref())
    }

    /// `report` signed, as the envelope sent.
    ///
    /// # Panics
    ///
    /// Never: a report and an envelope are always JSON.
    #[must_use]
    pub fn seal(&self, report: &Report) -> Vec<u8> {
        use ring::signature::KeyPair as _;
        let text = serde_json::to_string(report).expect("a report is JSON");
        let signature = self.pair.sign(text.as_bytes());
        let envelope = Envelope {
            report: text,
            key: key_id(self.pair.public_key().as_ref()),
            signature: hex(signature.as_ref()),
        };
        serde_json::to_vec(&envelope).expect("an envelope is JSON")
    }
}

/// What a canary sends: its report, as the exact text it signed, the ID
/// of its key, and the signature.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Envelope {
    /// The report's JSON, as signed.
    pub report: String,
    /// [`key_id`] of the key that signed it.
    pub key: String,
    /// The Ed25519 signature of `report`'s bytes, in hex.
    pub signature: String,
}

/// A canary's state, as it reports it each hour.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Report {
    /// [`REPORT_TYPE`].
    #[serde(rename = "type")]
    pub kind: String,
    /// [`REPORT_VERSION`].
    pub version: u32,
    /// The canary's name on the list.
    pub canary: String,
    /// When it was made, in seconds since 1970.
    pub time: i64,
    /// The release it runs.
    pub running: String,
    /// The channel it follows.
    pub channel: String,
    /// A release staged for its next start, if one is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub staged: Option<String>,
    /// Its last health check, if one has run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub health: Option<Health>,
    /// Its last update, when that update failed and it went back.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fell_back: Option<FellBack>,
    /// What dukovo-check found, or `None` if it did not run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub check: Option<Check>,
    /// The services that have failed.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub services_failed: Vec<String>,
}

/// A health check's result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Health {
    /// Whether it passed.
    pub passed: bool,
    /// When it ran, in seconds since 1970.
    pub time: i64,
    /// What it said.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub detail: String,
}

/// An update that failed: the release tried, and the one gone back to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FellBack {
    /// The release that failed.
    pub tried: String,
    /// The release the machine went back to.
    pub back_to: String,
}

/// What dukovo-check found.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Check {
    /// How many checks failed.
    pub failures: u32,
    /// How many warned.
    pub warnings: u32,
    /// The checks that failed, in words, at most [`LIST_MAX`].
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub failed: Vec<String>,
}

impl Report {
    /// A report of canary `canary` at `time`, running `running` on
    /// `channel`, with nothing else said yet.
    #[must_use]
    pub fn new(canary: &str, time: i64, running: &str, channel: &str) -> Report {
        Report {
            kind: REPORT_TYPE.to_string(),
            version: REPORT_VERSION,
            canary: canary.to_string(),
            time,
            running: running.to_string(),
            channel: channel.to_string(),
            staged: None,
            health: None,
            fell_back: None,
            check: None,
            services_failed: Vec::new(),
        }
    }

    /// Whether every field is within its bounds.
    fn bounded(&self) -> bool {
        let text = |field: &str| field.len() <= FIELD_MAX;
        let list = |items: &[String]| items.len() <= LIST_MAX && items.iter().all(|i| text(i));
        text(&self.running)
            && text(&self.channel)
            && self.staged.as_deref().is_none_or(text)
            && self.health.as_ref().is_none_or(|h| text(&h.detail))
            && self.fell_back.as_ref().is_none_or(|f| text(&f.tried) && text(&f.back_to))
            && self.check.as_ref().is_none_or(|c| list(&c.failed))
            && list(&self.services_failed)
    }
}

/// A manual colour that wins over a canary's reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Colour {
    /// Holds the waves.
    Red,
    /// Holds nothing, whatever the reports say.
    Green,
}

/// A colour set by hand, why, and when.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Override {
    /// The colour.
    pub colour: Colour,
    /// Why.
    pub reason: String,
    /// When, in seconds since 1970.
    pub set: i64,
}

/// A canary on the list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Canary {
    /// Its public key, 32 bytes in hex.
    pub key: String,
    /// When it was added, in seconds since 1970.
    pub added: i64,
    /// A colour set by hand, if one is.
    #[serde(default, rename = "override", skip_serializing_if = "Option::is_none")]
    pub over: Option<Override>,
}

/// The canaries the update server listens to, by name: root's file on
/// the update server, which the receiver only reads.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Canaries {
    /// Each canary, by name.
    #[serde(default)]
    pub canaries: BTreeMap<String, Canary>,
}

impl Canaries {
    /// The list kept at `path`, or an empty one if there is no file.
    ///
    /// # Errors
    ///
    /// If the file cannot be read or is not a list of canaries, or a
    /// name or key on it is not valid.
    pub fn load(path: &Path) -> Result<Canaries, String> {
        let canaries: Canaries = match std::fs::read(path) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .map_err(|err| format!("{}: {err}", path.display()))?,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Canaries::default(),
            Err(err) => return Err(format!("{}: {err}", path.display())),
        };
        for (name, canary) in &canaries.canaries {
            if !valid_name(name) {
                return Err(format!("{}: `{name}` is not a canary's name", path.display()));
            }
            if public_key(&canary.key).is_none() {
                return Err(format!("{}: {name}'s key is not 32 bytes in hex", path.display()));
            }
        }
        Ok(canaries)
    }

    /// Keeps the list at `path`, whole or not at all, readable by all.
    ///
    /// # Errors
    ///
    /// If it cannot be written.
    ///
    /// # Panics
    ///
    /// Never: the list always serialises.
    pub fn save(&self, path: &Path) -> Result<(), String> {
        let mut bytes = serde_json::to_vec_pretty(self).expect("canaries serialise");
        bytes.push(b'\n');
        write_whole(path, &bytes, 0o644).map_err(|err| format!("{}: {err}", path.display()))
    }

    /// Adds canary `name` with public key `key`, in hex.
    ///
    /// # Errors
    ///
    /// If the name or key is not valid, or either is on the list.
    pub fn add(&mut self, name: &str, key: &str, now: i64) -> Result<(), String> {
        if !valid_name(name) {
            return Err(format!(
                "`{name}` is not a canary's name: use 1 to 32 of a-z, 0-9 and -, not first -"
            ));
        }
        let key = key.to_ascii_lowercase();
        if public_key(&key).is_none() {
            return Err("the key is not 32 bytes in hex, as `sysupdate canary enrol` prints".into());
        }
        if self.canaries.contains_key(name) {
            return Err(format!("{name} is on the list already: remove it first"));
        }
        if let Some((other, _)) = self.canaries.iter().find(|(_, canary)| canary.key == key) {
            return Err(format!("that key is {other}'s already"));
        }
        self.canaries.insert(name.to_string(), Canary { key, added: now, over: None });
        Ok(())
    }

    /// Takes canary `name` off the list.
    ///
    /// # Errors
    ///
    /// If it is not on the list.
    pub fn remove(&mut self, name: &str) -> Result<(), String> {
        self.canaries.remove(name).map(|_| ()).ok_or_else(|| format!("no canary {name}"))
    }

    /// Sets canary `name`'s colour by hand, or, with `None`, goes back to
    /// its reports.
    ///
    /// # Errors
    ///
    /// If it is not on the list, or a colour is set with no reason.
    pub fn set_override(
        &mut self,
        name: &str,
        colour: Option<(Colour, &str)>,
        now: i64,
    ) -> Result<(), String> {
        let canary = self.canaries.get_mut(name).ok_or_else(|| format!("no canary {name}"))?;
        canary.over = match colour {
            Some((_, reason)) if reason.trim().is_empty() => {
                return Err("say why, so that the reason is on record".into());
            }
            Some((colour, reason)) => {
                Some(Override { colour, reason: reason.trim().to_string(), set: now })
            }
            None => None,
        };
        Ok(())
    }

    /// The canary whose key has ID `id`, and its key.
    fn by_key_id(&self, id: &str) -> Option<(&str, Vec<u8>)> {
        self.canaries.iter().find_map(|(name, canary)| {
            let key = public_key(&canary.key)?;
            (key_id(&key) == id).then_some((name.as_str(), key))
        })
    }
}

/// `key`, 32 bytes in hex, as bytes.
fn public_key(key: &str) -> Option<Vec<u8>> {
    unhex(key).filter(|bytes| bytes.len() == 32)
}

/// Why a report was not taken, as the receiver answers it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    /// Larger than [`REPORT_MAX`]: 413.
    TooLarge,
    /// Not an envelope, or not a report: 400.
    Malformed(String),
    /// Signed by a key not on the list: 403.
    UnknownKey,
    /// The signature does not verify: 403.
    BadSignature,
    /// Signed by one canary's key in another's name: 403.
    WrongName,
    /// Made more than [`AHEAD_LIMIT`] ahead of the server's clock: 400.
    Ahead,
    /// Not newer than the report kept: 409.
    NotNewer,
}

impl Refusal {
    /// The HTTP status the receiver answers with.
    #[must_use]
    pub fn status(&self) -> u16 {
        match self {
            Refusal::TooLarge => 413,
            Refusal::Malformed(_) | Refusal::Ahead => 400,
            Refusal::UnknownKey | Refusal::BadSignature | Refusal::WrongName => 403,
            Refusal::NotNewer => 409,
        }
    }
}

impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Refusal::TooLarge => write!(f, "a report is at most {REPORT_MAX} bytes"),
            Refusal::Malformed(why) => write!(f, "not a canary's report: {why}"),
            Refusal::UnknownKey => f.write_str("no canary on the list has that key"),
            Refusal::BadSignature => f.write_str("the signature does not verify"),
            Refusal::WrongName => f.write_str("the report names another canary than its key's"),
            Refusal::Ahead => f.write_str("the report's time is ahead of the server's clock"),
            Refusal::NotNewer => f.write_str("a report as new or newer is kept already"),
        }
    }
}

/// A report whose signature verified against the list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Verified {
    /// The canary's name.
    pub name: String,
    /// What it reported.
    pub report: Report,
}

/// `envelope` checked against `canaries` at `now`: its size, its key,
/// its signature, and then what it says. Nothing in the report is read
/// until its signature has verified.
///
/// # Errors
///
/// The [`Refusal`].
pub fn verify(envelope: &[u8], canaries: &Canaries, now: i64) -> Result<Verified, Refusal> {
    if envelope.len() > REPORT_MAX {
        return Err(Refusal::TooLarge);
    }
    let envelope: Envelope = serde_json::from_slice(envelope)
        .map_err(|_| Refusal::Malformed("not an envelope".into()))?;
    let (name, key) = canaries.by_key_id(&envelope.key).ok_or(Refusal::UnknownKey)?;
    let signature = unhex(&envelope.signature).ok_or(Refusal::BadSignature)?;
    ring::signature::UnparsedPublicKey::new(&ring::signature::ED25519, &key)
        .verify(envelope.report.as_bytes(), &signature)
        .map_err(|_| Refusal::BadSignature)?;
    let report: Report = serde_json::from_str(&envelope.report)
        .map_err(|err| Refusal::Malformed(format!("the report: {err}")))?;
    if report.kind != REPORT_TYPE || report.version != REPORT_VERSION {
        return Err(Refusal::Malformed(format!(
            "a report of type {} version {}, not {REPORT_TYPE} version {REPORT_VERSION}",
            report.kind, report.version
        )));
    }
    if report.canary != name {
        return Err(Refusal::WrongName);
    }
    if !report.bounded() {
        return Err(Refusal::Malformed("a field is too long, or a list too long".into()));
    }
    if report.time > now.saturating_add(AHEAD_LIMIT) {
        return Err(Refusal::Ahead);
    }
    Ok(Verified { name: name.to_string(), report })
}

/// Where canary `name`'s newest report is kept in `reports`.
#[must_use]
pub fn report_path(reports: &Path, name: &str) -> std::path::PathBuf {
    reports.join(format!("{name}.json"))
}

/// Keeps `envelope`, which [`verify`] took as `verified`, as its
/// canary's newest report in `reports`, if it is newer than the one
/// kept: whole or not at all, and readable by the update server's other
/// users.
///
/// # Errors
///
/// [`Refusal::NotNewer`], or why it could not be written.
pub fn keep(
    reports: &Path,
    envelope: &[u8],
    verified: &Verified,
    canaries: &Canaries,
) -> Result<(), KeepError> {
    let path = report_path(reports, &verified.name);
    if let Ok(kept) = std::fs::read(&path) {
        // Checked again, with no clock limit, so that a file that is not
        // a report of this canary's key never blocks a good one.
        if let Ok(kept) = verify(&kept, canaries, i64::MAX)
            && kept.name == verified.name
            && kept.report.time >= verified.report.time
        {
            return Err(KeepError::Refused(Refusal::NotNewer));
        }
    }
    write_whole(&path, envelope, 0o644)
        .map_err(|err| KeepError::Failed(format!("{}: {err}", path.display())))
}

/// Why [`keep`] kept nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeepError {
    /// The report is refused.
    Refused(Refusal),
    /// The disk would not take it.
    Failed(String),
}

/// Writes `bytes` to `path` through a file beside it with `mode`, then
/// renames it into place, so a reader sees the old file or the new one.
fn write_whole(path: &Path, bytes: &[u8], mode: u32) -> std::io::Result<()> {
    use std::os::unix::fs::{OpenOptionsExt as _, PermissionsExt as _};
    let partial = path.with_extension("partial");
    let _ = std::fs::remove_file(&partial);
    let mut file =
        std::fs::OpenOptions::new().write(true).create_new(true).mode(mode).open(&partial)?;
    file.set_permissions(std::fs::Permissions::from_mode(mode))?;
    file.write_all(bytes)?;
    file.sync_all()?;
    drop(file);
    std::fs::rename(&partial, path)
}

/// A release a channel offers, and since when, for the rule on canaries
/// that do not take it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Offer {
    /// The channel.
    pub channel: String,
    /// The release it offers.
    pub version: String,
    /// When a channel first offered it, in seconds since 1970.
    pub since: i64,
}

/// What the update server makes of a canary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Standing {
    /// Its name.
    pub name: String,
    /// Whether it holds nothing.
    pub green: bool,
    /// Why it is not green, or, when it is green by hand, why.
    pub reasons: Vec<String>,
    /// When its newest report that verified was made.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reported: Option<i64>,
    /// That report.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub report: Option<Report>,
    /// A colour set by hand.
    #[serde(rename = "override", skip_serializing_if = "Option::is_none")]
    pub over: Option<Override>,
    /// When it was added.
    pub added: i64,
}

/// Each canary on `canaries`, as its newest report in `reports` and the
/// releases `offered` show it at `now`. A report is checked again
/// against the list before anything in it counts.
#[must_use]
pub fn evaluate(canaries: &Canaries, reports: &Path, offered: &[Offer], now: i64) -> Vec<Standing> {
    canaries
        .canaries
        .iter()
        .map(|(name, canary)| {
            let kept = match std::fs::read(report_path(reports, name)) {
                Err(_) => Kept::Nothing,
                Ok(bytes) => match verify(&bytes, canaries, i64::MAX) {
                    Ok(verified) if verified.name == *name => {
                        Kept::Report(Box::new(verified.report))
                    }
                    Ok(_) | Err(_) => Kept::Unverified,
                },
            };
            judge(name, canary, &kept, offered, now)
        })
        .collect()
}

/// What the update server keeps for a canary.
enum Kept {
    /// No report.
    Nothing,
    /// A file that is not a report of this canary's key.
    Unverified,
    /// Its newest report, verified.
    Report(Box<Report>),
}

/// One canary's standing, from what is `kept` for it.
fn judge(name: &str, canary: &Canary, kept: &Kept, offered: &[Offer], now: i64) -> Standing {
    let report = match kept {
        Kept::Report(report) => Some(&**report),
        Kept::Nothing | Kept::Unverified => None,
    };
    let mut standing = Standing {
        name: name.to_string(),
        green: false,
        reasons: Vec::new(),
        reported: report.map(|r| r.time),
        report: report.cloned(),
        over: canary.over.clone(),
        added: canary.added,
    };
    if let Some(over) = &canary.over {
        standing.green = over.colour == Colour::Green;
        let by_hand = if standing.green { "green by hand" } else { "red by hand" };
        standing.reasons.push(format!("{by_hand}: {}", over.reason));
        return standing;
    }
    let Some(report) = report else {
        standing.reasons.push(match kept {
            Kept::Unverified => "its kept report does not verify".to_string(),
            Kept::Nothing | Kept::Report(_) => "no report since it was added".to_string(),
        });
        return standing;
    };
    let mut reasons = Vec::new();
    if now.saturating_sub(report.time) > SILENT_AFTER {
        reasons.push(format!("silent since {}", tuf::time::format(report.time)));
    }
    if let Some(health) = report.health.as_ref().filter(|h| !h.passed) {
        reasons.push(if health.detail.is_empty() {
            "its health check failed".to_string()
        } else {
            format!("its health check failed: {}", health.detail)
        });
    }
    if let Some(fell) = &report.fell_back {
        reasons.push(format!("went back to {} after {} failed", fell.back_to, fell.tried));
    }
    match &report.check {
        None => reasons.push("dukovo-check did not run".to_string()),
        Some(check) if check.failures > 0 => {
            let first = check.failed.first().map_or_else(String::new, |f| format!(", first: {f}"));
            reasons.push(format!("dukovo-check found {} failures{first}", check.failures));
        }
        Some(_) => {}
    }
    if !report.services_failed.is_empty() {
        reasons.push(format!("failed services: {}", report.services_failed.join(", ")));
    }
    if let Some(offer) = offered.iter().find(|offer| offer.channel == report.channel)
        && older(&report.running, &offer.version)
        && now.saturating_sub(offer.since) > BEHIND_AFTER
    {
        let hours = now.saturating_sub(offer.since) / 3600;
        reasons.push(format!("has not taken Dukovo {} in {hours} hours", offer.version));
    }
    standing.green = reasons.is_empty();
    standing.reasons = reasons;
    standing
}

/// Whether release `running` is older than `offered`, as an update
/// compares them; a version that does not parse counts as older.
fn older(running: &str, offered: &str) -> bool {
    match (slots::version::Version::parse(running), slots::version::Version::parse(offered)) {
        (Some(running), Some(offered)) => running < offered,
        (None, Some(_)) => true,
        _ => false,
    }
}

/// The names of the canaries in `standings` that are not green, in
/// order: what the plans' `red` should be.
#[must_use]
pub fn red(standings: &[Standing]) -> Vec<String> {
    standings.iter().filter(|s| !s.green).map(|s| s.name.clone()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::list::tests::scratch;

    const NOW: i64 = 1_800_000_000;

    fn key() -> Key {
        Key::from_pkcs8(&Key::generate().unwrap()).unwrap()
    }

    fn listed(names: &[(&str, &Key)]) -> Canaries {
        let mut canaries = Canaries::default();
        for (name, key) in names {
            canaries.add(name, &key.public_hex(), NOW - 86_400).unwrap();
        }
        canaries
    }

    fn healthy(name: &str, time: i64) -> Report {
        let mut report = Report::new(name, time, "0.16.0", "beta");
        report.health = Some(Health { passed: true, time, detail: String::new() });
        report.check = Some(Check { failures: 0, warnings: 1, failed: Vec::new() });
        report
    }

    /// `envelope` with its report's text changed by `change`, its
    /// signature left as it was.
    fn tampered(envelope: &[u8], change: impl Fn(&str) -> String) -> Vec<u8> {
        let mut envelope: Envelope = serde_json::from_slice(envelope).unwrap();
        envelope.report = change(&envelope.report);
        serde_json::to_vec(&envelope).unwrap()
    }

    #[test]
    fn a_sealed_report_verifies_and_reads_back() {
        let ref_box = key();
        let canaries = listed(&[("ref-box", &ref_box)]);
        let report = healthy("ref-box", NOW);
        let verified = verify(&ref_box.seal(&report), &canaries, NOW).unwrap();
        assert_eq!(verified, Verified { name: "ref-box".into(), report });
    }

    #[test]
    fn a_report_is_refused_unless_its_key_signature_name_and_time_hold() {
        let ref_box = key();
        let other = key();
        let canaries = listed(&[("ref-box", &ref_box), ("do-nyc", &other)]);
        let sealed = ref_box.seal(&healthy("ref-box", NOW));

        let stranger = key().seal(&healthy("ref-box", NOW));
        assert_eq!(verify(&stranger, &canaries, NOW), Err(Refusal::UnknownKey));

        let changed = tampered(&sealed, |text| text.replace("0.16.0", "0.16.9"));
        assert_eq!(verify(&changed, &canaries, NOW), Err(Refusal::BadSignature));

        let posing = other.seal(&healthy("ref-box", NOW));
        assert_eq!(verify(&posing, &canaries, NOW), Err(Refusal::WrongName));

        let ahead = ref_box.seal(&healthy("ref-box", NOW + AHEAD_LIMIT + 1));
        assert_eq!(verify(&ahead, &canaries, NOW), Err(Refusal::Ahead));
        let just = ref_box.seal(&healthy("ref-box", NOW + AHEAD_LIMIT));
        assert!(verify(&just, &canaries, NOW).is_ok());

        let mut wrong_type = healthy("ref-box", NOW);
        wrong_type.kind = "something-else".into();
        assert!(matches!(
            verify(&ref_box.seal(&wrong_type), &canaries, NOW),
            Err(Refusal::Malformed(_))
        ));
        let mut newer = healthy("ref-box", NOW);
        newer.version = 2;
        assert!(matches!(
            verify(&ref_box.seal(&newer), &canaries, NOW),
            Err(Refusal::Malformed(_))
        ));

        let mut long = healthy("ref-box", NOW);
        long.services_failed = vec!["x".repeat(FIELD_MAX + 1)];
        assert!(matches!(verify(&ref_box.seal(&long), &canaries, NOW), Err(Refusal::Malformed(_))));

        assert_eq!(verify(&vec![b' '; REPORT_MAX + 1], &canaries, NOW), Err(Refusal::TooLarge));
        assert!(matches!(verify(b"{}", &canaries, NOW), Err(Refusal::Malformed(_))));
        let mut extra: serde_json::Value = serde_json::from_slice(&sealed).unwrap();
        extra["more"] = serde_json::json!(1);
        let extra = serde_json::to_vec(&extra).unwrap();
        assert!(matches!(verify(&extra, &canaries, NOW), Err(Refusal::Malformed(_))));
    }

    #[test]
    fn the_list_takes_only_valid_unique_names_and_keys() {
        let a = key();
        let mut canaries = Canaries::default();
        canaries.add("ref-box", &a.public_hex().to_ascii_uppercase(), NOW).unwrap();
        assert_eq!(canaries.canaries["ref-box"].key, a.public_hex());
        assert!(canaries.add("ref-box", &key().public_hex(), NOW).is_err());
        assert!(canaries.add("other", &a.public_hex(), NOW).is_err());
        for bad in ["", "-x", "Ref", "a/b", "a.b", &"a".repeat(33)] {
            assert!(canaries.add(bad, &key().public_hex(), NOW).is_err(), "{bad:?}");
        }
        assert!(canaries.add("short", "abcd", NOW).is_err());
        assert!(canaries.add("not-hex", &"zz".repeat(32), NOW).is_err());
        assert!(canaries.set_override("ref-box", Some((Colour::Red, "  ")), NOW).is_err());
        canaries.set_override("ref-box", Some((Colour::Red, "disk swap")), NOW).unwrap();
        assert_eq!(canaries.canaries["ref-box"].over.as_ref().unwrap().reason, "disk swap");
        canaries.set_override("ref-box", None, NOW).unwrap();
        assert!(canaries.canaries["ref-box"].over.is_none());
        assert!(canaries.set_override("nobody", None, NOW).is_err());
        canaries.remove("ref-box").unwrap();
        assert!(canaries.remove("ref-box").is_err());

        let dir = scratch("canary-list");
        let path = dir.join("canaries.json");
        assert_eq!(Canaries::load(&path).unwrap(), Canaries::default());
        let saved = listed(&[("ref-box", &a)]);
        saved.save(&path).unwrap();
        assert_eq!(Canaries::load(&path).unwrap(), saved);
        std::fs::write(&path, r#"{"canaries":{"../x":{"key":"00","added":0}}}"#).unwrap();
        assert!(Canaries::load(&path).is_err());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn only_a_newer_report_replaces_the_one_kept() {
        let ref_box = key();
        let canaries = listed(&[("ref-box", &ref_box)]);
        let dir = scratch("canary-keep");
        let keep_at = |time: i64| {
            let sealed = ref_box.seal(&healthy("ref-box", time));
            let verified = verify(&sealed, &canaries, NOW).unwrap();
            keep(&dir, &sealed, &verified, &canaries)
        };
        keep_at(NOW - 60).unwrap();
        assert_eq!(keep_at(NOW - 60), Err(KeepError::Refused(Refusal::NotNewer)));
        assert_eq!(keep_at(NOW - 120), Err(KeepError::Refused(Refusal::NotNewer)));
        keep_at(NOW).unwrap();
        // A file that is not this canary's report never blocks a good one.
        std::fs::write(report_path(&dir, "ref-box"), b"junk").unwrap();
        keep_at(NOW - 3600).unwrap();
        use std::os::unix::fs::PermissionsExt as _;
        let mode = std::fs::metadata(report_path(&dir, "ref-box")).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o644);
        let _ = std::fs::remove_dir_all(dir);
    }

    /// The standing of canary `ref-box` given `report`, kept, and `offered`.
    fn standing(report: Option<&Report>, offered: &[Offer], over: Option<Colour>) -> Standing {
        let ref_box = key();
        let mut canaries = listed(&[("ref-box", &ref_box)]);
        if let Some(colour) = over {
            canaries.set_override("ref-box", Some((colour, "by hand")), NOW).unwrap();
        }
        let dir = scratch(&format!("canary-judge-{}", rand_suffix()));
        if let Some(report) = report {
            std::fs::write(report_path(&dir, "ref-box"), ref_box.seal(report)).unwrap();
        }
        let standings = evaluate(&canaries, &dir, offered, NOW);
        let _ = std::fs::remove_dir_all(dir);
        assert_eq!(standings.len(), 1);
        standings.into_iter().next().unwrap()
    }

    fn rand_suffix() -> u32 {
        use std::sync::atomic::{AtomicU32, Ordering};
        static NEXT: AtomicU32 = AtomicU32::new(0);
        NEXT.fetch_add(1, Ordering::Relaxed)
    }

    #[test]
    fn a_fresh_healthy_report_is_green() {
        let report = healthy("ref-box", NOW - 600);
        let standing = standing(Some(&report), &[], None);
        assert!(standing.green, "{:?}", standing.reasons);
        assert_eq!(standing.reported, Some(NOW - 600));
        assert_eq!(red(&[standing]), Vec::<String>::new());
    }

    #[test]
    fn each_failing_rule_says_why_the_canary_is_not_green() {
        let reasons = |report: &Report, offered: &[Offer]| {
            let standing = standing(Some(report), offered, None);
            assert!(!standing.green);
            standing.reasons
        };
        assert_eq!(
            standing(None, &[], None).reasons,
            vec!["no report since it was added".to_string()]
        );
        let silent = healthy("ref-box", NOW - SILENT_AFTER - 1);
        assert!(reasons(&silent, &[])[0].starts_with("silent since 2027-01-15T"));
        let mut sick = healthy("ref-box", NOW);
        sick.health = Some(Health { passed: false, time: NOW, detail: "netd failed".into() });
        assert_eq!(reasons(&sick, &[]), vec!["its health check failed: netd failed"]);
        let mut fell = healthy("ref-box", NOW);
        fell.fell_back = Some(FellBack { tried: "0.16.1".into(), back_to: "0.16.0".into() });
        assert_eq!(reasons(&fell, &[]), vec!["went back to 0.16.0 after 0.16.1 failed"]);
        let mut unchecked = healthy("ref-box", NOW);
        unchecked.check = None;
        assert_eq!(reasons(&unchecked, &[]), vec!["dukovo-check did not run"]);
        let mut failing = healthy("ref-box", NOW);
        failing.check =
            Some(Check { failures: 2, warnings: 0, failed: vec!["disk: smart: worn".into()] });
        assert_eq!(
            reasons(&failing, &[]),
            vec!["dukovo-check found 2 failures, first: disk: smart: worn"]
        );
        let mut services = healthy("ref-box", NOW);
        services.services_failed = vec!["nginx".into(), "sshd".into()];
        assert_eq!(reasons(&services, &[]), vec!["failed services: nginx, sshd"]);
    }

    #[test]
    fn a_canary_that_does_not_take_its_channels_release_in_six_hours_is_not_green() {
        let report = healthy("ref-box", NOW);
        let offer = |version: &str, since: i64| Offer {
            channel: "beta".into(),
            version: version.into(),
            since,
        };
        // Taken, or not offered long enough, or on another channel: green.
        assert!(standing(Some(&report), &[offer("0.16.0", NOW - 86_400)], None).green);
        assert!(standing(Some(&report), &[offer("0.16.1", NOW - BEHIND_AFTER)], None).green);
        let stable = Offer { channel: "stable".into(), ..offer("0.17.0", NOW - 86_400) };
        assert!(standing(Some(&report), &[stable], None).green);
        // Running a newer one than offered: green.
        assert!(standing(Some(&report), &[offer("0.15.9", NOW - 86_400)], None).green);
        let stuck = standing(Some(&report), &[offer("0.16.1", NOW - BEHIND_AFTER - 1)], None);
        assert_eq!(stuck.reasons, vec!["has not taken Dukovo 0.16.1 in 6 hours"]);
    }

    #[test]
    fn a_colour_set_by_hand_wins_until_it_is_taken_away() {
        let mut sick = healthy("ref-box", NOW);
        sick.health = Some(Health { passed: false, time: NOW, detail: String::new() });
        let green = standing(Some(&sick), &[], Some(Colour::Green));
        assert!(green.green);
        assert_eq!(green.reasons, vec!["green by hand: by hand"]);
        let red = standing(Some(&healthy("ref-box", NOW)), &[], Some(Colour::Red));
        assert!(!red.green);
        assert_eq!(red.reasons, vec!["red by hand: by hand"]);
    }

    #[test]
    fn a_kept_file_that_does_not_verify_counts_for_nothing() {
        let ref_box = key();
        let canaries = listed(&[("ref-box", &ref_box)]);
        let dir = scratch("canary-unverified");
        // Signed by a key since taken off the list.
        let old = key().seal(&healthy("ref-box", NOW));
        std::fs::write(report_path(&dir, "ref-box"), old).unwrap();
        let standings = evaluate(&canaries, &dir, &[], NOW);
        assert!(!standings[0].green);
        assert_eq!(standings[0].reasons, vec!["its kept report does not verify"]);
        assert_eq!(standings[0].report, None);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn no_canaries_hold_nothing() {
        let dir = scratch("canary-none");
        assert!(evaluate(&Canaries::default(), &dir, &[], NOW).is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }
}
