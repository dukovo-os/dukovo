// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Rescue's count of the starts that were not seen through (ADR 0460):
//! a GRUB environment block of its own beside the slot state, which the
//! boot menu (image/grub/rescue.cfg) adds one to at each start whose
//! last was not seen through, but an update's own try, and the health
//! check sets back to 0 once a start is. At [`AFTER`], the menu starts
//! Rescue by itself.
//!
//! It is apart from the slot state, so that a count cut short by a power
//! cut can only start Rescue or not, never another slot.

use crate::envblock::{self, BLOCK};

/// Where the count is on each EFI system partition, beside GRUB's menu.
pub const COUNT: &str = "boot/grub/rescue.env";

/// The block's one variable.
pub const STARTS: &str = "rescue_starts";

/// How many starts in a row that were not seen through start Rescue.
pub const AFTER: u8 = 3;

/// A count of `starts`, as a whole block; more than [`AFTER`] is written
/// as [`AFTER`], as the menu writes it.
#[must_use]
pub fn encode(starts: u8) -> [u8; BLOCK] {
    let starts = starts.min(AFTER);
    let digit = [b'0' + starts];
    let value = core::str::from_utf8(&digit).unwrap_or("0");
    envblock::write(
        &["# Starts not seen through, in a row: Rescue at 3 (ADR 0460)"],
        &[(STARTS, value)],
    )
    .unwrap_or([b'#'; BLOCK])
}

/// The count a block holds: 0 for a block without one, and [`AFTER`]
/// for any value but 0, 1 and 2, as the menu reads it. `None` if the
/// block is not GRUB's.
#[must_use]
pub fn decode(block: &[u8]) -> Option<u8> {
    let variables = envblock::parse(block)?;
    Some(match envblock::get(&variables, STARTS) {
        None | Some("0") => 0,
        Some("1") => 1,
        Some("2") => 2,
        Some(_) => AFTER,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_count_reads_back_as_the_menu_reads_it() {
        for starts in 0..=AFTER {
            assert_eq!(decode(&encode(starts)), Some(starts));
        }
        assert_eq!(decode(&encode(9)), Some(AFTER));
        let empty = envblock::write(&[], &[]).unwrap();
        assert_eq!(decode(&empty), Some(0));
        let odd = envblock::write(&[], &[(STARTS, "7")]).unwrap();
        assert_eq!(decode(&odd), Some(AFTER));
        assert_eq!(decode(b"not a block"), None);
        assert_eq!(encode(0).len(), BLOCK);
    }
}
