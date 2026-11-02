// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! rgrub-setup(8) (ADR 0306): GRUB put on a disk for a PC's BIOS, or on
//! an EFI system partition for UEFI, from the images of GRUB the system
//! carries in /system/boot/grub, which `./ob` builds from GRUB's pinned
//! source: what grub-install does, with no GRUB tools.
//!
//! For a BIOS, `boot.img` goes in the boot code of the disk's first
//! sector and `core.img` in its GPT's BIOS boot partition, as the disk
//! engine puts them ([`disk::grub`]); that `core.img` finds GRUB's files
//! in `/boot/grub` on the disk's second partition, its EFI system
//! partition, as Dukovo's disks have it. For UEFI, `BOOTX64.EFI` goes in
//! `EFI/BOOT` of an EFI system partition, the path firmware boots a disk
//! by when it has no entry of its own for it, written into its FAT by
//! the kernel's FAT crate, as the system leaves the partition unmounted
//! ([`disk::esp`]); that GRUB finds its files in `/boot/grub` on the
//! partition it was loaded from.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use disk::{Scheme, types};

use crate::args::{Arg, Args};
use crate::blockdev;
use crate::probe;
use crate::util::{self, Fail};

/// Where the system keeps GRUB's images.
const IMAGES: &str = "/system/boot/grub";

/// Where UEFI's GRUB goes on an EFI system partition.
const EFI_PATH: &str = "EFI/BOOT/BOOTX64.EFI";

/// The partition a BIOS's GRUB finds its files on.
const FILES_PARTITION: u32 = 2;

const USAGE: &str =
    "usage: rgrub-setup [-d DIRECTORY] DISK\n       rgrub-setup --efi [-d DIRECTORY] PARTITION";

/// rgrub-setup: GRUB for a BIOS on `DISK`, or with `--efi` for UEFI on
/// the EFI system partition `PARTITION`.
pub fn main(args: &mut Args) -> Result<i32, Fail> {
    let mut efi = false;
    let mut images = PathBuf::from(IMAGES);
    let mut operands: Vec<OsString> = Vec::new();
    while let Some(arg) = args.next()? {
        match arg {
            Arg::Long(l) if l == "efi" => efi = true,
            Arg::Short('d') => images = PathBuf::from(args.value()?),
            Arg::Long(l) if l == "directory" => images = PathBuf::from(args.value()?),
            Arg::Short('h') => {
                println!("{USAGE}");
                return Ok(0);
            }
            Arg::Long(l) if l == "help" => {
                println!("{USAGE}");
                return Ok(0);
            }
            Arg::Operand(operand) => operands.push(operand),
            other => return Err(args.unknown(&other)),
        }
    }
    let [target] = operands.as_slice() else {
        let what = if efi { "the EFI system partition" } else { "the disk" };
        return Err(util::usage(format!("say {what} to put GRUB on")));
    };
    if efi { setup_efi(&images, target) } else { setup_bios(&images, target) }
}

/// Reads GRUB's image `name` from `images`.
fn image(images: &Path, name: &str) -> Result<Vec<u8>, Fail> {
    let path = images.join(name);
    std::fs::read(&path)
        .map_err(|e| util::fail(format!("{}: {}", path.display(), util::strip_os_error(&e))))
}

/// GRUB for a BIOS on the disk `operand`.
fn setup_bios(images: &Path, operand: &OsString) -> Result<i32, Fail> {
    let boot = image(images, "boot.img")?;
    let core = image(images, "core.img")?;
    let opened = blockdev::open(operand, true, None)?;
    if !opened.writable {
        return Err(util::fail(format!("{} refuses writes", opened.shown)));
    }
    if let Some(name) = &opened.name
        && probe::block_text(name, "partition").is_some()
    {
        return Err(util::fail(format!(
            "{} is a partition; GRUB for a BIOS goes on the whole disk it is on",
            opened.shown
        )));
    }
    let failed = |why: String| util::fail(format!("{}: {why}", opened.shown));
    let device = opened.device().map_err(|e| failed(util::strip_os_error(&e)))?;
    let found = disk::read(&device).map_err(|e| failed(e.to_string()))?;
    let table = found.table.ok_or_else(|| failed("it has no partition table".into()))?;
    if table.scheme != Scheme::Gpt {
        return Err(failed(
            "it has an MBR; Dukovo's GRUB for a BIOS boots disks with a GPT, which rdisk makes"
                .into(),
        ));
    }
    let esp = types::parse("efi", Scheme::Gpt).expect("a known type");
    match table.get(FILES_PARTITION) {
        Some(part) if part.kind == esp => {}
        _ => {
            return Err(failed(format!(
                "its partition {FILES_PARTITION} is no EFI system partition, where Dukovo's \
                 GRUB for a BIOS finds its files (/boot/grub)"
            )));
        }
    }
    let setup = disk::grub::setup_bios(&device, &boot, &core).map_err(failed)?;
    println!(
        "Installed GRUB for a BIOS on {}: boot.img in its first sector, core.img ({} sectors) \
         in its BIOS boot partition, {}, from sector {}",
        opened.shown, setup.core_sectors, setup.partition, setup.core_sector
    );
    Ok(0)
}

/// GRUB for UEFI on the EFI system partition `operand`.
fn setup_efi(images: &Path, operand: &OsString) -> Result<i32, Fail> {
    let app = image(images, "BOOTX64.EFI")?;
    let opened = blockdev::open(operand, true, None)?;
    if !opened.writable {
        return Err(util::fail(format!("{} refuses writes", opened.shown)));
    }
    opened.refuse_in_use("put GRUB on it")?;
    let failed = |why: String| util::fail(format!("{}: {why}", opened.shown));
    let device = opened.device().map_err(|e| failed(util::strip_os_error(&e)))?;
    disk::esp::put_file(device, EFI_PATH, &app).map_err(failed)?;
    println!(
        "Installed GRUB for UEFI on {}: {EFI_PATH} ({} KiB)",
        opened.shown,
        app.len().div_ceil(1024)
    );
    Ok(0)
}
