// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! AMD CPU microcode containers: parsing and matching.
//!
//! AMD's microcode comes as containers, as in the `amd-ucode` directory of
//! linux-firmware. A container starts with the magic `DMA\0`, then an
//! equivalence table, which maps the CPUID signatures of the processors it
//! serves to equivalence IDs, then patch sections, each a 64-byte header
//! and the patch data. A patch is for every processor whose signature
//! the table maps to the patch's equivalence ID. A container has no length
//! of its own: it runs to the next container, to the end of the file, or
//! to an Intel update that follows it in a file that holds both vendors'
//! microcode.
//!
//! The processor checks a patch's signature itself, so nothing here
//! verifies one. Reference: the formats of AMD's published microcode files
//! as linux-firmware ships them; the kernel's own loader is in
//! `kernel/arch/x86_64/src/microcode.rs`.

use crate::{Error, Update, date_of, u32_at};

/// The first word of a container: "DMA\0".
pub const MAGIC: u32 = 0x0041_4d44;
/// Bytes in a patch's header, which the processor reads with the data.
pub const PATCH_HEADER: usize = 64;
/// The largest patch of any family, which is the size of the buffer a
/// patch is copied to before it is loaded.
pub const PATCH_MAX: usize = 15_872;
/// Section type of the equivalence table.
const TABLE_SECTION: u32 = 0;
/// Section type of a patch.
const PATCH_SECTION: u32 = 1;
/// Bytes in the header of a section: its type and its size.
const SECTION_HEADER: usize = 8;
/// Bytes in an equivalence table entry.
const TABLE_ENTRY: usize = 16;

/// The largest patch a processor of `signature`'s family takes, as the
/// patches of linux-firmware show: 3,458 bytes on family 16h, which has
/// the largest of the older families, and 14,368 on family 1Ah, which
/// leaves room for the next.
pub fn patch_limit(signature: u32) -> usize {
    let base = (signature >> 8) & 0xf;
    let family = if base == 0xf { base + ((signature >> 20) & 0xff) } else { base };
    match family {
        ..=0x14 => 3_200,
        0x15 => 4_096,
        0x16 => 3_458,
        0x17 | 0x18 => 3_200,
        0x19 => 5_568,
        _ => PATCH_MAX,
    }
}

/// One checked container.
#[derive(Clone, Copy)]
pub struct Container<'a> {
    bytes: &'a [u8],
    table: &'a [u8],
}

impl core::fmt::Debug for Container<'_> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Container")
            .field("len", &self.bytes.len())
            .field("patches", &self.patches().count())
            .finish()
    }
}

impl<'a> Container<'a> {
    /// Parses and checks the container at the start of `bytes`, which
    /// runs to the next container, an Intel update or the end of `bytes`.
    pub fn parse(bytes: &'a [u8]) -> Result<Container<'a>, Error> {
        if u32_at(bytes, 0) != MAGIC {
            return Err(Error::UnknownSection);
        }
        if bytes.len() < 4 + SECTION_HEADER || u32_at(bytes, 4) != TABLE_SECTION {
            return Err(Error::UnknownSection);
        }
        let table_size = u32_at(bytes, 8) as usize;
        if table_size == 0 || !table_size.is_multiple_of(TABLE_ENTRY) {
            return Err(Error::BadSize);
        }
        let table_end = (4 + SECTION_HEADER).checked_add(table_size).ok_or(Error::BadSize)?;
        let table = bytes.get(4 + SECTION_HEADER..table_end).ok_or(Error::BadSize)?;
        let mut end = table_end;
        loop {
            let rest = &bytes[end..];
            if rest.is_empty() || u32_at(rest, 0) == MAGIC {
                break;
            }
            let kind = u32_at(rest, 0);
            let size = u32_at(rest, 4) as usize;
            // An Intel update's header version is 1, the number of a
            // patch section; only a whole update that passes its checksums
            // ends the container.
            if kind == PATCH_SECTION && Update::parse(rest).is_ok() {
                break;
            }
            if kind != PATCH_SECTION {
                return Err(Error::UnknownSection);
            }
            if rest.len() < SECTION_HEADER || !(PATCH_HEADER..=PATCH_MAX).contains(&size) {
                return Err(Error::BadSize);
            }
            if rest.len() - SECTION_HEADER < size {
                return Err(Error::BadSize);
            }
            end += SECTION_HEADER + size;
        }
        Ok(Container { bytes: &bytes[..end], table })
    }

    /// The whole container, magic included.
    pub fn bytes(&self) -> &'a [u8] {
        self.bytes
    }

    /// The equivalence ID the container gives a processor of `signature`.
    pub fn equivalence_id(&self, signature: u32) -> Option<u16> {
        self.table
            .as_chunks::<TABLE_ENTRY>()
            .0
            .iter()
            .take_while(|entry| u32_at(*entry, 0) != 0)
            .find(|entry| u32_at(*entry, 0) == signature)
            .map(|entry| u16::from_le_bytes([entry[12], entry[13]]))
    }

    /// The patches of the container, in order.
    pub fn patches(&self) -> impl Iterator<Item = Patch<'a>> + use<'a> {
        let mut rest = &self.bytes[4 + SECTION_HEADER + self.table.len()..];
        core::iter::from_fn(move || {
            if rest.is_empty() {
                return None;
            }
            let size = u32_at(rest, 4) as usize;
            let patch = Patch { bytes: &rest[SECTION_HEADER..SECTION_HEADER + size] };
            rest = &rest[SECTION_HEADER + size..];
            Some(patch)
        })
    }

    /// The newest patch for a processor of `signature`.
    pub fn find(&self, signature: u32) -> Option<Patch<'a>> {
        let equivalence = self.equivalence_id(signature)?;
        self.patches()
            .filter(|p| {
                p.equivalence_id() == equivalence && p.bytes.len() <= patch_limit(signature)
            })
            .max_by_key(Patch::revision)
    }
}

/// One patch: its header and the data the processor loads.
#[derive(Clone, Copy)]
pub struct Patch<'a> {
    bytes: &'a [u8],
}

impl core::fmt::Debug for Patch<'_> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Patch")
            .field("revision", &format_args!("{:#x}", self.revision()))
            .field("equivalence_id", &format_args!("{:#x}", self.equivalence_id()))
            .field("len", &self.bytes.len())
            .finish()
    }
}

impl<'a> Patch<'a> {
    /// The revision the processor reports once the patch is loaded.
    pub fn revision(&self) -> u32 {
        u32_at(self.bytes, 4)
    }

    /// The equivalence ID of the processors the patch is for.
    pub fn equivalence_id(&self) -> u16 {
        u16::from_le_bytes([self.bytes[24], self.bytes[25]])
    }

    /// Release date as (year, month, day).
    pub fn date(&self) -> (u16, u8, u8) {
        date_of(u32_at(self.bytes, 0))
    }

    /// The whole patch, header included: what the processor is given.
    pub fn bytes(&self) -> &'a [u8] {
        self.bytes
    }
}

/// The containers in a file, in order, skipping Intel updates in a file
/// that holds both. Iteration stops after the first piece that fails its
/// checks, which is returned as an error.
pub fn containers(file: &[u8]) -> impl Iterator<Item = Result<Container<'_>, Error>> {
    crate::pieces(file).filter_map(|piece| match piece {
        Ok(crate::Piece::Amd(container)) => Some(Ok(container)),
        Ok(crate::Piece::Intel(_)) => None,
        Err(err) => Some(Err(err)),
    })
}

/// The newest patch in `file` for a processor of `signature`.
pub fn find(file: &[u8], signature: u32) -> Option<Patch<'_>> {
    containers(file)
        .filter_map(Result::ok)
        .filter_map(|container| container.find(signature))
        .max_by_key(Patch::revision)
}
