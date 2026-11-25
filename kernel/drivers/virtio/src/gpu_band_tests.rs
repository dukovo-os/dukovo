// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The stores that carry rows to a picture the host keeps, against the
//! simulated card of `gpu_card`: where a sheet places rectangles, the
//! band carrying text to a picture many times its height, a band the
//! host has not emptied yet, pictures of two widths on one band, and a
//! store of a picture's own.

use std::sync::Arc;
use std::vec::Vec;

use radix_hw::Dma;

use crate::Error;
use crate::gpu::{Band, Gpu, Packed, Rect, Rows, Same, Sheet, Target, code, format, put_whole};
use crate::gpu_card::{Card, never, paste, pixels, setup, setup_with};

/// The picture the tests write rows into: resource 1, on screen 0.
const PICTURE: Target = Target { resource: 1, width: 640, shown: true };
const SIZE: (u32, u32) = (640, 480);
/// A band of 25 rows of the picture.
const BAND: usize = 64 * 1024;

/// Makes the resource `to` of `height` rows with `backing` behind it,
/// on screen `scanout`.
fn keep(gpu: &mut Gpu, to: Target, height: u32, backing: (u64, u32), scanout: u32) {
    gpu.create(to.resource, format::BGRX, to.width, height).unwrap();
    gpu.attach(to.resource, backing.0, backing.1).unwrap();
    gpu.set_scanout(scanout, to.resource, Rect::sized(to.width, height)).unwrap();
    gpu.finish(&mut never).unwrap();
}

/// A card that shows the picture, and a band of `bytes` behind it.
fn setup_band(bytes: usize) -> (Arc<Card>, Gpu, Band) {
    let (card, mut gpu) = setup();
    let band = Band::new(Dma::fake(bytes));
    keep(&mut gpu, PICTURE, SIZE.1, band.backing(), 0);
    (card, gpu, band)
}

/// `rows`, the pixels of `rect`, as the band asks for them.
fn packed(rect: Rect, rows: &[u8]) -> Packed<'_> {
    Packed::new(rect, rows).unwrap()
}

/// What the glass shows of a picture nothing was written into.
fn blank((width, height): (u32, u32)) -> Vec<u8> {
    std::vec![0; width as usize * height as usize * 4]
}

/// Rectangles a console writes: letters, a line, a block taller than
/// the band, one pixel, and the last corner.
fn writes() -> Vec<Rect> {
    let cell = |column: u32, row: u32| Rect { x: column * 8, y: row * 16, width: 8, height: 16 };
    std::vec![
        cell(0, 0),
        cell(1, 0),
        cell(79, 29),
        Rect { x: 0, y: 32, width: 640, height: 16 },
        Rect { x: 200, y: 100, width: 100, height: 60 },
        Rect { x: 0, y: 200, width: 640, height: 60 },
        Rect { x: 333, y: 444, width: 1, height: 1 },
        Rect { x: 632, y: 470, width: 8, height: 10 },
        cell(1, 0),
    ]
}

#[test]
fn a_sheet_places_rectangles_on_shelves() {
    // Ten rows of a picture 100 pixels wide.
    let mut sheet = Sheet::new(100, 100 * 4 * 10);
    assert_eq!((sheet.width(), sheet.rows()), (100, 10));
    assert_eq!(sheet.place(40, 3), Some(0));
    assert_eq!(sheet.place(40, 5), Some(40 * 4));
    // No room beside: a shelf under the tallest of the first.
    assert_eq!(sheet.place(40, 2), Some(5 * 400));
    assert_eq!(sheet.place(60, 5), Some(5 * 400 + 40 * 4));
    // Full, and nothing changes for having asked.
    assert_eq!(sheet.place(10, 1), None);
    assert_eq!(sheet.place(10, 1), None);
    sheet.clear();
    assert_eq!(sheet.place(100, 10), Some(0));
    assert_eq!(sheet.place(1, 1), None);
}

#[test]
fn a_sheet_refuses_what_never_fits() {
    let mut sheet = Sheet::new(100, 100 * 4 * 10);
    assert_eq!(sheet.place(101, 1), None, "wider than the picture");
    assert_eq!(sheet.place(1, 11), None, "taller than the store");
    assert_eq!(sheet.place(0, 1), None);
    assert_eq!(sheet.place(1, 0), None);
    // A store too short for one row of the picture holds nothing.
    let mut short = Sheet::new(1000, 100);
    assert_eq!(short.rows(), 0);
    assert_eq!(short.place(1, 1), None);
}

#[test]
fn the_band_carries_text_to_a_picture_many_times_its_height() {
    let (card, mut gpu, mut band) = setup_band(BAND);
    let mut expected = blank(SIZE);
    for (seed, rect) in writes().into_iter().enumerate() {
        let rows = pixels(seed as u8, rect);
        band.put(&mut gpu, PICTURE, rect, &mut packed(rect, &rows), &mut never).unwrap();
        paste(&mut expected, 640, rect, &rows);
    }
    gpu.finish(&mut never).unwrap();
    assert_eq!(card.glass(0), expected);
    assert_eq!(gpu.lost(), 0);
    // The card answered each write as it was told of it: nothing was
    // waited for, and it was never asked to interrupt.
    assert_eq!(card.interrupts(), 0);
}

#[test]
fn rows_show_only_on_a_picture_that_is_on_a_screen() {
    let (card, mut gpu, mut band) = setup_band(BAND);
    let rect = Rect { x: 16, y: 16, width: 64, height: 16 };
    let rows = pixels(1, rect);
    let unseen = Target { shown: false, ..PICTURE };
    band.put(&mut gpu, unseen, rect, &mut packed(rect, &rows), &mut never).unwrap();
    gpu.finish(&mut never).unwrap();
    assert_eq!(card.glass(0), blank(SIZE), "copied, and not flushed");
    assert!(!card.state.lock().unwrap().commands.contains(&code::RESOURCE_FLUSH));
    gpu.flush(1, rect).unwrap();
    gpu.finish(&mut never).unwrap();
    let mut expected = blank(SIZE);
    paste(&mut expected, 640, rect, &rows);
    assert_eq!(card.glass(0), expected);
}

#[test]
fn a_write_does_not_wait_for_the_host() {
    let (card, mut gpu, mut band) = setup_band(BAND);
    card.state.lock().unwrap().hold = true;
    let rect = Rect { x: 0, y: 0, width: 640, height: 16 };
    band.put(&mut gpu, PICTURE, rect, &mut packed(rect, &pixels(1, rect)), &mut never).unwrap();
    assert_eq!(gpu.collect(), 2, "a transfer and a flush, told of and not waited for");
    assert_eq!(card.state.lock().unwrap().doorbells[0], 2);
    card.release();
    assert_eq!(gpu.collect(), 0);
    assert_eq!(card.interrupts(), 0);
}

#[test]
fn a_full_band_waits_for_the_host_to_empty_it() {
    let (card, mut gpu, mut band) = setup_band(BAND);
    card.state.lock().unwrap().hold = true;
    let mut waits = 0;
    let mut wait = || {
        waits += 1;
        card.release();
        card.state.lock().unwrap().hold = true;
        true
    };
    // Two lines of sixteen rows do not fit the band's twenty-five: the
    // second waits for the host to copy the first, whose rows it would
    // otherwise write over.
    let mut expected = blank(SIZE);
    for line in 0..2 {
        let rect = Rect { x: 0, y: line * 16, width: 640, height: 16 };
        let rows = pixels(line as u8 + 1, rect);
        band.put(&mut gpu, PICTURE, rect, &mut packed(rect, &rows), &mut wait).unwrap();
        paste(&mut expected, 640, rect, &rows);
    }
    assert_eq!(waits, 1);
    card.release();
    gpu.finish(&mut never).unwrap();
    assert_eq!(card.glass(0), expected);
}

#[test]
fn a_band_waits_when_no_more_commands_fit() {
    let (card, mut gpu, mut band) = setup_band(BAND);
    card.state.lock().unwrap().hold = true;
    let mut waits = 0;
    let mut wait = || {
        waits += 1;
        card.release();
        card.state.lock().unwrap().hold = true;
        true
    };
    // Sixteen letters are thirty-two commands, which is every slot.
    let mut expected = blank(SIZE);
    for column in 0..17 {
        let rect = Rect { x: column * 8, y: 0, width: 8, height: 16 };
        let rows = pixels(column as u8, rect);
        band.put(&mut gpu, PICTURE, rect, &mut packed(rect, &rows), &mut wait).unwrap();
        paste(&mut expected, 640, rect, &rows);
    }
    assert_eq!(waits, 1);
    card.release();
    gpu.finish(&mut never).unwrap();
    assert_eq!(card.glass(0), expected);
}

#[test]
fn pictures_of_two_widths_share_the_band() {
    let (card, gpu) = setup_with(0, &[Some(SIZE), Some((320, 200))], false);
    let mut gpu = gpu.unwrap();
    let mut band = Band::new(Dma::fake(BAND));
    let small = Target { resource: 2, width: 320, shown: true };
    keep(&mut gpu, PICTURE, SIZE.1, band.backing(), 0);
    keep(&mut gpu, small, 200, band.backing(), 1);
    card.state.lock().unwrap().hold = true;
    let mut waits = 0;
    let mut wait = || {
        waits += 1;
        card.release();
        card.state.lock().unwrap().hold = true;
        true
    };
    let (mut first, mut second) = (blank(SIZE), blank((320, 200)));
    let rect = Rect { x: 8, y: 16, width: 300, height: 20 };
    for round in 0..3u8 {
        let rows = pixels(round, rect);
        band.put(&mut gpu, PICTURE, rect, &mut packed(rect, &rows), &mut wait).unwrap();
        paste(&mut first, 640, rect, &rows);
        // Rows of another length are laid out anew, once the host has
        // read the ones in the band.
        let rows = pixels(round + 10, rect);
        band.put(&mut gpu, small, rect, &mut packed(rect, &rows), &mut wait).unwrap();
        paste(&mut second, 320, rect, &rows);
    }
    assert_eq!(waits, 5);
    card.release();
    gpu.finish(&mut never).unwrap();
    assert_eq!(card.glass(0), first);
    assert_eq!(card.glass(1), second);
}

/// A step of a small generator of numbers that look random.
fn next(state: &mut u32) -> u32 {
    *state ^= *state << 13;
    *state ^= *state >> 17;
    *state ^= *state << 5;
    *state
}

#[test]
fn any_mix_of_writes_shows_as_written() {
    let (card, mut gpu, mut band) = setup_band(BAND);
    let mut state = 0x2545_f491;
    let mut expected = blank(SIZE);
    let mut waits = 0;
    for round in 0..300u32 {
        // Now and then the host falls behind, and catches up only when
        // it is waited for.
        if next(&mut state).is_multiple_of(4) {
            card.state.lock().unwrap().hold = true;
        }
        let (width, height) = (1 + next(&mut state) % 200, 1 + next(&mut state) % 70);
        let (x, y) = (next(&mut state) % (641 - width), next(&mut state) % (481 - height));
        let rect = Rect { x, y, width, height };
        let rows = pixels(round as u8, rect);
        let mut wait = || {
            waits += 1;
            card.release();
            true
        };
        band.put(&mut gpu, PICTURE, rect, &mut packed(rect, &rows), &mut wait).unwrap();
        paste(&mut expected, 640, rect, &rows);
    }
    card.release();
    gpu.finish(&mut never).unwrap();
    assert!(waits > 0, "the band filled at least once");
    assert_eq!(card.glass(0), expected);
    assert_eq!(gpu.lost(), 0);
    assert_eq!(gpu.bad_entries(), 0);
}

#[test]
fn rows_the_card_refuses_are_counted_and_are_nobodys_error() {
    let (card, mut gpu, mut band) = setup_band(BAND);
    let rect = Rect { x: 0, y: 0, width: 8, height: 16 };
    let nowhere = Target { resource: 9, ..PICTURE };
    band.put(&mut gpu, nowhere, rect, &mut packed(rect, &pixels(1, rect)), &mut never).unwrap();
    // The change that comes next is not refused for it.
    gpu.flush(1, rect).unwrap();
    assert_eq!(gpu.finish(&mut never), Ok(()));
    assert_eq!(gpu.lost(), 2, "the transfer and the flush");
    assert_eq!(card.glass(0), blank(SIZE));
}

#[test]
fn rows_that_are_not_the_rectangles_are_not_sent() {
    let (card, mut gpu, mut band) = setup_band(BAND);
    let sent = card.state.lock().unwrap().commands.len();
    let rect = Rect { x: 0, y: 0, width: 8, height: 16 };
    let rows = pixels(1, rect);
    assert!(Packed::new(rect, &rows[..rows.len() - 4]).is_none());
    assert!(Packed::new(Rect { width: 0, ..rect }, &[]).is_none());
    // A row a pixel short, and one a pixel long.
    for len in [28, 36] {
        let mut odd = Same(&rows[..len]);
        assert_eq!(band.put(&mut gpu, PICTURE, rect, &mut odd, &mut never), Err(Error::BadRequest));
        assert_eq!(
            band.fill(&mut gpu, PICTURE, rect, &rows[..len], &mut never),
            Err(Error::BadRequest)
        );
    }
    let empty = Rect { width: 0, ..rect };
    assert_eq!(
        band.put(&mut gpu, PICTURE, empty, &mut Same(&[]), &mut never),
        Err(Error::BadRequest)
    );
    assert_eq!(band.fill(&mut gpu, PICTURE, empty, &[], &mut never), Err(Error::BadRequest));
    // A band too short for a row of the picture.
    let mut narrow = Band::new(Dma::fake(2048));
    assert_eq!(narrow.backing().1, 2048);
    let mut whole = packed(rect, &rows);
    assert_eq!(narrow.put(&mut gpu, PICTURE, rect, &mut whole, &mut never), Err(Error::BadRequest));
    assert_eq!(
        narrow.fill(&mut gpu, PICTURE, rect, &rows[..32], &mut never),
        Err(Error::BadRequest)
    );
    let store = Dma::fake(640 * 480 * 4);
    let put = |gpu: &mut Gpu, rect| {
        put_whole(gpu, &store, PICTURE, rect, &mut packed(rect, &rows), &mut never)
    };
    assert_eq!(put(&mut gpu, Rect { y: 470, ..rect }), Err(Error::BadRequest), "past the end");
    assert_eq!(put(&mut gpu, Rect { x: 636, ..rect }), Err(Error::BadRequest), "past the side");
    assert_eq!(card.state.lock().unwrap().commands.len(), sent);
}

#[test]
fn a_store_of_its_own_carries_the_same_rows() {
    let (card, mut gpu) = setup();
    let store = Dma::fake(640 * 480 * 4);
    keep(&mut gpu, PICTURE, SIZE.1, (store.phys(), store.len() as u32), 0);
    card.state.lock().unwrap().hold = true;
    let mut waits = 0;
    let mut wait = || {
        waits += 1;
        card.release();
        card.state.lock().unwrap().hold = true;
        true
    };
    let mut expected = blank(SIZE);
    for round in 0..3 {
        for (seed, rect) in writes().into_iter().enumerate() {
            let rows = pixels(seed as u8 + 16 * round, rect);
            put_whole(&mut gpu, &store, PICTURE, rect, &mut packed(rect, &rows), &mut wait)
                .unwrap();
            paste(&mut expected, 640, rect, &rows);
        }
    }
    // Fifty-four commands, waited for only when the slots ran out.
    assert_eq!(waits, 1);
    card.release();
    gpu.finish(&mut never).unwrap();
    assert_eq!(card.glass(0), expected);
}

/// Rows drawn as they are asked for, which counts the asking.
struct Drawn {
    rect: Rect,
    row: Vec<u8>,
    asked: Vec<u32>,
}

impl Rows for Drawn {
    fn row(&mut self, index: u32) -> &[u8] {
        self.asked.push(index);
        let whole = Rect { y: self.rect.y + index, height: 1, ..self.rect };
        self.row = pixels(7, whole);
        &self.row
    }
}

#[test]
fn rows_are_asked_for_once_each_from_the_top() {
    let (card, mut gpu, mut band) = setup_band(BAND);
    // Taller than the band's 25 rows: three parts.
    let rect = Rect { x: 0, y: 100, width: 640, height: 60 };
    let mut drawn = Drawn { rect, row: Vec::new(), asked: Vec::new() };
    card.state.lock().unwrap().hold = true;
    let mut wait = || {
        card.release();
        card.state.lock().unwrap().hold = true;
        true
    };
    band.put(&mut gpu, PICTURE, rect, &mut drawn, &mut wait).unwrap();
    card.release();
    gpu.finish(&mut never).unwrap();
    assert_eq!(drawn.asked, (0..60).collect::<Vec<_>>());
    let mut expected = blank(SIZE);
    for line in 0..60 {
        let whole = Rect { y: 100 + line, height: 1, ..rect };
        paste(&mut expected, 640, whole, &pixels(7, whole));
    }
    assert_eq!(card.glass(0), expected);
}

/// The transfers the card was sent after its first `skip` commands.
fn transfers(card: &Card, skip: usize) -> usize {
    let state = card.state.lock().unwrap();
    state.commands[skip..].iter().filter(|&&sent| sent == code::TRANSFER_TO_HOST_2D).count()
}

#[test]
fn an_area_of_one_colour_is_copied_from_one_strip() {
    let (card, mut gpu, mut band) = setup_band(BAND);
    let sent = card.state.lock().unwrap().commands.len();
    card.state.lock().unwrap().hold = true;
    let waits = std::cell::Cell::new(0);
    let mut wait = || {
        waits.set(waits.get() + 1);
        card.release();
        card.state.lock().unwrap().hold = true;
        true
    };
    // The whole picture: 480 rows from a strip of 25.
    let slate: Vec<u8> = [0x1c, 0x18, 0x15, 0].repeat(640);
    band.fill(&mut gpu, PICTURE, Rect::sized(640, 480), &slate, &mut wait).unwrap();
    // The strip was never sent again, so nothing waited for the host
    // to copy it: twenty commands fit the queue.
    assert_eq!(waits.get(), 0);
    // Rows written over it and a smaller area go beside the strip, or
    // wait for it as any rows do.
    let text = Rect { x: 16, y: 32, width: 80, height: 16 };
    band.put(&mut gpu, PICTURE, text, &mut packed(text, &pixels(3, text)), &mut wait).unwrap();
    let bar = Rect { x: 200, y: 300, width: 240, height: 5 };
    let sand: Vec<u8> = [0x8b, 0xc4, 0xe9, 0].repeat(240);
    band.fill(&mut gpu, PICTURE, bar, &sand, &mut wait).unwrap();
    card.release();
    gpu.finish(&mut never).unwrap();
    let mut expected = slate.repeat(480);
    paste(&mut expected, 640, text, &pixels(3, text));
    paste(&mut expected, 640, bar, &sand.repeat(5));
    assert_eq!(card.glass(0), expected);
    assert_eq!(transfers(&card, sent), 20 + 1 + 1);
    assert_eq!(gpu.lost(), 0);
}

#[test]
fn a_tall_area_waits_only_for_room_to_queue_its_parts() {
    // A band of two rows: 240 parts for the picture.
    let (card, mut gpu, mut band) = setup_band(2 * 640 * 4);
    card.state.lock().unwrap().hold = true;
    let mut waits = 0;
    let mut wait = || {
        waits += 1;
        card.release();
        card.state.lock().unwrap().hold = true;
        true
    };
    let white: Vec<u8> = [0xff, 0xff, 0xff, 0].repeat(640);
    band.fill(&mut gpu, PICTURE, Rect::sized(640, 480), &white, &mut wait).unwrap();
    card.release();
    gpu.finish(&mut never).unwrap();
    assert_eq!(card.glass(0), white.repeat(480));
    // 240 transfers and a flush through 32 slots.
    assert!((7..=9).contains(&waits), "{waits} waits");
    assert_eq!(gpu.lost(), 0);
}
