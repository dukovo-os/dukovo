// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! A whole machine restored from its backups (ADR 0464). `backup restore
//! --machine` receives the root's places from a backup into
//! `@snapshots/machine-restore/staged`, with the files of `/system/state`
//! the machine's record carries, and writes [`ABOUT`] last. At the next
//! start, before any service starts, Spark puts each staged subvolume in
//! the place of the one there, which it keeps, read-only, in
//! `@snapshots/machine-restore/before-<time>`; then it writes the state's
//! files and marks the restore done.
//!
//! Btrfs cannot rename a subvolume, so putting one in place is a removal
//! and a snapshot, and a power cut can fall between any two steps. As for
//! the settings after an update (`restore`), each step is decided afresh
//! from what is there ([`next`]), so the steps run again from wherever
//! they stopped and end the same. A mark beside each staged subvolume
//! says it is in place, written straight after the snapshot that puts
//! it; the mark [`DONE`] says the whole restore is, after which what was
//! staged is removed.

use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::fmt::Write as _;

/// The restore's folder in the root's top level.
pub const FOLDER: &str = "@snapshots/machine-restore";

/// The folder in [`FOLDER`] that is staged: a subvolume, holding the
/// staged subvolumes, the state's files, [`ABOUT`] and the marks.
pub const STAGED: &str = "staged";

/// What is staged, written last.
pub const ABOUT: &str = "about";

/// The mark that the restore is done.
pub const DONE: &str = "done";

/// The folder of the state's files, in [`STAGED`].
pub const STATE: &str = "state";

/// The state subvolume, where the state's files go.
pub const STATE_SUBVOLUME: &str = "@state";

/// The subvolumes a restore may put in place, by their names without
/// `@`, in the order it puts them: never a system slot, the snapshots or
/// the state.
pub const SUBVOLUMES: [&str; 4] = ["settings", "applications", "users", "shared"];

/// The name of the mark that the staged subvolume `name` is in place.
#[must_use]
pub fn mark(name: &str) -> String {
    let mut text = String::from(name);
    text.push_str(".done");
    text
}

/// The folder, in [`FOLDER`], that keeps what a restore staged at
/// `staged` replaced.
#[must_use]
pub fn kept_folder(staged: &str) -> String {
    let mut text = String::from("before-");
    text.push_str(staged);
    text
}

/// What a restore staged, as [`ABOUT`] says it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct About {
    /// The time of the backup restored, as its name.
    pub time: String,
    /// When it was staged, as a backup's time name.
    pub staged: String,
    /// The machine's name.
    pub machine: String,
    /// The subvolumes staged, in the order of [`SUBVOLUMES`].
    pub subvolumes: Vec<String>,
    /// Folders of the state whose files are removed before the state's
    /// files are written: `grants`.
    pub clear: Vec<String>,
    /// The state's files, by their path in the state, with their modes.
    pub state: Vec<(u16, String)>,
}

/// Whether `name` is a time as backups are named: `YYYY-MM-DD-HHMMSS`.
fn is_time(name: &str) -> bool {
    let bytes = name.as_bytes();
    bytes.len() == 17
        && bytes.iter().enumerate().all(|(i, &b)| match i {
            4 | 7 | 10 => b == b'-',
            _ => b.is_ascii_digit(),
        })
}

/// Whether `path` is a plain relative path: names of letters, digits,
/// `-`, `_` and `.` joined by `/`, none `.` or `..`.
fn is_plain(path: &str) -> bool {
    !path.is_empty()
        && path.split('/').all(|name| {
            !name.is_empty()
                && name != "."
                && name != ".."
                && name.bytes().all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
        })
}

impl About {
    /// Reads [`ABOUT`]'s text strictly: a line that does not read, a
    /// subvolume a restore may not put in place, or a path that is not
    /// plain, is an error.
    pub fn parse(text: &str) -> Result<About, String> {
        let mut about = About::default();
        for line in text.lines() {
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let (word, rest) = line.split_once(' ').unwrap_or((line, ""));
            match word {
                "time" if is_time(rest) => about.time = rest.into(),
                "staged" if is_time(rest) => about.staged = rest.into(),
                "machine" if !rest.is_empty() && is_plain(rest) && !rest.contains('/') => {
                    about.machine = rest.into();
                }
                "subvolume"
                    if SUBVOLUMES.contains(&rest)
                        && !about.subvolumes.iter().any(|s| s == rest) =>
                {
                    about.subvolumes.push(rest.into());
                }
                "clear" if is_plain(rest) && !rest.contains('/') => about.clear.push(rest.into()),
                "state" => {
                    let (mode, path) = rest.split_once(' ').unwrap_or(("", ""));
                    let mode = u16::from_str_radix(mode, 8).ok().filter(|m| *m <= 0o777);
                    match mode {
                        Some(mode) if is_plain(path) => about.state.push((mode, path.into())),
                        _ => return Err(alloc::format!("not a state file: {line:?}")),
                    }
                }
                _ => return Err(alloc::format!("not understood: {line:?}")),
            }
        }
        if about.time.is_empty() || about.staged.is_empty() || about.subvolumes.is_empty() {
            return Err("names no time, staging time or subvolume".into());
        }
        about.subvolumes.sort_by_key(|s| SUBVOLUMES.iter().position(|n| n == s));
        Ok(about)
    }

    /// The text of [`ABOUT`].
    #[must_use]
    pub fn text(&self) -> String {
        let mut text = String::from(
            "# A whole machine's restore, staged by backup(8) (ADR 0464): Spark puts\n\
             # these subvolumes in place at the next start.\n",
        );
        let _ = writeln!(text, "time {}", self.time);
        let _ = writeln!(text, "staged {}", self.staged);
        if !self.machine.is_empty() {
            let _ = writeln!(text, "machine {}", self.machine);
        }
        for name in &self.subvolumes {
            let _ = writeln!(text, "subvolume {name}");
        }
        for folder in &self.clear {
            let _ = writeln!(text, "clear {folder}");
        }
        for (mode, path) in &self.state {
            let _ = writeln!(text, "state {mode:04o} {path}");
        }
        text
    }
}

/// What is there of one staged subvolume, as each step looks.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Seen {
    /// The subvolume in its place, `@name`, is there.
    pub live: bool,
    /// The staged one is there.
    pub staged: bool,
    /// The one it replaces is kept.
    pub kept: bool,
    /// The mark that the staged one is in place is there.
    pub put: bool,
}

/// The next step of putting one staged subvolume in place.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    /// Snapshot `@name`, read-only, into the kept folder.
    Keep,
    /// Remove `@name`.
    Remove,
    /// Make `@name` a writable snapshot of the staged one, then write the
    /// mark.
    Put,
    /// Make `@name` a writable snapshot of the kept one, then write the
    /// mark: the staged one is gone, as only a damaged disk leaves it, and
    /// this subvolume stays as it was.
    PutKept,
    /// Nothing more to do for it.
    Done,
    /// Neither it, the staged one nor the kept one is there.
    Lost,
}

/// The next step for one staged subvolume, from what is there. Looking
/// cannot tell the subvolume put in place from the one it replaces, so
/// whoever runs the steps writes the mark straight after [`Step::Put`]
/// and [`Step::PutKept`];
/// a power cut before the mark has it removed and put again from the
/// staged one, which ends the same.
#[must_use]
pub fn next(seen: Seen) -> Step {
    match seen {
        Seen { put: true, live: true, .. } => Step::Done,
        Seen { live: true, kept: false, .. } => Step::Keep,
        Seen { live: true, kept: true, .. } => Step::Remove,
        Seen { live: false, staged: true, .. } => Step::Put,
        Seen { live: false, staged: false, kept: true, .. } => Step::PutKept,
        Seen { live: false, staged: false, kept: false, .. } => Step::Lost,
    }
}

/// How many steps one subvolume can take: keep, remove, put.
pub const MOST_STEPS: usize = 3;

/// Whether `path` is a plain relative path, as a state file's is.
#[must_use]
pub fn is_state_path(path: &str) -> bool {
    is_plain(path)
}

/// The path of the state file `path` beside its new copy, written before
/// it is renamed into place.
#[must_use]
pub fn new_copy(path: &str) -> String {
    let mut text = path.to_string();
    text.push_str(".restored");
    text
}

#[cfg(test)]
mod tests {
    use alloc::vec;

    use super::*;

    fn about() -> About {
        About {
            time: "2026-11-11-030000".into(),
            staged: "2026-11-12-101500".into(),
            machine: "web1".into(),
            subvolumes: vec!["settings".into(), "users".into()],
            clear: vec!["grants".into()],
            state: vec![(0o644, "bundled/installed".into()), (0o600, "grants/1000".into())],
        }
    }

    #[test]
    fn about_goes_through_its_text() {
        let about = about();
        assert_eq!(About::parse(&about.text()).unwrap(), about);
        // In any order, the subvolumes are put in place in one.
        let text = about
            .text()
            .replace("subvolume settings\nsubvolume users", "subvolume users\nsubvolume settings");
        assert_eq!(About::parse(&text).unwrap().subvolumes, ["settings", "users"]);
    }

    #[test]
    fn about_is_read_strictly() {
        let text = about().text();
        for (from, to) in [
            ("subvolume users", "subvolume system.a"),
            ("subvolume users", "subvolume snapshots"),
            ("subvolume users", "subvolume state"),
            ("subvolume users", "subvolume settings"),
            ("state 0600 grants/1000", "state 0600 ../settings/passwd"),
            ("state 0600 grants/1000", "state 0600 /system/settings/passwd"),
            ("state 0600 grants/1000", "state 1777 grants/1000"),
            ("state 0600 grants/1000", "state rw grants/1000"),
            ("clear grants", "clear ../settings"),
            ("time 2026-11-11-030000", "time yesterday"),
            ("machine web1", "machine ../web1"),
            ("machine web1", "owner root"),
        ] {
            let changed = text.replace(from, to);
            assert_ne!(changed, text);
            assert!(About::parse(&changed).is_err(), "{to}");
        }
        assert!(About::parse("time 2026-11-11-030000\nstaged 2026-11-11-030000\n").is_err());
    }

    /// The steps for one subvolume, applied to a model of what is there,
    /// as Spark runs them: after [`Step::Put`] the mark in the same round.
    /// Returns the steps taken, stopping after `cut` of them, as a power
    /// cut there would; a cut after Put but before its mark is `cut_mark`.
    fn run(seen: &mut Seen, cut: usize, cut_mark: bool) -> Vec<Step> {
        let mut taken = Vec::new();
        for _ in 0..=MOST_STEPS + 1 {
            if taken.len() == cut {
                break;
            }
            let step = next(*seen);
            match step {
                Step::Keep => seen.kept = true,
                Step::Remove => seen.live = false,
                Step::Put | Step::PutKept => {
                    seen.live = true;
                    if !(cut_mark && taken.len() + 1 == cut) {
                        seen.put = true;
                    }
                }
                Step::Done | Step::Lost => break,
            }
            taken.push(step);
        }
        taken
    }

    #[test]
    fn a_staged_subvolume_is_put_in_place_and_the_old_one_kept() {
        let mut seen = Seen { live: true, staged: true, ..Seen::default() };
        assert_eq!(run(&mut seen, usize::MAX, false), [Step::Keep, Step::Remove, Step::Put]);
        assert_eq!(seen, Seen { live: true, staged: true, kept: true, put: true });
        // Done is done: the next start changes nothing, staged or not.
        assert_eq!(run(&mut seen, usize::MAX, false), []);
        seen.staged = false;
        assert_eq!(next(seen), Step::Done);
    }

    #[test]
    fn a_power_cut_after_any_step_ends_the_same() {
        let whole = Seen { live: true, staged: true, kept: true, put: true };
        for cut in 0..=MOST_STEPS {
            for cut_mark in [false, true] {
                let mut seen = Seen { live: true, staged: true, ..Seen::default() };
                run(&mut seen, cut, cut_mark);
                // The subvolume is missing only between the removal and
                // the snapshot, and the next start carries on.
                run(&mut seen, usize::MAX, false);
                assert_eq!(seen, whole, "cut after {cut} steps, mark cut {cut_mark}");
            }
        }
    }

    #[test]
    fn a_staged_subvolume_gone_leaves_the_kept_one() {
        let mut seen = Seen { live: false, staged: false, kept: true, put: false };
        assert_eq!(run(&mut seen, usize::MAX, false), [Step::PutKept]);
        assert!(seen.live && seen.put);
        assert_eq!(next(seen), Step::Done);
        assert_eq!(next(Seen::default()), Step::Lost);
    }

    #[test]
    fn names_are_made_as_spark_and_backup_share_them() {
        assert_eq!(mark("users"), "users.done");
        assert_eq!(kept_folder("2026-11-12-101500"), "before-2026-11-12-101500");
        assert_eq!(new_copy("grants/1000"), "grants/1000.restored");
        assert!(is_state_path("bundled/installed"));
        assert!(!is_state_path("bundled/../x"));
    }
}
