// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! A write faster than its disk cannot fill memory (ADR 0547). In a
//! machine of 512 MiB, `dd` fills a Btrfs on a disk of 768 MiB, more than
//! the memory, from the console. While it writes, the data that waits to
//! be written (`dirty` in /processes/meminfo) stays under a fifth of
//! memory and a write's worth; nothing runs out of memory, `dd` ends with
//! no space left, the system goes on, and once the file goes a write fits
//! again. The host's `btrfs check` finds the full filesystem sound.

use super::super::*;
use super::install_uefi::wait_for_screen;

/// The test.
pub const CASE: Case = Case {
    name: "dirty-limit",
    about: "A write faster than its disk cannot fill memory (ADR 0547): in 512 MiB, dd fills a Btrfs of 768 MiB from the console; the data waiting to be written stays under a fifth of memory, nothing runs out of memory, dd ends with no space left, a write fits again once the file goes, and Linux's btrfs check finds the filesystem sound",
    firmware: Firmware::Uefi,
    media: Media::Usb,
    cmdline: "console-sign-in=off",
    cpus: 2,
    memory: "512M",
    reboot: false,
    image: TestImage::Plain,
    cpu_model: None,
    qemu: &[],
    script: fill,
    disks: DISKS,
    second: None,
};

/// The disk, `vda`, larger than the machine's memory.
const DISKS: &[Disk] = &[Disk {
    kib: 768 * 1024,
    fat: ZEROS,
    cluster: 0,
    label: "",
    table: Table::None,
    bus: Bus::Virtio,
    sector: 512,
    device: "",
    errors: None,
    read_only: false,
    check: btrfs_checked,
}];

/// What the line typed takes: `dd` in the background until the disk is
/// full, the largest `dirty` seen every fifth of a second while it runs,
/// and whether that stayed under a fifth of `total` and one write of
/// `dd`'s, 1 MiB, which the limit lets each writer finish.
const FILL: &str = "clear; rm -f /tmp/dd.status; (dd if=/devices/zero of=/tmp/d/fill bs=1M \
                    > /tmp/dd.out 2>&1; echo $? > /tmp/dd.status) & most=0; while [ ! -s \
                    /tmp/dd.status ]; do d=$(awk '/^dirty:/ {print $2}' /processes/meminfo); \
                    [ \"$d\" -gt \"$most\" ] && most=$d; sleep 0.2; done; total=$(awk \
                    '/^total:/ {print $2}' /processes/meminfo); grep 'No space' /tmp/dd.out; \
                    echo \"most dirty $most KiB of $total\"; [ \"$most\" -le $((total / 5 + \
                    1024)) ] && echo \"under: $((40 + 2))\"; echo \"filled: $((100 + $(cat \
                    /tmp/dd.status)))\"";

/// How long filling the disk may take, in seconds, on a machine without
/// KVM.
const FILL_SECONDS: u64 = 300;

/// The console: a Btrfs on the disk, filled, and written to again.
fn fill(vm: &mut Vm, _case: &Case) -> Result<(), String> {
    vm.expect("spark: started consoled (/system/commands/consoled) as process ")?;
    vm.expect_screen(&[&format!("{RELEASE} {VERSION} (tty1)"), "# "])?;
    vm.type_text(
        "mkfs-btrfs -q vda && mkdir /tmp/d && mount vda /tmp/d && echo \"mounted: $((40 + 2))\"\n",
    )?;
    vm.expect_screen(&["mounted: 42"])?;
    // `dd` ends with status 1, shown as 101 so that the line typed does
    // not hold what the test waits for.
    vm.type_text(&format!("{FILL}\n"))?;
    wait_for_screen(vm, "filled: 1", FILL_SECONDS)?;
    vm.expect_screen(&["No space left on device", "most dirty ", "under: 42", "filled: 101"])?;
    // The system goes on, and the room comes back.
    vm.type_text(
        "clear; rm /tmp/d/fill && sync && dd if=/devices/zero of=/tmp/d/after bs=1M count=64 \
         2> /dev/null && sync && echo \"written: $((40 + 2))\"\n",
    )?;
    wait_for_screen(vm, "written: 42", 120)?;
    if let Some(line) = vm.log().lines().find(|line| line.contains("Out of memory")) {
        return Err(format!("memory ran out while the disk filled: {}", line.trim()));
    }
    vm.type_text("umount /tmp/d && poweroff\n")?;
    vm.expect("spark: asked to power off the machine")?;
    vm.expect_exit(QEMU_EXIT_POWER_OFF)
}
