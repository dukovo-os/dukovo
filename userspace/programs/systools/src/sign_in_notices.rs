// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Notices on the sign-in screens (ADR 0504): what the owner should know
//! when they next sign in, kept until the next start or until whoever
//! wrote it takes it back. Each writer keeps one file in [`DIR`]: backup
//! for a backup that failed or is overdue (ADR 0463), bundled for an
//! installed app whose catalog knows flaws in it (ADR 0389),
//! bundled-withdrawn for an installed version a catalog or feed withdrew
//! (ADR 0593), bisect for what a search of `changes bisect` found, which
//! the health check of its last try keeps (ADR 0475), and tune for a
//! virtual machine no profile was chosen for, once after an install (ADR
//! 0476). consoled and serial-console show them under the issue text,
//! after a failed start's notice (ADR 0501). A writer tells the consoles
//! too, once, when its notice changes, unless it writes before they run,
//! as tune does.

use std::path::Path;

/// Where the notices are kept, one file each, in memory.
pub const DIR: &str = "/system/live/sign-in";

/// Writes `lines` as the notice `name`, or removes it when there are
/// none; whether it changed to a notice not shown before, which its
/// writer then tells the consoles of.
pub fn put(name: &str, lines: &[String]) -> bool {
    let path = Path::new(DIR).join(name);
    let before = std::fs::read_to_string(&path).unwrap_or_default();
    if lines.is_empty() {
        let _ = std::fs::remove_file(&path);
        return false;
    }
    let text = lines.iter().fold(String::new(), |text, line| text + line + "\n");
    if text == before {
        return false;
    }
    let written = std::fs::create_dir_all(DIR).and_then(|()| std::fs::write(&path, &text));
    if let Err(err) = written {
        eprintln!("{name}: cannot write {}: {err}", path.display());
        return false;
    }
    true
}

/// The notices kept, by their names.
#[must_use]
pub fn all() -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(DIR) else { return Vec::new() };
    let mut paths: Vec<_> = entries.flatten().map(|entry| entry.path()).collect();
    paths.sort();
    paths.iter().filter_map(|path| std::fs::read_to_string(path).ok()).collect()
}

/// One notice that is kept, with its name and when it was written.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    /// The name of its file.
    pub name: String,
    /// Its lines, as the consoles show them.
    pub lines: Vec<String>,
    /// When it was written, in seconds since 1970, where the machine says.
    pub at: Option<i64>,
}

/// The notice that the file at `path` is, called `name`: None when the file
/// is not there, cannot be read, or says nothing.
#[must_use]
pub fn entry_at(path: &Path, name: &str) -> Option<Entry> {
    let text = std::fs::read_to_string(path).ok()?;
    let lines: Vec<String> =
        text.lines().map(str::trim_end).filter(|line| !line.is_empty()).map(String::from).collect();
    if lines.is_empty() {
        return None;
    }
    let at = std::fs::metadata(path)
        .and_then(|meta| meta.modified())
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .and_then(|since| i64::try_from(since.as_secs()).ok());
    Some(Entry { name: name.to_string(), lines, at })
}

/// The notices kept in `dir`, by their names, each with its lines and
/// when it was written; what cannot be read is left out.
#[must_use]
pub fn entries_in(dir: &Path) -> Vec<Entry> {
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut paths: Vec<_> = entries.flatten().map(|entry| entry.path()).collect();
    paths.sort();
    paths.iter().filter_map(|path| entry_at(path, &path.file_name()?.to_string_lossy())).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn folder(what: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("notices-{what}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn the_notices_kept_are_listed_by_name_with_their_lines_and_time() {
        let dir = folder("kept");
        std::fs::write(
            dir.join("bundled"),
            "App X has a known flaw.
Update it.
",
        )
        .unwrap();
        std::fs::write(
            dir.join("backup"),
            "The last backup failed.
",
        )
        .unwrap();
        let found = entries_in(&dir);
        let names: Vec<&str> = found.iter().map(|entry| entry.name.as_str()).collect();
        assert_eq!(names, ["backup", "bundled"]);
        assert_eq!(found[0].lines, ["The last backup failed."]);
        assert_eq!(found[1].lines, ["App X has a known flaw.", "Update it."]);
        // Written just now: the time is within the last minute.
        let now =
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs()
                as i64;
        assert!(
            found
                .iter()
                .all(|entry| entry.at.is_some_and(|at| (now - 60..=now + 60).contains(&at)))
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn an_empty_notice_and_a_folder_that_is_not_there_give_nothing() {
        let dir = folder("empty");
        std::fs::write(
            dir.join("blank"),
            "
  
",
        )
        .unwrap();
        std::fs::create_dir(dir.join("subfolder")).unwrap();
        assert_eq!(entries_in(&dir), Vec::new());
        assert_eq!(entries_in(&dir.join("not-there")), Vec::new());
        assert_eq!(entry_at(&dir.join("not-there"), "x"), None);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
