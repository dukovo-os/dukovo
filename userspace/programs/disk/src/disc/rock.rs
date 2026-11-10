// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Rock Ridge (IEEE P1282, RRIP 1.10) in the System Use Sharing Protocol's
//! entries (IEEE P1281, SUSP 1.10): each record's POSIX mode, links, owner
//! and group (PX), times (TF), real name (NM) and a link's path (SL); and
//! in the root's first record, the protocol's mark (SP) and the
//! extension's reference (ER).
//!
//! A record holds at most 255 bytes. Entries that do not fit go to a
//! continuation area in a sector of their own kind ([`Areas`]), which a
//! CE entry in the record points at, as long names and links need.

use super::{EntryKind, Error, Image, both32, iso::short_date};

/// Which record of an entry's System Use is made.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Record {
    /// A directory's `.`, which describes the directory itself.
    Itself,
    /// A directory's `..`, which describes its parent.
    Parent,
    /// An entry in its directory, by its name.
    Child,
}

/// The longest continuation area: one sector.
const MAX_AREA: usize = 2048;
/// The CE entry's length.
const CE: usize = 28;

/// The extension's reference: its identifier, description and source, as
/// RRIP 1.10 gives them.
const ER_ID: &str = "RRIP_1991A";
const ER_DESCRIPTION: &str =
    "THE ROCK RIDGE INTERCHANGE PROTOCOL PROVIDES SUPPORT FOR POSIX FILE SYSTEM SEMANTICS";
const ER_SOURCE: &str = "PLEASE CONTACT DISC PUBLISHER FOR SPECIFICATION SOURCE.  SEE PUBLISHER IDENTIFIER IN PRIMARY VOLUME DESCRIPTOR FOR CONTACT INFORMATION.";

/// The continuation areas, packed into sectors from `base` on, an area
/// never across a sector's end.
pub(crate) struct Areas {
    base: u64,
    bytes: Vec<u8>,
}

impl Areas {
    /// No areas yet, to be placed from sector `base` on.
    pub fn new(base: u64) -> Areas {
        Areas { base, bytes: Vec::new() }
    }

    /// The sectors the areas fill.
    pub fn sectors(&self) -> u64 {
        (self.bytes.len() as u64).div_ceil(MAX_AREA as u64)
    }

    /// The areas' sectors.
    pub fn into_bytes(mut self) -> Vec<u8> {
        let len = self.sectors() as usize * MAX_AREA;
        self.bytes.resize(len, 0);
        self.bytes
    }

    /// Places `area`: its sector and its offset in it.
    fn place(&mut self, area: &[u8]) -> (u64, u32) {
        let used = self.bytes.len() % MAX_AREA;
        if used != 0 && used + area.len() > MAX_AREA {
            self.bytes.resize(self.bytes.len() + MAX_AREA - used, 0);
        }
        let at = self.bytes.len();
        self.bytes.extend_from_slice(area);
        (self.base + (at / MAX_AREA) as u64, (at % MAX_AREA) as u32)
    }
}

/// The System Use bytes of entry `entry`'s record `which`, at most `room`
/// of them in the record, the rest placed in `areas`.
pub(crate) fn system_use(
    image: &Image,
    entry: usize,
    which: Record,
    room: usize,
    areas: &mut Areas,
) -> Result<Vec<u8>, Error> {
    let described = match which {
        Record::Parent => image.entries[entry].parent,
        Record::Itself | Record::Child => entry,
    };
    let mut entries = Vec::new();
    if entry == 0 && which == Record::Itself {
        // The protocol's mark, first in the root's first record, with no
        // bytes to skip.
        entries.push(vec![b'S', b'P', 7, 1, 0xbe, 0xef, 0]);
    }
    entries.push(px(image, described));
    entries.push(tf(image, described));
    if which == Record::Child {
        let e = &image.entries[entry];
        entries.extend(nm(&e.name));
        if let EntryKind::Symlink(target) = &e.kind {
            entries.extend(sl(target));
        }
    }
    if entry == 0 && which == Record::Itself {
        entries.push(er());
    }
    let total: usize = entries.iter().map(Vec::len).sum();
    if total <= room {
        return Ok(entries.concat());
    }
    let mut record = Vec::new();
    let mut rest = entries.into_iter().peekable();
    while let Some(next) = rest.peek() {
        if record.len() + next.len() + CE > room {
            break;
        }
        record.extend(rest.next().expect("peeked"));
    }
    let area: Vec<u8> = rest.flatten().collect();
    if area.len() > MAX_AREA {
        let name = String::from_utf8_lossy(&image.entries[entry].name).into_owned();
        return Err(Error::TooLong(format!("the name or link {name}")));
    }
    let (sector, offset) = areas.place(&area);
    let mut ce = vec![0u8; CE];
    ce[..4].copy_from_slice(&[b'C', b'E', CE as u8, 1]);
    both32(&mut ce, 4, sector as u32);
    both32(&mut ce, 12, offset);
    both32(&mut ce, 20, area.len() as u32);
    record.extend(ce);
    Ok(record)
}

/// PX: the POSIX mode, with the file's type, its links, owner and group.
fn px(image: &Image, entry: usize) -> Vec<u8> {
    let e = &image.entries[entry];
    let kind = match e.kind {
        EntryKind::Dir { .. } => 0o040_000,
        EntryKind::File { .. } => 0o100_000,
        EntryKind::Symlink(_) => 0o120_000,
    };
    let mut out = vec![0u8; 36];
    out[..4].copy_from_slice(&[b'P', b'X', 36, 1]);
    both32(&mut out, 4, kind | e.mode);
    both32(&mut out, 12, e.links);
    both32(&mut out, 20, e.uid);
    both32(&mut out, 28, e.gid);
    out
}

/// TF: the times last modified, last read and last changed.
fn tf(image: &Image, entry: usize) -> Vec<u8> {
    let e = &image.entries[entry];
    // MODIFY, ACCESS and ATTRIBUTES, in that order, each of seven bytes.
    let mut out = vec![b'T', b'F', 26, 1, 0b1110];
    for time in [e.modified, e.accessed, e.changed] {
        out.extend_from_slice(&short_date(time));
    }
    out
}

/// NM: the name, in entries of at most 250 bytes, each but the last
/// saying it continues.
fn nm(name: &[u8]) -> Vec<Vec<u8>> {
    let parts: Vec<&[u8]> = name.chunks(250).collect();
    parts
        .iter()
        .enumerate()
        .map(|(i, part)| {
            let more = u8::from(i + 1 < parts.len());
            let mut out = vec![b'N', b'M', (5 + part.len()) as u8, 1, more];
            out.extend_from_slice(part);
            out
        })
        .collect()
}

/// SL: a link's path, as component records (the root, `.`, `..` or a
/// name, a long name in pieces), in entries of at most 255 bytes.
fn sl(target: &[u8]) -> Vec<Vec<u8>> {
    // Component records: flags, length, bytes.
    let mut components: Vec<Vec<u8>> = Vec::new();
    if target.first() == Some(&b'/') {
        components.push(vec![0x08, 0]);
    }
    for part in target.split(|&b| b == b'/').filter(|p| !p.is_empty()) {
        match part {
            b"." => components.push(vec![0x02, 0]),
            b".." => components.push(vec![0x04, 0]),
            name => {
                let pieces: Vec<&[u8]> = name.chunks(248).collect();
                for (i, piece) in pieces.iter().enumerate() {
                    let more = u8::from(i + 1 < pieces.len());
                    let mut record = vec![more, piece.len() as u8];
                    record.extend_from_slice(piece);
                    components.push(record);
                }
            }
        }
    }
    let mut entries: Vec<Vec<u8>> = Vec::new();
    let mut current = Vec::new();
    for component in components {
        if 5 + current.len() + component.len() > 255 {
            entries.push(std::mem::take(&mut current));
        }
        current.extend(component);
    }
    entries.push(current);
    let count = entries.len();
    entries
        .into_iter()
        .enumerate()
        .map(|(i, body)| {
            let more = u8::from(i + 1 < count);
            let mut out = vec![b'S', b'L', (5 + body.len()) as u8, 1, more];
            out.extend(body);
            out
        })
        .collect()
}

/// ER: the reference to Rock Ridge, which says the disc has it.
fn er() -> Vec<u8> {
    let (id, des, src) = (ER_ID.as_bytes(), ER_DESCRIPTION.as_bytes(), ER_SOURCE.as_bytes());
    let len = 8 + id.len() + des.len() + src.len();
    let mut out =
        vec![b'E', b'R', len as u8, 1, id.len() as u8, des.len() as u8, src.len() as u8, 1];
    out.extend_from_slice(id);
    out.extend_from_slice(des);
    out.extend_from_slice(src);
    out
}
