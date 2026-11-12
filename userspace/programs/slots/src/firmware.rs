// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The firmware set a slot starts with (ADR 0468), in
//! `startup/firmware.env` beside the slot's kernel and startup archive.
//! The optional firmware package stays outside the system (ADR 0007):
//! its files are kept once each, by SHA-256, in the root's `@firmware`
//! subvolume, and a slot only names the set it starts with, and the
//! files of it that GRUB loads. The file is written when the slot is
//! staged, after the slot is read back against its signed list and
//! before it is sealed, so that a slot and the firmware it starts with
//! are tried, proved and fallen back from together. A slot without the
//! file starts with no firmware set. It is a GRUB environment block
//! ([`crate::envblock`]), so that the menu reads it with `load_env`.

use alloc::string::String;

use crate::envblock::{self, BLOCK};

/// The file's path in the slot.
pub const PATH: &str = "startup/firmware.env";

/// The root's subvolume that holds every firmware set a slot names:
/// `files/<sha256>`, each file once, and `sets/<version>.json`, each
/// set's signed list.
pub const STORE: &str = "@firmware";

/// The variables the file holds, which the menu loads by name.
pub const VARIABLES: [&str; 2] = ["firmware_set", "firmware_microcode"];

/// The firmware set a slot names.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Named {
    /// The set's version, such as `2026.11.12`.
    pub set: String,
    /// The SHA-256 of its CPU microcode file, which GRUB loads from the
    /// store, if it has one.
    pub microcode: Option<String>,
}

impl Named {
    /// The file's block.
    ///
    /// # Panics
    ///
    /// If the version does not fit a block, as no set's does: sysupdate
    /// checks the version before it writes it.
    #[must_use]
    pub fn encode(&self) -> [u8; BLOCK] {
        envblock::write(
            &["# The firmware set this slot starts with (ADR 0468), read by the boot menu."],
            &[(VARIABLES[0], &self.set), (VARIABLES[1], self.microcode.as_deref().unwrap_or(""))],
        )
        .expect("a firmware set's name fits a block")
    }

    /// What the block `bytes` says, if it is one that names a set and,
    /// for microcode, a SHA-256.
    #[must_use]
    pub fn decode(bytes: &[u8]) -> Option<Named> {
        let variables = envblock::parse(bytes)?;
        let set = envblock::get(&variables, VARIABLES[0]).filter(|v| !v.is_empty())?;
        let microcode = match envblock::get(&variables, VARIABLES[1]).unwrap_or("") {
            "" => None,
            sha256 if is_sha256(sha256) => Some(String::from(sha256)),
            _ => return None,
        };
        Some(Named { set: String::from(set), microcode })
    }
}

/// Whether `text` is a SHA-256 in lower-case hex, as the store names its
/// files.
fn is_sha256(text: &str) -> bool {
    text.len() == 64 && text.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_back_as_written() {
        let sha256 = "ab".repeat(32);
        let named = Named { set: "2026.11.12".into(), microcode: Some(sha256.clone()) };
        assert_eq!(Named::decode(&named.encode()), Some(named));
        let none = Named { set: "2026.11.12".into(), microcode: None };
        assert_eq!(Named::decode(&none.encode()), Some(none));
        // A set must be named, and the microcode be a SHA-256, which the
        // menu puts in a path.
        assert_eq!(Named::decode(&envblock::write(&[], &[("firmware_set", "")]).unwrap()), None);
        let path = envblock::write(&[], &[("firmware_set", "1"), ("firmware_microcode", "../x")]);
        assert_eq!(Named::decode(&path.unwrap()), None);
        let upper = envblock::write(
            &[],
            &[("firmware_set", "1"), ("firmware_microcode", &"AB".repeat(32))],
        );
        assert_eq!(Named::decode(&upper.unwrap()), None);
    }
}
