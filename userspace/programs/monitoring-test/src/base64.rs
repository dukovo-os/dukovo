// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Base64 (RFC 4648), decoding only: what the mail server reads the
//! account from, in AUTH.

/// The bytes `text` stands for, or `None` when it is not base64.
pub fn decode(text: &str) -> Option<Vec<u8>> {
    let mut bytes = Vec::new();
    // `bits` holds the `held` bits of the digits not yet made into bytes.
    let (mut bits, mut held) = (0u32, 0u32);
    for digit in text.trim().trim_end_matches('=').bytes() {
        let value = match digit {
            b'A'..=b'Z' => digit - b'A',
            b'a'..=b'z' => digit - b'a' + 26,
            b'0'..=b'9' => digit - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => return None,
        };
        bits = (bits << 6) | u32::from(value);
        held += 6;
        if held >= 8 {
            held -= 8;
            bytes.push(u8::try_from((bits >> held) & 0xff).ok()?);
            bits &= (1 << held) - 1;
        }
    }
    Some(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn digits_become_the_bytes_they_stand_for() {
        assert_eq!(decode("AGFsaWNl").unwrap(), b"\0alice");
        assert_eq!(decode("YQ==").unwrap(), b"a");
        assert_eq!(decode("YWI=\r\n").unwrap(), b"ab");
        assert_eq!(decode("").unwrap(), b"");
    }

    #[test]
    fn what_is_not_base64_is_refused() {
        assert_eq!(decode("Y Q"), None);
        assert_eq!(decode("a*b"), None);
    }
}
