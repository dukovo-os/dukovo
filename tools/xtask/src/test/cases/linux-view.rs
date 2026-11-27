// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! A dynamically linked Linux program runs in the sandbox that linux(1)
//! builds for it, on the libraries of the Linux base bundle (ADRs 0618 to
//! 0622). ob builds userspace/linux/view.c on the build machine, as a Linux
//! machine would, against that machine's glibc, and a disk holds the program
//! beside the bundle's .dkv file, which ob packed from the port
//! ports/linux-base. Dukovo's root is on a disk, so that the bundle has room.
//! alice, the owner, installs the bundle for herself and runs the program
//! from Nukesh, by its path, from the folder of the disk: Nukesh sees a
//! dynamic program of Linux's and starts it through linux run, which gives it
//! /usr from the bundle, a /proc and a /sys that linuxd serves, and its own
//! folders of alice's home, and nothing else of it.
//!
//! The program makes the calls itself and says one line for a case that went
//! wrong and, at the end, that all 14 cases passed on Dukovo: the maps, the
//! CPU list and the status of /proc, the path /proc/self/exe gives, a write
//! through /proc/self/fd/1 that reaches standard output, the interfaces that
//! getifaddrs lists, the accounts of /etc, the resolver's file, the names
//! uname gives, the list of CPUs under /sys, the person's home kept out of
//! sight, personality(0xffffffff), which only linuxd answers, a file of
//! /dev/shm that a child and its parent map and share, and /proc/self/fd
//! listed through its own descriptor, which linuxd lists from the kernel's
//! list of the process's files. The same file runs on a Linux machine,
//! which is how its expectations are known to be Linux's.
//!
//! It then runs again with --restart: it ends linuxd with SIGKILL, and the
//! supervisor that linux run leaves beside it starts linuxd again, which the
//! program sees by a call that linuxd answers.

use super::super::*;

/// The disk of the bundle and the program: its label.
const LABEL: &str = "BUNDLES";

/// Where Spark mounts it: the second virtio disk's name.
const VOLUME: &str = "/volumes/vdb";

/// The Linux base's bundle file, which ob packs from ports/linux-base.
pub(in crate::test) const FILE: &str = "org.dukovo.linux-base-25.8.0.dkv";

/// The start of its name: the one bundle the disk holds, and the one the
/// other tests' disks of bundles leave out, as they have no use for it.
pub(in crate::test) const BUNDLE_PREFIX: &str = "org.dukovo.linux-base-";

/// The program's name on the disk.
const PROGRAM: &str = "view";

/// Where its source is, from the repository's root.
const SOURCE: &str = "userspace/linux/view.c";

/// How many cases the program has without `--restart`: the table of the
/// source, which a unit test counts. The restart adds one.
const CASES: usize = 14;

/// How long a run of the program may take, in seconds: the program ends a
/// case that waits for linuxd after 30, and starting the sandbox, its
/// supervisor and linuxd takes a while under emulation.
const RUN_SECONDS: u64 = 180;

/// Dukovo's root on a copy of the test image, and the bundle and the
/// program.
const DISKS: &[Disk] = &[
    Disk { label: image::ROOT_LABEL, ..btrfs_disk(Bus::Virtio, 512, ROOT, root_checked) },
    linux_view_disk(64 * 1024, LABEL),
];

/// The test.
pub const CASE: Case = Case {
    name: "linux-view",
    about: "a dynamically linked Linux program runs in the sandbox of linux(1) on the Linux base bundle (ADRs 0618 to 0622): the bundle, packed from ports/linux-base, installs from its .dkv file; the program view, built from userspace/linux/view.c with the build machine's glibc, runs by its path from Nukesh, which starts it through linux run; it reads /proc/self/maps, /proc/cpuinfo and /proc/self/status, finds its own path in /proc/self/exe, writes through /proc/self/fd/1, lists the interface lo with getifaddrs, finds its account and the resolver's file in /etc, sees the kernel as Linux, reads the CPU list of /sys, cannot see alice's home outside its own folders, has personality(0xffffffff) answered by linuxd, shares a file of /dev/shm with a child, and lists /proc/self/fd through its own descriptor; and with --restart it ends linuxd and the call is answered again once the supervisor has started it anew",
    firmware: Firmware::Uefi,
    media: Media::Usb,
    // As a normal boot: Spark starts the services, bundled among them.
    cmdline: "lockdep",
    cpus: 2,
    memory: "1G",
    reboot: false,
    image: TestImage::Plain,
    cpu_model: None,
    qemu: &[],
    script: linux_view,
    disks: DISKS,
    second: None,
};

/// Where the program is built, in the build output: a folder made anew for
/// each run.
fn build_folder(ctx: &Ctx) -> PathBuf {
    ctx.target.join("linux-view")
}

/// Builds the program with the build machine's C compiler and its glibc,
/// dynamically linked, as a program on a Linux machine is; returns its file.
/// Warnings are not errors here: the host's compiler is not the one the
/// source was kept clean with.
fn build(ctx: &Ctx) -> Result<PathBuf, String> {
    let out = build_folder(ctx);
    if out.exists() {
        std::fs::remove_dir_all(&out)
            .map_err(|err| format!("clearing {}: {err}", out.display()))?;
    }
    std::fs::create_dir_all(&out).map_err(|err| format!("making {}: {err}", out.display()))?;
    let program = out.join(PROGRAM);
    let mut cc = std::process::Command::new("cc");
    cc.args(["-std=c11", "-D_GNU_SOURCE", "-O2", "-Wall", "-Wextra", "-o"])
        .arg(&program)
        .arg(ctx.root.join(SOURCE));
    host_tool(&mut cc)?;
    Ok(program)
}

/// Makes the disk: FAT32 holding the Linux base's bundle and the program.
pub fn make_disk(ctx: &Ctx, disk: &Disk, path: &Path) -> Result<(), String> {
    make_bundle_disk(ctx, disk, path, |name| name.starts_with(BUNDLE_PREFIX))?;
    let program = build(ctx)?;
    let mut copy = std::process::Command::new("mcopy");
    copy.arg("-i").arg(path).arg(&program).arg(format!("::/{PROGRAM}"));
    host_tool(&mut copy).map(drop)
}

/// Runs the program from the disk with `arguments`, by its path, and waits
/// for the shell's marker. The lines of the cases that went well are left
/// out, so that the screen holds the rest whatever the number of cases.
fn run_program(vm: &mut Vm, arguments: &str) -> Result<(), String> {
    run_within(
        vm,
        &format!("clear; cd {VOLUME}; {VOLUME}/{PROGRAM} {arguments} | grep -v ' ok$'; cd"),
        RUN_SECONDS,
    )
}

fn linux_view(vm: &mut Vm, case: &Case) -> Result<(), String> {
    owner_signs_in(vm, case)?;
    vm.expect_anywhere("bundled: 0 bundles installed; answering at /system/live/bundled/socket")?;
    vm.expect_anywhere(&format!("spark: mounted the FAT filesystem of vdb at {VOLUME}"))?;
    // For herself, in ~/applications: the base is found by its ID where
    // bundle run finds the bundles of the person that runs the program.
    answer_prompt(
        vm,
        "$",
        &format!("clear; cd {VOLUME}; bundle install --yes {FILE} | tail -1; cd"),
    )?;
    answer_prompt_within(vm, "$", "clear; ls -d ~/applications/org.dukovo.linux-base*", 300)?;
    vm.expect_screen(&["/users/alice/applications/org.dukovo.linux-base"])?;

    // The program runs as it is: Nukesh finds that it is a dynamic program
    // of Linux's and runs it through linux run.
    run_program(vm, "")?;
    let all = format!("view: all {CASES} cases passed on Dukovo");
    vm.expect_screen(&["view: running on Dukovo", "view: wrote through /proc/self/fd/1", &all])?;

    // linuxd ends in the middle of the run, and the case that ended it sees
    // a call answered again.
    run_program(vm, "--restart")?;
    let all = format!("view: all {} cases passed on Dukovo", CASES + 1);
    vm.expect_screen(&["view: running on Dukovo", &all])?;

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

    fn source() -> String {
        std::fs::read_to_string(root().join(SOURCE)).expect(SOURCE)
    }

    #[test]
    fn the_case_count_is_the_rows_of_the_programs_table() {
        let source = source();
        let table =
            source.split("CASES[] = {").nth(1).expect("the table").split("};").next().unwrap();
        assert_eq!(table.lines().filter(|line| line.starts_with("\t{\"")).count(), CASES);
        // The restart is a case of its own, which the table does not hold.
        assert!(source.contains("RESTART = {\"restart\""));
    }

    #[test]
    fn the_program_says_the_lines_the_test_looks_for() {
        let source = source();
        for line in [
            "view: running on %s",
            "view: all %d cases passed on %s\\n",
            "view: wrote through /proc/self/fd/1",
        ] {
            assert!(source.contains(line), "{SOURCE} does not say `{line}`");
        }
        // Lines of the cases that went well end so, and the test leaves
        // them out of the screen.
        assert!(source.contains("view: %s ok"));
    }

    #[test]
    fn the_bundle_the_test_installs_is_the_one_the_recipe_packs() {
        let text = std::fs::read_to_string(root().join("ports/linux-base/recipe.toml")).unwrap();
        let recipe: crate::ports::Recipe = toml::from_str(&text).unwrap();
        let id = &recipe.bundle.as_ref().expect("the port is a bundle").id;
        assert_eq!(FILE, format!("{id}-{}.{}", recipe.version, dkv::EXTENSION));
        assert!(FILE.starts_with(BUNDLE_PREFIX));
    }
}
