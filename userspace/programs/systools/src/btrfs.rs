// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! mkfs-btrfs(8) and fsck-btrfs(8) (ADRs 0304 and 0305): Btrfs made and
//! checked by the kernel's own Btrfs crate, with the options of
//! btrfs-progs' mkfs.btrfs and `btrfs check`, by whose names, mkfs.btrfs
//! and btrfsck, they run too. Neither mends a Btrfs: the kernel's scrub
//! mends what a second copy can (ADR 0150).

use std::ffi::OsString;

use disk::btrfs::{self, Checking, CsumType, Layout, MIN_BYTES, NODESIZE, Options, PHASES, kind};
use disk::{Rng, signatures};

use crate::args::{Arg, Args};
use crate::mkfs::{self, LEFT, OPERATIONAL, USAGE};
use crate::util::{self, Fail};
use crate::{blockdev, links, probe, swap};

/// What mkfs.btrfs makes that mkfs-btrfs does not, by its options' names.
const NOT_MADE: [&str; 13] = [
    "M",
    "mixed",
    "O",
    "features",
    "R",
    "runtime-features",
    "r",
    "rootdir",
    "subvol",
    "shrink",
    "inode-flags",
    "device-uuid",
    "compress",
];

/// What `btrfs check` does that fsck-btrfs does not, by its options'
/// names: mending, and checking from other trees than the newest.
const NOT_CHECKED: [&str; 14] = [
    "repair",
    "init-csum-tree",
    "init-extent-tree",
    "clear-space-cache",
    "clear-ino-cache",
    "s",
    "super",
    "b",
    "backup",
    "r",
    "tree-root",
    "chunk-root",
    "Q",
    "qgroup-report",
];

/// `bytes` as btrfs-progs prints sizes: two decimals and a binary unit.
fn pretty(bytes: u64) -> String {
    let units = ["B", "KiB", "MiB", "GiB", "TiB", "PiB", "EiB"];
    let mut unit = 0;
    while unit + 1 < units.len() && bytes >= 1024u64 << (10 * unit) {
        unit += 1;
    }
    let scaled = bytes as f64 / (1u64 << (10 * unit)) as f64;
    format!("{scaled:.2}{}", units[unit])
}

/// A size option's value: bytes, or with K, M, G or T.
fn size_of(args: &mut Args, what: &str) -> Result<u64, Fail> {
    let text = args.text()?;
    util::parse_size(&text).ok_or_else(|| util::usage(format!("'{text}' is not a {what}")))
}

/// The checksum `--csum` names.
fn csum_named(text: &str) -> Result<CsumType, Fail> {
    match text.to_ascii_lowercase().as_str() {
        "crc32c" => Ok(CsumType::Crc32c),
        "xxhash" | "xxhash64" => Ok(CsumType::Xxhash64),
        "sha256" => Ok(CsumType::Sha256),
        "blake2" | "blake2b" => Ok(CsumType::Blake2b),
        _ => Err(util::usage(format!(
            "'{text}' is no checksum: crc32c, xxhash, sha256 or blake2"
        ))),
    }
}

/// Whether a profile `-m` names is DUP, or single.
fn dup_named(text: &str) -> Result<bool, Fail> {
    match text.to_ascii_lowercase().as_str() {
        "dup" => Ok(true),
        "single" => Ok(false),
        "raid0" | "raid1" | "raid1c3" | "raid1c4" | "raid10" | "raid5" | "raid6" => {
            Err(util::fail(format!(
                "{text} needs more devices; mkfs-btrfs makes a Btrfs on one, \
                 and `device add` and `balance` make it more"
            )))
        }
        _ => Err(util::usage(format!("'{text}' is no profile: dup or single"))),
    }
}

/// What is made, as mkfs.btrfs prints it.
fn summary(shown: &str, options: &Options<'_>, layout: &Layout) {
    let label = String::from_utf8_lossy(options.label);
    let csum = match options.csum {
        CsumType::Crc32c => "crc32c",
        CsumType::Xxhash64 => "xxhash64",
        CsumType::Sha256 => "sha256",
        CsumType::Blake2b => "blake2b",
    };
    println!("Label:              {}", if label.is_empty() { "(none)" } else { &label });
    println!("UUID:               {}", probe::uuid(&options.fsid));
    println!("Node size:          {}", options.nodesize);
    println!("Sector size:        {}", btrfs::SECTORSIZE);
    println!("Filesystem size:    {}", pretty(layout.total_bytes));
    println!("Block group profiles:");
    for (name, flag) in [("Data", kind::DATA), ("Metadata", kind::METADATA), ("System", kind::SYSTEM)] {
        for group in layout.groups.iter().filter(|g| g.flags & flag != 0) {
            let profile = if group.flags & kind::DUP != 0 { "DUP" } else { "single" };
            println!("  {:<18}{profile:<8}{:>12}", format!("{name}:"), pretty(group.length));
        }
    }
    println!("Checksum:           {csum}");
    println!("Device:             {shown}");
}

/// mkfs-btrfs(8), and mkfs.btrfs.
pub fn mkfs_btrfs(args: &mut Args) -> Result<i32, Fail> {
    let (mut label, mut nodesize, mut csum, mut dup) =
        (String::new(), u64::from(NODESIZE), CsumType::Crc32c, true);
    let (mut fsid, mut bytes, mut force, mut quiet) = (None, None, false, false);
    let mut operands: Vec<OsString> = Vec::new();
    while let Some(arg) = args.next()? {
        match arg {
            Arg::Short('L') => label = args.text()?,
            Arg::Long(l) if l == "label" => label = args.text()?,
            Arg::Short('n') => nodesize = size_of(args, "node size")?,
            Arg::Long(l) if l == "nodesize" => nodesize = size_of(args, "node size")?,
            Arg::Short('s') => sectorsize(args)?,
            Arg::Long(l) if l == "sectorsize" => sectorsize(args)?,
            Arg::Short('U') => fsid = Some(uuid_of(&args.text()?)?),
            Arg::Long(l) if l == "uuid" => fsid = Some(uuid_of(&args.text()?)?),
            Arg::Long(l) if l == "csum" || l == "checksum" => csum = csum_named(&args.text()?)?,
            Arg::Short('m') => dup = dup_named(&args.text()?)?,
            Arg::Long(l) if l == "metadata" => dup = dup_named(&args.text()?)?,
            Arg::Short('d') => data_profile(args)?,
            Arg::Long(l) if l == "data" => data_profile(args)?,
            Arg::Short('b') => bytes = Some(size_of(args, "size")?),
            Arg::Long(l) if l == "byte-count" => bytes = Some(size_of(args, "size")?),
            Arg::Short('f') => force = true,
            Arg::Long(l) if l == "force" => force = true,
            Arg::Short('q') => quiet = true,
            Arg::Long(l) if l == "quiet" => quiet = true,
            // Nothing is discarded: the trees made are all that is read.
            Arg::Short('K') => {}
            Arg::Long(l) if l == "nodiscard" => {}
            Arg::Short('V') => {
                println!("mkfs-btrfs, Dukovo systools {}", env!("CARGO_PKG_VERSION"));
                return Ok(0);
            }
            Arg::Long(l) if l == "version" => {
                println!("mkfs-btrfs, Dukovo systools {}", env!("CARGO_PKG_VERSION"));
                return Ok(0);
            }
            Arg::Short('h') => return Ok(mkfs_help()),
            Arg::Long(l) if l == "help" => return Ok(mkfs_help()),
            Arg::Operand(operand) => operands.push(operand),
            Arg::Short(c) if NOT_MADE.contains(&c.to_string().as_str()) => {
                return Err(util::fail(format!("-{c} is not supported by mkfs-btrfs")));
            }
            Arg::Long(l) if NOT_MADE.contains(&l.as_str()) => {
                return Err(util::fail(format!("--{l} is not supported by mkfs-btrfs")));
            }
            other => return Err(args.unknown(&other)),
        }
    }
    let path = match operands.as_slice() {
        [path] => path,
        [] => return Err(util::usage("say the device or file to make the Btrfs on".into())),
        _ => {
            return Err(util::fail(
                "mkfs-btrfs makes a Btrfs on one device; `device add` adds others".into(),
            ));
        }
    };
    if label.len() >= 256 || label.contains('\0') {
        return Err(util::usage("a label is at most 255 bytes, with no NUL".into()));
    }
    if !(4096..=65536).contains(&nodesize) || !nodesize.is_power_of_two() {
        return Err(util::usage(format!(
            "a node is 4 KiB, 8, 16, 32 or 64 KiB, not {nodesize} bytes"
        )));
    }
    let opened = mkfs::target(path, None, (force, "-f"), "a new filesystem on")?;
    let mut device = opened.device().map_err(|e| util::failed(&opened.shown, e))?;
    if let Some(bytes) = bytes {
        if bytes > device.len() {
            return Err(util::fail(format!("{} is smaller than {}", opened.shown, pretty(bytes))));
        }
        let sectors = bytes / u64::from(opened.sector);
        device = device.part(0, sectors).map_err(|e| util::failed(&opened.shown, e))?;
    }
    if device.len() < MIN_BYTES {
        return Err(util::fail(format!(
            "{} is too small: a Btrfs needs {} MiB at least",
            opened.shown,
            MIN_BYTES >> 20
        )));
    }
    if !force && let Some(found) = signatures::find(&device).first() {
        return Err(util::fail(format!(
            "{} holds a {} filesystem; -f makes a new one over it",
            opened.shown, found.name
        )));
    }
    let mut rng = Rng::system();
    let time = mkfs::source_date().unwrap_or_else(|| util::now_ns() / 1_000_000_000);
    let options = Options {
        label: label.as_bytes(),
        nodesize: nodesize as u32,
        csum,
        dup,
        fsid: fsid.unwrap_or_else(|| btrfs::new_uuid(&mut rng)),
        dev_uuid: btrfs::new_uuid(&mut rng),
        chunk_tree_uuid: btrfs::new_uuid(&mut rng),
        top_uuid: btrfs::new_uuid(&mut rng),
        time,
    };
    let layout = btrfs::make(&device, &options).map_err(util::fail)?;
    links::refresh_quietly();
    if !quiet {
        summary(&opened.shown, &options, &layout);
    }
    Ok(0)
}

/// `-s`'s sector size, which can only be 4096 bytes.
fn sectorsize(args: &mut Args) -> Result<(), Fail> {
    let sector = size_of(args, "sector size")?;
    if sector != u64::from(btrfs::SECTORSIZE) {
        return Err(util::fail(format!(
            "a sector of {sector} bytes is not supported; Dukovo's Btrfs has 4096"
        )));
    }
    Ok(())
}

/// `-d`'s profile of data, which can only be single.
fn data_profile(args: &mut Args) -> Result<(), Fail> {
    let profile = args.text()?;
    if !profile.eq_ignore_ascii_case("single") {
        dup_named(&profile)?;
        return Err(util::fail(format!("data is made single; {profile} data is not supported")));
    }
    Ok(())
}

/// A UUID as `-U` takes it.
fn uuid_of(text: &str) -> Result<[u8; 16], Fail> {
    swap::parse_uuid(text).ok_or_else(|| util::usage(format!("'{text}' is not a UUID")))
}

fn mkfs_help() -> i32 {
    println!(
        "usage: mkfs-btrfs [-L LABEL] [-n NODESIZE] [-m dup|single] [--csum TYPE] [-U UUID] \
         [-b SIZE] [-f] [-q] DEVICE"
    );
    0
}

/// fsck-btrfs(8), and btrfsck: checks a Btrfs without changing it, as
/// `btrfs check --readonly` does. Its exit status is fsck(8)'s: 0 if all
/// is well, 4 if something is wrong, 8 if it could not be checked, and 16
/// for a mistake in the command.
pub fn fsck_btrfs(args: &mut Args) -> Result<i32, Fail> {
    let as_fsck = |fail: Fail| Fail { code: if fail.usage { USAGE } else { fail.code }, ..fail };
    let (mut data, mut force) = (false, false);
    let mut operands: Vec<OsString> = Vec::new();
    while let Some(arg) = args.next().map_err(as_fsck)? {
        match arg {
            Arg::Long(l) if l == "check-data-csum" => data = true,
            // It never writes, and always says each phase as it starts.
            Arg::Long(l) if l == "readonly" || l == "progress" => {}
            Arg::Short('p') => {}
            Arg::Long(l) if l == "force" => force = true,
            Arg::Long(l) if l == "help" => {
                println!("usage: fsck-btrfs [--check-data-csum] [--force] DEVICE");
                return Ok(0);
            }
            Arg::Short('h') => {
                println!("usage: fsck-btrfs [--check-data-csum] [--force] DEVICE");
                return Ok(0);
            }
            Arg::Operand(operand) => operands.push(operand),
            Arg::Short(c) if NOT_CHECKED.contains(&c.to_string().as_str()) => {
                return Err(not_checked(&format!("-{c}")));
            }
            Arg::Long(l) if NOT_CHECKED.contains(&l.as_str()) => {
                return Err(not_checked(&format!("--{l}")));
            }
            other => return Err(as_fsck(args.unknown(&other))),
        }
    }
    let [path] = operands.as_slice() else {
        return Err(Fail { code: USAGE, message: "say the one device or file to check".into(), usage: true });
    };
    let opened = blockdev::open(path, false, None).map_err(|f| Fail { code: OPERATIONAL, ..f })?;
    if opened.uses() != 0 && !force {
        let name = opened.name.as_deref().unwrap_or(&opened.shown);
        return Err(util::fail_with(
            OPERATIONAL,
            format!(
                "{}; unmount it to check it, or --force checks it as it is being written",
                blockdev::in_use(name, opened.uses())
            ),
        ));
    }
    let failed = |why: String| util::fail_with(OPERATIONAL, format!("{}: {why}", opened.shown));
    let device = opened.device().map_err(|e| failed(e.to_string()))?;
    println!("Opening filesystem to check...");
    let checking = Checking::open(&device).map_err(failed)?;
    println!("Checking filesystem on {}", opened.shown);
    println!("UUID: {}", probe::uuid(&checking.fsid()));
    if let Some(label) = checking.label() {
        println!("Label: {label}");
    }
    let report = checking
        .check(data, &mut |n, name| println!("[{n}/{}] {name}", PHASES.len()))
        .map_err(failed)?;
    for problem in &report.problems {
        println!("ERROR: {problem}");
    }
    if report.log {
        println!("A log of fsyncs is waiting: mounting the filesystem replays it.");
    }
    let found = if report.problems.is_empty() {
        String::from("no error found")
    } else {
        format!("{} errors found", report.problems.len())
    };
    println!("found {} bytes used, {found}", report.bytes_used);
    println!("total tree blocks: {}", report.tree_blocks);
    println!("file data extents: {}", report.data_extents);
    println!(
        "{} subvolumes, {} directories, {} other files",
        report.subvolumes, report.directories, report.files
    );
    Ok(if report.problems.is_empty() { 0 } else { LEFT })
}

/// What fsck-btrfs says of an option that would mend, or check from
/// elsewhere.
fn not_checked(option: &str) -> Fail {
    Fail {
        code: USAGE,
        message: format!(
            "{option} is not supported: fsck-btrfs only checks, from the newest trees; \
             scrub mends what a second copy can"
        ),
        usage: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_checksums_and_profiles_read() {
        assert_eq!(pretty(8 << 20), "8.00MiB");
        assert_eq!(pretty(1 << 30), "1.00GiB");
        assert_eq!(pretty(51 * (1 << 20) + (1 << 19)), "51.50MiB");
        assert_eq!(pretty(500), "500.00B");
        assert_eq!(csum_named("xxhash").unwrap(), CsumType::Xxhash64);
        assert_eq!(csum_named("BLAKE2").unwrap(), CsumType::Blake2b);
        assert!(csum_named("md5").is_err());
        assert!(dup_named("DUP").unwrap());
        assert!(!dup_named("single").unwrap());
        assert!(dup_named("raid1").is_err());
    }
}
