// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Help sessions (ADR 0540): a person lets a helper, who needs no
//! account, sign in over SSH as `help` and work in a shell of theirs,
//! which they watch and end with one key.
//!
//! help-session(1) asks with `help-start`, sending the help screen's
//! terminal and one end of a socket pair. authd makes a resource domain
//! `help-ID` below its own, which Spark gives it the capability to
//! (`domain = true`), starts the owner's shell on the terminal in that
//! domain, and keeps the transcript, `/system/state/help/ID.log`, which
//! only root reads. When the helper signs in with the session's code, used
//! once, or with the key the owner named, authd hands sshd the socket end
//! and the transcript; sshd relays the helper's typing through the socket
//! to help-session and writes it to the transcript.
//!
//! The session ends when its owner ends it, its time is up, the helper's
//! connection or the shell ends, or help-session's connection closes.
//! The end stops the domain, so that nothing the helper started goes on,
//! and shuts the socket, which ends the helper's connection.
//!
//! Inside the help screen, which authd tells by a process's place in the
//! domain's `/processes`, nothing asks for a password, as the helper
//! could fake the question and read the answer: elevate runs only while
//! the owner allows it from outside the screen, `help-allow-elevate`,
//! and then without one.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::fd::{AsRawFd, OwnedFd};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::time::{Duration, Instant};

use accounts::User;
use accounts::request::{
    self, Answer, HELP_CODE_LENGTH, HELP_CODE_LETTERS, HELP_ELEVATE_MINUTES, HELP_FOLDER,
    HELP_MINUTES_MAX, HELP_MINUTES_MIN, HELP_NAME, HELP_NO_PASSWORDS, HELP_ONLY, HELP_WAITING,
    HelpEnded, HelpEvent, HelpKept, HelpProof, HelpStarted,
};
use radix_abi::capability::{self, Rights};
use radix_abi::directory::{self, OpenFlags};
use radix_abi::resource_domain::{self, NO_LIMIT, OomClass, WEIGHT_DEFAULT};
use radix_abi::startup::SLOT_DOMAIN;

use super::blocking::{Failed, unix_now};
use super::{Authd, Files, Peer, Wrong, shown, ssh_from, ssh_source, wait_after};
use crate::sys::{self, SYS};
use crate::{power, signin, sshkeys, util};

/// The public half of sshd's host key, whose fingerprint the owner gives
/// the helper to check.
const HOST_KEY: &str = "/system/state/sshd/ssh_host_ed25519_key.pub";

/// The list of the transcripts kept: a [`HelpKept`] line each.
const INDEX: &str = "/system/state/help/index";

/// The transcripts kept for each owner, the newest.
const KEPT: usize = 20;

/// The wrong codes that spend a session's code.
const WRONG_CODES: u32 = 5;

/// The minutes before its end at which both screens are told.
const WARNINGS: [u32; 2] = [5, 1];

/// The letters of a session's ID: lowercase letters and digits, without
/// the `i`, `l` and `o` that look like `1` and `0`, nor `1`.
const ID_LETTERS: &[u8; 32] = b"abcdefghjkmnpqrstuvwxyz023456789";

/// The length of a session's ID.
const ID_LENGTH: usize = 6;

/// What a session's domain is called: `help-` and its ID.
const DOMAIN_PREFIX: &str = "help-";

/// Why an elevate in the help screen is refused while its owner does not
/// allow it.
pub(super) const ELEVATE_NOT_ALLOWED: &str =
    "in a help session, elevate runs only while its owner allows it: help-session allow-elevate";

/// Why a keyring request from the help screen is refused.
pub(super) const KEYRING_SHUT: &str =
    "in a help session, the keyring stays shut: use it from another terminal";

/// What a `help-start` asked for, as [`Authd::make_help`] takes it.
#[derive(Clone, Copy)]
struct Asked<'a> {
    minutes: u32,
    key: Option<&'a [u8]>,
    term: &'a str,
    opened_ssh: bool,
}

/// The helper of a session, once they signed in.
struct Helper {
    /// Their address.
    address: String,
    /// What they signed in with: `code`, or their key's fingerprint.
    with: String,
    /// sshd's process that serves their connection, which alone says it
    /// ended.
    process: u64,
}

/// A help session.
pub(super) struct Help {
    /// Its ID.
    pub(super) id: String,
    /// Its owner's account number and name.
    owner: u32,
    owner_name: String,
    /// When it ends, on the clock authd waits by.
    ends: Instant,
    /// The code that signs the helper in, until it is used or spent.
    code: Option<String>,
    /// The key that signs the helper in, if the owner named one.
    key: Option<Vec<u8>>,
    /// The wrong codes typed for it.
    wrong_codes: u32,
    /// sshd's end of the socket pair, kept so that the end can shut it.
    socket: OwnedFd,
    /// The transcript, written at its end.
    transcript: File,
    /// Its domain, the domain's `/processes`, and a slot to look in it
    /// with.
    slots: signin::Spares,
    /// The shell in the help screen.
    pub(super) shell: i32,
    /// The helper, once signed in.
    helper: Option<Helper>,
    /// The warnings given, of [`WARNINGS`].
    warned: usize,
    /// Until when elevate runs in the help screen, as its owner allows.
    elevate_until: Option<Instant>,
    /// Whether it started sshd, which it stops at its end.
    opened_ssh: bool,
}

impl Help {
    fn domain(&self) -> u32 {
        self.slots.slot(0)
    }

    /// Whether the process `number` runs in its help screen: its domain's
    /// `/processes` finds it.
    fn holds(&self, number: u64) -> bool {
        let (processes, scratch) = (self.slots.slot(1), self.slots.slot(2));
        let name = number.to_string();
        let found = directory::open(
            &SYS,
            processes,
            name.as_bytes(),
            OpenFlags::DIRECTORY,
            Rights::READ,
            scratch,
            0,
        )
        .is_ok();
        let _ = capability::delete(&SYS, scratch);
        found
    }

    /// Whether it waits for a helper to sign in with a code.
    fn waits_for_code(&self) -> bool {
        self.helper.is_none() && self.code.is_some()
    }

    /// When it next has something to do: a warning, or its end.
    pub(super) fn next(&self) -> Instant {
        WARNINGS.get(self.warned).map_or(self.ends, |&minutes| {
            self.ends.checked_sub(Duration::from_secs(u64::from(minutes) * 60)).unwrap_or(self.ends)
        })
    }

    /// Writes a line to its transcript.
    fn note(&mut self, kind: &str, text: &str) {
        let line = transcript_line(unix_now(), kind, text);
        if let Err(err) = self.transcript.write_all(line.as_bytes()) {
            eprintln!("authd: cannot write the transcript of help session {}: {err}", self.id);
        }
    }

    /// What the owner was given to sign the helper in with, in words.
    fn how(&self) -> String {
        match &self.key {
            Some(key) => format!("the key {}", sshkeys::fingerprint(key)),
            None => "a code".into(),
        }
    }
}

/// A transcript's line: `SECONDS KIND TEXT`, with what the text holds
/// that is not printable shown, so that one line stays one.
pub(super) fn transcript_line(seconds: u64, kind: &str, text: &str) -> String {
    format!("{seconds} {kind} {}\n", printable(text.as_bytes()))
}

/// `bytes` as a transcript keeps them: control characters as `^X`, other
/// bytes outside printable ASCII as `\xNN`, and a backslash doubled.
pub(super) fn printable(bytes: &[u8]) -> String {
    let mut text = String::with_capacity(bytes.len());
    for &byte in bytes {
        match byte {
            b'\\' => text.push_str("\\\\"),
            b' '..=b'~' => text.push(char::from(byte)),
            0..=0x1f => {
                text.push('^');
                text.push(char::from(byte + 0x40));
            }
            0x7f => text.push_str("^?"),
            _ => text.push_str(&format!("\\x{byte:02x}")),
        }
    }
    text
}

/// Whether `typed` is `code`, compared in a time that does not say how
/// much of it was right.
fn same_code(typed: &str, code: &str) -> bool {
    let (typed, code) = (typed.as_bytes(), code.as_bytes());
    typed.len() == code.len() && typed.iter().zip(code).fold(0, |all, (a, b)| all | (a ^ b)) == 0
}

/// `count` letters of `letters`, chosen from `random`, a byte each: as
/// 32 divides 256, each letter is as likely as each other.
fn letters_from(random: &[u8], letters: &[u8; 32]) -> String {
    random.iter().map(|&byte| char::from(letters[usize::from(byte % 32)])).collect()
}

/// `count` random bytes.
fn random(count: usize) -> Result<Vec<u8>, String> {
    let mut bytes = vec![0; count];
    File::open("/devices/urandom")
        .and_then(|mut file| file.read_exact(&mut bytes))
        .map_err(|err| format!("cannot read /devices/urandom: {err}"))?;
    Ok(bytes)
}

/// The fingerprint of sshd's host key, if it has made one.
fn host_key() -> Option<String> {
    let text = fs::read_to_string(HOST_KEY).ok()?;
    let key = sshkeys::public_line(text.lines().next()?)?;
    Some(sshkeys::fingerprint(&key))
}

/// Whether Spark runs sshd, or is starting it.
fn sshd_runs() -> bool {
    use spark_api::show::{self, State};
    power::request("show").ok().and_then(|answer| show::parse(&answer).ok()).is_some_and(|shown| {
        shown.services.iter().any(|service| {
            service.name == "sshd"
                && matches!(service.state, State::Running | State::Starting | State::Pending)
        })
    })
}

/// The transcripts kept, from [`INDEX`].
fn kept() -> Vec<HelpKept> {
    fs::read_to_string(INDEX).unwrap_or_default().lines().filter_map(HelpKept::from_words).collect()
}

/// Writes `list` as [`INDEX`].
fn keep_index(list: &[HelpKept]) {
    let text: String = list.iter().map(|kept| kept.words() + "\n").collect();
    if let Err(why) = super::replace(INDEX, &text, 0o600) {
        eprintln!("authd: {why}");
    }
}

/// The transcript of the session `id`.
fn transcript_path(id: &str) -> String {
    format!("{HELP_FOLDER}/{id}.log")
}

/// `list` with `owner`'s oldest transcripts left out, so that with one
/// more they keep [`KEPT`]; the IDs left out, whose files go.
fn pruned(list: Vec<HelpKept>, owner: &str) -> (Vec<HelpKept>, Vec<String>) {
    let theirs = list.iter().filter(|kept| kept.owner == owner).count();
    let mut over = (theirs + 1).saturating_sub(KEPT);
    let mut gone = Vec::new();
    // The list is in the order the sessions started, the oldest first.
    let left = list
        .into_iter()
        .filter(|kept| {
            if over > 0 && kept.owner == owner && kept.ended != 0 {
                over -= 1;
                gone.push(kept.id.clone());
                false
            } else {
                true
            }
        })
        .collect();
    (left, gone)
}

/// A new session's ID, one no transcript has.
fn new_id(list: &[HelpKept]) -> Result<String, String> {
    for _ in 0..16 {
        let id = letters_from(&random(ID_LENGTH)?, ID_LETTERS);
        let taken = list.iter().any(|kept| kept.id == id);
        if !taken && fs::symlink_metadata(transcript_path(&id)).is_err() {
            return Ok(id);
        }
    }
    Err("cannot choose an ID for the help session".into())
}

impl Authd {
    /// The help session whose screen runs the process `number`, by its
    /// place in [`Authd::helps`].
    pub(super) fn help_of(&self, number: u64) -> Option<usize> {
        self.helps.iter().position(|help| help.holds(number))
    }

    /// What a request from inside a help screen may not do, and why:
    /// anything that asks for a password, opens the keyring, or starts or
    /// allows another help session. An elevate is let through while its
    /// owner allows it.
    pub(super) fn help_refuses(&self, request: &request::Request, peer: Peer) -> Option<String> {
        use accounts::request::Request;
        let asks = match request {
            Request::Check { .. }
            | Request::Unlock { .. }
            | Request::SharePassword { .. }
            | Request::HelpStart { .. }
            | Request::HelpAllowElevate { .. } => HELP_NO_PASSWORDS,
            Request::Passwd { .. } => HELP_NO_PASSWORDS,
            Request::KeyringSet { .. }
            | Request::KeyringGet { .. }
            | Request::KeyringList
            | Request::KeyringRemove { .. }
            | Request::KeyringUnlock { .. }
            | Request::KeyringLock => KEYRING_SHUT,
            Request::Elevate { .. } => ELEVATE_NOT_ALLOWED,
            _ => return None,
        };
        // Root inside the screen is an elevated command, which the owner
        // allowed: it may do what root does.
        if peer.is_root() {
            return None;
        }
        let at = self.help_of(peer.process)?;
        if matches!(request, Request::Elevate { .. }) && self.elevate_allowed(at) {
            return None;
        }
        Some(asks.to_string())
    }

    /// Whether elevate runs in the help screen of session `at` now.
    fn elevate_allowed(&self, at: usize) -> bool {
        self.helps[at].elevate_until.is_some_and(|until| until > Instant::now())
    }

    /// The domain an elevate from `peer` starts its command in: the help
    /// screen's, when it comes from one, so that the session's end ends
    /// it too.
    pub(super) fn elevate_domain(&self, peer: Peer) -> Option<u32> {
        let at = self.help_of(peer.process)?;
        Some(self.helps[at].domain())
    }

    /// Updates [`HELP_WAITING`]: there while a session waits for a
    /// helper with a code, for sshd to offer passwords to `help`.
    fn mark_waiting(&self) {
        let result = if self.helps.iter().any(Help::waits_for_code) {
            fs::write(HELP_WAITING, b"")
                .and_then(|()| fs::set_permissions(HELP_WAITING, fs::Permissions::from_mode(0o644)))
        } else {
            match fs::remove_file(HELP_WAITING) {
                Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(()),
                other => other,
            }
        };
        if let Err(err) = result {
            eprintln!("authd: cannot mark {HELP_WAITING}: {err}");
        }
    }

    /// Tells the owner's help-session of `event`, on its connection.
    fn tell_owner(&mut self, id: &str, event: &HelpEvent, now: Instant) {
        let line = event.line();
        for client in self.clients.iter_mut().filter(|c| c.help.as_deref() == Some(id)) {
            client.tell(line.as_bytes(), now);
        }
    }

    /// Starts a help session for `peer`, client `i`, as `help-start`
    /// asks (ADR 0540).
    #[allow(clippy::too_many_arguments)]
    pub(super) fn help_start(
        &mut self,
        i: usize,
        files: &Files,
        minutes: u32,
        key: Option<&[u8]>,
        open_ssh: bool,
        term: &str,
        password: &[u8],
        peer: Peer,
        now: Instant,
    ) -> (Answer, Instant) {
        let refused = |why: &str| (Answer::Refused(why.into()), now);
        let mut descriptors = std::mem::take(&mut self.clients[i].descriptors);
        if peer.is_root() {
            return refused(
                "a help session is a person's, with their shell: start it without elevate",
            );
        }
        let Some(user) = files.users.entries().find(|u| u.uid == peer.user && u.is_person()) else {
            return refused("only a person's account starts a help session");
        };
        let user = user.clone();
        if self.helps.iter().any(|help| help.owner == user.uid) {
            return refused("you have a help session already: help-session end ends it");
        }
        if !(HELP_MINUTES_MIN..=HELP_MINUTES_MAX).contains(&minutes) {
            return refused(&format!(
                "a help session lasts {HELP_MINUTES_MIN} minutes to {} hours",
                HELP_MINUTES_MAX / 60
            ));
        }
        if let Some(key) = key
            && sshkeys::algorithm_of(key).is_none()
        {
            return refused("the helper's key is not one sshd takes");
        }
        let socket = (descriptors.len() == 2).then(|| descriptors.remove(1));
        let terminal = descriptors.pop().filter(signin::is_terminal);
        let (Some(terminal), Some(socket)) = (terminal, socket) else {
            return refused(
                "a help-start request comes with the help screen's terminal and a socket",
            );
        };
        let Some(names) = self.names else {
            return refused(
                "authd holds no names capability to start sessions with: its service file needs \
                 `names = true`",
            );
        };
        if capability::info(&SYS, SLOT_DOMAIN).is_err() {
            return refused(
                "authd holds no capability to its own domain: its service file needs `domain = \
                 true`",
            );
        }
        // sshd must take the helper's sign-in.
        let mut opened_ssh = false;
        if !sshd_runs() {
            if !open_ssh {
                return refused(
                    "SSH is off, so a helper cannot sign in: an administrator's help-session start \
                     --open-ssh starts it for this session alone",
                );
            }
            if !accounts::is_admin(&user, &files.groups) {
                return refused("only an administrator starts SSH for a help session");
            }
            if let Err(answer) = self.check_password(files, &user.name, password, peer, now) {
                return answer;
            }
            if let Err(err) = fs::write(HELP_ONLY, b"") {
                return refused(&format!("cannot write {HELP_ONLY}: {err}"));
            }
            if let Err(fail) = power::request("start sshd") {
                let _ = fs::remove_file(HELP_ONLY);
                return refused(&format!("cannot start sshd: {}", fail.message));
            }
            opened_ssh = true;
        }
        let groups =
            accounts::other_groups(&user, &files.groups, radix_abi::names::GROUPS_MAX as usize);
        let asked = Asked { minutes, key, term, opened_ssh };
        match self.make_help(&user, &groups, names, &terminal, socket, &asked, now) {
            Ok(started) => {
                self.clients[i].help = Some(started.id.clone());
                (Answer::Done(started.words()), now)
            }
            Err(why) => {
                if opened_ssh {
                    stop_sshd();
                }
                (Answer::Refused(why), now)
            }
        }
    }

    /// Makes the session: its domain, shell and transcript.
    #[allow(clippy::too_many_arguments)]
    fn make_help(
        &mut self,
        user: &User,
        groups: &[u32],
        names: u32,
        terminal: &OwnedFd,
        socket: OwnedFd,
        asked: &Asked<'_>,
        now: Instant,
    ) -> Result<HelpStarted, String> {
        let Asked { minutes, key, term, opened_ssh } = *asked;
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(HELP_FOLDER)
            .map_err(|err| format!("cannot make {HELP_FOLDER}: {err}"))?;
        let (list, gone) = pruned(kept(), &user.name);
        for id in &gone {
            let _ = fs::remove_file(transcript_path(id));
        }
        let id = new_id(&list)?;
        let code = match key {
            Some(_) => None,
            None => Some(letters_from(&random(HELP_CODE_LENGTH)?, HELP_CODE_LETTERS)),
        };
        let slots = signin::Spares::new(3).map_err(|err| format!("no slots: {err}"))?;
        let (domain, processes) = (slots.slot(0), slots.slot(1));
        let name = format!("{DOMAIN_PREFIX}{id}");
        resource_domain::create(
            &SYS,
            SLOT_DOMAIN,
            name.as_bytes(),
            WEIGHT_DEFAULT,
            NO_LIMIT,
            OomClass::App,
            domain,
        )
        .map_err(|status| {
            format!("cannot make the domain {name}: {}", sys::status_name(status.0))
        })?;
        resource_domain::processes(&SYS, domain, processes).map_err(|status| {
            format!("cannot see the domain {name}'s processes: {}", sys::status_name(status.0))
        })?;
        let shell = signin::start_help(names, user, groups, terminal, term, domain, &id)?;
        let path = transcript_path(&id);
        let opened = OpenOptions::new().append(true).create_new(true).mode(0o600).open(&path);
        let transcript = match opened {
            Ok(file) => file,
            Err(err) => {
                let _ = resource_domain::stop(&SYS, domain);
                return Err(format!("cannot make {path}: {err}"));
            }
        };
        let started = unix_now();
        let until = started + u64::from(minutes) * 60;
        let mut help = Help {
            id: id.clone(),
            owner: user.uid,
            owner_name: user.name.clone(),
            ends: now + Duration::from_secs(u64::from(minutes) * 60),
            code: code.clone(),
            key: key.map(<[u8]>::to_vec),
            wrong_codes: 0,
            socket,
            transcript,
            slots,
            shell: shell as i32,
            helper: None,
            // A warning as long before the end as the session lasts is
            // never given.
            warned: WARNINGS.iter().take_while(|&&warning| warning >= minutes).count(),
            elevate_until: None,
            opened_ssh,
        };
        let how = help.how();
        let until_said = util::local_time(until as i64, "%H:%M");
        help.note(
            "start",
            &format!("{} started it, until {until_said}, for a helper with {how}", user.name),
        );
        audit!(
            "{} started help session {id} until {until_said}, for a helper with {how}: shell process {shell}",
            user.name
        );
        let mut list = list;
        list.push(HelpKept {
            id: id.clone(),
            owner: user.name.clone(),
            started: started as i64,
            ended: 0,
        });
        keep_index(&list);
        self.helps.push(help);
        self.mark_waiting();
        Ok(HelpStarted { id, until: until as i64, code, host_key: host_key() })
    }

    /// Whether sshd may let `key` sign a helper in: a waiting session
    /// takes it. sshd's to ask, before the client signs with it.
    pub(super) fn ssh_help_key(&self, key: &[u8], peer: Peer) -> Answer {
        if !peer.is_root() {
            return Answer::Refused("only sshd, which runs as root, signs helpers in".into());
        }
        let taken =
            self.helps.iter().any(|help| help.helper.is_none() && help.key.as_deref() == Some(key));
        if taken {
            Answer::Done(String::new())
        } else {
            Answer::Refused("no help session waits for this key".into())
        }
    }

    /// Signs a helper in, as `ssh-help` asks: sshd's to ask, for its
    /// process serving `connection`. A right one is answered with the
    /// session's ID, the socket end and the transcript.
    pub(super) fn ssh_help(
        &mut self,
        i: usize,
        files: &Files,
        connection: &str,
        proof: &HelpProof,
        peer: Peer,
        now: Instant,
    ) -> (Answer, Instant) {
        let refused = |why: String| (Answer::Refused(why), now);
        if !peer.is_root() {
            return refused("only sshd, which runs as root, signs helpers in".into());
        }
        let Some(from) = ssh_from(connection).map(str::to_string) else {
            return refused(
                "a helper's sign-in names its connection as SSH_CONNECTION does".into(),
            );
        };
        if let Some(why) = self.blocked_why(&from) {
            return refused(why);
        }
        let (at, with) = match proof {
            HelpProof::Code(typed) => {
                let typed = request::help_code(typed).unwrap_or_default();
                let found = self.helps.iter().position(|help| {
                    help.waits_for_code()
                        && help.code.as_deref().is_some_and(|code| same_code(&typed, code))
                });
                match found {
                    Some(at) => (at, "code".to_string()),
                    None => return self.wrong_code(files, &from, peer, now),
                }
            }
            HelpProof::Key { signed, signature } => {
                let signed = match sshkeys::verify(signed, signature) {
                    Ok(signed) if signed.user == HELP_NAME => signed,
                    Ok(_) => {
                        return refused("the signed sign-in request names another account".into());
                    }
                    Err(why) => {
                        audit!("refused a helper's sign-in from {from}: {why}");
                        return refused(why.into());
                    }
                };
                let found = self.helps.iter().position(|help| {
                    help.helper.is_none() && help.key.as_deref() == Some(signed.key)
                });
                let fingerprint = sshkeys::fingerprint(signed.key);
                match found {
                    Some(at) => (at, fingerprint),
                    None => {
                        audit!(
                            "refused a helper's sign-in from {from} with the key {fingerprint}: no help session waits for it"
                        );
                        return refused("no help session waits for this key".into());
                    }
                }
            }
        };
        self.wrong.remove(&(HELP_NAME.to_string(), ssh_source(&from)));
        self.sign_in_right(&from);
        let help = &mut self.helps[at];
        let copies =
            help.socket.try_clone().and_then(|socket| Ok((socket, help.transcript.try_clone()?)));
        let (socket, transcript) = match copies {
            Ok(copies) => copies,
            Err(err) => return refused(format!("cannot hand the help session over: {err}")),
        };
        help.code = None;
        help.helper =
            Some(Helper { address: from.clone(), with: with.clone(), process: peer.process });
        help.note(
            "helper",
            &format!(
                "signed in from {from} with {}",
                if with == "code" { "the code".into() } else { format!("the key {with}") }
            ),
        );
        let (id, owner) = (help.id.clone(), help.owner_name.clone());
        audit!(
            "a helper signed in to {owner}'s help session {id} from {from} with {with}, for process {}",
            peer.process
        );
        self.mark_waiting();
        self.tell_owner(&id, &HelpEvent::Helper { address: from, key: with }, now);
        self.clients[i].answer_fds = vec![socket, transcript.into()];
        (Answer::Done(id), now)
    }

    /// A code that signs no helper in, from `from`: each session that
    /// waits for one counts it, and five spend its code; the address
    /// counts it as a failed sign-in, and its next try waits, as after a
    /// wrong password.
    fn wrong_code(
        &mut self,
        files: &Files,
        from: &str,
        peer: Peer,
        now: Instant,
    ) -> (Answer, Instant) {
        audit!("a wrong help-session code from {from}, for process {}", peer.process);
        let mut spent = Vec::new();
        for help in self.helps.iter_mut().filter(|help| help.waits_for_code()) {
            help.wrong_codes += 1;
            if help.wrong_codes >= WRONG_CODES {
                help.code = None;
                help.note("spent", &format!("{WRONG_CODES} wrong codes spent the code"));
                spent.push((help.id.clone(), help.key.is_none()));
            }
        }
        for (id, ends) in spent {
            audit!("{WRONG_CODES} wrong codes spent the code of help session {id}");
            self.tell_owner(&id, &HelpEvent::Spent, now);
            if ends && let Some(at) = self.helps.iter().position(|help| help.id == id) {
                self.end_help(at, HelpEnded::Spent, now);
            }
        }
        self.mark_waiting();
        self.sign_in_failed(files, from, Failed::HelpCode, Some(HELP_NAME));
        let wrong = self
            .wrong
            .entry((HELP_NAME.to_string(), ssh_source(from)))
            .or_insert(Wrong { count: 0, next: now });
        wrong.count += 1;
        wrong.next = now + wait_after(wrong.count);
        (Answer::Refused(request::WRONG_PASSWORD.into()), wrong.next)
    }

    /// The helper's connection to session `id` ended: sshd's to say, from
    /// the process that serves it.
    pub(super) fn ssh_help_ended(&mut self, id: &str, peer: Peer, now: Instant) -> Answer {
        let at = self.helps.iter().position(|help| {
            help.id == id && help.helper.as_ref().is_some_and(|h| h.process == peer.process)
        });
        match at {
            Some(at) if peer.is_root() => {
                self.end_help(at, HelpEnded::Helper, now);
                Answer::Done(String::new())
            }
            _ => {
                Answer::Refused("only the sshd process that serves a helper says they left".into())
            }
        }
    }

    /// Ends `peer`'s help session, as its owner asks.
    pub(super) fn help_end(&mut self, peer: Peer, now: Instant) -> Answer {
        match self.helps.iter().position(|help| help.owner == peer.user) {
            Some(at) => {
                self.end_help(at, HelpEnded::Owner, now);
                Answer::Done(String::new())
            }
            None => Answer::Refused("you have no help session".into()),
        }
    }

    /// Lets elevate run in `peer`'s help screen for
    /// [`HELP_ELEVATE_MINUTES`], after their password: an administrator's
    /// to ask, from outside the screen.
    pub(super) fn help_allow_elevate(
        &mut self,
        files: &Files,
        password: &[u8],
        peer: Peer,
        now: Instant,
    ) -> (Answer, Instant) {
        let refused = |why: &str| (Answer::Refused(why.into()), now);
        let Some(at) = self.helps.iter().position(|help| help.owner == peer.user) else {
            return refused("you have no help session");
        };
        let user = match super::administrator(files, peer) {
            Ok(user) => user.clone(),
            Err(why) => return (Answer::Refused(why), now),
        };
        if let Err(answer) = self.check_password(files, &user.name, password, peer, now) {
            return answer;
        }
        let minutes = HELP_ELEVATE_MINUTES;
        let help = &mut self.helps[at];
        help.elevate_until = Some(now + Duration::from_secs(u64::from(minutes) * 60));
        let until = util::local_time((unix_now() + u64::from(minutes) * 60) as i64, "%H:%M");
        help.note("elevate", &format!("{} let elevate run until {until}", user.name));
        let id = help.id.clone();
        audit!("{} let elevate run in help session {id} until {until}", user.name);
        self.tell_owner(&id, &HelpEvent::Elevate { minutes }, now);
        (Answer::Done(minutes.to_string()), now)
    }

    /// Gives each session the warnings its time has come to, and ends
    /// those whose time is up.
    pub(super) fn help_tick(&mut self, now: Instant) {
        let mut at = 0;
        while at < self.helps.len() {
            if self.helps[at].ends <= now {
                self.end_help(at, HelpEnded::Deadline, now);
                continue;
            }
            if self.helps[at].next() <= now {
                let help = &mut self.helps[at];
                let minutes = WARNINGS[help.warned];
                help.warned += 1;
                let id = help.id.clone();
                self.tell_owner(&id, &HelpEvent::Left { minutes }, now);
            }
            at += 1;
        }
    }

    /// Ends the session whose help screen's shell, process `number`,
    /// ended: whether it was one.
    pub(super) fn help_shell_ended(&mut self, number: i32, now: Instant) -> bool {
        match self.helps.iter().position(|help| help.shell == number) {
            Some(at) => {
                self.end_help(at, HelpEnded::Session, now);
                true
            }
            None => false,
        }
    }

    /// Ends the sessions whose help-session connection closed.
    pub(super) fn end_orphaned_helps(&mut self, now: Instant) {
        let orphaned: Vec<String> =
            self.clients.iter().filter(|c| c.done).filter_map(|c| c.help.clone()).collect();
        for id in orphaned {
            if let Some(at) = self.helps.iter().position(|help| help.id == id) {
                self.end_help(at, HelpEnded::Owner, now);
            }
        }
    }

    /// Ends session `at`, for `why`: every process of its help screen,
    /// the helper's connection, and SSH if it started it; the owner is
    /// told, and the transcript and the audit log say so.
    pub(super) fn end_help(&mut self, at: usize, why: HelpEnded, now: Instant) {
        let mut help = self.helps.remove(at);
        let _ = resource_domain::stop(&SYS, help.domain());
        // SAFETY: shutting down our own descriptor's socket, which sshd's
        // copy shares: its relay reads the end.
        unsafe { libc::shutdown(help.socket.as_raw_fd(), libc::SHUT_RDWR) };
        help.note("end", why.word());
        let id = help.id.clone();
        let helped = match &help.helper {
            Some(helper) => format!(
                "; the helper from {} with {} changed what the transcript shows, which stays",
                helper.address, helper.with
            ),
            None => "; no helper signed in".into(),
        };
        audit!("help session {id} of {} ended ({}){helped}", help.owner_name, why.word());
        if help.opened_ssh && !self.helps.iter().any(|other| other.opened_ssh) {
            stop_sshd();
        }
        self.mark_waiting();
        let mut list = kept();
        if let Some(kept) = list.iter_mut().find(|kept| kept.id == id) {
            kept.ended = unix_now() as i64;
        }
        keep_index(&list);
        let line = HelpEvent::Ended(why).line();
        for client in self.clients.iter_mut().filter(|c| c.help.as_deref() == Some(id.as_str())) {
            client.tell(line.as_bytes(), now);
            // Once told, it closes.
            client.help = None;
            client.deadline = now + super::REQUEST_TIME;
        }
    }
}

/// Stops sshd, which a help session started, and lets it take everyone
/// again at its next start.
fn stop_sshd() {
    if let Err(fail) = power::request("stop sshd") {
        eprintln!("authd: cannot stop sshd after a help session: {}", fail.message);
    }
    let _ = fs::remove_file(HELP_ONLY);
}

/// The transcripts, as `help-log` asks: `peer`'s own, or every one
/// for root; with `id`, that one's lines.
pub(super) fn help_log(files: &Files, id: Option<&str>, peer: Peer) -> Answer {
    let name = files.users.entries().find(|u| u.uid == peer.user).map(|u| u.name.clone());
    let mine = |kept: &HelpKept| peer.is_root() || name.as_deref() == Some(kept.owner.as_str());
    let list: Vec<HelpKept> = kept().into_iter().filter(|kept| mine(kept)).collect();
    let Some(id) = id else {
        let words: Vec<String> = list.iter().map(HelpKept::words).collect();
        return Answer::Done(words.join("\t"));
    };
    if !list.iter().any(|kept| kept.id == id) {
        return Answer::Refused(format!("there is no help session {} of yours", shown(id)));
    }
    match fs::read(transcript_path(id)) {
        Ok(bytes) => {
            let text = String::from_utf8_lossy(&bytes);
            Answer::Done(text.lines().collect::<Vec<_>>().join("\t"))
        }
        Err(err) => Answer::Refused(format!("cannot read the transcript of {id}: {err}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_transcript_keeps_one_line_for_each_and_shows_what_is_not_printable() {
        assert_eq!(transcript_line(12, "typed", "ls -l"), "12 typed ls -l\n");
        assert_eq!(printable(b"a\tb\x1b[A\x7f\\\xc3\xa9"), "a^Ib^[[A^?\\\\\\xc3\\xa9");
        assert_eq!(printable(b"line\r\nnext"), "line^M^Jnext");
    }

    #[test]
    fn a_code_is_the_same_only_when_all_of_it_is() {
        assert!(same_code("KQ7RM2XW9HTP4BZE", "KQ7RM2XW9HTP4BZE"));
        assert!(!same_code("KQ7RM2XW9HTP4BZF", "KQ7RM2XW9HTP4BZE"));
        assert!(!same_code("KQ7RM2XW9HTP4BZ", "KQ7RM2XW9HTP4BZE"));
        assert!(!same_code("", "KQ7RM2XW9HTP4BZE"));
    }

    #[test]
    fn letters_are_chosen_evenly_from_the_alphabet() {
        let all: Vec<u8> = (0..=255).collect();
        let code = letters_from(&all, HELP_CODE_LETTERS);
        for letter in HELP_CODE_LETTERS {
            assert_eq!(code.bytes().filter(|b| b == letter).count(), 8);
        }
        assert!(request::help_code(&letters_from(&all[..16], HELP_CODE_LETTERS)).is_some());
        let id = letters_from(&all[..ID_LENGTH], ID_LETTERS);
        assert!(id.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit()));
    }

    fn kept(id: &str, owner: &str, started: i64, ended: i64) -> HelpKept {
        HelpKept { id: id.into(), owner: owner.into(), started, ended }
    }

    #[test]
    fn an_owner_keeps_the_newest_twenty_transcripts_and_others_keep_theirs() {
        let mut list: Vec<HelpKept> =
            (0..KEPT as i64).map(|n| kept(&format!("a{n}"), "alice", n, n + 1)).collect();
        list.insert(3, kept("b0", "bob", 2, 3));
        let (left, gone) = pruned(list.clone(), "alice");
        assert_eq!(gone, ["a0"]);
        assert_eq!(left.len(), KEPT);
        assert!(left.iter().any(|k| k.id == "b0"));
        // Bob's next one leaves all of Alice's.
        let (left, gone) = pruned(list, "bob");
        assert!(gone.is_empty());
        assert_eq!(left.len(), KEPT + 1);
    }

    #[test]
    fn a_session_still_running_is_never_pruned() {
        let mut list: Vec<HelpKept> =
            (0..KEPT as i64).map(|n| kept(&format!("a{n}"), "alice", n, n + 1)).collect();
        list[0].ended = 0;
        let (_, gone) = pruned(list, "alice");
        assert_eq!(gone, ["a1"]);
    }
}
