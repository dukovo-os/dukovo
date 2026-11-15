// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The refusals of snap(1) and backup(8) that have an ID of their own,
//! BAK-101 to BAK-107, each as the failure a command exits with. The ID
//! ends the line in parentheses, as the other tools' IDs do (`NET-181`):
//! the web console reads it from the last line of the command's error
//! output when its action names it, and the support center's page for it
//! is found by it.
//!
//! The words do not start with the command's name: `main` puts it before the
//! message.

use crate::util::{Fail, fail};

/// BAK-101: `backup show`, `run`, `verify`, `key`, `remove` or `restore`
/// for a name that no backup place has.
pub fn no_such_place(name: &str) -> Fail {
    fail(format!(
        "there is no backup place called {name}; `backup list` shows the places (BAK-101)"
    ))
}

/// BAK-102: `backup run` while another round holds the round's lock.
pub fn round_running() -> Fail {
    fail("another backup is running: wait for it to end, then try again (BAK-102)".into())
}

/// BAK-103: `backup restore` with a key that does not decrypt the
/// backup's record.
pub fn wrong_key() -> Fail {
    fail(
        "this key does not open these backups: give the recovery key of the machine that made \
         them (BAK-103)"
            .into(),
    )
}

/// BAK-104: `backup restore --machine` while a restore of a whole machine
/// is staged for the next start. A second would replace the first, which
/// the person has not seen carried out.
pub fn restore_staged() -> Fail {
    fail(
        "a restore of a whole machine is staged already: restart to carry it out, or end it \
         with `backup restore --machine --cancel` (BAK-104)"
            .into(),
    )
}

/// BAK-105: `snap browse` or `snap restore --from` for a snapshot that
/// `place` does not have.
pub fn no_such_snapshot(snapshot: &str, place: &str) -> Fail {
    fail(format!(
        "there is no snapshot called {snapshot} of {place}; `snap list {place}` shows its \
         snapshots (BAK-105)"
    ))
}

/// BAK-106: `snap restore --beside` for a file whose copy would have a
/// name that is taken. A restore never writes over a file.
pub fn already_there(path: &str) -> Fail {
    fail(format!("{path} is there already: restore with another label, or move it away (BAK-106)"))
}

/// BAK-107: `backup restore --machine` of a record that a later release
/// wrote, in a `format` this release does not know.
pub fn newer_backups(format: u64) -> Fail {
    fail(format!(
        "these backups were made by a newer Dukovo, in record format {format}: update this \
         machine, then restore (BAK-107)"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_refusal_ends_with_its_id_and_exits_with_status_1() {
        let refusals = [
            (no_such_place("lab"), "BAK-101"),
            (round_running(), "BAK-102"),
            (wrong_key(), "BAK-103"),
            (restore_staged(), "BAK-104"),
            (no_such_snapshot("lab-before", "/users"), "BAK-105"),
            (already_there("/users/ann/notes (lab).txt"), "BAK-106"),
            (newer_backups(3), "BAK-107"),
        ];
        for (refusal, id) in refusals {
            assert!(refusal.message.ends_with(&format!(" ({id})")), "{}", refusal.message);
            assert_eq!(refusal.message.lines().count(), 1, "{id}");
            assert_eq!((refusal.code, refusal.usage), (1, false), "{id}");
        }
    }

    #[test]
    fn the_words_name_what_was_asked_for_and_what_to_run() {
        assert!(no_such_place("lab").message.starts_with("there is no backup place called lab"));
        assert!(no_such_place("lab").message.contains("backup list"));
        assert!(restore_staged().message.contains("backup restore --machine --cancel"));
        let snapshot = no_such_snapshot("lab-before", "/users").message;
        assert!(snapshot.starts_with("there is no snapshot called lab-before of /users"));
        assert!(snapshot.contains("`snap list /users`"));
        assert!(already_there("/users/ann/a.txt").message.starts_with("/users/ann/a.txt is"));
        assert!(newer_backups(3).message.contains("record format 3"));
    }
}
