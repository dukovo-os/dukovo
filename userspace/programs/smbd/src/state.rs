// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! A signed-in session, as the listener hands it to the worker that
//! serves it, over the pipe authd gave the worker as its descriptor 0:
//! what NEGOTIATE settled, the session's number and keys, and whose it
//! is. The keys never pass through arguments, the environment or a file.

use std::io::{self, Read, Write};

use crate::crypto::{self, Keys, Signing};
use crate::negotiate::Negotiated;

/// What the worker starts from.
#[derive(Debug)]
pub struct Handoff {
    /// The connection's NEGOTIATE.
    pub negotiated: Negotiated,
    /// The session's number.
    pub session_id: u64,
    /// The lowest message number the client has not yet used.
    pub next_message: u64,
    /// Its keys.
    pub keys: Keys,
    /// The person's account.
    pub user: String,
    /// The PC's address.
    pub from: String,
}

/// The longest handoff read: far more than one takes.
const MAX: usize = 64 * 1024;

/// Writes `bytes` with their length first.
fn field(out: &mut Vec<u8>, bytes: &[u8]) {
    out.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
    out.extend_from_slice(bytes);
}

/// Reads the fields written by [`field`], one at a time.
struct Fields<'a>(&'a [u8]);

impl Fields<'_> {
    fn next(&mut self) -> io::Result<&[u8]> {
        let bad = || io::Error::new(io::ErrorKind::InvalidData, "a broken handoff");
        let length = self.0.get(..4).ok_or_else(bad)?;
        let length = u32::from_le_bytes(length.try_into().map_err(|_| bad())?) as usize;
        let bytes = self.0.get(4..4 + length).ok_or_else(bad)?;
        self.0 = &self.0[4 + length..];
        Ok(bytes)
    }

    fn array<const N: usize>(&mut self) -> io::Result<[u8; N]> {
        self.next()?
            .try_into()
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "a field of the wrong length"))
    }

    fn u16(&mut self) -> io::Result<u16> {
        Ok(u16::from_le_bytes(self.array()?))
    }

    fn u32(&mut self) -> io::Result<u32> {
        Ok(u32::from_le_bytes(self.array()?))
    }

    fn u64(&mut self) -> io::Result<u64> {
        Ok(u64::from_le_bytes(self.array()?))
    }

    fn text(&mut self) -> io::Result<String> {
        String::from_utf8(self.next()?.to_vec())
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "a name that is not UTF-8"))
    }
}

impl Handoff {
    /// Its bytes.
    pub fn encode(&self) -> Vec<u8> {
        let n = &self.negotiated;
        let mut out = Vec::new();
        field(&mut out, b"smbd handoff 1");
        field(&mut out, &n.dialect.to_le_bytes());
        field(&mut out, &crypto::cipher_id(n.cipher).to_le_bytes());
        field(&mut out, &n.signing.id().to_le_bytes());
        field(&mut out, &n.client_guid);
        field(&mut out, &n.client_security_mode.to_le_bytes());
        field(&mut out, &n.client_capabilities.to_le_bytes());
        let dialects: Vec<u8> = n.client_dialects.iter().flat_map(|d| d.to_le_bytes()).collect();
        field(&mut out, &dialects);
        field(&mut out, &n.server_guid);
        field(&mut out, &n.server_capabilities.to_le_bytes());
        field(&mut out, &n.preauth);
        field(&mut out, &self.session_id.to_le_bytes());
        field(&mut out, &self.next_message.to_le_bytes());
        field(&mut out, &self.keys.signing);
        field(&mut out, &self.keys.decrypt);
        field(&mut out, &self.keys.encrypt);
        field(&mut out, self.user.as_bytes());
        field(&mut out, self.from.as_bytes());
        out
    }

    /// The handoff in `bytes`.
    pub fn decode(bytes: &[u8]) -> io::Result<Handoff> {
        let bad = |what: &str| io::Error::new(io::ErrorKind::InvalidData, what.to_string());
        let mut f = Fields(bytes);
        if f.next()? != b"smbd handoff 1" {
            return Err(bad("not a handoff of this smbd's"));
        }
        let dialect = f.u16()?;
        let cipher = crypto::cipher_of(f.u16()?).ok_or_else(|| bad("an unknown cipher"))?;
        let signing = Signing::of(f.u16()?).ok_or_else(|| bad("an unknown signature"))?;
        let client_guid = f.array()?;
        let client_security_mode = f.u16()?;
        let client_capabilities = f.u32()?;
        let client_dialects =
            f.next()?.as_chunks::<2>().0.iter().map(|&pair| u16::from_le_bytes(pair)).collect();
        let server_guid = f.array()?;
        let server_capabilities = f.u32()?;
        let preauth = f.array()?;
        let session_id = f.u64()?;
        let next_message = f.u64()?;
        let keys =
            Keys { signing: f.array()?, decrypt: f.next()?.to_vec(), encrypt: f.next()?.to_vec() };
        let user = f.text()?;
        let from = f.text()?;
        let negotiated = Negotiated {
            dialect,
            cipher,
            signing,
            client_guid,
            client_security_mode,
            client_capabilities,
            client_dialects,
            server_guid,
            server_capabilities,
            preauth,
        };
        Ok(Handoff { negotiated, session_id, next_message, keys, user, from })
    }

    /// Writes it to `to`, the pipe's write end.
    pub fn send(&self, to: &mut impl Write) -> io::Result<()> {
        let mut bytes = self.encode();
        let sent = to.write_all(&bytes);
        bytes.fill(0);
        sent
    }

    /// Reads one from `from`, the pipe's read end, to its end.
    pub fn receive(from: &mut impl Read) -> io::Result<Handoff> {
        let mut bytes = Vec::new();
        from.take(MAX as u64 + 1).read_to_end(&mut bytes)?;
        if bytes.len() > MAX {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "a handoff too long"));
        }
        let handoff = Handoff::decode(&bytes);
        bytes.fill(0);
        handoff
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use smb2::crypto::encryption::Cipher;

    #[test]
    fn a_handoff_goes_and_comes_back() {
        let negotiated = Negotiated {
            dialect: 0x0311,
            cipher: Cipher::Aes256Gcm,
            signing: Signing::Gmac,
            client_guid: [1; 16],
            client_security_mode: 1,
            client_capabilities: 0x7f,
            client_dialects: vec![0x0300, 0x0311],
            server_guid: [2; 16],
            server_capabilities: 4,
            preauth: [3; 64],
        };
        let keys = Keys { signing: [4; 16], decrypt: vec![5; 32], encrypt: vec![6; 32] };
        let handoff = Handoff {
            negotiated,
            session_id: 77,
            next_message: 3,
            keys,
            user: "alice".into(),
            from: "::1".into(),
        };
        let mut pipe = Vec::new();
        handoff.send(&mut pipe).unwrap();
        let back = Handoff::receive(&mut &pipe[..]).unwrap();
        assert_eq!(back.negotiated, handoff.negotiated);
        assert_eq!((back.session_id, back.next_message), (77, 3));
        assert_eq!((back.user.as_str(), back.from.as_str()), ("alice", "::1"));
        assert_eq!((back.keys.signing, back.keys.encrypt.clone()), ([4; 16], vec![6; 32]));
        for end in 0..pipe.len() {
            assert!(Handoff::decode(&pipe[..end]).is_err());
        }
    }
}
