// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Spark, the first program (Phases 6, 7 and 9, spark(8)).
//!
//! The kernel starts Spark from the boot archive with the root of the
//! system's tree in `SLOT_ROOT` (an empty tmpfs), and from
//! `SLOT_FIRST_EXTRA` on a memory object for the programs it starts, a
//! capability to the `services` resource domain, the power
//! capability, a network capability for all of the network's ports
//! (radix-network(2)), the platform capability (radix-platform(2)) and
//! the names capability (radix-names(2), ADR 0214).
//! It acts for root, with the bypass right on its root, as the programs
//! and services it starts do. Spark v2:
//!
//! 1. builds the directory hierarchy on the root (the plan's spec 7.1,
//!    with plain names, ADR 0113): `/system`, `/applications`, `/users`,
//!    `/shared`, `/tmp`, `/devices`, `/processes`, `/kernel` and
//!    `/volumes`; and mounts the boot archive read-only on `/system`,
//!    tmpfs on `/system/settings`, `/system/state`, `/system/live` and
//!    `/tmp`, and the device, process and kernel trees; makes
//!    `/system/state/tmp`; makes the Unix names (`/bin`, `/etc`, `/dev`,
//!    `/proc` and the rest) links to them, hidden from listings by the
//!    root's `.hidden` (the boot archive has `/system`'s own, ADR 0114);
//!    then copies each default setting in
//!    `/system/resources/default-settings` that `/system/settings` lacks
//!    into it;
//! 2. mounts the FAT filesystem of each disk in `/devices` that holds one
//!    (each partition, or the whole disk if it has none) at
//!    `/volumes/<name>`, and puts Dukovo's root on a disk on `/system`
//!    instead of the boot archive: the root whose Btrfs UUID the kernel's
//!    command line names as `root=UUID=<uuid>`, as an installed system's
//!    boot menu does (ADR 0307), or with none named the first disk that
//!    holds one. An installed system starts from an archive of Spark
//!    alone, so when the root it names cannot be found, Spark says so
//!    and starts nothing; that is the first place to look when an
//!    installed system stops after Spark. The swap partitions of the
//!    disk that holds the root are turned on (`swap`, ADR 0311), and
//!    left out of the filesystems mounted. A partition of the type
//!    "Dukovo live", an install stick's DUKOVO_LIVE, and the install DVD,
//!    a disc whose ISO 9660 volume is labelled DUKOVO_LIVE (ADR 0433), are
//!    mounted read-only and are never the root. With `live` on the
//!    command line, as the install stick's and the DVD's boot menus start
//!    the live system (ADR 0308), no disk is the root and none is mounted
//!    but such a partition or disc: the machine's disks are the owner's,
//!    for the installer or mount(8);
//! 3. listens for requests at `/system/live/spark/control` and, with no
//!    arguments (a normal boot), starts the services the TOML files in
//!    `/system/settings/spark/services` describe (`services`,
//!    `supervisor`), converting the files of before Phase 13 first;
//! 4. with arguments, as the boot tests boot it, starts one program
//!    from `/system`, the one they name, with the arguments after it, and
//!    starts no services unless `--services` comes first, or
//!    `--service-dir DIR` to start those that DIR describes instead. The
//!    program gets the root directory to read and write and logs over the
//!    kernel log; Spark waits for it to exit, reaps it and checks that all
//!    of its memory came back. With `--posix` before the program, it is a
//!    POSIX one (Phase 8, radix-posix(7)): Spark runs it with `exec`, as
//!    its child, with a pseudo-terminal as its standard input, output and
//!    error, and copies what it writes there to the kernel log. With
//!    `--network` before those, the program gets Spark's network
//!    capability too, in `SLOT_NETWORK`; with `--firewall`, a POSIX one
//!    gets the firewall capability in `SLOT_FIREWALL`, for firewall(8), as
//!    the `network-pair` boot test needs; and with `--names`, a POSIX one
//!    gets the names capability as its descriptor 3, as the `principals`
//!    boot test needs.
//!
//! Then it supervises: it starts services again when they end, answers
//! the control socket and shuts the machine down when asked. Spark must
//! not end: the kernel stops if it does.

#![no_std]
#![no_main]
#![forbid(unsafe_code)]

extern crate alloc;

mod services;
mod supervisor;
mod swap;
mod views;

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use core::fmt::Write as _;

use libradix::abi::capability::{self, ObjectKind, Rights};
use libradix::abi::directory::{FileKind, FilesystemKind, MountFlags, OpenFlags};
use libradix::abi::errors::File as FileError;
use libradix::abi::errors::Kernel as KernelError;
use libradix::abi::notification::WaitFlags;
use libradix::abi::startup::{
    SLOT_FD_BASE, SLOT_FIREWALL, SLOT_FIRST_EXTRA, SLOT_LOG, SLOT_NETWORK, SLOT_PROCESS,
    SLOT_RANDOM, SLOT_ROOT, Startup,
};
use libradix::abi::{file, memory, process, syscalls, terminal};
use radix_rt::fs::{Dir, File};
use radix_rt::spawn::{self, Spawn};
use radix_rt::{SYS, args, entry, println, slots};
use supervisor::{POSIX_SLOTS, Program, SERVICES_DIR, Supervisor, exec_block};
use system_slots::Slot;
use system_slots::cmdline::Boot;
use system_slots::restore::{self, Seen, Step};

entry!(main);

/// The memory the program it starts gets: 2^order pages.
const PROGRAM_MEMORY_ORDER: u32 = 9;

/// The most arguments it passes on.
const ARGS_MAX: usize = 16;

/// How long an installed system whose root no disk holds waits before it
/// restarts, in seconds: long enough to read why, and each start that
/// does not finish counts towards Rescue (ADR 0460).
const ROOT_MISSING_RESTART: u64 = 30;

/// Set when the kernel's command line names a root no disk holds, as an
/// installed system's boot menu does.
static ROOT_MISSING: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

/// Where the kernel gave Spark its network capability, after the memory
/// for programs, the services' domain and the power capability.
const NETWORK: u32 = SLOT_FIRST_EXTRA + 3;

/// Where the kernel gave Spark the names capability, after the network
/// and the platform capability.
const NAMES: u32 = SLOT_FIRST_EXTRA + 5;

/// Where the kernel gave Spark the firewall capability (ADR 0334), after
/// the names, the clock and the root device capability.
const FIREWALL: u32 = SLOT_FIRST_EXTRA + 8;

/// The directories of the root, every one a mount point or a place for
/// one (the plan's spec 7.1), named so that they need no explaining
/// (ADR 0113).
const HIERARCHY: [&str; 9] = [
    "system",
    "applications",
    "users",
    "shared",
    "tmp",
    "devices",
    "processes",
    "kernel",
    "volumes",
];

/// Symbolic links from the names Unix programs and scripts look for to
/// where Dukovo keeps those things: `/bin/sh`, `#!/usr/bin/env`,
/// `/etc/hosts`, `/dev/null`, `/proc/mounts`, `/var/run` (ADRs 0081,
/// 0113 and 0114). `/usr/bin` and `/usr/lib` are `/system`'s links to
/// `commands` and `libraries`, which the boot archive holds.
const LINKS: [(&str, &str); 10] = [
    ("bin", "system/commands"),
    ("dev", "devices"),
    ("etc", "system/settings"),
    ("proc", "processes"),
    ("run", "system/live"),
    ("sbin", "system/commands"),
    ("sys", "kernel"),
    ("usr", "system"),
    ("var", "system/state"),
    ("system/state/run", "../live"),
];

/// What is mounted where before the disks, in this order. `/volumes` is
/// memory, as Linux's `/run/media` is: what is mounted there is for this
/// boot, and volumed's service file hands it the root of that tmpfs, the
/// one place beneath which a capability can name the mounts it makes
/// (ADR 0217). `/devices/shm` is the memory filesystem of `shm_open`,
/// `sem_open` and System V IPC (ADR 0490), as Linux's `/dev/shm`.
const MOUNTS: [(&str, FilesystemKind, MountFlags); 6] = [
    ("tmp", FilesystemKind::Tmpfs, MountFlags::NONE),
    ("volumes", FilesystemKind::Tmpfs, MountFlags::NONE),
    ("devices", FilesystemKind::Dev, MountFlags::NONE),
    ("devices/shm", FilesystemKind::Shm, MountFlags::NONE),
    ("processes", FilesystemKind::Proc, MountFlags::NONE),
    ("kernel", FilesystemKind::Sys, MountFlags::NONE),
];

/// What `/system` is with no Dukovo root on a disk, in this order: the
/// boot archive, read-only, and memory for what changes, which the next
/// boot starts without. `/system/settings`, `/system/state`,
/// `/system/live` and `/system/boot` are directories of the boot
/// archive's, for this.
const IN_MEMORY: [(&str, FilesystemKind, MountFlags); 4] = [
    ("system", FilesystemKind::Bootfs, MountFlags::READ_ONLY),
    ("system/settings", FilesystemKind::Tmpfs, MountFlags::NONE),
    ("system/state", FilesystemKind::Tmpfs, MountFlags::NONE),
    ("system/live", FilesystemKind::Tmpfs, MountFlags::NONE),
];

/// The subvolumes of Dukovo's root on a disk and where each is mounted, in
/// this order (ADR 0137): the system, read-only, then what changes. The
/// system holds the directories the others are mounted on, as the boot
/// archive does, and `/system/live` is memory here too. The system is the
/// slot GRUB started, `@system.a` or `@system.b` ([`on_disk`], ADR 0451),
/// slot A's here.
const ON_DISK: [(&str, &str, MountFlags); 6] = [
    ("@system.a", "system", MountFlags::READ_ONLY),
    ("@settings", "system/settings", MountFlags::NONE),
    ("@state", "system/state", MountFlags::NONE),
    ("@applications", "applications", MountFlags::NONE),
    ("@users", "users", MountFlags::NONE),
    ("@shared", "shared", MountFlags::NONE),
];

/// [`ON_DISK`] with the system in `slot`.
fn on_disk(slot: Slot) -> [(&'static str, &'static str, MountFlags); 6] {
    let mut subvolumes = ON_DISK;
    subvolumes[0].0 = slot.subvolume();
    subvolumes
}

/// Directories of the mounted tmpfs that Unix programs expect there:
/// `/var/tmp`, for temporary files that are to outlive a restart, such
/// as vi's recovery files, once `/system/state` is on a disk.
const MOUNTED_DIRS: [&str; 1] = ["system/state/tmp"];

/// The directories with links in them, and the `.hidden` file each gets:
/// the links' names, one a line, which ls(1) and the shell's completion
/// leave out unless asked for everything, as the file manager will and
/// as GNOME Files and Nemo leave out what a `.hidden` names (ADR 0113).
const HIDDEN: [(&str, &str); 2] = [
    (".hidden", "bin\ndev\netc\nproc\nrun\nsbin\nsys\nusr\nvar\n"),
    ("system/state/.hidden", "run\n"),
];

fn main(start: &Startup) -> u64 {
    let number = process::info(&SYS, SLOT_PROCESS).map_or(0, |info| info.number);
    println!("spark: Spark v2 is process {number}, moved by {:#x}", start.image_bias);
    if let Some(mut supervisor) = run() {
        supervisor.run();
    }
    // Spark never exits: with nothing to supervise, it waits for good.
    println!("spark: nothing more to do; waiting");
    idle()
}

/// Builds the hierarchy and mounts the disks, then starts the services or
/// the program its arguments name; returns the supervision of them.
fn run() -> Option<Supervisor> {
    let Some(root) = Dir::root() else {
        println!("spark: the kernel gave no root directory");
        return None;
    };
    let mut words = args().skip(1).peekable();
    // `--no-volumes` leaves every disk's filesystems unmounted, for the
    // disk check, which writes over a whole disk (ADR 0124); `--compress
    // <disk>` has the Btrfs of a disk compress what is written to it
    // (ADR 0134); `--watchdog <seconds>` sets the hardware watchdog's
    // timeout, 0 to leave it off, and `--test-hang` has Spark stop
    // responding once it runs, for the watchdog's boot test (ADR 0143).
    let (mut volumes, mut compress) = (true, Vec::new());
    let (mut watchdog, mut hang) = (supervisor::WATCHDOG_SECONDS, false);
    loop {
        if words.next_if_eq(&"--no-volumes").is_some() {
            volumes = false;
        } else if words.next_if_eq(&"--compress").is_some() {
            compress.extend(words.next());
        } else if words.next_if_eq(&"--watchdog").is_some() {
            watchdog = words.next().and_then(|w| w.parse().ok()).unwrap_or(watchdog);
        } else if words.next_if_eq(&"--test-hang").is_some() {
            hang = true;
        } else {
            break;
        }
    }
    if !build(&root, volumes, &compress) {
        return None;
    }
    // An old service file an update finds is converted before the new
    // defaults come in, so that it is the one kept (ADR 0216).
    services::convert_old(&root, SERVICES_DIR);
    seed_settings(&root);
    let Ok(supervised) = Dir::at(root.slot()) else { return None };
    let mut supervisor = Supervisor::new(supervised);
    // `test-hang` among the kernel's words has Spark stop responding at
    // every start, as an update that hangs does (ADR 0452's boot test).
    supervisor.set_watchdog(watchdog, hang, kernel_flag(&root, "test-hang"));
    if rescue(&root) {
        supervisor.set_rescue();
    }
    // The start menu's Troubleshooting: only the essentials (ADR 0501).
    if kernel_flag(&root, "essentials") {
        supervisor.set_essentials();
    }
    supervisor.listen();
    if ROOT_MISSING.load(core::sync::atomic::Ordering::Relaxed) {
        supervisor.restart_in(ROOT_MISSING_RESTART);
    }
    let (mut with_services, mut services) = (false, SERVICES_DIR);
    loop {
        if words.next_if_eq(&"--services").is_some() {
            with_services = true;
        } else if words.next_if_eq(&"--service-dir").is_some() {
            with_services = true;
            services = words.next().unwrap_or(SERVICES_DIR).trim_start_matches('/');
        } else {
            break;
        }
    }
    let (mut posix, mut network, mut names, mut firewall) = (false, None, None, None);
    loop {
        if words.next_if_eq(&"--posix").is_some() {
            posix = true;
        } else if words.next_if_eq(&"--network").is_some() {
            network = Some(NETWORK);
        } else if words.next_if_eq(&"--firewall").is_some() {
            firewall = Some(FIREWALL);
        } else if words.next_if_eq(&"--names").is_some() {
            names = Some(NAMES);
        } else {
            break;
        }
    }
    let path = words.next();
    if with_services || path.is_none() {
        supervisor.start_services(services);
    }
    let Some(path) = path else { return Some(supervisor) };
    let mut rest = [""; ARGS_MAX];
    let mut count = 0;
    for word in words.take(ARGS_MAX) {
        rest[count] = word;
        count += 1;
    }
    if posix {
        match run_posix(&root, path, &rest[..count], Given { network, names, firewall }) {
            Ok(program) => supervisor.watch_program(program),
            Err(what) => println!("spark: could not start {path}: {what}"),
        }
        return Some(supervisor);
    }
    run_native(&root, path, &rest[..count], network);
    supervisor.note_idle();
    Some(supervisor)
}

/// Runs the native program at `path` with `args`, with the network
/// capability in `network` if there is one, and waits for it to end.
fn run_native(root: &Dir, path: &str, args: &[&str], network: Option<u32>) {
    let image = match root.read_file(path.trim_start_matches('/')) {
        Ok(image) => image,
        Err(err) => {
            println!("spark: cannot read {path}: {err}");
            return;
        }
    };
    let pool = SLOT_FIRST_EXTRA;
    let is_memory = capability::info(&SYS, pool).is_ok_and(|info| info.kind == ObjectKind::Memory);
    if !is_memory {
        println!("spark: the kernel gave no memory for programs");
        return;
    }
    let spawn = Spawn {
        path,
        image: &image,
        args,
        pool,
        memory_order: PROGRAM_MEMORY_ORDER,
        share: &[],
        // The command line is the machine owner's: mounting or replacing
        // a disk needs control, which a POSIX program it names has too,
        // and it acts for root past the files' owners and modes.
        root: Some((
            root.slot(),
            Rights::READ.union(Rights::WRITE).union(Rights::CONTROL).union(Rights::BYPASS),
        )),
        network: network.map(|slot| (slot, Rights::ALL)),
    };
    let child = match spawn::spawn(&spawn) {
        Ok(child) => child,
        Err(err) => {
            println!("spark: could not start {path}: {err}");
            return;
        }
    };
    let number = child.number();
    println!("spark: started {path} as process {number}");
    match child.wait() {
        Ok(code) => println!("spark: process {number} ({path}) exited with code {code:#x}"),
        Err(err) => println!("spark: waiting for process {number} failed: {err}"),
    }
    child.reap();
    match memory::info(&SYS, pool) {
        Ok(info) if info.used == 0 => {
            println!("spark: reaped process {number}; all of its memory came back");
        }
        Ok(info) => {
            println!("spark: reaped process {number}, but {} pages are still used", info.used);
        }
        Err(status) => {
            println!("spark: reaped process {number}; memory info failed: {:#x}", status.0);
        }
    }
}

/// The capabilities of Spark's that a POSIX program it runs gets, by
/// their slots in Spark.
#[derive(Clone, Copy)]
struct Given {
    /// The network capability, for SLOT_NETWORK.
    network: Option<u32>,
    /// The names capability, for descriptor 3.
    names: Option<u32>,
    /// The firewall capability, for SLOT_FIREWALL.
    firewall: Option<u32>,
}

/// Starts the POSIX program at `path` with `args` (radix-posix(7)) as
/// Spark's child, on a new pseudo-terminal whose output goes to the
/// kernel log, with the capabilities `given` names.
fn run_posix(root: &Dir, path: &str, args: &[&str], given: Given) -> Result<Program, String> {
    let Given { network, names, firewall } = given;
    let (master, terminal) =
        open_pty(root).map_err(|what| format!("cannot make a pseudo-terminal: {what}"))?;
    let program = root
        .open(path.trim_start_matches('/'), OpenFlags::NONE, Rights::READ)
        .map_err(|err| format!("cannot open {path}: {err}"))?;
    let child = slots::take().ok_or("no free slot")?;
    let made = process::create(&SYS, SLOT_PROCESS, ObjectKind::Process, POSIX_SLOTS, child);
    if let Err(status) = made {
        slots::give_back(child);
        return Err(format!("cannot make a process: {:#x}", status.0));
    }
    // The root, the random source and the log as Spark has them, and the
    // terminal as descriptors 0, 1 and 2, all with the copy right, which
    // `fork` needs to copy them.
    let mut shares = [root.slot(), SLOT_RANDOM, SLOT_LOG]
        .into_iter()
        .zip([SLOT_ROOT, SLOT_RANDOM, SLOT_LOG])
        .chain(network.map(|slot| (slot, SLOT_NETWORK)))
        .chain(firewall.map(|slot| (slot, SLOT_FIREWALL)))
        .chain((0..3).map(|fd| (terminal.slot(), SLOT_FD_BASE + fd)))
        .chain(names.map(|slot| (slot, SLOT_FD_BASE + 3)));
    let shared = shares.try_for_each(|(from, to)| {
        process::share(&SYS, child, from, to, Rights::ALL, 0).map_err(|status| (to, status))
    });
    let started = shared
        .map_err(|(to, status)| format!("sharing slot {to} failed: {:#x}", status.0))
        .and_then(|()| {
            process::exec(&SYS, child, program.slot(), &exec_block(path, args, &[]))
                .map_err(|status| format!("exec failed: {:#x}", status.0))
        });
    // The terminal is the program's now: the master reads its end once the
    // program and its children have closed it.
    drop(terminal);
    drop(program);
    let number = process::info(&SYS, child).map_or(0, |info| info.number);
    // The child stays in the tree after its capability goes, for `wait`.
    let _ = capability::delete(&SYS, child);
    slots::give_back(child);
    started?;
    println!("spark: started {path} as process {number}, on a pseudo-terminal");
    Ok(Program { path: path.into(), number, master, line: Vec::new(), hung_up: false })
}

/// Opens a new pseudo-terminal: its master, and the terminal (which does
/// not become Spark's controlling terminal).
fn open_pty(root: &Dir) -> Result<(File, File), String> {
    let rw = Rights::READ.union(Rights::WRITE);
    let master = root.open("devices/ptmx", OpenFlags::NONE, rw).map_err(|err| format!("{err}"))?;
    let mut number = [0u8; 4];
    let request = |request, buffer: &mut [u8; 4]| {
        let address = buffer.as_mut_ptr() as u64;
        file::control(&SYS, master.slot(), request, 0, address, 4)
            .map_err(|status| format!("request {request:#x} failed: {:#x}", status.0))
    };
    request(terminal::GET_PTY_NUMBER, &mut number)?;
    request(terminal::SET_PTY_LOCK, &mut [0; 4])?;
    let path = format!("devices/pts/{}", u32::from_le_bytes(number));
    let terminal =
        root.open(&path, OpenFlags::NO_TERMINAL, rw).map_err(|err| format!("/{path}: {err}"))?;
    Ok((master, terminal))
}

/// Makes the hierarchy's directories, mounts what goes on them and makes
/// the links to them. Returns whether all of it worked. The disks'
/// filesystems are mounted unless not `volumes`, those of the disks in
/// `compress` compressed, and `/system` is Dukovo's root on a disk if one
/// holds it, else the boot archive and memory.
fn build(root: &Dir, volumes: bool, compress: &[&str]) -> bool {
    for name in HIERARCHY {
        if let Err(err) = root.make_dir(name) {
            println!("spark: cannot make /{name}: {err}");
            return false;
        }
    }
    for (path, kind, flags) in MOUNTS {
        // Everyone writes in /tmp; only root makes the mount points of
        // /volumes.
        let mounted = root
            .mount(path, kind, None, flags)
            .and_then(|()| if path == "volumes" { narrow(root, path) } else { Ok(()) });
        if let Err(err) = mounted {
            println!("spark: cannot mount {} on /{path}: {err}", kind.name());
            return false;
        }
    }
    let named = named_root(root);
    let live = kernel_flag(root, "live");
    let boot = slot_boot(root);
    let disk = if !volumes {
        println!("spark: leaving every disk's filesystems unmounted");
        None
    } else if live {
        // Two lines: a write to the log is cut at 256 bytes.
        println!(
            "spark: the live system: /system is the boot archive, in memory, and no disk is the root"
        );
        println!(
            "spark: only the install medium is mounted; mount(8) mounts the machine's own disks"
        );
        if rescue(root) {
            println!("spark: this is Rescue (ADR 0460): dukovo-rescue(8) runs on the consoles");
        }
        mount_disks(root, compress, true);
        None
    } else {
        let btrfs = mount_disks(root, compress, false);
        root_volume(root, &btrfs, named.as_deref(), boot.system())
    };
    // The settings come back before anything reads them, and a root
    // without them, as a power cut can leave one, gets them again; then a
    // whole machine restored from its backups takes its place.
    if let Some(disk) = &disk {
        restore_settings(root, disk, boot.fallback);
        restore_machine(root, disk);
    }
    let snapshots = disk.as_deref().and_then(|disk| mount_root(root, disk, boot.system()));
    let mounted = match (disk, snapshots) {
        (Some(disk), Some(snapshots)) => {
            println!("spark: /system is Dukovo's root on {disk}, compressed (zstd)");
            println!(
                "spark: started slot {}, {}",
                boot.system().letter(),
                boot.system().subvolume()
            );
            swap::turn_on(root, &disk);
            ON_DISK.len() + 1 + snapshots
        }
        _ => {
            // Two lines: a write to the log is cut at 256 bytes.
            if let Some(uuid) = named.as_deref().filter(|_| volumes && !live) {
                println!(
                    "spark: no disk holds the Dukovo root the boot menu names (root=UUID={uuid}), \
                     so this installed system cannot start"
                );
                println!(
                    "spark: /system is only the startup archive; check that the disk is \
                     connected, or start Rescue from the boot menu to look at it"
                );
                ROOT_MISSING.store(true, core::sync::atomic::Ordering::Relaxed);
            }
            for (path, kind, flags) in IN_MEMORY {
                let mounted = root.mount(path, kind, None, flags).and_then(|()| {
                    if kind == FilesystemKind::Tmpfs { narrow(root, path) } else { Ok(()) }
                });
                if let Err(err) = mounted {
                    println!("spark: cannot mount {} on /{path}: {err}", kind.name());
                    return false;
                }
            }
            IN_MEMORY.len()
        }
    };
    // Dukovo's root on a disk keeps these from the boot before.
    let again = |done: Result<(), radix_rt::fs::Error>| match done {
        Err(err) if err.is(FileError::Exists) => Ok(()),
        done => done,
    };
    for name in MOUNTED_DIRS {
        if let Err(err) = again(root.make_dir(name)) {
            println!("spark: cannot make /{name}: {err}");
            return false;
        }
    }
    for (path, target) in LINKS {
        if let Err(err) = again(root.make_link(path, target)) {
            println!("spark: cannot link /{path} to /{target}: {err}");
            return false;
        }
    }
    for (path, names) in HIDDEN {
        if let Err(err) = root.write_file(path, names.as_bytes()) {
            println!("spark: cannot write /{path}: {err}");
            return false;
        }
    }
    println!(
        "spark: built the hierarchy of {} directories and mounted {} filesystems",
        HIERARCHY.len(),
        MOUNTS.len() + mounted
    );
    true
}

/// Where the system keeps its default settings, and where the settings
/// are, which start with each default that is not there yet: the plan's
/// way for updates to add settings without replacing edited ones
/// (spark(8)). On Dukovo's root on a disk the first boot copies them all;
/// with `/system/settings` in memory, every boot does.
const DEFAULTS: &str = "system/resources/default-settings";
const SETTINGS: &str = "system/settings";

/// How deep the defaults' directories may nest.
const DEFAULTS_DEPTH: usize = 8;

/// Copies each default setting into `/system/settings` that is not there.
fn seed_settings(root: &Dir) {
    let Ok(defaults) = root.open_dir(DEFAULTS, Rights::READ) else {
        return;
    };
    let settings = match root.open_dir(SETTINGS, Rights::READ.union(Rights::WRITE)) {
        Ok(settings) => settings,
        Err(err) => {
            println!("spark: cannot open /{SETTINGS}: {err}");
            return;
        }
    };
    let mut added = 0;
    seed(&defaults, &settings, DEFAULTS_DEPTH, &mut added);
    println!("spark: added {added} default settings to /{SETTINGS}");
}

/// Copies the files beneath `from` that `to` lacks into it, with the
/// directories they are in, to `depth` levels; counts them in `added`.
fn seed(from: &Dir, to: &Dir, depth: usize, added: &mut usize) {
    let entries = match from.entries() {
        Ok(entries) => entries,
        Err(err) => {
            println!("spark: cannot list the default settings: {err}");
            return;
        }
    };
    for entry in entries {
        let name = entry.name.as_str();
        let done = match entry.kind {
            FileKind::Directory if depth > 1 => match to.make_dir(name) {
                Err(err) if !err.is(FileError::Exists) => Err(err),
                _ => from.open_dir(name, Rights::READ).and_then(|inner| {
                    let rw = Rights::READ.union(Rights::WRITE);
                    to.open_dir(name, rw).map(|out| seed(&inner, &out, depth - 1, added))
                }),
            },
            FileKind::File if to.stat(name).is_err() => {
                from.read_file(name).and_then(|data| to.write_file(name, &data)).map(|()| {
                    *added += 1;
                })
            }
            // A link, such as resolv.conf's to netd's (ADR 0191), whose
            // target may not be there yet.
            FileKind::Symlink if to.read_link(name).is_err() && to.stat(name).is_err() => {
                from.read_link(name).and_then(|target| {
                    let target = String::from_utf8_lossy(&target);
                    to.make_link(name, &target).map(|()| *added += 1)
                })
            }
            _ => Ok(()),
        };
        if let Err(err) = done {
            println!("spark: cannot add the default setting {name}: {err}");
        }
    }
}

/// The type of an EFI system partition, as `/kernel/block` gives it: in
/// a GPT, and in an MBR.
const EFI_SYSTEM: [&str; 2] = ["c12a7328-f81f-11d2-ba4b-00a0c93ec93b", "0xef"];

/// The GPT type of an install stick's DUKOVO_LIVE partition, "Dukovo
/// live" (ADR 0308), as `/kernel/block/<name>/parttype` gives it: the
/// signed release the installer installs, which nothing may write.
const DUKOVO_LIVE: &str = "5894c662-af90-4b47-baaf-2e3396443b25";

/// The label of the install DVD's ISO 9660 volume, as `ob image --iso`
/// makes it (ADR 0433): the DVD's counterpart of the stick's DUKOVO_LIVE
/// partition, holding the same signed release in its `release` directory.
const LIVE_DISC_LABEL: &[u8] = b"DUKOVO_LIVE";

/// Where a disc's ISO 9660 primary volume descriptor is, in bytes (the
/// 2,048-byte sector 16), how it starts (its type, 1, and `CD001`), and
/// where in it the volume's label is, padded with spaces (ECMA-119 8.4).
const ISO_PVD: u64 = 16 * 2048;
const ISO_PVD_LEN: usize = 2048;
const ISO_PVD_START: &[u8] = b"\x01CD001";
const ISO_VOLUME_ID: core::ops::Range<usize> = 40..72;

/// Whether `/devices/<name>` is the install DVD: an optical drive, as its
/// `/kernel/block/<name>/disc` file says, whose disc has an ISO 9660
/// volume labelled [`LIVE_DISC_LABEL`] (ADR 0433). Only a disc counts: a
/// disk with such a volume on it is not the medium Dukovo started from.
fn live_disc(root: &Dir, name: &str) -> bool {
    if root.read_file(&format!("kernel/block/{name}/disc")).is_err() {
        return false;
    }
    let Ok(device) = root.open(&format!("devices/{name}"), OpenFlags::NONE, Rights::READ) else {
        return false;
    };
    let mut pvd = alloc::vec![0u8; ISO_PVD_LEN];
    device.read_at(ISO_PVD, &mut pvd).is_ok_and(|got| got == pvd.len())
        && pvd.starts_with(ISO_PVD_START)
        && pvd[ISO_VOLUME_ID].trim_ascii_end() == LIVE_DISC_LABEL
}

/// Mounts the FAT, exFAT, NTFS, Btrfs or ext4 filesystem of each disk
/// partition, or of each disk with no partitions, at `/volumes/<name>`:
/// a Btrfs of several disks once, at the first of them. EFI system
/// partitions are left alone: what boots the machine is no volume of the
/// user's, as Windows, macOS and Linux's udisks have it. An install
/// stick's DUKOVO_LIVE partition, or the install DVD ([`live_disc`]), is
/// mounted read-only, and with `live_only`, as the live system starts, it
/// is the only one mounted.
/// Returns the names of those it mounted Btrfs from that may hold
/// Dukovo's root, and how.
fn mount_disks(root: &Dir, compress: &[&str], live_only: bool) -> Vec<(String, MountFlags)> {
    let devices: Vec<String> =
        match root.open_dir("devices", Rights::READ).and_then(|dev| dev.entries()) {
            Ok(entries) => entries
                .into_iter()
                .filter(|entry| entry.kind == FileKind::BlockDevice)
                .map(|entry| entry.name)
                .collect(),
            Err(err) => {
                println!("spark: cannot list /devices: {err}");
                return Vec::new();
            }
        };
    // A partition has its disk's name and its number, after a `p` when
    // the disk's name ends in a digit: `sda1`, but `nvme0n1p1`.
    let partitioned = |disk: &str| {
        let digit = disk.bytes().last().is_some_and(|b| b.is_ascii_digit());
        devices.iter().any(|name| {
            name.strip_prefix(disk)
                .and_then(|n| if digit { n.strip_prefix('p') } else { Some(n) })
                .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
        })
    };
    let mut btrfs = Vec::new();
    for name in devices.iter().filter(|name| !partitioned(name)) {
        // An empty drive, such as a card reader's slot with no card in
        // it, has no sectors: volumed mounts what is put in it (ADR 0430).
        if root
            .read_file(&format!("kernel/block/{name}/size"))
            .is_ok_and(|size| size.trim_ascii() == b"0")
        {
            let disc = root.read_file(&format!("kernel/block/{name}/disc")).ok();
            match disc.as_deref().and_then(|disc| disc_note(disc.trim_ascii())) {
                Some(note) => println!("spark: {name} {note}"),
                None => println!("spark: {name} has no medium in it; volumed mounts one put in"),
            }
            continue;
        }
        let kind = swap::part_type(root, name);
        if EFI_SYSTEM.contains(&kind.as_str()) {
            println!("spark: {name} is an EFI system partition; left alone");
            continue;
        }
        let medium = kind == DUKOVO_LIVE || live_disc(root, name);
        if live_only && !medium {
            continue;
        }
        // Turned on once the root is found, if it is on the root's disk.
        if swap::SWAP.contains(&kind.as_str()) {
            println!("spark: {name} is a swap partition; no filesystem to mount");
            continue;
        }
        let compress = compress.contains(&name.as_str()) && !medium;
        if let Some(flags) = mount_disk(root, name, compress, medium)
            && !medium
        {
            btrfs.push((name.clone(), flags));
        }
    }
    btrfs
}

/// What Spark says of an optical drive with no disc of data in it, as its
/// `/kernel/block/<drive>/disc` says what is in it (ADR 0431): `none`, a
/// disc's kind and what it holds (`cd-rom audio`, `dvd+r blank`), or
/// `unreadable`. `None` for a drive that has none of these to say.
fn disc_note(disc: &[u8]) -> Option<String> {
    let disc = core::str::from_utf8(disc).ok()?;
    let (kind, holds) = disc.split_once(' ').unwrap_or((disc, ""));
    Some(match (kind, holds) {
        ("none", _) => "has no disc in it; volumed mounts one put in".into(),
        ("unreadable", _) => "has a disc in it that cannot be read; left alone".into(),
        (_, "audio") => "holds a music CD; its tracks are not files, and it is not mounted".into(),
        (kind, "blank") => format!("holds a blank disc ({kind}); nothing to mount"),
        _ => return None,
    })
}

/// Whether the kernel's command line has `flag` among the words before a
/// lone `--`, which are the kernel's (dukovo-boot(7)): `live`, as the
/// install stick's boot menu starts the live system with, or
/// `test-hang`, as a boot test's broken update starts with.
fn kernel_flag(root: &Dir, flag: &str) -> bool {
    root.read_file("kernel/cmdline").is_ok_and(|line| {
        core::str::from_utf8(&line).is_ok_and(|line| {
            line.split_ascii_whitespace().take_while(|&word| word != "--").any(|word| word == flag)
        })
    })
}

/// Whether this is Rescue, which its boot menu entry starts as the live
/// system with `rescue=<uuid>`, the root to rescue (dukovo-boot(7)).
fn rescue(root: &Dir) -> bool {
    kernel_flag(root, "live")
        && root.read_file("kernel/cmdline").is_ok_and(|line| {
            core::str::from_utf8(&line).is_ok_and(|line| {
                line.split_ascii_whitespace()
                    .take_while(|&word| word != "--")
                    .any(|word| word.starts_with("rescue="))
            })
        })
}

/// What the kernel's command line says of the slots: the one GRUB
/// started, `slot=`, slot A when it names none, and the update that fell
/// back, `fallback=` (dukovo-boot(7)).
fn slot_boot(root: &Dir) -> Boot {
    root.read_file("kernel/cmdline")
        .ok()
        .and_then(|line| core::str::from_utf8(&line).ok().map(system_slots::cmdline::parse))
        .unwrap_or_default()
}

/// Puts the settings from before an update back after it fell back, the
/// update in the slot `fallback`, before any service starts (ADR 0454),
/// on the root on `/volumes/<disk>`: the failed attempt's are kept beside
/// the snapshot they replace, for a look at what went wrong. Each step is
/// decided from what is there, so that a power cut between two leaves
/// the next start to carry on ([`restore::next`]). A root without
/// `@settings`, as such a cut can leave one, gets an empty one, which the
/// defaults then fill.
fn restore_settings(root: &Dir, disk: &str, fallback: Option<Slot>) {
    let top = |name: &str| format!("volumes/{disk}/{name}");
    let update = |name: &str| format!("volumes/{disk}/{}/{name}", restore::UPDATE);
    let is_dir = |path: &str| root.stat(path).is_ok_and(|stat| stat.kind == FileKind::Directory);
    let mut put_back = false;
    for _ in 0..=restore::MOST_STEPS {
        let seen = Seen {
            settings: is_dir(&top(restore::SETTINGS)),
            before: is_dir(&update(restore::BEFORE_SETTINGS)),
            failed: is_dir(&update(restore::FAILED_SETTINGS)),
            restored: root.stat(&update(restore::RESTORED)).is_ok(),
        };
        let step = if put_back { Step::Mark } else { restore::next(seen, fallback.is_some()) };
        let (settings, before) = (top(restore::SETTINGS), update(restore::BEFORE_SETTINGS));
        let done = match step {
            Step::KeepFailed => {
                root.snapshot(&settings, root, &update(restore::FAILED_SETTINGS), true)
            }
            Step::RemoveSettings => root.remove_subvolume(&settings),
            Step::PutBack => root.snapshot(&before, root, &settings, false),
            Step::Mark => root.write_file(&update(restore::RESTORED), b"settings put back\n"),
            Step::MakeEmpty => root.make_subvolume(&settings),
            Step::Done => return,
        };
        if let Err(err) = done {
            println!("spark: cannot put the settings back ({step:?}): {err}");
            return;
        }
        match step {
            Step::KeepFailed => {
                let slot = fallback.map_or("?", Slot::letter);
                println!(
                    "spark: the update in slot {slot} fell back: putting the settings from before it back"
                );
                println!(
                    "spark: its own settings are kept in /{}",
                    update(restore::FAILED_SETTINGS)
                );
            }
            Step::Mark => println!("spark: the settings from before the update are back"),
            Step::MakeEmpty => println!("spark: {disk}'s root had no @settings: made it afresh"),
            _ => {}
        }
        put_back = step == Step::PutBack;
    }
}

/// Puts a whole machine that backup(8) staged from its backups in place,
/// before any service starts (ADR 0464), on the root on
/// `/volumes/<disk>`: each staged subvolume in the place of the one
/// there, which is kept, read-only, beside it, then the state's files the
/// machine's record carried: the app registry and the grants. Each step
/// is decided from what is there ([`machine::next`]), so a power cut
/// between two leaves the next start to carry on; once the mark that the
/// restore is done is written, what was staged is removed.
fn restore_machine(root: &Dir, disk: &str) {
    use system_slots::machine::{self, About, Seen as Found, Step as Next};
    let top = |name: &str| format!("volumes/{disk}/{name}");
    let base = top(machine::FOLDER);
    let staged = format!("{base}/{}", machine::STAGED);
    let exists = |path: &str| root.stat(path).is_ok();
    let is_dir = |path: &str| root.stat(path).is_ok_and(|stat| stat.kind == FileKind::Directory);
    let Ok(text) = root.read_file(&format!("{staged}/{}", machine::ABOUT)) else { return };
    let about = match core::str::from_utf8(&text)
        .map_err(|_| "not text".into())
        .and_then(About::parse)
    {
        Ok(about) => about,
        Err(why) => {
            println!("spark: a machine restore is staged, but its `about` does not read: {why}");
            return;
        }
    };
    let done = format!("{staged}/{}", machine::DONE);
    if !exists(&done) {
        let begun = about
            .subvolumes
            .iter()
            .any(|name| exists(&format!("{staged}/{}", machine::mark(name))));
        if !begun {
            let missing = about.subvolumes.iter().find(|n| !is_dir(&format!("{staged}/{n}")));
            if let Some(missing) = missing {
                println!("spark: the machine restore staged lacks {missing}: left as it is");
                return;
            }
            println!(
                "spark: restoring the machine {} from its backup of {} (ADR 0464)",
                about.machine, about.time
            );
        }
        let kept = format!("{base}/{}", machine::kept_folder(&about.staged));
        if !is_dir(&kept)
            && let Err(err) = root.make_dir(&kept)
        {
            println!("spark: cannot make /{kept}: {err}");
            return;
        }
        for name in &about.subvolumes {
            let live = top(&format!("@{name}"));
            let (from, keep) = (format!("{staged}/{name}"), format!("{kept}/{name}"));
            let mark = format!("{staged}/{}", machine::mark(name));
            for _ in 0..=machine::MOST_STEPS {
                let found = Found {
                    live: is_dir(&live),
                    staged: is_dir(&from),
                    kept: is_dir(&keep),
                    put: exists(&mark),
                };
                let step = machine::next(found);
                let result = match step {
                    Next::Keep => root.snapshot(&live, root, &keep, true),
                    Next::Remove => root.remove_subvolume(&live),
                    Next::Put => root
                        .snapshot(&from, root, &live, false)
                        .and_then(|()| root.write_file(&mark, b"in place\n")),
                    Next::PutKept => {
                        println!("spark: the restored @{name} is gone: keeping the one there was");
                        root.snapshot(&keep, root, &live, false)
                            .and_then(|()| root.write_file(&mark, b"kept as it was\n"))
                    }
                    Next::Done => break,
                    Next::Lost => {
                        println!("spark: @{name} is gone, and no copy of it is there");
                        return;
                    }
                };
                if let Err(err) = result {
                    println!("spark: cannot put the restored @{name} in place ({step:?}): {err}");
                    return;
                }
            }
        }
        if let Err(err) = restore_state(root, &top(machine::STATE_SUBVOLUME), &staged, &about) {
            println!("spark: cannot put the restored apps' registry and grants in place: {err}");
            return;
        }
        if let Err(err) = root.write_file(&done, b"restored\n") {
            println!("spark: cannot mark the machine restore done: {err}");
            return;
        }
        println!(
            "spark: the machine is as it was at {}; what it replaced is in /snapshots/{}",
            about.time,
            format!("{}/{}", machine::FOLDER, machine::kept_folder(&about.staged))
                .trim_start_matches("@snapshots/")
        );
    }
    // Done: what was staged goes.
    for name in machine::SUBVOLUMES {
        let path = format!("{staged}/{name}");
        if is_dir(&path)
            && let Err(err) = root.remove_subvolume(&path)
        {
            println!("spark: cannot remove /{path}: {err}");
        }
    }
    if let Err(err) = root.remove_subvolume(&staged) {
        println!("spark: cannot remove /{staged}: {err}");
    }
}

/// Writes the state's files a staged machine restore carries, `about`'s,
/// from `staged` into the state subvolume at `state`: the folders it
/// clears emptied of files first, then each file written beside its
/// place and renamed into it, root's with its mode. Each run ends the
/// same, so a power cut leaves the next start to write them again.
fn restore_state(
    root: &Dir,
    state: &str,
    staged: &str,
    about: &system_slots::machine::About,
) -> radix_rt::fs::Result<()> {
    use system_slots::machine;
    let rw = Rights::READ.union(Rights::WRITE);
    for folder in &about.clear {
        let Ok(dir) = root.open_dir(&format!("{state}/{folder}"), rw) else { continue };
        for entry in dir.entries()? {
            if entry.kind == FileKind::File {
                dir.remove(&entry.name)?;
            }
        }
    }
    for (mode, path) in &about.state {
        // Its folders, as a state without bundled's yet lacks them.
        let mut at = String::from(state);
        for name in path.split('/').rev().skip(1).collect::<Vec<_>>().into_iter().rev() {
            at.push('/');
            at.push_str(name);
            if root.stat(&at).is_err() {
                root.make_dir(&at)?;
            }
        }
        let to = format!("{state}/{path}");
        let new = machine::new_copy(&to);
        let data = root.read_file(&format!("{staged}/{}/{path}", machine::STATE))?;
        root.write_file(&new, &data)?;
        root.set_access(&new, Some(0), Some(0), Some(*mode))?;
        root.rename(&new, root, &to)?;
    }
    Ok(())
}

/// Mounts the filesystem on `/devices/<name>` at `/volumes/<name>`: FAT,
/// exFAT, NTFS, Btrfs or ext4, read-only if the disk is. A filesystem that
/// the disk or Radix cannot write (yet) mounts read-only, as Linux's
/// `mount` does with a disk that refuses writes, as an NTFS that Windows
/// hibernated with does.
/// FAT, exFAT and NTFS are tried first: `mkfs.btrfs` and `mkfs.ext4` clear
/// where they keep their boot sectors, but `mkfs.fat`, `mkfs.exfat` and
/// `mkntfs` can leave an older Btrfs or ext4 superblock behind, so theirs
/// is the newer. FAT, exFAT and NTFS each take the others' boot sectors
/// for none of theirs, and `mkfs.btrfs` and `mkfs.ext4` each clear the
/// other's superblock. A Btrfs mounted to
/// write compresses its data if `compress`. With `read_only`, as for an
/// install stick's DUKOVO_LIVE, the disk is opened and mounted only to
/// read. Returns how it mounted a Btrfs.
fn mount_disk(root: &Dir, name: &str, compress: bool, read_only: bool) -> Option<MountFlags> {
    let rights = if read_only { Rights::READ } else { Rights::READ.union(Rights::WRITE) };
    let opened = root.open(&alloc::format!("devices/{name}"), OpenFlags::NONE, rights);
    let (device, flags) = match opened {
        Ok(device) if read_only => (device, MountFlags::READ_ONLY),
        Ok(device) => (device, MountFlags::NONE),
        Err(err) if err.is(FileError::ReadOnly) => {
            match root.open(&alloc::format!("devices/{name}"), OpenFlags::NONE, Rights::READ) {
                Ok(device) => (device, MountFlags::READ_ONLY),
                Err(err) => {
                    println!("spark: cannot open /devices/{name}: {err}");
                    return None;
                }
            }
        }
        Err(err) => {
            println!("spark: cannot open /devices/{name}: {err}");
            return None;
        }
    };
    let at = alloc::format!("volumes/{name}");
    if let Err(err) = root.make_dir(&at) {
        println!("spark: cannot make /{at}: {err}");
        return None;
    }
    let mount = |kind, said, flags: MountFlags| match root.mount(&at, kind, Some(&device), flags) {
        // A disk that refuses writes fails with read_only, and what Radix
        // does not write yet is an argument it does not take, as on Linux
        // (EROFS, EINVAL).
        Err(radix_rt::fs::Error::Status(status))
            if !flags.contains(MountFlags::READ_ONLY)
                && (FileError::from_status(status) == Some(FileError::ReadOnly)
                    || KernelError::from_status(status) == Some(KernelError::InvalidArgument)) =>
        {
            let flags = MountFlags::READ_ONLY;
            root.mount(&at, kind, Some(&device), flags).map(|()| (said, flags))
        }
        done => done.map(|()| (said, flags)),
    };
    let packed = if compress { flags.union(MountFlags::COMPRESS) } else { flags };
    let mut mounted = mount(FilesystemKind::Fat, "FAT", flags);
    for (kind, said, flags) in [
        (FilesystemKind::Exfat, "exFAT", flags),
        (FilesystemKind::Ntfs, "NTFS", flags),
        (FilesystemKind::Btrfs, "Btrfs", packed),
        (FilesystemKind::Ext4, "ext4", flags),
        // Discs and their images, read-only: after the others, which an
        // image written over one of them could leave a superblock of, and
        // UDF first, which a DVD's ISO 9660 is a bridge to.
        (FilesystemKind::Udf, "UDF", flags),
        (FilesystemKind::Iso9660, "ISO 9660", flags),
    ] {
        if matches!(mounted, Err(err) if err.is(FileError::UnknownFilesystem)) {
            mounted = mount(kind, said, flags);
        }
    }
    match mounted {
        Ok((kind, flags)) => {
            let ro = if flags.contains(MountFlags::READ_ONLY) { ", read-only" } else { "" };
            let zstd =
                if flags.contains(MountFlags::COMPRESS) { ", compressed (zstd)" } else { "" };
            println!("spark: mounted the {kind} filesystem of {name} at /{at}{ro}{zstd}");
            (kind == "Btrfs").then_some(flags)
        }
        Err(err) => {
            let _ = root.remove(&at);
            let mut boot = [0u8; 512];
            let locked = err.is(FileError::UnknownFilesystem)
                && device.read_at(0, &mut boot).is_ok_and(|got| got == boot.len())
                && radix_bitlocker::format::volume(&boot).is_some();
            if locked {
                // unlock(8) unlocks it, and volumed(8) mounts what it
                // unlocks (ADR 0240).
                println!("spark: {name} is a locked BitLocker volume; `unlock {name}` unlocks it");
            } else if err.is(FileError::UnknownFilesystem) {
                println!(
                    "spark: {name} holds no FAT, exFAT, NTFS, Btrfs, ext4, UDF or ISO 9660 filesystem; left alone"
                );
            } else if err.is(FileError::InUse) {
                // A disk of a Btrfs mounted from another (ADR 0153), or
                // swapped to.
                println!("spark: {name} is in use already; left alone");
            } else {
                println!("spark: cannot mount {name}: {err}");
            }
            None
        }
    }
}

/// Which Btrfs of `btrfs`, mounted at `/volumes/<name>`, is Dukovo's root:
/// one with the subvolumes [`ON_DISK`] mounts, as `./ob image` and the
/// installer make it (ADR 0137). With a UUID `named` on the command line,
/// as an installed system's boot menu gives, only the Btrfs with that UUID
/// is (ADR 0307), so that a second disk with Dukovo on it, or an install
/// stick, never becomes the system GRUB did not start; with none, the
/// first is. The root holds the slot GRUB started, `slot`; it need not
/// hold `@settings`, which [`restore_settings`] makes again. Like
/// Fedora's, the root compresses what is written to it (ADR 0134):
/// mounted without, it is mounted again.
fn root_volume(
    root: &Dir,
    btrfs: &[(String, MountFlags)],
    named: Option<&str>,
    slot: Slot,
) -> Option<String> {
    let holds_root = |name: &str| {
        on_disk(slot).iter().filter(|(subvolume, _, _)| *subvolume != restore::SETTINGS).all(
            |(subvolume, _, _)| {
                root.stat(&format!("volumes/{name}/{subvolume}"))
                    .is_ok_and(|stat| stat.kind == FileKind::Directory)
            },
        )
    };
    let (name, flags) = btrfs.iter().find(|(name, _)| match named {
        Some(uuid) => btrfs_uuid(root, name).as_deref() == Some(uuid) && holds_root(name),
        None => holds_root(name),
    })?;
    if flags.contains(MountFlags::READ_ONLY) {
        println!("spark: {name} holds Dukovo's root but cannot be written; left alone");
        return None;
    }
    if !flags.contains(MountFlags::COMPRESS) {
        let at = format!("volumes/{name}");
        if let Err(err) = root.unmount(&at).and_then(|()| root.remove(&at)) {
            println!("spark: cannot mount {name} again to compress: {err}");
            return None;
        }
        mount_disk(root, name, true, false)?;
    }
    Some(name.clone())
}

/// The Dukovo root the kernel's command line names, `root=UUID=<uuid>`, in
/// lowercase: the last one, as the kernel takes a flag's last value, from
/// the words before a lone `--`, which are the kernel's (dukovo-boot(7)).
fn named_root(root: &Dir) -> Option<String> {
    let line = root.read_file("kernel/cmdline").ok()?;
    let line = core::str::from_utf8(&line).ok()?;
    line.split_ascii_whitespace()
        .take_while(|&word| word != "--")
        .filter_map(|word| word.strip_prefix("root=UUID="))
        .last()
        .map(str::to_ascii_lowercase)
}

/// Where a Btrfs keeps its first superblock, how much of it is read (4 KiB,
/// whole sectors of any disk), and where in it are its UUID and its magic
/// number (Btrfs's `btrfs_super_block`).
const SUPERBLOCK: u64 = 0x1_0000;
const SUPERBLOCK_READ: usize = 4096;
const SUPERBLOCK_FSID: usize = 0x20;
const SUPERBLOCK_MAGIC: usize = 0x40;
const BTRFS_MAGIC: &[u8; 8] = b"_BHRfS_M";

/// The UUID of the Btrfs on `/devices/<name>`, written as the command line
/// writes it: lowercase, in groups of 8, 4, 4, 4 and 12 digits.
fn btrfs_uuid(root: &Dir, name: &str) -> Option<String> {
    let device = root.open(&format!("devices/{name}"), OpenFlags::NONE, Rights::READ).ok()?;
    let mut head = alloc::vec![0u8; SUPERBLOCK_READ];
    if device.read_at(SUPERBLOCK, &mut head).ok()? != head.len() {
        return None;
    }
    if &head[SUPERBLOCK_MAGIC..SUPERBLOCK_MAGIC + 8] != BTRFS_MAGIC {
        return None;
    }
    let mut text = String::with_capacity(36);
    for (i, byte) in head[SUPERBLOCK_FSID..SUPERBLOCK_FSID + 16].iter().enumerate() {
        if matches!(i, 4 | 6 | 8 | 10) {
            text.push('-');
        }
        let _ = write!(text, "{byte:02x}");
    }
    Some(text)
}

/// Mounts the subvolumes of Dukovo's root on `/volumes/<disk>` where
/// [`ON_DISK`] says, with the system from `slot`, then memory on
/// `/system/live`, then the snapshots on `/snapshots`, read-only (ADR
/// 0461). Returns how many snapshot mounts it made, 0 or 1, if the rest
/// worked; if not, what it mounted is unmounted again. A root without
/// `@snapshots` starts all the same: snap(1) makes it.
fn mount_root(root: &Dir, disk: &str, slot: Slot) -> Option<usize> {
    let mut mounted = Vec::new();
    let mut failed = None;
    for (subvolume, at, flags) in on_disk(slot) {
        let rights = if flags.contains(MountFlags::READ_ONLY) {
            Rights::READ
        } else {
            Rights::READ.union(Rights::WRITE)
        };
        let path = format!("volumes/{disk}/{subvolume}");
        let from = match root.open_dir(&path, rights) {
            Ok(from) => from,
            Err(err) => {
                failed = Some(format!("cannot open /{path}: {err}"));
                break;
            }
        };
        if let Err(err) = root.bind(at, &from, flags) {
            failed = Some(format!("cannot mount /{path} on /{at}: {err}"));
            break;
        }
        mounted.push(at);
    }
    if failed.is_none() {
        let live = "system/live";
        match root
            .mount(live, FilesystemKind::Tmpfs, None, MountFlags::NONE)
            .and_then(|()| narrow(root, live))
        {
            Ok(()) => return Some(usize::from(mount_snapshots(root, disk))),
            Err(err) => failed = Some(format!("cannot mount tmpfs on /system/live: {err}")),
        }
    }
    println!("spark: {}; /system stays in memory", failed.unwrap_or_default());
    for at in mounted.into_iter().rev() {
        if let Err(err) = root.unmount(at) {
            println!("spark: cannot unmount /{at}: {err}");
        }
    }
    None
}

/// Where the root's snapshots are, and where they are mounted (ADR 0461).
const SNAPSHOTS: (&str, &str) = ("@snapshots", "snapshots");

/// Mounts the root's `@snapshots` on `/snapshots`, read-only. Returns
/// whether it did.
fn mount_snapshots(root: &Dir, disk: &str) -> bool {
    let (subvolume, at) = SNAPSHOTS;
    let path = format!("volumes/{disk}/{subvolume}");
    let mounted = root.open_dir(&path, Rights::READ).and_then(|from| {
        match root.make_dir(at) {
            Err(err) if !err.is(FileError::Exists) => return Err(err),
            _ => {}
        }
        root.bind(at, &from, MountFlags::READ_ONLY)
    });
    if let Err(err) = mounted {
        println!("spark: cannot mount /{path} on /{at}: {err}; snap(1) makes it");
        return false;
    }
    true
}

/// The mode of the root of each tmpfs Spark mounts in `/system`, and of
/// `/volumes`: only root changes what is there (ADR 0213). A tmpfs starts
/// as `/tmp` is, 1777, as on Linux.
const SYSTEM_MODE: u16 = 0o755;

/// Gives the tmpfs just mounted on `path` [`SYSTEM_MODE`].
fn narrow(root: &Dir, path: &str) -> radix_rt::fs::Result<()> {
    root.set_access(path, None, None, Some(SYSTEM_MODE))
}

/// Waits for good.
pub(crate) fn idle() -> ! {
    let made = slots::take().filter(|&slot| {
        process::create(&SYS, SLOT_PROCESS, ObjectKind::Notification, 0, slot).is_ok()
    });
    loop {
        match made {
            Some(slot) => {
                let _ = syscalls::wait(&SYS, slot, WaitFlags::NONE, 0);
            }
            None => {
                let _ = syscalls::r#yield(&SYS);
            }
        }
    }
}
