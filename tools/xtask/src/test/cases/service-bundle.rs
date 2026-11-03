// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! A bundle's service runs (ADR 0385). A second disk holds beacon, a
//! small test bundle whose service starts at boot, counts its starts in
//! its data folder, says as whom it runs, and crashes on its first start.
//! Alice cannot install it for herself: its service runs for everyone.
//! Through elevate she installs it for everyone: the sheet names the
//! service and its account, authd makes the account, and Spark starts the
//! service as it, in its data folder, which is the account's alone and
//! which Alice cannot read. It crashes, and Spark starts it again. In the
//! second boot it starts at boot, as its account; removed with --purge,
//! it stops, and its account and data go with it.

use super::super::*;

/// The bundle disk's label.
const LABEL: &str = "SERVICES";

/// Where Spark mounts it: the second virtio disk's name.
const VOLUME: &str = "/volumes/vdb";

/// The bundle's file.
const FILE: &str = "org.example.beacon-1.0.0.dkv";

/// Its data folder.
const DATA: &str = "/shared/services/org.example.beacon/beacon";

/// Dukovo's root on a copy of the test image, and the bundle's file.
const DISKS: &[Disk] = &[
    Disk { label: image::ROOT_LABEL, ..btrfs_disk(Bus::Virtio, 512, ROOT, root_checked) },
    service_files_disk(8 * 1024, LABEL),
];

/// The test.
pub const CASE: Case = Case {
    name: "service-bundle",
    about: "a bundle's service runs (ADR 0385): installed for everyone, its service runs as an account of its own in a data folder only it reads, Spark starts it again after it crashes and at the next boot, and removing the bundle with --purge takes the account and data away",
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
    script: install,
    disks: DISKS,
    second: Some(SecondBoot { cmdline: "lockdep", before: nothing_between, script: at_boot }),
};

fn install(vm: &mut Vm, case: &Case) -> Result<(), String> {
    owner_signs_in(vm, case)?;
    vm.expect_anywhere("bundled: 0 bundles installed; answering at /system/live/bundled/socket")?;
    vm.expect_anywhere(&format!("spark: mounted the FAT filesystem of vdb at {VOLUME}"))?;
    // Not for herself alone: its service runs for everyone.
    answer_prompt(
        vm,
        "$",
        &format!(
            "clear; cd {VOLUME}; bundle install --yes {FILE} 2>&1 | grep -o 'org.example.beacon \
             runs services, which run for everyone'"
        ),
    )?;
    vm.expect_screen(&["org.example.beacon runs services, which run for everyone"])?;
    answer_prompt(
        vm,
        "$",
        &format!("clear; elevate bundle install --for-everyone --yes {FILE} | tail -3; cd"),
    )?;
    answer_prompt(vm, &format!("{FILE}:"), "queen of hearts")?;
    vm.expect_screen(&[
        "beacon, as the account beacon, starting now and at each boot",
        "installed org.example.beacon 1.0.0, for everyone; its service beacon runs",
    ])?;
    vm.expect_anywhere("authd: made the account beacon (100) of a service of org.example.beacon")?;
    // As its account, in its data folder; it crashes, and starts again.
    vm.expect_anywhere("beacon: beacon 1.0.0 runs as beacon, start 1")?;
    vm.expect_anywhere("beacon: beacon crashes")?;
    vm.expect_anywhere("spark: starting beacon again in ")?;
    vm.expect_anywhere("beacon: beacon 1.0.0 runs as beacon, start 2")?;
    answer_prompt(
        vm,
        "$",
        &format!("clear; ls -ld {DATA}; ls {DATA}; echo \"status $?\"; id beacon"),
    )?;
    vm.expect_screen(&[
        "drwx------",
        "beacon beacon",
        "Permission denied",
        "status 2",
        "uid=100(beacon) gid=100(beacon) groups=100(beacon)",
    ])?;
    vm.press_power_button()?;
    vm.expect("spark: every service has stopped")?;
    vm.expect("Power: `spark` asked to power off the machine; syncing the filesystems")?;
    vm.expect_exit(QEMU_EXIT_POWER_OFF)
}

/// The second boot: Spark starts beacon at boot, as its account, on the
/// data it kept; removed with --purge, its account and data go too.
fn at_boot(vm: &mut Vm, case: &Case) -> Result<(), String> {
    boot_report(vm, case)?;
    vm.expect("Boot complete.")?;
    vm.expect_anywhere("beacon: beacon 1.0.0 runs as beacon, start 3")?;
    vm.expect_anywhere("consoled: started /system/commands/login on tty1 as process ")?;
    answer_prompt(vm, "Name:", "alice")?;
    answer_prompt(vm, "Password:", "queen of hearts")?;
    vm.expect_anywhere("authd: alice signed in on pts/")?;
    answer_prompt(
        vm,
        "$",
        "clear; elevate bundle remove --for-everyone --purge org.example.beacon; id beacon; echo \
         \"left: $(ls /shared/services).\"",
    )?;
    answer_prompt(vm, "--purge org.example.beacon:", "queen of hearts")?;
    vm.expect_screen(&[
        "removed org.example.beacon 1.0.0, with its services' accounts and data",
        "id: 'beacon': no such user",
        "left: .",
    ])?;
    vm.press_power_button()?;
    vm.expect("spark: every service has stopped")?;
    vm.expect_exit(QEMU_EXIT_POWER_OFF)
}
