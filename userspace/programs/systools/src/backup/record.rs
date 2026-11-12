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
//! It is JSON, encrypted and tagged as a backup is, one a round, in the
//! folder `machine`.

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::bundles::{grants, registry};
use crate::util::{self, Fail};

/// The record's format.
pub const FORMAT: u32 = 1;

/// The folder of `/system/state` bundled keeps its registry in.
const BUNDLED: &str = "bundled";

/// The folder of `/system/state` bundled keeps the grants in.
const GRANTS: &str = "grants";

/// The most a record holds of `/system/state`, in bytes: a person's
/// grants are at most 4096 lines.
const MOST: usize = 64 << 20;

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
}

/// Whether a record may carry the file `path` of `/system/state`: one of
/// bundled's registry's files, or a person's grants, by user number.
pub fn may_carry(path: &str) -> bool {
    match path.split_once('/') {
        Some((BUNDLED, name)) => registry::FILES.contains(&name),
        Some((GRANTS, uid)) => {
            !uid.is_empty()
                && uid.len() <= 10
                && uid.bytes().all(|b| b.is_ascii_digit())
                && (uid == "0" || !uid.starts_with('0'))
        }
        _ => false,
    }
}

impl Record {
    /// The record of the machine `machine` at `time`, whose places are
    /// `places`, with what it carries read from `state`, the machine's
    /// `/system/state`. Every registry file is carried, empty if bundled
    /// has not written it, so that a restore leaves none from before.
    pub fn gather(
        machine: &str,
        time: &str,
        places: BTreeMap<String, String>,
        state: &Path,
    ) -> Result<Record, Fail> {
        debug_assert_eq!(grants::FOLDER, "/system/state/grants");
        debug_assert_eq!(registry::STATE, "/system/state/bundled");
        let read = |path: &Path| match fs::read_to_string(path) {
            Ok(text) => Ok(text),
            Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(String::new()),
            Err(err) => Err(util::failed(&path.display(), err)),
        };
        let mut carried = BTreeMap::new();
        for name in registry::FILES {
            let path = format!("{BUNDLED}/{name}");
            carried.insert(path.clone(), read(&state.join(&path))?);
        }
        let folder = state.join(GRANTS);
        match fs::read_dir(&folder) {
            Ok(entries) => {
                for entry in entries {
                    let entry = entry.map_err(|e| util::failed(&folder.display(), e))?;
                    let Some(name) = entry.file_name().to_str().map(String::from) else {
                        continue;
                    };
                    let path = format!("{GRANTS}/{name}");
                    if may_carry(&path) && entry.file_type().is_ok_and(|t| t.is_file()) {
                        carried.insert(path, read(&entry.path())?);
                    }
                }
            }
            Err(err) if err.kind() == io::ErrorKind::NotFound => {}
            Err(err) => return Err(util::failed(&folder.display(), err)),
        }
        Ok(Record {
            format: FORMAT,
            machine: machine.into(),
            time: time.into(),
            places,
            state: carried,
        })
    }

    /// The record as its file holds it, before encryption.
    pub fn text(&self) -> String {
        let mut text = serde_json::to_string_pretty(self).unwrap_or_default();
        text.push('\n');
        text
    }

    /// Reads a record's text strictly: its format this Dukovo's, and
    /// nothing carried that a record may not carry.
    pub fn parse(text: &str) -> Result<Record, String> {
        if text.len() > MOST {
            return Err("larger than a record can be".into());
        }
        let record: Record = serde_json::from_str(text).map_err(|e| e.to_string())?;
        if record.format != FORMAT {
            return Err(format!(
                "a record of format {}, which this Dukovo does not read",
                record.format
            ));
        }
        if let Some(path) = record.state.keys().find(|p| !may_carry(p)) {
            return Err(format!("carries {path}, which a record never does"));
        }
        if let Some((folder, path)) = record
            .places
            .iter()
            .find(|(folder, path)| !super::layout::is_place(folder) || !path.starts_with('/'))
        {
            return Err(format!("names the place {folder} as {path:?}"));
        }
        Ok(record)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_record_carries_bundleds_registry_and_the_grants_alone() {
        assert!(may_carry("bundled/installed"));
        assert!(may_carry("bundled/defaults"));
        assert!(may_carry("grants/1000"));
        assert!(may_carry("grants/0"));
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
        ] {
            assert!(!may_carry(path), "{path}");
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
        let places = BTreeMap::from([("users".to_string(), "/users".to_string())]);
        let record = Record::gather("m", "2026-11-11-030000", places, &dir).unwrap();
        let _ = fs::remove_dir_all(&dir);
        assert_eq!(record.state.len(), registry::FILES.len() + 1);
        assert_eq!(record.state["bundled/installed"], "everyone\torg.example.notes\t1.0\n");
        assert_eq!(record.state["bundled/held"], "");
        assert_eq!(record.state["grants/1000"], "grant lines\n");
        assert_eq!(Record::parse(&record.text()).unwrap(), record);
    }

    #[test]
    fn a_record_that_carries_more_is_refused() {
        let mut record = Record {
            format: FORMAT,
            machine: "m".into(),
            time: "2026-11-11-030000".into(),
            places: BTreeMap::from([("users".to_string(), "/users".to_string())]),
            state: BTreeMap::new(),
        };
        assert!(Record::parse(&record.text()).is_ok());
        record.state.insert("../settings/passwd".into(), String::new());
        assert!(Record::parse(&record.text()).unwrap_err().contains("passwd"));
        record.state.clear();
        record.places.insert("../x".into(), "/x".into());
        assert!(Record::parse(&record.text()).is_err());
        record.places.remove("../x");
        record.format = 2;
        assert!(Record::parse(&record.text()).unwrap_err().contains("format 2"));
        assert!(Record::parse("{\"format\": 1, \"extra\": 1}").is_err());
    }
}
