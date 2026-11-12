// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Notices on the sign-in screens (ADR 0504): what the owner should know
//! when they next sign in, kept until the next start or until whoever
//! wrote it takes it back. Each writer keeps one file in [`DIR`]: backup
//! for a backup that failed or is overdue (ADR 0463), bundled for an
//! installed app whose catalog knows flaws in it (ADR 0389). consoled and
//! serial-console show them under the issue text, after a failed start's
//! notice (ADR 0501). A writer tells the consoles too, once, when its
//! notice changes.

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
