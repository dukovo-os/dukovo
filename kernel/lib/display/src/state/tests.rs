// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Tests of the model against two made-up cards: one like the firmware's
//! framebuffer, with one screen in one mode, and one like a virtio card,
//! with four screens it sets modes on and mirrors.

use radix_abi::display::{
    BLOB_MODE, CARD_HOT_PLUG, Change, FORMAT_ARGB8888, FORMAT_XRGB8888, PLANE_POINTER,
};

use super::*;

/// The firmware-like card's things: card 1, screen 2, pipe 3, main plane
/// 4 and pointer plane 5.
const FIRMWARE: u32 = 1;
/// The virtio-like card: card 10, screens 11 to 14, pipes 21 to 24, main
/// planes 31 to 34 and pointer planes 41 to 44.
const VIRTIO: u32 = 10;

fn mode(width: u32, height: u32) -> Mode {
    Mode { width, height, refresh_mhz: 60_000, ..Mode::default() }
}

struct Hardware {
    cards: Vec<Card>,
    screens: Vec<Screen>,
    pipes: Vec<Pipe>,
    planes: Vec<Plane>,
}

impl Hardware {
    fn both() -> Hardware {
        let formats = |format: u32| [format, 0, 0, 0, 0, 0, 0, 0];
        let mut hardware = Hardware {
            cards: vec![
                Card {
                    id: FIRMWARE,
                    flags: 0,
                    width_min: 1024,
                    height_min: 768,
                    width_max: 1024,
                    height_max: 768,
                },
                Card {
                    id: VIRTIO,
                    flags: CARD_HOT_PLUG | CARD_MIRRORS | CARD_SETS_MODES,
                    width_min: 64,
                    height_min: 64,
                    width_max: 16384,
                    height_max: 16384,
                },
            ],
            screens: vec![Screen {
                id: 2,
                card: FIRMWARE,
                plugged: true,
                failed: false,
                fixed_pipe: 3,
                native: mode(1024, 768),
            }],
            pipes: vec![Pipe {
                id: 3,
                card: FIRMWARE,
                color_table_entries: 0,
                variable_refresh: false,
            }],
            planes: vec![
                Plane {
                    id: 4,
                    card: FIRMWARE,
                    kind: PLANE_MAIN,
                    fixed_pipe: 3,
                    width_max: 1024,
                    height_max: 768,
                    formats: formats(FORMAT_XRGB8888),
                    scales: false,
                },
                Plane {
                    id: 5,
                    card: FIRMWARE,
                    kind: PLANE_POINTER,
                    fixed_pipe: 3,
                    width_max: 256,
                    height_max: 256,
                    formats: formats(FORMAT_ARGB8888),
                    scales: false,
                },
            ],
        };
        for index in 0..4 {
            hardware.screens.push(Screen {
                id: 11 + index,
                card: VIRTIO,
                plugged: index < 2,
                failed: false,
                fixed_pipe: 0,
                native: mode(1280, 800),
            });
            hardware.pipes.push(Pipe {
                id: 21 + index,
                card: VIRTIO,
                color_table_entries: 0,
                variable_refresh: false,
            });
            hardware.planes.push(Plane {
                id: 31 + index,
                card: VIRTIO,
                kind: PLANE_MAIN,
                fixed_pipe: 21 + index,
                width_max: 16384,
                height_max: 16384,
                formats: formats(FORMAT_XRGB8888),
                scales: false,
            });
            hardware.planes.push(Plane {
                id: 41 + index,
                card: VIRTIO,
                kind: PLANE_POINTER,
                fixed_pipe: 21 + index,
                width_max: 64,
                height_max: 64,
                formats: formats(FORMAT_ARGB8888),
                scales: false,
            });
        }
        hardware
    }

    fn things(&self) -> Things<'_> {
        Things {
            cards: &self.cards,
            screens: &self.screens,
            pipes: &self.pipes,
            planes: &self.planes,
        }
    }
}

/// A holder's pictures and modes.
struct Held {
    pictures: Vec<(u32, PictureShape)>,
    modes: Vec<(u32, Mode)>,
}

impl Held {
    /// Pictures 1 to 4: a frame and a pointer for each card. Modes 101
    /// (the firmware's), 102 (1280 by 800) and 103 (1920 by 1080).
    fn usual() -> Held {
        let picture = |card, format, width, height| PictureShape { card, format, width, height };
        Held {
            pictures: vec![
                (1, picture(FIRMWARE, FORMAT_XRGB8888, 1024, 768)),
                (2, picture(FIRMWARE, FORMAT_ARGB8888, 64, 64)),
                (3, picture(VIRTIO, FORMAT_XRGB8888, 1280, 800)),
                (4, picture(VIRTIO, FORMAT_ARGB8888, 64, 64)),
                (5, picture(VIRTIO, FORMAT_XRGB8888, 1920, 1080)),
            ],
            modes: vec![(101, mode(1024, 768)), (102, mode(1280, 800)), (103, mode(1920, 1080))],
        }
    }
}

impl Values for Held {
    fn picture(&self, number: u32) -> Option<PictureShape> {
        self.pictures.iter().find(|(held, _)| *held == number).map(|(_, shape)| *shape)
    }

    fn mode(&self, number: u32) -> Option<Mode> {
        self.modes.iter().find(|(held, _)| *held == number).map(|(_, mode)| *mode)
    }

    fn blob_kind(&self, number: u32) -> Option<u32> {
        self.mode(number).map(|_| BLOB_MODE)
    }
}

/// A state's storage, an element for each thing of `hardware`.
struct Stored {
    screens: Vec<ScreenState>,
    pipes: Vec<PipeState>,
    planes: Vec<PlaneState>,
}

impl Stored {
    fn blank(hardware: &Hardware) -> Stored {
        Stored {
            screens: hardware.screens.iter().map(|screen| ScreenState::new(screen.id)).collect(),
            pipes: hardware.pipes.iter().map(|pipe| PipeState::new(pipe.id)).collect(),
            planes: hardware.planes.iter().map(|plane| PlaneState::new(plane.id)).collect(),
        }
    }

    fn state(&mut self) -> State<'_> {
        State { screens: &mut self.screens, pipes: &mut self.pipes, planes: &mut self.planes }
    }

    fn view(&self) -> View<'_> {
        View { screens: &self.screens, pipes: &self.pipes, planes: &self.planes }
    }

    /// Applies `entries`, each a thing, a property and a value, and
    /// panics if one is refused.
    fn set(&mut self, held: &Held, entries: &[(u32, u32, u64)]) {
        for &(thing, property, value) in entries {
            apply(held, &mut self.state(), &Entry { thing, property, value })
                .unwrap_or_else(|reason| panic!("{thing}.{property} = {value}: reason {reason}"));
        }
    }
}

/// The entries that show picture 1 on the firmware card's one screen.
const CONSOLE: [(u32, u32, u64); 5] = [
    (3, PIPE_MODE, 101),
    (3, PIPE_ACTIVE, 1),
    (2, SCREEN_PIPE, 3),
    (4, PLANE_PIPE, 3),
    (4, PLANE_PICTURE, 1),
];

fn refusal(thing: u32, reason: u32) -> Result<(), Refusal> {
    Err(Refusal { thing, reason })
}

/// Checks `wanted` as a first change, with nothing showing.
fn check_fresh(hardware: &Hardware, held: &Held, wanted: &Stored) -> Result<(), Refusal> {
    let shown = Stored::blank(hardware);
    check(&hardware.things(), held, &shown.view(), &wanted.view(), false)
}

#[test]
fn an_entry_for_no_thing_or_no_property_is_refused() {
    let (hardware, held) = (Hardware::both(), Held::usual());
    let mut stored = Stored::blank(&hardware);
    let mut state = stored.state();
    let entry = |thing, property, value| Entry { thing, property, value };
    assert_eq!(apply(&held, &mut state, &entry(99, PIPE_ACTIVE, 1)), Err(REASON_NO_THING));
    // A card is a thing, but it has no property.
    assert_eq!(apply(&held, &mut state, &entry(FIRMWARE, PIPE_ACTIVE, 1)), Err(REASON_NO_THING));
    assert_eq!(apply(&held, &mut state, &entry(3, SCREEN_PIPE, 3)), Err(REASON_NO_PROPERTY));
    assert_eq!(apply(&held, &mut state, &entry(2, PLANE_PICTURE, 1)), Err(REASON_NO_PROPERTY));
    assert_eq!(apply(&held, &mut state, &entry(4, PIPE_MODE, 101)), Err(REASON_NO_PROPERTY));
}

#[test]
fn a_value_the_property_cannot_take_is_refused() {
    let (hardware, held) = (Hardware::both(), Held::usual());
    let mut stored = Stored::blank(&hardware);
    let mut state = stored.state();
    let bad = [
        (3, PIPE_ACTIVE, 2),
        (3, PIPE_MODE, 7),
        (3, PIPE_MODE, 1 << 32),
        (3, PIPE_VARIABLE_REFRESH, 9),
        (3, PIPE_COLOR_TABLE, 101),
        (2, SCREEN_PIPE, 99),
        (2, SCREEN_DEPTH, 9),
        (2, SCREEN_HDR_DETAILS, 101),
        (4, PLANE_PIPE, 4),
        (4, PLANE_PICTURE, 77),
        (4, PLANE_ORDER, 1 << 32),
        (4, PLANE_OPACITY, 0x1_0000),
        (4, PLANE_TURN, 45),
        (5, PLANE_HOT_SPOT, 1 << 32),
        (4, PLANE_CHANGED, Rect::sized(0, 16).pack()),
    ];
    for (thing, property, value) in bad {
        let entry = Entry { thing, property, value };
        assert_eq!(apply(&held, &mut state, &entry), Err(REASON_BAD_VALUE), "{entry:?}");
    }
    assert_eq!(stored.view().pipes, Stored::blank(&hardware).view().pipes);
}

#[test]
fn a_mode_of_no_size_or_no_rate_is_refused() {
    let hardware = Hardware::both();
    let mut held = Held::usual();
    held.modes.push((104, Mode { refresh_mhz: 0, ..mode(800, 600) }));
    held.modes.push((105, mode(0, 600)));
    let mut stored = Stored::blank(&hardware);
    for blob in [104, 105] {
        let entry = Entry { thing: 3, property: PIPE_MODE, value: blob };
        assert_eq!(apply(&held, &mut stored.state(), &entry), Err(REASON_BAD_VALUE));
    }
}

#[test]
fn a_changed_rectangle_is_reported_and_alters_nothing() {
    let (hardware, held) = (Hardware::both(), Held::usual());
    let mut stored = Stored::blank(&hardware);
    let rect = Rect { x: 8, y: 16, width: 64, height: 32 };
    let entry = Entry { thing: 4, property: PLANE_CHANGED, value: rect.pack() };
    assert_eq!(apply(&held, &mut stored.state(), &entry), Ok(Applied::Changed { plane: 4, rect }));
    assert_eq!(stored.view().planes, Stored::blank(&hardware).view().planes);
}

#[test]
fn a_console_on_the_firmware_card_passes() {
    let (hardware, held) = (Hardware::both(), Held::usual());
    let mut wanted = Stored::blank(&hardware);
    wanted.set(&held, &CONSOLE);
    assert_eq!(check_fresh(&hardware, &held, &wanted), Ok(()));
    // With a pointer, placed half off the screen's corner.
    let place = Rect { x: -32, y: -32, width: 0, height: 0 }.pack();
    wanted.set(&held, &[(5, PLANE_PIPE, 3), (5, PLANE_PICTURE, 2), (5, PLANE_PLACE, place)]);
    assert_eq!(check_fresh(&hardware, &held, &wanted), Ok(()));
}

#[test]
fn a_card_that_sets_no_modes_takes_only_the_one_it_was_found_in() {
    let (hardware, mut held) = (Hardware::both(), Held::usual());
    held.modes.push((106, Mode { refresh_mhz: 75_000, ..mode(1024, 768) }));
    for blob in [102, 106] {
        let mut wanted = Stored::blank(&hardware);
        wanted.set(&held, &[(3, PIPE_MODE, blob), (3, PIPE_ACTIVE, 1), (2, SCREEN_PIPE, 3)]);
        assert_eq!(check_fresh(&hardware, &held, &wanted), refusal(2, REASON_MODE));
    }
}

#[test]
fn a_card_that_sets_modes_takes_any_within_its_limits() {
    let (hardware, mut held) = (Hardware::both(), Held::usual());
    held.modes.push((107, mode(32, 600)));
    held.modes.push((108, Mode { refresh_mhz: 2_000_000, ..mode(800, 600) }));
    let with = |blob| {
        let mut wanted = Stored::blank(&hardware);
        wanted.set(&held, &[(21, PIPE_MODE, blob), (21, PIPE_ACTIVE, 1), (11, SCREEN_PIPE, 21)]);
        check_fresh(&hardware, &held, &wanted)
    };
    assert_eq!(with(103), Ok(()));
    assert_eq!(with(107), refusal(11, REASON_MODE));
    assert_eq!(with(108), refusal(11, REASON_MODE));
}

#[test]
fn a_pipe_that_makes_frames_needs_a_mode_and_a_screen() {
    let (hardware, held) = (Hardware::both(), Held::usual());
    let mut wanted = Stored::blank(&hardware);
    wanted.set(&held, &[(3, PIPE_ACTIVE, 1), (2, SCREEN_PIPE, 3)]);
    assert_eq!(check_fresh(&hardware, &held, &wanted), refusal(3, REASON_INCOMPLETE));
    let mut wanted = Stored::blank(&hardware);
    wanted.set(&held, &[(3, PIPE_ACTIVE, 1), (3, PIPE_MODE, 101)]);
    assert_eq!(check_fresh(&hardware, &held, &wanted), refusal(3, REASON_INCOMPLETE));
    // A screen on a pipe that makes none.
    let mut wanted = Stored::blank(&hardware);
    wanted.set(&held, &[(3, PIPE_MODE, 101), (2, SCREEN_PIPE, 3)]);
    assert_eq!(check_fresh(&hardware, &held, &wanted), refusal(2, REASON_INCOMPLETE));
}

#[test]
fn an_unplugged_or_failed_screen_cannot_be_fed() {
    let (mut hardware, held) = (Hardware::both(), Held::usual());
    let entries = [(21, PIPE_MODE, 102), (21, PIPE_ACTIVE, 1), (13, SCREEN_PIPE, 21)];
    let mut wanted = Stored::blank(&hardware);
    wanted.set(&held, &entries);
    assert_eq!(check_fresh(&hardware, &held, &wanted), refusal(13, REASON_UNPLUGGED));
    hardware.screens[3].plugged = true;
    assert_eq!(check_fresh(&hardware, &held, &wanted), Ok(()));
    hardware.screens[3].failed = true;
    assert_eq!(check_fresh(&hardware, &held, &wanted), refusal(13, REASON_UNPLUGGED));
}

#[test]
fn a_mirror_needs_a_card_that_mirrors() {
    let (mut hardware, held) = (Hardware::both(), Held::usual());
    let mut wanted = Stored::blank(&hardware);
    wanted.set(
        &held,
        &[
            (21, PIPE_MODE, 102),
            (21, PIPE_ACTIVE, 1),
            (11, SCREEN_PIPE, 21),
            (12, SCREEN_PIPE, 21),
            (31, PLANE_PICTURE, 3),
            (31, PLANE_PIPE, 21),
        ],
    );
    assert_eq!(check_fresh(&hardware, &held, &wanted), Ok(()));
    hardware.cards[1].flags &= !CARD_MIRRORS;
    assert_eq!(check_fresh(&hardware, &held, &wanted), refusal(11, REASON_NOT_OFFERED));
}

#[test]
fn things_of_two_cards_do_not_mix() {
    let (hardware, held) = (Hardware::both(), Held::usual());
    // The firmware's screen on a virtio pipe.
    let mut wanted = Stored::blank(&hardware);
    wanted.set(&held, &[(21, PIPE_MODE, 101), (21, PIPE_ACTIVE, 1), (2, SCREEN_PIPE, 21)]);
    assert_eq!(check_fresh(&hardware, &held, &wanted), refusal(2, REASON_OTHER_CARD));
    // A virtio plane on the firmware's pipe.
    let mut wanted = Stored::blank(&hardware);
    wanted.set(&held, &CONSOLE);
    wanted.set(&held, &[(31, PLANE_PIPE, 3)]);
    assert_eq!(check_fresh(&hardware, &held, &wanted), refusal(31, REASON_OTHER_CARD));
    // A virtio picture on the firmware's plane.
    let mut wanted = Stored::blank(&hardware);
    wanted.set(&held, &CONSOLE);
    wanted.set(&held, &[(4, PLANE_PICTURE, 3)]);
    assert_eq!(check_fresh(&hardware, &held, &wanted), refusal(4, REASON_OTHER_CARD));
}

#[test]
fn a_thing_fixed_to_a_pipe_goes_on_no_other() {
    let (hardware, held) = (Hardware::both(), Held::usual());
    let mut wanted = Stored::blank(&hardware);
    wanted.set(
        &held,
        &[(21, PIPE_MODE, 102), (21, PIPE_ACTIVE, 1), (11, SCREEN_PIPE, 21), (32, PLANE_PIPE, 21)],
    );
    assert_eq!(check_fresh(&hardware, &held, &wanted), refusal(32, REASON_NOT_OFFERED));
}

#[test]
fn a_plane_takes_only_its_formats_and_sizes() {
    let (hardware, held) = (Hardware::both(), Held::usual());
    let with = |entries: &[(u32, u32, u64)]| {
        let mut wanted = Stored::blank(&hardware);
        wanted.set(&held, &CONSOLE);
        wanted.set(&held, &[(5, PLANE_PIPE, 3)]);
        wanted.set(&held, entries);
        check_fresh(&hardware, &held, &wanted)
    };
    assert_eq!(with(&[]), Ok(()));
    // The pointer's picture on the main plane, and the frame on the
    // pointer's.
    assert_eq!(with(&[(4, PLANE_PICTURE, 2)]), refusal(4, REASON_FORMAT));
    assert_eq!(with(&[(5, PLANE_PICTURE, 1)]), refusal(5, REASON_FORMAT));
    // A part of the frame does not cover the mode.
    let half = Rect::sized(512, 768).pack();
    assert_eq!(with(&[(4, PLANE_SOURCE, half)]), refusal(4, REASON_SIZE));
    // Nor does the whole frame moved aside, or shown at another size.
    let aside = Rect { x: 8, y: 0, width: 1024, height: 768 }.pack();
    assert_eq!(with(&[(4, PLANE_PLACE, aside)]), refusal(4, REASON_SIZE));
    let small = Rect::sized(512, 384).pack();
    assert_eq!(with(&[(4, PLANE_PLACE, small)]), refusal(4, REASON_SIZE));
    // A part that reaches past the picture.
    let past = Rect { x: 32, y: 0, width: 64, height: 64 }.pack();
    assert_eq!(with(&[(5, PLANE_PICTURE, 2), (5, PLANE_SOURCE, past)]), refusal(5, REASON_SIZE));
}

#[test]
fn a_picture_larger_than_its_plane_takes_is_refused() {
    let (hardware, mut held) = (Hardware::both(), Held::usual());
    held.pictures
        .push((6, PictureShape { card: VIRTIO, format: FORMAT_ARGB8888, width: 128, height: 128 }));
    let mut wanted = Stored::blank(&hardware);
    wanted.set(
        &held,
        &[
            (21, PIPE_MODE, 102),
            (21, PIPE_ACTIVE, 1),
            (11, SCREEN_PIPE, 21),
            (41, PLANE_PIPE, 21),
            (41, PLANE_PICTURE, 6),
        ],
    );
    assert_eq!(check_fresh(&hardware, &held, &wanted), refusal(41, REASON_SIZE));
}

#[test]
fn what_no_card_of_this_release_does_is_refused_by_name() {
    let (hardware, held) = (Hardware::both(), Held::usual());
    let with = |entries: &[(u32, u32, u64)]| {
        let mut wanted = Stored::blank(&hardware);
        wanted.set(&held, &CONSOLE);
        wanted.set(&held, entries);
        check_fresh(&hardware, &held, &wanted)
    };
    assert_eq!(with(&[(3, PIPE_VARIABLE_REFRESH, 1)]), refusal(3, REASON_NOT_OFFERED));
    assert_eq!(with(&[(2, SCREEN_DEPTH, 10)]), refusal(2, REASON_NOT_OFFERED));
    assert_eq!(with(&[(4, PLANE_OPACITY, 0x8000)]), refusal(4, REASON_NOT_OFFERED));
    assert_eq!(with(&[(4, PLANE_TURN, 90)]), refusal(4, REASON_NOT_OFFERED));
}

#[test]
fn a_new_mode_needs_leave_to_blank_and_a_new_picture_does_not() {
    let (hardware, held) = (Hardware::both(), Held::usual());
    let mut shown = Stored::blank(&hardware);
    let start = [
        (21, PIPE_MODE, 102),
        (21, PIPE_ACTIVE, 1),
        (11, SCREEN_PIPE, 21),
        (31, PLANE_PIPE, 21),
        (31, PLANE_PICTURE, 3),
    ];
    shown.set(&held, &start);
    let things = hardware.things();
    let go = |entries: &[(u32, u32, u64)], may_blank: bool| {
        let mut wanted = Stored::blank(&hardware);
        wanted.set(&held, &start);
        wanted.set(&held, entries);
        check(&things, &held, &shown.view(), &wanted.view(), may_blank)
    };
    // The same state again, and a picture alone, blank nothing.
    assert_eq!(go(&[], false), Ok(()));
    assert_eq!(go(&[(31, PLANE_PICTURE, 0)], false), Ok(()));
    // A second screen coming on blanks nothing either.
    let second = [(22, PIPE_MODE, 102), (22, PIPE_ACTIVE, 1), (12, SCREEN_PIPE, 22)];
    assert_eq!(go(&second, false), Ok(()));
    // A new mode does, and so does stopping the pipe or moving the
    // screen to another.
    let bigger = [(21, PIPE_MODE, 103), (31, PLANE_PICTURE, 5)];
    assert_eq!(go(&bigger, false), refusal(21, REASON_WOULD_BLANK));
    assert_eq!(go(&bigger, true), Ok(()));
    let stop = [(21, PIPE_ACTIVE, 0), (11, SCREEN_PIPE, 0), (31, PLANE_PICTURE, 0)];
    assert_eq!(go(&stop, false), refusal(21, REASON_WOULD_BLANK));
    assert_eq!(go(&stop, true), Ok(()));
    let moved =
        [(12, SCREEN_PIPE, 21), (11, SCREEN_PIPE, 22), (22, PIPE_MODE, 102), (22, PIPE_ACTIVE, 1)];
    assert_eq!(go(&moved, false), refusal(11, REASON_WOULD_BLANK));
    assert_eq!(go(&moved, true), Ok(()));
}

#[test]
fn one_change_over_two_cards_is_checked_as_one() {
    let (hardware, held) = (Hardware::both(), Held::usual());
    let mut wanted = Stored::blank(&hardware);
    wanted.set(&held, &CONSOLE);
    wanted.set(
        &held,
        &[
            (21, PIPE_MODE, 102),
            (21, PIPE_ACTIVE, 1),
            (11, SCREEN_PIPE, 21),
            (31, PLANE_PIPE, 21),
            (31, PLANE_PICTURE, 3),
        ],
    );
    assert_eq!(check_fresh(&hardware, &held, &wanted), Ok(()));
    // A fault on the second card refuses the whole.
    wanted.set(&held, &[(31, PLANE_PICTURE, 5)]);
    assert_eq!(check_fresh(&hardware, &held, &wanted), refusal(31, REASON_SIZE));
}

#[test]
fn the_entry_named_is_the_last_that_set_the_thing() {
    let mut bytes = vec![0; Change::SIZE];
    let entries = [(3u32, PIPE_MODE, 101u64), (4, PLANE_PICTURE, 1), (3, PIPE_ACTIVE, 1)];
    bytes[Change::OFFSET_ENTRIES..][..4].copy_from_slice(&3u32.to_le_bytes());
    for (thing, property, value) in entries {
        bytes.extend_from_slice(&thing.to_le_bytes());
        bytes.extend_from_slice(&property.to_le_bytes());
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    let request = ChangeRequest::read(&bytes).unwrap();
    assert_eq!(entry_of(&request, 3), 2);
    assert_eq!(entry_of(&request, 4), 1);
    // A thing no entry names: one past the last.
    assert_eq!(entry_of(&request, 2), 3);
}

#[test]
fn a_first_change_starts_from_what_shows_without_its_pictures() {
    let (hardware, held) = (Hardware::both(), Held::usual());
    let mut shown = Stored::blank(&hardware);
    shown.set(&held, &CONSOLE);
    shown.set(&held, &[(4, PLANE_SOURCE, Rect::sized(1024, 768).pack())]);
    let mut mine = Stored::blank(&hardware);
    mine.state().adopt(&shown.view());
    let view = mine.view();
    assert_eq!(view.pipe(3), shown.view().pipe(3));
    assert_eq!(view.screen(2).unwrap().pipe, 3);
    assert_eq!(view.plane(4), Some(&PlaneState { pipe: 3, ..PlaneState::new(4) }));
    assert!(!view.shows(1));
    // One entry then shows the newcomer's own picture, and no mode is
    // set.
    let mut other = Held::usual();
    other.pictures[0].0 = 9;
    mine.set(&other, &[(4, PLANE_PICTURE, 9)]);
    let things = hardware.things();
    assert_eq!(check(&things, &other, &shown.view(), &mine.view(), false), Ok(()));
}

#[test]
fn a_state_knows_the_pictures_and_values_it_uses() {
    let (hardware, held) = (Hardware::both(), Held::usual());
    let mut wanted = Stored::blank(&hardware);
    wanted.set(&held, &CONSOLE);
    assert!(wanted.view().shows(1) && !wanted.view().shows(2));
    assert!(!wanted.view().uses(101));
    wanted.pipes[0].color_table = 55;
    assert!(wanted.view().uses(55));
}
