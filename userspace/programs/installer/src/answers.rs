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

use serde::{Deserialize, Serialize};

/// The version of the answers file this installer reads.
pub const VERSION: u32 = 1;

/// The machine's name when the answers give none, as hostname(5) has it.
pub const DEFAULT_HOSTNAME: &str = "dukovo";

/// The time zone when the answers give none.
pub const DEFAULT_TIME_ZONE: &str = "UTC";

/// The most swap the installer makes, in GiB.
pub const SWAP_MAX_GIB: u32 = 64;

/// The keys of what the installer's server installs will add: a profile,
/// its features, the web console, SSH, updates and closed firmware.
const LATER: [&str; 6] = ["profile", "features", "web_console", "ssh", "updates", "firmware"];

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
    /// Every other key: those of [`LATER`], which this installer takes
    /// none of yet, and mistyped ones, which [`Answers::check`] names.
    /// (serde cannot refuse unknown keys beside a flattened map.)
    #[serde(flatten)]
    pub later: BTreeMap<String, serde_json::Value>,
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
    /// The swap partition's size in GiB, 0 for none.
    #[serde(default)]
    pub swap_gib: u32,
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
    /// Two disks are mirrored with Btrfs RAID1.
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

    /// The target disk's name, without `/dev/` or `/devices/`.
    #[must_use]
    pub fn target(&self) -> &str {
        let target = self.disk.target.as_str();
        target.strip_prefix("/dev/").or_else(|| target.strip_prefix("/devices/")).unwrap_or(target)
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
        check_accounts(self, &mut problem);
        if problems.is_empty() { Ok(()) } else { Err(problems) }
    }

    /// Checks the disk's answers.
    fn check_disk(&self) -> Result<(), String> {
        match self.disk.layout {
            Layout::Guided => {}
            Layout::Free | Layout::Manual => {
                return Err("this layout comes with the installer's screens; use \"guided\"".into());
            }
            Layout::Mirror | Layout::Beside => {
                return Err("this layout is not ready yet; use \"guided\"".into());
            }
        }
        if !is_name(self.target(), 32) {
            return Err(format!(
                "target {:?} is not a disk's name, such as vda: lsblk lists them",
                self.disk.target
            ));
        }
        if self.disk.mirror.is_some() {
            return Err("mirror names the second disk of the mirror layout only".into());
        }
        if self.disk.swap_gib > SWAP_MAX_GIB {
            return Err(format!("swap_gib is at most {SWAP_MAX_GIB}"));
        }
        Ok(())
    }
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
            .replace("\"version\": 1,", "\"version\": 1, \"profile\": \"server\",");
        let problems = Answers::parse(text.as_bytes()).unwrap().check().unwrap_err();
        let keys: Vec<&str> = problems.iter().map(|p| p.split(':').next().unwrap()).collect();
        assert_eq!(
            keys,
            ["profile", "hostname", "time_zone", "disk", "users[0].user"],
            "{problems:?}"
        );
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
