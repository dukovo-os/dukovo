// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! UDF 2.01's structures (ECMA-167 3rd edition, OSTA UDF 2.01), as a
//! bridge beside ISO 9660's: the recognition sequence after ISO 9660's
//! descriptors, the anchors in sector 256 and the image's last, the main
//! and reserve volume descriptor sequences and the integrity sequence
//! before the first anchor, and one read-only partition after it that
//! holds the file set, a file entry for each entry, each directory's
//! identifiers, and the files' bytes, which ISO 9660's records point at
//! too.
//!
//! Every descriptor starts with a tag that says what it is and where, and
//! carries a CRC of the rest ([`tag`]). Names are OSTA compressed Unicode
//! ([`cs0`]): eight bits a character when every one fits, else sixteen.

use super::{Entry, EntryKind, Error, Image, SECTOR, date};

/// The UDF revision written: 2.01.
const REVISION: u16 = 0x0201;
/// The descriptors' version, ECMA-167 3rd edition's (NSR03).
const VERSION: u16 = 3;
/// A volume descriptor sequence's descriptors.
const SEQUENCE: u64 = 6;

/// The tags' identifiers.
mod id {
    pub const PRIMARY: u16 = 1;
    pub const ANCHOR: u16 = 2;
    pub const IMPLEMENTATION_USE: u16 = 4;
    pub const PARTITION: u16 = 5;
    pub const LOGICAL_VOLUME: u16 = 6;
    pub const UNALLOCATED: u16 = 7;
    pub const TERMINATING: u16 = 8;
    pub const INTEGRITY: u16 = 9;
    pub const FILE_SET: u16 = 256;
    pub const IDENTIFIER: u16 = 257;
    pub const FILE_ENTRY: u16 = 261;
}

/// An ICB's file types.
mod file_type {
    pub const DIRECTORY: u8 = 4;
    pub const FILE: u8 = 5;
    pub const SYMLINK: u8 = 12;
}

/// The longest extent a short allocation descriptor holds: 2^30 bytes
/// less a sector.
const MAX_EXTENT: u64 = (1 << 30) - SECTOR;

/// The UDF sectors before the files' bytes: (sector, bytes) for each.
pub(crate) fn head(image: &Image) -> Result<Vec<(u64, Vec<u8>)>, Error> {
    let l = &image.layout;
    let mut out = Vec::new();
    for (i, name) in [b"BEA01", b"NSR03", b"TEA01"].into_iter().enumerate() {
        let mut sector = vec![0u8; SECTOR as usize];
        sector[1..6].copy_from_slice(name);
        sector[6] = 1;
        out.push((l.vrs + i as u64, sector));
    }
    for start in [l.main_vds, l.reserve_vds] {
        for (i, descriptor) in volume_sequence(image, start).into_iter().enumerate() {
            out.push((start + i as u64, descriptor));
        }
    }
    out.push((l.integrity, integrity(image)));
    out.push((l.integrity + 1, terminating(l.integrity)));
    out.push((super::ANCHOR, anchor(image, super::ANCHOR)));
    let fsd = l.fsd - l.partition;
    out.push((l.fsd, file_set(image, fsd as u32)));
    out.push((l.fsd + 1, terminating(fsd + 1)));
    for i in 0..image.entries.len() {
        out.push((l.file_entries[i], file_entry(image, i)?));
        if matches!(image.entries[i].kind, EntryKind::Dir { .. }) {
            let (at, _) = l.udf_dirs[i];
            out.push((at, identifiers(image, i)?));
        }
    }
    Ok(out)
}

/// An anchor at `sector`: where the two volume descriptor sequences are.
pub(crate) fn anchor(image: &Image, sector: u64) -> Vec<u8> {
    let l = &image.layout;
    let mut out = vec![0u8; 512];
    let length = (SEQUENCE * SECTOR) as u32;
    put32(&mut out, 16, length);
    put32(&mut out, 20, l.main_vds as u32);
    put32(&mut out, 24, length);
    put32(&mut out, 28, l.reserve_vds as u32);
    tag(&mut out, id::ANCHOR, sector as u32);
    out
}

/// A volume descriptor sequence at `start`: the primary volume
/// descriptor, the implementation's, the partition's, the logical
/// volume's, the unallocated space's, and the terminator.
fn volume_sequence(image: &Image, start: u64) -> Vec<Vec<u8>> {
    let l = &image.layout;
    let options = &image.options;
    let label = &options.label;
    let mut primary = vec![0u8; 512];
    put32(&mut primary, 16, 0);
    dstring(&mut primary[24..56], label);
    put16(&mut primary, 56, 1);
    put16(&mut primary, 58, 1);
    put16(&mut primary, 60, 2);
    put16(&mut primary, 62, 2);
    put32(&mut primary, 64, 1);
    put32(&mut primary, 68, 1);
    // A volume set's identifier starts with 16 characters unique to it.
    dstring(&mut primary[72..200], &format!("{:016x}{label}", options.time));
    charspec(&mut primary[200..264]);
    charspec(&mut primary[264..328]);
    regid(&mut primary[344..376], "*Dukovo mkiso", &[]);
    timestamp(&mut primary[376..388], options.time);
    implementation(&mut primary[388..420]);
    tag(&mut primary, id::PRIMARY, start as u32);

    let mut usage = vec![0u8; 512];
    put32(&mut usage, 16, 1);
    regid(&mut usage[20..52], "*UDF LV Info", &suffix_with_os());
    charspec(&mut usage[52..116]);
    dstring(&mut usage[116..244], label);
    implementation(&mut usage[352..384]);
    tag(&mut usage, id::IMPLEMENTATION_USE, start as u32 + 1);

    let mut partition = vec![0u8; 512];
    put32(&mut partition, 16, 2);
    // Space allocated, partition 0, a file system of NSR03, read-only.
    put16(&mut partition, 20, 1);
    put16(&mut partition, 22, 0);
    regid(&mut partition[24..56], "+NSR03", &[]);
    put32(&mut partition, 184, 1);
    put32(&mut partition, 188, l.partition as u32);
    put32(&mut partition, 192, partition_length(image) as u32);
    implementation(&mut partition[196..228]);
    tag(&mut partition, id::PARTITION, start as u32 + 2);

    let mut volume = vec![0u8; 446];
    put32(&mut volume, 16, 3);
    charspec(&mut volume[20..84]);
    dstring(&mut volume[84..212], label);
    put32(&mut volume, 212, SECTOR as u32);
    regid(&mut volume[216..248], "*OSTA UDF Compliant", &REVISION.to_le_bytes());
    // The file set descriptor, where the files start.
    long_ad(&mut volume[248..264], SECTOR as u32, (l.fsd - l.partition) as u32, 0);
    put32(&mut volume, 264, 6);
    put32(&mut volume, 268, 1);
    implementation(&mut volume[272..304]);
    put32(&mut volume, 432, (2 * SECTOR) as u32);
    put32(&mut volume, 436, l.integrity as u32);
    // One partition map of type 1: volume 1's partition 0.
    volume[440..446].copy_from_slice(&[1, 6, 1, 0, 0, 0]);
    tag(&mut volume, id::LOGICAL_VOLUME, start as u32 + 3);

    let mut unallocated = vec![0u8; 24];
    put32(&mut unallocated, 16, 4);
    tag(&mut unallocated, id::UNALLOCATED, start as u32 + 4);

    vec![primary, usage, partition, volume, unallocated, terminating(start + 5)]
}

/// The partition's sectors: from after the first anchor to before the
/// last.
fn partition_length(image: &Image) -> u64 {
    image.layout.total - 1 - image.layout.partition
}

/// A terminating descriptor at `location`.
fn terminating(location: u64) -> Vec<u8> {
    let mut out = vec![0u8; 512];
    tag(&mut out, id::TERMINATING, location as u32);
    out
}

/// The logical volume's integrity descriptor: closed, with the next
/// unique ID and how many files and directories it holds.
fn integrity(image: &Image) -> Vec<u8> {
    let mut out = vec![0u8; 134];
    timestamp(&mut out[16..28], image.options.time);
    put32(&mut out, 28, 1);
    let next = image.entries.iter().map(|e| e.unique).max().unwrap_or(0).max(15) + 1;
    out[40..48].copy_from_slice(&next.to_le_bytes());
    put32(&mut out, 72, 1);
    put32(&mut out, 76, 46);
    put32(&mut out, 80, 0);
    put32(&mut out, 84, partition_length(image) as u32);
    implementation(&mut out[88..120]);
    let dirs = image.entries.iter().filter(|e| matches!(e.kind, EntryKind::Dir { .. })).count();
    put32(&mut out, 120, (image.entries.len() - dirs) as u32);
    put32(&mut out, 124, dirs as u32);
    for at in [128, 130, 132] {
        put16(&mut out, at, REVISION);
    }
    tag(&mut out, id::INTEGRITY, image.layout.integrity as u32);
    out
}

/// The file set descriptor, at the partition's block `location`.
fn file_set(image: &Image, location: u32) -> Vec<u8> {
    let l = &image.layout;
    let mut out = vec![0u8; 512];
    timestamp(&mut out[16..28], image.options.time);
    put16(&mut out, 28, 3);
    put16(&mut out, 30, 3);
    put32(&mut out, 32, 1);
    put32(&mut out, 36, 1);
    charspec(&mut out[48..112]);
    dstring(&mut out[112..240], &image.options.label);
    charspec(&mut out[240..304]);
    dstring(&mut out[304..336], &image.options.label);
    let root = (l.file_entries[0] - l.partition) as u32;
    long_ad(&mut out[400..416], SECTOR as u32, root, 0);
    regid(&mut out[416..448], "*OSTA UDF Compliant", &REVISION.to_le_bytes());
    tag(&mut out, id::FILE_SET, location);
    out
}

/// The entry whose file entry an identifier of `entry` points at: the
/// first entry with a file's bytes, for names that share them.
fn canonical(image: &Image, entry: usize) -> usize {
    if let EntryKind::File { data, .. } = image.entries[entry].kind {
        image
            .entries
            .iter()
            .position(|e| matches!(e.kind, EntryKind::File { data: d, .. } if d == data))
            .unwrap_or(entry)
    } else {
        entry
    }
}

/// Entry `i`'s file entry.
fn file_entry(image: &Image, i: usize) -> Result<Vec<u8>, Error> {
    let l = &image.layout;
    let e = &image.entries[i];
    let location = (l.file_entries[i] - l.partition) as u32;
    // Its extents (bytes, partition block), or its bytes in the entry.
    let (kind, length, extents, embedded) = match &e.kind {
        EntryKind::File { size, data } => {
            let at = image.data[*data].at - l.partition;
            (file_type::FILE, *size, split(*size, at), Vec::new())
        }
        EntryKind::Dir { .. } => {
            let (at, bytes) = l.udf_dirs[i];
            (file_type::DIRECTORY, bytes, split(bytes, at - l.partition), Vec::new())
        }
        EntryKind::Symlink(target) => {
            let components = path_components(target);
            (file_type::SYMLINK, components.len() as u64, Vec::new(), components)
        }
    };
    let descriptors =
        if embedded.is_empty() && !extents.is_empty() { extents.len() * 8 } else { embedded.len() };
    if 176 + descriptors > SECTOR as usize {
        let name = String::from_utf8_lossy(&e.name).into_owned();
        return Err(Error::TooLong(format!("the file {name}")));
    }
    let mut out = vec![0u8; 176 + descriptors];
    // The ICB tag: strategy 4, one entry, the file's type, and how its
    // bytes are found (0: short descriptors, 3: in the entry).
    put16(&mut out, 20, 4);
    put16(&mut out, 24, 1);
    out[27] = kind;
    put16(&mut out, 34, if kind == file_type::SYMLINK { 3 } else { 0 });
    put32(&mut out, 36, e.uid);
    put32(&mut out, 40, e.gid);
    put32(&mut out, 44, permissions(e.mode));
    let links = match e.kind {
        // Its identifier in its parent, and each directory's parent
        // identifier: one less than POSIX counts.
        EntryKind::Dir { .. } => e.links - 1,
        _ => e.links,
    };
    put16(&mut out, 48, links.min(u32::from(u16::MAX)) as u16);
    out[56..64].copy_from_slice(&length.to_le_bytes());
    let blocks: u64 = extents.iter().map(|&(len, _)| len.div_ceil(SECTOR)).sum();
    out[64..72].copy_from_slice(&blocks.to_le_bytes());
    timestamp(&mut out[72..84], e.accessed);
    timestamp(&mut out[84..96], e.modified);
    timestamp(&mut out[96..108], e.changed);
    put32(&mut out, 108, 1);
    implementation(&mut out[128..160]);
    out[160..168].copy_from_slice(&e.unique.to_le_bytes());
    put32(&mut out, 172, descriptors as u32);
    if embedded.is_empty() {
        for (n, (len, at)) in extents.into_iter().enumerate() {
            put32(&mut out, 176 + 8 * n, len as u32);
            put32(&mut out, 180 + 8 * n, at as u32);
        }
    } else {
        out[176..].copy_from_slice(&embedded);
    }
    tag(&mut out, id::FILE_ENTRY, location);
    Ok(out)
}

/// `bytes` from partition block `at` as extents a short descriptor
/// holds: (bytes, block). None for none.
fn split(bytes: u64, at: u64) -> Vec<(u64, u64)> {
    let mut out = Vec::new();
    let (mut left, mut at) = (bytes, at);
    while left > 0 {
        let part = left.min(MAX_EXTENT);
        out.push((part, at));
        left -= part;
        at += part / SECTOR;
    }
    out
}

/// POSIX permissions as UDF's: other's, group's and owner's in five bits
/// each (execute, write, read, change attributes, delete), the last two
/// with write.
fn permissions(mode: u32) -> u32 {
    let mut out = 0;
    for (shift, bits) in [(0, mode & 7), (5, mode >> 3 & 7), (10, mode >> 6 & 7)] {
        let x = bits & 1;
        let w = bits >> 1 & 1;
        let r = bits >> 2 & 1;
        out |= (x | w << 1 | r << 2 | w << 3 | w << 4) << shift;
    }
    out
}

/// A link's path as ECMA-167's path components: the root, `..`, `.` and
/// names.
fn path_components(target: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    if target.first() == Some(&b'/') {
        out.extend_from_slice(&[1, 0, 0, 0]);
    }
    for part in target.split(|&b| b == b'/').filter(|p| !p.is_empty()) {
        match part {
            b".." => out.extend_from_slice(&[3, 0, 0, 0]),
            b"." => out.extend_from_slice(&[4, 0, 0, 0]),
            name => {
                let name = cs0(&String::from_utf8_lossy(name));
                out.extend_from_slice(&[5, name.len() as u8, 0, 0]);
                out.extend_from_slice(&name);
            }
        }
    }
    out
}

/// A directory's identifiers' size in bytes.
pub(crate) fn identifiers_size(image: &Image, dir: usize) -> Result<u64, Error> {
    let EntryKind::Dir { iso, .. } = &image.entries[dir].kind else { return Ok(0) };
    let mut size = identifier_len(0);
    for &child in iso {
        let name = name_of(&image.entries[child])?;
        size += identifier_len(name.len());
    }
    Ok(size as u64)
}

/// A file identifier descriptor's length with a name of `len` bytes.
fn identifier_len(len: usize) -> usize {
    (38 + len).next_multiple_of(4)
}

/// An entry's name in compressed Unicode, at most 255 bytes.
fn name_of(entry: &Entry) -> Result<Vec<u8>, Error> {
    let name = cs0(&String::from_utf8_lossy(&entry.name));
    if name.len() > 255 {
        return Err(Error::TooLong(format!("the name {}", String::from_utf8_lossy(&entry.name))));
    }
    Ok(name)
}

/// Directory `dir`'s identifiers: its parent's, then one for each entry.
fn identifiers(image: &Image, dir: usize) -> Result<Vec<u8>, Error> {
    let l = &image.layout;
    let EntryKind::Dir { iso, .. } = &image.entries[dir].kind else { return Ok(Vec::new()) };
    let start = l.udf_dirs[dir].0 - l.partition;
    let mut out = Vec::new();
    let parent = image.entries[dir].parent;
    // The parent's: a directory, the parent, with no name.
    identifier(&mut out, start, (0x02 | 0x08, &[]), image, parent);
    for &child in iso {
        let entry = &image.entries[child];
        let characteristics = if matches!(entry.kind, EntryKind::Dir { .. }) { 0x02 } else { 0 };
        identifier(
            &mut out,
            start,
            (characteristics, &name_of(entry)?),
            image,
            canonical(image, child),
        );
    }
    Ok(out)
}

/// Adds to `out`, the identifiers of a directory from partition block
/// `start`, one of `name` with `characteristics` for entry `entry`.
fn identifier(
    out: &mut Vec<u8>,
    start: u64,
    (characteristics, name): (u8, &[u8]),
    image: &Image,
    entry: usize,
) {
    let l = &image.layout;
    let at = out.len();
    let len = identifier_len(name.len());
    let mut fid = vec![0u8; len];
    put16(&mut fid, 16, 1);
    fid[18] = characteristics;
    fid[19] = name.len() as u8;
    let icb = (l.file_entries[entry] - l.partition) as u32;
    long_ad(&mut fid[20..36], SECTOR as u32, icb, image.entries[entry].unique as u32);
    fid[38..38 + name.len()].copy_from_slice(name);
    let location = start + (at as u64 / SECTOR);
    tag(&mut fid, id::IDENTIFIER, location as u32);
    out.extend_from_slice(&fid);
}

/// Compressed Unicode: a byte saying 8 or 16 bits a character, then the
/// characters, Latin-1 when all fit, else UCS-2 big-endian, characters
/// past it as `_`.
fn cs0(text: &str) -> Vec<u8> {
    if text.chars().all(|c| (c as u32) < 0x100) {
        let mut out = vec![8];
        out.extend(text.chars().map(|c| c as u8));
        out
    } else {
        let mut out = vec![16];
        for c in text.chars() {
            let unit = if (c as u32) > 0xffff { '_' as u16 } else { c as u16 };
            out.extend_from_slice(&unit.to_be_bytes());
        }
        out
    }
}

/// A d-string: `text` in compressed Unicode, cut to fit, its length in the
/// last byte; empty for no text.
fn dstring(out: &mut [u8], text: &str) {
    out.fill(0);
    if text.is_empty() {
        return;
    }
    let mut bytes = cs0(text);
    let room = out.len() - 1;
    if bytes.len() > room {
        let unit = if bytes[0] == 16 { 2 } else { 1 };
        bytes.truncate(1 + (room - 1) / unit * unit);
    }
    out[..bytes.len()].copy_from_slice(&bytes);
    out[room] = bytes.len() as u8;
}

/// The character set every descriptor names: OSTA compressed Unicode.
fn charspec(out: &mut [u8]) {
    out.fill(0);
    let name = b"OSTA Compressed Unicode";
    out[1..=name.len()].copy_from_slice(name);
}

/// An entity identifier: no flags, `name`, and its suffix.
fn regid(out: &mut [u8], name: &str, suffix: &[u8]) {
    out.fill(0);
    out[1..=name.len()].copy_from_slice(name.as_bytes());
    out[24..24 + suffix.len()].copy_from_slice(suffix);
}

/// UDF's revision and the operating system's class, UNIX, as an
/// implementation's suffix holds them.
fn suffix_with_os() -> [u8; 4] {
    let [low, high] = REVISION.to_le_bytes();
    [low, high, 4, 0]
}

/// The implementation's identifier: Dukovo, of the UNIX class.
fn implementation(out: &mut [u8]) {
    regid(out, "*Dukovo", &[4, 0]);
}

/// A long allocation descriptor: `length` bytes at partition 0's block
/// `block`, and the unique ID's low 32 bits that UDF keeps in its
/// implementation use.
fn long_ad(out: &mut [u8], length: u32, block: u32, unique: u32) {
    put32(out, 0, length);
    put32(out, 4, block);
    put16(out, 8, 0);
    out[12..16].copy_from_slice(&unique.to_le_bytes());
}

/// A timestamp of `time` in UTC.
fn timestamp(out: &mut [u8], time: i64) {
    let (year, month, day, hour, minute, second) = date(time);
    // Local time, offset 0.
    put16(out, 0, 1 << 12);
    put16(out, 2, year.clamp(1, 9999) as u16);
    out[4..10].copy_from_slice(&[
        month as u8,
        day as u8,
        hour as u8,
        minute as u8,
        second as u8,
        0,
    ]);
    out[10..12].fill(0);
}

/// Fills `out`'s tag: its identifier, version, the checksum of the tag,
/// and the CRC of everything after it, at `location`.
fn tag(out: &mut [u8], identifier: u16, location: u32) {
    put16(out, 0, identifier);
    put16(out, 2, VERSION);
    out[5] = 0;
    put16(out, 6, 0);
    let body = &out[16..];
    let crc = crc_itu(body);
    let length = body.len() as u16;
    put16(out, 8, crc);
    put16(out, 10, length);
    put32(out, 12, location);
    out[4] = 0;
    let sum = out[..16]
        .iter()
        .enumerate()
        .filter(|&(i, _)| i != 4)
        .fold(0u8, |s, (_, &b)| s.wrapping_add(b));
    out[4] = sum;
}

/// CRC-ITU-T (CCITT, polynomial 0x1021, from 0), as ECMA-167 7.2.6 has
/// the tags carry it.
fn crc_itu(bytes: &[u8]) -> u16 {
    let mut crc = 0u16;
    for &byte in bytes {
        crc ^= u16::from(byte) << 8;
        for _ in 0..8 {
            crc = if crc & 0x8000 != 0 { crc << 1 ^ 0x1021 } else { crc << 1 };
        }
    }
    crc
}

fn put16(out: &mut [u8], at: usize, value: u16) {
    out[at..at + 2].copy_from_slice(&value.to_le_bytes());
}

fn put32(out: &mut [u8], at: usize, value: u32) {
    out[at..at + 4].copy_from_slice(&value.to_le_bytes());
}
