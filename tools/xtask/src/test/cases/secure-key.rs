// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The secure key, Ctrl+Alt+Delete (ADR 0708), on the machine of the
//! test `input-follow`. display-test borrows the screens from consoled
//! and ties descriptions of the input files to its holder. Ctrl and Alt
//! go down on the USB keyboard and reach the program; Delete then
//! reaches nobody: the kernel sees the three keys, puts the consoles in
//! front of the program, and from that moment only consoled reads a
//! key. The program is told that it was passed over, its descriptions
//! read the release of Ctrl and Alt and `paused`, and neither they nor
//! a description tied to nobody read what is typed while consoled's
//! menu shows. Esc closes the menu and gives the program the front
//! back, and its descriptions read on, without the Esc that closed it.
//! Pressed again and answered with Enter, the key leaves the consoles
//! in front of the program, whose shell runs a command while the
//! program reads none of it; the key and Esc then give the program the
//! front back. With no program in front, the menu's P powers the
//! machine off.

use super::super::*;
use super::input_follow::{CMDLINE, MACHINE, Told, VIDEO, rel};

/// The test.
pub const CASE: Case = Case {
    name: "secure-key",
    about: "Ctrl+Alt+Delete is seen in the kernel (ADR 0708): it puts the consoles and their menu in front of a program that has the screens, and until the menu is closed no description of an input file but consoled's reads a key, tied to the program's holder or to none; Esc gives the program the front back, Enter keeps the consoles in front for a shell, and the menu's P powers the machine off",
    firmware: Firmware::Uefi,
    media: Media::Usb,
    cmdline: CMDLINE,
    cpus: 2,
    memory: "1G",
    reboot: false,
    image: TestImage::Plain,
    cpu_model: None,
    qemu: MACHINE,
    script: secure_key,
    disks: &[],
    second: None,
};

/// consoled's menu, over a program that has the screens and with no
/// program behind.
const MENU_OVER: &str =
    "Ctrl+Alt+Delete    Enter: consoles    R: restart    P: power off    Esc: back";
const MENU: &str = "Ctrl+Alt+Delete    R: restart    P: power off    Esc: back";

/// Presses Ctrl+Alt+Delete on the USB keyboard: Ctrl (29) and Alt (56)
/// first, then Delete, and lets go of the three once `seen` has come of
/// it.
fn secure(vm: &mut Vm, seen: impl FnOnce(&mut Vm) -> Result<(), String>) -> Result<(), String> {
    vm.keys_on(VIDEO, &["ctrl", "alt"], true)?;
    std::thread::sleep(Duration::from_millis(100));
    vm.keys_on(VIDEO, &["delete"], true)?;
    let seen = seen(vm);
    vm.keys_on(VIDEO, &["delete", "alt", "ctrl"], false)?;
    seen
}

/// Closes consoled's menu with Esc, held down until the program has the
/// front again, so that its release comes after.
fn esc(vm: &mut Vm) -> Result<(), String> {
    vm.keys_on(VIDEO, &["esc"], true)?;
    let front = Told::says(vm, "input: ours is in front");
    vm.keys_on(VIDEO, &["esc"], false)?;
    front
}

fn secure_key(vm: &mut Vm, case: &Case) -> Result<(), String> {
    let mut told = Told::start(vm, case)?;
    told.tell(vm, "borrow", "input: ours has the screens")?;
    vm.type_on(VIDEO, "a")?;
    told.heard(vm, &[("kbd, tied to ours", "+30 -30")])?;
    // The key: the consoles come to the front, with their menu.
    secure(vm, |vm| Told::says(vm, "input: ours is passed over"))?;
    vm.expect_screen(&[MENU_OVER])?;
    // z on both keyboards and the mouse, while the menu shows: nobody
    // but consoled reads them, and the menu takes no z.
    vm.type_on(VIDEO, "z")?;
    vm.type_text("z")?;
    vm.input_events(VIDEO, &[rel("x", 7)])?;
    told.heard(
        vm,
        &[
            ("kbd, tied to nobody", "+29 +56 -29 -56"),
            ("mouse, tied to nobody", "nothing"),
            ("kbd, tied to ours", "+29 +56 -29 -56"),
            ("mouse, tied to ours", "nothing"),
            ("PS/2 keyboard, tied to ours", "paused"),
            ("QEMU USB Keyboard, tied to ours", "+29 +56 paused"),
            ("QEMU USB Mouse, tied to ours", "paused"),
        ],
    )?;
    // Esc closes the menu: the program is in front again and reads q
    // (16), and of the Esc (1) only what its keyboard's own file says.
    esc(vm)?;
    vm.type_on(VIDEO, "q")?;
    told.heard(
        vm,
        &[
            ("kbd, tied to nobody", "+16 -16"),
            ("kbd, tied to ours", "+16 -16"),
            ("PS/2 keyboard, tied to ours", "resumed"),
            ("QEMU USB Keyboard, tied to ours", "resumed -1 +16 -16"),
        ],
    )?;
    consoles_stay(vm, &mut told)?;
    // With no program in front, the key shows the menu all the same,
    // and P powers the machine off.
    told.tell(vm, "give", "input: the consoles have the screens")?;
    told.end(vm)?;
    secure(vm, |vm| vm.expect_screen(&[MENU]))?;
    vm.type_on(VIDEO, "p")?;
    vm.expect("consoled: Ctrl+Alt+Delete menu: powering off the machine")?;
    vm.expect("spark: every service has stopped")?;
    vm.expect_exit(QEMU_EXIT_POWER_OFF)
}

/// The key, answered with Enter: the consoles stay in front of the
/// program, and their shell runs a command of which the program reads
/// nothing. The key again, and Esc, give the program the front back.
fn consoles_stay(vm: &mut Vm, told: &mut Told) -> Result<(), String> {
    secure(vm, |vm| Told::says(vm, "input: ours is passed over"))?;
    vm.expect_screen(&[MENU_OVER])?;
    vm.type_on(VIDEO, "\n")?;
    vm.expect("consoled: Ctrl+Alt+Delete menu: the consoles stay in front")?;
    vm.type_on(VIDEO, "echo $((6 * 7)) held\n")?;
    vm.expect_screen(&["# echo $((6 * 7)) held", "42 held"])?;
    told.heard(
        vm,
        &[
            ("kbd, tied to nobody", "+29 +56 -29 -56"),
            ("kbd, tied to ours", "+29 +56 -29 -56"),
            ("QEMU USB Keyboard, tied to ours", "+29 +56 paused"),
        ],
    )?;
    // consoled, in front already, reads this one itself.
    secure(vm, |vm| vm.expect_screen(&[MENU_OVER]))?;
    esc(vm)?;
    vm.type_on(VIDEO, "q")?;
    told.heard(
        vm,
        &[
            ("kbd, tied to ours", "+16 -16"),
            ("QEMU USB Keyboard, tied to ours", "resumed -1 +16 -16"),
        ],
    )
}
