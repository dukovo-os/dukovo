// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The install stick's *Install Dukovo with speech* entry, as the live
//! menu starts it, with *install speech* on the command line: the
//! installer's screens say what they show to the screen reader, which
//! readerd's log follows (ADR 0422). The Welcome page's Screen reader
//! box starts checked, stops the reader unchecked and starts it again
//! checked, which says the page whole; its Text size and Sticky keys
//! change the consoles at once, as consoled's log shows. Each page is then answered by keys,
//! and each field is said as the focus reaches it, but a password's
//! characters never are: only "star". The second boot is from the disk
//! alone, whose consoles start with the text larger, sticky keys on and
//! the screen reader speaking, from the settings the install wrote.

use super::super::*;
use super::install_uefi::{INSTALL_DISKS, wait_for_screen};

pub const CASE: Case = Case {
    name: "install-speech",
    about: "The installer with speech says each page, field and key to the screen reader, never a password's characters; its Welcome page makes the text larger and turns sticky keys on at once; the installed system starts with them and with its screen reader on",
    firmware: Firmware::Uefi,
    media: Media::Usb,
    // As the Install Dukovo with speech entry gives it, with readerd's log
    // of what it says.
    cmdline: "install speech reader-log",
    cpus: 2,
    memory: "1G",
    reboot: false,
    image: TestImage::Live,
    cpu_model: None,
    // An output for the speech, played to nothing, and one network card,
    // so that the network page's fields are in a known order.
    qemu: &[
        "-audiodev",
        "none,id=snd0",
        "-device",
        "virtio-sound-pci,audiodev=snd0",
        "-netdev",
        "user,id=n0",
        "-device",
        "virtio-net-pci,netdev=n0",
    ],
    script: with_speech,
    disks: INSTALL_DISKS,
    second: Some(SecondBoot { cmdline: "", before: super::install_uefi::nothing, script: installed }),
};

/// alice's password, typed into the screens.
const PASSWORD: &str = "queen of hearts";

/// How long the install may take, in seconds: it copies the whole system.
const INSTALL_SECONDS: u64 = 240;

/// The first boot, from the stick: the screens, heard and answered by
/// keys.
fn with_speech(vm: &mut Vm, _case: &Case) -> Result<(), String> {
    vm.expect_anywhere("consoled: the screen reader is listening")?;
    vm.expect_anywhere("consoled: started /system/commands/dukovo-install on tty1 as process ")?;
    // The startup screen's checks, then the first page whole.
    vm.expect_anywhere("readerd: says: Checking the signed system release")?;
    vm.expect(&format!(
        "readerd: says: Install Dukovo {VERSION}. Welcome, step 1 of 6. This installs Dukovo"
    ))?;
    // The screen is read as ASCII: the list's marks, as the focus's, are
    // not.
    vm.expect_screen(&["Step 1 of 6", "[x] Screen reader", "Normal"])?;

    // Screen reader, the sixth field: unchecked, it stops the reader;
    // checked again, it starts it, not for the kernel flag but as the
    // settings now say, and the page is said whole.
    vm.press(&["down"; 5])?;
    vm.expect("readerd: says: Screen reader, check box, checked.")?;
    vm.press(&["spc"])?;
    vm.expect("consoled: the screen reader went away")?;
    vm.expect_screen(&["[ ] Screen reader"])?;
    let from = vm.log().len();
    vm.press(&["spc"])?;
    vm.expect("readerd: speaking with eSpeak NG's en voice at 175 words a minute")?;
    if vm.log()[from..].contains("for the kernel flag speech") {
        return Err("the Screen reader box started the reader for the kernel flag".into());
    }
    vm.expect("consoled: the screen reader is listening")?;
    vm.expect("readerd: says: Console 1")?;
    vm.expect(&format!("readerd: says: Install Dukovo {VERSION}. Welcome, step 1 of 6."))?;
    vm.expect_screen(&["[x] Screen reader"])?;

    // Text size, the next field: larger at once, the page drawn again at
    // the screen's new size.
    vm.press(&["down"])?;
    vm.expect("readerd: says: Text size, list, Normal. Ctrl+Alt+Plus and Minus change it anywhere.")?;
    vm.press(&["right"])?;
    vm.expect("consoled: text size larger")?;
    // A step above the live system's normal 12x24.
    vm.expect("consoled: console of 80x25 cells with font 16x32 at (0, 0)")?;
    vm.expect_anywhere("readerd: says: Larger.")?;
    // The page no longer fits: the focused field shows, with "more"
    // below it.
    vm.expect_screen(&["Larger", "more"])?;

    // Sticky keys, the next field: on at once.
    vm.press(&["down"])?;
    vm.expect("readerd: says: Sticky keys, check box, not checked.")?;
    vm.press(&["spc"])?;
    vm.expect("consoled: sticky keys on")?;
    vm.expect_anywhere("readerd: says: checked.")?;
    vm.expect_screen(&["[x] Sticky keys"])?;

    // Next, three fields on: the Network page, said whole.
    vm.press(&["down", "down", "down", "ret"])?;
    vm.expect("readerd: says: Network and name, step 2 of 6.")?;
    vm.press(&["down", "down", "down"])?;
    vm.expect("readerd: says: Computer name, edit, dukovo.")?;
    vm.press(&["backspace"])?;
    vm.expect("readerd: says: o.")?;
    vm.press(&["ret"])?;

    // Disk: the blank disk, as its choice; Software: Minimal, by Enter
    // on the first profile.
    vm.expect("readerd: says: Disk, step 3 of 6.")?;
    vm.press(&["spc", "ret"])?;
    vm.expect("readerd: says: Software, step 4 of 6.")?;
    vm.press(&["ret"])?;

    // You: the name is said as it is typed; the password never is.
    vm.expect("readerd: says: You, step 5 of 6.")?;
    vm.type_text("Alice Liddell")?;
    vm.press(&["tab", "tab"])?;
    vm.expect("readerd: says: Password, password, blank.")?;
    let from = vm.log().len();
    vm.type_text(PASSWORD)?;
    vm.expect("readerd: says: star.")?;
    vm.press(&["tab"])?;
    vm.expect("readerd: says: Repeat password, password, blank.")?;
    vm.type_text(PASSWORD)?;
    vm.press(&["ret"])?;
    vm.expect("readerd: says: Review, step 6 of 6.")?;
    let log = vm.log();
    let heard = log[from..].lines().filter_map(|line| line.split_once("readerd: says: "));
    for (_, said) in heard.take_while(|(_, said)| !said.starts_with("Review")) {
        if said != "star." && !said.contains("password, blank") {
            return Err(format!("the screen reader heard a password's character: {said:?}"));
        }
    }

    // The review lists them, and the install runs to Done.
    // The screen is read as ASCII, and the line wraps at the larger
    // size: its words one by one.
    vm.expect_screen(&["screen reader", "larger", "sticky keys", "Type vda to confirm"])?;
    vm.type_text("vda")?;
    vm.press(&["ret"])?;
    vm.expect("readerd: says: Installing Dukovo.")?;
    // The steps as they start. A step may be said with the one before it,
    // if both started at once, and the last ones not at all when the Done
    // page comes in the same draw, as it does under KVM: copying the
    // release, which takes seconds, is said.
    vm.expect("Copying Dukovo")?;
    wait_for_screen(vm, "Remove the install stick, then restart.", INSTALL_SECONDS)?;
    vm.expect("readerd: says: Dukovo is installed.")?;

    vm.press(&["alt-f2"])?;
    vm.expect_screen(&["# "])?;
    vm.type_text("poweroff\n")?;
    vm.expect("spark: asked to power off the machine")?;
    vm.expect_exit(QEMU_EXIT_POWER_OFF)
}

/// The second boot, from the installed disk alone: the consoles start
/// with the text larger and sticky keys on, and the screen reader with
/// them, from the settings the install wrote, not a kernel flag.
fn installed(vm: &mut Vm, _case: &Case) -> Result<(), String> {
    vm.expect("spark: /system is Dukovo's root on ")?;
    // A step above the installed system's normal 8x16, where the live
    // system's is 12x24.
    vm.expect_anywhere("consoled: text size larger")?;
    vm.expect_anywhere("consoled: console of 106x33 cells with font 12x24 at (4, 4)")?;
    vm.expect_anywhere("consoled: sticky keys on")?;
    vm.expect_anywhere("readerd: speaking with eSpeak NG's en voice at 175 words a minute")?;
    if vm.log().contains("for the kernel flag speech") {
        return Err("the installed system's screen reader started for a kernel flag".into());
    }
    vm.expect_anywhere("consoled: the screen reader is listening")?;
    vm.expect_screen(&[&format!("{RELEASE} {VERSION}")])?;
    answer_prompt(vm, "Name:", "alice")?;
    answer_prompt(vm, "Password:", PASSWORD)?;
    vm.expect_anywhere("authd: alice signed in on pts/")?;
    answer_prompt(
        vm,
        "$",
        "echo \"set\"\": $(grep -c -e 'text-size = \"larger\"' -e 'sticky-keys = true' \
         /system/settings/console/accessibility.toml)\"",
    )?;
    vm.expect_screen(&["set: 2"])?;
    vm.press_power_button()?;
    vm.expect("spark: every service has stopped")?;
    vm.expect_exit(QEMU_EXIT_POWER_OFF)
}
