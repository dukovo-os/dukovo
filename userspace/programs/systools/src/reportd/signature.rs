// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! A crash's signature (ADR 0531): what makes two crashes the same crash,
//! so that each is offered once and the reports of it group together.
//!
//! A program's crash: its file name, the signal, and the first three
//! frames that are not the machinery of an abort or a panic, each by its
//! function's name, or by its file and offset where it has none. A
//! panic: the program, its location and its message without the parts
//! that change from one run to the next. The kernel's: its message so,
//! and the first three functions of its backtrace. The ID is the first 16
//! hex digits of the signature's SHA-256.

use super::unwind::Frame;

/// How many frames make a signature.
const FRAMES: usize = 3;

/// The start of the names of functions that only carry a crash out:
/// the C library's abort, and Rust's panic and abort machinery.
const MACHINERY: [&str; 10] = [
    "abort",
    "raise",
    "__pthread_kill",
    "pthread_kill",
    "core::panicking::",
    "std::panicking::",
    "std::sys::",
    "std::process::abort",
    "rust_panic",
    "__rust_start_panic",
];

/// The signature of a crash of `program` by `signal`, from its trace.
pub fn of_signal(program: &str, signal: &str, frames: &[Frame]) -> String {
    let parts: Vec<String> = frames
        .iter()
        .filter(|f| !is_machinery(f))
        .take(FRAMES)
        .map(|f| match (&f.function, &f.file) {
            (Some((name, _)), _) => name.clone(),
            (None, Some(place)) => format!("{}+{:#x}", base_name(&place.path), place.offset),
            (None, None) => "?".to_string(),
        })
        .collect();
    format!("{program} {signal} {}", parts.join(" < "))
}

/// The signature of a panic of `program` at `location` with `message`.
pub fn of_panic(program: &str, location: &str, message: &str) -> String {
    format!("{program} panic {location} {}", steady(message))
}

/// The signature of a kernel panic with `message` and the functions of its
/// backtrace, innermost first.
pub fn of_kernel(message: &str, functions: &[String]) -> String {
    let functions: Vec<&str> = functions.iter().take(FRAMES).map(String::as_str).collect();
    format!("kernel panic {} {}", steady(message), functions.join(" < "))
}

/// The ID of `signature`: 16 hex digits.
pub fn id(signature: &str) -> String {
    let digest = ring::digest::digest(&ring::digest::SHA256, signature.as_bytes());
    super::server::hex(&digest.as_ref()[..8])
}

/// Whether `frame` is the machinery of an abort or a panic.
pub fn is_machinery(frame: &Frame) -> bool {
    frame.function.as_ref().is_some_and(|(name, _)| {
        let name = name.trim_start_matches('_');
        MACHINERY.iter().any(|m| name.starts_with(m.trim_start_matches('_')))
    })
}

/// `message` without what changes between runs of the same crash: each
/// number, decimal or hex (`0x` too), becomes `#`, and each quoted part
/// `"…"`.
fn steady(message: &str) -> String {
    let mut out = String::new();
    let mut chars = message.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '"' || c == '`' {
            out.push(c);
            out.push('…');
            // To the closing quote, which is kept; an open one runs out.
            for inner in chars.by_ref() {
                if inner == c {
                    out.push(c);
                    break;
                }
            }
        } else if c.is_ascii_digit() {
            out.push('#');
            while chars.next_if(|&n| n.is_ascii_hexdigit() || n == 'x').is_some() {}
        } else {
            out.push(c);
        }
    }
    out
}

fn base_name(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

#[cfg(test)]
mod tests {
    use super::super::unwind::Place;
    use super::*;

    fn named(name: &str) -> Frame {
        Frame { address: 1, file: None, function: Some((name.into(), 4)) }
    }

    #[test]
    fn a_signature_skips_the_abort_and_panic_machinery() {
        let frames = [
            named("__pthread_kill_implementation"),
            named("raise"),
            named("abort"),
            named("std::process::abort"),
            named("report_test::fault_here"),
            named("report_test::middle"),
            named("report_test::outer"),
            named("main"),
        ];
        assert_eq!(
            of_signal("crash-test", "SIGABRT", &frames),
            "crash-test SIGABRT report_test::fault_here < report_test::middle < report_test::outer"
        );
    }

    #[test]
    fn a_frame_without_a_name_is_its_file_and_offset() {
        let frames = [Frame {
            address: 0x55_0000,
            file: Some(Place { path: "/system/commands/x".into(), offset: 0x1a2b, build_id: None }),
            function: None,
        }];
        assert_eq!(of_signal("x", "SIGSEGV", &frames), "x SIGSEGV x+0x1a2b");
    }

    #[test]
    fn what_changes_between_runs_is_left_out() {
        assert_eq!(
            of_panic(
                "web",
                "src/main.rs:12:5",
                r#"index 7 out of range for slice of length 3 at "/data/x""#
            ),
            r#"web panic src/main.rs:12:5 index # out of range for slice of length # at "…""#
        );
        assert_eq!(steady("took 1500 ms, `cfg` 2"), "took # ms, `…` #");
        assert_eq!(
            of_kernel("page fault at 0xffff8000dead", &["radix::a".into(), "radix::b".into()]),
            "kernel panic page fault at # radix::a < radix::b"
        );
    }

    #[test]
    fn the_id_is_the_start_of_the_signatures_sha256() {
        // SHA-256("abc") starts ba7816bf8f01cfea.
        assert_eq!(id("abc"), "ba7816bf8f01cfea");
        assert_eq!(id("abc").len(), 16);
    }
}
