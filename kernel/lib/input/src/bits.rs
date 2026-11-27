// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! A bit for each code of a kind: the keys a device has, the keys that
//! are down.
//!
//! radix-input(2)'s INPUT_BITS and INPUT_STATE answer with such a set as
//! bytes: a code's eighth is the byte and the rest the bit in it, as
//! Linux's event devices answer.

/// A set of the codes below `N` times 64.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Bits<const N: usize>([u64; N]);

impl<const N: usize> Default for Bits<N> {
    fn default() -> Bits<N> {
        Bits::new()
    }
}

impl<const N: usize> Bits<N> {
    /// The set of no code.
    pub const fn new() -> Bits<N> {
        Bits([0; N])
    }

    /// The set of `codes`; those it has no room for are left out.
    pub fn of(codes: &[u16]) -> Bits<N> {
        let mut bits = Bits::new();
        for &code in codes {
            bits.set(code, true);
        }
        bits
    }

    /// Whether `code` is in the set.
    pub fn has(&self, code: u16) -> bool {
        let code = usize::from(code);
        self.0.get(code / 64).is_some_and(|word| word >> (code % 64) & 1 == 1)
    }

    /// Puts `code` in the set, or takes it out. Returns whether that
    /// changed the set; a code the set has no room for changes nothing.
    pub fn set(&mut self, code: u16, on: bool) -> bool {
        let code = usize::from(code);
        let Some(word) = self.0.get_mut(code / 64) else { return false };
        let bit = 1u64 << (code % 64);
        let was = *word & bit != 0;
        if on {
            *word |= bit;
        } else {
            *word &= !bit;
        }
        was != on
    }

    /// Puts every code of `other` in the set.
    pub fn merge(&mut self, other: &Bits<N>) {
        for (word, more) in self.0.iter_mut().zip(other.0) {
            *word |= more;
        }
    }

    /// Whether no code is in the set.
    pub fn is_empty(&self) -> bool {
        self.0.iter().all(|&word| word == 0)
    }

    /// How many codes are in the set.
    pub fn count(&self) -> u32 {
        self.0.iter().map(|word| word.count_ones()).sum()
    }

    /// The codes in the set, lowest first.
    pub fn codes(&self) -> impl Iterator<Item = u16> + '_ {
        self.0.iter().enumerate().flat_map(|(index, &word)| {
            (0..64)
                .filter(move |bit| word >> bit & 1 == 1)
                .map(move |bit| (index * 64 + bit) as u16)
        })
    }

    /// Writes the set's first `codes` codes as bytes into `out`, as many
    /// as fit, and returns how many bytes they all take.
    pub fn write(&self, codes: u32, out: &mut [u8]) -> usize {
        let bytes = (codes as usize).div_ceil(8).min(N * 8);
        for (index, byte) in out.iter_mut().enumerate().take(bytes) {
            *byte = self.0[index / 8].to_le_bytes()[index % 8];
        }
        bytes
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_go_in_come_out_and_are_listed_in_order() {
        let mut bits: Bits<2> = Bits::new();
        assert!(bits.is_empty());
        assert!(bits.set(3, true));
        assert!(bits.set(64, true));
        assert!(bits.set(127, true));
        // Already in: nothing changed.
        assert!(!bits.set(3, true));
        // No room for it: nothing changed, and it is not in.
        assert!(!bits.set(128, true));
        assert!(!bits.has(128));
        assert!(bits.has(3) && bits.has(64) && bits.has(127) && !bits.has(4));
        assert_eq!(bits.count(), 3);
        assert_eq!(bits.codes().collect::<Vec<_>>(), [3, 64, 127]);
        assert!(bits.set(64, false));
        assert!(!bits.set(64, false));
        assert_eq!(Bits::<2>::of(&[3, 127, 500]), bits);
        bits.merge(&Bits::of(&[3, 5]));
        assert_eq!(bits.codes().collect::<Vec<_>>(), [3, 5, 127]);
    }

    #[test]
    fn the_bytes_have_a_code_s_eighth_as_the_byte_and_the_rest_as_the_bit() {
        let bits: Bits<2> = Bits::of(&[0, 9, 16, 71]);
        let mut out = [0xff; 16];
        // Twenty codes take three bytes; the rest of `out` is not touched.
        assert_eq!(bits.write(20, &mut out), 3);
        assert_eq!(out[..4], [0b0000_0001, 0b0000_0010, 0b0000_0001, 0xff]);
        assert_eq!(bits.write(128, &mut out), 16);
        assert_eq!(out[8], 0b1000_0000);
        // A short buffer takes what fits, and is told the whole length.
        let mut short = [0; 1];
        assert_eq!(bits.write(128, &mut short), 16);
        assert_eq!(short, [1]);
        // No more bytes than the set has.
        assert_eq!(bits.write(4096, &mut out), 16);
    }
}
