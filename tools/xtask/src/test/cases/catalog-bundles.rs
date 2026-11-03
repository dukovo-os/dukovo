// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The catalog's bundles of ports install and run (ADR 0370). Dukovo's
//! root is on a disk, so that the homes have room, and a second disk
//! holds the `.dkv` files `ob ports` packed: ripgrep's and bat's, which
//! the image does not carry. Alice signs in, as the owner. ripgrep's
//! install sheet names it, its publisher, the warning that this computer
//! does not know the build's key, and rg as a command that runs with her
//! access, outside the sandbox; she installs it, then bat. Each command's
//! first run asks, with a warning, whether it may run with her access;
//! allowed always, rg finds a line in a file of hers, and bat shows it
//! with line numbers. Both are in bundle list, and each carries its
//! licenses and SBOM. The power button ends it.

use super::super::*;

/// The disk of bundles' label.
const LABEL: &str = "BUNDLES";

/// Where Spark mounts it: the second virtio disk's name.
const VOLUME: &str = "/volumes/vdb";

/// Dukovo's root on a copy of the test image, and the bundles.
const DISKS: &[Disk] = &[
    Disk { label: image::ROOT_LABEL, ..btrfs_disk(Bus::Virtio, 512, ROOT, root_checked) },
    bundle_files_disk(128 * 1024, LABEL),
];

/// The test.
pub const CASE: Case = Case {
    name: "catalog-bundles",
    about: "the catalog's bundles of ports install and run (ADR 0370): ripgrep's and bat's .dkv files, which ob ports packed and the image does not carry, install from a disk; the sheet says rg runs with your access, outside the sandbox; each command's first run asks with a warning, then rg finds a line and bat shows it numbered; each bundle carries its licenses and SBOM",
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
    script: catalog_bundles,
    disks: DISKS,
    second: None,
};

fn catalog_bundles(vm: &mut Vm, case: &Case) -> Result<(), String> {
    owner_signs_in(vm, case)?;
    vm.expect_anywhere("bundled: 0 bundles installed; answering at /system/live/bundled/socket")?;
    vm.expect_anywhere(&format!("spark: mounted the FAT filesystem of vdb at {VOLUME}"))?;
    answer_prompt(vm, "$", &format!("clear; cd {VOLUME}; ls"))?;
    vm.expect_screen(&["io.github.burntsushi.ripgrep-15.2.0.dkv", "io.github.sharkdp.bat-0.26.1.dkv"])?;
    // ripgrep's sheet: who made it, the unknown key, and its command,
    // which runs with her access.
    answer_prompt(vm, "$", "clear; bundle install io.github.burntsushi.ripgrep-15.2.0.dkv")?;
    vm.expect_screen(&[
        "ripgrep 15.2.0, for yourself",
        "  io.github.burntsushi.ripgrep, by Dukovo catalog",
        "  Warning: this computer does not know this key.",
        "  Commands that run with your access, outside its sandbox: rg",
    ])?;
    answer_prompt(vm, "Install it? [y/N]", "y")?;
    vm.expect_screen(&["installed io.github.burntsushi.ripgrep 15.2.0, for you"])?;
    answer_prompt(vm, "$", "clear; bundle install --yes io.github.sharkdp.bat-0.26.1.dkv | tail -1")?;
    vm.expect_screen(&["installed io.github.sharkdp.bat 0.26.1, for you"])?;
    // rg's first run asks, with a warning.
    answer_prompt(
        vm,
        "$",
        "clear; cd; printf 'alpha\\nneedle 42\\nomega\\n' > notes.txt; rg -n needle notes.txt; \
         echo \"rg $?\"",
    )?;
    vm.expect_screen(&["[warning] ", "A: always"])?;
    vm.type_text("a")?;
    vm.expect_anywhere("bundled: alice allowed io.github.burntsushi.ripgrep runs-with-your-access")?;
    vm.expect_screen(&["2:needle 42", "rg 0"])?;
    // Allowed always: no question the second time.
    answer_prompt(vm, "$", "clear; rg -c a notes.txt; echo \"again $?\"")?;
    vm.expect_screen(&["2", "again 0"])?;
    vm.expect_screen_without("[warning]")?;
    answer_prompt(vm, "$", "clear; bat -n --paging=never notes.txt; echo \"bat $?\"")?;
    vm.expect_screen(&["[warning] ", "A: always"])?;
    vm.type_text("a")?;
    vm.expect_screen(&["2 needle 42", "bat 0"])?;
    // Both listed; each carries its licenses and its SBOM, which lists
    // the crates it links.
    answer_prompt(
        vm,
        "$",
        "clear; bundle list | cut -c1-40; cd \
         applications/io.github.burntsushi.ripgrep; ls licenses; grep -c '^PackageName: ' \
         sbom.spdx; grep -q 'PackageName: regex-automata' sbom.spdx && echo 'sbom: crates'",
    )?;
    vm.expect_screen(&[
        "io.github.burntsushi.ripgrep",
        "io.github.sharkdp.bat",
        "README  notices  texts",
        "sbom: crates",
    ])?;
    vm.press_power_button()?;
    vm.expect("spark: every service has stopped")?;
    vm.expect("Power: `spark` asked to power off the machine; syncing the filesystems")?;
    vm.expect_exit(QEMU_EXIT_POWER_OFF)
}
