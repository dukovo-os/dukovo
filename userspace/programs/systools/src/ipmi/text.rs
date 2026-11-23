// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Strings in the type/length encoding that SDR records (IPMI 2.0 section
//! 43.15) and FRU areas (Platform Management FRU Information Storage
//! Definition 1.0, section 13) share: a byte whose top two bits say how
//! the bytes after it are written and whose low bits count them.

/// How the bytes of a string are written, from the top two bits of its
/// type/length byte.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Kind {
    /// Bytes with no text in them (type 00 in a FRU; Unicode in an SDR,
    /// which a BMC does not use for names).
    Binary,
    /// Two digits, and the space, dash and period, in a byte.
    BcdPlus,
    /// Three bytes hold four characters, 0x20 above their 6-bit value.
    Packed,
    /// One byte for each character, ASCII or Latin-1.
    Latin1,
}

impl Kind {
    /// The kind a type/length byte names.
    pub(super) fn of(type_length: u8) -> Kind {
        match type_length >> 6 {
            0 => Kind::Binary,
            1 => Kind::BcdPlus,
            2 => Kind::Packed,
            _ => Kind::Latin1,
        }
    }
}

/// The text of `bytes` written as `kind`.
pub(super) fn decode(kind: Kind, bytes: &[u8]) -> String {
    match kind {
        Kind::Binary => {
            let mut text = String::new();
            for byte in bytes {
                text.push_str(&format!("{byte:02X}"));
            }
            text
        }
        Kind::BcdPlus => bytes.iter().flat_map(|byte| [byte >> 4, byte & 0x0F]).map(bcd).collect(),
        Kind::Packed => packed(bytes),
        Kind::Latin1 => {
            let text: String = bytes.iter().map(|&byte| char::from(byte)).collect();
            text.trim_end_matches(['\0', ' ']).to_string()
        }
    }
}

/// The character of one BCD plus digit: 0 to 9, then space, dash and
/// period; the three left are reserved.
fn bcd(digit: u8) -> char {
    match digit {
        0..=9 => char::from(b'0' + digit),
        0xA => ' ',
        0xB => '-',
        0xC => '.',
        _ => '?',
    }
}

/// 6-bit ASCII packed: the first character is in bits 5:0 of the first
/// byte, the second in bits 7:6 of it and 3:0 of the next, and so on, so
/// that `n` bytes hold `n * 8 / 6` characters.
fn packed(bytes: &[u8]) -> String {
    let mut text = String::new();
    let mut bits = 0u32;
    let mut count = 0;
    for &byte in bytes {
        bits |= u32::from(byte) << count;
        count += 8;
        while count >= 6 {
            let value = u8::try_from(bits & 0x3F).unwrap_or(0);
            text.push(char::from(0x20 + value));
            bits >>= 6;
            count -= 6;
        }
    }
    text.trim_end().to_string()
}

/// The string at `at` in `area`, from its type/length byte, with the
/// offset after it. `None` when the byte is the end-of-fields marker 0xC1
/// or the string runs past the area. A length of 0 gives an empty string.
pub(super) fn field(area: &[u8], at: usize) -> Option<(String, usize)> {
    let type_length = *area.get(at)?;
    if type_length == 0xC1 {
        return None;
    }
    let length = usize::from(type_length & 0x3F);
    let bytes = area.get(at + 1..at + 1 + length)?;
    Some((decode(Kind::of(type_length), bytes), at + 1 + length))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_latin_one_string_is_its_bytes() {
        assert_eq!(decode(Kind::Latin1, b"Dell Inc. "), "Dell Inc.");
        assert_eq!(decode(Kind::Latin1, &[0x4D, 0xE9]), "M\u{e9}");
    }

    #[test]
    fn bcd_plus_holds_digits_space_dash_and_period() {
        assert_eq!(decode(Kind::BcdPlus, &[0x12, 0x3B, 0x4C, 0x5A]), "123-4.5 ");
        assert_eq!(decode(Kind::BcdPlus, &[0xD0]), "?0");
    }

    #[test]
    fn six_bit_ascii_packs_four_characters_in_three_bytes() {
        // "IPMI": I = 0x49 - 0x20 = 0x29, P = 0x30, M = 0x2D, I = 0x29.
        let value = 0x29 | (0x30 << 6) | (0x2D << 12) | (0x29 << 18);
        let bytes = [value as u8, (value >> 8) as u8, (value >> 16) as u8];
        assert_eq!(decode(Kind::Packed, &bytes), "IPMI");
        // Six bytes hold eight characters, and a space ends nothing.
        assert_eq!(decode(Kind::Packed, &[0, 0, 0]), "");
    }

    #[test]
    fn binary_is_shown_as_hex() {
        assert_eq!(decode(Kind::Binary, &[0x0A, 0xFF]), "0AFF");
    }

    #[test]
    fn a_field_is_read_by_its_type_length_byte() {
        let area = [0xC3, b'a', b'b', b'c', 0xC0, 0x41, 0x12, 0xC1];
        assert_eq!(field(&area, 0), Some(("abc".into(), 4)));
        assert_eq!(field(&area, 4), Some((String::new(), 5)));
        assert_eq!(field(&area, 5), Some(("12".into(), 7)));
        assert_eq!(field(&area, 7), None);
        assert_eq!(field(&[0xC5, b'a'], 0), None);
    }
}
