// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! What the kernel's command line says of the slots, as the boot menu
//! writes it (dukovo-boot(7)): `slot=a` or `slot=b`, the slot GRUB
//! started, whose subvolume Spark mounts on `/system`; and
//! `fallback=a` or `fallback=b`, the slot of an update that ran out of
//! tries, which the default starts with, so that Spark puts the settings
//! from before it back before any service starts (ADR 0454). Only the
//! words before a lone `--` are the kernel's; a flag's last value counts,
//! as the kernel takes it.

use crate::state::Slot;

/// What the command line says of the slots.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Boot {
    /// The slot GRUB started, or none, as a menu from before Phase 15
    /// starts `@system.a`.
    pub slot: Option<Slot>,
    /// The slot of an update that ran out of tries.
    pub fallback: Option<Slot>,
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
        }
    }
    // A fallback is to the slot that runs, from the other.
    if boot.fallback.is_some() && boot.fallback == boot.slot {
        boot.fallback = None;
    }
    boot
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_slot_and_the_fallback_before_a_lone_dashes() {
        let boot = parse("root=UUID=x slot=a fallback=b panic=10 quiet -- slot=b");
        assert_eq!(boot, Boot { slot: Some(Slot::A), fallback: Some(Slot::B) });
        assert_eq!(boot.system(), Slot::A);
        assert_eq!(parse("slot=b slot=a").slot, Some(Slot::A));
        assert_eq!(parse("root=UUID=x panic=10").system(), Slot::A);
        assert_eq!(parse("slot=c").slot, None);
        assert_eq!(parse("slot=b fallback=b").fallback, None);
        assert_eq!(parse("-- slot=b").slot, None);
    }
}
