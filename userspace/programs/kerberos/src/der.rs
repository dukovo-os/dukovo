// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The few parts of ASN.1's DER that Kerberos's messages use (RFC 4120
//! 5.2): sequences, explicit context and application tags, integers,
//! octet strings, GeneralString text, GeneralizedTime and the 32-bit
//! BIT STRINGs of options and flags.
//!
//! Writing builds each value from its parts, innermost first, so a
//! message reads as its ASN.1 does. Reading takes a value apart one field
//! at a time with [`Reader`], refusing what DER does not allow: a length
//! past the end, or one in more bytes than it needs. A message that does
//! not read starts its error with what was being read; the bytes in a
//! packet capture (`ob test boot` keeps one where a test asks) show which
//! field is wrong.

use crate::{Error, Result, time};

/// A SEQUENCE's tag.
pub const SEQUENCE: u8 = 0x30;
/// An INTEGER's tag.
const INTEGER: u8 = 0x02;
/// A BIT STRING's tag.
const BIT_STRING: u8 = 0x03;
/// An OCTET STRING's tag.
const OCTET_STRING: u8 = 0x04;
/// A GeneralizedTime's tag: KerberosTime.
const GENERALIZED_TIME: u8 = 0x18;
/// A GeneralString's tag: KerberosString and Realm.
const GENERAL_STRING: u8 = 0x1b;

/// `content` with its tag and length before it.
pub fn tlv(tag: u8, content: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(content.len() + 6);
    out.push(tag);
    let len = content.len();
    if len < 0x80 {
        out.push(len as u8);
    } else {
        // The length in as few bytes as it takes, after a byte that
        // counts them.
        let bytes = len.to_be_bytes();
        let skip = bytes.iter().take_while(|&&b| b == 0).count();
        out.push(0x80 | (bytes.len() - skip) as u8);
        out.extend_from_slice(&bytes[skip..]);
    }
    out.extend_from_slice(content);
    out
}

/// A SEQUENCE of `fields`, which are whole values already.
pub fn sequence(fields: &[Vec<u8>]) -> Vec<u8> {
    tlv(SEQUENCE, &fields.concat())
}

/// `value` under the explicit context tag `[number]`.
pub fn field(number: u8, value: &[u8]) -> Vec<u8> {
    tlv(0xa0 | number, value)
}

/// `value` under the explicit application tag `[APPLICATION number]`,
/// which names each of Kerberos's messages.
pub fn application(number: u8, value: &[u8]) -> Vec<u8> {
    tlv(0x60 | number, value)
}

/// An INTEGER, in as few bytes as hold it with its sign.
pub fn integer(value: i64) -> Vec<u8> {
    let bytes = value.to_be_bytes();
    let mut start = 0;
    // A leading byte may go when the next byte's top bit says the same.
    while start < 7
        && ((bytes[start] == 0 && bytes[start + 1] & 0x80 == 0)
            || (bytes[start] == 0xff && bytes[start + 1] & 0x80 != 0))
    {
        start += 1;
    }
    tlv(INTEGER, &bytes[start..])
}

/// An OCTET STRING.
pub fn octets(value: &[u8]) -> Vec<u8> {
    tlv(OCTET_STRING, value)
}

/// A GeneralString: a KerberosString or a Realm.
pub fn string(value: &str) -> Vec<u8> {
    tlv(GENERAL_STRING, value.as_bytes())
}

/// A KerberosTime: `seconds` since 1970 as a GeneralizedTime in UTC,
/// `YYYYMMDDHHMMSSZ`.
pub fn kerberos_time(seconds: i64) -> Vec<u8> {
    tlv(GENERALIZED_TIME, time::generalized(seconds).as_bytes())
}

/// A 32-bit BIT STRING, as KDCOptions, APOptions and TicketFlags are:
/// bit 0 is the top bit of `bits`.
pub fn bits(bits: u32) -> Vec<u8> {
    let mut content = vec![0];
    content.extend_from_slice(&bits.to_be_bytes());
    tlv(BIT_STRING, &content)
}

/// Takes DER values apart, one at a time, from the front.
#[derive(Clone, Copy)]
pub struct Reader<'a> {
    /// What is left to read.
    rest: &'a [u8],
    /// What is being read, for errors.
    what: &'static str,
}

impl<'a> Reader<'a> {
    /// Reads `data`, which is `what` (for errors).
    pub fn new(data: &'a [u8], what: &'static str) -> Self {
        Reader { rest: data, what }
    }

    /// Whether everything was read.
    pub fn is_empty(&self) -> bool {
        self.rest.is_empty()
    }

    /// The next value's tag, if there is one.
    pub fn peek(&self) -> Option<u8> {
        self.rest.first().copied()
    }

    /// An error about what is being read.
    pub fn malformed(&self, why: &str) -> Error {
        Error::Malformed(format!("{}: {why}", self.what))
    }

    /// The next value, whatever its tag: the tag, the content, and the
    /// whole value with its tag and length.
    pub fn any(&mut self) -> Result<(u8, &'a [u8], &'a [u8])> {
        let data = self.rest;
        let (&tag, after) = data.split_first().ok_or_else(|| self.malformed("ends early"))?;
        if tag & 0x1f == 0x1f {
            return Err(self.malformed("a tag past 30, which Kerberos never uses"));
        }
        let (&first, after) = after.split_first().ok_or_else(|| self.malformed("ends early"))?;
        let (len, after) = if first < 0x80 {
            (first as usize, after)
        } else {
            let count = (first & 0x7f) as usize;
            if count == 0 || count > 4 || after.len() < count {
                return Err(self.malformed("a length DER does not allow"));
            }
            let (len_bytes, after) = after.split_at(count);
            let len = len_bytes.iter().fold(0usize, |len, &b| len << 8 | b as usize);
            // DER says each length in as few bytes as it can.
            if len < 0x80 || len_bytes[0] == 0 {
                return Err(self.malformed("a length in more bytes than it needs"));
            }
            (len, after)
        };
        if after.len() < len {
            return Err(self.malformed("a value longer than the message"));
        }
        let (content, rest) = after.split_at(len);
        let whole = &data[..data.len() - rest.len()];
        self.rest = rest;
        Ok((tag, content, whole))
    }

    /// The next value, which must have `tag`: its content.
    pub fn take(&mut self, tag: u8) -> Result<&'a [u8]> {
        let found = self.peek();
        if found != Some(tag) {
            return Err(self.malformed(&match found {
                Some(found) => format!("tag {found:#04x} where {tag:#04x} belongs"),
                None => format!("ends where tag {tag:#04x} belongs"),
            }));
        }
        Ok(self.any()?.1)
    }

    /// The field `[number]`, which must be next: the value inside it.
    pub fn field(&mut self, number: u8) -> Result<&'a [u8]> {
        self.take(0xa0 | number)
    }

    /// The field `[number]`, if it is next.
    pub fn optional(&mut self, number: u8) -> Result<Option<&'a [u8]>> {
        if self.peek() == Some(0xa0 | number) { self.field(number).map(Some) } else { Ok(None) }
    }

    /// Skips what is left: fields after those this reader knows, which a
    /// later version of a message may add.
    pub fn skip_rest(&mut self) {
        self.rest = &[];
    }
}

/// The content of `value`, which must be one whole value with `tag`.
fn whole<'a>(value: &'a [u8], tag: u8, what: &'static str) -> Result<&'a [u8]> {
    let mut reader = Reader::new(value, what);
    let content = reader.take(tag)?;
    if !reader.is_empty() {
        return Err(reader.malformed("bytes after its value"));
    }
    Ok(content)
}

/// The fields of the SEQUENCE `value`, to read in turn.
pub fn read_sequence<'a>(value: &'a [u8], what: &'static str) -> Result<Reader<'a>> {
    Ok(Reader::new(whole(value, SEQUENCE, what)?, what))
}

/// The value inside `[APPLICATION number]`.
pub fn read_application<'a>(value: &'a [u8], number: u8, what: &'static str) -> Result<&'a [u8]> {
    whole(value, 0x60 | number, what)
}

/// The INTEGER `value`.
pub fn read_integer(value: &[u8], what: &'static str) -> Result<i64> {
    let content = whole(value, INTEGER, what)?;
    if content.is_empty() || content.len() > 8 {
        return Err(Error::Malformed(format!("{what}: an integer of {} bytes", content.len())));
    }
    let negative = content[0] & 0x80 != 0;
    let start = if negative { -1i64 } else { 0 };
    Ok(content.iter().fold(start, |n, &b| n << 8 | b as i64))
}

/// The INTEGER `value`, which must fit in an Int32.
pub fn read_int32(value: &[u8], what: &'static str) -> Result<i32> {
    let n = read_integer(value, what)?;
    i32::try_from(n).map_err(|_| Error::Malformed(format!("{what}: {n} is not an Int32")))
}

/// The INTEGER `value`, which must fit in a UInt32.
pub fn read_uint32(value: &[u8], what: &'static str) -> Result<u32> {
    let n = read_integer(value, what)?;
    u32::try_from(n).map_err(|_| Error::Malformed(format!("{what}: {n} is not a UInt32")))
}

/// The OCTET STRING `value`.
pub fn read_octets<'a>(value: &'a [u8], what: &'static str) -> Result<&'a [u8]> {
    whole(value, OCTET_STRING, what)
}

/// The GeneralString `value`, which Kerberos keeps to UTF-8 in practice.
pub fn read_string(value: &[u8], what: &'static str) -> Result<String> {
    let content = whole(value, GENERAL_STRING, what)?;
    String::from_utf8(content.to_vec())
        .map_err(|_| Error::Malformed(format!("{what}: text that is not UTF-8")))
}

/// The KerberosTime `value`, as seconds since 1970.
pub fn read_time(value: &[u8], what: &'static str) -> Result<i64> {
    let content = whole(value, GENERALIZED_TIME, what)?;
    std::str::from_utf8(content)
        .ok()
        .and_then(time::parse_generalized)
        .ok_or_else(|| Error::Malformed(format!("{what}: a time that is not YYYYMMDDHHMMSSZ")))
}

/// The 32-bit BIT STRING `value`: bit 0 is the top bit. A shorter string
/// has the bits it leaves out clear.
pub fn read_bits(value: &[u8], what: &'static str) -> Result<u32> {
    let content = whole(value, BIT_STRING, what)?;
    let Some((&_unused, bytes)) = content.split_first() else {
        return Err(Error::Malformed(format!("{what}: an empty BIT STRING")));
    };
    let mut word = [0u8; 4];
    for (to, from) in word.iter_mut().zip(bytes) {
        *to = *from;
    }
    Ok(u32::from_be_bytes(word))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn integers_take_as_few_bytes_as_hold_them() {
        assert_eq!(integer(0), [0x02, 1, 0]);
        assert_eq!(integer(127), [0x02, 1, 0x7f]);
        assert_eq!(integer(128), [0x02, 2, 0, 0x80]);
        assert_eq!(integer(-1), [0x02, 1, 0xff]);
        assert_eq!(integer(-129), [0x02, 2, 0xff, 0x7f]);
        assert_eq!(integer(0x7fff_ffff), [0x02, 4, 0x7f, 0xff, 0xff, 0xff]);
        for n in [0, 1, -1, 127, 128, -128, -129, 65_535, i64::from(i32::MIN), i64::MAX] {
            assert_eq!(read_integer(&integer(n), "n").unwrap(), n);
        }
    }

    #[test]
    fn long_lengths_read_back() {
        let content = vec![7u8; 300];
        let value = octets(&content);
        assert_eq!(&value[..4], [0x04, 0x82, 0x01, 0x2c]);
        assert_eq!(read_octets(&value, "big").unwrap(), &content[..]);
    }

    #[test]
    fn lengths_der_forbids_are_refused() {
        // 5 in two bytes, then a value longer than the message.
        assert!(read_octets(&[0x04, 0x81, 0x05, 1, 2, 3, 4, 5], "o").is_err());
        assert!(read_octets(&[0x04, 0x05, 1, 2], "o").is_err());
        assert!(read_octets(&[0x04, 0x01, 1, 2], "o").is_err());
    }

    #[test]
    fn fields_are_read_in_turn_and_optional_ones_may_be_missing() {
        let value = sequence(&[field(0, &integer(5)), field(2, &string("EXAMPLE.ORG"))]);
        let mut seq = read_sequence(&value, "test").unwrap();
        assert_eq!(read_integer(seq.field(0).unwrap(), "pvno").unwrap(), 5);
        assert!(seq.optional(1).unwrap().is_none());
        assert_eq!(read_string(seq.field(2).unwrap(), "realm").unwrap(), "EXAMPLE.ORG");
        assert!(seq.is_empty());
        assert!(seq.field(3).is_err());
    }

    #[test]
    fn bit_strings_put_bit_0_first() {
        // forwardable (1), renewable (8), canonicalize (15).
        let options = 1 << 30 | 1 << 23 | 1 << 16;
        assert_eq!(bits(options), [0x03, 5, 0, 0x40, 0x81, 0, 0]);
        assert_eq!(read_bits(&bits(options), "options").unwrap(), options);
        assert_eq!(read_bits(&[0x03, 2, 0, 0x40], "short").unwrap(), 1 << 30);
    }

    #[test]
    fn times_are_generalized_time_in_utc() {
        let value = kerberos_time(1_000_000_000);
        assert_eq!(&value[2..], b"20010909014640Z");
        assert_eq!(read_time(&value, "t").unwrap(), 1_000_000_000);
    }
}
