// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Two MegaRAID controllers of QEMU's (ADR 0664): a SAS 1078
//! (`megasas`) in RAID mode with one volume at target 0, and a SAS 2108
//! (`megasas-gen2`) in JBOD mode passing one disk through, at SCSI ID 0,
//! which the firmware numbers 0. The kernel brings both firmwares up,
//! names the volume `sda` and the disk `sdb`, and logs what each
//! controller is and has. From the console as root, the first
//! controller's folder in `/kernel/raid` says it is a SAS1078 in RAID
//! mode whose volume is `sda`, the second's that it is a SAS2108 passing
//! disk 0 through as `sdb`, both run, and *storage raid* names both.
//! The disk's health file fails to open, as QEMU's disks keep no log
//! pages.
//!
//! QEMU's models differ from a controller's firmware in two ways the
//! kernel meets here. They fail every SCSI command that moves no data,
//! TEST UNIT READY among them, as an internal target failure, so the
//! probe goes on to READ CAPACITY, as Linux's does. And they read only
//! the first byte of PD_GET_INFO's mailbox, so of the disks they number
//! by SCSI ID and LUN (`id << 8 | lun`), only one at SCSI ID 0 can be
//! asked about: the disk is there, and the unit tests of `radix-megaraid`
//! take a disk numbered above 255 through to its SCSI ID and LUN.

use super::super::*;

/// The two controllers and their disks, which read zeros, so no image is
/// made.
const CONTROLLERS: &[&str] = &[
    "-device",
    "megasas,id=raid0",
    "-drive",
    "driver=null-co,if=none,id=raidvolume,size=64M,read-zeroes=on",
    "-device",
    "scsi-hd,drive=raidvolume,bus=raid0.0,scsi-id=0,lun=0",
    "-device",
    "megasas-gen2,id=raid1,use_jbod=on",
    "-drive",
    "driver=null-co,if=none,id=raiddisk,size=64M,read-zeroes=on",
    "-device",
    "scsi-hd,drive=raiddisk,bus=raid1.0,scsi-id=0,lun=0",
];

pub const CASE: Case = Case {
    name: "megaraid",
    about: "two of QEMU's MegaRAID controllers, a SAS 1078 with a volume and a SAS 2108 in JBOD mode with a disk passed through: the kernel names them sda and sdb, /kernel/raid shows each controller's chip, mode, volume and disk and that both run, storage raid names both, and the disk's health file fails to open as QEMU's disks keep no log pages (ADR 0664)",
    firmware: Firmware::Uefi,
    media: Media::Usb,
    cmdline: "console-sign-in=off",
    cpus: 2,
    memory: "1G",
    reboot: false,
    image: TestImage::Plain,
    cpu_model: None,
    qemu: CONTROLLERS,
    script: megaraid,
    disks: &[],
    second: None,
};

/// What `/kernel/raid` says, as counts of the lines that match: each
/// line's label is in the typed command only before its `$(`, so the
/// screen shows a line only once the command has run.
const READ_FILES: &str = "clear; \
    echo \"first $(grep -c '^chip SAS1078$' /kernel/raid/0/controller) $(grep -c '^mode raid$' /kernel/raid/0/controller) $(grep -c 'os=sda' /kernel/raid/0/volumes)\"; \
    echo \"second $(grep -c '^chip SAS2108$' /kernel/raid/1/controller) $(grep -c '^mode hba$' /kernel/raid/1/controller) $(grep -c '^id=0.*state=jbod.*os=sdb' /kernel/raid/1/disks)\"; \
    echo \"running $(cat /kernel/raid/*/controller | grep -c '^state operational$')\"; \
    echo \"health $(cat /kernel/raid/1/health/0 > /dev/null 2>&1; echo $?)\"; \
    echo \"named $(storage raid | grep -c -E '^Controller (0: LSI MegaRAID SAS 8708EM2|1: LSI MegaRAID SAS 9260-8i)')\"";

/// Checks that the kernel's log has a line with every one of `needles`;
/// fails with what it said of the controllers, which a run on CI shows
/// without the serial log.
fn said(log: &str, needles: &[&str]) -> Result<(), String> {
    if log.lines().any(|line| needles.iter().all(|needle| line.contains(needle))) {
        return Ok(());
    }
    let of_them: Vec<&str> = log
        .lines()
        .filter_map(|line| line.find("Disk: ").map(|at| &line[at..]))
        .filter(|line| line.contains("MegaRAID"))
        .collect();
    Err(format!(
        "the kernel never said a line with {needles:?}; of the controllers it said: {}",
        of_them.join(" | ")
    ))
}

fn megaraid(vm: &mut Vm, case: &Case) -> Result<(), String> {
    boot_report(vm, case)?;
    vm.expect("Boot complete.")?;
    let log = vm.log();
    said(
        &log,
        &[
            "Disk: MegaRAID SAS1078 controller at ",
            ": LSI MegaRAID SAS 8708EM2, firmware package ",
            ", 1 volume, 0 pass-through disks, MSI",
        ],
    )?;
    said(
        &log,
        &[
            "Disk: MegaRAID SAS2108 controller at ",
            ": LSI MegaRAID SAS 9260-8i, firmware package ",
            ", 0 volumes, 1 pass-through disk, MSI",
        ],
    )?;
    said(&log, &["Disk: sda is volume 0 of ", " on the MegaRAID controller at "])?;
    said(&log, &["Disk: sdb is pass-through disk 0 of ", " on the MegaRAID controller at "])?;
    if let Some(line) = log.lines().find(|line| {
        line.contains("MegaRAID controller at")
            && (line.contains("could not") || line.contains("did not"))
    }) {
        return Err(format!("a controller was not set up whole: {line}"));
    }

    vm.expect("spark: started consoled (/system/commands/consoled) as process ")?;
    vm.expect_screen(&[&format!("{RELEASE} {VERSION} (tty1)"), "# "])?;
    answer_prompt(vm, "#", READ_FILES)?;
    vm.expect_screen(&["first 1 1 1", "second 1 1 1", "running 2", "health 1", "named 2"])
}
