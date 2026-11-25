// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Each record against the kernel's own reader or writer of it
//! (`radix_display::wire`), and the requests against a stand-in for the
//! holder's file.

use std::cell::RefCell;
use std::io;

use radix_abi::display::{
    BLOB_MODE, Blob, Buffer, CHANGE, CHECK_ONLY, CardInfo, Change, DisplayEvent, EVENT_FRONT,
    EVENT_SCREEN, FORMAT_XRGB8888, Get, KIND_CARD, KIND_PLANE, KIND_SCREEN, LIST, MAKE_PICTURE,
    MAY_BLANK, Mode, PART_MODES, PICTURE_KEPT, PIPE_ACTIVE, PLANE_PICTURE, PLANE_PLACE,
    PUT_RECTS_MAX, Picture, PipeInfo, PlaneInfo, Put, PutRect, REASON_SIZE, ScreenInfo, Thing,
};
use radix_display::Rect;
use radix_display::wire as kernel;

use super::wire::{self, ChangeBytes, PutBytes};
use super::{Control, Holder, Unmade};

fn mode() -> Mode {
    Mode {
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
    }
}

#[test]
fn a_listing_is_read_as_the_kernel_writes_it() {
    let listed = [
        Thing { id: 1, kind: KIND_CARD, card: 1, index: 0 },
        Thing { id: 3, kind: KIND_SCREEN, card: 1, index: 0 },
        Thing { id: 13, kind: KIND_PLANE, card: 6, index: 3 },
    ];
    let mut bytes = Vec::new();
    for thing in &listed {
        bytes.extend_from_slice(&kernel::thing_bytes(thing));
    }
    assert_eq!(wire::things(&bytes).collect::<Vec<_>>(), listed);
    // One cut short is left out.
    assert_eq!(wire::things(&bytes[..bytes.len() - 1]).count(), 2);
}

#[test]
fn what_get_asks_is_what_the_kernel_reads() {
    let asked = kernel::read_get(&wire::get(9, PART_MODES)).unwrap();
    assert_eq!((asked.thing, asked.part), (9, PART_MODES));
    assert_eq!(wire::get(9, PART_MODES).len(), Get::SIZE);
}

#[test]
fn each_things_info_is_read_as_the_kernel_writes_it() {
    let card = CardInfo {
        id: 6,
        kind: 2,
        flags: 13,
        screens: 4,
        pipes: 4,
        planes: 8,
        width_max: 16384,
        height_max: 8192,
        name: *b"virtio0\0\0\0\0\0\0\0\0\0",
        model: *b"Virtio graphics card\0\0\0\0\0\0\0\0\0\0\0\0",
    };
    assert_eq!(wire::card(&kernel::card_bytes(&card)), Some(card));
    assert_eq!(wire::text(&card.name), "virtio0");
    let screen = ScreenInfo {
        id: 9,
        card: 6,
        flags: 3,
        pipe: 7,
        width_mm: 597,
        height_mm: 336,
        modes: 21,
        monitor_data_bytes: 256,
        refresh_min_mhz: 48_000,
        refresh_max_mhz: 144_000,
        luminance_max: 350,
        product: 0xa0c1,
        serial: 0x0102_0304,
        depth: 10,
        reserved: [0; 3],
        maker: *b"DEL\0",
        name: *b"DELL U2720Q\0\0\0\0\0",
    };
    assert_eq!(wire::screen(&kernel::screen_bytes(&screen)), Some(screen));
    let pipe = PipeInfo {
        id: 7,
        card: 6,
        flags: 1,
        color_table_entries: 256,
        beat_ns: 6_951_396,
        beat_count: 1_000_000_007,
        beat_time_ns: 99_000_000_123,
        mode: mode(),
    };
    assert_eq!(wire::pipe(&kernel::pipe_bytes(&pipe)), Some(pipe));
    let plane = PlaneInfo {
        id: 11,
        card: 6,
        kind: 1,
        flags: 1,
        pipe: 7,
        fixed_pipe: 7,
        width_max: 16384,
        height_max: 16384,
        formats: [FORMAT_XRGB8888, 2, 3, 4, 5, 6, 7, 8],
    };
    assert_eq!(wire::plane(&kernel::plane_bytes(&plane)), Some(plane));
    // An answer cut short is none.
    assert_eq!(wire::card(&kernel::card_bytes(&card)[..CardInfo::SIZE - 1]), None);
    assert_eq!(wire::screen(&kernel::screen_bytes(&screen)[..ScreenInfo::SIZE - 1]), None);
    assert_eq!(wire::pipe(&kernel::pipe_bytes(&pipe)[..PipeInfo::SIZE - 1]), None);
    assert_eq!(wire::plane(&kernel::plane_bytes(&plane)[..PlaneInfo::SIZE - 1]), None);
}

#[test]
fn modes_are_read_in_the_order_given() {
    let slow = Mode { refresh_mhz: 60_000, flags: 0, ..mode() };
    let mut bytes = kernel::mode_bytes(&mode()).to_vec();
    bytes.extend_from_slice(&kernel::mode_bytes(&slow));
    assert_eq!(wire::modes(&bytes).collect::<Vec<_>>(), [mode(), slow]);
}

#[test]
fn a_buffer_and_a_picture_are_what_the_kernel_reads() {
    let buffer = Buffer {
        file: 40,
        card: 6,
        format: FORMAT_XRGB8888,
        width: 1920,
        height: 1080,
        row_bytes: 0,
        bytes: 0,
    };
    assert_eq!(kernel::read_buffer(&wire::buffer(&buffer)), Some(buffer));
    let made = Buffer { row_bytes: 7680, bytes: 8_294_400, ..buffer };
    assert_eq!(wire::buffer_made(&kernel::buffer_bytes(&made)), Some((7680, 8_294_400)));
    let picture = Picture {
        file: 40,
        card: 6,
        flags: 0,
        format: FORMAT_XRGB8888,
        width: 1920,
        height: 1080,
        row_bytes: 7680,
        picture: 0,
        offset: 4096,
    };
    let mut bytes = wire::picture(&picture);
    assert_eq!(kernel::read_picture(&bytes), Some(picture));
    bytes[kernel::PICTURE_NUMBER_AT..][..4].copy_from_slice(&17u32.to_le_bytes());
    assert_eq!(wire::picture_made(&bytes), Some(17));
}

#[test]
fn a_mode_is_kept_as_the_value_the_kernel_reads() {
    let mut bytes = wire::mode_blob(&mode());
    assert_eq!(kernel::read_blob(&bytes), Some((BLOB_MODE, Mode::SIZE as u32)));
    assert_eq!(kernel::read_mode(&bytes[Blob::SIZE..]), Some(mode()));
    bytes[kernel::BLOB_NUMBER_AT..][..4].copy_from_slice(&5u32.to_le_bytes());
    assert_eq!(wire::blob_made(&bytes), Some(5));
}

#[test]
fn a_change_is_what_the_kernel_reads_and_its_refusal_is_read_back() {
    let mut change = ChangeBytes::new(MAY_BLANK | CHECK_ONLY, 0xfeed);
    assert!(change.is_empty());
    change.set(7, PIPE_ACTIVE, 1);
    change.set(11, PLANE_PICTURE, 2);
    change.set(11, PLANE_PLACE, wire::rect(-5, 3, 640, 480));
    assert_eq!(change.len(), 3);
    let request = kernel::ChangeRequest::read(change.bytes_mut()).unwrap();
    assert_eq!((request.flags, request.cookie, request.len()), (MAY_BLANK | CHECK_ONLY, 0xfeed, 3));
    let entries: Vec<_> = request.entries().collect();
    assert_eq!((entries[0].thing, entries[0].property, entries[0].value), (7, PIPE_ACTIVE, 1));
    assert_eq!((entries[1].thing, entries[1].property, entries[1].value), (11, PLANE_PICTURE, 2));
    assert_eq!(Rect::unpack(entries[2].value, true), Rect { x: -5, y: 3, width: 640, height: 480 });
    // Nothing refused yet; then the kernel's refusal of the second entry.
    assert_eq!(change.refusal(), None);
    change.bytes_mut()[kernel::REFUSAL_AT..][..8].copy_from_slice(&kernel::refusal(1, REASON_SIZE));
    assert_eq!(change.refusal(), Some((1, REASON_SIZE)));
    assert_eq!(change.bytes_mut().len(), Change::SIZE + 3 * 16);
}

#[test]
fn the_pointers_place_is_packed_as_the_kernel_unpacks_it() {
    let argument = wire::pointer_place(13, -20, 700);
    assert_eq!((argument >> 32) as u32, 13);
    assert_eq!((argument >> 16) as u16 as i16, -20);
    assert_eq!(argument as u16 as i16, 700);
}

/// The rectangles of `put` as the kernel reads them for a picture of
/// `width` by `height`, with the rows of each.
fn parts(put: &mut PutBytes, width: u32, height: u32) -> Vec<(Rect, Vec<u8>)> {
    let bytes = put.bytes_for(2).to_vec();
    let count = kernel::PutRequest::count(&bytes).unwrap();
    let table = kernel::PutRequest::table_bytes(count).unwrap();
    let request = kernel::PutRequest::read(&bytes[..table], bytes.len() as u64).unwrap();
    assert_eq!(request.picture, 2);
    (0..request.len())
        .map(|index| {
            let part = request.part(index, width, height).expect("a rectangle the kernel takes");
            let len = part.rect.width as usize * 4 * part.rect.height as usize;
            (part.rect, bytes[part.offset as usize..][..len].to_vec())
        })
        .collect()
}

#[test]
fn a_puts_rectangles_and_rows_are_what_the_kernel_reads() {
    let mut put = PutBytes::new();
    assert!(put.is_empty());
    put.rect(8, 16, 2, 2).unwrap().copy_from_slice(&[1; 16]);
    put.rect(0, 32, 3, 1).unwrap().copy_from_slice(&[2; 12]);
    assert_eq!(put.row_bytes(), 28);
    let parts = parts(&mut put, 800, 600);
    assert_eq!(parts[0], (Rect { x: 8, y: 16, width: 2, height: 2 }, vec![1; 16]));
    assert_eq!(parts[1], (Rect { x: 0, y: 32, width: 3, height: 1 }, vec![2; 12]));
    // An empty rectangle is not added.
    assert!(put.rect(0, 0, 0, 4).is_none());
    put.clear();
    assert!(put.is_empty() && put.row_bytes() == 0);
}

#[test]
fn a_fill_shares_one_run_of_its_pixel() {
    let pixel = [0x1c, 0x18, 0x15, 0];
    let mut put = PutBytes::new();
    // A small one: all of it at once.
    assert_eq!(put.fill(0, 0, 100, 4, pixel), 0);
    assert_eq!(put.row_bytes(), 100 * 4 * 4);
    // A large one: 16384 pixels of the run paint 8 rows of 1920 at a
    // time, so 1080 rows are 135 rectangles and no more bytes than one.
    let mut put = PutBytes::new();
    assert_eq!(put.fill(0, 0, 1920, 1080, pixel), 0);
    assert_eq!(put.row_bytes(), 64 * 1024);
    let parts = parts(&mut put, 1920, 1080);
    assert_eq!(parts.len(), 135);
    let mut next = 0;
    for (rect, rows) in &parts {
        assert_eq!((rect.x, rect.y, rect.width), (0, next, 1920));
        assert!(rows.as_chunks::<4>().0.iter().all(|px| *px == pixel));
        next += rect.height as i32;
    }
    assert_eq!(next, 1080);
    // A narrower one in the same pixel takes the same run; another
    // pixel takes a run of its own.
    assert_eq!(put.fill(0, 0, 4, 1080, pixel), 0);
    assert_eq!(put.row_bytes(), 64 * 1024);
    assert_eq!(put.fill(0, 0, 4, 2, [0xff; 4]), 0);
    assert_eq!(put.row_bytes(), 64 * 1024 + 32);
}

#[test]
fn a_fill_stops_at_a_full_table_and_says_what_is_left() {
    let mut put = PutBytes::new();
    // The widest picture: a row of it at a time.
    let left = put.fill(0, 0, 16384, 300, [9; 4]);
    assert!(put.is_full());
    assert_eq!(left, 300 - PUT_RECTS_MAX as u16);
    assert_eq!(put.row_bytes(), 16384 * 4);
    assert!(put.rect(0, 0, 1, 1).is_none());
    assert_eq!(put.fill(0, 0, 1, 1, [9; 4]), 1);
    assert_eq!(parts(&mut put, 16384, 300).len(), PUT_RECTS_MAX as usize);
    assert_eq!(put.bytes_for(2).len(), Put::SIZE + 256 * PutRect::SIZE + 16384 * 4);
}

#[test]
fn events_are_read_as_the_kernel_writes_them() {
    let front = DisplayEvent { kind: EVENT_FRONT, time_ns: 5, ..DisplayEvent::default() };
    let screen = DisplayEvent {
        kind: EVENT_SCREEN,
        thing: 9,
        time_ns: 1_000_000_007,
        count: 60,
        cookie: 0xfeed,
        deadline_ns: 1_016_000_000,
    };
    let mut bytes = kernel::event_bytes(&front).to_vec();
    bytes.extend_from_slice(&kernel::event_bytes(&screen));
    assert_eq!(wire::events(&bytes).collect::<Vec<_>>(), [front, screen]);
}

#[test]
fn a_name_ends_at_its_first_zero() {
    assert_eq!(wire::text(b"firmware0\0\0\0"), "firmware0");
    assert_eq!(wire::text(b"full"), "full");
    assert_eq!(wire::text(&[0xff, 0xfe, 0]), "");
}

/// A stand-in for a holder's file: answers LIST with `things`, notes
/// every request, and refuses a CHANGE's second entry when told to.
#[derive(Default)]
struct Fake {
    things: Vec<Thing>,
    refuses: bool,
    asked: RefCell<Vec<(u32, u64, Vec<u8>)>>,
}

impl Control for Fake {
    fn control(&self, request: u32, argument: u64, buffer: &mut [u8]) -> io::Result<u64> {
        self.asked.borrow_mut().push((request, argument, buffer.to_vec()));
        match request {
            LIST => {
                // As many as fit, and how many there are.
                for (thing, out) in
                    self.things.iter().zip(buffer.as_chunks_mut::<{ Thing::SIZE }>().0)
                {
                    out.copy_from_slice(&kernel::thing_bytes(thing));
                }
                Ok(self.things.len() as u64)
            }
            MAKE_PICTURE => {
                buffer[kernel::PICTURE_NUMBER_AT..][..4].copy_from_slice(&3u32.to_le_bytes());
                Ok(0)
            }
            CHANGE if self.refuses => {
                buffer[kernel::REFUSAL_AT..][..8].copy_from_slice(&kernel::refusal(1, REASON_SIZE));
                Err(io::Error::from_raw_os_error(22))
            }
            CHANGE => Err(io::Error::from_raw_os_error(16)),
            _ => Ok(0),
        }
    }
}

#[test]
fn a_listing_longer_than_the_first_room_is_asked_for_again() {
    let things: Vec<Thing> =
        (1..=40).map(|id| Thing { id, kind: KIND_PLANE, card: 1, index: id - 1 }).collect();
    let holder = Holder::new(Fake { things: things.clone(), ..Fake::default() });
    assert_eq!(holder.things().unwrap(), things);
    let asked = holder.file().asked.borrow();
    assert_eq!(asked.len(), 2);
    assert_eq!((asked[0].2.len(), asked[1].2.len()), (32 * Thing::SIZE, 40 * Thing::SIZE));
}

#[test]
fn a_kept_picture_is_asked_for_as_one_the_card_keeps() {
    let holder = Holder::new(Fake::default());
    assert_eq!(holder.keep_picture(6, 1280, 800).unwrap(), 3);
    let asked = holder.file().asked.borrow();
    let picture = kernel::read_picture(&asked[0].2).unwrap();
    assert_eq!(asked[0].0, MAKE_PICTURE);
    assert_eq!((picture.card, picture.flags, picture.format), (6, PICTURE_KEPT, FORMAT_XRGB8888));
    assert_eq!((picture.width, picture.height), (1280, 800));
}

#[test]
fn a_change_that_fails_says_whether_an_entry_was_refused() {
    let mut change = ChangeBytes::new(0, 1);
    change.set(7, PIPE_ACTIVE, 1);
    change.set(11, PLANE_PICTURE, 2);
    let holder = Holder::new(Fake { refuses: true, ..Fake::default() });
    match holder.change(&mut change) {
        Err(Unmade::Refused { entry: 1, reason: REASON_SIZE }) => {}
        other => panic!("{other:?}"),
    }
    // Not in front: no entry is at fault.
    let mut change = ChangeBytes::new(0, 2);
    change.set(7, PIPE_ACTIVE, 1);
    let holder = Holder::new(Fake::default());
    match holder.change(&mut change) {
        Err(Unmade::Failed(error)) => assert_eq!(error.raw_os_error(), Some(16)),
        other => panic!("{other:?}"),
    }
}
