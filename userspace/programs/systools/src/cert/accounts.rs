// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! What anyone may know of the accounts at authorities (ADR 0513): when
//! each was made and whether it has a contact, never the contact or the
//! account's key.
//!
//! The accounts themselves are in a folder only root reads, since each
//! holds a key. `cert list` runs as anyone, so certd writes this summary
//! beside the store, 0644, from the accounts it finds: when an account is
//! made, and at each look, which also makes it for the accounts of a
//! machine that had them before the summary existed.

use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::Path;

use serde::{Deserialize, Serialize};

use super::store::{Lock, Store};
use crate::util;

/// What is told of one account.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Public {
    /// When it was made, in milliseconds since 1970.
    pub made_ms: i64,
    /// Whether it was made with a contact address.
    pub contact: bool,
}

/// The parts of an account's file that the summary is made from.
#[derive(Deserialize)]
struct Kept {
    created: String,
    contact: Option<String>,
}

/// The summary of the accounts in the folder of `store`, by the account's
/// file name. A file that does not read is left out.
fn summary(store: &Store) -> Result<BTreeMap<String, Public>, String> {
    let folder = store.accounts();
    let entries = match fs::read_dir(&folder) {
        Ok(entries) => entries,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(BTreeMap::new()),
        Err(err) => return Err(format!("{}: {}", folder.display(), util::strip_os_error(&err))),
    };
    let mut found = BTreeMap::new();
    for entry in entries.flatten() {
        let Ok(file) = entry.file_name().into_string() else { continue };
        if !file.ends_with(".json") {
            continue;
        }
        let Ok(bytes) = fs::read(entry.path()) else { continue };
        let Ok(kept) = serde_json::from_slice::<Kept>(&bytes) else { continue };
        let Some(made) = tuf::time::parse(&kept.created) else { continue };
        found.insert(
            file,
            Public { made_ms: made.saturating_mul(1_000), contact: kept.contact.is_some() },
        );
    }
    Ok(found)
}

/// The text of the summary file for `found`.
fn text_of(found: &BTreeMap<String, Public>) -> Result<String, String> {
    serde_json::to_string(found).map(|text| text + "\n").map_err(|why| why.to_string())
}

/// Writes the summary file anew, holding the store's lock. Nothing is
/// written when the file says it already.
///
/// # Errors
///
/// With why the accounts cannot be read or the file cannot be written.
pub fn write(store: &Store, _lock: &Lock) -> Result<(), String> {
    let found = summary(store)?;
    let path = store.accounts_public();
    let text = text_of(&found)?;
    if found.is_empty() && !path.exists() {
        return Ok(());
    }
    if fs::read_to_string(&path).is_ok_and(|now| now == text) {
        return Ok(());
    }
    replace(&path, &text)
}

/// Makes the summary file as the accounts say, taking the store's lock
/// only when it has to change.
///
/// # Errors
///
/// As [`write`], and with why the lock cannot be taken.
pub fn refresh(store: &Store) -> Result<(), String> {
    let found = summary(store)?;
    let current = fs::read_to_string(store.accounts_public()).ok();
    let wanted = text_of(&found)?;
    let nothing_to_say = found.is_empty() && current.is_none();
    if nothing_to_say || current.as_deref() == Some(wanted.as_str()) {
        return Ok(());
    }
    let lock = store.lock()?;
    write(store, &lock)
}

/// Writes `text` to `path`, 0644, through a temporary file and a rename.
fn replace(path: &Path, text: &str) -> Result<(), String> {
    let failed =
        |path: &Path, err: &io::Error| format!("{}: {}", path.display(), util::strip_os_error(err));
    let temporary = path.with_extension("new");
    let _ = fs::remove_file(&temporary);
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o644)
        .open(&temporary)
        .map_err(|err| failed(&temporary, &err))?;
    file.set_permissions(fs::Permissions::from_mode(0o644))
        .map_err(|err| failed(&temporary, &err))?;
    file.write_all(text.as_bytes()).map_err(|err| failed(&temporary, &err))?;
    file.sync_all().map_err(|err| failed(&temporary, &err))?;
    fs::rename(&temporary, path).map_err(|err| failed(path, &err))
}

/// What is told of the accounts, by the account's file name: none when the
/// file is not there or does not read.
pub fn read(store: &Store) -> BTreeMap<String, Public> {
    let Ok(bytes) = fs::read(store.accounts_public()) else { return BTreeMap::new() };
    serde_json::from_slice(&bytes).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::MetadataExt;
    use std::time::Duration;

    use super::*;
    use crate::cert::fixtures::Scratch;

    fn store(scratch: &Scratch) -> Store {
        Store::at(&scratch.0.join("certificates"), Duration::from_secs(5))
    }

    fn account(store: &Store, file: &str, contact: &str) {
        let lock = store.lock().unwrap();
        let text = format!(
            "{{\"id\":\"secret-id\",\"key_pkcs8\":\"SECRET\",\"created\":\"2026-11-02T10:00:00Z\",\
             \"contact\":{contact}}}"
        );
        store.add_account(&lock, file, &text).unwrap();
    }

    #[test]
    fn the_summary_says_when_and_whether_there_is_a_contact_and_nothing_else() {
        let scratch = Scratch::new("accounts-summary");
        let store = store(&scratch);
        // Nothing to say, nothing written.
        refresh(&store).unwrap();
        assert!(!store.accounts_public().exists());
        account(&store, "a.example.json", "null");
        account(&store, "b.example-14000.json", "\"mailto:me@example.org\"");
        refresh(&store).unwrap();
        let text = fs::read_to_string(store.accounts_public()).unwrap();
        assert_eq!(
            text,
            "{\"a.example.json\":{\"made_ms\":1793613600000,\"contact\":false},\
             \"b.example-14000.json\":{\"made_ms\":1793613600000,\"contact\":true}}\n"
        );
        assert!(!text.contains("SECRET") && !text.contains("me@example.org"));
        assert_eq!(fs::metadata(store.accounts_public()).unwrap().mode() & 0o7777, 0o644);
        assert_eq!(
            read(&store)["a.example.json"],
            Public { made_ms: 1_793_613_600_000, contact: false }
        );
        assert!(read(&store)["b.example-14000.json"].contact);
    }

    #[test]
    fn a_summary_that_is_current_is_not_written_again() {
        let scratch = Scratch::new("accounts-current");
        let store = store(&scratch);
        account(&store, "a.example.json", "null");
        refresh(&store).unwrap();
        let first = fs::metadata(store.accounts_public()).unwrap().modified().unwrap();
        std::thread::sleep(Duration::from_millis(20));
        refresh(&store).unwrap();
        assert_eq!(fs::metadata(store.accounts_public()).unwrap().modified().unwrap(), first);
        // A file that does not read is left out, not a failure.
        let lock = store.lock().unwrap();
        store.add_account(&lock, "c.example.json", "not json").unwrap();
        drop(lock);
        refresh(&store).unwrap();
        assert_eq!(read(&store).len(), 1);
    }

    #[test]
    fn nothing_is_known_when_the_file_is_missing_or_damaged() {
        let scratch = Scratch::new("accounts-none");
        let store = store(&scratch);
        assert!(read(&store).is_empty());
        fs::write(store.accounts_public(), "{").unwrap();
        assert!(read(&store).is_empty());
    }
}
