// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Go, the catalog's Go toolchain, runs on Dukovo. Dukovo's root is on a
//! disk grown to 3 GiB, so that the bundle and the build cache have room,
//! and a second disk holds the `.dkv` file `ob ports` packed for Go, a
//! module of userspace/go and two files for gofmt. Alice installs the
//! bundle for herself, in her own applications folder; go asks, with a
//! warning, whether it may run with her access, then says it is go1.27.4
//! on radix/amd64, and its environment names the bundle as GOROOT with cgo
//! off and no other toolchain taken. gofmt lists a file that is not
//! formatted, rewrites it as the formatted file on the disk is, and then
//! lists nothing.
//!
//! The module, a program that greets with goroutines and writes JSON and
//! has a test, is built, run, vetted and tested with the bundle's go. Its
//! packages and the standard library's they need are compiled for the first
//! time here, which takes long under emulation, and a second build is
//! served from the build cache and compiles nothing. Go's own tests of the
//! runtime and the library are the go-runtime test's.

use super::super::*;

/// The disk of bundles' label.
const LABEL: &str = "BUNDLES";

/// Where Spark mounts it: the second virtio disk's name.
const VOLUME: &str = "/volumes/vdb";

/// Go's bundle file.
pub(in crate::test) const FILE: &str = "dev.go.go-1.27.4.dkv";

/// The start of the name of the bundle file on the disk: the only bundle it
/// holds, and the one the disks of the other bundles leave out.
pub(in crate::test) const BUNDLE_PREFIX: &str = "dev.go.go-";

/// Where the repository keeps the module that the test builds, and the two
/// files for gofmt, and the names they have on the disk. The files in the
/// repository end in `.txt`, so that Go's tools, which build and vet every
/// `.go` file of a module, never read a program that is not formatted.
const MODULE: &str = "userspace/go/hello-module";
const BAD: &str = "userspace/go/gofmt/bad.go.txt";
const GOOD: &str = "userspace/go/gofmt/good.go.txt";

// The waits of this test, in seconds, are guesses and not measurements: no
// run of the test under KVM exists yet. Each is generous, a large multiple
// of what the step takes on a desktop, so that a slow emulated run still
// passes, and CI's timings will tune them.

/// Unpacking the bundle, about 280 MiB, from the FAT disk into the root.
const INSTALL_SECONDS: u64 = 900;
/// The first run of go and of gofmt: a program of tens of MiB read in.
const FIRST_RUN_SECONDS: u64 = 120;
/// The first build, which compiles each standard package the program needs.
const BUILD_SECONDS: u64 = 900;
/// `go vet`, which compiles the packages for the vet tool's type checking
/// and runs the tool; the packages are in the cache by then.
const VET_SECONDS: u64 = 600;
/// `go test`, which compiles the package's tests, links their binary and
/// runs it.
const TEST_SECONDS: u64 = 900;
/// `go run`, which links a program and starts it.
const RUN_SECONDS: u64 = 300;
/// A build that finds everything in the cache: go still hashes the files.
const CACHED_SECONDS: u64 = 120;
/// What a step of a few files takes, such as gofmt on one: the program's
/// start.
const QUICK_SECONDS: u64 = 60;

/// The room after the root the image makes, which is 512 MiB: with it the
/// root has 3 GiB.
const ROOM_KIB: u64 = 2560 * 1024;

/// Dukovo's root on a copy of the test image with room after it, and the
/// bundle, with the module and the files for gofmt.
const DISKS: &[Disk] = &[
    Disk {
        label: image::ROOT_LABEL,
        kib: ROOM_KIB,
        ..btrfs_disk(Bus::Virtio, 512, ROOT_ROOM, root_checked)
    },
    Disk { fat: GO_TOOLCHAIN_FILES, ..bundle_files_disk(192 * 1024, LABEL) },
];

/// The test.
pub const CASE: Case = Case {
    name: "go-toolchain",
    about: "Go, the catalog's Go toolchain, runs: on a root grown to 3 GiB, installed for one person from its .dkv file, go asks whether it may run with her access and says it is go1.27.4 on radix/amd64, its environment has the bundle as GOROOT, cgo off and GOTOOLCHAIN local; gofmt lists a file that is not formatted, rewrites it as the formatted file is and then lists nothing; a module of a program with goroutines, JSON and a test is built, run, vetted and tested, and a second go build is served from the build cache and compiles nothing",
    firmware: Firmware::Uefi,
    media: Media::Usb,
    cmdline: "lockdep",
    cpus: 4,
    memory: "2G",
    reboot: false,
    image: TestImage::Plain,
    cpu_model: None,
    qemu: &[],
    script: go_toolchain,
    disks: DISKS,
    second: None,
};

/// Makes the disk of bundles: Go's, the module in a folder of its own name
/// and the two files for gofmt, `bad.go` and `good.go`.
pub fn make_disk(ctx: &Ctx, disk: &Disk, path: &Path) -> Result<(), String> {
    make_bundle_disk(ctx, disk, path, |name| name.starts_with(BUNDLE_PREFIX))?;
    let image = path.display().to_string();
    let mut module = std::process::Command::new("mcopy");
    module.args(["-s", "-i", &image]).arg(ctx.root.join(MODULE)).arg("::/");
    host_tool(&mut module)?;
    for (source, name) in [(BAD, "bad.go"), (GOOD, "good.go")] {
        let mut copy = std::process::Command::new("mcopy");
        copy.args(["-i", &image]).arg(ctx.root.join(source)).arg(format!("::/{name}"));
        host_tool(&mut copy)?;
    }
    Ok(())
}

/// Runs `command`, whose output goes to the file `/tmp/<name>`, and waits
/// up to `seconds` for it to end, then for the screen to show `name` and
/// the status 0. A command that fails shows the end of its file first, so
/// that the test's failure says why.
pub(in crate::test) fn succeeds(vm: &mut Vm, name: &str, command: &str, seconds: u64) -> Result<(), String> {
    run_within(vm, &format!("clear; {}", logged(name, command)), seconds)?;
    vm.expect_screen(&[&format!("{name} 0")])
}

/// `command` with its output in `/tmp/<name>`, the end of that shown if it
/// fails, and then `name` and its status.
fn logged(name: &str, command: &str) -> String {
    format!(
        "{command} > /tmp/{name} 2>&1; status=$?; [ $status -eq 0 ] || tail -n 12 /tmp/{name}; \
         echo \"{name} $status\""
    )
}

fn go_toolchain(vm: &mut Vm, case: &Case) -> Result<(), String> {
    owner_signs_in(vm, case)?;
    vm.expect_anywhere("bundled: 0 bundles installed; answering at /system/live/bundled/socket")?;
    vm.expect_anywhere(&format!("spark: mounted the FAT filesystem of vdb at {VOLUME}"))?;
    // The root grows over the disk's room, in use: its partition, then its
    // Btrfs, named by a folder on it.
    unlock(vm)?;
    answer_prompt(vm, "$", "clear; elevate rdisk --grow /devices/vda")?;
    vm.expect_screen(&["/devices/vda3 grows from 512 MiB to 3 GiB."])?;
    answer_prompt(vm, "$", "clear; elevate device resize max /system/settings")?;
    vm.expect_screen(&["Resized device id 1 of '/system/settings' to "])?;

    // For herself: in ~/applications, not /applications. Its log goes to a
    // file, as the lines it prints say little and the step takes long.
    succeeds(
        vm,
        "install",
        &format!("cd {VOLUME}; bundle install --yes {FILE}"),
        INSTALL_SECONDS,
    )?;
    succeeds(vm, "applications", "cd; ls -d ~/applications/dev.go.go", QUICK_SECONDS)?;

    // go runs with the access of whoever runs it: its first run asks, with
    // a warning, and is allowed always.
    answer_prompt(vm, "$", &format!("clear; {}", logged("version", "go version")))?;
    vm.expect_screen(&["[warning] ", "A: always"])?;
    vm.type_text("a")?;
    // The shell's prompt is back when go has ended.
    answer_prompt_within(vm, "$", "clear; cat /tmp/version", FIRST_RUN_SECONDS)?;
    vm.expect_screen(&["go version go1.27.4 radix/amd64"])?;

    // The bundle is its own root, nothing downloads a toolchain, and C code
    // is not built, as the system has no C compiler.
    succeeds(vm, "env", "go env GOROOT GOOS GOARCH CGO_ENABLED GOTOOLCHAIN", QUICK_SECONDS)?;
    answer_prompt(vm, "$", "clear; echo environment $(cat /tmp/env)")?;
    vm.expect_screen(&["environment /users/alice/applications/dev.go.go radix amd64 0 local"])?;

    // gofmt, a command of its own, asks on its first run too. It lists the
    // file that is not formatted, rewrites it, and then lists nothing; the
    // file is then the one the disk holds formatted.
    run_within(vm, &format!("clear; mkdir ~/fmt; cp {VOLUME}/bad.go {VOLUME}/good.go ~/fmt"), QUICK_SECONDS)?;
    let list = format!("clear; cd ~/fmt; {}", logged("before", "gofmt -l ."));
    run_answering_until(vm, &list, "before 0", FIRST_RUN_SECONDS)?;
    answer_prompt(vm, "$", "clear; echo \"before: [$(cat /tmp/before)]\"")?;
    vm.expect_screen(&["before: [bad.go]"])?;
    succeeds(vm, "rewrite", "gofmt -w bad.go", QUICK_SECONDS)?;
    succeeds(vm, "after", "gofmt -l .", QUICK_SECONDS)?;
    answer_prompt(vm, "$", "clear; echo \"after: [$(cat /tmp/after)]\"")?;
    vm.expect_screen(&["after: []"])?;
    succeeds(vm, "same", "cmp bad.go good.go", QUICK_SECONDS)?;

    // The module: built, run, vetted and tested. The first build compiles
    // the standard packages it needs.
    succeeds(vm, "copy", &format!("cp -R {VOLUME}/hello-module ~/hello; cd ~/hello"), QUICK_SECONDS)?;
    succeeds(vm, "build", "go build -o hello .", BUILD_SECONDS)?;
    succeeds(vm, "hello", "./hello", RUN_SECONDS)?;
    answer_prompt(vm, "$", "clear; cat /tmp/hello")?;
    vm.expect_screen(&[
        "Hello, Alice!",
        "Hello, Bob!",
        r#"[{"name":"alice","line":"Hello, Alice!"},{"name":"bob","line":"Hello, Bob!"}]"#,
    ])?;
    succeeds(vm, "vet", "go vet ./...", VET_SECONDS)?;
    succeeds(vm, "test", "go test ./...", TEST_SECONDS)?;
    answer_prompt(vm, "$", "clear; echo \"passed: $(grep -c '^ok' /tmp/test)\"")?;
    vm.expect_screen(&["passed: 1"])?;
    succeeds(vm, "run", "go run . x", RUN_SECONDS)?;
    answer_prompt(vm, "$", "clear; cat /tmp/run")?;
    vm.expect_screen(&["Hello, X!", r#"[{"name":"x","line":"Hello, X!"}]"#])?;

    // A second build finds the program and everything under it in the
    // build cache, and starts no compile. The count is inside the echo, as
    // grep exits 1 when it counts nothing.
    succeeds(vm, "again", "go build -x -o hello .", CACHED_SECONDS)?;
    answer_prompt(vm, "$", "clear; echo \"compiles: $(grep -c /compile /tmp/again)\"")?;
    vm.expect_screen(&["compiles: 0"])?;

    vm.press_power_button()?;
    vm.expect("spark: every service has stopped")?;
    vm.expect("Power: `spark` asked to power off the machine; syncing the filesystems")?;
    vm.expect_exit(QEMU_EXIT_POWER_OFF)
}
