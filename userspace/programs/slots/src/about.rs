// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! What a slot says of itself, in `startup/slot.env` beside its kernel and
//! startup archive (ADR 0451): its version, which the boot menu names it
//! by, and the kernel's flags the slot always starts with, which are
//! none in a release and a deliberate fault in the boot tests' broken
//! updates. It is a GRUB environment block ([`crate::envblock`]), so that
//! the menu reads it with `load_env`, and part of the signed release like
//! every other file of the slot.

use alloc::string::String;

use crate::envblock::{self, BLOCK};

/// The file's path in the slot.
pub const PATH: &str = "startup/slot.env";

/// The variables the file holds, which the menu loads by name.
pub const VARIABLES: [&str; 2] = ["slot_version", "slot_flags"];

/// What a slot says of itself.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct About {
    /// The release's version, such as `0.15.0`.
    pub version: String,
    /// Kernel flags the slot's menu entries add, before the machine's
    /// own: empty in every release.
    pub flags: String,
}

impl About {
    /// The file's block.
    ///
    /// # Panics
    ///
    /// If the version and flags do not fit a block, as no release's do:
    /// the build checks what it writes.
    #[must_use]
    pub fn encode(&self) -> [u8; BLOCK] {
        envblock::write(
            &["# What this slot is (ADR 0451): its version, read by the boot menu."],
            &[(VARIABLES[0], &self.version), (VARIABLES[1], &self.flags)],
        )
        .expect("a slot's version and flags fit a block")
    }

    /// What the block `bytes` says, if it is one with a version.
    #[must_use]
    pub fn decode(bytes: &[u8]) -> Option<About> {
        let variables = envblock::parse(bytes)?;
        let version = envblock::get(&variables, VARIABLES[0]).filter(|v| !v.is_empty())?;
        let flags = envblock::get(&variables, VARIABLES[1]).unwrap_or("");
        Some(About { version: String::from(version), flags: String::from(flags) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_back_as_written() {
        let about = About { version: "0.15.0".into(), flags: String::new() };
        assert_eq!(About::decode(&about.encode()), Some(about));
        let broken = About { version: "0.15.0+panic".into(), flags: "selftest=page-fault".into() };
        assert_eq!(About::decode(&broken.encode()), Some(broken));
        assert_eq!(About::decode(&envblock::write(&[], &[("slot_flags", "x")]).unwrap()), None);
    }
}
