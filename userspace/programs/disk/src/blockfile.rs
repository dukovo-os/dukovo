// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! A [`Device`] as a block device of the kernel's crates (feature
//! `kernel`), so that the kernel's filesystems make, fill and check
//! filesystems on a disk, a partition or a file: Btrfs (ADRs 0304 and
//! 0305), and the FAT `./ob image` puts GRUB and the system on.

use std::io;

use radix_block::{BlockDevice, DeviceRef, Error as BlockError, Info, SECTOR, Segment, SegmentMut};

use crate::device::Device;

/// A [`Device`] as a block device of the kernel's crates.
pub struct BlockFile {
    device: Device,
    info: Info,
}

impl BlockFile {
    /// `device`, read and written in its logical sectors, refusing writes
    /// if `read_only`.
    pub fn new(device: Device, read_only: bool) -> BlockFile {
        let info = Info {
            sectors: device.len() / SECTOR,
            logical_block: device.sector_size(),
            block_size: device.sector_size().max(4096),
            read_only,
            max_segments: 256,
            discard_sectors: 0,
            rotational: false,
        };
        BlockFile { device, info }
    }

    /// As a block device to open a filesystem on.
    ///
    /// # Panics
    ///
    /// Never: the host's allocator aborts rather than fails.
    pub fn into_ref(self) -> DeviceRef {
        radix_vfs::mem::arc(self).expect("memory for a block device")
    }
}

impl BlockFile {
    /// Checks a request of `segments` from `sector`, as every block device
    /// does.
    fn fits(
        &self,
        sector: u64,
        segments: impl Iterator<Item = (usize, usize)>,
    ) -> Result<(), BlockError> {
        let i = &self.info;
        radix_block::check(i.sectors, i.unit(), i.max_segments, sector, segments).map(drop)
    }
}

/// What a failed read or write of a device is to the kernel's crates.
fn io_error(_: io::Error) -> BlockError {
    BlockError::Io
}

impl BlockDevice for BlockFile {
    fn info(&self) -> Info {
        self.info
    }

    fn read(&self, sector: u64, segments: &mut [SegmentMut<'_>]) -> Result<(), BlockError> {
        self.fits(sector, segments.iter().map(|s| (s.offset, s.len)))?;
        let mut at = sector * SECTOR;
        for s in segments {
            let out = &mut s.page.bytes_mut()[s.offset..s.offset + s.len];
            self.device.read_at(at, out).map_err(io_error)?;
            at += s.len as u64;
        }
        Ok(())
    }

    fn write(&self, sector: u64, segments: &[Segment<'_>]) -> Result<(), BlockError> {
        if self.info.read_only {
            return Err(BlockError::ReadOnly);
        }
        self.fits(sector, segments.iter().map(|s| (s.offset, s.len)))?;
        let mut at = sector * SECTOR;
        for s in segments {
            self.device
                .write_at(at, &s.page.bytes()[s.offset..s.offset + s.len])
                .map_err(io_error)?;
            at += s.len as u64;
        }
        Ok(())
    }

    fn flush(&self) -> Result<(), BlockError> {
        if self.info.read_only { Ok(()) } else { self.device.sync().map_err(io_error) }
    }
}
