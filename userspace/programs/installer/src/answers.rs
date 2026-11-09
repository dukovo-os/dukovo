// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The answers file (install-answers(5)): every question the installer
//! asks, answered in JSON, so that an install runs by itself and can be
//! repeated. The installer saves the answers of every install as
//! `/system/settings/install/answers.json`, without the passwords' hashes
//! unless the file asks to keep them.
//!
//! [`Answers::parse`] reads one and [`Answers::check`] says what is wrong
//! with it, naming the key, before anything is written to a disk.

use std::collections::BTreeMap;

use manifest::feature;
use serde::{Deserialize, Serialize};

/// The version of the answers file this installer reads.
pub const VERSION: u32 = 1;

/// The machine's name when the answers give none, as hostname(5) has it.
pub const DEFAULT_HOSTNAME: &str = "dukovo";

/// The time zone when the answers give none.
pub const DEFAULT_TIME_ZONE: &str = "UTC";

/// The most swap the installer makes, in GiB.
pub const SWAP_MAX_GIB: u32 = 64;

/// The keys of what later parts of the installer will add: SSH, updates
/// and closed firmware.
const LATER: [&str; 3] = ["ssh", "updates", "firmware"];

/// An answers file.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Answers {
    /// The file's version, [`VERSION`].
    pub version: u32,
    /// The keyboard layout, as keymap(1) names it.
    #[serde(default = "default_keyboard")]
    pub keyboard: String,
    /// The language, as a BCP 47 tag.
    #[serde(default = "default_language")]
    pub language: String,
    /// The disk and how it is laid out.
    pub disk: Disk,
    /// The machine's name, [`DEFAULT_HOSTNAME`] if not given.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hostname: Option<String>,
    /// The time zone, a name in the tz database, [`DEFAULT_TIME_ZONE`] if
    /// not given.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub time_zone: Option<String>,
    /// How each network card is set up, by its name; a card not named
    /// asks DHCP, as netd(8) has it.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub network: BTreeMap<String, Network>,
    /// The administrator's account.
    pub admin: Account,
    /// Other people's accounts, ordinary users.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub users: Vec<Account>,
    /// Whether the saved answers keep the passwords' hashes, so that a
    /// repeat install sets the same passwords.
    #[serde(default, skip_serializing_if = "is_false")]
    pub keep_password_hashes: bool,
    /// What the machine is for (ADR 0317): [`Profile::Minimal`] if not
    /// given.
    #[serde(default)]
    pub profile: Profile,
    /// The features added to the profile's, and taken from them.
    #[serde(default, skip_serializing_if = "Features::is_empty")]
    pub features: Features,
    /// Whether Dukovo Admin, the web console, is turned on: the profile's
    /// choice if not given. Saved for release 0.16, which brings it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub web_console: Option<bool>,
    /// Whether the installed system keeps a console on the first serial
    /// port, with `console=ttyS0` on its kernel's command line (ADR
    /// 0318): its sign-in and the startup-trouble menu show there too. As
    /// the installer itself runs, on the serial port or not, if not given.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub serial_console: Option<bool>,
    /// The screen reader, text size, sticky keys and slow keys of the
    /// installed system (ADR 0422): as they are when not given, but for
    /// the screen reader, which is on if the installer runs with it.
    #[serde(default, skip_serializing_if = "Accessibility::is_default")]
    pub accessibility: Accessibility,
    /// Every other key: those of [`LATER`], which this installer takes
    /// none of yet, and mistyped ones, which [`Answers::check`] names.
    /// (serde cannot refuse unknown keys beside a flattened map.)
    #[serde(flatten)]
    pub later: BTreeMap<String, serde_json::Value>,
}

/// What the machine is for: a starting set of features (ADR 0317).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Profile {
    /// The base system, Nukesh, the administration commands and updates,
    /// with nothing listening.
    #[default]
    Minimal,
    /// Minimal and the web console, with every feature offered.
    Server,
    /// Server, set up at first boot from the cloud's data, with the web
    /// console off.
    Cloud,
}

impl Profile {
    /// Every profile, in the order the installer offers them.
    pub const ALL: [Profile; 3] = [Profile::Minimal, Profile::Server, Profile::Cloud];

    /// Its name in the answers file.
    #[must_use]
    pub fn id(self) -> &'static str {
        match self {
            Profile::Minimal => "minimal",
            Profile::Server => "server",
            Profile::Cloud => "cloud",
        }
    }

    /// Whether it turns the web console on unless the answers say.
    #[must_use]
    pub fn web_console(self) -> bool {
        self == Profile::Server
    }

    /// The features it starts with, ticked on the Software page. Every
    /// 1.0 profile starts with none, as the plan has it: each feature is
    /// offered, off until picked. Desktop, in release 1.1, starts with
    /// some, which `remove` takes away.
    #[must_use]
    pub fn features(self) -> &'static [&'static str] {
        &[]
    }
}

/// The installed system's accessibility (ADR 0422): the screen reader
/// (reader(1)) and the consoles' text size and keys
/// (accessibility.toml(5)).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Accessibility {
    /// Whether the screen reader starts with the system: as the
    /// installer runs, with speech or not, if not given.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub screen_reader: Option<bool>,
    /// How big the consoles' text is.
    #[serde(default, skip_serializing_if = "TextSize::is_normal")]
    pub text_size: TextSize,
    /// Whether Shift, Ctrl and Alt, pressed and let go alone, hold for
    /// the next key.
    #[serde(default, skip_serializing_if = "is_false")]
    pub sticky_keys: bool,
    /// Whether a key counts only once held for a moment.
    #[serde(default, skip_serializing_if = "is_false")]
    pub slow_keys: bool,
}

/// What the consoles' settings file says first, as the installer writes
/// it.
const CONSOLE_HEADER: &str = "\
# The consoles' text size and keys (accessibility.toml(5)), as the install
# set them. consoled reads it as it starts and on `service consoled
# reload`, and writes the text size and sticky keys as they are changed
# from the keyboard.
";

impl Accessibility {
    /// Whether every setting is its default, which an answers file need
    /// not give.
    #[must_use]
    pub fn is_default(&self) -> bool {
        *self == Accessibility::default()
    }

    /// Whether the screen reader starts with the installed system: as the
    /// answers say, or as the installer runs, `with_speech`.
    #[must_use]
    pub fn screen_reader(&self, with_speech: bool) -> bool {
        self.screen_reader.unwrap_or(with_speech)
    }

    /// Whether the consoles' settings are all their defaults, which the
    /// system's default settings file already gives.
    #[must_use]
    pub fn consoles_as_default(&self) -> bool {
        self.text_size == TextSize::Normal && !self.sticky_keys && !self.slow_keys
    }

    /// The consoles' settings file, accessibility.toml(5), with these
    /// settings, and slow keys' delay in milliseconds if `delay` gives
    /// one.
    #[must_use]
    pub fn console_file(&self, delay: Option<u64>) -> String {
        let mut text = format!(
            "{CONSOLE_HEADER}text-size = \"{}\"\nsticky-keys = {}\nslow-keys = {}\n",
            self.text_size.id(),
            self.sticky_keys,
            self.slow_keys
        );
        if let Some(delay) = delay {
            text.push_str(&format!("slow-keys-delay = {delay}\n"));
        }
        text
    }
}

/// How big the consoles' text is (accessibility.toml(5)): the font the
/// screen's size chooses, or one or two steps bigger.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum TextSize {
    /// The font the screen's size chooses.
    #[default]
    Normal,
    /// A step bigger.
    Larger,
    /// Two steps bigger.
    Largest,
}

impl TextSize {
    /// Every size, from the smallest.
    pub const ALL: [TextSize; 3] = [TextSize::Normal, TextSize::Larger, TextSize::Largest];

    /// Its name in the answers file and accessibility.toml(5).
    #[must_use]
    pub fn id(self) -> &'static str {
        match self {
            TextSize::Normal => "normal",
            TextSize::Larger => "larger",
            TextSize::Largest => "largest",
        }
    }

    /// The size named `id`, if one is.
    #[must_use]
    pub fn from_id(id: &str) -> Option<TextSize> {
        TextSize::ALL.into_iter().find(|size| size.id() == id)
    }

    #[expect(
        clippy::trivially_copy_pass_by_ref,
        reason = "serde's skip_serializing_if passes a reference"
    )]
    fn is_normal(&self) -> bool {
        *self == TextSize::Normal
    }
}

/// Features added to a profile's, and taken from them, by their IDs, as
/// feature(8) names them.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Features {
    /// The features added.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub add: Vec<String>,
    /// The profile's features left out.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub remove: Vec<String>,
}

impl Features {
    /// Whether it adds and takes away nothing.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.add.is_empty() && self.remove.is_empty()
    }
}

/// The disk the system goes on.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Disk {
    /// How it is laid out.
    pub layout: Layout,
    /// The disk's name, as in `/devices` (`vda`, `nvme0n1`); `/dev/` or
    /// `/devices/` in front of it is left out.
    pub target: String,
    /// The second disk, for the mirror layout.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mirror: Option<String>,
    /// The swap partition's size in GiB, 0 for none, in the guided,
    /// free-space, mirror and next-to-Windows layouts; the manual layout
    /// lists its own. The mirror layout makes it on each disk.
    #[serde(default)]
    pub swap_gib: u32,
    /// The manual layout's changes, a disk at a time, as the installer's
    /// rdisk screen makes them: the target's, which holds the root and
    /// the EFI system partition, and any other disk's.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub changes: Vec<Change>,
    /// The space Dukovo takes next to Windows, in GiB, for the layout
    /// next to it: what Windows' volume is shrunk by.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub space_gib: Option<u32>,
}

/// What the manual layout changes on one disk. Partitions it does not
/// name are kept as they are.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Change {
    /// The disk's name, as `target`'s.
    pub disk: String,
    /// Whether a new, empty GPT replaces the disk's table first, as for a
    /// blank disk: every partition it had goes.
    #[serde(default, skip_serializing_if = "is_false")]
    pub new_table: bool,
    /// The partitions taken out, by number.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub delete: Vec<u32>,
    /// The partitions made, each with where it goes, and the partitions
    /// kept but formatted, with neither.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub partitions: Vec<Part>,
    /// Windows' partition, shrunk to make room next to it: the layout next
    /// to Windows works it out, and an answers file cannot ask for it.
    #[serde(skip)]
    pub shrink: Option<Shrink>,
}

/// A partition shrunk, after the filesystem in it: Windows', once
/// resize-ntfs(8) has shrunk its volume to fit.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Shrink {
    /// Its number.
    pub number: u32,
    /// Its new size, in the disk's logical sectors.
    pub sectors: u64,
}

/// A partition the manual layout makes or formats.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Part {
    /// Its number in the disk's table.
    pub number: u32,
    /// A new partition's first sector, in the disk's logical sectors.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub first: Option<u64>,
    /// A new partition's size, in the disk's logical sectors.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sectors: Option<u64>,
    /// A new partition's name in the GPT.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// What it is for.
    #[serde(rename = "use")]
    pub role: Role,
    /// The filesystem a data partition gets; none leaves a new one empty.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub format: Option<Format>,
}

impl Part {
    /// Whether the install makes it, rather than formatting one there is.
    #[must_use]
    pub fn is_new(&self) -> bool {
        self.first.is_some()
    }
}

/// What a partition is for, which says its type and its filesystem.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Role {
    /// Dukovo's root: Btrfs, mounted at `/`.
    Root,
    /// The EFI system partition: FAT32, mounted at `/system/boot`, which
    /// holds UEFI's GRUB and its menu.
    Efi,
    /// Swap, which Spark turns on when it is on the root's disk.
    Swap,
    /// The BIOS boot partition, which holds GRUB for a BIOS.
    BiosBoot,
    /// Someone's data, with the filesystem `format` names; Spark mounts
    /// it at `/volumes/<name>`.
    Data,
}

/// A data partition's filesystem.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Format {
    /// FAT32, which every system reads.
    Fat32,
    /// exFAT, for large files that every system reads.
    Exfat,
    /// Btrfs.
    Btrfs,
}

/// How the disk is laid out.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Layout {
    /// The disk is erased and gets the guided layout: a BIOS boot
    /// partition, an EFI system partition, swap if asked for, and the
    /// Dukovo root.
    Guided,
    /// Dukovo goes in the disk's free space, keeping what is there.
    Free,
    /// The partitions are chosen on the rdisk screen.
    Manual,
    /// Two disks are erased and each gets the guided layout; their roots
    /// are one Btrfs RAID1, so that either disk starts the system alone.
    Mirror,
    /// Dukovo goes next to Windows, which is shrunk.
    Beside,
}

/// How a network card is set up.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(untagged)]
pub enum Network {
    /// `"dhcp"`: it asks DHCP.
    Dhcp(String),
    /// A fixed address.
    Fixed(Fixed),
}

/// A network card's fixed address.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Fixed {
    /// The address with its prefix: `192.168.1.10/24`.
    pub address: String,
    /// The router.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gateway: Option<String>,
    /// DNS servers, up to three.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dns: Vec<String>,
}

/// An account.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Account {
    /// The person's full name.
    pub name: String,
    /// The account's name.
    pub user: String,
    /// The password's Argon2id hash, a PHC string, or `""` for an empty
    /// password. An answers file never holds a password itself.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub password_hash: Option<String>,
}

fn default_keyboard() -> String {
    "us".into()
}

fn default_language() -> String {
    "en-US".into()
}

#[expect(
    clippy::trivially_copy_pass_by_ref,
    reason = "serde's skip_serializing_if passes a reference"
)]
fn is_false(value: &bool) -> bool {
    !*value
}

impl Answers {
    /// Reads an answers file.
    ///
    /// # Errors
    ///
    /// If it is not JSON of the answers' keys, saying where.
    pub fn parse(bytes: &[u8]) -> Result<Answers, String> {
        let answers: Answers = serde_json::from_slice(bytes).map_err(|err| err.to_string())?;
        if answers.version != VERSION {
            return Err(format!(
                "the answers file is of version {}, and this installer reads version {VERSION}",
                answers.version
            ));
        }
        Ok(answers)
    }

    /// The answers as the installed system keeps them: without the
    /// passwords' hashes unless [`Answers::keep_password_hashes`].
    #[must_use]
    pub fn to_saved(&self) -> Vec<u8> {
        let mut saved = self.clone();
        if !saved.keep_password_hashes {
            saved.admin.password_hash = None;
            for user in &mut saved.users {
                user.password_hash = None;
            }
        }
        let mut bytes = serde_json::to_vec_pretty(&saved).unwrap_or_default();
        bytes.push(b'\n');
        bytes
    }

    /// The machine's name.
    #[must_use]
    pub fn hostname(&self) -> &str {
        self.hostname.as_deref().unwrap_or(DEFAULT_HOSTNAME)
    }

    /// The time zone.
    #[must_use]
    pub fn time_zone(&self) -> &str {
        self.time_zone.as_deref().unwrap_or(DEFAULT_TIME_ZONE)
    }

    /// The features the install adds, by their IDs: the profile's, less
    /// those removed, and those added, each once.
    #[must_use]
    pub fn chosen_features(&self) -> Vec<String> {
        let mut chosen: Vec<String> = Vec::new();
        let starting = self.profile.features().iter().map(|id| (*id).to_string());
        for id in starting.chain(self.features.add.iter().cloned()) {
            if !self.features.remove.contains(&id) && !chosen.contains(&id) {
                chosen.push(id);
            }
        }
        chosen
    }

    /// Whether the web console is turned on: as the answers say, or as
    /// the profile does.
    #[must_use]
    pub fn web_console(&self) -> bool {
        self.web_console.unwrap_or(self.profile.web_console())
    }

    /// Whether the installed system keeps a console on the first serial
    /// port: as the answers say, or as the installer runs, `on_serial`.
    #[must_use]
    pub fn serial_console(&self, on_serial: bool) -> bool {
        self.serial_console.unwrap_or(on_serial)
    }

    /// The target disk's name, without `/dev/` or `/devices/`.
    #[must_use]
    pub fn target(&self) -> &str {
        let target = self.disk.target.as_str();
        target.strip_prefix("/dev/").or_else(|| target.strip_prefix("/devices/")).unwrap_or(target)
    }

    /// The mirror layout's second disk's name, without `/dev/` or
    /// `/devices/`, if the answers name one.
    #[must_use]
    pub fn mirror(&self) -> Option<&str> {
        self.disk.mirror.as_deref().map(|mirror| {
            mirror
                .strip_prefix("/dev/")
                .or_else(|| mirror.strip_prefix("/devices/"))
                .unwrap_or(mirror)
        })
    }

    /// The disks the install goes on, as the install's messages name
    /// them: the target, or a mirror's two, `vda and vdb`.
    #[must_use]
    pub fn disks(&self) -> String {
        match self.mirror() {
            Some(mirror) if self.disk.layout == Layout::Mirror => {
                format!("{} and {mirror}", self.target())
            }
            _ => self.target().to_string(),
        }
    }

    /// Checks every answer this installer can check without the disks,
    /// and gives each problem, naming its key.
    ///
    /// # Errors
    ///
    /// The problems found, one a line.
    pub fn check(&self) -> Result<(), Vec<String>> {
        let mut problems = Vec::new();
        let mut problem = |key: &str, why: String| problems.push(format!("{key}: {why}"));
        for key in self.later.keys() {
            if LATER.contains(&key.as_str()) {
                problem(key, "this installer does not take it yet: leave it out".into());
            } else {
                problem(key, "not a key of the answers file: install-answers(5) lists them".into());
            }
        }
        if self.language != "en-US" {
            problem("language", "only en-US for now".into());
        }
        if !is_name(&self.keyboard, 32) {
            problem(
                "keyboard",
                format!("{:?} is not a layout's name: keymap --list lists them", self.keyboard),
            );
        }
        if let Err(why) = check_hostname(self.hostname()) {
            problem("hostname", why);
        }
        if let Err(why) = check_time_zone(self.time_zone()) {
            problem("time_zone", why);
        }
        if let Err(why) = self.check_disk() {
            problem("disk", why);
        }
        for (card, network) in &self.network {
            if let Err(why) = check_network(card, network) {
                problem(&format!("network.{card}"), why);
            }
        }
        if let Err(why) = check_features(&self.features) {
            problem("features", why);
        }
        check_accounts(self, &mut problem);
        if problems.is_empty() { Ok(()) } else { Err(problems) }
    }

    /// Checks the disk's answers. What only the disks can tell, as
    /// whether a partition fits, the install checks before it writes.
    fn check_disk(&self) -> Result<(), String> {
        match (self.disk.layout, self.disk.space_gib) {
            (Layout::Beside, None) => {
                return Err("space_gib gives the space Dukovo takes next to Windows".into());
            }
            (Layout::Beside, Some(_)) | (_, None) => {}
            (_, Some(_)) => return Err("space_gib is for the layout next to Windows only".into()),
        }
        if !is_name(self.target(), 32) {
            return Err(format!(
                "target {:?} is not a disk's name, such as vda: lsblk lists them",
                self.disk.target
            ));
        }
        match (self.disk.layout, self.mirror()) {
            (Layout::Mirror, None) => {
                return Err("mirror names the second disk of the mirror layout".into());
            }
            (Layout::Mirror, Some(mirror)) if !is_name(mirror, 32) => {
                return Err(format!(
                    "mirror {:?} is not a disk's name, such as vdb: lsblk lists them",
                    self.disk.mirror.as_deref().unwrap_or_default()
                ));
            }
            (Layout::Mirror, Some(mirror)) if mirror == self.target() => {
                return Err("mirror names a second disk, not the target again".into());
            }
            (Layout::Mirror, Some(_)) | (_, None) => {}
            (_, Some(_)) => {
                return Err("mirror names the second disk of the mirror layout only".into());
            }
        }
        if self.disk.swap_gib > SWAP_MAX_GIB {
            return Err(format!("swap_gib is at most {SWAP_MAX_GIB}"));
        }
        if self.disk.layout != Layout::Manual {
            if !self.disk.changes.is_empty() {
                return Err("changes are the manual layout's only".into());
            }
            return Ok(());
        }
        if self.disk.swap_gib != 0 {
            return Err("swap_gib is for the guided and free layouts; the manual layout lists \
                        its swap partition in changes"
                .into());
        }
        check_changes(self.target(), &self.disk.changes)
    }
}

/// The Argon2id hash of `password` at `cost`, with a new random salt: a
/// PHC string, as an answers file's `password_hash` and authd's password
/// file hold them.
///
/// # Errors
///
/// If the system gives no random numbers, or Argon2 fails.
pub fn hash_password(password: &[u8], cost: accounts::hash::Cost) -> Result<String, String> {
    let mut salt = [0u8; accounts::hash::SALT];
    ring::rand::SecureRandom::fill(&ring::rand::SystemRandom::new(), &mut salt)
        .map_err(|_| "the system gave no random numbers for the salt".to_string())?;
    accounts::hash::hash(password, &salt, cost)
}

/// Checks the features' IDs: whether the release has each, the install
/// checks before it writes ([`crate::features::check`]).
fn check_features(features: &Features) -> Result<(), String> {
    for (i, id) in features.add.iter().enumerate() {
        feature::check_id(id)?;
        if features.add[..i].contains(id) {
            return Err(format!("{id} is added twice"));
        }
        if features.remove.contains(id) {
            return Err(format!("{id} is both added and removed"));
        }
    }
    for id in &features.remove {
        feature::check_id(id)?;
    }
    Ok(())
}

/// Checks the accounts: their names, their full names and their
/// passwords' hashes, and that no name is used twice.
fn check_accounts(answers: &Answers, problem: &mut impl FnMut(&str, String)) {
    let mut seen = Vec::new();
    let all = std::iter::once(("admin".to_string(), &answers.admin))
        .chain(answers.users.iter().enumerate().map(|(i, user)| (format!("users[{i}]"), user)));
    for (key, account) in all {
        if let Err(why) = accounts::check_name(&account.user) {
            problem(&format!("{key}.user"), why);
        } else if seen.contains(&account.user.as_str()) {
            problem(&format!("{key}.user"), format!("{} is named twice", account.user));
        }
        seen.push(&account.user);
        if let Err(why) = accounts::check_full_name(&account.name) {
            problem(&format!("{key}.name"), why);
        }
        match account.password_hash.as_deref() {
            None => problem(
                &format!("{key}.password_hash"),
                "give the password's hash (dukovo-install --hash-password makes one), or \"\" for no password".into(),
            ),
            Some("") => {}
            Some(hash) if accounts::hash::readable(hash) => {}
            Some(_) => problem(&format!("{key}.password_hash"), "not an Argon2id hash this system reads".into()),
        }
    }
}

/// Checks the manual layout's changes: one root and one EFI system
/// partition, both on the target, each partition either made (with where
/// it goes) or formatted (with neither), and nothing named twice.
fn check_changes(target: &str, changes: &[Change]) -> Result<(), String> {
    if changes.is_empty() {
        return Err("the manual layout lists its changes".into());
    }
    let mut disks = Vec::new();
    for (i, change) in changes.iter().enumerate() {
        let key = format!("changes[{i}]");
        let disk = change.disk.as_str();
        if !is_name(disk, 32) {
            return Err(format!("{key}.disk {disk:?} is not a disk's name, such as vda"));
        }
        if disks.contains(&disk) {
            return Err(format!("{key}.disk: {disk} is named twice"));
        }
        disks.push(disk);
        if change.new_table && !change.delete.is_empty() {
            return Err(format!("{key}: a new table leaves nothing to delete"));
        }
        let mut numbers = Vec::new();
        for (j, part) in change.partitions.iter().enumerate() {
            let key = format!("{key}.partitions[{j}]");
            check_part(part, change, &key)?;
            if numbers.contains(&part.number) {
                return Err(format!("{key}: partition {} is named twice", part.number));
            }
            numbers.push(part.number);
            let on_target = disk == target;
            let only_on_target = match part.role {
                Role::Root | Role::Efi => Some("Dukovo's root and its EFI system partition go"),
                Role::Swap => Some("a swap partition goes, where Spark turns it on,"),
                Role::BiosBoot | Role::Data => None,
            };
            if let Some(what) = only_on_target.filter(|_| !on_target) {
                return Err(format!("{key}: {what} on the target, {target}"));
            }
        }
    }
    let count = |role: Role| {
        changes.iter().flat_map(|change| &change.partitions).filter(|p| p.role == role).count()
    };
    if count(Role::Root) != 1 || count(Role::Efi) != 1 {
        return Err(format!(
            "the manual layout makes or formats one root and one EFI system partition, on {target}"
        ));
    }
    if changes
        .iter()
        .any(|change| change.partitions.iter().filter(|p| p.role == Role::BiosBoot).count() > 1)
    {
        return Err("a disk has one BIOS boot partition at most".into());
    }
    Ok(())
}

/// Checks one partition of a manual change: a new one with its place, or
/// one there is, formatted, which is not also deleted.
fn check_part(part: &Part, change: &Change, key: &str) -> Result<(), String> {
    if part.number == 0 || part.number > 128 {
        return Err(format!("{key}.number is 1 to 128"));
    }
    match (part.first, part.sectors) {
        (Some(_), Some(0)) => return Err(format!("{key}.sectors is more than 0")),
        (Some(_), Some(_)) => {}
        (None, None) => {
            if change.new_table || change.delete.contains(&part.number) {
                return Err(format!(
                    "{key}: partition {} is gone before it can be formatted; give where a new \
                     one goes (first and sectors)",
                    part.number
                ));
            }
            if part.name.is_some() {
                return Err(format!("{key}.name is a new partition's only"));
            }
        }
        _ => return Err(format!("{key}: a new partition has both first and sectors")),
    }
    if part.name.as_ref().is_some_and(|name| name.encode_utf16().count() > 36) {
        return Err(format!("{key}.name is 36 characters at most"));
    }
    match (part.role, part.format) {
        (Role::Data, None) if !part.is_new() => {
            Err(format!("{key}: a data partition there is is named only to format it"))
        }
        (Role::Data, _) | (_, None) => Ok(()),
        (_, Some(_)) => Err(format!("{key}.format is a data partition's only")),
    }
}

/// Checks a machine's name as the approved installer does: letters,
/// digits and `-`, not at its start or end, 63 at most.
fn check_hostname(name: &str) -> Result<(), String> {
    let bytes = name.as_bytes();
    let fits = !bytes.is_empty()
        && bytes.len() <= 63
        && bytes.iter().all(|b| b.is_ascii_alphanumeric() || *b == b'-')
        && bytes.first() != Some(&b'-')
        && bytes.last() != Some(&b'-');
    if fits {
        Ok(())
    } else {
        Err(format!("{name:?}: use letters, digits and - (not at the start or end), 63 at most"))
    }
}

/// Checks that a time zone's name is a path inside the tz database:
/// `Area/City` words, with no `..`.
fn check_time_zone(zone: &str) -> Result<(), String> {
    let words_fit = zone.split('/').all(|word| {
        !word.is_empty()
            && word != "."
            && word != ".."
            && word.bytes().all(|b| b.is_ascii_alphanumeric() || b"+-_".contains(&b))
    });
    if words_fit && zone.len() <= 64 {
        Ok(())
    } else {
        Err(format!("{zone:?} is not a time zone's name, such as America/Denver"))
    }
}

/// Checks a network card's setup.
fn check_network(card: &str, network: &Network) -> Result<(), String> {
    if !is_name(card, 32) {
        return Err(format!("{card:?} is not a network card's name, such as net0"));
    }
    match network {
        Network::Dhcp(word) if word == "dhcp" => Ok(()),
        Network::Dhcp(word) => Err(format!("{word:?}: give \"dhcp\", or an address")),
        Network::Fixed(fixed) => check_fixed(fixed),
    }
}

/// Checks a fixed address, its router and its DNS servers.
fn check_fixed(fixed: &Fixed) -> Result<(), String> {
    let Some((address, prefix)) = fixed.address.split_once('/') else {
        return Err(format!(
            "{:?}: write the address with its prefix, like 192.168.1.10/24",
            fixed.address
        ));
    };
    let address: std::net::IpAddr =
        address.parse().map_err(|_| format!("{:?} is not an address", fixed.address))?;
    let most = if address.is_ipv4() { 32 } else { 128 };
    if !prefix.parse::<u8>().is_ok_and(|p| p <= most) {
        return Err(format!("{:?}: the prefix is 0 to {most}", fixed.address));
    }
    let router = fixed.gateway.iter();
    for server in router.chain(&fixed.dns) {
        server.parse::<std::net::IpAddr>().map_err(|_| format!("{server:?} is not an address"))?;
    }
    if fixed.dns.len() > 3 {
        return Err("up to three DNS servers".into());
    }
    Ok(())
}

/// Whether `text` is a plain name of up to `most` bytes: letters,
/// digits, `-`, `_` and `.`, not starting with `.`.
fn is_name(text: &str, most: usize) -> bool {
    !text.is_empty()
        && text.len() <= most
        && !text.starts_with('.')
        && text.bytes().all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// An answers file as the boot tests give one.
    pub(crate) const EXAMPLE: &str = r#"{
        "version": 1,
        "disk": { "layout": "guided", "target": "/dev/vda", "swap_gib": 1 },
        "hostname": "web01",
        "time_zone": "America/Denver",
        "network": { "net0": { "address": "10.0.0.20/24", "gateway": "10.0.0.1", "dns": ["10.0.0.1"] } },
        "admin": { "name": "Alice Liddell", "user": "alice", "password_hash": "" },
        "users": [ { "name": "Bob", "user": "bob", "password_hash": "" } ]
    }"#;

    #[test]
    fn reads_and_checks_an_answers_file() {
        let answers = Answers::parse(EXAMPLE.as_bytes()).unwrap();
        answers.check().unwrap();
        assert_eq!(answers.target(), "vda");
        assert_eq!(answers.keyboard, "us");
        assert_eq!(answers.hostname(), "web01");
        assert_eq!(answers.disk.layout, Layout::Guided);
    }

    #[test]
    fn names_each_problem_by_its_key() {
        let text = EXAMPLE
            .replace("\"web01\"", "\"-web\"")
            .replace("America/Denver", "../etc")
            .replace("\"bob\"", "\"alice\"")
            .replace("\"swap_gib\": 1", "\"swap_gib\": 65")
            .replace("\"version\": 1,", "\"version\": 1, \"ssh\": true,");
        let problems = Answers::parse(text.as_bytes()).unwrap().check().unwrap_err();
        let keys: Vec<&str> = problems.iter().map(|p| p.split(':').next().unwrap()).collect();
        assert_eq!(keys, ["ssh", "hostname", "time_zone", "disk", "users[0].user"], "{problems:?}");
    }

    #[test]
    fn a_profile_and_its_features() {
        let answers = Answers::parse(EXAMPLE.as_bytes()).unwrap();
        // Left out, the profile is Minimal: no feature, no web console.
        assert_eq!(answers.profile, Profile::Minimal);
        assert!(answers.chosen_features().is_empty());
        assert!(!answers.web_console());
        let text = EXAMPLE.replace(
            "\"version\": 1,",
            r#""version": 1, "profile": "server", "features": { "add": ["file-sharing"] },"#,
        );
        let server = Answers::parse(text.as_bytes()).unwrap();
        server.check().unwrap();
        assert_eq!(server.chosen_features(), ["file-sharing"]);
        assert!(server.web_console());
        let off = Answers { web_console: Some(false), ..server.clone() };
        assert!(!off.web_console());
        // They are saved, and read back the same.
        let saved = Answers::parse(&server.to_saved()).unwrap();
        assert_eq!((saved.profile, saved.features), (Profile::Server, server.features.clone()));
        // Each ID is checked, and named once.
        let problem = |features: &str| {
            let text = EXAMPLE.replace("\"version\": 1,", &format!("\"version\": 1, {features},"));
            Answers::parse(text.as_bytes()).unwrap().check().unwrap_err().join("; ")
        };
        assert!(problem(r#""features": { "add": ["File sharing"] }"#).contains("feature's ID"));
        let twice = r#""features": { "add": ["file-sharing", "file-sharing"] }"#;
        assert!(problem(twice).contains("added twice"));
        let both = r#""features": { "add": ["file-sharing"], "remove": ["file-sharing"] }"#;
        assert!(problem(both).contains("both added and removed"));
        let unknown = r#""profile": "desktop""#;
        let text = EXAMPLE.replace("\"version\": 1,", &format!("\"version\": 1, {unknown},"));
        assert!(Answers::parse(text.as_bytes()).unwrap_err().contains("unknown variant"));
    }

    #[test]
    fn the_serial_console_follows_the_installer_unless_the_answers_say() {
        let answers = Answers::parse(EXAMPLE.as_bytes()).unwrap();
        assert!(!answers.serial_console(false));
        assert!(answers.serial_console(true));
        let off = Answers { serial_console: Some(false), ..answers.clone() };
        assert!(!off.serial_console(true));
        let on = Answers { serial_console: Some(true), ..answers };
        assert!(on.serial_console(false));
        let saved = String::from_utf8(on.to_saved()).unwrap();
        assert!(saved.contains("\"serial_console\": true"), "{saved}");
        assert_eq!(Answers::parse(saved.as_bytes()).unwrap().serial_console, Some(true));
    }

    #[test]
    fn accessibility_is_as_the_answers_say_and_the_reader_as_the_installer_runs() {
        let answers = Answers::parse(EXAMPLE.as_bytes()).unwrap();
        assert!(answers.accessibility.is_default());
        assert!(!answers.accessibility.screen_reader(false));
        assert!(answers.accessibility.screen_reader(true));
        // Left out, it is not saved.
        assert!(!String::from_utf8(answers.to_saved()).unwrap().contains("accessibility"));
        let text = EXAMPLE.replace(
            "\"version\": 1,",
            r#""version": 1, "accessibility": { "screen_reader": false, "text_size": "larger", "sticky_keys": true },"#,
        );
        let given = Answers::parse(text.as_bytes()).unwrap();
        given.check().unwrap();
        let access = given.accessibility;
        assert!(!access.screen_reader(true));
        assert_eq!(
            (access.text_size, access.sticky_keys, access.slow_keys),
            (TextSize::Larger, true, false)
        );
        assert!(!access.consoles_as_default());
        let file = access.console_file(None);
        assert!(
            file.ends_with("text-size = \"larger\"\nsticky-keys = true\nslow-keys = false\n"),
            "{file}"
        );
        assert!(access.console_file(Some(500)).ends_with("slow-keys-delay = 500\n"));
        // Saved, and read back the same.
        assert_eq!(Answers::parse(&given.to_saved()).unwrap().accessibility, access);
        let typo = EXAMPLE.replace(
            "\"version\": 1,",
            r#""version": 1, "accessibility": { "text_size": "huge" },"#,
        );
        assert!(Answers::parse(typo.as_bytes()).unwrap_err().contains("unknown variant"));
    }

    #[test]
    fn refuses_another_version_and_unknown_keys() {
        let newer = EXAMPLE.replace("\"version\": 1", "\"version\": 2");
        assert!(Answers::parse(newer.as_bytes()).unwrap_err().contains("version 2"));
        let typo = EXAMPLE.replace("\"swap_gib\"", "\"swap\"");
        assert!(Answers::parse(typo.as_bytes()).unwrap_err().contains("unknown field"));
    }

    #[test]
    fn an_account_needs_a_hash_or_an_empty_password() {
        let text = EXAMPLE.replace(r#", "password_hash": "" }"#, " }");
        let problems = Answers::parse(text.as_bytes()).unwrap().check().unwrap_err();
        assert!(problems[0].starts_with("admin.password_hash: give"), "{problems:?}");
        let text = EXAMPLE.replacen(r#""password_hash": """#, r#""password_hash": "x""#, 1);
        let problems = Answers::parse(text.as_bytes()).unwrap().check().unwrap_err();
        assert!(problems[0].contains("not an Argon2id hash"), "{problems:?}");
    }

    #[test]
    fn the_saved_answers_keep_no_hash_unless_asked() {
        let text = EXAMPLE.replace(r#""password_hash": """#, r#""password_hash": "$argon2id$x""#);
        let answers = Answers::parse(text.as_bytes()).unwrap();
        let saved = String::from_utf8(answers.to_saved()).unwrap();
        assert!(!saved.contains("password_hash"), "{saved}");
        Answers::parse(saved.as_bytes()).unwrap();
        let kept = Answers { keep_password_hashes: true, ..answers };
        assert!(String::from_utf8(kept.to_saved()).unwrap().contains("$argon2id$x"));
    }

    /// A manual layout: a new GPT on vda with the root and its EFI
    /// system partition, and vdb's third partition formatted as exFAT.
    const MANUAL: &str = r#"{ "layout": "manual", "target": "vda", "changes": [
        { "disk": "vda", "new_table": true, "partitions": [
            { "number": 1, "first": 2048, "sectors": 1048576, "use": "efi" },
            { "number": 2, "first": 1050624, "sectors": 8388608, "use": "root", "name": "Dukovo" } ] },
        { "disk": "vdb", "partitions": [ { "number": 3, "use": "data", "format": "exfat" } ] } ] }"#;

    fn with_disk(disk: &str) -> Result<(), Vec<String>> {
        let text =
            EXAMPLE.replace(r#"{ "layout": "guided", "target": "/dev/vda", "swap_gib": 1 }"#, disk);
        Answers::parse(text.as_bytes()).unwrap().check()
    }

    #[test]
    fn checks_the_manual_layouts_changes() {
        with_disk(MANUAL).unwrap();
        let problem = |disk: String| with_disk(&disk).unwrap_err().join("; ");
        // The root goes on the target.
        let moved = MANUAL.replace(r#""target": "vda""#, r#""target": "vdb""#);
        assert!(problem(moved).contains("go on the target, vdb"));
        // One root, and a data partition there is is named to format it.
        let two = MANUAL.replace(r#""use": "efi""#, r#""use": "root""#);
        assert!(problem(two).contains("one root and one EFI system partition"));
        let unformatted = MANUAL.replace(r#", "format": "exfat""#, "");
        assert!(problem(unformatted).contains("named only to format it"));
        // A deleted partition cannot be formatted, nor a new one half placed.
        let gone = MANUAL.replace(r#"{ "disk": "vdb", "#, r#"{ "disk": "vdb", "delete": [3], "#);
        assert!(problem(gone).contains("is gone before it can be formatted"));
        let half = MANUAL.replace(r#", "sectors": 1048576"#, "");
        assert!(problem(half).contains("has both first and sectors"));
        // swap_gib is the other layouts'; changes are the manual one's.
        let swap = MANUAL.replace(r#""target": "vda","#, r#""target": "vda", "swap_gib": 2,"#);
        assert!(problem(swap).contains("lists its swap partition"));
        let guided = MANUAL.replace(r#""layout": "manual""#, r#""layout": "guided""#);
        assert!(problem(guided).contains("changes are the manual layout's only"));
        with_disk(r#"{ "layout": "free", "target": "vda", "swap_gib": 2 }"#).unwrap();
    }

    #[test]
    fn next_to_windows_names_the_space_dukovo_takes() {
        let beside =
            r#"{ "layout": "beside", "target": "nvme0n1", "swap_gib": 2, "space_gib": 40 }"#;
        with_disk(beside).unwrap();
        let problem = |disk: &str| with_disk(disk).unwrap_err().join("; ");
        let no_size = beside.replace(r#", "space_gib": 40"#, "");
        assert!(problem(&no_size).contains("space_gib gives the space"));
        let guided = beside.replace(r#""beside""#, r#""guided""#);
        assert!(problem(&guided).contains("next to Windows only"));
    }

    #[test]
    fn a_mirror_names_its_second_disk() {
        let mirror =
            r#"{ "layout": "mirror", "target": "vda", "mirror": "/dev/vdb", "swap_gib": 1 }"#;
        with_disk(mirror).unwrap();
        let text = EXAMPLE
            .replace(r#"{ "layout": "guided", "target": "/dev/vda", "swap_gib": 1 }"#, mirror);
        assert_eq!(Answers::parse(text.as_bytes()).unwrap().mirror(), Some("vdb"));
        let problem = |disk: &str| with_disk(disk).unwrap_err().join("; ");
        let alone = mirror.replace(r#", "mirror": "/dev/vdb""#, "");
        assert!(problem(&alone).contains("mirror names the second disk"));
        let twice = mirror.replace("/dev/vdb", "vda");
        assert!(problem(&twice).contains("not the target again"));
        let guided = mirror.replace(r#""layout": "mirror""#, r#""layout": "guided""#);
        assert!(problem(&guided).contains("mirror layout only"));
        assert!(problem(&mirror.replace("/dev/vdb", "v d b")).contains("not a disk's name"));
    }

    #[test]
    fn checks_fixed_addresses() {
        let fixed =
            |address: &str| Fixed { address: address.into(), gateway: None, dns: Vec::new() };
        assert!(check_fixed(&fixed("10.0.0.2/24")).is_ok());
        assert!(check_fixed(&fixed("fd00::2/64")).is_ok());
        assert!(check_fixed(&fixed("10.0.0.2")).is_err());
        assert!(check_fixed(&fixed("10.0.0.2/33")).is_err());
        assert!(check_fixed(&fixed("ten/8")).is_err());
    }
}
