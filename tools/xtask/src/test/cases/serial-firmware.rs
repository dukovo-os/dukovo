// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! A BMC's firmware console on COM2 (ADR 0481): the ACPI table SPCR names
//! the 16550 at I/O port 0x2F8 at 57600 baud, as a server's Serial-over-LAN
//! is described, and the command line has no `console=`. The kernel runs
//! COM2 at the table's speed from boot and says it is the firmware's
//! console, `/kernel/class/tty/ttyS1/firmware_console` holds its line, and
//! serial-console keeps root's shell on it, which `stty` finds at 57600
//! baud. `serial` lists the port as the firmware's, and says the system
//! has no boot menu to change, as it did not start from a Dukovo root on a
//! disk. The second boot adds `spcr=off`: COM2 runs at 115200 baud as any
//! other port, the log does not call it the firmware's console, and
//! serial-console has no port to keep a console on.

use super::super::*;

pub const CASE: Case = Case {
    name: "serial-firmware",
    about: "The ACPI table SPCR names COM2 at 57600 baud and there is no console= flag: the kernel runs it at that speed and calls it the firmware's console, serial-console keeps a shell on it, sysfs and serial say what it is; with spcr=off it is an ordinary port and no console",
    firmware: Firmware::Uefi,
    media: Media::Usb,
    cmdline: "lockdep console-sign-in=off",
    cpus: 2,
    memory: "1G",
    reboot: false,
    image: TestImage::Plain,
    cpu_model: None,
    qemu: &[
        "-chardev",
        SERIAL_LINE[0],
        "-device",
        "isa-serial,index=1,chardev=line0",
        "-acpitable",
        SPCR_TABLE,
    ],
    script: firmware,
    disks: &[],
    second: Some(SecondBoot {
        cmdline: "lockdep console-sign-in=off spcr=off",
        before: nothing,
        script: spcr_off,
    }),
};

/// What a second boot does to the host's disks first: nothing, as the case
/// has none.
fn nothing(_case: &Case, _disks: &[DiskImage]) -> Result<(), String> {
    Ok(())
}

/// The first boot: the SPCR's port is the console.
fn firmware(vm: &mut Vm, case: &Case) -> Result<(), String> {
    let line = vm.dir().join(format!("{}.line", case.name));
    let mut com2 = SocketLine::connect(&line, "COM2", vm.deadline)?;
    boot_report(vm, case)?;
    vm.expect_anywhere(
        "ACPI: SPCR: the firmware's console is the 16550 at I/O port 0x2f8, at 57600 baud",
    )?;
    vm.expect_anywhere(
        "Serial: COM2 at I/O port 0x2f8 is the terminal /devices/ttyS1, at 57600 baud, 8N1, the \
         firmware's console (ACPI SPCR); the kernel log and kdb are on it; typing by ISA \
         interrupt 3",
    )?;
    vm.expect("Boot complete.")?;
    vm.expect_anywhere(
        "serial-console: the firmware's console (ACPI SPCR) is ttyS1, at 57600n8",
    )?;
    // Root's shell on COM2, at the line the table gave it. The commands
    // are typed so that no echo holds an answer whole.
    com2.expect(&format!("{RELEASE} {VERSION} (ttyS1)"))?;
    com2.expect_prompt()?;
    com2.send(
        "cd /kernel/class/tty/ttyS1; cat firmware_console; stty -a | head -1; echo $((6 * 7)); \
         serial\r",
    )?;
    for answer in [
        "57600n8\r\n",
        "speed 57600 baud;",
        "42\r\n",
        "firmware, 57600n8",
        "The firmware sends its console to ttyS1 (ACPI SPCR), at 57600n8.",
        "At the next start: this system did not start from a Dukovo root on a disk.",
    ] {
        com2.expect(answer)?;
    }
    com2.expect_prompt()?;
    // The other ports' files have no such one: only COM2 is the firmware's.
    com2.send("cd ../ttyS0; echo \"on\"\"ly: $(ls | grep -c firm)\"\r")?;
    com2.expect("only: 0\r\n")?;
    com2.expect_prompt()?;
    com2.send("poweroff\r")?;
    vm.expect("spark: asked to power off the machine")?;
    vm.expect("spark: every service has stopped")?;
    vm.expect_exit(QEMU_EXIT_POWER_OFF)
}

/// The second boot, with `spcr=off`: COM2 is an ordinary port.
fn spcr_off(vm: &mut Vm, case: &Case) -> Result<(), String> {
    boot_report(vm, case)?;
    vm.expect_anywhere(
        "Serial: COM2 at I/O port 0x2f8 is the terminal /devices/ttyS1, at 115200 baud, 8N1; the \
         kernel log and kdb are on it; typing by ISA interrupt 3",
    )?;
    vm.expect("Boot complete.")?;
    vm.expect_anywhere("serial-console: no console= flag and no firmware console port")?;
    if vm.log().contains("the firmware's console (ACPI SPCR)") {
        return Err("a Serial line calls COM2 the firmware's console under spcr=off".into());
    }
    vm.press_power_button()?;
    vm.expect_exit(QEMU_EXIT_POWER_OFF)
}
