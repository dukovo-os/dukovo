// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! hardware(8): `hardware check` looks for hardware that is failing, and
//! `hardware last` shows what the last check found (ADR 0488).
//!
//! A check looks at six areas: the memory (the errors the memory
//! controller counted, and a test of a share of the free memory), the
//! disks (what each says of its health, its own self-test, and a read of
//! the disk), the CPUs (all of them under load, which must give the same
//! answers; their temperature; the machine checks), the hardware
//! monitors' temperatures and fans, the battery's capacity, and the
//! network cards' links. A quick check takes about three minutes; a full
//! one, which `--full` asks for, takes hours, mostly for the disks.
//!
//! Where to look: `check` holds a check's result and its exit status,
//! and the IDs of what it finds (HW-101 to HW-153, each with a page in
//! docs/errors); `plan` what a run does and how long it takes; `run` the
//! order it does it in; `saved` the result kept in
//! `/system/state/hardware/last.json`; one module for each area; and
//! `drivetest` and `scan` for a disk's self-test and read scan.

mod battery;
mod check;
mod cpu;
mod disks;
mod drivetest;
mod memory;
mod monitors;
mod network;
mod plan;
mod run;
mod saved;
mod scan;

use std::io::{self, BufRead, Write};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::args::{Arg, Args};
use crate::logger;
use crate::util::{self, Fail, fail, usage};
use check::Area;
use plan::Options;
use saved::Saved;

const USAGE: &str = "usage: hardware check [AREA...] [--full] [--disk DISK] [--time SECONDS] \
                     [--yes] [--json]\n       hardware last [--json]";

/// Set by SIGINT, SIGTERM or SIGHUP: the check stops, and aborts the
/// self-tests it started.
static STOP: AtomicBool = AtomicBool::new(false);

extern "C" fn on_signal(_: libc::c_int) {
    STOP.store(true, Ordering::Relaxed);
}

/// hardware(8). A run that could not start ends with status 3: 1 is a
/// check that failed.
pub fn main(args: &mut Args) -> Result<i32, Fail> {
    let ended = match args.subcommand() {
        Some(command) if command == "check" => check_command(args),
        Some(command) if command == "last" => last_command(args),
        _ => Err(usage(USAGE.into())),
    };
    ended.map_err(|fail| Fail { code: 3, ..fail })
}

/// The options of `check`.
fn parse(args: &mut Args) -> Result<Options, Fail> {
    let mut options = Options::default();
    while let Some(arg) = args.next()? {
        match arg {
            Arg::Long(name) if name == "full" => options.full = true,
            Arg::Long(name) if name == "yes" => options.yes = true,
            Arg::Short('y') => options.yes = true,
            Arg::Long(name) if name == "json" => options.json = true,
            Arg::Long(name) if name == "disk" => options.disk = Some(args.text()?),
            Arg::Long(name) if name == "time" => options.time = Some(args.number()?),
            Arg::Operand(name) => {
                let name = name.to_string_lossy();
                let area = Area::parse(&name).ok_or_else(|| {
                    let known: Vec<&str> = Area::ALL.iter().map(|area| area.name()).collect();
                    usage(format!("no area named {name}; the areas are {}", known.join(", ")))
                })?;
                if !options.areas.contains(&area) {
                    options.areas.push(area);
                }
            }
            other => return Err(args.unknown(&other)),
        }
    }
    if options.time == Some(0) {
        return Err(usage("--time takes a number of seconds above 0".into()));
    }
    let disks = options.areas.is_empty() || options.areas.contains(&Area::Disks);
    if options.disk.is_some() && !disks {
        return Err(usage("--disk is for the disks area".into()));
    }
    Ok(options)
}

/// Whether the answer to `question` on `input` is yes.
fn confirmed(question: &str, input: &mut dyn BufRead, out: &mut dyn Write) -> bool {
    let _ = write!(out, "{question}");
    let _ = out.flush();
    let mut answer = String::new();
    input.read_line(&mut answer).is_ok()
        && matches!(answer.trim().to_lowercase().as_str(), "y" | "yes")
}

fn handle_signals() {
    for signal in [libc::SIGINT, libc::SIGTERM, libc::SIGHUP] {
        // SAFETY: all zeros is a valid sigaction: numbers, a signal set and a function address.
        let mut action: libc::sigaction = unsafe { util::zeroed() };
        action.sa_sigaction = on_signal as *const () as usize;
        // SAFETY: the handler only stores to an atomic.
        unsafe { libc::sigaction(signal, &raw const action, std::ptr::null_mut()) };
    }
}

/// Keeps the result of a run and logs its line. Neither fails the run:
/// what it found is shown all the same.
fn keep(saved: &Saved) {
    if let Err(error) = saved.save(Path::new(saved::DIR)) {
        eprintln!(
            "hardware: cannot keep the result in {}: {}",
            saved::FILE,
            util::strip_os_error(&error)
        );
    }
    let (level, line) = saved.log_line();
    if let Err(fail) = logger::log("hardware", level, &line) {
        eprintln!("hardware: cannot write to the log: {}", fail.message);
    }
}

/// `hardware check`.
fn check_command(args: &mut Args) -> Result<i32, Fail> {
    let options = parse(args)?;
    if util::own_user() != 0 {
        return Err(fail(
            "hardware check needs an administrator: run elevate hardware check".into(),
        ));
    }
    let plan = plan::gather(&options)?;
    if plan.full && !options.yes {
        let stdin = io::stdin();
        if !confirmed(&plan.question(), &mut stdin.lock(), &mut io::stderr()) {
            return Err(fail("not run: a full check was not confirmed (--yes confirms it)".into()));
        }
    }
    handle_signals();
    if !options.json {
        println!("{}", plan.heading());
    }
    let sink = run::Sink::new(!options.json);
    run::execute(&plan, &STOP, &sink);
    let stopped = STOP.load(Ordering::Relaxed);
    let checks = sink.into_checks();
    let status = check::status(&checks, stopped);
    let saved = Saved {
        time: (util::now_ns() / 1_000_000_000) as i64,
        version: env!("CARGO_PKG_VERSION").to_string(),
        full: plan.full,
        status,
        checks,
    };
    // A run that was stopped did not look at all it was to, and so does
    // not replace the result of the last run that did.
    if !stopped {
        keep(&saved);
    }
    if options.json {
        println!("{}", saved.json()?);
    } else {
        println!("{}", check::summary(&saved.checks));
        if stopped {
            println!("Stopped before the end: this check's result is not kept.");
        }
    }
    Ok(status)
}

/// `hardware last`.
fn last_command(args: &mut Args) -> Result<i32, Fail> {
    let mut json = false;
    while let Some(arg) = args.next()? {
        match arg {
            Arg::Long(name) if name == "json" => json = true,
            other => return Err(args.unknown(&other)),
        }
    }
    let saved = Saved::load(Path::new(saved::FILE))?;
    if json {
        println!("{}", saved.json()?);
    } else {
        println!("{}", saved.heading());
        for check in &saved.checks {
            println!("{}", check.line());
        }
        println!("{}", check::summary(&saved.checks));
    }
    Ok(saved.status)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;

    fn options(items: &[&str]) -> Result<Options, Fail> {
        parse(&mut Args::new("hardware", items.iter().map(OsString::from).collect()))
    }

    #[test]
    fn the_areas_and_the_options_are_read() {
        let all = options(&[]).unwrap();
        assert_eq!(all, Options::default());
        let some =
            options(&["memory", "cpu", "--full", "--time", "20", "--yes", "--json"]).unwrap();
        assert_eq!(some.areas, [Area::Memory, Area::Cpu]);
        assert_eq!((some.full, some.time, some.yes, some.json), (true, Some(20), true, true));
        let disk = options(&["disks", "--disk", "sdb", "-y"]).unwrap();
        assert_eq!((disk.disk.as_deref(), disk.yes), (Some("sdb"), true));
        // An area named twice is checked once.
        assert_eq!(options(&["cpu", "cpu"]).unwrap().areas, [Area::Cpu]);
    }

    #[test]
    fn a_mistake_in_the_command_line_says_what_it_is() {
        let message = |items: &[&str]| options(items).unwrap_err().message;
        assert_eq!(
            message(&["video"]),
            "no area named video; the areas are memory, disks, cpu, sensors, battery, network"
        );
        assert_eq!(message(&["--time", "0"]), "--time takes a number of seconds above 0");
        assert_eq!(message(&["--time", "soon"]), "'soon' is not a valid number");
        assert_eq!(message(&["cpu", "--disk", "sda"]), "--disk is for the disks area");
        assert_eq!(message(&["--fast"]), "unrecognized option '--fast'");
    }

    #[test]
    fn a_run_that_cannot_start_ends_with_status_3_not_the_1_of_a_failed_check() {
        let ended = |items: &[&str]| {
            main(&mut Args::new("hardware", items.iter().map(OsString::from).collect()))
                .map_err(|fail| fail.code)
        };
        assert_eq!(ended(&[]), Err(3));
        assert_eq!(ended(&["jump"]), Err(3));
        assert_eq!(ended(&["check", "video"]), Err(3));
        assert_eq!(ended(&["check", "--time", "soon"]), Err(3));
        assert_eq!(ended(&["last", "--now"]), Err(3));
    }

    #[test]
    fn only_a_yes_confirms_a_full_check() {
        let ask = |answer: &str| {
            let mut out = Vec::new();
            let yes = confirmed("Start it? ", &mut answer.as_bytes(), &mut out);
            assert_eq!(out, b"Start it? ");
            yes
        };
        assert!(ask("y\n"));
        assert!(ask("YES\n"));
        assert!(!ask("n\n"));
        assert!(!ask("\n"));
        assert!(!ask(""));
        assert!(!ask("sure\n"));
    }
}
