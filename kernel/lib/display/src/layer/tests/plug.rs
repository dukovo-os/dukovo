// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Tests of what comes and goes while the layer runs: a monitor plugged
//! in and taken out, a screen whose modes change, what a monitor says of
//! itself, and a card taken away from under the log and from under a
//! holder.

use radix_abi::display::REASON_UNPLUGGED;

use super::*;

/// What a card finds of a screen with no monitor data.
fn found(plugged: bool, modes: &[Mode]) -> Found<'_> {
    Found { plugged, modes, data: &[] }
}

/// The base block of a monitor of maker DUK with model number
/// `product`: the start, the two numbers and a sum of zero.
fn monitor_data(product: u16) -> [u8; 128] {
    let mut data = [0u8; 128];
    data[..8].copy_from_slice(&[0, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0]);
    // Five bits a letter, A being 1: D, U and K, the first byte first.
    data[8..10].copy_from_slice(&(4u16 * 1024 + 21 * 32 + 11).to_be_bytes());
    data[10..12].copy_from_slice(&product.to_le_bytes());
    let sum = data.iter().fold(0u8, |sum, byte| sum.wrapping_add(*byte));
    data[127] = sum.wrapping_neg();
    data
}

/// The entries that feed the virtio-like card's screen `index` from its
/// pipe of the same place, in mode `mode`, with `picture` on it.
fn feed(index: usize, mode: u32, picture: u32) -> [(u32, u32, u64); 5] {
    let (pipe, plane) = (V_PIPES[index], V_MAINS[index]);
    [
        (pipe, PIPE_MODE, u64::from(mode)),
        (pipe, PIPE_ACTIVE, 1),
        (V_SCREENS[index], SCREEN_PIPE, u64::from(pipe)),
        (plane, PLANE_PIPE, u64::from(pipe)),
        (plane, PLANE_PICTURE, u64::from(picture)),
    ]
}

/// Gives `holder` a picture of the virtio-like card and a mode, both of
/// `width` by `height`, and returns the entries that show them on the
/// card's screen `index`.
fn on_virtio(
    rig: &mut Rig,
    holder: u32,
    index: usize,
    (width, height): (u32, u32),
) -> [(u32, u32, u64); 5] {
    let shape = PictureShape { card: V_CARD, format: FORMAT_XRGB8888, width, height };
    let (picture, _) = rig.picture(holder, shape, 40 + index as u32);
    let mode = rig.mode(holder, mode(width, height));
    feed(index, mode, picture)
}

#[test]
fn a_monitor_that_goes_cannot_be_fed_and_one_that_comes_can() {
    let mut rig = Rig::firmware();
    let holder = rig.in_front();
    let virtio = rig.add_virtio();
    rig.events(holder);

    // The second screen's monitor is taken out.
    assert_eq!(rig.layer.screen_found(V_CARD, 1, &found(false, &[])), Ok(true));
    let told: Vec<_> = rig.events(holder).iter().map(|event| (event.kind, event.thing)).collect();
    assert_eq!(told, [(EVENT_SCREEN, V_SCREENS[1])]);
    assert!(rig.layer.take_news());
    let screen = rig.layer.screen_info(V_SCREENS[1]).unwrap();
    assert_eq!((screen.flags & SCREEN_PLUGGED, screen.modes), (0, 1), "its modes are kept");
    let entries = on_virtio(&mut rig, holder, 1, (1280, 800));
    assert_eq!(rig.change(holder, 0, 0, &entries), Err(Unmade::Refused(2, REASON_UNPLUGGED)));
    assert!(virtio.asked().is_empty());

    // Found the same again: nobody is told.
    assert_eq!(rig.layer.screen_found(V_CARD, 1, &found(false, &[])), Ok(false));
    assert!(rig.events(holder).is_empty());
    assert!(!rig.layer.take_news());

    // Another monitor comes, which prefers another mode.
    let modes = [mode(1920, 1080), mode(1280, 800)];
    assert_eq!(rig.layer.screen_found(V_CARD, 1, &found(true, &modes)), Ok(true));
    assert_eq!(rig.kinds(holder), [EVENT_SCREEN]);
    let screen = rig.layer.screen_info(V_SCREENS[1]).unwrap();
    assert_eq!((screen.flags, screen.modes), (SCREEN_PLUGGED, 2));
    assert_eq!(rig.layer.screen_modes(V_SCREENS[1]), Some(&modes[..]));
    let entries = on_virtio(&mut rig, holder, 1, (1920, 1080));
    assert_eq!(rig.change(holder, 0, 0, &entries), DONE);
    let drawn = virtio.shown();
    assert_eq!((drawn.pipe, drawn.size, drawn.screens), (1, (1920, 1080), 0b10));

    // The first screen was not touched by any of it.
    assert_eq!(rig.layer.screen_modes(V_SCREENS[0]), Some(&[mode(1280, 800)][..]));
    // A screen or a card there is not.
    assert!(rig.layer.screen_found(V_CARD, 2, &found(true, &modes)).is_err());
    assert!(rig.layer.screen_found(99, 0, &found(true, &modes)).is_err());
}

#[test]
fn a_monitor_taken_out_under_a_holder_leaves_its_state_as_asked() {
    let mut rig = Rig::firmware();
    let holder = rig.in_front();
    let virtio = rig.add_virtio();
    let entries = on_virtio(&mut rig, holder, 0, (1280, 800));
    assert_eq!(rig.change(holder, 0, 0, &entries), DONE);
    virtio.asked();
    rig.events(holder);
    assert_eq!(rig.layer.screen_found(V_CARD, 0, &found(false, &[])), Ok(true));
    // The holder is told, and the card is asked for nothing: the pipe
    // goes on as the holder asked until the holder says otherwise.
    assert_eq!(rig.kinds(holder), [EVENT_SCREEN]);
    assert!(virtio.asked().is_empty());
    assert_eq!(rig.layer.screen_info(V_SCREENS[0]).unwrap().pipe, V_PIPES[0]);
    // Which it does by stopping the pipe.
    let (pipe, plane) = (V_PIPES[0], V_MAINS[0]);
    let entries = [
        (plane, PLANE_PICTURE, 0),
        (plane, PLANE_PIPE, 0),
        (V_SCREENS[0], SCREEN_PIPE, 0),
        (pipe, PIPE_ACTIVE, 0),
    ];
    assert_eq!(rig.change(holder, MAY_BLANK, 0, &entries), DONE);
    assert!(!virtio.shown().active);
}

#[test]
fn what_a_monitor_says_of_itself_is_kept_and_read() {
    let mut rig = Rig::firmware();
    let holder = rig.holder(1);
    let data = monitor_data(0x2a17);
    let news = Found { plugged: true, modes: &[], data: &data };
    assert_eq!(rig.layer.screen_found(CARD, 0, &news), Ok(true));
    assert_eq!(rig.kinds(holder), [EVENT_SCREEN]);
    let screen = rig.layer.screen_info(SCREEN).unwrap();
    assert_eq!((screen.maker, screen.product), (*b"DUK\0", 0x2a17));
    assert_eq!(screen.monitor_data_bytes, 128);
    assert_eq!(rig.layer.monitor_data(SCREEN), Some(&data[..]));
    // With no modes found, the one it had is still its mode.
    assert_eq!(rig.layer.screen_modes(SCREEN), Some(&[mode(1024, 768)][..]));
    assert_eq!(rig.layer.screen_found(CARD, 0, &news), Ok(false));

    // Bytes that are no monitor's are kept for whoever asks, and say
    // nothing.
    let news = Found { data: &data[..100], ..news };
    assert_eq!(rig.layer.screen_found(CARD, 0, &news), Ok(true));
    let screen = rig.layer.screen_info(SCREEN).unwrap();
    assert_eq!((screen.maker, screen.product, screen.monitor_data_bytes), ([0; 4], 0, 100));
}

#[test]
fn the_log_follows_its_screens_monitor_and_mode() {
    let mut rig = Rig::bare();
    let virtio = Fake::new(true);
    let screen = ScreenSpec { plugged: false, fixed_pipe: None, native: mode(1280, 800) };
    let spec = CardSpec {
        kind: CARD_VIRTIO,
        name: name("virtio0"),
        model: name("Virtio GPU"),
        flags: CARD_SETS_MODES | CARD_MIRRORS,
        width_min: 64,
        height_min: 64,
        width_max: 8192,
        height_max: 8192,
        pipes: 2,
        screens: &[screen, screen],
        main: planes(FORMAT_XRGB8888, 8192, 8192),
        pointer: planes(FORMAT_ARGB8888, 64, 64),
    };
    assert!(rig.layer.add_card(&spec, |_| Some(virtio.clone())).is_ok());
    // The first card here: pipes 2 and 3, screens 4 and 5.
    let (pipes, screens) = ([2, 3], [4, 5]);
    // No monitor: the log has nowhere to show.
    assert!(virtio.asked().is_empty());
    assert_eq!(rig.layer.pipe_info(pipes[0]).unwrap().flags, 0);

    // A monitor on the second screen: the log shows there, on the
    // card's first pipe, in the mode the monitor prefers.
    rig.layer.set_time(3 * BEAT);
    assert_eq!(rig.layer.screen_found(1, 1, &found(true, &[mode(1920, 1080)])), Ok(true));
    assert_eq!(virtio.asked(), [Call::LogTakes(0)]);
    let pipe = rig.layer.pipe_info(pipes[0]).unwrap();
    assert_eq!((pipe.flags, pipe.mode), (PIPE_IS_ACTIVE, mode(1920, 1080)));
    assert_eq!(rig.layer.screen_info(screens[1]).unwrap().pipe, pipes[0]);

    // The host's window is made smaller: the log takes the pipe again,
    // in the new mode.
    assert_eq!(rig.layer.screen_found(1, 1, &found(true, &[mode(800, 600)])), Ok(true));
    assert_eq!(virtio.asked(), [Call::LogTakes(0)]);
    assert_eq!(rig.layer.pipe_info(pipes[0]).unwrap().mode, mode(800, 600));

    // A monitor on the first screen: the log is on the first screen
    // that has one.
    assert_eq!(rig.layer.screen_found(1, 0, &found(true, &[mode(1024, 768)])), Ok(true));
    assert_eq!(virtio.asked(), [Call::LogTakes(0)]);
    assert_eq!(rig.layer.screen_info(screens[0]).unwrap().pipe, pipes[0]);
    assert_eq!(rig.layer.screen_info(screens[1]).unwrap().pipe, 0);
    assert_eq!(rig.layer.pipe_info(pipes[0]).unwrap().mode, mode(1024, 768));

    // Both go: the pipe stops, and the log waits for a monitor.
    assert_eq!(rig.layer.screen_found(1, 0, &found(false, &[])), Ok(true));
    assert_eq!(rig.layer.screen_found(1, 1, &found(false, &[])), Ok(true));
    let asked = virtio.asked();
    let Some(Call::Show(last)) = asked.last() else { panic!("the pipe was to stop: {asked:?}") };
    assert!(!last.active);
    assert_eq!(rig.layer.pipe_info(pipes[0]).unwrap().flags, 0);

    // Under a holder, the log is not laid out: the holder is told.
    let holder = rig.holder(1);
    assert_eq!(rig.layer.screen_found(1, 0, &found(true, &[mode(1024, 768)])), Ok(true));
    assert_eq!(virtio.asked().last(), Some(&Call::LogTakes(0)));
    let shape = PictureShape { card: 1, format: FORMAT_XRGB8888, width: 1024, height: 768 };
    let (picture, _) = rig.picture(holder, shape, 7);
    // Plane 6 is the main plane of the pipe the log was on.
    assert_eq!(rig.change(holder, 0, 0, &[(6, PLANE_PICTURE, u64::from(picture))]), DONE);
    virtio.asked();
    rig.events(holder);
    assert_eq!(rig.layer.screen_found(1, 1, &found(true, &[mode(640, 480)])), Ok(true));
    assert!(virtio.asked().is_empty());
    assert_eq!(rig.kinds(holder), [EVENT_SCREEN]);
}

#[test]
fn a_failed_screen_is_tried_again_once_it_is_found_anew() {
    let mut rig = Rig::firmware();
    let holder = rig.in_front();
    rig.firmware.fails.set(true);
    let other = u64::from(rig.frame(holder, 2));
    assert_eq!(rig.change(holder, NOW, 0, &[(MAIN, PLANE_PICTURE, other)]), DONE);
    assert_ne!(rig.layer.screen_info(SCREEN).unwrap().flags & SCREEN_FAILED, 0);
    rig.firmware.fails.set(false);
    // Found as it was: still failed.
    assert_eq!(rig.layer.screen_found(CARD, 0, &found(true, &[])), Ok(false));
    assert_ne!(rig.layer.screen_info(SCREEN).unwrap().flags & SCREEN_FAILED, 0);
    // Its monitor taken out and put back: it is tried again.
    assert_eq!(rig.layer.screen_found(CARD, 0, &found(false, &[])), Ok(true));
    assert_eq!(rig.layer.screen_found(CARD, 0, &found(true, &[])), Ok(true));
    assert_eq!(rig.layer.screen_info(SCREEN).unwrap().flags, SCREEN_PLUGGED);
    rig.firmware.asked();
    assert_eq!(rig.change(holder, NOW, 0, &[(MAIN, PLANE_PICTURE, other)]), DONE);
    assert_eq!(rig.firmware.shown().main, Some(2));
}

#[test]
fn the_log_passes_to_the_next_card_when_its_card_is_taken_away() {
    let mut rig = Rig::firmware();
    rig.firmware.asked();
    // A second card that can show the log, which the first has.
    let virtio = Fake::new(true);
    let screen = ScreenSpec { plugged: true, fixed_pipe: None, native: mode(1280, 800) };
    let spec = CardSpec {
        kind: CARD_VIRTIO,
        name: name("virtio0"),
        model: name("Virtio GPU"),
        flags: CARD_SETS_MODES,
        width_min: 64,
        height_min: 64,
        width_max: 8192,
        height_max: 8192,
        pipes: 1,
        screens: &[screen],
        main: planes(FORMAT_XRGB8888, 8192, 8192),
        pointer: planes(FORMAT_ARGB8888, 64, 64),
    };
    assert!(rig.layer.add_card(&spec, |_| Some(virtio.clone())).is_ok());
    assert!(virtio.asked().is_empty() && rig.firmware.asked().is_empty());

    rig.layer.set_time(5 * BEAT);
    let gone = rig.layer.remove_card(CARD).unwrap();
    assert!(Rc::ptr_eq(&gone, &rig.firmware));
    // The first card is asked nothing more, not even to give the log
    // up; the second has the log, on its pipe.
    assert!(rig.firmware.asked().is_empty());
    assert_eq!(virtio.asked(), [Call::LogTakes(0)]);
    assert!(rig.layer.log_card().is_some_and(|card| Rc::ptr_eq(card, &virtio)));
    let mut things = Vec::new();
    rig.layer.list(&mut |thing| things.push((thing.id, thing.kind)));
    let left =
        [(6, KIND_CARD), (8, KIND_SCREEN), (7, KIND_PIPE), (9, KIND_PLANE), (10, KIND_PLANE)];
    assert_eq!(things, left);
    assert!(rig.layer.card(CARD).is_none() && rig.layer.card_info(CARD).is_none());
    let pipe = rig.layer.pipe_info(7).unwrap();
    assert_eq!(
        (pipe.flags, pipe.mode, pipe.beat_time_ns),
        (PIPE_IS_ACTIVE, mode(1280, 800), 5 * BEAT)
    );
    assert_eq!(rig.layer.screen_info(8).unwrap().pipe, 7);
    assert!(rig.layer.remove_card(CARD).is_none(), "it is gone");

    // A holder takes the front on what is left, and gives it back.
    let holder = rig.holder(1);
    let shape = PictureShape { card: 6, format: FORMAT_XRGB8888, width: 1280, height: 800 };
    let (picture, _) = rig.picture(holder, shape, 3);
    assert_eq!(rig.change(holder, 0, 0, &[(9, PLANE_PICTURE, u64::from(picture))]), DONE);
    assert_eq!(virtio.asked()[0], Call::LogGives);
    rig.layer.end(holder);
    assert_eq!(virtio.asked()[0], Call::LogTakes(0));
    // With the last card gone there is nothing, and nothing breaks.
    assert!(rig.layer.remove_card(6).is_some());
    assert_eq!(rig.layer.count(), 0);
    assert!(virtio.asked().is_empty() && rig.layer.log_card().is_none());
}

#[test]
fn a_card_taken_away_under_a_holder_leaves_the_rest_of_its_state() {
    let mut rig = Rig::firmware();
    let holder = rig.in_front();
    let watcher = rig.holder(2);
    let virtio = rig.add_virtio();
    // The holder shows on both cards, and a change of the first card's
    // pipe waits for its beat.
    let entries = on_virtio(&mut rig, holder, 1, (1280, 800));
    assert_eq!(rig.change(holder, 0, 0, &entries), DONE);
    rig.layer.set_time(MS);
    let Ok(Made::Wait(seq)) = rig.change(holder, 0, 0, &[changed(MAIN, Rect::sized(4, 4))]) else {
        panic!("the change was to wait for its beat");
    };
    rig.events(holder);
    rig.events(watcher);
    rig.layer.take_news();
    rig.firmware.asked();
    virtio.asked();

    assert!(rig.layer.remove_card(CARD).is_some());
    // The change that waited is dropped, and whoever waits for it is
    // woken; both holders are told to look again.
    assert!(!rig.layer.waits(seq) && rig.layer.take_news());
    assert_eq!(rig.kinds(holder), [EVENT_SCREEN]);
    assert_eq!(rig.kinds(watcher), [EVENT_SCREEN]);
    // A holder was in front, so the card is not told of the log, and
    // the card left shows what it showed.
    assert!(rig.firmware.asked().is_empty() && virtio.asked().is_empty());
    assert_eq!(rig.layer.front(), Some(holder));
    assert_eq!(rig.layer.count(), 1 + 2 + 2 + 4);
    assert_eq!(rig.layer.screen_info(V_SCREENS[1]).unwrap().pipe, V_PIPES[1]);
    assert_eq!(rig.layer.screen_info(V_SCREENS[0]).unwrap().pipe, 0);
    let pipe = rig.layer.pipe_info(V_PIPES[1]).unwrap();
    assert_eq!((pipe.flags, pipe.mode), (PIPE_IS_ACTIVE, mode(1280, 800)));
    assert_eq!(rig.layer.plane_info(V_MAINS[1]).unwrap().pipe, V_PIPES[1]);

    // The holder's state still fits the things that are left: a change
    // of the other screen is made, and one that names a thing of the
    // card that went is refused.
    let entries = on_virtio(&mut rig, holder, 0, (800, 600));
    assert_eq!(rig.change(holder, 0, 0, &entries), DONE);
    let drawn = virtio.shown();
    assert_eq!((drawn.pipe, drawn.size, drawn.screens), (0, (800, 600), 0b01));
    let gone = [(MAIN, PLANE_PICTURE, 0)];
    assert!(matches!(rig.change(holder, 0, 0, &gone), Err(Unmade::Refused(0, _))));
    rig.layer.set_time(MS + BEAT);
    let Ok(Made::Wait(_)) = rig.change(holder, 0, 0, &[changed(V_MAINS[1], Rect::sized(2, 2))])
    else {
        panic!("the change was to wait for its beat");
    };
    rig.layer.set_time(MS + 2 * BEAT);
    rig.layer.beat(V_CARD);
    assert_eq!(virtio.shown().damage, [Rect::sized(2, 2)]);
    // Its picture of the card that went is still its own, to drop.
    assert_eq!(rig.layer.holder(holder).unwrap().drop_object(1), Ok(()));
    // The log has no card left that shows it: the holder ends, and the
    // screens are blank.
    rig.layer.end(holder);
    assert!(virtio.frames().iter().all(|drawn| !drawn.active));
}
