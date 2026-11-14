// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The web console's sign-ins and second steps (ADR 0514). For admind(8),
//! which runs as root: signing people in with a password and a second
//! step, or with a passkey alone; starting each session's worker as the
//! person; unlocking a session, and running its elevated commands; and
//! checking API tokens. For each person, from their worker or from
//! `admin` at a terminal: their own second steps and tokens.
//!
//! Each account's are in one file, `/system/state/authd/console/NAME.toml`,
//! in authd's own folder, which no other program's view holds:
//!
//! ```toml
//! handle = "9f86d081884c7d659a2feaa0c55ad015"   # the passkeys' user handle
//!
//! [app]
//! secret = "JBSWY3DPEHPK3PXPJBSWY3DPEHPK3PXP"    # base32, 20 bytes
//! added = 1794500000
//! last-step = 59816666                          # the last 30 s step taken
//!
//! [[passkeys]]
//! id = "..."                                    # its credential ID, hex
//! key = "..."                                   # its COSE_Key, hex
//! algorithm = -7
//! relying-party = "duke-nas.local"
//! name = "YubiKey 5C"
//! count = 41
//! added = 1794500000
//! used = 1794560000
//!
//! [recovery]
//! codes = ["...", "..."]                        # SHA-256 of each unused one
//! made = 1794500000
//!
//! [[trusted]]                                   # "Trust this browser"
//! hash = "..."                                  # SHA-256 of its token
//! until = 1797092000
//! from = "Firefox on Linux"
//!
//! [[tokens]]
//! name = "ansible"
//! hash = "..."                                  # SHA-256 of its secret part
//! scopes = ["*"]
//! until = 1797092000
//! made = 1794500000
//! used = 1794560000
//! ```
//!
//! Times are Unix seconds. The app's secret is the one kept as it is, as
//! RFC 6238 makes each code from it; the rest are hashes and public keys.
//! Sign-ins under way, challenges, apps being added and the sessions'
//! unlocks are in authd's memory alone: a restart forgets them, and the
//! console signs in again.
//!
//! Where to start when it misbehaves: authd's audit lines name each
//! sign-in, second step, unlock and elevated command, with how it was
//! proved, and each refusal with its reason; `user set --console` lets a
//! person who is not an administrator in; the `admin` boot test covers
//! each request.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::fs;
use std::io;
use std::os::fd::OwnedFd;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use accounts::User;
use accounts::console::{Assertion, Console, Elevator, Proof, Purpose, from_hex};
use accounts::request::{self, Answer, Mode, hex};
use accounts::signins::{self, Event, Seat};
use ring::digest;
use ring::rand::{SecureRandom, SystemRandom};
use serde::Deserialize;
use serde_json::json;

use super::proofs::{self, Ceremony, CodeRefusal, Expected};
use super::{
    Authd, Files, Peer, Session, Wrong, record, replace, shown, signal_command, wait_after,
};
use crate::admin_settings::{SecondStep, Settings};
use crate::{elevate, signin, util};

/// The folder of the accounts' files, inside authd's own.
pub const FOLDER: &str = "/system/state/authd/console";

/// The terminal a console session's sign-in records name, as last(1)
/// shows them.
pub const TERMINAL: &str = "web";

/// What the audit log names an API token's worker by.
const API: &str = "web console API";

/// How long a sign-in under way, and a passkey challenge, last.
const SIGN_IN_TIME: Duration = Duration::from_mins(5);

/// The most sign-ins under way, and challenges, kept: the oldest goes
/// first.
const SIGN_INS_MAX: usize = 128;
const CHALLENGES_MAX: usize = 128;

/// How long an app being added waits for its first code.
const APP_TIME: Duration = Duration::from_mins(10);

/// The most console sessions at once.
const LIVE_MAX: usize = 64;

/// A day, and how long "Trust this browser" lasts.
const DAY: i64 = 86_400;
const TRUSTED_FOR: i64 = 30 * DAY;

/// The most of each an account keeps.
const TRUSTED_MAX: usize = 16;
pub(crate) const PASSKEYS_MAX: usize = 16;
pub(crate) const TOKENS_MAX: usize = 32;

/// The longest an API token lasts, in days.
const TOKEN_DAYS_MAX: u32 = 366;

/// An expired token is listed this long, then forgotten.
const EXPIRED_KEPT: i64 = 30 * DAY;

/// The recovery codes made at once.
const RECOVERY_CODES: usize = 10;

/// The start of every API token: `dkv_NAME_SECRET`.
pub const TOKEN_PREFIX: &str = "dkv_";

/// The random bytes of a token's secret part, written in hex.
const TOKEN_SECRET: usize = 32;

/// How often a token's `used` is written, so that a busy one does not
/// write the file on each call.
const USED_EVERY: i64 = 3600;

/// The refusals said in more than one place.
const NO_SIGN_IN: &str =
    "that sign-in is not one under way, or began more than 5 minutes ago: sign in again";
const NO_CHALLENGE: &str = "that passkey challenge is not one authd gave, or was used, or is \
                            older than 5 minutes: try again";
const NO_SESSION: &str = "that web console session has ended: sign in again";
const NOT_A_NAME: &str = "passkeys work when the console is opened by the machine's name, such \
                          as duke-nas.local, not by its address";
const NOT_FOR_THIS: &str = "that challenge was given for something else";
const NO_STEP_NEEDED: &str = "that sign-in needs no second step";

/// authd's memory of the web console.
#[derive(Default)]
pub(super) struct State {
    /// The sign-ins under way, by their tickets.
    sign_ins: HashMap<Vec<u8>, SignIn>,
    /// The passkey challenges given out, by their bytes.
    challenges: HashMap<Vec<u8>, Challenge>,
    /// The apps being added, by their owners' numbers.
    apps: HashMap<u32, NewApp>,
    /// The console sessions signed in, by their IDs.
    live: HashMap<String, Live>,
}

/// A sign-in under way: whose, from where, for which admind, and how far.
struct SignIn {
    name: String,
    from: String,
    process: u64,
    given: Instant,
    stage: Stage,
}

/// How far a sign-in is.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Stage {
    /// The password was right; a second step of these kinds is to come,
    /// as `begin` answers them.
    Step(String),
    /// The password was right, and the sign-in rules ask for a second
    /// step the account has not got: the session may only add one.
    NeedsStep,
    /// Done, and how, for the audit log: `password and app code`.
    Done(String),
}

/// What a passkey challenge was given for.
#[derive(Clone, Debug, PartialEq, Eq)]
enum For {
    /// The second step of the sign-in with this ticket.
    SignIn(Vec<u8>),
    /// A sign-in with a passkey alone, from this address.
    Alone(String),
    /// Unlocking the session with this ID.
    Unlock(String),
    /// Adding a passkey to this account.
    Adding(String),
}

/// A passkey challenge given out.
struct Challenge {
    purpose: For,
    relying_party: String,
    /// The process it was given to, which alone may answer it.
    process: u64,
    given: Instant,
}

/// An authenticator app being added: its secret, until its first code
/// comes from the process that began it.
struct NewApp {
    secret: [u8; proofs::SECRET_BYTES],
    process: u64,
    given: Instant,
}

/// A console session signed in.
struct Live {
    name: String,
    number: u32,
    from: String,
    /// Its worker's process.
    worker: i32,
    started: Instant,
    /// Until when it is unlocked.
    unlocked: Option<Instant>,
    /// Whether it may only add a second step.
    needs_step: bool,
}

impl State {
    /// Forgets the sign-ins, challenges and apps being added that are too
    /// old.
    pub(super) fn tidy(&mut self, now: Instant) {
        self.sign_ins.retain(|_, s| now.saturating_duration_since(s.given) < SIGN_IN_TIME);
        self.challenges.retain(|_, c| now.saturating_duration_since(c.given) < SIGN_IN_TIME);
        self.apps.retain(|_, a| now.saturating_duration_since(a.given) < APP_TIME);
    }

    /// Forgets the console session whose worker, `process`, ended.
    pub(super) fn worker_ended(&mut self, process: i32) {
        self.live.retain(|_, live| live.worker != process);
    }

    /// The account and source whose wrong tries hold `asked` back, as
    /// [`Authd::take`] asks for a request that checks a password or a
    /// code: `asker` is the account of the person who asks.
    pub(super) fn turn(
        &self,
        asked: &Console,
        peer: Peer,
        asker: &str,
    ) -> Option<(String, String)> {
        match asked {
            Console::Begin { from, name, .. } => Some((name.clone(), web_source(from))),
            Console::Code { ticket, .. } => {
                self.sign_ins.get(ticket).map(|s| (s.name.clone(), web_source(&s.from)))
            }
            Console::Unlock { session, proof: Proof::Password(_) | Proof::Code(_) } => {
                self.live.get(session).map(|l| (l.name.clone(), web_source(&l.from)))
            }
            Console::Token { token, from } => {
                Some((token_owner(token).map_or("", |(owner, _)| owner).into(), web_source(from)))
            }
            Console::AppBegin { proof, .. }
            | Console::AppRemove { proof }
            | Console::PasskeyBegin { proof, .. }
            | Console::PasskeyRemove { proof, .. }
            | Console::Codes { proof }
            | Console::TokenAdd { proof, .. }
                if !proof.is_empty() =>
            {
                Some((asker.into(), peer.source()))
            }
            _ => None,
        }
    }

    /// The sign-in with `ticket`, if it is `peer`'s and not too old.
    fn sign_in(&mut self, ticket: &[u8], peer: Peer, now: Instant) -> Option<&mut SignIn> {
        self.sign_ins.get_mut(ticket).filter(|sign_in| {
            sign_in.process == peer.process
                && now.saturating_duration_since(sign_in.given) < SIGN_IN_TIME
        })
    }

    /// Keeps `sign_in`: its new ticket.
    fn add_sign_in(&mut self, sign_in: SignIn) -> Result<Vec<u8>, String> {
        let ticket = random::<32>()?.to_vec();
        oldest_out(&mut self.sign_ins, SIGN_INS_MAX, |s| s.given);
        self.sign_ins.insert(ticket.clone(), sign_in);
        Ok(ticket)
    }

    /// Keeps `challenge`: its bytes.
    fn add_challenge(&mut self, challenge: Challenge) -> Result<Vec<u8>, String> {
        let bytes = random::<32>()?.to_vec();
        oldest_out(&mut self.challenges, CHALLENGES_MAX, |c| c.given);
        self.challenges.insert(bytes.clone(), challenge);
        Ok(bytes)
    }

    /// Takes the challenge `bytes` out, used once, right or wrong: if it
    /// was given to `peer`, and is not too old.
    fn take_challenge(&mut self, bytes: &[u8], peer: Peer, now: Instant) -> Option<Challenge> {
        let challenge = self.challenges.remove(bytes)?;
        let fresh = now.saturating_duration_since(challenge.given) < SIGN_IN_TIME;
        (challenge.process == peer.process && fresh).then_some(challenge)
    }
}

/// Takes the oldest out of `map` until there is room for one more.
fn oldest_out<V>(map: &mut HashMap<Vec<u8>, V>, max: usize, given: impl Fn(&V) -> Instant) {
    while map.len() >= max {
        let Some(oldest) = map.iter().min_by_key(|(_, v)| given(v)).map(|(k, _)| k.clone()) else {
            return;
        };
        map.remove(&oldest);
    }
}

/// An account's file.
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
struct Store {
    #[serde(default)]
    handle: String,
    app: Option<App>,
    #[serde(default)]
    passkeys: Vec<Passkey>,
    recovery: Option<Recovery>,
    #[serde(default)]
    trusted: Vec<Trusted>,
    #[serde(default)]
    tokens: Vec<Token>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
struct App {
    secret: String,
    added: i64,
    last_step: Option<u64>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
struct Passkey {
    id: String,
    key: String,
    algorithm: i64,
    relying_party: String,
    name: String,
    count: u32,
    added: i64,
    used: i64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
struct Recovery {
    codes: Vec<String>,
    made: i64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
struct Trusted {
    hash: String,
    until: i64,
    from: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
struct Token {
    name: String,
    hash: String,
    scopes: Vec<String>,
    until: i64,
    made: i64,
    used: i64,
}

impl Store {
    fn path(name: &str) -> String {
        format!("{FOLDER}/{name}.toml")
    }

    /// `name`'s file, or an empty one when it has none.
    fn read(name: &str) -> Result<Store, String> {
        accounts::check_name(name)?;
        let path = Store::path(name);
        match fs::read_to_string(&path) {
            Ok(text) => Store::parse(&text).map_err(|why| format!("{path}: {why}")),
            Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(Store::default()),
            Err(err) => Err(format!("cannot read {path}: {}", util::strip_os_error(&err))),
        }
    }

    fn parse(text: &str) -> Result<Store, String> {
        toml::from_str(text).map_err(|err| err.message().to_string())
    }

    /// Writes `name`'s file anew, or removes it once it holds nothing.
    fn write(&self, name: &str) -> Result<(), String> {
        let path = Store::path(name);
        if !self.is_empty() {
            return replace(&path, &self.text(), 0o600);
        }
        match fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(err) => Err(format!("cannot remove {path}: {}", util::strip_os_error(&err))),
        }
    }

    fn is_empty(&self) -> bool {
        self.handle.is_empty()
            && self.app.is_none()
            && self.passkeys.is_empty()
            && self.recovery.is_none()
            && self.trusted.is_empty()
            && self.tokens.is_empty()
    }

    /// The file's text.
    fn text(&self) -> String {
        let mut text = String::new();
        if !self.handle.is_empty() {
            let _ = writeln!(text, "handle = {}", quoted(&self.handle));
        }
        if let Some(app) = &self.app {
            let _ =
                write!(text, "\n[app]\nsecret = {}\nadded = {}\n", quoted(&app.secret), app.added);
            if let Some(step) = app.last_step {
                let _ = writeln!(text, "last-step = {step}");
            }
        }
        for passkey in &self.passkeys {
            let _ = write!(
                text,
                "\n[[passkeys]]\nid = {}\nkey = {}\nalgorithm = {}\nrelying-party = {}\nname = {}\n\
                 count = {}\nadded = {}\nused = {}\n",
                quoted(&passkey.id),
                quoted(&passkey.key),
                passkey.algorithm,
                quoted(&passkey.relying_party),
                quoted(&passkey.name),
                passkey.count,
                passkey.added,
                passkey.used
            );
        }
        if let Some(recovery) = &self.recovery {
            let codes: Vec<String> = recovery.codes.iter().map(|code| quoted(code)).collect();
            let _ = write!(
                text,
                "\n[recovery]\ncodes = [{}]\nmade = {}\n",
                codes.join(", "),
                recovery.made
            );
        }
        for trusted in &self.trusted {
            let _ = write!(
                text,
                "\n[[trusted]]\nhash = {}\nuntil = {}\nfrom = {}\n",
                quoted(&trusted.hash),
                trusted.until,
                quoted(&trusted.from)
            );
        }
        for token in &self.tokens {
            let scopes: Vec<String> = token.scopes.iter().map(|scope| quoted(scope)).collect();
            let _ = write!(
                text,
                "\n[[tokens]]\nname = {}\nhash = {}\nscopes = [{}]\nuntil = {}\nmade = {}\n\
                 used = {}\n",
                quoted(&token.name),
                quoted(&token.hash),
                scopes.join(", "),
                token.until,
                token.made,
                token.used
            );
        }
        text
    }

    /// Whether it has a second step: an app or a passkey. Recovery codes
    /// come with one, and are none by themselves.
    fn has_step(&self) -> bool {
        self.app.is_some() || !self.passkeys.is_empty()
    }

    /// The kinds of second step it takes, as `begin` answers them.
    fn kinds(&self) -> String {
        let mut kinds = Vec::new();
        if self.app.is_some() {
            kinds.push("app");
        }
        if !self.passkeys.is_empty() {
            kinds.push("passkey");
        }
        if self.recovery.as_ref().is_some_and(|r| !r.codes.is_empty()) {
            kinds.push("recovery");
        }
        kinds.join(",")
    }

    /// The IDs of its passkeys for `relying_party`, in hex.
    fn passkey_ids(&self, relying_party: &str) -> Vec<String> {
        let theirs = self.passkeys.iter().filter(|p| p.relying_party == relying_party);
        theirs.map(|p| p.id.clone()).collect()
    }

    /// Makes new recovery codes in place of those there were: the codes,
    /// to show once.
    fn new_codes(&mut self, unix: i64) -> Result<Vec<String>, String> {
        let mut codes = Vec::with_capacity(RECOVERY_CODES);
        let mut hashes = Vec::with_capacity(RECOVERY_CODES);
        for _ in 0..RECOVERY_CODES {
            let code = proofs::recovery_code(&random::<10>()?);
            let hash = proofs::recovery_hash(&code).ok_or("a recovery code made does not read")?;
            hashes.push(hex(&hash));
            codes.push(code);
        }
        self.recovery = Some(Recovery { codes: hashes, made: unix });
        Ok(codes)
    }

    /// After a second step went: with none left, the recovery codes go
    /// too.
    fn tidy_steps(&mut self) {
        if !self.has_step() {
            self.recovery = None;
        }
    }

    /// Takes `typed`, an app's code, or with `recovery` a recovery code
    /// too, at `unix`: how it was proved, for the audit log, or why not.
    /// A recovery code is used once.
    fn take_code(&mut self, typed: &str, unix: i64, recovery: bool) -> Result<String, CodeRefusal> {
        let plain = typed.trim();
        if plain.len() == proofs::DIGITS && plain.bytes().all(|b| b.is_ascii_digit()) {
            let Some(app) = &mut self.app else { return Err(CodeRefusal::Wrong) };
            let mut secret = proofs::from_base32(&app.secret).ok_or(CodeRefusal::Wrong)?;
            let taken = proofs::check_app_code(&secret, plain, unix as u64, app.last_step);
            accounts::wipe(&mut secret);
            app.last_step = Some(taken?);
            return Ok("app code".into());
        }
        if !recovery {
            return Err(CodeRefusal::NotACode);
        }
        let hash = hex(&proofs::recovery_hash(plain).ok_or(CodeRefusal::NotACode)?);
        let Some(kept) = &mut self.recovery else { return Err(CodeRefusal::Wrong) };
        let found =
            kept.codes.iter().position(|code| proofs::same(code.as_bytes(), hash.as_bytes()));
        kept.codes.remove(found.ok_or(CodeRefusal::Wrong)?);
        Ok(format!("a recovery code, {} left", kept.codes.len()))
    }

    /// Checks `assertion` against the passkey it names, which must be
    /// one of this account's for its relying party, and keeps its new
    /// count: its name, or why not. With `verified`, the authenticator
    /// must have checked the person, by a PIN or their fingerprint.
    fn use_passkey(
        &mut self,
        assertion: &Assertion,
        verified: bool,
        unix: i64,
    ) -> Result<String, String> {
        let id = hex(&assertion.credential);
        let party = &assertion.relying_party;
        let Some(passkey) =
            self.passkeys.iter_mut().find(|p| p.id == id && p.relying_party == *party)
        else {
            return Err(format!("that passkey is not one of this account's for {}", shown(party)));
        };
        let key = from_hex(passkey.key.as_bytes())
            .ok_or("the passkey's key in authd's file does not read")?;
        let expected = Expected {
            ceremony: Ceremony::Get,
            challenge: &assertion.challenge,
            origin: &assertion.origin,
            relying_party: party,
            verified,
        };
        let used = proofs::check_assertion(
            &expected,
            &assertion.client_data,
            &assertion.authenticator_data,
            &assertion.signature,
            &key,
            passkey.count,
        )?;
        passkey.count = used.count;
        passkey.used = unix;
        Ok(passkey.name.clone())
    }

    /// Whether a browser with `token` is trusted at `unix`.
    fn trusts(&self, token: &[u8], unix: i64) -> bool {
        let hash = hex(&sha256(token));
        self.trusted
            .iter()
            .any(|t| t.until > unix && proofs::same(t.hash.as_bytes(), hash.as_bytes()))
    }

    /// Trusts the browser with `token`, described by `from`, for 30 days.
    fn trust(&mut self, token: &[u8], from: &str, unix: i64) {
        self.trusted.retain(|t| t.until > unix);
        while self.trusted.len() >= TRUSTED_MAX {
            let Some(soonest) = (0..self.trusted.len()).min_by_key(|&i| self.trusted[i].until)
            else {
                break;
            };
            self.trusted.remove(soonest);
        }
        let hash = hex(&sha256(token));
        self.trusted.push(Trusted { hash, until: unix + TRUSTED_FOR, from: shown(from) });
    }

    /// The token whose secret part is `secret`, if it has not expired.
    fn token(&mut self, secret: &str, unix: i64) -> Option<&mut Token> {
        let hash = hex(&sha256(secret.as_bytes()));
        self.tokens
            .iter_mut()
            .find(|t| t.until > unix && proofs::same(t.hash.as_bytes(), hash.as_bytes()))
    }
}

/// Where a sign-in is, once the password is right: a second step to come
/// when the account has one, or when the sign-in rules ask for one it has
/// not got, a session that may only add one; else done.
fn stage_for(rule: SecondStep, admin: bool, store: &Store) -> Stage {
    if store.has_step() {
        Stage::Step(store.kinds())
    } else if rules_need_step(rule, admin) {
        Stage::NeedsStep
    } else {
        Stage::Done("password".into())
    }
}

/// Whether the sign-in rules ask an account for a second step.
fn rules_need_step(rule: SecondStep, admin: bool) -> bool {
    match rule {
        SecondStep::IfSet => false,
        SecondStep::Administrators => admin,
        SecondStep::Everyone => true,
    }
}

/// Where a browser's tries come from, for slowing them down: its
/// address.
fn web_source(from: &str) -> String {
    format!("web {from}")
}

/// `from`, if it can be a browser's address as admind gives it.
fn address(from: &str) -> Option<&str> {
    let fine = !from.is_empty() && from.len() <= 64 && from.bytes().all(|b| b.is_ascii_graphic());
    fine.then_some(from)
}

/// Whether `name` can be a relying party: a DNS name, as WebAuthn needs,
/// never an address. A name's last label has a letter, so no IPv4
/// address is one, and no name has a colon, as IPv6 addresses do.
fn relying_party_ok(name: &str) -> bool {
    let label = |label: &str| {
        (1..=63).contains(&label.len())
            && label.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
            && !label.starts_with('-')
            && !label.ends_with('-')
    };
    name.len() <= 253
        && name.split('.').all(label)
        && name.rsplit('.').next().is_some_and(|last| last.bytes().any(|b| b.is_ascii_lowercase()))
}

/// Whether `name` can name a passkey, or a machine to an app: 1 to 64
/// characters, not all spaces, with no control characters.
fn label_ok(name: &str) -> bool {
    !name.trim().is_empty() && name.chars().count() <= 64 && !name.chars().any(char::is_control)
}

/// Whether `name` can name an API token: 1 to 32 of `a` to `z`, `0` to
/// `9` and `-`.
fn token_name_ok(name: &str) -> bool {
    (1..=32).contains(&name.len())
        && name.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// Whether `scope` can be an API token's: `*`, everything its owner may
/// do, or an area's name, alone or with `:read` for its reading alone.
fn scope_ok(scope: &str) -> bool {
    let area = scope.strip_suffix(":read").unwrap_or(scope);
    scope == "*"
        || ((1..=32).contains(&area.len())
            && area.starts_with(|c: char| c.is_ascii_lowercase())
            && area.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-'))
}

/// The most arguments, and bytes of them, of a console's elevated
/// command.
const ARGUMENTS_MAX: usize = 64;
const COMMAND_BYTES_MAX: usize = 32 * 1024;

/// The folders a console's elevated command is found in.
const COMMAND_FOLDERS: [&str; 2] = ["/system/commands/", "/applications/commands/"];

/// The shells, which the console never runs: it runs commands, never
/// text.
const SHELLS: [&[u8]; 7] = [b"nukesh", b"sh", b"bash", b"dash", b"zsh", b"ksh", b"fish"];

/// Why the web console may not run `command`, found at `program`, as an
/// administrator, if it may not.
fn console_command_refusal(command: &[Vec<u8>], program: &[u8]) -> Option<String> {
    if command.len() > ARGUMENTS_MAX + 1 {
        return Some(format!("a command of the console's has at most {ARGUMENTS_MAX} arguments"));
    }
    if command.iter().map(Vec::len).sum::<usize>() > COMMAND_BYTES_MAX {
        return Some("a command of the console's is at most 32 KiB".into());
    }
    let base = program.rsplit(|&b| b == b'/').next().unwrap_or_default();
    if SHELLS.contains(&base) {
        return Some("the web console runs commands, never a shell".into());
    }
    let program = String::from_utf8_lossy(program);
    // Each part of the path a name: no `.`, no `..`, no `//`.
    let plain = program.split('/').skip(1).all(|part| !matches!(part, "" | "." | ".."));
    if !plain || !COMMAND_FOLDERS.iter().any(|folder| program.starts_with(folder)) {
        return Some(format!(
            "the web console runs the system's and the apps' commands alone, not {program}"
        ));
    }
    None
}

/// The account and secret part of an API token, `dkv_NAME_SECRET`.
fn token_owner(token: &[u8]) -> Option<(&str, &str)> {
    let rest = std::str::from_utf8(token).ok()?.strip_prefix(TOKEN_PREFIX)?;
    let (owner, secret) = rest.rsplit_once('_')?;
    let hex_digits = secret.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'));
    (accounts::check_name(owner).is_ok() && secret.len() == TOKEN_SECRET * 2 && hex_digits)
        .then_some((owner, secret))
}

/// What an API token gives: its owner, its name, its scopes and until
/// when it lasts.
struct Given {
    owner: String,
    name: String,
    scopes: Vec<String>,
    until: i64,
}

/// What `token` gives, if it is an API token that has not expired; its
/// use is recorded at most once an hour.
fn find_token(token: &[u8], unix: i64) -> Result<Given, String> {
    let wrong = || String::from("that API token is not one of this machine's, or has expired");
    let (owner, secret) = token_owner(token).ok_or_else(wrong)?;
    let mut store = Store::read(owner)?;
    let found = store.token(secret, unix).ok_or_else(wrong)?;
    let given = Given {
        owner: owner.into(),
        name: found.name.clone(),
        scopes: found.scopes.clone(),
        until: found.until,
    };
    if unix - found.used >= USED_EVERY {
        found.used = unix;
        if let Err(why) = store.write(owner) {
            eprintln!("authd: {why}");
        }
    }
    Ok(given)
}

/// The account whose passkeys' user handle is `handle`.
fn owner_of_handle(handle: &str) -> Option<String> {
    if handle.is_empty() {
        return None;
    }
    let entries = fs::read_dir(FOLDER).ok()?;
    let names = entries.flatten().filter_map(|entry| {
        entry.file_name().to_str().and_then(|file| file.strip_suffix(".toml")).map(String::from)
    });
    names.into_iter().find(|name| Store::read(name).is_ok_and(|store| store.handle == handle))
}

/// Whether `user` may sign in to the web console now, or why not.
fn may_sign_in(files: &Files, user: &User) -> Result<(), String> {
    if user.uid == 0 {
        return Err("root does not sign in to the web console: sign in as an administrator, who \
                    makes changes there once the session is unlocked"
            .into());
    }
    if files.disabled(&user.name) {
        return Err(format!("the account {} is disabled", user.name));
    }
    if !accounts::may_use_console(user, &files.groups) {
        return Err(format!(
            "{} may not use the web console: an administrator lets them with `elevate user set \
             --console {}`",
            user.name, user.name
        ));
    }
    Ok(())
}

/// Records a wrong password or code for `name` from `from` at the web
/// console, for lastb(1), and for the person, who is told at their next
/// sign-in (ADR 0254).
fn failed(files: &Files, name: &str, from: &str) {
    let event = Event::Failed { name: shown(name), terminal: TERMINAL.into(), from: shown(from) };
    match files.user(name) {
        Some(user) => record(&signins::of(user.uid), user.gid, 0o640, event),
        None => record(signins::UNKNOWN, 0, 0o600, event),
    }
}

/// Moves `name`'s file to `new`'s, as the account is renamed. Its API
/// tokens, which name the account, end.
pub(super) fn renamed(name: &str, new: &str) -> Result<(), String> {
    let mut store = Store::read(name)?;
    if store.is_empty() {
        return Ok(());
    }
    store.tokens.clear();
    store.write(new)?;
    Store::default().write(name)
}

/// Removes `name`'s file, as the account goes.
pub(super) fn removed(name: &str) {
    if let Err(why) = Store::default().write(name) {
        eprintln!("authd: {why}");
    }
}

/// The JSON `steps` answers: an account's second steps and tokens, with
/// none of their secrets, and whether the sign-in rules ask for a step.
fn steps(store: &Store, needed: bool, unix: i64) -> String {
    let passkeys: Vec<_> = store
        .passkeys
        .iter()
        .map(|p| {
            json!({
                "id": p.id,
                "relying-party": p.relying_party,
                "name": p.name,
                "added": p.added,
                "used": p.used,
            })
        })
        .collect();
    let tokens: Vec<_> = store
        .tokens
        .iter()
        .map(|t| {
            json!({
                "name": t.name,
                "scopes": t.scopes,
                "until": t.until,
                "made": t.made,
                "used": t.used,
                "expired": t.until <= unix,
            })
        })
        .collect();
    json!({
        "app": store.app.as_ref().map(|app| json!({ "added": app.added })),
        "passkeys": passkeys,
        "recovery": store.recovery.as_ref().map(|r| json!({ "left": r.codes.len(), "made": r.made })),
        "trusted": store.trusted.iter().filter(|t| t.until > unix).count(),
        "tokens": tokens,
        "needed": needed,
    })
    .to_string()
}

/// A refusal, now.
fn no(why: impl Into<String>, now: Instant) -> (Answer, Instant) {
    (Answer::Refused(why.into()), now)
}

/// The start of a session's ID, as the audit log shows it.
fn short(session: &str) -> String {
    format!("{}…", session.chars().take(8).collect::<String>())
}

/// Unix seconds, now.
fn unix_now() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64)
}

/// Unix seconds at `until`, an instant from `now`.
fn unix_at(now: Instant, until: Instant) -> i64 {
    unix_now() + until.saturating_duration_since(now).as_secs() as i64
}

/// `unix` as a day, as the audit log says when a token ends.
fn day(unix: i64) -> String {
    util::local_time(unix, "%Y-%m-%d")
}

fn sha256(bytes: &[u8]) -> [u8; 32] {
    let mut out = [0u8; 32];
    out.copy_from_slice(digest::digest(&digest::SHA256, bytes).as_ref());
    out
}

/// `N` bytes from the random source.
fn random<const N: usize>() -> Result<[u8; N], String> {
    let mut bytes = [0u8; N];
    SystemRandom::new().fill(&mut bytes).map_err(|_| String::from("no random bytes to be had"))?;
    Ok(bytes)
}

/// `text` as a TOML string, in quotes.
fn quoted(text: &str) -> String {
    let mut quoted = String::from("\"");
    for character in text.chars() {
        match character {
            '"' => quoted.push_str("\\\""),
            '\\' => quoted.push_str("\\\\"),
            control if control.is_control() => {
                let _ = write!(quoted, "\\u{:04X}", u32::from(control));
            }
            other => quoted.push(other),
        }
    }
    quoted.push('"');
    quoted
}

impl Authd {
    /// Carries out `asked`, client `i`'s, from `peer`: admind's requests
    /// from root alone, the person's from the person.
    pub(super) fn console(
        &mut self,
        i: usize,
        files: &Files,
        asked: &Console,
        peer: Peer,
        now: Instant,
    ) -> (Answer, Instant) {
        if !asked.is_admind() {
            return self.console_person(files, asked, peer, now);
        }
        if !peer.is_root() {
            return no("only admind, which runs as root, asks this of authd", now);
        }
        match asked {
            Console::Begin { from, name, password } => {
                self.console_begin(files, from, name, password, peer, now)
            }
            Console::Trusted { ticket, token } => self.console_trusted(ticket, token, peer, now),
            Console::Code { ticket, code } => self.console_code(files, ticket, code, peer, now),
            Console::Challenge { purpose, relying_party } => {
                self.console_challenge(purpose, relying_party, peer, now)
            }
            Console::Passkey { purpose, assertion } => {
                self.console_passkey(files, purpose, assertion, peer, now)
            }
            Console::Start { ticket, trust, browser } => {
                self.console_start(i, files, ticket, *trust, browser, peer, now)
            }
            Console::Unlock { session, proof } => {
                self.console_unlock(files, session, proof, peer, now)
            }
            Console::Extend { session } => {
                let unlocked = match self.usable(session, now) {
                    Ok(live) => live.unlocked,
                    Err(why) => return no(why, now),
                };
                if unlocked.is_none_or(|until| until <= now) {
                    return no("the session is locked: unlock it again", now);
                }
                let until = now + elevate::UNLOCKED_FOR;
                if let Some(live) = self.console.live.get_mut(session) {
                    live.unlocked = Some(until);
                }
                (Answer::Done(unix_at(now, until).to_string()), now)
            }
            Console::Lock { session } => match self.console.live.get_mut(session) {
                Some(live) => {
                    live.unlocked = None;
                    (Answer::Done(String::new()), now)
                }
                None => no(NO_SESSION, now),
            },
            Console::End { session } => match self.console.live.remove(session) {
                Some(live) => {
                    audit!("{}'s web console session {} ended", live.name, short(session));
                    signal_command(live.worker, libc::SIGHUP);
                    (Answer::Done(String::new()), now)
                }
                None => no(NO_SESSION, now),
            },
            Console::Elevate { by, language, command } => {
                self.console_elevate(i, files, by, language, command, now)
            }
            Console::Token { token, from } => self.console_token(files, token, from, now),
            Console::TokenStart { token } => self.console_token_start(i, files, token, now),
            _ => no("not a request of admind's", now),
        }
    }

    /// Counts a wrong proof for `name` from `source`: how many in a row,
    /// and when the next try is taken.
    fn counted(&mut self, name: &str, source: String, now: Instant) -> (u32, Instant) {
        let wrong =
            self.wrong.entry((name.into(), source)).or_insert(Wrong { count: 0, next: now });
        wrong.count += 1;
        wrong.next = now + wait_after(wrong.count);
        (wrong.count, wrong.next)
    }

    /// `console-begin`: checks the password, then the account's right to
    /// the console, and answers what the sign-in needs next.
    fn console_begin(
        &mut self,
        files: &Files,
        from: &str,
        name: &str,
        password: &[u8],
        peer: Peer,
        now: Instant,
    ) -> (Answer, Instant) {
        let Some(from) = address(from) else {
            return no("admind gives the browser's address", now);
        };
        if let Err(refused) =
            self.check_password_from(files, name, password, web_source(from), peer, now)
        {
            if matches!(&refused.0, Answer::Refused(why) if why == request::WRONG_PASSWORD) {
                failed(files, name, from);
            }
            return refused;
        }
        // A right password is an account's.
        let Some(user) = files.user(name).cloned() else {
            return no(request::WRONG_PASSWORD, now);
        };
        // Said only to someone who knows the password.
        if let Err(why) = may_sign_in(files, &user) {
            audit!("refused {} the web console from {}: {why}", user.name, shown(from));
            return no(why, now);
        }
        let (store, settings) = match (Store::read(&user.name), Settings::read()) {
            (Ok(store), Ok(settings)) => (store, settings),
            (Err(why), _) | (_, Err(why)) => return no(why, now),
        };
        let admin = accounts::is_admin(&user, &files.groups);
        let stage = stage_for(settings.sign_in.second_step, admin, &store);
        let sign_in = SignIn {
            name: user.name,
            from: from.into(),
            process: peer.process,
            given: now,
            stage: stage.clone(),
        };
        match self.console.add_sign_in(sign_in) {
            Ok(ticket) => {
                let ticket = hex(&ticket);
                let answer = match stage {
                    Stage::Done(_) => format!("done {ticket}"),
                    Stage::Step(kinds) => format!("step {ticket} {kinds}"),
                    Stage::NeedsStep => format!("needs-step {ticket}"),
                };
                (Answer::Done(answer), now)
            }
            Err(why) => no(why, now),
        }
    }

    /// `console-trusted`: skips the second step for a browser the person
    /// trusted.
    fn console_trusted(
        &mut self,
        ticket: &[u8],
        token: &[u8],
        peer: Peer,
        now: Instant,
    ) -> (Answer, Instant) {
        let name = match self.console.sign_in(ticket, peer, now) {
            Some(SignIn { stage: Stage::Step(_), name, .. }) => name.clone(),
            Some(_) => return no(NO_STEP_NEEDED, now),
            None => return no(NO_SIGN_IN, now),
        };
        let store = match Store::read(&name) {
            Ok(store) => store,
            Err(why) => return no(why, now),
        };
        if !store.trusts(token, unix_now()) {
            return no("this browser is no longer trusted: give a second step", now);
        }
        if let Some(sign_in) = self.console.sign_in(ticket, peer, now) {
            sign_in.stage = Stage::Done("password, on a browser trusted for 30 days".into());
        }
        (Answer::Done("done".into()), now)
    }

    /// `console-code`: an app's code or a recovery code as the second
    /// step. A wrong one waits as a wrong password does.
    fn console_code(
        &mut self,
        files: &Files,
        ticket: &[u8],
        code: &[u8],
        peer: Peer,
        now: Instant,
    ) -> (Answer, Instant) {
        let (name, from) = match self.console.sign_in(ticket, peer, now) {
            Some(SignIn { stage: Stage::Step(_), name, from, .. }) => (name.clone(), from.clone()),
            Some(_) => return no(NO_STEP_NEEDED, now),
            None => return no(NO_SIGN_IN, now),
        };
        let not_a_code = "a code is the app's 6 digits, or a recovery code's 16 letters and digits";
        let Ok(typed) = std::str::from_utf8(code) else { return no(not_a_code, now) };
        let mut store = match Store::read(&name) {
            Ok(store) => store,
            Err(why) => return no(why, now),
        };
        match store.take_code(typed, unix_now(), true) {
            Ok(how) => {
                if let Err(why) = store.write(&name) {
                    return no(why, now);
                }
                self.wrong.remove(&(name.clone(), web_source(&from)));
                if how != "app code" {
                    audit!("{name} signed in from {} with {how}", shown(&from));
                }
                if let Some(sign_in) = self.console.sign_in(ticket, peer, now) {
                    sign_in.stage = Stage::Done(format!("password and {how}"));
                }
                (Answer::Done("done".into()), now)
            }
            Err(CodeRefusal::NotACode) => no(not_a_code, now),
            Err(refusal) => {
                let (count, next) = self.counted(&name, web_source(&from), now);
                audit!(
                    "a wrong second-step code for {name} from {}; {count} in a row",
                    shown(&from)
                );
                failed(files, &name, &from);
                let why = match refusal {
                    CodeRefusal::Used => "that code was used already: give the app's next one",
                    _ => "that code is not right",
                };
                (Answer::Refused(why.into()), next)
            }
        }
    }

    /// `console-challenge`: a passkey challenge, with the IDs of the
    /// account's passkeys for the relying party when the account is
    /// known.
    fn console_challenge(
        &mut self,
        purpose: &Purpose,
        relying_party: &str,
        peer: Peer,
        now: Instant,
    ) -> (Answer, Instant) {
        if !relying_party_ok(relying_party) {
            return no(NOT_A_NAME, now);
        }
        let (purpose, name) = match purpose {
            Purpose::SignIn(ticket) => match self.console.sign_in(ticket, peer, now) {
                Some(SignIn { stage: Stage::Step(_), name, .. }) => {
                    (For::SignIn(ticket.clone()), Some(name.clone()))
                }
                Some(_) => return no(NO_STEP_NEEDED, now),
                None => return no(NO_SIGN_IN, now),
            },
            Purpose::Alone(from) => match address(from) {
                Some(from) => (For::Alone(from.into()), None),
                None => return no("admind gives the browser's address", now),
            },
            Purpose::Unlock(session) => match self.console.live.get(session) {
                Some(live) => (For::Unlock(session.clone()), Some(live.name.clone())),
                None => return no(NO_SESSION, now),
            },
        };
        let mut words = Vec::new();
        if let Some(name) = name {
            let store = match Store::read(&name) {
                Ok(store) => store,
                Err(why) => return no(why, now),
            };
            words = store.passkey_ids(relying_party);
            if words.is_empty() {
                return no(format!("{name} has no passkey for {relying_party}"), now);
            }
        }
        let challenge = Challenge {
            purpose,
            relying_party: relying_party.into(),
            process: peer.process,
            given: now,
        };
        match self.console.add_challenge(challenge) {
            Ok(bytes) => {
                words.insert(0, hex(&bytes));
                (Answer::Done(words.join(" ")), now)
            }
            Err(why) => no(why, now),
        }
    }

    /// `console-passkey`: a passkey as the second step of a sign-in, or
    /// as the whole of one.
    fn console_passkey(
        &mut self,
        files: &Files,
        purpose: &Purpose,
        assertion: &Assertion,
        peer: Peer,
        now: Instant,
    ) -> (Answer, Instant) {
        let Some(challenge) = self.console.take_challenge(&assertion.challenge, peer, now) else {
            return no(NO_CHALLENGE, now);
        };
        if challenge.relying_party != assertion.relying_party {
            return no(NOT_FOR_THIS, now);
        }
        match (purpose, &challenge.purpose) {
            (Purpose::SignIn(ticket), For::SignIn(given)) if ticket == given => {
                self.passkey_second_step(ticket, assertion, peer, now)
            }
            (Purpose::Alone(from), For::Alone(given)) if from == given => {
                self.passkey_alone(files, from, assertion, peer, now)
            }
            _ => no(NOT_FOR_THIS, now),
        }
    }

    fn passkey_second_step(
        &mut self,
        ticket: &[u8],
        assertion: &Assertion,
        peer: Peer,
        now: Instant,
    ) -> (Answer, Instant) {
        let (name, from) = match self.console.sign_in(ticket, peer, now) {
            Some(SignIn { stage: Stage::Step(_), name, from, .. }) => (name.clone(), from.clone()),
            Some(_) => return no(NO_STEP_NEEDED, now),
            None => return no(NO_SIGN_IN, now),
        };
        let mut store = match Store::read(&name) {
            Ok(store) => store,
            Err(why) => return no(why, now),
        };
        let passkey = match store.use_passkey(assertion, false, unix_now()) {
            Ok(passkey) => passkey,
            Err(why) => {
                audit!("refused a passkey of {name}'s from {}: {why}", shown(&from));
                return no(why, now);
            }
        };
        if let Err(why) = store.write(&name) {
            return no(why, now);
        }
        if let Some(sign_in) = self.console.sign_in(ticket, peer, now) {
            sign_in.stage = Stage::Done(format!("password and passkey \"{}\"", shown(&passkey)));
        }
        (Answer::Done("done".into()), now)
    }

    /// A sign-in with a passkey alone: its user handle names the account,
    /// and the authenticator must have checked the person, as a PIN or a
    /// fingerprint is a second proof besides the key.
    fn passkey_alone(
        &mut self,
        files: &Files,
        from: &str,
        assertion: &Assertion,
        peer: Peer,
        now: Instant,
    ) -> (Answer, Instant) {
        let unknown = "that passkey is not one of an account's here";
        let handle = hex(&assertion.user_handle);
        let Some(user) = owner_of_handle(&handle).and_then(|name| files.user(&name).cloned())
        else {
            audit!("refused a passkey that is no account's, from {}", shown(from));
            return no(unknown, now);
        };
        if let Err(why) = may_sign_in(files, &user) {
            audit!("refused {} the web console from {}: {why}", user.name, shown(from));
            return no(why, now);
        }
        let mut store = match Store::read(&user.name) {
            Ok(store) => store,
            Err(why) => return no(why, now),
        };
        let passkey = match store.use_passkey(assertion, true, unix_now()) {
            Ok(passkey) => passkey,
            Err(why) => {
                audit!("refused a passkey of {}'s from {}: {why}", user.name, shown(from));
                return no(why, now);
            }
        };
        if let Err(why) = store.write(&user.name) {
            return no(why, now);
        }
        let stage = Stage::Done(format!("passkey \"{}\"", shown(&passkey)));
        let sign_in = SignIn {
            name: user.name.clone(),
            from: from.into(),
            process: peer.process,
            given: now,
            stage,
        };
        match self.console.add_sign_in(sign_in) {
            Ok(ticket) => (Answer::Done(format!("done {} {}", hex(&ticket), user.name)), now),
            Err(why) => no(why, now),
        }
    }

    /// `console-start`: once a sign-in is done, starts its session's
    /// worker as the person, with the session's socket and the log
    /// client `i` sent, and trusts the browser for 30 days when asked.
    #[allow(clippy::too_many_arguments)]
    fn console_start(
        &mut self,
        i: usize,
        files: &Files,
        ticket: &[u8],
        trust: bool,
        browser: &str,
        peer: Peer,
        now: Instant,
    ) -> (Answer, Instant) {
        let descriptors = std::mem::take(&mut self.clients[i].descriptors);
        let Ok([calls, replies, log]) = <[OwnedFd; 3]>::try_from(descriptors) else {
            return no(
                "a console session starts with three descriptors: the link's two pipes and the \
                 log",
                now,
            );
        };
        let Some(names) = self.names else {
            return no("authd holds no names capability to start the console's workers with", now);
        };
        let needs_step = match self.console.sign_in(ticket, peer, now).map(|s| &s.stage) {
            Some(Stage::Step(_)) => return no("the sign-in's second step has not come", now),
            Some(Stage::NeedsStep) => true,
            Some(Stage::Done(_)) => false,
            None => return no(NO_SIGN_IN, now),
        };
        if self.console.live.len() >= LIVE_MAX {
            return no(format!("{LIVE_MAX} web console sessions are signed in already"), now);
        }
        // Used once.
        let Some(sign_in) = self.console.sign_ins.remove(ticket) else {
            return no(NO_SIGN_IN, now);
        };
        let how = match sign_in.stage {
            Stage::Done(how) => how,
            _ => "password, to add the second step the sign-in rules ask for".into(),
        };
        let Some(user) = files.user(&sign_in.name).cloned() else {
            return no(format!("there is no account {}", shown(&sign_in.name)), now);
        };
        if let Err(why) = may_sign_in(files, &user) {
            return no(why, now);
        }
        let (session, token) = match (random::<16>(), random::<32>()) {
            (Ok(session), Ok(token)) => (hex(&session), token),
            (Err(why), _) | (_, Err(why)) => return no(why, now),
        };
        let groups =
            accounts::other_groups(&user, &files.groups, radix_abi::names::GROUPS_MAX as usize);
        let process = match signin::start_console(names, &user, &groups, &calls, &replies, &log) {
            Ok(process) => process as i32,
            Err(why) => return no(why, now),
        };
        let mut answer = format!("{process} {session}");
        if trust && !needs_step {
            let trusted = Store::read(&user.name).and_then(|mut store| {
                store.trust(&token, browser, unix_now());
                store.write(&user.name)
            });
            match trusted {
                Ok(()) => {
                    let _ = write!(answer, " {}", hex(&token));
                }
                Err(why) => eprintln!("authd: the browser is not trusted: {why}"),
            }
        }
        let from = shown(&sign_in.from);
        audit!(
            "{} signed in to the web console from {from} ({how}): session {}, worker process \
             {process}",
            user.name,
            short(&session)
        );
        let seat = Seat {
            name: user.name.clone(),
            terminal: TERMINAL.into(),
            from: from.clone(),
            session: process as u32,
        };
        record(&signins::of(user.uid), user.gid, 0o640, Event::In(seat));
        let recorded = Session {
            name: user.name.clone(),
            terminal: TERMINAL.into(),
            from: from.clone(),
            number: user.uid,
            group: user.gid,
            elevated: false,
            remote: false,
            recorded: true,
        };
        self.sessions.insert(process, recorded);
        let live = Live {
            name: user.name,
            number: user.uid,
            from,
            worker: process,
            started: now,
            unlocked: None,
            needs_step,
        };
        self.console.live.insert(session, live);
        (Answer::Done(answer), now)
    }

    /// The console session `session`, if it may still be unlocked and
    /// make changes: not one that may only add a second step, and not
    /// past the hours the sign-in rules give a session.
    fn usable(&self, session: &str, now: Instant) -> Result<&Live, String> {
        let live = self.console.live.get(session).ok_or(NO_SESSION)?;
        if live.needs_step {
            return Err(
                "this session may only add a second step: sign in again once it is added".into()
            );
        }
        let hours = Settings::read()?.sign_in.session_hours;
        let lasts = Duration::from_hours(u64::from(hours));
        if now.saturating_duration_since(live.started) >= lasts {
            return Err(NO_SESSION.into());
        }
        Ok(live)
    }

    /// `console-unlock`: unlocks a session for 15 minutes once the person
    /// proves again that it is them.
    fn console_unlock(
        &mut self,
        files: &Files,
        session: &str,
        proof: &Proof,
        peer: Peer,
        now: Instant,
    ) -> (Answer, Instant) {
        let (name, from) = match self.usable(session, now) {
            Ok(live) => (live.name.clone(), live.from.clone()),
            Err(why) => return no(why, now),
        };
        // An unlock is for running commands as an administrator, which
        // elevate refuses anyone else: the session would show unlocked and
        // still make no change.
        if !files.user(&name).is_some_and(|user| accounts::is_admin(user, &files.groups)) {
            return no(format!("{name} is not an administrator, and makes no changes"), now);
        }
        let how = match proof {
            Proof::Password(password) => {
                let source = web_source(&from);
                if let Err(refused) =
                    self.check_password_from(files, &name, password, source, peer, now)
                {
                    return refused;
                }
                "password".to_string()
            }
            Proof::Code(code) => {
                let mut store = match Store::read(&name) {
                    Ok(store) => store,
                    Err(why) => return no(why, now),
                };
                let typed = std::str::from_utf8(code).unwrap_or_default();
                match store.take_code(typed, unix_now(), false) {
                    Ok(how) => {
                        if let Err(why) = store.write(&name) {
                            return no(why, now);
                        }
                        self.wrong.remove(&(name.clone(), web_source(&from)));
                        how
                    }
                    Err(CodeRefusal::NotACode) => {
                        return no("unlocking takes the app's 6 digits", now);
                    }
                    Err(_) => {
                        let (count, next) = self.counted(&name, web_source(&from), now);
                        audit!(
                            "a wrong app code to unlock {name}'s web console session {}; {count} \
                             in a row",
                            short(session)
                        );
                        return (Answer::Refused("that code is not right".into()), next);
                    }
                }
            }
            Proof::Passkey(assertion) => {
                let Some(challenge) = self.console.take_challenge(&assertion.challenge, peer, now)
                else {
                    return no(NO_CHALLENGE, now);
                };
                if challenge.purpose != For::Unlock(session.into())
                    || challenge.relying_party != assertion.relying_party
                {
                    return no(NOT_FOR_THIS, now);
                }
                let mut store = match Store::read(&name) {
                    Ok(store) => store,
                    Err(why) => return no(why, now),
                };
                match store.use_passkey(assertion, true, unix_now()) {
                    Ok(passkey) => {
                        if let Err(why) = store.write(&name) {
                            return no(why, now);
                        }
                        format!("passkey \"{}\"", shown(&passkey))
                    }
                    Err(why) => {
                        audit!("refused a passkey of {name}'s to unlock: {why}");
                        return no(why, now);
                    }
                }
            }
        };
        let until = now + elevate::UNLOCKED_FOR;
        if let Some(live) = self.console.live.get_mut(session) {
            live.unlocked = Some(until);
        }
        audit!("{name} unlocked the web console session {} for 15 minutes ({how})", short(session));
        (Answer::Done(unix_at(now, until).to_string()), now)
    }

    /// `console-elevate`: runs a command as an administrator, as elevate
    /// does (ADR 0219), for an unlocked session or an API token whose
    /// owner is an administrator, with the three descriptors and the pipe
    /// client `i` sent. The client then waits for the command's end.
    fn console_elevate(
        &mut self,
        i: usize,
        files: &Files,
        by: &Elevator,
        language: &[String],
        command: &[Vec<u8>],
        now: Instant,
    ) -> (Answer, Instant) {
        let mut descriptors = std::mem::take(&mut self.clients[i].descriptors);
        if descriptors.len() != 4 {
            return no("an elevated command comes with its three descriptors and a pipe", now);
        }
        // Whose right, the audit log's words for it, and the journal's.
        let (name, whose, tag) = match by {
            Elevator::Session(session) => match self.usable(session, now) {
                Ok(live) if live.unlocked.is_some_and(|until| until > now) => (
                    live.name.clone(),
                    format!("in the web console session {}", short(session)),
                    session.chars().take(8).collect::<String>(),
                ),
                Ok(_) => return no("the session is locked: unlock it to make changes", now),
                Err(why) => return no(why, now),
            },
            Elevator::Token(token) => match find_token(token, unix_now()) {
                Ok(given) if given.scopes.iter().any(|s| s == "*") => (
                    given.owner,
                    format!("with the API token \"{}\"", given.name),
                    format!("token {}", given.name),
                ),
                Ok(given) => {
                    return no(
                        format!(
                            "the API token \"{}\" may not run commands as an administrator",
                            given.name
                        ),
                        now,
                    );
                }
                Err(why) => return no(why, now),
            },
        };
        let user = match files.user(&name).filter(|u| may_sign_in(files, u).is_ok()) {
            Some(user) if accounts::is_admin(user, &files.groups) => user.clone(),
            _ => {
                audit!(
                    "{name}, not an administrator, asked to elevate {whose}: {}",
                    elevate::shown_command(command)
                );
                return no(format!("{name} is not an administrator, and makes no changes"), now);
            }
        };
        let root = elevate::root_account(files.users.entries().cloned());
        let plan = match elevate::plan(Mode::Command, command, b"/", &root) {
            Ok(plan) => plan,
            Err(why) => return no(why, now),
        };
        if let Some(why) = console_command_refusal(command, &plan.program) {
            audit!("refused {name} a command {whose}: {why}");
            return no(why, now);
        }
        let mut env = elevate::environment(&root, &user, "", language, &plan.shown);
        // For the journal: which session, or which token, ran it.
        env.push(format!("DUKOVO_CONSOLE_SESSION={tag}"));
        let relay = descriptors.pop();
        let process = match elevate::start(&descriptors, &plan, &env) {
            Ok(process) => process as i32,
            Err(why) => return no(why, now),
        };
        audit!("{name} elevated {whose}: process {process} runs {}", plan.shown);
        let session = Session {
            name,
            terminal: TERMINAL.into(),
            from: String::new(),
            number: user.uid,
            group: user.gid,
            elevated: true,
            remote: false,
            recorded: false,
        };
        self.sessions.insert(process, session);
        self.hold(i, process, relay);
        (Answer::Done(process.to_string()), now)
    }

    /// `console-token`: whether a call's API token is one, and what it
    /// gives. A wrong one waits as a wrong password does.
    fn console_token(
        &mut self,
        files: &Files,
        token: &[u8],
        from: &str,
        now: Instant,
    ) -> (Answer, Instant) {
        let Some(from) = address(from) else {
            return no("admind gives the caller's address", now);
        };
        match find_token(token, unix_now()) {
            Ok(given) => {
                let owner = files.user(&given.owner).cloned();
                if let Some(Err(why)) = owner.map(|user| may_sign_in(files, &user)) {
                    return no(format!("the API token \"{}\" is no use: {why}", given.name), now);
                }
                self.wrong.remove(&(given.owner.clone(), web_source(from)));
                let scopes = given.scopes.join(",");
                let answer = format!("{} {} {scopes} {}", given.owner, given.name, given.until);
                (Answer::Done(answer), now)
            }
            Err(why) => {
                let owner = token_owner(token).map_or("", |(owner, _)| owner);
                let (count, next) = self.counted(owner, web_source(from), now);
                audit!(
                    "a wrong API token, for {}, from {}; {count} in a row",
                    shown(owner),
                    shown(from)
                );
                (Answer::Refused(why), next)
            }
        }
    }

    /// `console-token-start`: starts a worker for an API token's owner,
    /// as `console-start` does for a session.
    fn console_token_start(
        &mut self,
        i: usize,
        files: &Files,
        token: &[u8],
        now: Instant,
    ) -> (Answer, Instant) {
        let descriptors = std::mem::take(&mut self.clients[i].descriptors);
        let Ok([calls, replies, log]) = <[OwnedFd; 3]>::try_from(descriptors) else {
            return no(
                "a worker starts with three descriptors: the link's two pipes and the log",
                now,
            );
        };
        let Some(names) = self.names else {
            return no("authd holds no names capability to start the console's workers with", now);
        };
        let given = match find_token(token, unix_now()) {
            Ok(given) => given,
            Err(why) => return no(why, now),
        };
        let Some(user) = files.user(&given.owner).cloned() else {
            return no(format!("there is no account {}", given.owner), now);
        };
        if let Err(why) = may_sign_in(files, &user) {
            return no(why, now);
        }
        let groups =
            accounts::other_groups(&user, &files.groups, radix_abi::names::GROUPS_MAX as usize);
        let process = match signin::start_console(names, &user, &groups, &calls, &replies, &log) {
            Ok(process) => process as i32,
            Err(why) => return no(why, now),
        };
        audit!(
            "the API token \"{}\" of {}'s started worker process {process}",
            given.name,
            user.name
        );
        let session = Session {
            name: user.name,
            terminal: API.into(),
            from: String::new(),
            number: user.uid,
            group: user.gid,
            elevated: false,
            remote: false,
            recorded: false,
        };
        self.sessions.insert(process, session);
        (Answer::Done(process.to_string()), now)
    }

    /// Signs `name` out of the web console, each of their sessions, for
    /// `why`.
    pub(super) fn end_console_sessions(&mut self, name: &str, why: &str) {
        let theirs: Vec<String> = self
            .console
            .live
            .iter()
            .filter(|(_, l)| l.name == name)
            .map(|(s, _)| s.clone())
            .collect();
        for session in theirs {
            if let Some(live) = self.console.live.remove(&session) {
                audit!("{name}'s web console session {} ended: {why}", short(&session));
                signal_command(live.worker, libc::SIGHUP);
            }
        }
    }

    /// Checks a person's proof for a change to their own second steps or
    /// tokens: their password, or, from their own session's worker while
    /// the session is unlocked, nothing.
    fn console_proof(
        &mut self,
        files: &Files,
        user: &User,
        proof: &[u8],
        peer: Peer,
        now: Instant,
    ) -> Result<(), (Answer, Instant)> {
        if !proof.is_empty() {
            return self.check_password(files, &user.name, proof, peer, now);
        }
        let unlocked = self.console.live.values().any(|live| {
            live.number == user.uid
                && u64::try_from(live.worker).is_ok_and(|worker| worker == peer.process)
                && live.unlocked.is_some_and(|until| until > now)
        });
        if unlocked {
            Ok(())
        } else {
            Err(no("give your password, or unlock the console first", now))
        }
    }

    /// The person's own requests, about their own account.
    fn console_person(
        &mut self,
        files: &Files,
        asked: &Console,
        peer: Peer,
        now: Instant,
    ) -> (Answer, Instant) {
        let Some(user) = files.users.entries().find(|u| u.uid == peer.user).cloned() else {
            return no("only a person's account has second steps", now);
        };
        if !user.is_person() {
            return no(
                "only a person's account has second steps: root never signs in to the web console",
                now,
            );
        }
        let proof = match asked {
            Console::AppBegin { proof, .. }
            | Console::AppRemove { proof }
            | Console::PasskeyBegin { proof, .. }
            | Console::PasskeyRemove { proof, .. }
            | Console::Codes { proof }
            | Console::TokenAdd { proof, .. } => Some(proof),
            _ => None,
        };
        if let Some(proof) = proof
            && let Err(refused) = self.console_proof(files, &user, proof, peer, now)
        {
            return refused;
        }
        let adds = matches!(
            asked,
            Console::AppBegin { .. } | Console::PasskeyBegin { .. } | Console::TokenAdd { .. }
        );
        if adds && !accounts::may_use_console(&user, &files.groups) {
            return no(format!("{} may not use the web console", user.name), now);
        }
        let needed = match Settings::read() {
            Ok(settings) => rules_need_step(
                settings.sign_in.second_step,
                accounts::is_admin(&user, &files.groups),
            ),
            Err(why) => return no(why, now),
        };
        let mut store = match Store::read(&user.name) {
            Ok(store) => store,
            Err(why) => return no(why, now),
        };
        match self.change_steps(&user, &mut store, asked, needed, peer, now) {
            Ok(text) => (Answer::Done(text), now),
            Err(why) => no(why, now),
        }
    }

    /// Carries out a person's request about their own second steps or
    /// tokens, proved already: what to answer.
    fn change_steps(
        &mut self,
        user: &User,
        store: &mut Store,
        asked: &Console,
        needed: bool,
        peer: Peer,
        now: Instant,
    ) -> Result<String, String> {
        let name = &user.name;
        let unix = unix_now();
        let keep_one = |store: &Store, what: &str| {
            if needed && !store.has_step() {
                Err(format!(
                    "the sign-in rules ask {name} for a second step: add another before removing \
                     the {what}"
                ))
            } else {
                Ok(())
            }
        };
        match asked {
            Console::Steps => Ok(steps(store, needed, unix)),
            Console::AppBegin { host, .. } => {
                if !label_ok(host) {
                    return Err("the machine's name for the app is 1 to 64 characters".into());
                }
                let secret = random::<{ proofs::SECRET_BYTES }>()?;
                let uri = proofs::app_uri(host, name, &secret);
                self.console
                    .apps
                    .insert(user.uid, NewApp { secret, process: peer.process, given: now });
                Ok(uri)
            }
            Console::AppConfirm { code } => {
                let fresh = |new: &&NewApp| {
                    new.process == peer.process
                        && now.saturating_duration_since(new.given) < APP_TIME
                };
                let Some(new) = self.console.apps.get(&user.uid).filter(fresh) else {
                    return Err("begin adding the app again: it was not begun here, or was begun \
                                more than 10 minutes ago"
                        .into());
                };
                let typed = std::str::from_utf8(code).unwrap_or_default();
                let step = proofs::check_app_code(&new.secret, typed, unix as u64, None).map_err(
                    |_| {
                        "that is not the app's code: check that the phone's clock is right, and \
                         give the code it shows now"
                    },
                )?;
                let replaced = store.app.is_some();
                store.app = Some(App {
                    secret: proofs::base32(&new.secret),
                    added: unix,
                    last_step: Some(step),
                });
                let codes =
                    if store.recovery.is_none() { store.new_codes(unix)? } else { Vec::new() };
                store.write(name)?;
                self.console.apps.remove(&user.uid);
                let instead = if replaced { ", in place of the one before" } else { "" };
                audit!("{name} added an authenticator app{instead}");
                Ok(codes.join(" "))
            }
            Console::AppRemove { .. } => {
                if store.app.take().is_none() {
                    return Err(format!("{name} has no authenticator app"));
                }
                keep_one(store, "app")?;
                store.tidy_steps();
                store.write(name)?;
                audit!("{name} removed their authenticator app");
                Ok(String::new())
            }
            Console::PasskeyBegin { relying_party, .. } => {
                if !relying_party_ok(relying_party) {
                    return Err(NOT_A_NAME.into());
                }
                if store.passkeys.len() >= PASSKEYS_MAX {
                    return Err(format!("{name} has {PASSKEYS_MAX} passkeys: remove one first"));
                }
                if store.handle.is_empty() {
                    store.handle = hex(&random::<16>()?);
                    store.write(name)?;
                }
                let challenge = Challenge {
                    purpose: For::Adding(name.clone()),
                    relying_party: relying_party.clone(),
                    process: peer.process,
                    given: now,
                };
                let bytes = self.console.add_challenge(challenge)?;
                let mut words = vec![hex(&bytes), store.handle.clone()];
                words.extend(store.passkey_ids(relying_party));
                Ok(words.join(" "))
            }
            Console::PasskeyAdd {
                challenge,
                origin,
                relying_party,
                name: label,
                client_data,
                attestation,
            } => {
                let given =
                    self.console.take_challenge(challenge, peer, now).ok_or(NO_CHALLENGE)?;
                if given.purpose != For::Adding(name.clone())
                    || given.relying_party != *relying_party
                {
                    return Err(NOT_FOR_THIS.into());
                }
                if !label_ok(label) {
                    return Err("a passkey's name is 1 to 64 characters".into());
                }
                let expected = Expected {
                    ceremony: Ceremony::Create,
                    challenge,
                    origin,
                    relying_party,
                    verified: false,
                };
                let new = proofs::check_registration(&expected, client_data, attestation)?;
                let id = hex(&new.id);
                if store.passkeys.iter().any(|p| p.id == id) {
                    return Err("that passkey is added already".into());
                }
                if store.passkeys.len() >= PASSKEYS_MAX {
                    return Err(format!("{name} has {PASSKEYS_MAX} passkeys: remove one first"));
                }
                store.passkeys.push(Passkey {
                    id,
                    key: hex(&new.key),
                    algorithm: new.algorithm,
                    relying_party: relying_party.clone(),
                    name: label.trim().into(),
                    count: new.count,
                    added: unix,
                    used: 0,
                });
                let codes =
                    if store.recovery.is_none() { store.new_codes(unix)? } else { Vec::new() };
                store.write(name)?;
                audit!("{name} added a passkey \"{}\" for {relying_party}", shown(label.trim()));
                Ok(codes.join(" "))
            }
            Console::PasskeyRemove { id, .. } => {
                let id = hex(id);
                let at = store.passkeys.iter().position(|p| p.id == id);
                let gone = store.passkeys.remove(at.ok_or(format!("{name} has no such passkey"))?);
                keep_one(store, "passkey")?;
                store.tidy_steps();
                store.write(name)?;
                audit!("{name} removed their passkey \"{}\"", shown(&gone.name));
                Ok(String::new())
            }
            Console::PasskeyRename { id, name: label } => {
                if !label_ok(label) {
                    return Err("a passkey's name is 1 to 64 characters".into());
                }
                let id = hex(id);
                let passkey = store.passkeys.iter_mut().find(|p| p.id == id);
                passkey.ok_or(format!("{name} has no such passkey"))?.name = label.trim().into();
                store.write(name)?;
                Ok(String::new())
            }
            Console::Codes { .. } => {
                if !store.has_step() {
                    return Err("recovery codes come with a second step: add an app or a \
                                passkey first"
                        .into());
                }
                let codes = store.new_codes(unix)?;
                store.write(name)?;
                audit!("{name} made new recovery codes, and the old ones no longer work");
                Ok(codes.join(" "))
            }
            Console::TrustedForget => {
                let forgotten = store.trusted.len();
                store.trusted.clear();
                store.write(name)?;
                if forgotten > 0 {
                    audit!("{name} no longer trusts the {forgotten} browsers they trusted");
                }
                Ok(forgotten.to_string())
            }
            Console::TokenAdd { name: token_name, scopes, days, .. } => {
                if !token_name_ok(token_name) {
                    return Err("a token's name is 1 to 32 of a to z, 0 to 9 and -".into());
                }
                if scopes.is_empty() || scopes.len() > 16 || !scopes.iter().all(|s| scope_ok(s)) {
                    return Err("a token's scopes are * or the names of areas, 16 at most".into());
                }
                if !(1..=TOKEN_DAYS_MAX).contains(days) {
                    return Err(format!("a token lasts 1 to {TOKEN_DAYS_MAX} days"));
                }
                store.tokens.retain(|t| t.until > unix - EXPIRED_KEPT);
                if store.tokens.iter().any(|t| t.name == *token_name) {
                    return Err(format!(
                        "{name} has a token named {token_name}: remove it, or choose another name"
                    ));
                }
                if store.tokens.len() >= TOKENS_MAX {
                    return Err(format!("{name} has {TOKENS_MAX} tokens: remove one first"));
                }
                let secret = hex(&random::<TOKEN_SECRET>()?);
                let until = unix + i64::from(*days) * DAY;
                store.tokens.push(Token {
                    name: token_name.clone(),
                    hash: hex(&sha256(secret.as_bytes())),
                    scopes: scopes.clone(),
                    until,
                    made: unix,
                    used: 0,
                });
                store.write(name)?;
                let what = if scopes.iter().any(|s| s == "*") {
                    "everything".to_string()
                } else {
                    scopes.join(", ")
                };
                audit!(
                    "{name} made the API token \"{token_name}\", for {what}, until {}",
                    day(until)
                );
                Ok(format!("{TOKEN_PREFIX}{name}_{secret}"))
            }
            Console::TokenRemove { name: token_name } => {
                let before = store.tokens.len();
                store.tokens.retain(|t| t.name != *token_name);
                if store.tokens.len() == before {
                    return Err(format!("{name} has no token named {}", shown(token_name)));
                }
                store.write(name)?;
                audit!("{name} removed the API token \"{token_name}\"");
                Ok(String::new())
            }
            _ => Err("not a request about one's own second steps".into()),
        }
    }
}

#[cfg(test)]
mod tests;
