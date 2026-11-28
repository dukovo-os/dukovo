// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! When a program's frame showed (presentation-time), and frames that
//! need not wait for a beat (`wp_tearing_control_v1`): ADR 0728.

use window_test::{Client, Heard};

use super::direct::fills;
use super::{Program, World};

/// The time from one beat to the next of the stand-in card's screens,
/// which show 60 frames a second.
const BEAT_NS: u32 = 16_666_666;

/// Has `program` draw its window anew in one `shade` of grey and ask
/// when that shows, and returns what it is told of it: the beat's
/// number, whether the frame showed at a beat, and whether it showed
/// from the program's own pixels. Nothing, for a frame that never
/// showed.
fn asks_when(world: &mut World, program: &Program, shade: u8) -> Option<(u64, bool, bool)> {
    world
        .ask(program, move |client| {
            client.ask_shown();
            client.repaint(shade)
        })
        .unwrap();
    let told = |heard: &Heard| matches!(heard, Heard::Shown { .. } | Heard::NotShown);
    let heard = world.heard(program, told);
    world.settle();
    match heard.last() {
        Some(Heard::Shown { beat_ns, beat, at_beat, from_own }) => {
            assert_eq!(*beat_ns, BEAT_NS);
            Some((*beat, *at_beat, *from_own))
        }
        _ => None,
    }
}

#[test]
fn a_program_that_asks_is_told_the_beat_its_frame_showed_at() {
    let mut world = World::one_screen();
    let program = world.program();
    world.show(&program, (320, 200), 5);
    let (first, at_beat, from_own) = asks_when(&mut world, &program, 0x40).unwrap();
    assert!(at_beat && !from_own);
    let (second, ..) = asks_when(&mut world, &program, 0x80).unwrap();
    assert!(second > first, "{first} then {second}");
    // A commit that changes nothing is told of too, at the next beat.
    world.ask(&program, |client| {
        client.ask_shown();
        client.ask_frame().unwrap();
    });
    let heard = world.heard(&program, |heard| matches!(heard, Heard::Shown { .. }));
    let Some(Heard::Shown { beat, at_beat: true, from_own: false, .. }) = heard.last() else {
        panic!("{heard:?}");
    };
    assert!(*beat > second);
}

#[test]
fn a_frame_that_shows_from_the_program_s_own_pixels_is_told_as_one() {
    let mut world = World::one_screen();
    let program = world.program();
    fills(&mut world, &program, 6);
    let (_, at_beat, from_own) = asks_when(&mut world, &program, 0x40).unwrap();
    assert!(at_beat && from_own);
}

#[test]
fn a_frame_the_card_does_not_show_is_told_as_never_shown() {
    let mut world = World::one_screen();
    let program = world.program();
    world.show(&program, (320, 200), 5);
    world.card.fails.set(true);
    assert_eq!(asks_when(&mut world, &program, 0x40), None);
}

#[test]
fn a_full_screen_window_whose_frames_need_not_wait_has_them_shown_at_once() {
    let mut world = World::one_screen();
    let program = world.program();
    fills(&mut world, &program, 6);
    assert_eq!(world.card.at_once.get(), 0);
    // The hint holds from the commit after it.
    world.ask(&program, |client| client.tear(true)).unwrap();
    let (_, at_beat, from_own) = asks_when(&mut world, &program, 0x40).unwrap();
    assert!(!at_beat && from_own);
    assert_eq!(world.card.at_once.get(), 1);
    assert_eq!(world.colour(700, 300), [0x40; 3]);
    let (_, at_beat, _) = asks_when(&mut world, &program, 0x80).unwrap();
    assert!(!at_beat);
    assert_eq!(world.card.at_once.get(), 2);
    // And waits again when its program says so.
    world.ask(&program, |client| client.tear(false)).unwrap();
    let (_, at_beat, from_own) = asks_when(&mut world, &program, 0xc0).unwrap();
    assert!(at_beat && from_own);
    assert_eq!(world.card.at_once.get(), 2);
}

#[test]
fn a_window_among_others_waits_for_the_beat_whatever_its_program_says() {
    let mut world = World::one_screen();
    let program = world.program();
    world.show(&program, (320, 200), 5);
    world.ask(&program, |client| client.tear(true)).unwrap();
    let (_, at_beat, from_own) = asks_when(&mut world, &program, 0x40).unwrap();
    assert!(at_beat && !from_own);
    assert_eq!(world.card.at_once.get(), 0);
    // A second object for the same surface is the program's mistake.
    let second = world.ask(&program, Client::tear_again);
    assert!(second.is_err(), "{second:?}");
}
