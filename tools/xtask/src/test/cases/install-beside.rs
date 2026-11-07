// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Installing next to Windows under UEFI (ADR 0314). The virtio disk is
//! laid out as Windows lays a disk out: its EFI system partition holding
//! a stand-in for Windows Boot Manager, its reserved partition, C:, an
//! NTFS of 32 GiB that *mkntfs* made and *ntfscp* wrote files to, and
//! its recovery partition. The install stick boots into the live system,
//! and *dukovo-install --answers* installs Dukovo next to Windows as the
//! test's answers file says: *resize-ntfs* shrinks C: to 12 GiB, the
//! partition is cut to match, and Dukovo's own EFI system partition, its
//! swap and its root take the 20 GiB given back. The installer makes a
//! UEFI boot entry, *Dukovo*, first in the firmware's order. On the
//! host, Windows' partitions must be kept, its boot files untouched, and
//! ntfs-3g must find C: sound, of the size asked for and marked for
//! Windows to check, with every file as the host wrote it. The second
//! boot is from that disk alone, by the firmware's entry *Dukovo*: GRUB's
//! menu must offer *Windows Boot Manager* and the firmware's settings and
//! start Dukovo, where alice signs in and finds that GRUB remembered
//! Dukovo in its environment block.

use std::path::Path;
use std::process::Command;

use super::super::*;
use super::install_uefi::{
    PASSWORD, holds_install, nothing, power_off, signed_in, wait_for_screen,
};

pub const CASE: Case = Case {
    name: "install-beside",
    about: "The live system installs Dukovo next to Windows under UEFI from an answers file: C: shrunk from 32 to 12 GiB by resize-ntfs, Dukovo's partitions in the space it gave, a UEFI boot entry made; the host finds Windows' partitions kept, its boot files untouched and C: sound with its files, and the disk then boots by that entry, where GRUB's menu offers Windows Boot Manager, alice signs in, and GRUB remembered Dukovo",
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
    disks: WINDOWS_DISKS,
    second: Some(SecondBoot { cmdline: "", before: nothing, script: installed }),
};

/// The Windows disk, `vda`: sparse, so that the host keeps only what is
/// written to it.
const WINDOWS_DISKS: &[Disk] = &[Disk {
    kib: DISK_MIB * 1024,
    fat: INSTALL_BESIDE_WINDOWS,
    cluster: CLUSTER,
    label: "Windows",
    table: Table::None,
    bus: Bus::Virtio,
    sector: 512,
    device: "",
    errors: None,
    read_only: false,
    check: beside_disk,
}];

/// Windows' partitions, in order, as `sgdisk` makes them: number,
/// first MiB, size in MiB, type code and name. C: is partition 3.
const WINDOWS_PARTITIONS: &[(u32, u64, u64, &str, &str)] = &[
    (1, 1, 100, "ef00", "EFI system partition"),
    (2, 101, 16, "0c01", "Microsoft reserved partition"),
    (3, C_MIB, 32 << 10, "0700", "Basic data partition"),
    (4, C_MIB + (32 << 10), 512, "2700", "Windows recovery"),
];

/// Where C: starts, in MiB.
const C_MIB: u64 = 117;

/// The disk's size: Windows' partitions and a MiB for the GPT's copy.
const DISK_MIB: u64 = C_MIB + (32 << 10) + 512 + 1;

/// The size of C:'s clusters.
const CLUSTER: u32 = 4096;

/// What the answers file gives Dukovo: C: is shrunk by this.
const SPACE_MIB: u64 = 20 << 10;

/// Where Windows Boot Manager is on Windows' EFI system partition, and
/// what the test's stand-in for it holds: GRUB's menu looks for the
/// file, and never starts it here.
const BOOT_MANAGER: &str = "::/EFI/Microsoft/Boot/bootmgfw.efi";
const STAND_IN: &[u8] = b"A stand-in for Windows Boot Manager, which the test never starts.\n";

/// The disk after the install, by partition number: Windows' four, then
/// Dukovo's EFI system partition, swap and root.
const BESIDE: &[&str] = &[
    "efi",
    "windows-reserved",
    "windows-data",
    "windows-recovery",
    "efi",
    "swap",
    "dukovo-root",
];

/// The test's answers file: the layout next to Windows on `vda`, 20 GiB
/// for Dukovo with 1 GiB of swap, and otherwise install-uefi's answers.
const ANSWERS: &str = "/system/tests/install/beside.json";

/// How long the install may take, in seconds: it shrinks C: and copies
/// the whole system.
const INSTALL_SECONDS: u64 = 300;

/// Makes the Windows disk at `path`: its GPT by `sgdisk`, the EFI system
/// partition's FAT by `mkfs.fat` with the stand-in for Windows Boot
/// Manager in it by mtools, and C: by `mkntfs` and `ntfscp` in a file of
/// its own, copied into its partition by `dd` with the holes left holes.
pub fn make(disk: &Disk, path: &Path) -> Result<(), String> {
    let shown = path.display();
    std::fs::File::create(path)
        .and_then(|file| file.set_len(disk.kib * 1024))
        .map_err(|err| format!("making {shown}: {err}"))?;
    let mut sgdisk = Command::new(fat_tool("sgdisk"));
    sgdisk.arg("--clear");
    for &(number, first, mib, code, name) in WINDOWS_PARTITIONS {
        let (first, last) = (first << 11, ((first + mib) << 11) - 1);
        sgdisk.arg(format!("--new={number}:{first}:{last}"));
        sgdisk.arg(format!("--typecode={number}:{code}"));
        sgdisk.arg(format!("--change-name={number}:{name}"));
    }
    host_tool(sgdisk.arg(path))?;

    let esp = format!("{shown}@@{}", 1u64 << 20);
    let mut mkfs = Command::new(fat_tool("mkfs.fat"));
    mkfs.args(["-F", "32", "-n", "SYSTEM", "-i", "4f525448", "-S", "512", "--offset", "2048"]);
    host_tool(mkfs.arg(path).arg((100u64 << 10).to_string()))?;
    let source = path.with_extension("bootmgfw");
    std::fs::write(&source, STAND_IN).map_err(|err| format!("writing {}: {err}", source.display()))?;
    let copied = host_tool(Command::new("mmd").args(["-i", &esp, "::/EFI"]))
        .and_then(|_| host_tool(Command::new("mmd").args(["-i", &esp, "::/EFI/Microsoft"])))
        .and_then(|_| host_tool(Command::new("mmd").args(["-i", &esp, "::/EFI/Microsoft/Boot"])))
        .and_then(|_| {
            host_tool(Command::new("mcopy").args(["-i", &esp]).arg(&source).arg(BOOT_MANAGER))
        });
    let _ = std::fs::remove_file(&source);
    copied?;

    let c = path.with_extension("c");
    std::fs::File::create(&c)
        .and_then(|file| file.set_len(32 << 30))
        .map_err(|err| format!("making {}: {err}", c.display()))?;
    let made = (|| {
        let mut mkntfs = Command::new(fat_tool("mkntfs"));
        mkntfs.args(["-q", "-F", "-Q", "-L", disk.label, "-s", "512"]);
        // Where C: starts, and the disk geometry Windows writes.
        mkntfs.args(["-c", &disk.cluster.to_string(), "-p", &(C_MIB << 11).to_string()]);
        mkntfs.args(["-H", "255", "-S", "63"]);
        // A file has no size mkntfs can ask for: its sectors are named.
        host_tool(mkntfs.arg(&c).arg(((32u64 << 30) / 512).to_string()))?;
        let file = c.with_extension("file");
        for (name, made) in ntfs_files() {
            std::fs::write(&file, made_bytes(&made))
                .map_err(|err| format!("writing {}: {err}", file.display()))?;
            // ntfscp takes names as the locale has them.
            let mut ntfscp = Command::new(fat_tool("ntfscp"));
            ntfscp.env("LC_ALL", "C.UTF-8").arg("-q").arg(&c).arg(&file);
            host_tool(ntfscp.arg(format!("/{name}")))?;
        }
        let _ = std::fs::remove_file(&file);
        let mut dd = Command::new("dd");
        dd.arg(format!("if={}", c.display())).arg(format!("of={shown}"));
        host_tool(dd.args(["bs=1M", &format!("seek={C_MIB}"), "conv=notrunc,sparse", "status=none"]))
            .map(drop)
    })();
    let _ = std::fs::remove_file(&c);
    made
}

/// The first boot, from the stick: the live system installs Dukovo next
/// to Windows on `vda` as the answers file says, and its boot entry is
/// first in the firmware's order; then it powers off.
fn install(vm: &mut Vm, _case: &Case) -> Result<(), String> {
    vm.expect("spark: the live system: /system is the boot archive, in memory, and no disk is the root")?;
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
        "dukovo-install: Shrinking Windows (C:) on vda",
        "is installed on vda. Remove the install stick, then restart.",
    ])?;

    // The entry the installer made is the firmware's first.
    vm.type_text(
        "clear; efiboot > /tmp/e; n=$(awk '/^Boot[0-9A-F]+\\* Dukovo$/ { print substr($1, 5, 4) }' \
         /tmp/e); [ -n \"$n\" ] && grep -q \"^BootOrder: $n\" /tmp/e; echo \"first: $((30 + $?))\"\n",
    )?;
    vm.expect_screen(&["first: 30"])?;

    vm.type_text("poweroff\n")?;
    vm.expect("spark: asked to power off the machine")?;
    vm.expect_exit(QEMU_EXIT_POWER_OFF)
}

/// The second boot, from the disk alone: the firmware starts its entry
/// *Dukovo*, on Dukovo's EFI system partition, partition 5; GRUB's menu
/// offers Windows and the firmware's settings and starts Dukovo, where
/// alice signs in. GRUB wrote the entry it started to its environment
/// block, which alice reads through sudo: the host cannot, since this
/// boot's writes never reach the disk's file (QEMU's `snapshot=on`).
fn installed(vm: &mut Vm, _case: &Case) -> Result<(), String> {
    vm.expect("\"Dukovo\" from HD(5,GPT,")?;
    vm.expect("Windows Boot Manager")?;
    vm.expect("UEFI firmware settings")?;
    vm.expect("Booting `Dukovo Server")?;
    signed_in(vm)?;
    // A script, so that sudo's prompt, which names the command, fits on
    // its line; in alice's home, since root has a /tmp of its own.
    answer_prompt(
        vm,
        "$",
        "clear; echo 'mkdir -p /tmp/esp && mount -t fat sda5 /tmp/esp && head -n 2 \
         /tmp/esp/boot/grub/grubenv; umount /tmp/esp' > ~/env.sh; sudo sh ~/env.sh",
    )?;
    answer_prompt(vm, "env.sh:", PASSWORD)?;
    vm.expect_screen(&["saved_entry=dukovo"])?;
    power_off(vm)
}

/// The host's side, after each boot: Dukovo installed in the space C:
/// gave ([`holds_install`]), Windows' boot files untouched, and C: shrunk,
/// sound and marked for Windows to check, with its files.
fn beside_disk(disk: &DiskImage, log: &str) -> Result<(), String> {
    holds_install(disk, log, BESIDE)?;
    let shown = disk.path.display();
    let esp = format!("{shown}@@{}", 1u64 << 20);
    let manager = host_tool(Command::new("mtype").args(["-i", &esp, BOOT_MANAGER]))
        .map_err(|err| format!("Windows' EFI system partition lost its boot manager: {err}"))?;
    if manager != STAND_IN {
        return Err(format!("{shown}: Windows Boot Manager was changed"));
    }
    shrunk_c(disk)
}

/// Checks C: on the host, on a copy of its partition: 12 GiB, the size
/// it was shrunk to, which ntfs-3g's `ntfsinfo` must find it, the copy of
/// its boot sector last; marked for Windows to check at its next start,
/// as after `ntfsresize`; sound to `ntfsresize --info`; and each of
/// [`ntfs_files`] read by `ntfscat` as the host wrote it.
fn shrunk_c(disk: &DiskImage) -> Result<(), String> {
    let shown = disk.path.display();
    let file = std::fs::File::open(&disk.path).map_err(|e| format!("opening {shown}: {e}"))?;
    let device = disk::Device::whole(file, disk.bytes, 512);
    let found = disk::read(&device).map_err(|e| format!("{shown}: {e}"))?;
    let c = found.table.as_ref().and_then(|table| table.get(3)).ok_or("no partition 3")?;
    let mib = (32 << 10) - SPACE_MIB;
    if c.first != C_MIB << 11 || c.sectors() != mib << 11 {
        return Err(format!(
            "{shown}: C:'s partition is {} sectors at {}, not {} at {}",
            c.sectors(),
            c.first,
            mib << 11,
            C_MIB << 11
        ));
    }
    let copy = disk.path.with_extension("c");
    let _ = std::fs::remove_file(&copy);
    let mut dd = Command::new("dd");
    dd.arg(format!("if={shown}")).arg(format!("of={}", copy.display()));
    dd.args(["bs=1M", &format!("skip={C_MIB}"), &format!("count={mib}"), "conv=sparse"]);
    let checked = host_tool(dd.arg("status=none")).and_then(|_| ntfs_c(&copy, mib << 20));
    let _ = std::fs::remove_file(&copy);
    checked
}

/// Checks the copy of C: at `path`, of `bytes`, as [`shrunk_c`] says.
fn ntfs_c(path: &Path, bytes: u64) -> Result<(), String> {
    let shown = path.display();
    let mut resize = Command::new(fat_tool("ntfsresize"));
    let info = host_tool(resize.env("LC_ALL", "C").args(["-i", "-f", "-n"]).arg(path))
        .map_err(|err| format!("C: fails ntfs-3g's ntfsresize --info: {err}"))?;
    if !String::from_utf8_lossy(&info).contains("Accounting clusters") {
        return Err(format!("{shown}: ntfsresize did not count C:'s clusters"));
    }
    let info = host_tool(Command::new(fat_tool("ntfsinfo")).args(["-f", "-m"]).arg(path))
        .map_err(|err| format!("C:: ntfsinfo: {err}"))?;
    let info = String::from_utf8_lossy(&info);
    let clusters = (bytes - 512) / u64::from(CLUSTER);
    let size = format!("Volume Size in Clusters: {clusters}");
    if !info.contains(&size) {
        return Err(format!("C: on the host: ntfsinfo -m does not say \"{size}\""));
    }
    let flags = info
        .lines()
        .find_map(|line| line.trim().strip_prefix("Volume Flags:"))
        .and_then(|flags| flags.split_whitespace().next())
        .and_then(|flags| u16::from_str_radix(flags.trim_start_matches("0x"), 16).ok())
        .ok_or("C: on the host: ntfsinfo gives no volume flags")?;
    // `$Volume`'s dirty flag: Windows checks the volume at its next start.
    if flags & 1 == 0 {
        return Err("C: is not marked for Windows to check after its shrink".into());
    }
    for (name, made) in ntfs_files() {
        let mut ntfscat = Command::new(fat_tool("ntfscat"));
        ntfscat.env("LC_ALL", "C.UTF-8").arg("-f").arg(path).arg(format!("/{name}"));
        let read = host_tool(&mut ntfscat).map_err(|err| format!("C:: ntfscat {name}: {err}"))?;
        if read != made_bytes(&made) {
            return Err(format!("C: on the host: ntfscat reads other bytes of {name}"));
        }
    }
    Ok(())
}
