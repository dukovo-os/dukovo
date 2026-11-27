// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Every PCI root bus is scanned and each function knows its NUMA node
//! (Phase 19, ADR 0660). QEMU's q35 machine has two NUMA nodes and two
//! PCIe expander bridges, each a root bus of its own that no bridge of
//! the first root leads to: bus 0x40 on node 1 with an NVMe controller
//! (serial `pxb-nvme-1`) behind a root port, and bus 0x80 on node 0 with
//! an e1000e behind another. The kernel must find the root bus 0x40
//! beyond the first, say that the firmware's host bridge names it and
//! gives it node 1, and bind the NVMe controller at 0000:41:00.0 and the
//! network card at 0000:81:00.0. From the console as root, the
//! controller's and the card's `numa_node` files in the PCI devices
//! folder of `/kernel/bus` read 1 and 0, and the disk's serial number is
//! the one QEMU was given.

use super::super::*;

/// What QEMU is given besides [`TWO_NODES`]: the two expanders, a root
/// port on each, the NVMe controller on the first's and the card on the
/// second's. The controller's disk reads zeros, so no image is made.
const EXPANDERS: &[&str] = &[
    "-device",
    "pxb-pcie,id=pxb1,bus_nr=0x40,numa_node=1,bus=pcie.0",
    "-device",
    "pcie-root-port,id=rp1,bus=pxb1,chassis=10,slot=1",
    "-device",
    "pxb-pcie,id=pxb2,bus_nr=0x80,numa_node=0,bus=pcie.0",
    "-device",
    "pcie-root-port,id=rp2,bus=pxb2,chassis=11,slot=1",
    "-drive",
    "driver=null-co,if=none,id=pxbdisk,size=64M",
    "-device",
    "nvme,id=pxbnvme,serial=pxb-nvme-1,bus=rp1",
    "-device",
    "nvme-ns,drive=pxbdisk,bus=pxbnvme",
    "-netdev",
    "user,id=pxbnet",
    "-device",
    "e1000e,netdev=pxbnet,bus=rp2,mac=52:54:00:12:34:70",
];

/// [`TWO_NODES`] and [`EXPANDERS`] as one list of arguments.
const MACHINE: [&str; TWO_NODES.len() + EXPANDERS.len()] = {
    let mut all = [""; TWO_NODES.len() + EXPANDERS.len()];
    let mut at = 0;
    while at < TWO_NODES.len() {
        all[at] = TWO_NODES[at];
        at += 1;
    }
    let mut next = 0;
    while next < EXPANDERS.len() {
        all[at + next] = EXPANDERS[next];
        next += 1;
    }
    all
};

pub const CASE: Case = Case {
    name: "pci-roots",
    about: "two PCIe expander bridges, each a root bus of its own, on 2 NUMA nodes: the kernel scans bus 0x40 and bus 0x80 beyond the first root, says that the firmware's host bridges name them with nodes 1 and 0, binds the NVMe controller and the e1000e behind them, and their numa_node files read 1 and 0 (ADR 0660)",
    firmware: Firmware::Uefi,
    media: Media::Usb,
    cmdline: "console-sign-in=off",
    cpus: 4,
    memory: "1G",
    reboot: false,
    image: TestImage::Plain,
    cpu_model: None,
    qemu: &MACHINE,
    script: pci_roots,
    disks: &[],
    second: None,
};

/// The node, and the NVMe controller's serial number, as the shell reads
/// them: each line's label is in the typed command only before its
/// `$(`, so the screen shows a line only once the command has run.
const READ_FILES: &str = "clear; echo \"nvme node $(cat /kernel/bus/pci/devices/0000:41:00.0/numa_node)\"; \
                          echo \"card node $(cat /kernel/bus/pci/devices/0000:81:00.0/numa_node)\"; \
                          echo \"nvme serial $(cat /kernel/block/nvme0n1/device/serial)\"";

/// Checks that the kernel's log has a line with every one of `needles`.
fn said(log: &str, needles: &[&str]) -> Result<(), String> {
    if log.lines().any(|line| needles.iter().all(|needle| line.contains(needle))) {
        return Ok(());
    }
    Err(format!("the kernel never said a line with {needles:?}"))
}

fn pci_roots(vm: &mut Vm, case: &Case) -> Result<(), String> {
    boot_report(vm, case)?;
    vm.expect("Boot complete.")?;
    // The PCI scan and uACPI log among lines boot_report reads past.
    let log = vm.log();
    said(&log, &["PCI: root bus 0000:40 beyond the first, with "])?;
    said(&log, &["PCI: root bus 0000:80 beyond the first, with "])?;
    said(&log, &["PCI: ACPI host bridge ", " is root bus 0000:40, node 1"])?;
    said(&log, &["PCI: ACPI host bridge ", " is root bus 0000:80, node 0"])?;
    said(&log, &["Disk: nvme0 is an NVMe ", " controller at 0000:41:00.0: "])?;
    said(&log, &["Net: net0 is an Intel 82574L network card at 0000:81:00.0"])?;
    if let Some(line) = log.lines().find(|line| line.contains("which has no devices")) {
        return Err(format!("a root bus the firmware names was not scanned: {line}"));
    }

    vm.expect("spark: started consoled (/system/commands/consoled) as process ")?;
    vm.expect_screen(&[&format!("{RELEASE} {VERSION} (tty1)"), "# "])?;
    answer_prompt(vm, "#", READ_FILES)?;
    vm.expect_screen(&["nvme node 1", "card node 0", "nvme serial pxb-nvme-1"])
}
