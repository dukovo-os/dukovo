// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The records of radix-display(2), read from and written to bytes.
//!
//! A request's `buffer` is a program's memory, copied into the kernel
//! before anything looks at it. The readers here take those bytes and
//! give back values that are safe to use: every count is held to the
//! ABI's limit, every length is checked against the bytes there are, and
//! nothing is indexed that was not checked. A reader that returns `None`
//! means the request is invalid_argument; none of them can panic, which
//! the proofs at the end of the file check for the reader of a change's
//! entries.
//!
//! The writers lay a record out as the ABI's offsets say, into an array
//! of the record's size.

use radix_abi::display::{
    Blob, Buffer, CHECK_ONLY, CardInfo, Change, DisplayEvent, ENTRIES_MAX, Entry, Get, MAY_BLANK,
    Mode, NEWEST_WINS, NO_WAIT, NOW, PUT_RECTS_MAX, Picture, PipeInfo, PlaneInfo, Put, PutRect,
    ScreenInfo, Thing,
};

use crate::Rect;

fn le16(bytes: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(bytes.get(at..at.checked_add(2)?)?.try_into().ok()?))
}

fn le32(bytes: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(bytes.get(at..at.checked_add(4)?)?.try_into().ok()?))
}

fn le64(bytes: &[u8], at: usize) -> Option<u64> {
    Some(u64::from_le_bytes(bytes.get(at..at.checked_add(8)?)?.try_into().ok()?))
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

/// The flags a change may carry.
const CHANGE_FLAGS: u32 = CHECK_ONLY | MAY_BLANK | NO_WAIT | NOW | NEWEST_WINS;

/// A change as CHANGE's `buffer` holds it: the header, and the bytes of
/// its entries.
#[derive(Clone, Copy, Debug)]
pub struct ChangeRequest<'a> {
    /// CHECK_ONLY, MAY_BLANK, NO_WAIT, NOW and NEWEST_WINS bits.
    pub flags: u32,
    /// The caller's own number for the change.
    pub cookie: u64,
    entries: &'a [u8],
}

impl<'a> ChangeRequest<'a> {
    /// Reads a change from `bytes`. `None` if the header is cut short,
    /// carries a flag the ABI does not name or more than ENTRIES_MAX
    /// entries, or if the entries it counts are not all there.
    pub fn read(bytes: &'a [u8]) -> Option<ChangeRequest<'a>> {
        let flags = le32(bytes, Change::OFFSET_FLAGS)?;
        let count = le32(bytes, Change::OFFSET_ENTRIES)?;
        let cookie = le64(bytes, Change::OFFSET_COOKIE)?;
        if flags & !CHANGE_FLAGS != 0 || count > ENTRIES_MAX {
            return None;
        }
        let len = (count as usize).checked_mul(Entry::SIZE)?;
        let entries = bytes.get(Change::SIZE..Change::SIZE.checked_add(len)?)?;
        Some(ChangeRequest { flags, cookie, entries })
    }

    /// How many bytes of `buffer` hold a change whose header is `head`,
    /// entries included: what the kernel copies in before it reads the
    /// change. `None` if the header is cut short or counts more than
    /// ENTRIES_MAX entries.
    pub fn bytes_of(head: &[u8]) -> Option<usize> {
        let count = le32(head, Change::OFFSET_ENTRIES)?;
        if count > ENTRIES_MAX {
            return None;
        }
        (count as usize).checked_mul(Entry::SIZE)?.checked_add(Change::SIZE)
    }

    /// How many entries it has.
    pub fn len(&self) -> usize {
        self.entries.len() / Entry::SIZE
    }

    /// Whether it has no entry.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Entry `index`, if it has that many.
    pub fn entry(&self, index: usize) -> Option<Entry> {
        let at = index.checked_mul(Entry::SIZE)?;
        Some(Entry {
            thing: le32(self.entries, at.checked_add(Entry::OFFSET_THING)?)?,
            property: le32(self.entries, at.checked_add(Entry::OFFSET_PROPERTY)?)?,
            value: le64(self.entries, at.checked_add(Entry::OFFSET_VALUE)?)?,
        })
    }

    /// Its entries in order.
    pub fn entries(&self) -> impl Iterator<Item = Entry> + '_ {
        (0..self.len()).filter_map(|index| self.entry(index))
    }
}

/// The bytes a failed check writes back over a Change, from its `failed`
/// field on: which entry, and why.
pub fn refusal(failed: u32, reason: u32) -> [u8; 8] {
    let mut out = [0; 8];
    put32(&mut out, 0, failed);
    put32(&mut out, Change::OFFSET_REASON - Change::OFFSET_FAILED, reason);
    out
}

/// Where a failed check's [`refusal`] goes in CHANGE's `buffer`.
pub const REFUSAL_AT: usize = Change::OFFSET_FAILED;

/// One rectangle of a PUT: where it goes in the picture, and where its
/// rows start in the request's `buffer`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PutPart {
    /// The rectangle of the picture the rows fill.
    pub rect: Rect,
    /// Where the first row starts in `buffer`. The rows follow one
    /// another, each `rect.width` pixels of 4 bytes.
    pub offset: u64,
}

/// A PUT as the start of its `buffer` holds it: the picture, and the
/// bytes of its rectangles.
#[derive(Clone, Copy, Debug)]
pub struct PutRequest<'a> {
    /// The picture the rows go into.
    pub picture: u32,
    rects: &'a [u8],
    /// The bytes of the whole `buffer`, rows included.
    total: u64,
}

impl<'a> PutRequest<'a> {
    /// How many bytes of `buffer` hold the header and `rects` rectangles:
    /// what the kernel copies in before it reads the request.
    pub fn table_bytes(rects: u32) -> Option<usize> {
        if rects > PUT_RECTS_MAX {
            return None;
        }
        (rects as usize).checked_mul(PutRect::SIZE)?.checked_add(Put::SIZE)
    }

    /// How many rectangles the header in `bytes` counts.
    pub fn count(bytes: &[u8]) -> Option<u32> {
        le32(bytes, Put::OFFSET_RECTS)
    }

    /// Reads a PUT from `table`, the first bytes of a `buffer` of `total`
    /// bytes. `None` if the table is cut short or counts more than
    /// PUT_RECTS_MAX rectangles.
    pub fn read(table: &'a [u8], total: u64) -> Option<PutRequest<'a>> {
        let picture = le32(table, Put::OFFSET_PICTURE)?;
        let len = PutRequest::table_bytes(PutRequest::count(table)?)?;
        if (len as u64) > total {
            return None;
        }
        Some(PutRequest { picture, rects: table.get(Put::SIZE..len)?, total })
    }

    /// How many rectangles it has.
    pub fn len(&self) -> usize {
        self.rects.len() / PutRect::SIZE
    }

    /// Whether it has no rectangle.
    pub fn is_empty(&self) -> bool {
        self.rects.is_empty()
    }

    /// Rectangle `index`, checked against a picture of `width` by
    /// `height`: `None` if there is no such rectangle, if it is empty or
    /// not all in the picture, or if its rows start inside the table or
    /// run past the end of `buffer`.
    pub fn part(&self, index: usize, width: u32, height: u32) -> Option<PutPart> {
        let at = index.checked_mul(PutRect::SIZE)?;
        let field = |offset: usize| le16(self.rects, at.checked_add(offset)?);
        let rect = Rect {
            x: i32::from(field(PutRect::OFFSET_X)?),
            y: i32::from(field(PutRect::OFFSET_Y)?),
            width: u32::from(field(PutRect::OFFSET_WIDTH)?),
            height: u32::from(field(PutRect::OFFSET_HEIGHT)?),
        };
        let offset = u64::from(le32(self.rects, at.checked_add(PutRect::OFFSET_OFFSET)?)?);
        if rect.is_empty() || !rect.within(width, height) {
            return None;
        }
        let rows = u64::from(rect.width) * 4 * u64::from(rect.height);
        let table = (Put::SIZE + self.rects.len()) as u64;
        if offset < table || offset.checked_add(rows)? > self.total {
            return None;
        }
        Some(PutPart { rect, offset })
    }
}

/// What GET reads: the thing and the part asked for.
pub fn read_get(bytes: &[u8]) -> Option<Get> {
    Some(Get { thing: le32(bytes, Get::OFFSET_THING)?, part: le32(bytes, Get::OFFSET_PART)? })
}

/// What MAKE_BUFFER reads. The fields it writes back read as 0.
pub fn read_buffer(bytes: &[u8]) -> Option<Buffer> {
    Some(Buffer {
        file: le32(bytes, Buffer::OFFSET_FILE)?,
        card: le32(bytes, Buffer::OFFSET_CARD)?,
        format: le32(bytes, Buffer::OFFSET_FORMAT)?,
        width: le32(bytes, Buffer::OFFSET_WIDTH)?,
        height: le32(bytes, Buffer::OFFSET_HEIGHT)?,
        row_bytes: 0,
        bytes: 0,
    })
}

/// What MAKE_BUFFER writes back: the whole record.
pub fn buffer_bytes(buffer: &Buffer) -> [u8; Buffer::SIZE] {
    let mut out = [0; Buffer::SIZE];
    put32(&mut out, Buffer::OFFSET_FILE, buffer.file);
    put32(&mut out, Buffer::OFFSET_CARD, buffer.card);
    put32(&mut out, Buffer::OFFSET_FORMAT, buffer.format);
    put32(&mut out, Buffer::OFFSET_WIDTH, buffer.width);
    put32(&mut out, Buffer::OFFSET_HEIGHT, buffer.height);
    put32(&mut out, Buffer::OFFSET_ROW_BYTES, buffer.row_bytes);
    put64(&mut out, Buffer::OFFSET_BYTES, buffer.bytes);
    out
}

/// What MAKE_PICTURE reads. The number it writes back reads as 0.
pub fn read_picture(bytes: &[u8]) -> Option<Picture> {
    Some(Picture {
        file: le32(bytes, Picture::OFFSET_FILE)?,
        card: le32(bytes, Picture::OFFSET_CARD)?,
        flags: le32(bytes, Picture::OFFSET_FLAGS)?,
        format: le32(bytes, Picture::OFFSET_FORMAT)?,
        width: le32(bytes, Picture::OFFSET_WIDTH)?,
        height: le32(bytes, Picture::OFFSET_HEIGHT)?,
        row_bytes: le32(bytes, Picture::OFFSET_ROW_BYTES)?,
        picture: 0,
        offset: le64(bytes, Picture::OFFSET_OFFSET)?,
    })
}

/// Where MAKE_PICTURE writes the picture's number in its `buffer`.
pub const PICTURE_NUMBER_AT: usize = Picture::OFFSET_PICTURE;

/// What MAKE_BLOB reads before the value's bytes: its kind and how many
/// bytes follow.
pub fn read_blob(bytes: &[u8]) -> Option<(u32, u32)> {
    Some((le32(bytes, Blob::OFFSET_KIND)?, le32(bytes, Blob::OFFSET_BYTES)?))
}

/// Where MAKE_BLOB writes the value's number in its `buffer`.
pub const BLOB_NUMBER_AT: usize = Blob::OFFSET_BLOB;

/// A mode as a BLOB_MODE value's bytes hold it. `None` if they are not
/// exactly a Mode.
pub fn read_mode(bytes: &[u8]) -> Option<Mode> {
    if bytes.len() != Mode::SIZE {
        return None;
    }
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

fn put_mode(out: &mut [u8], mode: &Mode) {
    put32(out, Mode::OFFSET_WIDTH, mode.width);
    put32(out, Mode::OFFSET_HEIGHT, mode.height);
    put32(out, Mode::OFFSET_REFRESH_MHZ, mode.refresh_mhz);
    put32(out, Mode::OFFSET_FLAGS, mode.flags);
    put32(out, Mode::OFFSET_CLOCK_KHZ, mode.clock_khz);
    put16(out, Mode::OFFSET_H_SYNC_START, mode.h_sync_start);
    put16(out, Mode::OFFSET_H_SYNC_END, mode.h_sync_end);
    put16(out, Mode::OFFSET_H_TOTAL, mode.h_total);
    put16(out, Mode::OFFSET_V_SYNC_START, mode.v_sync_start);
    put16(out, Mode::OFFSET_V_SYNC_END, mode.v_sync_end);
    put16(out, Mode::OFFSET_V_TOTAL, mode.v_total);
}

/// A Mode, as PART_MODES gives one.
pub fn mode_bytes(mode: &Mode) -> [u8; Mode::SIZE] {
    let mut out = [0; Mode::SIZE];
    put_mode(&mut out, mode);
    out
}

/// A Thing, as LIST gives one.
pub fn thing_bytes(thing: &Thing) -> [u8; Thing::SIZE] {
    let mut out = [0; Thing::SIZE];
    put32(&mut out, Thing::OFFSET_ID, thing.id);
    put32(&mut out, Thing::OFFSET_KIND, thing.kind);
    put32(&mut out, Thing::OFFSET_CARD, thing.card);
    put32(&mut out, Thing::OFFSET_INDEX, thing.index);
    out
}

/// A CardInfo, as GET gives one.
pub fn card_bytes(info: &CardInfo) -> [u8; CardInfo::SIZE] {
    let mut out = [0; CardInfo::SIZE];
    put32(&mut out, CardInfo::OFFSET_ID, info.id);
    put32(&mut out, CardInfo::OFFSET_KIND, info.kind);
    put32(&mut out, CardInfo::OFFSET_FLAGS, info.flags);
    put32(&mut out, CardInfo::OFFSET_SCREENS, info.screens);
    put32(&mut out, CardInfo::OFFSET_PIPES, info.pipes);
    put32(&mut out, CardInfo::OFFSET_PLANES, info.planes);
    put32(&mut out, CardInfo::OFFSET_WIDTH_MAX, info.width_max);
    put32(&mut out, CardInfo::OFFSET_HEIGHT_MAX, info.height_max);
    out[CardInfo::OFFSET_NAME..][..16].copy_from_slice(&info.name);
    out[CardInfo::OFFSET_MODEL..][..32].copy_from_slice(&info.model);
    out
}

/// A ScreenInfo, as GET gives one.
pub fn screen_bytes(info: &ScreenInfo) -> [u8; ScreenInfo::SIZE] {
    let mut out = [0; ScreenInfo::SIZE];
    put32(&mut out, ScreenInfo::OFFSET_ID, info.id);
    put32(&mut out, ScreenInfo::OFFSET_CARD, info.card);
    put32(&mut out, ScreenInfo::OFFSET_FLAGS, info.flags);
    put32(&mut out, ScreenInfo::OFFSET_PIPE, info.pipe);
    put32(&mut out, ScreenInfo::OFFSET_WIDTH_MM, info.width_mm);
    put32(&mut out, ScreenInfo::OFFSET_HEIGHT_MM, info.height_mm);
    put32(&mut out, ScreenInfo::OFFSET_MODES, info.modes);
    put32(&mut out, ScreenInfo::OFFSET_MONITOR_DATA_BYTES, info.monitor_data_bytes);
    put32(&mut out, ScreenInfo::OFFSET_REFRESH_MIN_MHZ, info.refresh_min_mhz);
    put32(&mut out, ScreenInfo::OFFSET_REFRESH_MAX_MHZ, info.refresh_max_mhz);
    put32(&mut out, ScreenInfo::OFFSET_LUMINANCE_MAX, info.luminance_max);
    put32(&mut out, ScreenInfo::OFFSET_PRODUCT, info.product);
    put32(&mut out, ScreenInfo::OFFSET_SERIAL, info.serial);
    out[ScreenInfo::OFFSET_DEPTH] = info.depth;
    out[ScreenInfo::OFFSET_MAKER..][..4].copy_from_slice(&info.maker);
    out[ScreenInfo::OFFSET_NAME..][..16].copy_from_slice(&info.name);
    out
}

/// A PipeInfo, as GET gives one.
pub fn pipe_bytes(info: &PipeInfo) -> [u8; PipeInfo::SIZE] {
    let mut out = [0; PipeInfo::SIZE];
    put32(&mut out, PipeInfo::OFFSET_ID, info.id);
    put32(&mut out, PipeInfo::OFFSET_CARD, info.card);
    put32(&mut out, PipeInfo::OFFSET_FLAGS, info.flags);
    put32(&mut out, PipeInfo::OFFSET_COLOR_TABLE_ENTRIES, info.color_table_entries);
    put64(&mut out, PipeInfo::OFFSET_BEAT_NS, info.beat_ns);
    put64(&mut out, PipeInfo::OFFSET_BEAT_COUNT, info.beat_count);
    put64(&mut out, PipeInfo::OFFSET_BEAT_TIME_NS, info.beat_time_ns);
    put_mode(&mut out[PipeInfo::OFFSET_MODE..], &info.mode);
    out
}

/// A PlaneInfo, as GET gives one.
pub fn plane_bytes(info: &PlaneInfo) -> [u8; PlaneInfo::SIZE] {
    let mut out = [0; PlaneInfo::SIZE];
    put32(&mut out, PlaneInfo::OFFSET_ID, info.id);
    put32(&mut out, PlaneInfo::OFFSET_CARD, info.card);
    put32(&mut out, PlaneInfo::OFFSET_KIND, info.kind);
    put32(&mut out, PlaneInfo::OFFSET_FLAGS, info.flags);
    put32(&mut out, PlaneInfo::OFFSET_PIPE, info.pipe);
    put32(&mut out, PlaneInfo::OFFSET_FIXED_PIPE, info.fixed_pipe);
    put32(&mut out, PlaneInfo::OFFSET_WIDTH_MAX, info.width_max);
    put32(&mut out, PlaneInfo::OFFSET_HEIGHT_MAX, info.height_max);
    for (index, format) in info.formats.iter().enumerate() {
        put32(&mut out, PlaneInfo::OFFSET_FORMATS + index * 4, *format);
    }
    out
}

/// A DisplayEvent, as a read of a holder's file gives one.
pub fn event_bytes(event: &DisplayEvent) -> [u8; DisplayEvent::SIZE] {
    let mut out = [0; DisplayEvent::SIZE];
    put32(&mut out, DisplayEvent::OFFSET_KIND, event.kind);
    put32(&mut out, DisplayEvent::OFFSET_THING, event.thing);
    put64(&mut out, DisplayEvent::OFFSET_TIME_NS, event.time_ns);
    put64(&mut out, DisplayEvent::OFFSET_COUNT, event.count);
    put64(&mut out, DisplayEvent::OFFSET_COOKIE, event.cookie);
    put64(&mut out, DisplayEvent::OFFSET_DEADLINE_NS, event.deadline_ns);
    out
}

#[cfg(test)]
mod tests {
    use radix_abi::display::{PIPE_ACTIVE, PLANE_PICTURE, REASON_SIZE};

    use super::*;

    /// A change's bytes: its header, then `entries`.
    fn change(flags: u32, cookie: u64, entries: &[(u32, u32, u64)]) -> Vec<u8> {
        let mut bytes = vec![0; Change::SIZE];
        put32(&mut bytes, Change::OFFSET_FLAGS, flags);
        put32(&mut bytes, Change::OFFSET_ENTRIES, entries.len() as u32);
        put64(&mut bytes, Change::OFFSET_COOKIE, cookie);
        for &(thing, property, value) in entries {
            bytes.extend_from_slice(&thing.to_le_bytes());
            bytes.extend_from_slice(&property.to_le_bytes());
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        bytes
    }

    #[test]
    fn a_change_gives_its_entries_in_order() {
        let bytes = change(NO_WAIT | NEWEST_WINS, 7, &[(3, PIPE_ACTIVE, 1), (5, PLANE_PICTURE, 9)]);
        let request = ChangeRequest::read(&bytes).unwrap();
        assert_eq!((request.flags, request.cookie, request.len()), (NO_WAIT | NEWEST_WINS, 7, 2));
        let entries: Vec<Entry> = request.entries().collect();
        assert_eq!(entries[0], Entry { thing: 3, property: PIPE_ACTIVE, value: 1 });
        assert_eq!(entries[1], Entry { thing: 5, property: PLANE_PICTURE, value: 9 });
        assert_eq!(request.entry(2), None);
        // Its header says how long all of it is.
        assert_eq!(ChangeRequest::bytes_of(&bytes[..Change::SIZE]), Some(bytes.len()));
        assert_eq!(ChangeRequest::bytes_of(&bytes[..Change::OFFSET_ENTRIES + 3]), None);
    }

    #[test]
    fn a_change_cut_short_or_over_its_limits_is_refused() {
        let bytes = change(0, 0, &[(3, PIPE_ACTIVE, 1)]);
        assert!(ChangeRequest::read(&bytes[..bytes.len() - 1]).is_none());
        assert!(ChangeRequest::read(&bytes[..Change::SIZE - 1]).is_none());
        assert!(ChangeRequest::read(&change(1 << 5, 0, &[])).is_none());
        let mut many = change(0, 0, &[]);
        put32(&mut many, Change::OFFSET_ENTRIES, ENTRIES_MAX + 1);
        many.resize(Change::SIZE + (ENTRIES_MAX as usize + 1) * Entry::SIZE, 0);
        assert!(ChangeRequest::bytes_of(&many).is_none());
        assert!(ChangeRequest::read(&many).is_none());
        // The most the ABI allows is read.
        put32(&mut many, Change::OFFSET_ENTRIES, ENTRIES_MAX);
        assert_eq!(ChangeRequest::read(&many).unwrap().len(), ENTRIES_MAX as usize);
        assert!(ChangeRequest::read(&change(0, 0, &[])).unwrap().is_empty());
    }

    #[test]
    fn a_refusal_lands_on_the_fields_a_change_reads_back() {
        let mut bytes = change(0, 0, &[]);
        bytes[REFUSAL_AT..][..8].copy_from_slice(&refusal(4, REASON_SIZE));
        assert_eq!(le32(&bytes, Change::OFFSET_FAILED), Some(4));
        assert_eq!(le32(&bytes, Change::OFFSET_REASON), Some(REASON_SIZE));
    }

    /// A PUT's table: its header, then `rects` as (x, y, width, height,
    /// offset).
    fn put(picture: u32, rects: &[(u16, u16, u16, u16, u32)]) -> Vec<u8> {
        let mut bytes = vec![0; Put::SIZE];
        put32(&mut bytes, Put::OFFSET_PICTURE, picture);
        put32(&mut bytes, Put::OFFSET_RECTS, rects.len() as u32);
        for &(x, y, width, height, offset) in rects {
            for part in [x, y, width, height] {
                bytes.extend_from_slice(&part.to_le_bytes());
            }
            bytes.extend_from_slice(&offset.to_le_bytes());
            bytes.extend_from_slice(&0u32.to_le_bytes());
        }
        bytes
    }

    #[test]
    fn a_put_gives_each_rectangle_and_where_its_rows_start() {
        let table = put(2, &[(0, 16, 800, 16, 40), (8, 64, 8, 16, 40 + 800 * 4 * 16)]);
        assert_eq!(table.len(), 40);
        let total = 40 + 800 * 4 * 16 + 8 * 4 * 16;
        let request = PutRequest::read(&table, total).unwrap();
        assert_eq!((request.picture, request.len()), (2, 2));
        assert_eq!(
            request.part(0, 800, 600),
            Some(PutPart { rect: Rect { x: 0, y: 16, width: 800, height: 16 }, offset: 40 })
        );
        assert_eq!(request.part(1, 800, 600).unwrap().offset, 40 + 800 * 4 * 16);
        assert_eq!(request.part(2, 800, 600), None);
    }

    #[test]
    fn a_put_whose_rows_are_not_all_there_is_refused() {
        let table = put(2, &[(0, 0, 800, 16, 24)]);
        let rows = 800 * 4 * 16;
        assert!(PutRequest::read(&table, 24 + rows).unwrap().part(0, 800, 600).is_some());
        // One byte short.
        assert!(PutRequest::read(&table, 24 + rows - 1).unwrap().part(0, 800, 600).is_none());
        // Past the picture's bottom, and its right edge.
        assert!(PutRequest::read(&table, 24 + rows).unwrap().part(0, 800, 15).is_none());
        assert!(PutRequest::read(&table, 24 + rows).unwrap().part(0, 799, 600).is_none());
        // Rows that start inside the table.
        let inside = put(2, &[(0, 0, 8, 1, 23)]);
        assert!(PutRequest::read(&inside, 4096).unwrap().part(0, 800, 600).is_none());
        // An empty rectangle.
        let empty = put(2, &[(0, 0, 0, 16, 24)]);
        assert!(PutRequest::read(&empty, 4096).unwrap().part(0, 800, 600).is_none());
    }

    #[test]
    fn a_put_with_too_many_rectangles_is_refused() {
        assert_eq!(PutRequest::table_bytes(PUT_RECTS_MAX), Some(8 + 256 * 16));
        assert_eq!(PutRequest::table_bytes(PUT_RECTS_MAX + 1), None);
        let mut table = put(2, &[]);
        put32(&mut table, Put::OFFSET_RECTS, 3);
        // The header counts rectangles the table does not hold.
        assert!(PutRequest::read(&table, 4096).is_none());
        assert!(PutRequest::read(&put(2, &[]), 8).unwrap().is_empty());
    }

    #[test]
    fn a_mode_survives_its_bytes() {
        let mode = Mode {
            width: 2560,
            height: 1440,
            refresh_mhz: 143_856,
            flags: 1,
            clock_khz: 586_000,
            h_sync_start: 2568,
            h_sync_end: 2600,
            h_total: 2640,
            v_sync_start: 1446,
            v_sync_end: 1454,
            v_total: 1543,
        };
        assert_eq!(read_mode(&mode_bytes(&mode)), Some(mode));
        assert_eq!(read_mode(&mode_bytes(&mode)[..Mode::SIZE - 1]), None);
        assert_eq!(read_mode(&[0; Mode::SIZE + 1]), None);
    }

    #[test]
    fn a_picture_request_reads_every_field_but_the_answer() {
        let mut bytes = [0u8; Picture::SIZE];
        for (index, at) in [
            Picture::OFFSET_FILE,
            Picture::OFFSET_CARD,
            Picture::OFFSET_FLAGS,
            Picture::OFFSET_FORMAT,
            Picture::OFFSET_WIDTH,
            Picture::OFFSET_HEIGHT,
            Picture::OFFSET_ROW_BYTES,
            Picture::OFFSET_PICTURE,
        ]
        .into_iter()
        .enumerate()
        {
            put32(&mut bytes, at, index as u32 + 1);
        }
        put64(&mut bytes, Picture::OFFSET_OFFSET, 4096);
        let picture = read_picture(&bytes).unwrap();
        assert_eq!((picture.file, picture.card, picture.flags, picture.format), (1, 2, 3, 4),);
        assert_eq!((picture.width, picture.height, picture.row_bytes), (5, 6, 7));
        assert_eq!((picture.picture, picture.offset), (0, 4096));
        assert!(read_picture(&bytes[..Picture::SIZE - 1]).is_none());
    }

    #[test]
    fn an_event_and_a_screen_are_laid_out_as_the_abi_says() {
        let event = DisplayEvent {
            kind: 1,
            thing: 9,
            time_ns: 1_000_000_007,
            count: 60,
            cookie: 0xfeed,
            deadline_ns: 1_016_000_000,
        };
        let bytes = event_bytes(&event);
        assert_eq!(le32(&bytes, 0), Some(1));
        assert_eq!(le64(&bytes, DisplayEvent::OFFSET_DEADLINE_NS), Some(1_016_000_000));
        let screen = ScreenInfo {
            id: 4,
            depth: 10,
            maker: *b"DEL\0",
            name: *b"DELL U2720Q\0\0\0\0\0",
            ..ScreenInfo::default()
        };
        let bytes = screen_bytes(&screen);
        assert_eq!(bytes[ScreenInfo::OFFSET_DEPTH], 10);
        assert_eq!(&bytes[ScreenInfo::OFFSET_MAKER..][..3], b"DEL");
        assert_eq!(&bytes[ScreenInfo::OFFSET_NAME..][..11], b"DELL U2720Q");
    }
}

#[cfg(kani)]
mod proofs {
    use super::*;

    /// Whatever bytes a program gives for a change, reading it and every
    /// entry it counts reads nothing outside them and never panics.
    #[kani::proof]
    #[kani::unwind(5)]
    fn a_change_is_read_within_its_bytes() {
        let bytes: [u8; Change::SIZE + 3 * Entry::SIZE] = kani::any();
        let len: usize = kani::any_where(|&len: &usize| len <= Change::SIZE + 3 * Entry::SIZE);
        if let Some(request) = ChangeRequest::read(&bytes[..len]) {
            assert!(Change::SIZE + request.len() * Entry::SIZE <= len);
            for index in 0..request.len() {
                assert!(request.entry(index).is_some());
            }
            assert!(request.entry(request.len()).is_none());
        }
    }

    /// A rectangle of a PUT that is accepted lies in the picture, and its
    /// rows lie in the buffer, past the table.
    #[kani::proof]
    fn a_put_part_is_within_the_picture_and_the_buffer() {
        let table: [u8; Put::SIZE + PutRect::SIZE] = kani::any();
        let total: u64 = kani::any();
        let width: u32 = kani::any_where(|&side: &u32| side <= radix_abi::display::SIDE_MAX);
        let height: u32 = kani::any_where(|&side: &u32| side <= radix_abi::display::SIDE_MAX);
        let Some(request) = PutRequest::read(&table, total) else { return };
        let Some(part) = request.part(0, width, height) else { return };
        assert!(part.rect.within(width, height) && !part.rect.is_empty());
        let rows = u64::from(part.rect.width) * 4 * u64::from(part.rect.height);
        assert!(part.offset >= (Put::SIZE + PutRect::SIZE) as u64);
        assert!(part.offset + rows <= total);
    }
}
