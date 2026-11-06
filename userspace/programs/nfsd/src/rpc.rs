// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! ONC RPC version 2 (RFC 5531) over TCP: the record marking that frames
//! each call, the call's header with its credential, and the replies.
//! nfsd answers program 100003 (NFS), version 4, with AUTH_SYS's and
//! AUTH_NONE's credentials (RFC 5531's appendix A) and RPCSEC_GSS's
//! (RFC 2203, [`crate::gss`]).

use std::io::{self, Read};
use std::ops::Range;

use crate::gss::{self, RPCSEC_GSS};
use crate::xdr::{self, Reader, Writer};

/// NFS's program number.
pub const NFS_PROGRAM: u32 = 100_003;
/// The version of NFS served.
pub const NFS_VERSION: u32 = 4;

/// RPC's version.
const RPC_VERSION: u32 = 2;
/// A call, and a reply.
const CALL: u32 = 0;
const REPLY: u32 = 1;

/// Credential flavors.
pub const AUTH_NONE: u32 = 0;
/// The client's word for who calls (RFC 5531 appendix A).
pub const AUTH_SYS: u32 = 1;

/// The longest record read: a WRITE of 1 MiB and room for its COMPOUND.
pub const RECORD_MAX: usize = (1 << 20) + 64 * 1024;

/// Reads one record, its fragments joined: `None` at the end of the
/// connection before a record starts.
///
/// # Errors
///
/// The connection's, and `InvalidData` for a record past [`RECORD_MAX`].
pub fn read_record(input: &mut impl Read) -> io::Result<Option<Vec<u8>>> {
    let mut record = Vec::new();
    loop {
        let mut mark = [0u8; 4];
        if let Err(err) = input.read_exact(&mut mark) {
            if err.kind() == io::ErrorKind::UnexpectedEof && record.is_empty() {
                return Ok(None);
            }
            return Err(err);
        }
        let mark = u32::from_be_bytes(mark);
        let last = mark & 0x8000_0000 != 0;
        let n = (mark & 0x7fff_ffff) as usize;
        if record.len() + n > RECORD_MAX {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "a record past 1 MiB and 64 KiB",
            ));
        }
        let start = record.len();
        record.resize(start + n, 0);
        input.read_exact(&mut record[start..])?;
        if last {
            return Ok(Some(record));
        }
    }
}

/// `reply` framed as one record, its last fragment.
pub fn frame(reply: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(reply.len() + 4);
    out.extend_from_slice(&(0x8000_0000 | reply.len() as u32).to_be_bytes());
    out.extend_from_slice(reply);
    out
}

/// Who calls, by AUTH_SYS's word.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Caller {
    /// Their user's number.
    pub uid: u32,
    /// Their group's.
    pub gid: u32,
    /// Their other groups', at most 16.
    pub gids: Vec<u32>,
}

impl Caller {
    /// The caller of an AUTH_NONE call: no one.
    pub fn none() -> Caller {
        Caller { uid: NOBODY, gid: NOBODY, gids: Vec::new() }
    }
}

/// The number of the user no one is, as Linux's `nobody`: whom a call
/// as root, as a number with no account, or with no credential is served
/// as.
pub const NOBODY: u32 = 65_534;

/// A call's header.
#[derive(Debug)]
pub struct Call {
    /// Its transaction number, which the reply carries back.
    pub xid: u32,
    /// The program called.
    pub program: u32,
    /// Its version.
    pub version: u32,
    /// The procedure.
    pub procedure: u32,
    /// The credential's flavor.
    pub flavor: u32,
    /// Who calls, for AUTH_SYS; no one for AUTH_NONE and RPCSEC_GSS,
    /// whose context says who.
    pub caller: Caller,
    /// RPCSEC_GSS's credential, and what its verifier signs.
    pub gss: Option<GssCall>,
}

/// What an RPCSEC_GSS call carries besides its arguments.
#[derive(Debug)]
pub struct GssCall {
    /// The credential.
    pub cred: gss::Cred,
    /// Where the header its verifier signs is in the record: from the xid
    /// to the end of the credential.
    pub header: Range<usize>,
    /// The verifier's flavor and body: for a call under a context, a MIC
    /// of the header.
    pub verifier: (u32, Vec<u8>),
}

/// What is wrong with a call, as its reply says.
#[derive(Debug, PartialEq, Eq)]
pub enum Refusal {
    /// Not a call of RPC version 2: no reply at all if it is no call.
    NotCall,
    /// RPC's version is not 2.
    RpcMismatch,
    /// A credential nfsd does not take: AUTH_ERROR with this auth_stat.
    Auth(u32),
}

/// auth_stat: a credential that is malformed.
pub const AUTH_BADCRED: u32 = 1;
/// auth_stat: a flavor the server does not take.
pub const AUTH_TOOWEAK: u32 = 5;

/// Reads a call's header from `r`, leaving it at the procedure's
/// arguments.
///
/// # Errors
///
/// What the reply is to say, and with no xid for what is no call.
pub fn read_call(r: &mut Reader) -> Result<Call, (Option<u32>, Refusal)> {
    let start = r.position();
    let xid = r.u32().map_err(|_| (None, Refusal::NotCall))?;
    let bad = |refusal| (Some(xid), refusal);
    if r.u32() != Ok(CALL) {
        return Err((None, Refusal::NotCall));
    }
    if r.u32() != Ok(RPC_VERSION) {
        return Err(bad(Refusal::RpcMismatch));
    }
    let header = (|| -> xdr::Result<_> { Ok((r.u32()?, r.u32()?, r.u32()?)) })();
    let (program, version, procedure) = header.map_err(|_| bad(Refusal::Auth(AUTH_BADCRED)))?;
    let flavor = r.u32().map_err(|_| bad(Refusal::Auth(AUTH_BADCRED)))?;
    let body = r.opaque(400).map_err(|_| bad(Refusal::Auth(AUTH_BADCRED)))?;
    let header = start..r.position();
    // The verifier: AUTH_NONE's, or RPCSEC_GSS's MIC of the header.
    let verifier = (|| -> xdr::Result<_> { Ok((r.u32()?, r.opaque(400)?.to_vec())) })();
    let verifier = verifier.map_err(|_| bad(Refusal::Auth(AUTH_BADCRED)))?;
    let (caller, gss) = match flavor {
        AUTH_NONE => (Caller::none(), None),
        AUTH_SYS => (sys_caller(body).map_err(|_| bad(Refusal::Auth(AUTH_BADCRED)))?, None),
        RPCSEC_GSS => {
            let cred = gss::Cred::decode(body).map_err(|_| bad(Refusal::Auth(AUTH_BADCRED)))?;
            (Caller::none(), Some(GssCall { cred, header, verifier }))
        }
        _ => return Err(bad(Refusal::Auth(AUTH_TOOWEAK))),
    };
    Ok(Call { xid, program, version, procedure, flavor, caller, gss })
}

/// AUTH_SYS's body: a stamp, the machine's name, the user, the group and
/// at most 16 others.
fn sys_caller(body: &[u8]) -> xdr::Result<Caller> {
    let mut r = Reader::new(body);
    let _stamp = r.u32()?;
    let _machine = r.opaque(255)?;
    let uid = r.u32()?;
    let gid = r.u32()?;
    let n = r.count(16)?;
    let gids = (0..n).map(|_| r.u32()).collect::<xdr::Result<_>>()?;
    Ok(Caller { uid, gid, gids })
}

/// accept_stat: the call was carried out, and its results follow.
pub const SUCCESS: u32 = 0;
/// The program is not served here.
pub const PROG_UNAVAIL: u32 = 1;
/// The program is, but not this version: the lowest and highest follow.
pub const PROG_MISMATCH: u32 = 2;
/// The procedure is not the program's.
pub const PROC_UNAVAIL: u32 = 3;
/// The arguments could not be read.
pub const GARBAGE_ARGS: u32 = 4;
/// The server failed to carry the call out.
pub const SYSTEM_ERR: u32 = 5;

/// An accepted reply's header, with `stat`, for `xid`: what follows is
/// the results.
pub fn accepted(xid: u32, stat: u32) -> Writer {
    accepted_with(xid, AUTH_NONE, &[], stat)
}

/// [`accepted`], with the verifier of `flavor` and `body`: RPCSEC_GSS's
/// MIC of the call's sequence number, under a context.
pub fn accepted_with(xid: u32, flavor: u32, body: &[u8], stat: u32) -> Writer {
    let mut w = Writer::new();
    w.u32(xid).u32(REPLY).u32(0);
    w.u32(flavor).opaque(body);
    w.u32(stat);
    w
}

/// The reply to a call refused for `refusal`, if it gets one.
pub fn refused(xid: u32, refusal: &Refusal) -> Option<Vec<u8>> {
    let mut w = Writer::new();
    w.u32(xid).u32(REPLY).u32(1);
    match refusal {
        Refusal::NotCall => return None,
        Refusal::RpcMismatch => {
            w.u32(0).u32(RPC_VERSION).u32(RPC_VERSION);
        }
        Refusal::Auth(stat) => {
            w.u32(1).u32(*stat);
        }
    }
    Some(w.bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A call of `procedure` as user `uid`, with AUTH_SYS.
    pub fn call(xid: u32, procedure: u32, uid: u32) -> Writer {
        let mut cred = Writer::new();
        cred.u32(0).opaque(b"client").u32(uid).u32(uid).u32(1).u32(27);
        let mut w = Writer::new();
        w.u32(xid).u32(CALL).u32(RPC_VERSION).u32(NFS_PROGRAM).u32(NFS_VERSION).u32(procedure);
        w.u32(AUTH_SYS).opaque(&cred.bytes).u32(AUTH_NONE).u32(0);
        w
    }

    #[test]
    fn an_auth_sys_call_is_read() {
        let w = call(9, 1, 1000);
        let mut r = Reader::new(&w.bytes);
        let call = read_call(&mut r).unwrap();
        assert_eq!((call.xid, call.program, call.version, call.procedure), (9, NFS_PROGRAM, 4, 1));
        assert_eq!(call.caller, Caller { uid: 1000, gid: 1000, gids: vec![27] });
        assert_eq!(r.left(), 0);
    }

    #[test]
    fn an_rpcsec_gss_call_is_read_with_what_its_verifier_signs() {
        let cred = gss::Cred {
            proc: gss::proc::DATA,
            seq: 5,
            service: gss::Service::Privacy,
            handle: vec![7; 8],
        };
        let mut w = Writer::new();
        w.u32(3).u32(CALL).u32(RPC_VERSION).u32(NFS_PROGRAM).u32(4).u32(1);
        w.u32(RPCSEC_GSS).opaque(&cred.encode());
        let signed = w.len();
        w.u32(RPCSEC_GSS).opaque(b"mic!");
        let call = read_call(&mut Reader::new(&w.bytes)).unwrap();
        let gss = call.gss.unwrap();
        assert_eq!((gss.cred, gss.header), (cred, 0..signed));
        assert_eq!(gss.verifier, (RPCSEC_GSS, b"mic!".to_vec()));

        let mut odd = Writer::new();
        odd.u32(4).u32(CALL).u32(RPC_VERSION).u32(NFS_PROGRAM).u32(4).u32(0);
        odd.u32(390_003).opaque(&[0; 8]).u32(AUTH_NONE).u32(0);
        let err = read_call(&mut Reader::new(&odd.bytes)).unwrap_err();
        assert_eq!(err, (Some(4), Refusal::Auth(AUTH_TOOWEAK)));
        let reply = refused(4, &err.1).unwrap();
        assert_eq!(reply, [0, 0, 0, 4, 0, 0, 0, 1, 0, 0, 0, 1, 0, 0, 0, 1, 0, 0, 0, 5]);
    }

    #[test]
    fn records_join_their_fragments_and_have_a_limit() {
        let mut input: &[u8] = &[0, 0, 0, 2, b'a', b'b', 0x80, 0, 0, 1, b'c'];
        assert_eq!(read_record(&mut input).unwrap(), Some(b"abc".to_vec()));
        assert_eq!(read_record(&mut input).unwrap(), None);
        let mut huge: &[u8] = &[0x80, 0x20, 0, 0];
        assert!(read_record(&mut huge).is_err());
        assert_eq!(frame(b"xy"), [0x80, 0, 0, 2, b'x', b'y']);
    }
}
