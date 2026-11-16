// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The release the health check held after it fell back from it, which it
//! keeps in `/system/state/sysupdate/held.json`, and what `update check
//! --json` and `update settings --json` say of it.
//!
//! The health check writes the file; this module only reads it. A file
//! that is not there, cannot be read or is not the record is no hold of
//! this kind: the offer says it is held because it was asked, which is
//! what the settings alone can say.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::json;

/// The record, one object.
pub(crate) const FILE: &str = "/system/state/sysupdate/held.json";

/// What the health check wrote when it held the release it fell back
/// from.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub(crate) struct Held {
    /// The release held.
    pub(crate) version: String,
    /// Why: `fell-back`.
    pub(crate) why: String,
    /// When, in milliseconds since 1970.
    pub(crate) at_ms: u64,
    /// The number of the start that fell back.
    pub(crate) start: u64,
}

/// `held` of `update settings --json`: the record as the health check
/// wrote it.
#[derive(Debug, Serialize)]
pub(crate) struct HeldDocument {
    pub(crate) version: String,
    why: String,
    at_ms: u64,
    start: u64,
}

/// Why an offer is held, and when the reason began.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Because {
    /// `fell-back` or `asked`.
    pub(crate) because: &'static str,
    /// RFC 3339, for `fell-back`.
    pub(crate) at: Option<String>,
}

/// Takes the record away, with the hold it was made for. It is no fault
/// that there is none.
pub(crate) fn forget() {
    let _ = std::fs::remove_file(FILE);
}

impl Held {
    /// The record the health check kept, if there is one that reads.
    pub(crate) fn read() -> Option<Held> {
        Held::read_from(Path::new(FILE))
    }

    /// The record in `file`, if it is there and reads.
    pub(crate) fn read_from(file: &Path) -> Option<Held> {
        let text = std::fs::read_to_string(file).ok()?;
        serde_json::from_str(&text).ok().filter(|held: &Held| held.why == "fell-back")
    }

    /// Whether this record is the reason the hold at `version` is there.
    pub(crate) fn is_why(&self, version: &str) -> bool {
        self.version == version
    }

    /// The record as `update settings --json` says it.
    pub(crate) fn document(&self) -> HeldDocument {
        HeldDocument {
            version: self.version.clone(),
            why: self.why.clone(),
            at_ms: self.at_ms,
            start: self.start,
        }
    }
}

impl Because {
    /// Why the hold at `version` is there: because the release fell back
    /// when `held` names it, else because it was asked for.
    pub(crate) fn of(held: Option<&Held>, version: &str) -> Because {
        match held.filter(|held| held.is_why(version)) {
            Some(held) => Because {
                because: "fell-back",
                at: Some(json::time(i64::try_from(held.at_ms / 1000).unwrap_or(0))),
            },
            None => Because { because: "asked", at: None },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RECORD: &str =
        r#"{"version":"1.0.0-alpha.2","why":"fell-back","at_ms":1794000000000,"start":14}"#;

    fn scratch(name: &str, text: Option<&str>) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("update-held-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("held.json");
        if let Some(text) = text {
            std::fs::write(&file, text).unwrap();
        }
        file
    }

    #[test]
    fn the_record_the_health_check_wrote_is_read() {
        let file = scratch("reads", Some(RECORD));
        let held = Held::read_from(&file).unwrap();
        assert_eq!(held.version, "1.0.0-alpha.2");
        assert_eq!((held.at_ms, held.start), (1_794_000_000_000, 14));
        let _ = std::fs::remove_dir_all(file.parent().unwrap()); // a test's leftovers
    }

    #[test]
    fn a_missing_or_unreadable_record_is_no_hold_of_this_kind() {
        let missing = scratch("missing", None);
        assert_eq!(Held::read_from(&missing), None);
        let words = scratch("words", Some("held\n"));
        assert_eq!(Held::read_from(&words), None);
        let other = scratch("other", Some(&RECORD.replace("fell-back", "asked")));
        assert_eq!(Held::read_from(&other), None);
        let short = scratch("short", Some(r#"{"version":"1.0.0-alpha.2"}"#));
        assert_eq!(Held::read_from(&short), None);
        for file in [missing, words, other, short] {
            let _ = std::fs::remove_dir_all(file.parent().unwrap()); // a test's leftovers
        }
    }

    #[test]
    fn a_hold_is_a_fall_back_only_where_the_record_names_its_version() {
        let held: Held = serde_json::from_str(RECORD).unwrap();
        let fell = Because::of(Some(&held), "1.0.0-alpha.2");
        assert_eq!(fell.because, "fell-back");
        assert_eq!(fell.at.as_deref(), Some("2026-11-06T21:20:00Z"));
        assert_eq!(Because::of(Some(&held), "1.0.0-alpha.1").because, "asked");
        let asked = Because::of(None, "1.0.0-alpha.2");
        assert_eq!((asked.because, asked.at), ("asked", None));
    }
}
