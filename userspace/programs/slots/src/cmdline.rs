// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! What the kernel's command line says of the slots, as the boot menu
//! writes it (dukovo-boot(7)): `slot=a` or `slot=b`, the slot GRUB
//! started, whose subvolume Spark mounts on `/system`; and
//! `fallback=a` or `fallback=b`, the slot of an update that ran out of
//! tries, which the default starts with, so that Spark puts the settings
//! from before it back before any service starts (ADR 0454); and
//! `trial=N`, a try of `changes bisect` the menu started in place of the
//! usual start ([`crate::trial`], ADR 0475). Only the words before a lone
//! `--` are the kernel's; a flag's last value counts, as the kernel takes
//! it.

use crate::state::Slot;
use crate::trial::MAX_NUMBER;

/// What the command line says of the slots.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Boot {
    /// The slot GRUB started, or none, as a menu from before Phase 15
    /// starts `@system.a`.
    pub slot: Option<Slot>,
    /// The slot of an update that ran out of tries.
    pub fallback: Option<Slot>,
    /// The number of the try of `changes bisect` this start is.
    pub trial: Option<u8>,
}

impl Boot {
    /// The slot to mount on `/system`: the one named, or slot A.
    #[must_use]
    pub fn system(&self) -> Slot {
        self.slot.unwrap_or(Slot::A)
    }
}

/// What `line`, the kernel's command line, says of the slots. A value
/// that names no slot is left out, as if the flag were not there.
#[must_use]
pub fn parse(line: &str) -> Boot {
    let mut boot = Boot::default();
    for word in line.split_ascii_whitespace().take_while(|&word| word != "--") {
        if let Some(letter) = word.strip_prefix("slot=") {
            boot.slot = Slot::named(letter);
        } else if let Some(letter) = word.strip_prefix("fallback=") {
            boot.fallback = Slot::named(letter);
        } else if let Some(number) = word.strip_prefix("trial=") {
            boot.trial = try_number(number);
        }
    }
    // A fallback is to the slot that runs, from the other.
    if boot.fallback.is_some() && boot.fallback == boot.slot {
        boot.fallback = None;
    }
    boot
}

/// The try `text` numbers, as the menu writes it: 1 to the highest, in
/// digits with no leading 0.
fn try_number(text: &str) -> Option<u8> {
    let number = text.parse::<u8>().ok().filter(|&n| n != 0 && n <= MAX_NUMBER)?;
    (alloc::format!("{number}") == text).then_some(number)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_slot_and_the_fallback_before_a_lone_dashes() {
        let boot = parse("root=UUID=x slot=a fallback=b panic=10 quiet -- slot=b");
        assert_eq!(boot, Boot { slot: Some(Slot::A), fallback: Some(Slot::B), trial: None });
        assert_eq!(boot.system(), Slot::A);
        assert_eq!(parse("slot=b slot=a").slot, Some(Slot::A));
        assert_eq!(parse("root=UUID=x panic=10").system(), Slot::A);
        assert_eq!(parse("slot=c").slot, None);
        assert_eq!(parse("slot=b fallback=b").fallback, None);
        assert_eq!(parse("-- slot=b").slot, None);
    }

    #[test]
    fn reads_a_try_as_the_menu_numbers_it() {
        let boot = parse("trial=4 root=UUID=x slot=b panic=10 quiet");
        assert_eq!(boot, Boot { slot: Some(Slot::B), fallback: None, trial: Some(4) });
        assert_eq!(parse("trial=99").trial, Some(99));
        for odd in ["trial=0", "trial=100", "trial=04", "trial=+4", "trial=", "trial=x"] {
            assert_eq!(parse(odd).trial, None, "{odd}");
        }
        assert_eq!(parse("trial=2 trial=3").trial, Some(3));
        assert_eq!(parse("-- trial=2").trial, None);
    }
}
