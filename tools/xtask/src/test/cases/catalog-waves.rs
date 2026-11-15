// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! A catalog's bundles go out in waves (ADR 0544). A second disk holds a
//! catalog that ob signed with throwaway keys, which lists versions 1.0.0
//! and 1.1.0 of two small test bundles, signs the rollout of each 1.1.0
//! with the waves just begun, and holds diary 1.1.0's waves with a
//! reason. Alice adds the catalog and installs both 1.0.0 files, and the
//! machine's number is 700, in the last wave, which opens on day 7.
//! bundle update --check says that both updates wait for the machine's
//! wave, on day 7, and why diary's is held; updating by themselves and
//! bundle update with no ID take nothing. A canary, which is in every
//! wave, is offered both. Named, a bundle is taken at once. The power
//! button ends it.

use super::super::*;

/// The catalog's disk's label.
const LABEL: &str = "UPDATES";

/// Where Spark mounts it: the second virtio disk's name.
const VOLUME: &str = "/volumes/vdb";

/// Dukovo's root on a copy of the test image, and the catalog.
const DISKS: &[Disk] = &[
    Disk { label: image::ROOT_LABEL, ..btrfs_disk(Bus::Virtio, 512, ROOT, root_checked) },
    waves_site_disk(16 * 1024, LABEL),
];

/// The test.
pub const CASE: Case = Case {
    name: "catalog-waves",
    about: "a catalog's bundles go out in waves (ADR 0544): on a machine numbered 700 bundle update --check says an update waits for its wave, on day 7, and why a paused one is held; updating by themselves and bundle update take nothing; a canary is offered both; a bundle named is taken at once",
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
    script: catalog_waves,
    disks: DISKS,
    second: None,
};

/// `command` as root, with alice's password: she is an administrator.
fn as_root(command: &str) -> String {
    format!("echo 'queen of hearts' | elevate -S {command}")
}

fn catalog_waves(vm: &mut Vm, case: &Case) -> Result<(), String> {
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
    // The machine's number: in the last wave, which opens on day 7.
    answer_prompt(
        vm,
        "$",
        &format!(
            "clear; {}",
            as_root(
                "sh -c 'mkdir -p /system/state/sysupdate; echo 700 > /system/state/sysupdate/wave'"
            )
        ),
    )?;
    // Both updates wait, and diary's waves are held, with why.
    answer_prompt(vm, "$", "clear; bundle update --check")?;
    vm.expect_screen(&[
        "org.example.diary 1.1.0 reaches machines in waves; this machine's opens on day 7",
        "the waves are held: paused: bad logs",
        "`bundle update org.example.diary` takes it now",
        "org.example.fetcher 1.1.0 reaches machines in waves; this machine's opens on day 7",
        "`bundle update org.example.fetcher` takes it now",
        "0 to update, 0 up to date, 2 waiting for their wave",
    ])?;
    vm.expect_screen_without("INSTALLED")?;
    // By themselves, they take nothing either: bundled says so.
    answer_prompt(vm, "$", "clear; bundle update --automatic on")?;
    vm.expect_screen(&["Your bundles update by themselves from now on, daily"])?;
    vm.expect_anywhere(
        "bundled: org.example.diary for alice waits: org.example.diary 1.1.0 reaches machines in \
         waves; this machine's opens on day 7; the waves are held: paused: bad logs",
    )?;
    vm.expect_anywhere(
        "bundled: org.example.fetcher for alice waits: org.example.fetcher 1.1.0 reaches \
         machines in waves; this machine's opens on day 7",
    )?;
    answer_prompt(vm, "$", "clear; bundle update --automatic off")?;
    // bundle update with no ID takes nothing.
    answer_prompt(vm, "$", "clear; bundle update")?;
    vm.expect_screen(&[
        "org.example.diary 1.1.0 reaches machines in waves",
        "org.example.fetcher 1.1.0 reaches machines in waves",
        "0 updated, 0 not, 0 up to date, 2 waiting for their wave",
    ])?;
    answer_prompt(vm, "$", "clear; diary")?;
    vm.expect_screen(&["diary 1.0.0 ran"])?;
    vm.expect_screen_without("1.1.0 ran")?;
    // A canary is in every wave, the first of them, which is open: both
    // updates are offered, though diary's waves are held.
    answer_prompt(
        vm,
        "$",
        &format!(
            "clear; {}",
            as_root(
                "sh -c 'mkdir -p /system/settings/sysupdate; echo \"canary = true\" > \
                 /system/settings/sysupdate/updates.toml'"
            )
        ),
    )?;
    answer_prompt(vm, "$", "clear; bundle update --check")?;
    vm.expect_screen(&[
        "INSTALLED  NEWEST  CATALOG",
        "org.example.diary    1.0.0      1.1.0   test",
        "org.example.fetcher  1.0.0      1.1.0   test",
        "2 to update, 0 up to date",
    ])?;
    vm.expect_screen_without("reaches machines in waves")?;
    answer_prompt(
        vm,
        "$",
        &format!("clear; {}", as_root("rm /system/settings/sysupdate/updates.toml")),
    )?;
    // A bundle named is taken at once, whatever its wave.
    answer_prompt(vm, "$", "clear; bundle update --yes org.example.diary")?;
    vm.expect_screen(&[
        "installed org.example.diary 1.1.0 in place of 1.0.0, for you",
        "1 updated, 0 not, 0 up to date",
    ])?;
    answer_prompt(vm, "$", "clear; diary")?;
    vm.expect_screen(&["diary 1.1.0 ran"])?;
    vm.press_power_button()?;
    vm.expect("spark: every service has stopped")?;
    vm.expect("Power: `spark` asked to power off the machine; syncing the filesystems")?;
    vm.expect_exit(QEMU_EXIT_POWER_OFF)
}
