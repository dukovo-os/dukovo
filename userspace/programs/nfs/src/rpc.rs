// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! ONC RPC calls over TCP (RFC 5531): each call a record, as nfsd(8)
//! reads them, with AUTH_SYS's credential, the caller's numbers.

use std::io::{self, BufReader, Write};
use std::net::{SocketAddr, TcpStream};
use std::time::Duration;

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

    /// Calls NFS's `procedure` with `args` as `cred`, from the machine
    /// named `machine`: the results, after the reply's header.
    ///
    /// # Errors
    ///
    /// The connection's, a reply that is no reply to it (`InvalidData`),
    /// and the server's refusal of the call (`PermissionDenied`,
    /// `Unsupported`).
    pub fn call(
        &mut self,
        procedure: u32,
        cred: &Cred,
        machine: &[u8],
        args: &[u8],
    ) -> io::Result<Vec<u8>> {
        self.xid = self.xid.wrapping_add(1);
        let xid = self.xid;
        let mut body = Writer::new();
        body.u32(0).opaque(&machine[..machine.len().min(255)]).u32(cred.uid).u32(cred.gid);
        let gids = &cred.gids[..cred.gids.len().min(GROUPS_MAX)];
        body.u32(gids.len() as u32);
        for &gid in gids {
            body.u32(gid);
        }
        let mut w = Writer::new();
        w.u32(0);
        w.u32(xid).u32(CALL).u32(RPC_VERSION).u32(NFS_PROGRAM).u32(NFS_VERSION).u32(procedure);
        w.u32(AUTH_SYS).opaque(&body.bytes).u32(AUTH_NONE).u32(0);
        w.bytes.extend_from_slice(args);
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
            let start = results(&mut r)?;
            return Ok(record[start..].to_vec());
        }
    }
}

fn bad() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, "the server's reply is not RPC's")
}

/// Reads a reply's header from `r`: where its results start.
fn results(r: &mut Reader) -> io::Result<usize> {
    if r.u32() != Ok(REPLY) {
        return Err(bad());
    }
    match r.u32().map_err(|_| bad())? {
        // Accepted: a verifier, then how.
        0 => {
            r.u32().map_err(|_| bad())?;
            r.opaque(400).map_err(|_| bad())?;
            match r.u32().map_err(|_| bad())? {
                0 => Ok(r.position()),
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
                    "the server asks for Kerberos (sec=krb5), which nfs does not speak yet",
                )),
                stat => Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    format!("the server refused the caller's numbers (auth_stat {stat})"),
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
        let at = results(&mut r).unwrap();
        assert_eq!(&ok.bytes[at..], [0, 0, 0, 42]);

        let mut weak = Writer::new();
        weak.u32(REPLY).u32(1).u32(1).u32(AUTH_TOOWEAK);
        let err = results(&mut Reader::new(&weak.bytes)).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::PermissionDenied);
        assert!(err.to_string().contains("Kerberos"));

        let mut old = Writer::new();
        old.u32(REPLY).u32(0).u32(AUTH_NONE).u32(0).u32(2).u32(2).u32(3);
        let err = results(&mut Reader::new(&old.bytes)).unwrap_err();
        assert!(err.to_string().contains("NFS 2 to 3"));
    }
}
