// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! A virtual machine's own input devices: a virtio keyboard, mouse and
//! tablet, driven from the virtio specification (ADR 0706), beside the
//! PS/2 keyboard. The kernel's log names each with its file. display-test
//! (`display-test input`, as in `input-follow`) borrows the screens and
//! says what each device is: two keyboards, a pointer and a tablet, each
//! at its PCI address. A key on the virtio keyboard is read on its own
//! file and on _/devices/kbd_, and not on the PS/2 keyboard's. The mouse
//! reads its motion, its left button, and a notch of its wheel as 120;
//! the tablet reads where it points, in its own range on its own file
//! and as 0 to 65,535 on _/devices/mouse_. Given back, the consoles are
//! typed into on the virtio keyboard, and `hardware inputs` lists the
//! three as a virtual machine's own, which `hardware show` counts with
//! the PS/2 keyboard.

use super::super::*;
use super::input_follow::{CMDLINE, Told, VIDEO, abs, button, rel};

/// The test.
pub const CASE: Case = Case {
    name: "input-devices",
    about: "a virtual machine's virtio keyboard, mouse and tablet (ADR 0706): each is a file of its own that says what it is, a key reads on the keyboard's file and on /devices/kbd, the mouse reads its motion, a button and a notch of its wheel as 120, the tablet reads where it points, the consoles are typed into on the virtio keyboard, and hardware inputs lists the three, which hardware show counts",
    firmware: Firmware::Uefi,
    media: Media::Usb,
    cmdline: CMDLINE,
    cpus: 2,
    memory: "1G",
    reboot: false,
    image: TestImage::Plain,
    cpu_model: None,
    // The keyboard and the mouse are bound to the display, so that the
    // events sent to it reach them; where a pointer points reaches the
    // tablet, the one device that takes it. The tablet has one interrupt
    // for its queues and a change of its configuration alike, as a host
    // may give a device; the other two have one for each.
    qemu: &[
        "-vga",
        "none",
        "-device",
        "VGA,id=video0",
        "-device",
        "virtio-keyboard-pci,display=video0",
        "-device",
        "virtio-mouse-pci,display=video0",
        "-device",
        "virtio-tablet-pci,vectors=1",
        "-chardev",
        SERIAL_LINE[0],
        "-device",
        "isa-serial,index=1,chardev=line0",
    ],
    script: input_devices,
    disks: &[],
    second: None,
};

/// The three devices, as they name themselves.
const KEYBOARD: &str = "QEMU Virtio Keyboard";
const MOUSE: &str = "QEMU Virtio Mouse";
const TABLET: &str = "QEMU Virtio Tablet";

fn input_devices(vm: &mut Vm, case: &Case) -> Result<(), String> {
    let found = [KEYBOARD, MOUSE, TABLET].map(|name| format!("Input: {name} at "));
    let found: Vec<&str> = found.iter().map(String::as_str).collect();
    let mut told = Told::start_with(vm, case, &found)?;
    // Each is a device of its own, and says what it is and where.
    let said = told.tell(vm, "borrow", "input: ours has the screens")?;
    for device in [
        "PS/2 keyboard is keyboard, at ps2".to_string(),
        format!("{KEYBOARD} is keyboard, at pci-"),
        format!("{MOUSE} is pointer, at pci-"),
        format!("{TABLET} is tablet, at pci-"),
    ] {
        if !said.contains(&format!("input: device {device}")) {
            return Err(format!("display-test did not say `input: device {device}`:\n{said}"));
        }
    }
    // a (30) on the keyboard; the mouse moves, its left button (272)
    // goes down and up, and its wheel turns a notch away; the tablet
    // points at the right edge, halfway down.
    vm.type_on(VIDEO, "a")?;
    vm.input_events(VIDEO, &[rel("x", 10), rel("y", -5)])?;
    vm.input_events(VIDEO, &[button("left", true)])?;
    vm.input_events(VIDEO, &[button("left", false)])?;
    vm.input_events(VIDEO, &[button("wheel-up", true)])?;
    vm.input_events(VIDEO, &[abs("x", 0x7fff), abs("y", 0x4000)])?;
    let (keyboard, mouse, tablet) = (
        format!("{KEYBOARD}, tied to ours"),
        format!("{MOUSE}, tied to ours"),
        format!("{TABLET}, tied to ours"),
    );
    told.heard(
        vm,
        &[
            ("kbd, tied to ours", "+30 -30"),
            (&keyboard, "+30 -30"),
            ("PS/2 keyboard, tied to ours", "nothing"),
            (&mouse, "rel0:10 rel1:-5 +272 -272 rel11:120"),
            // The tablet's own file says what the tablet says, in the
            // tablet's range, which is 0 to 32767. /devices/mouse gives
            // every tablet's place as 0 to 65535.
            (&tablet, "abs0:32767 abs1:16384"),
            (
                "mouse, tied to ours",
                "10,-5,0,0,0,0,0,0 0,0,0,0,0,0,1,0 0,0,0,0,0,0,0,0 0,0,1,0,0,0,0,0 \
                 0,0,0,0,65535,32768,0,1",
            ),
        ],
    )?;
    told.tell(vm, "give", "input: the consoles have the screens")?;
    told.end(vm)?;
    on_the_consoles(vm)
}

/// The consoles' shell is typed into on the virtio keyboard, and the
/// commands list the three devices.
fn on_the_consoles(vm: &mut Vm) -> Result<(), String> {
    vm.type_on(VIDEO, "clear; hardware inputs | grep -e Virtio -e virtual\n")?;
    vm.expect_screen(&[KEYBOARD, MOUSE, TABLET, "  Connected: virtual, "])?;
    let screen = vm.screen()?;
    let virtual_ones = screen.iter().filter(|line| line.contains("Connected: virtual, ")).count();
    if virtual_ones != 3 {
        return Err(format!("hardware inputs lists {virtual_ones} virtual devices, not 3"));
    }
    vm.type_on(VIDEO, "hardware show | grep Inputs\n")?;
    vm.expect_screen(&["Inputs     4 devices"])?;
    vm.type_on(VIDEO, "poweroff\n")?;
    vm.expect("spark: asked to power off the machine")?;
    vm.expect("spark: every service has stopped")?;
    vm.expect_exit(QEMU_EXIT_POWER_OFF)
}
