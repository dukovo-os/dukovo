// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! XDR (RFC 4506), the encoding of ONC RPC and of everything NFS says:
//! big-endian 32-bit units, opaque data and strings padded to four bytes.

/// What is wrong with what came: too short, or a length past what the
/// protocol allows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Bad;

/// A result of decoding.
pub type Result<T> = core::result::Result<T, Bad>;

/// Reads XDR from a buffer.
pub struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    /// A reader of `bytes`, from their start.
    pub fn new(bytes: &'a [u8]) -> Reader<'a> {
        Reader { bytes, at: 0 }
    }

    /// The next `n` bytes, unpadded.
    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        let end = self.at.checked_add(n).ok_or(Bad)?;
        let out = self.bytes.get(self.at..end).ok_or(Bad)?;
        self.at = end;
        Ok(out)
    }

    /// An unsigned 32-bit integer.
    pub fn u32(&mut self) -> Result<u32> {
        let b = self.take(4)?;
        Ok(u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
    }

    /// A signed 32-bit integer.
    pub fn i32(&mut self) -> Result<i32> {
        Ok(self.u32()?.cast_signed())
    }

    /// An unsigned 64-bit integer.
    pub fn u64(&mut self) -> Result<u64> {
        Ok((u64::from(self.u32()?) << 32) | u64::from(self.u32()?))
    }

    /// A signed 64-bit integer.
    pub fn i64(&mut self) -> Result<i64> {
        Ok(self.u64()?.cast_signed())
    }

    /// A boolean: 0 or 1, nothing else.
    pub fn bool(&mut self) -> Result<bool> {
        match self.u32()? {
            0 => Ok(false),
            1 => Ok(true),
            _ => Err(Bad),
        }
    }

    /// Fixed-length opaque data of `n` bytes, and its padding.
    pub fn fixed(&mut self, n: usize) -> Result<&'a [u8]> {
        let out = self.take(n)?;
        self.take(pad(n))?;
        Ok(out)
    }

    /// Fixed-length opaque data into an array.
    pub fn array<const N: usize>(&mut self) -> Result<[u8; N]> {
        let mut out = [0; N];
        out.copy_from_slice(self.fixed(N)?);
        Ok(out)
    }

    /// Variable-length opaque data of at most `max` bytes.
    pub fn opaque(&mut self, max: usize) -> Result<&'a [u8]> {
        let n = self.u32()? as usize;
        if n > max {
            return Err(Bad);
        }
        self.fixed(n)
    }

    /// The count of a variable-length array, of at most `max` items, each
    /// of which takes at least four bytes: a count past what is left is
    /// refused before anything is made for it.
    pub fn count(&mut self, max: usize) -> Result<usize> {
        let n = self.u32()? as usize;
        if n > max || n > self.left() / 4 {
            return Err(Bad);
        }
        Ok(n)
    }

    /// A bitmap4: its words, at most eight.
    pub fn bitmap(&mut self) -> Result<Vec<u32>> {
        let n = self.count(8)?;
        (0..n).map(|_| self.u32()).collect()
    }

    /// The bytes not read yet.
    pub fn left(&self) -> usize {
        self.bytes.len() - self.at
    }

    /// Where it is, in bytes from the start.
    pub fn position(&self) -> usize {
        self.at
    }
}

/// The padding after `n` bytes of opaque data.
pub fn pad(n: usize) -> usize {
    (4 - n % 4) % 4
}

/// Writes XDR into a buffer.
#[derive(Default)]
pub struct Writer {
    /// What it wrote.
    pub bytes: Vec<u8>,
}

impl Writer {
    /// An empty writer.
    pub fn new() -> Writer {
        Writer::default()
    }

    /// An unsigned 32-bit integer.
    pub fn u32(&mut self, value: u32) -> &mut Writer {
        self.bytes.extend_from_slice(&value.to_be_bytes());
        self
    }

    /// A signed 32-bit integer.
    pub fn i32(&mut self, value: i32) -> &mut Writer {
        self.u32(value.cast_unsigned())
    }

    /// An unsigned 64-bit integer.
    pub fn u64(&mut self, value: u64) -> &mut Writer {
        self.bytes.extend_from_slice(&value.to_be_bytes());
        self
    }

    /// A signed 64-bit integer.
    pub fn i64(&mut self, value: i64) -> &mut Writer {
        self.u64(value.cast_unsigned())
    }

    /// A boolean.
    pub fn bool(&mut self, value: bool) -> &mut Writer {
        self.u32(u32::from(value))
    }

    /// Fixed-length opaque data, and its padding.
    pub fn fixed(&mut self, data: &[u8]) -> &mut Writer {
        self.bytes.extend_from_slice(data);
        self.bytes.resize(self.bytes.len() + pad(data.len()), 0);
        self
    }

    /// Variable-length opaque data, or a string.
    pub fn opaque(&mut self, data: &[u8]) -> &mut Writer {
        self.u32(data.len() as u32);
        self.fixed(data)
    }

    /// A bitmap4, without the zero words at its end.
    pub fn bitmap(&mut self, words: &[u32]) -> &mut Writer {
        let n = words.iter().rposition(|&w| w != 0).map_or(0, |last| last + 1);
        self.u32(n as u32);
        for &word in &words[..n] {
            self.u32(word);
        }
        self
    }

    /// How many bytes it wrote.
    pub fn len(&self) -> usize {
        self.bytes.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_go_both_ways() {
        let mut w = Writer::new();
        w.u32(7).u64(1 << 40).i64(-2).bool(true).opaque(b"abcde").fixed(b"xy").bitmap(&[1, 0, 0]);
        let mut r = Reader::new(&w.bytes);
        assert_eq!(r.u32(), Ok(7));
        assert_eq!(r.u64(), Ok(1 << 40));
        assert_eq!(r.i64(), Ok(-2));
        assert_eq!(r.bool(), Ok(true));
        assert_eq!(r.opaque(8), Ok(&b"abcde"[..]));
        assert_eq!(r.fixed(2), Ok(&b"xy"[..]));
        assert_eq!(r.bitmap(), Ok(vec![1]));
        assert_eq!(r.left(), 0);
    }

    #[test]
    fn padding_is_to_four_bytes() {
        let mut w = Writer::new();
        w.opaque(b"abcde");
        assert_eq!(w.bytes, [0, 0, 0, 5, b'a', b'b', b'c', b'd', b'e', 0, 0, 0]);
    }

    #[test]
    fn too_long_and_too_short_are_refused() {
        let mut w = Writer::new();
        w.opaque(&[1; 10]);
        assert_eq!(Reader::new(&w.bytes).opaque(9), Err(Bad));
        assert_eq!(Reader::new(&w.bytes[..8]).opaque(10), Err(Bad));
        assert_eq!(Reader::new(&[0, 0, 0, 2]).bool(), Err(Bad));
        // A count past what is left is refused before anything is made.
        assert_eq!(Reader::new(&[0, 0, 0, 200, 0, 0, 0, 1]).count(1000), Err(Bad));
    }
}
