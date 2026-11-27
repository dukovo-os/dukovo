// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Intel and AMD CPU microcode: parsing, checking and matching.
//!
//! Processors ship with microcode that later updates fix; firmware applies
//! some, and the operating system loads newer ones early in boot. Updates
//! are closed firmware, so Dukovo never includes them in the base system
//! (ADR 0007); `ob image --microcode` adds a file of them to an image, GRUB
//! loads it as a module, and the kernel looks for an update for each CPU
//! here before writing it to the processor.
//!
//! A file is any number of Intel updates and AMD containers back to back,
//! so that one file serves every machine (ADR 0792). An Intel update is a
//! 48-byte header, the update data, and optionally a table of further
//! processor signatures it applies to, as in the `intel-ucode` directory of
//! Intel's microcode releases. Reference: Intel 64 and IA-32 Architectures
//! Software Developer's Manual, volume 3, section 10.11 "Microcode Update
//! Facilities". AMD's containers are in [`amd`]. [`updates`] and [`find`]
//! read Intel's and skip AMD's; [`pieces`] gives both.

#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]

use core::fmt;

pub mod amd;

/// Bytes in an update header.
pub const HEADER: usize = 48;
/// Data size of an update whose header says 0.
const DEFAULT_DATA: usize = 2000;
/// Total size of an update whose header says 0.
const DEFAULT_TOTAL: usize = 2048;
/// Bytes before the entries of the extended signature table.
const EXT_HEADER: usize = 20;
/// Bytes per extended signature.
const EXT_ENTRY: usize = 12;

/// Why an update was rejected.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// Header or loader version other than 1.
    UnknownVersion,
    /// The sizes in the header are inconsistent or exceed the file.
    BadSize,
    /// A checksum does not add up.
    BadChecksum,
    /// An AMD container holds a section of a type that is neither its
    /// equivalence table nor a patch, or its magic is wrong.
    UnknownSection,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Error::UnknownVersion => "unknown microcode header version",
            Error::BadSize => "inconsistent microcode update size",
            Error::BadChecksum => "microcode checksum mismatch",
            Error::UnknownSection => "unknown section in an AMD microcode container",
        })
    }
}

fn u32_at(bytes: &[u8], offset: usize) -> u32 {
    bytes.get(offset..offset + 4).and_then(|b| b.try_into().ok()).map_or(0, u32::from_le_bytes)
}

/// A release date given as 0xMMDDYYYY in BCD, as (year, month, day).
fn date_of(date: u32) -> (u16, u8, u8) {
    let bcd = |value: u32| {
        (0..4).rev().fold(0u32, |acc, digit| acc * 10 + ((value >> (digit * 4)) & 0xf))
    };
    (bcd(date & 0xffff) as u16, bcd(date >> 24) as u8, bcd((date >> 16) & 0xff) as u8)
}

/// Sum of the little-endian 32-bit words of `bytes`.
fn word_sum(bytes: &[u8]) -> u32 {
    bytes.as_chunks::<4>().0.iter().fold(0u32, |sum, w| sum.wrapping_add(u32::from_le_bytes(*w)))
}

/// The CPU identity an update is matched against.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CpuId {
    /// CPUID leaf 1, EAX: family, model and stepping.
    pub signature: u32,
    /// `1 << platform ID`, where the platform ID is bits 50 to 52 of the
    /// `IA32_PLATFORM_ID` MSR.
    pub platform: u32,
}

/// One checked microcode update.
#[derive(Clone, Copy)]
pub struct Update<'a> {
    bytes: &'a [u8],
    data_size: usize,
}

impl fmt::Debug for Update<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Update")
            .field("revision", &format_args!("{:#x}", self.revision()))
            .field("signature", &format_args!("{:#x}", self.signature()))
            .field("platforms", &format_args!("{:#x}", self.platforms()))
            .field("len", &self.bytes.len())
            .finish()
    }
}

impl<'a> Update<'a> {
    /// Parses and checks the update at the start of `bytes`.
    pub fn parse(bytes: &'a [u8]) -> Result<Update<'a>, Error> {
        if bytes.len() < HEADER {
            return Err(Error::BadSize);
        }
        if u32_at(bytes, 0) != 1 || u32_at(bytes, 20) != 1 {
            return Err(Error::UnknownVersion);
        }
        let data_size = match u32_at(bytes, 28) as usize {
            0 => DEFAULT_DATA,
            size => size,
        };
        let total_size = match u32_at(bytes, 32) as usize {
            0 => DEFAULT_TOTAL,
            size => size,
        };
        if !data_size.is_multiple_of(4)
            || !total_size.is_multiple_of(4)
            || HEADER + data_size > total_size
            || total_size > bytes.len()
        {
            return Err(Error::BadSize);
        }
        let bytes = &bytes[..total_size];
        if word_sum(&bytes[..HEADER + data_size]) != 0 {
            return Err(Error::BadChecksum);
        }
        let update = Update { bytes, data_size };
        if let Some(table) = update.extended_table() {
            let count = u32_at(table, 0) as usize;
            let len = count.checked_mul(EXT_ENTRY).and_then(|n| n.checked_add(EXT_HEADER));
            let table = len.and_then(|len| table.get(..len)).ok_or(Error::BadSize)?;
            if word_sum(table) != 0 {
                return Err(Error::BadChecksum);
            }
            // Each extra signature's checksum stands in for the main one.
            let main =
                update.signature().wrapping_add(update.platforms()).wrapping_add(update.checksum());
            for entry in table[EXT_HEADER..].as_chunks::<EXT_ENTRY>().0 {
                let sum = word_sum(entry);
                if sum != main {
                    return Err(Error::BadChecksum);
                }
            }
        }
        Ok(update)
    }

    fn extended_table(&self) -> Option<&'a [u8]> {
        let rest = &self.bytes[HEADER + self.data_size..];
        (rest.len() >= EXT_HEADER).then_some(rest)
    }

    /// The revision the processor reports once the update is loaded.
    pub fn revision(&self) -> u32 {
        u32_at(self.bytes, 4)
    }

    /// Release date as (year, month, day).
    pub fn date(&self) -> (u16, u8, u8) {
        date_of(u32_at(self.bytes, 8))
    }

    /// The main processor signature.
    pub fn signature(&self) -> u32 {
        u32_at(self.bytes, 12)
    }

    fn checksum(&self) -> u32 {
        u32_at(self.bytes, 16)
    }

    /// The platform IDs of the main signature, as a bit mask.
    pub fn platforms(&self) -> u32 {
        u32_at(self.bytes, 24)
    }

    /// The whole update, header included.
    pub fn bytes(&self) -> &'a [u8] {
        self.bytes
    }

    /// The update data, which is what the processor is given. Its address
    /// must be 16-byte aligned when it is loaded.
    pub fn data(&self) -> &'a [u8] {
        &self.bytes[HEADER..HEADER + self.data_size]
    }

    /// Every (signature, platform mask) the update applies to.
    pub fn targets(&self) -> impl Iterator<Item = (u32, u32)> + 'a {
        let main = core::iter::once((self.signature(), self.platforms()));
        let extended = self.extended_table().map_or(&[][..], |table| {
            let count = u32_at(table, 0) as usize;
            &table[EXT_HEADER..EXT_HEADER + count * EXT_ENTRY]
        });
        main.chain(extended.as_chunks::<EXT_ENTRY>().0.iter().map(|e| (u32_at(e, 0), u32_at(e, 4))))
    }

    /// Whether the update is for `cpu`.
    pub fn matches(&self, cpu: CpuId) -> bool {
        self.targets().any(|(signature, platforms)| {
            signature == cpu.signature
                && ((platforms == 0 && cpu.platform == 0) || platforms & cpu.platform != 0)
        })
    }
}

/// One piece of a file that may hold both vendors' microcode.
#[derive(Clone, Copy, Debug)]
pub enum Piece<'a> {
    /// An Intel update.
    Intel(Update<'a>),
    /// An AMD container.
    Amd(amd::Container<'a>),
}

/// The pieces of a file, in order: a container if the next word is AMD's
/// magic, an Intel update otherwise. Iteration stops after the first
/// piece that fails its checks, which is returned as an error.
pub fn pieces(file: &[u8]) -> impl Iterator<Item = Result<Piece<'_>, Error>> {
    let mut rest = file;
    core::iter::from_fn(move || {
        if rest.is_empty() {
            return None;
        }
        let piece = if u32_at(rest, 0) == amd::MAGIC {
            amd::Container::parse(rest).map(|container| (Piece::Amd(container), container.bytes()))
        } else {
            Update::parse(rest).map(|update| (Piece::Intel(update), update.bytes()))
        };
        match piece {
            Ok((piece, bytes)) => {
                rest = &rest[bytes.len()..];
                Some(Ok(piece))
            }
            Err(err) => {
                rest = &[];
                Some(Err(err))
            }
        }
    })
}

/// The Intel updates in a file, in order. AMD containers in the file are
/// skipped. Iteration stops after the first piece that fails its checks,
/// which is returned as an error.
pub fn updates(file: &[u8]) -> impl Iterator<Item = Result<Update<'_>, Error>> {
    pieces(file).filter_map(|piece| match piece {
        Ok(Piece::Intel(update)) => Some(Ok(update)),
        Ok(Piece::Amd(_)) => None,
        Err(err) => Some(Err(err)),
    })
}

/// The newest update in `file` for `cpu`.
pub fn find(file: &[u8], cpu: CpuId) -> Option<Update<'_>> {
    updates(file).filter_map(Result::ok).filter(|u| u.matches(cpu)).max_by_key(Update::revision)
}

/// Builds update files for tests, here and in `ob`'s boot tests.
pub mod build {
    /// Encodes an update with the given header fields, `data_words` words of
    /// data and the extra signatures, with correct checksums, into `out`.
    /// Returns its length.
    pub fn update(
        revision: u32,
        signature: u32,
        platforms: u32,
        data_words: usize,
        extended: &[(u32, u32)],
        out: &mut [u8],
    ) -> usize {
        let data_size = data_words * 4;
        let ext_len = if extended.is_empty() { 0 } else { 20 + 12 * extended.len() };
        // Real updates are padded to a multiple of 1 KiB, which keeps the
        // data of every update in a file 16-byte aligned.
        let total = (super::HEADER + data_size + ext_len).next_multiple_of(1024);
        let out = &mut out[..total];
        out.fill(0);
        let mut put = |at: usize, value: u32| out[at..at + 4].copy_from_slice(&value.to_le_bytes());
        put(0, 1);
        put(4, revision);
        put(8, 0x0715_2019);
        put(12, signature);
        put(20, 1);
        put(24, platforms);
        put(28, data_size as u32);
        put(32, total as u32);
        for word in 0..data_words {
            put(super::HEADER + word * 4, 0x0101_0101u32.wrapping_mul(word as u32 + 1));
        }
        let sum = super::word_sum(&out[..super::HEADER + data_size]);
        let checksum = 0u32.wrapping_sub(sum);
        out[16..20].copy_from_slice(&checksum.to_le_bytes());
        if !extended.is_empty() {
            let main = signature.wrapping_add(platforms).wrapping_add(checksum);
            let table = super::HEADER + data_size;
            out[table..table + 4].copy_from_slice(&(extended.len() as u32).to_le_bytes());
            for (i, &(sig, pf)) in extended.iter().enumerate() {
                let at = table + 20 + i * 12;
                out[at..at + 4].copy_from_slice(&sig.to_le_bytes());
                out[at + 4..at + 8].copy_from_slice(&pf.to_le_bytes());
                let entry_sum = main.wrapping_sub(sig).wrapping_sub(pf);
                out[at + 8..at + 12].copy_from_slice(&entry_sum.to_le_bytes());
            }
            let ext_sum = super::word_sum(&out[table..table + ext_len]);
            out[table + 4..table + 8].copy_from_slice(&0u32.wrapping_sub(ext_sum).to_le_bytes());
        }
        total
    }

    /// Encodes an AMD container into `out` and returns its length. The
    /// equivalence table maps each (signature, equivalence ID), and each
    /// patch is (revision, equivalence ID, size in bytes, at least the
    /// 64-byte header).
    pub fn amd_container(
        equivalence: &[(u32, u16)],
        patches: &[(u32, u16, usize)],
        out: &mut [u8],
    ) -> usize {
        let mut at = 0;
        fn put(out: &mut [u8], at: &mut usize, bytes: &[u8]) {
            out[*at..*at + bytes.len()].copy_from_slice(bytes);
            *at += bytes.len();
        }
        put(out, &mut at, &super::amd::MAGIC.to_le_bytes());
        put(out, &mut at, &0u32.to_le_bytes());
        put(out, &mut at, &(16 * (equivalence.len() as u32 + 1)).to_le_bytes());
        for &(signature, id) in equivalence {
            put(out, &mut at, &signature.to_le_bytes());
            put(out, &mut at, &[0; 8]);
            put(out, &mut at, &id.to_le_bytes());
            put(out, &mut at, &[0; 2]);
        }
        // The table ends with an empty entry.
        put(out, &mut at, &[0; 16]);
        for &(revision, id, size) in patches {
            put(out, &mut at, &1u32.to_le_bytes());
            put(out, &mut at, &(size as u32).to_le_bytes());
            put(out, &mut at, &0x0415_2024u32.to_le_bytes());
            put(out, &mut at, &revision.to_le_bytes());
            put(out, &mut at, &[0; 16]);
            put(out, &mut at, &id.to_le_bytes());
            let rest = size - 26;
            out[at..at + rest].fill(0);
            // Patch data after the header, which differs between patches.
            out[at + 38..at + rest].fill(revision as u8);
            at += rest;
        }
        at
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Revision, signature, platforms and extra signatures of an update.
    type Spec<'a> = (u32, u32, u32, &'a [(u32, u32)]);

    fn file(updates: &[Spec<'_>]) -> Vec<u8> {
        let mut out = Vec::new();
        for &(revision, signature, platforms, extended) in updates {
            let mut buf = vec![0; 8192];
            let len = build::update(revision, signature, platforms, 16, extended, &mut buf);
            out.extend_from_slice(&buf[..len]);
        }
        out
    }

    const KABY_LAKE_R: CpuId = CpuId { signature: 0x806ea, platform: 1 << 7 };

    #[test]
    fn finds_the_newest_matching_update() {
        let bytes = file(&[
            (0x96, 0x806ea, 0xc0, &[]),
            (0xf4, 0x806ea, 0xc0, &[]),
            (0xff, 0x806ea, 0x01, &[]),
            (0x100, 0x906e9, 0x2a, &[]),
        ]);
        assert_eq!(updates(&bytes).count(), 4);
        let update = find(&bytes, KABY_LAKE_R).unwrap();
        assert_eq!(update.revision(), 0xf4);
        assert_eq!(update.date(), (2019, 7, 15));
        assert_eq!(update.data().len(), 64);
        assert!(find(&bytes, CpuId { signature: 0x806ec, platform: 0x80 }).is_none());
    }

    #[test]
    fn extended_signatures_match_too() {
        let bytes = file(&[(0x10, 0x506e3, 0x36, &[(0x806ea, 0x80), (0x806eb, 0xd0)])]);
        let update = find(&bytes, KABY_LAKE_R).unwrap();
        assert_eq!(update.targets().count(), 3);
        assert_eq!(bytes.len() % 1024, 0);
    }

    #[test]
    fn rejects_corruption() {
        let mut bytes = file(&[(1, 0x806ea, 0x80, &[])]);
        bytes[HEADER + 3] ^= 1;
        assert_eq!(updates(&bytes).next().unwrap().unwrap_err(), Error::BadChecksum);

        let mut bytes = file(&[(1, 0x806ea, 0x80, &[(0x806eb, 0x80)])]);
        let last = bytes.len() - 1024 + HEADER + 64 + 20 + 8;
        bytes[last] ^= 1;
        assert_eq!(Update::parse(&bytes).unwrap_err(), Error::BadChecksum);

        let mut bytes = file(&[(1, 0x806ea, 0x80, &[])]);
        bytes[0] = 2;
        assert_eq!(Update::parse(&bytes).unwrap_err(), Error::UnknownVersion);
        let bytes = file(&[(1, 0x806ea, 0x80, &[])]);
        assert_eq!(Update::parse(&bytes[..100]).unwrap_err(), Error::BadSize);
    }

    #[test]
    fn stops_at_the_first_bad_update() {
        let mut bytes = file(&[(1, 0x806ea, 0x80, &[]), (2, 0x806ea, 0x80, &[])]);
        bytes.extend_from_slice(&[0xff; 64]);
        let results: Vec<_> = updates(&bytes).collect();
        assert_eq!(results.len(), 3);
        assert!(results[2].is_err());
        assert_eq!(find(&bytes, KABY_LAKE_R).unwrap().revision(), 2);
    }

    /// Family 17h, model 71h, stepping 0, and family 19h, model 1, stepping 1.
    const ZEN_2: u32 = 0x00870f10;
    const ZEN_3: u32 = 0x00a00f11;

    fn amd_file(equivalence: &[(u32, u16)], patches: &[(u32, u16, usize)]) -> Vec<u8> {
        let mut buf = vec![0; 40_000];
        let len = build::amd_container(equivalence, patches, &mut buf);
        buf.truncate(len);
        buf
    }

    #[test]
    fn an_amd_patch_is_found_by_the_equivalence_id_of_the_signature() {
        let bytes = amd_file(
            &[(ZEN_2, 0x8870), (ZEN_3, 0xa010)],
            &[(0x10, 0xa010, 1024), (0x20, 0x8870, 2048)],
        );
        let container = amd::Container::parse(&bytes).unwrap();
        assert_eq!(container.bytes().len(), bytes.len());
        assert_eq!(container.patches().count(), 2);
        let patch = amd::find(&bytes, ZEN_2).unwrap();
        assert_eq!((patch.revision(), patch.equivalence_id()), (0x20, 0x8870));
        assert_eq!(patch.bytes().len(), 2048);
        assert_eq!(patch.date(), (2024, 4, 15));
        assert_eq!(amd::find(&bytes, ZEN_3).unwrap().revision(), 0x10);
        assert!(amd::find(&bytes, 0x00800f12).is_none());
    }

    #[test]
    fn the_highest_amd_revision_wins() {
        let bytes = amd_file(
            &[(ZEN_3, 0xa010)],
            &[(0x30, 0xa010, 1024), (0x50, 0xa010, 1024), (0x40, 0xa010, 1024)],
        );
        assert_eq!(amd::find(&bytes, ZEN_3).unwrap().revision(), 0x50);
    }

    #[test]
    fn the_highest_amd_revision_wins_across_containers() {
        let mut bytes = amd_file(&[(ZEN_3, 0xa010)], &[(0x30, 0xa010, 1024)]);
        bytes.extend(amd_file(&[(ZEN_3, 0xa010)], &[(0x60, 0xa010, 1024)]));
        assert_eq!(amd::containers(&bytes).count(), 2);
        assert_eq!(amd::find(&bytes, ZEN_3).unwrap().revision(), 0x60);
    }

    #[test]
    fn an_equivalence_table_ends_at_its_first_empty_entry() {
        let mut bytes = amd_file(&[(ZEN_2, 0x8870), (ZEN_3, 0xa010)], &[(0x10, 0xa010, 1024)]);
        // The first entry becomes empty, so the second is not read.
        bytes[12..16].fill(0);
        let container = amd::Container::parse(&bytes).unwrap();
        assert_eq!(container.equivalence_id(ZEN_3), None);
    }

    #[test]
    fn a_damaged_amd_container_is_refused() {
        let good = amd_file(&[(ZEN_3, 0xa010)], &[(0x10, 0xa010, 1024)]);
        let table_size =
            |bytes: &mut Vec<u8>, size: u32| bytes[8..12].copy_from_slice(&size.to_le_bytes());

        let truncated = &good[..good.len() - 1];
        assert_eq!(amd::Container::parse(truncated).unwrap_err(), Error::BadSize);

        let mut bytes = good.clone();
        table_size(&mut bytes, 24);
        assert_eq!(amd::Container::parse(&bytes).unwrap_err(), Error::BadSize);
        table_size(&mut bytes, 0);
        assert_eq!(amd::Container::parse(&bytes).unwrap_err(), Error::BadSize);
        table_size(&mut bytes, 0x7fff_fff0);
        assert_eq!(amd::Container::parse(&bytes).unwrap_err(), Error::BadSize);

        let mut bytes = good.clone();
        bytes[4] = 7;
        assert_eq!(amd::Container::parse(&bytes).unwrap_err(), Error::UnknownSection);

        // The patch section follows the table's 12 + 32 bytes.
        let mut bytes = good.clone();
        bytes[44] = 9;
        assert_eq!(amd::Container::parse(&bytes).unwrap_err(), Error::UnknownSection);

        let small = amd_file(&[(ZEN_3, 0xa010)], &[(0x10, 0xa010, 64)]);
        let mut bytes = small.clone();
        bytes[48..52].copy_from_slice(&(amd::PATCH_HEADER as u32 - 4).to_le_bytes());
        assert_eq!(amd::Container::parse(&bytes).unwrap_err(), Error::BadSize);

        let mut bytes = amd_file(&[(ZEN_3, 0xa010)], &[(0x10, 0xa010, amd::PATCH_MAX + 1)]);
        assert_eq!(amd::Container::parse(&bytes).unwrap_err(), Error::BadSize);
        bytes.clear();
        assert_eq!(amd::Container::parse(&bytes).unwrap_err(), Error::UnknownSection);
    }

    #[test]
    fn a_container_that_follows_with_the_wrong_magic_is_refused() {
        let mut bytes = amd_file(&[(ZEN_3, 0xa010)], &[(0x10, 0xa010, 1024)]);
        let second = bytes.len();
        bytes.extend(amd_file(&[(ZEN_3, 0xa010)], &[(0x20, 0xa010, 1024)]));
        bytes[second] ^= 1;
        // The word where the second container should start is read as a
        // section of the first, so the file is refused from there on.
        let results: Vec<_> = pieces(&bytes).collect();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].as_ref().unwrap_err(), &Error::UnknownSection);
    }

    #[test]
    fn a_patch_larger_than_the_family_takes_is_not_chosen() {
        let bytes = amd_file(&[(ZEN_2, 0x8870)], &[(0x10, 0x8870, 3200), (0x20, 0x8870, 3201)]);
        assert_eq!(amd::find(&bytes, ZEN_2).unwrap().revision(), 0x10);
        assert_eq!(amd::patch_limit(ZEN_2), 3200);
        assert_eq!(amd::patch_limit(ZEN_3), 5568);
        assert_eq!(amd::patch_limit(0x00600f12), 4096);
        assert_eq!(amd::patch_limit(0x00700f01), 3458);
        assert_eq!(amd::patch_limit(0x00b00f21), amd::PATCH_MAX);
    }

    #[test]
    fn a_mixed_file_splits_into_pieces_and_intel_still_finds_its_update() {
        let mut bytes = file(&[(0x96, 0x806ea, 0xc0, &[])]);
        bytes.extend(amd_file(&[(ZEN_3, 0xa010)], &[(0x10, 0xa010, 1000), (0x20, 0xa010, 1001)]));
        bytes.extend(file(&[(0xf4, 0x806ea, 0xc0, &[])]));
        let kinds: Vec<_> = pieces(&bytes)
            .map(|piece| match piece.unwrap() {
                Piece::Intel(_) => "intel",
                Piece::Amd(_) => "amd",
            })
            .collect();
        assert_eq!(kinds, ["intel", "amd", "intel"]);
        assert_eq!(updates(&bytes).count(), 2);
        assert_eq!(find(&bytes, KABY_LAKE_R).unwrap().revision(), 0xf4);
        assert_eq!(amd::containers(&bytes).count(), 1);
        assert_eq!(amd::find(&bytes, ZEN_3).unwrap().revision(), 0x20);
    }

    #[test]
    fn an_amd_only_file_has_no_intel_updates() {
        let bytes = amd_file(&[(ZEN_3, 0xa010)], &[(0x10, 0xa010, 1024)]);
        assert_eq!(updates(&bytes).count(), 0);
        assert!(find(&bytes, KABY_LAKE_R).is_none());
    }

    /// Parses every file of the directory in `AMD_UCODE_DIR`, such as
    /// linux-firmware's `amd-ucode`, and checks that each patch is within
    /// the limit of its family.
    #[test]
    #[ignore = "needs AMD_UCODE_DIR, a directory of linux-firmware's amd-ucode files"]
    fn the_amd_files_of_linux_firmware_parse() {
        let dir = std::env::var("AMD_UCODE_DIR").unwrap();
        let mut files = 0;
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.extension().is_none_or(|e| e != "bin") {
                continue;
            }
            let bytes = std::fs::read(&path).unwrap();
            let mut patches = 0;
            let mut largest = 0;
            for container in amd::containers(&bytes) {
                let container = container.unwrap();
                for patch in container.patches() {
                    patches += 1;
                    largest = largest.max(patch.bytes().len());
                    assert!(patch.bytes().len() <= amd::PATCH_MAX);
                }
            }
            assert!(patches > 0, "{} holds no patch", path.display());
            println!("{}: {patches} patches, largest {largest} bytes", path.display());
            files += 1;
        }
        assert!(files > 0);
    }
}
