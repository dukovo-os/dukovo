// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! A copy of the test image without its GRUB, on a virtio disk: the host
//! clears its first sector's boot code and its BIOS boot partition, takes
//! the file *EFI/BOOT/BOOTX64.EFI* out of its EFI system partition, and
//! gives the copy GUIDs and a FAT serial number of its own. Under a BIOS,
//! from the test image, *rgrub-setup* must refuse a partition for GRUB
//! for a BIOS, put GRUB for a BIOS on the disk, and put UEFI's GRUB in
//! its EFI system partition's FAT, unmounted. On the host, the FAT must
//! hold the same *BOOTX64.EFI* the test image does, and dosfstools'
//! *fsck.fat* must find it sound. The second boot is from the disk
//! alone, under the BIOS: the GRUB rgrub-setup put there must boot the
//! system (ADR 0306).

use std::process::Command;

use disk::{Guid, Rng, Scheme, types};

use super::super::*;

pub const CASE: Case = Case {
    name: "rgrub-setup",
    about: "A copy of the test image without its GRUB: rgrub-setup refuses a partition, puts GRUB for a BIOS on the disk and UEFI's GRUB in its EFI system partition's FAT, which the host's fsck.fat finds sound with the test image's BOOTX64.EFI; the BIOS then boots the system from that disk alone",
    firmware: Firmware::Bios,
    media: Media::Usb,
    cmdline: "console-sign-in=off",
    cpus: 2,
    memory: "1G",
    reboot: false,
    image: TestImage::Plain,
    cpu_model: None,
    qemu: &[],
    script: rgrub_setup,
    disks: GRUBLESS_DISKS,
    second: Some(SecondBoot { cmdline: "console-sign-in=off", before: nothing, script: from_disk }),
};

/// The copy of the test image, as a virtio disk: `vda`.
const GRUBLESS_DISKS: &[Disk] = &[Disk {
    kib: 0,
    fat: GRUBLESS,
    cluster: 0,
    label: "",
    table: Table::None,
    bus: Bus::Virtio,
    sector: 512,
    device: "",
    errors: None,
    read_only: false,
    check: grub_put_back,
}];

/// Where UEFI's GRUB is on an EFI system partition.
const EFI_PATH: &str = "::/EFI/BOOT/BOOTX64.EFI";

/// The file beside the disk that keeps the test image's `BOOTX64.EFI`.
fn kept_efi(disk: &DiskImage) -> PathBuf {
    disk.path.with_extension("efi")
}

/// Makes the disk `image` of the test image `base`, with `cmdline` in
/// its command-line slots, without its GRUB, and with GUIDs and a FAT
/// serial number of its own, so that the system tells it from the test
/// image; it is its EFI system partition, for mtools.
pub fn make(base: &[u8], cmdline: &str, image: DiskImage) -> Result<DiskImage, String> {
    let path = &image.path;
    let shown = path.display().to_string();
    std::fs::write(path, with_cmdline(base, cmdline)?).map_err(|e| format!("writing {shown}: {e}"))?;
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
        .map_err(|e| format!("opening {shown}: {e}"))?;
    let device = disk::Device::whole(file, base.len() as u64, 512);
    let found = disk::read(&device).map_err(|e| format!("{shown}: {e}"))?;
    let mut table = found.table.ok_or_else(|| format!("{shown} has no partition table"))?;
    let kind = |name| types::parse(name, Scheme::Gpt).expect("a known type");
    let part = |kind| {
        table
            .partitions
            .iter()
            .find(|p| p.kind == kind)
            .map(|p| (p.first, p.sectors()))
            .ok_or_else(|| format!("{shown} has no {kind:?} partition"))
    };
    let (bios_first, bios_sectors) = part(kind("bios-boot"))?;
    let (esp_first, esp_sectors) = part(kind("efi"))?;
    let esp = DiskImage { offset: esp_first * 512, bytes: esp_sectors * 512, ..image };

    let mtools = mtools_image(&esp);
    let kept = kept_efi(&esp);
    host_tool(Command::new("mcopy").args(["-n", "-i", &mtools, EFI_PATH]).arg(&kept))?;
    host_tool(Command::new("mdel").args(["-i", &mtools, EFI_PATH]))?;

    let written = |e: std::io::Error| format!("writing {shown}: {e}");
    device.zero(bios_first * 512, bios_sectors * 512).map_err(written)?;
    // New GUIDs, and the protective MBR with no boot code.
    let mut rng = Rng::seeded(0x6772_7562);
    table.disk_guid = Guid::random(&mut rng);
    for partition in &mut table.partitions {
        partition.guid = Guid::random(&mut rng);
    }
    table.boot_code = vec![0; 440];
    let writes = disk::plan(&device, &table).map_err(written)?;
    disk::apply(&device, &writes).map_err(written)?;
    // FAT32's serial number, in its boot sector and the backup's.
    let boot = device.read_sectors(esp_first, 1).map_err(written)?;
    if &boot[82..90] != b"FAT32   " {
        return Err(format!("{shown}'s EFI system partition is not FAT32"));
    }
    let backup = u64::from(u16::from_le_bytes([boot[50], boot[51]]));
    for sector in [esp_first, esp_first + backup] {
        device.write_at(sector * 512 + 67, &0x5247_5542u32.to_le_bytes()).map_err(written)?;
    }
    device.sync().map_err(written)?;
    Ok(esp)
}

/// The host's side, after each boot: the EFI system partition holds the
/// test image's `BOOTX64.EFI` again, `fsck.fat` finds it sound, and the
/// disk's first sector holds GRUB's `boot.img`.
fn grub_put_back(disk: &DiskImage, _log: &str) -> Result<(), String> {
    let back = disk.path.with_extension("back");
    let _ = std::fs::remove_file(&back);
    host_tool(Command::new("mcopy").args(["-n", "-i", &mtools_image(disk), EFI_PATH]).arg(&back))?;
    let read = |path: &Path| std::fs::read(path).map_err(|e| format!("reading {}: {e}", path.display()));
    let same = read(&back)? == read(&kept_efi(disk))?;
    let _ = std::fs::remove_file(&back);
    if !same {
        return Err("the EFI system partition's BOOTX64.EFI is not the test image's".into());
    }

    let mut file =
        std::fs::File::open(&disk.path).map_err(|e| format!("opening {}: {e}", disk.path.display()))?;
    let mut first = [0u8; 512];
    file.read_exact(&mut first).map_err(|e| format!("reading {}: {e}", disk.path.display()))?;
    // boot.img starts with its jump over the parameter block.
    if first[..2] != [0xeb, 0x63] || first[510..] != [0x55, 0xaa] {
        return Err("the disk's first sector holds no GRUB boot.img".into());
    }
    let copy = disk.path.with_extension("esp");
    let mut out =
        std::fs::File::create(&copy).map_err(|e| format!("making {}: {e}", copy.display()))?;
    image::copy_sparse(&mut file, disk.offset, &mut out, 0, disk.bytes)
        .and_then(|()| out.set_len(disk.bytes))
        .map_err(|e| format!("copying the EFI system partition to {}: {e}", copy.display()))?;
    let checked = host_tool(Command::new(fat_tool("fsck.fat")).arg("-n").arg(&copy));
    let _ = std::fs::remove_file(&copy);
    checked.map(drop)
}

/// Nothing to do on the host before the second boot.
fn nothing(_case: &Case, _disks: &[DiskImage]) -> Result<(), String> {
    Ok(())
}

/// The `rgrub-setup` test's first boot, from the test image.
fn rgrub_setup(vm: &mut Vm, _case: &Case) -> Result<(), String> {
    vm.expect("spark: started consoled (/system/commands/consoled) as process ")?;
    vm.expect_screen(&[&format!("{RELEASE} {VERSION} (tty1)"), "# "])?;

    vm.type_text("rgrub-setup vda2; echo \"status: $?\"\n")?;
    vm.expect_screen(&["/devices/vda2 is a partition; GRUB for a BIOS", "status: 1"])?;
    vm.type_text("rgrub-setup vda && echo \"bios: $((40 + 2))\"\n")?;
    vm.expect_screen(&["Installed GRUB for a BIOS on /devices/vda:", "bios: 42"])?;
    vm.type_text("rgrub-setup --efi vda2 && echo \"uefi: $((40 + 3))\"\n")?;
    vm.expect_screen(&["Installed GRUB for UEFI on /devices/vda2: EFI/BOOT/BOOTX64.EFI", "uefi: 43"])?;

    vm.type_text("poweroff\n")?;
    vm.expect("spark: asked to power off the machine")?;
    vm.expect_exit(QEMU_EXIT_POWER_OFF)
}

/// The second boot, from the disk alone: GRUB starts the system.
fn from_disk(vm: &mut Vm, _case: &Case) -> Result<(), String> {
    vm.expect("spark: started consoled (/system/commands/consoled) as process ")?;
    vm.expect_screen(&[&format!("{RELEASE} {VERSION} (tty1)"), "# "])?;
    vm.type_text("poweroff\n")?;
    vm.expect("spark: asked to power off the machine")?;
    vm.expect_exit(QEMU_EXIT_POWER_OFF)
}
