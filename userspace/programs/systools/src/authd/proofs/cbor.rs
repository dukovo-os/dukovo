// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The part of CBOR (RFC 8949) that WebAuthn's registrations use: an
//! attestation object, the COSE key in it, and the extensions after it
//! (ADR 0514).
//!
//! Integers, byte and text strings, arrays and maps of definite length,
//! `false`, `true` and `null`. Everything else is refused: lengths left
//! open, tags, floats and the other simple values, none of which an
//! authenticator writes in these places. Nothing is allocated for a
//! length before the bytes it counts are there, and nesting, items and
//! input are bounded, so no input makes the decoder take more than a
//! few kilobytes or recurse deeper than [`DEPTH_MAX`].

/// The deepest an item nests: an attestation object's statement holds
/// arrays of certificates, three levels down.
pub const DEPTH_MAX: usize = 8;

/// The most items one decoding takes, counting each map's keys and
/// values.
pub const ITEMS_MAX: usize = 256;

/// The longest input decoded, in bytes: an attestation object with a
/// certificate chain is a few kilobytes.
pub const INPUT_MAX: usize = 16 * 1024;

/// One decoded item, borrowing its strings from the input.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Item<'a> {
    /// An unsigned integer.
    Unsigned(u64),
    /// A negative integer, `-1 - n` for the `n` held.
    Negative(u64),
    /// A byte string.
    Bytes(&'a [u8]),
    /// A text string, checked to be UTF-8.
    Text(&'a str),
    /// An array.
    Array(Vec<Item<'a>>),
    /// A map, its pairs in the order written.
    Map(Vec<(Item<'a>, Item<'a>)>),
    /// `false` or `true`.
    Bool(bool),
    /// `null`.
    Null,
}

impl Item<'_> {
    /// The integer, when it is one that fits an `i64`.
    pub fn integer(&self) -> Option<i64> {
        match *self {
            Item::Unsigned(n) => i64::try_from(n).ok(),
            // -1 - n, for n up to i64::MAX.
            Item::Negative(n) => i64::try_from(n).ok().map(|n| -1 - n),
            _ => None,
        }
    }
}

/// Why an input is not one the decoder takes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// The input ends inside an item.
    Short,
    /// The input is longer than [`INPUT_MAX`].
    TooLong,
    /// The items nest deeper than [`DEPTH_MAX`].
    TooDeep,
    /// There are more than [`ITEMS_MAX`] items.
    TooMany,
    /// A length left open (additional information 31).
    OpenLength,
    /// A tag (major type 6).
    Tag,
    /// A float or a simple value other than `false`, `true` and `null`.
    Simple,
    /// Additional information 28 to 30, which RFC 8949 reserves.
    Reserved,
    /// A text string that is not UTF-8.
    NotText,
}

impl Error {
    /// In words, for the person whose passkey it was.
    pub fn words(self) -> &'static str {
        match self {
            Error::Short => "it ends in the middle of a value",
            Error::TooLong => "it is longer than 16 KiB",
            Error::TooDeep => "it nests more than 8 levels deep",
            Error::TooMany => "it holds more than 256 values",
            Error::OpenLength => "it has a value of open length",
            Error::Tag => "it has a tagged value",
            Error::Simple => "it has a float or another simple value",
            Error::Reserved => "it uses a length code CBOR reserves",
            Error::NotText => "it has text that is not UTF-8",
        }
    }
}

/// Decodes the item at the start of `input`, and says how many bytes it
/// took; what follows it is the caller's.
pub fn decode(input: &[u8]) -> Result<(Item<'_>, usize), Error> {
    if input.len() > INPUT_MAX {
        return Err(Error::TooLong);
    }
    let mut reader = Reader { input, at: 0, items: 0 };
    let item = reader.item(0)?;
    Ok((item, reader.at))
}

/// Decodes `input` as exactly one item, with nothing after it.
pub fn decode_whole(input: &[u8]) -> Result<Item<'_>, Error> {
    match decode(input)? {
        (item, used) if used == input.len() => Ok(item),
        // Something after the item is as wrong as a cut-off one.
        _ => Err(Error::Short),
    }
}

struct Reader<'a> {
    input: &'a [u8],
    at: usize,
    items: usize,
}

impl<'a> Reader<'a> {
    fn byte(&mut self) -> Result<u8, Error> {
        let byte = *self.input.get(self.at).ok_or(Error::Short)?;
        self.at += 1;
        Ok(byte)
    }

    fn take(&mut self, count: u64) -> Result<&'a [u8], Error> {
        let left = self.input.len() - self.at;
        let count = usize::try_from(count).map_err(|_| Error::Short)?;
        if count > left {
            return Err(Error::Short);
        }
        let taken = &self.input[self.at..self.at + count];
        self.at += count;
        Ok(taken)
    }

    /// The argument of an item's head: its value, or its length.
    fn argument(&mut self, info: u8) -> Result<u64, Error> {
        let width = match info {
            0..=23 => return Ok(u64::from(info)),
            24 => 1,
            25 => 2,
            26 => 4,
            27 => 8,
            28..=30 => return Err(Error::Reserved),
            _ => return Err(Error::OpenLength),
        };
        let bytes = self.take(width)?;
        Ok(bytes.iter().fold(0u64, |value, &byte| (value << 8) | u64::from(byte)))
    }

    /// Counts one more item against [`ITEMS_MAX`].
    fn count_one(&mut self) -> Result<(), Error> {
        if self.items == ITEMS_MAX {
            return Err(Error::TooMany);
        }
        self.items += 1;
        Ok(())
    }

    /// Whether `count` more items fit under [`ITEMS_MAX`], asked before
    /// any room is made for them; each counts itself as it is read.
    fn room_for(&self, count: u64) -> Result<usize, Error> {
        match usize::try_from(count) {
            Ok(count) if count <= ITEMS_MAX - self.items => Ok(count),
            _ => Err(Error::TooMany),
        }
    }

    /// The bytes not read yet.
    fn left(&self) -> u64 {
        (self.input.len() - self.at) as u64
    }

    fn item(&mut self, depth: usize) -> Result<Item<'a>, Error> {
        if depth >= DEPTH_MAX {
            return Err(Error::TooDeep);
        }
        self.count_one()?;
        let head = self.byte()?;
        let (major, info) = (head >> 5, head & 0x1f);
        if major == 7 {
            return match info {
                20 => Ok(Item::Bool(false)),
                21 => Ok(Item::Bool(true)),
                22 => Ok(Item::Null),
                28..=30 => Err(Error::Reserved),
                31 => Err(Error::OpenLength),
                _ => Err(Error::Simple),
            };
        }
        if major == 6 {
            return Err(Error::Tag);
        }
        let argument = self.argument(info)?;
        match major {
            0 => Ok(Item::Unsigned(argument)),
            1 => Ok(Item::Negative(argument)),
            2 => Ok(Item::Bytes(self.take(argument)?)),
            3 => {
                let bytes = self.take(argument)?;
                core::str::from_utf8(bytes).map(Item::Text).map_err(|_| Error::NotText)
            }
            4 => {
                // Each element is at least a byte, so a length past what
                // is left is short before it is too many.
                if argument > self.left() {
                    return Err(Error::Short);
                }
                let count = self.room_for(argument)?;
                let mut items = Vec::with_capacity(count);
                for _ in 0..count {
                    items.push(self.item(depth + 1)?);
                }
                Ok(Item::Array(items))
            }
            _ => {
                // A pair is at least two bytes, and two items.
                if argument > self.left() / 2 {
                    return Err(Error::Short);
                }
                let count = self.room_for(argument * 2)? / 2;
                let mut pairs = Vec::with_capacity(count);
                for _ in 0..count {
                    let key = self.item(depth + 1)?;
                    let value = self.item(depth + 1)?;
                    pairs.push((key, value));
                }
                Ok(Item::Map(pairs))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(text: &str) -> Vec<u8> {
        (0..text.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&text[i..i + 2], 16).unwrap())
            .collect()
    }

    fn whole(text: &str) -> Result<Item<'static>, Error> {
        let bytes: &'static [u8] = Box::leak(hex(text).into_boxed_slice());
        decode_whole(bytes)
    }

    #[test]
    fn integers_from_rfc_8949_appendix_a() {
        assert_eq!(whole("00"), Ok(Item::Unsigned(0)));
        assert_eq!(whole("17"), Ok(Item::Unsigned(23)));
        assert_eq!(whole("1818"), Ok(Item::Unsigned(24)));
        assert_eq!(whole("1903e8"), Ok(Item::Unsigned(1000)));
        assert_eq!(whole("1a000f4240"), Ok(Item::Unsigned(1_000_000)));
        assert_eq!(whole("1b000000e8d4a51000"), Ok(Item::Unsigned(1_000_000_000_000)));
        assert_eq!(whole("1bffffffffffffffff"), Ok(Item::Unsigned(u64::MAX)));
        assert_eq!(whole("20").unwrap().integer(), Some(-1));
        assert_eq!(whole("29").unwrap().integer(), Some(-10));
        assert_eq!(whole("3863").unwrap().integer(), Some(-100));
        assert_eq!(whole("3903e7").unwrap().integer(), Some(-1000));
        assert_eq!(whole("390100").unwrap().integer(), Some(-257));
        // -18446744073709551616 is a CBOR integer but not an i64.
        assert_eq!(whole("3bffffffffffffffff").unwrap().integer(), None);
        assert_eq!(whole("1bffffffffffffffff").unwrap().integer(), None);
    }

    #[test]
    fn strings_arrays_maps_and_simple_values_from_appendix_a() {
        assert_eq!(whole("40"), Ok(Item::Bytes(&[])));
        assert_eq!(whole("4401020304"), Ok(Item::Bytes(&[1, 2, 3, 4])));
        assert_eq!(whole("60"), Ok(Item::Text("")));
        assert_eq!(whole("6161"), Ok(Item::Text("a")));
        assert_eq!(whole("6449455446"), Ok(Item::Text("IETF")));
        assert_eq!(whole("62c3bc"), Ok(Item::Text("\u{fc}")));
        assert_eq!(whole("63e6b0b4"), Ok(Item::Text("\u{6c34}")));
        assert_eq!(whole("80"), Ok(Item::Array(vec![])));
        assert_eq!(
            whole("8301820203820405"),
            Ok(Item::Array(vec![
                Item::Unsigned(1),
                Item::Array(vec![Item::Unsigned(2), Item::Unsigned(3)]),
                Item::Array(vec![Item::Unsigned(4), Item::Unsigned(5)]),
            ]))
        );
        let long = whole("98190102030405060708090a0b0c0d0e0f101112131415161718181819").unwrap();
        assert_eq!(long, Item::Array((1..=25).map(Item::Unsigned).collect()));
        assert_eq!(whole("a0"), Ok(Item::Map(vec![])));
        assert_eq!(
            whole("a201020304"),
            Ok(Item::Map(vec![
                (Item::Unsigned(1), Item::Unsigned(2)),
                (Item::Unsigned(3), Item::Unsigned(4)),
            ]))
        );
        assert_eq!(
            whole("a26161016162820203"),
            Ok(Item::Map(vec![
                (Item::Text("a"), Item::Unsigned(1)),
                (Item::Text("b"), Item::Array(vec![Item::Unsigned(2), Item::Unsigned(3)])),
            ]))
        );
        assert_eq!(whole("f4"), Ok(Item::Bool(false)));
        assert_eq!(whole("f5"), Ok(Item::Bool(true)));
        assert_eq!(whole("f6"), Ok(Item::Null));
    }

    #[test]
    fn what_webauthn_never_writes_is_refused() {
        // undefined, simple(16), simple(255), half, single and double floats.
        for simple in ["f7", "f0", "f8ff", "f93c00", "fa47c35000", "fb3ff199999999999a"] {
            assert_eq!(whole(simple), Err(Error::Simple), "{simple}");
        }
        // Tags: a date, a bignum.
        assert_eq!(whole("c074323031332d30332d32315432303a30343a30305a"), Err(Error::Tag));
        assert_eq!(whole("c249010000000000000000"), Err(Error::Tag));
        // Open lengths: bytes, text, array, map, and the break code alone.
        for open in ["5f42010243030405ff", "7f657374726561646d696e67ff", "9fff", "bfff", "ff"] {
            assert_eq!(whole(open), Err(Error::OpenLength), "{open}");
        }
        for reserved in ["1c", "3d", "5e", "fc"] {
            assert_eq!(whole(reserved), Err(Error::Reserved), "{reserved}");
        }
        assert_eq!(whole("62c328"), Err(Error::NotText));
    }

    #[test]
    fn cut_short_or_followed_input_is_refused_at_every_byte() {
        let full = hex("a363666d74646e6f6e656761747453746d74a0686175746844617461420102");
        // Cut anywhere, the map is short.
        for end in 0..full.len() {
            assert_eq!(decode(&full[..end]).map(|_| ()), Err(Error::Short), "cut at {end}");
        }
        assert_eq!(decode_whole(&[0x01, 0x02]), Err(Error::Short));
        assert_eq!(decode(&[0x01, 0x02]), Ok((Item::Unsigned(1), 1)));
    }

    #[test]
    fn lengths_past_the_input_allocate_nothing() {
        // An array, a map and a byte string that claim 2^64 - 1 elements.
        assert_eq!(whole("9bffffffffffffffff"), Err(Error::Short));
        assert_eq!(whole("bbffffffffffffffff"), Err(Error::Short));
        assert_eq!(whole("5bffffffffffffffff"), Err(Error::Short));
        assert_eq!(whole("7bffffffffffffffff00"), Err(Error::Short));
    }

    #[test]
    fn depth_items_and_size_are_bounded() {
        let deep = |levels: usize| {
            let mut bytes = vec![0x81; levels];
            bytes.push(0x00);
            bytes
        };
        assert!(decode_whole(&deep(DEPTH_MAX - 1)).is_ok());
        assert_eq!(decode_whole(&deep(DEPTH_MAX)), Err(Error::TooDeep));
        let many = |count: usize| {
            let mut bytes = vec![0x99, (count >> 8) as u8, count as u8];
            bytes.extend(std::iter::repeat_n(0x00, count));
            bytes
        };
        // The array itself is one item.
        assert!(decode_whole(&many(ITEMS_MAX - 1)).is_ok());
        assert_eq!(decode_whole(&many(ITEMS_MAX)), Err(Error::TooMany));
        let mut big = vec![0x59, 0x40, 0x01];
        big.extend(std::iter::repeat_n(0u8, INPUT_MAX - 2));
        assert_eq!(decode(&big), Err(Error::TooLong));
    }
}
