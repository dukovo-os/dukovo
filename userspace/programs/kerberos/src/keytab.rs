// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Keytabs: a service's keys in a file, as MIT's kadmin writes them
//! (`ktadd`, format version 0x0502), so that a machine's services, as
//! nfsd(8), prove themselves without a password, and `kinit -k` gets
//! tickets with a key rather than one typed.
//!
//! The machine's keytab is /system/settings/krb5.keytab, or the file
//! `KRB5_KTNAME` names (with or without MIT's `FILE:` before it). A
//! keytab holds keys as a password would, so it is readable by root
//! alone: copy it from the KDC with `kadmin -q "ktadd -k FILE nfs/HOST"`
//! and `install -m 600`. `klist -k` lists what one holds.

use crate::crypto::{Enctype, Key};
use crate::messages::Principal;
use crate::{Error, Result};

/// Where the machine's keytab is, unless `KRB5_KTNAME` says.
pub const PATH: &str = "/system/settings/krb5.keytab";

/// One key in a keytab.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    /// Whose key it is.
    pub principal: Principal,
    /// When it was written, in seconds since 1970.
    pub timestamp: u32,
    /// The key's version number (kvno).
    pub kvno: u32,
    /// The key.
    pub key: Key,
}

/// The keytab in force: `KRB5_KTNAME`'s or [`PATH`].
pub fn path() -> String {
    let named = std::env::var("KRB5_KTNAME").unwrap_or_else(|_| PATH.into());
    named.strip_prefix("FILE:").map(str::to_string).unwrap_or(named)
}

/// The keys of the keytab at `path`, of the types Dukovo speaks; keys
/// of older types (DES, RC4) are skipped.
pub fn read(path: &str) -> Result<Vec<Entry>> {
    let data = std::fs::read(path).map_err(|err| {
        Error::Other(format!("reading the keytab {path}: {err}; `klist -k {path}` lists one"))
    })?;
    parse(&data).map_err(|err| Error::Other(format!("{path}: {err}")))
}

/// The keys of the keytab `data`.
pub fn parse(data: &[u8]) -> Result<Vec<Entry>> {
    let rest = data
        .strip_prefix(&[0x05, 0x02])
        .ok_or_else(|| Error::Malformed("not a keytab of version 0x0502, as MIT writes".into()))?;
    let mut r = Cursor { data: rest };
    let mut out = Vec::new();
    while !r.data.is_empty() {
        let size = r.u32()? as i32;
        // A negative size is a hole where an entry was removed.
        let record = r.bytes(size.unsigned_abs() as usize)?;
        if size > 0
            && let Some(entry) = entry(record)?
        {
            out.push(entry);
        }
    }
    Ok(out)
}

/// One record's entry, or `None` for a key of a type Dukovo does not
/// speak.
fn entry(record: &[u8]) -> Result<Option<Entry>> {
    let mut r = Cursor { data: record };
    let count = r.u16()?;
    let realm = r.text()?;
    let parts = (0..count).map(|_| r.text()).collect::<Result<Vec<_>>>()?;
    let name_type = r.u32()? as i32;
    let timestamp = r.u32()?;
    let kvno8 = r.u8()?;
    let keytype = r.u16()?;
    let len = r.u16()? as usize;
    let key_bytes = r.bytes(len)?;
    // A 32-bit kvno follows when there is room; it is the right one
    // past 255.
    let kvno = match r.data.len() {
        n if n >= 4 => match r.u32()? {
            0 => kvno8.into(),
            big => big,
        },
        _ => kvno8.into(),
    };
    let Some(enctype) = Enctype::from_number(keytype.into()) else { return Ok(None) };
    let principal = Principal { name_type, parts, realm };
    Ok(Some(Entry { principal, timestamp, kvno, key: Key::new(enctype, key_bytes)? }))
}

/// `entries` as a keytab file: for tests, and for the boot tests' hosts.
pub fn write(entries: &[Entry]) -> Vec<u8> {
    let mut out = vec![0x05, 0x02];
    for entry in entries {
        let mut record = Vec::new();
        record.extend_from_slice(&(entry.principal.parts.len() as u16).to_be_bytes());
        put_text(&mut record, &entry.principal.realm);
        for part in &entry.principal.parts {
            put_text(&mut record, part);
        }
        record.extend_from_slice(&(entry.principal.name_type as u32).to_be_bytes());
        record.extend_from_slice(&entry.timestamp.to_be_bytes());
        record.push(entry.kvno.min(255) as u8);
        record.extend_from_slice(&(entry.key.enctype.number() as u16).to_be_bytes());
        record.extend_from_slice(&(entry.key.bytes().len() as u16).to_be_bytes());
        record.extend_from_slice(entry.key.bytes());
        record.extend_from_slice(&entry.kvno.to_be_bytes());
        out.extend_from_slice(&(record.len() as u32).to_be_bytes());
        out.extend_from_slice(&record);
    }
    out
}

/// Text after its length in 2 bytes.
fn put_text(out: &mut Vec<u8>, text: &str) {
    out.extend_from_slice(&(text.len() as u16).to_be_bytes());
    out.extend_from_slice(text.as_bytes());
}

/// The keys of `entries` for `principal`, the newest version of each
/// type, in the order of `wanted`.
pub fn keys_for(entries: &[Entry], principal: &Principal, wanted: &[Enctype]) -> Vec<(u32, Key)> {
    let mut out = Vec::new();
    for &enctype in wanted {
        let newest = entries
            .iter()
            .filter(|e| e.principal.same(principal) && e.key.enctype == enctype)
            .max_by_key(|e| e.kvno);
        if let Some(entry) = newest {
            out.push((entry.kvno, entry.key.clone()));
        }
    }
    out
}

/// Reads a keytab from the front: big-endian, as version 0x0502 is.
struct Cursor<'a> {
    /// What is left.
    data: &'a [u8],
}

impl<'a> Cursor<'a> {
    fn bytes(&mut self, n: usize) -> Result<&'a [u8]> {
        if self.data.len() < n {
            return Err(Error::Malformed("a keytab that ends inside an entry".into()));
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

    fn text(&mut self) -> Result<String> {
        let len = self.u16()? as usize;
        String::from_utf8(self.bytes(len)?.to_vec())
            .map_err(|_| Error::Malformed("a keytab name that is not UTF-8".into()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keytabs_read_back_and_give_the_newest_key() {
        let nfs = Principal::service("nfs", "server.example.org", "EXAMPLE.ORG");
        let entry = |kvno, enctype| Entry {
            principal: nfs.clone(),
            timestamp: 1_762_400_000,
            kvno,
            key: Key::random(enctype).unwrap(),
        };
        let entries = [
            entry(2, Enctype::Aes256Sha1),
            entry(3, Enctype::Aes256Sha1),
            entry(300, Enctype::Aes128Sha1),
        ];
        let back = parse(&write(&entries)).unwrap();
        assert_eq!(back, entries);
        let keys = keys_for(
            &back,
            &nfs,
            &[Enctype::Aes256Sha384, Enctype::Aes256Sha1, Enctype::Aes128Sha1],
        );
        assert_eq!(keys.iter().map(|(kvno, _)| *kvno).collect::<Vec<_>>(), [3, 300]);
        assert!(parse(b"\x05\x01").is_err());
    }

    #[test]
    fn holes_and_old_types_are_skipped() {
        let mut data = vec![0x05, 0x02];
        data.extend_from_slice(&(-4i32).to_be_bytes());
        data.extend_from_slice(&[0; 4]);
        // An RC4 key (type 23) of alice@R, version 1.
        let mut record = vec![0, 1, 0, 1, b'R', 0, 5];
        record.extend_from_slice(b"alice");
        record.extend_from_slice(&1u32.to_be_bytes());
        record.extend_from_slice(&0u32.to_be_bytes());
        record.push(1);
        record.extend_from_slice(&23u16.to_be_bytes());
        record.extend_from_slice(&16u16.to_be_bytes());
        record.extend_from_slice(&[9; 16]);
        data.extend_from_slice(&(record.len() as u32).to_be_bytes());
        data.extend_from_slice(&record);
        assert_eq!(parse(&data).unwrap(), []);
    }
}
