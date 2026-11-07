// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The mirror layout under UEFI (ADR 0315). The install stick boots
//! beside two blank virtio disks, and *dukovo-install --answers* installs
//! Dukovo onto both, as the test's answers file says: each gets the
//! guided layout's partitions and GRUB, and the two roots are one Btrfs
//! RAID1. On the host, each disk holds the layout, its own GRUB and a
//! menu naming the root, and Linux's *btrfs check* finds the RAID1 sound
//! from both disks and from each alone.
//!
//! Then the first disk dies: it is taken out, and the second goes in its
//! place, with a blank disk beside it, as a person would replace a
//! failed one. The firmware starts the second disk's own GRUB, the kernel
//! mounts the root from it alone and warns, and alice signs in, finds the
//! system as the install left it, and writes a file, which the one disk
//! left now holds.

use super::super::*;
use super::install_uefi::{holds_install, power_off, signed_in, wait_for_screen};

pub const CASE: Case = Case {
    name: "install-mirror",
    about: "The live system installs Dukovo under UEFI onto two blank disks as a mirror, from an answers file: each disk gets the guided layout and GRUB, the roots one Btrfs RAID1 that Linux's btrfs check finds sound from both disks and from each alone; with the first disk taken out, the second starts by itself, the kernel warns of the disk missing, and alice signs in and writes a file",
    firmware: Firmware::Uefi,
    media: Media::Usb,
    cmdline: "",
    cpus: 2,
    memory: "1G",
    reboot: false,
    image: TestImage::Live,
    cpu_model: None,
    qemu: &[],
    script: install,
    disks: MIRROR_DISKS,
    second: Some(SecondBoot { cmdline: "", before: first_disk_dies, script: from_the_second }),
};

/// The two blank disks, `vda` and `vdb`, 4 GiB each as install-uefi's.
/// The second boot starts from the first's place.
const MIRROR_DISKS: &[Disk] = &[MIRROR_DISK, MIRROR_DISK];

/// A disk of the mirror.
const MIRROR_DISK: Disk = Disk {
    kib: 4 * 1024 * 1024,
    fat: INSTALL_TARGET,
    cluster: 0,
    label: "",
    table: Table::None,
    bus: Bus::Virtio,
    sector: 512,
    device: "",
    errors: None,
    read_only: false,
    check: mirrored_disk,
};

/// The test's answers file: the mirror layout of `vda` and `vdb`, with
/// 1 GiB of swap on each, and otherwise install-uefi's answers.
const ANSWERS: &str = "/system/tests/install/mirror.json";

/// The partitions each disk holds: the guided layout's.
const GUIDED: &[&str] = &["bios-boot", "efi", "swap", "dukovo-root"];

/// How long the install may take, in seconds: it copies the whole system,
/// then the balance writes it on both disks.
const INSTALL_SECONDS: u64 = 300;

/// What the kernel says as it mounts the root with the first disk gone.
const ONE_MISSING: &str = "1 of its 2 disks missing; mounted from the copies on the others";

/// The first boot, from the stick: the install onto both disks.
fn install(vm: &mut Vm, _case: &Case) -> Result<(), String> {
    vm.expect(
        "spark: the live system: /system is the boot archive, in memory, and no disk is the root",
    )?;
    vm.expect("spark: started consoled (/system/commands/consoled) as process ")?;
    vm.expect_screen(&[&format!("{RELEASE} {VERSION} (tty1)"), "# "])?;

    vm.type_text(&format!(
        "dukovo-install --answers {ANSWERS} > /tmp/out 2>&1; echo \"install: $((100 + $?))\"\n"
    ))?;
    wait_for_screen(vm, "install: 1", INSTALL_SECONDS)?;
    if !vm.screen()?.iter().any(|line| line.trim() == "install: 100") {
        vm.type_text("clear; tail -n 8 /tmp/install.log\n")?;
        let _ = vm.expect_screen(&["problem: "]);
        return Err(format!("dukovo-install failed: {}", vm.screen()?.join(" | ")));
    }
    vm.type_text("clear; cat /tmp/out\n")?;
    vm.expect_screen(&[
        "dukovo-install: Partitioning vda and vdb",
        "dukovo-install: Mirroring the root on both disks (Btrfs RAID1)",
        "is installed on vda and vdb. Remove the install stick, then restart.",
    ])?;

    // The log shows the second root added and every chunk converted.
    vm.type_text(
        "clear; grep -q 'the root is a Btrfs RAID1 of vda4 and vdb4' /tmp/install.log && \
         grep -q 'Done, had to relocate' /tmp/install.log; echo \"mirrored: $((10 + $?))\"\n",
    )?;
    vm.expect_screen(&["mirrored: 10"])?;

    vm.type_text("poweroff\n")?;
    vm.expect("spark: asked to power off the machine")?;
    vm.expect_exit(QEMU_EXIT_POWER_OFF)
}

/// Between the boots: the first disk dies and is taken out, and the
/// second goes in its place, with a blank disk where it was. The second
/// boot starts from the case's first disk, which now holds what the
/// second did.
fn first_disk_dies(_case: &Case, disks: &[DiskImage]) -> Result<(), String> {
    let [first, second] = disks else { return Err("the case has two disks".into()) };
    std::fs::rename(&second.path, &first.path)
        .and_then(|()| std::fs::File::create(&second.path))
        .and_then(|blank| blank.set_len(second.bytes))
        .map_err(|err| {
            format!("moving {} into {}'s place: {err}", second.path.display(), first.path.display())
        })
}

/// The second boot, from the second disk alone, in the first's place: the
/// firmware starts its own GRUB, the root mounts from it with a warning,
/// and alice signs in, finds the install's settings and swap, and writes
/// a file to her home.
fn from_the_second(vm: &mut Vm, _case: &Case) -> Result<(), String> {
    vm.expect("Booting `Dukovo")?;
    vm.expect_anywhere(ONE_MISSING)?;
    signed_in(vm)?;
    answer_prompt(
        vm,
        "$",
        "echo kept > ~/one-disk.txt && grep -q kept ~/one-disk.txt; echo \"wrote: $((40 + $?))\"",
    )?;
    vm.expect_screen(&["wrote: 40"])?;
    power_off(vm)
}

/// The host's side, after each boot. After the install, each disk holds
/// the guided layout and its own GRUB, its EFI system partition sound
/// with a menu naming the root, which Linux's `btrfs check` finds sound
/// from that disk alone; and from the second, the RAID1 of both, whose
/// superblocks name two disks. After the first disk died, the disk in
/// its place holds the second's install, and the blank one nothing.
fn mirrored_disk(disk: &DiskImage, log: &str) -> Result<(), String> {
    let after_the_death = log.contains(ONE_MISSING);
    let image = disk.path.display().to_string();
    let prefix = image.rsplit_once("-disk").map_or(image.as_str(), |(prefix, _)| prefix);
    let second = PathBuf::from(format!("{prefix}-disk1.img"));
    if after_the_death && disk.path == second {
        return Ok(());
    }
    holds_install(disk, log, GUIDED)?;
    if after_the_death || disk.path != second {
        return Ok(());
    }
    let first = PathBuf::from(format!("{prefix}-disk0.img"));
    both_roots_checked(disk, &[first, second])
}

/// Checks the RAID1 of the two disks' roots, the last partition of each
/// of `paths`, with Linux's `btrfs check` given both: the copies of each
/// root are checked beside the disks, and each root's superblock must
/// name the same filesystem and two disks.
fn both_roots_checked(disk: &DiskImage, paths: &[PathBuf]) -> Result<(), String> {
    // A Btrfs superblock's number of disks, past its filesystem's UUID.
    const NUM_DEVICES: u64 = 0x88;
    let tool = disk.btrfs.as_ref().ok_or("a Btrfs disk was made without the btrfs tool")?;
    let mut copies = Vec::new();
    let checked = (|| {
        let mut id = None;
        for path in paths {
            let shown = path.display().to_string();
            let file = std::fs::File::open(path).map_err(|e| format!("opening {shown}: {e}"))?;
            let device = disk::Device::whole(file, disk.bytes, 512);
            let found = disk::read(&device).map_err(|e| format!("{shown}: {e}"))?;
            let table = found.table.ok_or_else(|| format!("{shown} has no partition table"))?;
            let root = table.partitions.last().ok_or_else(|| format!("{shown} has no root"))?;
            let copy = path.with_extension("root");
            let mut from =
                std::fs::File::open(path).map_err(|e| format!("opening {shown}: {e}"))?;
            let mut to = std::fs::File::create(&copy)
                .map_err(|e| format!("making {}: {e}", copy.display()))?;
            copies.push(copy.clone());
            to.set_len(root.sectors() * 512)
                .and_then(|()| {
                    image::copy_sparse(
                        &mut from,
                        root.first * 512,
                        &mut to,
                        0,
                        root.sectors() * 512,
                    )
                })
                .map_err(|e| format!("copying the root of {shown}: {e}"))?;
            let fsid =
                btrfs_id(&copy).ok_or_else(|| format!("the root of {shown} holds no Btrfs"))?;
            if id.is_some_and(|id| id != fsid) {
                return Err("the two roots are not one Btrfs".to_string());
            }
            id = Some(fsid);
            let mut count = [0u8; 8];
            let copied = std::fs::File::open(&copy)
                .map_err(|e| format!("opening {}: {e}", copy.display()))?;
            std::os::unix::fs::FileExt::read_exact_at(
                &copied,
                &mut count,
                BTRFS_SUPERBLOCK + NUM_DEVICES,
            )
            .map_err(|e| format!("reading {}: {e}", copy.display()))?;
            if u64::from_le_bytes(count) != 2 {
                return Err(format!(
                    "the root of {shown} is a Btrfs of {} disks, not 2",
                    u64::from_le_bytes(count)
                ));
            }
        }
        let id = id.ok_or("no roots")?;
        btrfs_check_of(tool, &id, &copies, &disk.path.with_extension("blkid"))
    })();
    for copy in &copies {
        let _ = std::fs::remove_file(copy);
    }
    checked
}
