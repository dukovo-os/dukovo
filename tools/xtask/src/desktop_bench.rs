// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! `ob bench --desktop`: what the desktop's everyday work costs, beside
//! a Linux desktop in the same virtual machine (ADR 0724).
//!
//! Both systems run one program, `window-test cost`
//! (userspace/programs/window-test/src/cost.rs): Dukovo in `desktop`
//! (windowd), Linux in labwc with wlroots' pixman renderer
//! ([`crate::linux_desktop`], `linux-bench --only desktop`). The machine
//! has one screen, a virtio graphics card's, and a virtio keyboard and
//! mouse, and each desktop draws on the processor.
//!
//! The program shows one window and says in the log which step it is
//! at; this module sends the keys and the mouse's motions through QEMU
//! and measures:
//!
//! | Row | What | Measured |
//! |---|---|---|
//! | `desktop_idle` | a second of the desktop with nothing moving | host |
//! | `desktop_frame` | one frame of the window drawn anew | host |
//! | `desktop_key`, `desktop_key_p99` | a key, from the kernel's driver to the program | guest |
//! | `desktop_move` | one motion of the mouse that drags the window | host |
//! | `desktop_resize` | one size of the window while the mouse resizes it | host |
//!
//! A row measured from the host is the time the host's processors ran
//! the machine's CPUs for the step ([`Cpus::ran_ns`]), less what
//! they run in as long with nothing moving, by the step's count: all a
//! frame costs, in the desktop, the program and the kernel, whichever
//! of them does the work, by one clock for both systems. The key rows
//! are timed by the program.
//!
//! The rows are a look and gate nothing; `ob bench` without `--desktop`
//! does not measure them.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::bench::{self, Measurement};
use crate::config::{Firmware, Media};
use crate::process::{self, step};
use crate::qemu::{self, DirectBoot, Machine, Serial};
use crate::test::{self, Vm};
use crate::{Context, Ctx, Result, VERSION, linux_desktop, pin, userspace};

/// The machine's screen, keyboard and mouse, the same for both systems.
/// The keyboard and the mouse are bound to the screen, which is how QEMU
/// is told to send an event to them and not to the PS/2 ones.
const DEVICES: [&str; 8] = [
    "-vga",
    "none",
    "-device",
    "virtio-gpu-pci,id=video1,xres=1024,yres=768",
    "-device",
    "virtio-keyboard-pci,display=video1",
    "-device",
    "virtio-mouse-pci,display=video1",
];
const SCREEN: &str = "video1";

/// Radix's command line: Spark starts the services, and `window-test`
/// runs the desktop with itself in it.
const RADIX_CMDLINE: &str = "console-sign-in=off -- --services --posix \
                             /system/tests/posix/window-test in-desktop cost";

/// What Linux's init, `linux-bench`, is given after its kernel's command
/// line.
const LINUX_ARGS: &str = "-- --only desktop";

/// The compositor Linux's side runs, for the report.
const LINUX_DESKTOP: &str = "labwc 0.7.1 on wlroots 0.17.1, drawing with pixman";

/// The program's counts (`window-test`'s `cost.rs`, checked by a test).
const FRAMES: u32 = 600;
const KEYS: u32 = 200;

/// The motions of the move row and of the resize row: strokes one way
/// and back, so that the window stays where and near the size it was
/// whatever each desktop makes of a motion's length.
const STROKES: u32 = 10;
const STROKE: u32 = 10;

/// What one motion of each row's strokes is, in the mouse's units: far
/// enough that both desktops move the pointer by a pixel or more.
const MOVE_BY: (i32, i32) = (4, 0);
const RESIZE_BY: (i32, i32) = (2, 2);

/// Between two motions: slower than either desktop draws its frames, so
/// that each motion is drawn by itself.
const MOTION_PAUSE: Duration = Duration::from_millis(20);

/// Between two changes of a key.
const KEY_PAUSE: Duration = Duration::from_millis(5);

/// How long the desktop is left alone before a measurement starts and
/// after its last event, to finish what it was at.
const SETTLE: Duration = Duration::from_millis(500);

/// How long the machine is left alone before it is watched with nothing
/// moving: the desktop shows while the last services still start, and
/// what they run is not the desktop at rest.
const REST_SETTLE: Duration = Duration::from_secs(3);

/// How long the desktop is watched with nothing moving.
const IDLE_FOR: Duration = Duration::from_secs(3);

/// How long the pointer may take to come onto the window.
const POINTER_PATIENCE: Duration = Duration::from_mins(2);

/// What the program, and Linux's init, say when they end early.
const FAILED: &str = "window-test: FAILED";
const LINUX_ENDED: &str = "linux-bench: done";

/// The rows, in the report's order: the name, and what it is.
const ROWS: [(&str, &str); 6] = [
    ("desktop_idle", "A second with nothing moving (processor time)"),
    ("desktop_frame", "One frame of the window drawn anew"),
    ("desktop_key", "A key, from the driver to the program (the middle one)"),
    ("desktop_key_p99", "A key, from the driver to the program (99 in 100 are faster)"),
    ("desktop_move", "One motion of the mouse moving the window"),
    ("desktop_resize", "One size of the window, resized by the mouse"),
];

/// `ob bench --desktop`: measures Radix booted from the test image
/// `image`, and Linux with the kernel `linux` gives (the pinned one for
/// `Some(None)`, no Linux at all for `None`), `runs` times each in turn,
/// and writes the report to `dir`, and to `summary` too.
pub fn command(
    ctx: &Ctx,
    dir: &Path,
    image: &[u8],
    linux: Option<Option<String>>,
    runs: u32,
    summary: Option<&str>,
) -> Result<()> {
    let kvm = qemu::kvm_available();
    if !kvm {
        eprintln!(
            "    No KVM: QEMU emulates every instruction, so the numbers measure the emulator, \
             not the desktops. One run only."
        );
    }
    let runs = if kvm { runs } else { 1 };
    let linux = match linux {
        Some(given) => Some(linux_boot(ctx, dir, given)?),
        None => None,
    };
    let release = match &linux {
        Some((kernel, _)) => Some(bench::release_of(kernel)?),
        None => None,
    };
    let (mut ours, mut theirs) = (Vec::new(), Vec::new());
    for run in 1..=runs {
        step(&format!("Booting Radix with the desktop's rows, run {run} of {runs}"));
        bench::keep_best(&mut ours, radix_numbers(ctx, dir, image, kvm)?);
        if let Some((kernel, initrd)) = &linux {
            step(&format!("Booting Linux with the desktop's rows, run {run} of {runs}"));
            bench::keep_best(&mut theirs, linux_numbers(ctx, dir, kernel, initrd, kvm)?);
        }
    }
    let report = report(&bench::commit(ctx), &ours, &theirs, release.as_deref(), runs, kvm);
    let markdown = dir.join("desktop.md");
    std::fs::write(&markdown, &report).context(|| format!("writing {}", markdown.display()))?;
    if let Some(path) = summary {
        bench::append(path, &report)?;
    }
    eprintln!("\n{report}");
    step(&format!("Desktop report in {}", ctx.display(&markdown)));
    Ok(())
}

/// Builds `window-test` for Linux as a static program, which runs
/// beside whatever libraries the Linux desktop brings.
fn window_test_static(ctx: &Ctx) -> Result<PathBuf> {
    step("Building window-test as a static program for Linux");
    let mut cmd = userspace::host_cargo_of(ctx, "build", &["window-test"]);
    cmd.args(["--quiet", "--release", "--target", "x86_64-unknown-linux-gnu"])
        .env("RUSTFLAGS", "-C target-feature=+crt-static");
    process::run(ctx, &mut cmd)?;
    Ok(ctx.target.join("programs-host/x86_64-unknown-linux-gnu/release/window-test"))
}

/// The Linux kernel to boot, `given` or else the pinned one, and its
/// initramfs: `linux-bench` as init with the kernel's modules
/// ([`bench::linux_vm_boot`]), and after it `/window-test` and the Linux
/// desktop's tree ([`linux_desktop::tree`]).
fn linux_boot(ctx: &Ctx, dir: &Path, given: Option<String>) -> Result<(PathBuf, PathBuf)> {
    let (kernel, init) = bench::linux_vm_boot(ctx, dir, None, None, given)?;
    let program = window_test_static(ctx)?;
    let tree = linux_desktop::tree(ctx)?;
    let mut archive = std::fs::read(&init).context(|| format!("reading {}", init.display()))?;
    let program = std::fs::read(&program).context(|| format!("reading {}", program.display()))?;
    // A second archive after the first, numbered clear of it.
    bench::newc_entry(&mut archive, 1000, "window-test", 0o100_755, (0, 0), &program);
    archive.extend(bench::newc_tree(&tree, 1001)?);
    let initrd = dir.join("linux-desktop.cpio");
    std::fs::write(&initrd, archive).context(|| format!("writing {}", initrd.display()))?;
    Ok((kernel, initrd))
}

/// The machine both systems are measured on, booting `image` with
/// `ovmf`, or `direct`.
fn machine(
    dir: &Path,
    name: &str,
    kvm: bool,
    image: PathBuf,
    ovmf: Option<(PathBuf, PathBuf)>,
    direct: Option<DirectBoot>,
) -> Machine {
    Machine {
        firmware: Firmware::Uefi,
        media: Media::Usb,
        ovmf,
        image,
        memory: bench::VM_MEMORY.into(),
        cpus: bench::VM_CPUS,
        kvm,
        cpu_model: None,
        serial: Serial::Pipe,
        display: false,
        reboot: false,
        gdb: false,
        qmp: Some(dir.join(format!("{name}.qmp"))),
        extra: DEVICES.map(String::from).into_iter().chain(traced(dir, name)).collect(),
        write_log: None,
        direct,
    }
}

/// MEASUREMENT ONLY: QEMU's own log of the port and device-memory
/// accesses it serves, and how much of it has been read.
static TRACE: std::sync::Mutex<Option<(PathBuf, usize)>> = std::sync::Mutex::new(None);

/// MEASUREMENT ONLY: with `OB_TRACE_IO` set, the arguments that make QEMU
/// log every port and device-memory access it serves for machine `name`.
/// The log costs time at every access, so the rows of such a run say
/// nothing.
fn traced(dir: &Path, name: &str) -> Vec<String> {
    if std::env::var_os("OB_TRACE_IO").is_none() {
        return Vec::new();
    }
    let log = dir.join(format!("{name}.trace"));
    let _ = std::fs::remove_file(&log);
    *TRACE.lock().expect("the trace's lock") = Some((log.clone(), 0));
    let events = ["cpu_in", "cpu_out", "memory_region_ops_read", "memory_region_ops_write"];
    let mut args: Vec<String> =
        events.iter().flat_map(|event| ["-trace".to_string(), (*event).to_string()]).collect();
    args.extend(["-D".to_string(), log.display().to_string()]);
    args
}

/// MEASUREMENT ONLY: what QEMU logged until now is not counted by the
/// next [`trace_said`].
fn trace_mark() {
    if let Some((log, read)) = TRACE.lock().expect("the trace's lock").as_mut() {
        *read = std::fs::metadata(&*log).map_or(0, |data| data.len() as usize);
    }
}

/// MEASUREMENT ONLY: what QEMU logged since [`trace_mark`], counted by
/// port or by device region, the most frequent first.
fn trace_said(what: &str, count: u32) {
    let mut trace = TRACE.lock().expect("the trace's lock");
    let Some((log, read)) = trace.as_mut() else { return };
    let Ok(bytes) = std::fs::read(&*log) else { return };
    let new = String::from_utf8_lossy(bytes.get(*read..).unwrap_or_default()).into_owned();
    *read = bytes.len();
    let mut counts: std::collections::BTreeMap<String, u64> = std::collections::BTreeMap::new();
    for line in new.lines() {
        let word_after =
            |key: &str| line.split_once(key).and_then(|(_, rest)| rest.split_whitespace().next());
        let key = if let Some(at) = line.find("cpu_in ").or_else(|| line.find("cpu_out ")) {
            let event = line[at..].split_whitespace().next().unwrap_or("?");
            format!("{event} {}", word_after("addr ").unwrap_or("?"))
        } else if let Some(at) = line.find("memory_region_ops_") {
            let event = line[at..].split_whitespace().next().unwrap_or("?");
            let name = word_after("name ").unwrap_or("?");
            format!("{event} {name} size {}", word_after("size ").unwrap_or("?"))
        } else {
            continue;
        };
        *counts.entry(key).or_default() += 1;
    }
    let mut counts: Vec<(String, u64)> = counts.into_iter().collect();
    counts.sort_by(|one, other| other.1.cmp(&one.1));
    for (key, times) in counts.into_iter().take(24) {
        println!(
            "HOST {what}: trace {key}: {times} ({:.2} a turn)",
            times as f64 / f64::from(count)
        );
    }
}

/// Boots the test image `base` with the desktop and the program in it,
/// and measures the rows.
fn radix_numbers(ctx: &Ctx, dir: &Path, base: &[u8], kvm: bool) -> Result<Vec<Measurement>> {
    let name = "desktop-bench";
    let image = dir.join("desktop-bench.img");
    let bytes = test::with_cmdline(base, RADIX_CMDLINE).map_err(crate::Error::new)?;
    std::fs::write(&image, bytes).context(|| format!("writing {}", image.display()))?;
    let ovmf = qemu::prepare_ovmf(ctx, name)?;
    let machine = machine(dir, name, kvm, image.clone(), Some(ovmf), None);
    let mut vm = pin::start(ctx, &machine, dir, name, bench::TIMEOUT)?;
    let outcome = Cpus::of(&machine)
        .and_then(|cpus| drive(&mut vm, &cpus))
        .and_then(|rows| vm.expect_first(&["window-test: passed", FAILED]).map(|_| rows));
    let log = vm.log();
    drop(vm);
    let _ = std::fs::remove_file(&image);
    numbers_of(ctx, dir, name, "Dukovo", "window-test: ", outcome, &log)
}

/// Boots `kernel` with `initrd` ([`linux_boot`]) on the same machine,
/// and measures the rows.
fn linux_numbers(
    ctx: &Ctx,
    dir: &Path,
    kernel: &Path,
    initrd: &Path,
    kvm: bool,
) -> Result<Vec<Measurement>> {
    let name = "linux-desktop";
    let direct = DirectBoot {
        kernel: kernel.to_path_buf(),
        initrd: initrd.to_path_buf(),
        cmdline: format!("{} {LINUX_ARGS}", bench::LINUX_CMDLINE),
    };
    let machine = machine(dir, name, kvm, PathBuf::new(), None, Some(direct));
    let mut vm = pin::start(ctx, &machine, dir, name, bench::TIMEOUT)?;
    let outcome = Cpus::of(&machine)
        .and_then(|cpus| drive(&mut vm, &cpus))
        .and_then(|rows| vm.expect(LINUX_ENDED).map(|()| rows));
    let log = vm.log();
    drop(vm);
    numbers_of(ctx, dir, name, "Linux", "linux-bench: ", outcome, &log)
}

/// The rows of one boot: those [`drive`] measured, and the key rows the
/// program printed in `log`. A failed boot is an error that says what
/// `program` said.
fn numbers_of(
    ctx: &Ctx,
    dir: &Path,
    name: &str,
    system: &str,
    program: &str,
    outcome: std::result::Result<Vec<Measurement>, String>,
    log: &str,
) -> Result<Vec<Measurement>> {
    let why = match outcome {
        Ok(mut rows) => {
            let printed = bench::parse(log);
            let missing = ["desktop_key", "desktop_key_p99"]
                .into_iter()
                .find(|row| bench::find(&printed, row).is_none());
            match missing {
                None => {
                    rows.extend(printed.into_iter().filter(|row| row.name.starts_with("desktop_")));
                    return Ok(rows);
                }
                Some(row) => format!("the program printed no {row}"),
            }
        }
        Err(why) => why,
    };
    bench::show_trouble(log);
    crate::bail!(
        "the desktop rows' boot of {system} failed: {why}{}; serial log: {}",
        bench::said_by(log, program),
        ctx.display(&dir.join(format!("boot-{name}.log")))
    )
}

/// The host's threads that are the machine's CPUs.
struct Cpus(Vec<u64>);

impl Cpus {
    /// Those of `machine`, which runs: QEMU names them (QMP's
    /// `query-cpus-fast`). Where it emulates all the CPUs in one thread,
    /// that thread is counted once.
    fn of(machine: &Machine) -> std::result::Result<Cpus, String> {
        let socket = machine.qmp.as_deref().ok_or("the machine has no QMP socket")?;
        let mut threads = pin::vcpu_threads(socket)?;
        threads.sort_unstable();
        threads.dedup();
        Ok(Cpus(threads))
    }

    /// How long the host's processors have run them so far, in
    /// nanoseconds, summed: the first number of each thread's
    /// `schedstat`.
    fn ran_ns(&self) -> std::result::Result<u64, String> {
        let of = |thread: &u64| {
            let file = format!("/proc/{thread}/schedstat");
            let counted = std::fs::read_to_string(&file).map_err(|err| format!("{file}: {err}"))?;
            let ran = counted.split_whitespace().next().and_then(|ns| ns.parse::<u64>().ok());
            ran.ok_or_else(|| format!("{file} does not say how long the thread ran"))
        };
        self.0.iter().map(of).sum()
    }
}

/// How long the machine's CPUs had run at a moment.
struct Ran {
    ns: u64,
    at: Instant,
}

impl Ran {
    fn now(cpus: &Cpus) -> std::result::Result<Ran, String> {
        Ok(Ran { ns: cpus.ran_ns()?, at: Instant::now() })
    }

    /// The nanoseconds the CPUs ran since, and how long that was.
    fn since(&self, cpus: &Cpus) -> std::result::Result<(f64, Duration), String> {
        let now = Ran::now(cpus)?;
        Ok((now.ns.saturating_sub(self.ns) as f64, now.at - self.at))
    }

    /// What the CPUs ran since beyond what they run in as long with
    /// nothing moving, `idle` nanoseconds a second.
    fn work_since(&self, cpus: &Cpus, idle: f64) -> std::result::Result<f64, String> {
        let (ran, took) = self.since(cpus)?;
        Ok((ran - idle * took.as_secs_f64()).max(0.0))
    }
}

/// A motion of the mouse by `by`, as QMP's events.
fn motion(by: (i32, i32)) -> Vec<String> {
    [("x", by.0), ("y", by.1)]
        .into_iter()
        .filter(|(_, value)| *value != 0)
        .map(|(axis, value)| {
            format!(r#"{{"type":"rel","data":{{"axis":"{axis}","value":{value}}}}}"#)
        })
        .collect()
}

/// The left button going down or up, as QMP's event.
fn left_button(down: bool) -> [String; 1] {
    [format!(r#"{{"type":"btn","data":{{"down":{down},"button":"left"}}}}"#)]
}

/// Waits until the program says `what`.
fn step_said(vm: &mut Vm, what: &str) -> std::result::Result<(), String> {
    match vm.expect_first(&[what, FAILED, LINUX_ENDED])? {
        0 => Ok(()),
        _ => Err(format!("the program ended before it said `{what}`")),
    }
}

/// [`STROKES`] strokes of [`STROKE`] motions by `by` and as many back,
/// and the processors' work for them.
fn strokes(
    vm: &mut Vm,
    cpus: &Cpus,
    by: (i32, i32),
    idle: f64,
) -> std::result::Result<f64, String> {
    std::thread::sleep(SETTLE);
    let from = Ran::now(cpus)?;
    for _ in 0..STROKES {
        for by in [by, (-by.0, -by.1)] {
            for _ in 0..STROKE {
                vm.input_events(SCREEN, &motion(by))?;
                std::thread::sleep(MOTION_PAUSE);
            }
        }
    }
    std::thread::sleep(SETTLE);
    from.work_since(cpus, idle)
}

/// The count in the program's `cost: drew <count> sizes`, which `vm` has
/// just matched up to `drew `.
fn sizes_drawn(vm: &mut Vm) -> std::result::Result<u32, String> {
    step_said(vm, " sizes")?;
    let log = vm.log();
    let count = log
        .rsplit("cost: drew ")
        .find_map(|rest| rest.split_once(" sizes")?.0.parse::<u32>().ok())
        .filter(|count| *count > 0);
    count.ok_or_else(|| "the program drew the window at no size".into())
}

/// MEASUREMENT ONLY: prints what the host's KVM counted for the machine
/// since `before`, over `count` turns that took `took`, and what its
/// CPUs' threads ran.
fn host_said(
    vm: &mut Vm,
    before: &[(String, u64)],
    what: &str,
    count: u32,
    ran: f64,
    took: Duration,
) {
    println!(
        "HOST {what}: {count} turns in {} ms, the CPUs' threads ran {:.1} us a turn",
        took.as_millis(),
        ran / 1000.0 / f64::from(count)
    );
    for (name, now) in vm.host_counts() {
        let was = before.iter().find(|(key, _)| *key == name).map_or(0, |(_, value)| *value);
        let since = now.saturating_sub(was);
        if since > 0 {
            println!("HOST {what}: {name} {since} ({:.2} a turn)", since as f64 / f64::from(count));
        }
    }
    trace_said(what, count);
}

/// Sees `window-test cost` in `vm` through, from its first step to its
/// `cost: done`, and gives the rows measured from the host.
fn drive(vm: &mut Vm, cpus: &Cpus) -> std::result::Result<Vec<Measurement>, String> {
    let row = |name: &str, ns: f64, count: u32| Measurement {
        name: name.into(),
        ns: ns / f64::from(count),
        iterations: u64::from(count),
    };
    let mut rows = Vec::new();

    // The window shows and nothing moves.
    step_said(vm, "cost: ready for a key")?;
    std::thread::sleep(REST_SETTLE);
    trace_mark();
    let counted = vm.host_counts();
    let from = Ran::now(cpus)?;
    std::thread::sleep(IDLE_FOR);
    let (ran, took) = from.since(cpus)?;
    host_said(vm, &counted, "rest, a second", IDLE_FOR.as_secs() as u32, ran, took);
    let idle = ran / took.as_secs_f64();
    rows.push(row("desktop_idle", idle, 1));
    // MEASUREMENT ONLY: the seconds after, each by itself, which show
    // whether the machine had come to rest.
    for second in 1..=8 {
        let counted = vm.host_counts();
        let from = Ran::now(cpus)?;
        std::thread::sleep(Duration::from_secs(1));
        let (ran, _) = from.since(cpus)?;
        let now = vm.host_counts();
        let since = |name: &str| {
            let of = |counts: &[(String, u64)]| {
                counts.iter().find(|(key, _)| key == name).map_or(0, |(_, value)| *value)
            };
            of(&now).saturating_sub(of(&counted))
        };
        println!(
            "HOST rest later, second {second}: ran {:.1} us, {} halts, {} mmio, {} io, {} exits",
            ran / 1000.0,
            since("halt_exits"),
            since("mmio_exits"),
            since("io_exits"),
            since("exits")
        );
    }

    // A key starts the frames.
    trace_mark();
    let counted = vm.host_counts();
    let from = Ran::now(cpus)?;
    vm.keys_on(SCREEN, &["a"], true)?;
    step_said(vm, "cost: drew the frames")?;
    let (ran, took) = from.since(cpus)?;
    host_said(vm, &counted, "frames", FRAMES, ran, took);
    rows.push(row("desktop_frame", (ran - idle * took.as_secs_f64()).max(0.0), FRAMES));
    vm.keys_on(SCREEN, &["a"], false)?;

    // The keys the program times itself.
    step_said(vm, &format!("cost: ready for {KEYS} keys"))?;
    for _ in 0..KEYS {
        for down in [true, false] {
            vm.keys_on(SCREEN, &["a"], down)?;
            std::thread::sleep(KEY_PAUSE);
        }
    }

    // The pointer to the screen's corner, then onto the window.
    step_said(vm, "cost: ready for the pointer")?;
    vm.input_events(SCREEN, &motion((-3000, -3000)))?;
    let until = Instant::now() + POINTER_PATIENCE;
    while !vm.said("cost: ready for a press") {
        if vm.said(FAILED) || vm.said(LINUX_ENDED) || Instant::now() > until {
            return Err("the pointer did not come onto the window".into());
        }
        vm.input_events(SCREEN, &motion((8, 8)))?;
        std::thread::sleep(MOTION_PAUSE);
    }

    // The window moved by the pointer.
    step_said(vm, "cost: ready for a press")?;
    vm.input_events(SCREEN, &left_button(true))?;
    step_said(vm, "cost: moving")?;
    rows.push(row("desktop_move", strokes(vm, cpus, MOVE_BY, idle)?, STROKES * STROKE * 2));
    vm.input_events(SCREEN, &left_button(false))?;

    // The window resized by its corner, and drawn at each size.
    step_said(vm, "cost: ready for a press")?;
    vm.input_events(SCREEN, &left_button(true))?;
    step_said(vm, "cost: resizing")?;
    trace_mark();
    let counted = vm.host_counts();
    let from = Ran::now(cpus)?;
    let resizing = strokes(vm, cpus, RESIZE_BY, idle)?;
    let (ran, took) = from.since(cpus)?;
    host_said(vm, &counted, "resize", STROKES * STROKE * 2, ran, took);
    vm.input_events(SCREEN, &left_button(false))?;
    step_said(vm, "cost: drew ")?;
    rows.push(row("desktop_resize", resizing, sizes_drawn(vm)?));

    step_said(vm, "cost: done")?;
    Ok(rows)
}

/// The Markdown report of the checkout at `commit`: `ours` is Radix's
/// rows, `theirs` Linux's, of the kernel `release`, or empty where
/// Linux was not measured.
fn report(
    commit: &str,
    ours: &[Measurement],
    theirs: &[Measurement],
    release: Option<&str>,
    runs: u32,
    kvm: bool,
) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "## Dukovo desktop rows\n");
    let beside = match release {
        Some(release) => format!(
            "Linux is Linux {release} with {LINUX_DESKTOP}, on a virtual machine set up the \
             same way"
        ),
        None => "Linux was not measured this time, so there is no ratio".into(),
    };
    let _ = writeln!(
        out,
        "Dukovo {VERSION} at {}, its desktop in QEMU {} with {} CPUs and a virtio graphics \
         card, keyboard and mouse, on {}, Linux {}. {beside}. Both draw on the processor, a \
         window of 640 by 480 on a screen of 1024 by 768. A key's rows are timed by the \
         program in the window; the others are the time the host's processors ran the \
         machine's CPUs for the step, less what they run with nothing moving, by the step's \
         count{}. Lower is better. Nothing is gated on these rows.\n",
        commit,
        if kvm { "under KVM" } else { "without KVM (emulated, so not comparable)" },
        bench::VM_CPUS,
        bench::host_cpu(),
        bench::host_kernel(),
        if runs > 1 { format!(", the lowest of {runs} runs") } else { String::new() },
    );
    let _ = writeln!(out, "| Row | What | Dukovo | Linux | Ratio |");
    let _ = writeln!(out, "| --- | --- | ---: | ---: | ---: |");
    for (name, what) in ROWS {
        let shown = |rows: &[Measurement]| {
            bench::find(rows, name).map_or_else(String::new, |row| bench::show(row.ns))
        };
        let ratio = match (bench::find(ours, name), bench::find(theirs, name)) {
            (Some(ours), Some(theirs)) if theirs.ns > 0.0 => format!("{:.2}", ours.ns / theirs.ns),
            _ => String::new(),
        };
        let _ =
            writeln!(out, "| `{name}` | {what} | {} | {} | {ratio} |", shown(ours), shown(theirs));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The program's source, for the counts this module divides by.
    const PROGRAM: &str = include_str!("../../../userspace/programs/window-test/src/cost.rs");

    #[test]
    fn the_counts_are_the_program_s() {
        assert!(PROGRAM.contains(&format!("const FRAMES: u32 = {FRAMES};")));
        assert!(PROGRAM.contains(&format!("const KEYS: usize = {KEYS};")));
        for (name, _) in ROWS {
            assert!(PROGRAM.contains(&format!("`{name}`")), "{name}");
        }
    }

    #[test]
    fn a_motion_names_the_axes_it_moves_along() {
        assert_eq!(motion((4, 0)), [r#"{"type":"rel","data":{"axis":"x","value":4}}"#]);
        assert_eq!(motion((-2, -2)).len(), 2);
        assert_eq!(left_button(true), [r#"{"type":"btn","data":{"down":true,"button":"left"}}"#]);
    }

    #[test]
    fn the_report_has_a_ratio_where_both_were_measured() {
        let row = |name: &str, ns| Measurement { name: name.into(), ns, iterations: 1 };
        let ours = [row("desktop_frame", 500_000.0), row("desktop_key", 80_000.0)];
        let theirs = [row("desktop_frame", 1_000_000.0)];
        let text = report("0123456789ab", &ours, &theirs, Some("7.0.0"), 3, true);
        assert!(text.contains(
            "| `desktop_frame` | One frame of the window drawn anew | 500.0 µs | 1000.0 µs | 0.50 |"
        ));
        assert!(text.contains("(the middle one) | 80.0 µs |  |  |"));
        assert!(text.contains("the lowest of 3 runs"));
        let alone = report("0123456789ab", &ours, &[], None, 1, false);
        assert!(alone.contains("Linux was not measured this time"));
        assert!(alone.contains("without KVM"));
    }
}
