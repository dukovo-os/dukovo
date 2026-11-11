// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! ISO 9660's structures (ECMA-119) and Joliet's: the names, the
//! directories' records, the path tables and the volume descriptors.
//!
//! The primary tree's names are of interchange level 1, eight characters
//! and three of upper case, digits and `_`, made unique in each directory
//! with digits: Rock Ridge carries the real ones. Joliet's are the real
//! names in UCS-2, cut to 64 characters and made unique the same way, as
//! Windows reads them. A file larger than an extent's 4 GiB is several
//! records, all but the last marked as having more (level 3).

use super::rock::{self, Areas, Record};
use super::{Entry, EntryKind, Error, Image, Layout, SECTOR, both16, both32, date};

/// A directory record's fixed part, before its identifier.
const RECORD: usize = 33;
/// The most bytes an extent holds: 4 GiB less a sector.
const MAX_EXTENT: u64 = 0xffff_f800;
/// Joliet's longest name, in UCS-2 characters, without its `;1`.
const JOLIET_NAME: usize = 64;

/// A directory record's flags.
mod flag {
    pub const DIRECTORY: u8 = 1 << 1;
    pub const MULTI_EXTENT: u8 = 1 << 7;
}

/// Gives each entry its ISO 9660 and Joliet name, unique in its
/// directory, and puts each directory's entries in each tree's order.
pub(crate) fn name(image: &mut Image) -> Result<(), Error> {
    for dir in 0..image.entries.len() {
        let EntryKind::Dir { iso, .. } = &image.entries[dir].kind else { continue };
        let mut children = iso.clone();
        let mut taken_iso = std::collections::HashSet::new();
        let mut taken_joliet = std::collections::HashSet::new();
        for &child in &children {
            let entry = &image.entries[child];
            let is_dir = matches!(entry.kind, EntryKind::Dir { .. });
            let iso = unique(iso_name(&entry.name, is_dir), is_dir, &mut taken_iso);
            let joliet = unique_joliet(&joliet_name(&entry.name), &mut taken_joliet);
            let entry = &mut image.entries[child];
            entry.iso = iso;
            entry.joliet = joliet;
        }
        children.sort_by(|&a, &b| sort_key(&image.entries[a]).cmp(&sort_key(&image.entries[b])));
        let mut joliet = children.clone();
        joliet.sort_by(|&a, &b| image.entries[a].joliet.cmp(&image.entries[b].joliet));
        image.entries[dir].kind = EntryKind::Dir { iso: children, joliet };
    }
    Ok(())
}

/// The characters of `text` ISO 9660's d-characters allow: upper case,
/// digits and `_`; others become `_`.
fn d_characters(text: &[u8], most: usize) -> Vec<u8> {
    let text = String::from_utf8_lossy(text);
    text.chars()
        .map(|c| match c.to_ascii_uppercase() {
            c @ ('A'..='Z' | '0'..='9' | '_') => c as u8,
            _ => b'_',
        })
        .take(most)
        .collect()
}

/// An entry's name of eight and three characters: (name, extension); a
/// directory has no extension.
fn iso_name(name: &[u8], is_dir: bool) -> (Vec<u8>, Vec<u8>) {
    if is_dir {
        let base = d_characters(name, 8);
        return (if base.is_empty() { b"_".to_vec() } else { base }, Vec::new());
    }
    let (base, ext) = match name.iter().rposition(|&b| b == b'.') {
        Some(0) | None => (name, &b""[..]),
        Some(at) => (&name[..at], &name[at + 1..]),
    };
    let base = d_characters(base, 8);
    (if base.is_empty() { b"_".to_vec() } else { base }, d_characters(ext, 3))
}

/// `name` made unique among those `taken` in its directory, its end
/// replaced by a number, and written as ISO 9660 records it: `NAME.EXT;1`
/// for a file, `NAME` for a directory.
fn unique(
    name: (Vec<u8>, Vec<u8>),
    is_dir: bool,
    taken: &mut std::collections::HashSet<(Vec<u8>, Vec<u8>)>,
) -> Vec<u8> {
    let (mut base, ext) = name;
    let mut n = 0u32;
    while taken.contains(&(base.clone(), ext.clone())) {
        let digits = n.to_string();
        let keep = (8 - digits.len()).min(base.len());
        base.truncate(keep);
        base.extend_from_slice(digits.as_bytes());
        n += 1;
    }
    taken.insert((base.clone(), ext.clone()));
    if is_dir {
        base
    } else {
        let mut id = base;
        id.push(b'.');
        id.extend_from_slice(&ext);
        id.extend_from_slice(b";1");
        id
    }
}

/// ECMA-119 9.3's order: by name, then by extension, each padded with
/// spaces.
fn sort_key(entry: &Entry) -> ([u8; 8], [u8; 3]) {
    let id = &entry.iso;
    let end = id.iter().position(|&b| b == b';').unwrap_or(id.len());
    let (base, ext) = match id[..end].iter().position(|&b| b == b'.') {
        Some(at) => (&id[..at], &id[at + 1..end]),
        None => (&id[..end], &b""[..]),
    };
    let mut key = ([b' '; 8], [b' '; 3]);
    key.0[..base.len()].copy_from_slice(base);
    key.1[..ext.len()].copy_from_slice(ext);
    key
}

/// An entry's Joliet name: its name in UCS-2, without the characters
/// Joliet forbids, cut to 64 characters with its extension kept.
fn joliet_name(name: &[u8]) -> Vec<u16> {
    let text = String::from_utf8_lossy(name);
    let units: Vec<u16> = text
        .chars()
        .map(|c| match c {
            '*' | '/' | ':' | ';' | '?' | '\\' => '_' as u16,
            c if (c as u32) < 0x20 || (c as u32) > 0xffff => '_' as u16,
            c => c as u16,
        })
        .collect();
    if units.len() <= JOLIET_NAME {
        return units;
    }
    let dot =
        units.iter().rposition(|&u| u == '.' as u16).filter(|&at| at > 0 && units.len() - at <= 16);
    match dot {
        Some(at) => {
            let ext = &units[at..];
            let mut cut = units[..JOLIET_NAME - ext.len()].to_vec();
            cut.extend_from_slice(ext);
            cut
        }
        None => units[..JOLIET_NAME].to_vec(),
    }
}

/// `name` made unique among those `taken`, a number put before its
/// extension.
fn unique_joliet(name: &[u16], taken: &mut std::collections::HashSet<Vec<u16>>) -> Vec<u16> {
    let mut candidate = name.to_vec();
    let mut n = 1u32;
    while taken.contains(&candidate) {
        let digits: Vec<u16> = format!("~{n}").encode_utf16().collect();
        let dot =
            name.iter().rposition(|&u| u == '.' as u16).filter(|&at| at > 0).unwrap_or(name.len());
        let (base, ext) = name.split_at(dot);
        let keep = base.len().min(JOLIET_NAME - digits.len() - ext.len().min(JOLIET_NAME / 2));
        candidate = [&base[..keep], &digits, ext].concat();
        candidate.truncate(JOLIET_NAME);
        n += 1;
    }
    taken.insert(candidate.clone());
    candidate
}

/// The directories in the path table's order: level by level, each
/// level's by their parents' order and then by name.
pub(crate) fn dir_order(entries: &[Entry], joliet: bool) -> Vec<usize> {
    let mut order = vec![0];
    let mut next = 0;
    while next < order.len() {
        if let EntryKind::Dir { iso, joliet: jo } = &entries[order[next]].kind {
            let children = if joliet { jo } else { iso };
            order.extend(
                children.iter().filter(|&&c| matches!(entries[c].kind, EntryKind::Dir { .. })),
            );
        }
        next += 1;
    }
    order
}

/// A directory's identifier as its records and path table entry hold it.
fn identifier(entry: &Entry, joliet: bool) -> Vec<u8> {
    if joliet {
        let mut id: Vec<u8> = entry.joliet.iter().flat_map(|u| u.to_be_bytes()).collect();
        if !matches!(entry.kind, EntryKind::Dir { .. }) {
            id.extend_from_slice(&[0, b';', 0, b'1']);
        }
        id
    } else {
        entry.iso.clone()
    }
}

/// A path table's size in bytes.
pub(crate) fn path_table_size(entries: &[Entry], order: &[usize], joliet: bool) -> u64 {
    order
        .iter()
        .map(|&dir| {
            let len = if dir == 0 { 1 } else { identifier(&entries[dir], joliet).len() };
            8 + len + len % 2
        })
        .sum::<usize>() as u64
}

/// The path tables of the primary tree or Joliet's, little-endian and
/// big-endian.
pub(crate) fn path_tables(image: &Image, joliet: bool) -> (Vec<u8>, Vec<u8>) {
    let l = &image.layout;
    let (order, dirs) =
        if joliet { (&l.joliet_order, &l.joliet_dirs) } else { (&l.iso_order, &l.iso_dirs) };
    let number: std::collections::HashMap<usize, u16> =
        order.iter().enumerate().map(|(i, &d)| (d, (i + 1) as u16)).collect();
    let (mut little, mut big) = (Vec::new(), Vec::new());
    for &dir in order {
        let entry = &image.entries[dir];
        let id = if dir == 0 { vec![0] } else { identifier(entry, joliet) };
        let parent = number[&entry.parent];
        let extent = dirs[dir].0 as u32;
        for (table, little_endian) in [(&mut little, true), (&mut big, false)] {
            table.push(id.len() as u8);
            table.push(0);
            table.extend_from_slice(&if little_endian {
                extent.to_le_bytes()
            } else {
                extent.to_be_bytes()
            });
            table.extend_from_slice(&if little_endian {
                parent.to_le_bytes()
            } else {
                parent.to_be_bytes()
            });
            table.extend_from_slice(&id);
            if id.len() % 2 == 1 {
                table.push(0);
            }
        }
    }
    (little, big)
}

/// A directory record: `id`, the extent at `extent` of `size` bytes,
/// recorded at `time`, with `flags` and the System Use bytes `system`.
fn record(id: &[u8], (extent, size): (u64, u64), time: i64, flags: u8, system: &[u8]) -> Vec<u8> {
    let pad = usize::from(id.len().is_multiple_of(2));
    let len = RECORD + id.len() + pad + system.len();
    let mut out = vec![0u8; len + len % 2];
    out[0] = out.len() as u8;
    both32(&mut out, 2, extent as u32);
    both32(&mut out, 10, size as u32);
    out[18..25].copy_from_slice(&short_date(time));
    out[25] = flags;
    both16(&mut out, 28, 1);
    out[32] = id.len() as u8;
    out[33..33 + id.len()].copy_from_slice(id);
    let at = 33 + id.len() + pad;
    out[at..at + system.len()].copy_from_slice(system);
    out
}

/// The bytes an identifier of `id_len` bytes leaves a record for its
/// System Use: a record is at most 255 bytes, and even.
fn room(id_len: usize) -> usize {
    (255 - RECORD - id_len - usize::from(id_len.is_multiple_of(2))) & !1
}

/// Where an entry's bytes are, in the tree asked for: a directory's
/// extent, a file's, or none for a link or an empty file.
fn extent(image: &Image, layout: &Layout, entry: usize, joliet: bool) -> (u64, u64) {
    match image.entries[entry].kind {
        EntryKind::Dir { .. } => {
            let (at, sectors) =
                if joliet { layout.joliet_dirs[entry] } else { layout.iso_dirs[entry] };
            (at, sectors * SECTOR)
        }
        EntryKind::File { size, data } => (image.data[data].at, size),
        EntryKind::Symlink(_) => (layout.data.0, 0),
    }
}

/// The directory `dir`'s records in the primary tree or Joliet's, in
/// whole sectors: `.`, `..`, then its entries in the tree's order, a
/// record never across a sector's end. Rock Ridge's entries that do not
/// fit a record go to `areas`.
pub(crate) fn directory(
    image: &Image,
    layout: &Layout,
    dir: usize,
    joliet: bool,
    areas: &mut Areas,
) -> Result<Vec<u8>, Error> {
    let rock = image.options.rock && !joliet;
    let entry = &image.entries[dir];
    let EntryKind::Dir { iso, joliet: jo } = &entry.kind else { unreachable!("a directory") };
    let mut records = Vec::new();
    for (id, of, which) in [(0u8, dir, Record::Itself), (1, entry.parent, Record::Parent)] {
        let system =
            if rock { rock::system_use(image, dir, which, room(1), areas)? } else { Vec::new() };
        let at = extent(image, layout, of, joliet);
        records.push(record(&[id], at, image.entries[of].modified, flag::DIRECTORY, &system));
    }
    for &child in if joliet { jo } else { iso } {
        let entry = &image.entries[child];
        let id = identifier(entry, joliet);
        let system = if rock {
            rock::system_use(image, child, Record::Child, room(id.len()), areas)?
        } else {
            Vec::new()
        };
        let (start, size) = extent(image, layout, child, joliet);
        let flags = if matches!(entry.kind, EntryKind::Dir { .. }) { flag::DIRECTORY } else { 0 };
        // A file of more than an extent is several records of the same
        // name, each but the last saying more follow.
        let mut left = size;
        let mut at = start;
        loop {
            let part = left.min(MAX_EXTENT);
            let more = if left > part { flag::MULTI_EXTENT } else { 0 };
            records.push(record(&id, (at, part), entry.modified, flags | more, &system));
            left -= part;
            at += part / SECTOR;
            if left == 0 {
                break;
            }
        }
    }
    let mut out = Vec::new();
    for record in records {
        let used = out.len() % SECTOR as usize;
        if used + record.len() > SECTOR as usize {
            out.resize(out.len() + SECTOR as usize - used, 0);
        }
        out.extend_from_slice(&record);
    }
    out.resize(out.len().div_ceil(SECTOR as usize).max(1) * SECTOR as usize, 0);
    Ok(out)
}

/// The primary volume descriptor, or Joliet's supplementary one.
pub(crate) fn volume_descriptor(image: &Image, joliet: bool) -> Vec<u8> {
    let l = &image.layout;
    let options = &image.options;
    let mut out = vec![0u8; SECTOR as usize];
    out[0] = if joliet { 2 } else { 1 };
    out[1..6].copy_from_slice(b"CD001");
    out[6] = 1;
    let text = |out: &mut [u8], at: usize, len: usize, value: &str, d: bool| {
        if joliet {
            let units: Vec<u8> =
                value.encode_utf16().take(len / 2).flat_map(u16::to_be_bytes).collect();
            // UCS-2 spaces; a field of an odd length ends in a zero.
            out[at..at + len].fill(0);
            for pair in out[at..at + len].as_chunks_mut::<2>().0 {
                *pair = [0, b' '];
            }
            out[at..at + units.len()].copy_from_slice(&units);
        } else {
            let value = if d {
                d_characters(value.as_bytes(), len)
            } else {
                value.as_bytes().iter().take(len).copied().collect()
            };
            out[at..at + len].fill(b' ');
            out[at..at + value.len()].copy_from_slice(&value);
        }
    };
    text(&mut out, 8, 32, "DUKOVO", false);
    text(&mut out, 40, 32, &options.label, true);
    both32(&mut out, 80, l.total as u32);
    if joliet {
        // UCS-2 Level 3.
        out[88..91].copy_from_slice(b"%/E");
    }
    both16(&mut out, 120, 1);
    both16(&mut out, 124, 1);
    both16(&mut out, 128, SECTOR as u16);
    let (little, big, size) = if joliet { l.joliet_tables } else { l.iso_tables };
    both32(&mut out, 132, size as u32);
    out[140..144].copy_from_slice(&(little as u32).to_le_bytes());
    out[148..152].copy_from_slice(&(big as u32).to_be_bytes());
    let root = extent(image, l, 0, joliet);
    let root = record(&[0], root, image.entries[0].modified, flag::DIRECTORY, &[]);
    out[156..156 + 34].copy_from_slice(&root);
    text(&mut out, 190, 128, &options.label, true);
    text(&mut out, 318, 128, "", false);
    text(&mut out, 446, 128, "DUKOVO MKISO", false);
    text(&mut out, 574, 128, "DUKOVO MKISO", false);
    for at in [702, 739, 776] {
        text(&mut out, at, 37, "", false);
    }
    out[813..830].copy_from_slice(&long_date(Some(options.time)));
    out[830..847].copy_from_slice(&long_date(Some(options.time)));
    out[847..864].copy_from_slice(&long_date(None));
    out[864..881].copy_from_slice(&long_date(None));
    out[881] = 1;
    out
}

/// The volume descriptor set's terminator.
pub(crate) fn terminator() -> Vec<u8> {
    let mut out = vec![0u8; SECTOR as usize];
    out[0] = 255;
    out[1..6].copy_from_slice(b"CD001");
    out[6] = 1;
    out
}

/// A record's date: years since 1900, month, day, hour, minute, second
/// and the zone, UTC.
pub(crate) fn short_date(time: i64) -> [u8; 7] {
    let (year, month, day, hour, minute, second) = date(time);
    let year = (year - 1900).clamp(0, 255) as u8;
    [year, month as u8, day as u8, hour as u8, minute as u8, second as u8, 0]
}

/// A volume descriptor's date, in digits, or all zeros for none.
fn long_date(time: Option<i64>) -> [u8; 17] {
    let mut out = [b'0'; 17];
    out[16] = 0;
    if let Some(time) = time {
        let (year, month, day, hour, minute, second) = date(time);
        let text = format!(
            "{:04}{month:02}{day:02}{hour:02}{minute:02}{second:02}00",
            year.clamp(0, 9999)
        );
        out[..16].copy_from_slice(text.as_bytes());
    }
    out
}
