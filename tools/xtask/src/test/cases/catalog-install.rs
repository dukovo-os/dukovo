// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Bundles install from a catalog (ADR 0383). A second disk holds a
//! catalog that ob signed with throwaway keys, which lists ripgrep's
//! bundle alone, its first root, and the `.dkv` files of ripgrep and
//! bat. Alice, the owner, adds the catalog through elevate, with its
//! address on the disk and its root, and bundled reads it with the TUF
//! client. bundle catalog lists it after dukovo, the catalog the image
//! comes with (its first root is in catalog/metadata), which she then
//! turns off, so that nothing here reaches the network; bundle search
//! finds ripgrep in hers and not bat. The catalog also lists the known flaws of two test
//! advisories in crates ripgrep links (ADR 0389), which bundle search
//! flags. Installed by its ID, ripgrep's sheet says it comes from the
//! catalog, with no warning about its key, and names both flaws; rg runs,
//! and bundle list flags it too. The daily check, which turning updates
//! by themselves on starts at once, writes the flaws in the history and
//! keeps them for the next sign-in. bat's
//! file, which no catalog lists, shows the warning, and she says no.
//! ripgrep's own file from the disk installs as the catalog's. Once the
//! catalog is removed, nothing installs ripgrep, which is bound to it. The
//! power button ends it.
//!
//! The catalog vouches for ripgrep's publisher (ADR 0592). Before it is
//! added, Alice installs a file of ripgrep's ID signed by a key nobody
//! knows, and keeps a note in its data folder. A second such file, which
//! an administrator installs for everyone once the catalog is added, is
//! refused, naming the catalog. Alice's install of ripgrep by its ID
//! then warns that it replaces the app of that key, and sets its data
//! aside in `.set-aside` of her `app-data`, where the note is found. A
//! bundle of an ID no catalog lists moves to a new key with a rollover
//! file the old key signed, and a file the old key signs is refused
//! after.

use super::super::*;

/// The catalog's disk's label.
const LABEL: &str = "CATALOG";

/// Where Spark mounts it: the second virtio disk's name.
const VOLUME: &str = "/volumes/vdb";

/// The files of the publishers of other keys on the catalog's disk: two
/// of ripgrep's ID, and three of an ID no catalog lists.
const IMPOSTOR: &str = "io.github.burntsushi.ripgrep-0.0.1.dkv";
const IMPOSTOR_AGAIN: &str = "io.github.burntsushi.ripgrep-0.0.2.dkv";
const MOVER_FIRST: &str = "org.example.mover-1.0.0.dkv";
const MOVER_MOVED: &str = "org.example.mover-2.0.0.dkv";
const MOVER_OLD_KEY: &str = "org.example.mover-1.0.1.dkv";

/// Dukovo's root on a copy of the test image, and the catalog.
const DISKS: &[Disk] = &[
    Disk { label: image::ROOT_LABEL, ..btrfs_disk(Bus::Virtio, 512, ROOT, root_checked) },
    catalog_site_disk(64 * 1024, LABEL),
];

/// The test.
pub const CASE: Case = Case {
    name: "catalog-install",
    about: "bundles install from a catalog (ADR 0383): an administrator adds a catalog signed with throwaway keys, from a disk, and turns off the dukovo catalog the image comes with; bundle search finds what it lists, flagged with its known flaws (ADR 0389); ripgrep installs by its ID with no warning about its key, its sheet names its flaws, and it runs; bundle list flags it, and the daily check writes its flaws in the history; bat's file, which no catalog lists, shows the warning; ripgrep's own file installs as the catalog's; once the catalog is removed, nothing installs ripgrep; the catalog vouches for ripgrep's publisher (ADR 0592): a file of its ID that another key signed is refused for everyone, naming the catalog, and replaces what Alice had installed of that key, with its data set aside, when she installs ripgrep from the catalog; a bundle of another ID moves to a new key with a rollover file, and the old key's files are refused",
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
    // A file of ripgrep's ID that no catalog lists yet: its key is unknown,
    // and installing it binds the ID to that key for Alice. She keeps a
    // note in the data folder it has.
    answer_prompt(vm, "$", &format!("clear; bundle install {VOLUME}/{IMPOSTOR}"))?;
    vm.expect_screen(&[
        "ripgrep 0.0.1, for yourself",
        "  Warning: this computer does not know this key.",
    ])?;
    answer_prompt(vm, "Install it? [y/N]", "y")?;
    vm.expect_screen(&["installed io.github.burntsushi.ripgrep 0.0.1, for you"])?;
    answer_prompt(
        vm,
        "$",
        "clear; mkdir -p $HOME/app-data/io.github.burntsushi.ripgrep; echo kept > \
         $HOME/app-data/io.github.burntsushi.ripgrep/notes; echo \"made $?\"",
    )?;
    vm.expect_screen(&["made 0"])?;
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
    // The catalog the image comes with first, never read here, then hers.
    vm.expect_screen(&[
        "NAME",
        "dukovo  on   0 ",
        "https://updates.dukovo.org/catalog/",
        "test    on   1 ",
        &format!("file://{VOLUME}/catalog/"),
    ])?;
    // Turned off, so that what follows reads her catalog alone and never
    // the network; elevate does not ask again so soon.
    answer_prompt(vm, "$", "clear; elevate bundle catalog remove dukovo")?;
    vm.expect_screen(&["turned catalog dukovo off"])?;
    // What it lists, and what it does not.
    answer_prompt(vm, "$", "clear; bundle search RIP; bundle search bat; echo \"bat $?\"")?;
    vm.expect_screen(&[
        "VERSION  CATALOG  FLAWS    NAME",
        "io.github.burntsushi.ripgrep  15.2.0   test     2, high  ripgrep",
        "no bundle found",
        "bat 1",
    ])?;
    // The catalog vouches for ripgrep's publisher, so a file of its ID
    // that another key signed is refused where no one has installed the
    // ID yet, for everyone, and the message names the catalog.
    answer_prompt(
        vm,
        "$",
        &format!(
            "clear; elevate bundle install --for-everyone --yes {VOLUME}/{IMPOSTOR_AGAIN} 2>&1 \
             | grep -o '[b]elongs to the publisher catalog test vouches for: install it from test'"
        ),
    )?;
    vm.expect_screen(&["belongs to the publisher catalog test vouches for: install it from test"])?;
    // By its ID: fetched and checked by the client, and trusted as the
    // catalog's. The key Alice's file of the ID was signed by is not the
    // catalog's publisher's, so the sheet warns that this replaces it.
    answer_prompt(vm, "$", "clear; bundle install io.github.burntsushi.ripgrep")?;
    vm.expect_screen(&[
        "ripgrep 15.2.0, for yourself",
        "  From catalog test, which this computer trusts: it lists this very",
        "  file and vouches for the publisher of io.github.burntsushi.ripgrep.",
        "  Warning: io.github.burntsushi.ripgrep has been the app of key ",
        "  Installing this replaces that app: it is removed, and its data",
        "  is set aside, not deleted. From now on only catalog test",
        // Its flaws, by the catalog's list: the worse first.
        "  Warning: catalog test's list of ",
        " knows of 2 flaws",
        "  in the components it holds:",
        "    TEST-0001 (high) in regex-automata ",
        ", fixed in 999.0.0",
        "      A test flaw in regex-automata",
        "    TEST-0002 (low) in memchr ",
        ", which no version fixes yet",
    ])?;
    vm.expect_screen_without("does not know this key")?;
    answer_prompt(vm, "Install it? [y/N]", "y")?;
    vm.expect_screen(&["installed io.github.burntsushi.ripgrep 15.2.0"])?;
    vm.expect_anywhere("(from catalog test)")?;
    // Her note is in `.set-aside` of her data folder, in the folder named
    // for the ID and the day, and no longer where the app keeps its data.
    answer_prompt(
        vm,
        "$",
        "clear; cat $HOME/app-data/.set-aside/io.github.burntsushi.ripgrep-*/data/notes; test ! \
         -e $HOME/app-data/io.github.burntsushi.ripgrep/notes; echo \"moved $?\"",
    )?;
    vm.expect_screen(&["kept", "moved 0"])?;
    answer_prompt(vm, "$", "clear; rg --version | head -1")?;
    vm.expect_screen(&["[warning] ", "A: always"])?;
    vm.type_text("a")?;
    vm.expect_screen(&["ripgrep 15.2.0"])?;
    // Flagged where it is listed.
    answer_prompt(vm, "$", "clear; bundle list")?;
    vm.expect_screen(&[
        "FOR        FLAWS    NAME",
        "io.github.burntsushi.ripgrep  15.2.0   you        2, high  ripgrep",
    ])?;
    // The daily check, which turning updates by themselves on starts at
    // once, writes the flaws in the history, and the log.
    answer_prompt(vm, "$", "clear; bundle update --automatic on")?;
    vm.expect_anywhere(
        "bundled: io.github.burntsushi.ripgrep 15.2.0 for alice has known flaws: TEST-0001 \
         (high) in regex-automata ",
    )?;
    answer_prompt(
        vm,
        "$",
        "clear; bundle update --automatic off; bundle history | tail -1 | sed 's/^.*ripgrep  //' \
         | tr ',' '\\n'",
    )?;
    vm.expect_screen(&[
        "15.2.0 has known flaws: TEST-0001 (high) in regex-automata ",
        " TEST-0002 (low) in memchr ",
        " by itself",
    ])?;
    // And for the next sign-in (ADR 0504).
    answer_prompt(vm, "$", "clear; tr ':' '\\n' < /system/live/sign-in/bundled")?;
    vm.expect_screen(&[
        "bundled",
        " io.github.burntsushi.ripgrep 15.2.0 for alice has known flaws",
        " TEST-0001 (high) in regex-automata",
    ])?;
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
    vm.expect_screen(&[
        "installed io.github.burntsushi.ripgrep 15.2.0 in place of 15.2.0, for you",
    ])?;
    // An ID no catalog lists moves to a new key: its publisher signs the
    // second file with the new key and adds a rollover file, which the
    // first key signed, so the sheet says whose key it was and since when.
    answer_prompt(vm, "$", &format!("clear; bundle install {VOLUME}/{MOVER_FIRST}"))?;
    answer_prompt(vm, "Install it? [y/N]", "y")?;
    vm.expect_screen(&["installed org.example.mover 1.0.0, for you"])?;
    answer_prompt(vm, "$", &format!("clear; bundle install {VOLUME}/{MOVER_MOVED}"))?;
    vm.expect_screen(&[
        "mover 2.0.0, for yourself",
        "  A new key for org.example.mover: its key since ",
        "  which signed that it moved to this one on 2026-10-26. From now on only",
        "  this key may install it.",
    ])?;
    vm.expect_screen_without("does not know this key")?;
    answer_prompt(vm, "Install it? [y/N]", "y")?;
    vm.expect_screen(&["installed org.example.mover 2.0.0 in place of 1.0.0, for you"])?;
    // The ID is the new key's from now on: the first key's file is refused.
    answer_prompt(
        vm,
        "$",
        &format!(
            "clear; bundle install --yes {VOLUME}/{MOVER_OLD_KEY} 2>&1 | grep -o '[o]nly its \
             publisher.s key may install it'"
        ),
    )?;
    vm.expect_screen(&["only its publisher's key may install it"])?;
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
