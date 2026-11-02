// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Files put in the FAT of an EFI system partition by the kernel's own
//! FAT crate, as Radix writes them (ADR 0306), for rgrub-setup(8), which
//! puts UEFI's GRUB on a partition the system leaves unmounted.

use radix_fat::FatFs;
use radix_vfs::{Access, Error, FileSystem as _, Kind, NodeRef, Slice};

use crate::blockfile::BlockFile;
use crate::device::Device;

/// The time now, in nanoseconds since 1970, for the files' times.
fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_nanos()).unwrap_or(u64::MAX))
}

/// What an error of the FAT crate says.
fn describe(err: Error) -> String {
    match err {
        Error::UnknownFilesystem => String::from("it holds no FAT"),
        Error::NoSpace => String::from("its filesystem is full"),
        Error::Io => String::from("it could not be read or written"),
        Error::Corrupt => String::from("its filesystem is damaged; fsck-fat mends it"),
        Error::NotDirectory => String::from("a name on the way is a file's, not a directory's"),
        Error::IsDirectory => String::from("a directory has the file's name"),
        other => format!("{other:?}"),
    }
}

/// The entry `name` of `dir`, made as a directory or a file of `kind`
/// when it has none.
fn entry(dir: &NodeRef, name: &str, kind: Kind) -> Result<NodeRef, Error> {
    match dir.lookup(name) {
        Err(Error::NotFound) => dir.create(name, kind, Access::default_for(kind)),
        found => found,
    }
}

/// Writes `data` as the file `path` (names split by `/`) of the FAT on
/// `device`, making the directories on its way, in place of the file
/// there.
///
/// # Errors
///
/// If `device` holds no FAT, or one too full or damaged to write.
pub fn put_file(device: Device, path: &str, data: &[u8]) -> Result<(), String> {
    // The crate's clock, unless its caller gave it one.
    radix_vfs::set_clock(now);
    let fs = FatFs::mount(BlockFile::new(device, false).into_ref(), false).map_err(describe)?;
    let names: Vec<&str> = path.split('/').filter(|name| !name.is_empty()).collect();
    let (file, dirs) = names.split_last().ok_or("no file is named")?;
    let mut dir = fs.root();
    for name in dirs {
        dir = entry(&dir, name, Kind::Directory).map_err(describe)?;
    }
    let node = entry(&dir, file, Kind::File).map_err(describe)?;
    node.set_size(0).map_err(describe)?;
    let written = node.write(0, &Slice(data)).map_err(describe)?;
    if written != data.len() {
        return Err(format!("{written} of {} bytes were written", data.len()));
    }
    fs.sync().map_err(describe)
}
