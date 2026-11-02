// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Two blank virtio disks, one of 512-byte sectors and one of 4 KiB
//! sectors, are given a Btrfs at the console with Dukovo's own tools.
//! The first is made by *mkfs-btrfs*; *mkfs.btrfs* must then refuse to
//! make another over it without *-f*, and *mkfs -t btrfs -f* makes it
//! again, labelled, which */devices/disk/by-label* must lead to. The
//! second is made by *mkfs.btrfs -f*, with 4 KiB nodes, one copy of
//! metadata and xxHash checksums. Both must be found whole by
//! *fsck-btrfs* and by *btrfsck* with its *--check-data-csum*, which
//! must refuse *--repair*. On the host, Linux's *btrfs check* must find
//! both whole (ADRs 0304 and 0305).

use super::super::*;

pub const CASE: Case = Case {
    name: "btrfs-tools",
    about: "Two blank disks, of 512-byte and 4 KiB sectors, given a Btrfs at the console by mkfs-btrfs, which refuses to make one over another without -f; fsck-btrfs and btrfsck find both whole and refuse --repair, /devices/disk/by-label leads to one, and the host's btrfs check finds both whole",
    firmware: Firmware::Uefi,
    media: Media::Usb,
    cmdline: "console-sign-in=off",
    cpus: 2,
    memory: "1G",
    reboot: false,
    image: TestImage::Plain,
    cpu_model: None,
    qemu: &[],
    script: btrfs_tools,
    disks: BLANK_DISKS,
    second: None,
};

/// A blank disk of 160 MiB, of `sector`-byte sectors.
const fn blank(sector: u64) -> Disk {
    Disk {
        kib: 160 * 1024,
        fat: BTRFS_BLANK,
        cluster: 0,
        label: "",
        table: Table::None,
        bus: Bus::Virtio,
        sector,
        device: "",
        errors: None,
        read_only: false,
        check: btrfs_checked,
    }
}

const BLANK_DISKS: &[Disk] = &[blank(512), blank(4096)];

/// The `btrfs-tools` test.
fn btrfs_tools(vm: &mut Vm, _case: &Case) -> Result<(), String> {
    vm.expect("spark: started consoled (/system/commands/consoled) as process ")?;
    vm.expect_screen(&[&format!("{RELEASE} {VERSION} (tty1)"), "# "])?;

    vm.type_text("mkfs-btrfs vda | tail -3 && echo \"made: $((40 + 2))\"\n")?;
    vm.expect_screen(&["Checksum:           crc32c", "Device:             /devices/vda", "made: 42"])?;
    vm.type_text("mkfs.btrfs vda; echo \"status: $?\"\n")?;
    vm.expect_screen(&["/devices/vda holds a btrfs filesystem; -f makes a new one over it", "status: 1"])?;
    vm.type_text("mkfs -t btrfs -f -L made /devices/vda | grep Label\n")?;
    vm.expect_screen(&["Label:              made"])?;
    vm.type_text("mkfs.btrfs -f -q -n 4k -m single --csum xxhash vdb && echo \"made: $((40 + 3))\"\n")?;
    vm.expect_screen(&["made: 43"])?;

    vm.type_text("fsck-btrfs vda > /tmp/a; echo \"checked: $?\"; grep -c '^\\[' /tmp/a; tail -4 /tmp/a | head -1\n")?;
    vm.expect_screen(&["checked: 0", "7", "found 147456 bytes used, no error found"])?;
    vm.type_text("btrfsck --check-data-csum /devices/vdb > /tmp/b; echo \"checked: $?\"; grep found /tmp/b\n")?;
    vm.expect_screen(&["checked: 0", "found 36864 bytes used, no error found"])?;
    vm.type_text("fsck-btrfs --repair vdb; echo \"status: $?\"\n")?;
    vm.expect_screen(&["--repair is not supported", "status: 16"])?;
    vm.type_text("readlink /devices/disk/by-label/made\n")?;
    vm.expect_screen(&["/devices/vda"])?;

    vm.type_text("poweroff\n")?;
    vm.expect("spark: asked to power off the machine")?;
    vm.expect_exit(QEMU_EXIT_POWER_OFF)
}
