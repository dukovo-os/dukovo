// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! backup(8): snapshots kept on a USB disk, a share, a server or
//! S3-compatible storage, encrypted (ADRs 0463 and 0465).
//!
//! A destination is a folder, as a disk's in `/volumes` or a share's, a
//! server's, `ssh://user@host[:port]/path`, or a bucket's,
//! `s3://host[:port]/bucket[/prefix]`; each has a file of settings and a
//! key of its own in `/system/settings/backup` (backup.toml(5)). It keeps
//! either Btrfs's streams encrypted with age (`format = "btrfs"`), or a
//! restic repository (`format = "restic"`, ADR 0465), deduplicated, which
//! Linux's restic reads too, and which S3 storage always keeps. A round
//! makes a read-only snapshot `backup-<time>` of each place, with the
//! services' hooks run around it, and gives each destination the stream
//! of its changes from the snapshot it took before, or a whole one,
//! encrypted to its key and written as files. `backup verify` reads them
//! back, and with `--restore` restores the newest and compares it with
//! the snapshot it came from; `backup restore` receives a backup into the
//! place's `@snapshots`, where `snap restore --from` copies files out.
//!
//! - `backup add NAME TO [--format btrfs|restic] [--every day|week|hand]
//!   [--verify week|month|never] [--recipient KEY]... [--append-only]`
//!   adds a destination and shows its recovery key; S3 storage takes its
//!   access key from AWS_ACCESS_KEY_ID and AWS_SECRET_ACCESS_KEY.
//! - `backup remove NAME`, `backup list`, `backup show NAME`.
//! - `backup run [NAME...]` backs up now; `backup auto [--watch]` backs up
//!   each destination that is due, every hour with `--watch`.
//! - `backup verify [--restore] NAME`.
//! - `backup restore [--key FILE] [--place P] [--at TIME] [--into FOLDER]
//!   [--unchecked] NAME|SOURCE`, and `backup restore --machine [--key
//!   FILE] [--at TIME] [--unchecked] NAME|SOURCE`, which stages the whole
//!   machine for Spark to put in place at the next start (ADR 0464), or
//!   with `--cancel` withdraws what it staged.
//! - `backup key NAME` and `backup ssh-key` show the keys.

mod compare;
mod layout;
mod record;
mod restic;
mod s3;
mod store;
mod tag;

use std::cell::Cell;
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File};
use std::io::{self, BufReader, BufWriter, Read, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::rc::Rc;
use std::str::FromStr;
use std::time::Duration;

use age::secrecy::ExposeSecret;
use serde::{Deserialize, Serialize};

use self::layout::{Listing, Next};
use self::restic::{Keep, Repo};
use self::store::{Address, Folder, Joined, Parts, Ssh, Store};
use self::tag::TagKey;
use crate::alerts::protocol::{Alert, Severity, key_part};
use crate::args::{Arg, Args};
use crate::snap::{self, Place};
use crate::subvolume::{self, is_top};
use crate::util::{self, Fail};

/// The destinations' settings and keys.
const SETTINGS: &str = "/system/settings/backup";

/// What each destination last did.
const STATE: &str = "/system/state/backup";

/// Held by the round running, with its process number.
const LOCK: &str = "/system/live/backup.lock";

/// The machine's key for servers, in [`SETTINGS`].
const SSH_KEY: &str = "ssh_ed25519";

/// The servers' host keys, in [`SETTINGS`].
const KNOWN_HOSTS: &str = "known_hosts";

/// The names of the snapshots backups are made from.
const PREFIX: &str = "backup-";

/// The names of restored backups.
const RESTORED: &str = "restored-";

/// The apps installed for everyone, which backups carry besides snap's
/// places (ADR 0464).
const APPLICATIONS: &str = "/applications";

/// The machine's `/system/state`, whose app registry and grants each
/// round's record carries (ADR 0464).
const SYSTEM_STATE: &str = "/system/state";

/// Where a round writes the machine's record for a restic repository to
/// back up, and removes it.
const RECORD_SCRATCH: &str = "/system/live/backup-record";

/// How often the service looks for a destination that is due.
const ROUND: Duration = Duration::from_hours(1);

const NANOS: u64 = 1_000_000_000;

/// How often a destination is backed up by itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Every {
    Day,
    Week,
    /// Only by `backup run`.
    Hand,
}

/// How often a destination's newest backups are restored to check them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Verify {
    Week,
    Month,
    Never,
}

/// How a destination keeps backups.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Format {
    /// Btrfs's send streams, encrypted with age, in chains (ADR 0463).
    Btrfs,
    /// A restic repository (ADR 0465).
    Restic,
}

/// A destination's file, as written.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct DestinationFile {
    to: String,
    format: Option<String>,
    every: Option<String>,
    verify: Option<String>,
    chain: Option<usize>,
    chains: Option<usize>,
    daily: Option<u32>,
    weekly: Option<u32>,
    monthly: Option<u32>,
    append_only: Option<bool>,
    places: Option<Vec<String>>,
    recipients: Option<Vec<String>>,
}

/// A destination, with the defaults for what its file leaves out.
#[derive(Debug)]
struct Destination {
    name: String,
    /// A folder, an SSH address or an S3 one.
    to: String,
    format: Format,
    every: Every,
    verify: Verify,
    /// The most backups in a chain.
    chain: usize,
    /// The whole chains kept.
    chains: usize,
    /// The restic snapshots kept besides the newest.
    keep: Keep,
    /// Whether nothing is ever removed from a restic repository.
    append_only: bool,
    /// The places backed up; snap's when empty.
    places: Vec<String>,
    /// Those every file is encrypted to besides its own key.
    recipients: Vec<String>,
}

/// Whether `name` can name a destination.
fn may_name(name: &str) -> bool {
    (1..=32).contains(&name.len())
        && name.bytes().all(|b| b.is_ascii_alphanumeric() || b"-_".contains(&b))
        && !name.starts_with('-')
}

impl Destination {
    /// Destination `name`'s settings in `text`.
    fn parse(name: &str, text: &str) -> Result<Destination, String> {
        let file: DestinationFile = toml::from_str(text).map_err(|err| err.to_string())?;
        let every = match file.every.as_deref().unwrap_or("day") {
            "day" => Every::Day,
            "week" => Every::Week,
            "hand" => Every::Hand,
            other => return Err(format!("every is day, week or hand, not '{other}'")),
        };
        let verify = match file.verify.as_deref().unwrap_or("month") {
            "week" => Verify::Week,
            "month" => Verify::Month,
            "never" => Verify::Never,
            other => return Err(format!("verify is week, month or never, not '{other}'")),
        };
        let (chain, chains) = (file.chain.unwrap_or(30), file.chains.unwrap_or(2));
        if chain == 0 || chains == 0 {
            return Err("chain and chains are 1 or more".into());
        }
        let s3 = s3::Address::parse(&file.to)?.is_some();
        if !s3 && Address::parse(&file.to)?.is_none() && !file.to.starts_with('/') {
            return Err(format!("to is a folder from /, ssh:// or s3://, not '{}'", file.to));
        }
        let format = match file.format.as_deref() {
            Some("btrfs") if s3 => {
                return Err("S3 storage keeps only format = \"restic\"".into());
            }
            Some("btrfs") => Format::Btrfs,
            None if !s3 => Format::Btrfs,
            Some("restic") | None => Format::Restic,
            Some(other) => return Err(format!("format is btrfs or restic, not '{other}'")),
        };
        let by_days = [file.daily, file.weekly, file.monthly];
        if format == Format::Btrfs
            && (by_days.iter().any(Option::is_some) || file.append_only.is_some())
        {
            return Err("daily, weekly, monthly and append_only are for format = \"restic\"".into());
        }
        let chained = file.chain.is_some() || file.chains.is_some() || file.recipients.is_some();
        if format == Format::Restic && chained {
            return Err("chain, chains and recipients are for format = \"btrfs\"".into());
        }
        if by_days.iter().flatten().any(|&n| n > 1000) {
            return Err("daily, weekly and monthly are at most 1000".into());
        }
        let defaults = Keep::default();
        let keep = Keep {
            daily: file.daily.unwrap_or(defaults.daily),
            weekly: file.weekly.unwrap_or(defaults.weekly),
            monthly: file.monthly.unwrap_or(defaults.monthly),
        };
        let recipients = file.recipients.unwrap_or_default();
        for recipient in &recipients {
            age::x25519::Recipient::from_str(recipient)
                .map_err(|_| format!("'{recipient}' is not an age key (age1...)"))?;
        }
        let places = file.places.unwrap_or_default();
        if let Some(place) = places.iter().find(|p| !p.starts_with('/')) {
            return Err(format!("places are folders from /, not '{place}'"));
        }
        Ok(Destination {
            name: name.into(),
            to: file.to,
            format,
            every,
            verify,
            chain,
            chains,
            keep,
            append_only: file.append_only.unwrap_or(false),
            places,
            recipients,
        })
    }

    /// Destination `name`'s settings.
    fn read(name: &str) -> Result<Destination, Fail> {
        let path = settings_file(name);
        let text = match fs::read_to_string(&path) {
            Err(err) if err.kind() == io::ErrorKind::NotFound => {
                return Err(util::fail(format!("no destination is called {name}")));
            }
            other => other.map_err(|e| util::failed(&path.display(), e))?,
        };
        Destination::parse(name, &text)
            .map_err(|why| util::fail(format!("{}: {why}", path.display())))
    }

    /// Every destination, by name.
    fn all() -> Result<Vec<Destination>, Fail> {
        let entries = match fs::read_dir(SETTINGS) {
            Ok(entries) => entries,
            Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(err) => return Err(util::failed(&SETTINGS, err)),
        };
        let mut names: Vec<String> = entries
            .filter_map(Result::ok)
            .filter_map(|e| e.file_name().to_str()?.strip_suffix(".toml").map(String::from))
            .filter(|name| may_name(name))
            .collect();
        names.sort();
        names.iter().map(|name| Destination::read(name)).collect()
    }

    /// The places it backs up: those its file names, or snap's and the
    /// apps installed for everyone (ADR 0464).
    fn places(&self) -> Result<Vec<String>, Fail> {
        if self.places.is_empty() {
            let mut places = snap::Settings::read()?.places;
            if !places.iter().any(|p| p == APPLICATIONS) {
                places.push(APPLICATIONS.into());
            }
            return Ok(places);
        }
        Ok(self.places.clone())
    }

    /// Its own key, and the keys every file is encrypted to.
    fn keys(&self) -> Result<(age::x25519::Identity, Vec<age::x25519::Recipient>), Fail> {
        let identity = read_identity(&key_file(&self.name))?;
        let mut recipients = vec![identity.to_public()];
        for text in &self.recipients {
            let key = age::x25519::Recipient::from_str(text)
                .map_err(|_| util::fail(format!("{}: '{text}' is not an age key", self.name)))?;
            recipients.push(key);
        }
        Ok((identity, recipients))
    }

    /// Its store.
    fn store(&self, first_contact: bool) -> Result<Box<dyn Store>, Fail> {
        open_store(&self.to, Some(&machine()), first_contact, Some(&s3_file(&self.name)))
    }

    /// Opens its restic repository. When `adding`, makes it if the folder
    /// has none and marks the folder as its; a round refuses a folder
    /// without the mark ([`unmarked`]).
    fn repo(&self, identity: &age::x25519::Identity, adding: bool) -> Result<Repo, Fail> {
        let mut store = self.store(false)?;
        let public = identity.to_public().to_string();
        let marked = store.size(layout::MARK)?.is_some();
        if !marked && !adding {
            return Err(unmarked(store.as_ref()));
        }
        if marked {
            check_mark(store.as_mut(), true, &public, Format::Restic)?;
        }
        let repo = if Repo::exists(store.as_mut())? {
            Repo::open(self.store(false)?, &password(identity))?
        } else {
            Repo::init(self.store(false)?, &password(identity), self.append_only)?
        };
        if !marked {
            check_mark(store.as_mut(), false, &public, Format::Restic)?;
        }
        Ok(repo)
    }
}

fn settings_file(name: &str) -> PathBuf {
    Path::new(SETTINGS).join(format!("{name}.toml"))
}

fn key_file(name: &str) -> PathBuf {
    Path::new(SETTINGS).join(format!("{name}.key"))
}

/// The file of a destination's S3 access key.
fn s3_file(name: &str) -> PathBuf {
    Path::new(SETTINGS).join(format!("{name}.s3"))
}

/// A restic repository's password: the destination's recovery key, so
/// that the key that reads its age files reads it too.
fn password(identity: &age::x25519::Identity) -> String {
    identity.to_string().expose_secret().to_string()
}

/// The S3 access key in the environment, as Amazon's own tools take it.
fn s3_from_env() -> Result<s3::Credentials, Fail> {
    let var = |name: &str| std::env::var(name).ok().filter(|v| !v.is_empty());
    let (Some(id), Some(secret)) = (var("AWS_ACCESS_KEY_ID"), var("AWS_SECRET_ACCESS_KEY")) else {
        return Err(util::usage(
            "S3 storage needs an access key: set AWS_ACCESS_KEY_ID and AWS_SECRET_ACCESS_KEY, \
             and AWS_REGION if its region is not us-east-1"
                .into(),
        ));
    };
    let region = var("AWS_REGION").or_else(|| var("AWS_DEFAULT_REGION"));
    s3::Credentials::new(id, secret, region).map_err(util::usage)
}

/// The store of `to` for machine `machine`'s folder in it, or, with no
/// machine, the store whose folder `to` is. S3 storage is reached with
/// the access key in the file `s3_keys`, or without one, the
/// environment's.
fn open_store(
    to: &str,
    machine: Option<&str>,
    first_contact: bool,
    s3_keys: Option<&Path>,
) -> Result<Box<dyn Store>, Fail> {
    let settings = Path::new(SETTINGS);
    if let Some(address) = s3::Address::parse(to).map_err(util::fail)? {
        let keys = match s3_keys {
            Some(path) => s3::Credentials::read(path)?,
            None => s3_from_env()?,
        };
        return Ok(Box::new(s3::S3::new(address, keys, machine)));
    }
    match Address::parse(to).map_err(util::fail)? {
        Some(address) => {
            let mut ssh =
                Ssh::new(address, machine, settings.join(SSH_KEY), settings.join(KNOWN_HOSTS));
            if first_contact {
                ssh.trust_on_first_contact();
            }
            Ok(Box::new(ssh))
        }
        None => Ok(Box::new(Folder::new(Path::new(to), machine)?)),
    }
}

/// This machine's name at destinations: its host name, of the letters,
/// digits, `-`, `_` and `.` it has.
fn machine() -> String {
    let name: String = util::hostname()
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || "-_.".contains(*c))
        .collect();
    if name.is_empty() || name.starts_with('.') { "dukovo".into() } else { name }
}

/// Reads an age identity file: its first line that is not a comment.
fn read_identity(path: &Path) -> Result<age::x25519::Identity, Fail> {
    let text = fs::read_to_string(path).map_err(|e| util::failed(&path.display(), e))?;
    let line = text
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty() && !line.starts_with('#'))
        .unwrap_or("");
    age::x25519::Identity::from_str(line).map_err(|_| {
        util::fail(format!("{}: not an age key (AGE-SECRET-KEY-1...)", path.display()))
    })
}

/// What a destination last did, which only backup writes.
#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(default)]
struct State {
    /// The time of the snapshot it last took of each place, by path.
    sent: BTreeMap<String, String>,
    /// When a backup to it last began, and last ended well, in ns.
    tried: u64,
    done: u64,
    /// Why its last backup failed; empty if it did not.
    failed: String,
    /// When it was last verified by a restore, and why that failed.
    verified: u64,
    verify_failed: String,
}

impl State {
    fn path(name: &str) -> PathBuf {
        Path::new(STATE).join(format!("{name}.json"))
    }

    fn read(name: &str) -> State {
        fs::read(State::path(name))
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default()
    }

    fn write(&self, name: &str) -> Result<(), Fail> {
        fs::create_dir_all(STATE).map_err(|e| util::failed(&STATE, e))?;
        let path = State::path(name);
        let new = path.with_extension("json.new");
        let text = serde_json::to_vec_pretty(self).map_err(io::Error::other)?;
        fs::write(&new, text).map_err(|e| util::failed(&new.display(), e))?;
        fs::rename(&new, &path).map_err(|e| util::failed(&path.display(), e))
    }
}

/// The round's lock, removed when it goes.
struct Lock;

impl Lock {
    /// Takes the lock, unless a round still running holds it.
    fn take() -> Result<Lock, Fail> {
        for _ in 0..2 {
            match File::options().write(true).create_new(true).mode(0o600).open(LOCK) {
                Ok(mut file) => {
                    let _ = write!(file, "{}", std::process::id());
                    return Ok(Lock);
                }
                Err(err) if err.kind() == io::ErrorKind::AlreadyExists => {
                    let holder = fs::read_to_string(LOCK).ok().and_then(|t| t.trim().parse().ok());
                    // SAFETY: signal 0 checks that the process is there.
                    let alive = holder.is_some_and(|pid: i32| unsafe { libc::kill(pid, 0) } == 0);
                    if alive {
                        return Err(util::fail("another backup is running".into()));
                    }
                    let _ = fs::remove_file(LOCK);
                }
                Err(err) => return Err(util::failed(&LOCK, err)),
            }
        }
        Err(util::fail("another backup is running".into()))
    }
}

impl Drop for Lock {
    fn drop(&mut self) {
        let _ = fs::remove_file(LOCK);
    }
}

/// The time name of `ns` since 1970, in local time.
fn time_name(ns: u64) -> String {
    let seconds = (ns / NANOS) as i64;
    let (year, month, day, hour, minute) = snap::local(seconds);
    layout::time_name(year, month, day, hour, minute, seconds.rem_euclid(60) as u32)
}

/// The text of a time name, as `2026-11-11 03:00:00`.
fn shown_time(name: &str) -> String {
    if !layout::is_time(name) {
        return name.into();
    }
    format!("{} {}:{}:{}", &name[..10], &name[11..13], &name[13..15], &name[15..17])
}

/// The mark at a destination: its format, machine and key.
fn mark_text(key: &str, format: Format) -> String {
    let about = match format {
        Format::Btrfs => {
            "# Dukovo's backups of {machine} (backup(8), ADR 0463). Each file is a\n\
             # btrfs send stream, encrypted with age; the parts of one, in order,\n\
             # restore with Linux's tools:\n\
             #   cat PLACE/CHAIN/TIME.age* | age -d -i KEY | btrfs receive FOLDER\n\
             format = 1\n"
        }
        Format::Restic => {
            "# Dukovo's backups of {machine} (backup(8), ADR 0465): a restic\n\
             # repository, whose password is the recovery key. Linux's restic\n\
             # reads it:\n\
             #   restic -r FOLDER snapshots\n\
             format = 2\n"
        }
    };
    format!(
        "{}machine = \"{machine}\"\nkey = \"{key}\"\n",
        about.replace("{machine}", &machine()),
        machine = machine()
    )
}

/// The format and key named in a destination's mark, read from `store`.
fn read_mark(store: &mut dyn Store) -> Result<(Format, String), Fail> {
    #[derive(Deserialize)]
    struct Mark {
        format: u32,
        key: String,
    }
    let mut text = String::new();
    store
        .read(layout::MARK)?
        .read_to_string(&mut text)
        .map_err(|e| util::failed(&store.place(), e))?;
    let mark: Mark = toml::from_str(&text)
        .map_err(|err| util::fail(format!("{}/{}: {err}", store.place(), layout::MARK)))?;
    let format = match mark.format {
        1 => Format::Btrfs,
        2 => Format::Restic,
        other => {
            return Err(util::fail(format!(
                "{}: backups of format {other}, which this Dukovo does not read",
                store.place()
            )));
        }
    };
    Ok((format, mark.key))
}

/// Writes the mark, or checks that the one there is, `marked`, is for
/// `key` and `format`.
fn check_mark(store: &mut dyn Store, marked: bool, key: &str, format: Format) -> Result<(), Fail> {
    if marked {
        let (found, marked) = read_mark(store)?;
        if marked != key {
            return Err(util::fail(format!(
                "{}: holds backups made with another key; remove that folder, or back up \
                 elsewhere",
                store.place()
            )));
        }
        if found != format {
            return Err(util::fail(format!(
                "{}: holds backups of the other format; remove that folder, or back up elsewhere",
                store.place()
            )));
        }
        return Ok(());
    }
    store.put_whole(layout::MARK, mark_text(key, format).as_bytes())
}

/// A place backed up: where its snapshots are, and its folder's name at
/// destinations.
struct Backed {
    path: String,
    place: Place,
    folder: String,
}

impl Backed {
    fn new(path: &str) -> Result<Backed, Fail> {
        let place = snap::place(Path::new(path))?;
        let folder = place
            .folder
            .file_name()
            .and_then(|n| n.to_str())
            .filter(|n| layout::is_place(n) && *n != layout::RECORDS)
            .ok_or_else(|| util::fail(format!("{path}: no name for its backups")))?
            .to_string();
        Ok(Backed { path: path.into(), place, folder })
    }

    /// The snapshot of time `time` that backups are made from.
    fn snapshot(&self, time: &str) -> PathBuf {
        self.place.folder.join(format!("{PREFIX}{time}"))
    }
}

/// Encrypts the stream of the snapshot `snapshot`, whole or as its
/// changes from `parent`, to `recipients`, as the backup of time `time`
/// in `folder` of `store`; how many bytes it took.
fn write_backup(
    store: &mut dyn Store,
    folder: &str,
    time: &str,
    snapshot: &Path,
    parent: Option<&Path>,
    keys: &Keys,
) -> Result<u64, Fail> {
    let tagging = keys.tag.start(&format!("{folder}/{time}"));
    let parts = Parts::new(store, folder, time, store::PART, tagging);
    let mut buffered = BufWriter::with_capacity(1 << 20, encrypting(parts, &keys.recipients)?);
    subvolume::send::stream(snapshot, parent, &mut buffered)?;
    let encrypted =
        buffered.into_inner().map_err(|e| util::failed(&"encrypting", e.into_error()))?;
    let parts = encrypted.finish().map_err(|e| util::failed(&"encrypting", e))?;
    parts.finish()
}

/// Encrypts `data` to `keys` and writes it, tagged, as the file of time
/// `time` in `folder` of `store`, as a backup is written.
fn write_small(
    store: &mut dyn Store,
    folder: &str,
    time: &str,
    data: &[u8],
    keys: &Keys,
) -> Result<u64, Fail> {
    let tagging = keys.tag.start(&format!("{folder}/{time}"));
    let parts = Parts::new(store, folder, time, store::PART, tagging);
    let mut encrypted = encrypting(parts, &keys.recipients)?;
    encrypted.write_all(data).map_err(|e| util::failed(&"encrypting", e))?;
    let parts = encrypted.finish().map_err(|e| util::failed(&"encrypting", e))?;
    parts.finish()
}

/// A destination's keys, as a round writes with them: those every file
/// is encrypted to, and the key that tags them (ADR 0464).
struct Keys {
    recipients: Vec<age::x25519::Recipient>,
    tag: TagKey,
}

/// How what a destination holds is read: its tags checked with the key
/// the destination's own age key gives, or, with another of its keys,
/// unchecked, as `--unchecked` allows (ADR 0464).
enum Check {
    Tags(TagKey),
    Unchecked,
}

impl Check {
    /// The check for reading with `identity` the destination in `store`,
    /// whose `backup.toml` names its own key: its tags if `identity` is
    /// that key; otherwise unchecked, if `unchecked` allows it.
    fn of(
        store: &mut dyn Store,
        identity: &age::x25519::Identity,
        unchecked: bool,
    ) -> Result<Check, Fail> {
        let (_, own) = read_mark(store)?;
        if own == identity.to_public().to_string() {
            return Ok(Check::Tags(TagKey::of(identity)));
        }
        if unchecked {
            return Ok(Check::Unchecked);
        }
        Err(util::fail(format!(
            "{}: the key given is not the destination's own key, the recovery key, so it \
             cannot check who wrote the backups; --unchecked reads them all the same",
            store.place()
        )))
    }
}

/// A backup or record being read, decrypted; [`Reading::end`] reads what
/// is left and says whether its tag matched.
struct Reading<'a> {
    stream: Box<dyn Read + 'a>,
    checked: Rc<Cell<bool>>,
    /// Its path, for messages.
    at: String,
}

impl Read for Reading<'_> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        self.stream.read(buffer)
    }
}

impl Reading<'_> {
    /// Reads the rest, which checks the tag at the end.
    fn end(mut self) -> Result<(), Fail> {
        io::copy(&mut self.stream, &mut io::sink())
            .map_err(|e| util::fail(format!("{}: {}", self.at, util::strip_os_error(&e))))?;
        if !self.checked.get() {
            return Err(util::fail(format!("{}: its tag was not checked", self.at)));
        }
        Ok(())
    }
}

/// A writer that encrypts what it is given to `recipients`, in the age
/// format, onto `out`; its `finish` ends the file.
fn encrypting<W: Write>(
    out: W,
    recipients: &[age::x25519::Recipient],
) -> Result<age::stream::StreamWriter<W>, Fail> {
    let keys = recipients.iter().map(|r| r as &dyn age::Recipient);
    let encryptor = age::Encryptor::with_recipients(keys)
        .map_err(|err| util::fail(format!("encrypting: {err}")))?;
    encryptor.wrap_output(out).map_err(|e| util::failed(&"encrypting", e))
}

/// The decrypted stream of the backup or record whose files are `parts`
/// and whose tag's file is `tag`, read as `check` says.
fn read_backup<'a>(
    store: &'a mut dyn Store,
    parts: &[String],
    tag: Option<&str>,
    identity: &age::x25519::Identity,
    check: &Check,
) -> Result<Reading<'a>, Fail> {
    let first = parts.first().cloned().unwrap_or_default();
    let at = layout::tagged_path(&first);
    let check = match (check, tag) {
        (Check::Unchecked, _) => None,
        (Check::Tags(_), None) => {
            return Err(util::fail(format!(
                "{at}: has no tag, so who wrote it cannot be checked; --unchecked reads it all \
                 the same"
            )));
        }
        (Check::Tags(key), Some(tag)) => {
            let mut text = String::new();
            store
                .read(tag)?
                .take(1024)
                .read_to_string(&mut text)
                .map_err(|e| util::failed(&format!("{}/{tag}", store.place()), e))?;
            Some((key.start(&at), text))
        }
    };
    let joined = Joined::new(store, parts, check);
    let checked = joined.checked();
    let joined = BufReader::with_capacity(1 << 20, joined);
    let decryptor =
        age::Decryptor::new_buffered(joined).map_err(|err| util::fail(format!("{at}: {err}")))?;
    let stream = decryptor
        .decrypt(std::iter::once(identity as &dyn age::Identity))
        .map_err(|err| util::fail(format!("{at}: {err}")))?;
    Ok(Reading { stream: Box::new(stream), checked, at })
}

/// Asks Spark to run every running service's `before` or `after` backup
/// hook (service.toml(5)); what it said of each, as lines.
fn hooks(phase: &str) -> Vec<String> {
    match crate::power::request(&format!("hooks {phase}")) {
        Ok(answer) => answer.lines().filter(|l| !l.is_empty()).map(String::from).collect(),
        Err(fail) => vec![format!("error: the {phase} hooks: {}", fail.message)],
    }
}

/// Whether `dest` is due for a backup at `now`, by its state.
fn due(dest: &Destination, state: &State, now: u64) -> bool {
    let today = snap::local_day(now);
    match dest.every {
        Every::Hand => false,
        _ if state.done == 0 => true,
        Every::Day => snap::local_day(state.done) != today,
        Every::Week => today - snap::local_day(state.done) >= 7,
    }
}

/// How many days a destination backed up each day, or each week, may go
/// without a backup before the next sign-in says so (ADR 0504).
const OVERDUE_DAILY: u64 = 3;
const OVERDUE_WEEKLY: u64 = 14;

/// What the next sign-in should say of `dest` at `now`: its last backup
/// or check that failed, or how long it has gone without one, counted
/// from `added` until its first. None if all is well.
fn sign_in_line(dest: &Destination, state: &State, added: u64, now: u64) -> Option<String> {
    let name = &dest.name;
    if !state.failed.is_empty() {
        return Some(format!("backup: the last backup to {name} failed: {}", state.failed));
    }
    if !state.verify_failed.is_empty() {
        return Some(format!(
            "backup: restoring {name}'s newest backups to check them failed: {}",
            state.verify_failed
        ));
    }
    let allowed = match dest.every {
        Every::Hand => return None,
        Every::Day => OVERDUE_DAILY,
        Every::Week => OVERDUE_WEEKLY,
    };
    let since = if state.done == 0 { added } else { state.done };
    let days = now.saturating_sub(since) / (86_400 * NANOS);
    if days < allowed {
        return None;
    }
    Some(if state.done == 0 {
        format!(
            "backup: nothing has been backed up to {name} in the {days} days since it was added"
        )
    } else {
        format!("backup: the last backup to {name} was {days} days ago")
    })
}

/// The alerts that tell the owner of `dest` at `now` (ADR 0485), beside
/// its sign-in line: `backup-failed:NAME` while its last backup or check
/// failed, and `backup-overdue:NAME` from the day the sign-in says it is
/// overdue, critical once it is overdue twice over.
fn alerts_of(dest: &Destination, state: &State, added: u64, now: u64) -> Vec<Alert> {
    let name = &dest.name;
    let part = key_part(name);
    let mut alerts = Vec::new();
    if !state.failed.is_empty() {
        let title = format!("The last backup to {name} failed");
        alerts.push(Alert::new(
            format!("backup-failed:{part}"),
            Severity::Warning,
            &title,
            &failed_detail(&state.failed),
        ));
    } else if !state.verify_failed.is_empty() {
        let title = format!("Restoring {name}'s newest backups to check them failed");
        alerts.push(Alert::new(
            format!("backup-failed:{part}"),
            Severity::Warning,
            &title,
            &failed_detail(&state.verify_failed),
        ));
    }
    let (allowed, how) = match dest.every {
        Every::Hand => return alerts,
        Every::Day => (OVERDUE_DAILY, "every day"),
        Every::Week => (OVERDUE_WEEKLY, "every week"),
    };
    let since = if state.done == 0 { added } else { state.done };
    let days = now.saturating_sub(since) / (86_400 * NANOS);
    if days >= allowed {
        let severity = if days >= 2 * allowed { Severity::Critical } else { Severity::Warning };
        let title = if state.done == 0 {
            format!("Nothing has been backed up to {name} in the {days} days since it was added")
        } else {
            format!("The last backup to {name} was {days} days ago")
        };
        let detail = format!(
            "{name} is backed up {how}. `backup show {name}` says what it last did, and \
             `elevate backup run {name}` backs it up now."
        );
        alerts.push(Alert::new(format!("backup-overdue:{part}"), severity, &title, &detail));
    }
    alerts
}

fn failed_detail(why: &str) -> String {
    format!("{why}\n`backup show` and `log backup` say more.")
}

/// When `dest` was added, at `now` if that cannot be read: its settings
/// file's time.
fn added_at(dest: &Destination, now: u64) -> u64 {
    fs::metadata(settings_file(&dest.name))
        .and_then(|meta| meta.modified())
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(now, |since| since.as_secs() * NANOS)
}

/// Sets the service's alerts to what the destinations need at `now`; a
/// failure to reach alertd is logged once, until it is reached again.
fn tell_alerts(now: u64, unreached: &mut bool) {
    let Ok(dests) = Destination::all() else { return };
    let alerts: Vec<Alert> = dests
        .iter()
        .flat_map(|dest| alerts_of(dest, &State::read(&dest.name), added_at(dest, now), now))
        .collect();
    match crate::alerts::client::set(&alerts) {
        Ok(()) => *unreached = false,
        Err(why) if !*unreached => {
            *unreached = true;
            println!("backup: cannot tell alertd: {why}");
        }
        Err(_) => {}
    }
}

/// Keeps what the next sign-in should say of the destinations, and tells
/// the consoles when that changes to something.
fn tell_sign_in(now: u64) {
    let Ok(dests) = Destination::all() else { return };
    let lines: Vec<String> = dests
        .iter()
        .filter_map(|dest| sign_in_line(dest, &State::read(&dest.name), added_at(dest, now), now))
        .collect();
    if crate::sign_in_notices::put("backup", &lines) {
        let body = format!("{}\nbackup list shows each destination", lines.join("\n"));
        let _ = crate::notifyd::send(
            "backup",
            "Backups need a look",
            &body,
            crate::notifyd::Urgency::Normal,
        );
    }
}

/// Whether `dest` is due for a check by a restore at `now`.
fn verify_due(dest: &Destination, state: &State, now: u64) -> bool {
    let days = match dest.verify {
        Verify::Never => return false,
        Verify::Week => 7,
        Verify::Month => 30,
    };
    state.done != 0 && snap::local_day(now) - snap::local_day(state.verified) >= days
}

/// A destination that answered, ready for a round.
struct Ready {
    dest: Destination,
    store: Box<dyn Store>,
    listing: Listing,
    state: State,
    keys: Keys,
    places: Vec<String>,
    /// Its restic repository, open, if it keeps one.
    repo: Option<Repo>,
}

/// What a round needs of a destination: its store and what it holds, its
/// keys and places, and its restic repository if it keeps one.
type Prepared = (Box<dyn Store>, Listing, Keys, Vec<String>, Option<Repo>);

/// Opens `dest` for a round: its keys, its places, and its store, listed
/// and marked, or its restic repository, opened.
fn prepare(dest: &Destination) -> Result<Prepared, Fail> {
    let (identity, recipients) = dest.keys()?;
    let places = dest.places()?;
    let keys = Keys { recipients, tag: TagKey::of(&identity) };
    if dest.format == Format::Restic {
        let repo = dest.repo(&identity, false)?;
        let listing = Listing::read(std::iter::empty());
        return Ok((dest.store(false)?, listing, keys, places, Some(repo)));
    }
    let mut store = dest.store(false)?;
    let listing = Listing::read(store.list()?.iter().map(String::as_str));
    if !listing.marked {
        return Err(unmarked(store.as_ref()));
    }
    check_mark(store.as_mut(), true, &identity.to_public().to_string(), Format::Btrfs)?;
    Ok((store, listing, keys, places, None))
}

/// Why a round refuses a destination without the mark `backup add` wrote:
/// a share's folder before the share is mounted, or a disk's mount point
/// without the disk, would otherwise take the backups on this machine.
fn unmarked(store: &dyn Store) -> Fail {
    util::fail(format!(
        "{}: has no {}, which backup add wrote there; is the share mounted, or the disk \
         plugged in?",
        store.place(),
        layout::MARK
    ))
}

/// Backs up each of `dests` now; whether each did. Says what it does on
/// `out`, each line after `backup: `.
fn round(dests: Vec<Destination>, out: &mut dyn Write) -> Result<bool, Fail> {
    let _lock = Lock::take()?;
    let now = util::now_ns();
    let mut ready = Vec::new();
    let mut good = true;
    for dest in dests {
        let mut state = State::read(&dest.name);
        match prepare(&dest) {
            Ok((store, listing, keys, places, repo)) => {
                ready.push(Ready { dest, store, listing, state, keys, places, repo });
            }
            Err(fail) => {
                let _ = writeln!(out, "backup: {}: {}; not backed up", dest.name, fail.message);
                state.tried = now;
                state.failed = fail.message;
                state.write(&dest.name)?;
                good = false;
            }
        }
    }
    if ready.is_empty() {
        return Ok(good);
    }
    // The snapshots, with every service's hooks around them.
    let paths: BTreeSet<String> = ready.iter().flat_map(|r| r.places.iter().cloned()).collect();
    // A round in the same second as the one before waits for the next, so
    // that its snapshots have names of their own.
    let taken = |now: u64| {
        paths
            .iter()
            .any(|path| Backed::new(path).is_ok_and(|b| b.snapshot(&time_name(now)).exists()))
    };
    let mut now = now;
    while taken(now) {
        std::thread::sleep(Duration::from_millis(100));
        now = util::now_ns();
    }
    let time = time_name(now);
    for said in hooks("before") {
        let _ = writeln!(out, "backup: hook {said}");
    }
    let mut made = BTreeMap::new();
    let mut not_made = BTreeMap::new();
    for path in &paths {
        let snapshot = Backed::new(path).and_then(|backed| {
            snap::make_folder(&backed.place)?;
            subvolume::snapshot_to(&backed.place.top, &backed.snapshot(&time), true)?;
            Ok(backed)
        });
        match snapshot {
            Ok(backed) => {
                made.insert(path.clone(), backed);
            }
            Err(fail) => {
                not_made.insert(path.clone(), fail.message);
            }
        }
    }
    for said in hooks("after") {
        let _ = writeln!(out, "backup: hook {said}");
    }
    for mut r in ready {
        let name = r.dest.name.clone();
        r.state.tried = now;
        let failed = match r.repo.take() {
            Some(mut repo) => restic_round(&mut r, &mut repo, &made, &not_made, &time, now, out),
            None => streams_round(&mut r, &made, &not_made, &time, out),
        };
        if failed.is_empty() {
            r.state.done = now;
            r.state.failed.clear();
            let _ = writeln!(out, "backup: {name}: done, to {}", r.store.place());
        } else {
            r.state.failed = failed.join("; ");
            let _ = writeln!(out, "backup: {name}: {}", r.state.failed);
            good = false;
        }
        r.state.write(&name)?;
    }
    remove_unneeded(&made)?;
    Ok(good)
}

/// Gives `r` the round's backups as streams, and removes the chains it
/// no longer keeps; what failed.
fn streams_round(
    r: &mut Ready,
    made: &BTreeMap<String, Backed>,
    not_made: &BTreeMap<String, String>,
    time: &str,
    out: &mut dyn Write,
) -> Vec<String> {
    let name = r.dest.name.clone();
    let mut failed = Vec::new();
    for path in r.places.clone() {
        let path = &path;
        let Some(backed) = made.get(path) else {
            let why = not_made.get(path).cloned().unwrap_or_default();
            failed.push(format!("{path}: {why}"));
            continue;
        };
        match back_up(r, backed, time) {
            Ok(text) => {
                let _ = writeln!(out, "backup: {name}: {path}: {text}");
                r.state.sent.insert(path.clone(), time.to_string());
            }
            Err(fail) => failed.push(format!("{path}: {}", fail.message)),
        }
    }
    if let Err(fail) = write_record(r, made, time) {
        failed.push(format!("the machine's record: {}", fail.message));
    }
    // Old chains, and the records only they needed, go only once the
    // backups of this round are whole.
    let listing = r.store.list().map(|l| Listing::read(l.iter().map(String::as_str)));
    if let Ok(listing) = listing {
        let mut going: Vec<String> = listing.stray.clone();
        let mut old = Vec::new();
        for (place, chains) in &listing.places {
            for chain in layout::old_chains(chains, r.dest.chains) {
                going.push(format!("{place}/{}", chain.name));
                old.push(chain);
            }
        }
        for record in layout::old_records(&listing, &old) {
            going.push(record.path.clone());
            going.extend(record.tag.clone());
        }
        if let Err(fail) = r.store.remove(&going) {
            failed.push(fail.message);
        }
    }
    failed
}

/// Gives `r`'s restic repository the round's snapshots and the machine's
/// record, then forgets and prunes what it does not keep; what failed.
fn restic_round(
    r: &mut Ready,
    repo: &mut Repo,
    made: &BTreeMap<String, Backed>,
    not_made: &BTreeMap<String, String>,
    time: &str,
    now: u64,
    out: &mut dyn Write,
) -> Vec<String> {
    let name = r.dest.name.clone();
    let machine = machine();
    let mut failed = Vec::new();
    for path in r.places.clone() {
        let Some(backed) = made.get(&path) else {
            let why = not_made.get(&path).cloned().unwrap_or_default();
            failed.push(format!("{path}: {why}"));
            continue;
        };
        match repo.back_up(&backed.snapshot(time), &path, &backed.folder, &machine, time, now) {
            Ok(made) => {
                let _ = writeln!(
                    out,
                    "backup: {name}: {path}: {} new, of {} in {} files",
                    util::human(made.added, false),
                    util::human(made.read, false),
                    made.files
                );
                r.state.sent.insert(path.clone(), time.to_string());
            }
            Err(fail) => failed.push(format!("{path}: {}", fail.message)),
        }
    }
    let record = gather_record(&r.places, made, time).and_then(|record| {
        repo.back_up_record(&record.text(), Path::new(RECORD_SCRATCH), &machine, time, now)
    });
    if let Err(fail) = record {
        failed.push(format!("the machine's record: {}", fail.message));
    }
    // The newest snapshot of each place is always kept, so a place whose
    // backup failed keeps its last.
    if !r.dest.append_only
        && !repo.append_only()
        && let Err(fail) = repo.forget(r.dest.keep, &machine, now)
    {
        failed.push(fail.message);
    }
    failed
}

/// Backs up the place `backed` of the round at `time` to `r`; what it
/// wrote, in words.
fn back_up(r: &mut Ready, backed: &Backed, time: &str) -> Result<String, Fail> {
    let chains = r.listing.chains(&backed.folder);
    let sent = r.state.sent.get(&backed.path).map(String::as_str);
    let have = sent.is_some_and(|sent| is_top(&backed.snapshot(sent)));
    let next = layout::next(chains, sent, have, r.dest.chain);
    let (chain, parent) = match &next {
        Next::Whole => (time.to_string(), None),
        Next::Changes { chain, from } => (chain.clone(), Some(backed.snapshot(from))),
    };
    let folder = format!("{}/{chain}", backed.folder);
    let bytes = write_backup(
        r.store.as_mut(),
        &folder,
        time,
        &backed.snapshot(time),
        parent.as_deref(),
        &r.keys,
    )?;
    let what = if parent.is_some() { "changes" } else { "whole" };
    Ok(format!("{} ({what})", util::human(bytes, false)))
}

/// The machine's record of the round at `time` (ADR 0464): the places
/// `places`, by their folders at destinations, and the app registry and
/// grants.
fn gather_record(
    places: &[String],
    made: &BTreeMap<String, Backed>,
    time: &str,
) -> Result<record::Record, Fail> {
    let mut folders = BTreeMap::new();
    for path in places {
        let folder = match made.get(path) {
            Some(backed) => backed.folder.clone(),
            None => match Backed::new(path) {
                Ok(backed) => backed.folder,
                Err(_) => continue,
            },
        };
        folders.insert(folder, path.clone());
    }
    record::Record::gather(&machine(), time, folders, Path::new(SYSTEM_STATE))
}

/// Writes the machine's record of the round at `time` to `r`.
fn write_record(r: &mut Ready, made: &BTreeMap<String, Backed>, time: &str) -> Result<(), Fail> {
    let record = gather_record(&r.places, made, time)?;
    write_small(r.store.as_mut(), layout::RECORDS, time, record.text().as_bytes(), &r.keys)?;
    Ok(())
}

/// Removes every `backup-` snapshot of the places in `made` that no
/// destination last took.
fn remove_unneeded(made: &BTreeMap<String, Backed>) -> Result<(), Fail> {
    let mut needed: BTreeSet<(String, String)> = BTreeSet::new();
    for dest in Destination::all()? {
        for (path, time) in State::read(&dest.name).sent {
            needed.insert((path, time));
        }
    }
    for (path, backed) in made {
        for snapshot in snap::snapshots(&backed.place)? {
            let Some(time) = snapshot.name.strip_prefix(PREFIX) else { continue };
            if layout::is_time(time) && !needed.contains(&(path.clone(), time.to_string())) {
                subvolume::remove(&backed.place.folder.join(&snapshot.name))?;
            }
        }
    }
    Ok(())
}

/// The backup of `place` at `at` (the newest without it) at the
/// destination listed as `listing`: its chain's backups up to it.
fn chain_to<'l>(
    listing: &'l Listing,
    place: &str,
    at: Option<&str>,
) -> Result<&'l [layout::Backup], Fail> {
    for chain in listing.chains(place).iter().rev().filter(|c| c.whole()) {
        let end = match at {
            None => chain.backups.len(),
            Some(at) => match chain.backups.iter().position(|b| b.time == at) {
                Some(i) => i + 1,
                None => continue,
            },
        };
        return Ok(&chain.backups[..end]);
    }
    Err(util::fail(match at {
        Some(at) => format!("no backup of {place} was made at {}", shown_time(at)),
        None => format!("no backup of {place} there"),
    }))
}

/// Receives the backups `backups`, a chain's from its whole send, into
/// the folder `into`, the last as `name`; the others are made and removed
/// on the way.
fn receive_chain(
    store: &mut dyn Store,
    backups: &[layout::Backup],
    identity: &age::x25519::Identity,
    check: &Check,
    into: &Path,
    name: &str,
) -> Result<(), Fail> {
    let mut before: Option<PathBuf> = None;
    for (i, backup) in backups.iter().enumerate() {
        let last = i + 1 == backups.len();
        let made = if last { name.to_string() } else { format!(".{name}-{i}") };
        let mut stream = read_backup(store, &backup.parts, backup.tag.as_deref(), identity, check)?;
        // Its tag is checked once it is read to its end: a backup someone
        // else wrote is received, then removed.
        let received =
            subvolume::receive::receive_as(into, &mut stream, &made).and_then(|()| stream.end());
        if let Some(path) = before.take() {
            subvolume::remove(&path)?;
        }
        if let Err(fail) = received {
            let path = into.join(&made);
            if is_top(&path) {
                subvolume::remove(&path)?;
            }
            return Err(util::fail(format!("{}: {}", backup.time, fail.message)));
        }
        before = (!last).then(|| into.join(&made));
    }
    Ok(())
}

/// Reads, checks and decrypts the record `found` of `store`.
fn read_record(
    store: &mut dyn Store,
    found: &layout::Record,
    identity: &age::x25519::Identity,
    check: &Check,
) -> Result<record::Record, Fail> {
    let at = layout::tagged_path(&found.path);
    let mut reading = read_backup(
        store,
        std::slice::from_ref(&found.path),
        found.tag.as_deref(),
        identity,
        check,
    )?;
    let mut text = String::new();
    (&mut reading)
        .take(64 << 20)
        .read_to_string(&mut text)
        .map_err(|e| util::fail(format!("{at}: {}", util::strip_os_error(&e))))?;
    reading.end()?;
    let record = record::Record::parse(&text).map_err(|why| util::fail(format!("{at}: {why}")))?;
    if record.time != found.time {
        return Err(util::fail(format!("{at}: says it is of {}", record.time)));
    }
    Ok(record)
}

/// Reads every backup at the destination through, checking each chain's
/// links: each stream's parent is the backup before it. How many it read.
fn verify_reads(
    store: &mut dyn Store,
    listing: &Listing,
    identity: &age::x25519::Identity,
    check: &Check,
) -> Result<usize, Fail> {
    use radix_btrfs::send::stream::{Command as C, Reader};
    let mut count = 0;
    for (place, chains) in &listing.places {
        for chain in chains.iter().filter(|c| c.whole()) {
            let mut before: Option<([u8; 16], u64)> = None;
            for backup in &chain.backups {
                let at = format!("{place}/{}/{}", chain.name, backup.time);
                let failed =
                    |e: io::Error| util::fail(format!("{at}: {}", util::strip_os_error(&e)));
                let mut stream =
                    read_backup(store, &backup.parts, backup.tag.as_deref(), identity, check)?;
                let mut reader = Reader::new(&mut stream);
                let first = reader.command().map_err(failed)?;
                let this = match (first, before) {
                    (Some(C::Subvol { uuid, ctransid, .. }), None) => (uuid, ctransid),
                    (
                        Some(C::Snapshot { uuid, ctransid, clone_uuid, clone_ctransid, .. }),
                        Some(b),
                    ) if (clone_uuid, clone_ctransid) == b => (uuid, ctransid),
                    _ => return Err(util::fail(format!("{at}: not the next backup of its chain"))),
                };
                while reader.command().map_err(failed)?.is_some() {}
                drop(reader);
                stream.end()?;
                before = Some(this);
                count += 1;
            }
        }
    }
    Ok(count)
}

/// Verifies destination `dest`: every backup read through, and with
/// `restore`, the newest of each place restored and compared with the
/// snapshot it was made from.
fn verify(dest: &Destination, restore: bool, out: &mut dyn Write) -> Result<(), Fail> {
    if dest.format == Format::Restic {
        return verify_repo(dest, restore, out);
    }
    let (identity, _) = dest.keys()?;
    let mut store = dest.store(false)?;
    let listing = Listing::read(store.list()?.iter().map(String::as_str));
    if listing.places.is_empty() {
        return Err(util::fail(format!("{}: holds no backups yet", store.place())));
    }
    let check = Check::Tags(TagKey::of(&identity));
    let count = verify_reads(store.as_mut(), &listing, &identity, &check)?;
    for found in &listing.records {
        read_record(store.as_mut(), found, &identity, &check)?;
    }
    let _ = writeln!(
        out,
        "backup: {}: read {count} backups and {} records through, every chain whole",
        dest.name,
        listing.records.len()
    );
    if !restore {
        return Ok(());
    }
    let state = State::read(&dest.name);
    for path in dest.places()? {
        let backed = Backed::new(&path)?;
        let Some(time) = state.sent.get(&path) else { continue };
        let original = backed.snapshot(time);
        if !is_top(&original) {
            continue;
        }
        let backups = chain_to(&listing, &backed.folder, Some(time))?;
        snap::make_folder(&backed.place)?;
        let name = format!(".check-{time}");
        let copy = backed.place.folder.join(&name);
        let received =
            receive_chain(store.as_mut(), backups, &identity, &check, &backed.place.folder, &name);
        let compared = received.and_then(|()| compare::same(&copy, &original).map_err(util::fail));
        if is_top(&copy) {
            subvolume::remove(&copy)?;
        }
        let entries = compared.map_err(|fail| {
            util::fail(format!("{path}: the backup of {}: {}", shown_time(time), fail.message))
        })?;
        let _ = writeln!(
            out,
            "backup: {}: {path}: restored the backup of {} and found its {entries} files and \
             folders as they were",
            dest.name,
            shown_time(time)
        );
    }
    Ok(())
}

/// Verifies destination `dest`'s restic repository (ADR 0465): checked,
/// with every pack read and its data's hashes compared, and with
/// `restore`, the newest backup of each place restored and compared with
/// the snapshot it was made from.
fn verify_repo(dest: &Destination, restore: bool, out: &mut dyn Write) -> Result<(), Fail> {
    let (identity, _) = dest.keys()?;
    let mut repo = Repo::open(dest.store(false)?, &password(&identity))?;
    let snapshots = repo.snapshots(Some(&machine()))?;
    if snapshots.is_empty() {
        return Err(util::fail(format!("{}: holds no backups yet", repo.place())));
    }
    repo.check(true)?;
    let records = snapshots.iter().filter(|s| s.is_record()).count();
    let _ = writeln!(
        out,
        "backup: {}: checked the repository and read {} backups and {records} records through, \
         all whole",
        dest.name,
        snapshots.len() - records
    );
    if !restore {
        return Ok(());
    }
    let state = State::read(&dest.name);
    for path in dest.places()? {
        let backed = Backed::new(&path)?;
        let Some(time) = state.sent.get(&path) else { continue };
        let original = backed.snapshot(time);
        if !is_top(&original) {
            continue;
        }
        let Some(snapshot) = snapshots.iter().find(|s| s.path == path && s.time == *time) else {
            return Err(util::fail(format!(
                "{path}: no backup of {} in the repository",
                shown_time(time)
            )));
        };
        snap::make_folder(&backed.place)?;
        let copy = backed.place.folder.join(format!(".check-{time}"));
        if is_top(&copy) {
            subvolume::remove(&copy)?;
        }
        subvolume::make(&copy)?;
        let compared = repo
            .restore(snapshot, &copy)
            .and_then(|()| compare::same(&copy, &original).map_err(util::fail));
        subvolume::remove(&copy)?;
        let entries = compared.map_err(|fail| {
            util::fail(format!("{path}: the backup of {}: {}", shown_time(time), fail.message))
        })?;
        let _ = writeln!(
            out,
            "backup: {}: {path}: restored the backup of {} and found its {entries} files and \
             folders as they were",
            dest.name,
            shown_time(time)
        );
    }
    Ok(())
}

/// Adds the destination `name` at `to`, with the rest of its settings as
/// `text` says; shows its recovery key.
fn add(name: &str, to: &str, extra: &str, out: &mut dyn Write) -> Result<(), Fail> {
    if !may_name(name) {
        return Err(util::usage(format!(
            "'{name}' cannot name a destination: up to 32 letters, digits, '-' and '_'"
        )));
    }
    if settings_file(name).exists() {
        return Err(util::fail(format!("a destination is already called {name}")));
    }
    let text = format!(
        "# A destination of backup(8), as backup.toml(5) says.\nto = \"{}\"\n{extra}",
        to.replace('\\', "\\\\").replace('"', "\\\"")
    );
    let dest = Destination::parse(name, &text).map_err(util::usage)?;
    if Address::parse(to).map_err(util::fail)?.is_some() {
        ssh_key()?;
    }
    fs::create_dir_all(SETTINGS).map_err(|e| util::failed(&SETTINGS, e))?;
    let s3 = s3::Address::parse(to).map_err(util::fail)?.is_some();
    if s3 {
        write_private(&s3_file(name), s3_from_env()?.text().as_bytes())?;
    }
    let identity = age::x25519::Identity::generate();
    let public = identity.to_public().to_string();
    let made = add_store(&dest, &identity);
    if made.is_err() && s3 {
        let _ = fs::remove_file(s3_file(name));
    }
    let place = made?;
    let key = format!(
        "# The key of backup(8)'s destination {name}, at {place}.\n# public key: {public}\n{}\n",
        identity.to_string().expose_secret()
    );
    let key_path = key_file(name);
    File::options()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&key_path)
        .and_then(|mut file| file.write_all(key.as_bytes()).and_then(|()| file.sync_all()))
        .map_err(|e| util::failed(&key_path.display(), e))?;
    fs::write(settings_file(name), text)
        .map_err(|e| util::failed(&settings_file(name).display(), e))?;
    let _ = writeln!(
        out,
        "Added {name}: {place}, backed up {}.\n\nIts recovery key, which reads its backups on any \
         machine{}:\n\n    {}\n\nKeep it somewhere other than this machine. Without it and this \
         machine, the backups cannot be read.",
        match dest.every {
            Every::Day => "every day",
            Every::Week => "every week",
            Every::Hand => "by hand",
        },
        match dest.format {
            Format::Btrfs => "",
            Format::Restic => ", and is the password of its restic repository",
        },
        identity.to_string().expose_secret()
    );
    Ok(())
}

/// Readies the store of `dest`, which `backup add` adds with the key
/// `identity`: marks its folder, the server's host key taken on first
/// contact, and makes its restic repository if it keeps one. Where it
/// is.
fn add_store(dest: &Destination, identity: &age::x25519::Identity) -> Result<String, Fail> {
    let mut store = dest.store(true)?;
    let public = identity.to_public().to_string();
    match dest.format {
        Format::Btrfs => {
            let listing = Listing::read(store.list()?.iter().map(String::as_str));
            check_mark(store.as_mut(), listing.marked, &public, Format::Btrfs)?;
            Ok(store.place())
        }
        Format::Restic => {
            let marked = store.size(layout::MARK)?.is_some();
            if !marked && Repo::exists(store.as_mut())? {
                return Err(util::fail(format!(
                    "{}: holds a restic repository already; remove that folder, or back up \
                     elsewhere",
                    store.place()
                )));
            }
            dest.repo(identity, true).map(|repo| repo.place().to_string())
        }
    }
}

/// The machine's SSH key for servers, made the first time; its public
/// half.
fn ssh_key() -> Result<String, Fail> {
    let key = Path::new(SETTINGS).join(SSH_KEY);
    let public = key.with_extension("pub");
    if !key.exists() {
        fs::create_dir_all(SETTINGS).map_err(|e| util::failed(&SETTINGS, e))?;
        let comment = format!("backup@{}", machine());
        let status = Command::new("/system/commands/ssh-keygen")
            .args(["-q", "-t", "ed25519", "-N", "", "-C", &comment, "-f"])
            .arg(&key)
            .status()
            .map_err(|e| util::failed(&"ssh-keygen", e))?;
        if !status.success() {
            return Err(util::fail("ssh-keygen could not make the key".into()));
        }
    }
    fs::read_to_string(&public)
        .map(|t| t.trim().to_string())
        .map_err(|e| util::failed(&public.display(), e))
}

/// Lists the destinations, with what they last did.
fn list(out: &mut dyn Write) -> Result<(), Fail> {
    let mut rows = vec![
        ["NAME", "TO", "EVERY", "LAST BACKUP", "RESULT", "CHECKED"].map(String::from).to_vec(),
    ];
    for dest in Destination::all()? {
        let state = State::read(&dest.name);
        let when = |ns: u64| {
            if ns == 0 {
                "-".into()
            } else {
                util::local_time((ns / NANOS) as i64, "%Y-%m-%d %H:%M")
            }
        };
        let result = match (state.tried, state.failed.is_empty()) {
            (0, _) => "-".into(),
            (_, true) => "ok".into(),
            (_, false) => format!("failed: {}", state.failed),
        };
        let checked = match (state.verified, state.verify_failed.is_empty()) {
            (0, _) => "-".into(),
            (at, true) => when(at),
            (at, false) => format!("{} failed: {}", when(at), state.verify_failed),
        };
        let every = match dest.every {
            Every::Day => "day",
            Every::Week => "week",
            Every::Hand => "hand",
        };
        rows.push(vec![
            dest.name.clone(),
            dest.to.clone(),
            every.into(),
            when(state.done),
            result,
            checked,
        ]);
    }
    let widths: Vec<usize> =
        (0..6).map(|i| rows.iter().map(|r| util::width(&r[i])).max().unwrap_or(0)).collect();
    for row in rows {
        let line: Vec<String> =
            row.iter().zip(&widths).map(|(cell, w)| format!("{cell:<w$}", w = *w)).collect();
        let _ = writeln!(out, "{}", line.join("  ").trim_end());
    }
    Ok(())
}

/// Shows what destination `dest` holds.
fn show(dest: &Destination, out: &mut dyn Write) -> Result<(), Fail> {
    if dest.format == Format::Restic {
        let (identity, _) = dest.keys()?;
        let repo = Repo::open(dest.store(false)?, &password(&identity))?;
        let _ = writeln!(out, "{}: {} (restic)", dest.name, repo.place());
        let mut places: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for snapshot in repo.snapshots(Some(&machine()))?.into_iter().filter(|s| !s.is_record()) {
            places.entry(snapshot.path).or_default().push(shown_time(&snapshot.time));
        }
        if places.is_empty() {
            let _ = writeln!(out, "  no backups yet");
        }
        for (path, times) in places {
            let _ = writeln!(out, "  {path}:\n    {}", times.join(", "));
        }
        return Ok(());
    }
    let mut store = dest.store(false)?;
    let listing = Listing::read(store.list()?.iter().map(String::as_str));
    let _ = writeln!(out, "{}: {}", dest.name, store.place());
    if listing.places.is_empty() {
        let _ = writeln!(out, "  no backups yet");
    }
    for (place, chains) in &listing.places {
        let _ = writeln!(out, "  {place}:");
        for chain in chains {
            let times: Vec<String> = chain.backups.iter().map(|b| shown_time(&b.time)).collect();
            let whole = if chain.whole() { "" } else { " (without its whole backup)" };
            let _ = writeln!(out, "    {}{whole}", times.join(", "));
        }
    }
    Ok(())
}

/// What `backup restore` is asked.
struct Restore<'a> {
    /// A destination's name, or a machine's folder of backups.
    source: &'a str,
    /// The key to read them with; the destination's own without it.
    key: Option<&'a Path>,
    /// The place, by its path here or its folder there; every place
    /// without it.
    place: Option<&'a str>,
    /// The backup's time; the newest without it.
    at: Option<&'a str>,
    /// Where to receive them; the place's `@snapshots` folder without it.
    into: Option<&'a Path>,
    /// Read them although their tags cannot be checked.
    unchecked: bool,
}

/// What a restore reads backups from: a destination's streams, or its
/// restic repository.
trait Source {
    /// Where it is, for messages.
    fn place(&self) -> String;
    /// The folders of the places it holds backups of.
    fn folders(&self) -> Vec<String>;
    /// The time of the backup of the place `folder` made at `at`, or of
    /// its newest without it.
    fn backup_at(&self, folder: &str, at: Option<&str>) -> Result<String, Fail>;
    /// The time of the newest backup of `folder` from `time` or before.
    fn backup_upto(&self, folder: &str, time: &str) -> Option<String>;
    /// The newest record of the machine from `at` or before (the newest
    /// of all without it), read and checked.
    fn record(&mut self, at: Option<&str>) -> Result<record::Record, Fail>;
    /// Restores the backup of `folder` made at `time` into the folder
    /// `into`, as the read-only subvolume `name`.
    fn receive(&mut self, folder: &str, time: &str, into: &Path, name: &str) -> Result<(), Fail>;
}

fn no_backup(folder: &str, at: Option<&str>) -> Fail {
    util::fail(match at {
        Some(at) => format!("no backup of {folder} was made at {}", shown_time(at)),
        None => format!("no backup of {folder} there"),
    })
}

fn no_record(place: &str, at: Option<&str>) -> Fail {
    util::fail(match at {
        Some(at) => format!("{place}: no record of the machine from {} or before", shown_time(at)),
        None => format!("{place}: no record of the machine there"),
    })
}

/// A destination's streams, read with a key as `check` says.
struct Streams {
    store: Box<dyn Store>,
    listing: Listing,
    identity: age::x25519::Identity,
    check: Check,
}

impl Source for Streams {
    fn place(&self) -> String {
        self.store.place()
    }

    fn folders(&self) -> Vec<String> {
        self.listing.places.keys().cloned().collect()
    }

    fn backup_at(&self, folder: &str, at: Option<&str>) -> Result<String, Fail> {
        let backups = chain_to(&self.listing, folder, at)?;
        backups.last().map(|b| b.time.clone()).ok_or_else(|| no_backup(folder, at))
    }

    fn backup_upto(&self, folder: &str, time: &str) -> Option<String> {
        layout::chain_upto(&self.listing, folder, Some(time))?.last().map(|b| b.time.clone())
    }

    fn record(&mut self, at: Option<&str>) -> Result<record::Record, Fail> {
        let place = self.store.place();
        let found = layout::record_at(&self.listing, at).ok_or_else(|| no_record(&place, at))?;
        read_record(self.store.as_mut(), found, &self.identity, &self.check)
    }

    fn receive(&mut self, folder: &str, time: &str, into: &Path, name: &str) -> Result<(), Fail> {
        let backups = chain_to(&self.listing, folder, Some(time))?;
        receive_chain(self.store.as_mut(), backups, &self.identity, &self.check, into, name)
    }
}

/// A restic repository's snapshots, which backup(8) made (ADR 0465).
struct Snapshots {
    repo: Repo,
    snapshots: Vec<restic::Snapshot>,
}

impl Snapshots {
    fn of_folder<'a>(&'a self, folder: &'a str) -> impl Iterator<Item = &'a restic::Snapshot> {
        self.snapshots.iter().filter(move |s| !s.is_record() && s.folder == folder)
    }
}

impl Source for Snapshots {
    fn place(&self) -> String {
        self.repo.place().to_string()
    }

    fn folders(&self) -> Vec<String> {
        let folders: BTreeSet<&String> =
            self.snapshots.iter().filter(|s| !s.is_record()).map(|s| &s.folder).collect();
        folders.into_iter().cloned().collect()
    }

    fn backup_at(&self, folder: &str, at: Option<&str>) -> Result<String, Fail> {
        self.of_folder(folder)
            .filter(|s| at.is_none_or(|at| s.time == at))
            .map(|s| s.time.clone())
            .max()
            .ok_or_else(|| no_backup(folder, at))
    }

    fn backup_upto(&self, folder: &str, time: &str) -> Option<String> {
        self.of_folder(folder).filter(|s| s.time.as_str() <= time).map(|s| s.time.clone()).max()
    }

    fn record(&mut self, at: Option<&str>) -> Result<record::Record, Fail> {
        let found = self
            .snapshots
            .iter()
            .filter(|s| s.is_record() && at.is_none_or(|at| s.time.as_str() <= at))
            .max_by(|a, b| a.time.cmp(&b.time))
            .ok_or_else(|| no_record(self.repo.place(), at))?;
        let text = self.repo.read_record(found)?;
        let at = format!("{}: the record of {}", self.repo.place(), shown_time(&found.time));
        let record =
            record::Record::parse(&text).map_err(|why| util::fail(format!("{at}: {why}")))?;
        if record.time != found.time {
            return Err(util::fail(format!("{at}: says it is of {}", record.time)));
        }
        Ok(record)
    }

    fn receive(&mut self, folder: &str, time: &str, into: &Path, name: &str) -> Result<(), Fail> {
        let snapshot = self
            .of_folder(folder)
            .find(|s| s.time == time)
            .cloned()
            .ok_or_else(|| no_backup(folder, Some(time)))?;
        // Restored into a subvolume of its own, then kept read-only, as a
        // received backup is.
        let making = into.join(format!(".{name}-restoring"));
        if is_top(&making) {
            subvolume::remove(&making)?;
        }
        subvolume::make(&making)?;
        let restored = self
            .repo
            .restore(&snapshot, &making)
            .and_then(|()| subvolume::snapshot_to(&making, &into.join(name), true));
        subvolume::remove(&making)?;
        restored.map_err(|fail| {
            util::fail(format!("{folder} at {}: {}", shown_time(time), fail.message))
        })
    }
}

/// Opens the source of `ask`, as its mark says it keeps backups: its
/// streams, listed, with the key and check they are read with, or its
/// restic repository, opened with the key.
fn open_source(ask: &Restore) -> Result<Box<dyn Source>, Fail> {
    let (mut store, identity) = if may_name(ask.source) && settings_file(ask.source).exists() {
        let dest = Destination::read(ask.source)?;
        let identity = match ask.key {
            Some(key) => read_identity(key)?,
            None => dest.keys()?.0,
        };
        (dest.store(false)?, identity)
    } else {
        let key = ask.key.ok_or_else(|| {
            util::usage(format!(
                "{}: no destination is called that; name one, or give --key",
                ask.source
            ))
        })?;
        (open_store(ask.source, None, false, None)?, read_identity(key)?)
    };
    if store.size(layout::MARK)?.is_none() {
        return Err(util::fail(format!("{}: no backups of a machine there", store.place())));
    }
    if read_mark(store.as_mut())?.0 == Format::Restic {
        let repo = Repo::open(store, &password(&identity))?;
        let snapshots = repo.snapshots(None)?;
        return Ok(Box::new(Snapshots { repo, snapshots }));
    }
    let listing = Listing::read(store.list()?.iter().map(String::as_str));
    let check = Check::of(store.as_mut(), &identity, ask.unchecked)?;
    Ok(Box::new(Streams { store, listing, identity, check }))
}

/// Restores a backup of each place `ask` names, at its time, from its
/// source, into the folder it names or each place's `@snapshots`
/// folder.
fn restore(ask: &Restore, out: &mut dyn Write) -> Result<(), Fail> {
    let mut source = open_source(ask)?;
    // A place by its path here, or by its folder's name there.
    let folders: Vec<String> = match ask.place {
        Some(place) if place.starts_with('/') => vec![Backed::new(place)?.folder],
        Some(place) => vec![place.to_string()],
        None => source.folders(),
    };
    for folder in folders {
        let time = source.backup_at(&folder, ask.at)?;
        // Where it goes, and where it is shown.
        let (target, shown) = match ask.into {
            Some(into) => (into.to_path_buf(), into.to_path_buf()),
            None => {
                let backed = local_place(&folder).ok_or_else(|| {
                    util::fail(format!("no place here keeps snapshots in {folder}; give --into"))
                })?;
                snap::make_folder(&backed.place)?;
                (backed.place.folder, backed.place.shown)
            }
        };
        let name = format!("{RESTORED}{time}");
        source.receive(&folder, &time, &target, &name)?;
        let _ = writeln!(
            out,
            "Restored the backup of {folder} at {} as {}",
            shown_time(&time),
            shown.join(&name).display()
        );
    }
    Ok(())
}

/// The place here whose backups go in the folder `folder`, among snap's
/// and the apps'.
fn local_place(folder: &str) -> Option<Backed> {
    let mut paths = snap::Settings::read().ok()?.places;
    paths.push(APPLICATIONS.into());
    paths.into_iter().find_map(|p| Backed::new(&p).ok().filter(|b| b.folder == folder))
}

/// Restores the whole machine whose backups `ask` names, at its time
/// (ADR 0464): its record read and checked, each of the root's places it
/// names received into `@snapshots/machine-restore/staged` with the
/// state's files it carries, for Spark to put in place at the next
/// start; any other place received as `restored-<time>` in its own
/// `@snapshots` folder.
fn restore_machine(ask: &Restore, out: &mut dyn Write) -> Result<(), Fail> {
    use slots::machine::{self as staged, About};
    let mut source = open_source(ask)?;
    let record = source.record(ask.at)?;
    // The root's @snapshots, where the restore is staged.
    let settings = Backed::new("/system/settings")?;
    let snapshots = settings.place.folder.parent().unwrap_or(&settings.place.folder).to_path_buf();
    snap::make_folder(&settings.place)?;
    let top = snapshots.parent().unwrap_or(&snapshots).to_path_buf();
    let base = top.join(staged::FOLDER);
    fs::create_dir_all(&base).map_err(|e| util::failed(&base.display(), e))?;
    let staging = base.join(staged::STAGED);
    remove_staged(&staging)?;
    subvolume::make(&staging)?;
    let mut about = About {
        time: record.time.clone(),
        staged: time_name(util::now_ns()),
        machine: record.machine.clone(),
        ..About::default()
    };
    for (folder, path) in &record.places {
        let Some(time) = source.backup_upto(folder, &record.time) else {
            let _ = writeln!(out, "{path}: no backup of it from then; left as it is");
            continue;
        };
        let here = Backed::new(path).ok();
        let on_root = here.as_ref().is_some_and(|b| {
            b.folder == *folder && b.place.folder.parent() == Some(snapshots.as_path())
        });
        if staged::SUBVOLUMES.contains(&folder.as_str()) && on_root {
            source.receive(folder, &time, &staging, folder)?;
            about.subvolumes.push(folder.clone());
            let _ = writeln!(out, "{path}: staged its backup of {}", shown_time(&time));
            continue;
        }
        match here.filter(|b| b.folder == *folder) {
            Some(backed) => {
                snap::make_folder(&backed.place)?;
                let name = format!("{RESTORED}{time}");
                source.receive(folder, &time, &backed.place.folder, &name)?;
                let _ = writeln!(
                    out,
                    "{path}: restored its backup of {} as {}; snap restore --from copies files out",
                    shown_time(&time),
                    backed.place.shown.join(&name).display()
                );
            }
            None => {
                let _ = writeln!(
                    out,
                    "{path}: not on this machine; backup restore --place {folder} --into FOLDER \
                     restores it"
                );
            }
        }
    }
    if about.subvolumes.is_empty() {
        remove_staged(&staging)?;
        return Err(util::fail(format!(
            "{}: its record names none of the root's places with a backup",
            source.place()
        )));
    }
    // The state's files: the grants first cleared, as each person's are
    // the record's alone.
    about.clear.push("grants".into());
    for (path, text) in &record.state {
        let file = staging.join(staged::STATE).join(path);
        if let Some(parent) = file.parent() {
            fs::create_dir_all(parent).map_err(|e| util::failed(&parent.display(), e))?;
        }
        let mode = if path.starts_with("grants/") { 0o600 } else { 0o644 };
        write_private(&file, text.as_bytes())?;
        about.state.push((mode, path.clone()));
    }
    let partial = staging.join(format!("{}{}", staged::ABOUT, layout::PARTIAL));
    write_private(&partial, about.text().as_bytes())?;
    let about_path = staging.join(staged::ABOUT);
    fs::rename(&partial, &about_path).map_err(|e| util::failed(&about_path.display(), e))?;
    let _ = writeln!(
        out,
        "Staged {} as it was at {}. Restart to finish: before any service starts, the restored \
         places take the place of these, which are kept in {}.",
        record.machine,
        shown_time(&record.time),
        Path::new(SHOWN_SNAPSHOTS)
            .join(staged::FOLDER.trim_start_matches("@snapshots/"))
            .join(staged::kept_folder(&about.staged))
            .display()
    );
    Ok(())
}

/// Withdraws a whole machine's restore staged and not yet put in place.
fn cancel_machine(out: &mut dyn Write) -> Result<(), Fail> {
    use slots::machine as staged;
    let settings = Backed::new("/system/settings")?;
    let snapshots = settings.place.folder.parent().unwrap_or(&settings.place.folder);
    let top = snapshots.parent().unwrap_or(snapshots);
    let staging = top.join(staged::FOLDER).join(staged::STAGED);
    if !staging.join(staged::ABOUT).exists() {
        return Err(util::fail("no machine restore is staged".into()));
    }
    remove_staged(&staging)?;
    let _ =
        writeln!(out, "Withdrew the machine restore staged; the next start leaves all as it is.");
    Ok(())
}

/// Where Spark shows the root's `@snapshots`.
const SHOWN_SNAPSHOTS: &str = "/snapshots";

/// Removes what an earlier restore staged and did not finish.
fn remove_staged(staging: &Path) -> Result<(), Fail> {
    if !staging.exists() {
        return Ok(());
    }
    if let Ok(entries) = fs::read_dir(staging) {
        for entry in entries.flatten() {
            let path = entry.path();
            if is_top(&path) {
                subvolume::remove(&path)?;
            }
        }
    }
    if is_top(staging) {
        subvolume::remove(staging)
    } else {
        fs::remove_dir_all(staging).map_err(|e| util::failed(&staging.display(), e))
    }
}

/// Writes `data` to the file `path`, root's and readable by root alone.
fn write_private(path: &Path, data: &[u8]) -> Result<(), Fail> {
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)
        .map_err(|e| util::failed(&path.display(), e))?;
    file.write_all(data).map_err(|e| util::failed(&path.display(), e))?;
    file.sync_all().map_err(|e| util::failed(&path.display(), e))
}

pub fn main(args: &mut Args) -> Result<i32, Fail> {
    let usage = || {
        util::usage(
            "say add, remove, list, show, run, auto, verify, restore, key or ssh-key (backup(8))"
                .into(),
        )
    };
    let Some(action) = args.subcommand() else { return Err(usage()) };
    let action = action.to_str().unwrap_or("").to_string();
    let mut operands = Vec::new();
    let (mut extra, mut recipients) = (String::new(), Vec::new());
    let (mut key, mut place, mut at, mut into) = (None, None, None, None);
    let (mut watch, mut check_restore) = (false, false);
    let (mut machine_wide, mut unchecked, mut cancel) = (false, false, false);
    while let Some(arg) = args.next()? {
        match arg {
            Arg::Long(long)
                if action == "add" && ["format", "every", "verify"].contains(&long.as_str()) =>
            {
                extra.push_str(&format!("{long} = \"{}\"\n", args.text()?.replace('"', "")));
            }
            Arg::Long(long) if long == "recipient" && action == "add" => {
                recipients.push(args.text()?);
            }
            Arg::Long(long) if long == "append-only" && action == "add" => {
                extra.push_str("append_only = true\n");
            }
            Arg::Long(long) if long == "key" && action == "restore" => {
                key = Some(PathBuf::from(args.value()?));
            }
            Arg::Long(long) if long == "place" && action == "restore" => place = Some(args.text()?),
            Arg::Long(long) if long == "at" && action == "restore" => at = Some(args.text()?),
            Arg::Long(long) if long == "into" && action == "restore" => {
                into = Some(PathBuf::from(args.value()?));
            }
            Arg::Long(long) if long == "machine" && action == "restore" => machine_wide = true,
            Arg::Long(long) if long == "unchecked" && action == "restore" => unchecked = true,
            Arg::Long(long) if long == "cancel" && action == "restore" => cancel = true,
            Arg::Long(long) if long == "watch" && action == "auto" => watch = true,
            Arg::Long(long) if long == "restore" && action == "verify" => check_restore = true,
            Arg::Operand(operand) => operands.push(operand.to_string_lossy().into_owned()),
            other => return Err(args.unknown(&other)),
        }
    }
    if !recipients.is_empty() {
        let quoted: Vec<String> =
            recipients.iter().map(|r| format!("\"{}\"", r.replace('"', ""))).collect();
        extra.push_str(&format!("recipients = [{}]\n", quoted.join(", ")));
    }
    // Only list and ssh-key are anyone's: the rest reach the destination
    // with the machine's keys, change the settings, or read every file of
    // the places.
    // SAFETY: geteuid only reads.
    if !["list", "ssh-key"].contains(&action.as_str()) && unsafe { libc::geteuid() } != 0 {
        return Err(util::fail(format!(
            "backup {action} needs an administrator's session (elevate)"
        )));
    }
    let mut out = io::stdout();
    match (action.as_str(), operands.as_slice()) {
        ("add", [name, to]) => add(name, to, &extra, &mut out).map(|()| 0),
        ("remove", [name]) => {
            let dest = Destination::read(name)?;
            for path in [settings_file(name), key_file(name), s3_file(name), State::path(name)] {
                match fs::remove_file(&path) {
                    Err(err) if err.kind() != io::ErrorKind::NotFound => {
                        return Err(util::failed(&path.display(), err));
                    }
                    _ => {}
                }
            }
            // Its snapshots, unless another destination starts from them.
            let places: BTreeMap<String, Backed> = dest
                .places()?
                .into_iter()
                .filter_map(|path| Some((path.clone(), Backed::new(&path).ok()?)))
                .collect();
            remove_unneeded(&places)?;
            tell_sign_in(util::now_ns());
            let _ = writeln!(
                out,
                "Removed {name}. Its backups stay at {}; only its recovery key reads them now.",
                dest.to
            );
            Ok(0)
        }
        ("list", []) => list(&mut out).map(|()| 0),
        ("show", [name]) => show(&Destination::read(name)?, &mut out).map(|()| 0),
        ("run", names) => {
            let dests = if names.is_empty() {
                Destination::all()?
            } else {
                names.iter().map(|n| Destination::read(n)).collect::<Result<_, _>>()?
            };
            if dests.is_empty() {
                return Err(util::fail("no destinations; add one with backup add".into()));
            }
            let good = round(dests, &mut out)?;
            tell_sign_in(util::now_ns());
            Ok(if good { 0 } else { 1 })
        }
        ("auto", []) => {
            // The service alone raises alerts, so that they have one
            // source: `backup run` from a shell changes the sign-in line,
            // and the service's next round the alerts.
            let mut unreached = false;
            loop {
                let good = auto(&mut out);
                tell_sign_in(util::now_ns());
                if !watch {
                    return Ok(if good { 0 } else { 1 });
                }
                tell_alerts(util::now_ns(), &mut unreached);
                std::thread::sleep(ROUND);
            }
        }
        ("verify", [name]) => {
            let dest = Destination::read(name)?;
            let checked = verify(&dest, check_restore, &mut out);
            if check_restore {
                let mut state = State::read(name);
                state.verified = util::now_ns();
                state.verify_failed = match &checked {
                    Ok(()) => String::new(),
                    Err(fail) => fail.message.clone(),
                };
                state.write(name)?;
                tell_sign_in(util::now_ns());
            }
            checked.map(|()| 0)
        }
        ("restore", []) if machine_wide && cancel => cancel_machine(&mut out).map(|()| 0),
        ("restore", [source]) => {
            let ask = Restore {
                source,
                key: key.as_deref(),
                place: place.as_deref(),
                at: at.as_deref(),
                into: into.as_deref(),
                unchecked,
            };
            if machine_wide {
                if ask.place.is_some() || ask.into.is_some() {
                    return Err(util::usage(
                        "--machine restores every place where it was; it takes no --place or \
                         --into"
                            .into(),
                    ));
                }
                restore_machine(&ask, &mut out).map(|()| 0)
            } else {
                restore(&ask, &mut out).map(|()| 0)
            }
        }
        ("key", [name]) => {
            let identity = read_identity(&key_file(&Destination::read(name)?.name))?;
            let _ = writeln!(out, "{}", identity.to_string().expose_secret());
            Ok(0)
        }
        ("ssh-key", []) => {
            let _ = writeln!(out, "{}", ssh_key()?);
            Ok(0)
        }
        _ => Err(usage()),
    }
}

/// One round of the service: each destination that is due backed up, and
/// verified by a restore if that is due too. Whether all went well.
fn auto(out: &mut dyn Write) -> bool {
    let now = util::now_ns();
    let dests = match Destination::all() {
        Ok(dests) => dests,
        Err(fail) => {
            let _ = writeln!(out, "backup: {}; nothing done this round", fail.message);
            return false;
        }
    };
    let due: Vec<Destination> =
        dests.into_iter().filter(|d| due(d, &State::read(&d.name), now)).collect();
    // A disk not plugged in waits for the next round, unsaid.
    let due: Vec<Destination> = due
        .into_iter()
        .filter(|d| {
            Address::parse(&d.to).is_ok_and(|a| a.is_some())
                || s3::Address::parse(&d.to).is_ok_and(|a| a.is_some())
                || Path::new(&d.to).is_dir()
        })
        .collect();
    if due.is_empty() {
        return true;
    }
    let names: Vec<String> = due.iter().map(|d| d.name.clone()).collect();
    let mut good = match round(due, out) {
        Ok(good) => good,
        Err(fail) => {
            let _ = writeln!(out, "backup: {}", fail.message);
            false
        }
    };
    for name in names {
        let Ok(dest) = Destination::read(&name) else { continue };
        let mut state = State::read(&name);
        if !state.failed.is_empty() || !verify_due(&dest, &state, now) {
            continue;
        }
        let checked = verify(&dest, true, out);
        state.verified = util::now_ns();
        state.verify_failed = match checked {
            Ok(()) => String::new(),
            Err(fail) => {
                let _ = writeln!(out, "backup: {name}: verify: {}", fail.message);
                good = false;
                fail.message
            }
        };
        let _ = state.write(&name);
    }
    let _ = out.flush();
    good
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_destination_takes_the_defaults_for_what_it_leaves_out() {
        let dest = Destination::parse("stick", "to = \"/volumes/BACKUP\"\n").unwrap();
        assert_eq!(
            (dest.every, dest.verify, dest.chain, dest.chains),
            (Every::Day, Verify::Month, 30, 2)
        );
        assert!(dest.places.is_empty() && dest.recipients.is_empty());
        let text = "to = \"ssh://me@nas:2222/srv\"\nevery = \"week\"\nverify = \"never\"\nchain = 7\n\
                    places = [\"/users\"]\n";
        let dest = Destination::parse("nas", text).unwrap();
        assert_eq!((dest.every, dest.verify, dest.chain), (Every::Week, Verify::Never, 7));
        for bad in [
            "to = \"relative\"\n",
            "to = \"/v\"\nevery = \"hour\"\n",
            "to = \"/v\"\nchain = 0\n",
            "to = \"/v\"\nrecipients = [\"not a key\"]\n",
            "to = \"/v\"\nplaces = [\"users\"]\n",
            "to = \"/v\"\nmonthly = 1\n",
            "to = \"ssh://host/p\"\n",
        ] {
            assert!(Destination::parse("x", bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn destinations_are_named_plainly() {
        assert!(may_name("stick") && may_name("nas-2") && may_name("off_site"));
        for bad in ["", "-x", "a b", "a/b", "../x", &"x".repeat(33)] {
            assert!(!may_name(bad), "{bad}");
        }
    }

    #[test]
    fn a_destination_is_due_by_its_days() {
        let dest =
            |every| Destination { every, ..Destination::parse("d", "to = \"/v\"\n").unwrap() };
        let day = 86_400 * NANOS;
        let now = 20_000 * day + 12 * 3600 * NANOS;
        let state = |done| State { done, ..State::default() };
        assert!(due(&dest(Every::Day), &state(0), now));
        assert!(!due(&dest(Every::Day), &state(now - NANOS), now));
        assert!(due(&dest(Every::Day), &state(now - 2 * day), now));
        assert!(!due(&dest(Every::Week), &state(now - 6 * day), now));
        assert!(due(&dest(Every::Week), &state(now - 7 * day), now));
        assert!(!due(&dest(Every::Hand), &state(0), now));
        let checked = State { done: now, verified: now - 31 * day, ..State::default() };
        assert!(verify_due(&dest(Every::Day), &checked, now));
        let never = Destination { verify: Verify::Never, ..dest(Every::Day) };
        assert!(!verify_due(&never, &checked, now));
    }

    #[test]
    fn the_sign_in_tells_of_a_failed_or_overdue_backup() {
        let dest =
            |every| Destination { every, ..Destination::parse("stick", "to = \"/v\"\n").unwrap() };
        let day = 86_400 * NANOS;
        let now = 20_000 * day;
        let done = |done| State { done, ..State::default() };
        let line = |every, state: &State, added| sign_in_line(&dest(every), state, added, now);
        assert_eq!(line(Every::Day, &done(now - 2 * day), 0), None);
        assert_eq!(
            line(Every::Day, &done(now - 3 * day), 0).unwrap(),
            "backup: the last backup to stick was 3 days ago"
        );
        assert_eq!(line(Every::Week, &done(now - 13 * day), 0), None);
        assert!(line(Every::Week, &done(now - 14 * day), 0).is_some());
        assert_eq!(line(Every::Hand, &done(1), 0), None);
        assert_eq!(line(Every::Day, &done(0), now - day), None);
        assert_eq!(
            line(Every::Day, &done(0), now - 5 * day).unwrap(),
            "backup: nothing has been backed up to stick in the 5 days since it was added"
        );
        let failed = State { done: now, failed: "the disk is full".into(), ..State::default() };
        assert_eq!(
            line(Every::Hand, &failed, 0).unwrap(),
            "backup: the last backup to stick failed: the disk is full"
        );
        let unchecked =
            State { done: now, verify_failed: "a chain is short".into(), ..State::default() };
        assert!(line(Every::Day, &unchecked, 0).unwrap().contains("to check them failed: a chain"));
    }

    #[test]
    fn the_alerts_tell_of_a_failed_or_overdue_backup_as_the_sign_in_does() {
        let dest = |every| Destination {
            every,
            ..Destination::parse("Off_Site", "to = \"/v\"\n").unwrap()
        };
        let day = 86_400 * NANOS;
        let now = 20_000 * day;
        let done = |done| State { done, ..State::default() };
        let alerts = |every, state: &State, added| alerts_of(&dest(every), state, added, now);
        let keys =
            |found: Vec<Alert>| found.into_iter().map(|a| (a.key, a.severity)).collect::<Vec<_>>();
        assert_eq!(alerts(Every::Day, &done(now - 2 * day), 0), []);
        let late = alerts(Every::Day, &done(now - 3 * day), 0);
        assert_eq!(late[0].title, "The last backup to Off_Site was 3 days ago");
        assert!(late[0].detail.contains("every day") && late[0].invalid().is_none());
        assert_eq!(keys(late), [("backup-overdue:off_site".to_string(), Severity::Warning)]);
        // Overdue twice over is critical.
        assert_eq!(
            keys(alerts(Every::Day, &done(now - 6 * day), 0)),
            [("backup-overdue:off_site".to_string(), Severity::Critical)]
        );
        assert_eq!(alerts(Every::Week, &done(now - 13 * day), 0), []);
        assert_eq!(alerts(Every::Hand, &done(1), 0), []);
        assert!(
            alerts(Every::Day, &done(0), now - 5 * day)[0].title.contains("since it was added")
        );
        // A failure and lateness are told apart.
        let failed =
            State { done: now - 4 * day, failed: "the disk is full".into(), ..State::default() };
        let found = alerts(Every::Day, &failed, 0);
        assert_eq!(found[0].title, "The last backup to Off_Site failed");
        assert!(found[0].detail.starts_with("the disk is full\n"));
        assert_eq!(
            keys(found),
            [
                ("backup-failed:off_site".to_string(), Severity::Warning),
                ("backup-overdue:off_site".to_string(), Severity::Warning)
            ]
        );
        let unchecked =
            State { done: now, verify_failed: "a chain is short".into(), ..State::default() };
        assert!(alerts(Every::Day, &unchecked, 0)[0].title.contains("to check them failed"));
    }

    #[test]
    fn times_are_shown_as_people_read_them() {
        assert_eq!(shown_time("2026-11-11-030507"), "2026-11-11 03:05:07");
        assert_eq!(shown_time("other"), "other");
    }

    #[test]
    fn keys_go_and_come_back_through_their_files() {
        let dir = std::env::temp_dir().join(format!("backup-key-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let identity = age::x25519::Identity::generate();
        let path = dir.join("k.key");
        fs::write(&path, format!("# a comment\n{}\n", identity.to_string().expose_secret()))
            .unwrap();
        let back = read_identity(&path).unwrap();
        assert_eq!(back.to_public().to_string(), identity.to_public().to_string());
        fs::write(&path, "nothing\n").unwrap();
        assert!(read_identity(&path).is_err());
        fs::remove_dir_all(dir).unwrap();
    }

    /// The age format both ways with the age of Linux's distributions,
    /// where it is installed (skipped where not): what backup writes, as
    /// two parts, `age -d` reads with the recovery key, and what `age -r`
    /// writes, backup reads.
    #[test]
    fn linuxs_age_reads_what_backup_writes_and_backup_reads_what_it_writes() {
        if Command::new("age").arg("--version").output().is_err() {
            eprintln!("age is not installed; skipped");
            return;
        }
        let dir = std::env::temp_dir().join(format!("backup-age-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let identity = age::x25519::Identity::generate();
        let key = dir.join("recovery.key");
        fs::write(&key, format!("{}\n", identity.to_string().expose_secret())).unwrap();
        let data: Vec<u8> = (0..300_000u32).map(|i| (i * 7 % 251) as u8).collect();

        let mut store = Folder::new(&dir, None).unwrap();
        let tag = TagKey::of(&identity);
        let tagging = tag.start("users/c/2026-11-11-030000");
        let mut parts = Parts::new(&mut store, "users/c", "2026-11-11-030000", 100_000, tagging);
        let mut writer = encrypting(&mut parts, &[identity.to_public()]).unwrap();
        writer.write_all(&data).unwrap();
        writer.finish().unwrap();
        parts.finish().unwrap();
        let mut names: Vec<_> = fs::read_dir(dir.join("users/c"))
            .unwrap()
            .map(|e| e.unwrap().path())
            .filter(|p| p.to_string_lossy().contains(".age"))
            .collect();
        names.sort();
        assert!(names.len() > 1, "{names:?}");
        let joined: Vec<u8> = names.iter().flat_map(|path| fs::read(path).unwrap()).collect();
        fs::write(dir.join("joined.age"), joined).unwrap();
        let output = Command::new("age")
            .args(["-d", "-i"])
            .arg(&key)
            .arg(dir.join("joined.age"))
            .output()
            .unwrap();
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        assert!(output.stdout == data, "age read something else");

        let public = identity.to_public().to_string();
        let mut child = Command::new("age")
            .args(["-r", &public, "-o"])
            .arg(dir.join("users/c/2026-11-12-030000.age"))
            .stdin(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        child.stdin.take().unwrap().write_all(&data).unwrap();
        assert!(child.wait().unwrap().success());
        let forged = ["users/c/2026-11-12-030000.age".to_string()];
        let mut back = Vec::new();
        {
            let mut reading =
                read_backup(&mut store, &forged, None, &identity, &Check::Unchecked).unwrap();
            reading.read_to_end(&mut back).unwrap();
            reading.end().unwrap();
        }
        assert!(back == data, "backup read something else");
        // Unchecked, it reads: anyone with the public key wrote it. With
        // the tags checked, it has none, and a tag made with another key
        // is refused at its end.
        let checked = Check::Tags(TagKey::of(&identity));
        let refused = read_backup(&mut store, &forged, None, &identity, &checked).err().unwrap();
        assert!(refused.message.contains("has no tag"), "{}", refused.message);
        let mut other =
            TagKey::of(&age::x25519::Identity::generate()).start("users/c/2026-11-12-030000");
        other.update(&fs::read(dir.join(&forged[0])).unwrap());
        fs::write(dir.join("users/c/2026-11-12-030000.tag"), other.text()).unwrap();
        let tag_path = "users/c/2026-11-12-030000.tag";
        {
            let mut reading =
                read_backup(&mut store, &forged, Some(tag_path), &identity, &checked).unwrap();
            let read = io::copy(&mut reading, &mut io::sink());
            assert!(read.is_err() || reading.end().is_err());
        }
        // What backup wrote reads with its tag checked.
        let parts: Vec<String> = names
            .iter()
            .map(|p| p.strip_prefix(&dir).unwrap().to_string_lossy().into_owned())
            .collect();
        let tag_path = "users/c/2026-11-11-030000.tag";
        let mut reading =
            read_backup(&mut store, &parts, Some(tag_path), &identity, &checked).unwrap();
        back.clear();
        reading.read_to_end(&mut back).unwrap();
        reading.end().unwrap();
        assert!(back == data, "backup read something else");
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_backup_is_encrypted_to_its_keys_and_read_back() {
        let identity = age::x25519::Identity::generate();
        let other = age::x25519::Identity::generate();
        let keys = [identity.to_public(), other.to_public()];
        let encryptor =
            age::Encryptor::with_recipients(keys.iter().map(|r| r as &dyn age::Recipient)).unwrap();
        let mut sealed = Vec::new();
        let mut writer = encryptor.wrap_output(&mut sealed).unwrap();
        writer.write_all(&vec![5u8; 200_000]).unwrap();
        writer.finish().unwrap();
        for key in [&identity, &other] {
            let decryptor = age::Decryptor::new_buffered(sealed.as_slice()).unwrap();
            let mut back = Vec::new();
            decryptor
                .decrypt(std::iter::once(key as &dyn age::Identity))
                .unwrap()
                .read_to_end(&mut back)
                .unwrap();
            assert_eq!(back, vec![5u8; 200_000]);
        }
        // Cut short, or changed, it is refused.
        let wrong = age::x25519::Identity::generate();
        let decryptor = age::Decryptor::new_buffered(sealed.as_slice()).unwrap();
        assert!(decryptor.decrypt(std::iter::once(&wrong as &dyn age::Identity)).is_err());
        let mut cut = sealed[..sealed.len() - 100].to_vec();
        let mut back = Vec::new();
        let decryptor = age::Decryptor::new_buffered(cut.as_slice()).unwrap();
        let read = decryptor
            .decrypt(std::iter::once(&identity as &dyn age::Identity))
            .unwrap()
            .read_to_end(&mut back);
        assert!(read.is_err());
        let middle = cut.len() / 2;
        cut[middle] ^= 1;
        let decryptor = age::Decryptor::new_buffered(cut.as_slice()).unwrap();
        let read = decryptor
            .decrypt(std::iter::once(&identity as &dyn age::Identity))
            .unwrap()
            .read_to_end(&mut back);
        assert!(read.is_err());
    }
}
