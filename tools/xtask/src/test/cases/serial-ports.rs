// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! COM2 and a PCI serial port beside COM1, each a terminal with the name
//! Linux gives it (ADR 0480). COM2 is `/devices/ttyS1`: the flag
//! console=ttyS1,57600 has the kernel run it at 57600 baud from its first
//! use, and serial-log=ttyS0,ttyS4 keeps the kernel log off it. The PCI
//! port is `/devices/ttyS4`, with the log from its first line.
//! serial-console keeps root's shell on both, `/kernel/class/tty` says
//! what each is, and the log stays off the PCI port while its shell has
//! it open.

use super::super::*;

pub const CASE: Case = Case {
    name: "serial-ports",
    about: "COM2 and a PCI serial port as ttyS1 and ttyS4: console= sets a port's line from boot and gives it a shell, and serial-log= keeps the log off one",
    firmware: Firmware::Uefi,
    media: Media::Usb,
    cmdline: "lockdep console-sign-in=off console=ttyS1,57600 console=ttyS4 serial-log=ttyS0,ttyS4",
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
        "-chardev",
        PORT_LINE,
        "-device",
        "pci-serial,chardev=port",
    ],
    script: serial_ports,
    disks: &[],
    second: None,
};

/// The `serial-ports` case.
fn serial_ports(vm: &mut Vm, case: &Case) -> Result<(), String> {
    let deadline = vm.deadline;
    let com2_line = vm.dir().join(format!("{}.line", case.name));
    let mut com2 = SocketLine::connect(&com2_line, "COM2", deadline)?;
    let mut card = SocketLine::connect(&port_line(vm.dir(), case.name), "the PCI port", deadline)?;
    boot_report(vm, case)?;
    // Each port's name, line and part in the log, as the kernel says them.
    vm.expect_anywhere(
        "Serial: COM2 at I/O port 0x2f8 is the terminal /devices/ttyS1, at 57600 baud, 8N1; the \
         kernel log is not on it; typing by ISA interrupt 3",
    )?;
    vm.expect_anywhere("Serial: COM2 takes typing by ISA interrupt 3 (GSI ")?;
    vm.expect_anywhere(
        ", is the terminal /devices/ttyS4, at 115200 baud, 8N1; the kernel log and kdb are on it; \
         typing polled every 10 ms while it is open",
    )?;
    vm.expect("Boot complete.")?;
    // The card's port has the log from its first line.
    card.expect(&format!("Radix {VERSION} ({RELEASE} {VERSION})\r\n"))?;
    card.expect("Boot complete.\r\n")?;
    // Root's shell on COM2, at the line console= gave it from boot.
    com2.expect(&format!("{RELEASE} {VERSION} (ttyS1)"))?;
    com2.expect_prompt()?;
    com2.send(
        "tty; stty -a | head -1; echo $((6 * 7)); cd /kernel/class/tty/ttyS1; cat console port \
         kind interrupt\r",
    )?;
    for answer in ["/devices/ttyS1\r\n", "speed 57600 baud;", "42\r\n", "N\r\n0x2F8\r\ncom2\r\nisa 3\r\n"]
    {
        com2.expect(answer)?;
    }
    com2.expect_prompt()?;
    // And on the card's port.
    card.expect(&format!("{RELEASE} {VERSION} (ttyS4)"))?;
    card.expect_prompt()?;
    card.send("tty; echo $((6 * 8)); cd /kernel/class/tty/ttyS4; cat console kind interrupt\r")?;
    for answer in ["/devices/ttyS4\r\n", "48\r\n", "Y\r\nqemu\r\npolled\r\n"] {
        card.expect(answer)?;
    }
    card.expect_prompt()?;
    // A line logged while the card's shell has its port open goes to COM1
    // and not to the port, which the shell draws on; COM2 never gets the
    // log at all.
    com2.send("echo serial-ports: $((7 * 9)) logged > /devices/kmsg\r")?;
    vm.expect("serial-ports: 63 logged")?;
    // The serial thread looks for more of the log at least every 100 ms.
    std::thread::sleep(Duration::from_millis(500));
    if card.sent().contains("63 logged") {
        return Err("the kernel log went out on ttyS4 while its shell had it open".into());
    }
    if com2.sent().contains("Boot complete.") {
        return Err("the kernel log went out on ttyS1, which serial-log= leaves out".into());
    }
    com2.send("poweroff\r")?;
    vm.expect("spark: asked to power off the machine")?;
    vm.expect("spark: every service has stopped")?;
    vm.expect_exit(QEMU_EXIT_POWER_OFF)
}
