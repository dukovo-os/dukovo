// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! QEMU's two-port PCI serial card, `pci-serial-2x`: 16550As on PCI, as
//! the ports of the lab's ASIX AX99100 card and Intel AMT's
//! Serial-over-LAN port show themselves. The kernel finds both ports in
//! the card's I/O BAR and makes them terminals with the names Linux
//! gives them, `/devices/ttyS4` and `ttyS5`, and consoles beside COM1
//! (ADRs 0360 and 0480). The first port, a Unix socket the test reads and
//! types on, gets the whole boot log, with what was logged before the
//! card was found replayed, in `\r\n` lines, and `kdb` answers a command
//! typed on it.

use super::super::*;

pub const CASE: Case = Case {
    name: "pci-serial",
    about: "a two-port PCI serial card: the kernel log from its start, and kdb, on its ports",
    firmware: Firmware::Uefi,
    media: Media::Usb,
    cmdline: "qemu-exit kdb lockdep",
    cpus: 2,
    memory: "1G",
    reboot: false,
    image: TestImage::Plain,
    cpu_model: None,
    qemu: &["-chardev", SERIAL_LINE[0], "-device", "pci-serial-2x,chardev1=line0"],
    script: pci_serial,
    disks: &[],
    second: None,
};

/// The `pci-serial` case.
fn pci_serial(vm: &mut Vm, case: &Case) -> Result<(), String> {
    // QEMU listens on the port's socket from its start, before the
    // firmware runs; what the port sends before the test connects is lost.
    let line = vm.dir().join(format!("{}.line", case.name));
    let mut port = SocketLine::connect(&line, "the PCI serial port", vm.deadline)?;
    boot_report(vm, case)?;
    for name in ["ttyS4", "ttyS5"] {
        vm.expect_anywhere(&format!(
            ", is the terminal /devices/{name}, at 115200 baud, 8N1; the kernel log and kdb are \
             on it"
        ))?;
    }
    vm.expect_anywhere("Serial: the QEMU PCI 16550A adapter at 0000:00:")?;
    vm.expect("Boot complete.")?;
    vm.expect("kdb on CPU 0, boot complete")?;
    vm.expect("kdb> ")?;
    // The log from its first line, replayed when the card was found.
    port.expect(&format!("Radix {VERSION} ({RELEASE} {VERSION})\r\n"))?;
    port.expect("KASLR: kernel moved by 0x")?;
    port.expect("Serial: the QEMU PCI 16550A adapter at 0000:00:")?;
    // And what was logged after, which the `serial` thread sends.
    port.expect("Boot complete.\r\n")?;
    port.expect("kdb on CPU 0, boot complete")?;
    port.expect("kdb> ")?;
    // `kdb` answers a command typed on the port, on the port and on COM1.
    port.send("cpus\r")?;
    port.expect("2 CPUs")?;
    port.expect("kdb> ")?;
    vm.expect("2 CPUs")?;
    vm.expect("kdb> ")?;
    port.send("continue\r")?;
    port.expect("kdb: continuing")?;
    vm.expect("kdb: continuing")?;
    vm.expect_exit(QEMU_EXIT_SUCCESS)
}
