// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The machine's record (ADR 0464): what a round writes beside the
//! places' backups so that a whole machine can be restored from them.
//! It names each place by its folder at the destination and its path on
//! the machine, and carries the files of `/system/state` that say which
//! apps are installed and what each person allowed them: bundled(8)'s
//! registry and the grants. The apps themselves are in the places, in
//! `/applications` and each person's `applications` folder.
//!
//! Format 2 carries authd's files of the accounts too (ADR 0564): the
//! accounts are in the settings place, and these are what signs each in
//! and what each kept, so that a restored machine's people sign in with
//! the passwords and second steps they had and find their keyrings.
//!
//! It is JSON, encrypted and tagged as a backup is, one a round, in the
//! folder `machine`.
//!
//! Format 2 adds what a person moving to new hardware needs to see before
//! anything is received: the machine's hardware, how many bytes each
//! place's backups hold, and the texts of the network and firewall settings, so
//! that a plan reads them without receiving the settings place. A record
//! of format 1, which earlier rounds wrote, is still read; it has none of
//! these.

use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::io;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::bundles::{grants, registry};
use crate::dhcp_server::mac_of;
use crate::util::{self, Fail};

/// The format a round writes: the machine's hardware, the bytes of each
/// place and the network and firewall settings come with it.
pub const FORMAT: u32 = 2;

/// The format rounds wrote before the hardware was kept, which is still
/// read.
const FIRST_FORMAT: u32 = 1;

/// The folder of `/system/state` bundled keeps its registry in.
pub(super) const BUNDLED: &str = "bundled";

/// The folder of `/system/state` bundled keeps the grants in.
const GRANTS: &str = "grants";

/// The folder of `/system/state` authd keeps its files in (authd(8)).
const AUTHD: &str = "authd";

/// authd's files of all the accounts that a record carries, each always,
/// empty if authd has not written it, so that a restore leaves none from
/// before: the password hashes, the sharing passwords' hashes, the
/// numbers of removed accounts, which no account gets again, and whether
/// each person's jobs run while they are signed out. Its other files, the
/// notices for a next sign-in and the addresses it blocks, are the
/// machine's own.
const AUTHD_FILES: [&str; 4] = ["passwords", "sharing", "retired", "jobs"];

/// authd's folder of each person's keyring, by user number, sealed with
/// a key from their password.
const KEYRINGS: &str = "keyrings";

/// authd's folder of each person's second steps and API tokens for the
/// web console, `NAME.toml`.
const CONSOLE: &str = "console";

/// The most a record holds of `/system/state`, in bytes: a person's
/// grants are at most 4096 lines.
const MOST: usize = 64 << 20;

/// The most a record carries of each of the two settings files, in
/// bytes.
pub const SETTINGS_MOST: usize = 64 << 10;

/// The most cards and disks a record lists, and the most bytes of a name
/// or model in it. A machine with more has the first of them listed.
pub(super) const CARDS_MOST: usize = 64;
pub(super) const DISKS_MOST: usize = 256;
pub(super) const TEXT_MOST: usize = 128;

/// The most accounts a record names: as many as people's account numbers
/// allow is more than any machine has, and a record is read whole.
pub(super) const ACCOUNTS_MOST: usize = 4096;

/// A network card of the machine, as `/kernel/class/net` shows it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Card {
    /// Its name: `net0`.
    pub name: String,
    /// Its MAC address, as the kernel writes it: `52:54:00:12:34:56`.
    pub mac: String,
    /// The model the driver gives, when it gives one.
    pub model: Option<String>,
}

/// A disk of the machine, as `/kernel/block` shows it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Disk {
    /// The model the driver gives, when it gives one.
    pub model: Option<String>,
    /// Its size in bytes.
    pub size: u64,
}

/// The serial port the machine's console is on.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Serial {
    /// The port's terminal: `ttyS1`.
    pub device: String,
    /// What the kernel calls the port, when it says: `com2`, `amt-sol`.
    pub model: Option<String>,
}

/// What the machine had when the round was made, for the person who
/// restores it on other hardware to compare with what they have.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Hardware {
    pub cards: Vec<Card>,
    /// The disks that stay in the machine: removable ones are left out.
    pub disks: Vec<Disk>,
    /// Whether the disks of its root Btrfs mirror each other.
    pub mirror: bool,
    /// The serial port its console is on, if it has one.
    pub serial: Option<Serial>,
    /// The processor, as `admin about` names it.
    pub cpu: Option<String>,
}

/// The settings a restore of the machine may need before it has received
/// the settings place. Each is the file's text, or `None` for a file
/// larger than [`SETTINGS_MOST`], which only the settings place holds.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Settings {
    /// network(5)'s file; empty when the machine has none.
    pub network: Option<String>,
    /// firewall(8)'s file; empty when the machine has none.
    pub firewall: Option<String>,
}

/// What a round says of the machine beside the files it carries: the
/// members of a record that format 2 adds.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Facts {
    pub hardware: Hardware,
    /// The bytes each place's newest chain holds, by the place's folder.
    pub bytes: BTreeMap<String, u64>,
    pub settings: Settings,
    /// The names of people's accounts, in the order of the accounts file.
    pub accounts: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Record {
    pub format: u32,
    /// The machine's name, as its folder at the destination has it.
    pub machine: String,
    /// The round's time.
    pub time: String,
    /// Each place backed up, by its folder's name at the destination: its
    /// path on the machine.
    pub places: BTreeMap<String, String>,
    /// Files of `/system/state`, by their path in it, as text.
    pub state: BTreeMap<String, String>,
    /// Format 2: the bytes a restore of each place receives, by its
    /// folder. A place whose backup failed in the round has none. For
    /// Btrfs streams they are the bytes of its chain up to this round's
    /// stream, whole and changes; for restic, the bytes the round read.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub bytes: BTreeMap<String, u64>,
    /// Format 2: the machine's hardware.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hardware: Option<Hardware>,
    /// Format 2: the settings the plan of a restore reads.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub settings: Option<Settings>,
    /// Format 2: the names of people's accounts, which the settings place
    /// holds, so that a plan names them without receiving it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub accounts: Vec<String>,
}

/// Why a record's text is not read.
#[derive(Debug, PartialEq, Eq)]
pub enum Unread {
    /// A later Dukovo wrote it, in a format this one does not know
    /// (BAK-107).
    Newer(u64),
    /// Anything else, in words.
    Wrong(String),
}

impl fmt::Display for Unread {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Unread::Newer(format) => {
                write!(out, "a record of format {format}, which this Dukovo does not read")
            }
            Unread::Wrong(why) => out.write_str(why),
        }
    }
}

/// Whether a record may carry the file `path` of `/system/state`: one of
/// bundled's registry's files, a person's grants, by user number, or one
/// of authd's files of the accounts: [`AUTHD_FILES`], a person's keyring
/// by user number, or their console file by account name.
pub fn may_carry(path: &str) -> bool {
    match path.split_once('/') {
        Some((BUNDLED, name)) => registry::FILES.contains(&name),
        Some((GRANTS, uid)) => is_user_number(uid),
        Some((AUTHD, name)) => match name.split_once('/') {
            None => AUTHD_FILES.contains(&name),
            Some((KEYRINGS, uid)) => is_user_number(uid),
            Some((CONSOLE, file)) => file.strip_suffix(".toml").is_some_and(is_account_name),
            Some(_) => false,
        },
        _ => false,
    }
}

/// Whether `name` is a user number as a file's name: decimal, without a
/// leading zero.
fn is_user_number(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 10
        && name.bytes().all(|b| b.is_ascii_digit())
        && (name == "0" || !name.starts_with('0'))
}

/// Whether the file `path` the record carries holds a secret, or what
/// stands for one (a hash, a sealed keyring), and so is root's alone,
/// mode 0600, as authd and bundled write them; bundled's registry is
/// anyone's to read, 0644.
pub fn mode_of(path: &str) -> u16 {
    if path.starts_with("bundled/") { 0o644 } else { 0o600 }
}

impl Record {
    /// The record of the machine `machine` at `time`, whose places are
    /// `places`, with what it carries read from `state`, the machine's
    /// `/system/state`, and what the round says of the machine, `facts`.
    /// Every registry file is carried, empty if bundled has not written
    /// it, so that a restore leaves none from before.
    pub fn gather(
        machine: &str,
        time: &str,
        places: BTreeMap<String, String>,
        state: &Path,
        facts: Facts,
    ) -> Result<Record, Fail> {
        debug_assert_eq!(grants::FOLDER, "/system/state/grants");
        debug_assert_eq!(registry::STATE, "/system/state/bundled");
        let read = |path: &Path| match fs::read_to_string(path) {
            Ok(text) => Ok(text),
            Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(String::new()),
            Err(err) => Err(util::failed(&path.display(), err)),
        };
        let mut carried = BTreeMap::new();
        let files = registry::FILES.iter().map(|name| format!("{BUNDLED}/{name}"));
        for path in files.chain(AUTHD_FILES.iter().map(|name| format!("{AUTHD}/{name}"))) {
            let text = read(&state.join(&path))?;
            carried.insert(path, text);
        }
        for folder in
            [GRANTS.to_string(), format!("{AUTHD}/{KEYRINGS}"), format!("{AUTHD}/{CONSOLE}")]
        {
            let at = state.join(&folder);
            let entries = match fs::read_dir(&at) {
                Ok(entries) => entries,
                Err(err) if err.kind() == io::ErrorKind::NotFound => continue,
                Err(err) => return Err(util::failed(&at.display(), err)),
            };
            for entry in entries {
                let entry = entry.map_err(|e| util::failed(&at.display(), e))?;
                let Some(name) = entry.file_name().to_str().map(String::from) else { continue };
                // What authd or bundled leaves while it writes a file,
                // `NAME.new`, is no file of theirs yet.
                let path = format!("{folder}/{name}");
                if may_carry(&path) && entry.file_type().is_ok_and(|t| t.is_file()) {
                    carried.insert(path, read(&entry.path())?);
                }
            }
        }
        Ok(Record {
            format: FORMAT,
            machine: machine.into(),
            time: time.into(),
            places,
            state: carried,
            bytes: facts.bytes,
            accounts: facts.accounts,
            hardware: Some(facts.hardware),
            settings: Some(facts.settings),
        })
    }

    /// The record as its file holds it, before encryption.
    pub fn text(&self) -> String {
        let mut text = serde_json::to_string_pretty(self).unwrap_or_default();
        text.push('\n');
        text
    }

    /// Reads a record's text strictly: its format one this Dukovo reads,
    /// the members of that format and no others, and nothing carried that
    /// a record may not carry.
    pub fn parse(text: &str) -> Result<Record, Unread> {
        let wrong = |why: String| Unread::Wrong(why);
        if text.len() > MOST {
            return Err(wrong("larger than a record can be".into()));
        }
        // The format is read first, so that a later one's new members are
        // not taken for mistakes.
        #[derive(Deserialize)]
        struct Header {
            format: u64,
        }
        let header: Header = serde_json::from_str(text).map_err(|e| wrong(e.to_string()))?;
        if header.format > u64::from(FORMAT) {
            return Err(Unread::Newer(header.format));
        }
        let record: Record = serde_json::from_str(text).map_err(|e| wrong(e.to_string()))?;
        record.check().map_err(wrong)?;
        Ok(record)
    }

    /// Whether the record has the members its format has: format 1 none
    /// of format 2's, format 2 its hardware, its settings and authd's
    /// files of the accounts.
    fn check_members(&self) -> Result<(), String> {
        let authd = |path: &String| path.starts_with("authd/");
        let has_second = self.hardware.is_some()
            || self.settings.is_some()
            || !self.bytes.is_empty()
            || !self.accounts.is_empty()
            || self.state.keys().any(authd);
        let accounts_files =
            AUTHD_FILES.iter().all(|name| self.state.contains_key(&format!("{AUTHD}/{name}")));
        match self.format {
            FIRST_FORMAT if has_second => {
                Err("says format 1 but has members only format 2 has".into())
            }
            FORMAT if self.hardware.is_none() || self.settings.is_none() => {
                Err("says format 2 but lacks its hardware or settings".into())
            }
            FORMAT if !accounts_files => Err("says format 2 but lacks authd's files".into()),
            _ => Ok(()),
        }
    }

    /// The folders of `/system/state` a restore empties before it writes
    /// the files the record carries into them, as each person's files
    /// there are the record's alone: the grants, and with format 2 authd's
    /// keyrings and console files. A record of format 1 carries none of
    /// authd's, and its restore leaves them as they are.
    pub fn cleared(&self) -> Vec<String> {
        let mut folders = vec![GRANTS.to_string()];
        if self.format >= FORMAT {
            folders.extend([format!("{AUTHD}/{KEYRINGS}"), format!("{AUTHD}/{CONSOLE}")]);
        }
        folders
    }

    /// Whether the record is one this Dukovo reads, as its format says it
    /// is made.
    fn check(&self) -> Result<(), String> {
        if !(FIRST_FORMAT..=FORMAT).contains(&self.format) {
            return Err(format!(
                "a record of format {}, which this Dukovo does not read",
                self.format
            ));
        }
        if let Some(path) = self.state.keys().find(|p| !may_carry(p)) {
            return Err(format!("carries {path}, which a record never does"));
        }
        if let Some((folder, path)) = self
            .places
            .iter()
            .find(|(folder, path)| !super::layout::is_place(folder) || !path.starts_with('/'))
        {
            return Err(format!("names the place {folder} as {path:?}"));
        }
        self.check_members()?;
        if let Some(folder) = self.bytes.keys().find(|folder| !self.places.contains_key(*folder)) {
            return Err(format!("gives the bytes of {folder}, which it has no place for"));
        }
        if let Some(hardware) = &self.hardware {
            hardware.check()?;
        }
        if self.accounts.len() > ACCOUNTS_MOST
            || !self.accounts.iter().all(|name| is_account_name(name))
        {
            return Err("names its accounts in a way a round does not".into());
        }
        if let Some(settings) = &self.settings {
            for (file, text) in [("network", &settings.network), ("firewall", &settings.firewall)] {
                if text.as_ref().is_some_and(|text| text.len() > SETTINGS_MOST) {
                    return Err(format!("carries the {file} settings larger than it may"));
                }
            }
        }
        Ok(())
    }
}

/// Whether `name` has the shape of an account's name, as authd gives
/// one: lowercase letters, digits, `_`, `-` and `.`, from a letter or `_`.
fn is_account_name(name: &str) -> bool {
    let mut chars = name.chars();
    name.len() <= accounts::NAME_MAX
        && chars.next().is_some_and(|c| c.is_ascii_lowercase() || c == '_')
        && chars.all(|c| matches!(c, 'a'..='z' | '0'..='9' | '_' | '-' | '.'))
}

impl Hardware {
    /// Whether the hardware is as a round writes it: a few cards with
    /// their MAC addresses, disks and short plain names, the lists no
    /// longer than a round lists.
    fn check(&self) -> Result<(), String> {
        let plain = |text: &str| text.len() <= TEXT_MOST && !text.chars().any(char::is_control);
        let optional = |text: &Option<String>| text.as_deref().is_none_or(plain);
        if self.cards.len() > CARDS_MOST || self.disks.len() > DISKS_MOST {
            return Err("lists more cards or disks than a round does".into());
        }
        for card in &self.cards {
            if !plain(&card.name) || mac_of(&card.mac).is_none() || !optional(&card.model) {
                return Err(format!("names the card {:?} in a way a round does not", card.name));
            }
        }
        let serial_plain = self
            .serial
            .as_ref()
            .is_none_or(|serial| plain(&serial.device) && optional(&serial.model));
        if !serial_plain
            || !self.disks.iter().all(|disk| optional(&disk.model))
            || !optional(&self.cpu)
        {
            return Err("names its hardware in a way a round does not".into());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn the_places() -> BTreeMap<String, String> {
        BTreeMap::from([("users".to_string(), "/users".to_string())])
    }

    fn the_hardware() -> Hardware {
        Hardware {
            cards: vec![
                Card {
                    name: "net0".into(),
                    mac: "52:54:00:12:34:56".into(),
                    model: Some("Intel 82540EM".into()),
                },
                Card { name: "net1".into(), mac: "52:54:00:12:34:57".into(), model: None },
            ],
            disks: vec![Disk { model: Some("WD20EFZX".into()), size: 2_000_398_934_016 }],
            mirror: true,
            serial: Some(Serial { device: "ttyS1".into(), model: Some("com2".into()) }),
            cpu: Some("Intel Core i5-6500".into()),
        }
    }

    fn the_facts() -> Facts {
        Facts {
            hardware: the_hardware(),
            bytes: BTreeMap::from([("users".to_string(), 412_000_000_000)]),
            settings: Settings {
                network: Some("[net0]\naddress 192.168.1.12/24\n".into()),
                firewall: Some(String::new()),
            },
            accounts: vec!["owner".into(), "helper".into()],
        }
    }

    /// authd's files of the accounts, as a round on a machine where authd
    /// wrote none carries them.
    fn authds_files() -> BTreeMap<String, String> {
        AUTHD_FILES.iter().map(|name| (format!("authd/{name}"), String::new())).collect()
    }

    /// A record of the second format, as a round writes it.
    fn the_record() -> Record {
        Record {
            format: FORMAT,
            machine: "m".into(),
            time: "2026-11-11-030000".into(),
            places: the_places(),
            state: authds_files(),
            bytes: the_facts().bytes,
            hardware: Some(the_hardware()),
            settings: Some(the_facts().settings),
            accounts: the_facts().accounts,
        }
    }

    /// The text of a record of the first format, as the rounds that wrote
    /// it did.
    const FIRST: &str = r#"{
  "format": 1,
  "machine": "duke-nas",
  "time": "2026-11-10-020412",
  "places": {"users": "/users"},
  "state": {"bundled/installed": "everyone\torg.example.notes\t1.0\n", "grants/1000": "g\n"}
}
"#;

    #[test]
    fn a_record_carries_bundleds_registry_the_grants_and_authds_files_of_the_accounts_alone() {
        for path in [
            "bundled/installed",
            "bundled/defaults",
            "grants/1000",
            "grants/0",
            "authd/passwords",
            "authd/sharing",
            "authd/retired",
            "authd/jobs",
            "authd/keyrings/1000",
            "authd/keyrings/0",
            "authd/console/owner.toml",
            "authd/console/ann.lee-2.toml",
        ] {
            assert!(may_carry(path), "{path}");
        }
        for path in [
            "bundled/catalogs",
            "bundled/../grants/1000",
            "grants/01000",
            "grants/1000.bad",
            "grants/",
            "grants/12345678901",
            "backup/stick",
            "slots",
            "/system/state/grants/1000",
            // authd's own: what it tells at a next sign-in, and the
            // addresses it blocks, are this machine's.
            "authd/notices",
            "authd/blocked",
            "authd/passwords.new",
            "authd/passwords/x",
            "authd/keyrings/1000.new",
            "authd/keyrings/01000",
            "authd/keyrings/owner",
            "authd/console/owner.toml.new",
            "authd/console/owner",
            "authd/console/../passwords",
            "authd/console/Owner.toml",
            "authd/console/.toml",
            "authd/console/a/b.toml",
            "authd/other/1000",
            "authd/",
            "authd",
        ] {
            assert!(!may_carry(path), "{path}");
        }
        // Each is root's alone but bundled's registry.
        assert_eq!(mode_of("bundled/installed"), 0o644);
        for path in ["grants/1000", "authd/passwords", "authd/keyrings/1000"] {
            assert_eq!(mode_of(path), 0o600, "{path}");
        }
    }

    #[test]
    fn a_record_is_gathered_and_read_back() {
        let dir = std::env::temp_dir().join(format!("record-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("bundled/catalogs")).unwrap();
        fs::create_dir_all(dir.join("grants")).unwrap();
        fs::write(dir.join("bundled/installed"), "everyone\torg.example.notes\t1.0\n").unwrap();
        fs::write(dir.join("grants/1000"), "grant lines\n").unwrap();
        fs::write(dir.join("grants/1000.bad"), "set aside\n").unwrap();
        fs::write(dir.join("logs"), "not carried\n").unwrap();
        fs::create_dir_all(dir.join("authd/keyrings")).unwrap();
        fs::create_dir_all(dir.join("authd/console")).unwrap();
        fs::write(dir.join("authd/passwords"), "owner:$argon2id$v=19$...:\n").unwrap();
        fs::write(dir.join("authd/retired"), "1001:gone:2026-11-01\n").unwrap();
        fs::write(dir.join("authd/notices"), "owner\tnot carried\n").unwrap();
        fs::write(dir.join("authd/blocked"), "not carried\n").unwrap();
        fs::write(dir.join("authd/keyrings/1000"), "sealed\n").unwrap();
        fs::write(dir.join("authd/keyrings/1000.new"), "being written\n").unwrap();
        fs::write(dir.join("authd/console/owner.toml"), "[totp]\n").unwrap();
        fs::write(dir.join("authd/console/owner.toml.new"), "being written\n").unwrap();
        let record =
            Record::gather("m", "2026-11-11-030000", the_places(), &dir, the_facts()).unwrap();
        let _ = fs::remove_dir_all(&dir);
        assert_eq!(record.format, 2);
        assert_eq!(record.state.len(), registry::FILES.len() + 1 + AUTHD_FILES.len() + 2);
        assert_eq!(record.state["bundled/installed"], "everyone\torg.example.notes\t1.0\n");
        assert_eq!(record.state["bundled/held"], "");
        assert_eq!(record.state["grants/1000"], "grant lines\n");
        assert_eq!(record.state["authd/passwords"], "owner:$argon2id$v=19$...:\n");
        assert_eq!(record.state["authd/retired"], "1001:gone:2026-11-01\n");
        // One authd has not written is carried empty, so that a restore
        // leaves none from before.
        assert_eq!(record.state["authd/sharing"], "");
        assert_eq!(record.state["authd/keyrings/1000"], "sealed\n");
        assert_eq!(record.state["authd/console/owner.toml"], "[totp]\n");
        assert_eq!(record.cleared(), ["grants", "authd/keyrings", "authd/console"]);
        assert_eq!(record.hardware, Some(the_hardware()));
        assert_eq!(record.bytes["users"], 412_000_000_000);
        assert_eq!(record.settings, Some(the_facts().settings));
        assert_eq!(Record::parse(&record.text()).unwrap(), record);
    }

    #[test]
    fn a_record_of_the_second_format_has_the_members_the_plan_reads() {
        let record = the_record();
        let value: serde_json::Value = serde_json::from_str(&record.text()).unwrap();
        assert_eq!(
            value,
            serde_json::json!({
                "format": 2,
                "machine": "m",
                "time": "2026-11-11-030000",
                "places": {"users": "/users"},
                "state": {
                    "authd/jobs": "",
                    "authd/passwords": "",
                    "authd/retired": "",
                    "authd/sharing": "",
                },
                "bytes": {"users": 412_000_000_000u64},
                "hardware": {
                    "cards": [
                        {"name": "net0", "mac": "52:54:00:12:34:56", "model": "Intel 82540EM"},
                        {"name": "net1", "mac": "52:54:00:12:34:57", "model": null},
                    ],
                    "disks": [{"model": "WD20EFZX", "size": 2_000_398_934_016u64}],
                    "mirror": true,
                    "serial": {"device": "ttyS1", "model": "com2"},
                    "cpu": "Intel Core i5-6500",
                },
                "settings": {
                    "network": "[net0]\naddress 192.168.1.12/24\n",
                    "firewall": "",
                },
                "accounts": ["owner", "helper"],
            })
        );
        assert_eq!(Record::parse(&record.text()).unwrap(), record);
        // A machine that says nothing of itself is still a record of the
        // second format: its members are empty, not missing.
        let bare = Record { hardware: Some(Hardware::default()), bytes: BTreeMap::new(), ..record };
        assert_eq!(Record::parse(&bare.text()).unwrap(), bare);
    }

    #[test]
    fn a_record_of_the_first_format_is_still_read_without_the_members_of_the_second() {
        let record = Record::parse(FIRST).unwrap();
        assert_eq!((record.format, record.machine.as_str()), (1, "duke-nas"));
        assert_eq!(record.places["users"], "/users");
        assert_eq!(record.state["grants/1000"], "g\n");
        assert!(record.hardware.is_none() && record.settings.is_none());
        assert!(record.bytes.is_empty());
        // Written again it is the same record, in its own format.
        let again = Record::parse(&record.text()).unwrap();
        assert_eq!(again, record);
        assert!(!record.text().contains("hardware"));
        // Its restore empties the grants alone: it has no files of
        // authd's to put in their place.
        assert_eq!(record.cleared(), ["grants"]);
    }

    #[test]
    fn the_members_of_each_format_are_the_ones_it_has() {
        let mut first = Record::parse(FIRST).unwrap();
        for with in [
            Record { hardware: Some(Hardware::default()), ..first.clone() },
            Record { settings: Some(Settings::default()), ..first.clone() },
            Record { bytes: the_facts().bytes, ..first.clone() },
            Record { accounts: the_facts().accounts, ..first.clone() },
            Record { state: authds_files(), ..first.clone() },
        ] {
            let why = Record::parse(&with.text()).unwrap_err().to_string();
            assert!(why.contains("format 1"), "{why}");
        }
        first.format = 2;
        let why = Record::parse(&first.text()).unwrap_err().to_string();
        assert!(why.contains("lacks its hardware or settings"), "{why}");
        for without in
            [Record { hardware: None, ..the_record() }, Record { settings: None, ..the_record() }]
        {
            assert!(Record::parse(&without.text()).is_err());
        }
        // Each of authd's files of all the accounts, which a round always
        // carries.
        for name in AUTHD_FILES {
            let mut record = the_record();
            record.state.remove(&format!("authd/{name}"));
            let why = Record::parse(&record.text()).unwrap_err().to_string();
            assert!(why.contains("lacks authd's files"), "{name}: {why}");
        }
        first.format = 0;
        assert!(Record::parse(&first.text()).unwrap_err().to_string().contains("format 0"));
    }

    #[test]
    fn a_later_format_is_told_apart_whatever_else_it_carries() {
        let text = r#"{"format": 3, "machine": "m", "something": ["new"]}"#;
        assert_eq!(Record::parse(text).unwrap_err(), Unread::Newer(3));
        assert_eq!(
            Unread::Newer(3).to_string(),
            "a record of format 3, which this Dukovo does not read"
        );
        assert_eq!(
            Record::parse(r#"{"format": 9999999999999}"#).unwrap_err(),
            Unread::Newer(9_999_999_999_999)
        );
        // A record of this format with a member of its own that it does not
        // have is a mistake, not a later record.
        let extra = the_record().text().replacen("\"machine\"", "\"extra\": 1, \"machine\"", 1);
        assert!(matches!(Record::parse(&extra), Err(Unread::Wrong(_))));
        assert!(matches!(Record::parse("not json"), Err(Unread::Wrong(_))));
        assert!(matches!(Record::parse("{}"), Err(Unread::Wrong(_))));
    }

    #[test]
    fn a_record_that_carries_more_is_refused() {
        let mut record = the_record();
        assert!(Record::parse(&record.text()).is_ok());
        record.state.insert("../settings/passwd".into(), String::new());
        assert!(Record::parse(&record.text()).unwrap_err().to_string().contains("passwd"));
        record.state.remove("../settings/passwd");
        record.state.insert("authd/notices".into(), String::new());
        assert!(Record::parse(&record.text()).unwrap_err().to_string().contains("notices"));
        record.state.remove("authd/notices");
        record.places.insert("../x".into(), "/x".into());
        assert!(Record::parse(&record.text()).is_err());
        record.places.remove("../x");
        record.format = FORMAT + 1;
        assert_eq!(Record::parse(&record.text()).unwrap_err(), Unread::Newer(3));
        assert!(Record::parse("{\"format\": 1, \"extra\": 1}").is_err());
    }

    #[test]
    fn the_bytes_are_of_the_places_the_record_has() {
        let mut record = the_record();
        record.bytes.insert("shared".into(), 1);
        let why = Record::parse(&record.text()).unwrap_err().to_string();
        assert!(why.contains("shared"), "{why}");
    }

    #[test]
    fn the_settings_are_carried_to_64_kib_each() {
        let mut record = the_record();
        let at = |size: usize| Some("n".repeat(size));
        record.settings = Some(Settings { network: at(SETTINGS_MOST), firewall: None });
        assert_eq!(Record::parse(&record.text()).unwrap(), record);
        record.settings = Some(Settings { network: at(SETTINGS_MOST + 1), firewall: None });
        assert!(Record::parse(&record.text()).unwrap_err().to_string().contains("network"));
        record.settings = Some(Settings { network: None, firewall: at(SETTINGS_MOST + 1) });
        assert!(Record::parse(&record.text()).unwrap_err().to_string().contains("firewall"));
    }

    #[test]
    fn the_hardware_is_plain_and_not_longer_than_a_round_lists() {
        let refused = |change: &dyn Fn(&mut Hardware)| {
            let mut record = the_record();
            change(record.hardware.as_mut().unwrap());
            assert!(Record::parse(&record.text()).is_err());
        };
        refused(&|h| h.cards[0].mac = "52:54:00:12:34".into());
        refused(&|h| h.cards[0].mac = "not a mac".into());
        refused(&|h| h.cards[0].name = "net\n0".into());
        refused(&|h| h.cards[0].model = Some("x".repeat(TEXT_MOST + 1)));
        refused(&|h| h.cpu = Some("a\u{1b}[31m".into()));
        refused(&|h| h.serial = Some(Serial { device: "tty\0".into(), model: None }));
        refused(&|h| h.disks[0].model = Some("\t".into()));
        refused(&|h| h.cards = vec![h.cards[0].clone(); CARDS_MOST + 1]);
        refused(&|h| h.disks = vec![h.disks[0].clone(); DISKS_MOST + 1]);
        let mut record = the_record();
        record.hardware.as_mut().unwrap().cards =
            vec![record.hardware.clone().unwrap().cards[0].clone(); CARDS_MOST];
        assert!(Record::parse(&record.text()).is_ok());
    }

    #[test]
    fn the_accounts_are_names_as_authd_gives_them() {
        let with = |accounts: Vec<String>| Record { accounts, ..the_record() };
        for good in [vec![], vec!["owner".into(), "_build".into(), "ann.lee-2".into()]] {
            let record = with(good);
            assert_eq!(Record::parse(&record.text()).unwrap(), record);
        }
        let most = "a".repeat(accounts::NAME_MAX);
        assert!(Record::parse(&with(vec![most.clone()]).text()).is_ok());
        for bad in [String::new(), format!("{most}a"), "Owner".into(), "2nd".into(), "a b".into()] {
            let why = Record::parse(&with(vec![bad.clone()]).text()).unwrap_err().to_string();
            assert!(why.contains("accounts"), "{bad:?}: {why}");
        }
        assert!(Record::parse(&with(vec!["a".into(); ACCOUNTS_MOST + 1]).text()).is_err());
    }
}
