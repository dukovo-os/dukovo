// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! `ob test go-suite`: Go's own tests of its standard library, run on
//! Dukovo with the `go` command of the catalog's Go bundle.
//!
//! The `go-runtime` boot test runs the test binaries of 28 packages that ob
//! builds. This run is the whole of it: `go test -short` of every package
//! of `std` that has tests, as `go` itself compiles, links and runs them on
//! Radix: 258 packages. Its machine is larger than a boot test's, and few
//! changes can break it, so it is not one of the boot tests: it is a run of
//! its own, as `ob soak` is, on the same machine harness, and CI runs it in
//! `.github/workflows/go-suite.yml` on pull requests that touch Go and when
//! someone asks. Its first run in CI took 553 seconds under KVM, the boot
//! and the bundle's install included.
//!
//! The machine has 4 CPUs and 6 GiB of memory, as Go's linker builds each
//! test binary in memory and `go test` links up to four at once, and a root
//! grown to 12 GiB for the bundle and the build cache. Before the boot ob
//! asks the toolchain's `go` for the packages with tests and writes them to
//! the disk of the bundle as `packages.lst`, with `skips.lst`, the tests of
//! them that `userspace/go/skips.txt` names ([`golang::Known::Suite`]), and
//! `go-suite.sh` ([`SCRIPT`]). alice installs the bundle and runs the
//! script. The packages that have no skip are tested in one call of `go
//! test`, which runs them in parallel; each package that has skips is tested
//! in a call of its own, as a `-skip` pattern applies to every package of a
//! call. The script then says how many packages there were and how many
//! failed, and for each that failed, the first lines that show why.
//!
//! The run passes when no package failed. A package passes when `go test`
//! says `ok` for it: one it says nothing of, because it did not get so far,
//! failed. The script's whole log is copied to the disk, which Dukovo's
//! root writes through `elevate`, and ob saves it as
//! `out/test/go-suite.log` once the machine is off, whether the run passed
//! or not; the serial log, `out/test/boot-go-suite.log`, has the boot and
//! the kernel's own words. A script that outlasts its wait is stopped, and
//! the log of what it ran so far is saved all the same.

use super::*;
use crate::golang;

/// The disk's label.
const LABEL: &str = "GOSUITE";

/// Where Spark mounts the disk: the second virtio disk's name.
const VOLUME: &str = "/volumes/vdb";

/// The disk's file of the packages to test, one an import path a line.
const PACKAGES: &str = "packages.lst";
/// The disk's file of the skips: a line of a package and its `-skip`
/// pattern for each package that has one.
const SKIPS: &str = "skips.lst";
/// The disk's file of the script, and of the log the script's own is copied
/// to.
const SCRIPT_FILE: &str = "go-suite.sh";
const LOG_FILE: &str = "go-suite.log";

/// Where the host keeps the log, in the folder of the boot tests' logs.
const LOG: &str = "go-suite.log";

/// Where the script keeps its log and its other files: a folder of alice's
/// home, which root reads through `elevate` to save the log. Each account
/// has a `/tmp` of its own (ADR 0255), so root's is not alice's.
const SCRATCH: &str = "/users/alice/go-suite";

/// What a run is limited to if the person who starts it names no other, in
/// seconds: the sum of the waits below. Each is generous, several times what
/// the step took in CI under KVM, where the whole run took 553 seconds, so
/// that a slower machine, or one without KVM, still ends with a verdict.
const TOTAL_SECONDS: u64 =
    BOOT_SECONDS + INSTALL_SECONDS + FIRST_RUN_SECONDS + SUITE_SECONDS + SAVE_SECONDS;

// Under the 100 minutes go-suite.yml gives the step, which builds the image
// and the disk before the machine starts.
const _: () = assert!(TOTAL_SECONDS < 80 * 60);

/// The boot, the owner's account and the root grown over its room.
const BOOT_SECONDS: u64 = 600;
/// Unpacking the bundle, about 280 MiB, into the root.
const INSTALL_SECONDS: u64 = 900;
/// The first run of go: a program of tens of MiB read in.
const FIRST_RUN_SECONDS: u64 = 120;
/// The script: the packages' tests, which compile and link a few hundred
/// test binaries, mostly four at a time, in under 8 minutes in CI.
const SUITE_SECONDS: u64 = 45 * 60;
/// Copying the log to the disk and the machine's power off.
const SAVE_SECONDS: u64 = 300;

/// The room after the root the image makes, which is 512 MiB: with it the
/// root has 12 GiB.
const ROOM_KIB: u64 = 11_776 * 1024;

/// The longest a test binary may run, as `go test -timeout` takes it: a
/// hung test is ended with a dump of its goroutines instead of holding the
/// run for [`SUITE_SECONDS`].
const PACKAGE_TIMEOUT: &str = "15m";

/// The script, `sh go-suite.sh`, as `@VOLUME@`, where the disk is, and
/// `@SCRATCH@`, where it keeps its files, are filled in. It prints a line,
/// `go-suite: FAIL PACKAGE`, and the first lines of the package's test
/// output that say what failed (a failed test, a panic or a fatal error), for
/// each package that has no `ok` line in what `go test` printed, and last
/// `go-suite: N packages, M failed`, so that it is the line left on the
/// screen. The same lines end the log.
const SCRIPT: &str = r#"#!/bin/sh
volume=@VOLUME@
scratch=@SCRATCH@
log=$scratch/go-suite.log
failed=$scratch/go-suite.failed
mkdir -p "$scratch"
: > "$log"
# A -skip applies to every package of a call, so a package that has skips has
# a call of its own, and the others share one, which go test runs in parallel.
plain=$(awk 'FILENAME == ARGV[1] { skipped[$1] = 1; next } !($1 in skipped) { print $1 }' \
	"$volume/skips.lst" "$volume/packages.lst")
go test -short -timeout @TIMEOUT@ $plain >> "$log" 2>&1
while read -r package skip; do
	go test -short -timeout @TIMEOUT@ -run=. "-skip=$skip" "$package" >> "$log" 2>&1
done < "$volume/skips.lst"
awk 'FILENAME == ARGV[1] { if ($1 == "ok") passed[$2] = 1; next } !($1 in passed) { print $1 }' \
	"$log" "$volume/packages.lst" > "$failed"
{
	while read -r package; do
		echo "go-suite: FAIL $package"
		awk -v package="$package" '
			/^(ok|FAIL|\?)[ \t]/ {
				if ($2 == package) {
					for (i = 1; i <= n && shown < 6; i++) {
						if (lines[i] ~ /^(--- FAIL:|panic:|fatal error:)/) {
							print "    " lines[i]
							shown++
						}
					}
				}
				n = 0
				next
			}
			{ lines[++n] = $0 }' "$log"
	done < "$failed"
	total=$(awk 'END { print NR }' "$volume/packages.lst")
	count=$(awk 'END { print NR }' "$failed")
	echo "go-suite: $total packages, $count failed"
} > "$scratch/go-suite.summary"
cat "$scratch/go-suite.summary"
cat "$scratch/go-suite.summary" >> "$log"
"#;

/// The script with the disk where Spark mounts it and its files in
/// [`SCRATCH`].
fn script() -> String {
    script_for(VOLUME, SCRATCH)
}

/// [`SCRIPT`] for a disk at `volume` and files in `scratch`.
fn script_for(volume: &str, scratch: &str) -> String {
    SCRIPT
        .replace("@VOLUME@", volume)
        .replace("@SCRATCH@", scratch)
        .replace("@TIMEOUT@", PACKAGE_TIMEOUT)
}

/// The disks: Dukovo's root on a copy of the test image with room after it,
/// and the bundle with the files of this run.
const DISKS: &[Disk] = &[
    Disk {
        label: image::ROOT_LABEL,
        kib: ROOM_KIB,
        ..btrfs_disk(Bus::Virtio, 512, ROOT_ROOM, root_checked)
    },
    Disk { fat: GO_SUITE_FILES, ..bundle_files_disk(192 * 1024, LABEL) },
];

/// The machine.
const GO_SUITE: Case = Case {
    name: "go-suite",
    about: "Go's own tests of its standard library, run on Dukovo with the go command of the Go bundle",
    firmware: Firmware::Uefi,
    media: Media::Usb,
    cmdline: "",
    cpus: 4,
    memory: "6G",
    reboot: false,
    image: TestImage::Plain,
    cpu_model: None,
    qemu: &[],
    script: go_suite,
    disks: DISKS,
    second: None,
};

/// `ob test go-suite`: builds what the machine needs, boots it and runs the
/// suite. `limit` is how long the machine may run, or the sum of the
/// waits ([`TOTAL_SECONDS`]) if nothing is given.
pub fn command(ctx: &Ctx, profile: crate::config::Profile, limit: Option<Duration>) -> Result<()> {
    // The file is checked before the ports and the image are built, which
    // take long: a line that is no skip fails here. Its packages are
    // checked against the ones `go list` names when the disk is made.
    golang::read_skips(ctx, &golang::Known::Shape)?;
    let limit = limit.unwrap_or(Duration::from_secs(TOTAL_SECONDS));
    step(&format!(
        "Running Go's own tests of its standard library on Dukovo, for up to {} minutes",
        limit.as_secs() / 60
    ));
    boot_tests_of(ctx, profile, &[&GO_SUITE], limit, false)
}

/// Makes the disk: the one of the `go-toolchain` test, with the bundle,
/// and the list of packages, their skips and the script.
pub fn make_disk(ctx: &Ctx, disk: &Disk, path: &Path) -> Result<(), String> {
    cases::go_toolchain::make_disk(ctx, disk, path)?;
    let toolchain = crate::ports::find_toolchain(ctx).map_err(|err| err.to_string())?;
    let packages = golang::std_test_packages(ctx, &toolchain).map_err(|err| err.to_string())?;
    let skips =
        golang::read_skips(ctx, &golang::Known::Suite(&packages)).map_err(|err| err.to_string())?;
    let image = path.display().to_string();
    let files = [
        (PACKAGES, format!("{}\n", packages.join("\n"))),
        (SKIPS, golang::suite_skip_list(&skips, &packages)),
        (SCRIPT_FILE, script()),
    ];
    for (name, text) in files {
        let source = path.with_extension(name);
        std::fs::write(&source, text)
            .map_err(|err| format!("writing {}: {err}", source.display()))?;
        let copied = host_tool(
            std::process::Command::new("mcopy")
                .args(["-i", &image])
                .arg(&source)
                .arg(format!("::/{name}")),
        );
        let _ = std::fs::remove_file(&source);
        copied?;
    }
    Ok(())
}

/// What the script said of the run: how many packages it tested, and which
/// failed.
#[derive(Debug, PartialEq, Eq)]
struct Verdict {
    packages: usize,
    failed: Vec<String>,
}

impl Verdict {
    /// Reads the lines the script ends its log with. `None` if there is
    /// no line of the count, as the script did not get so far.
    fn parse(log: &str) -> Option<Verdict> {
        let counted = log.lines().rev().find_map(|line| {
            let counts = line.trim().strip_prefix("go-suite: ")?;
            let (packages, failed) = counts.split_once(" packages, ")?;
            let failed = failed.strip_suffix(" failed")?;
            Some((packages.parse::<usize>().ok()?, failed.parse::<usize>().ok()?))
        })?;
        let failed: Vec<String> = log
            .lines()
            .filter_map(|line| line.trim().strip_prefix("go-suite: FAIL "))
            .map(str::to_string)
            .collect();
        (failed.len() == counted.1).then_some(Verdict { packages: counted.0, failed })
    }

    /// Why the run did not pass, if it did not.
    fn fault(&self) -> Option<String> {
        (!self.failed.is_empty()).then(|| {
            format!(
                "the tests of {} of {} packages did not pass: {}",
                self.failed.len(),
                self.packages,
                self.failed.join(", ")
            )
        })
    }
}

fn go_suite(vm: &mut Vm, case: &Case) -> Result<(), String> {
    owner_signs_in(vm, case)?;
    vm.expect_anywhere("bundled: 0 bundles installed; answering at /system/live/bundled/socket")?;
    vm.expect_anywhere(&format!("spark: mounted the FAT filesystem of vdb at {VOLUME}"))?;
    unlock(vm)?;
    answer_prompt(vm, "$", "clear; elevate rdisk --grow /devices/vda")?;
    vm.expect_screen(&["/devices/vda3 grows from 512 MiB to 12 GiB."])?;
    answer_prompt(vm, "$", "clear; elevate device resize max /system/settings")?;
    vm.expect_screen(&["Resized device id 1 of '/system/settings' to "])?;

    let file = cases::go_toolchain::FILE;
    cases::go_toolchain::succeeds(
        vm,
        "install",
        &format!("cd {VOLUME}; bundle install --yes {file}"),
        INSTALL_SECONDS,
    )?;
    // go runs with the access of whoever runs it: its first run asks, with
    // a warning, and is allowed always.
    run_answering_until(vm, "clear; go version", "go version go1.27.4", FIRST_RUN_SECONDS)?;

    // A run that outlasts its wait is stopped, and what it logged so far is
    // saved all the same, so that the log shows where it was.
    let ran = run_within(vm, &format!("clear; sh {VOLUME}/{SCRIPT_FILE}"), SUITE_SECONDS);
    if ran.is_err() {
        vm.press(&["ctrl-c"])?;
    }

    // The root belongs to root, and so does the disk: the log goes there
    // through elevate, unlocked again as the suite took longer than it
    // stays unlocked.
    unlock(vm)?;
    cases::go_toolchain::succeeds(
        vm,
        "save",
        &format!("elevate cp {SCRATCH}/{LOG_FILE} {VOLUME}/{LOG_FILE}"),
        SAVE_SECONDS,
    )?;
    vm.press_power_button()?;
    vm.expect("spark: every service has stopped")?;
    vm.expect("Power: `spark` asked to power off the machine; syncing the filesystems")?;
    vm.expect_exit(QEMU_EXIT_POWER_OFF)?;

    let image = vm.dir().join(format!("{}-disk1.img", case.name)).display().to_string();
    let log = host_tool(std::process::Command::new("mtype").args([
        "-i",
        &image,
        &format!("::/{LOG_FILE}"),
    ]))?;
    let saved = vm.dir().join(LOG);
    std::fs::write(&saved, &log).map_err(|err| format!("writing {}: {err}", saved.display()))?;
    ran.map_err(|err| {
        format!("the suite did not end in time ({err}); {} has its log", saved.display())
    })?;
    let verdict = Verdict::parse(&String::from_utf8_lossy(&log))
        .ok_or_else(|| format!("{} has no count of the packages tested", saved.display()))?;
    verdict.fault().map_or(Ok(()), Err)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A `go` that prints what `go test` prints for each package it is
    /// given: `ok` for all but three, one that fails a test and one that
    /// says nothing; and keeps its arguments in `$CALLS`.
    const FAKE_GO: &str = r#"#!/bin/sh
echo "$*" >> "$CALLS"
for arg in "$@"; do
	case $arg in
	bad/one) printf -- '--- FAIL: TestOne (0.00s)\n    one_test.go:3: boom\nFAIL\nFAIL\tbad/one\t0.010s\n' ;;
	lost/three) ;;
	-*|test|15m) ;;
	*) printf 'ok  \t%s\t0.010s\n' "$arg" ;;
	esac
done
"#;

    #[test]
    fn the_script_runs_go_test_short_for_every_package_and_skips_in_calls_of_their_own() {
        let text = script();
        assert!(text.contains("volume=/volumes/vdb\n"), "{text}");
        assert!(text.contains(&format!("scratch={SCRATCH}\n")), "{text}");
        assert!(text.contains(&format!("go test -short -timeout {PACKAGE_TIMEOUT} $plain")));
        assert!(text.contains(&format!("-timeout {PACKAGE_TIMEOUT} -run=. \"-skip=$skip\"")));
        assert!(text.contains("echo \"go-suite: $total packages, $count failed\""));
        for name in [PACKAGES, SKIPS] {
            assert!(text.contains(&format!("$volume/{name}")), "{name}");
        }
        assert!(!text.contains('@'), "a placeholder is left in the script");
    }

    #[test]
    fn the_script_counts_and_names_the_packages_that_did_not_pass() {
        let dir = std::env::temp_dir().join(format!("go-suite-script-{}", std::process::id()));
        let (volume, bin) = (dir.join("volume"), dir.join("bin"));
        std::fs::create_dir_all(&volume).unwrap();
        std::fs::create_dir_all(&bin).unwrap();
        let go = bin.join("go");
        std::fs::write(&go, FAKE_GO).unwrap();
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(&go, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        std::fs::write(volume.join(PACKAGES), "bad/one\nlost/three\nos\nos/exec\nstrings\n")
            .unwrap();
        std::fs::write(volume.join(SKIPS), "os ^TestA$|^TestB$\nos/exec ^TestC$\n").unwrap();
        let path = format!("{}:{}", bin.display(), std::env::var("PATH").unwrap());
        let ran = std::process::Command::new("sh")
            .arg("-c")
            .arg(script_for(&volume.display().to_string(), &dir.display().to_string()))
            .env("PATH", path)
            .env("CALLS", dir.join("calls"))
            .output()
            .unwrap();
        assert!(ran.status.success(), "{}", String::from_utf8_lossy(&ran.stderr));

        // The packages without skips share a call, and each of the others
        // has its own, with the run of every test and the skip pattern.
        let calls = std::fs::read_to_string(dir.join("calls")).unwrap();
        assert_eq!(
            calls.lines().collect::<Vec<_>>(),
            [
                "test -short -timeout 15m bad/one lost/three strings",
                "test -short -timeout 15m -run=. -skip=^TestA$|^TestB$ os",
                "test -short -timeout 15m -run=. -skip=^TestC$ os/exec",
            ]
        );
        let said = String::from_utf8_lossy(&ran.stdout).into_owned();
        assert_eq!(
            said,
            "go-suite: FAIL bad/one\n    --- FAIL: TestOne (0.00s)\n\
             go-suite: FAIL lost/three\n\
             go-suite: 5 packages, 2 failed\n"
        );
        let log = std::fs::read_to_string(dir.join("go-suite.log")).unwrap();
        assert!(log.contains("ok  \tos\t0.010s\n"), "{log}");
        assert!(log.ends_with(&said), "{log}");
        assert_eq!(
            Verdict::parse(&log),
            Some(Verdict { packages: 5, failed: vec!["bad/one".into(), "lost/three".into()] })
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_run_passes_when_the_count_says_no_package_failed() {
        let log = "ok  \tos\t0.1s\ngo-suite: 340 packages, 0 failed\n";
        let verdict = Verdict::parse(log).unwrap();
        assert_eq!(verdict, Verdict { packages: 340, failed: vec![] });
        assert_eq!(verdict.fault(), None);

        let log = "go-suite: FAIL net\n    --- FAIL: TestDial (0.1s)\ngo-suite: FAIL os\n\
                   go-suite: 340 packages, 2 failed\n";
        let verdict = Verdict::parse(log).unwrap();
        assert_eq!(
            verdict.fault().unwrap(),
            "the tests of 2 of 340 packages did not pass: net, os"
        );
    }

    #[test]
    fn a_log_without_a_whole_count_is_no_verdict() {
        assert_eq!(Verdict::parse(""), None);
        assert_eq!(Verdict::parse("ok  \tos\t0.1s\n"), None);
        assert_eq!(Verdict::parse("go-suite: 340 packages, many failed\n"), None);
        // A count that says 1 failed, with no package named, is not taken
        // for a pass or a fail.
        assert_eq!(Verdict::parse("go-suite: 340 packages, 1 failed\n"), None);
    }
}
