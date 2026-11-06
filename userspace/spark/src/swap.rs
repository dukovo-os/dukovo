// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Swap on an installed system (ADR 0311): Spark turns on each swap
//! partition of the disk that holds Dukovo's root, as the installer makes
//! one, the way systemd's gpt-auto-generator does by the Discoverable
//! Partitions Specification. Dukovo has no fstab, and a partition's type
//! says what it is for: the swap a disk's own system uses goes with that
//! disk, wherever the disk is found.
//!
//! A partition of that type whose area mkswap(8) did not make is left
//! alone, and so is one that Linux hibernated into, as its header then
//! holds Linux's hibernation signature instead of `SWAPSPACE2`:
//! turning it on would write over the memory Linux means to wake with.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use libradix::abi::block;
use libradix::abi::capability::Rights;
use libradix::abi::directory::{FileKind, OpenFlags};
use libradix::abi::file;
use radix_rt::fs::Dir;
use radix_rt::{SYS, println};

/// The type of a swap partition, as `/kernel/block` gives it: Linux swap
/// in a GPT, and in an MBR.
pub const SWAP: [&str; 2] = ["0657fd6d-a4ab-43c4-84e5-0933c84b4f4f", "0x82"];

/// A swap area's header fills its first page, which ends with this.
const PAGE: usize = 4096;
const MAGIC: &[u8; 10] = b"SWAPSPACE2";

/// The disk the partition `name` is on: `vda` for `vda3`, and `nvme0n1`
/// for `nvme0n1p3`, whose disk's own name ends in a digit.
fn disk_of(name: &str) -> &str {
    let disk = name.trim_end_matches(|c: char| c.is_ascii_digit());
    match disk.strip_suffix('p') {
        Some(before) if before.ends_with(|c: char| c.is_ascii_digit()) => before,
        _ => disk,
    }
}

/// Whether `name` is a partition of `disk`: its name and a number, after
/// a `p` when the disk's name ends in a digit.
fn is_partition_of(name: &str, disk: &str) -> bool {
    let digit = disk.ends_with(|c: char| c.is_ascii_digit());
    name.strip_prefix(disk)
        .and_then(|number| if digit { number.strip_prefix('p') } else { Some(number) })
        .is_some_and(|number| !number.is_empty() && number.bytes().all(|b| b.is_ascii_digit()))
}

/// The type of the partition `name`, in lowercase; empty if it has none.
pub fn part_type(root: &Dir, name: &str) -> String {
    root.read_file(&format!("kernel/block/{name}/parttype")).map_or_else(
        |_| String::new(),
        |kind| String::from_utf8_lossy(&kind).trim_end().to_ascii_lowercase(),
    )
}

/// Turns on the swap partitions of the disk that holds the partition
/// `root_partition`, Dukovo's root. Says what it turned on, and why it
/// left one alone; a failure stops nothing, as the system runs without
/// swap.
pub fn turn_on(root: &Dir, root_partition: &str) {
    let disk = disk_of(root_partition);
    let names: Vec<String> =
        match root.open_dir("devices", Rights::READ).and_then(|dev| dev.entries()) {
            Ok(entries) => entries
                .into_iter()
                .filter(|entry| entry.kind == FileKind::BlockDevice)
                .map(|entry| entry.name)
                .filter(|name| is_partition_of(name, disk))
                .collect(),
            Err(err) => {
                println!("spark: cannot list /devices for swap: {err}");
                return;
            }
        };
    for name in names.iter().filter(|name| SWAP.contains(&part_type(root, name).as_str())) {
        match swap_on(root, name) {
            Ok(()) => println!("spark: turned on swap on {name}, a swap partition of {disk}"),
            Err(why) => println!("spark: left the swap partition {name} off: {why}"),
        }
    }
}

/// Turns on the swap area on `/devices/<name>`, as swapon(8) does with no
/// options.
fn swap_on(root: &Dir, name: &str) -> Result<(), String> {
    let rights = Rights::READ.union(Rights::WRITE);
    let device = root
        .open(&format!("devices/{name}"), OpenFlags::NONE, rights)
        .map_err(|err| format!("cannot open it: {err}"))?;
    let mut header = alloc::vec![0u8; PAGE];
    let read = device.read_at(0, &mut header).map_err(|err| format!("cannot read it: {err}"))?;
    if read != PAGE || &header[PAGE - MAGIC.len()..] != MAGIC {
        return Err("it holds no swap area mkswap made (or Linux hibernated into it)".into());
    }
    // No priority asked: the kernel gives the next of Linux's defaults.
    let mut flags = 0u32.to_le_bytes();
    let address = flags.as_mut_ptr() as u64;
    file::control(&SYS, device.slot(), block::SWAP_ON, address, address, flags.len() as u64)
        .map(drop)
        .map_err(|status| format!("the kernel refused it ({:#x})", status.0))
}
