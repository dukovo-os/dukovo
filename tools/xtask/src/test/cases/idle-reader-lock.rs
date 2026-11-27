// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! An idle-class reader of a busy disk holds up no one (ADR 0548), at the
//! console as root. A Btrfs on a virtio disk holds two files of random
//! bytes; two services of different I/O weights read the disk raw, 16
//! threads each, and QEMU holds the disk to 64 MiB a second, so that
//! requests always wait in Dukovo's queue and the disk is never free.
//! Then `ionice -c 3 cat` reads the larger file, and a best-effort `cat`
//! reads the smaller beside it: Btrfs holds its lock while it reads, so
//! the idle-class read, which went only once the disk was free, held the
//! other for as long as the disk was busy, as Rescue's rewrite held every
//! program of the root's filesystem on a laptop while a test read its
//! disk. The best-effort read ends within 30 seconds, and the idle-class
//! one within two minutes, while the disk stays busy.

use super::super::*;
use super::install_uefi::wait_for_screen;

/// The service files of the io-weight test, which read `vda` raw.
const TESTS: &str = "/system/tests/services/io-weight";

/// Where Spark reads service files.
const FILES: &str = "/system/settings/spark/services";

/// What QEMU lets the disk do each second, in bytes: less than the
/// readers ask for, so that requests wait in Dukovo's queue.
const DISK_RATE: u64 = 64 << 20;

/// The disk, `vda`, which holds the Btrfs and which the readers read.
const DISKS: &[Disk] = &[Disk {
    kib: 256 * 1024,
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

/// The test.
pub const CASE: Case = Case {
    name: "idle-reader-lock",
    about: "an idle-class reader of a busy disk holds up no one (ADR 0548): while two services of different I/O weights read a throttled virtio disk raw, a best-effort read of a file on its Btrfs ends within 30 seconds beside an idle-class read of another, and the idle-class read ends within two minutes",
    firmware: Firmware::Uefi,
    media: Media::Usb,
    cmdline: "console-sign-in=off",
    cpus: 2,
    memory: "1G",
    reboot: false,
    image: TestImage::Plain,
    cpu_model: None,
    qemu: &[],
    script: idle_reader,
    disks: DISKS,
    second: None,
};

/// A Btrfs on the disk with the two files, mounted again so that nothing
/// of them is cached.
const SETUP: &str = "clear; mkfs-btrfs -q vda && mkdir /tmp/d && mount vda /tmp/d && dd \
                     if=/devices/urandom of=/tmp/d/big bs=1M count=2 2> /devices/null && dd \
                     if=/devices/urandom of=/tmp/d/small bs=64K count=1 2> /devices/null && \
                     umount /tmp/d && mount vda /tmp/d && echo \"ready: $((40 + 2))\"";

/// The idle-class read in the background; two seconds on, the
/// best-effort read, timed by the kernel's clock; then the idle-class
/// read's end. Its status is shown as 40 and up, so that the line typed
/// does not hold what the test waits for.
const READS: &str = "clear; ionice -c 3 cat /tmp/d/big > /devices/null & i=$!; sleep 2; a=$(cut -d ' ' \
                     -f 2 /processes/uptime); c=$(cat /tmp/d/small | wc -c | tr -d ' '); b=$(cut -d \
                     ' ' -f 2 /processes/uptime); echo \"small $c in $(( (b - a) / 1000000 )) ms\"; \
                     wait $i; echo \"idle read: $(( $? + 40 ))\"";

fn idle_reader(vm: &mut Vm, _case: &Case) -> Result<(), String> {
    vm.expect("spark: started consoled (/system/commands/consoled) as process ")?;
    vm.expect_screen(&[&format!("{RELEASE} {VERSION} (tty1)"), "# "])?;
    answer_prompt(vm, "#", SETUP)?;
    answer_prompt_within(vm, "#", "echo finished", 120)?;
    vm.expect_screen(&["ready: 42", "finished"])?;

    // The disk is slower than the readers, so that it is never free.
    vm.monitor(&format!("block_set_io_throttle disk0 {DISK_RATE} 0 0 0 0 0"))?;
    answer_prompt(
        vm,
        "#",
        &format!(
            "clear; cp {TESTS}/iow-light.toml {TESTS}/iow-heavy.toml {FILES}/ && for s in \
             iow-light iow-heavy; do service $s start -q; done; echo \"started $?\""
        ),
    )?;
    vm.expect_screen(&["started 0"])?;
    for service in ["iow-light", "iow-heavy"] {
        vm.expect_anywhere(&format!(
            "spark: started {service} (/system/tests/posix/diskread) as process "
        ))?;
    }
    vm.expect_anywhere("diskread: 16 threads read 64 MiB of /devices/vda")?;

    answer_prompt(vm, "#", READS)?;
    wait_for_screen(vm, "small 65536 in ", 30)
        .map_err(|err| format!("a read of the Btrfs waited behind an idle-class one: {err}"))?;
    answer_prompt_within(vm, "#", "echo finished", 120)
        .map_err(|err| format!("the idle-class read never went while the disk was busy: {err}"))?;
    vm.expect_screen(&["idle read: 40", "finished"])?;

    answer_prompt(
        vm,
        "#",
        "clear; for s in iow-light iow-heavy; do service $s stop -q; done; umount /tmp/d && echo \
         \"stopped $((40 + 2))\"",
    )?;
    vm.expect_screen(&["stopped 42"])?;
    vm.type_text("poweroff\n")?;
    vm.expect("spark: asked to power off the machine")?;
    vm.expect_exit(QEMU_EXIT_POWER_OFF)
}
