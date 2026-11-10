// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! floppy(1): the floppy drives, PC and USB (ADRs 0435 and 0436). A
//! floppy drive cannot say that a disk went in, so the kernel looks only
//! when asked: `floppy look` asks (radix-block(2)'s REREAD_PARTITIONS on
//! the drive), and volumed(8) mounts what it finds. `floppy format`
//! formats a disk as Linux's fdformat and mkfs.fat do together: a track
//! at a time (FORMAT_TRACK), each track's request made on the drive
//! opened again, as the first takes the disk out of the kernel; then it
//! looks, reads the whole disk back, makes a FAT filesystem on it, and
//! mounts it.
//!
//! When it breaks: the kernel's log says what the drive answered for a
//! track (`Disk: fd0: ...`), and `/kernel/block/<drive>/formats` lists
//! the sizes of disk a drive formats.

use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::FileExt;

use disk::{Rng, fat};
use radix_abi::Status;
use radix_abi::block;
use radix_abi::errors::{File as FileError, Kernel};

use crate::args::{Arg, Args};
use crate::blockdev;
use crate::df;
use crate::probe;
use crate::sys;
use crate::util::{self, Fail};
use crate::{links, volumed};

/// A 3.5-inch floppy disk's cylinders and heads, of either size.
const CYLINDERS: u32 = 80;
const HEADS: u32 = 2;

/// A size of disk, as `/kernel/block/<drive>/formats` names it, and its
/// sectors of 512 bytes, which FORMAT_TRACK takes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Size {
    name: &'static str,
    sectors: u32,
}

/// The sizes the kernel formats.
const SIZES: [Size; 2] =
    [Size { name: "1.44M", sectors: 2880 }, Size { name: "720K", sectors: 1440 }];

impl Size {
    /// The size `text` names: `1.44M`, `1440K` or `1440`; `720K` or
    /// `720`, in either case.
    fn named(text: &str) -> Option<Size> {
        let text = text.trim().to_ascii_uppercase();
        let text = text.strip_suffix('B').unwrap_or(&text);
        let kib = match text {
            "1.44M" | "1.44" | "1440K" | "1440" => 1440,
            "720K" | "720" => 720,
            _ => return None,
        };
        SIZES.into_iter().find(|size| size.sectors == kib * 2)
    }

    /// Its sectors on a track.
    fn track_sectors(self) -> u32 {
        self.sectors / (CYLINDERS * HEADS)
    }
}

/// The floppy drives, by name: those whose `/kernel/block` directory has
/// `formats`.
fn drives() -> Vec<String> {
    let mut names: Vec<String> = probe::devices()
        .unwrap_or_default()
        .into_iter()
        .filter(|(name, partition)| !partition && probe::block_text(name, "formats").is_some())
        .map(|(name, _)| name)
        .collect();
    names.sort();
    names
}

/// The sizes of disk the drive `name` formats, as the kernel lists them.
fn formats_of(name: &str) -> Vec<Size> {
    let listed = probe::block_text(name, "formats").unwrap_or_default();
    listed.split_whitespace().filter_map(Size::named).collect()
}

/// The drive `operand` names: `fd0` or `/devices/fd0`. Fails if it is not
/// a floppy drive.
fn drive(operand: &str) -> Result<String, Fail> {
    let name = operand.strip_prefix("/devices/").unwrap_or(operand);
    if probe::block_text(name, "formats").is_none() {
        let known = drives();
        let have = if known.is_empty() { "there is none".into() } else { known.join(", ") };
        return Err(util::fail(format!("{name} is not a floppy drive (floppy drives: {have})")));
    }
    Ok(name.to_string())
}

/// The sectors of the disk in the drive `name`: 0 if there is none.
fn sectors_in(name: &str) -> u64 {
    probe::block_text(name, "size").and_then(|sectors| sectors.parse().ok()).unwrap_or(0)
}

/// What is in the drive `name`, in words: `a 1.44M disk`, `no disk`.
fn contents(name: &str) -> String {
    let bytes = sectors_in(name) * 512;
    let size = SIZES.iter().find(|size| u64::from(size.sectors) * 512 == bytes);
    let protected = probe::block_text(name, "ro").as_deref() == Some("1");
    let protected = if protected { ", write-protected" } else { "" };
    match (bytes, size) {
        (0, _) => "no disk".to_string(),
        (_, Some(size)) => format!("a {} disk{protected}", size.name),
        (bytes, None) => format!("a disk of {bytes} bytes{protected}"),
    }
}

/// Where the drive `name`'s disk is mounted, if it is.
fn mounted_at(name: &str) -> Option<String> {
    let mounts = volumed::volume_mounts().ok()?;
    let mount = mounts.iter().find(|m| m.source == name)?;
    Some(df::mount_path(mount).display().to_string())
}

/// Has the kernel look for a disk in the drive `name` (REREAD_PARTITIONS),
/// which it finds and puts in `/devices` before this returns.
fn look(name: &str) -> Result<(), String> {
    let device = File::open(format!("/devices/{name}")).map_err(|e| util::strip_os_error(&e))?;
    blockdev::control(&device, block::REREAD_PARTITIONS, &mut [])
        .map_err(|status| util::strip_os_error(&sys::io_error(status)))
}

/// Formats track (`cylinder`, `head`) of the disk in the drive `name` as a
/// disk of `size`. The drive is opened again for each, as the first takes
/// the disk out of the kernel and the device opened before goes with it.
fn format_track(name: &str, size: Size, cylinder: u32, head: u32) -> Result<(), String> {
    let path = format!("/devices/{name}");
    let device = OpenOptions::new().read(true).write(true).open(&path);
    let device = device.map_err(|e| format!("cannot open {path}: {}", util::strip_os_error(&e)))?;
    let mut track = [0u8; block::FORMAT_SIZE as usize];
    for (at, value) in [size.sectors, cylinder, head].into_iter().enumerate() {
        track[at * 4..at * 4 + 4].copy_from_slice(&value.to_le_bytes());
    }
    blockdev::control(&device, block::FORMAT_TRACK, &mut track)
        .map_err(|status| why_not_formatted(status, size))
}

/// Why FORMAT_TRACK failed, in words.
fn why_not_formatted(status: Status, size: Size) -> String {
    match (FileError::from_status(status), Kernel::from_status(status)) {
        (Some(FileError::NoDevice), _) => "there is no disk in the drive".into(),
        (Some(FileError::ReadOnly), _) => "the disk is write-protected".into(),
        (Some(FileError::InUse), _) => "the drive is in use; eject it first".into(),
        (_, Some(Kernel::NotSupported)) => {
            format!("the drive does not format {} disks, or the disk is not one", size.name)
        }
        _ => util::strip_os_error(&sys::io_error(status)),
    }
}

/// Reads the whole of the disk in the drive `name`, a track at a time:
/// fails at the first track that does not read.
fn verify(name: &str, size: Size) -> Result<(), String> {
    let path = format!("/devices/{name}");
    let device = File::open(&path)
        .map_err(|e| format!("cannot open {path}: {}", util::strip_os_error(&e)))?;
    let track = (size.track_sectors() * 512) as usize;
    let mut buffer = vec![0u8; track];
    for at in 0..CYLINDERS * HEADS {
        if let Err(err) = device.read_exact_at(&mut buffer, u64::from(at) * track as u64) {
            let (cylinder, head) = (at / HEADS, at % HEADS);
            let why = util::strip_os_error(&err);
            return Err(format!("cylinder {cylinder}, head {head} does not read back: {why}"));
        }
    }
    Ok(())
}

/// Makes a FAT filesystem labelled `label` on the whole of the disk in
/// the drive `name`: FAT12, with the disk's own geometry, as mkfs.fat
/// makes one on a floppy disk.
fn make_filesystem(name: &str, label: Option<String>) -> Result<(), Fail> {
    let operand = std::ffi::OsString::from(format!("/devices/{name}"));
    let opened = crate::mkfs::target(&operand, None, (true, "-I"), "a new filesystem on")?;
    let device = opened.device().map_err(|e| util::failed(&opened.shown, e))?;
    let options =
        fat::Options { label, now: crate::mkfs::source_date(), ..fat::Options::default() };
    fat::format(&device, &options, &mut Rng::system()).map_err(util::fail)?;
    links::refresh_quietly();
    Ok(())
}

/// `floppy`: each floppy drive, the sizes it formats, what is in it and
/// where that is mounted.
fn list(out: &mut dyn Write) -> Result<i32, Fail> {
    let drives = drives();
    if drives.is_empty() {
        writeln!(out, "floppy: there are no floppy drives")?;
        return Ok(0);
    }
    for name in drives {
        let formats: Vec<&str> = formats_of(&name).iter().map(|size| size.name).collect();
        let at = mounted_at(&name).map_or_default(|at| format!(", mounted at {at}"));
        writeln!(out, "{name}: formats {}; {}{at}", formats.join(" and "), contents(&name))?;
    }
    Ok(0)
}

/// `floppy format [-s SIZE] [-n LABEL] [--no-verify] [--no-filesystem]
/// DRIVE`.
fn format(args: &mut Args, out: &mut dyn Write) -> Result<i32, Fail> {
    let (mut size, mut label, mut verify_it, mut filesystem) = (None, None, true, true);
    let mut operand = None;
    while let Some(arg) = args.next()? {
        match arg {
            Arg::Short('s') => size = Some(args.text()?),
            Arg::Long(l) if l == "size" => size = Some(args.text()?),
            Arg::Short('n') => label = Some(args.text()?),
            Arg::Long(l) if l == "label" => label = Some(args.text()?),
            Arg::Long(l) if l == "no-verify" => verify_it = false,
            Arg::Long(l) if l == "no-filesystem" => filesystem = false,
            Arg::Operand(o) if operand.is_none() => {
                operand = Some(o.to_string_lossy().into_owned());
            }
            other => return Err(args.unknown(&other)),
        }
    }
    let Some(operand) = operand else {
        return Err(util::usage("say which drive to format".into()));
    };
    let name = drive(&operand)?;
    let formats = formats_of(&name);
    let size = match size {
        Some(text) => {
            let size = Size::named(&text)
                .ok_or_else(|| util::usage(format!("a size is 1.44M or 720K, not '{text}'")))?;
            if !formats.contains(&size) {
                return Err(util::fail(format!("{name} does not format {} disks", size.name)));
            }
            size
        }
        None => *formats.first().ok_or_else(|| util::fail(format!("{name} formats no disks")))?,
    };
    if let Some(at) = mounted_at(&name) {
        return Err(util::fail(format!(
            "{name} is mounted at {at}; formatting would lose what is on it (eject it first)"
        )));
    }
    let opened = blockdev::open(std::ffi::OsStr::new(&format!("/devices/{name}")), true, None)?;
    if opened.is_device() && !opened.writable {
        return Err(util::fail(format!("the disk in {name} is write-protected")));
    }
    opened.refuse_in_use("formatting")?;
    drop(opened);

    let tty = util::is_terminal(1);
    writeln!(out, "floppy: formatting the disk in {name} as a {} disk", size.name)?;
    for cylinder in 0..CYLINDERS {
        if tty {
            write!(out, "\rcylinder {}/{CYLINDERS}", cylinder + 1)?;
            out.flush()?;
        }
        for head in 0..HEADS {
            format_track(&name, size, cylinder, head).map_err(|why| {
                if tty {
                    let _ = writeln!(out);
                }
                util::fail(format!("{name}: cylinder {cylinder}, head {head}: {why}"))
            })?;
        }
    }
    if tty {
        writeln!(out)?;
    }
    look(&name).map_err(|why| util::fail(format!("{name}: cannot look at the disk: {why}")))?;
    if sectors_in(&name) != u64::from(size.sectors) {
        let found = contents(&name);
        return Err(util::fail(format!(
            "{name}: the disk was formatted, but reads as {found}; it may be of another density"
        )));
    }
    if verify_it {
        writeln!(out, "floppy: reading the disk back")?;
        verify(&name, size).map_err(|why| util::fail(format!("{name}: {why}")))?;
    }
    if !filesystem {
        writeln!(out, "floppy: formatted the disk in {name}")?;
        return Ok(0);
    }
    // volumed found the disk at the look, as a disk put in, and mounted
    // a filesystem the format left on it, if one was: the drive of a
    // virtual machine writes nothing when it formats.
    if let Some(at) = mounted_at(&name) {
        volumed::unmount(std::path::Path::new(&at))
            .map_err(|why| util::fail(format!("{at}: cannot unmount: {why}")))?;
    }
    writeln!(out, "floppy: making a FAT filesystem on it")?;
    make_filesystem(&name, label)?;
    // volumed may have mounted it already, as a disk put in.
    let said = match mounted_at(&name) {
        Some(at) => format!("mounted the FAT filesystem of {name} at {at}"),
        None => volumed::mount(&name),
    };
    writeln!(out, "floppy: {said}")?;
    Ok(0)
}

pub fn main(args: &mut Args) -> Result<i32, Fail> {
    let mut out = io::stdout().lock();
    let Some(command) = args.subcommand() else {
        // `floppy --help`, or nothing: no other option is taken.
        if let Some(arg) = args.next()? {
            if !matches!(&arg, Arg::Long(l) if l == "help") {
                return Err(args.unknown(&arg));
            }
            writeln!(
                out,
                "usage: floppy [look DRIVE | format [-s SIZE] [-n LABEL] [--no-verify] [--no-filesystem] DRIVE]"
            )?;
            return Ok(0);
        }
        return list(&mut out);
    };
    match command.to_string_lossy().as_ref() {
        "look" => {
            let mut operand = None;
            while let Some(arg) = args.next()? {
                match arg {
                    Arg::Operand(o) if operand.is_none() => {
                        operand = Some(o.to_string_lossy().into_owned());
                    }
                    other => return Err(args.unknown(&other)),
                }
            }
            let Some(operand) = operand else {
                return Err(util::usage("say which drive to look at".into()));
            };
            let name = drive(&operand)?;
            look(&name).map_err(|why| util::fail(format!("{name}: cannot look: {why}")))?;
            writeln!(out, "{name}: {}", contents(&name))?;
            Ok(0)
        }
        "format" => format(args, &mut out),
        other => Err(util::usage(format!("'{other}' is not a floppy command: look or format"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_are_named_as_the_kernel_and_people_name_them() {
        for text in ["1.44M", "1.44m", "1.44MB", "1440K", "1440"] {
            assert_eq!(Size::named(text), Some(SIZES[0]), "{text}");
        }
        for text in ["720K", "720k", "720KB", "720"] {
            assert_eq!(Size::named(text), Some(SIZES[1]), "{text}");
        }
        for text in ["2.88M", "1.2M", "", "360K"] {
            assert_eq!(Size::named(text), None, "{text}");
        }
        assert_eq!(SIZES[0].track_sectors(), 18);
        assert_eq!(SIZES[1].track_sectors(), 9);
    }
}
