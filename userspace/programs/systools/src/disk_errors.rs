// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The refusals of storage(8), diskhealth(8) and scrub(8) that have an ID
//! of their own, DSK-101 and DSK-102, each as the failure a command exits
//! with. The ID ends the line in parentheses, as the other tools' IDs do
//! (`NET-181`): the web console reads it from the last line of the
//! command's error output when its action names it, and the support
//! center's page for it is found by it.
//!
//! The words do not start with the command's name: `main` puts it before the
//! message.

use crate::util::{Fail, fail};

/// DSK-101: `storage grow` for a name that `/kernel/block` does not list
/// as a disk, and `diskhealth` for one it does not list at all.
pub fn no_such_disk(disk: &str) -> Fail {
    fail(format!("there is no disk called {disk}; `storage show` lists the disks (DSK-101)"))
}

/// DSK-102: `scrub start` for a filesystem whose scrub is running. Two
/// scrubs of one filesystem would read every block twice and report the
/// same errors twice, and the kept result is one a filesystem.
pub fn being_checked(path: &str) -> Fail {
    fail(format!(
        "every block of {path} is being checked already: wait for that scrub to end (DSK-102)"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_refusal_ends_with_its_id_and_exits_with_status_1() {
        for (refusal, id) in
            [(no_such_disk("sdz"), "DSK-101"), (being_checked("/volumes/data"), "DSK-102")]
        {
            assert!(refusal.message.ends_with(&format!(" ({id})")), "{}", refusal.message);
            assert_eq!(refusal.message.lines().count(), 1, "{id}");
            assert_eq!((refusal.code, refusal.usage), (1, false), "{id}");
        }
    }

    #[test]
    fn the_words_name_what_was_asked_for_and_what_to_run() {
        let disk = no_such_disk("sdz").message;
        assert!(disk.starts_with("there is no disk called sdz"), "{disk}");
        assert!(disk.contains("storage show"), "{disk}");
        let scrub = being_checked("/volumes/data").message;
        assert!(scrub.starts_with("every block of /volumes/data"), "{scrub}");
    }
}
