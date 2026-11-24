// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The graphics driver against the simulated card of `gpu_card`: every
//! command, a picture in pieces and one on two screens, shared pages, a
//! monitor that comes and goes, the pointer's own queue, answers that
//! are late, short, wrong or missing, and a card started again for the
//! panic report. The stores that carry rows have their tests in
//! `gpu_band_tests`.

use std::vec::Vec;

use radix_hw::{Dma, Mmio};

use crate::gpu::{
    EDID_MAX, EVENT_DISPLAY, Gpu, POINTER_SIDE, Place, Rect, Scanout, Shared, code, entry, feature,
    format,
};
use crate::gpu_card::{
    Card, Pointer, VECTORS, never, paste, pixels, setup, setup_with, show, store,
};
use crate::tests::{BAR_SIZE, alloc, function};
use crate::{Error, NO_VECTOR, Transport, device_type, feature as common_feature, status};

#[test]
fn finds_the_graphics_card() {
    assert_eq!(device_type(&function(16, 0x1050)), Some(device_type::GPU));
}

#[test]
fn sets_the_card_up_with_what_it_offers() {
    let (card, gpu) = setup();
    assert_eq!(gpu.scanouts(), 2);
    assert!(gpu.has_edid() && !gpu.shares_pages() && !gpu.failed());
    let s = card.state.lock().unwrap();
    assert_ne!(s.status & status::DRIVER_OK, 0);
    let agreed = common_feature::VERSION_1 | common_feature::EVENT_IDX | feature::EDID;
    assert_eq!(s.driver_features, agreed);
    // An interrupt for what the screens show, one for the answers, and
    // none for the pointer's queue.
    assert_eq!((s.config_vector, s.vectors), (0, [1, NO_VECTOR]));
    assert_eq!((s.rings[0].size, s.rings[1].size), (64, 16));
}

#[test]
fn shared_pages_are_taken_only_when_asked_for() {
    let offered = feature::EDID | feature::RESOURCE_BLOB;
    let (_, gpu) = setup_with(offered, &[Some((640, 480))], false);
    assert!(!gpu.unwrap().shares_pages());
    let (_, gpu) = setup_with(offered, &[Some((640, 480))], true);
    assert!(gpu.unwrap().shares_pages());
    // Asked for, and not offered.
    let (_, gpu) = setup_with(0, &[Some((640, 480))], true);
    let gpu = gpu.unwrap();
    assert!(!gpu.shares_pages() && !gpu.has_edid());
}

#[test]
fn a_card_with_no_screen_or_too_many_is_refused() {
    for screens in [0, 17] {
        let (card, gpu) = setup_with(0, &std::vec![None; screens], false);
        assert!(matches!(gpu, Err(Error::MissingFeature(_))));
        assert_ne!(card.state.lock().unwrap().status & status::FAILED, 0);
    }
}

#[test]
fn a_card_that_takes_no_vector_is_refused() {
    let card = Card::new(0, &[Some((640, 480))]);
    card.state.lock().unwrap().no_vectors = true;
    let regs = card.clone();
    let transport = Transport::find(&function(16, 0x1050), &mut |_, _| {
        Some(Mmio::fake(regs.clone(), BAR_SIZE))
    })
    .unwrap();
    assert!(matches!(Gpu::new(transport, VECTORS, false, &mut alloc), Err(Error::NoVector)));
}

#[test]
fn says_what_each_screen_shows() {
    let (card, mut gpu) = setup();
    let screens = gpu.display_info(&mut never).unwrap();
    assert_eq!(screens[0], Scanout { enabled: true, width: 640, height: 480 });
    assert!(screens[1..].iter().all(|screen| *screen == Scanout::default()));
    assert_eq!(card.state.lock().unwrap().commands, [code::GET_DISPLAY_INFO]);
}

#[test]
fn a_monitor_that_comes_or_goes_is_an_event() {
    let (card, mut gpu) = setup();
    assert_eq!(gpu.events(), 0);
    card.plug(1, Some((1024, 768)));
    assert_eq!(gpu.events(), EVENT_DISPLAY);
    assert_eq!(card.interrupts(), 1, "the configuration's interrupt");
    gpu.clear_events(EVENT_DISPLAY);
    assert_eq!(gpu.events(), 0);
    let screens = gpu.display_info(&mut never).unwrap();
    assert_eq!(screens[1], Scanout { enabled: true, width: 1024, height: 768 });
    card.plug(0, None);
    assert_eq!(gpu.events(), EVENT_DISPLAY);
    assert!(!gpu.display_info(&mut never).unwrap()[0].enabled);
}

#[test]
fn reads_a_screens_monitor_data() {
    let (card, mut gpu) = setup();
    let data: Vec<u8> = (0..256u32).map(|i| (i * 7) as u8).collect();
    card.state.lock().unwrap().screens[0].edid.clone_from(&data);
    let mut out = [0u8; EDID_MAX];
    assert_eq!(gpu.edid(0, &mut out, &mut never), Ok(256));
    assert_eq!(out[..256], data[..]);
    // A screen with none, and one the card does not have.
    assert_eq!(gpu.edid(1, &mut out, &mut never), Ok(0));
    assert_eq!(gpu.edid(2, &mut out, &mut never), Err(Error::BadRequest));
    // A card that gives none is not asked.
    let (card, gpu) = setup_with(0, &[Some((640, 480))], false);
    assert_eq!(gpu.unwrap().edid(0, &mut out, &mut never), Err(Error::BadRequest));
    assert!(card.state.lock().unwrap().commands.is_empty());
}

#[test]
fn a_picture_reaches_the_screen_with_one_doorbell() {
    let (card, mut gpu) = setup();
    let memory = store(1, 640, 480);
    show(&mut gpu, 1, &memory, (640, 480), 0);
    assert_eq!(gpu.in_flight(), 5);
    assert!(card.glass(0).is_empty(), "nothing is sent until the card is told");
    gpu.finish(&mut never).unwrap();
    assert_eq!(card.glass(0), pixels(1, Rect::sized(640, 480)));
    let s = card.state.lock().unwrap();
    assert_eq!(s.doorbells, [1, 0]);
    assert_eq!(s.interrupts, [0, 0], "answered before anyone waited");
    let expected = [
        code::RESOURCE_CREATE_2D,
        code::RESOURCE_ATTACH_BACKING,
        code::TRANSFER_TO_HOST_2D,
        code::SET_SCANOUT,
        code::RESOURCE_FLUSH,
    ];
    assert_eq!(s.commands, expected);
}

#[test]
fn only_what_is_flushed_shows() {
    let (card, mut gpu) = setup();
    let memory = store(1, 640, 480);
    show(&mut gpu, 1, &memory, (640, 480), 0);
    gpu.finish(&mut never).unwrap();
    // Another picture in the same memory: a part is copied and shown.
    memory.write(0, &pixels(2, Rect::sized(640, 480)));
    let part = Rect { x: 100, y: 50, width: 200, height: 120 };
    let offset = u64::from(part.y) * 640 * 4 + u64::from(part.x) * 4;
    gpu.transfer(1, part, offset).unwrap();
    gpu.flush(1, part).unwrap();
    gpu.finish(&mut never).unwrap();
    let mut expected = pixels(1, Rect::sized(640, 480));
    paste(&mut expected, 640, part, &pixels(2, part));
    assert_eq!(card.glass(0), expected);
    // Copied and not flushed: the glass is as it was.
    gpu.transfer(1, Rect::sized(640, 480), 0).unwrap();
    gpu.finish(&mut never).unwrap();
    assert_eq!(card.glass(0), expected);
    gpu.flush(1, Rect::sized(640, 480)).unwrap();
    gpu.finish(&mut never).unwrap();
    assert_eq!(card.glass(0), pixels(2, Rect::sized(640, 480)));
}

#[test]
fn a_backing_can_be_in_pieces() {
    let (card, mut gpu) = setup();
    let whole = pixels(3, Rect::sized(640, 480));
    // Three pieces that are not one after another in memory, the middle
    // one ending inside a row.
    let cuts = [0, 100 * 2560, 300 * 2560 + 1000, whole.len()];
    let table = Dma::fake(4096);
    let mut pieces = Vec::new();
    for (index, cut) in cuts.windows(2).enumerate() {
        let piece = Dma::fake(cut[1] - cut[0]);
        piece.write(0, &whole[cut[0]..cut[1]]);
        table.write(index * 16, &entry(piece.phys(), (cut[1] - cut[0]) as u32));
        pieces.push(piece);
    }
    gpu.create(1, format::BGRX, 640, 480).unwrap();
    gpu.attach_table(1, table.phys(), 3).unwrap();
    gpu.transfer(1, Rect::sized(640, 480), 0).unwrap();
    gpu.set_scanout(0, 1, Rect::sized(640, 480)).unwrap();
    gpu.finish(&mut never).unwrap();
    assert_eq!(card.glass(0), whole);
}

#[test]
fn two_screens_show_two_parts_of_one_picture() {
    let (card, gpu) = setup_with(0, &[Some((640, 480)), Some((640, 480))], false);
    let mut gpu = gpu.unwrap();
    let memory = store(4, 1280, 480);
    let (left, right) = (Rect::sized(640, 480), Rect { x: 640, ..Rect::sized(640, 480) });
    gpu.create(1, format::BGRX, 1280, 480).unwrap();
    gpu.attach(1, memory.phys(), memory.len() as u32).unwrap();
    gpu.transfer(1, Rect::sized(1280, 480), 0).unwrap();
    gpu.set_scanout(0, 1, left).unwrap();
    gpu.set_scanout(1, 1, right).unwrap();
    gpu.finish(&mut never).unwrap();
    assert_eq!(card.glass(0), pixels(4, left));
    assert_eq!(card.glass(1), pixels(4, right));
    // One flush across the middle reaches both.
    memory.write(0, &pixels(5, Rect::sized(1280, 480)));
    let across = Rect { x: 600, y: 10, width: 80, height: 20 };
    gpu.transfer(1, across, 10 * 1280 * 4 + 600 * 4).unwrap();
    gpu.flush(1, across).unwrap();
    gpu.finish(&mut never).unwrap();
    let (mut first, mut second) = (pixels(4, left), pixels(4, right));
    let on_left = Rect { width: 40, ..across };
    paste(&mut first, 640, on_left, &pixels(5, on_left));
    let on_right = Rect { x: 640, width: 40, ..across };
    paste(&mut second, 640, Rect { x: 0, ..on_right }, &pixels(5, on_right));
    assert_eq!(card.glass(0), first);
    assert_eq!(card.glass(1), second);
    // A screen told to show nothing does.
    gpu.set_scanout(1, 0, Rect::default()).unwrap();
    gpu.finish(&mut never).unwrap();
    assert!(card.glass(1).is_empty());
}

#[test]
fn shared_pages_show_without_a_copy() {
    let (card, gpu) = setup_with(feature::RESOURCE_BLOB, &[Some((600, 400))], true);
    let mut gpu = gpu.unwrap();
    // Rows of 640 pixels' bytes for a picture 600 wide, in two pieces.
    let image = Rect::sized(640, 400);
    let (first, second) = (Dma::fake(150 * 2560), Dma::fake(250 * 2560));
    let whole = pixels(6, image);
    first.write(0, &whole[..150 * 2560]);
    second.write(0, &whole[150 * 2560..]);
    let table = Dma::fake(4096);
    table.write(0, &entry(first.phys(), first.len() as u32));
    table.write(16, &entry(second.phys(), second.len() as u32));
    let shared =
        Shared { resource: 2, format: format::BGRX, width: 600, height: 400, row_bytes: 2560 };
    let shown = Rect::sized(600, 400);
    gpu.create_shared(2, 400 * 2560, table.phys(), 2).unwrap();
    gpu.set_scanout_shared(0, &shared, shown).unwrap();
    gpu.finish(&mut never).unwrap();
    assert_eq!(card.glass(0), pixels(6, shown));
    // The program draws in its pages, and a flush is all it takes.
    let part = Rect { x: 8, y: 160, width: 32, height: 16 };
    for line in 0..16 {
        let row = pixels(7, Rect { y: part.y + line, height: 1, ..part });
        second.write((10 + line as usize) * 2560 + 32, &row);
    }
    gpu.flush(2, part).unwrap();
    gpu.finish(&mut never).unwrap();
    let mut expected = pixels(6, shown);
    paste(&mut expected, 600, part, &pixels(7, part));
    assert_eq!(card.glass(0), expected);
    let commands = card.state.lock().unwrap().commands.clone();
    assert!(!commands.contains(&code::TRANSFER_TO_HOST_2D));
}

#[test]
fn a_card_that_shares_no_pages_is_not_asked_to() {
    let (card, mut gpu) = setup();
    assert_eq!(gpu.create_shared(2, 4096, 0x1000, 1), Err(Error::BadRequest));
    assert!(card.state.lock().unwrap().commands.is_empty());
}

#[test]
fn a_resource_let_go_of_shows_no_more() {
    let (card, mut gpu) = setup();
    let memory = store(1, 640, 480);
    show(&mut gpu, 1, &memory, (640, 480), 0);
    gpu.unref(1).unwrap();
    gpu.finish(&mut never).unwrap();
    assert!(card.glass(0).is_empty());
    gpu.flush(1, Rect::sized(640, 480)).unwrap();
    assert_eq!(gpu.finish(&mut never), Err(Error::Refused(code::ERR_INVALID_RESOURCE_ID)));
    // Its number can be used again.
    show(&mut gpu, 1, &memory, (640, 480), 0);
    gpu.finish(&mut never).unwrap();
    assert_eq!(card.glass(0), pixels(1, Rect::sized(640, 480)));
}

#[test]
fn the_first_refusal_is_the_error_and_the_rest_is_carried_out() {
    let (card, mut gpu) = setup();
    let memory = store(1, 640, 480);
    gpu.flush(9, Rect::sized(16, 16)).unwrap();
    gpu.set_scanout(7, 9, Rect::sized(16, 16)).unwrap();
    show(&mut gpu, 1, &memory, (640, 480), 0);
    assert_eq!(gpu.finish(&mut never), Err(Error::Refused(code::ERR_INVALID_RESOURCE_ID)));
    assert_eq!(card.glass(0), pixels(1, Rect::sized(640, 480)));
    assert!(!gpu.failed());
    // The error was that change's alone.
    assert_eq!(gpu.finish(&mut never), Ok(()));
    gpu.flush(1, Rect::sized(640, 480)).unwrap();
    assert_eq!(gpu.finish(&mut never), Ok(()));
}

#[test]
fn what_the_card_cannot_take_is_refused_by_its_code() {
    let (_, mut gpu) = setup();
    let refused = |gpu: &mut Gpu| gpu.finish(&mut never).unwrap_err();
    gpu.create(0, format::BGRX, 64, 64).unwrap();
    assert_eq!(refused(&mut gpu), Error::Refused(code::ERR_INVALID_RESOURCE_ID));
    gpu.create(1, 99, 64, 64).unwrap();
    assert_eq!(refused(&mut gpu), Error::Refused(code::ERR_INVALID_PARAMETER));
    gpu.create(1, format::BGRX, 8192, 8192).unwrap();
    assert_eq!(refused(&mut gpu), Error::Refused(code::ERR_OUT_OF_MEMORY));
    gpu.create(1, format::BGRA, 64, 64).unwrap();
    gpu.set_scanout(5, 1, Rect::sized(64, 64)).unwrap();
    assert_eq!(refused(&mut gpu), Error::Refused(code::ERR_INVALID_SCANOUT_ID));
    gpu.set_scanout(0, 1, Rect::sized(65, 64)).unwrap();
    assert_eq!(refused(&mut gpu), Error::Refused(code::ERR_INVALID_PARAMETER));
}

#[test]
fn an_answer_cut_short_or_of_the_wrong_kind_is_a_refusal() {
    let (card, mut gpu) = setup();
    card.state.lock().unwrap().short = Some((code::RESOURCE_CREATE_2D, 8));
    gpu.create(1, format::BGRX, 64, 64).unwrap();
    assert_eq!(gpu.finish(&mut never), Err(Error::Refused(0)));
    // A whole header, and not all sixteen screens after it.
    for bytes in [8, 24, 407] {
        card.state.lock().unwrap().short = Some((code::GET_DISPLAY_INFO, bytes));
        assert_eq!(gpu.display_info(&mut never), Err(Error::Refused(0)));
    }
    let mut s = card.state.lock().unwrap();
    (s.short, s.answer_with) = (None, Some((code::GET_DISPLAY_INFO, code::OK_NODATA)));
    drop(s);
    assert_eq!(gpu.display_info(&mut never), Err(Error::Refused(code::OK_NODATA)));
    card.state.lock().unwrap().answer_with = Some((code::GET_EDID, code::ERR_UNSPEC));
    let mut out = [0u8; EDID_MAX];
    assert_eq!(gpu.edid(0, &mut out, &mut never), Err(Error::Refused(code::ERR_UNSPEC)));
    assert!(!gpu.failed());
    card.state.lock().unwrap().answer_with = None;
    assert!(gpu.display_info(&mut never).is_ok());
}

#[test]
fn answers_that_come_late_are_waited_for_with_an_interrupt() {
    let (card, mut gpu) = setup();
    let memory = store(1, 640, 480);
    card.state.lock().unwrap().hold = true;
    show(&mut gpu, 1, &memory, (640, 480), 0);
    let mut waits = 0;
    let mut wait = || {
        waits += 1;
        if waits == 2 {
            card.release();
        }
        waits < 5
    };
    assert_eq!(gpu.finish(&mut wait), Ok(()));
    assert_eq!(waits, 2);
    assert_eq!(card.glass(0), pixels(1, Rect::sized(640, 480)));
    // The card was asked to interrupt for the first answer after the
    // wait began, and not again once nobody waited.
    assert_eq!(card.interrupts(), 1);
    gpu.flush(1, Rect::sized(640, 480)).unwrap();
    gpu.finish(&mut never).unwrap();
    assert_eq!(card.interrupts(), 1);
}

#[test]
fn a_card_that_does_not_answer_is_reset() {
    let (card, mut gpu) = setup();
    card.state.lock().unwrap().silent = true;
    gpu.create(1, format::BGRX, 64, 64).unwrap();
    let mut waits = 0;
    let mut wait = || {
        waits += 1;
        waits < 3
    };
    assert_eq!(gpu.finish(&mut wait), Err(Error::NoAnswer));
    assert_eq!(waits, 3);
    assert!(gpu.failed());
    assert_eq!(card.state.lock().unwrap().status, 0, "reset");
    assert_eq!(gpu.create(2, format::BGRX, 64, 64), Err(Error::DeviceFailed));
    assert_eq!(gpu.finish(&mut never), Err(Error::DeviceFailed));
    assert_eq!(gpu.move_pointer(Place::default(), &mut never), Err(Error::DeviceFailed));
    assert_eq!(gpu.in_flight(), 0);
}

#[test]
fn a_card_that_asks_for_a_reset_is_given_up_on() {
    let (card, mut gpu) = setup();
    card.state.lock().unwrap().hold = true;
    gpu.create(1, format::BGRX, 64, 64).unwrap();
    gpu.kick();
    card.state.lock().unwrap().status |= status::NEEDS_RESET;
    assert!(gpu.failed());
    // It is not waited for.
    assert_eq!(gpu.finish(&mut never), Err(Error::NoAnswer));
}

#[test]
fn no_more_is_queued_than_fits() {
    let (card, mut gpu) = setup();
    for sent in 0..32 {
        // Room is room for the longest command, of three buffers, which
        // the queue's last two entries are not.
        assert_eq!(gpu.has_room(), sent < 31);
        gpu.flush(1, Rect::sized(16, 16)).unwrap();
    }
    assert_eq!(gpu.flush(1, Rect::sized(16, 16)), Err(Error::Full));
    assert_eq!(gpu.in_flight(), 32);
    assert!(gpu.finish(&mut never).is_err(), "there is no resource 1");
    assert_eq!(card.state.lock().unwrap().commands.len(), 32);
    // Commands with a table take three of the queue's 64 entries each.
    for _ in 0..21 {
        assert!(gpu.has_room());
        gpu.attach_table(1, 0x1000, 0).unwrap();
    }
    assert!(!gpu.has_room());
    assert_eq!(gpu.attach_table(1, 0x1000, 0), Err(Error::Full));
    assert!(gpu.finish(&mut never).is_err());
    assert!(gpu.has_room());
    assert_eq!(gpu.bad_entries(), 0);
}

/// A pointer's picture, as resource 3.
fn pointer_picture(gpu: &mut Gpu) -> Dma {
    let memory = store(8, POINTER_SIDE, POINTER_SIDE);
    let all = Rect::sized(POINTER_SIDE, POINTER_SIDE);
    gpu.create(3, format::BGRA, POINTER_SIDE, POINTER_SIDE).unwrap();
    gpu.attach(3, memory.phys(), memory.len() as u32).unwrap();
    gpu.transfer(3, all, 0).unwrap();
    gpu.finish(&mut never).unwrap();
    memory
}

#[test]
fn the_pointer_has_a_queue_of_its_own() {
    let (card, mut gpu) = setup();
    let _memory = pointer_picture(&mut gpu);
    let place = Place { scanout: 0, x: 10, y: 20 };
    gpu.set_pointer(place, 3, (4, 5), &mut never).unwrap();
    {
        let s = card.state.lock().unwrap();
        assert_eq!(s.pointer, Pointer { resource: 3, hot: (4, 5), place });
        assert_eq!(s.pointer_pixels, pixels(8, Rect::sized(POINTER_SIDE, POINTER_SIDE)));
    }
    // More moves than the queue has entries: each is sent at once, and
    // its buffer is taken back when the next is sent.
    for step in 0..40 {
        gpu.move_pointer(Place { scanout: 1, x: step, y: 2 * step }, &mut never).unwrap();
    }
    let s = card.state.lock().unwrap();
    assert_eq!(s.pointed.len(), 41);
    assert_eq!(s.pointed[0], (code::UPDATE_CURSOR, place));
    assert_eq!(s.pointed[40], (code::MOVE_CURSOR, Place { scanout: 1, x: 39, y: 78 }));
    assert_eq!(s.pointer.resource, 3, "a move keeps the picture");
    assert_eq!(s.interrupts[1], 0);
    drop(s);
    // Resource 0 hides it.
    gpu.set_pointer(place, 0, (0, 0), &mut never).unwrap();
    assert_eq!(card.state.lock().unwrap().pointer.resource, 0);
}

#[test]
fn the_pointer_moves_while_a_frame_is_still_being_shown() {
    let (card, mut gpu) = setup();
    let memory = store(1, 640, 480);
    card.state.lock().unwrap().hold = true;
    show(&mut gpu, 1, &memory, (640, 480), 0);
    gpu.kick();
    let place = Place { scanout: 0, x: 300, y: 200 };
    gpu.move_pointer(place, &mut never).unwrap();
    assert_eq!(card.state.lock().unwrap().pointer.place, place);
    assert_eq!(gpu.collect(), 5, "the frame's commands still wait");
    card.release();
    gpu.finish(&mut never).unwrap();
}

#[test]
fn a_card_that_keeps_the_pointers_commands_is_reset() {
    let (card, mut gpu) = setup();
    card.state.lock().unwrap().keeps_pointer = true;
    for step in 0..16 {
        gpu.move_pointer(Place { scanout: 0, x: step, y: 0 }, &mut never).unwrap();
    }
    let mut waits = 0;
    let mut wait = || {
        waits += 1;
        waits < 3
    };
    assert_eq!(gpu.move_pointer(Place::default(), &mut wait), Err(Error::NoAnswer));
    assert_eq!(waits, 3);
    assert!(gpu.failed());
    assert_eq!(card.state.lock().unwrap().status, 0, "reset");
}

/// A wait that only counts: a card that is polled answers meanwhile.
fn polled(waits: &mut u32) -> impl FnMut() -> bool + '_ {
    move || {
        *waits += 1;
        *waits < 100
    }
}

#[test]
fn a_card_started_again_is_polled_and_keeps_nothing() {
    let (card, mut gpu) = setup();
    let memory = store(1, 640, 480);
    show(&mut gpu, 1, &memory, (640, 480), 0);
    gpu.finish(&mut never).unwrap();
    // In the middle of a frame the card has not answered.
    card.state.lock().unwrap().hold = true;
    gpu.flush(1, Rect::sized(640, 480)).unwrap();
    gpu.kick();
    assert!(gpu.restart());
    assert!(!gpu.failed());
    assert_eq!(gpu.in_flight(), 0);
    {
        let mut s = card.state.lock().unwrap();
        assert_ne!(s.status & status::DRIVER_OK, 0);
        assert_eq!((s.config_vector, s.vectors), (NO_VECTOR, [NO_VECTOR; 2]));
        assert!(s.resources.is_empty());
        s.hold = false;
    }
    assert!(card.glass(0).is_empty());
    // The same numbers, anew, with a card that answers late and is
    // never asked to interrupt.
    card.state.lock().unwrap().hold = true;
    show(&mut gpu, 1, &memory, (640, 480), 0);
    let mut waits = 0;
    let mut wait = || {
        waits += 1;
        card.release();
        true
    };
    gpu.finish(&mut wait).unwrap();
    assert_eq!(waits, 1);
    assert_eq!(card.glass(0), pixels(1, Rect::sized(640, 480)));
    assert_eq!(card.state.lock().unwrap().interrupts, [0, 0]);
    let mut waits = 0;
    gpu.move_pointer(Place { scanout: 0, x: 1, y: 2 }, &mut polled(&mut waits)).unwrap();
    assert_eq!(waits, 0);
}

#[test]
fn a_card_that_stopped_answering_can_be_started_again() {
    let (card, mut gpu) = setup();
    card.state.lock().unwrap().silent = true;
    gpu.create(1, format::BGRX, 64, 64).unwrap();
    assert_eq!(gpu.finish(&mut || false), Err(Error::NoAnswer));
    assert!(gpu.failed());
    card.state.lock().unwrap().silent = false;
    assert!(gpu.restart());
    let memory = store(2, 640, 480);
    show(&mut gpu, 1, &memory, (640, 480), 0);
    gpu.finish(&mut never).unwrap();
    assert_eq!(card.glass(0), pixels(2, Rect::sized(640, 480)));
    // Shared pages are not for a card that reports a panic.
    assert!(!gpu.shares_pages());
}
