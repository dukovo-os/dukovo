// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! As *installed-uefi*, under a BIOS: GRUB's *core.img* in the BIOS boot
//! partition loads the kernel and the startup archive from the root's
//! slot, Spark runs the system from that root, and the idle machine uses
//! less than 100 MiB, counting the startup archive (ADR 0307). The second
//! boot names a root no disk holds, *root=UUID=* of zeros, after the boot
//! menu's own: Spark must not take the disk's root instead, and must say
//! that the installed system cannot start.

use super::super::*;

pub const CASE: Case = Case {
    name: "installed-bios",
    about: "An installed system's disk boots under a BIOS from its root's slot within the idle memory budget; named a root no disk holds, Spark takes no other root and says the system cannot start",
    firmware: Firmware::Bios,
    media: Media::Disk,
    cmdline: "console-sign-in=off",
    cpus: 2,
    memory: "512M",
    reboot: false,
    image: TestImage::Installed,
    cpu_model: None,
    qemu: &[],
    script: super::installed_uefi::installed,
    disks: &[],
    second: Some(SecondBoot {
        cmdline: "console-sign-in=off root=UUID=00000000-0000-0000-0000-000000000000",
        before: nothing,
        script: no_root,
    }),
};

/// Nothing to do on the host before the second boot.
fn nothing(_case: &Case, _disks: &[DiskImage]) -> Result<(), String> {
    Ok(())
}

/// The second boot: the root the command line names last is on no disk.
fn no_root(vm: &mut Vm, _case: &Case) -> Result<(), String> {
    vm.expect(
        "spark: no disk holds the Dukovo root the boot menu names \
         (root=UUID=00000000-0000-0000-0000-000000000000), so this installed system cannot start",
    )?;
    vm.expect("spark: built the hierarchy of ")?;
    if vm.log().contains("spark: /system is Dukovo's root on ") {
        return Err("Spark ran the system from a root the command line did not name".into());
    }
    Ok(())
}
