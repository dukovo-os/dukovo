// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! A crash as reportd keeps it (ADR 0531), from a kernel's record, a
//! service's panic or a kernel dump: what it was, its signature, and the
//! lines of a report's Crash section. Nothing of the program's memory is
//! kept but the trace the unwinder made of it.

use super::record::Record;
use super::signature;
use super::store::{Kept, crash_id};
use super::unwind::Frame;
use crate::reports::protocol::{Crash, Kind};

/// What a signal's code says of how it came, as Linux numbers them; none
/// for a code that says nothing more than the signal.
fn code_words(signal: u32, code: i32) -> Option<&'static str> {
    let signal = i32::try_from(signal).ok()?;
    Some(match (signal, code) {
        (libc::SIGSEGV, 1) => "address not mapped",
        (libc::SIGSEGV, 2) => "access not allowed",
        (libc::SIGBUS, 1) => "misaligned address",
        (libc::SIGBUS, 4) => "memory error",
        (libc::SIGILL, 2) => "illegal instruction",
        (libc::SIGFPE, 1) => "divide by zero",
        (libc::SIGTRAP, 1) => "breakpoint",
        _ => return None,
    })
}

/// `SIGSEGV`, `SIGSEGV (address not mapped)`.
fn signal_text(signal: u32, code: i32) -> String {
    let name = i32::try_from(signal)
        .ok()
        .and_then(crate::util::signal_name)
        .map_or_else(|| format!("signal {signal}"), |name| format!("SIG{name}"));
    match code_words(signal, code) {
        Some(words) => format!("{name} ({words})"),
        None => name,
    }
}

/// Whether the signal comes with the address of a fault.
fn has_address(signal: u32, code: i32) -> bool {
    code > 0 && [libc::SIGSEGV, libc::SIGBUS].iter().any(|&s| i32::try_from(signal) == Ok(s))
}

/// The first frame with a name that is not the machinery of an abort.
fn first_named(frames: &[Frame]) -> Option<&str> {
    frames
        .iter()
        .filter(|f| !signature::is_machinery(f))
        .find_map(|f| f.function.as_ref().map(|(name, _)| name.as_str()))
}

/// What reportd knows of a crash besides its record.
pub struct Context<'a> {
    /// The service whose domain the process ran in, if it is one.
    pub service: Option<&'a str>,
    /// The program's version.
    pub version: Option<String>,
    /// Now, in seconds since 1970, when the record has no time.
    pub now: u64,
}

/// The crash `record` is, with `frames` its trace.
pub fn of_record(record: &Record, frames: &[Frame], context: &Context<'_>) -> Kept {
    let program = record.program();
    let at = record.wall_ns.map_or(context.now, |ns| ns / 1_000_000_000);
    let signal = signal_text(record.signal, record.code);
    let bare = signal.split(' ').next().unwrap_or_default().to_string();
    let signature = signature::of_signal(&program, &bare, frames);
    let id = signature::id(&signature);
    let what = match first_named(frames) {
        Some(name) => format!("{signal} in {name}"),
        None => signal.clone(),
    };
    let mut lines = vec![match (has_address(record.signal, record.code), context.service) {
        (true, Some(service)) => {
            format!("{program} (the service {service}) stopped: {signal} at {:#x}", record.address)
        }
        (true, None) => format!("{program} stopped: {signal} at {:#x}", record.address),
        (false, Some(service)) => format!("{program} (the service {service}) stopped: {signal}"),
        (false, None) => format!("{program} stopped: {signal}"),
    }];
    if let Some(version) = &context.version {
        lines.push(format!("Version: {program} {version}"));
    }
    lines.push(format!("Thread: {}", record.thread_name));
    lines.push(format!("Signature: {id}"));
    lines.extend(frames.iter().enumerate().map(|(n, frame)| format!("{n:>2} {}", frame.line())));
    if frames.is_empty() {
        lines.push("(no frames: the program's files could not be read)".into());
    }
    Kept {
        crash: Crash {
            id: crash_id(at, record.process),
            kind: Kind::Signal,
            program,
            version: context.version.clone(),
            service: context.service.map(str::to_string),
            user: record.user,
            at,
            what,
            signature: id,
            seen: 1,
            reported: None,
        },
        signature,
        lines,
        forgotten: false,
        offered: false,
    }
}

/// The crash a service's panic is: `program` ran as `service` for
/// `user`, and its log's `panicked at LOCATION:` line and the message
/// after it say where and why.
pub fn of_panic(
    program: &str,
    service: &str,
    user: u32,
    location: &str,
    message: &[String],
    context: &Context<'_>,
) -> Kept {
    let first = message.first().map_or("", String::as_str);
    let signature = signature::of_panic(program, location, first);
    let id = signature::id(&signature);
    let mut lines = vec![format!("{program} (the service {service}) panicked at {location}")];
    if let Some(version) = &context.version {
        lines.push(format!("Version: {program} {version}"));
    }
    lines.push(format!("Signature: {id}"));
    lines.extend(message.iter().cloned());
    Kept {
        crash: Crash {
            id: crash_id(context.now, 0),
            kind: Kind::Panic,
            program: program.to_string(),
            version: context.version.clone(),
            service: Some(service.to_string()),
            user,
            at: context.now,
            what: format!("panic at {location}"),
            signature: id,
            seen: 1,
            reported: None,
        },
        signature,
        lines,
        forgotten: false,
        offered: false,
    }
}

/// The crash a kernel dump is: its `== Panic` section, `report`, which the
/// consoles showed; `at` when it panicked.
pub fn of_kernel(report: &[String], at: u64, context: &Context<'_>) -> Kept {
    let first = report.first().map_or("", String::as_str);
    // `Radix panic on CPU N at S.US s: MESSAGE`.
    let message = first.split_once(" s: ").map_or(first, |(_, message)| message);
    let functions: Vec<String> = report
        .iter()
        .skip_while(|line| line.trim() != "Backtrace:")
        .skip(1)
        .filter_map(|line| {
            // `  #0  0xffffffffa031416e  radix::selftest::crash_reboot+0x3e`
            let name = line.split_whitespace().nth(2)?;
            Some(name.split('+').next().unwrap_or(name).to_string())
        })
        .collect();
    let signature = signature::of_kernel(message, &functions);
    let id = signature::id(&signature);
    let mut lines = vec![format!("The kernel panicked: {message}")];
    if let Some(version) = &context.version {
        lines.push(format!("Version: Radix {version}"));
    }
    lines.push(format!("Signature: {id}"));
    lines.extend(report.iter().cloned());
    Kept {
        crash: Crash {
            id: crash_id(at, 0),
            kind: Kind::Kernel,
            program: "kernel".into(),
            version: context.version.clone(),
            service: None,
            user: 0,
            at,
            what: format!("kernel panic: {message}"),
            signature: id,
            seen: 1,
            reported: None,
        },
        signature,
        lines,
        forgotten: false,
        offered: false,
    }
}

/// The program at `path`'s version: the system's for the system's own
/// programs, its bundle's for an app's.
pub fn version_of(path: Option<&str>, system: &str) -> Option<String> {
    let path = path?;
    if path.starts_with("/system/") {
        return Some(system.to_string());
    }
    // `/applications/ID/...` or `/users/NAME/applications/ID/...`.
    let parts: Vec<&str> = path.split('/').collect();
    let at = parts.iter().position(|part| *part == crate::bundled::APPLICATIONS)?;
    parts.get(at + 1).filter(|id| !id.is_empty())?;
    let folder = std::path::PathBuf::from(parts[..=at + 1].join("/"));
    let text = std::fs::read_to_string(folder.join(crate::bundles::MANIFEST)).ok()?;
    Some(manifest::bundle::from_json(&text).ok()?.version.to_string())
}

#[cfg(test)]
mod tests {
    use super::super::unwind::Place;
    use super::*;

    fn named(name: &str) -> Frame {
        Frame {
            address: 0x10_1000,
            file: Some(Place {
                path: "/system/tests/report/crash-test".into(),
                offset: 0x1000,
                build_id: Some("7be951e4".into()),
            }),
            function: Some((name.into(), 0x4c)),
        }
    }

    fn record() -> Record {
        Record {
            dropped: 0,
            signal: 11,
            code: 1,
            address: 0,
            process: 4012,
            thread: 4012,
            user: 0,
            group: 0,
            ancestors: vec![1],
            wall_ns: Some(1_763_028_131_000_000_000),
            thread_name: "crash-test".into(),
            pc: 0x10_1000,
            sp: 0,
            fp: 0,
            path: Some("/system/tests/report/crash-test".into()),
            domain: "crash-test".into(),
            mappings: Vec::new(),
            stack: Vec::new(),
        }
    }

    #[test]
    fn a_segfault_says_where_and_in_what() {
        let frames = [named("report_test::fault_here"), named("report_test::middle")];
        let context =
            Context { service: Some("crash-test"), version: Some("0.16.0".into()), now: 0 };
        let kept = of_record(&record(), &frames, &context);
        assert_eq!(kept.crash.id, "20251113T100211-4012");
        assert_eq!(kept.crash.what, "SIGSEGV (address not mapped) in report_test::fault_here");
        assert_eq!(
            kept.signature,
            "crash-test SIGSEGV report_test::fault_here < report_test::middle"
        );
        assert_eq!(
            kept.lines[0],
            "crash-test (the service crash-test) stopped: SIGSEGV (address not mapped) at 0x0"
        );
        assert_eq!(kept.lines[3], format!("Signature: {}", kept.crash.signature));
        assert_eq!(
            kept.lines[4],
            " 0 report_test::fault_here+0x4c (/system/tests/report/crash-test +0x1000, build 7be951e4)"
        );
    }

    #[test]
    fn an_abort_has_no_address() {
        let record = Record { signal: 6, code: -6, ..record() };
        let context = Context { service: None, version: None, now: 0 };
        let kept = of_record(&record, &[named("abort"), named("main")], &context);
        assert_eq!(kept.lines[0], "crash-test stopped: SIGABRT");
        assert_eq!(kept.crash.what, "SIGABRT in main");
    }

    #[test]
    fn a_kernel_dump_is_signed_by_its_message_and_backtrace() {
        let report: Vec<String> = [
            "Radix panic on CPU 0 at 3612.481930 s: selftest crash-reboot: deliberate panic",
            "  at kernel/radix/src/selftest.rs:118:13",
            "Backtrace:",
            "  #0  0xffffffffa031416e  radix::selftest::crash_reboot+0x3e",
            "  #1  0xffffffffa0310000  radix::selftest::run+0x10",
        ]
        .map(String::from)
        .to_vec();
        let context = Context { service: None, version: Some("0.16.0".into()), now: 0 };
        let kept = of_kernel(&report, 1_763_028_131, &context);
        assert_eq!(
            kept.signature,
            "kernel panic selftest crash-reboot: deliberate panic \
             radix::selftest::crash_reboot < radix::selftest::run"
        );
        assert_eq!(kept.crash.kind, Kind::Kernel);
        assert_eq!(kept.lines[0], "The kernel panicked: selftest crash-reboot: deliberate panic");
    }

    #[test]
    fn a_system_program_has_the_systems_version() {
        assert_eq!(version_of(Some("/system/commands/ls"), "0.16.0").as_deref(), Some("0.16.0"));
        assert_eq!(version_of(None, "0.16.0"), None);
        assert_eq!(version_of(Some("/tmp/a.out"), "0.16.0"), None);
    }
}
