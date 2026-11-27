// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Patterns as the C library matches them: POSIX extended regular
//! expressions (regex(3)), as grep(1) and the tools that follow it take
//! them, and the shell's wildcards (fnmatch(3)).

use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;

/// A compiled extended regular expression of the C library's.
pub struct Regex {
    compiled: Box<RegexT>,
    /// Room for the match and its groups that [`Regex::captures`] asks
    /// for; 0 for one compiled without groups.
    groups: usize,
}

/// regmatch_t: where a match or group starts and ends, -1 for a group
/// that took no part.
#[repr(C)]
#[derive(Clone, Copy)]
struct Match {
    start: Offset,
    end: Offset,
}

/// regoff_t, an int in glibc and an ssize_t in mlibc.
#[cfg(target_env = "gnu")]
type Offset = libc::c_int;
#[cfg(not(target_env = "gnu"))]
type Offset = isize;

/// Room for the C library's regex_t, whose size the libc crate does not
/// give for every system: mlibc's and glibc's are well under it.
#[repr(C, align(8))]
struct RegexT([u8; 256]);

// SAFETY: these are the C library's functions, as regex.h declares them;
// RegexT has room and alignment for any regex_t.
unsafe extern "C" {
    fn regcomp(regex: *mut RegexT, pattern: *const libc::c_char, flags: libc::c_int)
    -> libc::c_int;
    fn regexec(
        regex: *const RegexT,
        text: *const libc::c_char,
        nmatch: usize,
        matches: *mut Match,
        flags: libc::c_int,
    ) -> libc::c_int;
    fn regerror(
        code: libc::c_int,
        regex: *const RegexT,
        buffer: *mut libc::c_char,
        size: usize,
    ) -> usize;
    fn regfree(regex: *mut RegexT);
}

/// regcomp(3)'s REG_EXTENDED, REG_ICASE and REG_NOSUB, the same in every
/// C library.
const REG_EXTENDED: libc::c_int = 1;
const REG_ICASE: libc::c_int = 2;
const REG_NOSUB: libc::c_int = 8;

impl Regex {
    /// Compiles the extended regular expression `pattern`, ignoring case
    /// if asked.
    ///
    /// # Errors
    ///
    /// The C library's words for what is wrong with the pattern, and for
    /// a pattern with a NUL byte.
    pub fn new(pattern: &str, ignore_case: bool) -> Result<Regex, String> {
        Regex::compile(pattern.as_bytes(), ignore_case, 0)
    }

    /// Compiles the extended regular expression `pattern`, as [`Regex::new`]
    /// does, keeping where its groups match, for [`Regex::captures`]: the
    /// `=~` of a shell's `[[`, whose groups fill `BASH_REMATCH`.
    ///
    /// # Errors
    ///
    /// As [`Regex::new`]'s.
    pub fn with_groups(pattern: &[u8], ignore_case: bool) -> Result<Regex, String> {
        // Room for every group the pattern could have, and the match.
        let groups = pattern.iter().filter(|&&b| b == b'(').count() + 1;
        Regex::compile(pattern, ignore_case, groups)
    }

    fn compile(pattern: &[u8], ignore_case: bool, groups: usize) -> Result<Regex, String> {
        let text = crate::c_string(pattern).map_err(|_| "a pattern has a NUL in it".to_string())?;
        let mut compiled = Box::new(RegexT([0; 256]));
        let keep = if groups == 0 { REG_NOSUB } else { 0 };
        let flags = REG_EXTENDED | keep | if ignore_case { REG_ICASE } else { 0 };
        // SAFETY: `compiled` has room for a regex_t, `text` is a C string.
        let code = unsafe { regcomp(&raw mut *compiled, text.as_ptr(), flags) };
        if code != 0 {
            let mut buffer = [0u8; 256];
            // SAFETY: regerror writes at most the buffer's length into it,
            // ending with a NUL, and only reads `compiled`.
            unsafe {
                regerror(code, &raw const *compiled, buffer.as_mut_ptr().cast(), buffer.len())
            };
            let end = buffer.iter().position(|&b| b == 0).unwrap_or(0);
            // A pattern regcomp refused holds nothing to free.
            return Err(String::from_utf8_lossy(&buffer[..end]).into_owned());
        }
        Ok(Regex { compiled, groups })
    }

    /// Whether it matches somewhere in `text`. A text with a NUL byte,
    /// which no C string holds, matches nothing.
    #[must_use]
    pub fn is_match(&self, text: &str) -> bool {
        let Ok(text) = crate::c_string(text.as_bytes()) else { return false };
        // SAFETY: the expression was compiled; regexec only reads it and
        // the C string, and no matches are asked for. POSIX makes regexec
        // safe to call from several threads at once.
        unsafe {
            regexec(&raw const *self.compiled, text.as_ptr(), 0, std::ptr::null_mut(), 0) == 0
        }
    }
}

impl Regex {
    /// Where it first matches in `text`, and where each of its groups
    /// matched there: the byte ranges, the match's first, then one for
    /// each `(` of the pattern, `None` for a group that took no part. The
    /// C library leaves the slots past the pattern's last group as `None`
    /// too. `None` when it does not match, as for a text with a NUL byte.
    /// One compiled with [`Regex::new`] gives no ranges.
    #[must_use]
    pub fn captures(&self, text: &[u8]) -> Option<Vec<Option<std::ops::Range<usize>>>> {
        let c_text = crate::c_string(text).ok()?;
        let mut matches = vec![Match { start: -1, end: -1 }; self.groups];
        let room = if self.groups == 0 { std::ptr::null_mut() } else { matches.as_mut_ptr() };
        // SAFETY: the expression was compiled; regexec reads it and the C
        // string, and writes at most `groups` matches, which `matches` has
        // room for. POSIX makes regexec safe to call from several threads
        // at once.
        let found =
            unsafe { regexec(&raw const *self.compiled, c_text.as_ptr(), self.groups, room, 0) };
        if found != 0 {
            return None;
        }
        let range = |m: &Match| {
            let start = usize::try_from(m.start).ok()?;
            let end = usize::try_from(m.end).ok()?;
            (start <= end && end <= text.len()).then_some(start..end)
        };
        Some(matches.iter().map(range).collect())
    }
}

impl Drop for Regex {
    fn drop(&mut self) {
        // SAFETY: it was compiled, and is freed once.
        unsafe { regfree(&raw mut *self.compiled) };
    }
}

/// Whether `name` matches the shell pattern `pattern`, with `*`, `?` and
/// `[...]`, as fnmatch(3) with no flags matches it. A pattern or name
/// with a NUL byte matches nothing.
#[must_use]
pub fn fnmatch(pattern: &str, name: &OsStr) -> bool {
    let (Ok(pattern), Ok(name)) =
        (crate::c_string(pattern.as_bytes()), crate::c_string(name.as_bytes()))
    else {
        return false;
    };
    // SAFETY: both are C strings, which fnmatch only reads.
    unsafe { libc::fnmatch(pattern.as_ptr(), name.as_ptr(), 0) == 0 }
}

#[cfg(test)]
mod tests {
    use std::ffi::OsStr;

    use super::{Regex, fnmatch};

    #[test]
    fn the_groups_of_a_match_are_where_they_matched() {
        let regex = Regex::with_groups(b"^([a-z]+)-([0-9]+)?(x)?", false).unwrap();
        let found = regex.captures(b"abc-123").unwrap();
        assert_eq!(found, [Some(0..7), Some(0..3), Some(4..7), None]);
        assert_eq!(regex.captures(b"abc-").unwrap()[..3], [Some(0..4), Some(0..3), None]);
        assert_eq!(regex.captures(b"123"), None);
        assert_eq!(regex.captures(b"ab\0c"), None, "no C string holds a NUL");
        assert!(Regex::with_groups(b"(", false).is_err());
        assert_eq!(Regex::new("b", false).unwrap().captures(b"abc"), Some(Vec::new()));
    }

    #[test]
    fn an_extended_expression_matches_anywhere() {
        let regex = Regex::new("b+c$", false).unwrap();
        assert!(regex.is_match("abbc"));
        assert!(!regex.is_match("abcd"));
        assert!(!regex.is_match("ABC"));
        assert!(Regex::new("b+c$", true).unwrap().is_match("ABC"));
        assert!(!regex.is_match("bc\0"));
    }

    #[test]
    fn a_broken_expression_is_refused_with_the_c_library_s_words() {
        let error = Regex::new("a(b", false).err().unwrap();
        assert!(!error.is_empty());
        assert!(Regex::new("a\0b", false).is_err());
    }

    #[test]
    fn a_wildcard_matches_as_the_shell_does() {
        assert!(fnmatch("*.log", OsStr::new("boot.log")));
        assert!(fnmatch("disk[0-9]", OsStr::new("disk3")));
        assert!(!fnmatch("*.log", OsStr::new("boot.txt")));
        assert!(!fnmatch("*", OsStr::new("a\0b")));
    }
}
