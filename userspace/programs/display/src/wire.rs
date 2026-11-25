// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The records of radix-display(2), as a program writes and reads them.
//!
//! The writers lay out what a request's `buffer` starts with, as the
//! interface's offsets say. The readers take what the kernel wrote back
//! and return `None` for bytes that are too few, so that nothing here
//! panics whatever a `buffer` holds. The kernel's side of each record is
//! `radix_display::wire`, and the tests check the two against each other.

use radix_abi::display::{
    BLOB_MODE, Blob, Buffer, CardInfo, Change, DisplayEvent, ENTRIES_MAX, Entry, Get, Mode,
    PUT_RECTS_MAX, Picture, PipeInfo, PlaneInfo, Put, PutRect, ScreenInfo, Thing,
};

fn le16(bytes: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(bytes.get(at..at.checked_add(2)?)?.try_into().ok()?))
}

fn le32(bytes: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(bytes.get(at..at.checked_add(4)?)?.try_into().ok()?))
}

fn le64(bytes: &[u8], at: usize) -> Option<u64> {
    Some(u64::from_le_bytes(bytes.get(at..at.checked_add(8)?)?.try_into().ok()?))
}

fn array<const N: usize>(bytes: &[u8], at: usize) -> Option<[u8; N]> {
    bytes.get(at..at.checked_add(N)?)?.try_into().ok()
}

fn put16(out: &mut [u8], at: usize, value: u16) {
    out[at..at + 2].copy_from_slice(&value.to_le_bytes());
}

fn put32(out: &mut [u8], at: usize, value: u32) {
    out[at..at + 4].copy_from_slice(&value.to_le_bytes());
}

fn put64(out: &mut [u8], at: usize, value: u64) {
    out[at..at + 8].copy_from_slice(&value.to_le_bytes());
}

/// The Thing structures LIST wrote into `bytes`: every whole one.
pub fn things(bytes: &[u8]) -> impl Iterator<Item = Thing> + '_ {
    bytes.as_chunks::<{ Thing::SIZE }>().0.iter().filter_map(|thing| {
        Some(Thing {
            id: le32(thing, Thing::OFFSET_ID)?,
            kind: le32(thing, Thing::OFFSET_KIND)?,
            card: le32(thing, Thing::OFFSET_CARD)?,
            index: le32(thing, Thing::OFFSET_INDEX)?,
        })
    })
}

/// What GET reads: `part` of `thing`.
pub fn get(thing: u32, part: u32) -> [u8; Get::SIZE] {
    let mut out = [0; Get::SIZE];
    put32(&mut out, Get::OFFSET_THING, thing);
    put32(&mut out, Get::OFFSET_PART, part);
    out
}

/// A card's PART_INFO.
pub fn card(bytes: &[u8]) -> Option<CardInfo> {
    Some(CardInfo {
        id: le32(bytes, CardInfo::OFFSET_ID)?,
        kind: le32(bytes, CardInfo::OFFSET_KIND)?,
        flags: le32(bytes, CardInfo::OFFSET_FLAGS)?,
        screens: le32(bytes, CardInfo::OFFSET_SCREENS)?,
        pipes: le32(bytes, CardInfo::OFFSET_PIPES)?,
        planes: le32(bytes, CardInfo::OFFSET_PLANES)?,
        width_max: le32(bytes, CardInfo::OFFSET_WIDTH_MAX)?,
        height_max: le32(bytes, CardInfo::OFFSET_HEIGHT_MAX)?,
        name: array(bytes, CardInfo::OFFSET_NAME)?,
        model: array(bytes, CardInfo::OFFSET_MODEL)?,
    })
}

/// A screen's PART_INFO.
pub fn screen(bytes: &[u8]) -> Option<ScreenInfo> {
    Some(ScreenInfo {
        id: le32(bytes, ScreenInfo::OFFSET_ID)?,
        card: le32(bytes, ScreenInfo::OFFSET_CARD)?,
        flags: le32(bytes, ScreenInfo::OFFSET_FLAGS)?,
        pipe: le32(bytes, ScreenInfo::OFFSET_PIPE)?,
        width_mm: le32(bytes, ScreenInfo::OFFSET_WIDTH_MM)?,
        height_mm: le32(bytes, ScreenInfo::OFFSET_HEIGHT_MM)?,
        modes: le32(bytes, ScreenInfo::OFFSET_MODES)?,
        monitor_data_bytes: le32(bytes, ScreenInfo::OFFSET_MONITOR_DATA_BYTES)?,
        refresh_min_mhz: le32(bytes, ScreenInfo::OFFSET_REFRESH_MIN_MHZ)?,
        refresh_max_mhz: le32(bytes, ScreenInfo::OFFSET_REFRESH_MAX_MHZ)?,
        luminance_max: le32(bytes, ScreenInfo::OFFSET_LUMINANCE_MAX)?,
        product: le32(bytes, ScreenInfo::OFFSET_PRODUCT)?,
        serial: le32(bytes, ScreenInfo::OFFSET_SERIAL)?,
        depth: *bytes.get(ScreenInfo::OFFSET_DEPTH)?,
        reserved: [0; 3],
        maker: array(bytes, ScreenInfo::OFFSET_MAKER)?,
        name: array(bytes, ScreenInfo::OFFSET_NAME)?,
    })
}

/// A Mode, as PART_MODES gives each and a pipe's PART_INFO holds one.
pub fn mode(bytes: &[u8]) -> Option<Mode> {
    Some(Mode {
        width: le32(bytes, Mode::OFFSET_WIDTH)?,
        height: le32(bytes, Mode::OFFSET_HEIGHT)?,
        refresh_mhz: le32(bytes, Mode::OFFSET_REFRESH_MHZ)?,
        flags: le32(bytes, Mode::OFFSET_FLAGS)?,
        clock_khz: le32(bytes, Mode::OFFSET_CLOCK_KHZ)?,
        h_sync_start: le16(bytes, Mode::OFFSET_H_SYNC_START)?,
        h_sync_end: le16(bytes, Mode::OFFSET_H_SYNC_END)?,
        h_total: le16(bytes, Mode::OFFSET_H_TOTAL)?,
        v_sync_start: le16(bytes, Mode::OFFSET_V_SYNC_START)?,
        v_sync_end: le16(bytes, Mode::OFFSET_V_SYNC_END)?,
        v_total: le16(bytes, Mode::OFFSET_V_TOTAL)?,
    })
}

/// The modes PART_MODES wrote into `bytes`: every whole one, the
/// preferred one first.
pub fn modes(bytes: &[u8]) -> impl Iterator<Item = Mode> + '_ {
    bytes.as_chunks::<{ Mode::SIZE }>().0.iter().filter_map(|bytes| mode(bytes))
}

/// A pipe's PART_INFO.
pub fn pipe(bytes: &[u8]) -> Option<PipeInfo> {
    Some(PipeInfo {
        id: le32(bytes, PipeInfo::OFFSET_ID)?,
        card: le32(bytes, PipeInfo::OFFSET_CARD)?,
        flags: le32(bytes, PipeInfo::OFFSET_FLAGS)?,
        color_table_entries: le32(bytes, PipeInfo::OFFSET_COLOR_TABLE_ENTRIES)?,
        beat_ns: le64(bytes, PipeInfo::OFFSET_BEAT_NS)?,
        beat_count: le64(bytes, PipeInfo::OFFSET_BEAT_COUNT)?,
        beat_time_ns: le64(bytes, PipeInfo::OFFSET_BEAT_TIME_NS)?,
        mode: mode(bytes.get(PipeInfo::OFFSET_MODE..)?)?,
    })
}

/// A plane's PART_INFO.
pub fn plane(bytes: &[u8]) -> Option<PlaneInfo> {
    let mut formats = [0; 8];
    for (index, format) in formats.iter_mut().enumerate() {
        *format = le32(bytes, PlaneInfo::OFFSET_FORMATS + index * 4)?;
    }
    Some(PlaneInfo {
        id: le32(bytes, PlaneInfo::OFFSET_ID)?,
        card: le32(bytes, PlaneInfo::OFFSET_CARD)?,
        kind: le32(bytes, PlaneInfo::OFFSET_KIND)?,
        flags: le32(bytes, PlaneInfo::OFFSET_FLAGS)?,
        pipe: le32(bytes, PlaneInfo::OFFSET_PIPE)?,
        fixed_pipe: le32(bytes, PlaneInfo::OFFSET_FIXED_PIPE)?,
        width_max: le32(bytes, PlaneInfo::OFFSET_WIDTH_MAX)?,
        height_max: le32(bytes, PlaneInfo::OFFSET_HEIGHT_MAX)?,
        formats,
    })
}

/// What MAKE_BUFFER reads.
pub fn buffer(buffer: &Buffer) -> [u8; Buffer::SIZE] {
    let mut out = [0; Buffer::SIZE];
    put32(&mut out, Buffer::OFFSET_FILE, buffer.file);
    put32(&mut out, Buffer::OFFSET_CARD, buffer.card);
    put32(&mut out, Buffer::OFFSET_FORMAT, buffer.format);
    put32(&mut out, Buffer::OFFSET_WIDTH, buffer.width);
    put32(&mut out, Buffer::OFFSET_HEIGHT, buffer.height);
    out
}

/// What MAKE_BUFFER wrote back: the bytes from one row to the next, and
/// the bytes of the picture.
pub fn buffer_made(bytes: &[u8]) -> Option<(u32, u64)> {
    Some((le32(bytes, Buffer::OFFSET_ROW_BYTES)?, le64(bytes, Buffer::OFFSET_BYTES)?))
}

/// What MAKE_PICTURE reads.
pub fn picture(picture: &Picture) -> [u8; Picture::SIZE] {
    let mut out = [0; Picture::SIZE];
    put32(&mut out, Picture::OFFSET_FILE, picture.file);
    put32(&mut out, Picture::OFFSET_CARD, picture.card);
    put32(&mut out, Picture::OFFSET_FLAGS, picture.flags);
    put32(&mut out, Picture::OFFSET_FORMAT, picture.format);
    put32(&mut out, Picture::OFFSET_WIDTH, picture.width);
    put32(&mut out, Picture::OFFSET_HEIGHT, picture.height);
    put32(&mut out, Picture::OFFSET_ROW_BYTES, picture.row_bytes);
    put64(&mut out, Picture::OFFSET_OFFSET, picture.offset);
    out
}

/// The picture's number, which MAKE_PICTURE wrote back.
pub fn picture_made(bytes: &[u8]) -> Option<u32> {
    le32(bytes, Picture::OFFSET_PICTURE)
}

/// What MAKE_BLOB reads for a mode: a Blob, then the Mode.
pub fn mode_blob(mode: &Mode) -> [u8; Blob::SIZE + Mode::SIZE] {
    let mut out = [0; Blob::SIZE + Mode::SIZE];
    put32(&mut out, Blob::OFFSET_KIND, BLOB_MODE);
    put32(&mut out, Blob::OFFSET_BYTES, Mode::SIZE as u32);
    let at = Blob::SIZE;
    put32(&mut out, at + Mode::OFFSET_WIDTH, mode.width);
    put32(&mut out, at + Mode::OFFSET_HEIGHT, mode.height);
    put32(&mut out, at + Mode::OFFSET_REFRESH_MHZ, mode.refresh_mhz);
    put32(&mut out, at + Mode::OFFSET_FLAGS, mode.flags);
    put32(&mut out, at + Mode::OFFSET_CLOCK_KHZ, mode.clock_khz);
    put16(&mut out, at + Mode::OFFSET_H_SYNC_START, mode.h_sync_start);
    put16(&mut out, at + Mode::OFFSET_H_SYNC_END, mode.h_sync_end);
    put16(&mut out, at + Mode::OFFSET_H_TOTAL, mode.h_total);
    put16(&mut out, at + Mode::OFFSET_V_SYNC_START, mode.v_sync_start);
    put16(&mut out, at + Mode::OFFSET_V_SYNC_END, mode.v_sync_end);
    put16(&mut out, at + Mode::OFFSET_V_TOTAL, mode.v_total);
    out
}

/// The value's number, which MAKE_BLOB wrote back.
pub fn blob_made(bytes: &[u8]) -> Option<u32> {
    le32(bytes, Blob::OFFSET_BLOB)
}

/// A rectangle as PLANE_SOURCE, PLANE_PLACE and PLANE_CHANGED pack one:
/// the column in bits 0 to 15, the row in bits 16 to 31, the width in
/// bits 32 to 47 and the height in bits 48 to 63.
pub fn rect(x: i16, y: i16, width: u16, height: u16) -> u64 {
    u64::from(x as u16)
        | u64::from(y as u16) << 16
        | u64::from(width) << 32
        | u64::from(height) << 48
}

/// MOVE_POINTER's `argument`: the plane, and where its hot spot goes.
pub fn pointer_place(plane: u32, x: i16, y: i16) -> u64 {
    u64::from(plane) << 32 | u64::from(x as u16) << 16 | u64::from(y as u16)
}

/// A CHANGE's `buffer`: the Change, then its entries.
#[derive(Clone, Debug)]
pub struct ChangeBytes {
    bytes: Vec<u8>,
}

impl ChangeBytes {
    /// A change of no entry yet, with `flags` (CHECK_ONLY and the rest)
    /// and the caller's `cookie` for it.
    pub fn new(flags: u32, cookie: u64) -> ChangeBytes {
        let mut bytes = vec![0; Change::SIZE];
        put32(&mut bytes, Change::OFFSET_FLAGS, flags);
        put64(&mut bytes, Change::OFFSET_COOKIE, cookie);
        ChangeBytes { bytes }
    }

    /// How many entries it has.
    pub fn len(&self) -> usize {
        (self.bytes.len() - Change::SIZE) / Entry::SIZE
    }

    /// Whether it has no entry.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Adds an entry: `property` of `thing` is to be `value`. One past
    /// ENTRIES_MAX is left out, and the kernel refuses what a change
    /// that long would have been for.
    pub fn set(&mut self, thing: u32, property: u32, value: u64) {
        if self.len() >= ENTRIES_MAX as usize {
            return;
        }
        self.bytes.extend_from_slice(&thing.to_le_bytes());
        self.bytes.extend_from_slice(&property.to_le_bytes());
        self.bytes.extend_from_slice(&value.to_le_bytes());
        let count = self.len() as u32;
        put32(&mut self.bytes, Change::OFFSET_ENTRIES, count);
    }

    /// The bytes, for CHANGE to read and write back.
    pub fn bytes_mut(&mut self) -> &mut [u8] {
        &mut self.bytes
    }

    /// The entry a failed CHANGE wrote back and why, a REASON_*: `None`
    /// when the failure was no entry's.
    pub fn refusal(&self) -> Option<(u32, u32)> {
        let reason = le32(&self.bytes, Change::OFFSET_REASON)?;
        (reason != 0).then_some((le32(&self.bytes, Change::OFFSET_FAILED)?, reason))
    }
}

/// How many bytes of one pixel a [`PutBytes::fill`] keeps at most: the
/// rectangles it adds share them, whatever their size.
const FILL_BYTES: usize = 64 * 1024;

/// Where the rows start in a [`PutBytes`]: after the Put and a table of
/// every rectangle a PUT may carry.
const ROWS_AT: usize = Put::SIZE + PUT_RECTS_MAX as usize * PutRect::SIZE;

/// A run of one pixel in a PUT's rows, which rectangles that fill share.
#[derive(Clone, Copy, Debug)]
struct Run {
    at: usize,
    pixels: usize,
    pixel: [u8; 4],
}

/// A PUT's `buffer`: the Put, its rectangles, and their rows of
/// FORMAT_XRGB8888 pixels. The table always has room for PUT_RECTS_MAX
/// rectangles, so that the rows never move as rectangles are added.
#[derive(Clone, Debug)]
pub struct PutBytes {
    bytes: Vec<u8>,
    rects: usize,
    run: Option<Run>,
}

impl Default for PutBytes {
    fn default() -> PutBytes {
        PutBytes::new()
    }
}

impl PutBytes {
    /// A PUT of no rectangle yet.
    pub fn new() -> PutBytes {
        PutBytes { bytes: vec![0; ROWS_AT], rects: 0, run: None }
    }

    /// Whether it has no rectangle.
    pub fn is_empty(&self) -> bool {
        self.rects == 0
    }

    /// Whether its table is full.
    pub fn is_full(&self) -> bool {
        self.rects >= PUT_RECTS_MAX as usize
    }

    /// The bytes of its rows.
    pub fn row_bytes(&self) -> usize {
        self.bytes.len() - ROWS_AT
    }

    /// Forgets its rectangles and rows, and keeps its memory for the
    /// next ones.
    pub fn clear(&mut self) {
        self.bytes.truncate(ROWS_AT);
        self.rects = 0;
        self.run = None;
    }

    /// Notes a rectangle whose rows start at `offset` of the bytes.
    fn note(&mut self, x: u16, y: u16, width: u16, height: u16, offset: usize) {
        let at = Put::SIZE + self.rects * PutRect::SIZE;
        put16(&mut self.bytes, at + PutRect::OFFSET_X, x);
        put16(&mut self.bytes, at + PutRect::OFFSET_Y, y);
        put16(&mut self.bytes, at + PutRect::OFFSET_WIDTH, width);
        put16(&mut self.bytes, at + PutRect::OFFSET_HEIGHT, height);
        put32(&mut self.bytes, at + PutRect::OFFSET_OFFSET, offset as u32);
        put32(&mut self.bytes, at + PutRect::OFFSET_RESERVED, 0);
        self.rects += 1;
    }

    /// Adds a rectangle of `width` by `height` whose left column is `x`
    /// and top row `y`, and returns its rows to draw in: `width` pixels
    /// of four bytes each, row after row, all zero. `None` if the table
    /// is full, the rectangle is empty, or its rows would start past
    /// what an offset can say.
    pub fn rect(&mut self, x: u16, y: u16, width: u16, height: u16) -> Option<&mut [u8]> {
        let len = usize::from(width) * 4 * usize::from(height);
        let at = self.bytes.len();
        if self.is_full() || len == 0 || u32::try_from(at + len).is_err() {
            return None;
        }
        self.bytes.resize(at + len, 0);
        self.note(x, y, width, height, at);
        Some(&mut self.bytes[at..])
    }

    /// Adds rectangles that paint `width` by `height` at (`x`, `y`) in
    /// `pixel`, from the top down. They share one run of the pixel, so a
    /// large rectangle takes no more bytes than a small one. Returns how
    /// many of its rows are still to paint because the table filled up:
    /// the caller sends what there is, and fills the rest in the next
    /// PUT.
    pub fn fill(&mut self, x: u16, y: u16, width: u16, height: u16, pixel: [u8; 4]) -> u16 {
        if width == 0 || height == 0 {
            return 0;
        }
        let row = usize::from(width);
        // Enough of the pixel to paint all of it at once, or a good part
        // of a large one; a row of it at the least.
        let wanted = (row * usize::from(height)).min((FILL_BYTES / 4).max(row));
        let run = match self.run {
            Some(run) if run.pixel == pixel && run.pixels >= wanted => run,
            _ => {
                let at = self.bytes.len();
                if u32::try_from(at + wanted * 4).is_err() {
                    return height;
                }
                for _ in 0..wanted {
                    self.bytes.extend_from_slice(&pixel);
                }
                let run = Run { at, pixels: wanted, pixel };
                self.run = Some(run);
                run
            }
        };
        // Whole rows of the rectangle, as many as the run holds.
        let step = (run.pixels / row).min(usize::from(u16::MAX)) as u16;
        let (mut top, mut left) = (y, height);
        while left > 0 && !self.is_full() {
            let rows = left.min(step);
            self.note(x, top, width, rows, run.at);
            top = top.saturating_add(rows);
            left -= rows;
        }
        left
    }

    /// The bytes of a PUT of these rectangles into `picture`, for the
    /// request to read.
    pub fn bytes_for(&mut self, picture: u32) -> &mut [u8] {
        put32(&mut self.bytes, Put::OFFSET_PICTURE, picture);
        put32(&mut self.bytes, Put::OFFSET_RECTS, self.rects as u32);
        &mut self.bytes
    }
}

/// The events a read of a holder's file put into `bytes`: every whole
/// one, the oldest first.
pub fn events(bytes: &[u8]) -> impl Iterator<Item = DisplayEvent> + '_ {
    bytes.as_chunks::<{ DisplayEvent::SIZE }>().0.iter().filter_map(|event| {
        Some(DisplayEvent {
            kind: le32(event, DisplayEvent::OFFSET_KIND)?,
            thing: le32(event, DisplayEvent::OFFSET_THING)?,
            time_ns: le64(event, DisplayEvent::OFFSET_TIME_NS)?,
            count: le64(event, DisplayEvent::OFFSET_COUNT)?,
            cookie: le64(event, DisplayEvent::OFFSET_COOKIE)?,
            deadline_ns: le64(event, DisplayEvent::OFFSET_DEADLINE_NS)?,
        })
    })
}

/// The text of a name the kernel padded with zeros, as a card's or a
/// monitor's: up to the first zero, or nothing if it is not UTF-8.
pub fn text(name: &[u8]) -> &str {
    let end = name.iter().position(|&byte| byte == 0).unwrap_or(name.len());
    core::str::from_utf8(&name[..end]).unwrap_or("")
}
