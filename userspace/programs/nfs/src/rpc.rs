// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! ONC RPC calls over TCP (RFC 5531): each call a record, as nfsd(8)
//! reads them, with AUTH_SYS's credential, the caller's numbers, or
//! RPCSEC_GSS's under a Kerberos context (ADR 0399), whose verifier signs
//! the call's header and whose service may sign or seal its arguments.

use std::fmt;
use std::io::{self, BufReader, Write};
use std::net::{SocketAddr, TcpStream};
use std::time::Duration;

use kerberos::gss::Context;
use nfsd::gss::{self, InitResult, RPCSEC_GSS, Service};
use nfsd::rpc::{self as wire, AUTH_NONE, AUTH_SYS, NFS_PROGRAM, NFS_VERSION};
use nfsd::xdr::{Reader, Writer};

/// RPC's version, and a call and a reply.
const RPC_VERSION: u32 = 2;
const CALL: u32 = 0;
const REPLY: u32 = 1;

/// The most other groups AUTH_SYS carries.
pub const GROUPS_MAX: usize = 16;

/// auth_stat: the server takes no credential this weak (RFC 5531).
const AUTH_TOOWEAK: u32 = 5;

/// Who calls, by AUTH_SYS's word: numbers the server takes as they are.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Cred {
    /// The user's number.
    pub uid: u32,
    /// The group's.
    pub gid: u32,
    /// The other groups', at most [`GROUPS_MAX`].
    pub gids: Vec<u32>,
}

/// A context set up with the server, for one person's calls.
pub struct GssSession {
    /// The context's keys.
    pub context: Context,
    /// The server's handle for it.
    pub handle: Vec<u8>,
    /// What it protects.
    pub service: Service,
    /// The next call's sequence number.
    seq: u32,
}

impl GssSession {
    /// A context the server knows as `handle`.
    pub fn new(context: Context, handle: Vec<u8>, service: Service) -> GssSession {
        GssSession { context, handle, service, seq: 0 }
    }

    /// Whether it is spent, its ticket ended or its sequence numbers used
    /// up, so that another must be set up.
    pub fn spent(&self, now: i64) -> bool {
        self.seq >= gss::MAXSEQ || self.context.expires() <= now
    }
}

/// How a call says who calls.
pub enum Auth<'a> {
    /// AUTH_SYS: `cred`'s numbers, from the machine named `machine`.
    Sys(&'a Cred, &'a [u8]),
    /// RPCSEC_GSS, under a context.
    Gss(&'a mut GssSession),
}

/// The server no longer holds a call's context (CREDPROBLEM or
/// CTXPROBLEM): another is set up and the call sent again.
#[derive(Debug)]
pub struct ContextLost;

impl fmt::Display for ContextLost {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("the server no longer holds the Kerberos context")
    }
}

impl std::error::Error for ContextLost {}

/// Whether `err` is a [`ContextLost`].
pub fn context_lost(err: &io::Error) -> bool {
    err.get_ref().is_some_and(<dyn std::error::Error + Send + Sync>::is::<ContextLost>)
}

/// A connection to a server's NFS.
pub struct Conn {
    input: BufReader<TcpStream>,
    xid: u32,
}

impl Conn {
    /// Connects to `addr`, waiting at most `timeout` for it, and for each
    /// reply after.
    pub fn connect(addr: SocketAddr, timeout: Duration, xid: u32) -> io::Result<Conn> {
        let stream = TcpStream::connect_timeout(&addr, timeout)?;
        let _ = stream.set_nodelay(true);
        stream.set_read_timeout(Some(timeout))?;
        stream.set_write_timeout(Some(timeout))?;
        Ok(Conn { input: BufReader::with_capacity(64 * 1024, stream), xid })
    }

    /// Calls NFS's `procedure` with `args`, as `auth` says: the results,
    /// after the reply's header, opened if the context sealed them.
    ///
    /// # Errors
    ///
    /// The connection's, a reply that is no reply to it (`InvalidData`),
    /// the server's refusal of the call (`PermissionDenied`,
    /// `Unsupported`), and [`ContextLost`].
    pub fn call(&mut self, procedure: u32, auth: Auth<'_>, args: &[u8]) -> io::Result<Vec<u8>> {
        self.xid = self.xid.wrapping_add(1);
        let xid = self.xid;
        let mut w = Writer::new();
        w.u32(0);
        w.u32(xid).u32(CALL).u32(RPC_VERSION).u32(NFS_PROGRAM).u32(NFS_VERSION).u32(procedure);
        match auth {
            Auth::Sys(cred, machine) => {
                w.u32(AUTH_SYS).opaque(&sys_body(cred, machine)).u32(AUTH_NONE).u32(0);
                w.bytes.extend_from_slice(args);
                let reply = self.exchange(xid, w)?;
                Ok(reply.results().to_vec())
            }
            Auth::Gss(session) => {
                let seq = session.seq;
                session.seq += 1;
                let cred = gss::Cred {
                    proc: gss::proc::DATA,
                    seq,
                    service: session.service,
                    handle: session.handle.clone(),
                };
                w.u32(RPCSEC_GSS).opaque(&cred.encode());
                // The verifier signs the header, from the xid on.
                let mic = session.context.get_mic(&w.bytes[4..]);
                w.u32(RPCSEC_GSS).opaque(&mic);
                let body = gss::protect(&mut session.context, session.service, seq, args)
                    .map_err(io::Error::other)?;
                w.bytes.extend_from_slice(&body);
                let reply = self.exchange(xid, w)?;
                if reply.flavor != RPCSEC_GSS
                    || gss::check_reply_verifier(&session.context, seq, reply.verifier()).is_err()
                {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "the server's reply is not under the Kerberos context: it may not be the \
                         server the ticket is for",
                    ));
                }
                gss::unprotect(&session.context, session.service, seq, reply.results())
                    .map_err(|_| bad())
            }
        }
    }

    /// Sets up a context for `service` with GSS-API's first token,
    /// `token`: the server's results, and the verifier its reply carries,
    /// which signs the window once the context is set up.
    ///
    /// # Errors
    ///
    /// As [`Conn::call`]'s.
    pub fn init(&mut self, service: Service, token: &[u8]) -> io::Result<(InitResult, Vec<u8>)> {
        self.xid = self.xid.wrapping_add(1);
        let xid = self.xid;
        let cred = gss::Cred { proc: gss::proc::INIT, seq: 0, service, handle: Vec::new() };
        let mut w = Writer::new();
        w.u32(0);
        // INIT is a call of NULL, procedure 0, with the token as its
        // argument.
        w.u32(xid).u32(CALL).u32(RPC_VERSION).u32(NFS_PROGRAM).u32(NFS_VERSION).u32(0);
        w.u32(RPCSEC_GSS).opaque(&cred.encode()).u32(AUTH_NONE).u32(0);
        w.bytes.extend_from_slice(&gss::init_args(token));
        let reply = self.exchange(xid, w)?;
        let results = InitResult::decode(reply.results()).map_err(|_| bad())?;
        Ok((results, reply.verifier().to_vec()))
    }

    /// Sends the call `w`, whose first four bytes are left for the record
    /// mark, and reads the reply to `xid`.
    fn exchange(&mut self, xid: u32, mut w: Writer) -> io::Result<Reply> {
        // The record mark, in the four bytes left for it: one fragment.
        let mark = 0x8000_0000 | (w.bytes.len() - 4) as u32;
        w.bytes[..4].copy_from_slice(&mark.to_be_bytes());
        let mut stream: &TcpStream = self.input.get_ref();
        stream.write_all(&w.bytes)?;
        loop {
            let Some(record) = wire::read_record(&mut self.input)? else {
                return Err(io::ErrorKind::UnexpectedEof.into());
            };
            let mut r = Reader::new(&record);
            // A reply to a call given up on: the next is this one's.
            if r.u32() != Ok(xid) {
                continue;
            }
            let (flavor, verifier, start) = header(&mut r)?;
            return Ok(Reply { record, flavor, verifier, start });
        }
    }
}

/// AUTH_SYS's body: `cred`'s numbers, from `machine`.
fn sys_body(cred: &Cred, machine: &[u8]) -> Vec<u8> {
    let mut body = Writer::new();
    body.u32(0).opaque(&machine[..machine.len().min(255)]).u32(cred.uid).u32(cred.gid);
    let gids = &cred.gids[..cred.gids.len().min(GROUPS_MAX)];
    body.u32(gids.len() as u32);
    for &gid in gids {
        body.u32(gid);
    }
    body.bytes
}

/// An accepted reply: its verifier, and where its results start.
struct Reply {
    record: Vec<u8>,
    flavor: u32,
    verifier: (usize, usize),
    start: usize,
}

impl Reply {
    /// The verifier's body.
    fn verifier(&self) -> &[u8] {
        &self.record[self.verifier.0..self.verifier.1]
    }

    /// The results.
    fn results(&self) -> &[u8] {
        &self.record[self.start..]
    }
}

fn bad() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, "the server's reply is not RPC's")
}

/// Reads an accepted reply's header from `r`: its verifier's flavor,
/// where its verifier's body is, and where its results start.
fn header(r: &mut Reader) -> io::Result<(u32, (usize, usize), usize)> {
    if r.u32() != Ok(REPLY) {
        return Err(bad());
    }
    match r.u32().map_err(|_| bad())? {
        // Accepted: a verifier, then how.
        0 => {
            let flavor = r.u32().map_err(|_| bad())?;
            let body = r.opaque(400).map_err(|_| bad())?;
            let end = r.position() - nfsd::xdr::pad(body.len());
            let verifier = (end - body.len(), end);
            match r.u32().map_err(|_| bad())? {
                0 => Ok((flavor, verifier, r.position())),
                2 => {
                    let low = r.u32().map_err(|_| bad())?;
                    let high = r.u32().map_err(|_| bad())?;
                    Err(io::Error::new(
                        io::ErrorKind::Unsupported,
                        format!("the server speaks NFS {low} to {high}, not NFS 4"),
                    ))
                }
                1 => Err(io::Error::new(io::ErrorKind::Unsupported, "the server has no NFS")),
                stat => Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("the server could not take the call (accept_stat {stat})"),
                )),
            }
        }
        // Denied: RPC's version, or the credential.
        1 => match r.u32().map_err(|_| bad())? {
            0 => Err(io::Error::new(io::ErrorKind::Unsupported, "the server speaks no RPC 2")),
            _ => match r.u32().map_err(|_| bad())? {
                AUTH_TOOWEAK => Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "the server asks for Kerberos: mount with sec=krb5, krb5i or krb5p",
                )),
                gss::CREDPROBLEM | gss::CTXPROBLEM => {
                    Err(io::Error::new(io::ErrorKind::PermissionDenied, ContextLost))
                }
                stat => Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    format!("the server refused the call's credential (auth_stat {stat})"),
                )),
            },
        },
        _ => Err(bad()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replies_are_read() {
        let mut ok = Writer::new();
        ok.u32(REPLY).u32(0).u32(AUTH_NONE).u32(0).u32(0).u32(42);
        let mut r = Reader::new(&ok.bytes);
        let (flavor, verifier, at) = header(&mut r).unwrap();
        assert_eq!((flavor, verifier.0, verifier.1), (AUTH_NONE, 16, 16));
        assert_eq!(&ok.bytes[at..], [0, 0, 0, 42]);

        let mut weak = Writer::new();
        weak.u32(REPLY).u32(1).u32(1).u32(AUTH_TOOWEAK);
        let err = header(&mut Reader::new(&weak.bytes)).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::PermissionDenied);
        assert!(err.to_string().contains("Kerberos"));

        let mut lost = Writer::new();
        lost.u32(REPLY).u32(1).u32(1).u32(gss::CTXPROBLEM);
        assert!(context_lost(&header(&mut Reader::new(&lost.bytes)).unwrap_err()));

        let mut old = Writer::new();
        old.u32(REPLY).u32(0).u32(AUTH_NONE).u32(0).u32(2).u32(2).u32(3);
        let err = header(&mut Reader::new(&old.bytes)).unwrap_err();
        assert!(err.to_string().contains("NFS 2 to 3"));
    }
}
