// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Users and groups between Dukovo and the server: each call names its
//! caller's other groups, as this machine's group file lists them, and an
//! owner the server names `name@domain` is this machine's account of that
//! name. AUTH_SYS's servers, Linux's among them, name owners by number,
//! which are taken as they are.

use std::time::{Duration, Instant};

use accounts::{Group, Table, User};

use crate::rpc::{Cred, GROUPS_MAX};

/// The number of the user no one is, Linux's `nobody`: whom a name with
/// no account here is shown as.
pub const NOBODY: u32 = 65_534;

/// How long the files are kept before they are read again.
const FRESH: Duration = Duration::from_secs(30);

/// The users and groups, as last read.
pub struct Ids {
    users: Table<User>,
    groups: Table<Group>,
    read: Option<Instant>,
}

impl Ids {
    /// None yet: they are read when first asked about.
    pub fn new() -> Ids {
        Ids { users: accounts::users(""), groups: accounts::groups(""), read: None }
    }

    /// Reads the files again if they were read long ago.
    fn fresh(&mut self) {
        if self.read.is_some_and(|read| read.elapsed() < FRESH) {
            return;
        }
        let text = |path| std::fs::read_to_string(path).unwrap_or_default();
        self.users = accounts::users(&text(accounts::PASSWD));
        self.groups = accounts::groups(&text(accounts::GROUP));
        self.read = Some(Instant::now());
    }

    /// The credential of `uid` in group `gid`, with the other groups the
    /// group file lists them in.
    pub fn cred(&mut self, uid: u32, gid: u32) -> Cred {
        self.fresh();
        let gids = match self.users.entries().find(|user| user.uid == uid) {
            Some(user) => accounts::other_groups(user, &self.groups, GROUPS_MAX),
            None => Vec::new(),
        };
        Cred { uid, gid, gids }
    }

    /// The number of the owner the server names `owner`.
    pub fn uid(&mut self, owner: &[u8]) -> u32 {
        number(owner).unwrap_or_else(|| {
            self.fresh();
            let name = local(owner);
            self.users.entries().find(|user| user.name == name).map_or(NOBODY, |user| user.uid)
        })
    }

    /// The number of the group the server names `group`.
    pub fn gid(&mut self, group: &[u8]) -> u32 {
        number(group).unwrap_or_else(|| {
            self.fresh();
            let name = local(group);
            self.groups.entries().find(|g| g.name == name).map_or(NOBODY, |g| g.gid)
        })
    }
}

/// An owner named by number, as AUTH_SYS's servers name them.
fn number(name: &[u8]) -> Option<u32> {
    std::str::from_utf8(name).ok()?.parse().ok()
}

/// A name without its `@domain`.
fn local(name: &[u8]) -> &str {
    let text = std::str::from_utf8(name).unwrap_or("");
    text.split_once('@').map_or(text, |(name, _)| name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn owners_by_number_and_by_name() {
        let mut ids = Ids::new();
        ids.users = accounts::users("alice:x:1000:1000::/users/alice:/bin/sh\n");
        ids.groups = accounts::groups("alice:x:1000:\nmusic:x:1001:alice\nadmins:x:27:bob\n");
        ids.read = Some(Instant::now());
        assert_eq!(ids.uid(b"1000"), 1000);
        assert_eq!(ids.uid(b"alice@example.org"), 1000);
        assert_eq!(ids.uid(b"carol@example.org"), NOBODY);
        assert_eq!(ids.gid(b"music@example.org"), 1001);
        assert_eq!(ids.cred(1000, 1000), Cred { uid: 1000, gid: 1000, gids: vec![1001] });
        assert_eq!(ids.cred(1002, 100).gids, Vec::<u32>::new());
    }
}
