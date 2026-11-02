// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! GRUB put on a disk for a PC's BIOS, as GRUB's grub-bios-setup puts it
//! (ADR 0306), for rgrub-setup(8) and `./ob image`: GRUB's `boot.img` in
//! the boot code of the disk's first sector, which the BIOS runs, and
//! GRUB's `core.img` in the disk's GPT's BIOS boot partition, which
//! `boot.img` loads.
//!
//! The places are those of GRUB 2's `include/grub/i386/pc/boot.h`:
//! `boot.img` keeps the disk's own bytes where a DOS boot sector has its
//! parameter block and from the disk signature to the end of the
//! partition table, and is told the first sector of `core.img`; the
//! first sector of `core.img`, GRUB's `diskboot.img`, is told where the
//! rest of it is, as one run of sectors. No Reed-Solomon codes are added
//! after `core.img`, as `grub-bios-setup --no-rs-codes` leaves them out.

use crate::device::Device;
use crate::table::Scheme;
use crate::types;

/// A sector as a BIOS reads it.
const SECTOR: usize = 512;

/// Where a DOS boot sector's parameter block is, which `boot.img` leaves
/// as the disk has it (`GRUB_BOOT_MACHINE_BPB_START` and `_END`).
const BPB: std::ops::Range<usize> = 0x03..0x5a;

/// Where `boot.img` is told the first sector of `core.img`
/// (`GRUB_BOOT_MACHINE_KERNEL_SECTOR`), a little-endian u64.
const KERNEL_SECTOR: usize = 0x5c;

/// The jump `boot.img` takes when the BIOS gave it a floppy's drive
/// number, which a hard disk's `boot.img` does not take, for BIOSes that
/// give a wrong one (`GRUB_BOOT_MACHINE_DRIVE_CHECK`).
const DRIVE_CHECK: usize = 0x66;

/// From the disk signature to the end of the partition table: the
/// disk's own (`GRUB_BOOT_MACHINE_WINDOWS_NT_MAGIC` to `_PART_END`).
const TABLE: std::ops::Range<usize> = 0x1b8..0x1fe;

/// Where `diskboot.img` keeps the run of sectors of the rest of
/// `core.img`: its first sector (u64), how many (u16), and the segment
/// they are loaded at (u16), at the end of its sector.
const BLOCKLIST: usize = SECTOR - 12;

/// The segment the rest of `core.img` is loaded at: after its first
/// sector, at 0x8000 (`GRUB_BOOT_I386_PC_KERNEL_SEG` + 0x20).
const SEGMENT: u16 = 0x0820;

/// Where GRUB was put.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Setup {
    /// The first sector of `core.img`.
    pub core_sector: u64,
    /// How many sectors `core.img` takes.
    pub core_sectors: u64,
    /// The BIOS boot partition it is in.
    pub partition: u32,
}

fn u16_at(bytes: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([bytes[at], bytes[at + 1]])
}

/// Checks that `boot` and `core` are GRUB's `boot.img` and a `core.img`
/// for a disk (`grub-mkimage -O i386-pc`).
fn check_images(boot: &[u8], core: &[u8]) -> Result<(), String> {
    if boot.len() != SECTOR || boot[510..512] != [0x55, 0xaa] {
        return Err("boot.img is not GRUB's: it is one sector that ends 55 AA".into());
    }
    if core.len() < 2 * SECTOR || u16_at(core, BLOCKLIST + 10) != SEGMENT {
        return Err("core.img is not GRUB's for a disk (grub-mkimage -O i386-pc)".into());
    }
    Ok(())
}

/// Puts GRUB on `disk` for its BIOS: `core` in its GPT's BIOS boot
/// partition, and `boot` in its first sector, which keeps the protective
/// MBR's table.
///
/// # Errors
///
/// If the images are not GRUB's, the disk has no GPT, no BIOS boot
/// partition as large as `core`, or sectors other than 512 bytes, or it
/// cannot be written.
pub fn setup_bios(disk: &Device, boot: &[u8], core: &[u8]) -> Result<Setup, String> {
    check_images(boot, core)?;
    if disk.sector_size() as usize != SECTOR {
        return Err(format!(
            "the disk has sectors of {} bytes; a BIOS boots disks of 512-byte sectors",
            disk.sector_size()
        ));
    }
    let found = crate::read(disk).map_err(|e| format!("cannot read the partition table: {e}"))?;
    let table = found.table.ok_or("the disk has no partition table")?;
    if table.scheme != Scheme::Gpt {
        return Err("the disk has an MBR, not a GPT with a BIOS boot partition".into());
    }
    let bios_boot = types::parse("bios-boot", Scheme::Gpt).ok_or("no BIOS boot partition type")?;
    let part = table
        .partitions
        .iter()
        .find(|p| p.kind == bios_boot)
        .ok_or("the GPT has no BIOS boot partition for GRUB's core.img")?;
    let (first, room) = (part.first, part.sectors());
    let sectors = core.len().div_ceil(SECTOR) as u64;
    if sectors > room {
        return Err(format!(
            "core.img takes {sectors} sectors, and the BIOS boot partition has {room}"
        ));
    }
    let mut image = core.to_vec();
    image.resize(sectors as usize * SECTOR, 0);
    image[BLOCKLIST..BLOCKLIST + 8].copy_from_slice(&(first + 1).to_le_bytes());
    image[BLOCKLIST + 8..BLOCKLIST + 10].copy_from_slice(&((sectors - 1) as u16).to_le_bytes());
    image[BLOCKLIST + 10..BLOCKLIST + 12].copy_from_slice(&SEGMENT.to_le_bytes());
    let written = |e: std::io::Error| format!("cannot write the disk: {e}");
    disk.write_sectors(first, &image).map_err(written)?;

    let old = disk.read_sectors(0, 1).map_err(|e| format!("cannot read the disk: {e}"))?;
    let mut mbr = boot.to_vec();
    mbr[BPB].copy_from_slice(&old[BPB]);
    mbr[TABLE].copy_from_slice(&old[TABLE]);
    mbr[KERNEL_SECTOR..KERNEL_SECTOR + 8].copy_from_slice(&first.to_le_bytes());
    mbr[DRIVE_CHECK..DRIVE_CHECK + 2].copy_from_slice(&[0x90, 0x90]);
    disk.write_sectors(0, &mbr).map_err(written)?;
    disk.sync().map_err(written)?;
    Ok(Setup { core_sector: first, core_sectors: sectors, partition: part.number })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn images_are_checked() {
        let mut boot = vec![0u8; SECTOR];
        boot[510] = 0x55;
        boot[511] = 0xaa;
        let mut core = vec![0u8; 3 * SECTOR];
        core[BLOCKLIST + 10..BLOCKLIST + 12].copy_from_slice(&SEGMENT.to_le_bytes());
        assert!(check_images(&boot, &core).is_ok());
        assert!(check_images(&boot[..400], &core).is_err());
        assert!(check_images(&boot, &core[..SECTOR]).is_err());
        core[BLOCKLIST + 10] = 0;
        assert!(check_images(&boot, &core).is_err());
    }
}
