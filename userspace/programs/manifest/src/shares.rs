// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The shares file (shares.toml(5), ADR 0228): the folders smbd(8)
//! offers Windows PCs and other computers on the network, which share(1)
//! changes and smbd reads again whenever it changes.
//!
//! ```toml
//! shares_version = 1
//!
//! [[share]]
//! name = "Music"
//! folder = "/users/chase/Music"
//! comment = "Chase's music"
//! read-only = true
//! ```
//!
//! Each share has a name, which a PC asks for as `\\computer\name`, in
//! any case, and a folder. Whoever signs in reaches the folder as
//! themselves: the owners, groups and modes of its files decide what they
//! may read and change, as at Dukovo's own shell. `read-only` keeps
//! everyone from changing anything there, whatever the files allow.
//!
//! `nfs` lists the networks whose machines may mount it over NFS, as
//! `computer:/name`, from nfsd(8) (ADR 0343): addresses, or networks as
//! an address and the length of its prefix. NFS's machines say who calls
//! and are taken at their word, so a share has none unless it is given
//! them.

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::net::IpAddr;
use core::str::FromStr;

use serde::Deserialize;

use crate::Error;

/// Where the file is.
pub const PATH: &str = "/system/settings/shares.toml";

/// The version of the file this reads and writes.
pub const VERSION: u32 = 1;

/// The longest name of a share, in characters, as Windows takes them.
pub const NAME_MAX: usize = 80;

/// The longest comment, in characters.
pub const COMMENT_MAX: usize = 256;

/// The name a PC asks for to list the shares: the pipes of remote
/// procedure calls, which is no folder.
pub const IPC: &str = "IPC$";

/// One share.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Share {
    /// Its name.
    pub name: String,
    /// The folder it shares, an absolute path.
    pub folder: String,
    /// What a PC's list of the shares says of it.
    #[serde(default)]
    pub comment: String,
    /// Whether nothing in it can be changed through it.
    #[serde(default, rename = "read-only")]
    pub read_only: bool,
    /// The networks NFS's clients may mount it from: none keeps it from
    /// NFS (ADR 0343).
    #[serde(default)]
    pub nfs: Vec<String>,
}

/// The number NFS's filehandles carry for the share named `name`, and
/// the name its folder has in an NFS worker's view, `.nfs-` and it in
/// hexadecimal (ADR 0343): FNV-1a of the name in lower case, so it is the
/// same after a restart.
pub fn nfs_key(name: &str) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in name.to_lowercase().bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// The most networks a share lists for NFS.
pub const NETWORKS_MAX: usize = 64;

/// A network: an address and the length of the prefix its machines
/// share, the whole address for one machine.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Network {
    /// Its address.
    pub address: IpAddr,
    /// The length of its prefix, in bits.
    pub prefix: u8,
}

impl Network {
    /// The network `text` names: `192.168.1.0/24`, `fd00::/8`, or one
    /// machine's address.
    pub fn parse(text: &str) -> Result<Network, String> {
        let bad = || format!("{text} is not an address, or an address and /prefix");
        let (address, prefix) = match text.split_once('/') {
            Some((address, prefix)) => (address, Some(prefix)),
            None => (text, None),
        };
        let address = IpAddr::from_str(address).map_err(|_| bad())?;
        let most = if address.is_ipv4() { 32 } else { 128 };
        let prefix = match prefix {
            Some(p) => p.parse::<u8>().ok().filter(|&p| p <= most).ok_or_else(bad)?,
            None => most,
        };
        Ok(Network { address, prefix })
    }

    /// Whether `address` is on it. An IPv4 address that a dual-stack
    /// socket shows as IPv6 (`::ffff:a.b.c.d`) is taken as itself.
    pub fn contains(&self, address: IpAddr) -> bool {
        let address = match address {
            IpAddr::V6(v6) => v6.to_ipv4_mapped().map_or(address, IpAddr::V4),
            v4 => v4,
        };
        match (self.address, address) {
            (IpAddr::V4(net), IpAddr::V4(a)) => {
                let mask = u32::MAX.checked_shl(32 - u32::from(self.prefix)).unwrap_or(0);
                u32::from(net) & mask == u32::from(a) & mask
            }
            (IpAddr::V6(net), IpAddr::V6(a)) => {
                let mask = u128::MAX.checked_shl(128 - u32::from(self.prefix)).unwrap_or(0);
                u128::from(net) & mask == u128::from(a) & mask
            }
            _ => false,
        }
    }
}

impl Share {
    /// Whether a machine at `address` may mount it over NFS.
    pub fn nfs_allows(&self, address: IpAddr) -> bool {
        self.nfs.iter().filter_map(|n| Network::parse(n).ok()).any(|n| n.contains(address))
    }
}

/// The shares.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Shares {
    /// Each, in the order of the file.
    pub shares: Vec<Share>,
}

/// The file as it is written.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct File {
    shares_version: Option<u32>,
    #[serde(default)]
    share: Vec<Share>,
}

impl Shares {
    /// The share named `name`, in any case, as Windows asks for one.
    pub fn find(&self, name: &str) -> Option<&Share> {
        self.shares.iter().find(|s| same_name(&s.name, name))
    }

    /// Adds `share`, checked, after the others: an error for a name
    /// taken.
    pub fn add(&mut self, share: Share) -> Result<(), String> {
        check(&share)?;
        if self.find(&share.name).is_some() {
            return Err(format!("there is a share called {} already", share.name));
        }
        self.shares.push(share);
        Ok(())
    }

    /// Takes the share named `name` away: whether there was one.
    pub fn remove(&mut self, name: &str) -> bool {
        let before = self.shares.len();
        self.shares.retain(|s| !same_name(&s.name, name));
        self.shares.len() != before
    }

    /// The file's text, with a comment that says what it is.
    pub fn to_toml(&self) -> String {
        let mut text = String::from(
            "# The folders smbd(8) shares with Windows PCs and other computers on the\n\
             # network (shares.toml(5), ADR 0228). share(1) changes this file: share add,\n\
             # share remove, share list.\n\n",
        );
        text.push_str(&format!("shares_version = {VERSION}\n"));
        for share in &self.shares {
            text.push_str("\n[[share]]\n");
            text.push_str(&format!("name = {}\n", quoted(&share.name)));
            text.push_str(&format!("folder = {}\n", quoted(&share.folder)));
            if !share.comment.is_empty() {
                text.push_str(&format!("comment = {}\n", quoted(&share.comment)));
            }
            if share.read_only {
                text.push_str("read-only = true\n");
            }
            if !share.nfs.is_empty() {
                let networks: Vec<String> = share.nfs.iter().map(|n| quoted(n)).collect();
                text.push_str(&format!("nfs = [{}]\n", networks.join(", ")));
            }
        }
        text
    }
}

/// Reads the file's `text`: every share checked, and no name twice.
pub fn from_toml(text: &str) -> Result<Shares, Error> {
    let file: File = toml::from_str(text).map_err(|err| {
        let mut error = Error::from_toml(text, &err);
        error.message = error.message.replace("service.toml(5)", "shares.toml(5)");
        error
    })?;
    if let Some(version) = file.shares_version
        && version > VERSION
    {
        return Err(Error::new(format!(
            "shares_version {version} is newer than this Dukovo reads, {VERSION}"
        )));
    }
    let mut shares = Shares::default();
    for share in file.share {
        let name = share.name.clone();
        shares.add(share).map_err(|why| Error::new(format!("the share {name}: {why}")))?;
    }
    Ok(shares)
}

/// Whether two names of shares are the same, as Windows compares them.
pub fn same_name(a: &str, b: &str) -> bool {
    a.chars().flat_map(char::to_lowercase).eq(b.chars().flat_map(char::to_lowercase))
}

/// Checks a share's name: 1 to [`NAME_MAX`] characters, none that
/// Windows keeps out of a share's name (`\ / : * ? " < > |`, `[ ] ; = +
/// ,` and controls), not [`IPC`], and not only dots or spaces.
pub fn check_name(name: &str) -> Result<(), String> {
    let count = name.chars().count();
    if count == 0 || count > NAME_MAX {
        return Err(format!("a share's name has 1 to {NAME_MAX} characters"));
    }
    if let Some(c) = name.chars().find(|c| c.is_control() || "\\/:*?\"<>|[];=+,".contains(*c)) {
        let shown =
            if c.is_control() { "a control character".to_string() } else { format!("`{c}`") };
        return Err(format!("a share's name cannot hold {shown}"));
    }
    if name.chars().all(|c| c == '.' || c == ' ') || name.starts_with(' ') || name.ends_with(' ') {
        return Err("a share's name cannot start or end with a space, or be only dots".into());
    }
    if same_name(name, IPC) {
        return Err(format!("{IPC} is the name of the pipes PCs list the shares through"));
    }
    Ok(())
}

/// Checks a share: its name, an absolute folder, and a comment on one
/// line.
pub fn check(share: &Share) -> Result<(), String> {
    check_name(&share.name)?;
    let normal = share.folder.starts_with('/')
        && !share.folder.contains('\0')
        && !share.folder.split('/').any(|part| part == "." || part == "..");
    if !normal {
        return Err(format!(
            "its folder, {}, is not an absolute path without . or ..",
            share.folder
        ));
    }
    if share.comment.chars().count() > COMMENT_MAX || share.comment.chars().any(char::is_control) {
        return Err(format!("its comment is one line of at most {COMMENT_MAX} characters"));
    }
    if share.nfs.len() > NETWORKS_MAX {
        return Err(format!("it lists more than {NETWORKS_MAX} networks for NFS"));
    }
    for network in &share.nfs {
        Network::parse(network)?;
    }
    Ok(())
}

/// `text` as a TOML basic string.
fn quoted(text: &str) -> String {
    let mut out = String::from("\"");
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if c.is_control() => out.push_str(&format!("\\u{:04X}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    fn music() -> Share {
        Share {
            name: "Music".into(),
            folder: "/users/chase/Music".into(),
            comment: "Chase's \"good\" music \\ all of it".into(),
            read_only: true,
            nfs: Vec::new(),
        }
    }

    #[test]
    fn shares_go_to_the_file_and_back() {
        let mut shares = Shares::default();
        shares.add(music()).unwrap();
        let other = Share {
            name: "Ünïcode files".into(),
            folder: "/shared/files".into(),
            comment: String::new(),
            read_only: false,
            nfs: vec!["192.168.1.0/24".into(), "fd00::1".into()],
        };
        shares.add(other).unwrap();
        let text = shares.to_toml();
        assert_eq!(from_toml(&text), Ok(shares.clone()));
        assert!(text.contains("read-only = true"));
        assert!(text.contains("nfs = [\"192.168.1.0/24\", \"fd00::1\"]"));
        assert_eq!(from_toml(""), Ok(Shares::default()));
    }

    #[test]
    fn names_are_the_same_in_any_case() {
        let mut shares = Shares::default();
        shares.add(music()).unwrap();
        assert_eq!(shares.find("MUSIC").map(|s| s.folder.as_str()), Some("/users/chase/Music"));
        assert!(shares.add(Share { name: "music".into(), ..music() }).is_err());
        assert!(shares.remove("mUsIc"));
        assert!(!shares.remove("Music"));
    }

    #[test]
    fn networks_hold_their_machines() {
        let lan = Network::parse("192.168.1.0/24").unwrap();
        assert!(lan.contains("192.168.1.77".parse().unwrap()));
        assert!(lan.contains("::ffff:192.168.1.77".parse().unwrap()));
        assert!(!lan.contains("192.168.2.1".parse().unwrap()));
        assert!(!lan.contains("fe80::1".parse().unwrap()));
        let one = Network::parse("10.0.2.2").unwrap();
        assert_eq!(one.prefix, 32);
        assert!(one.contains("10.0.2.2".parse().unwrap()));
        assert!(!one.contains("10.0.2.3".parse().unwrap()));
        assert!(Network::parse("0.0.0.0/0").unwrap().contains("8.8.8.8".parse().unwrap()));
        let v6 = Network::parse("fd00::/8").unwrap();
        assert!(v6.contains("fd12::5".parse().unwrap()));
        assert!(!v6.contains("fe80::1".parse().unwrap()));
        for bad in ["", "host", "10.0.0.0/33", "fd00::/129", "10.0.0.0/", "10.0.0.0/x"] {
            assert!(Network::parse(bad).is_err(), "{bad}");
        }
        let share = Share { nfs: vec!["10.0.2.0/24".into()], ..music() };
        assert!(share.nfs_allows("10.0.2.15".parse().unwrap()));
        assert!(!music().nfs_allows("10.0.2.15".parse().unwrap()));
        assert!(check(&Share { nfs: vec!["nope".into()], ..music() }).is_err());
    }

    #[test]
    fn bad_names_folders_and_files_are_refused() {
        for name in ["", "a/b", "a\\b", "c$:", "ipc$", "..", " x", "x ", "a\tb", &"x".repeat(81)] {
            assert!(check_name(name).is_err(), "{name:?}");
        }
        for name in ["C$", "Music", "My files", "x".repeat(80).as_str()] {
            assert_eq!(check_name(name), Ok(()), "{name:?}");
        }
        for folder in ["relative", "/a/../b", "/a/./b", ""] {
            assert!(check(&Share { folder: folder.into(), ..music() }).is_err(), "{folder}");
        }
        let twice =
            "[[share]]\nname = \"a\"\nfolder = \"/a\"\n[[share]]\nname = \"A\"\nfolder = \"/b\"\n";
        assert!(from_toml(twice).unwrap_err().message.contains("already"));
        let unknown = "[[share]]\nname = \"a\"\nfolder = \"/a\"\nguest = true\n";
        assert_eq!(from_toml(unknown).unwrap_err().line, Some(4));
        assert!(from_toml("shares_version = 2\n").is_err());
    }
}
