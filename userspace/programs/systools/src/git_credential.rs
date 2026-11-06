// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! git-credential-keyring(1): git's credential helper on your keyring
//! (ADR 0394), the one the system's gitconfig names. git runs it as `git
//! credential-keyring get`, `store` or `erase`, with a credential on the
//! standard input as gitcredentials(7) describes it: lines of
//! `name=value`, ending at an empty line.
//!
//! Each credential is an entry of your keyring (ADR 0393), named
//! `git PROTOCOL://USER@HOST`, with `/PATH` after it when git says one
//! (credential.useHttpPath), and holding the password or token. `get`
//! answers with the entry of the user git names, or with the first entry
//! for that address when it names none. `erase` removes only an entry
//! whose secret is the one git found wrong, so a token kept since stays.
//!
//! git goes on as if there were no helper when this one has nothing to
//! say: when the keyring is locked, `get` says so on the standard error
//! and git asks for the token. Where to start when it misbehaves:
//! `keyring list` shows the entries it made, and `GIT_TRACE=1` shows git
//! running it.

use std::io::{self, BufRead, Write};

use accounts::request::{self, Answer, Request};
use accounts::wipe;

use crate::account::ask;
use crate::args::{Arg, Args};
use crate::util::{Fail, usage};

/// The first word of every entry this helper keeps.
const PREFIX: &str = "git ";

/// What git asks.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Action {
    Get,
    Store,
    Erase,
}

/// The parts of a credential this helper reads; git's others, such as
/// `wwwauth[]`, are left out.
#[derive(Default)]
struct Credential {
    protocol: String,
    host: String,
    path: String,
    username: String,
    password: Vec<u8>,
}

impl Drop for Credential {
    fn drop(&mut self) {
        wipe(&mut self.password);
    }
}

impl Credential {
    /// The credential in `input`, as gitcredentials(7) writes it; a line
    /// that is not `name=value` is left out.
    fn read(input: &mut impl BufRead) -> io::Result<Credential> {
        let mut credential = Credential::default();
        let mut line = Vec::new();
        loop {
            line.clear();
            if input.read_until(b'\n', &mut line)? == 0 {
                break;
            }
            let text = line.strip_suffix(b"\n").unwrap_or(&line);
            if text.is_empty() {
                break;
            }
            let Some(at) = text.iter().position(|&b| b == b'=') else { continue };
            let (name, value) = (&text[..at], &text[at + 1..]);
            let value_text = || String::from_utf8_lossy(value).into_owned();
            match name {
                b"protocol" => credential.protocol = value_text(),
                b"host" => credential.host = value_text(),
                b"path" => credential.path = value_text(),
                b"username" => credential.username = value_text(),
                b"password" => credential.password = value.to_vec(),
                _ => {}
            }
        }
        wipe(&mut line);
        Ok(credential)
    }

    /// The address git asks about, without the user: `https://HOST` and
    /// `/PATH` when git says one.
    fn address(&self) -> String {
        if self.path.is_empty() {
            format!("{}://{}", self.protocol, self.host)
        } else {
            format!("{}://{}/{}", self.protocol, self.host, self.path)
        }
    }

    /// The name of its entry, for `username`.
    fn entry(&self, username: &str) -> String {
        let address = self.address();
        let (protocol, rest) = address.split_once("://").unwrap_or(("", &address));
        format!("{PREFIX}{protocol}://{username}@{rest}")
    }

    /// The user an entry's `name` is for, if it is this credential's
    /// address.
    fn user_of<'a>(&self, name: &'a str) -> Option<&'a str> {
        let rest = name.strip_prefix(PREFIX)?;
        let rest = rest.strip_prefix(&self.protocol)?.strip_prefix("://")?;
        let (username, at) = rest.rsplit_once('@')?;
        let address = self.address();
        let wanted = address.split_once("://").map_or(address.as_str(), |(_, a)| a);
        (at == wanted).then_some(username)
    }
}

/// git-credential-keyring(1).
pub fn main(args: &mut Args) -> Result<i32, Fail> {
    let action = match args.next()? {
        Some(Arg::Operand(word)) => match word.to_str() {
            Some("get") => Some(Action::Get),
            Some("store") => Some(Action::Store),
            Some("erase") => Some(Action::Erase),
            // gitcredentials(7): a helper ignores what it does not know.
            _ => None,
        },
        Some(other) => return Err(args.unknown(&other)),
        None => return Err(usage("Usage: git-credential-keyring get|store|erase".into())),
    };
    let credential = Credential::read(&mut io::stdin().lock())?;
    // A credential with no address names nothing to keep or give.
    let Some(action) =
        action.filter(|_| !credential.protocol.is_empty() && !credential.host.is_empty())
    else {
        return Ok(0);
    };
    match action {
        Action::Get => get(&credential),
        Action::Store => store(&credential),
        Action::Erase => erase(&credential),
    }
    Ok(0)
}

/// Says why the keyring did not help, once, on the standard error: git
/// then asks for the token itself.
fn say(why: &str) {
    eprintln!("git-credential-keyring: {why}");
}

/// The entries for the credential's address, with their users: those of
/// its user alone, when git names one.
fn entries(credential: &Credential) -> Result<Vec<(String, String)>, String> {
    let names = match ask(&Request::KeyringList).map_err(|fail| fail.message)? {
        Answer::Done(names) => names,
        Answer::Refused(why) => return Err(why),
    };
    Ok(names
        .split('\t')
        .filter_map(|name| Some((name.to_string(), credential.user_of(name)?.to_string())))
        .filter(|(_, user)| credential.username.is_empty() || *user == credential.username)
        .collect())
}

/// The secret of the entry `name`.
fn secret(name: &str) -> Result<Vec<u8>, String> {
    match ask(&Request::KeyringGet { name: name.into() }).map_err(|fail| fail.message)? {
        Answer::Done(hexed) => {
            request::unhex(&hexed).ok_or_else(|| format!("authd answered `{hexed}`"))
        }
        Answer::Refused(why) => Err(why),
    }
}

/// `get`: writes the user and the password of the first entry for the
/// credential, if there is one.
fn get(credential: &Credential) {
    let found = entries(credential).and_then(|entries| match entries.into_iter().next() {
        Some((name, user)) => Ok(Some((user, secret(&name)?))),
        None => Ok(None),
    });
    match found {
        Ok(Some((user, mut password))) => {
            let mut answer = format!("username={user}\npassword=").into_bytes();
            answer.extend_from_slice(&password);
            answer.push(b'\n');
            let mut out = io::stdout().lock();
            let _ = out.write_all(&answer).and_then(|()| out.flush());
            wipe(&mut answer);
            wipe(&mut password);
        }
        Ok(None) => {}
        Err(why) => say(&why),
    }
}

/// `store`: keeps the credential git found right.
fn store(credential: &Credential) {
    if credential.username.is_empty() || credential.password.is_empty() {
        return;
    }
    let name = credential.entry(&credential.username);
    if !request::keyring_name(&name) {
        return say(&format!("{name} is too long a name for a keyring's entry; it is not kept"));
    }
    match ask(&Request::KeyringSet { name, secret: credential.password.clone() }) {
        Ok(Answer::Done(_)) => {}
        Ok(Answer::Refused(why)) => say(&format!("{why}; the token is not kept")),
        Err(fail) => say(&fail.message),
    }
}

/// `erase`: removes the entries for the credential that git found wrong,
/// but only one whose secret is the password git sent, when it sent one.
fn erase(credential: &Credential) {
    let entries = match entries(credential) {
        Ok(entries) => entries,
        Err(why) => return say(&why),
    };
    for (name, _) in entries {
        let mut kept = match secret(&name) {
            Ok(kept) => kept,
            Err(why) => return say(&why),
        };
        let wrong = credential.password.is_empty() || kept == credential.password;
        wipe(&mut kept);
        if wrong && let Ok(Answer::Refused(why)) = ask(&Request::KeyringRemove { name }) {
            say(&why);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn credential(text: &str) -> Credential {
        Credential::read(&mut text.as_bytes()).unwrap()
    }

    #[test]
    fn reads_what_git_writes() {
        let asked = credential(
            "protocol=https\nhost=example.org:8443\nusername=alice\npassword=a=b\nwwwauth[]=Basic\n\n",
        );
        assert_eq!(asked.protocol, "https");
        assert_eq!(asked.host, "example.org:8443");
        assert_eq!(asked.username, "alice");
        assert_eq!(asked.password, b"a=b");
        assert_eq!(asked.entry("alice"), "git https://alice@example.org:8443");
    }

    #[test]
    fn an_entry_names_its_user_and_address() {
        let asked = credential("protocol=https\nhost=example.org\n");
        assert_eq!(asked.user_of("git https://alice@example.org"), Some("alice"));
        assert_eq!(asked.user_of("git https://alice@example.org.evil"), None);
        assert_eq!(asked.user_of("git http://alice@example.org"), None);
        assert_eq!(asked.user_of("ssh https://alice@example.org"), None);
        // A user name may hold `@`, as an e-mail address does.
        assert_eq!(asked.user_of("git https://a@b.c@example.org"), Some("a@b.c"));
        let with_path = credential("protocol=https\nhost=example.org\npath=group/repo.git\n");
        assert_eq!(with_path.entry("alice"), "git https://alice@example.org/group/repo.git");
        assert_eq!(with_path.user_of("git https://alice@example.org"), None);
        assert_eq!(
            with_path.user_of("git https://alice@example.org/group/repo.git"),
            Some("alice")
        );
    }
}
