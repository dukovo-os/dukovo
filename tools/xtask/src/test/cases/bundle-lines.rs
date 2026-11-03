// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! A bundle's lines keep its major versions side by side (ADR 0387), and
//! each person's add-ons stay with the line they were added with (ADR
//! 0388). A second disk holds a catalog that ob signed with throwaway
//! keys, which lists tide's line 1, in 1.0.0 and 1.1.0, and its line 2, in
//! 2.0.0, and the first version of each line as a file. Alice adds the
//! catalog and installs tide by its ID, which takes line 2, then line 1's
//! file, which installs beside it. tide runs the newest line, and tide-1
//! always line 1. With line 1's tidepm, which runs with her access once she
//! allows it, she adds the command wave, which bundled links beside her
//! commands, and which runs tide's line 1 while plain tide runs line 2; an
//! add-on named as a system command is not linked. Her default runs once
//! she sets it, and the line a project's .tide-version names beats it, in
//! its folder and below. A version no line installed has stops tide,
//! saying so. An update takes line 1 to 1.1.0, wave with it, and leaves
//! line 2 as it is; removing line 2 leaves line 1 running, and removing
//! line 1 with its data takes wave and its add-ons away. The power button
//! ends it.

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
    about: "a bundle's lines keep its major versions side by side (ADR 0387), with each person's add-ons (ADR 0388): two lines of tide install beside each other; tide runs the newest, tide-1 line 1, then her default, then the line a project's .tide-version names; a version not installed stops it; an add-on added with line 1 is linked and runs line 1; an update stays in its line; removing a line leaves the other, and with its data takes its add-ons",
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
    // An add-on of line 1, which runs line 1; not one named as a system
    // command.
    answer_prompt(
        vm,
        "$",
        "clear; tidepm-1 add wave; tidepm-1 add ls > /dev/null; echo \"wave: $(wave)\"; echo \
         \"plain: $(tide)\"; echo \"ls: $(test -e applications/commands/ls && echo linked || echo \
         not linked)\"",
    )?;
    vm.expect_screen(&["[warning] ", "A: always"])?;
    vm.type_text("a")?;
    vm.expect_anywhere("bundled: alice allowed org.example.tide@1 runs-with-your-access")?;
    vm.expect_screen(&[
        "added wave to /users/alice/applications/add-ons/org.example.tide@1",
        "wave: wave, on tide 1.0.0",
        "plain: tide 2.0.0",
        "ls: not linked",
    ])?;
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
        "clear; bundle update --yes | tail -1; echo \"one: $(tide-1)\"; echo \"wave: $(wave)\"; \
         bundle list | grep tide",
    )?;
    vm.expect_screen(&[
        "1 updated, 0 not, 1 up to date",
        "one: tide 1.1.0",
        "wave: wave, on tide 1.1.0",
        "org.example.tide@1",
        "org.example.tide@2",
    ])?;
    // Removing line 2 leaves line 1, and takes tide-2 away.
    answer_prompt(
        vm,
        "$",
        "clear; bundle default tide --clear; bundle remove --keep-data org.example.tide@2; echo \
         \"left: $(tide)\"; echo \"tide-2: $(command -v tide-2 || echo none)\"",
    )?;
    vm.expect_screen(&[
        "org.example.tide has no default line for you now",
        "removed org.example.tide@2 2.0.0",
        "left: tide 1.1.0",
        "tide-2: none",
    ])?;
    // Removing line 1 with its data takes its add-ons, and wave.
    answer_prompt(
        vm,
        "$",
        "clear; bundle remove --purge org.example.tide@1 | tr ',' '\\n'; echo \"wave: $(command -v \
         wave || echo none)\"; echo \"add-ons: $(ls applications/add-ons).\"",
    )?;
    vm.expect_screen(&[
        "removed org.example.tide@1 1.1.0",
        " and its add-ons in /users/alice/applications/add-ons/org.example.tide@1",
        "wave: none",
        "add-ons: .",
    ])?;
    vm.press_power_button()?;
    vm.expect("spark: every service has stopped")?;
    vm.expect_exit(QEMU_EXIT_POWER_OFF)
}
