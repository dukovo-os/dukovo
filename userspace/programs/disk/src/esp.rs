// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Files put in the FAT of an EFI system partition by the kernel's own
//! FAT crate, as Radix writes them (ADR 0306), for rgrub-setup(8), which
//! puts UEFI's GRUB on a partition the system leaves unmounted.

use radix_fat::FatFs;
use radix_vfs::{Access, Error, FileSystem as _, Kind, NodeRef, Slice, SliceMut};

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

/// Whether the FAT on `device` holds the file `path` (names split by
/// `/`, in any case, as FAT finds them), as the installer looks for
/// Windows Boot Manager on an EFI system partition. Nothing is written.
///
/// # Errors
///
/// If `device` holds no FAT, or one too damaged to read.
pub fn has_file(device: Device, path: &str) -> Result<bool, String> {
    let fs = FatFs::mount(BlockFile::new(device, true).into_ref(), true).map_err(describe)?;
    let mut node = fs.root();
    for name in path.split('/').filter(|name| !name.is_empty()) {
        node = match node.lookup(name) {
            Ok(found) => found,
            Err(Error::NotFound | Error::NotDirectory) => return Ok(false),
            Err(err) => return Err(describe(err)),
        };
    }
    Ok(node.kind() == Kind::File)
}

/// The node at `path` (names split by `/`) beneath `node`, if there is one.
fn find(mut node: NodeRef, path: &str) -> Result<Option<NodeRef>, String> {
    for name in path.split('/').filter(|name| !name.is_empty()) {
        node = match node.lookup(name) {
            Ok(found) => found,
            Err(Error::NotFound | Error::NotDirectory) => return Ok(None),
            Err(err) => return Err(describe(err)),
        };
    }
    Ok(Some(node))
}

/// The first `most` bytes of the file `path` of the FAT on `device`, or
/// `None` if it has none. Nothing is written.
///
/// # Errors
///
/// If `device` holds no FAT, or one too damaged to read.
pub fn read_file(device: Device, path: &str, most: usize) -> Result<Option<Vec<u8>>, String> {
    let fs = FatFs::mount(BlockFile::new(device, true).into_ref(), true).map_err(describe)?;
    let Some(node) = find(fs.root(), path)?.filter(|node| node.kind() == Kind::File) else {
        return Ok(None);
    };
    let size = usize::try_from(node.stat().size).unwrap_or(usize::MAX).min(most);
    let mut bytes = vec![0u8; size];
    let read = node.read(0, &mut SliceMut(&mut bytes)).map_err(describe)?;
    bytes.truncate(read);
    Ok(Some(bytes))
}

/// Writes `data` over the file `path` of the FAT on `device` where it
/// is, when it is there and as long: its clusters stay where they are,
/// so that only their sectors are written, as GRUB writes its
/// environment blocks, and a power cut leaves the file whole or with
/// some of its sectors new. Otherwise it is made as [`put_file`] makes
/// one.
///
/// # Errors
///
/// If `device` holds no FAT, or one too full or damaged to write.
pub fn overwrite_file(device: Device, path: &str, data: &[u8]) -> Result<(), String> {
    radix_vfs::set_clock(now);
    let again = device.part(0, device.sectors()).map_err(|err| err.to_string())?;
    let fs = FatFs::mount(BlockFile::new(again, false).into_ref(), false).map_err(describe)?;
    let found = find(fs.root(), path)?
        .filter(|node| node.kind() == Kind::File && node.stat().size == data.len() as u64);
    let Some(node) = found else {
        drop(fs);
        return put_file(device, path, data);
    };
    let written = node.write(0, &Slice(data)).map_err(describe)?;
    if written != data.len() {
        return Err(format!("{written} of {} bytes were written", data.len()));
    }
    fs.sync().map_err(describe)
}
