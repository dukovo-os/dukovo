// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The settings put back after an update that did not prove itself (ADR
//! 0454). Staging an update snapshots `@settings` and `@state`, read-only,
//! into `@snapshots/update`. When the update falls back, the default
//! starts with `fallback=` on its command line, and Spark, before any
//! service starts, keeps the failed attempt's settings beside the
//! snapshot as `settings-failed`, for a look at what went wrong, and
//! makes `@settings` a writable snapshot of the one from before the
//! update. The state is not put back: it holds the logs, and the
//! two-version rule keeps it readable by the version before.
//!
//! Btrfs cannot rename a subvolume, so putting back is a removal and a
//! snapshot, and a power cut can fall between any two steps. Each step is
//! decided afresh from what is there ([`next`]), so the steps run again
//! from wherever they stopped, and end the same; the file `restored` in
//! the update's folder says they are done.

/// The update's folder in the root's top level.
pub const UPDATE: &str = "@snapshots/update";

/// The settings subvolume.
pub const SETTINGS: &str = "@settings";

/// The update's snapshots and files, in [`UPDATE`]: the settings and the
/// state from before the update, the failed attempt's settings, what the
/// update was, and the mark that the settings are back.
pub const BEFORE_SETTINGS: &str = "settings";
/// The state from before the update.
pub const BEFORE_STATE: &str = "state";
/// The failed attempt's settings, kept by a fallback.
pub const FAILED_SETTINGS: &str = "settings-failed";
/// What the update was: its versions and slot, written as it is staged.
pub const ABOUT: &str = "about";
/// The mark that a fallback put the settings back.
pub const RESTORED: &str = "restored";

/// What is there, as each step looks.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Seen {
    /// `@settings` is there.
    pub settings: bool,
    /// The snapshot of the settings from before the update is there.
    pub before: bool,
    /// The failed attempt's settings are kept.
    pub failed: bool,
    /// The mark that the settings are back is there.
    pub restored: bool,
}

/// The next step of putting the settings back.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    /// Snapshot `@settings`, read-only, as the failed attempt's.
    KeepFailed,
    /// Remove `@settings`.
    RemoveSettings,
    /// Make `@settings` a writable snapshot of the one from before.
    PutBack,
    /// Write the mark.
    Mark,
    /// Make an empty `@settings`, as a root without one and without a
    /// snapshot to put back needs to start: the defaults fill it.
    MakeEmpty,
    /// Nothing more to do: the settings are back, or there is nothing to
    /// put back.
    Done,
}

/// The next step, from what is there, when the update fell back
/// (`fallback`) or not: without a fallback, only a missing `@settings`
/// is mended. Looking cannot tell the settings put back from the failed
/// ones, so whoever runs the steps writes the mark straight after
/// [`Step::PutBack`]; a power cut before the mark has them put back again
/// from the snapshot, which ends the same.
#[must_use]
pub fn next(seen: Seen, fallback: bool) -> Step {
    if !fallback || seen.restored || !seen.before {
        return if seen.settings { Step::Done } else { Step::MakeEmpty };
    }
    match (seen.settings, seen.failed) {
        (true, false) => Step::KeepFailed,
        (true, true) => Step::RemoveSettings,
        (false, _) => Step::PutBack,
    }
}

/// How many steps putting the settings back can take: keep, remove, put
/// back and mark.
pub const MOST_STEPS: usize = 4;

#[cfg(test)]
mod tests {
    use alloc::vec::Vec;

    use super::*;

    /// The steps, applied to a model of the subvolumes, as Spark runs
    /// them: after [`Step::PutBack`] comes [`Step::Mark`] in the same
    /// round. Returns the steps taken, stopping after `cut` of them, as a
    /// power cut there would.
    fn run(seen: &mut Seen, fallback: bool, cut: usize) -> Vec<Step> {
        let mut taken = Vec::new();
        let mut put_back = false;
        for _ in 0..=MOST_STEPS + 1 {
            if taken.len() == cut {
                break;
            }
            let step = if put_back { Step::Mark } else { next(*seen, fallback) };
            match step {
                Step::KeepFailed => seen.failed = true,
                Step::RemoveSettings => seen.settings = false,
                Step::PutBack => {
                    seen.settings = true;
                    put_back = true;
                }
                Step::Mark => {
                    seen.restored = true;
                    put_back = false;
                }
                Step::MakeEmpty => seen.settings = true,
                Step::Done => break,
            }
            taken.push(step);
        }
        taken
    }

    #[test]
    fn a_fallback_keeps_the_failed_settings_and_puts_the_old_ones_back() {
        let mut seen = Seen { settings: true, before: true, ..Seen::default() };
        let steps = run(&mut seen, true, usize::MAX);
        assert_eq!(steps, [Step::KeepFailed, Step::RemoveSettings, Step::PutBack, Step::Mark]);
        assert_eq!(seen, Seen { settings: true, before: true, failed: true, restored: true });
        // Done is done: the next start with the flag changes nothing.
        assert_eq!(run(&mut seen, true, usize::MAX), []);
    }

    #[test]
    fn a_power_cut_after_any_step_ends_the_same() {
        let whole = {
            let mut seen = Seen { settings: true, before: true, ..Seen::default() };
            run(&mut seen, true, usize::MAX);
            seen
        };
        for cut in 0..MOST_STEPS {
            let mut seen = Seen { settings: true, before: true, ..Seen::default() };
            run(&mut seen, true, cut);
            // The settings are never missing for good: the next start
            // carries on.
            run(&mut seen, true, usize::MAX);
            assert_eq!(seen, whole, "cut after {cut} steps");
        }
    }

    #[test]
    fn without_a_snapshot_or_a_fallback_only_missing_settings_are_mended() {
        let mut seen = Seen { settings: true, ..Seen::default() };
        assert_eq!(run(&mut seen, true, usize::MAX), []);
        let mut missing = Seen::default();
        assert_eq!(run(&mut missing, true, usize::MAX), [Step::MakeEmpty]);
        let mut snapshot = Seen { settings: true, before: true, ..Seen::default() };
        assert_eq!(run(&mut snapshot, false, usize::MAX), []);
        let mut cut = Seen { before: true, failed: true, ..Seen::default() };
        assert_eq!(run(&mut cut, false, usize::MAX), [Step::MakeEmpty]);
    }
}
