// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! A secret an action takes (ADR 0561): a password or a key that its
//! command reads on its standard input, as `passwd` reads a new password
//! from a pipe.
//!
//! A secret is never one of the command's words, so it is never in the
//! journal, the audit, a task's row or the Show command. It is never kept:
//! [`Secret`] prints and serializes as `(secret)`, wherever it would be
//! shown or stored, and its bytes are overwritten with zeros when it goes.
//! The copies that the HTTP server's buffers and the JSON reader made
//! before the action had it, and those of the link that carries a
//! person's command to their worker, are freed as they are: admind's and
//! the worker's own memory alone held them.

use std::fmt;

use serde::de::{self, Deserializer, Visitor};
use serde::{Deserialize, Serialize, Serializer};

/// What a secret shows as, wherever it would be printed or kept: the
/// arguments of a task, the examples of `GET /api/1/actions`, a log line.
pub const SHOWN: &str = "(secret)";

/// The most bytes an action's command is given on its input: what a pipe
/// holds before its reader has read any (POSIX's `PIPE_BUF`), so that
/// writing them never waits for the command.
pub const INPUT_MAX: usize = 4096;

/// The most bytes a secret of up to `chars` characters takes as a line:
/// four for each character, and its end.
// The registry's rules hold each action's secrets to [`INPUT_MAX`] by it.
#[cfg_attr(not(test), allow(dead_code))]
pub const fn line_bytes(chars: usize) -> usize {
    chars * 4 + 1
}

/// A password or a key: text of one line, which shows as `(secret)` and
/// is overwritten with zeros when it goes.
pub struct Secret(Vec<u8>);

impl Secret {
    /// The lines that `secrets` are, one after another, each ended by
    /// `\n`: what a command that reads them from a pipe is given.
    pub fn lines(secrets: &[&Secret]) -> Secret {
        // The room is taken once, so that no copy is left behind by a
        // growing buffer.
        let mut bytes = Vec::with_capacity(secrets.iter().map(|secret| secret.0.len() + 1).sum());
        for secret in secrets {
            bytes.extend_from_slice(&secret.0);
            bytes.push(b'\n');
        }
        Secret(bytes)
    }

    /// Its bytes, for the pipe its command reads.
    pub fn bytes(&self) -> &[u8] {
        &self.0
    }
}

impl From<String> for Secret {
    fn from(text: String) -> Secret {
        Secret(text.into_bytes())
    }
}

impl Drop for Secret {
    fn drop(&mut self) {
        accounts::wipe(&mut self.0);
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(SHOWN)
    }
}

impl fmt::Display for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(SHOWN)
    }
}

impl Serialize for Secret {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(SHOWN)
    }
}

impl<'de> Deserialize<'de> for Secret {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Secret, D::Error> {
        deserializer.deserialize_string(Text)
    }
}

/// Reads a secret from text. Text that is owned already, as a JSON value's
/// is, is taken as it is, with no copy.
struct Text;

impl Visitor<'_> for Text {
    type Value = Secret;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("text")
    }

    fn visit_str<E: de::Error>(self, text: &str) -> Result<Secret, E> {
        Ok(Secret(text.as_bytes().to_vec()))
    }

    fn visit_string<E: de::Error>(self, text: String) -> Result<Secret, E> {
        Ok(Secret::from(text))
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    const MARKER: &str = "marker-5f1e-a secret";

    #[test]
    fn a_secret_shows_and_serializes_as_secret_and_never_as_itself() {
        let secret = Secret::from(MARKER.to_string());
        assert_eq!(format!("{secret}"), SHOWN);
        assert_eq!(format!("{secret:?}"), SHOWN);
        assert_eq!(format!("{secret:#?}"), SHOWN);
        assert_eq!(serde_json::to_value(&secret).unwrap(), json!(SHOWN));
        assert_eq!(secret.bytes(), MARKER.as_bytes());
    }

    #[test]
    fn a_secret_is_read_from_json_text_and_from_nothing_else() {
        let owned: Secret = serde_json::from_value(json!(MARKER)).unwrap();
        assert_eq!(owned.bytes(), MARKER.as_bytes());
        let borrowed: Secret = serde_json::from_str(&json!(MARKER).to_string()).unwrap();
        assert_eq!(borrowed.bytes(), MARKER.as_bytes());
        for other in [json!(7), json!(null), json!(true), json!(["a"]), json!({})] {
            let refused = serde_json::from_value::<Secret>(other.clone()).unwrap_err();
            assert!(refused.to_string().contains("expected text"), "{other}: {refused}");
        }
    }

    #[test]
    fn secrets_as_lines_end_each_with_a_line_end_in_their_order() {
        let (current, new) =
            (Secret::from("old pass".to_string()), Secret::from("néw".to_string()));
        assert_eq!(Secret::lines(&[&current, &new]).bytes(), "old pass\nnéw\n".as_bytes());
        assert_eq!(Secret::lines(&[&new]).bytes(), "néw\n".as_bytes());
        assert_eq!(Secret::lines(&[]).bytes(), b"");
        assert_eq!(format!("{:?}", Secret::lines(&[&current])), SHOWN);
    }

    #[test]
    fn a_line_of_a_secret_takes_at_most_four_bytes_a_character_and_its_end() {
        assert_eq!(line_bytes(1), 5);
        assert_eq!(line_bytes(1023), INPUT_MAX - 3);
        let widest = "\u{10ffff}".repeat(10);
        assert_eq!(widest.len() + 1, line_bytes(10));
    }
}
