// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Credentials: a ticket with its session key and times, kept in the
//! keyring (ADR 0397) rather than in a file under /tmp, as MIT keeps
//! them. Each is the keyring entry `krb5 CLIENT SERVER`, which holds the
//! credential as MIT's credential cache files hold one (the "ccache"
//! format, version 4), so a credential reads the same on both.
//!
//! Like MIT's default cache, the keyring holds one client's credentials
//! at a time: kinit for another principal replaces them. The keyring is
//! reached through [`Keyring`], which kinit(1) and the file systems give
//! over authd; the tests give one in memory. `keyring list` shows the
//! entries, and `klist` what they hold.

use crate::crypto::{Enctype, Key};
use crate::messages::{Principal, flags};
use crate::{Error, Result, time};

/// What each credential's keyring entry's name starts with.
pub const PREFIX: &str = "krb5 ";

/// A keyring: the person's own, over authd.
pub trait Keyring {
    /// Every entry's name.
    fn names(&mut self) -> Result<Vec<String>>;
    /// The secret of the entry `name`, if there is one.
    fn get(&mut self, name: &str) -> Result<Option<Vec<u8>>>;
    /// Keeps `secret` as the entry `name`.
    fn set(&mut self, name: &str, secret: &[u8]) -> Result<()>;
    /// Removes the entry `name`.
    fn remove(&mut self, name: &str) -> Result<()>;
}

/// A ticket and what its holder needs to use it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Credential {
    /// Whose it is.
    pub client: Principal,
    /// The service it is for.
    pub server: Principal,
    /// The session key.
    pub key: Key,
    /// When the client proved itself, in seconds since 1970.
    pub authtime: i64,
    /// When the ticket starts.
    pub starttime: i64,
    /// When it ends.
    pub endtime: i64,
    /// Until when it may be renewed, or 0.
    pub renew_till: i64,
    /// TicketFlags.
    pub flags: u32,
    /// The Ticket, as the KDC sent it.
    pub ticket: Vec<u8>,
}

impl Credential {
    /// Whether it is still good at `now`, with a minute to spare, so
    /// that it does not end on the way to the service.
    pub fn valid_at(&self, now: i64) -> bool {
        self.starttime <= now + 60 && now + 60 < self.endtime
    }

    /// Its keyring entry's name: `krb5 CLIENT SERVER`.
    pub fn entry(&self) -> String {
        format!("{PREFIX}{} {}", self.client, self.server)
    }

    /// The credential as a ccache file holds one (version 4): both
    /// principals, the key, the four times, flags, no addresses and no
    /// authorization data, the ticket and no second ticket. Integers
    /// are big-endian.
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        put_principal(&mut out, &self.client);
        put_principal(&mut out, &self.server);
        out.extend_from_slice(&(self.key.enctype.number() as u16).to_be_bytes());
        put_data(&mut out, self.key.bytes());
        for t in [self.authtime, self.starttime, self.endtime, self.renew_till] {
            out.extend_from_slice(&(t.clamp(0, u32::MAX.into()) as u32).to_be_bytes());
        }
        out.push(0); // is_skey: not a user-to-user ticket
        out.extend_from_slice(&self.flags.to_be_bytes());
        out.extend_from_slice(&0u32.to_be_bytes()); // addresses
        out.extend_from_slice(&0u32.to_be_bytes()); // authorization data
        put_data(&mut out, &self.ticket);
        put_data(&mut out, &[]); // second ticket
        out
    }

    /// The credential `data` holds, as [`Credential::encode`] writes it.
    pub fn decode(data: &[u8]) -> Result<Credential> {
        let mut r = Cursor { data };
        let client = r.principal()?;
        let server = r.principal()?;
        let enctype_number = r.u16()?;
        let key_bytes = r.data()?;
        let enctype = Enctype::from_number(enctype_number.into()).ok_or_else(|| {
            Error::Malformed(format!("a credential's key of type {enctype_number}"))
        })?;
        let key = Key::new(enctype, key_bytes)?;
        let authtime = r.u32()?.into();
        let starttime = r.u32()?.into();
        let endtime = r.u32()?.into();
        let renew_till = r.u32()?.into();
        let _is_skey = r.u8()?;
        let flags = r.u32()?;
        for _ in 0..r.u32()? {
            let _kind = r.u16()?;
            r.data()?;
        }
        for _ in 0..r.u32()? {
            let _kind = r.u16()?;
            r.data()?;
        }
        let ticket = r.data()?.to_vec();
        r.data()?;
        Ok(Credential {
            client,
            server,
            key,
            authtime,
            starttime,
            endtime,
            renew_till,
            flags,
            ticket,
        })
    }

    /// Its line in klist's listing: start, end, renew-till and the
    /// service, then its flags.
    pub fn listing(&self) -> String {
        let mut line = format!(
            "{}  {}  {}",
            time::shown(self.starttime),
            time::shown(self.endtime),
            self.server
        );
        if self.renew_till != 0 {
            line.push_str(&format!("\n\trenew until {}", time::shown(self.renew_till)));
        }
        let letters = flags::letters(self.flags);
        if !letters.is_empty() {
            line.push_str(&format!("\n\tFlags: {letters}, Etype: {}", self.key.enctype.name()));
        }
        line
    }
}

/// A principal as a ccache holds one: its name type, its parts' count,
/// the realm, then each part, each text after its length.
fn put_principal(out: &mut Vec<u8>, principal: &Principal) {
    out.extend_from_slice(&(principal.name_type as u32).to_be_bytes());
    out.extend_from_slice(&(principal.parts.len() as u32).to_be_bytes());
    put_data(out, principal.realm.as_bytes());
    for part in &principal.parts {
        put_data(out, part.as_bytes());
    }
}

/// `data` after its length in 4 bytes.
fn put_data(out: &mut Vec<u8>, data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    out.extend_from_slice(data);
}

/// Reads a ccache credential from the front.
struct Cursor<'a> {
    /// What is left.
    data: &'a [u8],
}

impl<'a> Cursor<'a> {
    /// The next `n` bytes.
    fn bytes(&mut self, n: usize) -> Result<&'a [u8]> {
        if self.data.len() < n {
            return Err(Error::Malformed("a credential that ends early".into()));
        }
        let (head, rest) = self.data.split_at(n);
        self.data = rest;
        Ok(head)
    }

    fn u8(&mut self) -> Result<u8> {
        Ok(self.bytes(1)?[0])
    }

    fn u16(&mut self) -> Result<u16> {
        Ok(u16::from_be_bytes(self.bytes(2)?.try_into().expect("2 bytes")))
    }

    fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_be_bytes(self.bytes(4)?.try_into().expect("4 bytes")))
    }

    /// Bytes after their length in 4 bytes.
    fn data(&mut self) -> Result<&'a [u8]> {
        let len = self.u32()? as usize;
        self.bytes(len)
    }

    /// Text after its length.
    fn text(&mut self) -> Result<String> {
        String::from_utf8(self.data()?.to_vec())
            .map_err(|_| Error::Malformed("a credential's name that is not UTF-8".into()))
    }

    fn principal(&mut self) -> Result<Principal> {
        let name_type = self.u32()? as i32;
        let count = self.u32()?;
        if count > 16 {
            return Err(Error::Malformed(format!("a principal of {count} parts")));
        }
        let realm = self.text()?;
        let parts = (0..count).map(|_| self.text()).collect::<Result<_>>()?;
        Ok(Principal { name_type, parts, realm })
    }
}

/// The keyring's credentials, as many as read; an entry that does not
/// read is skipped, so one bad entry does not hide the rest.
pub fn all(keyring: &mut dyn Keyring) -> Result<Vec<Credential>> {
    let mut out = Vec::new();
    for name in keyring.names()?.iter().filter(|name| name.starts_with(PREFIX)) {
        if let Some(secret) = keyring.get(name)?
            && let Ok(credential) = Credential::decode(&secret)
        {
            out.push(credential);
        }
    }
    Ok(out)
}

/// The client whose credentials the keyring holds, if any: the one of
/// its ticket-granting ticket, or else of any.
pub fn client(keyring: &mut dyn Keyring) -> Result<Option<Principal>> {
    let all = all(keyring)?;
    let tgt = all.iter().find(|c| c.server.is_tgs()).or_else(|| all.first());
    Ok(tgt.map(|c| c.client.clone()))
}

/// Keeps `credential`, as kinit does: the keyring's other credentials go
/// first, as a fresh cache starts empty.
pub fn start(keyring: &mut dyn Keyring, credential: &Credential) -> Result<()> {
    destroy(keyring)?;
    keyring.set(&credential.entry(), &credential.encode())
}

/// Keeps `credential` beside the others, as a service ticket is kept.
pub fn add(keyring: &mut dyn Keyring, credential: &Credential) -> Result<()> {
    keyring.set(&credential.entry(), &credential.encode())
}

/// The kept credential of `client` for `server`, if there is one.
pub fn find(
    keyring: &mut dyn Keyring,
    client: &Principal,
    server: &Principal,
) -> Result<Option<Credential>> {
    let name = format!("{PREFIX}{client} {server}");
    keyring.get(&name)?.map(|secret| Credential::decode(&secret)).transpose()
}

/// Removes every credential: how many there were.
pub fn destroy(keyring: &mut dyn Keyring) -> Result<usize> {
    let names: Vec<String> =
        keyring.names()?.into_iter().filter(|name| name.starts_with(PREFIX)).collect();
    for name in &names {
        keyring.remove(name)?;
    }
    Ok(names.len())
}

/// A keyring in memory, for tests here and in the crates that use this.
#[derive(Default)]
pub struct MemoryKeyring(pub std::collections::BTreeMap<String, Vec<u8>>);

impl Keyring for MemoryKeyring {
    fn names(&mut self) -> Result<Vec<String>> {
        Ok(self.0.keys().cloned().collect())
    }
    fn get(&mut self, name: &str) -> Result<Option<Vec<u8>>> {
        Ok(self.0.get(name).cloned())
    }
    fn set(&mut self, name: &str, secret: &[u8]) -> Result<()> {
        self.0.insert(name.into(), secret.to_vec());
        Ok(())
    }
    fn remove(&mut self, name: &str) -> Result<()> {
        self.0.remove(name);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn credential(client: &str, server: Principal) -> Credential {
        Credential {
            client: Principal::parse(client, None).unwrap(),
            server,
            key: Key::random(Enctype::Aes256Sha1).unwrap(),
            authtime: 1_762_400_000,
            starttime: 1_762_400_000,
            endtime: 1_762_436_000,
            renew_till: 0,
            flags: flags::INITIAL | flags::PRE_AUTHENT,
            ticket: vec![0x61, 3, 1, 2, 3],
        }
    }

    #[test]
    fn credentials_read_back_and_are_valid_while_they_last() {
        let c = credential("alice@EXAMPLE.ORG", Principal::tgs("EXAMPLE.ORG"));
        assert_eq!(Credential::decode(&c.encode()).unwrap(), c);
        assert_eq!(c.entry(), "krb5 alice@EXAMPLE.ORG krbtgt/EXAMPLE.ORG@EXAMPLE.ORG");
        assert!(c.valid_at(1_762_400_000));
        assert!(!c.valid_at(1_762_436_000 - 30));
        assert!(Credential::decode(&c.encode()[..20]).is_err());
    }

    #[test]
    fn kinit_starts_afresh_and_service_tickets_are_added() {
        let mut keyring = MemoryKeyring::default();
        keyring.set("git https://alice@example.org", b"token").unwrap();
        let tgt = credential("alice@EXAMPLE.ORG", Principal::tgs("EXAMPLE.ORG"));
        start(&mut keyring, &tgt).unwrap();
        let nfs =
            credential("alice@EXAMPLE.ORG", Principal::service("nfs", "server", "EXAMPLE.ORG"));
        add(&mut keyring, &nfs).unwrap();
        assert_eq!(all(&mut keyring).unwrap().len(), 2);
        assert_eq!(client(&mut keyring).unwrap(), Some(tgt.client.clone()));
        assert_eq!(find(&mut keyring, &nfs.client, &nfs.server).unwrap(), Some(nfs));
        let bob = credential("bob@EXAMPLE.ORG", Principal::tgs("EXAMPLE.ORG"));
        start(&mut keyring, &bob).unwrap();
        assert_eq!(all(&mut keyring).unwrap(), [bob]);
        assert_eq!(destroy(&mut keyring).unwrap(), 1);
        // What is not Kerberos's stays.
        assert_eq!(keyring.names().unwrap(), ["git https://alice@example.org"]);
    }
}
