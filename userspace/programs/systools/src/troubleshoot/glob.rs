// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The globs of a signature (ADR 0474): `*` for any run of characters,
//! `?` for any one character, and every other character for itself,
//! matched against the whole text and with capitals told apart. A glob
//! comes from data fetched from the network, so what it can do is held
//! to what cannot run long.
//!
//! The matcher keeps no place to come back to. A glob of `k` characters
//! that are not `*` has the states 0 to `k`: state `i` is "the first `i`
//! of them matched". A text is read once, a character at a time, and
//! after each one the set of states that can hold is worked out from the
//! set before it: a state moves to the next if the character is what the
//! next item takes, and stays if a `*` follows it. That set is one bit
//! per state, in four machine words, so a character costs a few word
//! operations whatever the glob and whatever came before it: the time is
//! the text's length, and a glob such as `*a*a*a*a*b` is no worse than
//! `*b` on a text of `a`s.

/// The most bytes a glob may have.
pub const MAX_LEN: usize = 200;

/// The words in a set of states: 64 states each, and a glob of
/// [`MAX_LEN`] bytes has at most 201.
const WORDS: usize = 4;

/// A set of states, one bit each.
type States = [u64; WORDS];

/// The empty set.
const NONE: States = [0; WORDS];

/// A glob, ready to match.
#[derive(Debug)]
pub struct Glob {
    /// The items that are not `*`: the last state.
    last: usize,
    /// The states whose next item is `?`.
    any: States,
    /// The states a `*` follows: they stay.
    stay: States,
    /// For each ASCII character, the states whose next item is it.
    next: Vec<States>,
}

impl Glob {
    /// The glob `pattern`.
    ///
    /// # Errors
    ///
    /// Why it is no glob: it is empty, longer than [`MAX_LEN`] bytes, or
    /// has a character that is not printable ASCII.
    pub fn new(pattern: &str) -> Result<Glob, String> {
        if pattern.is_empty() {
            return Err("it is empty".into());
        }
        if pattern.len() > MAX_LEN {
            return Err(format!(
                "it is {} bytes, and at most {MAX_LEN} are allowed",
                pattern.len()
            ));
        }
        if !pattern.bytes().all(|byte| byte.is_ascii_graphic() || byte == b' ') {
            return Err("it has a character that is not printable ASCII".into());
        }
        let mut glob = Glob { last: 0, any: NONE, stay: NONE, next: vec![NONE; 128] };
        for byte in pattern.bytes() {
            match byte {
                // Runs of `*` set the same bit.
                b'*' => add(&mut glob.stay, glob.last),
                b'?' => {
                    add(&mut glob.any, glob.last);
                    glob.last += 1;
                }
                _ => {
                    add(&mut glob.next[usize::from(byte)], glob.last);
                    glob.last += 1;
                }
            }
        }
        Ok(glob)
    }

    /// Whether the glob matches all of `text`.
    #[must_use]
    pub fn matches(&self, text: &str) -> bool {
        let mut states = NONE;
        add(&mut states, 0);
        for &byte in text.as_bytes() {
            // The bytes after the first of a character belong to it.
            if (0x80..0xC0).contains(&byte) {
                continue;
            }
            // A character that is not ASCII is taken by `?` and `*` alone.
            let takes =
                if byte < 0x80 { join(self.any, self.next[usize::from(byte)]) } else { self.any };
            states = join(shift(keep(states, takes)), keep(states, self.stay));
            if states == NONE {
                return false;
            }
        }
        has(states, self.last)
    }
}

/// Puts `state` in `set`.
fn add(set: &mut States, state: usize) {
    set[state / 64] |= 1 << (state % 64);
}

/// Whether `state` is in `set`.
fn has(set: States, state: usize) -> bool {
    (set[state / 64] >> (state % 64)) & 1 == 1
}

/// The states in both sets.
fn keep(set: States, other: States) -> States {
    std::array::from_fn(|i| set[i] & other[i])
}

/// The states in either set.
fn join(set: States, other: States) -> States {
    std::array::from_fn(|i| set[i] | other[i])
}

/// Each state moved to the next.
fn shift(set: States) -> States {
    let mut moved = NONE;
    let mut carry = 0;
    for (word, into) in set.iter().zip(moved.iter_mut()) {
        *into = (word << 1) | carry;
        carry = word >> 63;
    }
    moved
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The same question asked the plain way, in time of the glob's
    /// length times the text's: the reference the matcher is held to.
    fn plain(pattern: &[char], text: &[char]) -> bool {
        // `row[j]`: the items so far match the first `j` characters of the text.
        let mut row = vec![false; text.len() + 1];
        row[0] = true;
        for &item in pattern {
            let mut next = vec![false; text.len() + 1];
            if item == '*' {
                let mut any = false;
                for (into, &before) in next.iter_mut().zip(&row) {
                    any |= before;
                    *into = any;
                }
            } else {
                for (j, &character) in text.iter().enumerate() {
                    next[j + 1] = row[j] && (item == '?' || item == character);
                }
            }
            row = next;
        }
        row[text.len()]
    }

    /// Every string of `alphabet` of `longest` characters or fewer.
    fn all_strings(alphabet: &[char], longest: usize) -> Vec<String> {
        let mut found = vec![String::new()];
        let mut layer = vec![String::new()];
        for _ in 0..longest {
            layer = layer
                .iter()
                .flat_map(|text| alphabet.iter().map(move |letter| format!("{text}{letter}")))
                .collect();
            found.extend(layer.iter().cloned());
        }
        found
    }

    #[test]
    fn matches_the_whole_text_with_star_and_question_mark() {
        let yes = |pattern: &str, text: &str| Glob::new(pattern).unwrap().matches(text);
        assert!(yes("abc", "abc"));
        assert!(!yes("abc", "abcd") && !yes("abc", "xabc") && !yes("abc", "ab"));
        assert!(yes("*", "") && yes("*", "anything at all") && yes("**", "x"));
        assert!(yes("?", "a") && !yes("?", "") && !yes("?", "ab"));
        assert!(yes("*I/O error*", "blk: I/O error, dev vda, sector 8"));
        assert!(yes("*I/O error*", "I/O error"));
        assert!(!yes("*I/O error*", "i/o error"));
        assert!(yes("a*b*c", "abc") && yes("a*b*c", "aXXbYYc") && !yes("a*b*c", "aXXbYY"));
        assert!(yes("a?c", "abc") && !yes("a?c", "ac") && !yes("a?c", "abbc"));
        assert!(yes("*a", "bba") && !yes("*a", "bab") && yes("a*", "abb") && !yes("a*", "bab"));
        assert!(yes("a  b", "a  b") && !yes("a  b", "a b"));
        // Nothing but `*` and `?` is special.
        assert!(yes("[a]\\.+(x)", "[a]\\.+(x)") && !yes("[a]", "a") && !yes("a.c", "abc"));
        assert!(yes("a*b", "a*b") && yes("a?b", "a?b"));
        // A character is one thing to `?`, whatever its bytes.
        assert!(yes("a?c", "a\u{e9}c") && yes("a?c", "a\u{1f600}c") && !yes("a??c", "a\u{e9}c"));
        assert!(yes("a*c", "a\u{e9}\u{1f600}c") && !yes("a", "\u{e9}"));
    }

    #[test]
    fn what_is_no_glob_is_refused() {
        assert!(Glob::new("").is_err());
        assert!(Glob::new("caf\u{e9}").is_err());
        for bad in ["tab\there", "new\nline", "esc\u{1b}[0m", "nul\0", "del\u{7f}"] {
            assert!(Glob::new(bad).is_err(), "{bad:?}");
        }
        assert!(Glob::new("fine, with a space and ~!@#$%^&()-_=+[]{}|;:'\",.<>/\\`").is_ok());
        assert!(Glob::new(&"a".repeat(MAX_LEN)).is_ok());
        assert!(Glob::new(&"a".repeat(MAX_LEN + 1)).is_err());
        assert!(Glob::new(&"*".repeat(MAX_LEN)).is_ok());
        assert!(Glob::new(&"*".repeat(MAX_LEN + 1)).is_err());
    }

    #[test]
    fn it_agrees_with_the_plain_way_on_every_small_glob_and_text() {
        let patterns = all_strings(&['a', 'b', '*', '?'], 6);
        let texts = all_strings(&['a', 'b', '\u{e9}'], 5);
        let texts: Vec<(String, Vec<char>)> =
            texts.into_iter().map(|text| (text.clone(), text.chars().collect())).collect();
        for pattern in patterns.iter().filter(|pattern| !pattern.is_empty()) {
            let glob = Glob::new(pattern).unwrap();
            let items: Vec<char> = pattern.chars().collect();
            for (text, chars) in &texts {
                assert_eq!(glob.matches(text), plain(&items, chars), "{pattern:?} on {text:?}");
            }
        }
    }

    #[test]
    fn it_agrees_with_the_plain_way_at_the_edges_of_its_words() {
        // The last state is in a word of its own, or at the end of one,
        // for these lengths.
        for length in [1, 62, 63, 64, 65, 126, 127, 128, 129, 190, 191, 192, 193, 199, 200] {
            let literal: String = (0..length).map(|i| if i % 3 == 0 { 'a' } else { 'b' }).collect();
            let with_any: String = literal
                .chars()
                .enumerate()
                .map(|(i, c)| if i % 5 == 4 { '?' } else { c })
                .collect();
            let with_star = format!("*{}", &literal[1..]);
            for pattern in [&literal, &with_any, &with_star] {
                let items: Vec<char> = pattern.chars().collect();
                let glob = Glob::new(pattern).unwrap();
                let mut texts = vec![literal.clone(), format!("x{literal}"), format!("{literal}x")];
                texts.push(literal[..length - 1].to_string());
                texts.push(literal.replace('b', "c"));
                texts.push(literal.replace('a', "b"));
                for text in &texts {
                    let chars: Vec<char> = text.chars().collect();
                    assert_eq!(glob.matches(text), plain(&items, &chars), "{length} {text:?}");
                }
            }
            assert!(Glob::new(&literal).unwrap().matches(&literal), "{length}");
        }
    }

    #[test]
    fn a_glob_that_would_make_a_backtracking_matcher_run_long_does_not() {
        // 99 stars and a's, then a star and b: a matcher that backs up
        // tries about 400 choose 99 ways to end in failure on this text.
        let pattern = format!("{}*b", "*a".repeat(99));
        assert_eq!(pattern.len(), MAX_LEN);
        let glob = Glob::new(&pattern).unwrap();
        let text = "a".repeat(400);
        assert!(!glob.matches(&text));
        assert!(glob.matches(&format!("{text}b")));
        assert!(!glob.matches(&format!("b{text}")));
        // The same through `?`.
        let glob = Glob::new(&format!("{}b", "*?".repeat(99))).unwrap();
        assert!(!glob.matches(&"a".repeat(4000)));
        assert!(glob.matches(&format!("{}b", "a".repeat(4000))));
        // A long text, once through.
        let long = "x".repeat(1 << 20);
        assert!(Glob::new("*x*x*x*y").unwrap().matches(&format!("{long}y")));
        assert!(!Glob::new("*x*x*x*y").unwrap().matches(&long));
    }
}
