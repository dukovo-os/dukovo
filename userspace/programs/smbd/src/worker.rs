// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! A share session: `smbd --worker`, as authd(8) starts it for a person
//! whose PC signed in, running as them, with the PC's connection as its
//! descriptor 3 and the session's keys on the pipe that is its 0.
//!
//! Everything a PC sends after it signed in is encrypted with the
//! session's key; anything else is refused, and a message that does not
//! decrypt, or that uses a message number twice, ends the connection, so
//! that no one between the PC and the machine can read, change or replay
//! what it sends. The worker answers each frame's requests, one or a
//! compound of several, in order, as the PC's user: the kernel decides
//! what they may read and change, by the files' owners, groups and modes.
//!
//! A change notification is held until the PC cancels it or closes its
//! folder; smbd does not yet watch folders, so a PC sees what changed
//! when it lists the folder again.

use std::collections::{BTreeSet, HashMap};
use std::fs::{self, File};
use std::io::{self, BufReader};
use std::net::TcpStream;
use std::os::fd::FromRawFd;
use std::path::Path;
use std::process::ExitCode;
use std::time::{Duration, Instant, SystemTime};

use manifest::shares::{self, Shares};
use smb2::crypto::encryption::Cipher;

use crate::crypto::{self, Keys, Sealer, Signing};
use crate::fs::{Kind, Locks, Open, Opens, Tree, access};
use crate::negotiate::Negotiated;
use crate::rpc::{self, Listed, Server};
use crate::state::Handoff;
use crate::sys;
use crate::wire::{self, Header, Out, Status, command, flags, status};

/// The most credits one response grants.
const CREDITS_MAX: u16 = 512;
/// The most message numbers one request may use: a write of
/// [`wire::MAX_IO`] uses 64.
const CHARGE_MAX: u64 = 128;
/// How far past the lowest unused message number a request's may be.
const WINDOW: u64 = 1 << 16;
/// How many message numbers above the lowest unused one are remembered
/// before a gap below them is given up on.
const SEEN_MAX: usize = 4096;
/// The most shares a session connects to at once.
const TREES_MAX: usize = 256;
/// The most files and folders a session holds open.
pub const OPENS_MAX: usize = 4096;
/// The most change notifications a session waits on.
const WAITS_MAX: usize = 256;
/// How often shares.toml is looked at for changes.
const CATALOG_EVERY: Duration = Duration::from_secs(1);

/// The message numbers a session's PC has used (MS-SMB2 3.3.5.2.3):
/// each may be used once, so that a recorded message cannot be sent
/// again.
#[derive(Debug, Default)]
struct Window {
    /// The lowest number not yet used: every one below it was.
    low: u64,
    /// The numbers above `low` already used.
    seen: BTreeSet<u64>,
}

impl Window {
    /// Takes the `count` numbers from `first`, unless one was used or is
    /// out of the window.
    fn take(&mut self, first: u64, count: u64) -> bool {
        let count = count.max(1);
        if count > CHARGE_MAX {
            return false;
        }
        let Some(end) = first.checked_add(count) else { return false };
        if first < self.low || end > self.low.saturating_add(WINDOW) {
            return false;
        }
        if self.seen.range(first..end).next().is_some() {
            return false;
        }
        self.seen.extend(first..end);
        if self.seen.len() > SEEN_MAX {
            // A number the PC skipped long ago: it may not use it now.
            self.low = self.seen.first().copied().unwrap_or(self.low);
        }
        while self.seen.remove(&self.low) {
            self.low += 1;
        }
        true
    }
}

/// shares.toml, as it was when last read.
#[derive(Debug, Default)]
pub struct Catalog {
    /// Its shares.
    pub shares: Shares,
    /// Its modification time.
    stamp: Option<SystemTime>,
    /// When it was last looked at.
    looked: Option<Instant>,
    /// Whether it was read.
    read: bool,
}

impl Catalog {
    /// Reads shares.toml again if it changed since it was read, at most
    /// once in [`CATALOG_EVERY`] unless `now` says so: whether it did.
    pub fn refresh(&mut self, now: bool) -> bool {
        let at = Instant::now();
        if !now && self.looked.is_some_and(|looked| at.duration_since(looked) < CATALOG_EVERY) {
            return false;
        }
        self.looked = Some(at);
        let stamp = fs::metadata(shares::PATH).and_then(|meta| meta.modified()).ok();
        if self.read && stamp == self.stamp {
            return false;
        }
        self.stamp = stamp;
        self.read = true;
        self.shares = read_shares();
        true
    }

    /// What the srvsvc pipe tells of the shares, on machine `name`.
    pub fn server(&self, name: &str) -> Server {
        let mut listed: Vec<Listed> = self
            .shares
            .shares
            .iter()
            .map(|share| Listed {
                name: share.name.clone(),
                kind: rpc::DISK,
                comment: share.comment.clone(),
            })
            .collect();
        listed.push(Listed { name: shares::IPC.into(), kind: rpc::IPC, comment: "IPC".into() });
        Server { name: name.to_uppercase(), shares: listed, names: Box::new(crate::lsa::System) }
    }
}

/// The shares in shares.toml; none when it is not there or cannot be
/// read, which the log says.
pub fn read_shares() -> Shares {
    match fs::read_to_string(shares::PATH) {
        Ok(text) => shares::from_toml(&text).unwrap_or_else(|err| {
            eprintln!("smbd: {}: {err}; no shares until it is mended", shares::PATH);
            Shares::default()
        }),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Shares::default(),
        Err(err) => {
            eprintln!("smbd: cannot read {}: {err}", shares::PATH);
            Shares::default()
        }
    }
}

/// A change notification held for a PC.
#[derive(Debug)]
struct Waiting {
    /// Its async number.
    async_id: u64,
    /// The request's message number.
    message_id: u64,
    /// The request's credit charge.
    credit_charge: u16,
    /// The folder's open.
    open: u64,
}

/// What a request before this one in a compound left for a related one
/// (MS-SMB2 3.3.5.2.7.2).
#[derive(Debug, Default)]
pub struct Chain {
    /// The tree.
    pub tree: u32,
    /// The open, which a FileId of all ones means.
    pub file: Option<u64>,
    /// The request's status, which a related request after a failure
    /// fails with too.
    pub failed: Option<Status>,
    /// The async number the request went pending as.
    pub pending: Option<u64>,
}

/// A request's answer: its status and body.
#[derive(Debug)]
pub struct Reply {
    /// SUCCESS, or a status that still comes with the command's body,
    /// as STATUS_BUFFER_OVERFLOW does.
    pub status: Status,
    /// The body.
    pub body: Vec<u8>,
}

impl Reply {
    /// A successful one with `body`.
    pub fn ok(body: Vec<u8>) -> Reply {
        Reply { status: status::SUCCESS, body }
    }

    /// The four-byte body most commands answer with.
    pub fn empty() -> Reply {
        let mut out = Out::new();
        out.u16(4).u16(0);
        Reply::ok(out.0)
    }
}

/// A share session.
pub struct Worker {
    /// The session's number.
    pub session_id: u64,
    /// Its keys.
    keys: Keys,
    /// How it signs.
    signing: Signing,
    /// How it encrypts.
    cipher: Cipher,
    /// What encrypts its responses.
    sealer: Sealer,
    /// The connection's NEGOTIATE, which VALIDATE_NEGOTIATE_INFO checks.
    pub negotiated: Negotiated,
    /// The message numbers used.
    window: Window,
    /// The shares.
    pub catalog: Catalog,
    /// The shares connected to, by tree number.
    pub trees: HashMap<u32, Tree>,
    /// The last tree number given.
    next_tree: u32,
    /// The open files, folders and pipes.
    pub opens: Opens,
    /// The locks on them.
    pub locks: Locks,
    /// The change notifications held.
    waiting: Vec<Waiting>,
    /// The last async number given.
    next_async: u64,
    /// The machine's name.
    pub server_name: String,
    /// Whether the session ended, by LOGOFF.
    ended: bool,
    /// Why the connection is to end, after a request found it must.
    pub drop_connection: Option<&'static str>,
}

impl Worker {
    /// The session `handoff` describes.
    pub fn new(handoff: Handoff) -> Worker {
        let Handoff { negotiated, session_id, next_message, keys, .. } = handoff;
        let mut catalog = Catalog::default();
        catalog.refresh(true);
        Worker {
            session_id,
            signing: negotiated.signing,
            cipher: negotiated.cipher,
            sealer: Sealer { cipher: negotiated.cipher, count: 0 },
            keys,
            negotiated,
            window: Window { low: next_message, seen: BTreeSet::new() },
            catalog,
            trees: HashMap::new(),
            next_tree: 0,
            opens: Opens::default(),
            locks: Locks::default(),
            waiting: Vec::new(),
            next_async: 0,
            server_name: sys::hostname(),
            ended: false,
            drop_connection: None,
        }
    }

    /// Serves the PC on `stream` until it goes, logs off, or breaks the
    /// protocol.
    pub fn serve(&mut self, stream: &TcpStream) -> io::Result<()> {
        let mut reader = BufReader::with_capacity(256 * 1024, stream);
        let mut writer = stream;
        loop {
            let Some(frame) = wire::read_frame(&mut reader)? else { return Ok(()) };
            let answers = match self.frame(&frame) {
                Ok(answers) => answers,
                Err(why) => {
                    eprintln!("smbd: ended the connection: {why}");
                    return Ok(());
                }
            };
            for answer in answers {
                wire::write_frame(&mut writer, &answer)?;
            }
            if self.ended {
                return Ok(());
            }
        }
    }

    /// Answers one frame: the frames to send back. An error ends the
    /// connection, and says why.
    pub fn frame(&mut self, frame: &[u8]) -> Result<Vec<Vec<u8>>, &'static str> {
        let mut extra = Vec::new();
        self.sync_shares(&mut extra);
        if frame.starts_with(&wire::PROTOCOL_ENCRYPTED) {
            if crypto::sealed_session(frame) != Ok(self.session_id) {
                return Err("a message encrypted for another session");
            }
            let plain = crypto::unseal(frame, &self.keys, self.cipher)
                .map_err(|_| "a message that does not decrypt with the session's key")?;
            let compound = self.compound(&plain, &mut extra)?;
            let mut out = Vec::with_capacity(1 + extra.len());
            if !compound.is_empty() {
                out.push(self.sealer.seal(&self.keys, self.session_id, &compound));
            }
            for message in extra {
                out.push(self.sealer.seal(&self.keys, self.session_id, &message));
            }
            return Ok(out);
        }
        if !frame.starts_with(&wire::PROTOCOL) {
            return Err("a message that is not SMB 2 or 3");
        }
        let header = Header::parse(frame).map_err(|_| "a broken header")?;
        match header.command {
            command::NEGOTIATE => Err("a second NEGOTIATE"),
            command::CANCEL => Ok(Vec::new()),
            _ => {
                if !self.window.take(header.message_id, u64::from(header.credit_charge)) {
                    return Err("a message number used before, or out of the window");
                }
                // Everything after sign-in is encrypted (MS-SMB2 3.3.5.2.9).
                let mut response = header.response(status::ACCESS_DENIED);
                response.credits = grant(&header);
                let mut message = wire::message(&response, &wire::error_body());
                crypto::sign(&mut message, &self.keys.signing, self.signing);
                Ok(vec![message])
            }
        }
    }

    /// Answers the requests in `plain`, one or a compound: the responses,
    /// chained as the requests were. Answers that go on their own, as a
    /// cancelled notification's, go in `extra`.
    fn compound(
        &mut self,
        plain: &[u8],
        extra: &mut Vec<Vec<u8>>,
    ) -> Result<Vec<u8>, &'static str> {
        let mut responses: Vec<Vec<u8>> = Vec::new();
        let mut chain = Chain::default();
        let mut first = true;
        let mut at = 0;
        loop {
            let rest = &plain[at..];
            let header = Header::parse(rest).map_err(|_| "a broken header")?;
            let next = header.next_command as usize;
            let end = if next == 0 {
                plain.len()
            } else if !next.is_multiple_of(8) || next < wire::HEADER || next > rest.len() {
                return Err("a compound whose messages do not chain");
            } else {
                at + next
            };
            let message = &plain[at..end];
            if header.command == command::CANCEL {
                self.cancel(&header, extra);
            } else {
                if !self.window.take(header.message_id, u64::from(header.credit_charge)) {
                    return Err("a message number used before, or out of the window");
                }
                responses.push(self.request(&header, message, &mut chain, first, extra));
                if let Some(why) = self.drop_connection {
                    return Err(why);
                }
                if self.ended {
                    break;
                }
            }
            first = false;
            if next == 0 {
                break;
            }
            at = end;
        }
        let count = responses.len();
        let mut out = Vec::new();
        for (i, mut response) in responses.into_iter().enumerate() {
            if i + 1 < count {
                let padded = response.len().next_multiple_of(8);
                response.resize(padded, 0);
                response[20..24].copy_from_slice(&(padded as u32).to_le_bytes());
            }
            out.extend_from_slice(&response);
        }
        Ok(out)
    }

    /// Answers one request: its response message.
    fn request(
        &mut self,
        header: &Header,
        message: &[u8],
        chain: &mut Chain,
        first: bool,
        extra: &mut Vec<Vec<u8>>,
    ) -> Vec<u8> {
        let related = header.is_related() && !first;
        if !related {
            chain.tree = header.tree_id();
            chain.file = None;
            chain.failed = None;
        }
        chain.pending = None;
        let result = if related && let Some(failed) = chain.failed {
            Err(failed)
        } else if !related && header.session_id != self.session_id {
            Err(status::USER_SESSION_DELETED)
        } else {
            self.dispatch(header, message, chain, extra)
        };
        let reply = match result {
            Ok(reply) => reply,
            Err(code) => Reply { status: code, body: wire::error_body() },
        };
        if is_failure(reply.status) {
            chain.failed = Some(reply.status);
        }
        let mut response = header.response(reply.status);
        response.credits = grant(header);
        response.session_id = self.session_id;
        match chain.pending {
            Some(async_id) => {
                response.flags |= flags::ASYNC_COMMAND;
                response.async_id = async_id;
            }
            None => response.async_id = u64::from(chain.tree) << 32,
        }
        wire::message(&response, &reply.body)
    }

    /// Runs one request's command.
    fn dispatch(
        &mut self,
        header: &Header,
        message: &[u8],
        chain: &mut Chain,
        extra: &mut Vec<Vec<u8>>,
    ) -> Result<Reply, Status> {
        match header.command {
            command::NEGOTIATE => {
                self.drop_connection = Some("a second NEGOTIATE");
                Err(status::INVALID_PARAMETER)
            }
            // One session to a connection: another sign-in starts another.
            command::SESSION_SETUP => Err(status::REQUEST_NOT_ACCEPTED),
            command::LOGOFF => {
                self.close_all(extra);
                self.ended = true;
                Ok(Reply::empty())
            }
            command::TREE_CONNECT => self.tree_connect(message, chain),
            command::TREE_DISCONNECT => {
                if self.trees.remove(&chain.tree).is_none() {
                    return Err(status::NETWORK_NAME_DELETED);
                }
                self.close_tree(chain.tree, extra);
                Ok(Reply::empty())
            }
            command::CREATE => self.create(message, chain),
            command::CLOSE => self.close(message, chain, extra),
            command::FLUSH => self.flush(message, chain),
            command::READ => self.read(message, chain),
            command::WRITE => self.write(message, chain),
            command::LOCK => self.lock(message, chain),
            command::IOCTL => self.ioctl(message, chain),
            command::ECHO => Ok(Reply::empty()),
            command::QUERY_DIRECTORY => self.query_directory(message, chain),
            command::CHANGE_NOTIFY => self.change_notify(header, message, chain),
            command::QUERY_INFO => self.query_info(message, chain),
            command::SET_INFO => self.set_info(message, chain),
            // No oplocks are granted, so none breaks.
            command::OPLOCK_BREAK => Err(status::INVALID_PARAMETER),
            _ => Err(status::NOT_SUPPORTED),
        }
    }

    /// TREE_CONNECT (MS-SMB2 3.3.5.7): connects to a share, by
    /// `\\machine\share`, or IPC$.
    fn tree_connect(&mut self, message: &[u8], chain: &mut Chain) -> Result<Reply, Status> {
        let offset = usize::from(wire::u16_at(message, wire::HEADER + 4)?);
        let length = usize::from(wire::u16_at(message, wire::HEADER + 6)?);
        let path = wire::utf16(wire::slice_at(message, offset, length)?)?;
        let name = path
            .trim_start_matches('\\')
            .split_once('\\')
            .map(|(_, share)| share)
            .filter(|share| !share.is_empty() && !share.contains('\\'))
            .ok_or(status::BAD_NETWORK_NAME)?;
        if self.trees.len() >= TREES_MAX {
            return Err(status::INSUFFICIENT_RESOURCES);
        }
        let tree = if shares::same_name(name, shares::IPC) {
            Tree { name: shares::IPC.into(), root: None, read_only: false }
        } else {
            self.catalog.refresh(true);
            let share = self.catalog.shares.find(name).ok_or(status::BAD_NETWORK_NAME)?;
            let root = fs::canonicalize(&share.folder).map_err(|_| status::BAD_NETWORK_NAME)?;
            if !root.is_dir() {
                return Err(status::BAD_NETWORK_NAME);
            }
            if !sys::may(&root, libc::R_OK | libc::X_OK) {
                return Err(status::ACCESS_DENIED);
            }
            Tree { name: share.name.clone(), root: Some(root), read_only: share.read_only }
        };
        let (kind, share_flags, maximal) = match &tree.root {
            // SMB2_SHAREFLAG_NO_CACHING.
            None => (2u8, 0x0000_0030u32, access::ALL),
            Some(root) => (1u8, 0u32, maximal_access(root, tree.read_only, false)),
        };
        loop {
            self.next_tree = self.next_tree.wrapping_add(1);
            if self.next_tree != 0
                && self.next_tree != u32::MAX
                && !self.trees.contains_key(&self.next_tree)
            {
                break;
            }
        }
        self.trees.insert(self.next_tree, tree);
        chain.tree = self.next_tree;
        let mut out = Out::new();
        // SMB2_SHAREFLAG_ENCRYPT_DATA: the session's encryption holds for
        // every share.
        out.u16(16).u8(kind).u8(0).u32(share_flags | 0x0000_8000).u32(0).u32(maximal);
        Ok(Reply::ok(out.0))
    }

    /// The tree a request is for, or STATUS_NETWORK_NAME_DELETED.
    pub fn tree(&self, chain: &Chain) -> Result<&Tree, Status> {
        self.trees.get(&chain.tree).ok_or(status::NETWORK_NAME_DELETED)
    }

    /// The open a request names by its FileId at `at`: the one before it
    /// in a compound for all ones.
    pub fn file_id(&self, message: &[u8], at: usize, chain: &mut Chain) -> Result<u64, Status> {
        let persistent = wire::u64_at(message, at)?;
        let volatile = wire::u64_at(message, at + 8)?;
        let id = if persistent == u64::MAX && volatile == u64::MAX {
            chain.file.ok_or(status::INVALID_PARAMETER)?
        } else if persistent == volatile {
            volatile
        } else {
            return Err(status::FILE_CLOSED);
        };
        self.tree(chain)?;
        let open = self.opens.all.get(&id).ok_or(status::FILE_CLOSED)?;
        if open.tree != chain.tree {
            return Err(status::FILE_CLOSED);
        }
        chain.file = Some(id);
        Ok(id)
    }

    /// The open `id`, which [`Worker::file_id`] found.
    pub fn open_mut(&mut self, id: u64) -> Result<&mut Open, Status> {
        self.opens.all.get_mut(&id).ok_or(status::FILE_CLOSED)
    }

    /// CHANGE_NOTIFY (MS-SMB2 3.3.5.19): held until cancelled or its
    /// folder is closed.
    fn change_notify(
        &mut self,
        header: &Header,
        message: &[u8],
        chain: &mut Chain,
    ) -> Result<Reply, Status> {
        let id = self.file_id(message, wire::HEADER + 8, chain)?;
        let open = self.open_mut(id)?;
        if !matches!(open.kind, Kind::Directory) {
            return Err(status::INVALID_PARAMETER);
        }
        if open.access & access::READ_DATA == 0 {
            return Err(status::ACCESS_DENIED);
        }
        if self.waiting.len() >= WAITS_MAX {
            return Err(status::INSUFFICIENT_RESOURCES);
        }
        self.next_async += 1;
        let async_id = self.next_async;
        self.waiting.push(Waiting {
            async_id,
            message_id: header.message_id,
            credit_charge: header.credit_charge,
            open: id,
        });
        chain.pending = Some(async_id);
        Ok(Reply { status: status::PENDING, body: wire::error_body() })
    }

    /// CANCEL (MS-SMB2 3.3.5.16): ends the held request it names, which
    /// answers STATUS_CANCELLED. A CANCEL itself has no answer.
    fn cancel(&mut self, header: &Header, extra: &mut Vec<Vec<u8>>) {
        let at = self.waiting.iter().position(|waiting| {
            if header.is_async() {
                waiting.async_id == header.async_id
            } else {
                waiting.message_id == header.message_id
            }
        });
        if let Some(at) = at {
            let waiting = self.waiting.remove(at);
            extra.push(self.finished(&waiting, status::CANCELLED));
        }
    }

    /// The final answer to a held notification, with `code`.
    fn finished(&self, waiting: &Waiting, code: Status) -> Vec<u8> {
        let header = Header {
            credit_charge: waiting.credit_charge,
            status: code,
            command: command::CHANGE_NOTIFY,
            credits: 0,
            flags: flags::SERVER_TO_REDIR | flags::ASYNC_COMMAND,
            next_command: 0,
            message_id: waiting.message_id,
            async_id: waiting.async_id,
            session_id: self.session_id,
            signature: [0; 16],
        };
        let body = if code == status::NOTIFY_CLEANUP {
            let mut out = Out::new();
            out.u16(9).u16(0).u32(0).u8(0);
            out.0
        } else {
            wire::error_body()
        };
        wire::message(&header, &body)
    }

    /// Ends the notifications held on open `id`, which was closed.
    pub fn end_waits(&mut self, id: u64, extra: &mut Vec<Vec<u8>>) {
        let (ended, kept) =
            std::mem::take(&mut self.waiting).into_iter().partition(|w| w.open == id);
        self.waiting = kept;
        for waiting in ended {
            extra.push(self.finished(&waiting, status::NOTIFY_CLEANUP));
        }
    }

    /// Closes open `id`: lets go of its locks, ends what waits on it,
    /// and deletes it if it was to be. Whether it was there.
    pub fn close_open(&mut self, id: u64, extra: &mut Vec<Vec<u8>>) -> Option<Open> {
        let mut open = self.opens.all.remove(&id)?;
        self.locks.drop_open(id);
        self.end_waits(id, extra);
        open.file = None;
        if open.delete_on_close {
            // The last open of a file deletes it: another still open
            // takes the duty over.
            let other = self
                .opens
                .all
                .values_mut()
                .find(|other| other.path == open.path && !matches!(other.kind, Kind::Pipe(_)));
            match other {
                Some(other) => other.delete_on_close = true,
                None => {
                    let removed = match open.kind {
                        Kind::Directory => fs::remove_dir(&open.path),
                        _ => fs::remove_file(&open.path),
                    };
                    if let Err(err) = removed {
                        eprintln!("smbd: cannot delete {}: {err}", open.path.display());
                    }
                }
            }
        }
        Some(open)
    }

    /// Closes every open of tree `tree`.
    fn close_tree(&mut self, tree: u32, extra: &mut Vec<Vec<u8>>) {
        let ids: Vec<u64> = self
            .opens
            .all
            .iter()
            .filter(|(_, open)| open.tree == tree)
            .map(|(&id, _)| id)
            .collect();
        for id in ids {
            self.close_open(id, extra);
        }
    }

    /// Closes everything, as LOGOFF does.
    fn close_all(&mut self, extra: &mut Vec<Vec<u8>>) {
        let ids: Vec<u64> = self.opens.all.keys().copied().collect();
        for id in ids {
            self.close_open(id, extra);
        }
        self.trees.clear();
    }

    /// Reads shares.toml again when it changed, and disconnects the
    /// trees of shares removed, or whose folder or read-only changed: the
    /// PC then hears STATUS_NETWORK_NAME_DELETED and connects again.
    fn sync_shares(&mut self, extra: &mut Vec<Vec<u8>>) {
        if !self.catalog.refresh(false) {
            return;
        }
        let gone: Vec<u32> = self
            .trees
            .iter()
            .filter(|(_, tree)| {
                let Some(root) = &tree.root else { return false };
                match self.catalog.shares.find(&tree.name) {
                    None => true,
                    Some(share) => {
                        share.read_only != tree.read_only
                            || fs::canonicalize(&share.folder).ok().as_deref()
                                != Some(root.as_path())
                    }
                }
            })
            .map(|(&id, _)| id)
            .collect();
        for id in gone {
            self.trees.remove(&id);
            self.close_tree(id, extra);
        }
    }
}

/// Whether `code` is a failure, which a related request after it fails
/// with too: an error, not a warning such as STATUS_BUFFER_OVERFLOW.
fn is_failure(code: Status) -> bool {
    code >> 30 == 3
}

/// The credits a response grants: those asked for, at least one.
fn grant(header: &Header) -> u16 {
    header.credits.clamp(1, CREDITS_MAX)
}

/// The most a person may do to `path` (MS-SMB2 3.3.5.9's MaximalAccess),
/// as its modes and the share allow, for their user. `parent_writable`
/// says whether they may delete it.
pub fn maximal_access(path: &Path, read_only: bool, parent_writable: bool) -> u32 {
    let mut mask =
        access::READ_ATTRIBUTES | access::READ_EA | access::READ_CONTROL | access::SYNCHRONIZE;
    if sys::may(path, libc::R_OK) {
        mask |= access::READ_DATA;
    }
    if sys::may(path, libc::W_OK) {
        mask |= access::WRITE_DATA
            | access::APPEND_DATA
            | access::WRITE_EA
            | access::WRITE_ATTRIBUTES
            | access::DELETE_CHILD;
    }
    if sys::may(path, libc::X_OK) {
        mask |= access::EXECUTE;
    }
    if fs::metadata(path).is_ok_and(|meta| std::os::unix::fs::MetadataExt::uid(&meta) == sys::uid())
    {
        // The owner sets its times and modes.
        mask |= access::WRITE_ATTRIBUTES | access::WRITE_DAC;
    }
    if parent_writable {
        mask |= access::DELETE;
    }
    if read_only {
        mask &= !access::CHANGES;
    }
    mask
}

/// `smbd --worker`: serves the session authd started it for.
pub fn run() -> ExitCode {
    if !sys::is_open(0) || !sys::is_open(3) {
        eprintln!("smbd: --worker is started by authd, with a session's pipe and connection");
        return ExitCode::FAILURE;
    }
    // SAFETY: authd starts the worker with the session's pipe as its
    // descriptor 0, which nothing else here owns.
    let mut pipe = unsafe { File::from_raw_fd(0) };
    let handoff = match Handoff::receive(&mut pipe) {
        Ok(handoff) => handoff,
        Err(err) => {
            eprintln!("smbd: cannot read the session's handoff: {err}");
            return ExitCode::FAILURE;
        }
    };
    drop(pipe);
    // SAFETY: and the PC's connection as its descriptor 3.
    let stream = unsafe { TcpStream::from_raw_fd(3) };
    sys::keep_alive(&stream);
    let _ = stream.set_nodelay(true);
    eprintln!("smbd: {} signed in from {}", handoff.user, handoff.from);
    let mut worker = Worker::new(handoff);
    match worker.serve(&stream) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("smbd: the connection ended: {err}");
            ExitCode::SUCCESS
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn message_numbers_are_used_once() {
        let mut window = Window { low: 5, seen: BTreeSet::new() };
        assert!(window.take(5, 1));
        assert!(!window.take(5, 1));
        assert!(!window.take(4, 1));
        assert!(window.take(7, 2));
        assert!(!window.take(8, 1));
        assert!(window.take(6, 0));
        assert_eq!((window.low, window.seen.len()), (9, 0));
        assert!(!window.take(9 + WINDOW, 1));
        assert!(!window.take(9, CHARGE_MAX + 1));
        assert!(!window.take(u64::MAX, 2));
    }

    #[test]
    fn a_long_gap_is_given_up() {
        let mut window = Window { low: 0, seen: BTreeSet::new() };
        for id in 1..=SEEN_MAX as u64 + 1 {
            assert!(window.take(id, 1));
        }
        assert_eq!(window.low, SEEN_MAX as u64 + 2);
        assert!(!window.take(0, 1));
    }

    #[test]
    fn failures_are_errors_only() {
        assert!(is_failure(status::ACCESS_DENIED));
        assert!(!is_failure(status::BUFFER_OVERFLOW));
        assert!(!is_failure(status::SUCCESS));
        assert!(!is_failure(status::PENDING));
    }
}
