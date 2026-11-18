// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Many threads reading one disk raw at once, more than its queue has
//! slots for, at the console as root: 32 threads each on a SATA disk of
//! an AHCI controller, an NVMe namespace and a virtio disk, all at once,
//! each reading 1 MiB at a time, which the disk's queue carries in parts
//! (eight on AHCI, whose commands take 128 KiB). After 15 seconds of that,
//! a read of 8 MiB of each disk gets through, the readers have read
//! without an error, and each disk holds what it held. A thread that holds
//! slots of a queue never waits for more, so threads that each hold some
//! never wait on one another for ever, as sixteen did on a SATA disk
//! before. Then an idle-class read of the NVMe namespace, on the first
//! CPU, while a best-effort read goes on the second: it ends once the
//! other has, though the requests that held it back ended in the other
//! CPU's lane.

use super::super::*;

/// The disks, of 64 MiB the readers read round and round: `sda`, the
/// first SATA disk, before the boot stick; `nvme0n1`; and `vda`.
const DISKS: &[Disk] = &[
    Disk { kib: 64 * 1024, ..raw_disk(Bus::Sata, raw_unchanged) },
    Disk { kib: 64 * 1024, ..raw_disk(Bus::Nvme(0), raw_unchanged) },
    Disk { kib: 64 * 1024, ..raw_disk(Bus::Virtio, raw_unchanged) },
];

/// The disks' names, as [`DISKS`] has them.
const NAMES: [&str; 3] = ["sda", "nvme0n1", "vda"];

/// The test.
pub const CASE: Case = Case {
    name: "disk-readers",
    about: "many threads reading one disk raw at once, more than its queue holds: 32 threads each on a SATA, an NVMe and a virtio disk for 15 seconds; then a read of 8 MiB of each gets through, the readers read without an error, and the disks are unchanged; then an idle-class read of the NVMe disk on one CPU ends while a best-effort read goes on the other",
    firmware: Firmware::Uefi,
    media: Media::Usb,
    cmdline: "console-sign-in=off",
    cpus: 2,
    memory: "1G",
    reboot: false,
    image: TestImage::Plain,
    cpu_model: None,
    qemu: &[],
    script: disk_readers,
    disks: DISKS,
    second: None,
};

/// The readers of each disk start in the background, each writing what it
/// says to a file of its disk's name; after 15 seconds, `head` reads the
/// first 8 MiB of each disk while they go on, and the readers stop.
const READERS: &str = "clear; r=/system/tests/posix/diskread; $r /devices/sda 32 64 > /tmp/sda 2>&1 & \
                       a=$!; $r /devices/nvme0n1 32 64 > /tmp/nvme0n1 2>&1 & b=$!; $r /devices/vda \
                       32 64 > /tmp/vda 2>&1 & c=$!; sleep 15; for d in sda nvme0n1 vda; do echo \
                       \"$d gave $(head -c 8388608 /devices/$d | wc -c | tr -d ' ')\"; done; kill $a \
                       $b $c; echo \"stopped $?\"";

/// A best-effort read of the whole NVMe namespace on the second CPU, and
/// beside it an idle-class read of its first 8 MiB on the first.
const IDLE_READ: &str = "clear; taskset -c 1 cat /devices/nvme0n1 > /devices/null & taskset -c 0 \
                         ionice -c 3 head -c 8388608 /devices/nvme0n1 | wc -c | sed 's/^/idle read /'";

fn disk_readers(vm: &mut Vm, _case: &Case) -> Result<(), String> {
    vm.expect("spark: started consoled (/system/commands/consoled) as process ")?;
    vm.expect_screen(&[&format!("{RELEASE} {VERSION} (tty1)"), "# "])?;

    answer_prompt(vm, "#", READERS)?;
    // With the queue stuck, a read of the disk never ends, and the prompt
    // never comes back.
    answer_prompt_within(vm, "#", "echo finished", 180)
        .map_err(|err| format!("a disk's readers held its queue up: {err}"))?;
    let gave: Vec<String> = NAMES.iter().map(|name| format!("{name} gave 8388608")).collect();
    let mut wanted: Vec<&str> = gave.iter().map(String::as_str).collect();
    wanted.extend(["stopped 0", "finished"]);
    vm.expect_screen(&wanted)?;

    // What the readers said: that they started, and no read that failed,
    // which would end the reader with a line saying so.
    answer_prompt(vm, "#", "clear; cat /tmp/sda /tmp/nvme0n1 /tmp/vda; echo \"said $?\"")?;
    let started: Vec<String> =
        NAMES.iter().map(|name| format!("diskread: 32 threads read 64 MiB of /devices/{name}")).collect();
    let mut wanted: Vec<&str> = started.iter().map(String::as_str).collect();
    wanted.push("said 0");
    vm.expect_screen(&wanted)?;
    let screen = vm.screen()?;
    if let Some(line) = screen.iter().find(|line| line.contains("diskread: a read")) {
        return Err(format!("a reader's read failed: {}", line.trim_end()));
    }

    // The idle-class read waits in the first CPU's lane for the disk to
    // be empty; the best-effort one's requests go in the second's.
    answer_prompt(vm, "#", IDLE_READ)?;
    answer_prompt_within(vm, "#", "echo finished", 60)
        .map_err(|err| format!("an idle-class read never went once the disk was free: {err}"))?;
    vm.expect_screen(&["idle read 8388608", "finished"])?;

    vm.type_text("poweroff\n")?;
    vm.expect("spark: asked to power off the machine")?;
    vm.expect_exit(QEMU_EXIT_POWER_OFF)
}
