// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Programs in Go run on Radix: Go's runtime, its scheduler and its standard
//! library on the system calls, signals and files Radix gives them. ob builds
//! the programs with the Go toolchain the go port installs (src/golang.rs),
//! and a disk holds them, as the boot archive does not: the Go test program,
//! the test binaries of 22 packages of Go's standard library, each in a
//! folder with the package's files and test data, which its tests read, and
//! two scripts. alice, the owner, signs in at the console.
//!
//! go-runtime-test runs with the number of CPUs the machine has, and its
//! fourteen checks each say they are ok: Go's scheduler on as many threads
//! as there are CPUs, goroutines switching through channels, a mutex, a wait
//! group and a condition variable, timers, tickers and clocks, a loop that
//! only a signal to its thread can interrupt, a panic and a nil dereference
//! that end a program with Go's traceback, signals sent to the process and
//! one that ends it, a gibibyte allocated and given back, files written,
//! synced, read, renamed and removed, programs started with arguments, an
//! environment and a pipe between them, the account and the folder the
//! process runs in, the time zones of the system, and random bytes.
//!
//! Then the test binaries run one at a time, each from its package's
//! folder, short and under the five minutes its own timeout allows, without
//! the tests that userspace/go/skips.txt names, each with the feature Radix
//! lacks. Each says ok, or fails with the end of what it printed. Every
//! package runs before the test ends, so that one run shows them all.
//!
//! Without KVM the packages take many times as long as under it, and a
//! package's tests may pass the five minutes their binary allows, which
//! ends it with its own timeout and a dump of its goroutines. This test
//! waits seven and a half minutes for each package, whatever --timeout
//! says. Its other waits, for the boot and the console, are three times its
//! line in times.txt, 21 minutes, or --timeout when that is more: on a host
//! without KVM, --timeout 1800 leaves room for a slow boot.

use super::super::*;
use crate::golang;

/// The disk's label, and where Spark mounts it: the first virtio disk, as
/// nothing else is on one.
const LABEL: &str = "GOTESTS";
const VOLUME: &str = "/volumes/vda";

/// How long go-runtime-test may take, in seconds: its checks have deadlines
/// of their own, and the longest, memory's, with its watchdog's grace is
/// just over five minutes, so that a check that hangs is reported by name.
const RUNTIME_SECONDS: u64 = 600;

/// How long a package's test binary may take, in seconds: more than the five
/// minutes of its own timeout, so that a binary that times out says so.
const PACKAGE_SECONDS: u64 = 450;

/// What go-runtime-test prints, in order: a line for each check, then the
/// count, as the screen shows them.
const LINES: &[&str] = &[
    "go-runtime: threads ok",
    "go-runtime: channels-and-mutexes ok",
    "go-runtime: timers ok",
    "go-runtime: clocks ok",
    "go-runtime: preemption ok",
    "go-runtime: panic-trace ok",
    "go-runtime: nil-dereference ok",
    "go-runtime: signals ok",
    "go-runtime: memory ok",
    "go-runtime: files ok",
    "go-runtime: pipes-and-exec ok",
    "go-runtime: environment ok",
    "go-runtime: time-zones ok",
    "go-runtime: random ok",
    "go-runtime: 14 checks, 0 failed",
];

/// The script that runs a package's test binary, `run-test.sh PACKAGE`, from
/// the package's folder on the disk. The pattern of the tests to skip is the
/// line of skips.lst that starts with the package's name, if there is one.
/// It says one line, `go-test: PACKAGE ok` or `go-test: PACKAGE FAILED`,
/// and for a failure the first lines that say what failed (a failed test,
/// a panic, a fatal error or a signal) and the end of what the binary
/// printed, from the file the binary wrote it to.
const RUN_TEST: &str = r#"#!/bin/sh
package=$1
log=/tmp/go-test.log
skip=$(sed -n "s|^$package ||p" /volumes/vda/skips.lst)
cd "/volumes/vda/src/$package" || exit 1
set -- -test.short -test.timeout=5m
# -test.run=. runs every test, as no -test.run does, and tells the
# package's own TestMain that the run is not whole, as os/exec's must be
# told to not demand that each of its helper commands ran.
if [ -n "$skip" ]; then
	set -- "$@" -test.run=. "-test.skip=$skip"
fi
"./$(basename "$package").test" "$@" >"$log" 2>&1
status=$?
if [ "$status" -eq 0 ] && [ "$(tail -n 1 "$log")" = PASS ]; then
	echo "go-test: $package ok"
else
	echo "go-test: $package FAILED, status $status"
	grep -E -A 3 -m 6 '^(--- FAIL|panic:|fatal error:|SIG[A-Z]+:)' "$log" | head -n 16
	tail -n 6 "$log"
fi
"#;

/// A disk for the programs: more than their binaries, about 130 MiB, and
/// their test data take.
const DISKS: &[Disk] = &[go_files_disk(512 * 1024, LABEL)];

/// The test.
pub const CASE: Case = Case {
    name: "go-runtime",
    about: "programs in Go run on Radix: go-runtime-test, built by ob with the go port's toolchain and run from a disk, passes its fourteen checks with the machine's CPUs (threads, channels, mutexes, timers, clocks, preemption by signal, a panic and a nil dereference ending with Go's traceback, signals, memory given back, files, processes and pipes, the environment, time zones, random bytes), and the test binaries of 22 packages of Go's standard library, from runtime and sync to os, io, strings, math/big and encoding/json, each in its own folder with its files and test data, pass, but for the tests userspace/go/skips.txt names",
    firmware: Firmware::Uefi,
    media: Media::Usb,
    cmdline: "lockdep",
    cpus: 4,
    memory: "2G",
    reboot: false,
    image: TestImage::Plain,
    cpu_model: None,
    qemu: &[],
    script: go_runtime,
    disks: DISKS,
    second: None,
};

/// Makes the disk of Go programs: FAT32 holding what [`golang::build_tests`]
/// builds, with the skip list as the script reads it and the script.
pub fn make_disk(ctx: &Ctx, disk: &Disk, path: &Path) -> Result<(), String> {
    let toolchain = crate::ports::find_toolchain(ctx).map_err(|err| err.to_string())?;
    let programs = ctx.target.join("go/tests");
    golang::build_tests(ctx, &toolchain, &programs).map_err(|err| err.to_string())?;
    let skips = golang::skips(ctx).map_err(|err| err.to_string())?;
    for (name, text) in [("skips.lst", golang::skip_list(&skips)), ("run-test.sh", RUN_TEST.into())]
    {
        let file = programs.join(name);
        std::fs::write(&file, text).map_err(|err| format!("writing {}: {err}", file.display()))?;
    }
    let mut mkfs = std::process::Command::new(fat_tool("mkfs.fat"));
    mkfs.args(["-F", "32", "-n", disk.label, "-i", "4f525448", "-C"])
        .arg(path)
        .arg(disk.kib.to_string());
    host_tool(&mut mkfs)?;
    let mut entries: Vec<PathBuf> = std::fs::read_dir(&programs)
        .map_err(|err| format!("reading {}: {err}", programs.display()))?
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .collect();
    entries.sort();
    let mut copy = std::process::Command::new("mcopy");
    copy.args(["-s", "-i"]).arg(path).args(&entries).arg("::/");
    host_tool(&mut copy).map(drop)
}

fn go_runtime(vm: &mut Vm, case: &Case) -> Result<(), String> {
    owner_signs_in(vm, case)?;
    vm.expect_anywhere(&format!("spark: mounted the FAT filesystem of vda at {VOLUME}"))?;

    // The test program, with the machine's CPUs. Its output goes to a file,
    // with its exit code on a last line, so that the lines are read from
    // the screen when it has ended.
    run_within(
        vm,
        &format!(
            "clear; {VOLUME}/{} {} > /tmp/go-runtime 2>&1; echo \"exit code $?\" >> \
             /tmp/go-runtime",
            golang::RUNTIME_TEST,
            case.cpus
        ),
        RUNTIME_SECONDS,
    )?;
    answer_prompt(vm, "$", "clear; cat /tmp/go-runtime")?;
    vm.expect_screen(LINES)?;
    vm.expect_screen(&["exit code 0"])?;
    vm.expect_screen_without("FAILED")?;

    // The standard library's tests, a package at a time. A package that
    // fails does not end the test: each is read from the screen when it has
    // ended, and the failures are reported together.
    let mut failed = Vec::new();
    for package in golang::TEST_PACKAGES {
        run_within(vm, &format!("clear; sh {VOLUME}/run-test.sh {package}"), PACKAGE_SECONDS)?;
        let screen = vm.screen()?;
        let ok = format!("go-test: {package} ok");
        if !screen.iter().any(|line| line.trim() == ok) {
            let shown: Vec<&str> =
                screen.iter().map(String::as_str).filter(|line| !line.is_empty()).collect();
            let tail = shown[shown.len().saturating_sub(20)..].join(" | ");
            failed.push(format!("{package}: {tail}"));
        }
    }
    if !failed.is_empty() {
        return Err(format!(
            "the tests of {} of {} packages did not pass: {}",
            failed.len(),
            golang::TEST_PACKAGES.len(),
            failed.join("; ")
        ));
    }
    vm.press_power_button()?;
    vm.expect("spark: every service has stopped")?;
    vm.expect("Power: `spark` asked to power off the machine; syncing the filesystems")?;
    vm.expect_exit(QEMU_EXIT_POWER_OFF)
}
