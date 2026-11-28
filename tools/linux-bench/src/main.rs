// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! `linux-bench`: the cost on Linux of the operations that `ob bench`
//! measures on Radix, as a baseline (docs/design/ipc.md).
//!
//! ```text
//! linux-bench [--rounds <count>] [--posix-bench <program>] [--git <program>]
//!             [--disc <device>] [--sd <device>] [--only scaling|display|desktop]
//! ```
//!
//! With `--only scaling` it runs only the POSIX benchmarks' open/close
//! scaling benchmark (`ob bench --scaling`); as process 1 it gets that
//! from the kernel command line, after `--`. With `--only display` it
//! runs only the display and input rows ([`display`]), on the machine
//! `ob bench` boots for them, which has a virtio graphics card and a
//! virtio mouse. With `--only desktop`, as process 1, it runs the Linux
//! desktop that `ob bench --desktop` measures Dukovo's beside
//! ([`desktop`]).
//!
//! It prints one line per benchmark, in the format of the Radix benchmark
//! programs:
//!
//! ```text
//! BENCH name=<name> ns=<nanoseconds per operation> iterations=<count>
//! ```
//!
//! The system calls are timed in a loop, as lmbench's lat_syscall does.
//! Each round trip is a pair of threads passing an 8-byte message there
//! and back, as in lmbench's lat_pipe and lat_unix, once with both threads
//! on one CPU, once on two, and (for pipes) once with a pair on each of
//! two CPUs at once. A context switch is two processes on one
//! CPU yielding to each other, as in lmbench's lat_ctx; a fresh page is an
//! anonymous mapping, its first touch and its removal; a TLB shootdown is
//! a page losing its access while another thread of the process runs on
//! another CPU; memory bandwidth is `rep movsb` between two buffers larger
//! than the caches, the same instructions Radix runs. The file benchmarks
//! work on a tmpfs, as Radix's do: opening and closing a file by its name
//! beneath a directory, its status, and reading and writing its first
//! page, which is in memory.
//!
//! Last it runs the POSIX benchmarks, the C program that Radix runs too
//! (`userspace/posix/bench.c`, built with this machine's C compiler and C
//! library), given with `--posix-bench`, or at `/posix-bench` as process 1,
//! where it also gives them the virtual machine's disks, `/dev/vda` and
//! `/dev/nvme0n1`, for their disk benchmarks, with `--disc` the
//! optical drive at that device, for their disc benchmarks, and with
//! `--sd` the SD card at that device, for their SD card benchmarks, each
//! once it shows up (the AHCI controller's ports are probed after its
//! module loads, and an SD card is set up after the SD host's driver
//! loads). They time TCP and UDP over
//! the loopback interface too, which process 1 brings up first, as a
//! booting system does, and `git status` with the git given with
//! `--git`, or at `/git` as process 1 (ADR 0294).
//!
//! As process 1 it also runs `/server-bench` (`userspace/posix/serverbench.c`,
//! built for Linux, with the servers it runs under `/applications`) when
//! the initramfs holds one, on a tmpfs at `/tmp`, with the environment
//! `TZ=UTC` and `PATH=/usr/bin:/bin` and nothing else: nginx, PHP, Valkey
//! and PostgreSQL, whose rows print in the same format as the rest.
//! It prints a `BENCHCOUNT` line after each measured pass too, with the
//! context switches and the CPU time (user, system, idle) of the pass from
//! `/proc/stat`, and the page faults (`pgfault` in `/proc/vmstat`) and the
//! interrupts (the total of the `intr` line of `/proc/stat`) of the pass,
//! each left out if its file cannot be read.
//!
//! After the TLB shootdowns it prints how many TLB interrupts reached the
//! other CPU (the `TLB` line of `/proc/interrupts`), as Radix prints its
//! kernel's events:
//!
//! ```text
//! BENCHCOUNT group=tlb_shootdown_linux rounds=<count> ipis=<count>
//! ```
//!
//! `ob bench` runs it on the host, and also as the init process of a Linux
//! virtual machine set up like the one Radix runs in: as process 1 it
//! mounts `/proc`, `/dev` and `/sys`, loads the kernel modules of the initramfs
//! (Btrfs, where the kernel has it as a module), mounts the Btrfs disk,
//! prints `linux-bench: done` at the end and powers the machine off.
//!
//! With `--btrfs-log make` or `--btrfs-log list`, as process 1, it
//! makes or lists the Btrfs log of `ob fixture btrfs-log` instead
//! ([`btrfs_log`]).
//!
//! As process 1 it also prints what Linux does with the CPUs' clock and
//! idle states (`linux-bench: CPU frequency:` and `linux-bench: idle:`),
//! and holds every CPU at its base frequency when the kernel command line
//! has `linux-bench.clock=base` ([`hold_clock`]), so that the lab's
//! performance tests run Linux at the clock Dukovo runs at (ADR 0557).
//! If the lines show a different clock than the rows ran at, start with
//! `/sys/devices/system/cpu/cpufreq` on the twin.

#![forbid(unsafe_code)]

mod btrfs_log;
mod desktop;
mod display;

use std::io::{Read, Write};
use std::os::fd::{AsFd, BorrowedFd, OwnedFd};
use std::os::unix::fs::FileExt;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Barrier};
use std::time::Instant;

use sysapi::linux::{self, Ending};
use sysapi::memory::{self, Access, Mapping};
use sysapi::process::{self, Fork, UsageOf};
use sysapi::{fd, fs, random, terminal, volatile};

/// The kernel command line word that has process 1 hold the CPUs at their
/// base frequency. A word with a dot is left to modules, so Linux neither
/// warns about it nor passes it to init, and it stays clear of `--`: the
/// lab appends `console=` to the end of a kit's command line, which after
/// `--` would reach init.
const CLOCK_WORD: &str = "linux-bench.clock=base";

/// Where Linux keeps the CPUs' frequency policies, one folder each.
const CPUFREQ: &str = "/sys/devices/system/cpu/cpufreq";

/// Where intel_pstate keeps its own switches, when it is the driver.
const INTEL_PSTATE: &str = "/sys/devices/system/cpu/intel_pstate";

/// Where the CPU idle states are described.
const CPUIDLE: &str = "/sys/devices/system/cpu/cpuidle";

/// Where the POSIX benchmarks are in the virtual machine's initramfs.
const POSIX_BENCH: &str = "/posix-bench";

/// Where git is in the initramfs, for the git benchmark.
const GIT: &str = "/git";

/// Where the server benchmarks are in the initramfs, when `ob lab` made
/// the kit with them, and the servers they run.
const SERVER_BENCH: &str = "/server-bench";
const SERVERS: [&str; 4] = ["nginx", "php", "valkey", "postgresql"];

/// The virtual machine's disks, for the POSIX benchmarks' disk benchmarks:
/// a virtio disk and an NVMe one.
const DISK: &str = "/dev/vda";
const NVME: &str = "/dev/nvme0n1";

/// How long an optical drive given with `--disc`, or an SD card given
/// with `--sd`, may take to show up: the AHCI module probes its ports in
/// the background, and the SD host's driver sets a card up after it loads.
const DRIVE_WAIT: std::time::Duration = std::time::Duration::from_secs(10);

/// The virtual machine's Btrfs disk, for the POSIX benchmarks' Btrfs
/// benchmarks, and where it is mounted.
const BTRFS_DISK: &str = "/dev/vdb";
const BTRFS_AT: &str = "/mnt/vdb";

/// Where `ob bench` puts the kernel modules the disk needs, in the
/// initramfs: named so that they load in the order of their names.
const MODULES: &str = "/modules";

fn main() {
    let mut rounds = 100_000u64;
    let mut posix = None;
    let mut btrfs_log = None;
    let mut git = None;
    let mut disc = None;
    let mut sd = None;
    let mut scaling = false;
    let mut display = false;
    let mut desktop = false;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        let value = args.next();
        match (arg.as_str(), value) {
            ("--rounds", Some(count)) if count.parse::<u64>().is_ok_and(|count| count > 0) => {
                rounds = count.parse().unwrap_or(rounds);
            }
            ("--posix-bench", Some(program)) => posix = Some(PathBuf::from(program)),
            ("--git", Some(program)) => git = Some(PathBuf::from(program)),
            ("--disc", Some(device)) => disc = Some(PathBuf::from(device)),
            ("--sd", Some(device)) => sd = Some(PathBuf::from(device)),
            ("--only", Some(what)) if what == "scaling" => scaling = true,
            ("--only", Some(what)) if what == "display" => display = true,
            ("--only", Some(what)) if what == "desktop" => desktop = true,
            ("--btrfs-log", Some(mode)) if mode == "make" || mode == "list" => {
                btrfs_log = Some(mode);
            }
            _ => {
                eprintln!(
                    "usage: linux-bench [--rounds <count>] [--posix-bench <program>] \
                     [--git <program>] [--disc <device>] [--sd <device>] \
                     [--btrfs-log make|list] [--only scaling|display|desktop]"
                );
                std::process::exit(2);
            }
        }
    }
    if std::process::id() == 1 {
        mount("proc", "/proc");
        mount("devtmpfs", "/dev");
        mount("sysfs", "/sys");
        load_modules();
        report_cpu_frequency();
        report_idle();
        if std::fs::read_to_string("/proc/cmdline")
            .is_ok_and(|line| line.split_whitespace().any(|word| word == CLOCK_WORD))
        {
            hold_clock();
        }
    }
    if let Some(mode) = btrfs_log {
        if mode == "make" {
            btrfs_log::make();
        } else {
            btrfs_log::list();
        }
        power_off();
    }
    if display {
        // `ob bench`'s display boot: the display and input rows, and
        // nothing else.
        display::run();
        if std::process::id() == 1 {
            println!("linux-bench: done");
            power_off();
        }
        return;
    }
    if desktop {
        // `ob bench --desktop`'s boot: the desktop with the program the
        // host times in it, and nothing else.
        if std::process::id() == 1 {
            desktop::run();
            println!("linux-bench: done");
            power_off();
        } else {
            eprintln!("linux-bench: --only desktop is for process 1 of `ob bench --desktop`");
            std::process::exit(2);
        }
        return;
    }
    let cpus = linux::allowed_cpus().expect("sched_getaffinity");
    let Some(&first) = cpus.first() else {
        eprintln!("linux-bench: this process may run on no CPU");
        std::process::exit(1);
    };
    if scaling {
        // `ob bench --scaling`: the POSIX benchmarks' open/close scaling
        // benchmark, and nothing else.
        let program = posix.unwrap_or_else(|| PathBuf::from(POSIX_BENCH));
        std::io::stdout().flush().ok();
        match std::process::Command::new(&program).arg("--scaling").status() {
            Ok(status) if status.success() => {}
            Ok(status) => eprintln!("linux-bench: {} failed: {status}", program.display()),
            Err(err) => eprintln!("linux-bench: cannot run {}: {err}", program.display()),
        }
        if std::process::id() == 1 {
            println!("linux-bench: done");
            power_off();
        }
        return;
    }
    pin(first);

    let calls = rounds * 2;
    report("getppid", calls, getppid);
    report("sched_yield", calls, std::thread::yield_now);
    report("fcntl_getfd", calls, fcntl_getfd);
    report("getrandom_64", calls / 4, getrandom_64);
    report("fresh_page", rounds / 2, fresh_page);
    memory_copy();
    files(rounds / 2);

    for (place, other) in [("same_cpu", Some(first)), ("cross_cpu", cpus.get(1).copied())] {
        let Some(other) = other else {
            eprintln!("linux-bench: one CPU only, so no round trips across two");
            continue;
        };
        let pair = Pair { other, rounds };
        pair.run(&format!("pipe_roundtrip_{place}"), pipes());
        pair.run(&format!("unix_socket_roundtrip_{place}"), sockets());
        pair.run(&format!("eventfd_roundtrip_{place}"), eventfds());
        pair.futex(&format!("futex_pingpong_{place}"));
    }
    context_switch(rounds);
    if let Some(&other) = cpus.get(1) {
        tlb_shootdown(other, rounds / 2);
        for (name, count, body) in [
            ("getppid_parallel", calls, getppid as fn()),
            ("fcntl_getfd_parallel", calls, fcntl_getfd),
            ("getrandom_64_parallel", calls / 4, getrandom_64),
        ] {
            parallel(name, other, count, body);
        }
        pipe_roundtrip_parallel(first, other, rounds);
    }
    let posix = posix.or_else(|| (std::process::id() == 1).then(|| PathBuf::from(POSIX_BENCH)));
    if let Some(program) = posix {
        let mut disks: Vec<(&str, &Path)> = if std::process::id() == 1 {
            vec![("--disk", Path::new(DISK)), ("--nvme", Path::new(NVME))]
        } else {
            Vec::new()
        };
        let given = Instant::now();
        for (flag, drive) in [("--sd", &sd), ("--disc", &disc)] {
            if let Some(drive) = drive {
                while !drive.exists() && given.elapsed() < DRIVE_WAIT {
                    std::thread::sleep(std::time::Duration::from_millis(100));
                }
                disks.push((flag, drive.as_path()));
            }
        }
        disks.retain(|(_, disk)| {
            let there = disk.exists();
            if !there {
                eprintln!(
                    "linux-bench: the virtual machine has no {}, for the disk benchmarks",
                    disk.display()
                );
            }
            there
        });
        if std::process::id() == 1 && mount_btrfs() {
            disks.push(("--btrfs", Path::new(BTRFS_AT)));
        }
        let network = std::process::id() != 1 || loopback_up();
        // The initramfs holds git when `ob bench` built one; an empty
        // file stands for none.
        let git = git.or_else(|| {
            let at = Path::new(GIT);
            (std::process::id() == 1 && at.metadata().is_ok_and(|m| m.len() > 0))
                .then(|| at.to_path_buf())
        });
        if let Some(git) = &git {
            disks.push(("--git", git.as_path()));
        }
        posix_bench(&program, &cpus, &disks, network);
    }

    if std::process::id() == 1 {
        server_bench(&cpus);
    }

    if std::process::id() == 1 {
        // The init process of `ob bench`'s Linux virtual machine.
        println!("linux-bench: done");
        power_off();
    }
}

/// Powers the machine off, as its process 1, with nothing saved first:
/// what is not on the disks yet is lost, as at a power cut.
fn power_off() {
    if std::process::id() != 1 {
        return;
    }
    // Let the serial port send the last lines first. A write to the
    // console returns once the bytes are queued, and the kernel prints
    // its own "reboot: Power down" straight to the port, which landed in
    // the middle of "linux-bench: done" and hid it from `ob bench`.
    std::io::stdout().flush().ok();
    std::io::stderr().flush().ok();
    // On a descriptor that is not a terminal, tcdrain fails at once, which
    // is fine here.
    let _ = terminal::drain(std::io::stdout());
    let _ = terminal::drain(std::io::stderr());
    let err = linux::end(Ending::PowerOff);
    eprintln!("linux-bench: cannot power off: {err}");
    loop {
        std::thread::park();
    }
}

fn getppid() {
    // The call is what is timed; its answer is not used.
    let _ = linux::parent_through_system_call();
}

/// A descriptor to look up: an eventfd, which every Linux has.
fn some_fd() -> BorrowedFd<'static> {
    static FD: std::sync::OnceLock<OwnedFd> = std::sync::OnceLock::new();
    FD.get_or_init(|| linux::event_counter().expect("cannot make an eventfd")).as_fd()
}

/// Looks a descriptor up and reads its flags: the closest Linux has to a
/// capability's `info` method.
fn fcntl_getfd() {
    fd::close_on_exec(some_fd()).expect("fcntl");
}

fn getrandom_64() {
    let mut bytes = [0u8; 64];
    random::fill(&mut bytes).expect("getrandom");
}

const PAGE: usize = 4096;

/// Maps `len` bytes of fresh anonymous memory.
fn map_anonymous(len: usize) -> Mapping {
    Mapping::anonymous(len).expect("mmap")
}

/// Touches a page of `mapping` with one eight-byte store, as the first
/// touch of a fresh page.
fn touch(mapping: &mut Mapping) {
    let bytes = mapping.bytes_mut().expect("a mapping that may be written");
    volatile::write(
        <&mut [u8; 8]>::try_from(&mut bytes[..8]).expect("eight bytes"),
        1u64.to_ne_bytes(),
    );
}

/// A fresh page: mapped, touched (a page fault that zeroes it), unmapped.
fn fresh_page() {
    let mut page = map_anonymous(PAGE);
    touch(&mut page);
}

/// The bytes of each of the two buffers memory bandwidth is measured
/// between, and how many times it copies one into the other.
const COPY_BYTES: usize = 32 << 20;
/// Copies in a batch, and batches: the fastest batch counts, as on Radix,
/// so that a moment of contention on a shared machine does not.
const COPY_BATCH: u64 = 6;
const COPY_BATCHES: usize = 5;

/// Memory bandwidth: nanoseconds per MiB copied, in the fastest of a few
/// batches of copies.
fn memory_copy() {
    let (mut from, mut to) = (map_anonymous(COPY_BYTES), map_anonymous(COPY_BYTES));
    let from = from.bytes_mut().expect("a mapping that may be written");
    let to = to.bytes_mut().expect("a mapping that may be written");
    // Fill the source, and copy once, so that no page is touched for the
    // first time below.
    from.fill(0x5a);
    memory::copy_bytes(to, from);
    let mib = COPY_BATCH * (COPY_BYTES >> 20) as u64;
    let fastest = (0..COPY_BATCHES)
        .map(|_| {
            let started = Instant::now();
            for _ in 0..COPY_BATCH {
                memory::copy_bytes(to, from);
            }
            started.elapsed()
        })
        .min()
        .expect("at least one batch");
    let ns = fastest.as_nanos() as f64 / mib as f64;
    println!("BENCH name=memory_copy ns={ns:.2} iterations={mib}");
}

/// Two processes on this thread's CPU (a child inherits its pinning), each
/// yielding to the other, in another address space. A yield switches only
/// when the other process may run first (EEVDF), so both count their
/// context switches with getrusage(2), and the line gives what one switch
/// costs, as `ob bench` does for Radix with the kernel's count.
fn context_switch(rounds: u64) {
    let (ready_read, ready_write) = std::io::pipe().expect("cannot make a pipe");
    let (count_read, count_write) = std::io::pipe().expect("cannot make a pipe");
    // The process has one thread here: every pair has been joined.
    let child = match process::fork().expect("fork") {
        Fork::Parent(child) => child,
        Fork::Child => {
            let (mut ready_write, mut count_write) = (ready_write, count_write);
            let _ = ready_write.write_all(&[1]);
            let before = switches();
            for _ in 0..rounds {
                std::thread::yield_now();
            }
            let _ = count_write.write_all(&(switches() - before).to_le_bytes());
            // The child ends here, without unwinding the parent's state.
            process::exit_now(0);
        }
    };
    drop((ready_write, count_write));
    let mut byte = [0u8];
    (&ready_read).read_exact(&mut byte).expect("the other process did not start");
    let before = switches();
    let started = Instant::now();
    for _ in 0..rounds {
        std::thread::yield_now();
    }
    let elapsed = started.elapsed();
    let ours = switches() - before;
    let mut theirs = [0u8; 8];
    (&count_read).read_exact(&mut theirs).expect("the other process did not report");
    let total = (ours + u64::from_le_bytes(theirs)).max(1);
    let ns = elapsed.as_nanos() as f64 / total as f64;
    println!("BENCH name=context_switch ns={ns:.2} iterations={total}");
    process::wait(child).expect("cannot wait for the other process");
}

/// The context switches of this process so far, voluntary or not.
fn switches() -> u64 {
    let usage = process::usage(UsageOf::Process).expect("getrusage");
    usage.voluntary_switches + usage.involuntary_switches
}

/// A TLB shootdown: this thread takes a page's access away and gives it
/// back while another thread of the process spins on `other`, so every
/// removal must reach that CPU's TLB. Giving access back needs no flush.
fn tlb_shootdown(other: usize, rounds: u64) {
    let mut page = map_anonymous(PAGE);
    touch(&mut page);
    let mut protect = |access| page.protect(access).expect("mprotect");
    // The same system calls while no other thread runs: the shootdown is
    // the difference.
    let started = Instant::now();
    for _ in 0..rounds {
        protect(Access::None);
        protect(Access::ReadWrite);
    }
    print_line("tlb_local", started, rounds);
    let stop = Arc::new(AtomicBool::new(false));
    let ready = Arc::new(Barrier::new(2));
    let (go, done) = (ready.clone(), stop.clone());
    let spinner = std::thread::spawn(move || {
        pin(other);
        go.wait();
        while !done.load(Ordering::Relaxed) {
            std::hint::spin_loop();
        }
    });
    ready.wait();
    let interrupts = tlb_interrupts(other);
    let started = Instant::now();
    for _ in 0..rounds {
        protect(Access::None);
        protect(Access::ReadWrite);
    }
    print_line("tlb_shootdown", started, rounds);
    if let (Some(before), Some(after)) = (interrupts, tlb_interrupts(other)) {
        println!(
            "BENCHCOUNT group=tlb_shootdown_linux rounds={rounds} ipis={}",
            after.saturating_sub(before)
        );
    }
    stop.store(true, Ordering::Relaxed);
    spinner.join().expect("the spinning thread panicked");
}

/// Runs `body` `count` times here and on `other` at once; prints what one
/// run cost here.
fn parallel(name: &str, other: usize, count: u64, body: fn()) {
    let ready = Arc::new(Barrier::new(2));
    let go = ready.clone();
    let partner = std::thread::spawn(move || {
        pin(other);
        go.wait();
        for _ in 0..count {
            body();
        }
    });
    ready.wait();
    let started = Instant::now();
    for _ in 0..count {
        body();
    }
    print_line(name, started, count);
    partner.join().expect("the other thread panicked");
}

/// Where the file benchmarks work: a tmpfs. In the virtual machine that is
/// the root, the initramfs; on the host, /dev/shm, or the temporary
/// directory where there is none.
fn file_dir() -> PathBuf {
    if std::process::id() == 1 {
        return PathBuf::from("/bench");
    }
    let shm = std::path::Path::new("/dev/shm");
    let base = if shm.is_dir() { shm.to_path_buf() } else { std::env::temp_dir() };
    base.join(format!("linux-bench-{}", std::process::id()))
}

/// The file benchmarks: openat(2) and close(2) of a file by its name
/// beneath a directory, as Radix's paths always are; fstatat(2); and
/// pread(2) and pwrite(2) of the file's first 4 KiB, which is in memory.
fn files(rounds: u64) {
    let dir = file_dir();
    let path = dir.join("bench");
    std::fs::create_dir_all(&dir).expect("cannot make the benchmark directory");
    std::fs::write(&path, [0x5a; 4096]).expect("cannot write the benchmark file");
    let directory = std::fs::File::open(&dir).expect("cannot open the benchmark directory");
    let name = c"bench";
    report("file_open_close", rounds, || {
        let fd = fs::open_at(&directory, name, libc::O_RDONLY).expect("openat");
        drop(fd);
    });
    report("file_stat", rounds, || {
        fs::stat_at(&directory, name).expect("fstatat");
    });
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&path)
        .expect("cannot open the benchmark file");
    let mut buffer = [0x5au8; 4096];
    report("file_read_4k", rounds, || {
        let read = file.read_at(&mut buffer, 0).expect("pread");
        assert_eq!(read, 4096, "pread returned {read}");
    });
    report("file_write_4k", rounds, || {
        let wrote = file.write_at(&buffer, 0).expect("pwrite");
        assert_eq!(wrote, 4096, "pwrite wrote {wrote}");
    });
    drop(file);
    drop(directory);
    let _ = std::fs::remove_dir_all(&dir);
}

/// Runs `body` `count` times and prints what one run cost.
fn report(name: &str, count: u64, mut body: impl FnMut()) {
    let started = Instant::now();
    for _ in 0..count {
        body();
    }
    print_line(name, started, count);
}

/// The TLB shootdown interrupts CPU `cpu` has taken: its column of the
/// `TLB` line of `/proc/interrupts`, or `None` where there is none.
fn tlb_interrupts(cpu: usize) -> Option<u64> {
    tlb_column(&std::fs::read_to_string("/proc/interrupts").ok()?, cpu)
}

/// CPU `cpu`'s count on the `TLB` line of `/proc/interrupts`' `text`.
fn tlb_column(text: &str, cpu: usize) -> Option<u64> {
    let mut lines = text.lines();
    let column = lines.next()?.split_whitespace().position(|name| name == format!("CPU{cpu}"))?;
    let line = lines.find(|line| line.trim_start().starts_with("TLB:"))?;
    line.split_whitespace().nth(1 + column)?.parse().ok()
}

/// Mounts a filesystem of kind `kind` on `at`, which the virtual
/// machine's init finds empty: `/proc`, `/sys`, and `/dev` as the kernel's
/// devtmpfs, where its disk is.
fn mount(kind: &str, at: &str) {
    let _ = std::fs::create_dir_all(at);
    if let Err(err) = linux::mount(kind, Path::new(at), kind, 0, "") {
        eprintln!("linux-bench: cannot mount {at}: {err}");
    }
}

/// The text of the file at `path` without its final newline, or `None`
/// where the file is not there or cannot be read.
fn read_sysfs(path: &str) -> Option<String> {
    Some(std::fs::read_to_string(path).ok()?.trim().to_string())
}

/// A frequency in kHz, as the files of cpufreq give it, in MHz.
fn megahertz(path: &str) -> Option<u64> {
    Some(read_sysfs(path)?.parse::<u64>().ok()? / 1000)
}

/// The folders of the CPUs' frequency policies (`policy0`, `policy1`, ...),
/// in the order of their numbers.
fn policies() -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(CPUFREQ) else { return Vec::new() };
    let mut found: Vec<(u32, PathBuf)> = entries
        .filter_map(|entry| {
            let path = entry.ok()?.path();
            let number = path.file_name()?.to_str()?.strip_prefix("policy")?.parse().ok()?;
            Some((number, path))
        })
        .collect();
    found.sort();
    found.into_iter().map(|(_, path)| path).collect()
}

/// Whether Linux lets the CPUs run above their base frequency, from
/// intel_pstate's `no_turbo` or cpufreq's `boost`; `None` where neither is
/// there.
fn turbo_allowed() -> Option<bool> {
    if let Some(no_turbo) = read_sysfs(&format!("{INTEL_PSTATE}/no_turbo")) {
        return Some(no_turbo == "0");
    }
    Some(read_sysfs(&format!("{CPUFREQ}/boost"))? == "1")
}

/// Prints the facts of the first CPU's frequency policy on one line, so a
/// run's output says which clock Linux ran the rows at.
fn report_cpu_frequency() {
    let policy = format!("{CPUFREQ}/policy0");
    if !Path::new(&policy).exists() {
        println!(
            "linux-bench: CPU frequency: no cpufreq driver; the CPUs run at the clock the \
             firmware or the host gives them"
        );
        return;
    }
    let file = |name: &str| read_sysfs(&format!("{policy}/{name}")).unwrap_or_default();
    let mut line = file("scaling_driver");
    if let Some(status) = read_sysfs(&format!("{INTEL_PSTATE}/status")) {
        line += &format!(" ({status})");
    }
    line += &format!(", {}", file("scaling_governor"));
    if let Some(preference) = read_sysfs(&format!("{policy}/energy_performance_preference")) {
        line += &format!(", energy preference {preference}");
    }
    let mhz = |name: &str| megahertz(&format!("{policy}/{name}")).unwrap_or_default();
    line += &format!(
        ", {} to {} MHz of {} to {}",
        mhz("scaling_min_freq"),
        mhz("scaling_max_freq"),
        mhz("cpuinfo_min_freq"),
        mhz("cpuinfo_max_freq")
    );
    if let Some(base) = megahertz(&format!("{policy}/base_frequency")) {
        line += &format!(", base {base} MHz");
    }
    if let Some(turbo) = turbo_allowed() {
        line += if turbo { ", turbo on" } else { ", turbo off" };
    }
    println!("linux-bench: CPU frequency: {line}");
}

/// Prints the idle driver and governor, and the first CPU's idle states in
/// the order Linux enters them as it goes deeper.
fn report_idle() {
    let driver = read_sysfs(&format!("{CPUIDLE}/current_driver")).filter(|name| name != "none");
    let Some(driver) = driver else {
        println!("linux-bench: idle: no cpuidle driver");
        return;
    };
    let governor = read_sysfs(&format!("{CPUIDLE}/current_governor"))
        .or_else(|| read_sysfs(&format!("{CPUIDLE}/current_governor_ro")))
        .unwrap_or_default();
    let cpu0 = "/sys/devices/system/cpu/cpu0/cpuidle";
    let mut states: Vec<(u32, String)> = std::fs::read_dir(cpu0)
        .into_iter()
        .flatten()
        .filter_map(|entry| {
            let path = entry.ok()?.path();
            let number = path.file_name()?.to_str()?.strip_prefix("state")?.parse().ok()?;
            Some((number, read_sysfs(path.join("name").to_str()?)?))
        })
        .collect();
    states.sort();
    let names: Vec<String> = states.into_iter().map(|(_, name)| name).collect();
    println!("linux-bench: idle: {driver}, {governor}: {}", names.join(" "));
}

/// Holds every CPU at its base frequency: turbo off, the performance
/// governor and the minimum at the maximum, as Dukovo's `benchmark`
/// profile does (ADR 0557), then prints what the policies read back as.
/// It runs before any benchmark, so linux-bench's own rows run at the
/// held clock too.
fn hold_clock() {
    let policies = policies();
    if policies.is_empty() {
        println!("linux-bench: CPU frequency not held: no cpufreq driver");
        return;
    }
    let turbo_off = if Path::new(&format!("{INTEL_PSTATE}/no_turbo")).exists() {
        Some((format!("{INTEL_PSTATE}/no_turbo"), "1"))
    } else if Path::new(&format!("{CPUFREQ}/boost")).exists() {
        Some((format!("{CPUFREQ}/boost"), "0"))
    } else {
        None
    };
    if let Some((path, value)) = turbo_off
        && let Err(err) = std::fs::write(&path, value)
    {
        println!("linux-bench: CPU frequency not held: cannot write {path}: {err}");
        return;
    }
    for policy in &policies {
        if let Err(why) = hold_policy(policy) {
            println!("linux-bench: CPU frequency not held: {why}");
            return;
        }
    }
    // A write to `scaling_min_freq` changes a frequency request whose
    // policy update runs later, as deferred work, so the limits may read
    // as they were for a moment after the writes.
    let mut tries = 1;
    let (held, cpus) = loop {
        match held_clock(&policies) {
            Ok(held) => break held,
            Err(_) if tries < HOLD_TRIES => {
                tries += 1;
                std::thread::sleep(HOLD_PAUSE);
            }
            Err(why) => {
                println!("linux-bench: CPU frequency not held: {why}");
                return;
            }
        }
    };
    let turbo = if turbo_allowed() == Some(false) { "turbo off, " } else { "" };
    println!("linux-bench: CPU frequency held at {held} MHz on {cpus} CPUs ({turbo}performance)");
}

/// How many times, [`HOLD_PAUSE`] apart, [`hold_clock`] reads the limits
/// back before it says the clock is not held: about a second in all.
const HOLD_TRIES: u32 = 20;
const HOLD_PAUSE: std::time::Duration = std::time::Duration::from_millis(50);

/// The frequency every policy in `policies` is held at, as its minimum
/// and maximum read back, and how many CPUs they cover; or why they are
/// not held at one frequency.
fn held_clock(policies: &[PathBuf]) -> Result<(u64, usize), String> {
    let mut held = None;
    let mut cpus = 0;
    for policy in policies {
        let limits = (
            megahertz(&format!("{}/scaling_min_freq", policy.display())),
            megahertz(&format!("{}/scaling_max_freq", policy.display())),
        );
        let (Some(min), Some(max)) = limits else {
            return Err(format!("cannot read the limits of {}", policy.display()));
        };
        if min != max {
            return Err(format!("{} may run {min} to {max} MHz", policy.display()));
        }
        if let Some(other) = held.filter(|&mhz| mhz != min) {
            return Err(format!("the policies differ: {other} MHz and {min} MHz"));
        }
        held = Some(min);
        cpus += read_sysfs(&format!("{}/related_cpus", policy.display()))
            .map_or(1, |list| list.split_whitespace().count());
    }
    held.map(|mhz| (mhz, cpus)).ok_or_else(|| "no cpufreq policy".to_string())
}

/// Gives one frequency policy the performance governor and its maximum
/// as its minimum. The governor alone has intel_pstate ask the CPU for
/// its highest level as the minimum too, but `scaling_min_freq` does not
/// show it; setting the minimum does the same on every driver, and it
/// can be read back.
fn hold_policy(policy: &Path) -> Result<(), String> {
    let path = |name: &str| policy.join(name);
    let write = |name: &str, value: &str| {
        std::fs::write(path(name), value)
            .map_err(|err| format!("cannot write {}: {err}", path(name).display()))
    };
    write("scaling_governor", "performance")?;
    let max = std::fs::read_to_string(path("scaling_max_freq"))
        .map_err(|err| format!("cannot read {}: {err}", path("scaling_max_freq").display()))?;
    write("scaling_min_freq", max.trim())
}

/// Loads the kernel modules in [`MODULES`], in the order of their names.
fn load_modules() {
    let Ok(entries) = std::fs::read_dir(MODULES) else { return };
    let mut paths: Vec<PathBuf> = entries.filter_map(|entry| Some(entry.ok()?.path())).collect();
    paths.sort();
    for path in paths {
        let file = match std::fs::File::open(&path) {
            Ok(file) => file,
            Err(err) => {
                eprintln!("linux-bench: cannot open {}: {err}", path.display());
                continue;
            }
        };
        let compressed = path.extension().is_some_and(|e| e == "zst" || e == "xz" || e == "gz");
        match linux::load_module(&file, compressed) {
            Err(err) if err.raw_os_error() != Some(libc::EEXIST) => {
                eprintln!("linux-bench: cannot load {}: {err}", path.display());
            }
            _ => {}
        }
    }
}

/// Mounts the Btrfs disk [`BTRFS_DISK`] on [`BTRFS_AT`], as Linux's
/// `mount -o compress=zstd:1` does (Fedora's default), so that what the
/// benchmarks write is compressed as Radix compresses it; returns whether
/// it did.
fn mount_btrfs() -> bool {
    if !Path::new(BTRFS_DISK).exists() {
        eprintln!("linux-bench: the virtual machine has no {BTRFS_DISK}, for the Btrfs benchmarks");
        return false;
    }
    let _ = std::fs::create_dir_all(BTRFS_AT);
    let mounted = linux::mount(BTRFS_DISK, Path::new(BTRFS_AT), "btrfs", 0, "compress=zstd:1");
    if let Err(err) = &mounted {
        eprintln!("linux-bench: cannot mount {BTRFS_DISK}: {err}");
    }
    mounted.is_ok()
}

fn print_line(name: &str, started: Instant, count: u64) {
    let ns = started.elapsed().as_nanos() as f64 / count as f64;
    println!("BENCH name={name} ns={ns:.2} iterations={count}");
}

/// The calling thread, pinned already, and one pinned to `other`.
struct Pair {
    other: usize,
    rounds: u64,
}

/// The two ends of a round trip: the asking thread writes to `ask.1` and
/// reads its answer from `ask.0`; the answering one reads from `answer.0`
/// and writes back to `answer.1`.
type Ends<R, W> = ((R, W), (R, W));

impl Pair {
    /// Times `rounds` 8-byte round trips between the two threads.
    fn run<R, W>(&self, name: &str, ends: Ends<R, W>)
    where
        R: Read + Send + 'static,
        W: Write + Send + 'static,
    {
        let ((mut ask_read, mut ask_write), (mut answer_read, mut answer_write)) = ends;
        let ready = Arc::new(Barrier::new(2));
        let (other, rounds, go) = (self.other, self.rounds, ready.clone());
        let answerer = std::thread::spawn(move || {
            pin(other);
            go.wait();
            answer(&mut answer_read, &mut answer_write, rounds);
        });
        ready.wait();
        let started = Instant::now();
        ask(&mut ask_read, &mut ask_write, self.rounds);
        print_line(name, started, self.rounds);
        answerer.join().expect("the answering thread panicked");
    }

    /// Times `rounds` futex ping-pongs: a word that says whose turn it is,
    /// each thread waking the other after it hands the turn over.
    fn futex(&self, name: &str) {
        let turn = Arc::new(AtomicU32::new(0));
        let ready = Arc::new(Barrier::new(2));
        let (other, rounds, go, theirs) = (self.other, self.rounds, ready.clone(), turn.clone());
        let answerer = std::thread::spawn(move || {
            pin(other);
            go.wait();
            for _ in 0..rounds {
                while theirs.load(Ordering::Acquire) == 0 {
                    linux::futex_wait(&theirs, 0);
                }
                theirs.store(0, Ordering::Release);
                linux::futex_wake(&theirs, 1);
            }
        });
        ready.wait();
        let started = Instant::now();
        for _ in 0..self.rounds {
            turn.store(1, Ordering::Release);
            linux::futex_wake(&turn, 1);
            while turn.load(Ordering::Acquire) == 1 {
                linux::futex_wait(&turn, 1);
            }
        }
        print_line(name, started, self.rounds);
        answerer.join().expect("the answering thread panicked");
    }
}

/// Asks `rounds` 8-byte questions and checks each answer.
fn ask(read: &mut impl Read, write: &mut impl Write, rounds: u64) {
    let mut word = [0u8; 8];
    // Never 0, which an eventfd would not pass on.
    for message in 1..=rounds {
        write.write_all(&message.to_le_bytes()).expect("writing the question");
        read.read_exact(&mut word).expect("reading the answer");
        assert_eq!(u64::from_le_bytes(word), message, "a wrong answer");
    }
}

/// Answers `rounds` 8-byte questions with themselves.
fn answer(read: &mut impl Read, write: &mut impl Write, rounds: u64) {
    let mut word = [0u8; 8];
    for _ in 0..rounds {
        read.read_exact(&mut word).expect("reading the question");
        write.write_all(&word).expect("writing the answer");
    }
}

/// Pipe round trips on both CPUs at once, a pair of threads on each, so
/// the pairs share nothing; prints what a round trip of the pair here
/// cost. Radix's `endpoint_call_parallel` does the same with endpoints.
fn pipe_roundtrip_parallel(here: usize, other: usize, rounds: u64) {
    let ready = Arc::new(Barrier::new(4));
    let mut threads = Vec::new();
    let mut askers = Vec::new();
    for cpu in [here, other] {
        let (asker, (mut read, mut write)) = pipes();
        let go = ready.clone();
        threads.push(std::thread::spawn(move || {
            pin(cpu);
            go.wait();
            answer(&mut read, &mut write, rounds);
        }));
        askers.push(asker);
    }
    let [(mut read, mut write), (mut other_read, mut other_write)] =
        <[_; 2]>::try_from(askers).expect("one asker per CPU");
    let go = ready.clone();
    threads.push(std::thread::spawn(move || {
        pin(other);
        go.wait();
        ask(&mut other_read, &mut other_write, rounds);
    }));
    ready.wait();
    let started = Instant::now();
    ask(&mut read, &mut write, rounds);
    print_line("pipe_roundtrip_parallel", started, rounds);
    for thread in threads {
        thread.join().expect("a round-trip thread panicked");
    }
}

fn pipes() -> Ends<std::io::PipeReader, std::io::PipeWriter> {
    let question = std::io::pipe().expect("cannot make a pipe");
    let answer = std::io::pipe().expect("cannot make a pipe");
    ((answer.0, question.1), (question.0, answer.1))
}

fn sockets() -> Ends<UnixStream, UnixStream> {
    let (asker, answerer) = UnixStream::pair().expect("cannot make a socket pair");
    let asker_write = asker.try_clone().expect("cannot clone a socket");
    let answerer_write = answerer.try_clone().expect("cannot clone a socket");
    ((asker, asker_write), (answerer, answerer_write))
}

fn eventfds() -> Ends<std::fs::File, std::fs::File> {
    let make = || {
        let fd = linux::event_counter().expect("cannot make an eventfd");
        let clone = fd.try_clone().expect("cannot duplicate an eventfd");
        (std::fs::File::from(fd), std::fs::File::from(clone))
    };
    // An eventfd adds each 8-byte write to its counter, and a read takes
    // the counter: with one message in flight, what was written.
    let (question, answer) = (make(), make());
    ((answer.0, question.1), (question.0, answer.1))
}

/// Brings the loopback interface up, as the virtual machine's process 1
/// finds it down: false if it could not.
fn loopback_up() -> bool {
    if let Err(err) = linux::bring_up("lo") {
        eprintln!("linux-bench: cannot bring lo up: {err}");
        return false;
    }
    true
}

/// Runs the POSIX benchmarks at `program`, which print their own lines,
/// on every CPU this process may run on: it pins itself and its threads
/// as it needs, as on Radix, given each option of `disks` with its path
/// (the disks, and git). With `network` they time the loopback
/// interface too.
fn posix_bench(program: &Path, cpus: &[usize], disks: &[(&str, &Path)], network: bool) {
    allow(cpus);
    std::io::stdout().flush().ok();
    let mut command = std::process::Command::new(program);
    if network {
        command.arg("--network");
    }
    for (option, disk) in disks {
        command.arg(option).arg(disk);
    }
    match command.status() {
        Ok(status) if status.success() => {}
        Ok(status) => eprintln!("linux-bench: {} failed: {status}", program.display()),
        Err(err) => eprintln!("linux-bench: cannot run {}: {err}", program.display()),
    }
}

/// Runs the server benchmarks at [`SERVER_BENCH`] on every CPU of `cpus`,
/// if the initramfs holds them (an empty file or none stands for no
/// servers). The servers keep their files in `/tmp` and take shared
/// memory from `/dev/shm`, so both are made tmpfs, as on Radix; the
/// benchmarks drive the servers over the loopback interface.
fn server_bench(cpus: &[usize]) {
    if !Path::new(SERVER_BENCH).metadata().is_ok_and(|meta| meta.len() > 0) {
        return;
    }
    if !loopback_up() {
        eprintln!("linux-bench: the loopback interface is down, so no server benchmarks");
        return;
    }
    // A tmpfs holds its root folder at mode 1777 unless told otherwise.
    mount("tmpfs", "/tmp");
    mount("tmpfs", "/dev/shm");
    allow(cpus);
    std::io::stdout().flush().ok();
    let mut command = std::process::Command::new(SERVER_BENCH);
    command.args(SERVERS).env_clear().env("TZ", "UTC").env("PATH", "/usr/bin:/bin");
    match command.status() {
        Ok(status) if status.success() => {}
        Ok(status) => eprintln!("linux-bench: {SERVER_BENCH} failed: {status}"),
        Err(err) => eprintln!("linux-bench: cannot run {SERVER_BENCH}: {err}"),
    }
}

/// Lets the calling thread run on each of `cpus` again.
fn allow(cpus: &[usize]) {
    linux::set_allowed_cpus(cpus).expect("sched_setaffinity");
}

/// Pins the calling thread to `cpu`.
fn pin(cpu: usize) {
    linux::set_allowed_cpus(&[cpu])
        .unwrap_or_else(|err| panic!("cannot pin a thread to CPU {cpu}: {err}"));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_a_cpus_tlb_interrupts() {
        let text = "           CPU0       CPU2       CPU3\n\
                    \x20 8:          0          0          0   IO-APIC    8-edge      rtc0\n\
                    TLB:      78240      75740      76113   TLB shootdowns\n";
        assert_eq!(tlb_column(text, 0), Some(78240));
        assert_eq!(tlb_column(text, 2), Some(75740));
        assert_eq!(tlb_column(text, 3), Some(76113));
        // A CPU that is not online has no column.
        assert_eq!(tlb_column(text, 1), None);
        assert_eq!(tlb_column("           CPU0\n  8:  0  rtc0\n", 0), None);
    }
}
