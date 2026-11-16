// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The reason kept with the hold of the system or the firmware set
//! (ADR 0565), in `/system/state/sysupdate/hold-reasons.json`.
//!
//! `update hold ITEM --why TEXT` writes it, as root, and `update unhold`
//! takes it away. The hold itself stays in updates.toml(5), which has no
//! key for a reason: that file is for what updated reads, and a reason is
//! for people. A reason belongs to the version the hold named when it was
//! written, and counts only while the hold names that version: settings
//! put back from before an update bring an older hold back without it.
//! Anyone may read the file, and so anyone who can sign in sees the
//! reason. A file that is not there, does not read or is of another
//! format holds none.
//!
//! An app's reason is not here: it is the note of the `held` event in
//! bundled(8)'s history.

use std::io;
use std::path::Path;

use serde::{Deserialize, Serialize};

use super::online::Hold;
use crate::json;

/// The file, one line of JSON, 0644, replaced whole.
pub const FILE: &str = "/system/state/sysupdate/hold-reasons.json";

/// The mode of the file: everyone reads it.
const MODE: u32 = 0o644;

/// Why one item is held.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Reason {
    /// The version the hold named when the reason was written.
    pub version: String,
    /// What was said: one line of at most 200 characters.
    pub text: String,
    /// The user ID of who held it; for an administrator who used
    /// `elevate`, their own and not root's.
    pub by: u32,
    /// When, in milliseconds since 1970.
    pub at_ms: u64,
}

/// The reasons of the system's hold and the firmware's.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Reasons {
    pub system: Option<Reason>,
    pub firmware: Option<Reason>,
}

impl Reasons {
    /// The reasons kept, none when there is no file or it does not read.
    #[must_use]
    pub fn read() -> Reasons {
        Reasons::read_from(Path::new(FILE))
    }

    /// The reasons `file` keeps.
    #[must_use]
    pub fn read_from(file: &Path) -> Reasons {
        let Ok(text) = std::fs::read_to_string(file) else { return Reasons::default() };
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) else {
            return Reasons::default();
        };
        if value.get("format").and_then(serde_json::Value::as_u64) != Some(u64::from(json::FORMAT))
        {
            return Reasons::default();
        }
        serde_json::from_value(value).unwrap_or_default()
    }

    /// The reason of `item`, `system` or `firmware`, whatever its version.
    fn slot(&mut self, item: &str) -> &mut Option<Reason> {
        if item == "system" { &mut self.system } else { &mut self.firmware }
    }

    /// The reason of `item`, `system` or `firmware`, while it stands:
    /// the version the hold names, `held`, is the one it was written for.
    #[must_use]
    pub fn standing(&self, item: &str, held: Option<&str>) -> Option<&Reason> {
        let reason = if item == "system" { &self.system } else { &self.firmware };
        reason.as_ref().filter(|reason| held == Some(reason.version.as_str()))
    }

    /// The reasons that stand against `hold`, which is what `update
    /// settings --json` says as `hold_reasons`.
    #[must_use]
    pub fn standing_of(&self, hold: &Hold) -> Reasons {
        Reasons {
            system: self.standing("system", hold.system.as_deref()).cloned(),
            firmware: self.standing("firmware", hold.firmware.as_deref()).cloned(),
        }
    }

    /// Keeps `reason` for `item`, or takes the one it has away.
    pub fn set(&mut self, item: &str, reason: Option<Reason>) {
        *self.slot(item) = reason;
    }

    /// Takes away each reason that does not stand against `hold`.
    pub fn drop_stale(&mut self, hold: &Hold) {
        *self = self.standing_of(hold);
    }

    /// The file's text: one line, with its newline.
    ///
    /// # Panics
    ///
    /// Never: strings and numbers always serialize.
    #[must_use]
    pub fn text(&self) -> String {
        let mut line = json::line(self).expect("reasons are strings and numbers");
        line.push('\n');
        line
    }

    /// Writes the reasons to the file.
    ///
    /// # Errors
    ///
    /// If the file cannot be replaced.
    pub fn save(&self) -> io::Result<()> {
        self.save_to(Path::new(FILE))
    }

    /// Writes the reasons to `file`.
    ///
    /// # Errors
    ///
    /// If `file` cannot be replaced.
    pub fn save_to(&self, file: &Path) -> io::Result<()> {
        crate::util::replace_whole(file, &self.text(), MODE)
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use super::*;

    fn reason(version: &str, text: &str) -> Reason {
        Reason { version: version.into(), text: text.into(), by: 1000, at_ms: 1_794_000_000_000 }
    }

    fn scratch(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("reasons-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("hold-reasons.json")
    }

    fn held(system: Option<&str>, firmware: Option<&str>) -> Hold {
        Hold { system: system.map(Into::into), firmware: firmware.map(Into::into) }
    }

    #[test]
    fn the_reasons_are_written_whole_and_read_back() {
        let file = scratch("whole");
        let reasons = Reasons {
            system: Some(reason("1.0.0-alpha.2", "Waiting for the nginx fix")),
            firmware: None,
        };
        reasons.save_to(&file).unwrap();
        assert_eq!(
            std::fs::read_to_string(&file).unwrap(),
            "{\"format\":1,\"system\":{\"version\":\"1.0.0-alpha.2\",\"text\":\"Waiting for the \
             nginx fix\",\"by\":1000,\"at_ms\":1794000000000},\"firmware\":null}\n"
        );
        assert_eq!(std::fs::metadata(&file).unwrap().permissions().mode() & 0o777, 0o644);
        assert_eq!(Reasons::read_from(&file), reasons);
        // Written again, it replaces what was there and leaves no file beside it.
        Reasons::default().save_to(&file).unwrap();
        assert_eq!(Reasons::read_from(&file), Reasons::default());
        assert!(!file.with_extension("json.partial").exists());
        let _ = std::fs::remove_dir_all(file.parent().unwrap()); // a test's leftovers
    }

    #[test]
    fn a_file_that_is_missing_or_not_the_record_holds_no_reason() {
        let file = scratch("missing");
        assert_eq!(Reasons::read_from(&file), Reasons::default());
        for text in [
            "reasons\n",
            "{\"system\":null}",
            "{\"format\":2,\"system\":{\"version\":\"1\",\"text\":\"x\",\"by\":0,\"at_ms\":1}}",
            "{\"format\":1,\"system\":{\"version\":\"1\"}}",
        ] {
            std::fs::write(&file, text).unwrap();
            assert_eq!(Reasons::read_from(&file), Reasons::default(), "{text:?}");
        }
        let _ = std::fs::remove_dir_all(file.parent().unwrap()); // a test's leftovers
    }

    #[test]
    fn a_reason_counts_only_while_the_hold_names_its_version() {
        let reasons = Reasons {
            system: Some(reason("1.0.0-alpha.2", "Waiting for the nginx fix")),
            firmware: Some(reason("fw-7", "The new set breaks the dock")),
        };
        let now = held(Some("1.0.0-alpha.2"), Some("fw-7"));
        assert_eq!(
            reasons.standing("system", now.system.as_deref()).unwrap().text,
            "Waiting for the nginx fix"
        );
        assert_eq!(reasons.standing("firmware", now.firmware.as_deref()).unwrap().by, 1000);
        assert_eq!(reasons.standing_of(&now), reasons);
        // The settings were put back to a hold at another version, or none.
        let other = held(Some("1.0.0-alpha.1"), None);
        assert_eq!(reasons.standing("system", other.system.as_deref()), None);
        assert_eq!(reasons.standing("firmware", other.firmware.as_deref()), None);
        assert_eq!(reasons.standing_of(&other), Reasons::default());
    }

    #[test]
    fn a_stale_reason_is_dropped_and_a_set_or_cleared_one_is_kept_or_gone() {
        let mut reasons = Reasons {
            system: Some(reason("1.0.0-alpha.2", "old")),
            firmware: Some(reason("fw-7", "kept")),
        };
        reasons.drop_stale(&held(Some("1.0.0-alpha.3"), Some("fw-7")));
        assert_eq!(reasons.system, None);
        assert_eq!(reasons.firmware.as_ref().unwrap().text, "kept");
        reasons.set("system", Some(reason("1.0.0-alpha.3", "new")));
        assert_eq!(reasons.system.as_ref().unwrap().text, "new");
        reasons.set("firmware", None);
        assert_eq!(reasons.firmware, None);
    }
}
