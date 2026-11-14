// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! What the last look at the update server found, kept in
//! `/system/state/sysupdate/checked` for dukovo-check(8), which says
//! whether an update waits and whether the checks still happen.
//!
//! Each read of the channel's release writes it: a check of updated(8),
//! `update check`, `sysupdate check` and a stage that follows the channel.
//! Its time is when the server was last asked, which is not when an
//! update last came. A read that failed leaves the file as it was, so
//! that its age grows while the server cannot be reached.

use std::io;
use std::path::Path;

use serde::{Deserialize, Serialize};

use super::online::Plan;

/// The record, one line of JSON.
pub(crate) const FILE: &str = "/system/state/sysupdate/checked";

/// What the channel offered, against the release this system runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Found {
    /// The release this system runs, or an older one.
    Current,
    /// A newer release this system takes now.
    Update,
    /// A newer release in a wave that does not reach this machine yet.
    Waiting,
    /// A newer release the owner holds back.
    Held,
    /// A newer release that was withdrawn.
    Withdrawn,
}

/// One look at the update server.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Checked {
    /// When the channel was read, in seconds since 1970.
    pub(crate) time: u64,
    /// The release this system ran then.
    pub(crate) running: String,
    /// The release the channel offered.
    pub(crate) offered: String,
    pub(crate) found: Found,
}

impl Checked {
    /// What `plan` says of the channel for a system running `running`,
    /// read at `time`.
    pub(crate) fn of(plan: &Plan, running: &str, time: u64) -> Checked {
        let (offered, found) = match plan {
            Plan::Current { offered } => (offered, Found::Current),
            Plan::Waiting { offered, .. } => (offered, Found::Waiting),
            Plan::Held { offered, .. } => (offered, Found::Held),
            Plan::Withdrawn { offered, .. } => (offered, Found::Withdrawn),
            Plan::Update { offered, .. } => (offered, Found::Update),
        };
        Checked { time, running: running.to_string(), offered: offered.clone(), found }
    }

    /// Keeps the record in [`FILE`].
    ///
    /// # Errors
    ///
    /// When the file cannot be written, as it cannot be to a person who
    /// is not root.
    pub(crate) fn keep(&self) -> io::Result<()> {
        self.keep_in(Path::new(FILE))
    }

    /// Keeps the record in `file`: written to a file beside it and renamed
    /// over it, so that a reader finds the old record or the new.
    fn keep_in(&self, file: &Path) -> io::Result<()> {
        let mut partial = file.as_os_str().to_owned();
        partial.push(".partial");
        let line = serde_json::to_string(self).map_err(io::Error::other)?;
        std::fs::write(&partial, format!("{line}\n"))?;
        std::fs::rename(&partial, file)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record() -> Checked {
        Checked {
            time: 1_794_000_000,
            running: "0.16.0".into(),
            offered: "0.17.0".into(),
            found: Found::Update,
        }
    }

    #[test]
    fn what_a_plan_found_is_kept_as_a_word() {
        let plan = Plan::Current { offered: "0.16.0".into() };
        let checked = Checked::of(&plan, "0.16.0", 5);
        assert_eq!(checked.found, Found::Current);
        assert_eq!((checked.time, checked.offered.as_str()), (5, "0.16.0"));
        let plan = Plan::Withdrawn { offered: "0.17.0".into(), reason: "a bug".into() };
        assert_eq!(Checked::of(&plan, "0.16.0", 5).found, Found::Withdrawn);
        let plan = Plan::Waiting { offered: "0.17.0".into(), day: 3, held: None };
        assert_eq!(Checked::of(&plan, "0.16.0", 5).found, Found::Waiting);
        let plan = Plan::Held { offered: "0.17.0".into(), held: "0.16.1".into() };
        assert_eq!(Checked::of(&plan, "0.16.0", 5).found, Found::Held);
    }

    #[test]
    fn the_record_is_one_line_that_reads_back_and_the_old_one_is_replaced() {
        let dir = std::env::temp_dir().join(format!("sysupdate-checked-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("checked");
        record().keep_in(&file).unwrap();
        let text = std::fs::read_to_string(&file).unwrap();
        assert_eq!(
            text,
            "{\"time\":1794000000,\"running\":\"0.16.0\",\"offered\":\"0.17.0\",\"found\":\"update\"}\n"
        );
        assert_eq!(serde_json::from_str::<Checked>(&text).unwrap(), record());
        let later = Checked { found: Found::Current, ..record() };
        later.keep_in(&file).unwrap();
        let text = std::fs::read_to_string(&file).unwrap();
        assert_eq!(serde_json::from_str::<Checked>(&text).unwrap(), later);
        assert!(!dir.join("checked.partial").exists());
        let _ = std::fs::remove_dir_all(dir); // a test's leftovers
    }
}
