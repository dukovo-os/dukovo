// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! A 64 MiB virtio disk that *mkntfs* made, of 4 KiB clusters, which
//! *ntfscp* wrote files to, is shrunk at the console by Dukovo's own
//! *resize-ntfs*, as an installer shrinks Windows' volume. Mounted at
//! boot, the disk must be refused, with words that say to unmount it.
//! Unmounted, *resize-ntfs --check* must find it sound, *ntfsresize -m*
//! must give a smallest size under the one asked for, and a test run
//! (*-n*) must write nothing. The shrink to 24 MiB, with
//! *--no-windows-check*, must then move what lay past the new end
//! (*mkntfs* puts the journal and `$MFTMirr` mid-volume), and Radix must
//! mount the shrunk volume and read its files. On the host, the disk cut
//! to 24 MiB, as the partitioner then cuts a partition, ntfs-3g must find
//! the volume sound and clean (*ntfsresize --info* and *ntfsfix -n*), of
//! the size asked for, and *ntfscat* must read every file as the host
//! wrote it (ADR 0312).

use std::process::Command;

use super::super::*;

pub const CASE: Case = Case {
    name: "ntfs-resize",
    about: "An NTFS disk the host made and wrote files to, shrunk at the console by resize-ntfs: refused while mounted, checked, its smallest size given by ntfsresize -m, a test run writing nothing, then shrunk to 24 MiB, its journal and $MFTMirr moved; Radix mounts it and reads its files, and on the host ntfs-3g finds it sound and clean at its new size and ntfscat reads every file back",
    firmware: Firmware::Uefi,
    media: Media::Usb,
    cmdline: "console-sign-in=off",
    cpus: 2,
    memory: "1G",
    reboot: false,
    image: TestImage::Plain,
    cpu_model: None,
    qemu: &[],
    script: ntfs_resize,
    disks: NTFS_RESIZE_DISKS,
    second: None,
};

/// The `ntfs-resize` test's disk: a virtio disk of 512-byte sectors and
/// a 64 MiB NTFS of 4 KiB clusters, which `mkntfs` made and `ntfscp`
/// wrote [`ntfs_files`] to.
const NTFS_RESIZE_DISKS: &[Disk] = &[Disk {
    kib: 64 * 1024,
    fat: NTFS,
    cluster: CLUSTER,
    label: "SHRINKME",
    table: Table::None,
    bus: Bus::Virtio,
    sector: 512,
    device: "",
    errors: None,
    read_only: false,
    check: ntfs_shrunk,
}];

/// The size of the volume's clusters.
const CLUSTER: u32 = 4096;

/// The size the volume is shrunk to: the partition it would then fill.
const SHRUNK: u64 = 24 << 20;

/// The `ntfs-resize` test.
fn ntfs_resize(vm: &mut Vm, _case: &Case) -> Result<(), String> {
    vm.expect("spark: mounted the NTFS filesystem of vda at /volumes/vda")?;
    vm.expect("spark: started consoled (/system/commands/consoled) as process ")?;
    vm.expect_screen(&[&format!("{RELEASE} {VERSION} (tty1)"), "# "])?;

    // Spark mounted it at boot: nothing shrinks a volume in use.
    vm.type_text("resize-ntfs -s 24Mi vda; echo \"status: $((10 + $?))\"\n")?;
    vm.expect_screen(&["is mounted at /volumes/vda; unmount it first", "status: 11"])?;

    vm.type_text("clear; umount /volumes/vda && resize-ntfs --check vda | tail -1\n")?;
    vm.expect_screen(&["No problems found."])?;
    // ntfs-3g's name and its option: the smallest, in MB of 1,000,000.
    vm.type_text(
        "clear; [ \"$(ntfsresize -m /devices/vda | cut -d: -f2)\" -lt 25 ] && \
         echo \"small: $((10 + 2))\"\n",
    )?;
    vm.expect_screen(&["small: 12"])?;
    vm.type_text("clear; resize-ntfs -n -s 24Mi vda | tail -1\n")?;
    vm.expect_screen(&["The test run wrote nothing."])?;

    // It moves a MB or more: at least the journal.
    vm.type_text(
        "clear; resize-ntfs --no-windows-check -s 24Mi vda > /tmp/r; echo \"shrunk: $((20 + $?))\"; \
         grep -q '^Moved [1-9]' /tmp/r && echo \"moved: $((20 + 2))\"\n",
    )?;
    vm.expect_screen(&["shrunk: 20", "moved: 22"])?;
    vm.type_text("clear; grep Successfully /tmp/r\n")?;
    vm.expect_screen(&[&format!("it is {SHRUNK} bytes now.")])?;

    // Radix mounts the shrunk volume, and reads its files.
    vm.type_text(
        "clear; mkdir /tmp/n && mount vda /tmp/n && cat /tmp/n/hello.txt && \
         wc -c < /tmp/n/big.bin && umount /tmp/n; echo \"read: $((30 + $?))\"\n",
    )?;
    vm.expect_screen(&["Written by ntfscp.", "1048699", "read: 30"])?;

    vm.type_text("poweroff\n")?;
    vm.expect("spark: asked to power off the machine")?;
    vm.expect_exit(QEMU_EXIT_POWER_OFF)
}

/// Checks the shrunk volume on the host. The disk is cut to the size
/// the volume was shrunk to first, as the partitioner then shrinks a
/// partition, so that its last sector is the copy of the boot sector,
/// where ntfs-3g looks for it. ntfs-3g must then find it sound and clean
/// ([`phase11::ntfs_checked`]), `ntfsinfo` must find it the whole
/// clusters before that last sector, and `ntfscat` must read each of
/// [`ntfs_files`] as the host wrote it.
fn ntfs_shrunk(disk: &DiskImage, log: &str) -> Result<(), String> {
    let image = disk.path.display();
    std::fs::OpenOptions::new()
        .write(true)
        .open(&disk.path)
        .and_then(|file| file.set_len(SHRUNK))
        .map_err(|err| format!("cutting {image} to {SHRUNK} bytes: {err}"))?;
    phase11::ntfs_checked(disk, log)?;
    let info = host_tool(Command::new(fat_tool("ntfsinfo")).arg("-m").arg(&disk.path))?;
    let clusters = (SHRUNK - disk.sector) / u64::from(CLUSTER);
    let want = format!("Volume Size in Clusters: {clusters}");
    if !String::from_utf8_lossy(&info).contains(&want) {
        return Err(format!("{image}: ntfsinfo -m does not say \"{want}\""));
    }
    for (name, made) in ntfs_files() {
        let mut ntfscat = Command::new(fat_tool("ntfscat"));
        ntfscat.env("LC_ALL", "C.UTF-8").arg(&disk.path).arg(format!("/{name}"));
        let read = host_tool(&mut ntfscat).map_err(|err| format!("{image}: ntfscat {name}: {err}"))?;
        if read != made_bytes(&made) {
            return Err(format!("{image}: ntfscat reads other bytes of {name}"));
        }
    }
    Ok(())
}
