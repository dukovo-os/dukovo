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
//! - `backup remove NAME`, `backup list [--json]`, `backup show NAME
//!   [--json]`; `backup find [--json]` looks in the volumes plugged in for
//!   machines' backups and recovery keys.
//! - `backup run [NAME...]` backs up now; `backup auto [--watch]` backs up
//!   each destination that is due, every hour with `--watch`.
//! - `backup verify [--restore] NAME`. `run`, `verify` and `restore` take
//!   `--progress`, which says each place before its turn as a task shows
//!   steps.
//! - `backup restore [--key FILE] [--place P] [--at TIME] [--into FOLDER]
//!   [--unchecked] NAME|SOURCE`, and `backup restore --machine [--key
//!   FILE] [--at TIME] [--unchecked] NAME|SOURCE`, which stages the whole
//!   machine for Spark to put in place at the next start (ADR 0464), or
//!   with `--cancel` withdraws what it staged.
//! - `backup key NAME [--json]` and `backup ssh-key [--json]` show the
//!   keys.

pub(crate) mod cards;
mod compare;
mod facts;
mod find;
mod json;
mod layout;
mod plan;
mod record;
mod restic;
mod s3;
mod store;
mod tag;

use std::cell::Cell;
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File};
use std::io::{self, BufReader, BufWriter, Read, Write};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
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
use crate::alerts::protocol::{About, Alert, Severity, key_part};
use crate::args::{Arg, Args};
use crate::backup_errors;
use crate::proc::Mount;
use crate::snap::{self, Place};
use crate::subvolume::{self, is_top};
use crate::util::{self, Fail};
use crate::{df, probe, volumed};

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
pub(crate) const RESTORED: &str = "restored-";

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
pub(crate) fn may_name(name: &str) -> bool {
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
                return Err(backup_errors::no_such_place(name));
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

/// Destination `name`'s settings file.
pub(crate) fn settings_file(name: &str) -> PathBuf {
    Path::new(SETTINGS).join(format!("{name}.toml"))
}

/// Destination `name`'s recovery key's file.
pub(crate) fn key_file(name: &str) -> PathBuf {
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
    identity_of(&text).ok_or_else(|| {
        util::fail(format!("{}: not an age key (AGE-SECRET-KEY-1...)", path.display()))
    })
}

/// The age key in `text`: its first line that is not blank or a comment,
/// as a key file has it.
fn identity_of(text: &str) -> Option<age::x25519::Identity> {
    let line = text
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty() && !line.starts_with('#'))
        .unwrap_or("");
    age::x25519::Identity::from_str(line).ok()
}

/// The most bytes of a key a restore reads from its input: a key file's
/// three lines.
const KEY_INPUT_MOST: u64 = 4096;

/// The key `--key` gives a restore: the file it names, or with `-` the
/// input, where the web console writes a key typed in it (ADR 0561). A key
/// that is not one, as one typed with a mistake in it, opens nothing
/// (BAK-103). What was read of the input is wiped.
fn given_key(key: &Path, input: &mut dyn Read) -> Result<age::x25519::Identity, Fail> {
    let mut bytes = Vec::new();
    if key == Path::new("-") {
        input
            .take(KEY_INPUT_MOST)
            .read_to_end(&mut bytes)
            .map_err(|e| util::failed(&"the key on the input", e))?;
    } else {
        bytes = fs::read(key).map_err(|e| util::failed(&key.display(), e))?;
    }
    let identity = std::str::from_utf8(&bytes).ok().and_then(identity_of);
    accounts::wipe(&mut bytes);
    identity.ok_or_else(backup_errors::wrong_key)
}

/// What a destination last did, which only backup writes.
#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(default)]
struct State {
    /// The time of the snapshot it last took of each place, by path.
    sent: BTreeMap<String, String>,
    /// The bytes it holds of each place's newest chain, by path: a restore
    /// of the place receives them. A place whose chain was begun before
    /// this was kept has none.
    bytes: BTreeMap<String, u64>,
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
                        return Err(backup_errors::round_running());
                    }
                    let _ = fs::remove_file(LOCK);
                }
                Err(err) => return Err(util::failed(&LOCK, err)),
            }
        }
        Err(backup_errors::round_running())
    }

    /// Whether a round holds the lock. A person who may not read the lock
    /// file, which is root's, can see that it is there, not whose it is,
    /// and goes by that.
    fn held() -> bool {
        let Ok(text) = fs::read_to_string(LOCK) else { return Path::new(LOCK).exists() };
        let Ok(pid) = text.trim().parse::<i32>() else { return true };
        // SAFETY: signal 0 checks that the process is there.
        let alive = unsafe { libc::kill(pid, 0) } == 0;
        alive || io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
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
    let stream = decryptor.decrypt(std::iter::once(identity as &dyn age::Identity)).map_err(
        |err| match err {
            age::DecryptError::NoMatchingKeys => backup_errors::wrong_key(),
            err => util::fail(format!("{at}: {err}")),
        },
    )?;
    Ok(Reading { stream: Box::new(stream), checked, at })
}

/// Whether `identity` opens the newest record of the machine in `store`,
/// whose files `listing` lists: whether the backups are made for it. An
/// error when there is no record, or it cannot be read to its header.
fn opens(
    store: &mut dyn Store,
    listing: &Listing,
    identity: &age::x25519::Identity,
) -> Result<bool, Fail> {
    let found = layout::record_at(listing, None).ok_or_else(|| no_record(&store.place(), None))?;
    let at = layout::tagged_path(&found.path);
    let reader = BufReader::new(store.read(&found.path)?);
    let decryptor =
        age::Decryptor::new_buffered(reader).map_err(|err| util::fail(format!("{at}: {err}")))?;
    match decryptor.decrypt(std::iter::once(identity as &dyn age::Identity)) {
        Ok(_) => Ok(true),
        Err(age::DecryptError::NoMatchingKeys) => Ok(false),
        Err(err) => Err(util::fail(format!("{at}: {err}"))),
    }
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

/// How many days `dest`, which is backed up each day or each week, has
/// gone without a backup at `now`, counted from `added` until its first,
/// when that is as many as it may go; none otherwise, and for a
/// destination backed up by hand.
fn overdue_days(dest: &Destination, state: &State, added: u64, now: u64) -> Option<u64> {
    let allowed = match dest.every {
        Every::Hand => return None,
        Every::Day => OVERDUE_DAILY,
        Every::Week => OVERDUE_WEEKLY,
    };
    let since = if state.done == 0 { added } else { state.done };
    let days = now.saturating_sub(since) / (86_400 * NANOS);
    (days >= allowed).then_some(days)
}

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
    let days = overdue_days(dest, state, added, now)?;
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
        alerts.push(
            Alert::new(
                format!("backup-failed:{part}"),
                Severity::Warning,
                &title,
                &failed_detail(&state.failed),
            )
            .about(About::Data),
        );
    } else if !state.verify_failed.is_empty() {
        let title = format!("Restoring {name}'s newest backups to check them failed");
        alerts.push(
            Alert::new(
                format!("backup-failed:{part}"),
                Severity::Warning,
                &title,
                &failed_detail(&state.verify_failed),
            )
            .about(About::Data),
        );
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
        alerts.push(
            Alert::new(format!("backup-overdue:{part}"), severity, &title, &detail)
                .about(About::Data),
        );
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

/// The steps of a task, said as `step N of M: WORDS` before each place's
/// turn when it was asked to (`--progress`): the form a console's task
/// reads (C2c). A step past the last is not said, since the line would
/// not be one.
struct Steps {
    on: bool,
    of: usize,
    at: usize,
    /// Whether the task is for several destinations, whose places are then
    /// told apart by the destination.
    several: bool,
}

impl Steps {
    /// `of` steps, said when `on`.
    fn new(on: bool, of: usize) -> Steps {
        Steps { on, of, at: 0, several: false }
    }

    /// The next step, `words` being what it does.
    fn next(&mut self, out: &mut dyn Write, words: &str) {
        self.at += 1;
        if self.on && self.at <= self.of && !words.trim().is_empty() {
            let _ = writeln!(out, "step {} of {}: {words}", self.at, self.of);
        }
    }
}

/// Where a round says what it does: `out`, and its steps when it is asked
/// for them.
struct Said<'a> {
    out: &'a mut dyn Write,
    steps: Steps,
}

impl Said<'_> {
    /// The step of backing up `path` to destination `dest`.
    fn place(&mut self, path: &str, dest: &str) {
        let words = if self.steps.several { format!("{path} to {dest}") } else { path.to_string() };
        self.steps.next(self.out, &words);
    }
}

impl Write for Said<'_> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.out.write(bytes)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.out.flush()
    }
}

/// Backs up each of `dests` now; whether each did. Says what it does on
/// `out`, each line after `backup: `, and with `progress` a step before
/// each place of each destination.
fn round(dests: Vec<Destination>, out: &mut dyn Write, progress: bool) -> Result<bool, Fail> {
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
    let places: usize = ready.iter().map(|r| r.places.len()).sum();
    let mut steps = Steps::new(progress, places);
    steps.several = ready.len() > 1;
    let mut said = Said { out, steps };
    let out = &mut said;
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
    out: &mut Said<'_>,
) -> Vec<String> {
    let name = r.dest.name.clone();
    let mut failed = Vec::new();
    for path in r.places.clone() {
        let path = &path;
        out.place(path, &name);
        let Some(backed) = made.get(path) else {
            let why = not_made.get(path).cloned().unwrap_or_default();
            failed.push(format!("{path}: {why}"));
            continue;
        };
        match back_up(r, backed, time) {
            Ok((text, chain)) => {
                let _ = writeln!(out, "backup: {name}: {path}: {text}");
                r.state.sent.insert(path.clone(), time.to_string());
                match chain {
                    Some(bytes) => r.state.bytes.insert(path.clone(), bytes),
                    None => r.state.bytes.remove(path),
                };
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
    out: &mut Said<'_>,
) -> Vec<String> {
    let name = r.dest.name.clone();
    let machine = machine();
    let mut failed = Vec::new();
    for path in r.places.clone() {
        out.place(&path, &name);
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
                r.state.bytes.insert(path.clone(), made.read);
            }
            Err(fail) => failed.push(format!("{path}: {}", fail.message)),
        }
    }
    let record = gather_record(&r.places, made, time, &r.state).and_then(|record| {
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
/// wrote, in words, and the bytes of the chain it is now in, if they are
/// known: the chain's earlier bytes are those the last round kept.
fn back_up(r: &mut Ready, backed: &Backed, time: &str) -> Result<(String, Option<u64>), Fail> {
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
    let chain = match parent {
        None => Some(bytes),
        Some(_) => r.state.bytes.get(&backed.path).map(|had| had.saturating_add(bytes)),
    };
    Ok((format!("{} ({what})", util::human(bytes, false)), chain))
}

/// The bytes the round at `time` left at a destination whose last state
/// is `state`, by the folders of `places`: those of the places it backed
/// up in that round and knows the bytes of.
fn bytes_of(places: &BTreeMap<String, String>, state: &State, time: &str) -> BTreeMap<String, u64> {
    places
        .iter()
        .filter(|(_, path)| state.sent.get(*path).is_some_and(|sent| sent == time))
        .filter_map(|(folder, path)| Some((folder.clone(), *state.bytes.get(path)?)))
        .collect()
}

/// The machine's record of the round at `time` (ADR 0464): the places
/// `places`, by their folders at destinations, the machine's hardware and
/// settings, and the app registry and grants. `state` is the
/// destination's, with this round's places in it.
fn gather_record(
    places: &[String],
    made: &BTreeMap<String, Backed>,
    time: &str,
    state: &State,
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
    let sources = facts::Sources::system();
    let facts = record::Facts {
        hardware: facts::this_machine(),
        bytes: bytes_of(&folders, state, time),
        settings: facts::settings(&sources)?,
        accounts: facts::accounts(&sources.passwd)?,
    };
    record::Record::gather(&machine(), time, folders, Path::new(SYSTEM_STATE), facts)
}

/// Writes the machine's record of the round at `time` to `r`.
fn write_record(r: &mut Ready, made: &BTreeMap<String, Backed>, time: &str) -> Result<(), Fail> {
    let record = gather_record(&r.places, made, time, &r.state)?;
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
    let record = record::Record::parse(&text).map_err(|why| unread(&at, why))?;
    if record.time != found.time {
        return Err(util::fail(format!("{at}: says it is of {}", record.time)));
    }
    Ok(record)
}

/// Why the record at `at` is not read, as a restore says it: BAK-107 for
/// one a later release wrote.
fn unread(at: &str, why: record::Unread) -> Fail {
    match why {
        record::Unread::Newer(format) => backup_errors::newer_backups(format),
        record::Unread::Wrong(why) => util::fail(format!("{at}: {why}")),
    }
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
fn verify(
    dest: &Destination,
    restore: bool,
    out: &mut dyn Write,
    progress: bool,
) -> Result<(), Fail> {
    if dest.format == Format::Restic {
        return verify_repo(dest, restore, out, progress);
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
    let places = dest.places()?;
    let mut steps = Steps::new(progress, restorable(&places, &state));
    for path in places {
        let backed = Backed::new(&path)?;
        let Some(time) = state.sent.get(&path) else { continue };
        let original = backed.snapshot(time);
        if !is_top(&original) {
            continue;
        }
        steps.next(out, &path);
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

/// How many of `places` a verify by a restore restores: those with a
/// backup that this machine still has the snapshot of.
fn restorable(places: &[String], state: &State) -> usize {
    places
        .iter()
        .filter(|path| {
            let time = state.sent.get(*path);
            let backed = Backed::new(path).ok();
            time.zip(backed).is_some_and(|(time, backed)| is_top(&backed.snapshot(time)))
        })
        .count()
}

/// Verifies destination `dest`'s restic repository (ADR 0465): checked,
/// with every pack read and its data's hashes compared, and with
/// `restore`, the newest backup of each place restored and compared with
/// the snapshot it was made from.
fn verify_repo(
    dest: &Destination,
    restore: bool,
    out: &mut dyn Write,
    progress: bool,
) -> Result<(), Fail> {
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
    let places = dest.places()?;
    let mut steps = Steps::new(progress, restorable(&places, &state));
    for path in places {
        let backed = Backed::new(&path)?;
        let Some(time) = state.sent.get(&path) else { continue };
        let original = backed.snapshot(time);
        if !is_top(&original) {
            continue;
        }
        steps.next(out, &path);
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

/// The words for an S3 address that the console may not add yet.
const S3_FOR_A_TERMINAL: &str = "S3 places are added at a terminal for now: backup add reads \
                                 its keys from the environment";

/// Whether a destination may be added at `to` from the console: a folder
/// from `/` or an `ssh://` address, as `backup add` takes them, and not an
/// S3 address, whose keys `backup add` reads from its environment.
///
/// # Errors
///
/// Why `to` cannot be, in words for a person.
pub(crate) fn check_to(to: &str) -> Result<(), String> {
    // Any text that begins as an S3 address is one, whether or not it is
    // whole: what is said is that these are not added here.
    if !matches!(s3::Address::parse(to), Ok(None)) {
        return Err(S3_FOR_A_TERMINAL.into());
    }
    match Address::parse(to)? {
        Some(_) => Ok(()),
        None if to.starts_with('/') => Ok(()),
        None => Err(format!("'{to}' is a folder from /, or an ssh:// address")),
    }
}

/// Whether `text` is a time as `backup show` lists a backup's: `--at`
/// takes one.
pub(crate) fn is_time(text: &str) -> bool {
    layout::is_time(text)
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

/// Where the volumes plugged in are.
const VOLUMES: &str = "/volumes";

/// Whether the volume mounted at `volume` is on a drive whose medium is
/// plugged in and out, among `mounts`.
fn removable_volume(mounts: &[Mount], volume: &Path) -> bool {
    mounts.iter().find(|mount| df::mount_path(mount) == volume).is_some_and(|mount| {
        let name = mount.source.strip_suffix("-unlocked").unwrap_or(&mount.source);
        let disk = match probe::block_text(name, "partition") {
            Some(_) => probe::disk_of(name),
            None => name,
        };
        probe::block_text(disk, "removable").as_deref() == Some("1")
    })
}

/// When the key file of destination `name` last changed, in milliseconds
/// since 1970; none without one. A stat needs no right to read the key.
fn key_changed_ms(name: &str) -> Option<u64> {
    let meta = fs::metadata(key_file(name)).ok()?;
    let seconds = u64::try_from(meta.ctime()).ok()?;
    let nanos = u64::try_from(meta.ctime_nsec()).ok()?;
    Some(seconds * 1000 + nanos / 1_000_000)
}

/// The machine and the time of the backup a whole-machine restore has
/// staged for the next start, if there is one: its `about` is readable by
/// everyone, so that the console's pages say so to whoever uses them.
fn staged_restore() -> Option<(String, String)> {
    use slots::machine as staged;
    let (_, staging) = staging_folders().ok()?;
    let about =
        staged::About::parse(&fs::read_to_string(staging.join(staged::ABOUT)).ok()?).ok()?;
    Some((about.machine, about.time))
}

/// `backup list --json`.
fn list_json(out: &mut dyn Write) -> Result<(), Fail> {
    let now = util::now_ns();
    let running = Lock::held();
    let mounts = volumed::volume_mounts().unwrap_or_default();
    let removable = |volume: &Path| removable_volume(&mounts, volume);
    let mut places = Vec::new();
    for dest in Destination::all()? {
        let state = State::read(&dest.name);
        let outside = json::Outside {
            added: added_at(&dest, now),
            key_ms: key_changed_ms(&dest.name),
            places: dest.places().unwrap_or_else(|_| dest.places.clone()),
            plugged_in: json::plugged_in(&dest.to, Path::new(VOLUMES), &removable),
        };
        places.push(json::destination(&dest, &state, &outside, running, now));
    }
    let shown = json::list(&places, staged_restore());
    writeln!(out, "{shown}").map_err(|e| util::failed(&"standard output", e))
}

/// Lists the destinations, with what they last did; as JSON with `json`.
fn list(out: &mut dyn Write, json: bool) -> Result<(), Fail> {
    if json {
        return list_json(out);
    }
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

/// The path here of each place `dest` backs up, by its folder there.
fn paths_by_folder(dest: &Destination) -> BTreeMap<String, String> {
    let paths = dest.places().unwrap_or_default();
    paths.into_iter().filter_map(|path| Some((Backed::new(&path).ok()?.folder, path))).collect()
}

/// Shows what destination `dest` holds; as JSON with `json`.
fn show(dest: &Destination, out: &mut dyn Write, json: bool) -> Result<(), Fail> {
    if dest.format == Format::Restic {
        let (identity, _) = dest.keys()?;
        let repo = Repo::open(dest.store(false)?, &password(&identity))?;
        if json {
            let snapshots = repo.snapshots(Some(&machine()))?;
            let shown = json::show_restic(&dest.name, repo.place(), &snapshots);
            return writeln!(out, "{shown}").map_err(|e| util::failed(&"standard output", e));
        }
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
    if json {
        let shown =
            json::show_streams(&dest.name, &store.place(), &listing, &paths_by_folder(dest));
        return writeln!(out, "{shown}").map_err(|e| util::failed(&"standard output", e));
    }
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

/// `backup find`: the machines' backups and the recovery keys in the
/// volumes plugged in; as JSON with `json`.
fn find_backups(out: &mut dyn Write, json: bool) -> Result<(), Fail> {
    let mounts = volumed::volume_mounts().unwrap_or_default();
    // Spark mounts this machine's root under /volumes too (ADR 0473).
    let own = fs::read_to_string(spark_api::root::PATH)
        .ok()
        .and_then(|text| spark_api::root::Root::parse(&text));
    let own = own.as_ref().map(|root| root.disk.as_str());
    let found = find::find(Path::new(VOLUMES), own, &|volume| removable_volume(&mounts, volume));
    let said = if json {
        format!("{}\n", find::json(&found))
    } else {
        find::text(&found, Path::new(VOLUMES))
    };
    out.write_all(said.as_bytes()).map_err(|e| util::failed(&"standard output", e))
}

/// What `backup restore` is asked.
struct Restore<'a> {
    /// A destination's name, or a machine's folder of backups.
    source: &'a str,
    /// The key to read them with, from `--key`; the destination's own
    /// without it.
    key: Option<age::x25519::Identity>,
    /// The place, by its path here or its folder there; every place
    /// without it.
    place: Option<&'a str>,
    /// The backup's time; the newest without it.
    at: Option<&'a str>,
    /// Where to receive them; the place's `@snapshots` folder without it.
    into: Option<&'a Path>,
    /// Read them although their tags cannot be checked.
    unchecked: bool,
    /// Say a step before each place restored (`--progress`).
    progress: bool,
    /// With `--machine`, where the backup's cards go (`--card`).
    cards: Vec<cards::Asked>,
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
        let record = record::Record::parse(&text).map_err(|why| unread(&at, why))?;
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
        let identity = match &ask.key {
            Some(key) => key.clone(),
            None => dest.keys()?.0,
        };
        (dest.store(false)?, identity)
    } else {
        let key = ask.key.clone().ok_or_else(|| {
            // A name is a place that is not there; a folder wants its key.
            if may_name(ask.source) {
                return backup_errors::no_such_place(ask.source);
            }
            util::usage(format!(
                "{}: no destination is called that; name one, or give --key",
                ask.source
            ))
        })?;
        (open_store(ask.source, None, false, None)?, key)
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
    let check = match Check::of(store.as_mut(), &identity, ask.unchecked) {
        Ok(check) => check,
        // A key that is not the recovery key may be another the backups
        // are made for, which reads them unchecked; one that opens none
        // of them is the wrong key.
        Err(fail) => match opens(store.as_mut(), &listing, &identity) {
            Ok(false) => return Err(backup_errors::wrong_key()),
            _ => return Err(fail),
        },
    };
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
    let mut steps = Steps::new(ask.progress, folders.len());
    for folder in folders {
        steps.next(out, ask.place.filter(|place| place.starts_with('/')).unwrap_or(&folder));
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

/// The root's `@snapshots` folder, and the folder in the root's top level
/// that a restore of a whole machine is staged in.
fn staging_folders() -> Result<(PathBuf, PathBuf), Fail> {
    use slots::machine as staged;
    let settings = Backed::new("/system/settings")?;
    let snapshots = settings.place.folder.parent().unwrap_or(&settings.place.folder).to_path_buf();
    let top = snapshots.parent().unwrap_or(&snapshots).to_path_buf();
    Ok((snapshots, top.join(staged::FOLDER).join(staged::STAGED)))
}

/// Whether a restore of a whole machine is staged: its `about` is
/// written last.
fn is_staged(staging: &Path) -> bool {
    staging.join(slots::machine::ABOUT).exists()
}

/// This machine, as a plan compares a record with it.
fn here(staging: &Path) -> plan::Here {
    plan::Here {
        hardware: facts::this_machine(),
        free: room_at(staging),
        staged: is_staged(staging),
    }
}

/// The bytes free in the filesystem that `staging` is in, read at the
/// nearest of its folders that is there: the root's, where a restore is
/// staged. `/` would not do, as it is the kernel's empty root and no disk
/// (radix-files(7)).
fn room_at(staging: &Path) -> Option<u64> {
    let folder = staging.ancestors().find(|folder| folder.is_dir())?;
    df::statvfs(folder.as_os_str()).ok().map(|space| space.available)
}

/// `backup restore --machine --plan --json`: what a restore of the whole
/// machine whose backups `ask` names would bring across, compared with
/// this one, with its cards paired as `ask` says. Nothing is received.
fn plan_machine(ask: &Restore, out: &mut dyn Write) -> Result<(), Fail> {
    let (_, staging) = staging_folders()?;
    let mut source = open_source(ask)?;
    let record = source.record(ask.at)?;
    let here = here(&staging);
    let pairs = plan::pairs(&record, &here, &ask.cards)?;
    let _ = writeln!(out, "{}", plan::plan(&record, &here, &pairs));
    Ok(())
}

/// Restores the whole machine whose backups `ask` names, at its time
/// (ADR 0464): its record read and checked, each of the root's places it
/// names received into `@snapshots/machine-restore/staged` with the
/// state's files it carries, for Spark to put in place at the next
/// start; any other place received as `restored-<time>` in its own
/// `@snapshots` folder. With `--card`, the staged network(5) gives the
/// old cards' addresses as it asks. A restore staged already is left as
/// it is (BAK-104); one that fails leaves nothing staged.
fn restore_machine(ask: &Restore, out: &mut dyn Write) -> Result<(), Fail> {
    let (snapshots, staging) = staging_folders()?;
    if is_staged(&staging) {
        return Err(backup_errors::restore_staged());
    }
    let mut source = open_source(ask)?;
    let record = source.record(ask.at)?;
    let here = here(&staging);
    // The cards are checked against the record before anything is
    // received, and against the received file after.
    plan::pairs(&record, &here, &ask.cards)?;
    let settings = Backed::new("/system/settings")?;
    snap::make_folder(&settings.place)?;
    let base = staging.parent().unwrap_or(&staging);
    fs::create_dir_all(base).map_err(|e| util::failed(&base.display(), e))?;
    remove_staged(&staging)?;
    subvolume::make(&staging)?;
    let staged = stage_machine(ask, source.as_mut(), &record, &here, &snapshots, &staging, out);
    if staged.is_err() {
        let _ = remove_staged(&staging);
    }
    staged
}

/// Receives what [`restore_machine`] stages into `staging`, and writes
/// its `about` last.
fn stage_machine(
    ask: &Restore,
    source: &mut dyn Source,
    record: &record::Record,
    here: &plan::Here,
    snapshots: &Path,
    staging: &Path,
    out: &mut dyn Write,
) -> Result<(), Fail> {
    use slots::machine::{self as staged, About};
    let mut about = About {
        time: record.time.clone(),
        staged: time_name(util::now_ns()),
        machine: record.machine.clone(),
        ..About::default()
    };
    let mut steps = Steps::new(ask.progress, record.places.len());
    for (folder, path) in &record.places {
        steps.next(out, path);
        let Some(time) = source.backup_upto(folder, &record.time) else {
            let _ = writeln!(out, "{path}: no backup of it from then; left as it is");
            continue;
        };
        let here_place = Backed::new(path).ok();
        let on_root = here_place
            .as_ref()
            .is_some_and(|b| b.folder == *folder && b.place.folder.parent() == Some(snapshots));
        if staged::SUBVOLUMES.contains(&folder.as_str()) && on_root {
            if path == SETTINGS_PLACE && !ask.cards.is_empty() {
                stage_settings(source, folder, &time, staging, record, here, &ask.cards, out)?;
            } else {
                source.receive(folder, &time, staging, folder)?;
            }
            about.subvolumes.push(folder.clone());
            let _ = writeln!(out, "{path}: staged its backup of {}", shown_time(&time));
            continue;
        }
        match here_place.filter(|b| b.folder == *folder) {
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
        return Err(util::fail(format!(
            "{}: its record names none of the root's places with a backup",
            source.place()
        )));
    }
    if !ask.cards.is_empty() && !about.subvolumes.iter().any(|name| name == "settings") {
        return Err(util::fail(
            "--card changes the restored network settings, and the record has no backup of \
             /system/settings from then"
                .into(),
        ));
    }
    // The state's files: the grants, and authd's keyrings and console
    // files, first cleared, as each person's are the record's alone.
    about.clear = record.cleared();
    for (path, text) in &record.state {
        let file = staging.join(staged::STATE).join(path);
        if let Some(parent) = file.parent() {
            fs::create_dir_all(parent).map_err(|e| util::failed(&parent.display(), e))?;
        }
        write_private(&file, text.as_bytes())?;
        about.state.push((record::mode_of(path), path.clone()));
    }
    let partial = staging.join(format!("{}{}", staged::ABOUT, layout::PARTIAL));
    // Readable by everyone, as the folder is: `backup list` tells anyone
    // that a restore is staged and from where, and it names no secret.
    write_with_mode(&partial, about.text().as_bytes(), 0o644)?;
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

/// The settings place, whose network(5) file `--card` changes.
const SETTINGS_PLACE: &str = "/system/settings";

/// Stages the settings place's backup `folder` of `time` with its
/// network(5) file changed as `asked` says: received under another name,
/// read-only as every received backup is, then snapshotted under its own
/// name to be changed, as Spark later snapshots it into place.
#[expect(clippy::too_many_arguments, reason = "the parts of one staged restore")]
fn stage_settings(
    source: &mut dyn Source,
    folder: &str,
    time: &str,
    staging: &Path,
    record: &record::Record,
    here: &plan::Here,
    asked: &[cards::Asked],
    out: &mut dyn Write,
) -> Result<(), Fail> {
    let received = format!(".{folder}-received");
    source.receive(folder, time, staging, &received)?;
    let copy = staging.join(folder);
    subvolume::snapshot_to(&staging.join(&received), &copy, false)?;
    subvolume::remove(&staging.join(&received))?;
    let relative = Path::new(crate::netd::SETTINGS)
        .strip_prefix(SETTINGS_PLACE)
        .unwrap_or(Path::new("network"));
    let file = copy.join(relative);
    let text = match fs::read_to_string(&file) {
        Ok(text) => text,
        Err(err) if err.kind() == io::ErrorKind::NotFound => String::new(),
        Err(err) => return Err(util::failed(&file.display(), err)),
    };
    let old = cards::old_cards(&text, record.hardware.as_ref());
    let pairs = cards::pair(&text, &old, &here.hardware.cards, asked)?;
    replace_file(&file, cards::rewritten(&text, &pairs).as_bytes())?;
    for pair in pairs.iter().filter(|pair| pair.asked) {
        let to = pair.new.as_ref().map_or_else(String::new, |card| format!(" {}", card.name));
        let said = match pair.choice {
            cards::Choice::Fixed => format!("its addresses go to this machine's{to}"),
            cards::Choice::Automatic => "it asks DHCP for an address".to_string(),
        };
        let shown = pair.old.name.as_deref().unwrap_or(&pair.old.section);
        let _ = writeln!(out, "{}: {shown} of {}: {said}", crate::netd::SETTINGS, record.machine);
    }
    Ok(())
}

/// Replaces the file `path` with `data` in one rename, with the owner and
/// the mode it had.
fn replace_file(path: &Path, data: &[u8]) -> Result<(), Fail> {
    let meta = fs::metadata(path).ok();
    let mode = meta.as_ref().map_or(0o644, |meta| meta.permissions().mode() & 0o7777);
    let partial = path.with_file_name(format!(
        "{}{}",
        path.file_name().and_then(|name| name.to_str()).unwrap_or("file"),
        layout::PARTIAL
    ));
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(mode)
        .open(&partial)
        .map_err(|e| util::failed(&partial.display(), e))?;
    let written = file
        .write_all(data)
        .and_then(|()| file.set_permissions(fs::Permissions::from_mode(mode)))
        .and_then(|()| match &meta {
            Some(meta) => std::os::unix::fs::fchown(&file, Some(meta.uid()), Some(meta.gid())),
            None => Ok(()),
        })
        .and_then(|()| file.sync_all())
        .and_then(|()| fs::rename(&partial, path));
    if let Err(err) = written {
        let _ = fs::remove_file(&partial);
        return Err(util::failed(&path.display(), err));
    }
    Ok(())
}

/// Withdraws a whole machine's restore staged and not yet put in place.
fn cancel_machine(out: &mut dyn Write) -> Result<(), Fail> {
    let (_, staging) = staging_folders()?;
    if !is_staged(&staging) {
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
    write_with_mode(path, data, 0o600)
}

/// Writes `data` to the file `path` with `mode`, whatever the umask, and
/// syncs it.
fn write_with_mode(path: &Path, data: &[u8], mode: u32) -> Result<(), Fail> {
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(mode)
        .open(path)
        .map_err(|e| util::failed(&path.display(), e))?;
    file.set_permissions(fs::Permissions::from_mode(mode))
        .map_err(|e| util::failed(&path.display(), e))?;
    file.write_all(data).map_err(|e| util::failed(&path.display(), e))?;
    file.sync_all().map_err(|e| util::failed(&path.display(), e))
}

pub fn main(args: &mut Args) -> Result<i32, Fail> {
    let usage = || {
        util::usage(
            "say add, remove, list, show, find, run, auto, verify, restore, key or ssh-key (backup(8))"
                .into(),
        )
    };
    let Some(action) = args.subcommand() else { return Err(usage()) };
    let action = action.to_str().unwrap_or("").to_string();
    let mut operands = Vec::new();
    let (mut extra, mut recipients) = (String::new(), Vec::new());
    let (mut key, mut place, mut at, mut into) = (None, None, None, None);
    let (mut watch, mut check_restore, mut json, mut progress) = (false, false, false, false);
    let (mut machine_wide, mut unchecked, mut cancel, mut plan) = (false, false, false, false);
    let mut asked_cards = Vec::new();
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
            Arg::Long(long) if long == "plan" && action == "restore" => plan = true,
            Arg::Long(long) if long == "card" && action == "restore" => {
                asked_cards.push(cards::Asked::parse(&args.text()?)?);
            }
            Arg::Long(long)
                if long == "json"
                    && ["list", "show", "find", "key", "ssh-key", "restore"]
                        .contains(&action.as_str()) =>
            {
                json = true;
            }
            Arg::Long(long)
                if long == "progress"
                    && ["run", "verify", "restore"].contains(&action.as_str()) =>
            {
                progress = true;
            }
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
        ("list", []) => list(&mut out, json).map(|()| 0),
        ("show", [name]) => show(&Destination::read(name)?, &mut out, json).map(|()| 0),
        ("find", []) => find_backups(&mut out, json).map(|()| 0),
        ("run", names) => {
            let dests = if names.is_empty() {
                Destination::all()?
            } else {
                names.iter().map(|n| Destination::read(n)).collect::<Result<_, _>>()?
            };
            if dests.is_empty() {
                return Err(util::fail("no destinations; add one with backup add".into()));
            }
            let good = round(dests, &mut out, progress)?;
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
            let checked = verify(&dest, check_restore, &mut out, progress);
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
            if !machine_wide && (plan || !asked_cards.is_empty()) {
                return Err(util::usage("--plan and --card go with --machine".into()));
            }
            if plan != json || (plan && progress) || cancel {
                return Err(util::usage(
                    "--plan prints JSON, with --json, and takes no --progress; --cancel takes \
                     no source"
                        .into(),
                ));
            }
            let key = match key {
                Some(key) => Some(given_key(&key, &mut io::stdin())?),
                None => None,
            };
            let ask = Restore {
                source,
                key,
                place: place.as_deref(),
                at: at.as_deref(),
                into: into.as_deref(),
                unchecked,
                progress,
                cards: asked_cards,
            };
            if machine_wide {
                if ask.place.is_some() || ask.into.is_some() {
                    return Err(util::usage(
                        "--machine restores every place where it was; it takes no --place or \
                         --into"
                            .into(),
                    ));
                }
                if plan {
                    return plan_machine(&ask, &mut out).map(|()| 0);
                }
                restore_machine(&ask, &mut out).map(|()| 0)
            } else {
                restore(&ask, &mut out).map(|()| 0)
            }
        }
        ("key", [name]) => {
            let identity = read_identity(&key_file(&Destination::read(name)?.name))?;
            if json {
                let public = identity.to_public().to_string();
                let shown = json::key(name, identity.to_string().expose_secret(), &public);
                let _ = writeln!(out, "{shown}");
            } else {
                let _ = writeln!(out, "{}", identity.to_string().expose_secret());
            }
            Ok(0)
        }
        ("ssh-key", []) => {
            let public = ssh_key()?;
            if json {
                let _ = writeln!(out, "{}", json::ssh_key(&public));
            } else {
                let _ = writeln!(out, "{public}");
            }
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
    let mut good = match round(due, out, false) {
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
        let checked = verify(&dest, true, out, false);
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
    fn the_record_gives_the_bytes_of_the_places_this_round_backed_up() {
        let places = BTreeMap::from([
            ("users".to_string(), "/users".to_string()),
            ("shared".to_string(), "/shared".to_string()),
            ("applications".to_string(), "/applications".to_string()),
        ]);
        let state = State {
            sent: BTreeMap::from([
                ("/users".to_string(), "2026-11-14-013000".to_string()),
                // Backed up the round before: its place failed in this one.
                ("/shared".to_string(), "2026-11-13-013000".to_string()),
                ("/applications".to_string(), "2026-11-14-013000".to_string()),
            ]),
            bytes: BTreeMap::from([
                ("/users".to_string(), 412),
                ("/shared".to_string(), 7),
                ("/gone".to_string(), 9),
            ]),
            ..State::default()
        };
        // /applications has no bytes known, as after an upgrade in the
        // middle of a chain.
        assert_eq!(
            bytes_of(&places, &state, "2026-11-14-013000"),
            BTreeMap::from([("users".to_string(), 412)])
        );
        assert!(bytes_of(&places, &State::default(), "2026-11-14-013000").is_empty());
    }

    #[test]
    fn a_file_is_written_with_its_mode_whatever_the_umask() {
        let dir = std::env::temp_dir().join(format!("backup-mode-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        // 0666 is more than a umask of 022 lets a new file have.
        for mode in [0o644, 0o600, 0o666] {
            let file = dir.join(format!("{mode:o}"));
            write_with_mode(&file, b"machine atlas\n", mode).unwrap();
            let meta = fs::metadata(&file).unwrap();
            assert_eq!(meta.permissions().mode() & 0o777, mode, "{mode:o}");
            assert_eq!(fs::read(&file).unwrap(), b"machine atlas\n");
        }
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_room_is_that_of_the_folder_a_restore_is_staged_in() {
        let dir = std::env::temp_dir().join(format!("backup-room-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let there = df::statvfs(dir.as_os_str()).unwrap().available;
        // The staging folder is not there until a restore is staged.
        let room = room_at(&dir.join("machine").join("staged")).unwrap();
        // The bytes free move as other tests write; within 64 MiB of each other.
        assert!(room.abs_diff(there) < 64 << 20, "{room} against {there}");
        assert_eq!(room_at(Path::new("")), None);
        let _ = fs::remove_dir_all(&dir);
    }

    /// The lines a task is made of, as the console's tasks read them: `step
    /// N of M: WORDS`, with 1 <= N <= M and words.
    fn is_step(line: &str, n: usize, of: usize) -> bool {
        line.strip_prefix("step ").and_then(|rest| rest.split_once(": ")).is_some_and(
            |(numbers, words)| numbers == format!("{n} of {of}") && !words.trim().is_empty(),
        )
    }

    #[test]
    fn steps_are_said_only_when_asked_and_never_past_the_last() {
        let mut out = Vec::new();
        let mut steps = Steps::new(true, 2);
        steps.next(&mut out, "/users");
        steps.next(&mut out, "/shared");
        // A third that was not counted is not said, nor an empty one.
        steps.next(&mut out, "/applications");
        let said = String::from_utf8(out).unwrap();
        let lines: Vec<&str> = said.lines().collect();
        assert_eq!(lines, ["step 1 of 2: /users", "step 2 of 2: /shared"]);
        assert!(is_step(lines[0], 1, 2) && is_step(lines[1], 2, 2));
        let mut quiet = Vec::new();
        let mut off = Steps::new(false, 2);
        off.next(&mut quiet, "/users");
        assert!(quiet.is_empty());
        let mut empty = Vec::new();
        Steps::new(true, 1).next(&mut empty, " ");
        assert!(empty.is_empty());
    }

    #[test]
    fn a_round_says_each_place_by_its_path_and_with_several_destinations_by_its_destination_too() {
        let mut out = Vec::new();
        let mut said = Said { out: &mut out, steps: Steps::new(true, 2) };
        said.place("/users", "stick");
        writeln!(said, "backup: stick: /users: 1.0 MB (whole)").unwrap();
        said.steps.several = true;
        said.place("/shared", "server");
        let text = String::from_utf8(out).unwrap();
        assert_eq!(
            text.lines().collect::<Vec<_>>(),
            [
                "step 1 of 2: /users",
                "backup: stick: /users: 1.0 MB (whole)",
                "step 2 of 2: /shared to server"
            ]
        );
    }

    #[test]
    fn a_verify_restores_the_places_it_has_a_snapshot_of_and_no_others() {
        // No backup of the place is in the state, or this machine has none
        // of it.
        let state = State::default();
        let places = vec!["/users".to_string(), "/no-such-place".to_string()];
        assert_eq!(restorable(&places, &state), 0);
        assert_eq!(restorable(&[], &state), 0);
    }

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
        assert_eq!(late[0].kind(), Some(About::Data));
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
        assert!(found.iter().all(|alert| alert.kind() == Some(About::Data)), "{found:?}");
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

    #[test]
    fn a_restore_takes_its_key_from_a_file_or_from_the_input() {
        let dir = std::env::temp_dir().join(format!("backup-given-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let identity = age::x25519::Identity::generate();
        let text = format!("# created: 2026-11-14\n{}\n", identity.to_string().expose_secret());
        let public = identity.to_public().to_string();
        let path = dir.join("k.key");
        fs::write(&path, &text).unwrap();
        let from_file = given_key(&path, &mut io::empty()).unwrap();
        assert_eq!(from_file.to_public().to_string(), public);
        // `-` reads the input, as the web console writes a key typed in it.
        let from_input = given_key(Path::new("-"), &mut text.as_bytes()).unwrap();
        assert_eq!(from_input.to_public().to_string(), public);
        // A key with a mistake in it, or none, opens nothing (BAK-103).
        let typo = text.replace("AGE-SECRET-KEY-1", "AGE-SECRET-KEY-2");
        for mut wrong in [typo.as_bytes(), b"", b"\xff\xfe"] {
            let Err(fail) = given_key(Path::new("-"), &mut wrong) else { panic!("a wrong key") };
            assert!(fail.message.ends_with("(BAK-103)"), "{}", fail.message);
        }
        // Only a key file's worth of the input is read.
        let long = format!("{}{text}", "#\n".repeat(KEY_INPUT_MOST as usize));
        assert!(given_key(Path::new("-"), &mut long.as_bytes()).is_err());
        // A file that is not there is said as such, not as a wrong key.
        let Err(fail) = given_key(&dir.join("gone.key"), &mut io::empty()) else {
            panic!("no file")
        };
        assert!(!fail.message.contains("BAK-103"), "{}", fail.message);
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

    #[test]
    fn the_console_adds_a_folder_or_an_ssh_address_and_not_an_s3_place() {
        for to in ["/volumes/LAB/dukovo-backup", "/", "ssh://u@host/path", "ssh://u@host:2222/~/b"]
        {
            assert_eq!(check_to(to), Ok(()), "{to}");
        }
        // Whatever begins as an S3 address is one, whole or not.
        for to in ["s3://bucket/x", "s3+http://nas:9000/bucket", "s3://", "s3://host/BAD"] {
            assert_eq!(
                check_to(to),
                Err("S3 places are added at a terminal for now: backup add reads its keys from \
                     the environment"
                    .to_string()),
                "{to}"
            );
        }
        assert_eq!(
            check_to("relative"),
            Err("'relative' is a folder from /, or an ssh:// address".to_string())
        );
        assert!(check_to("ssh://host/path").unwrap_err().contains("no user"));
        assert!(check_to("").is_err());
    }

    #[test]
    fn a_time_is_year_month_day_then_hour_minute_second() {
        assert!(is_time("2026-11-11-030012"));
        for text in ["2026-11-11-0300", "2026-11-11-0300121", "2026-11-11 030012", "yesterday", ""]
        {
            assert!(!is_time(text), "{text:?}");
        }
    }
}
