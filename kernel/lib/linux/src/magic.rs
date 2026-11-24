// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The links in /proc that the kernel answers for a process's own (ADR
//! 0620): which paths name them. The kernel's Linux table looks at a path
//! with [`parse`] before any walk; what the link leads to is the
//! kernel's to find.

/// A link of the calling process's own that the kernel answers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Magic {
    /// The program it runs.
    Exe,
    /// Its working directory.
    Cwd,
    /// Its root.
    Root,
    /// One of its descriptors.
    Fd(u32),
}

/// The link `path` names, if it is one of the calling process's own and
/// `own` is its number: `/proc/self/`, `/proc/thread-self/` or
/// `/proc/<own>/`, then `exe`, `cwd`, `root` or `fd/<n>`; or `/dev/fd/<n>`
/// and `/dev/std{in,out,err}`. Only an absolute path, with nothing after
/// the link's name but, for `cwd` and `root`, slashes; a number is
/// decimal, without leading zeros, as Linux's /proc reads it.
pub fn parse(path: &[u8], own: u64) -> Option<Magic> {
    if path.first() != Some(&b'/') {
        return None;
    }
    let slash_after = path.last() == Some(&b'/');
    let mut parts = path.split(|&b| b == b'/').filter(|part| !part.is_empty());
    let found = match parts.next()? {
        b"proc" => {
            let whose = parts.next()?;
            let own_one = whose == b"self"
                || whose == b"thread-self"
                || number(whose).is_some_and(|n| u64::from(n) == own);
            if !own_one {
                return None;
            }
            match parts.next()? {
                b"exe" => Magic::Exe,
                b"cwd" => Magic::Cwd,
                b"root" => Magic::Root,
                b"fd" => Magic::Fd(number(parts.next()?)?),
                _ => return None,
            }
        }
        b"dev" => match parts.next()? {
            b"fd" => Magic::Fd(number(parts.next()?)?),
            b"stdin" => Magic::Fd(0),
            b"stdout" => Magic::Fd(1),
            b"stderr" => Magic::Fd(2),
            _ => return None,
        },
        _ => return None,
    };
    if parts.next().is_some() {
        return None;
    }
    // A slash after a name the walk would take as a directory: the
    // working directory and the root are, and the rest go to the walk.
    if slash_after && !matches!(found, Magic::Cwd | Magic::Root) {
        return None;
    }
    Some(found)
}

/// `bytes` as a decimal number that fits a descriptor, without leading
/// zeros.
fn number(bytes: &[u8]) -> Option<u32> {
    if bytes.is_empty() || bytes.len() > 10 || (bytes.len() > 1 && bytes[0] == b'0') {
        return None;
    }
    let mut n: u32 = 0;
    for &b in bytes {
        if !b.is_ascii_digit() {
            return None;
        }
        n = n.checked_mul(10)?.checked_add(u32::from(b - b'0'))?;
    }
    Some(n)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn own_links_are_the_kernels() {
        assert_eq!(parse(b"/proc/self/exe", 7), Some(Magic::Exe));
        assert_eq!(parse(b"/proc/thread-self/cwd", 7), Some(Magic::Cwd));
        assert_eq!(parse(b"/proc/7/root", 7), Some(Magic::Root));
        assert_eq!(parse(b"//proc//self//fd//12", 7), Some(Magic::Fd(12)));
        assert_eq!(parse(b"/proc/self/cwd/", 7), Some(Magic::Cwd));
        assert_eq!(parse(b"/dev/fd/63", 7), Some(Magic::Fd(63)));
        assert_eq!(parse(b"/dev/stdin", 7), Some(Magic::Fd(0)));
        assert_eq!(parse(b"/dev/stdout", 7), Some(Magic::Fd(1)));
        assert_eq!(parse(b"/dev/stderr", 7), Some(Magic::Fd(2)));
        assert_eq!(parse(b"/proc/self/fd/0", 7), Some(Magic::Fd(0)));
    }

    #[test]
    fn others_go_to_the_walk() {
        // Another process's, a relative path, more after the link, a
        // slash after a file's link, numbers Linux would not read.
        assert_eq!(parse(b"/proc/8/exe", 7), None);
        assert_eq!(parse(b"proc/self/exe", 7), None);
        assert_eq!(parse(b"/proc/self/cwd/file", 7), None);
        assert_eq!(parse(b"/proc/self/exe/", 7), None);
        assert_eq!(parse(b"/proc/self/fd/01", 7), None);
        assert_eq!(parse(b"/proc/self/fd/", 7), None);
        assert_eq!(parse(b"/proc/self/fd/1x", 7), None);
        assert_eq!(parse(b"/proc/self/fd/99999999999", 7), None);
        assert_eq!(parse(b"/proc/self/maps", 7), None);
        assert_eq!(parse(b"/proc/self", 7), None);
        assert_eq!(parse(b"/dev/null", 7), None);
        assert_eq!(parse(b"/dev/fd", 7), None);
        assert_eq!(parse(b"/", 7), None);
        assert_eq!(parse(b"", 7), None);
    }

    #[test]
    fn numbers_are_decimal_descriptors() {
        assert_eq!(number(b"0"), Some(0));
        assert_eq!(number(b"4294967295"), Some(u32::MAX));
        assert_eq!(number(b"4294967296"), None);
        assert_eq!(number(b"007"), None);
        assert_eq!(number(b"-1"), None);
    }
}
