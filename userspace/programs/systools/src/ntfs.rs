// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! resize-ntfs(8), and ntfsresize (ADR 0312): an NTFS volume checked and
//! shrunk by the kernel's own NTFS crate, through the disk engine, with
//! the options of ntfs-3g's ntfsresize, by whose name it runs too.
//!
//! It shrinks the volume alone, as ntfsresize does, and says the size to
//! shrink the partition to after, which rdisk(8) does. It does not grow
//! a volume. The installer shrinks Windows' volume with the same engine.

use std::ffi::OsString;
use std::io::{self, IsTerminal, Write};

use disk::Device;
use disk::ntfs::{self, Info, Options, Refused, Report, Step};

use crate::args::{Arg, Args};
use crate::blockdev;
use crate::util::{self, Fail};

/// A million bytes: ntfsresize's MB, in which it prints sizes.
const MB: u64 = 1_000_000;

/// What resize-ntfs is asked to do.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Asked {
    /// Shrink the volume to `--size`.
    Shrink,
    /// Say its sizes and the smallest it shrinks to (`--info`).
    Info,
    /// Say only the smallest, in MB, for scripts (`--info-mb-only`).
    InfoMb,
    /// Check it, and say what is wrong (`--check`).
    Check,
}

/// resize-ntfs(8), and ntfsresize.
pub fn resize_ntfs(args: &mut Args) -> Result<i32, Fail> {
    let mut asked = Asked::Shrink;
    let (mut size, mut dry_run, mut windows_check) = (None, false, true);
    let (mut bar, mut force) = (true, false);
    let mut operands: Vec<OsString> = Vec::new();
    while let Some(arg) = args.next()? {
        match arg {
            Arg::Short('i') => asked = Asked::Info,
            Arg::Long(l) if l == "info" => asked = Asked::Info,
            Arg::Short('m') => asked = Asked::InfoMb,
            Arg::Long(l) if l == "info-mb-only" => asked = Asked::InfoMb,
            Arg::Short('c') => asked = Asked::Check,
            Arg::Long(l) if l == "check" => asked = Asked::Check,
            Arg::Short('s') => size = Some(size_of(&args.text()?)?),
            Arg::Long(l) if l == "size" => size = Some(size_of(&args.text()?)?),
            Arg::Short('n') => dry_run = true,
            Arg::Long(l) if l == "no-action" => dry_run = true,
            Arg::Long(l) if l == "no-windows-check" => windows_check = false,
            Arg::Short('P') => bar = false,
            Arg::Long(l) if l == "no-progress-bar" => bar = false,
            Arg::Short('f') => force = true,
            Arg::Long(l) if l == "force" => force = true,
            // Bad clusters are always left out of a shrunk volume, and
            // what it says is all it says.
            Arg::Short('b' | 'v') => {}
            Arg::Long(l) if l == "bad-sectors" || l == "verbose" => {}
            Arg::Short('x') => return Err(not_grown()),
            Arg::Long(l) if l == "expand" => return Err(not_grown()),
            Arg::Short('V') => return Ok(version()),
            Arg::Long(l) if l == "version" => return Ok(version()),
            Arg::Short('h') => return Ok(help()),
            Arg::Long(l) if l == "help" => return Ok(help()),
            Arg::Operand(operand) => operands.push(operand),
            other => return Err(args.unknown(&other)),
        }
    }
    let [path] = operands.as_slice() else {
        return Err(util::usage("say the one disk, partition or file the volume is on".into()));
    };
    if asked == Asked::Shrink && size.is_none() {
        return Err(util::usage(
            "say the size to shrink to with --size; --info says the smallest".into(),
        ));
    }
    let write = asked == Asked::Shrink && !dry_run;
    let opened = blockdev::open(path, write, None)?;
    if write && !opened.writable {
        return Err(util::fail(format!("{} cannot be written", opened.shown)));
    }
    // A volume in use is read as it is written: --force reads it all the
    // same, and nothing shrinks one.
    if opened.uses() != 0 && (write || !force) {
        let name = opened.name.as_deref().unwrap_or(&opened.shown);
        let unless = if write { "" } else { ", or --force reads it as it is" };
        return Err(util::fail(format!(
            "{}; unmount it first{unless}",
            blockdev::in_use(name, opened.uses())
        )));
    }
    let device = opened.device().map_err(|e| util::failed(&opened.shown, e))?;
    let shown = opened.shown.as_str();
    match (asked, size) {
        (Asked::Check, _) => check(shown, &device),
        (Asked::Info, _) => info(shown, &device),
        (Asked::InfoMb, _) => info_mb(shown, &device),
        (Asked::Shrink, Some(bytes)) => {
            let options = Options { bytes, dry_run, check_at_windows_start: windows_check };
            shrink(shown, &device, options, bar)
        }
        (Asked::Shrink, None) => unreachable!("a shrink without a size was refused above"),
    }
}

/// `--check`: what the check finds.
fn check(shown: &str, device: &Device) -> Result<i32, Fail> {
    println!("Checking filesystem consistency ...");
    let report = ntfs::check(device).map_err(|why| util::fail(format!("{shown}: {why}")))?;
    let cs = u64::from(report.cluster_size);
    println!("Files' records in use: {}", report.records);
    println!("Space in use       : {} MB", ((report.held - report.bad) * cs).div_ceil(MB));
    if report.lost > 0 {
        println!("Lost clusters      : {}, which Windows' chkdsk gives back", report.lost);
    }
    if report.bad > 0 {
        println!("Bad clusters       : {}, which no file uses", report.bad);
    }
    problems(&report)
}

/// The problems `report` lists, and an exit status of 1 if it lists
/// any.
fn problems(report: &Report) -> Result<i32, Fail> {
    for problem in &report.problems {
        println!("ERROR: {problem}");
    }
    if report.sound() {
        println!("No problems found.");
        return Ok(0);
    }
    if report.count > report.problems.len() as u64 {
        println!("... and {} more", report.count - report.problems.len() as u64);
    }
    Err(util::fail(format!(
        "{} problems found; let Windows mend the volume (chkdsk /f) first",
        report.count
    )))
}

/// `--info`: the volume's sizes, and the smallest it shrinks to.
fn info(shown: &str, device: &Device) -> Result<i32, Fail> {
    let info = info_of(shown, device)?;
    let report = &info.report;
    let cs = u64::from(report.cluster_size);
    let used = (report.held - report.bad) * cs;
    println!("Device name        : {shown}");
    println!("Cluster size       : {cs} bytes");
    println!("Current volume size: {} bytes ({} MB)", info.bytes, info.bytes.div_ceil(MB));
    println!("Current device size: {} bytes ({} MB)", device.len(), device.len().div_ceil(MB));
    println!("Checking filesystem consistency ...");
    if !report.sound() {
        return problems(report);
    }
    let percent = used as f64 * 100.0 / (report.clusters * cs) as f64;
    println!("Space in use       : {} MB ({percent:.1}%)", used.div_ceil(MB));
    if let Some(why) = info.kept {
        return Err(util::fail(format!("{shown} cannot be shrunk now: {why}")));
    }
    println!(
        "You might resize at {} bytes or {} MB (freeing {} MB).",
        info.smallest,
        info.smallest.div_ceil(MB),
        info.bytes.saturating_sub(info.smallest) / MB
    );
    Ok(0)
}

/// `--info-mb-only`: the smallest it shrinks to, in MB, as ntfsresize
/// says it.
fn info_mb(shown: &str, device: &Device) -> Result<i32, Fail> {
    let info = info_of(shown, device)?;
    if !info.report.sound() {
        return problems(&info.report);
    }
    if let Some(why) = info.kept {
        return Err(util::fail(format!("{shown} cannot be shrunk now: {why}")));
    }
    println!("Minsize (in MB): {}", info.smallest.div_ceil(MB));
    Ok(0)
}

/// What the volume on `device` is, for a shrink.
fn info_of(shown: &str, device: &Device) -> Result<Info, Fail> {
    ntfs::info(device).map_err(|why| util::fail(format!("{shown}: {why}")))
}

/// Shrinks the volume on `device` as `options` say, showing how far it
/// is if `bar`.
fn shrink(shown: &str, device: &Device, options: Options, bar: bool) -> Result<i32, Fail> {
    let mut progress = Progress { bar: bar && io::stdout().is_terminal(), percent: None };
    let shrunk = ntfs::shrink(device, options, &mut |step| progress.step(step));
    progress.end();
    let shrunk = match shrunk {
        Ok(shrunk) => shrunk,
        Err(Refused::Unsound(report)) => return problems(&report),
        Err(why) => return Err(util::fail(format!("{shown}: {why}"))),
    };
    let moved = format!("{} MB of files' data", shrunk.moved.div_ceil(MB));
    let sectors = shrunk.bytes / u64::from(device.sector_size());
    if options.dry_run {
        println!("A shrink would move {moved}, and rewrite {} records.", shrunk.records);
        println!("The test run wrote nothing.");
        return Ok(0);
    }
    if !shrunk.report.sound() {
        return problems(&shrunk.report);
    }
    println!("Moved {moved} below the new end, and rewrote {} records.", shrunk.records);
    println!("Successfully resized NTFS on {shown}: it is {} bytes now.", shrunk.bytes);
    println!(
        "Shrink its partition to {sectors} sectors ({} bytes), starting where it starts, with \
         rdisk(8): the volume leaves the rest unused until then.",
        shrunk.bytes
    );
    if options.check_at_windows_start {
        println!("Windows checks the volume at its next start, as after ntfsresize.");
    }
    Ok(0)
}

/// What a shrink says as it goes.
struct Progress {
    /// Whether to show how far the copying is, on a terminal.
    bar: bool,
    /// The percent last shown.
    percent: Option<u64>,
}

impl Progress {
    fn step(&mut self, step: Step) {
        match step {
            Step::Checking => println!("Checking filesystem consistency ..."),
            Step::Moving { done, total } => {
                if self.percent.is_none() {
                    println!("Relocating needed data ...");
                }
                // Nothing to copy is all of it copied.
                let percent = (done * 100).checked_div(total).unwrap_or(100);
                if self.bar && self.percent != Some(percent) {
                    print!("\r{percent:3} percent completed");
                    let _ = io::stdout().flush();
                }
                self.percent = Some(percent);
            }
            Step::Rechecking => {
                self.end();
                println!("Checking the shrunk volume ...");
            }
        }
    }

    /// Ends the line the copying's percent is on.
    fn end(&mut self) {
        if self.bar && self.percent.take().is_some() {
            println!();
        }
    }
}

/// `--size`'s size as ntfsresize reads it: bytes, or with k, M, G or T,
/// powers of 1000; with Ki, Mi, Gi or Ti, and a B after or not, powers
/// of 1024.
fn size_of(text: &str) -> Result<u64, Fail> {
    let digits = text.find(|c: char| !c.is_ascii_digit()).unwrap_or(text.len());
    let (number, suffix) = text.split_at(digits);
    let (base, power) = match suffix.strip_suffix("iB").or_else(|| suffix.strip_suffix('i')) {
        Some(binary) => (1024u64, binary),
        None => (1000u64, suffix),
    };
    let power = match power {
        "" if base == 1000 => Some(0),
        "k" | "K" => Some(1),
        "M" => Some(2),
        "G" => Some(3),
        "T" => Some(4),
        _ => None,
    };
    number
        .parse::<u64>()
        .ok()
        .zip(power)
        .and_then(|(n, power)| n.checked_mul(base.checked_pow(power)?))
        .ok_or_else(|| util::usage(format!("'{text}' is not a size")))
}

/// What `--expand` gets.
fn not_grown() -> Fail {
    util::fail("resize-ntfs only shrinks a volume; it does not grow one".into())
}

fn version() -> i32 {
    println!("resize-ntfs, Dukovo systools {}", env!("CARGO_PKG_VERSION"));
    0
}

fn help() -> i32 {
    println!(
        "usage: resize-ntfs [--info | --info-mb-only | --check] [--force] DEVICE\n       \
         resize-ntfs --size SIZE [--no-action] [--no-windows-check] [--no-progress-bar] DEVICE"
    );
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_read_as_ntfsresize_reads_them() {
        assert_eq!(size_of("512").unwrap(), 512);
        assert_eq!(size_of("20G").unwrap(), 20_000_000_000);
        assert_eq!(size_of("300k").unwrap(), 300_000);
        assert_eq!(size_of("300M").unwrap(), 300_000_000);
        assert_eq!(size_of("20Gi").unwrap(), 20 << 30);
        assert_eq!(size_of("64MiB").unwrap(), 64 << 20);
        assert!(size_of("20GB").is_err());
        assert!(size_of("i").is_err());
        assert!(size_of("M").is_err());
        assert!(size_of("99999999999T").is_err());
    }
}
