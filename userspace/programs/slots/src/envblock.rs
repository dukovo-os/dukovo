// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! GRUB's environment blocks, which its `load_env` reads and `save_env`
//! writes: a first line, [`SIGNATURE`], then a line `name=value` for each
//! variable, then `#` to the end, 1024 bytes in all as `grub-editenv`
//! makes them. A line that starts with `#` is a comment. A value keeps a
//! backslash or a newline behind a backslash.
//!
//! GRUB writes a block in place, over the blocks of its file, so it never
//! grows: a block it cannot fit a value in it leaves as it was. The slot
//! state ([`crate::state`]) and each slot's `startup/slot.env`
//! ([`crate::about`]) are blocks of this kind, read here as GRUB reads
//! them.

use alloc::string::String;
use alloc::vec::Vec;

/// The first line of every block.
pub const SIGNATURE: &[u8] = b"# GRUB Environment Block\n";

/// How long a block is: GRUB's default, and its own `grubenv`'s.
pub const BLOCK: usize = 1024;

/// The variables of the block `bytes`, in order, as GRUB's `load_env`
/// sets them: a later line of a name takes the place of an earlier one
/// when GRUB reads it, so [`get`] takes the last. `None` if `bytes` does
/// not start with [`SIGNATURE`], which GRUB refuses.
#[must_use]
pub fn parse(bytes: &[u8]) -> Option<Vec<(String, String)>> {
    let rest = bytes.strip_prefix(SIGNATURE)?;
    let mut variables = Vec::new();
    for line in rest.split(|&b| b == b'\n') {
        // GRUB's iteration stops at a line it cannot read, as the padding
        // at the end is one.
        if line.first() == Some(&b'#') || line.is_empty() {
            continue;
        }
        let Some(equals) = line.iter().position(|&b| b == b'=') else { continue };
        let (name, value) = (&line[..equals], &line[equals + 1..]);
        let (Ok(name), Some(value)) = (core::str::from_utf8(name), unescape(value)) else {
            continue;
        };
        variables.push((String::from(name), value));
    }
    Some(variables)
}

/// The value of the variable `name` among `variables`: the last line of
/// it, as GRUB sets them in order.
#[must_use]
pub fn get<'a>(variables: &'a [(String, String)], name: &str) -> Option<&'a str> {
    variables.iter().rev().find(|(n, _)| n == name).map(|(_, value)| value.as_str())
}

/// A value read from a block: each byte after a backslash taken as it is.
fn unescape(bytes: &[u8]) -> Option<String> {
    let mut out = Vec::with_capacity(bytes.len());
    let mut escaped = false;
    for &byte in bytes {
        if escaped || byte != b'\\' {
            out.push(byte);
            escaped = false;
        } else {
            escaped = true;
        }
    }
    String::from_utf8(out).ok()
}

/// A block of `BLOCK` bytes holding `comments`, each a line of its own
/// after [`SIGNATURE`] and each starting with `#`, and `variables`, in
/// order. `None` if they do not fit, or a name is not one GRUB sets.
#[must_use]
pub fn write(comments: &[&str], variables: &[(&str, &str)]) -> Option<[u8; BLOCK]> {
    let mut text = Vec::with_capacity(BLOCK);
    text.extend_from_slice(SIGNATURE);
    for comment in comments {
        if !comment.starts_with('#') || comment.contains('\n') {
            return None;
        }
        text.extend_from_slice(comment.as_bytes());
        text.push(b'\n');
    }
    for (name, value) in variables {
        let named = !name.is_empty()
            && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
            && !name.as_bytes()[0].is_ascii_digit();
        if !named {
            return None;
        }
        text.extend_from_slice(name.as_bytes());
        text.push(b'=');
        for byte in value.bytes() {
            if byte == b'\\' || byte == b'\n' {
                text.push(b'\\');
            }
            text.push(byte);
        }
        text.push(b'\n');
    }
    // GRUB finds its free space as the `#`s at the end after a newline,
    // so at least one is kept.
    if text.len() >= BLOCK {
        return None;
    }
    let mut block = [b'#'; BLOCK];
    block[..text.len()].copy_from_slice(&text);
    Some(block)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_what_it_writes_and_what_grub_editenv_writes() {
        let block = write(&["# a comment"], &[("a", "1"), ("b", ""), ("c", "x\\y")]).unwrap();
        assert_eq!(block.len(), BLOCK);
        assert!(block.starts_with(b"# GRUB Environment Block\n# a comment\na=1\nb=\nc=x\\\\y\n#"));
        let read = parse(&block).unwrap();
        assert_eq!(get(&read, "a"), Some("1"));
        assert_eq!(get(&read, "b"), Some(""));
        assert_eq!(get(&read, "c"), Some("x\\y"));
        assert_eq!(get(&read, "d"), None);
        // grub-editenv's empty block, and one with saved_entry set.
        let mut empty = SIGNATURE.to_vec();
        empty.resize(BLOCK, b'#');
        assert_eq!(parse(&empty).unwrap(), Vec::new());
        let mut saved = b"# GRUB Environment Block\nsaved_entry=dukovo\n".to_vec();
        saved.resize(BLOCK, b'#');
        assert_eq!(get(&parse(&saved).unwrap(), "saved_entry"), Some("dukovo"));
    }

    #[test]
    fn takes_the_last_of_a_name_and_refuses_a_block_without_its_first_line() {
        let mut twice = b"# GRUB Environment Block\nx=1\nx=2\n".to_vec();
        twice.resize(BLOCK, b'#');
        assert_eq!(get(&parse(&twice).unwrap(), "x"), Some("2"));
        assert!(parse(b"# GRUB Environment Bloc\nx=1\n").is_none());
        assert!(parse(&[0u8; BLOCK]).is_none());
    }

    #[test]
    fn refuses_what_grub_could_not_read_back() {
        assert!(write(&["not a comment"], &[]).is_none());
        assert!(write(&[], &[("1st", "x")]).is_none());
        assert!(write(&[], &[("a-b", "x")]).is_none());
        let long = "x".repeat(BLOCK);
        assert!(write(&[], &[("a", long.as_str())]).is_none());
        // The last byte is always one of the `#`s GRUB finds space by.
        let just = "x".repeat(BLOCK - SIGNATURE.len() - 4);
        let block = write(&[], &[("a", just.as_str())]).unwrap();
        assert_eq!(block[BLOCK - 1], b'#');
        assert!(write(&[], &[("a", &"x".repeat(BLOCK - SIGNATURE.len() - 3))]).is_none());
    }
}
