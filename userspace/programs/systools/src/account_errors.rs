// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The refusals of user(8) and passwd(1) that have an ID of their own,
//! ACC-101, ACC-103, ACC-104 (a key, a session) and ACC-105, each as the
//! failure a command exits with. The ID ends the line in parentheses, as the other tools' IDs do
//! (`NET-181`): the web console reads it from the last line of the
//! command's error output when its action names it, and the support
//! center's page for it is found by it.
//!
//! ACC-102, that a change would leave no administrator, and ACC-104 for a
//! token that an account does not have, are authd's: it refuses `Disable`,
//! `Remove`, `Set` and `TokenRemoveOf` with the words it gives, which the
//! command prints as they are.
//!
//! The words do not start with the command's name: `main` puts it before the
//! message.

use crate::util::{Fail, fail};

/// ACC-101: a `user` subcommand, or `passwd`, for a name that no account
/// has.
pub fn no_such_account(name: &str) -> Fail {
    fail(format!("there is no account called {name}; `user list` shows the accounts (ACC-101)"))
}

/// ACC-103: `user key add` for a line that is not a public key that Dukovo
/// takes. `why` says what is wrong with it.
pub fn key_refused(why: &str) -> Fail {
    fail(format!(
        "{why}: Dukovo takes Ed25519 and ECDSA (P-256 and P-384) keys, as one line the way \
         ssh-keygen writes it, with no options before the key (ACC-103)"
    ))
}

/// ACC-104: `user key remove` for a fingerprint that none of `name`'s keys
/// has.
pub fn no_such_key(name: &str, fingerprint: &str) -> Fail {
    fail(format!(
        "{name} has no key with the fingerprint {fingerprint}; `user show {name}` lists their keys \
         (ACC-104)"
    ))
}

/// ACC-104 too: `user end` for a session that `name` does not have, or
/// whose number is now another process's.
pub fn no_such_session(name: &str, session: &str) -> Fail {
    fail(format!(
        "{name} has no session {session}; `user show {name}` lists their sessions (ACC-104)"
    ))
}

/// ACC-105: `user add` or `user rename` for a name that an account has
/// already.
pub fn name_taken(name: &str) -> Fail {
    fail(format!("there is an account called {name} already: choose another name (ACC-105)"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_refusal_ends_with_its_id_and_exits_with_status_1() {
        let refusals = [
            (no_such_account("ann"), "ACC-101"),
            (key_refused("that is not a public key"), "ACC-103"),
            (no_such_key("ann", "SHA256:abc"), "ACC-104"),
            (no_such_session("ann", "41-1763110000"), "ACC-104"),
            (name_taken("ann"), "ACC-105"),
        ];
        for (refusal, id) in refusals {
            assert!(refusal.message.ends_with(&format!(" ({id})")), "{}", refusal.message);
            assert_eq!(refusal.message.lines().count(), 1, "{id}");
            assert_eq!((refusal.code, refusal.usage), (1, false), "{id}");
        }
    }

    #[test]
    fn the_words_name_what_was_asked_for_and_what_to_run() {
        assert!(no_such_account("ann").message.starts_with("there is no account called ann"));
        assert!(no_such_account("ann").message.contains("`user list`"));
        let key = no_such_key("ann", "SHA256:abc").message;
        assert!(key.starts_with("ann has no key with the fingerprint SHA256:abc"));
        assert!(key.contains("`user show ann`"));
        assert!(no_such_session("ann", "41-7").message.contains("no session 41-7"));
        assert!(name_taken("ann").message.starts_with("there is an account called ann already"));
        let refused = key_refused("keys of the kind ssh-dss are not taken").message;
        assert!(
            refused.starts_with("keys of the kind ssh-dss are not taken: Dukovo takes Ed25519")
        );
    }
}
