// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The MCP clients an administrator lets in to the web console
//! (ADR 0690), which the console's pages call AI clients.
//!
//! A client is its owner's: one of at most 16 in
//! `/system/state/authd/clients/OWNER.toml`, each with a name, how it
//! connects, what it may reach and the day it ends. The files are apart
//! from the accounts' own in `console/`, so that an authd from before
//! this one still reads those.
//!
//! What a client shows to be let in is of a kind of its own, so that
//! nothing made for one use is taken for another:
//!
//! - **A token**, `dkc_OWNER_SECRET`, for a client that connects over
//!   HTTPS. It is shown once, when the client is added, and authd keeps
//!   its SHA-256.
//! - **A pass**, `dkp_OWNER_SECRET`, for a client that connects over
//!   SSH. `admin mcp` asks for one as the owner each time it starts,
//!   which is how authd, and not admind, sees who asks. A pass is kept
//!   in memory alone and lasts 12 hours.
//!
//! Neither is an API token, and a client changes nothing by itself:
//! `console-elevate` refuses both kinds before it looks anything up. The
//! one program authd starts as an administrator for a client is `admind
//! --read`, which runs the console's reads and nothing else (ADR 0691).
//! authd writes that command itself. admind sends the action's name and
//! its arguments, and authd adds the areas that the client was given
//! whole, which the program holds the read to.
//!
//! A client that connects over SSH is as trusted as its owner's account
//! on this machine: any program that runs as the owner here can ask for
//! its pass. That is what holding no secret means, and why a client is
//! added with the owner's password.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::fs;
use std::io;
use std::os::fd::OwnedFd;
use std::time::{Duration, Instant};

use accounts::User;
use accounts::console::{Console, Way};
use accounts::request::{Answer, Mode, hex};
use serde::Deserialize;
use serde_json::{Value, json};

use super::{
    API, Authd, DAY, EXPIRED_KEPT, Files, Peer, Session, TOKEN_SECRET, USED_EVERY, address,
    console_command_refusal, day, may_sign_in, no, oldest_out, person, quoted, random, replace,
    sha256, shown, token_name_ok, unix_now, web_source,
};
use crate::authd::blocking::Failed;
use crate::authd::proofs;
use crate::{elevate, signin, util};

/// The folder of the owners' files, inside authd's own.
pub const FOLDER: &str = "/system/state/authd/clients";

/// How a client's token starts.
pub const TOKEN_PREFIX: &str = "dkc_";

/// How a client's pass starts.
pub const PASS_PREFIX: &str = "dkp_";

/// The most clients an account keeps.
pub(crate) const CLIENTS_MAX: usize = 16;

/// The most scopes a client has: `*:read` and every area by its name
/// leave room.
pub(crate) const SCOPES_MAX: usize = 32;

/// The most days a client lasts.
const DAYS_MAX: u32 = 366;

/// How long a pass lasts.
const PASS_FOR: Duration = Duration::from_hours(12);

/// The most passes of one client that work at once: an MCP client starts
/// `admin mcp` twice as it connects, and each start again takes a new
/// one.
const PASSES_EACH: usize = 8;

/// The most passes authd keeps.
const PASSES_MAX: usize = 256;

/// The names `admin mcp` reads as what to do, which no client has.
const RESERVED: [&str; 4] = ["add", "list", "remove", "help"];

/// The longest name of an action, and the most bytes of a read's
/// arguments.
const ACTION_MAX: usize = 64;
const ARGUMENTS_MAX: usize = 16 * 1024;

/// What authd starts for a client's read as an administrator: admind(8),
/// as the program that runs one read.
const READ: [&str; 2] = ["/system/commands/admind", "--read"];

/// What is said of a token or a pass that gives nothing, whichever part
/// of it is wrong.
const WRONG: &str = "that AI client's token or pass is not one of this machine's, or has ended";

/// An account's clients.
#[derive(Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
struct Records {
    #[serde(default)]
    clients: Vec<Record>,
}

/// One client.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
struct Record {
    name: String,
    /// [`Way::word`].
    way: String,
    /// The SHA-256 of its token's secret, in hexadecimal; nothing for a
    /// client that connects over SSH.
    #[serde(default)]
    hash: String,
    scopes: Vec<String>,
    /// When it ends, was made and was last used, in Unix seconds; never
    /// used is 0.
    until: i64,
    made: i64,
    used: i64,
    /// The address it was last used from.
    #[serde(default)]
    from: String,
}

/// A client to add.
struct New<'a> {
    name: &'a str,
    way: Way,
    scopes: &'a [String],
    days: u32,
}

impl Records {
    fn path(owner: &str) -> String {
        format!("{FOLDER}/{owner}.toml")
    }

    /// `owner`'s file, or an empty one when it has none.
    fn read(owner: &str) -> Result<Records, String> {
        accounts::check_name(owner)?;
        let path = Records::path(owner);
        match fs::read_to_string(&path) {
            Ok(text) => Records::parse(&text).map_err(|why| format!("{path}: {why}")),
            Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(Records::default()),
            Err(err) => Err(format!("cannot read {path}: {}", util::strip_os_error(&err))),
        }
    }

    fn parse(text: &str) -> Result<Records, String> {
        toml::from_str(text).map_err(|err| err.message().to_string())
    }

    /// Writes `owner`'s file anew, or removes it once it holds nothing.
    fn write(&self, owner: &str) -> Result<(), String> {
        let path = Records::path(owner);
        if !self.clients.is_empty() {
            return replace(&path, &self.text(), 0o600);
        }
        match fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(err) => Err(format!("cannot remove {path}: {}", util::strip_os_error(&err))),
        }
    }

    /// The file's text.
    fn text(&self) -> String {
        let mut text = String::new();
        for client in &self.clients {
            let scopes: Vec<String> = client.scopes.iter().map(|scope| quoted(scope)).collect();
            let _ = write!(
                text,
                "{}[[clients]]\nname = {}\nway = {}\nhash = {}\nscopes = [{}]\nuntil = {}\nmade = \
                 {}\nused = {}\nfrom = {}\n",
                if text.is_empty() { "" } else { "\n" },
                quoted(&client.name),
                quoted(&client.way),
                quoted(&client.hash),
                scopes.join(", "),
                client.until,
                client.made,
                client.used,
                quoted(&client.from)
            );
        }
        text
    }

    /// Drops the clients that ended more than 30 days ago.
    fn tidy(&mut self, unix: i64) {
        self.clients.retain(|client| client.until > unix - EXPIRED_KEPT);
    }

    /// Adds `owner`'s client `new` at `unix`, whose token's secret is
    /// `secret` when it connects over HTTPS: when it ends.
    fn add(&mut self, owner: &str, new: &New<'_>, secret: &str, unix: i64) -> Result<i64, String> {
        if !name_ok(new.name) {
            return Err("an AI client's name is 1 to 32 of a to z, 0 to 9 and -, and not add, \
                        list, remove or help"
                .into());
        }
        let scopes = new.scopes;
        if scopes.is_empty() || scopes.len() > SCOPES_MAX || !scopes.iter().all(|s| scope_ok(s)) {
            return Err(format!(
                "an AI client's scopes are the names of areas, or *, each alone or with :read, \
                 {SCOPES_MAX} at most"
            ));
        }
        if !(1..=DAYS_MAX).contains(&new.days) {
            return Err(format!("an AI client lasts 1 to {DAYS_MAX} days"));
        }
        self.tidy(unix);
        if self.clients.iter().any(|client| client.name == new.name) {
            return Err(format!(
                "{owner} has an AI client named {}: remove it, or choose another name",
                new.name
            ));
        }
        if self.clients.len() >= CLIENTS_MAX {
            return Err(format!("{owner} has {CLIENTS_MAX} AI clients: remove one first"));
        }
        let hash = match new.way {
            Way::Https => hex(&sha256(secret.as_bytes())),
            Way::Ssh => String::new(),
        };
        let until = unix + i64::from(new.days) * DAY;
        self.clients.push(Record {
            name: new.name.into(),
            way: new.way.word().into(),
            hash,
            scopes: scopes.to_vec(),
            until,
            made: unix,
            used: 0,
            from: String::new(),
        });
        Ok(until)
    }

    /// Removes the client `name`: whether there was one.
    fn remove(&mut self, name: &str) -> bool {
        let before = self.clients.len();
        self.clients.retain(|client| client.name != name);
        self.clients.len() < before
    }

    /// The client `name`, if it has not ended.
    fn current(&mut self, name: &str, unix: i64) -> Option<&mut Record> {
        self.clients.iter_mut().find(|client| client.name == name && client.until > unix)
    }

    /// The client whose token's secret is `secret`, if it has not ended.
    fn of_secret(&mut self, secret: &str, unix: i64) -> Option<&mut Record> {
        let hash = hex(&sha256(secret.as_bytes()));
        self.clients.iter_mut().find(|client| {
            client.until > unix
                && client.way == Way::Https.word()
                && proofs::same(client.hash.as_bytes(), hash.as_bytes())
        })
    }

    /// `owner`'s clients for a list, with none of their secrets.
    fn json(&self, owner: &str, unix: i64) -> Vec<Value> {
        self.clients
            .iter()
            .map(|client| {
                json!({
                    "owner": owner,
                    "name": client.name,
                    "way": client.way,
                    "scopes": client.scopes,
                    "until": client.until,
                    "made": client.made,
                    "used": client.used,
                    "from": client.from,
                    "ended": client.until <= unix,
                })
            })
            .collect()
    }
}

impl Record {
    /// What it gives its holder.
    fn found(&self, owner: &str) -> Option<Found> {
        Some(Found {
            owner: owner.into(),
            name: self.name.clone(),
            way: Way::of(self.way.as_bytes())?,
            scopes: self.scopes.clone(),
            until: self.until,
        })
    }

    /// Takes in that it was used at `unix`, from `from` when that is
    /// known: whether its file is to be written, which is once an hour at
    /// most.
    fn used_at(&mut self, unix: i64, from: Option<&str>) -> bool {
        if unix - self.used < USED_EVERY {
            return false;
        }
        self.used = unix;
        if let Some(from) = from {
            self.from = shown(from);
        }
        true
    }
}

/// Whether `name` can name a client: as an API token's, and not a word
/// that `admin mcp` reads as what to do.
pub(crate) fn name_ok(name: &str) -> bool {
    token_name_ok(name) && !RESERVED.contains(&name)
}

/// Whether `scope` can be a client's: an area's name or `*`, alone, for
/// every read there and to ask for its changes, or with `:read`, for the
/// reads its owner has before unlocking.
pub(crate) fn scope_ok(scope: &str) -> bool {
    let area = scope.strip_suffix(":read").unwrap_or(scope);
    area == "*" || (!area.contains(':') && super::scope_ok(area))
}

/// The areas a client of `scopes` was given whole, not their reading
/// alone: what it reads as an administrator.
fn whole(scopes: &[String]) -> Vec<&str> {
    scopes.iter().map(String::as_str).filter(|scope| !scope.ends_with(":read")).collect()
}

/// Whether `credential` starts as a client's token or pass does. Those
/// run nothing through `console-elevate`, whatever else is true of them.
pub(super) fn is_clients(credential: &[u8]) -> bool {
    credential.starts_with(TOKEN_PREFIX.as_bytes())
        || credential.starts_with(PASS_PREFIX.as_bytes())
}

/// Which of the two a client shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Token,
    Pass,
}

/// The kind, the owner and the secret part of a client's token or pass.
fn parts(credential: &[u8]) -> Option<(Kind, &str, &str)> {
    let text = std::str::from_utf8(credential).ok()?;
    let (kind, rest) = match text.strip_prefix(TOKEN_PREFIX) {
        Some(rest) => (Kind::Token, rest),
        None => (Kind::Pass, text.strip_prefix(PASS_PREFIX)?),
    };
    let (owner, secret) = rest.rsplit_once('_')?;
    let hex_digits = secret.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'));
    (accounts::check_name(owner).is_ok() && secret.len() == TOKEN_SECRET * 2 && hex_digits)
        .then_some((kind, owner, secret))
}

/// The owner a token or a pass names, for the audit log; nothing when it
/// names none.
pub(super) fn owner_of(credential: &[u8]) -> &str {
    parts(credential).map_or("", |(_, owner, _)| owner)
}

/// The passes authd has given, by the SHA-256 of their secrets.
#[derive(Default)]
pub(in crate::authd) struct Passes {
    given: HashMap<Vec<u8>, Pass>,
}

/// A pass: whose client's, and when it was given.
struct Pass {
    owner: String,
    name: String,
    given: Instant,
}

impl Passes {
    /// Forgets the passes that are over.
    pub(super) fn tidy(&mut self, now: Instant) {
        self.given.retain(|_, pass| now.saturating_duration_since(pass.given) < PASS_FOR);
    }

    /// Keeps a pass whose secret is `secret` for `owner`'s client `name`.
    /// The oldest of the client's goes when it has as many as it may.
    fn give(&mut self, owner: &str, name: &str, secret: &str, now: Instant) {
        self.tidy(now);
        let of_client = |pass: &Pass| pass.owner == owner && pass.name == name;
        while self.given.values().filter(|pass| of_client(pass)).count() >= PASSES_EACH {
            let oldest = self
                .given
                .iter()
                .filter(|(_, pass)| of_client(pass))
                .min_by_key(|(_, pass)| pass.given)
                .map(|(hash, _)| hash.clone());
            match oldest {
                Some(hash) => self.given.remove(&hash),
                None => break,
            };
        }
        oldest_out(&mut self.given, PASSES_MAX, |pass| pass.given);
        let pass = Pass { owner: owner.into(), name: name.into(), given: now };
        self.given.insert(sha256(secret.as_bytes()).to_vec(), pass);
    }

    /// The name of `owner`'s client whose pass has the secret `secret`,
    /// if the pass is not over.
    fn client(&self, owner: &str, secret: &str, now: Instant) -> Option<&str> {
        let pass = self.given.get(sha256(secret.as_bytes()).as_slice())?;
        (pass.owner == owner && now.saturating_duration_since(pass.given) < PASS_FOR)
            .then_some(pass.name.as_str())
    }

    /// Forgets the passes of `owner`'s client `name`, or of every client
    /// of `owner`'s when `name` is none.
    pub(super) fn forget(&mut self, owner: &str, name: Option<&str>) {
        self.given
            .retain(|_, pass| pass.owner != owner || name.is_some_and(|name| pass.name != name));
    }
}

/// What a client's token or pass gives: whose client, its name, how it
/// connects, what it may reach, and when it ends.
#[derive(Debug, PartialEq, Eq)]
struct Found {
    owner: String,
    name: String,
    way: Way,
    scopes: Vec<String>,
    until: i64,
}

impl Found {
    /// The answer that says it: `OWNER NAME WAY SCOPES UNTIL`.
    fn answer(&self) -> String {
        let scopes = self.scopes.join(",");
        format!("{} {} {} {scopes} {}", self.owner, self.name, self.way.word(), self.until)
    }
}

/// The client in `records`, `owner`'s, that `kind` and `secret` are the
/// token or the pass of, if it has not ended and connects that way.
fn holder<'a>(
    records: &'a mut Records,
    passes: &Passes,
    (kind, owner, secret): (Kind, &str, &str),
    unix: i64,
    now: Instant,
) -> Option<&'a mut Record> {
    match kind {
        Kind::Token => records.of_secret(secret, unix),
        Kind::Pass => {
            let name = passes.client(owner, secret, now)?;
            records.current(name, unix).filter(|client| client.way == Way::Ssh.word())
        }
    }
}

/// What `credential` gives, if it is the token or a pass of a client
/// that has not ended. Its use is recorded at most once an hour, with
/// the address `from` when one is given.
fn find(
    passes: &Passes,
    credential: &[u8],
    from: Option<&str>,
    unix: i64,
    now: Instant,
) -> Result<Found, String> {
    let (kind, owner, secret) = parts(credential).ok_or(WRONG)?;
    let mut records = Records::read(owner)?;
    let client = holder(&mut records, passes, (kind, owner, secret), unix, now).ok_or(WRONG)?;
    let found = client.found(owner).ok_or(WRONG)?;
    if client.used_at(unix, from)
        && let Err(why) = records.write(owner)
    {
        eprintln!("authd: {why}");
    }
    Ok(found)
}

/// Why `user`, who owns a client, gives it nothing now, if so: the
/// account may not sign in to the console, or is no administrator's.
fn owner_refusal(files: &Files, user: &User) -> Option<String> {
    if let Err(why) = may_sign_in(files, user) {
        return Some(why);
    }
    (!accounts::is_admin(user, &files.groups))
        .then(|| format!("{} is not an administrator, and has no AI clients", user.name))
}

/// Moves `name`'s clients to `new`, as the account is renamed. Those
/// that connect over HTTPS end: their tokens name the account.
pub(super) fn renamed(name: &str, new: &str) -> Result<(), String> {
    let mut records = Records::read(name)?;
    if records.clients.is_empty() {
        return Ok(());
    }
    records.clients.retain(|client| client.way == Way::Ssh.word());
    records.write(new)?;
    Records::default().write(name)
}

/// Removes `name`'s clients, as the account goes.
pub(super) fn removed(name: &str) {
    if let Err(why) = Records::default().write(name) {
        eprintln!("authd: {why}");
    }
}

/// Every account's clients, for a list, with none of their secrets.
fn all(unix: i64) -> Result<Vec<Value>, String> {
    let entries = match fs::read_dir(FOLDER) {
        Ok(entries) => entries,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => return Err(format!("cannot read {FOLDER}: {}", util::strip_os_error(&err))),
    };
    let mut owners: Vec<String> = entries
        .flatten()
        .filter_map(|entry| {
            entry.file_name().to_str().and_then(|file| file.strip_suffix(".toml")).map(String::from)
        })
        .collect();
    owners.sort();
    let mut clients = Vec::new();
    for owner in owners {
        clients.extend(Records::read(&owner)?.json(&owner, unix));
    }
    Ok(clients)
}

impl Authd {
    /// A person's request about their own clients, proved already where
    /// it needs proof: what to answer.
    pub(super) fn own_clients(
        &mut self,
        files: &Files,
        user: &User,
        asked: &Console,
        now: Instant,
    ) -> Result<String, String> {
        let owner = &user.name;
        let unix = unix_now();
        let mut records = Records::read(owner)?;
        match asked {
            Console::Clients => Ok(json!({ "clients": records.json(owner, unix) }).to_string()),
            Console::ClientAdd { name, way, scopes, days, .. } => {
                if let Some(why) = owner_refusal(files, user) {
                    return Err(why);
                }
                let secret = hex(&random::<TOKEN_SECRET>()?);
                let new = New { name, way: *way, scopes, days: *days };
                let until = records.add(owner, &new, &secret, unix)?;
                records.write(owner)?;
                audit!(
                    "{owner} let in the AI client \"{name}\", over {}, for {}, until {}",
                    way.word(),
                    scopes.join(", "),
                    day(until)
                );
                Ok(match way {
                    Way::Https => format!("{TOKEN_PREFIX}{owner}_{secret}"),
                    Way::Ssh => String::new(),
                })
            }
            Console::ClientRemove { name } => {
                if !records.remove(name) {
                    return Err(format!("{owner} has no AI client named {}", shown(name)));
                }
                records.write(owner)?;
                self.console.passes.forget(owner, Some(name));
                audit!("{owner} removed the AI client \"{name}\"");
                Ok(String::new())
            }
            Console::ClientPass { name, from } => {
                if let Some(why) = owner_refusal(files, user) {
                    return Err(why);
                }
                let from = if from.is_empty() { None } else { address(from) };
                let over_ssh = |client: &&mut Record| client.way == Way::Ssh.word();
                let Some(client) = records.current(name, unix).filter(over_ssh) else {
                    return Err(format!(
                        "{owner} has no AI client named {} that connects over SSH and has not \
                         ended",
                        shown(name)
                    ));
                };
                let until = client.until;
                if client.used_at(unix, from) {
                    records.write(owner)?;
                }
                let secret = hex(&random::<TOKEN_SECRET>()?);
                self.console.passes.give(owner, name, &secret, now);
                audit!("gave {owner} a pass for the AI client \"{name}\"");
                let ends = (unix + PASS_FOR.as_secs() as i64).min(until);
                Ok(format!("{PASS_PREFIX}{owner}_{secret} {ends}"))
            }
            _ => Err("not a request about one's own AI clients".into()),
        }
    }

    /// `console-client`: whether a call's token or pass is a client's,
    /// and what it gives. A wrong token from the network waits as a wrong
    /// password does, and counts towards blocking its address. A wrong
    /// pass does neither: it comes from this machine, where a count would
    /// let anyone who runs a program here hold a client's owner back.
    pub(super) fn console_client(
        &mut self,
        files: &Files,
        token: &[u8],
        from: &str,
        now: Instant,
    ) -> (Answer, Instant) {
        let from = match (from.is_empty(), address(from)) {
            (true, _) => None,
            (false, Some(from)) => Some(from),
            (false, None) => return no("admind gives the caller's address, or nothing", now),
        };
        if let Some(why) = from.and_then(|from| self.blocked_why(from)) {
            return no(why, now);
        }
        match find(&self.console.passes, token, from, unix_now(), now) {
            Ok(found) => {
                let refusal = match files.user(&found.owner) {
                    Some(user) => owner_refusal(files, user),
                    None => Some(format!("there is no account {}", found.owner)),
                };
                if let Some(why) = refusal {
                    return no(format!("the AI client \"{}\" is no use: {why}", found.name), now);
                }
                if let Some(from) = from {
                    self.wrong.remove(&(found.owner.clone(), web_source(from)));
                }
                (Answer::Done(found.answer()), now)
            }
            Err(why) => {
                let Some(from) = from else { return no(why, now) };
                let owner = owner_of(token);
                let (count, next) = self.counted(owner, web_source(from), now);
                audit!(
                    "a wrong AI client token, for {}, from {}; {count} in a row",
                    shown(owner),
                    shown(from)
                );
                let named = (!owner.is_empty()).then_some(owner);
                self.sign_in_failed(files, from, Failed::ConsoleClient, named);
                (Answer::Refused(why), next)
            }
        }
    }

    /// `console-client-of`: whether `owner` has a client `name` that may
    /// still connect, answered as `console-client` is.
    pub(super) fn console_client_of(
        files: &Files,
        owner: &str,
        name: &str,
        now: Instant,
    ) -> (Answer, Instant) {
        let gone = || format!("{} has no AI client named {}", shown(owner), shown(name));
        let found = match Records::read(owner) {
            Ok(mut records) => {
                records.current(name, unix_now()).and_then(|client| client.found(owner))
            }
            Err(why) => return no(why, now),
        };
        let Some(found) = found else { return no(gone(), now) };
        match files.user(owner).map(|user| owner_refusal(files, user)) {
            Some(None) => (Answer::Done(found.answer()), now),
            Some(Some(why)) => no(format!("the AI client \"{name}\" is no use: {why}"), now),
            None => no(gone(), now),
        }
    }

    /// `console-client-start`: starts a worker for a client's owner, as
    /// `console-start` does for a session.
    pub(super) fn console_client_start(
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
        let found = match find(&self.console.passes, token, None, unix_now(), now) {
            Ok(found) => found,
            Err(why) => return no(why, now),
        };
        let Some(user) = files.user(&found.owner).cloned() else {
            return no(format!("there is no account {}", found.owner), now);
        };
        if let Some(why) = owner_refusal(files, &user) {
            return no(why, now);
        }
        let groups =
            accounts::other_groups(&user, &files.groups, radix_abi::names::GROUPS_MAX as usize);
        let process = match signin::start_console(names, &user, &groups, &calls, &replies, &log) {
            Ok(process) => process as i32,
            Err(why) => return no(why, now),
        };
        audit!(
            "the AI client \"{}\" of {}'s started worker process {process}",
            found.name,
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
            job: None,
        };
        self.sessions.insert(process, session);
        (Answer::Done(process.to_string()), now)
    }

    /// `console-client-read`: runs one of the console's reads as an
    /// administrator for a client whose owner is one, with the three
    /// descriptors and the pipe client `i` sent. authd starts `admind
    /// --read` with the areas the client was given whole, the action and
    /// its arguments; that program runs the action only if it is a read
    /// in one of those areas (ADR 0691). The client then waits for its
    /// end, as for an elevated command.
    pub(super) fn console_client_read(
        &mut self,
        i: usize,
        files: &Files,
        token: &[u8],
        action: &str,
        arguments: &[u8],
        now: Instant,
    ) -> (Answer, Instant) {
        let mut descriptors = std::mem::take(&mut self.clients[i].descriptors);
        if descriptors.len() != 4 {
            return no("a read comes with its three descriptors and a pipe", now);
        }
        if let Some(why) = read_refusal(action, arguments) {
            return no(why, now);
        }
        let found = match find(&self.console.passes, token, None, unix_now(), now) {
            Ok(found) => found,
            Err(why) => return no(why, now),
        };
        let name = &found.name;
        let user = match files.user(&found.owner) {
            Some(user) => user.clone(),
            None => return no(format!("there is no account {}", found.owner), now),
        };
        if let Some(why) = owner_refusal(files, &user) {
            audit!("refused the AI client \"{name}\" of {}'s a read: {why}", user.name);
            return no(why, now);
        }
        let areas = whole(&found.scopes);
        if areas.is_empty() {
            return no(
                format!("ADM-144: the AI client \"{name}\" reads nothing as an administrator"),
                now,
            );
        }
        let command: Vec<Vec<u8>> = [READ[0], READ[1], &areas.join(","), action]
            .iter()
            .map(|word| word.as_bytes().to_vec())
            .chain([arguments.to_vec()])
            .collect();
        let root = elevate::root_account(files.users.entries().cloned());
        let plan = match elevate::plan(Mode::Command, &command, b"/", &root) {
            Ok(plan) => plan,
            Err(why) => return no(why, now),
        };
        if let Some(why) = console_command_refusal(&command, &plan.program) {
            return no(why, now);
        }
        let env = elevate::environment(&root, &user, "", &[], &plan.shown);
        let relay = descriptors.pop();
        let process = match elevate::start(&descriptors, &plan, &env, None) {
            Ok(process) => process as i32,
            Err(why) => return no(why, now),
        };
        audit!(
            "the AI client \"{name}\" of {}'s reads {} as an administrator: process {process}",
            user.name,
            shown(action)
        );
        let session = Session {
            name: user.name,
            terminal: API.into(),
            from: String::new(),
            number: user.uid,
            group: user.gid,
            elevated: true,
            remote: false,
            recorded: false,
            job: None,
        };
        self.sessions.insert(process, session);
        self.hold(i, process, relay);
        (Answer::Done(process.to_string()), now)
    }

    /// `console-clients-all`: every account's clients, for an
    /// administrator inside elevate.
    pub(super) fn console_clients_all(now: Instant) -> (Answer, Instant) {
        match all(unix_now()) {
            Ok(clients) => (Answer::Done(json!({ "clients": clients }).to_string()), now),
            Err(why) => no(why, now),
        }
    }

    /// `console-client-remove-of`: removes `owner`'s client `name`, for
    /// an administrator inside elevate.
    pub(super) fn console_client_remove_of(
        &mut self,
        files: &Files,
        owner: &str,
        name: &str,
        peer: Peer,
        now: Instant,
    ) -> (Answer, Instant) {
        if let Err(why) = person(files, owner, peer, "removes another account's AI clients") {
            return no(why, now);
        }
        let mut records = match Records::read(owner) {
            Ok(records) => records,
            Err(why) => return no(why, now),
        };
        if !records.remove(name) {
            return no(format!("{owner} has no AI client named {}", shown(name)), now);
        }
        if let Err(why) = records.write(owner) {
            return no(why, now);
        }
        self.console.passes.forget(owner, Some(name));
        audit!(
            "removed the AI client \"{name}\" of {owner}, for process {} (user {})",
            peer.process,
            peer.user
        );
        (Answer::Done(String::new()), now)
    }
}

/// Why `action` and `arguments` are not what a read is asked with, if
/// they are not: an action's name is letters, digits, `.` and `-`, and
/// its arguments are text without a NUL, which ends an argument of a
/// program's.
fn read_refusal(action: &str, arguments: &[u8]) -> Option<String> {
    let named = (1..=ACTION_MAX).contains(&action.len())
        && action.starts_with(|c: char| c.is_ascii_lowercase())
        && action
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'.' | b'-'));
    if !named {
        return Some("that is not the name of an action of the console's".into());
    }
    if arguments.len() > ARGUMENTS_MAX {
        return Some("a read's arguments are at most 16 KiB".into());
    }
    (std::str::from_utf8(arguments).is_err() || arguments.contains(&0))
        .then(|| "a read's arguments are JSON".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    const DAY_ONE: i64 = 1_800_000_000;

    fn scopes(words: &[&str]) -> Vec<String> {
        words.iter().map(|word| (*word).to_string()).collect()
    }

    /// `owner`'s records with one client of each way, added at
    /// [`DAY_ONE`] for 90 days, the token's secret `secret`.
    fn two(owner: &str, secret: &str) -> Records {
        let mut records = Records::default();
        let read = scopes(&["*:read", "services"]);
        let logs = scopes(&["logs"]);
        let ssh = New { name: "workstation", way: Way::Ssh, scopes: &read, days: 90 };
        let https = New { name: "night-watch", way: Way::Https, scopes: &logs, days: 90 };
        assert_eq!(records.add(owner, &ssh, "", DAY_ONE), Ok(DAY_ONE + 90 * DAY));
        assert_eq!(records.add(owner, &https, secret, DAY_ONE), Ok(DAY_ONE + 90 * DAY));
        records
    }

    #[test]
    fn a_clients_file_reads_back_as_it_was_written() {
        let secret = "ab".repeat(TOKEN_SECRET);
        let mut records = two("chase", &secret);
        records.clients[1].used = DAY_ONE + 60;
        records.clients[1].from = "192.0.2.7".into();
        let text = records.text();
        assert_eq!(Records::parse(&text), Ok(Records { clients: records.clients.clone() }));
        assert!(text.starts_with("[[clients]]\nname = \"workstation\"\nway = \"ssh\"\n"), "{text}");
        // The token itself is never kept: its SHA-256 is.
        assert!(!text.contains(&secret), "{text}");
        assert_eq!(records.clients[0].hash, "");
        assert_eq!(records.clients[1].hash.len(), 64);
        // No clients, no text, and a file that names something else is
        // refused, not read as nothing.
        assert_eq!(Records::default().text(), "");
        assert_eq!(Records::parse(""), Ok(Records::default()));
        assert!(Records::parse("[[clients]]\nname = \"a\"\n").is_err());
        assert!(Records::parse(&format!("{text}\n[[tokens]]\nname = \"a\"\n")).is_err());
    }

    #[test]
    fn names_and_scopes() {
        assert!(name_ok("workstation") && name_ok("ci-2") && name_ok(&"a".repeat(32)));
        assert!(!name_ok("") && !name_ok("Workstation") && !name_ok("a_b"));
        assert!(!name_ok(&"a".repeat(33)));
        // What `admin mcp` reads as what to do names no client.
        for word in RESERVED {
            assert!(!name_ok(word), "{word}");
        }
        for scope in ["*", "*:read", "services", "services:read", "private-network"] {
            assert!(scope_ok(scope), "{scope}");
        }
        for scope in ["", ":read", "Services", "services:write", "services:read:read", "a b", "*x"]
        {
            assert!(!scope_ok(scope), "{scope}");
        }
        assert!(!scope_ok(&"a".repeat(33)));
    }

    #[test]
    fn a_client_is_added_within_its_limits() {
        fn over_ssh<'a>(name: &'a str, scopes: &'a [String], days: u32) -> New<'a> {
            New { name, way: Way::Ssh, scopes, days }
        }
        let all = scopes(&["*"]);
        let new = |name, days| over_ssh(name, &all, days);
        let mut records = two("chase", &"ab".repeat(TOKEN_SECRET));
        let refused = |records: &mut Records, new: &New<'_>| {
            let before = records.clients.len();
            let why = records.add("chase", new, "", DAY_ONE).unwrap_err();
            assert_eq!(records.clients.len(), before, "{why}");
            why
        };
        assert!(refused(&mut records, &new("list", 30)).contains("name is 1 to 32"));
        assert!(refused(&mut records, &new("a", 0)).contains("1 to 366 days"));
        assert!(refused(&mut records, &new("a", 367)).contains("1 to 366 days"));
        assert!(refused(&mut records, &new("workstation", 30)).contains("has an AI client named"));
        let none: Vec<String> = Vec::new();
        let many = scopes(&["logs"; SCOPES_MAX + 1]);
        let wrong = scopes(&["logs", "logs:write"]);
        for scopes in [&none, &many, &wrong] {
            let new = New { name: "a", way: Way::Https, scopes, days: 30 };
            assert!(refused(&mut records, &new).contains("scopes are the names of areas"));
        }
        // 16 at most.
        for n in 2..CLIENTS_MAX {
            let name = format!("client-{n}");
            assert!(records.add("chase", &over_ssh(&name, &all, 366), "", DAY_ONE).is_ok());
        }
        assert!(refused(&mut records, &new("one-more", 30)).contains("has 16 AI clients"));
        // One that ended stays in the list for 30 days, and its name
        // stays taken; then it goes, and there is room again.
        let ended = DAY_ONE + 90 * DAY;
        let kept = ended + EXPIRED_KEPT - 1;
        let why = records.add("chase", &new("one-more", 30), "", kept).unwrap_err();
        assert!(why.contains("has 16 AI clients"), "{why}");
        let why = records.add("chase", &new("workstation", 30), "", kept).unwrap_err();
        assert!(why.contains("has an AI client named"), "{why}");
        let gone = ended + EXPIRED_KEPT;
        assert_eq!(records.add("chase", &new("workstation", 30), "", gone), Ok(gone + 30 * DAY));
        assert_eq!(records.clients.len(), CLIENTS_MAX - 1);
    }

    #[test]
    fn tokens_and_passes_are_told_apart_by_how_they_start() {
        let secret = "0f".repeat(TOKEN_SECRET);
        let token = format!("dkc_chase_{secret}");
        let pass = format!("dkp_first_last_{secret}");
        assert_eq!(parts(token.as_bytes()), Some((Kind::Token, "chase", secret.as_str())));
        assert_eq!(parts(pass.as_bytes()), Some((Kind::Pass, "first_last", secret.as_str())));
        assert_eq!(owner_of(token.as_bytes()), "chase");
        assert!(is_clients(token.as_bytes()) && is_clients(pass.as_bytes()));
        // An API token is neither, and a client's is no API token.
        let api = format!("{}chase_{secret}", super::super::TOKEN_PREFIX);
        assert_eq!(parts(api.as_bytes()), None);
        assert!(!is_clients(api.as_bytes()));
        assert_eq!(super::super::token_owner(token.as_bytes()), None);
        assert_eq!(super::super::token_owner(pass.as_bytes()), None);
        for wrong in [
            "dkc_chase".to_string(),
            format!("dkc_chase_{}", &secret[2..]),
            format!("dkc_chase_{}", secret.to_uppercase()),
            format!("dkc__{secret}"),
            format!("dkc_../x_{secret}"),
            format!("DKC_chase_{secret}"),
        ] {
            assert_eq!(parts(wrong.as_bytes()), None, "{wrong}");
            assert_eq!(owner_of(wrong.as_bytes()), "");
        }
        assert_eq!(parts(b"dkc_chase_\xff"), None);
    }

    #[test]
    fn a_token_finds_its_client_until_the_client_ends() {
        let secret = "ab".repeat(TOKEN_SECRET);
        let other = "cd".repeat(TOKEN_SECRET);
        let mut records = two("chase", &secret);
        let passes = Passes::default();
        let now = Instant::now();
        let ends = DAY_ONE + 90 * DAY;
        let mut name = |secret: &str, unix| {
            holder(&mut records, &passes, (Kind::Token, "chase", secret), unix, now)
                .map(|client| client.name.clone())
        };
        assert_eq!(name(&secret, DAY_ONE).as_deref(), Some("night-watch"));
        assert_eq!(name(&secret, ends - 1).as_deref(), Some("night-watch"));
        assert_eq!(name(&secret, ends), None);
        assert_eq!(name(&other, DAY_ONE), None);
        // The client that connects over SSH has no token: the SHA-256 of
        // nothing is not its hash, and a token's secret is no pass.
        assert_eq!(name("", DAY_ONE), None);
        assert_eq!(
            holder(&mut records, &passes, (Kind::Pass, "chase", &secret), DAY_ONE, now),
            None
        );
    }

    #[test]
    fn a_pass_finds_its_client_for_12_hours() {
        let secret = "ab".repeat(TOKEN_SECRET);
        let mut records = two("chase", &"cd".repeat(TOKEN_SECRET));
        let mut passes = Passes::default();
        let start = Instant::now();
        passes.give("chase", "workstation", &secret, start);
        let name = |records: &mut Records, passes: &Passes, owner, unix, now| {
            holder(records, passes, (Kind::Pass, owner, &secret), unix, now)
                .map(|client| client.name.clone())
        };
        let last = start + PASS_FOR - Duration::from_secs(1);
        assert_eq!(
            name(&mut records, &passes, "chase", DAY_ONE, start).as_deref(),
            Some("workstation")
        );
        assert_eq!(
            name(&mut records, &passes, "chase", DAY_ONE, last).as_deref(),
            Some("workstation")
        );
        assert_eq!(name(&mut records, &passes, "chase", DAY_ONE, start + PASS_FOR), None);
        // It is its owner's alone, it is no token, and it ends with the
        // client, however new it is.
        assert_eq!(name(&mut records, &passes, "alice", DAY_ONE, start), None);
        assert_eq!(
            holder(&mut records, &passes, (Kind::Token, "chase", &secret), DAY_ONE, start),
            None
        );
        assert_eq!(name(&mut records, &passes, "chase", DAY_ONE + 90 * DAY, start), None);
        // A pass for a client that connects over HTTPS gives nothing,
        // should one ever be kept.
        passes.give("chase", "night-watch", &secret, start);
        assert_eq!(name(&mut records, &passes, "chase", DAY_ONE, start), None);
        // A pass goes when its client is removed, and when the account
        // does.
        passes.give("chase", "workstation", &secret, start);
        passes.forget("chase", Some("night-watch"));
        assert_eq!(passes.client("chase", &secret, start), Some("workstation"));
        passes.forget("alice", None);
        assert_eq!(passes.client("chase", &secret, start), Some("workstation"));
        passes.forget("chase", Some("workstation"));
        assert_eq!(passes.client("chase", &secret, start), None);
        passes.give("chase", "workstation", &secret, start);
        passes.forget("chase", None);
        assert_eq!(passes.client("chase", &secret, start), None);
        // One that was removed and never forgotten still finds nothing.
        passes.give("chase", "workstation", &secret, start);
        assert!(records.remove("workstation") && !records.remove("workstation"));
        assert_eq!(name(&mut records, &passes, "chase", DAY_ONE, start), None);
    }

    #[test]
    fn a_client_has_eight_passes_at_once_and_the_oldest_goes() {
        let mut passes = Passes::default();
        let start = Instant::now();
        let secret = |n: usize| format!("{n:064x}");
        for n in 0..=PASSES_EACH {
            passes.give("chase", "workstation", &secret(n), start + Duration::from_secs(n as u64));
        }
        passes.give("chase", "other", &secret(100), start);
        let now = start + Duration::from_secs(60);
        assert_eq!(passes.client("chase", &secret(0), now), None);
        for n in 1..=PASSES_EACH {
            assert_eq!(passes.client("chase", &secret(n), now), Some("workstation"), "{n}");
        }
        assert_eq!(passes.client("chase", &secret(100), now), Some("other"));
        // Those that are over are forgotten when authd tidies.
        passes.tidy(start + PASS_FOR + Duration::from_secs(1));
        assert_eq!(passes.given.len(), PASSES_EACH - 1);
        passes.tidy(start + PASS_FOR + Duration::from_secs(60));
        assert!(passes.given.is_empty());
    }

    #[test]
    fn the_areas_given_whole_are_what_is_read_as_an_administrator() {
        assert_eq!(
            whole(&scopes(&["*:read", "services", "logs:read", "logs"])),
            ["services", "logs"]
        );
        assert_eq!(whole(&scopes(&["*"])), ["*"]);
        assert!(whole(&scopes(&["*:read", "logs:read"])).is_empty());
    }

    #[test]
    fn a_reads_action_and_arguments_are_checked() {
        assert_eq!(read_refusal("logs.read-all", br#"{"service":"nginx"}"#), None);
        assert_eq!(read_refusal("storage.health", b""), None);
        for action in ["", "Logs.read", "logs read", "logs/read", "-x", "--read", ".x"] {
            let why = read_refusal(action, b"{}").unwrap_or_default();
            assert!(why.contains("not the name of an action"), "{action}: {why}");
        }
        assert!(read_refusal(&"a".repeat(ACTION_MAX), b"{}").is_none());
        assert!(read_refusal(&"a".repeat(ACTION_MAX + 1), b"{}").is_some());
        assert!(read_refusal("a.b", &vec![b' '; ARGUMENTS_MAX]).is_none());
        assert!(read_refusal("a.b", &vec![b' '; ARGUMENTS_MAX + 1]).is_some());
        assert!(read_refusal("a.b", b"{\"a\":\"\0\"}").is_some());
        assert!(read_refusal("a.b", b"\xff").is_some());
    }

    #[test]
    fn a_clients_use_is_recorded_once_an_hour() {
        let mut records = two("chase", &"ab".repeat(TOKEN_SECRET));
        let client = &mut records.clients[1];
        assert!(client.used_at(DAY_ONE, Some("192.0.2.7")));
        assert!(!client.used_at(DAY_ONE + USED_EVERY - 1, Some("192.0.2.8")));
        assert_eq!((client.used, client.from.as_str()), (DAY_ONE, "192.0.2.7"));
        assert!(client.used_at(DAY_ONE + USED_EVERY, Some("192.0.2.8")));
        assert_eq!((client.used, client.from.as_str()), (DAY_ONE + USED_EVERY, "192.0.2.8"));
        // Where from stays as it was when it is not known.
        assert!(client.used_at(DAY_ONE + 2 * USED_EVERY, None));
        assert_eq!(client.from, "192.0.2.8");
    }

    #[test]
    fn a_list_and_an_answer_tell_no_secret() {
        let secret = "ab".repeat(TOKEN_SECRET);
        let records = two("chase", &secret);
        let list = json!(records.json("chase", DAY_ONE + 90 * DAY)).to_string();
        assert!(!list.contains(&secret) && !list.contains("hash"), "{list}");
        assert!(!list.contains(&records.clients[1].hash), "{list}");
        assert!(list.contains(r#""name":"night-watch""#) && list.contains(r#""ended":true"#));
        let found = records.clients[0].found("chase").expect("a way");
        assert_eq!(
            found.answer(),
            format!("chase workstation ssh *:read,services {}", DAY_ONE + 90 * DAY)
        );
        // A client whose way this authd does not know connects no way.
        let mut unknown = records.clients[0].clone();
        unknown.way = "carrier-pigeon".into();
        assert_eq!(unknown.found("chase"), None);
    }
}
