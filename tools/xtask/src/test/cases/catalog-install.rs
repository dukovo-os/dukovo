// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Bundles install from a catalog (ADR 0383). A second disk holds a
//! catalog that ob signed with throwaway keys, which lists ripgrep's
//! bundle alone, its first root, and the `.dkv` files of ripgrep and
//! bat. Alice, the owner, adds the catalog through elevate, with its
//! address on the disk and its root, and bundled reads it with the TUF
//! client. bundle catalog lists it; bundle search finds ripgrep there and
//! not bat. Installed by its ID, ripgrep's sheet says it comes from the
//! catalog, with no warning, and rg runs. bat's file, which no catalog
//! lists, shows the warning, and she says no. ripgrep's own file from the
//! disk installs as the catalog's. Once the catalog is removed, nothing
//! installs ripgrep, which is bound to it. The power button ends it.

use super::super::*;

/// The catalog's disk's label.
const LABEL: &str = "CATALOG";

/// Where Spark mounts it: the second virtio disk's name.
const VOLUME: &str = "/volumes/vdb";

/// Dukovo's root on a copy of the test image, and the catalog.
const DISKS: &[Disk] = &[
    Disk { label: image::ROOT_LABEL, ..btrfs_disk(Bus::Virtio, 512, ROOT, root_checked) },
    catalog_site_disk(64 * 1024, LABEL),
];

/// The test.
pub const CASE: Case = Case {
    name: "catalog-install",
    about: "bundles install from a catalog (ADR 0383): an administrator adds a catalog signed with throwaway keys, from a disk; bundle search finds what it lists; ripgrep installs by its ID with no warning and runs; bat's file, which no catalog lists, shows the warning; ripgrep's own file installs as the catalog's; once the catalog is removed, nothing installs ripgrep",
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
    script: catalog_install,
    disks: DISKS,
    second: None,
};

fn catalog_install(vm: &mut Vm, case: &Case) -> Result<(), String> {
    owner_signs_in(vm, case)?;
    vm.expect_anywhere("bundled: 0 bundles installed; answering at /system/live/bundled/socket")?;
    vm.expect_anywhere(&format!("spark: mounted the FAT filesystem of vdb at {VOLUME}"))?;
    // The catalog, added by an administrator with its root.
    answer_prompt(
        vm,
        "$",
        &format!(
            "clear; elevate bundle catalog add test file://{VOLUME}/catalog {VOLUME}/root.json"
        ),
    )?;
    // Her password, at the console's own prompt; the pane asks no more
    // for 15 minutes.
    answer_prompt(vm, &format!("{VOLUME}/root.json:"), "queen of hearts")?;
    vm.expect_screen(&["added catalog test, which lists 1 bundle"])?;
    vm.expect_anywhere(&format!(
        "bundled: root added catalog test, at file://{VOLUME}/catalog/, which lists 1 bundle"
    ))?;
    answer_prompt(vm, "$", "clear; bundle catalog")?;
    vm.expect_screen(&["NAME", "test  on   1 ", &format!("file://{VOLUME}/catalog/")])?;
    // What it lists, and what it does not.
    answer_prompt(vm, "$", "clear; bundle search RIP; bundle search bat; echo \"bat $?\"")?;
    vm.expect_screen(&[
        "VERSION  CATALOG  NAME",
        "io.github.burntsushi.ripgrep  15.2.0   test     ripgrep",
        "no bundle found",
        "bat 1",
    ])?;
    // By its ID: fetched and checked by the client, and trusted as the
    // catalog's, with no warning.
    answer_prompt(vm, "$", "clear; bundle install io.github.burntsushi.ripgrep")?;
    vm.expect_screen(&[
        "ripgrep 15.2.0, for yourself",
        "  From catalog test, which this computer trusts: it lists this very file.",
    ])?;
    vm.expect_screen_without("Warning")?;
    answer_prompt(vm, "Install it? [y/N]", "y")?;
    vm.expect_screen(&["installed io.github.burntsushi.ripgrep 15.2.0, for you"])?;
    vm.expect_anywhere("(from catalog test)")?;
    answer_prompt(vm, "$", "clear; rg --version | head -1")?;
    vm.expect_screen(&["[warning] ", "A: always"])?;
    vm.type_text("a")?;
    vm.expect_screen(&["ripgrep 15.2.0"])?;
    // bat's file: no catalog lists it.
    answer_prompt(
        vm,
        "$",
        &format!(
            "clear; bundle install {VOLUME}/io.github.sharkdp.bat-0.26.1.dkv; echo \"status $?\""
        ),
    )?;
    vm.expect_screen(&[
        "bat 0.26.1, for yourself",
        "  Warning: this computer does not know this key.",
        "  No catalog it trusts lists this file. That the bundle comes from",
    ])?;
    answer_prompt(vm, "Install it? [y/N]", "n")?;
    vm.expect_screen(&["not installed, as you said", "status 1"])?;
    // ripgrep's own file from the disk: the catalog lists these bytes.
    answer_prompt(
        vm,
        "$",
        &format!("clear; cd {VOLUME}; bundle install io.github.burntsushi.ripgrep-15.2.0.dkv"),
    )?;
    vm.expect_screen(&[
        "  From catalog test, which this computer trusts: it lists this very file.",
        "  The bundle has come from there since ",
    ])?;
    answer_prompt(vm, "Install it? [y/N]", "y")?;
    vm.expect_screen(&["installed io.github.burntsushi.ripgrep 15.2.0 in place of 15.2.0, for you"])?;
    // Removed: ripgrep is bound to the catalog, so nothing installs it.
    answer_prompt(vm, "$", "clear; elevate bundle catalog remove test")?;
    vm.expect_screen(&["removed catalog test"])?;
    answer_prompt(
        vm,
        "$",
        "clear; bundle install --yes io.github.burntsushi.ripgrep-15.2.0.dkv 2>&1 | grep -o \
         'comes from catalog test, since its first install'",
    )?;
    vm.expect_screen(&["comes from catalog test, since its first install"])?;
    vm.press_power_button()?;
    vm.expect("spark: every service has stopped")?;
    vm.expect("Power: `spark` asked to power off the machine; syncing the filesystems")?;
    vm.expect_exit(QEMU_EXIT_POWER_OFF)
}
