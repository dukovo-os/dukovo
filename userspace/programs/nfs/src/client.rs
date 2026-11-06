// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! A client of NFS 4.1 and 4.2 (RFC 8881, RFC 7862): its client ID and
//! session, made again when the server loses them, and the COMPOUNDs sent
//! through the session's one slot, each after a SEQUENCE.
//!
//! A COMPOUND a connection lost is sent again, on a new connection, in
//! the same slot with the same sequence number, and the server answers it
//! from its reply cache when it was carried out; those that change files
//! ask for their reply to be kept. A server that lost the client's state,
//! after it restarted or the lease ran out, gives a new client ID: the
//! client's [`Client::generation`] counts them, so that files opened
//! before are opened again.

use std::fmt;
use std::io;
use std::net::SocketAddr;
use std::thread;
use std::time::{Duration, Instant};

use nfsd::nfs::{err, op};
use nfsd::xdr::{self, Reader, Writer};

use crate::rpc::{self, Auth, Conn, Cred};
use crate::security::Kerberos;

/// COMPOUND, NFS version 4's one procedure but NULL.
const COMPOUND: u32 = 1;

/// How long a server's DELAY and GRACE are waited out before the call
/// fails: Linux's server's grace period is 90 seconds.
const WAIT_MAX: Duration = Duration::from_secs(180);

/// The most a COMPOUND carries besides a READ's or WRITE's data.
const OVERHEAD: u32 = 4096;

/// What a call failed with: the server's status, the connection's, or
/// Kerberos's refusal of the caller.
#[derive(Debug)]
pub enum Fail {
    Status(u32),
    Io(io::Error),
    /// The caller has no ticket, or the server refused it: what to do.
    Denied(String),
}

impl From<xdr::Bad> for Fail {
    fn from(_: xdr::Bad) -> Fail {
        Fail::Status(err::BADXDR)
    }
}

impl Fail {
    /// The errno that says it.
    pub fn errno(&self) -> i32 {
        match self {
            Fail::Status(status) => errno(*status),
            Fail::Io(e) => match e.kind() {
                io::ErrorKind::PermissionDenied => libc::EACCES,
                io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock => libc::ETIMEDOUT,
                _ => libc::EIO,
            },
            Fail::Denied(_) => libc::EACCES,
        }
    }
}

impl fmt::Display for Fail {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Fail::Status(err::WRONGSEC) => f.write_str(
                "the server takes another security flavor for this folder: mount with the sec= \
                 its export names (sys, krb5, krb5i or krb5p)",
            ),
            Fail::Status(err::MINOR_VERS_MISMATCH) => {
                f.write_str("the server speaks neither NFS 4.2 nor 4.1")
            }
            Fail::Status(status) => {
                let text = io::Error::from_raw_os_error(errno(*status)).to_string();
                let text = text.split(" (os error").next().unwrap_or(&text).to_string();
                write!(f, "{text} (NFS status {status})")
            }
            Fail::Io(e) => write!(f, "{e}"),
            Fail::Denied(why) => f.write_str(why),
        }
    }
}

/// The errno an NFS status stands for.
pub fn errno(status: u32) -> i32 {
    match status {
        err::PERM => libc::EPERM,
        err::NOENT => libc::ENOENT,
        err::NXIO => libc::ENXIO,
        err::ACCESS | err::WRONGSEC | err::SHARE_DENIED => libc::EACCES,
        err::EXIST => libc::EEXIST,
        err::XDEV => libc::EXDEV,
        err::NOTDIR => libc::ENOTDIR,
        err::ISDIR => libc::EISDIR,
        err::INVAL | err::BADNAME | err::BADOWNER | err::BADTYPE | err::WRONG_TYPE => libc::EINVAL,
        err::FBIG => libc::EFBIG,
        err::NOSPC => libc::ENOSPC,
        err::ROFS => libc::EROFS,
        err::MLINK => libc::EMLINK,
        err::NAMETOOLONG => libc::ENAMETOOLONG,
        err::NOTEMPTY => libc::ENOTEMPTY,
        err::DQUOT => libc::EDQUOT,
        err::STALE | err::BADHANDLE => libc::ESTALE,
        err::NOTSUPP | err::ATTRNOTSUPP => libc::EOPNOTSUPP,
        err::DENIED | err::LOCKED | err::DELAY => libc::EAGAIN,
        err::SYMLINK => libc::ELOOP,
        err::FILE_OPEN => libc::EBUSY,
        err::OPENMODE => libc::EBADF,
        err::TOOSMALL => libc::ERANGE,
        _ => libc::EIO,
    }
}

/// Whether `status` says an open's stateid is no longer the server's.
pub fn lost_state(status: u32) -> bool {
    // ADMIN_REVOKED and DELEG_REVOKED, which nfsd does not name.
    matches!(status, err::BAD_STATEID | err::STALE_STATEID | err::EXPIRED | 10_047 | 10_087)
}

/// How long to wait for replies, and how often to try.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Timing {
    /// How long a reply is waited for.
    pub timeout: Duration,
    /// How many times a call is sent again before it fails, if not hard.
    pub retrans: u32,
    /// Whether calls are sent again for as long as the server is away.
    pub hard: bool,
}

/// The operations of a COMPOUND, after its SEQUENCE.
pub struct Ops {
    w: Writer,
    n: u32,
    /// Whether the server is to keep the reply, to give it again to the
    /// same call sent again: for what changes files.
    pub keep: bool,
}

impl Ops {
    /// No operations, their reply not kept.
    pub fn new() -> Ops {
        Ops { w: Writer::new(), n: 0, keep: false }
    }

    /// No operations, their reply kept.
    pub fn kept() -> Ops {
        Ops { keep: true, ..Ops::new() }
    }

    /// Adds the operation `code`: its arguments are written to what this
    /// returns.
    pub fn op(&mut self, code: u32) -> &mut Writer {
        self.n += 1;
        self.w.u32(code)
    }

    /// PUTFH `fh`.
    pub fn putfh(&mut self, fh: &[u8]) -> &mut Ops {
        self.op(op::PUTFH).opaque(fh);
        self
    }
}

/// A COMPOUND's results, after its SEQUENCE's.
pub struct Results {
    bytes: Vec<u8>,
    start: usize,
}

impl Results {
    /// A walk through the results, in order.
    pub fn walk(&self) -> Walk<'_> {
        Walk { r: Reader::new(&self.bytes[self.start..]) }
    }
}

/// Reads results one by one.
pub struct Walk<'a> {
    r: Reader<'a>,
}

impl<'a> Walk<'a> {
    /// The result of operation `code`, which must come next: a reader at
    /// what it gives, or its status if not OK.
    pub fn next(&mut self, code: u32) -> Result<&mut Reader<'a>, Fail> {
        let got = self.r.u32()?;
        let status = self.r.u32()?;
        if got != code {
            return Err(Fail::Status(err::BADXDR));
        }
        if status != err::OK {
            return Err(Fail::Status(status));
        }
        Ok(&mut self.r)
    }
}

/// Whose a call is.
#[derive(Clone, Copy)]
enum Who<'a> {
    /// The machine's: the calls that keep the client ID, the session and
    /// the lease, which are no one's and go with AUTH_SYS's numbers of
    /// root, as RFC 8881's SP4_NONE lets them.
    Machine,
    /// A person's, as `cred` says.
    Person(&'a Cred),
}

/// The client's state on the server.
struct ClientId {
    id: u64,
    /// CREATE_SESSION's sequence number, the next to give.
    sequence: u32,
    /// Whether RECLAIM_COMPLETE was sent: opens need it.
    reclaimed: bool,
}

struct Session {
    id: [u8; 16],
    /// The slot's sequence number, the next to give.
    sequence: u32,
}

/// A server's NFS, as a client of it.
pub struct Client {
    addr: SocketAddr,
    /// How long replies are waited for, and how often calls are sent.
    pub timing: Timing,
    /// The minor version spoken: 2, or 1.
    minor: u32,
    /// Whether the minor version was given, and so is not stepped down.
    minor_given: bool,
    machine: Vec<u8>,
    owner: Vec<u8>,
    verifier: [u8; 8],
    conn: Option<Conn>,
    client: Option<ClientId>,
    session: Option<Session>,
    xid: u32,
    /// How many client IDs the server gave: opens of an earlier one are
    /// gone.
    pub generation: u64,
    /// The server's lease, which calls renew.
    pub lease: Duration,
    /// The most bytes a READ asks for and a WRITE carries.
    pub max_read: u32,
    pub max_write: u32,
    /// When the server last answered.
    pub last: Instant,
    /// Whether the mount ended, and the client with it: the lease is not
    /// renewed, which would make a client ID again.
    ended: bool,
    /// Each caller's Kerberos context, for `sec=krb5`, `krb5i` and
    /// `krb5p`; none for `sec=sys`.
    kerberos: Option<Kerberos>,
}

impl Client {
    /// A client of the server at `addr`, which names itself `machine` and
    /// is told apart by `owner` and the `verifier` of this mount; speaking
    /// minor version `minor`, or 4.2 then 4.1 if `None`. Nothing is sent
    /// yet.
    pub fn new(
        addr: SocketAddr,
        timing: Timing,
        minor: Option<u32>,
        machine: &[u8],
        owner: &[u8],
        verifier: [u8; 8],
    ) -> Client {
        Client {
            addr,
            timing,
            minor: minor.unwrap_or(2),
            minor_given: minor.is_some(),
            machine: machine.to_vec(),
            owner: owner.to_vec(),
            verifier,
            conn: None,
            client: None,
            session: None,
            xid: u32::from_be_bytes([verifier[0], verifier[1], verifier[2], verifier[3]]),
            generation: 0,
            lease: Duration::from_secs(90),
            max_read: 1 << 20,
            max_write: 1 << 20,
            last: Instant::now(),
            ended: false,
            kerberos: None,
        }
    }

    /// Calls go under each caller's Kerberos context from now on.
    pub fn use_kerberos(&mut self, kerberos: Kerberos) {
        self.kerberos = Some(kerberos);
    }

    /// The client ID, for open-owners.
    pub fn client_id(&self) -> u64 {
        self.client.as_ref().map_or(0, |c| c.id)
    }

    /// Sends `ops` after a SEQUENCE, as `cred`: their results, once the
    /// server carried them out. A lost connection, and the server's DELAY
    /// and GRACE, are waited out, for as long as [`Timing`] says; a lost
    /// session or client ID is made again.
    ///
    /// # Errors
    ///
    /// SEQUENCE's status, or the connection's once it is given up on.
    pub fn run(&mut self, cred: &Cred, ops: &Ops) -> Result<Results, Fail> {
        self.run_as(Who::Person(cred), ops)
    }

    /// [`Client::run`], as `who`.
    fn run_as(&mut self, who: Who<'_>, ops: &Ops) -> Result<Results, Fail> {
        let started = Instant::now();
        let mut lost = 0;
        loop {
            match self.attempt(who, ops) {
                Ok(Ok(results)) => return Ok(results),
                Ok(Err(wait)) => {
                    if started.elapsed() > WAIT_MAX {
                        return Err(Fail::Status(err::DELAY));
                    }
                    thread::sleep(wait);
                }
                Err(Fail::Io(e)) => {
                    self.conn = None;
                    lost += 1;
                    let refused = e.kind() == io::ErrorKind::ConnectionRefused;
                    if !self.timing.hard && lost > self.timing.retrans {
                        return Err(Fail::Io(e));
                    }
                    if refused || lost > 1 {
                        thread::sleep(Duration::from_secs(u64::from(lost.min(10))));
                    }
                }
                Err(fail) => return Err(fail),
            }
        }
    }

    /// Sends `ops` once: their results, or how long to wait before they
    /// are sent again.
    fn attempt(&mut self, who: Who<'_>, ops: &Ops) -> Result<Result<Results, Duration>, Fail> {
        self.ready()?;
        if self.client.as_ref().is_some_and(|c| !c.reclaimed) {
            // Nothing to reclaim: opens may be made.
            let mut reclaim = Ops::kept();
            reclaim.op(op::RECLAIM_COMPLETE).bool(false);
            match self.sequenced(Who::Machine, &reclaim)? {
                Ok(results) => match results.walk().next(op::RECLAIM_COMPLETE) {
                    Ok(_) | Err(Fail::Status(err::COMPLETE_ALREADY)) => {
                        if let Some(client) = &mut self.client {
                            client.reclaimed = true;
                        }
                    }
                    Err(fail) => return Err(fail),
                },
                Err(wait) => return Ok(Err(wait)),
            }
        }
        self.sequenced(who, ops)
    }

    /// Sends `ops` once after a SEQUENCE in the session's slot.
    fn sequenced(&mut self, who: Who<'_>, ops: &Ops) -> Result<Result<Results, Duration>, Fail> {
        let session = self.session.as_ref().ok_or(Fail::Status(err::BADSESSION))?;
        let mut w = Writer::new();
        w.opaque(b"").u32(self.minor).u32(ops.n + 1);
        w.u32(op::SEQUENCE).fixed(&session.id).u32(session.sequence).u32(0).u32(0).bool(ops.keep);
        w.bytes.extend_from_slice(&ops.w.bytes);
        let reply = self.call(who, &w.bytes)?;
        let mut r = Reader::new(&reply);
        let status = r.u32()?;
        r.opaque(1024)?;
        r.u32()?;
        if r.left() == 0 {
            // No result at all: the COMPOUND itself was refused.
            return Err(Fail::Status(status));
        }
        let code = r.u32()?;
        let sequenced = r.u32()?;
        if code != op::SEQUENCE {
            return Err(Fail::Status(err::BADXDR));
        }
        match sequenced {
            err::OK => {
                if let Some(session) = &mut self.session {
                    session.sequence = session.sequence.wrapping_add(1);
                }
                // sessionid, sequenceid, slotid, highest and target
                // slots, status flags.
                r.fixed(16)?;
                for _ in 0..5 {
                    r.u32()?;
                }
                self.last = Instant::now();
                if status == err::DELAY || status == 10_013 {
                    // DELAY, or GRACE: the server is not ready yet.
                    return Ok(Err(Duration::from_secs(1)));
                }
                let start = r.position();
                Ok(Ok(Results { bytes: reply, start }))
            }
            // The slot is busy with the same call: it is sent again.
            err::DELAY => Ok(Err(Duration::from_millis(500))),
            // A call sent again after its connection was lost, which the
            // server carried out without keeping its reply (one that asked
            // for none: a read), or (SEQ_FALSE_RETRY) took for another: it
            // goes again as a new call, in the slot's next number, as
            // Linux's client sends it.
            err::RETRY_UNCACHED_REP | 10_076 => {
                if let Some(session) = &mut self.session {
                    session.sequence = session.sequence.wrapping_add(1);
                }
                Ok(Err(Duration::ZERO))
            }
            err::BADSESSION
            | err::DEADSESSION
            | err::CONN_NOT_BOUND_TO_SESSION
            | err::BADSLOT
            | err::BAD_HIGH_SLOT
            | err::SEQ_MISORDERED => {
                self.session = None;
                Ok(Err(Duration::ZERO))
            }
            err::STALE_CLIENTID | err::EXPIRED => {
                self.session = None;
                self.client = None;
                Ok(Err(Duration::ZERO))
            }
            other => Err(Fail::Status(other)),
        }
    }

    /// Sends a COMPOUND's arguments, `args`, as `who`: its reply's
    /// results. A person's call on a Kerberos mount goes under their
    /// context, set up again once if the server lost it.
    fn call(&mut self, who: Who<'_>, args: &[u8]) -> Result<Vec<u8>, Fail> {
        if self.conn.is_none() {
            let conn = Conn::connect(self.addr, self.timing.timeout, self.xid).map_err(Fail::Io)?;
            self.conn = Some(conn);
        }
        let conn = self.conn.as_mut().ok_or(Fail::Status(err::IO))?;
        let root = Cred::default();
        let mut set_up_again = true;
        let reply = loop {
            let reply = match (&mut self.kerberos, who) {
                (Some(kerberos), Who::Person(cred)) => {
                    let session = kerberos.session(cred.uid, conn);
                    let session = match session {
                        Ok(session) => session,
                        Err(fail) => {
                            if matches!(fail, Fail::Io(_)) {
                                self.conn = None;
                            }
                            return Err(fail);
                        }
                    };
                    conn.call(COMPOUND, Auth::Gss(session), args)
                }
                (_, Who::Person(cred)) => conn.call(COMPOUND, Auth::Sys(cred, &self.machine), args),
                (_, Who::Machine) => conn.call(COMPOUND, Auth::Sys(&root, &self.machine), args),
            };
            match (reply, &mut self.kerberos, who) {
                (Err(e), Some(kerberos), Who::Person(cred))
                    if rpc::context_lost(&e) && set_up_again =>
                {
                    kerberos.forget(cred.uid);
                    set_up_again = false;
                }
                (reply, ..) => break reply,
            }
        };
        self.xid = self.xid.wrapping_add(1);
        match reply {
            Ok(reply) => Ok(reply),
            Err(e) => {
                self.conn = None;
                Err(match e.kind() {
                    _ if rpc::context_lost(&e) => {
                        Fail::Denied("the server keeps losing the Kerberos context".into())
                    }
                    io::ErrorKind::PermissionDenied | io::ErrorKind::Unsupported => {
                        Fail::Status(io_status(&e))
                    }
                    _ => Fail::Io(e),
                })
            }
        }
    }

    /// Makes the client ID and session if there are none.
    fn ready(&mut self) -> Result<(), Fail> {
        if self.client.is_none() {
            self.session = None;
            self.exchange_id()?;
        }
        if self.session.is_none() {
            match self.create_session() {
                Ok(()) => {}
                Err(Fail::Status(err::STALE_CLIENTID | err::SEQ_MISORDERED)) => {
                    self.exchange_id()?;
                    self.create_session()?;
                }
                Err(fail) => return Err(fail),
            }
        }
        Ok(())
    }

    /// A COMPOUND of operation `code` alone, with `args`, without a
    /// SEQUENCE: the reply, and where the operation's result starts.
    fn bare(&mut self, code: u32, args: &[u8]) -> Result<(Vec<u8>, usize), Fail> {
        let mut w = Writer::new();
        w.opaque(b"").u32(self.minor).u32(1).u32(code);
        w.bytes.extend_from_slice(args);
        let reply = self.call(Who::Machine, &w.bytes)?;
        let mut r = Reader::new(&reply);
        let status = r.u32()?;
        r.opaque(1024)?;
        if r.u32()? == 0 {
            return Err(Fail::Status(status));
        }
        let mut walk = Walk { r };
        walk.next(code)?;
        let at = walk.r.position();
        Ok((reply, at))
    }

    /// EXCHANGE_ID: a client ID, stepping down to 4.1 from 4.2 if the
    /// server speaks only that.
    fn exchange_id(&mut self) -> Result<(), Fail> {
        let mut args = Writer::new();
        args.fixed(&self.verifier).opaque(&self.owner);
        // No flags; SP4_NONE; no implementation named.
        args.u32(0).u32(0).u32(0);
        let (reply, at) = match self.bare(op::EXCHANGE_ID, &args.bytes) {
            Err(Fail::Status(err::MINOR_VERS_MISMATCH)) if !self.minor_given && self.minor == 2 => {
                self.minor = 1;
                self.bare(op::EXCHANGE_ID, &args.bytes)?
            }
            other => other?,
        };
        let mut r = Reader::new(&reply[at..]);
        let id = r.u64()?;
        let sequence = r.u32()?;
        self.client = Some(ClientId { id, sequence, reclaimed: false });
        self.generation += 1;
        Ok(())
    }

    /// CREATE_SESSION: a session of one slot, with no back channel to be
    /// called on: no delegations are asked for.
    fn create_session(&mut self) -> Result<(), Fail> {
        let client = self.client.as_ref().ok_or(Fail::Status(err::STALE_CLIENTID))?;
        let room = (1 << 20) + 2 * OVERHEAD;
        let mut args = Writer::new();
        args.u64(client.id).u32(client.sequence).u32(0);
        // The fore channel: no padding, a READ's or WRITE's room, 8 KiB
        // replies kept, 16 operations, one slot, no RDMA.
        args.u32(0).u32(room).u32(room).u32(8192).u32(16).u32(1).u32(0);
        // The back channel, which is not bound: Linux's server's least.
        args.u32(0).u32(4096).u32(4096).u32(0).u32(2).u32(1).u32(0);
        // The callback program, and AUTH_NONE for it.
        args.u32(0x4000_0000).u32(1).u32(0);
        let (reply, at) = self.bare(op::CREATE_SESSION, &args.bytes)?;
        let mut r = Reader::new(&reply[at..]);
        let id = r.array::<16>()?;
        r.u32()?;
        r.u32()?;
        let _pad = r.u32()?;
        let request = r.u32()?;
        let response = r.u32()?;
        if let Some(client) = &mut self.client {
            client.sequence = client.sequence.wrapping_add(1);
        }
        let room = |size: u32| size.saturating_sub(OVERHEAD) / 4096 * 4096;
        self.max_write = self.max_write.min(room(request)).max(4096);
        self.max_read = self.max_read.min(room(response)).max(4096);
        self.session = Some(Session { id, sequence: 1 });
        Ok(())
    }

    /// Sets the most bytes READs and WRITEs carry to at most `read` and
    /// `write`, each a multiple of 4 KiB.
    pub fn limit_io(&mut self, read: u64, write: u64) {
        let cut = |now: u32, most: u64| {
            if most == 0 {
                return now;
            }
            let most = u32::try_from(most).unwrap_or(u32::MAX);
            now.min(most / 4096 * 4096).max(4096)
        };
        self.max_read = cut(self.max_read, read);
        self.max_write = cut(self.max_write, write);
    }

    /// A SEQUENCE alone, which renews the lease.
    pub fn renew(&mut self) -> Result<(), Fail> {
        if self.ended {
            return Ok(());
        }
        self.run_as(Who::Machine, &Ops::new()).map(drop)
    }

    /// Ends the session and client ID, for the server to free what it
    /// kept of them; a failure is not this mount's to report.
    pub fn goodbye(&mut self) {
        self.ended = true;
        if let Some(session) = self.session.take() {
            let _ = self.bare(op::DESTROY_SESSION, &session.id);
        }
        if let Some(client) = self.client.take() {
            let mut args = Writer::new();
            args.u64(client.id);
            let _ = self.bare(op::DESTROY_CLIENTID, &args.bytes);
        }
        self.conn = None;
    }

    /// Takes back the slot's last sequence number and the connection, as
    /// when a reply was lost on the way: for the tests.
    #[cfg(test)]
    pub fn lose_reply(&mut self) {
        if let Some(session) = &mut self.session {
            session.sequence = session.sequence.wrapping_sub(1);
        }
        self.conn = None;
    }

    /// Forgets the session and client ID, as a client that restarted: for
    /// the tests.
    #[cfg(test)]
    pub fn forget(&mut self, verifier: [u8; 8]) {
        self.session = None;
        self.client = None;
        self.conn = None;
        self.verifier = verifier;
    }
}

/// The status a refusal of the connection's stands for.
fn io_status(e: &io::Error) -> u32 {
    if e.kind() == io::ErrorKind::PermissionDenied && e.to_string().contains("Kerberos") {
        err::WRONGSEC
    } else if e.kind() == io::ErrorKind::PermissionDenied {
        err::ACCESS
    } else {
        err::MINOR_VERS_MISMATCH
    }
}
