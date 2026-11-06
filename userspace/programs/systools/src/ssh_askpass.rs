// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! ssh-askpass-keyring(1): OpenSSH's askpass in your session (ADR 0395),
//! through which ssh, ssh-add and ssh-keygen ask each question: the
//! session's environment names it in `SSH_ASKPASS`, with
//! `SSH_ASKPASS_REQUIRE=force`. OpenSSH runs it with the question as its
//! one operand and reads the answer from its standard output; a failing
//! exit is no answer.
//!
//! A key's passphrase comes from your keyring (ADR 0393), from the entry
//! `ssh PATH`, PATH being the key file's. The kept passphrase is given
//! only once this command has opened the key with it, so one changed
//! since with `ssh-keygen -p` is asked for again rather than given
//! wrong. A passphrase typed is kept once it opens the key; one that does
//! not is handed to OpenSSH, which says it is wrong and asks again.
//! Every other question, a password, a PIN or a new passphrase, is asked
//! at the terminal as OpenSSH asks it without an askpass: unseen, but for
//! a yes or no.
//!
//! Where to start when it misbehaves: `keyring list` shows the
//! passphrases kept, `ssh -v` shows ssh running it, and
//! `SSH_ASKPASS_REQUIRE=never` before a command has OpenSSH ask on its
//! own.

use std::io::{self, Write};
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};

use accounts::request;
use accounts::wipe;
use ssh_key::PrivateKey;

use crate::args::{Arg, Args};
use crate::keyring;
use crate::prompt;
use crate::util::{Fail, usage};

/// The terminal, where the questions are asked.
const TERMINAL: &str = "/devices/tty";

/// The first word of each entry this command keeps.
const PREFIX: &str = "ssh ";

/// What ssh-add adds to a key's file in its question for `ssh-add -c`.
const CONFIRMED: &str = " (will confirm each use)";

/// What OpenSSH asks.
#[derive(Debug, PartialEq, Eq)]
enum Question<'a> {
    /// The passphrase of the key in that file.
    Passphrase(&'a str),
    /// A yes or no, typed where it is seen.
    YesOrNo,
    /// Anything else, typed unseen.
    Unseen,
}

/// What `prompt` asks, in OpenSSH 10.6's words: ssh, ssh-keygen and
/// ssh-add each name a key's file in their own way.
fn question(prompt: &str) -> Question<'_> {
    let asked = prompt.trim_end().strip_suffix(':').unwrap_or(prompt);
    // ssh(1), loading a key: `Enter passphrase for key 'FILE': `.
    if let Some(rest) = asked.strip_prefix("Enter passphrase for key '")
        && let Some(file) = rest.strip_suffix('\'')
    {
        return Question::Passphrase(file);
    }
    // ssh-keygen(1), signing with a key: `Enter passphrase for "FILE": `.
    if let Some(rest) = asked.strip_prefix("Enter passphrase for \"")
        && let Some(file) = rest.strip_suffix('"')
    {
        return Question::Passphrase(file);
    }
    // ssh-add(1): `Enter passphrase for FILE: `, and again after a wrong
    // one.
    let added = asked
        .strip_prefix("Enter passphrase for ")
        .or_else(|| asked.strip_prefix("Bad passphrase, try again for "));
    if let Some(rest) = added {
        return Question::Passphrase(rest.strip_suffix(CONFIRMED).unwrap_or(rest));
    }
    // ssh(1)'s question about a host's key, and its asking again.
    if prompt.contains("(yes/no") || prompt.contains("Please type 'yes'") {
        return Question::YesOrNo;
    }
    Question::Unseen
}

/// ssh-askpass-keyring(1).
pub fn main(args: &mut Args) -> Result<i32, Fail> {
    let prompt = match args.next()? {
        Some(Arg::Operand(prompt)) => prompt.to_string_lossy().into_owned(),
        Some(other) => return Err(args.unknown(&other)),
        None => return Err(usage("Usage: ssh-askpass-keyring QUESTION".into())),
    };
    // OpenSSH's hint: `confirm` asks leave to use a key, with the exit
    // status as the answer, and `none` only tells.
    match std::env::var("SSH_ASKPASS_PROMPT").as_deref() {
        Ok("confirm") => return Ok(if confirmed(&prompt) { 0 } else { 1 }),
        Ok("none") => {
            tell(&format!("{prompt}\n"));
            return Ok(0);
        }
        _ => {}
    }
    let answer = match question(&prompt) {
        Question::Passphrase(file) => passphrase(&prompt, Path::new(file)),
        Question::YesOrNo => typed(&prompt, true),
        Question::Unseen => typed(&prompt, false),
    };
    let Some(mut answer) = answer else { return Ok(1) };
    answer.push(b'\n');
    let mut out = io::stdout().lock();
    let written = out.write_all(&answer).and_then(|()| out.flush());
    wipe(&mut answer);
    Ok(if written.is_ok() { 0 } else { 1 })
}

/// The line typed at the terminal after `prompt`, seen or not; `None`
/// with no terminal, as in a session without one, or when Ctrl+C stopped
/// the question.
fn typed(prompt: &str, shown: bool) -> Option<Vec<u8>> {
    let terminal = std::fs::OpenOptions::new().read(true).write(true).open(TERMINAL).ok()?;
    let fd = terminal.as_raw_fd();
    match prompt::ask_on(fd, fd, prompt, shown) {
        Ok(prompt::Answer::Line(line)) => Some(line),
        Ok(prompt::Answer::Stopped) | Err(_) => None,
    }
}

/// Writes `text` on the terminal, if there is one.
fn tell(text: &str) {
    if let Ok(mut terminal) = std::fs::OpenOptions::new().write(true).open(TERMINAL) {
        let _ = terminal.write_all(text.as_bytes());
    }
}

/// Asks `prompt` as a yes or no; no unless it is answered yes.
fn confirmed(prompt: &str) -> bool {
    let question = format!("{} (yes/no) ", prompt.trim_end());
    typed(&question, true)
        .is_some_and(|answer| matches!(answer.trim_ascii(), b"y" | b"Y" | b"yes" | b"Yes" | b"YES"))
}

/// A key's file, read once to try passphrases on.
struct Key {
    path: PathBuf,
    sealed: PrivateKey,
}

impl Key {
    /// The key in `file`, if it is an OpenSSH key sealed with a
    /// passphrase; a file named in the current folder is the one OpenSSH
    /// read.
    fn read(file: &Path) -> Option<Key> {
        let path = std::path::absolute(file).ok()?;
        let text = std::fs::read(&path).ok()?;
        let sealed = PrivateKey::from_openssh(&text).ok()?;
        sealed.is_encrypted().then_some(Key { path, sealed })
    }

    /// Its entry's name, if the file's path can be one.
    fn entry(&self) -> Option<String> {
        let name = format!("{PREFIX}{}", self.path.to_str()?);
        request::keyring_name(&name).then_some(name)
    }

    /// Whether `passphrase` opens it. The key opened is wiped as it is
    /// dropped.
    fn opens(&self, passphrase: &[u8]) -> bool {
        self.sealed.decrypt(passphrase).is_ok()
    }
}

/// The passphrase of the key in `file`: the one kept, if it opens the
/// key, or one typed, which is kept when it opens the key. A file this
/// command cannot open, as one that is not an OpenSSH key, or whose
/// path is not text, has its passphrase asked for and not kept.
fn passphrase(prompt: &str, file: &Path) -> Option<Vec<u8>> {
    let key = Key::read(file);
    let Some((key, name)) = key.as_ref().and_then(|key| Some((key, key.entry()?))) else {
        return typed(prompt, false);
    };
    match keyring::entry(&name) {
        Ok(Some(kept)) if key.opens(&kept) => return Some(kept),
        // Changed since it was kept: asked for, and kept again below.
        Ok(Some(mut kept)) => wipe(&mut kept),
        Ok(None) => {}
        // Most often a locked keyring: the question is asked as OpenSSH
        // would ask it, and nothing is kept.
        Err(why) => {
            let answer = typed(prompt, false)?;
            tell(&format!("ssh-askpass-keyring: {why}; the passphrase is not kept\n"));
            return Some(answer);
        }
    }
    let answer = typed(prompt, false)?;
    if !answer.is_empty()
        && key.opens(&answer)
        && let Err(why) = keyring::keep(&name, &answer)
    {
        tell(&format!("ssh-askpass-keyring: {why}; the passphrase is not kept\n"));
    }
    Some(answer)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_the_key_in_each_question() {
        let key = "/users/alice/.ssh/id_ed25519";
        for prompt in [
            "Enter passphrase for key '/users/alice/.ssh/id_ed25519': ",
            "Enter passphrase for \"/users/alice/.ssh/id_ed25519\": ",
            "Enter passphrase for /users/alice/.ssh/id_ed25519: ",
            "Enter passphrase for /users/alice/.ssh/id_ed25519 (will confirm each use): ",
            "Bad passphrase, try again for /users/alice/.ssh/id_ed25519: ",
        ] {
            assert_eq!(question(prompt), Question::Passphrase(key), "{prompt}");
        }
        // A file named with spaces, as ssh-add names it.
        assert_eq!(
            question("Enter passphrase for my keys/work key: "),
            Question::Passphrase("my keys/work key")
        );
    }

    #[test]
    fn asks_the_rest_as_openssh_would() {
        let host = "The authenticity of host 'example.org' can't be established.\nAre you \
                    sure you want to continue connecting (yes/no/[fingerprint])? ";
        assert_eq!(question(host), Question::YesOrNo);
        assert_eq!(question("Please type 'yes', 'no' or the fingerprint: "), Question::YesOrNo);
        assert_eq!(question("alice@example.org's password: "), Question::Unseen);
        assert_eq!(question("Enter passphrase: "), Question::Unseen);
        assert_eq!(question("Enter new passphrase (empty for no passphrase): "), Question::Unseen);
        assert_eq!(question("Enter PIN for authenticator: "), Question::Unseen);
    }
}
