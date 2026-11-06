// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Each person's keyring (ADR 0393): the secrets authd keeps for them,
//! such as git's tokens and their SSH keys' passphrases.
//!
//! A keyring is one file, `/system/state/authd/keyrings/NUMBER`, named by
//! the account's number, in authd's own folder, which no other program's
//! view holds. Its entries are sealed together with ChaCha20-Poly1305
//! under a random key of the keyring's own, and that key is sealed under
//! a key made from the person's password with Argon2id, with a salt of its
//! own, at the cost their password's hash has:
//!
//! ```text
//! dukovo-keyring 1
//! key m=65536,t=3,p=4 SALT NONCE SEALED-KEY
//! entries NONCE SEALED-ENTRIES
//! ```
//!
//! each field after the cost in base64. The file is no use without the
//! password. Changing the password seals the same key again and leaves
//! the entries as they were. No one else holds the key, so a password
//! forgotten and reset loses the keyring, and authd says so.
//!
//! authd opens a person's keyring when they sign in with their password,
//! at a console or over SSH, or ask with `keyring unlock`, and makes it
//! then if they have none. It holds it open, in its own memory, while one
//! of their sessions runs, and closes it, wiping the key, when the last
//! ends. Each change writes the whole file again, sealed with a new nonce.
//!
//! Where to start when it misbehaves: authd's log says `authd: keyring of
//! NAME: ...` when a keyring does not open or cannot be written, and
//! when one opens and closes; `keyring list` says whether it is open. The
//! `keyring` boot test covers it.

use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::io;
use std::time::Instant;

use accounts::User;
use accounts::hash::{self, Cost};
use accounts::request::{self, Answer, Request};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD_NO_PAD as BASE64;
use ring::aead::{Aad, CHACHA20_POLY1305, LessSafeKey, NONCE_LEN, Nonce, UnboundKey};
use ring::rand::{SecureRandom, SystemRandom};

use super::{Authd, Files, Peer, replace};
use crate::util;

/// The start of the names of Kerberos's entries, which kinit(1) keeps
/// (ADR 0397).
const KERBEROS_PREFIX: &str = "krb5 ";

/// The folder of the keyrings' files, inside authd's own.
pub const FOLDER: &str = "/system/state/authd/keyrings";

/// A keyring file's first line, which says its format.
const FORMAT: &str = "dukovo-keyring 1";

/// The length of a key, and of the salt the password's key is made with,
/// as RFC 9106 recommends.
const KEY: usize = 32;
const SALT: usize = 16;

/// The most entries a keyring holds.
const ENTRIES_MAX: usize = 1024;

/// What a keyring request is refused with while the keyring is closed.
const CLOSED: &str = "your keyring is locked: it opens when you sign in with your password, or \
                      with `keyring unlock`";

/// Where `owner`'s keyring is kept.
pub fn path(owner: u32) -> String {
    format!("{FOLDER}/{owner}")
}

/// A key, wiped when dropped.
struct Key([u8; KEY]);

impl Drop for Key {
    fn drop(&mut self) {
        self.0.fill(0);
        std::hint::black_box(&self.0);
    }
}

/// A person's keyring, open.
pub struct Keyring {
    /// The number of the account whose it is, which its file is named by
    /// and which each sealed part is bound to.
    owner: u32,
    key: Key,
    /// The `key` line's fields: the keyring's key, sealed under the
    /// password's.
    sealed_key: String,
    /// The secrets, by their entries' names.
    entries: BTreeMap<String, Vec<u8>>,
}

impl Drop for Keyring {
    fn drop(&mut self) {
        for secret in self.entries.values_mut() {
            accounts::wipe(secret);
        }
    }
}

/// Why a keyring did not open.
#[derive(Debug, PartialEq, Eq)]
pub enum Unopened {
    /// The password is not the one it was sealed under, or its key line
    /// was changed: the two cannot be told apart.
    WrongPassword,
    /// Its file is not one authd wrote, or was changed.
    Broken(String),
}

/// `N` random bytes.
fn random<const N: usize>() -> Result<[u8; N], String> {
    let mut bytes = [0u8; N];
    SystemRandom::new()
        .fill(&mut bytes)
        .map_err(|_| "the kernel gave no random bytes".to_string())?;
    Ok(bytes)
}

/// The key made from `password` with `salt`, at `cost`.
fn password_key(password: &[u8], salt: &[u8], cost: Cost) -> Result<Key, String> {
    let mut key = Key([0; KEY]);
    hash::derive(password, salt, cost, &mut key.0)?;
    Ok(key)
}

/// What a sealed part is bound to: the format, the part, and its owner,
/// so that one moved to another line, or into another person's file, does
/// not open.
fn bound_to(part: &str, owner: u32) -> String {
    format!("{FORMAT} {part} {owner}")
}

/// ChaCha20-Poly1305 with `key`.
fn cipher(key: &Key) -> LessSafeKey {
    // A key of the length the algorithm takes is never refused.
    LessSafeKey::new(UnboundKey::new(&CHACHA20_POLY1305, &key.0).expect("a 32-byte key"))
}

/// `plain`, sealed as `part` of `owner`'s keyring with `key` and a new
/// nonce: the nonce and the sealed bytes, in base64, separated by a space.
/// A random 96-bit nonce is safe for far more writes than a keyring sees.
fn seal(key: &Key, part: &str, owner: u32, plain: &[u8]) -> Result<String, String> {
    let nonce = random::<NONCE_LEN>()?;
    let mut sealed = plain.to_vec();
    let aad = bound_to(part, owner);
    let done = cipher(key).seal_in_place_append_tag(
        Nonce::assume_unique_for_key(nonce),
        Aad::from(aad.as_bytes()),
        &mut sealed,
    );
    if done.is_err() {
        accounts::wipe(&mut sealed);
        return Err(format!("ChaCha20-Poly1305 did not seal the keyring's {part}"));
    }
    Ok(format!("{} {}", BASE64.encode(nonce), BASE64.encode(&sealed)))
}

/// What `nonce` and `sealed` hold, if `key` sealed them as `part` of
/// `owner`'s keyring.
fn unseal(key: &Key, part: &str, owner: u32, nonce: &str, sealed: &str) -> Option<Vec<u8>> {
    let nonce: [u8; NONCE_LEN] = BASE64.decode(nonce).ok()?.try_into().ok()?;
    let mut bytes = BASE64.decode(sealed).ok()?;
    let aad = bound_to(part, owner);
    let opened = cipher(key)
        .open_in_place(Nonce::assume_unique_for_key(nonce), Aad::from(aad.as_bytes()), &mut bytes)
        .map(|plain| plain.len());
    match opened {
        Ok(length) => {
            bytes.truncate(length);
            Some(bytes)
        }
        Err(_) => {
            accounts::wipe(&mut bytes);
            None
        }
    }
}

/// The `key` line's fields for `key`, sealed under `password` at `cost`
/// with a new salt.
fn seal_key(key: &Key, owner: u32, password: &[u8], cost: Cost) -> Result<String, String> {
    let salt = random::<SALT>()?;
    let sealing = password_key(password, &salt, cost)?;
    let sealed = seal(&sealing, "key", owner, &key.0)?;
    Ok(format!("{} {} {sealed}", cost.text(), BASE64.encode(salt)))
}

/// The entries as they are sealed: a line for each, its name, a tab and
/// its secret in hexadecimal. A name holds no control character, so no
/// tab or newline.
fn entries_text(entries: &BTreeMap<String, Vec<u8>>) -> Vec<u8> {
    let mut text = Vec::new();
    for (name, secret) in entries {
        text.extend_from_slice(name.as_bytes());
        text.push(b'\t');
        let mut hexed = request::hex(secret).into_bytes();
        text.extend_from_slice(&hexed);
        accounts::wipe(&mut hexed);
        text.push(b'\n');
    }
    text
}

/// The entries in `text`, as [`entries_text`] wrote them.
fn parse_entries(text: &[u8]) -> Option<BTreeMap<String, Vec<u8>>> {
    let text = std::str::from_utf8(text).ok()?;
    let mut entries = BTreeMap::new();
    for line in text.lines() {
        let (name, hexed) = line.split_once('\t')?;
        if !request::keyring_name(name) {
            return None;
        }
        entries.insert(name.to_string(), request::unhex(hexed)?);
    }
    Some(entries)
}

impl Keyring {
    /// A new, empty keyring for `owner`, sealed under `password` at
    /// `cost`.
    pub fn make(owner: u32, password: &[u8], cost: Cost) -> Result<Keyring, String> {
        let key = Key(random::<KEY>()?);
        let sealed_key = seal_key(&key, owner, password, cost)?;
        Ok(Keyring { owner, key, sealed_key, entries: BTreeMap::new() })
    }

    /// Opens `owner`'s keyring, from its file's `text`, with `password`.
    pub fn open(owner: u32, text: &str, password: &[u8]) -> Result<Keyring, Unopened> {
        let broken = |what: &str| Unopened::Broken(what.to_string());
        let mut lines = text.lines();
        if lines.next() != Some(FORMAT) {
            return Err(broken("its first line is not `dukovo-keyring 1`"));
        }
        let key_line = lines.next().and_then(|line| line.strip_prefix("key "));
        let entries_line = lines.next().and_then(|line| line.strip_prefix("entries "));
        let (Some(key_line), Some(entries_line), None) = (key_line, entries_line, lines.next())
        else {
            return Err(broken("it is not a key line and an entries line after the first"));
        };
        let [cost, salt, nonce, sealed] = key_line.split(' ').collect::<Vec<_>>()[..] else {
            return Err(broken("its key line is not a cost, a salt, a nonce and a key"));
        };
        let cost =
            Cost::from_text(cost).ok_or_else(|| broken("its cost is not one authd spends"))?;
        let salt = BASE64.decode(salt).ok().filter(|salt| salt.len() == SALT);
        let salt = salt.ok_or_else(|| broken("its salt is not 16 bytes in base64"))?;
        let unsealing = password_key(password, &salt, cost).map_err(Unopened::Broken)?;
        let mut opened =
            unseal(&unsealing, "key", owner, nonce, sealed).ok_or(Unopened::WrongPassword)?;
        let key = <[u8; KEY]>::try_from(opened.as_slice()).map(Key);
        accounts::wipe(&mut opened);
        let key = key.map_err(|_| broken("its key is not 32 bytes"))?;
        let Some((nonce, sealed)) = entries_line.split_once(' ') else {
            return Err(broken("its entries line is not a nonce and the entries"));
        };
        let mut plain = unseal(&key, "entries", owner, nonce, sealed)
            .ok_or_else(|| broken("its entries do not open with its key: the file was changed"))?;
        let entries = parse_entries(&plain);
        accounts::wipe(&mut plain);
        let entries = entries.ok_or_else(|| broken("its entries are not names and secrets"))?;
        Ok(Keyring { owner, key, sealed_key: key_line.to_string(), entries })
    }

    /// Seals the keyring's key again under `password`, at `cost`, as a
    /// change of password does; its entries stay as they are.
    pub fn reseal(&mut self, password: &[u8], cost: Cost) -> Result<(), String> {
        self.sealed_key = seal_key(&self.key, self.owner, password, cost)?;
        Ok(())
    }

    /// The text of its file.
    pub fn text(&self) -> Result<String, String> {
        let mut plain = entries_text(&self.entries);
        let sealed = seal(&self.key, "entries", self.owner, &plain);
        accounts::wipe(&mut plain);
        Ok(format!("{FORMAT}\nkey {}\nentries {}\n", self.sealed_key, sealed?))
    }

    /// The secret of the entry `name`.
    pub fn get(&self, name: &str) -> Option<&[u8]> {
        self.entries.get(name).map(Vec::as_slice)
    }

    /// Keeps `secret` as the entry `name`, in place of one of that name.
    pub fn set(&mut self, name: &str, secret: &[u8]) -> Result<(), String> {
        if !self.entries.contains_key(name) && self.entries.len() >= ENTRIES_MAX {
            return Err(format!("a keyring holds at most {ENTRIES_MAX} entries"));
        }
        if let Some(mut old) = self.entries.insert(name.to_string(), secret.to_vec()) {
            accounts::wipe(&mut old);
        }
        Ok(())
    }

    /// Removes the entry `name`; whether there was one.
    pub fn remove(&mut self, name: &str) -> bool {
        self.entries.remove(name).map(|mut secret| accounts::wipe(&mut secret)).is_some()
    }

    /// The entries' names, in order.
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.entries.keys().map(String::as_str)
    }

    /// Writes its file, whole, in place of the one there.
    fn write(&self) -> Result<(), String> {
        replace(&path(self.owner), &self.text()?, 0o600)
    }
}

impl Authd {
    /// Opens `user`'s keyring with `password`, which was just found to be
    /// theirs, and makes it if they have none: at a sign-in with a
    /// password, and for `keyring unlock`. An account with no password
    /// has no keyring, since nothing could lock it.
    pub(super) fn open_keyring(
        &mut self,
        files: &Files,
        user: &User,
        password: &[u8],
    ) -> Result<(), String> {
        if self.keyrings.contains_key(&user.uid) {
            return Ok(());
        }
        let Some(cost) = files.hash(&user.name).and_then(hash::cost_of) else {
            return Err(format!(
                "{} has no password, and so no keyring: nothing could lock it; set one with passwd",
                user.name
            ));
        };
        let file = path(user.uid);
        let keyring = match fs::read_to_string(&file) {
            Ok(text) => {
                Keyring::open(user.uid, &text, password).map_err(|unopened| match unopened {
                    Unopened::WrongPassword => format!(
                        "{file} does not open with {}'s password: it was sealed under another, or \
                     changed; it is left as it is",
                        user.name
                    ),
                    Unopened::Broken(why) => {
                        format!("{file} cannot be read, {why}; it is left as it is")
                    }
                })?
            }
            Err(err) if err.kind() == io::ErrorKind::NotFound => {
                let keyring = Keyring::make(user.uid, password, cost)?;
                keyring.write()?;
                keyring
            }
            Err(err) => return Err(format!("cannot read {file}: {}", util::strip_os_error(&err))),
        };
        eprintln!("authd: keyring of {}: open", user.name);
        self.keyrings.insert(user.uid, keyring);
        Ok(())
    }

    /// Opens `user`'s keyring at a sign-in with `password`; one that does
    /// not open is said in authd's log, and the sign-in goes on without
    /// it. A sign-in with no password opens none.
    pub(super) fn open_keyring_at_sign_in(&mut self, files: &Files, user: &User, password: &[u8]) {
        if password.is_empty() {
            return;
        }
        if let Err(why) = self.open_keyring(files, user, password) {
            eprintln!("authd: keyring of {}: {why}", user.name);
        }
    }

    /// Closes the keyrings of the people with no session left, nor an SSH
    /// connection whose password opened theirs, wiping their keys.
    pub(super) fn close_idle_keyrings(&mut self) {
        let signed_in: HashSet<u32> = self
            .sessions
            .values()
            .filter(|session| !session.elevated)
            .map(|session| session.number)
            .chain(self.tickets.values().map(|ticket| ticket.number))
            .collect();
        self.keyrings.retain(|owner, _| {
            let keep = signed_in.contains(owner);
            if !keep {
                eprintln!("authd: keyring of {}: closed", util::user_name(*owner));
            }
            keep
        });
    }

    /// Carries out a keyring request from `peer`, of their own keyring.
    pub(super) fn keyring(
        &mut self,
        files: &Files,
        request: &Request,
        peer: Peer,
        now: Instant,
    ) -> (Answer, Instant) {
        let Some(user) = files.users.entries().find(|u| u.uid == peer.user && u.is_person()) else {
            let why = "only a person's account has a keyring: an administrator's own is reached \
                       outside elevate";
            return (Answer::Refused(why.into()), now);
        };
        match request {
            Request::KeyringUnlock { password } => {
                if let Err(refused) = self.check_password(files, &user.name, password, peer, now) {
                    return refused;
                }
                let answer = match self.open_keyring(files, user, password) {
                    Ok(()) => Answer::Done(String::new()),
                    Err(why) => Answer::Refused(why),
                };
                (answer, now)
            }
            Request::KeyringLock => {
                if self.keyrings.remove(&user.uid).is_some() {
                    eprintln!("authd: keyring of {}: locked, as they asked", user.name);
                }
                (Answer::Done(String::new()), now)
            }
            _ => (self.keyring_entry(user, request), now),
        }
    }

    /// The Kerberos tickets in the keyring of the person numbered `uid`,
    /// for `peer`: nfs(1), as root, or the person themselves. Only the
    /// entries named `krb5 ...` are given, so that a mount's program
    /// that calls as each person learns nothing else of theirs.
    pub(super) fn kerberos_tickets(&self, files: &Files, uid: u32, peer: Peer) -> Answer {
        if !peer.is_root() && peer.user != uid {
            return Answer::Refused(
                "only nfs, which runs as root, or the person themselves reads their Kerberos \
                 tickets"
                    .into(),
            );
        }
        let name = files.users.entries().find(|u| u.uid == uid).map(|u| u.name.clone());
        let Some(keyring) = self.keyrings.get(&uid) else {
            return Answer::Refused(format!(
                "the keyring of {} is closed, so their Kerberos tickets are out of reach: they \
                 sign in with their password, or run keyring unlock",
                name.unwrap_or_else(|| uid.to_string())
            ));
        };
        let lines: Vec<String> = keyring
            .names()
            .filter(|entry| entry.starts_with(KERBEROS_PREFIX))
            .filter_map(|entry| Some(format!("{entry}\t{}", request::hex(keyring.get(entry)?))))
            .collect();
        if peer.is_root() {
            audit!(
                "gave {} Kerberos tickets of {} to process {}",
                lines.len(),
                name.as_deref().unwrap_or("no one"),
                peer.process
            );
        }
        Answer::Done(lines.join("\n"))
    }

    /// Carries out a request about an entry of `user`'s keyring, which
    /// must be open.
    fn keyring_entry(&mut self, user: &User, request: &Request) -> Answer {
        let Some(keyring) = self.keyrings.get_mut(&user.uid) else {
            return Answer::Refused(CLOSED.into());
        };
        let missing = |name: &str| format!("your keyring has no entry {name}");
        match request {
            Request::KeyringGet { name } => match keyring.get(name) {
                Some(secret) => Answer::Done(request::hex(secret)),
                None => Answer::Refused(missing(name)),
            },
            Request::KeyringList => Answer::Done(keyring.names().collect::<Vec<_>>().join("\t")),
            Request::KeyringSet { name, secret } => {
                if !request::keyring_name(name) {
                    return Answer::Refused(format!(
                        "an entry's name is 1 to {} bytes of text, with no control characters",
                        request::KEYRING_NAME_MAX
                    ));
                }
                if let Err(why) = keyring.set(name, secret) {
                    return Answer::Refused(why);
                }
                written(keyring)
            }
            Request::KeyringRemove { name } => {
                if !keyring.remove(name) {
                    return Answer::Refused(missing(name));
                }
                written(keyring)
            }
            _ => Answer::Refused("not a request about a keyring's entries".into()),
        }
    }

    /// Seals `user`'s keyring under their new password `new`, as a change
    /// of password does: the keyring open now, or its file opened with
    /// `current`, the password they had. One that cannot be opened, as
    /// when an administrator sets a password without the old one, is
    /// emptied, since no one else holds its key, as is one whose new
    /// password is empty. What is to be said of it, if anything.
    pub(super) fn reseal_keyring(
        &mut self,
        user: &User,
        current: Option<&[u8]>,
        new: &[u8],
    ) -> Option<String> {
        let file = path(user.uid);
        let open = self.keyrings.remove(&user.uid);
        let opened = match open {
            Some(keyring) => Some(keyring),
            None if fs::metadata(&file).is_err() => return None,
            None => current.and_then(|current| {
                let text = fs::read_to_string(&file).ok()?;
                Keyring::open(user.uid, &text, current).ok()
            }),
        };
        let why_emptied = match opened {
            Some(_) if new.is_empty() => "with no password, nothing could lock it",
            Some(mut keyring) => {
                let sealed = keyring.reseal(new, Cost::DEFAULT).and_then(|()| keyring.write());
                self.keyrings.insert(user.uid, keyring);
                return sealed.err().map(|why| {
                    format!(
                        "the keyring of {} is still sealed under the old password: {why}",
                        user.name
                    )
                });
            }
            None => "only the old password opened it",
        };
        self.keyrings.remove(&user.uid);
        if let Err(err) = fs::remove_file(&file)
            && err.kind() != io::ErrorKind::NotFound
        {
            return Some(format!("cannot empty {file}: {}", util::strip_os_error(&err)));
        }
        audit!("emptied the keyring of {}: {why_emptied}", user.name);
        Some(format!("The keyring of {} was emptied: {why_emptied}.", user.name))
    }

    /// Removes `user`'s keyring, as their account goes.
    pub(super) fn remove_keyring(&mut self, user: &User) {
        self.keyrings.remove(&user.uid);
        let file = path(user.uid);
        if let Err(err) = fs::remove_file(&file)
            && err.kind() != io::ErrorKind::NotFound
        {
            eprintln!("authd: cannot remove {file}: {}", util::strip_os_error(&err));
        }
    }
}

/// The answer to a change of `keyring`'s entries, once its file is
/// written.
fn written(keyring: &Keyring) -> Answer {
    match keyring.write() {
        Ok(()) => Answer::Done(String::new()),
        Err(why) => Answer::Refused(format!("{why}; the change lasts only until you sign out")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALICE: u32 = 1000;

    fn alice() -> Keyring {
        Keyring::make(ALICE, b"queen of hearts", Cost::TEST).unwrap()
    }

    #[test]
    fn opens_with_its_password_alone() {
        let mut keyring = alice();
        keyring.set("git https://example.org", b"s3cret").unwrap();
        let text = keyring.text().unwrap();
        let opened = Keyring::open(ALICE, &text, b"queen of hearts").unwrap();
        assert_eq!(opened.get("git https://example.org"), Some(&b"s3cret"[..]));
        assert_eq!(opened.names().collect::<Vec<_>>(), ["git https://example.org"]);
        assert_eq!(
            Keyring::open(ALICE, &text, b"queen of spades").err(),
            Some(Unopened::WrongPassword)
        );
    }

    /// The file says nothing of the entries in the clear: not a secret,
    /// nor a name.
    #[test]
    fn its_file_holds_no_secret_in_the_clear() {
        let mut keyring = alice();
        keyring.set("token for example.org", b"s3cret-token").unwrap();
        let text = keyring.text().unwrap();
        assert!(text.starts_with("dukovo-keyring 1\nkey m=8192,t=1,p=1 "));
        assert!(!text.contains("s3cret") && !text.contains("example.org"));
        assert!(!text.contains(&request::hex(b"s3cret-token")));
    }

    /// Bound to its owner: another person's number does not open it, even
    /// with the same password.
    #[test]
    fn is_bound_to_its_owner() {
        let text = alice().text().unwrap();
        assert_eq!(
            Keyring::open(ALICE + 1, &text, b"queen of hearts").err(),
            Some(Unopened::WrongPassword)
        );
    }

    #[test]
    fn a_new_password_opens_the_same_entries() {
        let mut keyring = alice();
        keyring.set("ssh /users/alice/.ssh/id_ed25519", b"passphrase").unwrap();
        keyring.reseal(b"the white rabbit", Cost::TEST).unwrap();
        let text = keyring.text().unwrap();
        assert_eq!(
            Keyring::open(ALICE, &text, b"queen of hearts").err(),
            Some(Unopened::WrongPassword)
        );
        let opened = Keyring::open(ALICE, &text, b"the white rabbit").unwrap();
        assert_eq!(opened.get("ssh /users/alice/.ssh/id_ed25519"), Some(&b"passphrase"[..]));
    }

    /// An entries line changed, or taken from another file, is refused,
    /// not read as empty.
    #[test]
    fn a_changed_file_is_refused() {
        let mut keyring = alice();
        keyring.set("a", b"1").unwrap();
        let text = keyring.text().unwrap();
        let other = alice().text().unwrap();
        let entries = |text: &str| text.lines().nth(2).unwrap().to_string();
        let swapped = text.replace(&entries(&text), &entries(&other));
        assert!(matches!(
            Keyring::open(ALICE, &swapped, b"queen of hearts"),
            Err(Unopened::Broken(_))
        ));
        for broken in ["", "dukovo-keyring 2\n", "dukovo-keyring 1\nkey x\nentries y z\n"] {
            assert!(matches!(
                Keyring::open(ALICE, broken, b"queen of hearts"),
                Err(Unopened::Broken(_))
            ));
        }
    }

    /// Any bytes are a secret; a removed entry goes.
    #[test]
    fn keeps_any_bytes_and_removes() {
        let mut keyring = alice();
        keyring.set("binary", &[0, 9, 10, 255]).unwrap();
        keyring.set("gone", b"x").unwrap();
        assert!(keyring.remove("gone"));
        assert!(!keyring.remove("gone"));
        let opened = Keyring::open(ALICE, &keyring.text().unwrap(), b"queen of hearts").unwrap();
        assert_eq!(opened.get("binary"), Some(&[0, 9, 10, 255][..]));
        assert_eq!(opened.get("gone"), None);
    }

    #[test]
    fn its_folder_is_authds() {
        assert!(FOLDER.starts_with(accounts::STATE));
        assert_eq!(path(ALICE), "/system/state/authd/keyrings/1000");
    }
}
