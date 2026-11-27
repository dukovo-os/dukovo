// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! `ob test linux-suite`: LTP's system call tests and the seccomp
//! differential, run on the build machine's Linux and in Dukovo's Linux
//! view, with Linux's results as the standard (ADR 0628).
//!
//! LTP, the Linux Test Project, tests Linux's system calls with a program
//! for each case: 1,552 cases at its release 20260930, of which those that
//! pass on a Linux machine as an ordinary user, some 800, are the measure.
//! ob fetches LTP at that release's commit, builds the system call tests
//! with the build machine's C compiler and glibc, and runs every case on
//! the build machine with `suite-run` (userspace/linux/suite-run.c), as an
//! ordinary user, one at a time, each limited to [`CASE_SECONDS`]. LTP is
//! GPL-2.0 code: it is built and run here and on the test machine, never
//! put in an image or a bundle.
//!
//! The cases that passed on Linux, but for the skips of
//! tools/xtask/src/test/linux-suite-skips.txt ([`read_skips`]), then run
//! on Dukovo, in a machine of their own, as go-suite's does: the disk holds
//! the Linux base's bundle, the cases' programs, their list, suite-run, and
//! seccomp-diff (userspace/posix/linux/seccomp-diff.c), which makes 2,000
//! seccomp filters from a seed and says, a line for each, what Linux or
//! Dukovo made of it. alice installs the base, copies the programs to her
//! home and runs suite-run and seccomp-diff under `linux run`, which gives
//! suite-run's folder to it and nothing else of hers. The results go back to
//! the disk, and ob judges them once the machine is off: every case that
//! passed on Linux and has no skip must pass on Dukovo, and seccomp-diff's
//! lines must be Linux's, one for one. A case that does not pass on Linux,
//! because it needs root, a device or a kernel option the build machine
//! lacks, is reported and not judged.
//!
//! ob writes what it found to `out/test/linux-suite.txt`: the counts on both
//! systems, each case that failed on Dukovo with its status, and the first
//! line where seccomp-diff's output parts, and under each case that failed
//! the first lines of its log that say why ([`faults_of`]). The logs of the
//! cases that did not pass on Dukovo are in `out/test/linux-suite-failed.log`,
//! every thread of the machine and what it waited for, each five minutes of
//! the run, in `out/test/linux-suite-threads.txt`, so that a run that hangs
//! shows where; the serial log, `out/test/boot-linux-suite.log`, has the
//! kernel's own words.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

use super::*;

/// The disk's label.
const LABEL: &str = "LINUXSUITE";

/// Where Spark mounts the disk: the second virtio disk's name.
const VOLUME: &str = "/volumes/vdb";

/// LTP's repository, and the commit of its release 20260930, which the
/// programs name as their version.
const LTP_GIT: &str = "https://github.com/linux-test-project/ltp.git";
const LTP_COMMIT: &str = "2279d708c817db657611a30c427c7d06c4360049";
const LTP_RELEASE: &str = "20260930";

/// The list of the system call cases, from LTP's root: a line a case, its
/// name and its command.
const RUNTEST: &str = "runtest/syscalls";

/// The libraries the cases may load: glibc's and GCC's, which the Linux
/// base holds. A case linked with another, which configure found on the
/// build machine, could not start on Dukovo.
const BASE_LIBRARIES: &[&str] = &[
    "libc.so.6",
    "libm.so.6",
    "libpthread.so.0",
    "libdl.so.2",
    "librt.so.1",
    "libresolv.so.2",
    "libgcc_s.so.1",
    "ld-linux-x86-64.so.2",
];

/// The runner and the seccomp differential, in the repository.
const RUNNER_SOURCE: &str = "userspace/linux/suite-run.c";
const SECCOMP_SOURCE: &str = "userspace/posix/linux/seccomp-diff.c";

/// The seed of seccomp-diff's filters, and how many it makes.
const SECCOMP_SEED: &str = "1";
const SECCOMP_FILTERS: &str = "2000";

/// The skips, from the repository's root.
const SKIPS: &str = "tools/xtask/src/test/linux-suite-skips.txt";

/// The deferred list, whose rows the skips name.
const DEFERRED: &str = "docs/deferred.md";

/// The longest a case may run, on either system, in seconds. LTP's library
/// ends a test that outlasts its own limit, 30 seconds and its runtime for
/// most and 2 minutes for some, such as kill11's core dumps, and suite-run
/// shortens the runtimes to a tenth: a case still running after this is
/// stuck past LTP's own watch, and suite-run ends it with its process
/// group.
const CASE_SECONDS: u64 = 180;

/// The names on the disk: the script, the folder of LTP's programs and the
/// list of the cases in it, the runner, seccomp-diff, and what the script
/// writes back.
const SCRIPT_FILE: &str = "linux-suite.sh";
const LTP_ON_DISK: &str = "ltp";
const CASES_FILE: &str = "cases.lst";
const RUNNER_FILE: &str = "suite-run";
const SECCOMP_FILE: &str = "seccomp-diff";
const RESULTS_FILE: &str = "results.txt";
const SECCOMP_RESULTS_FILE: &str = "seccomp.txt";
const FAILED_FILE: &str = "failed.log";
const THREADS_FILE: &str = "threads.txt";

/// What ob saves in the folder of the boot tests' logs: its report, and
/// what the run left on Dukovo, each file's path there, its name on the
/// disk, which it is copied to, and the name ob saves it by. The script's
/// own log is outside the folder it works in, which it makes anew.
const REPORT: &str = "linux-suite.txt";
const SAVED: [(&str, &str, &str); 5] = [
    ("/users/alice/linux-suite/out/results.txt", RESULTS_FILE, "linux-suite-dukovo.txt"),
    ("/users/alice/linux-suite/out/seccomp.txt", SECCOMP_RESULTS_FILE, "linux-suite-seccomp.txt"),
    ("/users/alice/linux-suite/out/failed.log", FAILED_FILE, "linux-suite-failed.log"),
    ("/users/alice/linux-suite/out/threads.txt", THREADS_FILE, "linux-suite-threads.txt"),
    (SCRIPT_LOG, "script.log", "linux-suite-script.log"),
];

/// Where the script's output goes on Dukovo.
const SCRIPT_LOG: &str = "/users/alice/linux-suite.log";

/// Where the script works: a folder of alice's home, which `linux run`
/// gives the runner as the folder it starts in.
const WORK: &str = "/users/alice/linux-suite";

/// What a run is limited to if the person who starts it names no other, in
/// seconds: the sum of the waits below.
const TOTAL_SECONDS: u64 =
    BOOT_SECONDS + INSTALL_SECONDS + COPY_SECONDS + SUITE_SECONDS + SAVE_SECONDS;

// Under the 150 minutes linux-suite.yml gives the step, which also builds
// LTP, runs it on the build machine, and builds the image and the disk.
const _: () = assert!(TOTAL_SECONDS < 120 * 60);

/// The boot, the owner's account and the root grown over its room.
const BOOT_SECONDS: u64 = 600;
/// Installing the Linux base.
const INSTALL_SECONDS: u64 = 300;
/// Copying the cases' programs, some 200 MiB, to alice's home.
const COPY_SECONDS: u64 = 600;
/// The cases and seccomp-diff: on Linux the cases that pass take some
/// six and a half minutes; a case that hangs on Dukovo takes LTP's limit,
/// or at worst [`CASE_SECONDS`].
const SUITE_SECONDS: u64 = 75 * 60;
/// Copying the results to the disk and the machine's power off.
const SAVE_SECONDS: u64 = 300;

/// The room after the root the image makes, which is 512 MiB: with it the
/// root has 2 GiB, for the base and the programs.
const ROOM_KIB: u64 = 1536 * 1024;

/// The script, `sh linux-suite.sh`, as `@VOLUME@`, where the disk is,
/// `@WORK@`, where it works, and the run's numbers are filled in. seccomp-diff,
/// which takes seconds, runs before the cases, so that a run of them that
/// hangs leaves its lines all the same. It ends with the line
/// `linux-suite: done`.
const SCRIPT: &str = r#"#!/bin/sh
volume=@VOLUME@
work=@WORK@
rm -rf "$work"
mkdir -p "$work/out"
cp -R "$volume/@LTP@" "$work/@LTP@"
cp "$volume/@RUNNER@" "$volume/@SECCOMP@" "$work/"
chmod 755 "$work/@RUNNER@" "$work/@SECCOMP@" "$work"/@LTP@/testcases/bin/*
cd "$work"
# Each five minutes, every thread and what it waits for.
(while sleep 300; do echo "== threads at $(date)"; ps -eLo pid,lwp,stat,wchan,args; done) \
	> "$work/out/@THREADS@" 2>&1 &
watch=$!
linux run -- "$work/@SECCOMP@" @SEED@ @FILTERS@ > "$work/out/@SECCOMP_RESULTS@" 2>&1
echo "seccomp-diff status $?"
linux run -- "$work/@RUNNER@" "$work/@LTP@/@CASES@" "$work/@LTP@" "$work/out" @SECONDS@
echo "suite-run status $?"
kill "$watch"
# The logs of the cases that did not pass, in one file.
awk '$2 != "0" { print $1 }' "$work/out/@RESULTS@" | while read -r name; do
	echo "== $name"
	cat "$work/out/logs/$name.log"
done > "$work/out/@FAILED@"
echo "linux-suite: done"
"#;

/// The script with the disk where Spark mounts it and its files in
/// [`WORK`].
fn script() -> String {
    script_for(VOLUME, WORK)
}

/// [`SCRIPT`] for a disk at `volume` and files in `work`.
fn script_for(volume: &str, work: &str) -> String {
    SCRIPT
        .replace("@VOLUME@", volume)
        .replace("@WORK@", work)
        .replace("@LTP@", LTP_ON_DISK)
        .replace("@CASES@", CASES_FILE)
        .replace("@RUNNER@", RUNNER_FILE)
        .replace("@SECCOMP@", SECCOMP_FILE)
        .replace("@SECONDS@", &CASE_SECONDS.to_string())
        .replace("@SEED@", SECCOMP_SEED)
        .replace("@FILTERS@", SECCOMP_FILTERS)
        .replace("@SECCOMP_RESULTS@", SECCOMP_RESULTS_FILE)
        .replace("@RESULTS@", RESULTS_FILE)
        .replace("@FAILED@", FAILED_FILE)
        .replace("@THREADS@", THREADS_FILE)
}

/// The disks: Dukovo's root on a copy of the test image with room after it,
/// and the base, the programs and the script.
const DISKS: &[Disk] = &[
    Disk {
        label: image::ROOT_LABEL,
        kib: ROOM_KIB,
        ..btrfs_disk(Bus::Virtio, 512, ROOT_ROOM, root_checked)
    },
    linux_suite_disk(512 * 1024, LABEL),
];

/// The machine.
const LINUX_SUITE: Case = Case {
    name: "linux-suite",
    about: "LTP's system call tests and the seccomp differential, run in Dukovo's Linux view and judged against the build machine's Linux",
    firmware: Firmware::Uefi,
    media: Media::Usb,
    cmdline: "",
    cpus: 4,
    memory: "4G",
    reboot: false,
    image: TestImage::Plain,
    cpu_model: None,
    qemu: &[],
    script: linux_suite,
    disks: DISKS,
    second: None,
};

/// Where ob builds and runs on the build machine.
fn work_folder(ctx: &Ctx) -> PathBuf {
    ctx.target.join("linux-suite")
}

/// LTP's installed system call tests: `testcases/bin` beneath it.
fn ltp_root(ctx: &Ctx) -> PathBuf {
    work_folder(ctx).join("ltp")
}

/// `ob test linux-suite`: builds LTP and runs it on the build machine, then
/// builds what Dukovo's machine needs, boots it and runs the cases that
/// passed. `limit` is how long the machine may run, or the sum of the waits
/// ([`TOTAL_SECONDS`]) if nothing is given.
pub fn command(ctx: &Ctx, profile: crate::config::Profile, limit: Option<Duration>) -> Result<()> {
    if running_as_root()? {
        return Err(Error::new(
            "LTP's tests are judged as an ordinary user runs them, and as root they test other \
             things",
        )
        .hint("run `./ob test linux-suite` as an ordinary user"));
    }
    let deferred = read_text(ctx, DEFERRED)?;
    let skips = parse_skips(&read_text(ctx, SKIPS)?, &deferred)
        .map_err(|err| Error::new(format!("{SKIPS}: {err}")))?;
    let source = crate::ports::fetch(ctx, "ltp", LTP_GIT, LTP_COMMIT, &[])?;
    let all = parse_cases(
        &std::fs::read_to_string(source.join(RUNTEST))
            .context(|| format!("reading {}", source.join(RUNTEST).display()))?,
    );
    check_skips_name_cases(&skips, &all).map_err(|err| Error::new(format!("{SKIPS}: {err}")))?;
    build_ltp(ctx, &source)?;
    let work = work_folder(ctx);
    let runner = build_c(ctx, RUNNER_SOURCE, &work.join(RUNNER_FILE))?;
    let seccomp = build_c(ctx, SECCOMP_SOURCE, &work.join(SECCOMP_FILE))?;

    step(&format!("Running LTP's {} system call cases on this machine's Linux", all.len()));
    let on_linux = work.join("on-linux");
    if on_linux.exists() {
        std::fs::remove_dir_all(&on_linux)
            .context(|| format!("clearing {}", on_linux.display()))?;
    }
    let list = work.join("all.lst");
    std::fs::write(&list, case_list(&all)).context(|| format!("writing {}", list.display()))?;
    let mut run = std::process::Command::new(&runner);
    run.arg(&list).arg(ltp_root(ctx)).arg(&on_linux).arg(CASE_SECONDS.to_string());
    process::run(ctx, &mut run)?;
    let linux = parse_results(&read_file(&on_linux.join(RESULTS_FILE))?);
    let mut diff = std::process::Command::new(&seccomp);
    diff.args([SECCOMP_SEED, SECCOMP_FILTERS]);
    let expected = process::output(ctx, &mut diff)?;
    if !expected.status.success() {
        return Err(Error::new(format!(
            "seccomp-diff failed on this machine ({})",
            expected.status
        )));
    }
    std::fs::write(work.join("seccomp-linux.txt"), &expected.stdout)
        .context(|| "writing seccomp-diff's lines".to_string())?;

    let chosen = chosen_cases(&all, &linux, &skips);
    std::fs::write(work.join(CASES_FILE), case_list(&chosen))
        .context(|| format!("writing {}", work.join(CASES_FILE).display()))?;
    step(&format!(
        "{} cases passed on Linux; {} of them run on Dukovo, {} skipped",
        linux.values().filter(|outcome| outcome.status == "0").count(),
        chosen.len(),
        skipped(&all, &linux, &skips).len()
    ));

    // What an earlier run saved is not this one's.
    let logs = ctx.out.join("test");
    for (_, _, saved) in SAVED {
        let _ = std::fs::remove_file(logs.join(saved));
    }
    let limit = limit.unwrap_or(Duration::from_secs(TOTAL_SECONDS));
    step(&format!(
        "Running them in Dukovo's Linux view, for up to {} minutes",
        limit.as_secs() / 60
    ));
    let booted = boot_tests_of(ctx, profile, &[&LINUX_SUITE], limit, false);

    // Whatever the machine got through is judged and reported, even when it
    // did not end in time.
    let saved = |index: usize| read_file(&logs.join(SAVED[index].2)).unwrap_or_default();
    let verdict = Verdict {
        why: faults_of(&saved(2)),
        ..Verdict::judge(
            &all,
            &linux,
            &skips,
            &chosen,
            &parse_results(&saved(0)),
            &String::from_utf8_lossy(&expected.stdout),
            &saved(1),
        )
    };
    let report = logs.join(REPORT);
    std::fs::write(&report, verdict.report())
        .context(|| format!("writing {}", report.display()))?;
    step(&format!("The suite's report is {}", ctx.display(&report)));
    booted?;
    match verdict.fault() {
        Some(fault) => Err(Error::new(format!("{fault}; {} has the list", ctx.display(&report)))),
        None => Ok(()),
    }
}

/// Whether ob runs as root.
fn running_as_root() -> Result<bool> {
    use std::os::unix::fs::MetadataExt as _;
    let own = std::fs::metadata("/proc/self").context(|| "reading /proc/self".to_string())?;
    Ok(own.uid() == 0)
}

/// The text of `path`, from the repository's root.
fn read_text(ctx: &Ctx, path: &str) -> Result<String> {
    std::fs::read_to_string(ctx.root.join(path)).context(|| format!("reading {path}"))
}

/// The text of the file `path`, whatever bytes it holds.
fn read_file(path: &Path) -> Result<String> {
    let bytes = std::fs::read(path).context(|| format!("reading {}", path.display()))?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

/// Builds LTP's system call tests from `source` and installs them in
/// [`ltp_root`], once for each commit: the build leaves a stamp of it.
fn build_ltp(ctx: &Ctx, source: &Path) -> Result<()> {
    let root = ltp_root(ctx);
    let stamp = root.join(".built");
    if std::fs::read_to_string(&stamp).is_ok_and(|built| built.trim() == LTP_COMMIT) {
        return Ok(());
    }
    step(&format!("Building LTP {LTP_RELEASE}'s system call tests"));
    if root.exists() {
        std::fs::remove_dir_all(&root).context(|| format!("clearing {}", root.display()))?;
    }
    let jobs = std::thread::available_parallelism().map_or(4, std::num::NonZero::get).to_string();
    let make = |args: &[&str]| {
        let mut cmd = std::process::Command::new("make");
        cmd.current_dir(source).args(args);
        cmd
    };
    process::run(ctx, &mut make(&["autotools"]))?;
    // NUMA's and TI-RPC's libraries are not in the Linux base, and the
    // system call tests do without them.
    let mut configure = std::process::Command::new("./configure");
    configure
        .current_dir(source)
        .arg(format!("--prefix={}", root.display()))
        .args(["--without-numa", "--without-tirpc"]);
    process::run(ctx, &mut configure)?;
    process::run(ctx, &mut make(&["-j", &jobs, "-C", "lib"]))?;
    process::run(ctx, &mut make(&["-j", &jobs, "-C", "testcases/kernel/syscalls"]))?;
    process::run(ctx, &mut make(&["-C", "testcases/kernel/syscalls", "install"]))?;
    check_libraries(&root.join("testcases/bin"))?;
    std::fs::write(&stamp, LTP_COMMIT).context(|| format!("writing {}", stamp.display()))
}

/// Checks that each program in `bin` loads only libraries the Linux base
/// has ([`BASE_LIBRARIES`]).
fn check_libraries(bin: &Path) -> Result<()> {
    let mut wrong = Vec::new();
    for entry in std::fs::read_dir(bin).context(|| format!("reading {}", bin.display()))? {
        let path = entry.context(|| format!("reading {}", bin.display()))?.path();
        let bytes = std::fs::read(&path).context(|| format!("reading {}", path.display()))?;
        for library in needed_libraries(&bytes) {
            if !BASE_LIBRARIES.contains(&library.as_str()) {
                wrong.push(format!("{} needs {library}", path.display()));
            }
        }
    }
    if wrong.is_empty() {
        return Ok(());
    }
    Err(Error::new(format!(
        "LTP's programs need libraries the Linux base does not have: {}",
        wrong.join(", ")
    ))
    .hint("configure found them on this machine; build LTP where it does not, or take them out"))
}

/// The libraries an ELF program names in its dynamic section, `DT_NEEDED`.
/// None for a file that is no ELF, such as a script.
fn needed_libraries(bytes: &[u8]) -> Vec<String> {
    const DT_NEEDED: u64 = 1;
    let Some(elf) = crate::elf::Elf::parse(bytes) else { return Vec::new() };
    let (Some(dynamic), Some(strings)) = (elf.section(".dynamic"), elf.section(".dynstr")) else {
        return Vec::new();
    };
    let (Some(entries), Some(strings)) = (elf.data(dynamic), elf.data(strings)) else {
        return Vec::new();
    };
    // Each entry is a tag and a value, 8 bytes each.
    entries
        .as_chunks::<16>()
        .0
        .iter()
        .filter_map(|entry| {
            let tag = crate::elf::read_u64(entry, 0)?;
            let value = usize::try_from(crate::elf::read_u64(entry, 8)?).ok()?;
            (tag == DT_NEEDED).then(|| crate::elf::c_string(strings, value).map(str::to_string))?
        })
        .collect()
}

/// Builds the C program `source` with the build machine's compiler, as a
/// Linux program that runs with glibc, at `out`.
fn build_c(ctx: &Ctx, source: &str, out: &Path) -> Result<PathBuf> {
    if let Some(folder) = out.parent() {
        process::create_dir(folder)?;
    }
    let mut cc = std::process::Command::new("cc");
    cc.args(["-O2", "-Wall", "-o"]).arg(out).arg(ctx.root.join(source));
    process::run(ctx, &mut cc)?;
    Ok(out.to_path_buf())
}

/// A case of LTP's list: its name and its command, a program and its
/// arguments.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Entry {
    name: String,
    command: String,
}

impl Entry {
    /// The program the command runs.
    fn program(&self) -> &str {
        self.command.split_whitespace().next().unwrap_or_default()
    }
}

/// Reads a runtest file: a line a case, its name and its command, and
/// blank lines and `#` comments.
fn parse_cases(text: &str) -> Vec<Entry> {
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .filter_map(|line| {
            let (name, command) = line.split_once(char::is_whitespace)?;
            let command = command.split_whitespace().collect::<Vec<_>>().join(" ");
            (!command.is_empty()).then(|| Entry { name: name.to_string(), command })
        })
        .collect()
}

/// The text of a list of cases, as suite-run reads it.
fn case_list(cases: &[Entry]) -> String {
    cases.iter().fold(String::new(), |mut list, case| {
        let _ = writeln!(list, "{} {}", case.name, case.command);
        list
    })
}

/// How a case ended, as suite-run writes it: its status and how long it
/// ran.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Outcome {
    status: String,
    millis: u64,
}

/// Reads suite-run's results: a line a case, its name, its status and the
/// milliseconds it ran. A line cut short, as the last of a run that was
/// stopped may be, is left out.
fn parse_results(text: &str) -> BTreeMap<String, Outcome> {
    text.lines()
        .filter_map(|line| {
            let mut words = line.split_whitespace();
            let (name, status, millis) = (words.next()?, words.next()?, words.next()?);
            let millis = millis.parse().ok()?;
            Some((name.to_string(), Outcome { status: status.to_string(), millis }))
        })
        .collect()
}

/// A skip: the cases a pattern names, why, and the row of the deferred list
/// that holds the work.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Skip {
    pattern: String,
    reason: String,
}

impl Skip {
    fn matches(&self, name: &str) -> bool {
        matches_pattern(&self.pattern, name)
    }
}

/// Whether `name` matches `pattern`, in which a `*` stands for any run of
/// characters.
fn matches_pattern(pattern: &str, name: &str) -> bool {
    match pattern.split_once('*') {
        None => pattern == name,
        Some((head, tail)) => {
            let Some(rest) = name.strip_prefix(head) else { return false };
            (0..=rest.len())
                .filter(|&at| rest.is_char_boundary(at))
                .any(|at| matches_pattern(tail, &rest[at..]))
        }
    }
}

/// Reads the text of the skip file: a line each of a case's name, or a
/// pattern of names with `*`, and the reason, which names the feature
/// Dukovo lacks and quotes words of the row of `deferred`, the deferred
/// list, that holds the work: nothing is skipped without a row to bring it
/// back, and a row that is closed fails the skips that quote it.
fn parse_skips(text: &str, deferred: &str) -> std::result::Result<Vec<Skip>, String> {
    let rows: Vec<&str> = deferred.lines().filter(|line| line.starts_with('|')).collect();
    let mut skips: Vec<Skip> = Vec::new();
    for (i, line) in text.lines().enumerate() {
        let (at, line) = (i + 1, line.trim());
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (pattern, reason) = line.split_once(char::is_whitespace).unwrap_or((line, ""));
        let reason = reason.trim();
        let quoted = reason.split('"').nth(1).filter(|words| !words.trim().is_empty());
        let Some(quoted) = quoted else {
            return Err(format!(
                "line {at}: a skip is a case and the reason, which quotes words of its row of \
                 {DEFERRED} in double quotes"
            ));
        };
        if !rows.iter().any(|row| row.contains(quoted)) {
            return Err(format!("line {at}: no row of {DEFERRED} has \"{quoted}\""));
        }
        if !pattern.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '*') {
            return Err(format!("line {at}: `{pattern}` is not the name of a case or a pattern"));
        }
        if skips.iter().any(|skip| skip.pattern == pattern) {
            return Err(format!("line {at}: {pattern} is skipped twice"));
        }
        skips.push(Skip { pattern: pattern.to_string(), reason: reason.to_string() });
    }
    Ok(skips)
}

/// Checks that each skip names a case of the list.
fn check_skips_name_cases(skips: &[Skip], cases: &[Entry]) -> std::result::Result<(), String> {
    for skip in skips {
        if !cases.iter().any(|case| skip.matches(&case.name)) {
            return Err(format!("{} names no case of LTP's {RUNTEST}", skip.pattern));
        }
    }
    Ok(())
}

/// The cases that run on Dukovo: those that passed on Linux and that no
/// skip names, in the list's order.
fn chosen_cases(cases: &[Entry], linux: &BTreeMap<String, Outcome>, skips: &[Skip]) -> Vec<Entry> {
    cases
        .iter()
        .filter(|case| linux.get(&case.name).is_some_and(|outcome| outcome.status == "0"))
        .filter(|case| !skips.iter().any(|skip| skip.matches(&case.name)))
        .cloned()
        .collect()
}

/// The cases that passed on Linux and that a skip names.
fn skipped<'a>(
    cases: &'a [Entry],
    linux: &BTreeMap<String, Outcome>,
    skips: &[Skip],
) -> Vec<&'a Entry> {
    cases
        .iter()
        .filter(|case| linux.get(&case.name).is_some_and(|outcome| outcome.status == "0"))
        .filter(|case| skips.iter().any(|skip| skip.matches(&case.name)))
        .collect()
}

/// Makes the disk: FAT32 holding the Linux base's bundle, the programs of
/// the cases [`command`] chose, stripped, their list, suite-run,
/// seccomp-diff and the script.
pub fn make_disk(ctx: &Ctx, disk: &Disk, path: &Path) -> Result<(), String> {
    make_bundle_disk(ctx, disk, path, |name| name.starts_with(cases::linux_view::BUNDLE_PREFIX))?;
    let work = work_folder(ctx);
    let cases =
        parse_cases(&std::fs::read_to_string(work.join(CASES_FILE)).map_err(|err| {
            format!("reading the cases chosen (run `./ob test linux-suite`): {err}")
        })?);
    let staged = work.join("disk");
    if staged.exists() {
        std::fs::remove_dir_all(&staged)
            .map_err(|err| format!("clearing {}: {err}", staged.display()))?;
    }
    let bin = staged.join(LTP_ON_DISK).join("testcases/bin");
    std::fs::create_dir_all(&bin).map_err(|err| format!("making {}: {err}", bin.display()))?;
    let installed = ltp_root(ctx).join("testcases/bin");
    let programs: BTreeSet<&str> = cases.iter().map(Entry::program).collect();
    for program in programs {
        host_tool(
            std::process::Command::new("strip")
                .arg("--strip-unneeded")
                .arg("-o")
                .arg(bin.join(program))
                .arg(installed.join(program)),
        )?;
    }
    let ltp = staged.join(LTP_ON_DISK);
    let write = |path: PathBuf, text: String| {
        std::fs::write(&path, text).map_err(|err| format!("writing {}: {err}", path.display()))
    };
    write(ltp.join(CASES_FILE), case_list(&cases))?;
    write(staged.join(SCRIPT_FILE), script())?;
    for file in [RUNNER_FILE, SECCOMP_FILE] {
        std::fs::copy(work.join(file), staged.join(file))
            .map_err(|err| format!("copying {file}: {err}"))?;
    }
    let image = path.display().to_string();
    for name in [LTP_ON_DISK, SCRIPT_FILE, RUNNER_FILE, SECCOMP_FILE] {
        host_tool(
            std::process::Command::new("mcopy")
                .args(["-s", "-i", &image])
                .arg(staged.join(name))
                .arg(format!("::/{name}")),
        )?;
    }
    Ok(())
}

fn linux_suite(vm: &mut Vm, case: &Case) -> Result<(), String> {
    use super::cases::go_toolchain::succeeds;
    owner_signs_in(vm, case)?;
    vm.expect_anywhere("bundled: 0 bundles installed; answering at /system/live/bundled/socket")?;
    vm.expect_anywhere(&format!("spark: mounted the FAT filesystem of vdb at {VOLUME}"))?;
    unlock(vm)?;
    answer_prompt(vm, "$", "clear; elevate rdisk --grow /devices/vda")?;
    vm.expect_screen(&["/devices/vda3 grows from 512 MiB to 2 GiB."])?;
    answer_prompt(vm, "$", "clear; elevate device resize max /system/settings")?;
    vm.expect_screen(&["Resized device id 1 of '/system/settings' to "])?;
    succeeds(
        vm,
        "base",
        &format!("cd {VOLUME}; bundle install --yes {}", cases::linux_view::FILE),
        INSTALL_SECONDS,
    )?;

    // A run that outlasts its wait is stopped, and what it wrote so far is
    // saved all the same, so that the results show where it was.
    let ran = run_within(
        vm,
        &format!(
            "clear; cd; sh {VOLUME}/{SCRIPT_FILE} > {SCRIPT_LOG} 2>&1; tail -n 4 {SCRIPT_LOG}"
        ),
        COPY_SECONDS + SUITE_SECONDS,
    );
    if ran.is_err() {
        vm.press(&["ctrl-c"])?;
    }

    // The disk belongs to root: the results go there through elevate,
    // unlocked again as the run took longer than it stays unlocked.
    // A file the run did not get to write is passed over.
    unlock(vm)?;
    for (from, name, _) in SAVED {
        succeeds(
            vm,
            "save",
            &format!("[ ! -f {from} ] || elevate cp {from} {VOLUME}/{name}"),
            SAVE_SECONDS,
        )?;
    }
    vm.press_power_button()?;
    vm.expect("spark: every service has stopped")?;
    vm.expect("Power: `spark` asked to power off the machine; syncing the filesystems")?;
    vm.expect_exit(QEMU_EXIT_POWER_OFF)?;

    let image = vm.dir().join(format!("{}-disk1.img", case.name)).display().to_string();
    for (_, name, saved) in SAVED {
        let text = host_tool(std::process::Command::new("mtype").args([
            "-i",
            &image,
            &format!("::/{name}"),
        ]))
        .unwrap_or_default();
        let saved = vm.dir().join(saved);
        std::fs::write(&saved, text)
            .map_err(|err| format!("writing {}: {err}", saved.display()))?;
    }
    ran.map_err(|err| format!("the run did not end in time ({err})"))
}

/// What the run found, on both systems.
#[derive(Debug, PartialEq, Eq)]
struct Verdict {
    /// LTP's cases, all of them.
    cases: usize,
    /// How the cases ended on Linux: passed, did not apply, and the
    /// others, each with its status.
    linux_passed: usize,
    linux_not_applied: usize,
    linux_other: Vec<(String, String)>,
    /// The cases that passed on Linux and that skips name, with the reason.
    skipped: Vec<(String, String)>,
    /// How many cases ran on Dukovo, and how many of them passed.
    chosen: usize,
    dukovo_passed: usize,
    /// The cases that passed on Linux and did not on Dukovo, with their
    /// status there: `missing` when the run ended before them.
    failed: Vec<(String, String)>,
    /// What the logs of the failed cases say of why ([`faults_of`]).
    why: BTreeMap<String, Vec<String>>,
    /// The milliseconds the chosen cases ran, on Linux and on Dukovo.
    linux_millis: u64,
    dukovo_millis: u64,
    /// Where seccomp-diff's lines part, if they do.
    seccomp: Option<String>,
    /// seccomp-diff's last line on Linux, its count and digest.
    seccomp_summary: String,
}

impl Verdict {
    fn judge(
        all: &[Entry],
        linux: &BTreeMap<String, Outcome>,
        skips: &[Skip],
        chosen: &[Entry],
        dukovo: &BTreeMap<String, Outcome>,
        seccomp_linux: &str,
        seccomp_dukovo: &str,
    ) -> Verdict {
        let status = |name: &str| linux.get(name).map_or("missing", |outcome| &outcome.status);
        let linux_other = all
            .iter()
            .filter(|case| !matches!(status(&case.name), "0" | "32"))
            .map(|case| (case.name.clone(), status(&case.name).to_string()))
            .collect();
        let skipped = skipped(all, linux, skips)
            .into_iter()
            .map(|case| {
                let skip = skips.iter().find(|skip| skip.matches(&case.name));
                (case.name.clone(), skip.map_or_else(String::new, |skip| skip.reason.clone()))
            })
            .collect();
        let failed: Vec<(String, String)> = chosen
            .iter()
            .filter_map(|case| match dukovo.get(&case.name) {
                Some(outcome) if outcome.status == "0" => None,
                Some(outcome) => Some((case.name.clone(), outcome.status.clone())),
                None => Some((case.name.clone(), "missing".to_string())),
            })
            .collect();
        let millis = |results: &BTreeMap<String, Outcome>| -> u64 {
            chosen.iter().filter_map(|case| results.get(&case.name)).map(|o| o.millis).sum()
        };
        Verdict {
            cases: all.len(),
            linux_passed: all.iter().filter(|case| status(&case.name) == "0").count(),
            linux_not_applied: all.iter().filter(|case| status(&case.name) == "32").count(),
            linux_other,
            skipped,
            chosen: chosen.len(),
            dukovo_passed: chosen.len() - failed.len(),
            failed,
            why: BTreeMap::new(),
            linux_millis: millis(linux),
            dukovo_millis: millis(dukovo),
            seccomp: seccomp_difference(seccomp_linux, seccomp_dukovo),
            seccomp_summary: seccomp_linux.lines().last().unwrap_or_default().to_string(),
        }
    }

    /// The report ob writes: the counts, then the cases by what became of
    /// them.
    fn report(&self) -> String {
        let seconds = |millis: u64| millis.div_ceil(1000);
        let mut text = format!(
            "linux-suite: LTP {LTP_RELEASE}, {RUNTEST}: {} cases\n\
             On Linux: {} passed, {} did not apply (TCONF), {} other\n\
             On Dukovo: {} of the {} that passed on Linux ran, {} skipped; {} passed, {} did \
             not\n\
             Time of the cases that ran on Dukovo: {} s on Linux, {} s on Dukovo\n",
            self.cases,
            self.linux_passed,
            self.linux_not_applied,
            self.linux_other.len(),
            self.chosen,
            self.linux_passed,
            self.skipped.len(),
            self.dukovo_passed,
            self.failed.len(),
            seconds(self.linux_millis),
            seconds(self.dukovo_millis),
        );
        match &self.seccomp {
            None => text
                .push_str(&format!("seccomp-diff: the same on both ({})\n", self.seccomp_summary)),
            Some(difference) => text.push_str(&format!("seccomp-diff: {difference}\n")),
        }
        let mut list = |heading: &str, cases: &[(String, String)]| {
            if !cases.is_empty() {
                text.push_str(&format!("\n{heading}:\n"));
                for (name, why) in cases {
                    text.push_str(&format!("  {name} {why}\n"));
                    for line in self.why.get(name).into_iter().flatten() {
                        text.push_str(&format!("      {line}\n"));
                    }
                }
            }
        };
        list(
            "Passed on Linux and not on Dukovo (LTP's status adds 1 for a failure, 2 for a \
             broken test, 4 for a warning, 32 for a test that does not apply)",
            &self.failed,
        );
        list("Skipped", &self.skipped);
        list("Did not pass on Linux, so not judged", &self.linux_other);
        text
    }

    /// Why the run did not pass, if it did not.
    fn fault(&self) -> Option<String> {
        let mut faults = Vec::new();
        if !self.failed.is_empty() {
            let names: Vec<&str> = self.failed.iter().map(|(name, _)| name.as_str()).collect();
            let shown = names.iter().take(12).copied().collect::<Vec<_>>().join(", ");
            let more = names.len().saturating_sub(12);
            faults.push(format!(
                "{} of {} cases that pass on Linux did not pass on Dukovo: {shown}{}",
                self.failed.len(),
                self.chosen,
                if more > 0 { format!(" and {more} more") } else { String::new() }
            ));
        }
        if let Some(difference) = &self.seccomp {
            faults.push(format!("seccomp-diff's lines are not Linux's: {difference}"));
        }
        (!faults.is_empty()).then(|| faults.join("; "))
    }
}

/// The most lines of a failed case's log the report shows, and the most
/// characters of each.
const WHY_LINES: usize = 2;
const WHY_WIDTH: usize = 160;

/// What the logs of the failed cases say of why, from the file of them
/// the script writes (`== NAME` and then its log, a case after another):
/// for each, its first lines that LTP marks as a failure, a broken test,
/// a warning or a test that does not apply, or else the last line it
/// wrote, each cut to [`WHY_WIDTH`] characters.
fn faults_of(failed: &str) -> BTreeMap<String, Vec<String>> {
    let mut faults = BTreeMap::new();
    for section in failed.split("== ").filter(|section| !section.is_empty()) {
        let (name, log) = section.split_once('\n').unwrap_or((section, ""));
        let cut = |line: &str| line.trim().chars().take(WHY_WIDTH).collect::<String>();
        let marked = |line: &&str| {
            ["TFAIL: ", "TBROK: ", "TWARN: ", "TCONF: "].iter().any(|mark| line.contains(mark))
        };
        let mut why: Vec<String> = log.lines().filter(marked).take(WHY_LINES).map(cut).collect();
        if why.is_empty() {
            why.extend(log.lines().rev().find(|line| !line.trim().is_empty()).map(cut));
        }
        faults.insert(name.trim().to_string(), why);
    }
    faults
}

/// Where seccomp-diff's lines on Dukovo part from Linux's, if they do.
fn seccomp_difference(linux: &str, dukovo: &str) -> Option<String> {
    let (linux, dukovo): (Vec<&str>, Vec<&str>) =
        (linux.lines().collect(), dukovo.lines().collect());
    let at = (0..linux.len().max(dukovo.len())).find(|&at| linux.get(at) != dukovo.get(at))?;
    let line =
        |lines: &[&str]| lines.get(at).map_or("no line".to_string(), |line| format!("`{line}`"));
    Some(format!("line {} is {} on Linux and {} on Dukovo", at + 1, line(&linux), line(&dukovo)))
}

#[cfg(test)]
mod tests {
    use super::*;

    const DEFERRED_TEXT: &str = "| Item | Why | Waits on |\n| --- | --- | --- |\n\
        | Open file description locks (F_OFD_SETLK) | no one asked | A need |\n\
        Prose that mentions inotify is no row.\n";

    fn entries(text: &str) -> Vec<Entry> {
        parse_cases(text)
    }

    fn outcomes(pairs: &[(&str, &str, u64)]) -> BTreeMap<String, Outcome> {
        pairs
            .iter()
            .map(|(name, status, millis)| {
                (name.to_string(), Outcome { status: status.to_string(), millis: *millis })
            })
            .collect()
    }

    #[test]
    fn a_runtest_file_gives_each_case_its_name_and_command() {
        let cases = entries(
            "#DESCRIPTION:Kernel system calls\nabort01 abort01\n\n\
             mmap10_1   mmap10 -a\nfcntl12_64 fcntl12_64\n",
        );
        assert_eq!(
            cases,
            [
                Entry { name: "abort01".into(), command: "abort01".into() },
                Entry { name: "mmap10_1".into(), command: "mmap10 -a".into() },
                Entry { name: "fcntl12_64".into(), command: "fcntl12_64".into() },
            ]
        );
        assert_eq!(cases[1].program(), "mmap10");
        assert_eq!(case_list(&cases[1..2]), "mmap10_1 mmap10 -a\n");
    }

    #[test]
    fn results_are_read_a_case_a_line_and_a_line_cut_short_is_left_out() {
        let results = parse_results("abort01 0 12\naccept02 32 5\nfcntl36 timeout 60000\nbrk0");
        assert_eq!(
            results,
            outcomes(&[("abort01", "0", 12), ("accept02", "32", 5), ("fcntl36", "timeout", 60000)])
        );
    }

    #[test]
    fn a_skip_quotes_its_row_of_the_deferred_list() {
        let skips = parse_skips(
            "# comment\nfcntl3* has no OFD locks \"Open file description locks\"\n",
            DEFERRED_TEXT,
        )
        .unwrap();
        assert_eq!(skips.len(), 1);
        assert!(skips[0].matches("fcntl36_64") && skips[0].matches("fcntl3"));
        assert!(!skips[0].matches("fcntl12"));

        let refused = |text: &str| parse_skips(text, DEFERRED_TEXT).unwrap_err();
        assert!(refused("fcntl36 it fails\n").contains("quotes words of its row"));
        assert!(refused("inotify01 no \"inotify\"\n").contains("no row of docs/deferred.md"));
        assert!(refused("fcntl36 \"\"\n").contains("quotes words"));
        assert!(refused("fcntl-36 \"Open file\"\n").contains("is not the name of a case"));
        assert!(
            refused("fcntl36 a \"Open file\"\nfcntl36 b \"Open file\"\n").contains("skipped twice")
        );
    }

    #[test]
    fn a_skip_must_name_a_case() {
        let cases = entries("abort01 abort01\nfcntl36 fcntl36\n");
        let skip = |pattern: &str| Skip { pattern: pattern.into(), reason: "r".into() };
        assert!(check_skips_name_cases(&[skip("fcntl*")], &cases).is_ok());
        assert!(
            check_skips_name_cases(&[skip("inotify*")], &cases).unwrap_err().contains("inotify*")
        );
    }

    #[test]
    fn the_cases_that_passed_on_linux_and_have_no_skip_run_on_dukovo() {
        let cases = entries("abort01 abort01\naccept01 accept01\nfcntl36 fcntl36\nbrk01 brk01\n");
        let linux = outcomes(&[
            ("abort01", "0", 10),
            ("accept01", "32", 1),
            ("fcntl36", "0", 30),
            ("brk01", "0", 5),
        ]);
        let skips = [Skip { pattern: "fcntl*".into(), reason: "\"Open file\"".into() }];
        let chosen = chosen_cases(&cases, &linux, &skips);
        let names: Vec<&str> = chosen.iter().map(|case| case.name.as_str()).collect();
        assert_eq!(names, ["abort01", "brk01"]);
        assert_eq!(skipped(&cases, &linux, &skips).len(), 1);

        // brk01 fails on Dukovo, and seccomp-diff's second line differs.
        let dukovo = outcomes(&[("abort01", "0", 40), ("brk01", "1", 20)]);
        let verdict = Verdict::judge(
            &cases,
            &linux,
            &skips,
            &chosen,
            &dukovo,
            "0: a\n1: b\nseccomp-diff: 2 filters, digest 1\n",
            "0: a\n1: c\nseccomp-diff: 2 filters, digest 2\n",
        );
        assert_eq!(verdict.failed, [("brk01".to_string(), "1".to_string())]);
        assert_eq!((verdict.chosen, verdict.dukovo_passed), (2, 1));
        assert_eq!((verdict.linux_millis, verdict.dukovo_millis), (15, 60));
        assert_eq!(
            verdict.seccomp.as_deref(),
            Some("line 2 is `1: b` on Linux and `1: c` on Dukovo")
        );
        let fault = verdict.fault().unwrap();
        assert!(fault.starts_with("1 of 2 cases that pass on Linux did not pass on Dukovo: brk01"));
        assert!(fault.contains("seccomp-diff's lines are not Linux's"), "{fault}");
        let report = verdict.report();
        assert!(
            report.contains("On Linux: 3 passed, 1 did not apply (TCONF), 0 other\n"),
            "{report}"
        );
        assert!(report.contains("  brk01 1\n"), "{report}");
        let verdict = Verdict {
            why: faults_of("== brk01\nbrk01.c:20: TFAIL: brk() failed: ENOMEM\n"),
            ..verdict
        };
        let report = verdict.report();
        assert!(report.contains("  brk01 1\n      brk01.c:20: TFAIL: brk() failed: ENOMEM\n"));
        assert!(report.contains("Skipped:\n  fcntl36 \"Open file\"\n"), "{report}");

        // A run that ended early leaves cases missing.
        let verdict =
            Verdict::judge(&cases, &linux, &skips, &chosen, &outcomes(&[]), "same\n", "same\n");
        assert_eq!(verdict.failed.len(), 2);
        assert_eq!(verdict.failed[0].1, "missing");
        assert_eq!(verdict.seccomp, None);

        // A whole run passes.
        let dukovo = outcomes(&[("abort01", "0", 40), ("brk01", "0", 20)]);
        let verdict = Verdict::judge(&cases, &linux, &skips, &chosen, &dukovo, "x\n", "x\n");
        assert_eq!(verdict.fault(), None);
        assert!(verdict.report().contains("seccomp-diff: the same on both (x)\n"));
    }

    #[test]
    fn a_failed_case_is_shown_with_the_lines_that_say_why() {
        let failed = "== mmap01\ntst_test.c:1: TINFO: Timeout per run is 0h 00m 30s\n\
                      mmap01.c:9: TFAIL: mmap() failed: EINVAL\n\
                      mmap01.c:12: TBROK: munmap() failed: EINVAL\n\
                      mmap01.c:14: TFAIL: a third\n\
                      == pipe08\nstarted\nwaiting\n\n";
        let faults = faults_of(failed);
        assert_eq!(
            faults["mmap01"],
            [
                "mmap01.c:9: TFAIL: mmap() failed: EINVAL",
                "mmap01.c:12: TBROK: munmap() failed: EINVAL"
            ]
        );
        // A log with no marked line shows its last.
        assert_eq!(faults["pipe08"], ["waiting"]);
        let long = format!("== x\nx.c:1: TFAIL: {}\n", "a".repeat(400));
        assert_eq!(faults_of(&long)["x"][0].chars().count(), WHY_WIDTH);
        assert!(faults_of("").is_empty());
    }

    #[test]
    fn patterns_match_any_run_of_characters_for_a_star() {
        assert!(matches_pattern("fcntl*_64", "fcntl12_64"));
        assert!(!matches_pattern("fcntl*_64", "fcntl12"));
        assert!(matches_pattern("*", "anything"));
        assert!(matches_pattern("a*b*c", "aXbYc"));
        assert!(!matches_pattern("abc", "abcd"));
    }

    #[test]
    fn what_is_saved_is_where_the_script_writes_it() {
        let out = format!("{WORK}/out/");
        let written = [RESULTS_FILE, SECCOMP_RESULTS_FILE, FAILED_FILE, THREADS_FILE];
        for ((from, name, _), file) in SAVED.iter().zip(written) {
            assert_eq!(*from, format!("{out}{file}"));
            assert_eq!(*name, file);
        }
        assert!(!Path::new(SCRIPT_LOG).starts_with(WORK), "the script removes {WORK} as it starts");
    }

    #[test]
    fn the_script_fills_every_placeholder() {
        let text = script();
        assert!(!text.contains('@'), "{text}");
        assert!(text.contains(&format!("work={WORK}\n")));
        assert!(text.contains(&format!(" {CASE_SECONDS}\n")));
        assert!(text.contains(&format!("{SECCOMP_FILE}\" {SECCOMP_SEED} {SECCOMP_FILTERS} >")));
    }

    #[test]
    fn the_script_runs_the_cases_and_seccomp_diff_and_collects_the_logs_of_failures() {
        let dir = std::env::temp_dir().join(format!("linux-suite-script-{}", std::process::id()));
        let (volume, work, bin) = (dir.join("volume"), dir.join("work"), dir.join("bin"));
        std::fs::create_dir_all(volume.join(LTP_ON_DISK).join("testcases/bin")).unwrap();
        std::fs::create_dir_all(&bin).unwrap();
        let executable = |path: &Path, text: &str| {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::write(path, text).unwrap();
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
        };
        // A linux that runs its program, and a runner and a differential
        // that say what they were given.
        executable(&bin.join("linux"), "#!/bin/sh\nshift; [ \"$1\" = -- ] && shift; exec \"$@\"\n");
        executable(
            &volume.join(RUNNER_FILE),
            "#!/bin/sh\nprintf 'good 0 5\\nbad 1 7\\n' > \"$3/results.txt\"\nmkdir -p \"$3/logs\"\n\
             echo 'bad said why' > \"$3/logs/bad.log\"\necho \"runner $*\"\n",
        );
        executable(&volume.join(SECCOMP_FILE), "#!/bin/sh\necho \"seccomp $*\"\n");
        std::fs::write(volume.join(LTP_ON_DISK).join(CASES_FILE), "good good\nbad bad\n").unwrap();
        std::fs::write(volume.join(LTP_ON_DISK).join("testcases/bin/good"), "").unwrap();
        let path = format!("{}:{}", bin.display(), std::env::var("PATH").unwrap());
        let ran = std::process::Command::new("sh")
            .arg("-c")
            .arg(script_for(&volume.display().to_string(), &work.display().to_string()))
            .env("PATH", path)
            .output()
            .unwrap();
        let said = String::from_utf8_lossy(&ran.stdout).into_owned();
        assert!(ran.status.success(), "{}", String::from_utf8_lossy(&ran.stderr));
        assert!(
            said.contains("suite-run status 0\n") && said.ends_with("linux-suite: done\n"),
            "{said}"
        );
        let out = work.join("out");
        let read = |name: &str| std::fs::read_to_string(out.join(name)).unwrap();
        assert_eq!(
            read(SECCOMP_RESULTS_FILE),
            format!("seccomp {SECCOMP_SEED} {SECCOMP_FILTERS}\n")
        );
        assert_eq!(read(FAILED_FILE), "== bad\nbad said why\n");
        assert!(out.join(THREADS_FILE).is_file());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_dynamic_program_names_the_libraries_it_loads() {
        let me = std::fs::read(std::env::current_exe().unwrap()).unwrap();
        let needed = needed_libraries(&me);
        assert!(needed.iter().any(|library| library == "libc.so.6"), "{needed:?}");
        assert!(needed_libraries(b"#!/bin/sh\n").is_empty());
    }

    #[test]
    fn the_pinned_release_is_a_whole_commit() {
        assert_eq!(LTP_COMMIT.len(), 40);
        assert!(LTP_COMMIT.chars().all(|c| c.is_ascii_hexdigit()));
    }
}
