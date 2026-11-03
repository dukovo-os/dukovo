// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Canonical JSON, the bytes a TUF signature is over: the OLPC form that
//! securesystemslib writes, which the TUF specification names. Objects'
//! keys are sorted by code point, there is no space between tokens,
//! strings escape only `"` and `\`, and numbers are integers.

use serde_json::Value;

/// Why a value has no canonical form.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotCanonical;

impl std::fmt::Display for NotCanonical {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "a number that is not an integer has no canonical JSON")
    }
}

impl std::error::Error for NotCanonical {}

/// The canonical JSON of `value`.
///
/// # Errors
///
/// [`NotCanonical`] for a number that is not an integer.
pub fn canonical(value: &Value) -> Result<Vec<u8>, NotCanonical> {
    let mut out = Vec::new();
    write(value, &mut out)?;
    Ok(out)
}

fn write(value: &Value, out: &mut Vec<u8>) -> Result<(), NotCanonical> {
    match value {
        Value::Null => out.extend_from_slice(b"null"),
        Value::Bool(true) => out.extend_from_slice(b"true"),
        Value::Bool(false) => out.extend_from_slice(b"false"),
        Value::Number(number) => {
            if let Some(n) = number.as_i64() {
                out.extend_from_slice(n.to_string().as_bytes());
            } else if let Some(n) = number.as_u64() {
                out.extend_from_slice(n.to_string().as_bytes());
            } else {
                return Err(NotCanonical);
            }
        }
        Value::String(text) => string(text, out),
        Value::Array(items) => {
            out.push(b'[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(b',');
                }
                write(item, out)?;
            }
            out.push(b']');
        }
        Value::Object(map) => {
            // UTF-8's byte order is code point order.
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort_unstable();
            out.push(b'{');
            for (i, key) in keys.into_iter().enumerate() {
                if i > 0 {
                    out.push(b',');
                }
                string(key, out);
                out.push(b':');
                write(&map[key], out)?;
            }
            out.push(b'}');
        }
    }
    Ok(())
}

fn string(text: &str, out: &mut Vec<u8>) {
    out.push(b'"');
    for byte in text.bytes() {
        if byte == b'"' || byte == b'\\' {
            out.push(b'\\');
        }
        out.push(byte);
    }
    out.push(b'"');
}

#[cfg(test)]
mod tests {
    use super::*;

    fn canon(text: &str) -> String {
        String::from_utf8(canonical(&serde_json::from_str(text).unwrap()).unwrap()).unwrap()
    }

    #[test]
    fn keys_sort_and_space_goes() {
        assert_eq!(
            canon(r#"{ "b": [1, -2, true, null], "a": {"z": "x", "é": "", "Z": false} }"#),
            r#"{"a":{"Z":false,"z":"x","é":""},"b":[1,-2,true,null]}"#
        );
    }

    #[test]
    fn strings_escape_only_quotes_and_backslashes() {
        // The JSON escapes come back as the characters they stand for.
        assert_eq!(canon(r#""a\"b\\c\ndé""#), "\"a\\\"b\\\\c\nd\u{e9}\"");
        assert_eq!(canon("18446744073709551615"), "18446744073709551615");
        assert!(canonical(&serde_json::from_str("1.5").unwrap()).is_err());
    }
}
