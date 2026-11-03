// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! A bundle's lines keep its major versions side by side (ADR 0387). A
//! second disk holds a catalog that ob signed with throwaway keys, which
//! lists tide's line 1, in 1.0.0 and 1.1.0, and its line 2, in 2.0.0,
//! and the first version of each line as a file. Alice adds the catalog
//! and installs tide by its ID, which takes line 2, then line 1's file,
//! which installs beside it. tide runs the newest line, and tide-1 always
//! line 1; her default runs once she sets it, and the line a project's
//! .tide-version names beats it, in its folder and below. A version no
//! line installed has stops tide, saying so. An update takes line 1 to 1.1.0
//! and leaves line 2 as it is, and removing line 2 leaves line 1 running.
//! The power button ends it.

use super::super::*;

/// The catalog's disk's label.
const LABEL: &str = "LINES";

/// Where Spark mounts it: the second virtio disk's name.
const VOLUME: &str = "/volumes/vdb";

/// Dukovo's root on a copy of the test image, and the catalog.
const DISKS: &[Disk] = &[
    Disk { label: image::ROOT_LABEL, ..btrfs_disk(Bus::Virtio, 512, ROOT, root_checked) },
    lines_site_disk(16 * 1024, LABEL),
];

/// The test.
pub const CASE: Case = Case {
    name: "bundle-lines",
    about: "a bundle's lines keep its major versions side by side (ADR 0387): two lines of tide install beside each other; tide runs the newest, tide-1 line 1, then her default, then the line a project's .tide-version names; a version not installed stops it; an update stays in its line; removing a line leaves the other",
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
    script: lines,
    disks: DISKS,
    second: None,
};

fn lines(vm: &mut Vm, case: &Case) -> Result<(), String> {
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
    vm.expect_screen(&["added catalog test, which lists 3 bundles"])?;
    // By its ID, the newest line; line 1's file installs beside it.
    answer_prompt(
        vm,
        "$",
        &format!(
            "clear; bundle install --yes org.example.tide | tail -1; cd {VOLUME}; bundle install \
             --yes org.example.tide-1.0.0.dkv | tail -1; cd"
        ),
    )?;
    vm.expect_screen(&[
        "installed org.example.tide@2 2.0.0, for you",
        "installed org.example.tide@1 1.0.0, for you",
    ])?;
    // The newest runs, and each line by its own name.
    answer_prompt(
        vm,
        "$",
        "clear; echo \"plain: $(tide)\"; echo \"one: $(tide-1)\"; echo \"two: $(tide-2)\"",
    )?;
    vm.expect_screen(&["plain: tide 2.0.0", "one: tide 1.0.0", "two: tide 2.0.0"])?;
    // Her default, then a project's file, in its folder and below.
    answer_prompt(vm, "$", "clear; bundle default tide 1; echo \"default: $(tide)\"")?;
    vm.expect_screen(&["1 is your default line of org.example.tide", "default: tide 1.0.0"])?;
    answer_prompt(
        vm,
        "$",
        "clear; mkdir -p p/q; echo 2.0.3 > p/.tide-version; cd p/q; echo \"here: $(tide)\"; \
         bundle default tide; cd",
    )?;
    vm.expect_screen(&[
        "here: tide 2.0.0",
        "org.example.tide: 2 runs here, as /users/alice/p/.tide-version asks",
        "lines installed: 1, 2",
        "your default: 1",
    ])?;
    // A version no line installed has stops it.
    answer_prompt(
        vm,
        "$",
        "clear; echo v3 > p/.tide-version; cd p; tide; echo \"status $?\"; cd; rm -r p",
    )?;
    vm.expect_screen(&["tide: this project uses org.example.tide 3, as", "status 1"])?;
    vm.expect_screen_without("tide 2.0.0")?;
    // An update stays in its line.
    answer_prompt(
        vm,
        "$",
        "clear; bundle update --yes | tail -1; echo \"one: $(tide-1)\"; bundle list | grep tide",
    )?;
    vm.expect_screen(&[
        "1 updated, 0 not, 1 up to date",
        "one: tide 1.1.0",
        "org.example.tide@1",
        "org.example.tide@2",
    ])?;
    // Removing line 2 leaves line 1, and takes tide-2 away.
    answer_prompt(
        vm,
        "$",
        "clear; bundle default tide --clear; bundle remove --keep-data org.example.tide@2; echo \
         \"left: $(tide)\"; command -v tide-2 || echo \"tide-2 is gone\"",
    )?;
    vm.expect_screen(&[
        "org.example.tide has no default line for you now",
        "removed org.example.tide@2 2.0.0",
        "left: tide 1.1.0",
        "tide-2 is gone",
    ])?;
    vm.press_power_button()?;
    vm.expect("spark: every service has stopped")?;
    vm.expect_exit(QEMU_EXIT_POWER_OFF)
}
