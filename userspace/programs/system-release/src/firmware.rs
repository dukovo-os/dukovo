// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The optional firmware package as firmware sets (ADR 0468). Closed
//! firmware stays out of the system (ADR 0007): a set is its own signed
//! list, `firmware/<version>.json` among the update repository's
//! targets, signed by the release keys as a release is, and its files
//! sit in the repository's store beside the releases'. A channel may
//! name the set it offers beside its release. On a machine, each slot
//! names the set it starts with (`slots::firmware`), so that a new set
//! is tried as a new version is, and falls back by itself.
//!
//! A set says how each of its files is loaded. Only CPU microcode is
//! loaded so far, by GRUB, as a module the kernel recognises (ADR 0014);
//! a list that names any other way is refused, so that no set holds a
//! file a machine would keep but never load.

use std::collections::HashSet;
use std::path::Path;

use serde::{Deserialize, Serialize};
use slots::version::Version;

use crate::list::is_sha256;

/// Where the set `version` is among the targets.
#[must_use]
pub fn target_path(version: &str) -> String {
    format!("firmware/{version}.json")
}

/// One firmware set.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Set {
    /// Its version, such as `2026.11.12`, the date it was made, compared
    /// as a system version is.
    pub firmware: String,
    /// Its files.
    pub files: Vec<File>,
}

/// One file of a set.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct File {
    /// Its name, such as `intel-ucode.bin`: one word, for messages.
    pub name: String,
    /// How it is loaded.
    pub load: Load,
    /// Its SHA-256, by which the store holds it.
    pub sha256: String,
    /// Its size in bytes.
    pub size: u64,
    /// Its license, as an SPDX expression, for the license manifest
    /// (ADR 0021).
    pub license: String,
}

/// How a file of a set is loaded.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Load {
    /// CPU microcode, which GRUB hands the kernel as a module, and which
    /// the kernel writes to every CPU before anything else (ADR 0014).
    Microcode,
}

impl Set {
    /// Reads a set's list, checked.
    ///
    /// # Errors
    ///
    /// If it is not JSON of a set, or fails [`Set::check`].
    pub fn parse(bytes: &[u8]) -> Result<Set, String> {
        let set: Set = serde_json::from_slice(bytes)
            .map_err(|err| format!("the firmware set's list is malformed: {err}"))?;
        set.check()?;
        Ok(set)
    }

    /// The list as it is signed.
    ///
    /// # Panics
    ///
    /// Never: its fields are all JSON's.
    #[must_use]
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut bytes = serde_json::to_vec_pretty(self).expect("a firmware set is JSON");
        bytes.push(b'\n');
        bytes
    }

    /// Checks the set: a version, and files each named once, by a word,
    /// with a SHA-256 and a license, and one microcode file at most.
    ///
    /// # Errors
    ///
    /// The first thing wrong.
    pub fn check(&self) -> Result<(), String> {
        if Version::parse(&self.firmware).is_none() || self.firmware.len() > 64 {
            return Err(format!("the firmware set's version {:?} is not a version", self.firmware));
        }
        let mut names = HashSet::new();
        for file in &self.files {
            let word = !file.name.is_empty()
                && file.name.len() <= 64
                && file.name.bytes().all(|b| b.is_ascii_alphanumeric() || b"-_.+".contains(&b))
                && !file.name.starts_with('.');
            if !word {
                return Err(format!(
                    "firmware {}: {:?} is not a file's name",
                    self.firmware, file.name
                ));
            }
            if !names.insert(file.name.as_str()) {
                return Err(format!("firmware {}: {} is named twice", self.firmware, file.name));
            }
            if !is_sha256(&file.sha256) {
                return Err(format!("firmware {}: {} has no SHA-256", self.firmware, file.name));
            }
            if file.license.trim().is_empty() {
                return Err(format!("firmware {}: {} has no license", self.firmware, file.name));
            }
        }
        if self.files.iter().filter(|f| f.load == Load::Microcode).count() > 1 {
            return Err(format!("firmware {} has more than one microcode file", self.firmware));
        }
        Ok(())
    }

    /// Its CPU microcode file, if it has one.
    #[must_use]
    pub fn microcode(&self) -> Option<&File> {
        self.files.iter().find(|f| f.load == Load::Microcode)
    }

    /// The files' total size in bytes.
    #[must_use]
    pub fn bytes(&self) -> u64 {
        self.files.iter().map(|f| f.size).sum()
    }
}

/// The firmware sets among `targets`, as the TUF client read them, each
/// where its version says, oldest first.
///
/// # Errors
///
/// If a set's list does not check, or is not where its version says.
pub fn sets(targets: &[(String, Vec<u8>)]) -> Result<Vec<(Set, Vec<u8>)>, String> {
    let mut sets = Vec::new();
    for (path, bytes) in targets.iter().filter(|(path, _)| path.starts_with("firmware/")) {
        let set = Set::parse(bytes)?;
        if target_path(&set.firmware) != *path {
            return Err(format!("{path} holds the list of firmware {}", set.firmware));
        }
        sets.push((set, bytes.clone()));
    }
    sets.sort_by(|(a, _), (b, _)| compare(&a.firmware, &b.firmware));
    Ok(sets)
}

/// Two sets' versions compared, by precedence.
#[must_use]
pub fn compare(a: &str, b: &str) -> std::cmp::Ordering {
    match (Version::parse(a), Version::parse(b)) {
        (Some(a), Some(b)) => a.cmp(&b),
        _ => a.cmp(b),
    }
}

/// The firmware set `version`, or the newest, of the repository folder
/// `dir`, read from `root` as a computer reads it, with its list's bytes
/// as signed.
///
/// # Errors
///
/// If the repository does not check from `root`, or holds no such set.
pub fn open(dir: &Path, root: &[u8], version: Option<&str>) -> Result<(Set, Vec<u8>), String> {
    let sets = sets(&crate::repo::read_targets(dir, root)?)?;
    let found = match version {
        Some(version) => sets.into_iter().find(|(set, _)| set.firmware == version),
        None => sets.into_iter().last(),
    };
    found.ok_or_else(|| match version {
        Some(version) => format!("{} holds no firmware {version}", dir.display()),
        None => format!("{} holds no firmware set", dir.display()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn microcode(name: &str) -> File {
        File {
            name: name.into(),
            load: Load::Microcode,
            sha256: "0".repeat(64),
            size: 12,
            license: "LicenseRef-Intel-Microcode".into(),
        }
    }

    #[test]
    fn checks_a_set() {
        let set = Set { firmware: "2026.11.12".into(), files: vec![microcode("intel-ucode.bin")] };
        assert_eq!(Set::parse(&set.to_bytes()).unwrap(), set);
        assert_eq!(set.microcode().unwrap().name, "intel-ucode.bin");
        let bad = |set: Set| set.check().unwrap_err();
        assert!(
            bad(Set { firmware: "2026-11-12".into(), ..set.clone() }).contains("not a version")
        );
        let named = |name: &str| Set { files: vec![microcode(name)], ..set.clone() };
        assert!(bad(named("../x")).contains("not a file's name"));
        assert!(bad(named(".hidden")).contains("not a file's name"));
        let twice = Set { files: vec![microcode("a"), microcode("a")], ..set.clone() };
        assert!(bad(twice).contains("named twice"));
        let two = Set { files: vec![microcode("a"), microcode("b")], ..set.clone() };
        assert!(bad(two).contains("more than one microcode"));
        let unlicensed = File { license: " ".into(), ..microcode("a") };
        assert!(bad(Set { files: vec![unlicensed], ..set.clone() }).contains("no license"));
        // A way of loading this system does not know is refused.
        let text = String::from_utf8(set.to_bytes()).unwrap().replace("microcode\"", "driver\"");
        assert!(Set::parse(text.as_bytes()).unwrap_err().contains("malformed"));
    }

    #[test]
    fn finds_sets_where_their_versions_say() {
        let set = |v: &str| Set { firmware: v.into(), files: vec![microcode("intel-ucode.bin")] };
        let targets = vec![
            (target_path("2026.11.12"), set("2026.11.12").to_bytes()),
            ("system/0.15.0.json".into(), b"{}".to_vec()),
            (target_path("2026.9.1"), set("2026.9.1").to_bytes()),
        ];
        let found: Vec<String> =
            sets(&targets).unwrap().into_iter().map(|(s, _)| s.firmware).collect();
        assert_eq!(found, ["2026.9.1", "2026.11.12"]);
        let moved = vec![(target_path("2026.11.13"), set("2026.11.12").to_bytes())];
        assert!(sets(&moved).unwrap_err().contains("holds the list of firmware 2026.11.12"));
    }
}
