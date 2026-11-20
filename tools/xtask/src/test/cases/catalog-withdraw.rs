// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! A catalog withdraws versions of its bundles, and machines leave them
//! (ADR 0593). A second disk holds a catalog that ob signed with
//! throwaway keys, listing versions 1.0.0 and 1.1.0 of two small test
//! bundles, tally and spinner, twice: as it was, and after it withdrew
//! tally 1.1.0 as broken and spinner 1.0.0 as malicious. Alice adds the
//! catalog from a copy of the first, installs tally 1.0.0 and updates it
//! to 1.1.0, installs spinner 1.0.0 and runs it, and the copy is replaced
//! by the second. The next read of the catalog writes both withdrawals
//! in the audit log: bundled goes back to tally 1.0.0, and stops spinner,
//! whose run is told to end. bundle list says spinner is stopped, and
//! running it fails; bundle history says tally 1.1.0 was withdrawn and
//! bundled went back; the file of tally 1.1.0 is refused, and so is
//! going back to it; bundle update takes spinner's fix. The power button
//! ends it.

use super::super::*;

/// The catalog's disk's label.
const LABEL: &str = "WITHDRAW";

/// Where Spark mounts it: the second virtio disk's name.
const VOLUME: &str = "/volumes/vdb";

/// Dukovo's root on a copy of the test image, and the catalog.
const DISKS: &[Disk] = &[
    Disk { label: image::ROOT_LABEL, ..btrfs_disk(Bus::Virtio, 512, ROOT, root_checked) },
    withdraw_site_disk(32 * 1024, LABEL),
];

/// The test.
pub const CASE: Case = Case {
    name: "catalog-withdraw",
    about: "a catalog withdraws versions of its bundles, and machines leave them (ADR 0593): the catalog withdraws one version of a bundle as broken and one of another as malicious; on the next read of the catalog bundled goes back from the broken one to the version kept, and stops the malicious one, whose run is told to end; bundle list says it is stopped and running it fails; bundle history says what happened; the withdrawn files and going back to a withdrawn version are refused; bundle update takes the fixed version",
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
    script: catalog_withdraw,
    disks: DISKS,
    second: None,
};

fn catalog_withdraw(vm: &mut Vm, case: &Case) -> Result<(), String> {
    owner_signs_in(vm, case)?;
    vm.expect_anywhere("bundled: 0 bundles installed; answering at /system/live/bundled/socket")?;
    vm.expect_anywhere(&format!("spark: mounted the FAT filesystem of vdb at {VOLUME}"))?;
    // The catalog as it was, in a folder that the test replaces.
    answer_prompt(
        vm,
        "$",
        &format!("clear; cp -R {VOLUME}/before $HOME/site; echo copied-$((6 * 7))"),
    )?;
    vm.expect_screen(&["copied-42"])?;
    answer_prompt(
        vm,
        "$",
        &format!("clear; elevate bundle catalog add test file://$HOME/site {VOLUME}/root.json"),
    )?;
    answer_prompt(vm, &format!("{VOLUME}/root.json:"), "queen of hearts")?;
    vm.expect_screen(&["added catalog test, which lists 4 bundles"])?;
    answer_prompt(
        vm,
        "$",
        &format!(
            "clear; cd {VOLUME}; bundle install --yes org.example.tally-1.0.0.dkv | tail -1; \
             bundle install --yes org.example.spinner-1.0.0.dkv | tail -1; cd"
        ),
    )?;
    vm.expect_screen(&[
        "installed org.example.tally 1.0.0, for you",
        "installed org.example.spinner 1.0.0, for you",
    ])?;
    // No rollouts are signed, so an update is taken at once.
    answer_prompt(vm, "$", "clear; tally; bundle update --yes org.example.tally")?;
    vm.expect_screen(&[
        "tally 1.0.0 ran",
        "installed org.example.tally 1.1.0 in place of 1.0.0, for you",
    ])?;
    answer_prompt(vm, "$", "clear; tally")?;
    vm.expect_screen(&["tally 1.0.0 ran", "tally 1.1.0 ran"])?;
    // spinner runs on, as a job of this shell, until it is withdrawn. What
    // it says when it is stopped goes to a file: on the screen it could
    // come after a prompt and wrap past column 80.
    answer_prompt(vm, "$", "clear; spinner 2>spinner-said & sleep 3; echo started-$((6 * 7))")?;
    vm.expect_screen(&["spinner 1.0.0 spins", "started-42"])?;
    // The catalog as it is after the withdrawals; reading it tells bundled.
    answer_prompt(vm, "$", &format!("clear; rm -r $HOME/site; cp -R {VOLUME}/after $HOME/site"))?;
    answer_prompt(vm, "$", "clear; bundle update --check")?;
    vm.expect_anywhere(
        "catalog test withdrew org.example.tally 1.1.0 as broken: it loses tallies",
    )?;
    vm.expect_anywhere(
        "catalog test withdrew org.example.spinner 1.0.0 as malicious: it sends the keys away",
    )?;
    vm.expect_anywhere(
        "bundled stopped org.example.spinner 1.0.0 for alice, which catalog test withdrew as \
         malicious: it sends the keys away; 1 running told to end",
    )?;
    vm.expect_anywhere("bundled went back to org.example.tally 1.0.0 for alice, from 1.1.0")?;
    // The run ends, and says why.
    answer_prompt(vm, "$", "clear; sleep 1; cat spinner-said")?;
    vm.expect_screen(&[
        "bundle: org.example.spinner was stopped: catalog test withdrew 1.0.0: it sends",
    ])?;
    // The NAME column starts at 49 here: its row's end, from the name on,
    // fits in 80 columns.
    answer_prompt(
        vm,
        "$",
        "clear; bundle list | cut -c1-40; bundle list | grep spinner | cut -c49-",
    )?;
    vm.expect_screen(&[
        "org.example.spinner  1.0.0    you",
        "org.example.tally    1.0.0    you",
        "spinner, stopped by catalog test: it sends",
    ])?;
    answer_prompt(vm, "$", "clear; spinner")?;
    vm.expect_screen(&["org.example.spinner 1.0.0 was stopped by catalog test: it sends"])?;
    vm.expect_screen_without("spinner 1.0.0 spins")?;
    // tally went back to 1.0.0 with the data 1.0.0 left (ADR 0384), and
    // its history says so.
    answer_prompt(vm, "$", "clear; tally")?;
    vm.expect_screen(&["org.example.tally: brought back the data 1.0.0 left", "tally 1.0.0 ran"])?;
    vm.expect_screen_without("tally 1.1.0 ran")?;
    answer_prompt(vm, "$", "clear; bundle history org.example.tally")?;
    vm.expect_screen(&[
        "1.1.0 was withdrawn: broken, by catalog",
        "went back to 1.0.0, from 1.1.0",
    ])?;
    // What was withdrawn is not installed, nor gone back to.
    answer_prompt(
        vm,
        "$",
        &format!("clear; bundle install --yes {VOLUME}/org.example.tally-1.1.0.dkv"),
    )?;
    vm.expect_screen(&["org.example.tally 1.1.0 was withdrawn by catalog test on"])?;
    answer_prompt(vm, "$", "clear; bundle rollback org.example.tally")?;
    vm.expect_screen(&["org.example.tally 1.1.0 was withdrawn by catalog test: it loses"])?;
    // A version that is not withdrawn takes the stopped one's place.
    answer_prompt(vm, "$", "clear; bundle update --yes org.example.spinner")?;
    vm.expect_screen(&["installed org.example.spinner 1.1.0 in place of 1.0.0, for you"])?;
    answer_prompt(vm, "$", "clear; spinner")?;
    vm.expect_screen(&["spinner 1.1.0 is fixed"])?;
    vm.press_power_button()?;
    vm.expect("spark: every service has stopped")?;
    vm.expect("Power: `spark` asked to power off the machine; syncing the filesystems")?;
    vm.expect_exit(QEMU_EXIT_POWER_OFF)
}
