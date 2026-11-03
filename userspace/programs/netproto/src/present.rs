// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! DNS records as text, in the presentation format of zone files (RFC
//! 1035's section 5, with the later types' own RFCs), as dig(1) and
//! host(1) show them (ADR 0341): each type's fields in order, names with
//! their final dot, text in quotes with what is not printable as `\DDD`,
//! keys and signatures in base64, digests in hexadecimal, and a type
//! this module does not know as RFC 3597's `\# LENGTH HEX`. A record
//! whose data does not hold what its type says is shown that way too.

use std::fmt::Write as _;
use std::net::{Ipv4Addr, Ipv6Addr};

use crate::dns::{self, Record};

// Types beyond those `dns` names.

/// A host's CPU and system (RFC 1035).
pub const HINFO: u16 = 13;
/// Who is responsible for a name (RFC 1183).
pub const RP: u16 = 17;
/// An AFS database's server (RFC 1183).
pub const AFSDB: u16 = 18;
/// A rule that rewrites a name (RFC 3403).
pub const NAPTR: u16 = 35;
/// A name for a whole subtree (RFC 6672).
pub const DNAME: u16 = 39;
/// The digest of a child zone's key (RFC 4034).
pub const DS: u16 = 43;
/// An SSH host key's fingerprint (RFC 4255).
pub const SSHFP: u16 = 44;
/// A signature of a set of records (RFC 4034).
pub const RRSIG: u16 = 46;
/// The next name, and the types a name has (RFC 4034).
pub const NSEC: u16 = 47;
/// A zone's key (RFC 4034).
pub const DNSKEY: u16 = 48;
/// NSEC's hashed kin (RFC 5155).
pub const NSEC3: u16 = 50;
/// How a zone's NSEC3 names are hashed (RFC 5155).
pub const NSEC3PARAM: u16 = 51;
/// A TLS server's certificate, for DANE (RFC 6698).
pub const TLSA: u16 = 52;
/// A DS a child asks its parent for (RFC 7344).
pub const CDS: u16 = 59;
/// A DNSKEY a child asks its parent for (RFC 7344).
pub const CDNSKEY: u16 = 60;
/// A service's endpoints and parameters (RFC 9460).
pub const SVCB: u16 = 64;
/// SVCB for HTTPS (RFC 9460).
pub const HTTPS: u16 = 65;
/// SPF's old type, text as TXT's (RFC 4408).
pub const SPF: u16 = 99;
/// A whole zone, in a question (RFC 5936).
pub const AXFR: u16 = 252;
/// The authorities that may issue a name's certificates (RFC 8659).
pub const CAA: u16 = 257;

// Classes beyond IN.

/// Chaos, where servers answer `version.bind` and the like.
pub const CH: u16 = 3;
/// Hesiod.
pub const HS: u16 = 4;
/// Every class, in a question.
pub const ANY_CLASS: u16 = 255;

/// Every type with a name, and the name.
const TYPES: &[(u16, &str)] = &[
    (dns::A, "A"),
    (dns::NS, "NS"),
    (dns::CNAME, "CNAME"),
    (dns::SOA, "SOA"),
    (dns::PTR, "PTR"),
    (HINFO, "HINFO"),
    (dns::MX, "MX"),
    (dns::TXT, "TXT"),
    (RP, "RP"),
    (AFSDB, "AFSDB"),
    (dns::AAAA, "AAAA"),
    (dns::SRV, "SRV"),
    (NAPTR, "NAPTR"),
    (DNAME, "DNAME"),
    (dns::OPT, "OPT"),
    (DS, "DS"),
    (SSHFP, "SSHFP"),
    (RRSIG, "RRSIG"),
    (NSEC, "NSEC"),
    (DNSKEY, "DNSKEY"),
    (NSEC3, "NSEC3"),
    (NSEC3PARAM, "NSEC3PARAM"),
    (TLSA, "TLSA"),
    (CDS, "CDS"),
    (CDNSKEY, "CDNSKEY"),
    (SVCB, "SVCB"),
    (HTTPS, "HTTPS"),
    (SPF, "SPF"),
    (AXFR, "AXFR"),
    (dns::ANY, "ANY"),
    (CAA, "CAA"),
];

/// A type's name: its mnemonic, or `TYPEn` (RFC 3597).
pub fn type_name(rtype: u16) -> String {
    TYPES
        .iter()
        .find(|(t, _)| *t == rtype)
        .map_or_else(|| format!("TYPE{rtype}"), |(_, name)| (*name).to_string())
}

/// The type named `name`, in any case, or written `TYPEn`.
pub fn type_by_name(name: &str) -> Option<u16> {
    let upper = name.to_ascii_uppercase();
    TYPES
        .iter()
        .find(|(_, n)| *n == upper)
        .map(|(t, _)| *t)
        .or_else(|| upper.strip_prefix("TYPE").and_then(|n| n.parse().ok()))
}

/// A class's name: IN, CH, HS, ANY, or `CLASSn`.
pub fn class_name(class: u16) -> String {
    match class {
        dns::IN => "IN".into(),
        CH => "CH".into(),
        HS => "HS".into(),
        ANY_CLASS => "ANY".into(),
        other => format!("CLASS{other}"),
    }
}

/// The class named `name`, in any case, or written `CLASSn`.
pub fn class_by_name(name: &str) -> Option<u16> {
    let upper = name.to_ascii_uppercase();
    match upper.as_str() {
        "IN" => Some(dns::IN),
        "CH" | "CHAOS" => Some(CH),
        "HS" | "HESIOD" => Some(HS),
        "ANY" => Some(ANY_CLASS),
        _ => upper.strip_prefix("CLASS").and_then(|n| n.parse().ok()),
    }
}

/// A response code's name, as dig's `status:` gives it.
pub fn rcode_name(rcode: u16) -> String {
    match rcode {
        0 => "NOERROR".into(),
        1 => "FORMERR".into(),
        2 => "SERVFAIL".into(),
        3 => "NXDOMAIN".into(),
        4 => "NOTIMP".into(),
        5 => "REFUSED".into(),
        6 => "YXDOMAIN".into(),
        7 => "YXRRSET".into(),
        8 => "NXRRSET".into(),
        9 => "NOTAUTH".into(),
        10 => "NOTZONE".into(),
        16 => "BADVERS".into(),
        23 => "BADCOOKIE".into(),
        other => format!("RESERVED{other}"),
    }
}

/// An opcode's name.
pub fn opcode_name(opcode: u16) -> String {
    match opcode {
        0 => "QUERY".into(),
        1 => "IQUERY".into(),
        2 => "STATUS".into(),
        4 => "NOTIFY".into(),
        5 => "UPDATE".into(),
        other => format!("RESERVED{other}"),
    }
}

/// The name at `at` with its final dot, or `None` if it does not read.
pub fn name_at(message: &[u8], at: usize) -> Option<(String, usize)> {
    let (name, end) = dns::read_name(message, at)?;
    Some((format!("{name}."), end))
}

/// The data of `record` of `message`, as a zone file writes it.
pub fn record_data(message: &[u8], record: &Record) -> String {
    let (start, end) = record.data;
    let data = &message[start..end];
    typed(message, record.rtype, start, end).unwrap_or_else(|| unknown(data))
}

/// RFC 3597's form: `\# LENGTH HEX`, the hexadecimal left out when there
/// is no data.
pub fn unknown(data: &[u8]) -> String {
    if data.is_empty() { "\\# 0".into() } else { format!("\\# {} {}", data.len(), hex(data)) }
}

/// Bytes in hexadecimal, as DS records show digests: upper case.
pub fn hex(data: &[u8]) -> String {
    data.iter().fold(String::with_capacity(data.len() * 2), |mut out, byte| {
        let _ = write!(out, "{byte:02X}");
        out
    })
}

/// Bytes in base64, with its padding (RFC 4648).
pub fn base64(data: &[u8]) -> String {
    const DIGITS: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let group =
            chunk.iter().enumerate().fold(0u32, |g, (i, &b)| g | u32::from(b) << (16 - 8 * i));
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(char::from(DIGITS[(group >> (18 - 6 * i) & 63) as usize]));
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// A character string (RFC 1035's <character-string>) in quotes, with a
/// quote and a backslash escaped and what is not printable as `\DDD`.
pub fn quoted(text: &[u8]) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for &byte in text {
        match byte {
            b'"' | b'\\' => {
                out.push('\\');
                out.push(char::from(byte));
            }
            0x20..=0x7e => out.push(char::from(byte)),
            _ => {
                let _ = write!(out, "\\{byte:03}");
            }
        }
    }
    out.push('"');
    out
}

/// The character strings that make up `data`, or `None` if one runs
/// past its end.
fn strings(data: &[u8]) -> Option<Vec<&[u8]>> {
    let mut out = Vec::new();
    let mut at = 0;
    while let Some(&len) = data.get(at) {
        out.push(data.get(at + 1..at + 1 + usize::from(len))?);
        at += 1 + usize::from(len);
    }
    Some(out)
}

/// A time of RRSIG's, seconds since 1970, as `YYYYMMDDHHMMSS` in UTC.
fn sig_time(seconds: u32) -> String {
    let days = i64::from(seconds / 86_400);
    let rest = seconds % 86_400;
    // Howard Hinnant's days-to-civil, for days since 1970-01-01.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}{month:02}{day:02}{:02}{:02}{:02}", rest / 3600, rest / 60 % 60, rest % 60)
}

/// The types an NSEC or NSEC3 record's bitmap names (RFC 4034).
fn type_bitmap(data: &[u8]) -> Option<Vec<String>> {
    let mut types = Vec::new();
    let mut at = 0;
    while at < data.len() {
        let window = u16::from(*data.get(at)?);
        let len = usize::from(*data.get(at + 1)?);
        let bits = data.get(at + 2..at + 2 + len).filter(|b| (1..=32).contains(&b.len()))?;
        for (i, &byte) in bits.iter().enumerate() {
            for bit in 0..8 {
                if byte & (0x80 >> bit) != 0 {
                    types.push(type_name(window * 256 + (i * 8 + bit) as u16));
                }
            }
        }
        at += 2 + len;
    }
    Some(types)
}

fn u16_at(message: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_be_bytes([*message.get(at)?, *message.get(at + 1)?]))
}

fn u32_at(message: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_be_bytes(message.get(at..at + 4)?.try_into().ok()?))
}

/// The data from `start` to `end` of `message` as type `rtype`'s, or
/// `None` for a type it does not know or data that does not fit it.
fn typed(message: &[u8], rtype: u16, start: usize, end: usize) -> Option<String> {
    let data = message.get(start..end)?;
    // A name inside the data, which must end inside it too.
    let name = |at: usize| name_at(message, at).filter(|&(_, after)| after <= end);
    let only_name = |at: usize| name(at).filter(|&(_, after)| after == end).map(|(n, _)| n);
    Some(match rtype {
        dns::A => Ipv4Addr::from(<[u8; 4]>::try_from(data).ok()?).to_string(),
        dns::AAAA => Ipv6Addr::from(<[u8; 16]>::try_from(data).ok()?).to_string(),
        dns::NS | dns::CNAME | dns::PTR | DNAME => only_name(start)?,
        dns::MX | AFSDB => format!("{} {}", u16_at(message, start)?, only_name(start + 2)?),
        dns::TXT | SPF => strings(data)?.iter().map(|s| quoted(s)).collect::<Vec<_>>().join(" "),
        HINFO => {
            let parts = strings(data)?;
            if parts.len() != 2 {
                return None;
            }
            format!("{} {}", quoted(parts[0]), quoted(parts[1]))
        }
        RP => {
            let (mailbox, at) = name(start)?;
            format!("{mailbox} {}", only_name(at)?)
        }
        dns::SOA => {
            let (mname, at) = name(start)?;
            let (rname, at) = name(at)?;
            if at + 20 != end {
                return None;
            }
            let n: Vec<u32> = (0..5).map(|i| u32_at(message, at + 4 * i)).collect::<Option<_>>()?;
            format!("{mname} {rname} {} {} {} {} {}", n[0], n[1], n[2], n[3], n[4])
        }
        dns::SRV => format!(
            "{} {} {} {}",
            u16_at(message, start)?,
            u16_at(message, start + 2)?,
            u16_at(message, start + 4)?,
            only_name(start + 6)?
        ),
        NAPTR => {
            let order = u16_at(message, start)?;
            let preference = u16_at(message, start + 2)?;
            let mut at = start + 4;
            let mut texts = Vec::new();
            for _ in 0..3 {
                let len = usize::from(*message.get(at)?);
                texts.push(quoted(
                    message.get(at + 1..at + 1 + len).filter(|_| at + 1 + len <= end)?,
                ));
                at += 1 + len;
            }
            format!("{order} {preference} {} {}", texts.join(" "), only_name(at)?)
        }
        DS | CDS => {
            if data.len() < 5 {
                return None;
            }
            format!("{} {} {} {}", u16_at(message, start)?, data[2], data[3], hex(&data[4..]))
        }
        DNSKEY | CDNSKEY => {
            if data.len() < 5 {
                return None;
            }
            format!("{} {} {} {}", u16_at(message, start)?, data[2], data[3], base64(&data[4..]))
        }
        SSHFP => {
            if data.len() < 3 {
                return None;
            }
            format!("{} {} {}", data[0], data[1], hex(&data[2..]))
        }
        TLSA => {
            if data.len() < 4 {
                return None;
            }
            format!("{} {} {} {}", data[0], data[1], data[2], hex(&data[3..]))
        }
        RRSIG => {
            if data.len() < 19 {
                return None;
            }
            let (signer, at) = name(start + 18)?;
            format!(
                "{} {} {} {} {} {} {} {signer} {}",
                type_name(u16_at(message, start)?),
                data[2],
                data[3],
                u32_at(message, start + 4)?,
                sig_time(u32_at(message, start + 8)?),
                sig_time(u32_at(message, start + 12)?),
                u16_at(message, start + 16)?,
                base64(&message[at..end])
            )
        }
        NSEC => {
            let (next, at) = name(start)?;
            let types = type_bitmap(&message[at..end])?;
            if types.is_empty() { next } else { format!("{next} {}", types.join(" ")) }
        }
        NSEC3PARAM => {
            let salt_len = usize::from(*data.get(4)?);
            if data.len() != 5 + salt_len {
                return None;
            }
            let salt = if salt_len == 0 { "-".into() } else { hex(&data[5..]) };
            format!("{} {} {} {salt}", data[0], data[1], u16_at(message, start + 2)?)
        }
        CAA => {
            let flags = *data.first()?;
            let tag_len = usize::from(*data.get(1)?);
            let tag = data.get(2..2 + tag_len).filter(|t| !t.is_empty())?;
            if !tag.iter().all(u8::is_ascii_alphanumeric) {
                return None;
            }
            format!("{flags} {} {}", String::from_utf8_lossy(tag), quoted(&data[2 + tag_len..]))
        }
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A message holding one answer of type `rtype` whose data is `data`,
    /// with its owner `example.` at offset 12 so names can point there.
    fn message(rtype: u16, data: &[u8]) -> (Vec<u8>, Record) {
        let mut m = vec![0, 1, 0x81, 0x80, 0, 0, 0, 1, 0, 0, 0, 0];
        m.extend_from_slice(b"\x07example\x00");
        m.extend_from_slice(&rtype.to_be_bytes());
        m.extend_from_slice(&[0, 1, 0, 0, 0x0e, 0x10]);
        m.extend_from_slice(&(data.len() as u16).to_be_bytes());
        let start = m.len();
        m.extend_from_slice(data);
        let records = dns::records(&m).unwrap();
        assert_eq!(records[0].data, (start, m.len()));
        let record = records[0];
        (m, record)
    }

    fn text(rtype: u16, data: &[u8]) -> String {
        let (m, r) = message(rtype, data);
        record_data(&m, &r)
    }

    #[test]
    fn records_read_as_zone_files_write_them() {
        assert_eq!(text(dns::A, &[192, 0, 2, 1]), "192.0.2.1");
        assert_eq!(text(dns::MX, &[0, 10, 0xc0, 12]), "10 example.");
        assert_eq!(text(dns::TXT, b"\x05he\"y\x01\x01\x07"), "\"he\\\"y\\001\" \"\\007\"");
        let mut soa = b"\x02ns\xc0\x0c\x04host\xc0\x0c".to_vec();
        for n in [2026110201u32, 7200, 900, 1209600, 300] {
            soa.extend_from_slice(&n.to_be_bytes());
        }
        assert_eq!(
            text(dns::SOA, &soa),
            "ns.example. host.example. 2026110201 7200 900 1209600 300"
        );
        assert_eq!(text(dns::SRV, &[0, 1, 0, 5, 0x01, 0xbb, 0xc0, 12]), "1 5 443 example.");
        assert_eq!(text(CAA, b"\x00\x05issueletsencrypt.org"), "0 issue \"letsencrypt.org\"");
        assert_eq!(text(DS, &[0x30, 0x39, 13, 2, 0xab, 0x01]), "12345 13 2 AB01");
        assert_eq!(text(DNSKEY, &[1, 1, 3, 13, b'M', b'a', b'n']), "257 3 13 TWFu");
        assert_eq!(text(SSHFP, &[4, 2, 0xde, 0xad]), "4 2 DEAD");
        // The NSEC of example. naming A (1), MX (15) and RRSIG (46).
        let mut nsec = b"\x01a\xc0\x0c".to_vec();
        nsec.extend_from_slice(&[0, 6, 0x40, 0x01, 0, 0, 0, 0x02]);
        assert_eq!(text(NSEC, &nsec), "a.example. A MX RRSIG");
    }

    #[test]
    fn signatures_show_their_times_and_keys() {
        let mut sig = vec![0, 1, 13, 1, 0, 0, 0x0e, 0x10];
        sig.extend_from_slice(&1_798_761_600u32.to_be_bytes()); // 2027-01-01
        sig.extend_from_slice(&1_761_955_200u32.to_be_bytes()); // 2025-11-01
        sig.extend_from_slice(&[0x30, 0x39, 0xc0, 12, b'M', b'a']);
        assert_eq!(
            text(RRSIG, &sig),
            "A 13 1 3600 20270101000000 20251101000000 12345 example. TWE="
        );
    }

    #[test]
    fn what_does_not_fit_its_type_shows_as_unknown() {
        assert_eq!(text(dns::A, &[1, 2, 3]), "\\# 3 010203");
        assert_eq!(text(4321, &[]), "\\# 0");
        assert_eq!(text(dns::TXT, b"\x05ab"), "\\# 3 056162");
        // A name that runs past the record's data.
        assert_eq!(text(dns::CNAME, &[0xc0]), "\\# 1 C0");
        assert_eq!(type_name(4321), "TYPE4321");
        assert_eq!(type_by_name("caa"), Some(CAA));
        assert_eq!(type_by_name("type99"), Some(99));
        assert_eq!(class_by_name("chaos"), Some(CH));
        assert_eq!(base64(b"Ma"), "TWE=");
        assert_eq!(base64(b"M"), "TQ==");
        assert_eq!(base64(b""), "");
    }
}
