// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Filehandles: what a client holds a file by. Each names the file's
//! share, its device and its inode number, so it lasts as long as the
//! file does, through renames and nfsd's restarts (FH4_PERSISTENT). nfsd
//! keeps where it saw each file, its folder's and its own name, to reach
//! it by; a handle it has not seen since it started is looked for below
//! its share.

use std::collections::HashMap;

/// The version of the handles' layout, their first byte.
const VERSION: u8 = 1;

/// What a filehandle names.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Fh {
    /// The root clients mount from, which holds a folder for each share
    /// they may reach.
    Root,
    /// A share's folder, by its key.
    Share(u64),
    /// A file in a share: the share's key, the file's device and inode
    /// number.
    File(u64, u64, u64),
}

impl Fh {
    /// Its bytes, as a client holds them.
    pub fn encode(&self) -> Vec<u8> {
        let mut out = vec![VERSION];
        match *self {
            Fh::Root => out.push(0),
            Fh::Share(key) => {
                out.push(1);
                out.extend_from_slice(&key.to_be_bytes());
            }
            Fh::File(key, dev, ino) => {
                out.push(2);
                for n in [key, dev, ino] {
                    out.extend_from_slice(&n.to_be_bytes());
                }
            }
        }
        out
    }

    /// The filehandle of `bytes`, if they are one of nfsd's.
    pub fn decode(bytes: &[u8]) -> Option<Fh> {
        let n = |at: usize| -> Option<u64> {
            Some(u64::from_be_bytes(bytes.get(at..at + 8)?.try_into().ok()?))
        };
        match bytes {
            [VERSION, 0] => Some(Fh::Root),
            [VERSION, 1, ..] if bytes.len() == 10 => Some(Fh::Share(n(2)?)),
            [VERSION, 2, ..] if bytes.len() == 26 => Some(Fh::File(n(2)?, n(10)?, n(18)?)),
            _ => None,
        }
    }
}

/// Where a file was seen: in its folder, by its device and inode number,
/// or in its share's folder for none; under its name. A name found by a
/// search may have several parts.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Seen {
    /// Its folder, none for the share's own.
    folder: Option<(u64, u64)>,
    /// Its name there.
    name: Vec<u8>,
}

/// The most files remembered: past it, nfsd forgets them all, and looks
/// for those it is asked for again.
const SEEN_MAX: usize = 1 << 20;

/// The most folders a file's path goes through.
const DEPTH_MAX: usize = 2048;

/// Where nfsd saw each file, by its share, device and inode number.
#[derive(Default)]
pub struct Places {
    seen: HashMap<(u64, u64, u64), Seen>,
}

impl Places {
    /// Remembers that the file `dev`, `ino` of share `key` is `name` in
    /// the folder `folder`, none for the share's own.
    pub fn learn(&mut self, key: u64, folder: Option<(u64, u64)>, name: &[u8], dev: u64, ino: u64) {
        if self.seen.len() >= SEEN_MAX && !self.seen.contains_key(&(key, dev, ino)) {
            self.seen.clear();
        }
        // A folder is never its own place.
        if folder == Some((dev, ino)) {
            return;
        }
        self.seen.insert((key, dev, ino), Seen { folder, name: name.to_vec() });
    }

    /// The file's path below its share's folder, if nfsd saw it and each
    /// folder it is in.
    pub fn path(&self, key: u64, dev: u64, ino: u64) -> Option<Vec<u8>> {
        let mut names = Vec::new();
        let mut at = (dev, ino);
        loop {
            let seen = self.seen.get(&(key, at.0, at.1))?;
            names.push(seen.name.as_slice());
            match seen.folder {
                None => break,
                Some(folder) => at = folder,
            }
            if names.len() > DEPTH_MAX {
                return None;
            }
        }
        names.reverse();
        Some(names.join(&b'/'))
    }

    /// Forgets the file.
    pub fn forget(&mut self, key: u64, dev: u64, ino: u64) {
        self.seen.remove(&(key, dev, ino));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handles_go_both_ways_and_others_are_not_taken() {
        for fh in [Fh::Root, Fh::Share(7), Fh::File(7, 8, u64::MAX)] {
            assert_eq!(Fh::decode(&fh.encode()), Some(fh));
        }
        assert_eq!(Fh::decode(&[]), None);
        assert_eq!(Fh::decode(&[2, 0]), None);
        assert_eq!(Fh::decode(&[1, 2, 0, 0]), None);
        assert_eq!(Fh::decode(&[1, 0, 0]), None);
    }

    #[test]
    fn paths_follow_their_folders() {
        let mut places = Places::default();
        places.learn(1, None, b"a", 9, 10);
        places.learn(1, Some((9, 10)), b"b", 9, 11);
        places.learn(1, Some((9, 11)), b"c.txt", 9, 12);
        assert_eq!(places.path(1, 9, 12), Some(b"a/b/c.txt".to_vec()));
        // Renaming a folder moves what is in it.
        places.learn(1, None, b"z", 9, 10);
        assert_eq!(places.path(1, 9, 12), Some(b"z/b/c.txt".to_vec()));
        // Another share's file, and one whose folder is forgotten.
        assert_eq!(places.path(2, 9, 12), None);
        places.forget(1, 9, 11);
        assert_eq!(places.path(1, 9, 12), None);
        // A loop is not followed for ever.
        places.learn(1, Some((9, 21)), b"x", 9, 20);
        places.learn(1, Some((9, 20)), b"y", 9, 21);
        assert_eq!(places.path(1, 9, 20), None);
    }
}
