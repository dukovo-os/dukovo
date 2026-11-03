// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! An app's data, as each version left it (ADR 0384). Before another
//! version of an app first runs for a person, `bundle run`, as that
//! person, keeps a copy of the app's data folder as the version that last
//! ran left it, so that going back to that version brings its data back
//! too. It runs as the person, so it copies only what they may read.
//!
//! - `~/app-data/.snapshots/ID/.ran` says which version last ran.
//! - `~/app-data/.snapshots/ID/VERSION` is the data that version left.
//!
//! Going to a newer version, the data is copied and goes on with it.
//! Going back to an older one whose data is kept, the two change places,
//! so that what the newer version left is kept in turn. A copy is kept
//! for 30 days.

use std::cmp::Ordering;
use std::fs;
use std::io;
use std::os::unix::fs::DirBuilderExt;
use std::path::Path;
use std::time::{Duration, SystemTime};

use super::catalogs::compare_versions;

/// The folder of copies, in `~/app-data`.
pub const SNAPSHOTS: &str = ".snapshots";
/// The file that says which version last ran, in an app's copies.
const RAN: &str = ".ran";
/// How long a copy is kept.
pub const KEPT_FOR: Duration = Duration::from_hours(30 * 24);

/// A version as a folder's name: no `/`, and not hidden.
fn usable(version: &str) -> bool {
    !version.is_empty() && !version.contains('/') && !version.starts_with('.')
}

/// Copies the folder `from` to `to`, which must not be there: folders,
/// files and links, each as it is; anything else is left out.
fn copy_tree(from: &Path, to: &Path) -> io::Result<()> {
    fs::create_dir(to)?;
    for entry in fs::read_dir(from)? {
        let entry = entry?;
        let (source, target) = (entry.path(), to.join(entry.file_name()));
        let kind = entry.file_type()?;
        if kind.is_dir() {
            copy_tree(&source, &target)?;
        } else if kind.is_file() {
            fs::copy(&source, &target)?;
        } else if kind.is_symlink() {
            std::os::unix::fs::symlink(fs::read_link(&source)?, &target)?;
        }
    }
    fs::set_permissions(to, fs::metadata(from)?.permissions())
}

/// Marks `folder` as made `now`, which [`prune`] counts from.
fn touch(folder: &Path, now: SystemTime) {
    let _ = fs::File::open(folder).and_then(|f| f.set_modified(now));
}

/// Removes `path`, a folder, if it is there.
fn remove(path: &Path) -> io::Result<()> {
    match fs::remove_dir_all(path) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        other => other,
    }
}

/// Removes the copies in `copies` older than [`KEPT_FOR`].
fn prune(copies: &Path, now: SystemTime) {
    let Ok(entries) = fs::read_dir(copies) else { return };
    for entry in entries.flatten() {
        let old = entry
            .metadata()
            .and_then(|m| m.modified())
            .is_ok_and(|made| now.duration_since(made).is_ok_and(|age| age > KEPT_FOR));
        if old && entry.file_type().is_ok_and(|t| t.is_dir()) {
            let _ = fs::remove_dir_all(entry.path());
        }
    }
}

/// Brings the data of app `id` in `app_data` to `version`, which is about
/// to run, `now`: what is to be said of it, if anything.
///
/// # Errors
///
/// If a copy cannot be made or put back: the data is then as it was.
pub fn carry(
    app_data: &Path,
    id: &str,
    version: &str,
    now: SystemTime,
) -> Result<Option<String>, String> {
    let copies = app_data.join(SNAPSHOTS).join(id);
    let failed = |what: &Path, e: io::Error| format!("{}: {e}", what.display());
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(&copies)
        .map_err(|e| failed(&copies, e))?;
    let ran_file = copies.join(RAN);
    let ran = match fs::read_to_string(&ran_file) {
        Ok(text) => text.trim().to_string(),
        Err(e) if e.kind() == io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(failed(&ran_file, e)),
    };
    let mark = |version: &str| {
        let new = copies.join(".ran.new");
        fs::write(&new, format!("{version}\n"))
            .and_then(|()| fs::rename(&new, &ran_file))
            .map_err(|e| failed(&ran_file, e))
    };
    if ran == version {
        prune(&copies, now);
        return Ok(None);
    }
    if ran.is_empty() || !usable(&ran) || !usable(version) {
        mark(version)?;
        return Ok(None);
    }
    let data = app_data.join(id);
    let kept = copies.join(&ran);
    let back = copies.join(version);
    let going_back = compare_versions(version, &ran) == Ordering::Less && back.is_dir();
    remove(&kept).map_err(|e| failed(&kept, e))?;
    let said = if going_back {
        fs::rename(&data, &kept).map_err(|e| failed(&data, e))?;
        if let Err(e) = fs::rename(&back, &data) {
            let _ = fs::rename(&kept, &data);
            return Err(failed(&back, e));
        }
        touch(&kept, now);
        format!(
            "{id}: brought back the data {version} left, and kept what {ran} left for {} days",
            KEPT_FOR.as_secs() / 86_400
        )
    } else {
        if let Err(e) = copy_tree(&data, &kept) {
            let _ = fs::remove_dir_all(&kept);
            return Err(failed(&kept, e));
        }
        touch(&kept, now);
        format!(
            "{id}: kept a copy of the data {ran} left, for {} days, to go back to",
            KEPT_FOR.as_secs() / 86_400
        )
    };
    mark(version)?;
    prune(&copies, now);
    Ok(Some(said))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read(path: &Path) -> String {
        fs::read_to_string(path).unwrap()
    }

    #[test]
    fn data_goes_forward_and_comes_back() {
        let app_data = std::env::temp_dir().join(format!("snapshots-{}", std::process::id()));
        let _ = fs::remove_dir_all(&app_data);
        let id = "org.example.notes";
        let data = app_data.join(id);
        fs::create_dir_all(data.join("inner")).unwrap();
        fs::write(data.join("notes"), "1.0.0\n").unwrap();
        fs::write(data.join("inner/x"), "x").unwrap();
        std::os::unix::fs::symlink("notes", data.join("link")).unwrap();
        let now = SystemTime::now();
        // The first run says nothing; the same version again, nothing.
        assert_eq!(carry(&app_data, id, "1.0.0", now), Ok(None));
        assert_eq!(carry(&app_data, id, "1.0.0", now), Ok(None));
        // Forward: a copy of what 1.0.0 left, and the data goes on.
        let said = carry(&app_data, id, "1.1.0", now).unwrap().unwrap();
        assert!(said.contains("kept a copy of the data 1.0.0 left, for 30 days"), "{said}");
        let copy = app_data.join(SNAPSHOTS).join(id).join("1.0.0");
        assert_eq!(read(&copy.join("notes")), "1.0.0\n");
        assert_eq!(read(&copy.join("inner/x")), "x");
        assert_eq!(fs::read_link(copy.join("link")).unwrap(), Path::new("notes"));
        fs::write(data.join("notes"), "1.0.0\n1.1.0\n").unwrap();
        // Back: 1.0.0's data comes back, and 1.1.0's is kept.
        let said = carry(&app_data, id, "1.0.0", now).unwrap().unwrap();
        assert!(said.contains("brought back the data 1.0.0 left"), "{said}");
        assert_eq!(read(&data.join("notes")), "1.0.0\n");
        let newer = app_data.join(SNAPSHOTS).join(id).join("1.1.0");
        assert_eq!(read(&newer.join("notes")), "1.0.0\n1.1.0\n");
        // Back to a version with no copy: the data stays as it is.
        let said = carry(&app_data, id, "0.9.0", now).unwrap().unwrap();
        assert!(said.contains("kept a copy of the data 1.0.0 left"), "{said}");
        assert_eq!(read(&data.join("notes")), "1.0.0\n");
        // Copies older than 30 days go.
        let later = now + KEPT_FOR + Duration::from_secs(60);
        assert_eq!(carry(&app_data, id, "0.9.0", later), Ok(None));
        assert!(!newer.exists() && !copy.exists());
        fs::remove_dir_all(&app_data).unwrap();
    }
}
