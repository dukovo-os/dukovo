// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Btrfs, made and checked by the kernel's own Btrfs crate (feature
//! `btrfs`; ADRs 0304 and 0305): for mkfs-btrfs(8) and fsck-btrfs(8) on
//! Dukovo, and for `./ob image`, which then writes the Dukovo root with
//! the same crate's writer.
//!
//! [`BlockFile`] gives the crate a [`Device`] as the block device it
//! reads and writes: a disk, a partition or a file.

use std::io;

pub use crate::blockfile::BlockFile;

pub use radix_btrfs::check::{PHASES, Report};
pub use radix_btrfs::chunk::kind;
pub use radix_btrfs::csum::CsumType;
pub use radix_btrfs::make::{Layout, MIN_BYTES, NODESIZE, Options, SECTORSIZE};

use crate::device::Device;
use crate::signatures;

/// What a filesystem's errors say, in words.
pub fn describe(err: radix_vfs::Error) -> String {
    match err {
        radix_vfs::Error::NoSpace => {
            format!("the device is too small: a Btrfs needs {} MiB at least", MIN_BYTES >> 20)
        }
        radix_vfs::Error::Io => String::from("the device could not be read or written"),
        radix_vfs::Error::Corrupt => String::from("it is damaged"),
        radix_vfs::Error::UnknownFilesystem => String::from("it holds no Btrfs"),
        radix_vfs::Error::NotSupported => String::from("it uses what Dukovo does not support"),
        other => format!("{other:?}"),
    }
}

/// Makes the Btrfs `options` says on `device`, the whole of it, and wipes
/// the marks of what it held before, a Btrfs's copies of its superblock
/// past the new one's end too; returns the chunks made.
///
/// # Errors
///
/// Why it could not be made, in words.
pub fn make(device: &Device, options: &Options<'_>) -> Result<Layout, String> {
    let file = BlockFile::new(device.part(0, device.sectors()).map_err(|e| e.to_string())?, false);
    let layout = radix_btrfs::make::make(&file.into_ref(), options).map_err(describe)?;
    let written = |e: io::Error| format!("cannot write the device: {e}");
    signatures::wipe(device, "btrfs").map_err(written)?;
    for old in signatures::find(device) {
        if old.name == "btrfs" && old.offset >= layout.total_bytes {
            device.write_at(old.offset, &vec![0u8; old.len]).map_err(written)?;
        }
    }
    device.sync().map_err(written)?;
    Ok(layout)
}

/// A Btrfs opened to be checked, which nothing changes.
pub struct Checking {
    volume: radix_vfs::mem::KBox<radix_btrfs::Volume>,
}

impl Checking {
    /// The Btrfs on `device`, whose trees the check starts from.
    ///
    /// # Errors
    ///
    /// Why it cannot be checked, in words: it is no Btrfs, or those trees
    /// cannot be read.
    pub fn open(device: &Device) -> Result<Checking, String> {
        let file =
            BlockFile::new(device.part(0, device.sectors()).map_err(|e| e.to_string())?, true);
        let volume = radix_btrfs::Volume::open(file.into_ref()).map_err(describe)?;
        Ok(Checking { volume })
    }

    /// Its UUID.
    pub fn fsid(&self) -> [u8; 16] {
        self.volume.superblock().fsid
    }

    /// Its label, if it has one.
    pub fn label(&self) -> Option<String> {
        self.volume.superblock().label().map(String::from)
    }

    /// Checks it, and its data too if `data`, calling `phase` as each
    /// phase starts.
    ///
    /// # Errors
    ///
    /// Why the check could not go on, in words.
    pub fn check(&self, data: bool, phase: &mut dyn FnMut(usize, &str)) -> Result<Report, String> {
        radix_btrfs::check::check(&self.volume, data, phase).map_err(describe)
    }
}

/// A new random UUID, from the system's random numbers.
pub fn new_uuid(rng: &mut crate::Rng) -> [u8; 16] {
    let mut uuid = [0u8; 16];
    rng.fill(&mut uuid);
    uuid[6] = uuid[6] & 0x0f | 0x40;
    uuid[8] = uuid[8] & 0x3f | 0x80;
    uuid
}
