// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Reading the kernel's text files and writing Linux's: the `key: value`
//! lines of `/processes` (radix-proc(5)), the octal escapes it writes for
//! a newline or a backslash, and a page of lines built one at a time.
//!
//! Every file the filesystem makes goes through [`Page`], so a wrong
//! space or tab in a golden test of a generator is fixed here or in the
//! generator's format string, never in the reader's program.

use std::fmt::{self, Write};

/// The value of the line `key: value` of `text`, a file of such lines.
/// The key is matched whole, so `user` finds `user: 0` and not
/// `user_ns: 5`.
pub fn value<'a>(text: &'a [u8], key: &str) -> Option<&'a [u8]> {
    text.split(|&byte| byte == b'\n')
        .find_map(|line| line.strip_prefix(key.as_bytes())?.strip_prefix(b": "))
}

/// The number that starts the value of `key`: 1024 for `total: 1024 KiB`.
pub fn number(text: &[u8], key: &str) -> Option<u64> {
    let value = value(text, key)?;
    let digits = value.split(|&byte| byte == b' ').next()?;
    std::str::from_utf8(digits).ok()?.parse().ok()
}

/// The value of `key` as text, where it is text.
pub fn word<'a>(text: &'a [u8], key: &str) -> Option<&'a str> {
    std::str::from_utf8(value(text, key)?).ok()
}

/// `bytes` with each backslash and three octal digits (`\040`), which
/// `/processes` writes for a newline or a backslash in a name or path, as
/// the byte it stands for.
pub fn unescape(bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes.len());
    let mut rest = bytes;
    while let Some((&byte, after)) = rest.split_first() {
        rest = after;
        if byte == b'\\'
            && let Some(digits) = rest.get(..3)
            && digits.iter().all(|digit| (b'0'..=b'7').contains(digit))
        {
            let code = digits.iter().fold(0u32, |sum, digit| sum * 8 + u32::from(digit - b'0'));
            // Three octal digits reach 511: only a value that is a byte
            // is one the kernel wrote.
            if let Ok(byte) = u8::try_from(code) {
                out.push(byte);
                rest = &rest[3..];
                continue;
            }
        }
        out.push(byte);
    }
    out
}

/// The fields of a line of `key=value` pairs separated by spaces, whose
/// last pair, `last_key`, takes the rest of the line, spaces and all: a
/// thread's `name` and a descriptor's `path`. The pairs before it, and
/// the rest of the line unescaped.
pub fn pairs<'a>(line: &'a [u8], last_key: &str) -> (Vec<(&'a str, &'a str)>, Option<Vec<u8>>) {
    let marker = format!(" {last_key}=");
    let at = line.windows(marker.len()).position(|window| window == marker.as_bytes());
    let (head, tail) = match at {
        Some(at) => (&line[..at], Some(unescape(&line[at + marker.len()..]))),
        None => (line, None),
    };
    let Ok(head) = std::str::from_utf8(head) else { return (Vec::new(), tail) };
    let fields = head.split(' ').filter_map(|field| field.split_once('=')).collect();
    (fields, tail)
}

/// A decimal number as a name has it: digits only, no sign, no leading
/// zero, and one that fits a `u64`. `None` for anything else, which is
/// not the name of a process, a thread, a descriptor, a CPU or a socket.
pub fn decimal(name: &str) -> Option<u64> {
    let digits = name.as_bytes();
    // Nineteen digits always fit.
    if digits.is_empty() || digits.len() > 19 || !digits.iter().all(u8::is_ascii_digit) {
        return None;
    }
    if digits.len() > 1 && digits[0] == b'0' {
        return None;
    }
    name.parse().ok()
}

/// A text made a line at a time.
#[derive(Default)]
pub struct Page(String);

impl Page {
    /// An empty page.
    pub fn new() -> Page {
        Page::default()
    }

    /// Adds `text` and a newline.
    pub fn line(&mut self, text: fmt::Arguments<'_>) {
        self.push(text);
        self.0.push('\n');
    }

    /// Adds `text`.
    pub fn push(&mut self, text: fmt::Arguments<'_>) {
        // A String grows as it is written to, so formatting cannot fail.
        let _ = self.0.write_fmt(text);
    }

    /// The text.
    pub fn finish(self) -> String {
        self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_key_is_matched_whole() {
        let text = b"user_ns: 5\nuser: 7\nname: a b\n";
        assert_eq!(value(text, "user"), Some(&b"7"[..]));
        assert_eq!(number(text, "user_ns"), Some(5));
        assert_eq!(word(text, "name"), Some("a b"));
        assert_eq!(value(text, "use"), None);
    }

    #[test]
    fn a_number_stops_at_its_unit() {
        assert_eq!(number(b"total: 1024 KiB\n", "total"), Some(1024));
        assert_eq!(number(b"total: many\n", "total"), None);
    }

    #[test]
    fn octal_escapes_become_bytes() {
        assert_eq!(unescape(b"a\\012b\\134c"), b"a\nb\\c");
        // Too short, not octal or past a byte: left as written.
        assert_eq!(unescape(b"\\01"), b"\\01");
        assert_eq!(unescape(b"\\089"), b"\\089");
        assert_eq!(unescape(b"\\777"), b"\\777");
    }

    #[test]
    fn the_last_pair_keeps_its_spaces() {
        let line = b"number=7 run=sleeping cpu=2 name=my\\012thread x";
        let (fields, name) = pairs(line, "name");
        assert_eq!(fields, [("number", "7"), ("run", "sleeping"), ("cpu", "2")]);
        assert_eq!(name.as_deref(), Some(&b"my\nthread x"[..]));
        let (fields, name) = pairs(b"slot=3 kind=thread rights=read", "path");
        assert_eq!(fields.len(), 3);
        assert_eq!(name, None);
    }

    #[test]
    fn names_of_numbers_are_strict() {
        assert_eq!(decimal("0"), Some(0));
        assert_eq!(decimal("4094"), Some(4094));
        assert_eq!(decimal("4294967296"), Some(4_294_967_296));
        for name in ["", "007", "+5", "-1", "1x", "99999999999999999999", " 1"] {
            assert_eq!(decimal(name), None, "{name:?}");
        }
    }

    #[test]
    fn a_page_is_its_lines() {
        let mut page = Page::new();
        page.line(format_args!("a: {}", 1));
        page.push(format_args!("b"));
        page.line(format_args!("c"));
        assert_eq!(page.finish(), "a: 1\nbc\n");
    }
}
