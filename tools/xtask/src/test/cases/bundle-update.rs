// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Bundles update from their catalog, go back and hold (ADR 0384). A
//! second disk holds a catalog that ob signed with throwaway keys, which
//! lists versions 1.0.0 and 1.1.0 of two small test bundles, and the
//! files of their 1.0.0. Alice adds the catalog and installs both 1.0.0
//! files; diary keeps a line in its data for each run. She turns on
//! updating by themselves: diary updates at once, and fetcher 1.1.0,
//! which asks to connect out, waits for her yes. diary 1.1.0 runs on a
//! copy of what 1.0.0 left. bundle update shows fetcher's new permission
//! and asks. She goes back to diary 1.0.0, whose data comes back as it
//! was, and which is held there; bundle update leaves it, and bundle
//! history says what was done. Let go, it updates again, and can go
//! back. The power button ends it.

use super::super::*;

/// The catalog's disk's label.
const LABEL: &str = "UPDATES";

/// Where Spark mounts it: the second virtio disk's name.
const VOLUME: &str = "/volumes/vdb";

/// Dukovo's root on a copy of the test image, and the catalog.
const DISKS: &[Disk] = &[
    Disk { label: image::ROOT_LABEL, ..btrfs_disk(Bus::Virtio, 512, ROOT, root_checked) },
    update_site_disk(16 * 1024, LABEL),
];

/// The test.
pub const CASE: Case = Case {
    name: "bundle-update",
    about: "bundles update from their catalog, go back and hold (ADR 0384): updating by themselves updates diary and leaves fetcher, which asks for more, waiting; diary 1.1.0 runs on a copy of 1.0.0's data; bundle update shows fetcher's new permission and asks; going back brings 1.0.0's data back and holds it; bundle history says what was done",
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
    script: bundle_update,
    disks: DISKS,
    second: None,
};

fn bundle_update(vm: &mut Vm, case: &Case) -> Result<(), String> {
    owner_signs_in(vm, case)?;
    vm.expect_anywhere("bundled: 0 bundles installed; answering at /system/live/bundled/socket")?;
    vm.expect_anywhere(&format!("spark: mounted the FAT filesystem of vdb at {VOLUME}"))?;
    answer_prompt(
        vm,
        "$",
        &format!(
            "clear; elevate bundle catalog add test file://{VOLUME}/catalog {VOLUME}/root.json"
        ),
    )?;
    answer_prompt(vm, &format!("{VOLUME}/root.json:"), "queen of hearts")?;
    vm.expect_screen(&["added catalog test, which lists 4 bundles"])?;
    // The first versions, from their files, which the catalog lists.
    answer_prompt(
        vm,
        "$",
        &format!(
            "clear; cd {VOLUME}; bundle install --yes org.example.diary-1.0.0.dkv | tail -1; \
             bundle install --yes org.example.fetcher-1.0.0.dkv | tail -1; cd"
        ),
    )?;
    vm.expect_screen(&[
        "installed org.example.diary 1.0.0, for you",
        "installed org.example.fetcher 1.0.0, for you",
    ])?;
    answer_prompt(vm, "$", "clear; diary")?;
    vm.expect_screen(&["diary 1.0.0 ran"])?;
    answer_prompt(vm, "$", "clear; bundle update --check")?;
    vm.expect_screen(&[
        "INSTALLED  NEWEST  CATALOG",
        "org.example.diary    1.0.0      1.1.0   test",
        "org.example.fetcher  1.0.0      1.1.0   test",
        "2 to update, 0 up to date",
    ])?;
    // By themselves: diary at once; fetcher, which asks for more, waits.
    answer_prompt(vm, "$", "clear; bundle update --automatic on")?;
    vm.expect_screen(&["Your bundles update by themselves from now on, daily"])?;
    vm.expect_anywhere(
        "bundled: updated org.example.diary 1.0.0 to 1.1.0 for alice by itself, from catalog test",
    )?;
    vm.expect_anywhere(
        "bundled: org.example.fetcher 1.1.0 for alice: waits for your yes: it asks for more than \
         1.0.0 did: network.outbound",
    )?;
    // 1.1.0's first run keeps a copy of what 1.0.0 left, and goes on.
    answer_prompt(vm, "$", "clear; diary")?;
    vm.expect_screen(&[
        "org.example.diary: kept a copy of the data 1.0.0 left, for 30 days",
        "diary 1.0.0 ran",
        "diary 1.1.0 ran",
    ])?;
    // Asked: fetcher's sheet says what it asks for anew.
    answer_prompt(vm, "$", "clear; bundle update")?;
    vm.expect_screen(&[
        "fetcher 1.1.0, for yourself, in place of 1.0.0",
        "Connect to the internet and to services on this computer (at install), new in this \
         version",
    ])?;
    answer_prompt(vm, "Install it? [y/N]", "y")?;
    vm.expect_screen(&[
        "installed org.example.fetcher 1.1.0 in place of 1.0.0, for you",
        "1 updated, 0 not, 1 up to date",
    ])?;
    answer_prompt(vm, "$", "clear; fetcher")?;
    vm.expect_screen(&["fetcher 1.1.0"])?;
    // Back to diary 1.0.0, and its data as 1.0.0 left it.
    answer_prompt(vm, "$", "clear; bundle rollback org.example.diary")?;
    vm.expect_screen(&[
        "went back to org.example.diary 1.0.0, from 1.1.0",
        "it is held there until `bundle unhold org.example.diary`",
    ])?;
    answer_prompt(vm, "$", "clear; diary")?;
    vm.expect_screen(&["org.example.diary: brought back the data 1.0.0 left", "diary 1.0.0 ran"])?;
    vm.expect_screen_without("1.1.0 ran")?;
    // Held: updates leave it.
    answer_prompt(vm, "$", "clear; bundle update; bundle hold org.example.diary")?;
    vm.expect_screen(&[
        "org.example.diary is held at 1.0.0, so 1.1.0 waits",
        "0 updated, 1 not, 1 up to date",
        "org.example.diary is held already",
    ])?;
    answer_prompt(vm, "$", "clear; bundle history org.example.diary")?;
    vm.expect_screen(&[
        "org.example.diary  installed 1.0.0, from catalog test",
        "org.example.diary  updated 1.0.0 to 1.1.0, from catalog test, by itself",
        "org.example.diary  went back to 1.0.0, from 1.1.0",
    ])?;
    // Let go, it updates, and can go back again.
    answer_prompt(
        vm,
        "$",
        "clear; bundle unhold org.example.diary; bundle update --yes | tail -1; bundle info \
         org.example.diary | tail -1",
    )?;
    vm.expect_screen(&[
        "org.example.diary updates again",
        "1 updated, 0 not, 1 up to date",
        "It can go back to 1.0.0",
    ])?;
    vm.press_power_button()?;
    vm.expect("spark: every service has stopped")?;
    vm.expect("Power: `spark` asked to power off the machine; syncing the filesystems")?;
    vm.expect_exit(QEMU_EXIT_POWER_OFF)
}
