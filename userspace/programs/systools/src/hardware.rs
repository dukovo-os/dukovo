// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! hardware(8): `hardware check` looks for hardware that is failing,
//! `hardware last` shows what the last check found (ADR 0488), `hardware
//! history` lists the checks that were kept, `hardware show` says what
//! the machine is, `hardware power` its batteries and UPSes (ADR 0741),
//! `hardware screens` its graphics cards and screens, `hardware inputs`
//! its keyboards, mice and other input devices (ADR 0706), and
//! `hardware memtest` asks for the memory to be tested at the next
//! start.
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
//! `/system/state/hardware/last.json`; `history` the line of each kept
//! result, in `/system/state/hardware/history`; `show` what the machine
//! is; `sensors` and `devices` the temperatures and the devices it has;
//! `memtest` the memory test at the next start; one module for each area;
//! and `drivetest` and `scan` for a disk's self-test and read scan.

mod battery;
pub(crate) mod check;
mod cpu;
mod devices;
mod disks;
mod drivetest;
mod history;
mod inputs;
mod memory;
pub(crate) mod memtest;
mod monitors;
mod network;
mod plan;
mod power;
mod run;
pub(crate) mod saved;
mod scan;
mod screens;
mod sensors;
mod show;
mod words;

use std::io::{self, BufRead, Write};
use std::path::Path;

use sysapi::signal::{self, Caught};

use crate::admin_about::Sources;
use crate::args::{Arg, Args};
use crate::util::{self, Fail, fail, usage};
use crate::{hwinfo, logger, lscpu};
use check::Area;
use plan::Options;
use run::Steps;
use saved::Saved;

const USAGE: &str = "usage: hardware check [AREA...] [--full] [--disk DISK] [--time SECONDS] \
                     [--yes] [--progress] [--json]\n       hardware last [--json]\n       \
                     hardware history [--json]\n       hardware show [--json]\n       hardware \
                     sensors [--json]\n       hardware devices [--json]\n       hardware power [--json]\n       hardware screens \
                     [--json]\n       hardware inputs [--json]\n       hardware memtest --next-start | --cancel | --status [--json]";

/// Set by SIGINT, SIGTERM or SIGHUP: the check stops, and aborts the
/// self-tests it started.
static STOP: Caught = Caught::new();

/// hardware(8). A run that could not start ends with status 3: 1 is a
/// check that failed.
pub fn main(args: &mut Args) -> Result<i32, Fail> {
    let ended = match args.subcommand() {
        Some(command) if command == "check" => check_command(args),
        Some(command) if command == "last" => last_command(args),
        Some(command) if command == "history" => history_command(args),
        Some(command) if command == "show" => show_command(args),
        Some(command) if command == "sensors" => sensors_command(args),
        Some(command) if command == "devices" => devices_command(args),
        Some(command) if command == "power" => power_command(args),
        Some(command) if command == "screens" => screens_command(args),
        Some(command) if command == "inputs" => inputs_command(args),
        // A refusal of memtest is a status 1 of its own, as `changes bisect`'s.
        Some(command) if command == "memtest" => {
            return memtest::main(args)
                .map_err(|fail| if fail.usage { Fail { code: 3, ..fail } } else { fail });
        }
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
            Arg::Long(name) if name == "progress" => options.progress = true,
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

/// Has SIGINT, SIGTERM and SIGHUP set [`STOP`].
fn handle_signals() {
    for number in [libc::SIGINT, libc::SIGTERM, libc::SIGHUP] {
        let _ = signal::catch(number, &STOP, 0);
    }
}

/// Keeps the result of a run, adds its line to the history and logs its
/// line. None fails the run: what it found is shown all the same.
fn keep(saved: &Saved) {
    let dir = Path::new(saved::DIR);
    if let Err(error) = saved.save(dir) {
        eprintln!(
            "hardware: cannot keep the result in {}: {}",
            saved::FILE,
            util::strip_os_error(&error)
        );
    }
    if let Err(error) = history::append(dir, &history::Line::of(saved)) {
        eprintln!(
            "hardware: cannot add the result to {}: {}",
            history::FILE,
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
    // The steps go to standard error beside a JSON result, which keeps
    // standard output to the JSON.
    let steps = match (options.progress, options.json) {
        (false, _) => Steps::Off,
        (true, false) => Steps::Output,
        (true, true) => Steps::Errors,
    };
    let sink = run::Sink::new(!options.json, steps);
    run::execute(&plan, STOP.flag(), &sink);
    let stopped = STOP.is_set();
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

/// The command line of a command that takes `--json` and nothing else:
/// whether it was given.
fn json_only(args: &mut Args) -> Result<bool, Fail> {
    let mut json = false;
    while let Some(arg) = args.next()? {
        match arg {
            Arg::Long(name) if name == "json" => json = true,
            other => return Err(args.unknown(&other)),
        }
    }
    Ok(json)
}

/// `hardware last`.
fn last_command(args: &mut Args) -> Result<i32, Fail> {
    let json = json_only(args)?;
    last(Path::new(saved::FILE), json, &mut io::stdout().lock())
}

/// Shows the result in `file` on `out`, and ends with its status. With
/// nothing kept it says so as an error, which ends with status 3 and says
/// it on standard error; its JSON is then `null` on `out`.
fn last(file: &Path, json: bool, out: &mut dyn Write) -> Result<i32, Fail> {
    let Some(saved) = Saved::read(file)? else {
        if json {
            writeln!(out, "null")?;
        }
        return Err(fail(saved::NONE.to_string()));
    };
    if json {
        writeln!(out, "{}", saved.json()?)?;
    } else {
        writeln!(out, "{}", saved.heading())?;
        for check in &saved.checks {
            writeln!(out, "{}", check.line())?;
        }
        writeln!(out, "{}", check::summary(&saved.checks))?;
    }
    Ok(saved.status)
}

/// `hardware history`.
fn history_command(args: &mut Args) -> Result<i32, Fail> {
    let json = json_only(args)?;
    list(Path::new(history::FILE), json, &mut io::stdout().lock())
}

/// Shows the history in `file` on `out`: the JSON array of its lines, or a
/// line of text for each.
fn list(file: &Path, json: bool, out: &mut dyn Write) -> Result<i32, Fail> {
    let lines = history::read(file)?;
    if json {
        let text = serde_json::to_string(&lines)
            .map_err(|error| fail(format!("cannot write JSON: {error}")))?;
        writeln!(out, "{text}")?;
    } else if lines.is_empty() {
        writeln!(out, "No check has kept a result yet.")?;
    } else {
        for line in &lines {
            writeln!(out, "{}", line.text())?;
        }
    }
    Ok(0)
}

/// `hardware show`. What the tables say of the memory is told when this
/// program holds the platform capability that reads them whole, and is
/// `null` when it does not.
fn show_command(args: &mut Args) -> Result<i32, Fail> {
    let json = json_only(args)?;
    let tables = hwinfo::read_tables().ok();
    let show = show::gather(&Sources::machine(), &lscpu::summary(&lscpu::cpuid), tables.as_ref());
    let mut out = io::stdout().lock();
    if json {
        let text = show.json().map_err(|error| fail(format!("cannot write JSON: {error}")))?;
        writeln!(out, "{text}")?;
    } else {
        write!(out, "{}", show.text())?;
    }
    Ok(0)
}

/// `hardware sensors`: what the monitors, the thermal zones and the
/// disks' last readings say, which is nothing on a machine with none.
fn sensors_command(args: &mut Args) -> Result<i32, Fail> {
    let json = json_only(args)?;
    let found = sensors::gather(&sensors::Sources::system());
    let mut out = io::stdout().lock();
    if json {
        let text = found.json().map_err(|error| fail(format!("cannot write JSON: {error}")))?;
        writeln!(out, "{text}")?;
    } else {
        write!(out, "{}", found.text(util::now_ns() / 1_000_000))?;
    }
    Ok(0)
}

/// `hardware devices`: the PCI functions and USB devices, and the drivers
/// that took them where the system says.
fn devices_command(args: &mut Args) -> Result<i32, Fail> {
    let json = json_only(args)?;
    let found = devices::gather(&devices::Sources::system());
    let mut out = io::stdout().lock();
    if json {
        let text = found.json().map_err(|error| fail(format!("cannot write JSON: {error}")))?;
        writeln!(out, "{text}")?;
    } else {
        write!(out, "{}", found.text())?;
    }
    Ok(0)
}

/// `hardware power`: the batteries, adapters and UPSes, and when a UPS
/// shuts the machine down.
fn power_command(args: &mut Args) -> Result<i32, Fail> {
    let json = json_only(args)?;
    let found = power::system();
    let mut out = io::stdout().lock();
    if json {
        let text = found.json().map_err(|error| fail(format!("cannot write JSON: {error}")))?;
        writeln!(out, "{text}")?;
    } else {
        write!(out, "{}", found.text())?;
    }
    Ok(0)
}

/// `hardware screens`: the graphics cards the system draws through and
/// their screens, as the kernel's display layer has them, and which
/// program is in front.
fn screens_command(args: &mut Args) -> Result<i32, Fail> {
    let json = json_only(args)?;
    let found = screens::gather(&screens::Sources::system());
    let mut out = io::stdout().lock();
    if json {
        let text = found.json().map_err(|error| fail(format!("cannot write JSON: {error}")))?;
        writeln!(out, "{text}")?;
    } else {
        write!(out, "{}", found.text())?;
    }
    Ok(0)
}

/// `hardware inputs`: the keyboards, mice and other input devices the
/// kernel reads, as its input hub has them.
fn inputs_command(args: &mut Args) -> Result<i32, Fail> {
    let json = json_only(args)?;
    let found = inputs::gather(Path::new(inputs::STATE));
    let mut out = io::stdout().lock();
    if json {
        let text = found.json().map_err(|error| fail(format!("cannot write JSON: {error}")))?;
        writeln!(out, "{text}")?;
    } else {
        write!(out, "{}", found.text())?;
    }
    Ok(0)
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
            options(&["memory", "cpu", "--full", "--time", "20", "--yes", "--json", "--progress"])
                .unwrap();
        assert_eq!(some.areas, [Area::Memory, Area::Cpu]);
        assert_eq!((some.full, some.time, some.yes, some.json), (true, Some(20), true, true));
        assert!(some.progress && !all.progress);
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
        assert_eq!(ended(&["history", "--now"]), Err(3));
        assert_eq!(ended(&["show", "--now"]), Err(3));
        assert_eq!(ended(&["show", "memory"]), Err(3));
        assert_eq!(ended(&["sensors", "--now"]), Err(3));
        assert_eq!(ended(&["devices", "pci"]), Err(3));
        assert_eq!(ended(&["screens", "virtio0"]), Err(3));
        assert_eq!(ended(&["inputs", "0"]), Err(3));
    }

    fn folder(call: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("hardware-main-{}-{call}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// What `last` prints for `file`, and how it ends.
    fn shown_last(file: &Path, json: bool) -> (Result<i32, String>, String) {
        let mut out = Vec::new();
        let ended = last(file, json, &mut out).map_err(|fail| fail.message);
        (ended, String::from_utf8(out).unwrap())
    }

    #[test]
    fn last_with_nothing_kept_prints_null_for_json_and_ends_with_the_words_and_status_3() {
        let dir = folder("last-none");
        let file = dir.join("last.json");
        let (ended, out) = shown_last(&file, true);
        assert_eq!(ended, Err("no check has run yet: run elevate hardware check".to_string()));
        assert_eq!(out, "null\n");
        let (ended, out) = shown_last(&file, false);
        assert_eq!(ended, Err("no check has run yet: run elevate hardware check".to_string()));
        assert_eq!(out, "");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn last_with_a_result_kept_prints_it_and_ends_with_its_status() {
        let dir = folder("last-kept");
        let kept = Saved {
            time: 1_794_000_000,
            version: "0.16.0".into(),
            full: true,
            status: 2,
            checks: vec![check::Check::warning(
                Area::Disks,
                "sda health",
                check::Problem::DiskEarlyWarning,
                "3 media errors".into(),
            )],
        };
        kept.save(&dir).unwrap();
        let file = dir.join("last.json");
        let (ended, out) = shown_last(&file, true);
        assert_eq!(ended, Ok(2));
        let value: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(
            (&value["time"], &value["full"], &value["status"]),
            (&1_794_000_000.into(), &true.into(), &2.into())
        );
        let (ended, out) = shown_last(&file, false);
        assert_eq!(ended, Ok(2));
        assert!(out.contains("A full check ended "), "{out}");
        assert!(out.contains("sda health"), "{out}");
        assert!(out.ends_with("0 passed, 1 warning, 0 FAILED, 0 skipped\n"), "{out}");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn history_with_nothing_kept_is_an_empty_array_or_a_sentence_and_not_an_error() {
        let dir = folder("history-none");
        let file = dir.join("history");
        let mut out = Vec::new();
        assert_eq!(list(&file, true, &mut out).unwrap(), 0);
        assert_eq!(String::from_utf8(out).unwrap(), "[]\n");
        let mut out = Vec::new();
        assert_eq!(list(&file, false, &mut out).unwrap(), 0);
        assert_eq!(String::from_utf8(out).unwrap(), "No check has kept a result yet.\n");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn history_lists_the_kept_lines_oldest_first_as_an_array_or_a_line_each() {
        let dir = folder("history-kept");
        let run = |time: i64, status: i32| Saved {
            time,
            version: "0.16.0".into(),
            full: false,
            status,
            checks: vec![check::Check::passed(Area::Cpu, "cpu load", String::new())],
        };
        for (time, status) in [(1_794_000_000, 0), (1_794_086_400, 3)] {
            history::append(&dir, &history::Line::of(&run(time, status))).unwrap();
        }
        let file = dir.join("history");
        let mut out = Vec::new();
        assert_eq!(list(&file, true, &mut out).unwrap(), 0);
        let value: serde_json::Value = serde_json::from_slice(&out).unwrap();
        let at: Vec<i64> =
            value.as_array().unwrap().iter().map(|line| line["at_ms"].as_i64().unwrap()).collect();
        assert_eq!(at, [1_794_000_000_000, 1_794_086_400_000]);
        assert_eq!(value[1]["status"], 3);
        assert_eq!(value[0]["ids"], serde_json::json!([]));
        let mut out = Vec::new();
        assert_eq!(list(&file, false, &mut out).unwrap(), 0);
        let text = String::from_utf8(out).unwrap();
        assert_eq!(text.lines().count(), 2);
        assert!(text.lines().all(|line| line.contains("  quick  1 passed, 0 warnings")), "{text}");
        let _ = std::fs::remove_dir_all(dir);
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
