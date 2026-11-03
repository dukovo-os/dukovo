// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! A service bundle's updates (ADR 0386). A second disk holds three
//! versions of lighthouse, whose service keeps a line in its data for
//! each start and answers its health check on a socket, but for 2.0.0,
//! which never answers. Alice installs 1.0.0 for everyone; as an
//! administrator she sets a greeting in an override file in settings.d
//! and changes the file bundled wrote, and service check says so. 1.1.0
//! keeps her file, puts the new one beside it as a .default, keeps a copy
//! of the data 1.0.0 left, and passes its health check, greeting as she
//! set. 2.0.0 does not answer within 60 seconds: bundled goes back to
//! 1.1.0 by itself, with the data 1.1.0 left, holds it, and says why in
//! the history.

use super::super::*;

/// The bundle disk's label.
const LABEL: &str = "SERVICES";

/// Where Spark mounts it: the second virtio disk's name.
const VOLUME: &str = "/volumes/vdb";

/// The bundle's ID.
const ID: &str = "org.example.lighthouse";

/// Where the service files are.
const FILES: &str = "/system/settings/spark/services";

/// Its account's data folder, and the copies beside it.
const DATA: &str = "/shared/services/org.example.lighthouse";

/// Dukovo's root on a copy of the test image, and the bundles' files.
const DISKS: &[Disk] = &[
    Disk { label: image::ROOT_LABEL, ..btrfs_disk(Bus::Virtio, 512, ROOT, root_checked) },
    service_files_disk(8 * 1024, LABEL),
];

/// The test.
pub const CASE: Case = Case {
    name: "service-update",
    about: "a service bundle's updates (ADR 0386): an administrator's override in settings.d and her changed service file stay, the new file goes beside it as a .default, the data is copied before each update, and a version that fails its health check goes back by itself, with its data",
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
    script: update,
    disks: DISKS,
    second: None,
};

/// The file of lighthouse `version`.
fn file(version: &str) -> String {
    format!("{ID}-{version}.dkv")
}

/// Installs lighthouse `version` for everyone, from the disk, with its
/// message a part to a line. The message is split once elevate has ended:
/// it keeps the terminal raw while the command runs, so a command beside
/// it in a pipeline would write lines without their carriage returns.
fn install(vm: &mut Vm, version: &str) -> Result<(), String> {
    answer_prompt(
        vm,
        "$",
        &format!(
            "clear; cd {VOLUME}; elevate bundle install --for-everyone --yes {} > /tmp/said \
             2>&1; cd; tr ';,' '\\n\\n' < /tmp/said",
            file(version)
        ),
    )
}

fn update(vm: &mut Vm, case: &Case) -> Result<(), String> {
    owner_signs_in(vm, case)?;
    vm.expect_anywhere("bundled: 0 bundles installed; answering at /system/live/bundled/socket")?;
    vm.expect_anywhere(&format!("spark: mounted the FAT filesystem of vdb at {VOLUME}"))?;
    // 1.0.0; the first elevate asks for her password.
    install(vm, "1.0.0")?;
    answer_prompt(vm, &format!("{}:", file("1.0.0")), "queen of hearts")?;
    vm.expect_screen(&[" its service lighthouse runs"])?;
    vm.expect("lighthouse: lighthouse 1.0.0 runs, greeting none, start 1")?;
    // Her greeting in an override, and a change to the file bundled
    // wrote; started again, it greets.
    answer_prompt(
        vm,
        "$",
        &format!(
            "clear; elevate sh -c 'mkdir -p {FILES}/settings.d && echo \"[environment]\" > \
             {FILES}/settings.d/lighthouse.toml && echo \"GREETING = \\\"hello\\\"\" >> \
             {FILES}/settings.d/lighthouse.toml && sed -i \"s/cpu_weight = 100/cpu_weight = 90/\" \
             {FILES}/lighthouse.toml && service lighthouse restart' >/dev/null; service check \
             lighthouse | tr ';,' '\\n\\n'"
        ),
    )?;
    vm.expect_screen(&[
        "/system/settings/spark/services/lighthouse.toml: ok",
        " with /system/settings/spark/services/settings.d/lighthouse.toml over it",
    ])?;
    vm.expect("lighthouse: lighthouse 1.0.0 runs, greeting hello, start 2")?;
    // 1.1.0 keeps her file and override, copies the data, and answers.
    install(vm, "1.1.0")?;
    vm.expect_screen(&[
        "installed org.example.lighthouse 1.1.0 in place of 1.0.0",
        " the data of its account lighthouse: kept a copy of the data 1.0.0",
        " its service lighthouse keeps the file an administrator changed",
        " its service lighthouse runs",
    ])?;
    vm.expect("lighthouse: lighthouse 1.1.0 runs, greeting hello, start 3")?;
    answer_prompt(vm, "$", "clear; service check lighthouse | tr ';' '\\n'")?;
    vm.expect_screen(&[" these settings differ: limits.cpu_weight"])?;
    // 2.0.0 never answers: back to 1.1.0, with the data it left, held.
    install(vm, "2.0.0")?;
    vm.expect("lighthouse: lighthouse 2.0.0 runs, start 4, and never answers")?;
    vm.expect_anywhere("org.example.lighthouse 2.0.0 failed its health check")?;
    vm.expect("lighthouse: lighthouse 1.1.0 runs, greeting hello, start 4")?;
    vm.expect_screen(&[
        " but its service lighthouse did not answer at",
        " so bundled went back to org.example.lighthouse 1.1.0",
        " it is held there until `bundle unhold org.example.lighthouse`",
    ])?;
    answer_prompt(
        vm,
        "$",
        &format!(
            "clear; elevate sh -c 'tr \"\\n\" \" \" < {DATA}/lighthouse/notes; echo; ls -m \
             {DATA}/.snapshots/lighthouse'; bundle history {ID} | grep -q 'failed its health check' && echo \"the history says why\""
        ),
    )?;
    vm.expect_screen(&["1.0.0 1.0.0 1.1.0 1.1.0", "1.0.0, 2.0.0", "the history says why"])?;
    vm.press_power_button()?;
    vm.expect("spark: every service has stopped")?;
    vm.expect_exit(QEMU_EXIT_POWER_OFF)
}
