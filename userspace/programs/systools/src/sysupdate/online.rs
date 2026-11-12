// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Updates from the update server, the machine's side (ADR 0466). The
//! server's repository holds every release it offers and a channel file
//! for each of stable, beta and nightly that names one; all of it is
//! read through Dukovo's TUF client (ADR 0380), from the root this
//! system trusts, with the trusted metadata kept in the state between
//! checks, so that the server can never take it back to older metadata.
//!
//! A check reads the channel this system follows (`updates.toml`) and,
//! for a newer release, its list. A release names the oldest version
//! that updates to it directly: an older system takes the release it
//! names first, a bridge, and the newer one at its next update. Then
//! only the files the running slot lacks are fetched, each by its
//! SHA-256 from the repository's store, into the state's `files`
//! folder, which `sysupdate stage` builds the spare slot from: a file at
//! the same place with the same contents is not fetched, nor one moved
//! elsewhere, which is copied. A fetch cut short goes on from where it
//! stopped, every file is hashed as it lands and refused unless it is
//! the file the list names, and a request carries no machine ID or
//! account, nothing but the file it asks for.

use std::collections::{HashMap, HashSet};
use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Deserialize;
use slots::version::Version;
use system_release::firmware::{self, Set};
use system_release::list::{Release, hex, sha256_file};
use system_release::repo::target_path;
use system_release::rollout::{self, Rollout, Withdrawal};
use system_release::updates::{CHANNELS, Channel, channel_path};

use super::stage::TRUSTED_ROOT;
use crate::util::{self, Fail, fail};

/// Where the owner says which channel to follow, and from which server.
pub const SETTINGS: &str = "/system/settings/sysupdate/updates.toml";

/// The update server's repository of system releases.
pub const SERVER: &str = "https://updates.dukovo.org/system/";

/// The channel followed unless the settings say another.
pub const DEFAULT_CHANNEL: &str = "stable";

/// sysupdate's state: the trusted metadata, the lists read, and the
/// files fetched for the next stage.
const STATE: &str = "/system/state/sysupdate";

/// How many times a file is asked for before the fetch gives up, the
/// waits between growing from [`WAIT`].
const TRIES: u32 = 4;
const WAIT: Duration = Duration::from_secs(2);

/// The most bridges one update goes through.
const MOST_BRIDGES: usize = 16;

/// Where the trusted metadata is kept.
fn metadata_dir() -> PathBuf {
    Path::new(STATE).join("metadata")
}

/// Where the lists and channel files read are kept.
fn lists_dir() -> PathBuf {
    Path::new(STATE).join("lists")
}

/// Where the files fetched for the next stage are kept, by SHA-256.
#[must_use]
pub fn downloads() -> PathBuf {
    Path::new(STATE).join("files")
}

/// Removes the files fetched, once a stage no longer needs them.
pub fn clear_downloads() {
    let _ = std::fs::remove_dir_all(downloads());
}

/// The settings file as written.
#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct SettingsFile {
    channel: Option<String>,
    server: Option<String>,
    canary: Option<bool>,
    check: Option<Check>,
    apps: Option<Apps>,
    unattended: Option<bool>,
    restart: Option<String>,
    hold: Option<Hold>,
}

/// What updated(8) does when it finds an update (ADR 0502).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Check {
    /// Tells whoever is signed in; the default.
    #[default]
    Notify,
    /// Fetches the system update's files too, then tells.
    Download,
    /// Checks nothing.
    Off,
}

/// What updated(8) does with app updates (ADR 0502).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Apps {
    /// Tells whoever is signed in; the default.
    #[default]
    Notify,
    /// Updates the apps installed for everyone, as `bundle update` does.
    Auto,
}

/// Versions held back (ADR 0502): an offer of this version or an older
/// one is not taken until the hold is lifted; a newer one is.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Hold {
    /// The system version held back.
    pub system: Option<String>,
    /// The firmware set held back.
    pub firmware: Option<String>,
}

/// Which channel this system follows, and from which server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Settings {
    /// One of [`CHANNELS`].
    pub channel: String,
    /// The repository's address, ending in a slash.
    pub server: String,
    /// Whether this machine is one of the project's canaries, which take
    /// each release on its first day (ADR 0456).
    pub canary: bool,
    /// What updated(8) does when it finds an update.
    pub check: Check,
    /// What updated(8) does with app updates.
    pub apps: Apps,
    /// Whether updated(8) stages each update itself, as a server left to
    /// run on its own does, and goes back from a withdrawn release.
    pub unattended: bool,
    /// The time of day, `HH:MM`, an unattended machine restarts into an
    /// update it staged; without one, it waits for a restart.
    pub restart: Option<String>,
    /// The versions held back.
    pub hold: Hold,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            channel: DEFAULT_CHANNEL.into(),
            server: SERVER.into(),
            canary: false,
            check: Check::Notify,
            apps: Apps::Notify,
            unattended: false,
            restart: None,
            hold: Hold::default(),
        }
    }
}

impl Settings {
    /// Reads `updates.toml`'s text.
    ///
    /// # Errors
    ///
    /// If it is not TOML of these settings, names no channel there is,
    /// or a server that is not an http or https address.
    pub fn parse(text: &str) -> Result<Settings, String> {
        let file: SettingsFile = toml::from_str(text).map_err(|err| err.to_string())?;
        let channel = file.channel.unwrap_or_else(|| DEFAULT_CHANNEL.into());
        check_channel(&channel)?;
        let mut server = file.server.unwrap_or_else(|| SERVER.into());
        if !server.starts_with("https://") && !server.starts_with("http://") {
            return Err(format!("server = \"{server}\" is not an https:// or http:// address"));
        }
        if server.contains(['"', '\\', ' ']) {
            return Err(format!("server = {server:?} is not an address"));
        }
        if !server.ends_with('/') {
            server.push('/');
        }
        tuf::Url::parse(&server).map_err(|err| format!("server = \"{server}\": {err}"))?;
        if let Some(restart) = &file.restart
            && minutes_of_day(restart).is_none()
        {
            return Err(format!("restart = \"{restart}\" is not a time of day, HH:MM"));
        }
        if file.restart.is_some() && file.unattended != Some(true) {
            return Err("restart is for an unattended machine: set unattended = true too".into());
        }
        Ok(Settings {
            channel,
            server,
            canary: file.canary.unwrap_or(false),
            check: file.check.unwrap_or_default(),
            apps: file.apps.unwrap_or_default(),
            unattended: file.unattended.unwrap_or(false),
            restart: file.restart,
            hold: file.hold.unwrap_or_default(),
        })
    }

    /// This system's settings, or the defaults where there are none.
    ///
    /// # Errors
    ///
    /// If the file cannot be read or is not right, naming it.
    pub fn load() -> Result<Settings, Fail> {
        match std::fs::read_to_string(SETTINGS) {
            Ok(text) => Settings::parse(&text).map_err(|why| fail(format!("{SETTINGS}: {why}"))),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(Settings::default()),
            Err(err) => Err(util::failed(&SETTINGS, err)),
        }
    }

    /// The file's text: the channel, and the server when it is not the
    /// project's.
    #[must_use]
    pub fn text(&self) -> String {
        let mut text = format!(
            "# Which releases this system is offered: stable, beta or nightly\n# (sysupdate(8)).\nchannel = \"{}\"\n",
            self.channel
        );
        if self.server != SERVER {
            text.push_str(&format!("server = \"{}\"\n", self.server));
        }
        if self.canary {
            text.push_str("canary = true\n");
        }
        match self.check {
            Check::Notify => {}
            Check::Download => text.push_str("check = \"download\"\n"),
            Check::Off => text.push_str("check = \"off\"\n"),
        }
        if self.apps == Apps::Auto {
            text.push_str("apps = \"auto\"\n");
        }
        if self.unattended {
            text.push_str("unattended = true\n");
        }
        if let Some(restart) = &self.restart {
            text.push_str(&format!("restart = \"{restart}\"\n"));
        }
        if self.hold != Hold::default() {
            text.push_str("\n# Versions held back until `update unhold` (update(8)).\n[hold]\n");
            for (item, version) in
                [("system", &self.hold.system), ("firmware", &self.hold.firmware)]
            {
                if let Some(version) = version {
                    text.push_str(&format!("{item} = \"{version}\"\n"));
                }
            }
        }
        text
    }

    /// Writes the settings, whole or not at all.
    ///
    /// # Errors
    ///
    /// If the file cannot be written.
    pub fn save(&self) -> Result<(), Fail> {
        let path = Path::new(SETTINGS);
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|err| util::failed(&dir.display(), err))?;
        }
        let partial = path.with_extension("toml.partial");
        std::fs::write(&partial, self.text())
            .and_then(|()| std::fs::rename(&partial, path))
            .map_err(|err| util::failed(&SETTINGS, err))
    }
}

/// The minutes since midnight of a time of day, `HH:MM`.
#[must_use]
pub fn minutes_of_day(time: &str) -> Option<u32> {
    let (hours, minutes) = time.split_once(':')?;
    let (hours, minutes): (u32, u32) = (hours.parse().ok()?, minutes.parse().ok()?);
    (hours < 24 && minutes < 60 && time.len() == 5).then_some(hours * 60 + minutes)
}

/// Refuses a channel there is not.
///
/// # Errors
///
/// If `name` is not one of [`CHANNELS`].
pub fn check_channel(name: &str) -> Result<(), String> {
    if CHANNELS.contains(&name) {
        Ok(())
    } else {
        Err(format!("there is no {name} channel; the channels are {}", CHANNELS.join(", ")))
    }
}

/// The update server, read through the TUF client from what this system
/// trusts.
pub struct Client {
    updater: tuf::Updater,
    base: tuf::Url,
    /// The settings it was made from.
    pub settings: Settings,
}

impl Client {
    /// Reads the server's newest metadata, from the trusted metadata this
    /// system keeps, or from the root it was built with the first time.
    ///
    /// # Errors
    ///
    /// If the server cannot be reached, or its metadata is not signed
    /// by keys this system trusts, has expired, or is older than what
    /// this system has read before.
    pub fn connect(settings: Settings) -> Result<Client, Fail> {
        let dir = metadata_dir();
        if !dir.join("root.json").is_file() {
            let root =
                std::fs::read(TRUSTED_ROOT).map_err(|err| util::failed(&TRUSTED_ROOT, err))?;
            tuf::init(&dir, &root).map_err(|err| fail(err.to_string()))?;
        }
        let url = |sub: &str| {
            tuf::Url::parse(&format!("{}{sub}", settings.server))
                .map_err(|err| fail(format!("{}: {err}", settings.server)))
        };
        let base = url("")?;
        let mut updater = tuf::Updater::new(&dir, &url("metadata/")?, Some(&url("targets/")?))
            .map_err(|err| fail(format!("the trusted metadata in {}: {err}", dir.display())))?;
        updater.refresh().map_err(|err| refused(&settings.server, &err))?;
        Ok(Client { updater, base, settings })
    }

    /// The target `path`'s bytes, checked against the signed targets, or
    /// `None` if the server does not list it.
    fn target(&mut self, path: &str) -> Result<Option<Vec<u8>>, Fail> {
        let server = self.settings.server.clone();
        let Some(target) = self.updater.target(path).map_err(|err| refused(&server, &err))? else {
            return Ok(None);
        };
        let file = lists_dir().join(path.replace('/', "-"));
        if !tuf::Updater::cached(&file, &target) {
            self.updater
                .download_target(path, &target, &file)
                .map_err(|err| refused(&server, &err))?;
        }
        std::fs::read(&file).map(Some).map_err(|err| util::failed(&file.display(), err))
    }

    /// The channel this system follows: the release it offers.
    ///
    /// # Errors
    ///
    /// If the server has no such channel, or its file does not check.
    pub fn channel(&mut self) -> Result<Channel, Fail> {
        let name = self.settings.channel.clone();
        let bytes = self
            .target(&channel_path(&name))?
            .ok_or_else(|| fail(format!("{} has no {name} channel", self.settings.server)))?;
        Channel::parse(&bytes, &name).map_err(fail)
    }

    /// The list of release `version`, or `None` if the server does not
    /// offer it.
    ///
    /// # Errors
    ///
    /// If the list does not check, or is another release's.
    pub fn release(&mut self, version: &str) -> Result<Option<Release>, Fail> {
        let Some(bytes) = self.target(&target_path(version))? else { return Ok(None) };
        let release = Release::parse(&bytes).map_err(fail)?;
        if release.version != version {
            return Err(fail(format!(
                "{} is the list of Dukovo {}",
                target_path(version),
                release.version
            )));
        }
        Ok(Some(release))
    }

    /// Which waves of release `version` are open, or `None` if the
    /// server signs no rollout for it, which opens it to every machine
    /// (ADR 0469).
    ///
    /// # Errors
    ///
    /// If the rollout does not check, or is another release's.
    pub fn rollout(&mut self, version: &str) -> Result<Option<Rollout>, Fail> {
        let path = rollout::target_path(version);
        let Some(bytes) = self.target(&path)? else { return Ok(None) };
        let rollout: Rollout =
            serde_json::from_slice(&bytes).map_err(|err| fail(format!("{path}: {err}")))?;
        if rollout.release != version || rollout.wave > rollout::LAST {
            return Err(fail(format!("{path} is not a rollout of Dukovo {version}")));
        }
        Ok(Some(rollout))
    }

    /// The withdrawal of release `version`, if the release keys signed
    /// one (ADR 0469).
    ///
    /// # Errors
    ///
    /// If the withdrawal does not check, or is another release's.
    pub fn withdrawal(&mut self, version: &str) -> Result<Option<Withdrawal>, Fail> {
        let path = rollout::withdrawal_path(version);
        let Some(bytes) = self.target(&path)? else { return Ok(None) };
        let withdrawal = Withdrawal::parse(&bytes).map_err(fail)?;
        if withdrawal.release != version {
            return Err(fail(format!("{path} withdraws Dukovo {}", withdrawal.release)));
        }
        Ok(Some(withdrawal))
    }

    /// The firmware set `version`, as signed, or `None` if the server
    /// does not offer it (ADR 0468).
    ///
    /// # Errors
    ///
    /// If the list does not check, or is another set's.
    pub fn firmware(&mut self, version: &str) -> Result<Option<(Set, Vec<u8>)>, Fail> {
        let path = firmware::target_path(version);
        let Some(bytes) = self.target(&path)? else { return Ok(None) };
        let set = Set::parse(&bytes).map_err(fail)?;
        if set.firmware != version {
            return Err(fail(format!("{path} is the list of firmware {}", set.firmware)));
        }
        Ok(Some((set, bytes)))
    }

    /// Where the file of SHA-256 `sha256` is on the server.
    fn file_url(&self, sha256: &str) -> Result<tuf::Url, Fail> {
        self.base.resolve(&format!("files/{sha256}")).map_err(|err| fail(err.to_string()))
    }

    /// Fetches the file of SHA-256 `sha256`, `size` bytes, into `dir` as
    /// [`fetch`] does: whole, checked, and going on from a cut fetch.
    ///
    /// # Errors
    ///
    /// As [`fetch`].
    pub fn fetch_file(&self, sha256: &str, size: u64, dir: &Path) -> Result<(), Fail> {
        fetch_file(&self.file_url(sha256)?, sha256, size, dir)
    }
}

/// A TUF client's failure, in words for the owner: the server and why.
fn refused(server: &str, err: &tuf::Error) -> Fail {
    fail(format!("{server}: {err}; nothing was changed"))
}

/// What a check finds.
pub enum Plan {
    /// The channel offers no newer release: it offers `offered`.
    Current {
        /// The channel's release.
        offered: String,
    },
    /// The channel's release `offered` is newer, and reaches this machine
    /// in a wave not open yet (ADR 0469).
    Waiting {
        /// The channel's release.
        offered: String,
        /// The day after its release this machine's wave opens.
        day: i64,
        /// Why no further wave opens now, if one is held.
        held: Option<String>,
    },
    /// The channel's release `offered` is newer, and held back at
    /// `held` (ADR 0502).
    Held {
        /// The channel's release.
        offered: String,
        /// The version held back.
        held: String,
    },
    /// The channel's release `offered` was withdrawn.
    Withdrawn {
        /// The release.
        offered: String,
        /// Why.
        reason: String,
    },
    /// An update: `release`, the channel's release `offered` or a bridge
    /// to it.
    Update {
        /// The channel's release.
        offered: String,
        /// The oldest version that updates to `offered` directly.
        from: String,
        /// The release to stage now.
        release: Release,
    },
}

/// What to stage for a system running `running`: the release the channel
/// offers or, with `version`, that release; or the bridge it goes
/// through first.
///
/// # Errors
///
/// If a version cannot be compared, a release the way needs is not on
/// the server, or the bridges go round.
pub fn plan(client: &mut Client, running: &str, version: Option<&str>) -> Result<Plan, Fail> {
    let offered = match version {
        Some(version) => version.to_string(),
        None => client.channel()?.version,
    };
    let from = version_of(running)?;
    if version_of(&offered)? <= from {
        return Ok(Plan::Current { offered });
    }
    if let Some(withdrawal) = client.withdrawal(&offered)? {
        return Ok(Plan::Withdrawn { offered, reason: withdrawal.reason });
    }
    // A version held back is not taken; a newer one is (ADR 0502).
    if version.is_none()
        && let Some(held) = client.settings.hold.system.clone()
        && version_of(&offered)? <= version_of(&held)?
    {
        return Ok(Plan::Held { offered, held });
    }
    // The owner who names a version skips the waves: they asked for it.
    if version.is_none()
        && let Some(rollout) = client.rollout(&offered)?
    {
        let number = wave_number();
        if !rollout::reaches(&rollout, number, client.settings.canary) {
            let day = rollout::day_for(number);
            return Ok(Plan::Waiting { offered, day, held: rollout.held });
        }
    }
    let server = client.settings.server.clone();
    let mut release = client
        .release(&offered)?
        .ok_or_else(|| fail(format!("{server} does not offer Dukovo {offered}")))?;
    let first = release.updates_from.clone();
    for _ in 0..MOST_BRIDGES {
        if from >= version_of(&release.updates_from)? {
            return Ok(Plan::Update { offered, from: first, release });
        }
        let bridge = release.updates_from.clone();
        let next = client.release(&bridge)?.ok_or_else(|| {
            fail(format!(
                "Dukovo {} updates only from {bridge} on, and {server} does not offer {bridge}",
                release.version
            ))
        })?;
        if version_of(&next.version)? <= from
            || version_of(&next.version)? >= version_of(&release.version)?
        {
            return Err(fail(format!(
                "Dukovo {} names {bridge} as the release to update from first, which is not \
                 between it and this system's",
                release.version
            )));
        }
        release = next;
    }
    Err(fail(format!("Dukovo {offered} is more than {MOST_BRIDGES} bridges away")))
}

/// The lines that say the release `offered` reaches this machine in a
/// wave not open yet, on `day` after its release, and why the waves are
/// `held`, if they are.
#[must_use]
pub fn waiting(offered: &str, day: i64, held: Option<&str>) -> Vec<String> {
    let mut lines =
        vec![format!("it reaches machines in waves; this machine's opens on day {day}")];
    lines.extend(held.map(|why| format!("the waves are held: {why}")));
    lines.push(format!("`sysupdate stage --version {offered}` takes it now"));
    lines
}

/// Where this machine keeps its wave number.
pub const WAVE: &str = "/system/state/sysupdate/wave";

/// This machine's number from 0 to 999, which places it in a wave (ADR
/// 0456): drawn the first time it is needed, so that machines cloned from
/// one image draw their own, and kept to itself.
pub fn wave_number() -> u16 {
    if let Some(number) = std::fs::read_to_string(WAVE)
        .ok()
        .and_then(|text| text.trim().parse::<u16>().ok())
        .filter(|n| *n < 1000)
    {
        return number;
    }
    let mut bytes = [0u8; 2];
    let drawn = std::fs::File::open("/dev/urandom").and_then(|mut f| f.read_exact(&mut bytes));
    let number = if drawn.is_ok() { u16::from_le_bytes(bytes) % 1000 } else { 999 };
    if let Some(dir) = Path::new(WAVE).parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(WAVE, format!("{number}\n"));
    number
}

/// The releases between `older` and `newer`, oldest first, that the
/// update server offered when this system last read its signed targets:
/// a rollback from `newer` to `older` goes past them, beyond the one
/// step the settings' two-version rule covers (ADR 0454). Without
/// targets read, as on a system updated only from files, none.
#[must_use]
pub fn releases_between(older: &str, newer: &str) -> Vec<String> {
    let Ok(bytes) = std::fs::read(metadata_dir().join("targets.json")) else { return Vec::new() };
    let Ok(targets) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
        return Vec::new();
    };
    between(&targets, older, newer)
}

/// The system releases signed `targets` lists between `older` and
/// `newer`, oldest first.
fn between(targets: &serde_json::Value, older: &str, newer: &str) -> Vec<String> {
    let (Some(low), Some(high)) = (Version::parse(older), Version::parse(newer)) else {
        return Vec::new();
    };
    let mut between: Vec<String> = targets["signed"]["targets"]
        .as_object()
        .into_iter()
        .flatten()
        .filter_map(|(path, _)| path.strip_prefix("system/")?.strip_suffix(".json"))
        .filter(|version| Version::parse(version).is_some_and(|v| v > low && v < high))
        .map(str::to_string)
        .collect();
    between.sort_by(|a, b| Version::parse(a).cmp(&Version::parse(b)));
    between
}

/// `text` as a version, to compare.
fn version_of(text: &str) -> Result<Version<'_>, Fail> {
    Version::parse(text).ok_or_else(|| fail(format!("{text} is not a version this system reads")))
}

/// The files a stage of a release needs that this system lacks.
#[derive(Debug, Default)]
pub struct Needed {
    /// Each file to fetch, by SHA-256, and its size.
    pub fetch: Vec<(String, u64)>,
    /// Each file the running slot holds at another place, to copy.
    pub copy: Vec<(String, PathBuf)>,
    /// The bytes to fetch.
    pub bytes: u64,
}

/// What of `release` the slot at `slot` and the files fetched before
/// lack: a file the slot holds at the same place with the same contents
/// is kept by the stage, and one it holds elsewhere is copied. Only
/// files whose size matches are read and hashed.
///
/// # Errors
///
/// If the slot cannot be read.
pub fn needed(slot: &Path, release: &Release) -> Result<Needed, Fail> {
    let mut by_size: HashMap<u64, Vec<PathBuf>> = HashMap::new();
    index(slot, &mut by_size)?;
    let mut hashes: HashMap<PathBuf, Option<String>> = HashMap::new();
    let mut hash = |path: &Path| -> Option<String> {
        hashes.entry(path.to_path_buf()).or_insert_with(|| sha256_file(path).ok()).clone()
    };
    let fetched = downloads();
    let mut needed = Needed::default();
    let mut wanted: HashSet<&str> = HashSet::new();
    for entry in &release.files {
        let Some((sha256, size)) = entry.file() else { continue };
        let at = slot.join(&entry.path);
        let same_size = |path: &Path| {
            std::fs::symlink_metadata(path).is_ok_and(|m| m.is_file() && m.len() == size)
        };
        if same_size(&at) && hash(&at).as_deref() == Some(sha256) {
            continue;
        }
        if !wanted.insert(sha256) || same_size(&fetched.join(sha256)) {
            continue;
        }
        let elsewhere = by_size
            .get(&size)
            .and_then(|paths| paths.iter().find(|path| hash(path).as_deref() == Some(sha256)));
        match elsewhere {
            Some(path) => needed.copy.push((sha256.to_string(), path.clone())),
            None => {
                needed.fetch.push((sha256.to_string(), size));
                needed.bytes += size;
            }
        }
    }
    Ok(needed)
}

/// Every regular file beneath `dir`, by size.
fn index(dir: &Path, by_size: &mut HashMap<u64, Vec<PathBuf>>) -> Result<(), Fail> {
    let entries = std::fs::read_dir(dir).map_err(|err| util::failed(&dir.display(), err))?;
    for entry in entries {
        let entry = entry.map_err(|err| util::failed(&dir.display(), err))?;
        let path = entry.path();
        let Ok(meta) = std::fs::symlink_metadata(&path) else { continue };
        if meta.is_dir() {
            index(&path, by_size)?;
        } else if meta.is_file() {
            by_size.entry(meta.len()).or_default().push(path);
        }
    }
    Ok(())
}

/// What [`fetch`] did.
#[derive(Debug, Default)]
pub struct Fetched {
    /// Files fetched from the server.
    pub fetched: usize,
    /// Files copied from elsewhere in the running slot.
    pub copied: usize,
    /// Bytes fetched.
    pub bytes: u64,
}

/// Fetches and copies what `needed` names into [`downloads`], each file
/// whole and checked before it takes its name.
///
/// # Errors
///
/// If a file cannot be fetched after [`TRIES`] tries, is not the file
/// the list names, or cannot be written.
pub fn fetch(client: &Client, needed: &Needed) -> Result<Fetched, Fail> {
    let dir = downloads();
    std::fs::create_dir_all(&dir).map_err(|err| util::failed(&dir.display(), err))?;
    let mut done = Fetched::default();
    for (sha256, from) in &needed.copy {
        let partial = dir.join(format!(".{sha256}.partial"));
        std::fs::copy(from, &partial)
            .and_then(|_| std::fs::rename(&partial, dir.join(sha256)))
            .map_err(|err| util::failed(&from.display(), err))?;
        done.copied += 1;
    }
    for (sha256, size) in &needed.fetch {
        let url = client.file_url(sha256)?;
        fetch_file(&url, sha256, *size, &dir)?;
        done.fetched += 1;
        done.bytes += size;
    }
    Ok(done)
}

/// Why one try at a file failed.
enum Problem {
    /// Worth another try: the connection, or the server, for now.
    Again(String),
    /// Not: the file is not on the server, or is not the file listed.
    Refused(String),
}

/// Fetches the file at `url`, which must be `size` bytes of SHA-256
/// `sha256`, into `dir`, going on from what an earlier try left.
fn fetch_file(url: &tuf::Url, sha256: &str, size: u64, dir: &Path) -> Result<(), Fail> {
    let partial = dir.join(format!(".{sha256}.partial"));
    let mut why = String::new();
    for attempt in 0..TRIES {
        if attempt > 0 {
            std::thread::sleep(WAIT * 2u32.pow(attempt - 1));
        }
        match fetch_once(url, sha256, size, &partial) {
            Ok(()) => {
                let to = dir.join(sha256);
                return std::fs::rename(&partial, &to)
                    .map_err(|err| util::failed(&to.display(), err));
            }
            Err(Problem::Refused(refused)) => {
                let _ = std::fs::remove_file(&partial);
                return Err(fail(refused));
            }
            Err(Problem::Again(again)) => why = again,
        }
    }
    Err(fail(format!("{url}: {why}; tried {TRIES} times, and a later fetch goes on from there")))
}

/// One try at a file, from the end of `partial`.
fn fetch_once(url: &tuf::Url, sha256: &str, size: u64, partial: &Path) -> Result<(), Problem> {
    let again = |what: &dyn std::fmt::Display| Problem::Again(what.to_string());
    let refused = || {
        Problem::Refused(format!(
            "{url} is not the file the signed list names: it was refused, and nothing was \
             changed"
        ))
    };
    let mut have = std::fs::metadata(partial).map_or(0, |m| m.len());
    if have >= size {
        let _ = std::fs::remove_file(partial);
        have = 0;
    }
    let (mut body, part) = match tuf::http::get_from(url, have) {
        Ok(answer) => answer,
        Err(tuf::http::Error::NotFound(status)) => {
            return Err(Problem::Refused(format!(
                "{url}: the server does not hold this file of the release (HTTP {status})"
            )));
        }
        Err(err) => return Err(again(&err)),
    };
    let mut context = ring::digest::Context::new(&ring::digest::SHA256);
    let mut written = 0;
    let mut file = if part {
        let mut kept = Vec::new();
        std::fs::File::open(partial)
            .and_then(|mut f| f.read_to_end(&mut kept))
            .map_err(|err| again(&err))?;
        context.update(&kept);
        written = kept.len() as u64;
        std::fs::OpenOptions::new().append(true).open(partial).map_err(|err| again(&err))?
    } else {
        std::fs::File::create(partial).map_err(|err| again(&err))?
    };
    let mut buffer = vec![0; 256 * 1024];
    loop {
        let n = match body.read(&mut buffer) {
            Ok(0) => break,
            Ok(n) => n,
            Err(err) => return Err(again(&err)),
        };
        if written + n as u64 > size {
            return Err(refused());
        }
        file.write_all(&buffer[..n]).map_err(|err| again(&err))?;
        context.update(&buffer[..n]);
        written += n as u64;
    }
    if written != size {
        return Err(again(&format!("cut short at {written} of {size} bytes")));
    }
    if hex(context.finish().as_ref()) != sha256 {
        return Err(refused());
    }
    file.sync_all().map_err(|err| again(&err))
}

/// What the server offers, in words: the release the channel offers, or
/// the one asked for by `version`.
#[must_use]
pub fn offers(settings: &Settings, version: Option<&str>, offered: &str) -> String {
    match version {
        Some(_) => format!("Dukovo {offered} was asked for"),
        None => format!("the {} channel offers Dukovo {offered}", settings.channel),
    }
}

/// `MiB` for a count of bytes, to one place.
#[must_use]
pub fn mib(bytes: u64) -> String {
    format!("{:.1} MiB", bytes as f64 / 1_048_576.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_releases_between_two_come_from_the_signed_targets_in_order() {
        let targets = serde_json::json!({ "signed": { "targets": {
            "system/0.16.0.json": {}, "system/0.14.1.json": {}, "system/0.15.0.json": {},
            "system/0.14.0.json": {}, "system/0.16.0.rollout.json": {},
            "channels/stable.json": {}, "firmware/2026.2.1.json": {}
        } } });
        assert_eq!(between(&targets, "0.14.0", "0.16.0"), ["0.14.1", "0.15.0"]);
        assert!(between(&targets, "0.15.0", "0.16.0").is_empty());
        assert!(between(&targets, "0.14.1", "0.15.0").is_empty());
        assert!(between(&targets, "not", "0.16.0").is_empty());
    }

    #[test]
    fn settings_read_with_defaults_and_refuse_strange_values() {
        assert_eq!(Settings::parse("").unwrap(), Settings::default());
        let beta =
            Settings::parse("channel = \"beta\"\nserver = \"http://10.0.2.2:8080/system\"\n")
                .unwrap();
        assert_eq!(beta.channel, "beta");
        assert_eq!(beta.server, "http://10.0.2.2:8080/system/");
        assert_eq!(Settings::parse(&beta.text()).unwrap(), beta);
        assert!(!Settings::default().text().contains("server"));
        assert!(Settings::parse("channel = \"lts\"").unwrap_err().contains("no lts channel"));
        assert!(Settings::parse("server = \"ftp://x/\"").unwrap_err().contains("not an https"));
        assert!(Settings::parse("server = \"http://a b/\"").is_err());
        // What updated does, and what is held back (ADR 0502).
        let unattended = Settings::parse(
            "check = \"download\"\napps = \"auto\"\nunattended = true\nrestart = \"03:30\"\n\
             [hold]\nsystem = \"0.15.2\"\n",
        )
        .unwrap();
        assert_eq!(
            (unattended.check, unattended.apps, unattended.unattended),
            (Check::Download, Apps::Auto, true)
        );
        assert_eq!(unattended.hold.system.as_deref(), Some("0.15.2"));
        assert_eq!(Settings::parse(&unattended.text()).unwrap(), unattended);
        assert!(Settings::parse("unattended = true\nrestart = \"25:00\"").is_err());
        assert!(Settings::parse("restart = \"03:00\"").unwrap_err().contains("unattended"));
        assert!(Settings::parse("check = \"sometimes\"").is_err());
        assert_eq!(minutes_of_day("03:30"), Some(210));
        assert!(Settings::parse("chanel = \"beta\"").is_err());
    }

    fn release(version: &str, updates_from: &str, files: &[(&str, &[u8])]) -> Release {
        let mut text = format!(
            "{{\"format\":1,\"version\":\"{version}\",\"commit\":\"c\",\"toolchain\":\"t\",\
             \"built\":\"2026-11-06T00:00:00Z\",\"updates_from\":\"{updates_from}\",\"files\":["
        );
        let entries: Vec<String> = files
            .iter()
            .map(|(path, data)| {
                if let Some(folder) = path.strip_suffix('/') {
                    return format!(
                        "{{\"path\":\"{folder}\",\"kind\":\"directory\",\"mode\":\"0755\"}}"
                    );
                }
                let sha = hex(ring::digest::digest(&ring::digest::SHA256, data).as_ref());
                format!(
                    "{{\"path\":\"{path}\",\"kind\":\"file\",\"mode\":\"0644\",\"size\":{},\"sha256\":\"{sha}\"}}",
                    data.len()
                )
            })
            .collect();
        text.push_str(&entries.join(","));
        text.push_str("]}");
        Release::parse(text.as_bytes()).unwrap()
    }

    #[test]
    fn only_files_the_slot_lacks_are_needed() {
        let dir = std::env::temp_dir().join(format!("sysupdate-needed-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("slot/moved")).unwrap();
        std::fs::write(dir.join("slot/same"), b"same").unwrap();
        std::fs::write(dir.join("slot/changed"), b"old!").unwrap();
        std::fs::write(dir.join("slot/moved/from"), b"moved contents").unwrap();
        let release = release(
            "0.15.0",
            "0.14.0",
            &[
                ("changed", b"new!"),
                ("moved/", b""),
                ("moved/to", b"moved contents"),
                ("new", b"brand new"),
                ("same", b"same"),
                ("twice", b"brand new"),
            ],
        );
        let needed = needed(&dir.join("slot"), &release).unwrap();
        // "new!" and "brand new" fetched, once each; the moved file copied.
        assert_eq!(needed.fetch.len(), 2);
        assert_eq!(needed.bytes, 4 + 9);
        assert_eq!(needed.copy.len(), 1);
        assert_eq!(needed.copy[0].1, dir.join("slot/moved/from"));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
