// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The catalog's commands built with Go run on Dukovo (ADR 0600): micro,
//! fzf, duf and lazygit. Dukovo's root is on a disk, and a second disk
//! holds the `.dkv` files `ob ports` packed for them. Alice installs each
//! for herself, in her own applications folder; each command's first run
//! asks, with a warning, whether it may run with her access, is allowed
//! always, and says its version. micro opens a new file of hers, she types
//! a line, saves it with Ctrl-S and quits with Ctrl-Q, and the file holds
//! the line: Phase 17's exit test. fzf filters a list given -f, and in a
//! pipe shows the list and picks the line whose start she types; duf shows
//! /users on Btrfs; in a new git repository lazygit greets her once, shows
//! its status and the file she made, and q quits it. The power button ends
//! it.

use super::super::*;

/// The disk of bundles' label.
const LABEL: &str = "BUNDLES";

/// Where Spark mounts it: the second virtio disk's name.
const VOLUME: &str = "/volumes/vdb";

/// The bundle files, and what each command's first run prints of its
/// version.
const BUNDLES: &[(&str, &str, &str)] = &[
    ("io.github.zyedidia.micro-2.0.15.dkv", "micro -version", "Version: 2.0.15"),
    ("io.github.junegunn.fzf-0.75.0.dkv", "fzf --version", "0.75.0 (c7bf2b8)"),
    ("io.github.muesli.duf-0.9.1.dkv", "duf --version", "duf 0.9.1"),
    ("io.github.jesseduffield.lazygit-0.66.3.dkv", "lazygit --version", "version=0.66.3, os=radix"),
];

/// How long a step may take under emulation: a program of up to 20 MiB
/// read in at its first run, or a bundle unpacked.
const SECONDS: u64 = 120;

/// The line Alice types into micro.
const LINE: &str = "Hello from micro on Dukovo";

/// Dukovo's root on a copy of the test image, and the bundles.
const DISKS: &[Disk] = &[
    Disk { label: image::ROOT_LABEL, ..btrfs_disk(Bus::Virtio, 512, ROOT, root_checked) },
    Disk { fat: GO_APPS_FILES, ..bundle_files_disk(64 * 1024, LABEL) },
];

/// The test.
pub const CASE: Case = Case {
    name: "go-commands",
    about: "the catalog's commands built with Go run (ADR 0600): micro, fzf, duf and lazygit, installed for one person from their .dkv files, each asking at its first run whether it may run with her access; micro opens a file, takes a typed line, saves it with Ctrl-S and quits with Ctrl-Q, and the file holds the line (Phase 17's exit test); fzf filters with -f and picks a typed line from a pipe; duf shows /users on Btrfs; lazygit greets her once in a new repository, shows its status and her file, and quits on q",
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
    script: go_commands,
    disks: DISKS,
    second: None,
};

/// Waits up to `seconds` for the screen to show each of `needles`: a
/// full-screen program draws a while after it starts under emulation.
fn shows_within(vm: &mut Vm, needles: &[&str], seconds: u64) -> Result<(), String> {
    let deadline = Instant::now() + Duration::from_secs(seconds);
    loop {
        let screen = vm.screen()?;
        match screen_shows(&screen, needles) {
            Ok(()) => return Ok(()),
            Err(err) if Instant::now() >= deadline => return Err(err),
            Err(_) => std::thread::sleep(Duration::from_millis(500)),
        }
    }
}

fn go_commands(vm: &mut Vm, case: &Case) -> Result<(), String> {
    owner_signs_in(vm, case)?;
    vm.expect_anywhere("bundled: 0 bundles installed; answering at /system/live/bundled/socket")?;
    vm.expect_anywhere(&format!("spark: mounted the FAT filesystem of vdb at {VOLUME}"))?;
    // For herself, in ~/applications; each command runs with her access,
    // so its first run asks, with a warning, and is allowed always.
    for (n, &(file, version, says)) in BUNDLES.iter().enumerate() {
        let name = format!("install{n}");
        super::go_toolchain::succeeds(
            vm,
            &name,
            &format!("bundle install --yes {VOLUME}/{file}"),
            SECONDS,
        )?;
        run_answering_until(vm, &format!("clear; {version}"), says, SECONDS)?;
    }

    // micro: a new file, a line typed into it, saved with Ctrl-S and
    // quit with Ctrl-Q, which micro's terminal settings leave to it rather
    // than to the terminal's flow control. The file then holds the line.
    answer_prompt(vm, "$", "clear; micro notes.txt; echo \"micro $?\"")?;
    shows_within(vm, &["notes.txt"], SECONDS)?;
    vm.type_text(LINE)?;
    vm.expect_screen(&[LINE])?;
    vm.press(&["ctrl-s"])?;
    vm.expect_screen(&["Saved"])?;
    vm.press(&["ctrl-q"])?;
    vm.expect_screen(&["micro 0"])?;
    answer_prompt(vm, "$", "clear; echo \"notes: $(cat notes.txt)\"")?;
    vm.expect_screen(&[&format!("notes: {LINE}")])?;

    // fzf: -f filters a list without asking; in a pipe it shows the list on
    // the terminal, and Enter picks the line that matches what she typed.
    answer_prompt(
        vm,
        "$",
        "clear; printf 'apple\\nbanana\\ncherry\\n' > fruit; fzf -f nan < fruit > filtered; echo \
         \"filtered: $(cat filtered)\"",
    )?;
    vm.expect_screen(&["filtered: banana"])?;
    answer_prompt(vm, "$", "clear; fzf < fruit > picked; echo \"picked: $(cat picked)\"")?;
    shows_within(vm, &["3/3"], SECONDS)?;
    vm.type_text("che")?;
    vm.expect_screen(&["1/3"])?;
    vm.press(&["ret"])?;
    vm.expect_screen(&["picked: cherry"])?;

    // duf: /users, a subvolume of the root's Btrfs, from /processes/mounts,
    // with its sizes from statvfs. `/` itself is the tmpfs of her view
    // (ADR 0215), which duf counts as a special device.
    answer_prompt(vm, "$", "clear; duf /users")?;
    vm.expect_screen(&["1 local device", "/users", "btrfs"])?;

    // lazygit, in a new repository with a file in it: its greeting once,
    // confirmed with Enter, then its panels, the status and the file among
    // them; q quits.
    answer_prompt(
        vm,
        "$",
        "clear; mkdir repo; cd repo; git init -q; echo hi > hello.txt; lazygit; echo \"lazygit \
         $?\"",
    )?;
    shows_within(vm, &["Thanks for using lazygit!"], SECONDS)?;
    vm.press(&["ret"])?;
    shows_within(vm, &["Status", "hello.txt"], SECONDS)?;
    vm.press(&["q"])?;
    vm.expect_screen(&["lazygit 0"])?;

    vm.press_power_button()?;
    vm.expect("spark: every service has stopped")?;
    vm.expect("Power: `spark` asked to power off the machine; syncing the filesystems")?;
    vm.expect_exit(QEMU_EXIT_POWER_OFF)
}
