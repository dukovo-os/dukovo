// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The room a stage needs on the Dukovo root, checked before it writes
//! anything (ADR 0451). The spare slot starts as a snapshot of the running
//! one, which shares its data, so the stage writes only the files the
//! release changes. Their sizes, plus a margin for Btrfs's metadata (the
//! extent and checksum trees, and the copies of tree blocks a write makes),
//! are compared with the space the root has free. A stage that would run
//! out refuses at the start, with the slot state and the running slot as
//! they were, rather than fail half way through a write.

use std::path::Path;

use crate::util::{Fail, fail};

/// One MiB, the unit the refusal counts in.
const MIB: u64 = 1 << 20;

/// What the margin adds whatever the files come to, in bytes: the
/// directories and links the stage writes, the slot's own subvolume, and
/// the blocks a transaction copies.
const MARGIN_FIXED: u64 = 16 * MIB;

/// The part of the files' sizes the margin adds for the checksums and
/// extent records of what is written: an eighth.
const MARGIN_SHARE: u64 = 8;

/// What a stage that writes `changed` bytes needs on the root: the bytes,
/// an eighth more for Btrfs's metadata, and 16 MiB.
#[must_use]
pub fn needed(changed: u64) -> u64 {
    changed.saturating_add(changed.div_ceil(MARGIN_SHARE)).saturating_add(MARGIN_FIXED)
}

/// The refusal for a stage that needs `needed` bytes where `free` are
/// free, in MiB, the need rounded up and the room down.
#[must_use]
pub fn refusal(needed: u64, free: u64) -> String {
    format!(
        "the update needs {} MiB on the Dukovo root, and {} MiB are free",
        needed.div_ceil(MIB),
        free / MIB
    )
}

/// Refuses a stage of a release that changes `changed` bytes of the
/// slot's files if the root has less than it needs free.
///
/// # Errors
///
/// [`refusal`]'s words, when `free` is less than [`needed`] of `changed`.
pub fn check(changed: u64, free: u64) -> Result<(), Fail> {
    let needed = needed(changed);
    if free < needed {
        return Err(fail(refusal(needed, free)));
    }
    Ok(())
}

/// The bytes free for a new file on the root's Btrfs, mounted at `top`.
///
/// # Errors
///
/// If the filesystem will not say how full it is.
pub fn free(top: &Path) -> Result<u64, Fail> {
    crate::df::statvfs(top.as_os_str())
        .map(|space| space.available)
        .map_err(|err| crate::util::failed(&top.display(), err))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_margin_is_an_eighth_and_16_mib() {
        assert_eq!(needed(0), 16 * MIB);
        assert_eq!(needed(800 * MIB), 800 * MIB + 100 * MIB + 16 * MIB);
        // An eighth rounds up: a byte more than a multiple of eight.
        assert_eq!(needed(9), 9 + 2 + 16 * MIB);
    }

    #[test]
    fn the_sum_does_not_overflow() {
        assert_eq!(needed(u64::MAX), u64::MAX);
    }

    #[test]
    fn the_refusal_rounds_the_need_up_and_the_room_down() {
        // 16 MiB and one byte needs 17; 3 MiB less a byte is 2 free.
        assert_eq!(
            refusal(16 * MIB + 1, 3 * MIB - 1),
            "the update needs 17 MiB on the Dukovo root, and 2 MiB are free"
        );
        assert_eq!(
            refusal(16 * MIB, 0),
            "the update needs 16 MiB on the Dukovo root, and 0 MiB are free"
        );
    }

    #[test]
    fn a_stage_that_fits_goes_ahead_and_one_that_does_not_is_refused() {
        assert!(check(0, 16 * MIB).is_ok());
        assert!(check(80 * MIB, needed(80 * MIB)).is_ok());
        let err = check(80 * MIB, needed(80 * MIB) - 1).unwrap_err();
        assert_eq!(
            err.message,
            "the update needs 106 MiB on the Dukovo root, and 105 MiB are free"
        );
    }
}
