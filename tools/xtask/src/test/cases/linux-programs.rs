// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Static Linux programs run on Dukovo: a program built for Linux makes its
//! system calls with the processor's syscall instruction, and the kernel
//! serves them under the Linux personality it gives a program that does not
//! carry Dukovo's note (ADR 0610). ob builds the programs on the build
//! machine, as a Linux machine would, and a disk holds them, as the boot
//! archive does not: userspace/linux/core.c twice, as linux-core with the
//! host's C library and as linux-core-musl with musl-gcc, and the C test
//! programs of userspace/posix that pass on Linux, in a folder of their
//! own. alice, the owner, signs in at the console.
//!
//! linux-core and linux-core-musl run from the disk, given a program and a
//! working-directory printer of the Dukovo system and the folder the shell
//! is in. Each makes the calls itself, so that it is the kernel's answer
//! that is checked, and says one line for a case that went wrong and, at
//! the end, that all 23 cases passed on Dukovo: threads from the C library
//! and from a raw clone (a new thread word, the cleared word that wakes a
//! join), a fork that sets the child's thread word, a vfork whose parent
//! resumes after the child's exec, programs started from a Linux one (itself
//! and one of Dukovo's, with the working directory crossing to it),
//! arch_prctl's thread base, the break growing and shrinking, memory given
//! back reading as zeros, mremap shrinking, growing and failing, the names
//! uname gives, getcwd's length, the records of getdents64, statx, signals
//! through the C library's restorer, one that interrupts a blocked read and
//! one that a restart leaves, futexes with a deadline in absolute time, the
//! calls the personality refuses, the identities, the lowest free
//! descriptor, the umask, and a pseudo-terminal's settings through the C
//! library's termios calls. The same file runs on a Linux machine, which
//! is how its expectations are known to be Linux's.
//!
//! Then the test programs of userspace/posix that pass on a Linux machine as
//! they are ([`ON_LINUX`]) run from /tmp, alone and under a time limit, and
//! each says ok or FAILED. The others check what Dukovo's own C library
//! answers (its paths, its error numbers, its limits) and fail on Linux
//! too, so they could not say whether the kernel answers as Linux does. The
//! test ends when every program has run, so that one run shows all that
//! failed.
//!
//! Building the programs needs musl-gcc (the package musl-tools), which ob
//! doctor checks; the test says so when it is missing.

use super::super::*;

/// The disk's label, and where Spark mounts it: the first virtio disk, as
/// nothing else is on one.
const LABEL: &str = "LINUXTESTS";
const VOLUME: &str = "/volumes/vda";

/// The program built with the host's C library and the one built with musl.
const CORE: &str = "linux-core";
const CORE_MUSL: &str = "linux-core-musl";

/// How many cases linux-core has: the table of userspace/linux/core.c, which
/// a unit test counts.
const CORE_CASES: usize = 23;

/// How long a run of linux-core may take, in seconds: the program ends a
/// case that takes thirty and the script ends the program at 250, so that a
/// hang is reported by the program's name.
const CORE_SECONDS: u64 = 300;

/// How long the C test programs may take together, in seconds: the script
/// gives each program 120 and ends it after that.
const POSIX_SECONDS: u64 = 120 * ON_LINUX.len() as u64 + 30;

/// Where the sources are, from the repository's root.
const CORE_SOURCE: &str = "userspace/linux/core.c";
const POSIX_SOURCES: &str = "userspace/posix";

/// The test programs of userspace/posix that pass on a Linux machine, built
/// there as static programs with its C library: what they check is Linux's
/// behaviour as much as Dukovo's.
const ON_LINUX: &[&str] = &["hello", "locks"];

/// The script that runs one of the two linux-core programs,
/// `run-core.sh PROGRAM`, from the folder of the script. It names the
/// programs of the Dukovo system that two cases run and the folder the shell
/// is in, and ends the program after 250 seconds. It says what the program
/// said but for the lines of the cases that went well, so that the screen
/// holds the rest whatever the number of cases, and then the program's exit
/// status.
const RUN_CORE: &str = r#"#!/bin/sh
program=$1
dir=$(cd "$(dirname "$0")" && pwd)
log=/tmp/$program.log
timeout -k 5 250 "$dir/$program" --native /system/commands/true \
	--native-pwd /system/commands/pwd --started-in "$(pwd -P)" >"$log" 2>&1
status=$?
grep -v ' ok$' "$log" | cut -c 1-78
echo "$program: exit status $status"
"#;

/// The script that runs the C test programs, `run-posix.sh [TEST...]`: those
/// named, or every program in the posix folder beside the script, each from
/// /tmp, alone, for at most 120 seconds. It says a line for each program,
/// "linux-posix: NAME ok" or "linux-posix: NAME FAILED, status N" with the
/// first failed check the program printed, and a last line that says every
/// test passed or how many failed.
const RUN_POSIX: &str = r#"#!/bin/sh
dir=$(cd "$(dirname "$0")" && pwd)/posix
cd /tmp || exit 1
tests=$*
if [ -z "$tests" ]; then
	tests=$(ls "$dir")
fi
count=0
failed=0
for name in $tests; do
	count=$((count + 1))
	log=/tmp/linux-posix-$name.log
	timeout -k 5 120 "$dir/$name" >"$log" 2>&1
	status=$?
	if [ "$status" -eq 0 ]; then
		echo "linux-posix: $name ok"
	else
		failed=$((failed + 1))
		echo "linux-posix: $name FAILED, status $status"
		grep -m 1 FAILED "$log" | cut -c 1-78
	fi
done
if [ "$failed" -eq 0 ]; then
	echo "linux-posix: every test passed"
else
	echo "linux-posix: $failed of $count tests FAILED"
fi
"#;

/// A disk for the programs, which take a few mebibytes: a small FAT32, with
/// room to spare.
const DISKS: &[Disk] = &[linux_files_disk(64 * 1024, LABEL)];

/// The test.
pub const CASE: Case = Case {
    name: "linux-programs",
    about: "static Linux programs run on Dukovo under the Linux personality (ADR 0610): linux-core, built from userspace/linux/core.c with the build machine's C library and with musl, passes its 23 cases twice, making the calls itself (threads from the C library and from a raw clone, fork, vfork, execve of itself and of a program of Dukovo with the working directory crossing, arch_prctl, brk, madvise, mremap, uname, getcwd, getdents64, statx, signals and restarts, futexes with absolute deadlines, refused calls, identities, descriptors, umask, terminal settings), and the C test programs of userspace/posix that pass on Linux each run alone and pass on Dukovo",
    firmware: Firmware::Uefi,
    media: Media::Usb,
    cmdline: "lockdep",
    cpus: 4,
    memory: "2G",
    reboot: false,
    image: TestImage::Plain,
    cpu_model: None,
    qemu: &[],
    script: linux_programs,
    disks: DISKS,
    second: None,
};

/// Where the programs are built, in the build output: a folder made anew
/// for each run, so that a program that was left out of the test is not
/// copied from an earlier run.
fn build_folder(ctx: &Ctx) -> PathBuf {
    ctx.target.join("linux-programs")
}

/// What the test says when musl-gcc is missing: ob doctor lists it, and the
/// packages are named for the systems that ob doctor knows.
fn musl_missing() -> String {
    "musl-gcc is not installed, and the linux-programs test builds linux-core with musl: \
     install musl-tools (Debian, Ubuntu), musl-gcc (Fedora) or musl (Arch Linux); \
     `./ob doctor` lists it"
        .into()
}

/// Builds linux-core with the host's C library and with musl into `out`.
/// Warnings are not errors here: the host's compiler is not the one the
/// source was kept clean with.
fn build_core(ctx: &Ctx, out: &Path) -> Result<(), String> {
    let source = ctx.root.join(CORE_SOURCE);
    for (compiler, program) in [("cc", CORE), ("musl-gcc", CORE_MUSL)] {
        if compiler == "musl-gcc" && process::find_program(compiler).is_none() {
            return Err(musl_missing());
        }
        let mut cc = std::process::Command::new(compiler);
        cc.args(["-std=c11", "-D_GNU_SOURCE", "-O2", "-Wall", "-Wextra", "-static", "-pthread"])
            .arg("-o")
            .arg(out.join(program))
            .arg(&source);
        host_tool(&mut cc)?;
    }
    Ok(())
}

/// Builds the tests of [`ON_LINUX`] as static Linux programs with the host's
/// C compiler into `out/posix`. The tests are kept clean with Radix's
/// compiler and C library, so this compiler's own warnings are not errors.
fn build_posix(ctx: &Ctx, out: &Path) -> Result<(), String> {
    let sources = ctx.root.join(POSIX_SOURCES);
    let folder = out.join("posix");
    std::fs::create_dir_all(&folder)
        .map_err(|err| format!("making {}: {err}", folder.display()))?;
    for name in ON_LINUX {
        let mut cc = std::process::Command::new("cc");
        cc.args(["-std=c11", "-D_GNU_SOURCE", "-O2", "-w", "-static", "-I"])
            .arg(&sources)
            .arg("-o")
            .arg(folder.join(name))
            .arg(sources.join(format!("{name}.c")))
            .args(["-lpthread", "-lm"]);
        host_tool(&mut cc).map_err(|err| format!("building the test {name} for Linux: {err}"))?;
    }
    Ok(())
}

/// Builds everything the disk holds into a folder of the build output and
/// writes the two scripts beside it; returns the folder.
fn build(ctx: &Ctx) -> Result<PathBuf, String> {
    let out = build_folder(ctx);
    if out.exists() {
        std::fs::remove_dir_all(&out)
            .map_err(|err| format!("clearing {}: {err}", out.display()))?;
    }
    std::fs::create_dir_all(&out).map_err(|err| format!("making {}: {err}", out.display()))?;
    build_core(ctx, &out)?;
    build_posix(ctx, &out)?;
    for (name, text) in [("run-core.sh", RUN_CORE), ("run-posix.sh", RUN_POSIX)] {
        let file = out.join(name);
        std::fs::write(&file, text).map_err(|err| format!("writing {}: {err}", file.display()))?;
    }
    Ok(out)
}

/// Makes the disk of Linux programs: FAT32 holding what [`build`] makes.
pub fn make_disk(ctx: &Ctx, disk: &Disk, path: &Path) -> Result<(), String> {
    let programs = build(ctx)?;
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

/// The last lines the screen shows, for a message about what it lacks.
fn screen_tail(screen: &[String]) -> String {
    let shown: Vec<&str> =
        screen.iter().map(String::as_str).filter(|line| !line.is_empty()).collect();
    shown[shown.len().saturating_sub(20)..].join(" | ")
}

/// Checks that a line of the screen is each of `lines`, as a run of
/// `program` printed them: whole lines, so that a count that says some
/// cases were skipped is not taken for one that says all passed.
fn require_lines(screen: &[String], program: &str, lines: &[&str]) -> Result<(), String> {
    for line in lines {
        if !screen.iter().any(|shown| shown.trim() == *line) {
            return Err(format!(
                "{program} did not say \"{line}\"; the screen shows: {}",
                screen_tail(screen)
            ));
        }
    }
    Ok(())
}

fn linux_programs(vm: &mut Vm, case: &Case) -> Result<(), String> {
    owner_signs_in(vm, case)?;
    vm.expect_anywhere(&format!("spark: mounted the FAT filesystem of vda at {VOLUME}"))?;

    // The two builds of linux-core, one after the other. Each says it ran on
    // Dukovo: where it does not, the program takes its answers for Linux's,
    // which are stricter in places and would hide a kernel that answers as
    // neither.
    for program in [CORE, CORE_MUSL] {
        run_within(vm, &format!("clear; sh {VOLUME}/run-core.sh {program}"), CORE_SECONDS)?;
        let screen = vm.screen()?;
        require_lines(
            &screen,
            program,
            &[
                "linux-core: running on Dukovo",
                &format!("linux-core: all {CORE_CASES} passed"),
                &format!("{program}: exit status 0"),
            ],
        )?;
    }

    // The C test programs, one at a time. A program that fails does not end
    // the test: each says a line, and the failures are reported together.
    run_within(vm, &format!("clear; sh {VOLUME}/run-posix.sh"), POSIX_SECONDS)?;
    let screen = vm.screen()?;
    if !screen.iter().any(|line| line.trim() == "linux-posix: every test passed") {
        let failed: Vec<&str> = screen
            .iter()
            .map(|line| line.trim())
            .filter(|line| line.starts_with("linux-posix: ") && line.contains(" FAILED"))
            .collect();
        return Err(format!(
            "the C test programs did not all pass: {}; the screen shows: {}",
            failed.join("; "),
            screen_tail(&screen)
        ));
    }

    vm.press_power_button()?;
    vm.expect("spark: every service has stopped")?;
    vm.expect("Power: `spark` asked to power off the machine; syncing the filesystems")?;
    vm.expect_exit(QEMU_EXIT_POWER_OFF)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The repository's root, from this crate's folder.
    fn root() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
    }

    fn read(path: &str) -> String {
        std::fs::read_to_string(root().join(path)).expect(path)
    }

    #[test]
    fn the_case_count_is_the_rows_of_the_programs_table() {
        let source = read(CORE_SOURCE);
        let table =
            source.split("CASES[] = {").nth(1).expect("the table").split("};").next().unwrap();
        assert_eq!(table.lines().filter(|line| line.starts_with("\t{\"")).count(), CORE_CASES);
    }

    #[test]
    fn the_tests_that_run_on_linux_are_c_programs_of_the_posix_folder() {
        for name in ON_LINUX {
            assert!(root().join(format!("{POSIX_SOURCES}/{name}.c")).is_file(), "{name}.c");
        }
    }

    #[test]
    fn the_scripts_run_each_program_under_a_time_limit() {
        // A program that hangs is ended by timeout, which keeps it from
        // holding the test.
        assert_eq!(RUN_CORE.matches("timeout -k").count(), 1);
        assert_eq!(RUN_POSIX.matches("timeout -k").count(), 1);
        assert!(musl_missing().contains("musl-tools"));
    }
}
