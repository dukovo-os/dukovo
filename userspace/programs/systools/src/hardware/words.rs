// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! A line of one of the kernel's accounts of a class of devices, as
//! _/kernel/class/display/state_ and _/kernel/class/input/state_ write
//! them (radix-sysfs(5)): what the line is of, that thing's number, and
//! `key=value` words, the last of which may be a text with spaces in it.

/// A line's `key=value` words. The value of `last`, which may hold
/// spaces, runs to the line's end.
pub(super) struct Words<'a> {
    line: &'a str,
    last: &'a str,
}

impl<'a> Words<'a> {
    /// The words of `line`, the value of whose `last` key runs to its
    /// end; `last` is empty for a line with no such key.
    pub(super) fn of(line: &'a str, last: &'a str) -> Words<'a> {
        Words { line, last }
    }

    pub(super) fn text(&self, key: &str) -> Option<&'a str> {
        if key == self.last {
            let at = self.line.find(&format!(" {key}="))?;
            return Some(&self.line[at + key.len() + 2..]);
        }
        let before = self.line.find(&format!(" {}=", self.last)).unwrap_or(self.line.len());
        self.line[..before].split(' ').find_map(|word| word.strip_prefix(key)?.strip_prefix('='))
    }

    pub(super) fn number(&self, key: &str) -> u32 {
        self.text(key).and_then(|value| value.parse().ok()).unwrap_or_default()
    }

    pub(super) fn flag(&self, key: &str) -> bool {
        self.number(key) != 0
    }

    /// A number that 0 stands for "not said" in.
    pub(super) fn said(&self, key: &str) -> Option<u32> {
        Some(self.number(key)).filter(|&value| value != 0)
    }

    /// A text that may be empty.
    pub(super) fn named(&self, key: &str) -> Option<String> {
        self.text(key).map(str::trim).filter(|text| !text.is_empty()).map(String::from)
    }
}

/// The thing's number and its words, of a line that starts with `kind`.
pub(super) fn line_of<'a>(line: &'a str, kind: &str, last: &'a str) -> Option<(u32, Words<'a>)> {
    let rest = line.strip_prefix(kind)?.strip_prefix(' ')?;
    let id = rest.split(' ').next()?.parse().ok()?;
    Some((id, Words::of(rest, last)))
}
