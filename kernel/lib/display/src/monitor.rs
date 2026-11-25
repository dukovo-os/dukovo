// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! What a monitor says of itself.
//!
//! A monitor describes itself to the card it is plugged into with a few
//! blocks of 128 bytes (VESA's Enhanced Extended Display Identification
//! Data, EDID 1.4): who made it, what it is called, how large its
//! picture is, the modes it shows and the range of rates it follows. A
//! block of extension (CTA-861) may follow with more modes and with what
//! the monitor does with high dynamic range. A card's driver fetches the
//! bytes; [`Monitor::read`] reads what the layer tells programs of the
//! screen, and [`modes`] lists the modes.
//!
//! The bytes come from outside the machine, from a monitor, a cable or a
//! host, and are wrong often enough: every block is read as a fixed 128
//! bytes that summed to zero, every place inside it is a constant or
//! checked, and a part that contradicts itself is left out and the rest
//! is still read.

use radix_abi::display::{MODE_PREFERRED, Mode};

/// The bytes of one block of a monitor's data.
pub const BLOCK: usize = 128;

/// The most bytes of a monitor's data the layer keeps: the base block
/// and three of extension.
pub const DATA_MAX: usize = 4 * BLOCK;

/// The bytes every base block starts with.
const HEADER: [u8; 8] = [0x00, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x00];

/// Where the four 18-byte descriptors of the base block are.
const DESCRIPTORS: [usize; 4] = [54, 72, 90, 108];

/// The bytes of a descriptor.
const DESCRIPTOR: usize = 18;

/// The tag of a descriptor that holds the monitor's name.
const TAG_NAME: u8 = 0xfc;
/// The tag of a descriptor that holds the range of rates it follows.
const TAG_RANGE: u8 = 0xfd;

/// The tag of a CTA-861 block of extension.
const EXTENSION_CTA: u8 = 0x02;
/// A CTA data block whose first byte says what it is.
const CTA_EXTENDED: u8 = 7;
/// The extended tag of the high dynamic range block (CTA-861.3).
const CTA_HDR: u8 = 0x06;
/// The bits of that block's transfer functions that are high dynamic
/// range: SMPTE ST 2084 and hybrid log-gamma.
const HDR_FUNCTIONS: u8 = 0b1100;

/// The modes of the three established timing bytes, from bit 7 of the
/// first: width, height and frames a second. The one interlaced mode
/// among them has no size here and is left out.
const ESTABLISHED: [(u32, u32, u32); 17] = [
    (720, 400, 70),
    (720, 400, 88),
    (640, 480, 60),
    (640, 480, 67),
    (640, 480, 72),
    (640, 480, 75),
    (800, 600, 56),
    (800, 600, 60),
    (800, 600, 72),
    (800, 600, 75),
    (832, 624, 75),
    (0, 0, 0),
    (1024, 768, 60),
    (1024, 768, 70),
    (1024, 768, 75),
    (1280, 1024, 75),
    (1152, 870, 75),
];

/// Two to the power of each 32nd from 0 to 31, times 65536: the steps
/// the high dynamic range block counts brightness in.
const ROOTS: [u32; 32] = [
    65536, 66971, 68438, 69936, 71468, 73032, 74632, 76266, 77936, 79642, 81386, 83169, 84990,
    86851, 88752, 90696, 92682, 94711, 96785, 98905, 101070, 103283, 105545, 107856, 110218,
    112631, 115098, 117618, 120194, 122825, 125515, 128263,
];

/// Why bytes are not a monitor's data.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fault {
    /// There are fewer than a block of them.
    Short,
    /// They do not start as a base block does.
    Header,
    /// The base block does not sum to zero.
    Checksum,
}

/// What a monitor says of itself, apart from its modes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Monitor {
    /// Its maker: three capital letters and a zero.
    pub maker: [u8; 4],
    /// Its model number.
    pub product: u32,
    /// Its serial number, or 0.
    pub serial: u32,
    /// Its name, padded with zeros; all zeros if it gives none.
    pub name: [u8; 16],
    /// The width of its picture in millimetres, or 0 if it does not say.
    pub width_mm: u32,
    /// The height, or 0.
    pub height_mm: u32,
    /// The bits a colour it takes, or 0 if it does not say.
    pub depth: u8,
    /// The slowest rate it follows, in frames in 1000 seconds, or 0.
    pub refresh_min_mhz: u32,
    /// The fastest, or 0.
    pub refresh_max_mhz: u32,
    /// Whether it takes high dynamic range.
    pub hdr: bool,
    /// The brightest it shows, in candelas a square metre, or 0 if it
    /// does not say.
    pub luminance_max: u32,
}

impl Monitor {
    /// Reads what `data` says of the monitor.
    ///
    /// # Errors
    ///
    /// A [`Fault`] if the base block is not there or not sound. A block
    /// of extension that is not sound is left out.
    pub fn read(data: &[u8]) -> Result<Monitor, Fault> {
        let base = base(data)?;
        let mut monitor = Monitor {
            maker: maker(base[8], base[9]),
            product: u32::from(u16::from_le_bytes([base[10], base[11]])),
            serial: u32::from_le_bytes([base[12], base[13], base[14], base[15]]),
            ..Monitor::default()
        };
        let (version, revision, input) = (base[18], base[19], base[20]);
        if version == 1 && revision >= 4 && input & 0x80 != 0 {
            // A digital input of EDID 1.4 says its depth in bits 4 to 6.
            monitor.depth = match (input >> 4) & 7 {
                depth @ 1..=6 => 4 + 2 * depth,
                _ => 0,
            };
        }
        if base[21] != 0 && base[22] != 0 {
            monitor.width_mm = u32::from(base[21]) * 10;
            monitor.height_mm = u32::from(base[22]) * 10;
        }
        for (index, at) in DESCRIPTORS.into_iter().enumerate() {
            let Some(descriptor) = descriptor(base, at) else { continue };
            if descriptor[0] != 0 || descriptor[1] != 0 {
                // A timing. The first says the picture's size to the
                // millimetre, where the block above has centimetres.
                let width = nibbled(descriptor[12], descriptor[14] >> 4);
                let height = nibbled(descriptor[13], descriptor[14] & 0xf);
                if index == 0 && width != 0 && height != 0 {
                    (monitor.width_mm, monitor.height_mm) = (width, height);
                }
                continue;
            }
            match descriptor[3] {
                TAG_NAME => monitor.name = name(descriptor),
                TAG_RANGE => {
                    if let Some((min, max)) = range(descriptor) {
                        (monitor.refresh_min_mhz, monitor.refresh_max_mhz) = (min, max);
                    }
                }
                _ => {}
            }
        }
        for extension in extensions(data, base) {
            for (tag, payload) in DataBlocks::of(extension) {
                // The block's bytes after its extended tag: the transfer
                // functions, the kinds of metadata, then the brightest
                // picture the monitor wants, if it says.
                if tag == CTA_EXTENDED && payload.first() == Some(&CTA_HDR) {
                    monitor.hdr = payload.get(1).is_some_and(|&bits| bits & HDR_FUNCTIONS != 0);
                    monitor.luminance_max = payload.get(3).map_or(0, |&coded| luminance(coded));
                }
            }
        }
        Ok(monitor)
    }
}

/// Calls `found` for each mode `data` lists: the timings of the base
/// block, the first of them marked MODE_PREFERRED, then those of each
/// block of extension, then the standard and the established timings,
/// which say a size and a rate and no more. A mode may be listed twice.
/// Bytes that are not a monitor's data list none.
pub fn modes(data: &[u8], found: &mut dyn FnMut(Mode)) {
    let Ok(base) = base(data) else { return };
    for (index, at) in DESCRIPTORS.into_iter().enumerate() {
        if let Some(mode) = descriptor(base, at).and_then(timing) {
            let flags = if index == 0 { MODE_PREFERRED } else { 0 };
            found(Mode { flags, ..mode });
        }
    }
    for extension in extensions(data, base) {
        // The timings fill the block from where its data blocks end, up
        // to the checksum.
        let first = usize::from(extension[2]);
        if first < 4 {
            continue;
        }
        for at in (first..BLOCK - DESCRIPTOR).step_by(DESCRIPTOR) {
            match descriptor(extension, at).and_then(timing) {
                Some(mode) => found(mode),
                None => break,
            }
        }
    }
    for &[first, second] in base[38..54].as_chunks::<2>().0 {
        if let Some(mode) = standard(first, second) {
            found(mode);
        }
    }
    let bits = u32::from_be_bytes([0, base[35], base[36], base[37]]);
    for (index, &(width, height, hertz)) in ESTABLISHED.iter().enumerate() {
        if bits & (1 << (23 - index)) != 0 && width != 0 {
            found(sized(width, height, hertz * 1000));
        }
    }
}

/// The brightness the high dynamic range block codes as `coded`, in
/// candelas a square metre: 50 times two to the power of `coded` 32nds.
/// A code of 0 says nothing.
pub fn luminance(coded: u8) -> u32 {
    if coded == 0 {
        return 0;
    }
    let (whole, part) = (u32::from(coded) / 32, usize::from(coded) % 32);
    (((50u64 << whole) * u64::from(ROOTS[part]) + (1 << 15)) >> 16) as u32
}

/// Block `index` of `data`, if all of it is there.
fn block(data: &[u8], index: usize) -> Option<&[u8; BLOCK]> {
    let start = index.checked_mul(BLOCK)?;
    data.get(start..start.checked_add(BLOCK)?)?.try_into().ok()
}

/// Whether a block's bytes sum to zero, as the last of them makes them.
fn sums_to_zero(block: &[u8; BLOCK]) -> bool {
    block.iter().fold(0u8, |sum, &byte| sum.wrapping_add(byte)) == 0
}

/// The base block of `data`, if it is one and is sound.
fn base(data: &[u8]) -> Result<&[u8; BLOCK], Fault> {
    let base = block(data, 0).ok_or(Fault::Short)?;
    if base[..8] != HEADER {
        return Err(Fault::Header);
    }
    if !sums_to_zero(base) {
        return Err(Fault::Checksum);
    }
    Ok(base)
}

/// The CTA blocks of extension that follow `base` in `data` and are
/// sound: as many as the base block counts, as far as `data` goes and no
/// further than [`DATA_MAX`].
fn extensions<'a>(data: &'a [u8], base: &[u8; BLOCK]) -> impl Iterator<Item = &'a [u8; BLOCK]> {
    let counted = usize::from(base[126]).min(DATA_MAX / BLOCK - 1);
    (1..=counted)
        .filter_map(|index| block(data, index))
        .filter(|block| block[0] == EXTENSION_CTA && sums_to_zero(block))
}

/// The 18 bytes of a descriptor at `at` in `block`.
fn descriptor(block: &[u8; BLOCK], at: usize) -> Option<&[u8; DESCRIPTOR]> {
    block.get(at..at.checked_add(DESCRIPTOR)?)?.try_into().ok()
}

/// A number of 8 low bits and a few high ones from another byte.
fn nibbled(low: u8, high: u8) -> u32 {
    u32::from(low) | u32::from(high) << 8
}

/// The maker's three letters, which two bytes pack 5 bits each with 1
/// for A. A code that is no letter reads as a question mark.
fn maker(high: u8, low: u8) -> [u8; 4] {
    let packed = u16::from_be_bytes([high, low]);
    let letter = |shift: u32| match (packed >> shift) & 0x1f {
        code @ 1..=26 => b'A' + code as u8 - 1,
        _ => b'?',
    };
    [letter(10), letter(5), letter(0), 0]
}

/// The name a name descriptor holds: up to 13 characters, ended by a
/// line feed and padded with spaces. A byte that is not printable ASCII
/// reads as a question mark.
fn name(descriptor: &[u8; DESCRIPTOR]) -> [u8; 16] {
    let text = &descriptor[5..];
    let text = text.split(|&byte| byte == b'\n').next().unwrap_or(text);
    let text = text.trim_ascii_end();
    let mut name = [0; 16];
    for (out, &byte) in name.iter_mut().zip(text) {
        *out = if byte.is_ascii_graphic() || byte == b' ' { byte } else { b'?' };
    }
    name
}

/// The slowest and fastest rates a range descriptor gives, in frames in
/// 1000 seconds. `None` if they are not a range.
fn range(descriptor: &[u8; DESCRIPTOR]) -> Option<(u32, u32)> {
    // EDID 1.4 adds 255 to rates that do not fit a byte: bit 1 of the
    // flags for the fastest, and bit 0 with it for the slowest too.
    let flags = descriptor[4] & 3;
    let min = u32::from(descriptor[5]) + if flags == 3 { 255 } else { 0 };
    let max = u32::from(descriptor[6]) + if flags & 2 != 0 { 255 } else { 0 };
    (min != 0 && min <= max).then_some((min * 1000, max * 1000))
}

/// The mode a detailed timing descriptor describes. `None` for a
/// descriptor that is no timing, for an interlaced mode, and for a
/// timing that contradicts itself: no size, a sync pulse outside the
/// blanking, or a rate that rounds to nothing.
fn timing(descriptor: &[u8; DESCRIPTOR]) -> Option<Mode> {
    let d = descriptor;
    let clock = u32::from(u16::from_le_bytes([d[0], d[1]]));
    let (width, h_blank) = (nibbled(d[2], d[4] >> 4), nibbled(d[3], d[4] & 0xf));
    let (height, v_blank) = (nibbled(d[5], d[7] >> 4), nibbled(d[6], d[7] & 0xf));
    let (h_front, h_sync) = (nibbled(d[8], d[11] >> 6), nibbled(d[9], (d[11] >> 4) & 3));
    let v_front = u32::from(d[10] >> 4) | u32::from((d[11] >> 2) & 3) << 4;
    let v_sync = u32::from(d[10] & 0xf) | u32::from(d[11] & 3) << 4;
    let interlaced = d[17] & 0x80 != 0;
    if clock == 0 || width == 0 || height == 0 || interlaced {
        return None;
    }
    if h_front + h_sync > h_blank || v_front + v_sync > v_blank {
        return None;
    }
    let (h_total, v_total) = (width + h_blank, height + v_blank);
    // The clock is in units of 10 kHz, and the rate in frames in 1000
    // seconds.
    let pixels = u64::from(h_total) * u64::from(v_total);
    let refresh_mhz = (u64::from(clock) * 10_000_000 + pixels / 2) / pixels;
    if refresh_mhz == 0 {
        return None;
    }
    // No sum here passes 13 bits: a size or a blanking has 12, and a
    // sync pulse lies inside the blanking.
    Some(Mode {
        width,
        height,
        refresh_mhz: refresh_mhz as u32,
        flags: 0,
        clock_khz: clock * 10,
        h_sync_start: (width + h_front) as u16,
        h_sync_end: (width + h_front + h_sync) as u16,
        h_total: h_total as u16,
        v_sync_start: (height + v_front) as u16,
        v_sync_end: (height + v_front + v_sync) as u16,
        v_total: v_total as u16,
    })
}

/// A mode of a size and a rate, with no timings.
fn sized(width: u32, height: u32, refresh_mhz: u32) -> Mode {
    Mode { width, height, refresh_mhz, ..Mode::default() }
}

/// The mode a standard timing's two bytes say: the width in eighths
/// less 31, then the shape in the top two bits and the rate over 60 in
/// the rest. `None` for a pair that is unused.
fn standard(first: u8, second: u8) -> Option<Mode> {
    if first == 0 || (first, second) == (1, 1) {
        return None;
    }
    let width = (u32::from(first) + 31) * 8;
    let height = match second >> 6 {
        0 => width * 10 / 16,
        1 => width * 3 / 4,
        2 => width * 4 / 5,
        _ => width * 9 / 16,
    };
    Some(sized(width, height, (u32::from(second & 0x3f) + 60) * 1000))
}

/// The data blocks of a CTA block of extension: each its tag and the
/// bytes after its first.
struct DataBlocks<'a> {
    /// The bytes the blocks fill.
    bytes: &'a [u8],
}

impl<'a> DataBlocks<'a> {
    /// The data blocks of `extension`, which lie from its byte 4 to
    /// where its timings start.
    fn of(extension: &'a [u8; BLOCK]) -> DataBlocks<'a> {
        let end = usize::from(extension[2]).min(BLOCK - 1);
        DataBlocks { bytes: extension.get(4..end).unwrap_or(&[]) }
    }
}

impl<'a> Iterator for DataBlocks<'a> {
    type Item = (u8, &'a [u8]);

    fn next(&mut self) -> Option<(u8, &'a [u8])> {
        let (&head, rest) = self.bytes.split_first()?;
        // A block that says it is longer than what is left ends the
        // list.
        let (payload, rest) = rest.split_at_checked(usize::from(head & 0x1f))?;
        self.bytes = rest;
        Some((head >> 5, payload))
    }
}

#[cfg(test)]
mod tests;

#[cfg(kani)]
mod proofs {
    use super::*;

    /// Whatever 18 bytes a monitor gives as a timing, reading them never
    /// panics, and a mode that comes of them has a size, a rate, and
    /// sync pulses inside its blanking.
    #[kani::proof]
    fn a_timing_is_a_mode_or_nothing() {
        let bytes: [u8; DESCRIPTOR] = kani::any();
        if let Some(mode) = timing(&bytes) {
            assert!(mode.width != 0 && mode.height != 0 && mode.refresh_mhz != 0);
            assert!(u32::from(mode.h_sync_start) >= mode.width);
            assert!(mode.h_sync_start <= mode.h_sync_end && mode.h_sync_end <= mode.h_total);
            assert!(u32::from(mode.v_sync_start) >= mode.height);
            assert!(mode.v_sync_start <= mode.v_sync_end && mode.v_sync_end <= mode.v_total);
        }
    }

    /// Every code of brightness is read without a wrap.
    #[kani::proof]
    fn every_brightness_code_is_read() {
        let coded: u8 = kani::any();
        let brightness = luminance(coded);
        assert!(brightness <= 12_526);
        assert!(coded == 0 || brightness >= 50);
    }
}
