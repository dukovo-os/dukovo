// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! inputd(8), which hands the input devices to the program that has the
//! screens (ADR 0707), on the machine of `input-follow`. display-test
//! asks it as three programs it refuses: one that sends no holder, one
//! that sends a file that is no holder of the screens, and a holder that
//! speaks a version inputd does not know. Having borrowed the screens
//! from consoled, it asks as their holder, with a copy of its
//! description that can only be passed on, and is handed the PS/2
//! keyboard, the USB keyboard and the USB mouse, each with what it is.
//! What it was handed reads the keys while its holder is in front, and
//! `paused` once the screens are lent on. A tablet plugged in is handed
//! out as it comes and reads where it points; pulled out, it is said to
//! have gone, and its description reads nothing more. Then the test
//! reads inputd's own account in _/processes_: it holds less than 2 MiB
//! of memory of its own, and through ten seconds of typing and of a
//! mouse moving it was not woken once.

use super::super::*;
use super::input_follow::{CMDLINE, MACHINE, Told, VIDEO, abs, rel};

/// The test.
pub const CASE: Case = Case {
    name: "inputd",
    about: "inputd hands the input devices to the holder of the screens (ADR 0707): it refuses a program that sends no holder, a file that is no holder and a version it does not know; it hands a holder each device with what it is, tied to that holder, then a tablet as it is plugged in, and says when it is pulled out; it holds less than 2 MiB of its own and is not woken by a key or a motion",
    firmware: Firmware::Uefi,
    media: Media::Usb,
    cmdline: CMDLINE,
    cpus: 2,
    memory: "1G",
    reboot: false,
    image: TestImage::Plain,
    cpu_model: None,
    qemu: MACHINE,
    script: inputd,
    disks: &[],
    second: None,
};

/// The most memory of its own inputd may hold, in KiB: the pages no
/// file backs and the kernel's memory for it.
const MEMORY_MOST_KIB: u64 = 2048;

/// What inputd's status in _/processes_ says under `key`, as the
/// console shows it after `mark`.
fn status(vm: &mut Vm, mark: &str, key: &str) -> Result<u64, String> {
    let screen = vm.screen()?;
    let after = screen.iter().skip_while(|line| !line.contains(mark));
    let value = after.clone().find_map(|line| line.trim().strip_prefix(key)?.strip_prefix(": "));
    let number = value.and_then(|value| value.split(' ').next()?.parse().ok());
    number.ok_or_else(|| format!("inputd's status shows no {key}: {}", screen.join(" | ")))
}

/// Shows inputd's memory and how often it has waited on the console,
/// after a line that reads `mark` and a number the shell works out,
/// which the command typed does not show. Returns the memory of its
/// own in KiB and the times it waited.
fn account(vm: &mut Vm, mark: &str) -> Result<(u64, u64), String> {
    let keys = "anonymous_memory|kernel_memory|voluntary_switches";
    let shown = format!("{mark} 42");
    vm.type_text(&format!(
        "clear; echo {mark} $((6 * 7)); grep -E '^({keys}):' /processes/$(pgrep -x inputd)/status; \
         echo $((6 * 8)) shown\n"
    ))?;
    vm.expect_screen(&[&shown, "48 shown"])?;
    let pages = status(vm, &shown, "anonymous_memory")?;
    let kernel = status(vm, &shown, "kernel_memory")?;
    Ok((pages * 4 + kernel.div_ceil(1024), status(vm, &shown, "voluntary_switches")?))
}

fn inputd(vm: &mut Vm, case: &Case) -> Result<(), String> {
    let mut told = Told::start(vm, case)?;
    vm.expect_anywhere("inputd: handing the input devices out at /system/live/inputd/devices")?;
    // Reasons 1, 1 and 2 of inputd(8).
    let refused = "input: inputd refused, for reason";
    told.tell(vm, "strangers", &format!("{refused} 1"))?;
    Told::says(vm, &format!("{refused} 1"))?;
    Told::says(vm, &format!("{refused} 2"))?;
    told.tell(vm, "borrow", "input: ours has the screens")?;
    told.tell_all(
        vm,
        "inputd",
        &[
            "input: inputd handed out PS/2 keyboard, keyboard",
            "input: inputd handed out QEMU USB Keyboard, keyboard",
            "input: inputd handed out QEMU USB Mouse, pointer",
        ],
        "input: inputd handed out 3 devices",
    )?;
    vm.type_on(VIDEO, "a")?;
    vm.input_events(VIDEO, &[rel("x", 10), rel("y", -5)])?;
    told.heard(
        vm,
        &[
            ("PS/2 keyboard, from inputd", "nothing"),
            ("QEMU USB Keyboard, from inputd", "+30 -30"),
            ("QEMU USB Mouse, from inputd", "rel0:10 rel1:-5"),
        ],
    )?;
    // A tablet comes, points at the right edge halfway down, and goes.
    vm.monitor("device_add usb-tablet,bus=xhci.0,port=4,id=tablet,display=video0")?;
    Told::says(vm, "input: inputd handed out QEMU USB Tablet, tablet")?;
    vm.input_events(VIDEO, &[abs("x", 0x7fff), abs("y", 0x4000)])?;
    told.heard(vm, &[("QEMU USB Tablet, from inputd", "abs0:65535 abs1:32768")])?;
    vm.monitor("device_del tablet")?;
    Told::says(vm, "input: inputd says that QEMU USB Tablet, from inputd went")?;
    told.heard(vm, &[("QEMU USB Tablet, from inputd", "gone")])?;
    // What inputd handed out is tied to the holder that asked.
    told.tell(vm, "lend", "input: theirs has the screens")?;
    vm.type_on(VIDEO, "c")?;
    told.heard(
        vm,
        &[("QEMU USB Keyboard, from inputd", "paused"), ("kbd, tied to theirs", "+46 -46")],
    )?;
    told.tell(vm, "back", "input: ours has the screens again")?;
    told.tell(vm, "give", "input: the consoles have the screens")?;
    told.end(vm)?;
    small_and_asleep(vm)
}

/// inputd holds little, and keys and motions do not wake it: it waits
/// as often after ten seconds of them as before.
fn small_and_asleep(vm: &mut Vm) -> Result<(), String> {
    let (memory, before) = account(vm, "first")?;
    if memory >= MEMORY_MOST_KIB {
        return Err(format!("inputd holds {memory} KiB of its own, not under {MEMORY_MOST_KIB}"));
    }
    // Typed after a colon, so that the line it makes runs nothing.
    vm.type_on(VIDEO, ": ")?;
    for _ in 0..10 {
        vm.type_on(VIDEO, "x")?;
        vm.input_events(VIDEO, &[rel("x", 4), rel("y", 4)])?;
        std::thread::sleep(Duration::from_millis(900));
    }
    vm.type_on(VIDEO, "\n")?;
    let (_, after) = account(vm, "second")?;
    if after != before {
        return Err(format!("inputd waited {before} times, then {after}: something woke it"));
    }
    vm.type_text("poweroff\n")?;
    vm.expect("spark: asked to power off the machine")?;
    vm.expect("spark: every service has stopped")?;
    vm.expect_exit(QEMU_EXIT_POWER_OFF)
}
