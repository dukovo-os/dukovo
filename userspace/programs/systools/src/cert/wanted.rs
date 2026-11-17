// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The certificates that could not be got yet, which certd tries again
//! (ADR 0513).
//!
//! An order that `cert get` makes by HTTP, and that fails after it began, is
//! kept so that certd asks the authority again by itself: not one made with
//! `--dry-run`, `--check`, `--staging` or `--dns`, which a person must
//! watch. Beside the store of certificates:
//!
//! ```text
//! wanted/                      root's folder, 0700
//!   shop.toml                  what was asked and how it went, 0600
//! wanted.json                  what anyone may read of them, 0644
//! ```
//!
//! A record holds the name of the account's key file at the authority, which
//! only root reads, so `cert wanted` shows the summary certd and `cert` write
//! beside the folder, as `cert list` shows the summary of the accounts, and
//! leaves that name out ([`Public`]). An order that failed before the
//! authority was asked has no account yet: when the person agreed to the
//! terms with `--agree-terms`, the record says so and certd makes the account
//! with the contact address it holds, and when not, nothing is kept. A record that does not read is left as
//! it is and is told of by whoever finds it; nothing acts on it, and `cert
//! forget` removes it.
//!
//! Every function that reads or changes a record takes the store's lock,
//! which the caller holds, as the store's own do: a record and the summary are
//! never changed by two at once, and a name is checked by the rule of
//! certificate names before it becomes a file's.
//!
//! The first try is when `cert get` fails; the next is an hour later, and
//! so on for a day, then every 6 hours, until 14 days after the first try,
//! when the record is kept as given up and tried no more ([`next_try`]). An
//! authority that asks for a wait, as a rate limit does, is waited for.
//!
//! When it misbehaves, `cert wanted` shows what each record says, `log certd`
//! what certd did with it, and the `admin-certificates` boot test covers it.

use std::fs;
use std::io;
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::path::{Path, PathBuf};

use manifest::service;
use serde::{Deserialize, Serialize};
use tuf::url::Url;

use super::acme;
use super::history::By;
use super::store::{self, KeyKind, Kind, Lock, Store};

/// An hour, in seconds.
const HOUR: i64 = 3_600;

/// How long after the first try a certificate is tried every hour, and how
/// long it must have failed for before an alert says so.
pub const FIRST_DAY: i64 = 24 * HOUR;

/// The time between tries once the first day is over.
const LATER_EVERY: i64 = 6 * HOUR;

/// How long after the first try certd keeps trying.
const GIVE_UP_AFTER: i64 = 14 * FIRST_DAY;

/// The version of `wanted.json`, and of what `cert wanted --json` tells.
pub const FORMAT: u32 = 1;

/// The seconds between the tries of the first day: an hour, and in a lab
/// build the `retry_s` of the lab's settings file when it has one, so that a
/// test of a retry does not wait an hour.
fn every_hour() -> i64 {
    lab_retry().unwrap_or(HOUR)
}

/// The lab's own settings of certd, which only a lab build reads.
#[cfg(feature = "lab")]
const LAB_SETTINGS: &str = "/system/settings/certd-lab.toml";

/// The `retry_s` of the lab's settings, when the file is there, reads, and
/// asks for 1 second or more.
#[cfg(feature = "lab")]
fn lab_retry() -> Option<i64> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Lab {
        retry_s: i64,
    }
    let text = fs::read_to_string(LAB_SETTINGS).ok()?;
    let lab: Lab = toml::from_str(&text).ok()?;
    (lab.retry_s >= 1).then_some(lab.retry_s)
}

/// A build that is not the lab's has no settings of the lab's to read.
#[cfg(not(feature = "lab"))]
fn lab_retry() -> Option<i64> {
    None
}

/// What a wanted certificate is asked for: what `cert get` was asked, and the
/// account it was asked through.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Order {
    /// The certificate's name in the store.
    pub name: String,
    /// The DNS names, the first the common name.
    pub names: Vec<String>,
    /// The authority's directory address.
    pub directory: String,
    pub key: KeyKind,
    pub profile: Option<String>,
    /// The account's key file in the accounts folder, which only root
    /// reads.
    pub account: String,
    /// The contact address a new account is made with, if one was given.
    pub email: Option<String>,
    /// Whether the person agreed to the authority's terms of service when they
    /// ordered, so that certd may make the account itself when there is none
    /// yet: an order that failed in the checks before the authority was asked
    /// has made none.
    pub agree_terms: bool,
}

/// A certificate that could not be got yet, and how its tries went.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Record {
    pub order: Order,
    /// The first try, in milliseconds since 1970.
    pub first_ms: i64,
    /// The last try, or the day it was given up.
    pub last_ms: i64,
    /// The next try, in milliseconds since 1970.
    pub next_ms: i64,
    /// How many tries have failed.
    pub tries: u32,
    /// What the last try said, as the history says it.
    pub why: String,
    /// Who made the last try: `cert` the first, certd the others.
    pub by: By,
    /// Whether certd tries no more.
    pub given_up: bool,
}

/// A record whose file does not read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Unreadable {
    /// The certificate's name, from the file's.
    pub name: String,
    /// Why, with the file's path first.
    pub why: String,
}

/// What `cert wanted` tells of a record: all of it but the account.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Public {
    pub name: String,
    pub names: Vec<String>,
    pub directory: String,
    pub first_ms: i64,
    pub last_ms: i64,
    pub next_ms: i64,
    pub tries: u32,
    pub why: String,
    pub given_up: bool,
}

/// `wanted.json`, which is what `cert wanted --json` tells too.
#[derive(Serialize, Deserialize)]
pub struct Document {
    pub format: u32,
    pub wanted: Vec<Public>,
}

/// Milliseconds since 1970 of `seconds`.
fn ms(seconds: i64) -> i64 {
    seconds.saturating_mul(1_000)
}

/// When the try after the one that failed at `tried` is, in seconds since 1970
/// (as is `first`, the first try), or none when it would come more than 14
/// days after the first. `not_before` is the time the authority asked not to
/// be asked before, if it did: the next try is never sooner than it.
pub fn next_try(first: i64, tried: i64, not_before: Option<i64>) -> Option<i64> {
    let wait = if tried.saturating_sub(first) < FIRST_DAY { every_hour() } else { LATER_EVERY };
    let at = tried.saturating_add(wait).max(not_before.unwrap_or(i64::MIN));
    (at.saturating_sub(first) <= GIVE_UP_AFTER).then_some(at)
}

impl Record {
    /// The record of an order that failed for the first time at `now`, in
    /// seconds since 1970, for `why`.
    pub fn first(order: Order, now: i64, why: &str, not_before: Option<i64>) -> Record {
        let mut record = Record {
            order,
            first_ms: ms(now),
            last_ms: ms(now),
            next_ms: ms(now),
            tries: 1,
            why: why.to_string(),
            by: By::Cert,
            given_up: false,
        };
        record.schedule(now, not_before);
        record
    }

    /// The record after certd's try at `now` failed for `why`.
    pub fn failed_again(&self, now: i64, why: &str, not_before: Option<i64>) -> Record {
        let mut record = Record {
            last_ms: ms(now),
            tries: self.tries.saturating_add(1),
            why: why.to_string(),
            by: By::Certd,
            ..self.clone()
        };
        record.schedule(now, not_before);
        record
    }

    /// The record when certd stops trying at `now` for `why`, whatever the
    /// time.
    pub fn gave_up(&self, now: i64, why: &str) -> Record {
        Record {
            last_ms: ms(now),
            why: why.to_string(),
            by: By::Certd,
            given_up: true,
            ..self.clone()
        }
    }

    /// Sets when the next try is, after one at `now`, or gives the record up.
    fn schedule(&mut self, now: i64, not_before: Option<i64>) {
        match next_try(self.first_ms / 1_000, now, not_before) {
            Some(at) => self.next_ms = ms(at),
            None => {
                self.given_up = true;
                self.why = format!("{} (tried for 14 days)", self.why);
            }
        }
    }

    /// Whether certd is to try it at `now`, in seconds since 1970.
    pub fn is_due(&self, now: i64) -> bool {
        !self.given_up && self.next_ms <= ms(now)
    }

    /// When certd stops trying, in seconds since 1970.
    pub fn gives_up_at(&self) -> i64 {
        (self.first_ms / 1_000).saturating_add(GIVE_UP_AFTER)
    }

    /// Whether it has failed for a day at `now` and is still tried.
    pub fn has_failed_for_a_day(&self, now: i64) -> bool {
        !self.given_up && ms(now) - self.first_ms >= ms(FIRST_DAY)
    }

    /// What anyone may know of it.
    pub fn public(&self) -> Public {
        Public {
            name: self.order.name.clone(),
            names: self.order.names.clone(),
            directory: self.order.directory.clone(),
            first_ms: self.first_ms,
            last_ms: self.last_ms,
            next_ms: self.next_ms,
            tries: self.tries,
            why: self.why.clone(),
            given_up: self.given_up,
        }
    }
}

/// A record's file, as TOML reads it.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct File {
    name: String,
    names: Vec<String>,
    directory: String,
    key: String,
    profile: Option<String>,
    account: String,
    email: Option<String>,
    agree_terms: Option<bool>,
    first_ms: i64,
    last_ms: i64,
    next_ms: i64,
    tries: u32,
    why: String,
    by: String,
    given_up: Option<bool>,
}

/// The record that file `stem`.toml holds as `text`.
fn parse(stem: &str, text: &str) -> Result<Record, String> {
    let file: File = toml::from_str(text).map_err(|err| err.message().to_string())?;
    if file.name != stem {
        return Err(format!(
            "it is for `{}`, which is not its file's name",
            file.name.escape_debug()
        ));
    }
    service::check_certificate(stem)?;
    acme::check_names(&file.names, false)?;
    let key = acme::key_kind(&file.key)?;
    let by = match file.by.as_str() {
        "cert" => By::Cert,
        "certd" => By::Certd,
        other => return Err(format!("`by` is `{}`, not cert or certd", other.escape_debug())),
    };
    let directory = Url::parse(&file.directory)
        .map_err(|why| format!("`directory` is not an address: {why}"))?;
    if acme::account_file(&directory)? != file.account {
        return Err(String::from("`account` is not the file kept for `directory`"));
    }
    if let Some(email) = &file.email {
        acme::check_email(email)?;
    }
    if file.tries == 0 {
        return Err(String::from("`tries` is 0: a record is made by a try that failed"));
    }
    let order = Order {
        name: file.name,
        names: file.names,
        directory: file.directory,
        key,
        profile: file.profile,
        account: file.account,
        email: file.email,
        agree_terms: file.agree_terms.unwrap_or(false),
    };
    Ok(Record {
        order,
        first_ms: file.first_ms,
        last_ms: file.last_ms,
        next_ms: file.next_ms,
        tries: file.tries,
        why: file.why,
        by,
        given_up: file.given_up.unwrap_or(false),
    })
}

/// The text of a record's file.
fn text_of(record: &Record) -> String {
    let order = &record.order;
    let names: Vec<String> = order.names.iter().map(|name| store::quoted(name)).collect();
    let mut text = format!(
        "name = {}\nnames = [{}]\ndirectory = {}\nkey = \"{}\"\n",
        store::quoted(&order.name),
        names.join(", "),
        store::quoted(&order.directory),
        order.key.word()
    );
    if let Some(profile) = &order.profile {
        text.push_str(&format!("profile = {}\n", store::quoted(profile)));
    }
    if let Some(email) = &order.email {
        text.push_str(&format!("email = {}\n", store::quoted(email)));
    }
    if order.agree_terms {
        text.push_str("agree_terms = true\n");
    }
    text.push_str(&format!(
        "account = {}\nfirst_ms = {}\nlast_ms = {}\nnext_ms = {}\ntries = {}\nwhy = {}\nby = \"{}\"\n",
        store::quoted(&order.account),
        record.first_ms,
        record.last_ms,
        record.next_ms,
        record.tries,
        store::quoted(&record.why),
        record.by.word()
    ));
    if record.given_up {
        text.push_str("given_up = true\n");
    }
    text
}

/// The path of the file of record `name`, which is checked as a certificate's
/// name first: it is part of a file's name.
fn path_of(store: &Store, name: &str) -> Result<PathBuf, String> {
    service::check_certificate(name)?;
    Ok(store.wanted().join(format!("{name}.toml")))
}

/// Record `name`, or none when there is none.
///
/// # Errors
///
/// With why it cannot be read or does not read as a record.
pub fn get(store: &Store, _lock: &Lock, name: &str) -> Result<Option<Record>, String> {
    let path = path_of(store, name)?;
    let text = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(store::failed(&path, &err)),
    };
    parse(name, &text).map(Some).map_err(|why| format!("{}: {why}", path.display()))
}

/// Every record, by name, each with why it does not read when it does not.
///
/// # Errors
///
/// With why the folder cannot be read.
pub fn list(store: &Store, lock: &Lock) -> Result<Vec<Result<Record, Unreadable>>, String> {
    let folder = store.wanted();
    let entries = match fs::read_dir(&folder) {
        Ok(entries) => entries,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => return Err(store::failed(&folder, &err)),
    };
    let mut names: Vec<String> = entries
        .flatten()
        .filter_map(|entry| entry.file_name().into_string().ok())
        .filter_map(|file| file.strip_suffix(".toml").map(str::to_string))
        .collect();
    names.sort();
    let mut found = Vec::new();
    for name in names {
        match get(store, lock, &name) {
            Ok(Some(record)) => found.push(Ok(record)),
            // Taken away since the folder was read.
            Ok(None) => {}
            Err(why) => found.push(Err(Unreadable { name, why })),
        }
    }
    Ok(found)
}

/// Keeps `record`, in place of the one of its name, and makes the summary
/// say so.
///
/// # Errors
///
/// With why the folder, the file or the summary cannot be written.
pub fn write(store: &Store, lock: &Lock, record: &Record) -> Result<(), String> {
    let path = path_of(store, &record.order.name)?;
    let folder = store.wanted();
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(&folder)
        .map_err(|err| store::failed(&folder, &err))?;
    // The umask may have taken bits off the mode, and an earlier folder may
    // have had others.
    fs::set_permissions(&folder, fs::Permissions::from_mode(0o700))
        .map_err(|err| store::failed(&folder, &err))?;
    replace(&path, 0o600, &text_of(record))?;
    publish(store, lock)
}

/// Takes away record `name`, whatever it holds, and makes the summary say so:
/// whether there was one.
///
/// # Errors
///
/// With why the file cannot be removed or the summary written.
pub fn remove(store: &Store, lock: &Lock, name: &str) -> Result<bool, String> {
    let path = path_of(store, name)?;
    match fs::remove_file(&path) {
        Ok(()) => {}
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(err) => return Err(store::failed(&path, &err)),
    }
    store::sync_dir(&store.wanted())?;
    publish(store, lock)?;
    Ok(true)
}

/// Whether `record` is still the one kept for its name: not taken away or
/// replaced by another order since it was read.
pub fn is_current(store: &Store, lock: &Lock, record: &Record) -> bool {
    get(store, lock, &record.order.name)
        .ok()
        .flatten()
        .is_some_and(|kept| kept.first_ms == record.first_ms)
}

/// Writes the summary anew from the records that read, unless it says so
/// already.
///
/// # Errors
///
/// With why the records cannot be listed or the summary written.
pub fn publish(store: &Store, lock: &Lock) -> Result<(), String> {
    let wanted: Vec<Public> = list(store, lock)?
        .iter()
        .filter_map(|found| found.as_ref().ok())
        .map(Record::public)
        .collect();
    let path = store.wanted_public();
    if wanted.is_empty() && !path.exists() {
        return Ok(());
    }
    let text = serde_json::to_string(&Document { format: FORMAT, wanted })
        .map(|text| text + "\n")
        .map_err(|why| why.to_string())?;
    if fs::read_to_string(&path).is_ok_and(|now| now == text) {
        return Ok(());
    }
    replace(&path, 0o644, &text)
}

/// What the summary tells of the certificates that are wanted: none when
/// there is no summary.
///
/// # Errors
///
/// With why the file cannot be read or is not a summary.
pub fn read_public(store: &Store) -> Result<Vec<Public>, String> {
    let path = store.wanted_public();
    let text = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => return Err(store::failed(&path, &err)),
    };
    let summary: Document =
        serde_json::from_str(&text).map_err(|why| format!("{}: {why}", path.display()))?;
    if summary.format != FORMAT {
        return Err(format!("{}: its format is {}, not {FORMAT}", path.display(), summary.format));
    }
    Ok(summary.wanted)
}

/// Why certd can no longer try `record`, if it cannot: the account it names
/// has been taken away, and the person did not agree to the terms for a new
/// one to be made, or a certificate of its name was added by hand since, which
/// a new one would replace.
pub fn cannot_go_on(store: &Store, record: &Record) -> Option<String> {
    let order = &record.order;
    let name = &order.name;
    // An accounts folder that cannot be read now is not a reason to give up:
    // the try says what it could not do.
    if let Ok(None) = store.account(&order.account)
        && !order.agree_terms
    {
        return Some(format!(
            "the account at {} was taken away: get the certificate again with `elevate cert get \
             {name}`",
            acme::authority_words(&order.directory)
        ));
    }
    match store.get(name) {
        Ok(stored) if stored.kind != Kind::Acme => {
            Some(format!("{name} was added by hand since: remove it first, or choose another name"))
        }
        _ => None,
    }
}

/// Writes `text` to `path` with `mode`, through a new file and a rename, so
/// that a reader sees the old file or the new.
fn replace(path: &Path, mode: u32, text: &str) -> Result<(), String> {
    let mut temporary = path.as_os_str().to_owned();
    temporary.push(".new");
    let temporary = PathBuf::from(temporary);
    // A file a crash left, which `write_new` would refuse to overwrite.
    let _ = fs::remove_file(&temporary);
    store::write_new(&temporary, mode, text)?;
    fs::rename(&temporary, path).map_err(|err| store::failed(path, &err))?;
    match path.parent() {
        Some(folder) => store::sync_dir(folder),
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests;
