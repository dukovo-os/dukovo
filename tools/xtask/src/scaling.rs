// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! `ob bench --scaling`: how opening and closing files scales with the
//! workers doing it at once, on Radix and on Linux in the same virtual
//! machine, with [`CPUS`] CPUs.
//!
//! The POSIX benchmarks' C program (`userspace/posix/bench.c`) runs with
//! `--scaling`: 1, 4 and 8 workers, threads of one process and then
//! processes of their own, open and close a file each, in a directory
//! each, on a tmpfs, all at once. Nothing a worker opens is another's,
//! so what they share is the kernel's own state: on Radix the
//! capability store, which one lock guards; on Linux the descriptor
//! table, which threads share and processes do not. Each row is the
//! time from the moment the workers are let go together until the last
//! of them is done, per open and close (ADR 0416); a kernel that shares
//! nothing keeps it at one worker's time up to [`CPUS`] workers, and at
//! twice that with 8 (two workers a CPU).
//!
//! It is the measurement that decided whether the capability store's
//! one lock was worth splitting. With `--gate` it fails unless Radix
//! takes at most Linux's time on every row ([`bench::GATE_RATIO`]),
//! measuring both again while a row is over, as `ob bench --gate` does
//! (ADR 0107, ADR 0409).

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use crate::bench::{self, Counts, Measurement};
use crate::config::{Firmware, Media, Profile};
use crate::process::{self, step};
use crate::qemu::{self, DirectBoot, Machine, Serial};
use crate::test::{self, Built, TestImage, Vm};
use crate::{Context, Ctx, Result, VERSION, baseline, kernel, userspace};

/// The CPUs of both virtual machines: as many as a CI runner has.
const CPUS: u32 = 4;

/// The workers of each row, as the C program runs them.
const WORKERS: [u32; 3] = [1, 4, 8];

/// Spark's command line: the POSIX benchmarks with `--scaling` only.
const RADIX_CMDLINE: &str = "-- --posix /system/tests/posix/bench --scaling";

/// What linux-bench, as Linux's init, gets after `--` on its command
/// line.
const LINUX_ARGS: &str = "-- --only scaling";

/// The C program's row name for `workers` threads or processes.
fn name(threads: bool, workers: u32) -> String {
    format!("posix_open_close_{}_{workers}", if threads { "threads" } else { "processes" })
}

/// `ob bench --scaling`.
pub fn command(
    ctx: &Ctx,
    profile: Profile,
    runs: u32,
    linux: bool,
    gate: bool,
    given_kernel: Option<String>,
    summary: Option<&str>,
) -> Result<()> {
    qemu::require_qemu()?;
    let built =
        Built { kernel: kernel::build(ctx, profile)?, programs: userspace::build(ctx, profile)? };
    let dir = ctx.out.join("bench-scaling");
    process::create_dir(&dir)?;
    let image = test::build_test_image(ctx, &built, &dir, TestImage::Plain, false)?;
    let kvm = qemu::kvm_available();
    let runs = if kvm { runs } else { 1 };
    if !kvm {
        eprintln!(
            "    No KVM: QEMU emulates every instruction, so the numbers measure the emulator, \
             and Linux's virtual machine does not run. One run only."
        );
    }
    let vm_kernel = if linux && kvm {
        let posix = bench::posix_for_linux(ctx, &dir)?;
        let (kernel, modules) = match bench::linux_kernel(given_kernel)? {
            Some(kernel) => (kernel, None),
            None => {
                let pinned = baseline::fetch(ctx)?;
                (pinned.kernel, Some(pinned.modules))
            }
        };
        let initrd = bench::linux_init(ctx, &dir, Some(&posix), None, &kernel, modules.as_deref())?;
        Some((kernel, initrd))
    } else {
        None
    };
    let (mut radix, mut linux_vm, mut counts) = (Vec::new(), Vec::new(), Vec::new());
    // One run: Radix, then Linux; each row keeps its fastest so far on
    // each side.
    let mut measure = |radix: &mut Vec<Measurement>, linux_vm: &mut Vec<Measurement>, of: &str| {
        step(&format!("Booting Radix with the open/close scaling benchmark on {CPUS} CPUs{of}"));
        let (measured, counted) = boot(ctx, &dir, Side::Radix(&image), kvm)?;
        bench::keep_best(radix, measured);
        // The kernel's counts of the first run: they count the same work
        // every run, unlike the times.
        if counts.is_empty() {
            counts = counted;
        }
        if let Some((kernel, initrd)) = &vm_kernel {
            step(&format!("Measuring the same on Linux on {CPUS} CPUs{of}"));
            let (measured, _) = boot(ctx, &dir, Side::Linux(kernel, initrd), true)?;
            bench::keep_best(linux_vm, measured);
        }
        Ok::<(), crate::Error>(())
    };
    for run in 1..=runs {
        let of = if runs > 1 { format!(", run {run} of {runs}") } else { String::new() };
        measure(&mut radix, &mut linux_vm, &of)?;
    }
    // A row over the gate is measured again, on both sides, up to
    // `MORE_RUNS` more times: a run can only lower each side's fastest
    // time, so what the host slowed is set aside and a real difference
    // stays (ADR 0107).
    let mut measured = runs;
    if gate && !linux_vm.is_empty() {
        while measured < runs + bench::MORE_RUNS {
            let over = over_gate(&radix, &linux_vm);
            if over.is_empty() {
                break;
            }
            step(&format!(
                "Over the gate after {measured} runs: {}. Measuring both again",
                over.join("; ")
            ));
            measured += 1;
            let of = format!(", run {measured} of at most {}", runs + bench::MORE_RUNS);
            measure(&mut radix, &mut linux_vm, &of)?;
        }
    }
    drop(measure);
    let release = match &vm_kernel {
        Some((kernel, _)) => Some(bench::release_of(kernel)?),
        None => None,
    };
    let mut report = report(ctx, &radix, &linux_vm, release.as_deref(), measured, kvm);
    report.push_str(&counts_report(&counts));
    let markdown = dir.join("report.md");
    std::fs::write(&markdown, &report).context(|| format!("writing {}", markdown.display()))?;
    if let Some(path) = summary {
        bench::append(path, &report)?;
    }
    eprintln!("\n{report}");
    step(&format!("Scaling report in {}", ctx.display(&markdown)));
    if gate {
        check_gate(&radix, &linux_vm, measured)?;
        step(
            "Scaling gate passed: Radix opens and closes files as fast as Linux in the same \
             virtual machine, or faster, with every number of workers",
        );
    }
    Ok(())
}

/// Every row's name, threads first, as the report shows them.
fn rows() -> impl Iterator<Item = String> {
    [true, false].into_iter().flat_map(|threads| WORKERS.map(|workers| name(threads, workers)))
}

/// The rows over the gate, each with its ratio (or that it is missing).
fn over_gate(radix: &[Measurement], linux: &[Measurement]) -> Vec<String> {
    let ns = |of: &[Measurement], name: &str| of.iter().find(|m| m.name == name).map(|m| m.ns);
    let mut over = Vec::new();
    for row in rows() {
        match (ns(radix, &row), ns(linux, &row)) {
            (Some(r), Some(l)) if l > 0.0 && r / l <= bench::GATE_RATIO => {}
            (Some(r), Some(l)) if l > 0.0 => {
                over.push(format!("{row} ({} times Linux)", bench::shown(r / l)));
            }
            _ => over.push(format!("{row} (not measured)")),
        }
    }
    over
}

/// Fails unless Linux was measured and every row of Radix's took at most
/// [`bench::GATE_RATIO`] times Linux's time.
fn check_gate(radix: &[Measurement], linux: &[Measurement], runs: u32) -> Result<()> {
    if linux.is_empty() {
        crate::bail!(
            "the scaling gate needs Linux measured in the same virtual machine, under KVM, and \
             that did not run"
        );
    }
    let over = over_gate(radix, linux);
    if !over.is_empty() {
        crate::bail!(
            "the scaling gate failed: Radix is slower than Linux in the same virtual machine \
             after {runs} runs at {}",
            over.join("; ")
        );
    }
    Ok(())
}

/// The system a boot measures.
#[derive(Clone, Copy)]
enum Side<'a> {
    /// Radix, from this test image.
    Radix(&'a [u8]),
    /// Linux: this kernel, with this initramfs (linux-bench and the C
    /// program).
    Linux(&'a Path, &'a Path),
}

/// Boots one side on [`CPUS`] CPUs and reads its `BENCH` and
/// `BENCHCOUNT` lines.
fn boot(ctx: &Ctx, dir: &Path, side: Side, kvm: bool) -> Result<(Vec<Measurement>, Vec<Counts>)> {
    let (image, direct, log, done) = match side {
        Side::Radix(base) => {
            let image = dir.join("scaling.img");
            let bytes = test::with_cmdline(base, RADIX_CMDLINE).map_err(crate::Error::new)?;
            std::fs::write(&image, bytes).context(|| format!("writing {}", image.display()))?;
            (image, None, "scaling-radix", "posix-bench: done")
        }
        Side::Linux(kernel, initrd) => {
            let direct = DirectBoot {
                kernel: kernel.to_path_buf(),
                initrd: initrd.to_path_buf(),
                cmdline: format!("{} {LINUX_ARGS}", bench::LINUX_CMDLINE),
            };
            (PathBuf::new(), Some(direct), "scaling-linux", "linux-bench: done")
        }
    };
    let machine = Machine {
        firmware: Firmware::Bios,
        media: Media::Usb,
        ovmf: None,
        image: image.clone(),
        memory: bench::VM_MEMORY.into(),
        cpus: CPUS,
        kvm,
        cpu_model: None,
        serial: Serial::Pipe,
        display: false,
        reboot: false,
        gdb: false,
        qmp: None,
        extra: Vec::new(),
        direct,
    };
    let mut vm = Vm::start(ctx, &machine, dir, log, bench::TIMEOUT).map_err(crate::Error::new)?;
    let outcome = vm.expect(done);
    let text = vm.log();
    drop(vm);
    if !image.as_os_str().is_empty() {
        let _ = std::fs::remove_file(&image);
    }
    if let Err(reason) = outcome {
        bench::show_trouble(&text);
        crate::bail!(
            "the scaling boot failed: {reason}; serial log: {}",
            ctx.display(&dir.join(format!("boot-{log}.log")))
        );
    }
    let measured = bench::parse(&text);
    if measured.iter().all(|m| !m.name.starts_with("posix_open_close_")) {
        crate::bail!(
            "the scaling boot printed no numbers; serial log: {}",
            ctx.display(&dir.join(format!("boot-{log}.log")))
        );
    }
    Ok((measured, bench::parse_counts(&text)))
}

/// The report: each row's time on both sides, their ratio, and how many
/// times one worker's time each side's is.
fn report(
    ctx: &Ctx,
    radix: &[Measurement],
    linux: &[Measurement],
    release: Option<&str>,
    runs: u32,
    kvm: bool,
) -> String {
    let ns = |of: &[Measurement], name: &str| of.iter().find(|m| m.name == name).map(|m| m.ns);
    let mut out = String::from("## Open and close, workers at once\n\n");
    let _ = writeln!(
        out,
        "Radix {VERSION} at {}, in QEMU with {CPUS} CPUs{}, on {}; Linux {} in the same \
         virtual machine. Each worker opens and closes its own file in its own tmpfs \
         directory; a row is the time from their common start until the last of them is done, \
         per open and close, fastest of {runs} run(s). \"Scaling\" is that time over one \
         worker's: 1.00 up to {CPUS} workers, and 2.00 with 8, if nothing is shared.\n",
        bench::commit(ctx),
        if kvm { ", KVM" } else { ", no KVM (emulated)" },
        bench::host_cpu(),
        release.unwrap_or("(not measured)"),
    );
    out.push_str(
        "| Workers | Radix | Linux | Radix / Linux | Radix scaling | Linux scaling |\n\
         |---|---:|---:|---:|---:|---:|\n",
    );
    for threads in [true, false] {
        let one = (ns(radix, &name(threads, 1)), ns(linux, &name(threads, 1)));
        for workers in WORKERS {
            let row = name(threads, workers);
            let (r, l) = (ns(radix, &row), ns(linux, &row));
            let shown = |v: Option<f64>| v.map_or("–".into(), bench::show);
            let ratio = |a: Option<f64>, b: Option<f64>| match (a, b) {
                (Some(a), Some(b)) if b > 0.0 => format!("{:.2}", a / b),
                _ => "–".into(),
            };
            let _ = writeln!(
                out,
                "| {workers} {} | {} | {} | {} | {} | {} |",
                if threads { "threads" } else { "processes" },
                shown(r),
                shown(l),
                ratio(r, l),
                ratio(r, one.0),
                ratio(l, one.1),
            );
        }
    }
    out
}

/// The events Radix's kernel counted, per open and close, in the order
/// the C program prints them (`scaling_count_names`), with their headings.
const EVENTS: [(&str, &str); 8] = [
    ("switches", "Switches"),
    ("ipis", "IPIs"),
    ("halts", "Halts"),
    ("mutex_contended", "Mutex found taken"),
    ("mutex_waits", "Mutex sleeps"),
    ("mutex_handoffs", "Mutex handoffs"),
    ("mutex_wakes", "Mutex let-gos"),
    ("cap_cache_flushes", "Capability cache flushes"),
];

/// A second table: what Radix's kernel did per open and close in each row,
/// which shows whether workers queue for a lock and sleep while they wait.
/// Empty when the boot printed no counts.
fn counts_report(counts: &[Counts]) -> String {
    if counts.is_empty() {
        return String::new();
    }
    let mut out =
        String::from("\nRadix's kernel events per open and close (first run):\n\n| Workers |");
    for (_, heading) in EVENTS {
        let _ = write!(out, " {heading} |");
    }
    out.push_str("\n|---|");
    out.push_str(&"---:|".repeat(EVENTS.len()));
    out.push('\n');
    for threads in [true, false] {
        for workers in WORKERS {
            let Some(row) = counts.iter().find(|c| c.group == name(threads, workers)) else {
                continue;
            };
            let _ = write!(out, "| {workers} {} |", if threads { "threads" } else { "processes" });
            for (event, _) in EVENTS {
                match row.events.iter().find(|(name, _)| name == event) {
                    Some((_, count)) if row.rounds > 0 => {
                        let _ = write!(out, " {:.3} |", *count as f64 / row.rounds as f64);
                    }
                    _ => out.push_str(" – |"),
                }
            }
            out.push('\n');
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rows_name_what_the_c_program_prints() {
        let source = include_str!("../../../userspace/posix/bench.c");
        assert!(source.contains("posix_open_close_%s_%d"));
        assert!(source.contains("\"threads\" : \"processes\""));
        assert!(source.contains("{1, 4, 8}"));
        assert_eq!(name(true, 4), "posix_open_close_threads_4");
        assert_eq!(name(false, 8), "posix_open_close_processes_8");
    }

    #[test]
    fn counts_name_what_the_c_program_counts() {
        let source = include_str!("../../../userspace/posix/bench.c");
        for (event, _) in EVENTS {
            assert!(source.contains(&format!("\"{event}\"")), "bench.c does not count {event}");
        }
    }

    #[test]
    fn counts_show_per_open_and_close() {
        let counts = bench::parse_counts(
            "BENCHCOUNT group=posix_open_close_threads_4 rounds=200 switches=50 ipis=0 halts=0 \
             mutex_contended=150 mutex_waits=100 mutex_handoffs=40 mutex_wakes=60 \
             cap_cache_flushes=0",
        );
        let report = counts_report(&counts);
        assert!(report.contains(
            "| 4 threads | 0.250 | 0.000 | 0.000 | 0.750 | 0.500 | 0.200 | 0.300 | 0.000 |"
        ));
        assert!(counts_report(&[]).is_empty());
    }

    /// Every row at `ns`, as the C program prints it, but for the rows
    /// `change` gives another time or (with `None`) leaves out.
    fn measured(ns: f64, change: &[(usize, Option<f64>)]) -> Vec<Measurement> {
        let mut text = String::new();
        for (at, row) in rows().enumerate() {
            let time = change.iter().find(|(which, _)| *which == at).map_or(Some(ns), |c| c.1);
            if let Some(time) = time {
                let _ = writeln!(text, "BENCH name={row} ns={time} iterations=1000");
            }
        }
        bench::parse(&text)
    }

    #[test]
    fn the_gate_holds_every_row_to_linux() {
        let linux = measured(600.0, &[]);
        // Level with Linux passes.
        assert!(over_gate(&measured(600.0, &[]), &linux).is_empty());
        assert!(check_gate(&measured(600.0, &[]), &linux, 3).is_ok());
        // One row over fails, and says by how much: the fourth row is
        // one process.
        let slow = measured(590.0, &[(3, Some(606.0))]);
        assert_eq!(over_gate(&slow, &linux), ["posix_open_close_processes_1 (1.01 times Linux)"]);
        assert!(check_gate(&slow, &linux, 9).is_err());
        // A row Radix did not print is over too.
        let missing = measured(500.0, &[(0, None)]);
        assert_eq!(over_gate(&missing, &linux), ["posix_open_close_threads_1 (not measured)"]);
        // No Linux, no gate.
        assert!(check_gate(&linux, &[], 3).is_err());
    }

    #[test]
    fn linux_bench_takes_the_scaling_option() {
        let source = include_str!("../../linux-bench/src/main.rs");
        assert!(source.contains("\"--only\", Some(what)) if what == \"scaling\""));
        assert!(LINUX_ARGS.ends_with("--only scaling"));
    }
}
