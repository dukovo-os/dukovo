// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! NFS 4.1 and 4.2's COMPOUND (RFC 8881, RFC 7862): its operations in
//! turn, each on the file the one before left current, and the sessions
//! that order them. The files' own operations are in `ops`; this has the
//! compound, its sessions and its clients.

use std::net::IpAddr;
use std::time::{Instant, SystemTime};

use crate::fh::{Fh, Places};
use crate::ipc::{Answer, Request, Stat, Time};
use crate::nfs::{err, op};
use crate::rpc::{AUTH_SYS, RECORD_MAX};
use crate::shares::{self, NfsShare};
use crate::state::{Channel, Client, Handle, Session, Slot, State, Stateid};
use crate::xdr::{self, Reader, Writer};

/// What does the work on files: the workers, each as its person.
pub trait Backend {
    /// Serves `request` in the worker of the person numbered `uid`.
    fn call(&mut self, uid: u32, request: Request) -> Answer;
    /// Ends every worker, when the shares change: each starts again
    /// with the shares as they are.
    fn restart(&mut self);
}

/// Who calls, and from where.
#[derive(Clone, Copy, Debug)]
pub struct Context {
    /// The client's address.
    pub peer: IpAddr,
    /// The person served: AUTH_SYS's user, or no one for root and for a
    /// call with no credential.
    pub uid: u32,
}

/// The most operations in a COMPOUND, as CREATE_SESSION grants them.
const OPS_MAX: u32 = 128;
/// The most slots of a session.
const SLOTS_MAX: u32 = 64;
/// The largest answer kept for a replay: what a client asks to be kept
/// is small (an OPEN's, a CREATE's), and the slots of every session hold
/// at most [`SESSIONS_MAX`] times [`SLOTS_MAX`] of them.
const CACHED_MAX: u32 = 8 * 1024;
/// The most sessions, of all clients.
const SESSIONS_MAX: usize = 128;
/// The most sessions of one client.
const CLIENT_SESSIONS_MAX: usize = 8;
/// The most client records.
const CLIENTS_MAX: usize = 512;
/// The longest tag a COMPOUND carries.
const TAG_MAX: usize = 1024;

/// EXCHANGE_ID's flags: no pNFS, and a record that was confirmed.
const USE_NON_PNFS: u32 = 0x0001_0000;
const CONFIRMED_R: u32 = 0x8000_0000;
/// The client asks to update a confirmed record.
const UPD_CONFIRMED_REC_A: u32 = 0x4000_0000;

/// Where a COMPOUND is: a file, or the root.
#[derive(Clone, Debug)]
pub enum Here {
    /// The root clients mount from.
    Root,
    /// A file in a share.
    File {
        /// The share, by its place in the server's list.
        share: usize,
        /// The file's path, as the worker reaches it.
        path: Vec<u8>,
        /// Its filehandle: the share's for its folder.
        fh: Fh,
        /// Its stat, while nothing changed it.
        stat: Option<Stat>,
    },
}

/// What a COMPOUND carries from one operation to the next.
pub struct Cur {
    /// The person served.
    pub uid: u32,
    /// The client's address.
    pub peer: IpAddr,
    /// The current filehandle's file.
    pub here: Option<Here>,
    /// The saved one's.
    pub saved: Option<Here>,
    /// The current stateid.
    pub stateid: Option<Stateid>,
    /// The saved one.
    pub saved_stateid: Option<Stateid>,
    /// The client whose session the COMPOUND is in.
    pub client: Option<u64>,
}

/// Why an operation failed: its status, and what its answer says then,
/// for the few that say more than the status.
pub enum Fail {
    /// The status alone.
    Status(u32),
    /// The status and the rest of the answer.
    With(u32, Writer),
}

impl From<u32> for Fail {
    fn from(status: u32) -> Fail {
        Fail::Status(status)
    }
}

impl From<xdr::Bad> for Fail {
    fn from(_: xdr::Bad) -> Fail {
        Fail::Status(err::BADXDR)
    }
}

/// An operation's answer after its status: its results, or why it
/// failed.
pub type Done = Result<Writer, Fail>;

/// SEQUENCE's outcome: a new request in a slot, or an answer to send
/// again.
enum Sequenced {
    New { body: Writer, session: [u8; 16], slot: usize, keep: bool },
    Replay(Vec<u8>),
}

/// The NFS server.
pub struct Server<B> {
    /// The workers.
    pub backend: B,
    /// The shares NFS serves.
    pub shares: Vec<NfsShare>,
    /// When shares.toml last changed, as last read: none to keep the
    /// shares given.
    shares_seen: Option<Option<SystemTime>>,
    /// The clients' state.
    pub state: State,
    /// Where files were seen.
    pub places: Places,
    /// This run's write verifier: a client that sees another writes
    /// again what it did not COMMIT.
    pub verifier: [u8; 8],
    /// The server's owner and scope, for EXCHANGE_ID: the same for each
    /// run on this computer.
    owner: Vec<u8>,
    /// Whether the workers reach each share at `/.nfs-` and its key, as
    /// authd attaches it, or at its folder, in the tests.
    pub attached: bool,
    /// When nfsd started: the root's times while shares.toml has none.
    pub started: Time,
    /// When the leases were last looked at.
    swept: Instant,
}

impl<B: Backend> Server<B> {
    /// A server with `backend`, for the run numbered `boot`, with the
    /// write verifier `verifier` and `owner`'s name for itself, reading
    /// shares.toml as it changes.
    pub fn new(backend: B, boot: u32, verifier: [u8; 8], owner: &str) -> Server<B> {
        let now = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).unwrap_or_default();
        Server {
            backend,
            shares: Vec::new(),
            shares_seen: Some(None),
            state: State::new(boot),
            places: Places::default(),
            verifier,
            owner: format!("dukovo:{owner}").into_bytes(),
            attached: true,
            started: Time { seconds: now.as_secs().cast_signed(), nanos: now.subsec_nanos() },
            swept: Instant::now(),
        }
    }

    /// Serves `shares` alone, at their folders, as the tests do.
    #[cfg(test)]
    pub fn with_shares(mut self, shares: Vec<NfsShare>) -> Server<B> {
        self.shares = shares;
        self.shares_seen = None;
        self.attached = false;
        self
    }

    /// Reads shares.toml again if it changed, and starts the workers
    /// again for the shares it has now; and lets the clients whose leases
    /// ran out go.
    fn refresh(&mut self, now: Instant) {
        if let Some(seen) = self.shares_seen {
            let changed = shares::changed();
            if changed != seen {
                self.shares_seen = Some(changed);
                let shares = shares::load();
                if shares != self.shares {
                    self.shares = shares;
                    self.backend.restart();
                    self.state.forget_handles();
                    self.places.forget_missing();
                }
            }
        }
        if now.duration_since(self.swept).as_secs() >= 1 {
            self.swept = now;
            let handles = self.state.expire(now);
            self.close_handles(handles);
        }
    }

    /// The root's times: when shares.toml last changed, or when nfsd
    /// started.
    pub fn root_time(&self) -> Time {
        match self.shares_seen {
            Some(Some(changed)) => {
                let since = changed.duration_since(SystemTime::UNIX_EPOCH).unwrap_or_default();
                Time { seconds: since.as_secs().cast_signed(), nanos: since.subsec_nanos() }
            }
            _ => self.started,
        }
    }

    /// Whether a client at `peer` may reach any share: one that may not
    /// is not served at all.
    pub fn reaches(&mut self, peer: IpAddr) -> bool {
        self.refresh(Instant::now());
        self.shares.iter().any(|share| share.allows(peer))
    }

    /// Closes the workers' handles of states that ended.
    pub fn close_handles(&mut self, handles: Vec<Handle>) {
        for (uid, handle) in handles {
            let _ = self.backend.call(uid, Request::Close(handle));
        }
    }

    /// Answers a COMPOUND's arguments: its results, or none for
    /// arguments that cannot be read at all.
    pub fn compound(&mut self, cx: &Context, args: &[u8]) -> Option<Vec<u8>> {
        let now = Instant::now();
        self.refresh(now);
        let mut r = Reader::new(args);
        let tag = r.opaque(TAG_MAX).ok()?.to_vec();
        let minor = r.u32().ok()?;
        let count = r.count(usize::MAX).ok()?;
        if !(1..=2).contains(&minor) {
            return Some(reply(err::MINOR_VERS_MISMATCH, &tag, 0, &[]));
        }
        let mut cur = Cur {
            uid: cx.uid,
            peer: cx.peer,
            here: None,
            saved: None,
            stateid: None,
            saved_stateid: None,
            client: None,
        };
        let mut results = Writer::new();
        let mut done = 0;
        let mut status = err::OK;
        let mut slot = None;
        // An answer to keep for a replay may be no larger than a slot
        // holds.
        let mut room = usize::MAX;
        for i in 0..count {
            let before = results.bytes.len();
            let Ok(code) = r.u32() else {
                results.u32(op::ILLEGAL).u32(err::BADXDR);
                done += 1;
                status = err::BADXDR;
                break;
            };
            let outcome = if code == op::SEQUENCE {
                if i == 0 {
                    match self.sequence(&mut cur, &mut r, count, now) {
                        Ok(Sequenced::Replay(answer)) => return Some(answer),
                        Ok(Sequenced::New { body, session, slot: at, keep }) => {
                            slot = Some((session, at, keep));
                            if keep {
                                room = CACHED_MAX as usize - (12 + tag.len());
                            }
                            Ok(body)
                        }
                        Err(fail) => Err(fail),
                    }
                } else {
                    Err(Fail::Status(err::SEQUENCE_POS))
                }
            } else if i == 0 && !sessionless(code) {
                Err(Fail::Status(err::OP_NOT_IN_SESSION))
            } else if i == 0 && count > 1 {
                Err(Fail::Status(err::NOT_ONLY_OP))
            } else {
                self.operation(code, &mut cur, &mut r, now)
            };
            results.u32(if known(code) { code } else { op::ILLEGAL });
            done += 1;
            match outcome {
                Ok(body) => {
                    results.u32(err::OK);
                    results.bytes.extend_from_slice(&body.bytes);
                }
                Err(Fail::Status(s)) => {
                    results.u32(s);
                    status = s;
                }
                Err(Fail::With(s, body)) => {
                    results.u32(s);
                    results.bytes.extend_from_slice(&body.bytes);
                    status = s;
                }
            }
            if results.bytes.len() > room {
                results.bytes.truncate(before);
                results.u32(if known(code) { code } else { op::ILLEGAL });
                results.u32(err::REP_TOO_BIG_TO_CACHE);
                status = err::REP_TOO_BIG_TO_CACHE;
            }
            if status != err::OK {
                break;
            }
        }
        let answer = reply(status, &tag, done, &results.bytes);
        if let Some((id, at, keep)) = slot
            && let Some(session) = self.state.sessions.get_mut(&id)
        {
            session.slots[at].reply = keep.then(|| answer.clone());
        }
        Some(answer)
    }

    /// One operation, after SEQUENCE.
    fn operation(&mut self, code: u32, cur: &mut Cur, r: &mut Reader, now: Instant) -> Done {
        match code {
            op::EXCHANGE_ID => self.exchange_id(r, now),
            op::CREATE_SESSION => self.create_session(r, now),
            op::DESTROY_SESSION => {
                let id: [u8; 16] = r.array()?;
                self.state.sessions.remove(&id).ok_or(err::BADSESSION)?;
                Ok(Writer::new())
            }
            op::DESTROY_CLIENTID => {
                let id = r.u64()?;
                if !self.state.clients.contains_key(&id) {
                    return Err(err::STALE_CLIENTID.into());
                }
                if self.state.sessions.values().any(|s| s.client == id) {
                    return Err(err::CLIENTID_BUSY.into());
                }
                let handles = self.state.purge(id);
                self.close_handles(handles);
                Ok(Writer::new())
            }
            op::BIND_CONN_TO_SESSION => {
                let id: [u8; 16] = r.array()?;
                let _direction = r.u32()?;
                let _rdma = r.bool()?;
                if !self.state.sessions.contains_key(&id) {
                    return Err(err::BADSESSION.into());
                }
                // CDFS4_FORE: the connection carries calls, not callbacks.
                let mut w = Writer::new();
                w.fixed(&id).u32(1).bool(false);
                Ok(w)
            }
            op::RECLAIM_COMPLETE => {
                let one_fs = r.bool()?;
                let client = cur.client.and_then(|id| self.state.clients.get_mut(&id));
                let client = client.ok_or(err::OP_NOT_IN_SESSION)?;
                if !one_fs {
                    if client.reclaimed {
                        return Err(err::COMPLETE_ALREADY.into());
                    }
                    client.reclaimed = true;
                }
                Ok(Writer::new())
            }
            op::BACKCHANNEL_CTL => {
                let _program = r.u32()?;
                callback_security(r)?;
                Ok(Writer::new())
            }
            _ => self.file_operation(code, cur, r),
        }
    }

    /// SEQUENCE: the request's slot in its session, new or sent again.
    fn sequence(
        &mut self,
        cur: &mut Cur,
        r: &mut Reader,
        ops: usize,
        now: Instant,
    ) -> Result<Sequenced, Fail> {
        let id: [u8; 16] = r.array()?;
        let sequence = r.u32()?;
        let at = r.u32()? as usize;
        let _highest = r.u32()?;
        let keep = r.bool()?;
        let session = self.state.sessions.get_mut(&id).ok_or(err::BADSESSION)?;
        if ops > session.fore.max_ops as usize {
            return Err(err::TOO_MANY_OPS.into());
        }
        let slots = session.slots.len() as u32;
        let slot = session.slots.get_mut(at).ok_or(err::BADSLOT)?;
        if sequence == slot.sequence {
            return slot.reply.clone().map(Sequenced::Replay).ok_or(err::RETRY_UNCACHED_REP.into());
        }
        if sequence != slot.sequence.wrapping_add(1) {
            return Err(err::SEQ_MISORDERED.into());
        }
        slot.sequence = sequence;
        slot.reply = None;
        let client = session.client;
        if let Some(c) = self.state.clients.get_mut(&client) {
            c.renewed = now;
        }
        cur.client = Some(client);
        let mut w = Writer::new();
        w.fixed(&id).u32(sequence).u32(at as u32).u32(slots - 1).u32(slots - 1);
        // sr_status_flags: nothing to say.
        w.u32(0);
        Ok(Sequenced::New { body: w, session: id, slot: at, keep })
    }

    /// EXCHANGE_ID: the client's record, new or found.
    fn exchange_id(&mut self, r: &mut Reader, now: Instant) -> Done {
        let verifier: [u8; 8] = r.array()?;
        let owner = r.opaque(1024)?.to_vec();
        let flags = r.u32()?;
        match r.u32()? {
            // SP4_NONE: the only state protection nfsd offers.
            0 => {}
            _ => return Err(err::NOTSUPP.into()),
        }
        let ids = r.count(1)?;
        for _ in 0..ids {
            r.opaque(1024)?;
            r.opaque(1024)?;
            r.i64()?;
            r.u32()?;
        }
        let records: Vec<(u64, bool, [u8; 8])> = self
            .state
            .clients
            .iter()
            .filter(|(_, c)| c.owner == owner)
            .map(|(id, c)| (*id, c.confirmed, c.verifier))
            .collect();
        let confirmed = records.iter().find(|(_, confirmed, _)| *confirmed);
        let (id, flags_out) = match confirmed {
            Some(&(id, _, v)) if v == verifier => (id, USE_NON_PNFS | CONFIRMED_R),
            Some(_) if flags & UPD_CONFIRMED_REC_A != 0 => return Err(err::NOT_SAME.into()),
            None if flags & UPD_CONFIRMED_REC_A != 0 => return Err(err::NOENT.into()),
            _ => {
                // A client new, or started again: a new record, which the
                // old one's state goes with once it is confirmed.
                for &(old, confirmed, _) in &records {
                    if !confirmed {
                        self.state.clients.remove(&old);
                    }
                }
                if self.state.clients.len() >= CLIENTS_MAX {
                    // Records whose leases end go each second.
                    return Err(err::DELAY.into());
                }
                let id = self.state.new_client_id();
                let client = Client {
                    owner,
                    verifier,
                    confirmed: false,
                    sequence: 1,
                    last_session: None,
                    renewed: now,
                    reclaimed: false,
                };
                self.state.clients.insert(id, client);
                (id, USE_NON_PNFS)
            }
        };
        let client = self.state.clients.get_mut(&id).ok_or(err::SERVERFAULT)?;
        client.renewed = now;
        let mut w = Writer::new();
        w.u64(id).u32(client.sequence).u32(flags_out);
        // SP4_NONE; the server's owner, minor ID 0; its scope.
        w.u32(0).u64(0).opaque(&self.owner).opaque(&self.owner);
        // nfs_impl_id4: who wrote it.
        w.u32(1)
            .opaque(b"dukovo.org")
            .opaque(concat!("nfsd ", env!("CARGO_PKG_VERSION")).as_bytes());
        w.i64(0).u32(0);
        Ok(w)
    }

    /// CREATE_SESSION: a session for a client, which confirms its record.
    fn create_session(&mut self, r: &mut Reader, now: Instant) -> Done {
        let id = r.u64()?;
        let sequence = r.u32()?;
        let _flags = r.u32()?;
        let fore = Channel::read(r)?;
        let back = Channel::read(r)?;
        let _program = r.u32()?;
        callback_security(r)?;
        let client = self.state.clients.get(&id).ok_or(err::STALE_CLIENTID)?;
        if sequence == client.sequence.wrapping_sub(1) {
            let kept = client.last_session.clone().ok_or(err::SEQ_MISORDERED)?;
            return Ok(Writer { bytes: kept });
        }
        if sequence != client.sequence {
            return Err(err::SEQ_MISORDERED.into());
        }
        if fore.max_request < 1024 || fore.max_response < 1024 {
            return Err(err::TOOSMALL.into());
        }
        let theirs = self.state.sessions.values().filter(|s| s.client == id).count();
        if theirs >= CLIENT_SESSIONS_MAX || self.state.sessions.len() >= SESSIONS_MAX {
            return Err(err::NOSPC.into());
        }
        // The record this one replaces, of the same client before it
        // started again, goes now.
        let owner = client.owner.clone();
        let old: Vec<u64> = self
            .state
            .clients
            .iter()
            .filter(|(other, c)| **other != id && c.confirmed && c.owner == owner)
            .map(|(other, _)| *other)
            .collect();
        for other in old {
            let handles = self.state.purge(other);
            self.close_handles(handles);
        }
        let granted = Channel {
            pad: 0,
            max_request: fore.max_request.min(RECORD_MAX as u32),
            max_response: fore.max_response.min(RECORD_MAX as u32),
            max_cached: fore.max_cached.min(CACHED_MAX),
            max_ops: fore.max_ops.clamp(1, OPS_MAX),
            max_requests: fore.max_requests.clamp(1, SLOTS_MAX),
        };
        let session_id = self.state.new_session_id(id);
        let slots = (0..granted.max_requests).map(|_| Slot::default()).collect();
        self.state.sessions.insert(session_id, Session { client: id, slots, fore: granted });
        let mut w = Writer::new();
        // No flags: no back channel, as nfsd has no callbacks to make.
        w.fixed(&session_id).u32(sequence).u32(0);
        granted.write(&mut w);
        back.write(&mut w);
        let client = self.state.clients.get_mut(&id).ok_or(err::SERVERFAULT)?;
        client.confirmed = true;
        client.sequence = sequence.wrapping_add(1);
        client.renewed = now;
        client.last_session = Some(w.bytes.clone());
        Ok(w)
    }
}

/// Whether `code` may come first in a COMPOUND without SEQUENCE: alone.
fn sessionless(code: u32) -> bool {
    matches!(
        code,
        op::EXCHANGE_ID
            | op::CREATE_SESSION
            | op::DESTROY_SESSION
            | op::DESTROY_CLIENTID
            | op::BIND_CONN_TO_SESSION
    )
}

/// Whether `code` is an operation of NFS 4's, any minor version's.
fn known(code: u32) -> bool {
    (op::ACCESS..=op::REMOVEXATTR).contains(&code)
}

/// Reads callback_sec_parms4<>: the callbacks' credentials, which nfsd
/// has no use for.
fn callback_security(r: &mut Reader) -> xdr::Result<()> {
    let n = r.count(16)?;
    for _ in 0..n {
        match r.u32()? {
            0 => {}
            AUTH_SYS => {
                r.u32()?;
                r.opaque(255)?;
                r.u32()?;
                r.u32()?;
                let gids = r.count(16)?;
                for _ in 0..gids {
                    r.u32()?;
                }
            }
            6 => {
                r.u32()?;
                r.opaque(1024)?;
                r.opaque(1024)?;
            }
            _ => return Err(xdr::Bad),
        }
    }
    Ok(())
}

/// COMPOUND4res: the status, the tag, and `done` results.
fn reply(status: u32, tag: &[u8], done: u32, results: &[u8]) -> Vec<u8> {
    let mut w = Writer::new();
    w.u32(status).opaque(tag).u32(done);
    w.bytes.extend_from_slice(results);
    w.bytes
}
