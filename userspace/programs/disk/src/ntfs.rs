// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! NTFS volumes checked and shrunk by the kernel's own NTFS crate
//! (feature `ntfs`; ADR 0312): for resize-ntfs(8), and for the installer,
//! which shrinks Windows' volume to make room for Dukovo beside it.
//!
//! [`BlockFile`] gives the crate a [`Device`] as the block device it
//! reads and writes: a disk, a partition or a file. A shrink changes the
//! volume alone. The partition it is in is shrunk after, to
//! [`Shrunk::bytes`], by the partition table: until then the volume
//! simply leaves the rest of it unused.

pub use radix_ntfs::check::Report;
pub use radix_ntfs::shrink::{Info, Options, Refused, Shrunk, Step};

use radix_vfs::Error;

use crate::blockfile::BlockFile;
use crate::device::Device;

/// What is said of a volume BitLocker encrypts: what is on the disk is
/// no NTFS a shrink can read until BitLocker is turned off.
pub const ENCRYPTED: &str =
    "BitLocker encrypts it: turn BitLocker off in Windows, and let it finish decrypting, first";

/// Whether BitLocker encrypts the volume on `device`: its boot sector
/// names BitLocker where an NTFS one says "NTFS".
pub fn encrypted(device: &Device) -> bool {
    let mut name = [0u8; 8];
    device.read_at(3, &mut name).is_ok() && name == *b"-FVE-FS-"
}

/// What the volume on `device` is, for a shrink: its size, the smallest
/// it shrinks to, why it would not be shrunk, and what the check finds.
/// Nothing is written.
///
/// # Errors
///
/// Why it could not be read, in words: BitLocker encrypts it, it holds
/// no NTFS volume, or it is damaged.
pub fn info(device: &Device) -> Result<Info, String> {
    if encrypted(device) {
        return Err(ENCRYPTED.into());
    }
    radix_ntfs::shrink::info(block_file(device, true)?.into_ref()).map_err(|r| r.to_string())
}

/// Checks the volume on `device`, as `ntfsresize --check` does, writing
/// nothing.
///
/// # Errors
///
/// Why it could not be checked, in words.
pub fn check(device: &Device) -> Result<Report, String> {
    if encrypted(device) {
        return Err(ENCRYPTED.into());
    }
    radix_ntfs::check::check(block_file(device, true)?.into_ref())
        .map_err(|err| Refused::Failed(err).to_string())
}

/// Shrinks the volume on `device` as `options` say, telling `progress`
/// how far it is; a dry run writes nothing. The device is flushed after.
///
/// # Errors
///
/// Why it was not shrunk: a refusal changes nothing, and only
/// [`Refused::Failed`] can come once something is written.
pub fn shrink(
    device: &Device,
    options: Options,
    progress: &mut dyn FnMut(Step),
) -> Result<Shrunk, Refused> {
    if encrypted(device) {
        return Err(Refused::Kept(ENCRYPTED));
    }
    let file = block_file(device, options.dry_run).map_err(|_| Refused::Failed(Error::Io))?;
    let shrunk = radix_ntfs::shrink::shrink(file.into_ref(), options, progress)?;
    device.sync().map_err(|_| Refused::Failed(Error::Io))?;
    Ok(shrunk)
}

/// `device`, whole, as the crate's block device.
fn block_file(device: &Device, read_only: bool) -> Result<BlockFile, String> {
    let whole = device.part(0, device.sectors()).map_err(|e| e.to_string())?;
    Ok(BlockFile::new(whole, read_only))
}
