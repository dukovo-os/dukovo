// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The installed system's accounts as Rescue reads them (ADR 0540). When
//! Rescue turns SSH on, authd(8) and sshd(8) run in Rescue's own live
//! system, but sign people in by the installed system's accounts, keys,
//! host key and sshd_config(5), which they read and never write.
//!
//! dukovo-rescue(8) links the root it mounted at [`ROOT`], in a folder
//! only root may write: a link in `/tmp`, which anyone may write, could
//! name accounts of someone else's making. authd, which holds the whole
//! tree, reads the root there ([`installed`]). sshd sees only its own
//! folders, and none of the root: dukovo-rescue copies the few files it
//! reads, [`SSHD_FILES`], into its folder ([`SSHD_COPIES`], [`for_sshd`]),
//! so that the program that faces the network never sees the password
//! hashes or a home. And only a system the boot menu started as Rescue,
//! with `rescue=` on the kernel's command line, reads either at all.

use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// Where dukovo-rescue(8) links the root it mounted.
pub const ROOT: &str = "/system/live/rescue/root";

/// Where dukovo-rescue(8) copies [`SSHD_FILES`] from the root, in sshd's
/// own folder, laid out as the root's subvolumes are, in Rescue's memory.
pub const SSHD_COPIES: &str = "/system/state/sshd/rescue";

/// The installed system's files sshd reads: its settings, the account
/// lists its AllowGroups and DenyGroups check, and its host key.
pub const SSHD_FILES: [&str; 5] = [
    crate::sshd_config::PATH,
    crate::PASSWD,
    crate::GROUP,
    crate::sshd_config::HOST_KEY,
    crate::sshd_config::HOST_KEY_PUBLIC,
];

/// The installed system's places, and the root's subvolumes that hold
/// them.
const PLACES: [(&str, &str); 3] =
    [("/system/settings/", "@settings/"), ("/system/state/", "@state/"), ("/users/", "@users/")];

/// The kernel's command line.
const CMDLINE: &str = "/kernel/cmdline";

/// Where nothing is: what a read in Rescue finds when there is no root.
const NOWHERE: &str = "/system/live/rescue/no-root";

/// Whether this is Rescue: the boot menu starts it with `rescue=` and
/// the root's UUID among the kernel's flags, those before `--`.
pub fn is_rescue() -> bool {
    static RESCUE: OnceLock<bool> = OnceLock::new();
    *RESCUE.get_or_init(|| std::fs::read_to_string(CMDLINE).is_ok_and(|line| rescue_in(&line)))
}

/// Whether the kernel's command line `line` starts Rescue.
fn rescue_in(line: &str) -> bool {
    line.split_whitespace().take_while(|word| *word != "--").any(|w| w.starts_with("rescue="))
}

/// The root Rescue mounted, as [`ROOT`] names it: in Rescue, once it is
/// linked there by root. `None` elsewhere, or before.
pub fn root() -> Option<&'static Path> {
    let linked = std::fs::symlink_metadata(ROOT).is_ok_and(|link| link.uid() == 0);
    (is_rescue() && linked).then_some(Path::new(ROOT))
}

/// Where `path`, a file of the installed system's, is in the root at
/// `root`, or in a copy laid out as it is: its settings, state and users'
/// homes are subvolumes of it. `None` for a path in none of them.
pub fn place(root: &Path, path: &str) -> Option<PathBuf> {
    PLACES.iter().find_map(|(at, subvolume)| {
        let rest = path.strip_prefix(at).filter(|rest| !rest.split('/').any(|p| p == ".."))?;
        Some(root.join(subvolume).join(rest))
    })
}

/// `path`, a file of the installed system's, as authd reads it: in
/// Rescue, in the root it mounted, and nowhere if it is in none of the
/// root's subvolumes or Rescue mounted none; elsewhere, as it is.
pub fn installed(path: &str) -> PathBuf {
    if !is_rescue() {
        return PathBuf::from(path);
    }
    // Nothing there: a read finds nothing, rather than Rescue's own.
    root().and_then(|root| place(root, path)).unwrap_or_else(|| PathBuf::from(NOWHERE))
}

/// `path`, one of [`SSHD_FILES`], as sshd reads it: in Rescue, the copy
/// dukovo-rescue made, which is not there unless SSH is on; elsewhere,
/// as it is.
pub fn for_sshd(path: &str) -> PathBuf {
    if !is_rescue() {
        return PathBuf::from(path);
    }
    place(Path::new(SSHD_COPIES), path).unwrap_or_else(|| PathBuf::from(NOWHERE))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_kernel_flag_starts_rescue() {
        assert!(rescue_in("quiet rescue=1b2c-33 console=ttyS0"));
        assert!(!rescue_in("quiet console=ttyS0"));
        // After `--`, a word is the first program's, not the kernel's.
        assert!(!rescue_in("quiet -- rescue=1b2c-33"));
        assert!(!rescue_in("norescue=1"));
    }

    #[test]
    fn the_installed_systems_files_are_in_the_roots_subvolumes() {
        let root = Path::new("/system/live/rescue/root");
        let placed = |path| place(root, path).map(|p| p.display().to_string());
        assert_eq!(
            placed("/system/settings/passwd").as_deref(),
            Some("/system/live/rescue/root/@settings/passwd")
        );
        assert_eq!(
            placed("/system/state/authd/passwords").as_deref(),
            Some("/system/live/rescue/root/@state/authd/passwords")
        );
        assert_eq!(
            placed("/users/alice/.ssh/authorized_keys").as_deref(),
            Some("/system/live/rescue/root/@users/alice/.ssh/authorized_keys")
        );
        // Nothing outside them, and no way out of one.
        assert_eq!(placed("/system/commands/sh"), None);
        assert_eq!(placed("/system/settings/../commands/sh"), None);
        assert_eq!(placed("/users/alice/../../etc"), None);
    }

    #[test]
    fn sshd_s_copies_are_in_its_own_folder() {
        // Each file sshd reads has its place among the copies, and the
        // copies stay in the folder Spark gives sshd.
        for path in SSHD_FILES {
            let copy = place(Path::new(SSHD_COPIES), path).expect(path);
            assert!(copy.starts_with("/system/state/sshd/"), "{}", copy.display());
        }
    }
}
