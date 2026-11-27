// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Tests of the layer against stand-in cards that write down what they
//! are asked: one like the firmware's framebuffer, with one screen in
//! one mode that shows the kernel's log, and one like a virtio card,
//! with two screens it sets modes on. Time is the test's to tell, so a
//! beat comes when the test says it does.

use std::alloc::Global;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

use radix_abi::display::{
    self as abi, BLOB_MODE, CARD_FIRMWARE, CARD_MIRRORS, CARD_SETS_MODES, CARD_VIRTIO,
    EVENT_ATTENTION, EVENT_BACK, EVENT_BORROWER_ENDED, EVENT_FRONT, FORMAT_ARGB8888,
    FORMAT_XRGB8888, KIND_CARD, KIND_PIPE, KIND_PLANE, KIND_SCREEN, PIPE_ACTIVE, PIPE_IS_ACTIVE,
    PIPE_MODE, PLANE_CHANGED, PLANE_HOT_SPOT, PLANE_PIPE, REASON_BAD_VALUE, REASON_NOT_OFFERED,
    REASON_SIZE, REASON_WOULD_BLANK, SCREEN_FAILED, SCREEN_PIPE, SCREEN_PLUGGED,
};

use super::*;
use crate::frame::{Failed, PlaneSpec, ScreenSpec, name};
use crate::state::PictureShape;

mod front;
mod plug;

/// A millisecond, in the nanoseconds the layer keeps time in.
const MS: u64 = 1_000_000;
/// From one beat to the next of the stand-in cards' screens, which run
/// at 100 frames a second: beat 1 is when the pipe starts, and beat `n`
/// is `(n - 1) * BEAT` after it.
const BEAT: u64 = 10 * MS;
/// How long before a beat the stand-in cards need a change.
const LATCH: u64 = 2 * MS;

/// The firmware-like card's things, by the numbers the layer gives the
/// first card: the card, its pipe, its screen, and the pipe's main and
/// pointer planes.
const CARD: u32 = 1;
const PIPE: u32 = 2;
const SCREEN: u32 = 3;
const MAIN: u32 = 4;
const POINTER: u32 = 5;

/// The virtio-like card's, added second: the card, its two pipes, its
/// two screens, and the main plane of each pipe (the pointer plane is
/// the number after).
const V_CARD: u32 = 6;
const V_PIPES: [u32; 2] = [7, 8];
const V_SCREENS: [u32; 2] = [9, 10];
const V_MAINS: [u32; 2] = [11, 13];

fn mode(width: u32, height: u32) -> Mode {
    Mode { width, height, refresh_mhz: 100_000, ..Mode::default() }
}

/// A mode at half the rate: its beats are two of [`BEAT`] apart.
fn slow_mode(width: u32, height: u32) -> Mode {
    Mode { refresh_mhz: 50_000, ..mode(width, height) }
}

/// A stand-in picture, told apart by its tag.
struct Pixels {
    shape: PictureShape,
    tag: u32,
}

impl Picture for Pixels {
    fn shape(&self) -> PictureShape {
        self.shape
    }

    fn row_bytes(&self) -> u32 {
        self.shape.width * 4
    }
}

/// A frame as a card was asked to show it.
#[derive(Clone, Debug, PartialEq)]
struct Drawn {
    /// The pipe, by its place on the card.
    pipe: usize,
    active: bool,
    /// The mode's width and height.
    size: (u32, u32),
    screens: u32,
    /// The main plane's picture, by its tag.
    main: Option<u32>,
    /// The pointer's picture, by its tag, and where its corner is.
    pointer: Option<(u32, i32, i32)>,
    /// What the change said was new: the mode, all of the main plane,
    /// the pointer's picture, and the pointer's place alone.
    mode: bool,
    whole: bool,
    pointer_new: bool,
    moved: bool,
    damage: Vec<Rect>,
}

/// What a stand-in card was asked.
#[derive(Clone, Debug, PartialEq)]
enum Call {
    /// The log takes the pipe at this place on the card.
    LogTakes(usize),
    LogGives,
    Show(Drawn),
    /// Rows for a picture, by its tag, and the pipe that shows it.
    Put {
        picture: u32,
        rect: Rect,
        bytes: usize,
        on: Option<usize>,
    },
    /// Its beat is needed by this time.
    Wake(u64),
}

/// A stand-in card.
struct Fake {
    log: bool,
    calls: RefCell<Vec<Call>>,
    /// Set to have it confirm no change.
    fails: Cell<bool>,
    /// Set to have its own check refuse every frame with this.
    refuses: Cell<Option<PlaneFault>>,
}

impl Fake {
    fn new(log: bool) -> Rc<Fake> {
        Rc::new(Fake {
            log,
            calls: RefCell::new(Vec::new()),
            fails: Cell::new(false),
            refuses: Cell::new(None),
        })
    }

    /// What it was asked since this was last called.
    fn asked(&self) -> Vec<Call> {
        self.calls.take()
    }

    /// The frames it was asked to show since this was last called, with
    /// everything else it was asked forgotten.
    fn frames(&self) -> Vec<Drawn> {
        let shown = |call| if let Call::Show(drawn) = call { Some(drawn) } else { None };
        self.asked().into_iter().filter_map(shown).collect()
    }

    /// The one frame it was asked to show since this was last called.
    fn shown(&self) -> Drawn {
        let mut frames = self.frames();
        assert_eq!(frames.len(), 1, "one frame was to be shown: {frames:?}");
        frames.remove(0)
    }
}

impl Card<Pixels> for Rc<Fake> {
    fn shows_log(&self) -> bool {
        self.log
    }

    fn log_takes(&self, frame: &Frame<'_, Pixels>) {
        assert!(frame.active && frame.main.is_none() && frame.pointer.is_none());
        self.calls.borrow_mut().push(Call::LogTakes(frame.pipe));
    }

    fn log_gives(&self) {
        self.calls.borrow_mut().push(Call::LogGives);
    }

    fn check(&self, _frame: &Frame<'_, Pixels>) -> Result<(), PlaneFault> {
        self.refuses.get().map_or(Ok(()), Err)
    }

    fn show(&self, frame: &Frame<'_, Pixels>, change: &Change<'_>) -> Result<(), Failed> {
        if self.fails.get() {
            return Err(Failed);
        }
        self.calls.borrow_mut().push(Call::Show(Drawn {
            pipe: frame.pipe,
            active: frame.active,
            size: (frame.mode.width, frame.mode.height),
            screens: frame.screens,
            main: frame.main.as_ref().map(|main| main.picture.tag),
            pointer: frame.pointer.as_ref().map(|shown| (shown.picture.tag, shown.x, shown.y)),
            mode: change.mode,
            whole: change.main,
            pointer_new: change.pointer,
            moved: change.moved,
            damage: change.damage.to_vec(),
        }));
        Ok(())
    }

    fn put(&self, picture: &Pixels, rect: Rect, rows: &[u8], on: Option<&Frame<'_, Pixels>>) {
        let on = on.map(|frame| frame.pipe);
        let put = Call::Put { picture: picture.tag, rect, bytes: rows.len(), on };
        self.calls.borrow_mut().push(put);
    }

    fn latch_ns(&self) -> u64 {
        LATCH
    }

    fn wake_for(&self, at_ns: u64) {
        self.calls.borrow_mut().push(Call::Wake(at_ns));
    }
}

/// A layer with the firmware-like card on it.
struct Rig {
    layer: Layer<Rc<Pixels>, Rc<Fake>, Global>,
    firmware: Rc<Fake>,
}

fn planes(format: u32, width_max: u32, height_max: u32) -> PlaneSpec {
    PlaneSpec { width_max, height_max, formats: [format, 0, 0, 0, 0, 0, 0, 0] }
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

/// A change's bytes: its header, then `entries`, each a thing, a
/// property and a value.
fn change_bytes(flags: u32, cookie: u64, entries: &[(u32, u32, u64)]) -> Vec<u8> {
    let mut bytes = vec![0; abi::Change::SIZE];
    bytes[abi::Change::OFFSET_FLAGS..][..4].copy_from_slice(&flags.to_le_bytes());
    bytes[abi::Change::OFFSET_ENTRIES..][..4]
        .copy_from_slice(&(entries.len() as u32).to_le_bytes());
    bytes[abi::Change::OFFSET_COOKIE..][..8].copy_from_slice(&cookie.to_le_bytes());
    for &(thing, property, value) in entries {
        bytes.extend_from_slice(&thing.to_le_bytes());
        bytes.extend_from_slice(&property.to_le_bytes());
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    bytes
}

/// An entry that says `rect` of plane `plane`'s picture changed.
fn changed(plane: u32, rect: Rect) -> (u32, u32, u64) {
    (plane, PLANE_CHANGED, rect.pack())
}

impl Rig {
    /// A layer with no card yet.
    fn bare() -> Rig {
        Rig { layer: Layer::new_in(Global), firmware: Fake::new(true) }
    }

    /// A layer whose first card, added at time 0, is like the firmware's
    /// framebuffer: 1024 by 768, with the kernel's log on it.
    fn firmware() -> Rig {
        let mut rig = Rig::bare();
        rig.add_firmware();
        rig
    }

    /// Adds the card like the firmware's framebuffer.
    fn add_firmware(&mut self) {
        let screens = [ScreenSpec { plugged: true, fixed_pipe: Some(0), native: mode(1024, 768) }];
        let spec = CardSpec {
            kind: CARD_FIRMWARE,
            name: name("firmware0"),
            model: name("Firmware framebuffer"),
            flags: 0,
            width_min: 1024,
            height_min: 768,
            width_max: 1024,
            height_max: 768,
            pipes: 1,
            screens: &screens,
            main: planes(FORMAT_XRGB8888, 1024, 768),
            pointer: planes(FORMAT_ARGB8888, 256, 256),
        };
        assert!(self.layer.add_card(&spec, |_| Some(self.firmware.clone())).is_ok());
    }

    /// Adds a card like a virtio one, which sets modes on its two
    /// screens and shows no log.
    fn add_virtio(&mut self) -> Rc<Fake> {
        let virtio = Fake::new(false);
        let screen = ScreenSpec { plugged: true, fixed_pipe: None, native: mode(1280, 800) };
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
        assert!(self.layer.add_card(&spec, |_| Some(virtio.clone())).is_ok());
        virtio
    }

    /// A new holder.
    fn holder(&mut self, key: usize) -> u32 {
        self.layer.holder_of(key, false).unwrap()
    }

    /// Gives `holder` a picture and returns its number and the picture.
    fn picture(&mut self, holder: u32, shape: PictureShape, tag: u32) -> (u32, Rc<Pixels>) {
        let picture = Rc::new(Pixels { shape, tag });
        let number = self.layer.holder(holder).unwrap().keep_picture(picture.clone()).unwrap();
        (number, picture)
    }

    /// Gives `holder` a picture that fills the firmware-like card's
    /// screen and returns its number.
    fn frame(&mut self, holder: u32, tag: u32) -> u32 {
        let shape = PictureShape { card: CARD, format: FORMAT_XRGB8888, width: 1024, height: 768 };
        self.picture(holder, shape, tag).0
    }

    /// Gives `holder` a mode and returns its number.
    fn mode(&mut self, holder: u32, mode: Mode) -> u32 {
        self.layer.holder(holder).unwrap().keep_value(BLOB_MODE, Some(mode)).unwrap()
    }

    fn change(
        &mut self,
        holder: u32,
        flags: u32,
        cookie: u64,
        entries: &[(u32, u32, u64)],
    ) -> Result<Made, Unmade> {
        let bytes = change_bytes(flags, cookie, entries);
        let request = ChangeRequest::read(&bytes).unwrap();
        self.layer.change(holder, Asked::of(&request), &request)
    }

    /// The events `holder` has not read.
    fn events(&mut self, holder: u32) -> Vec<DisplayEvent> {
        let holder = self.layer.holder(holder).unwrap();
        core::iter::from_fn(|| holder.next_event()).collect()
    }

    /// The kinds of the events `holder` has not read.
    fn kinds(&mut self, holder: u32) -> Vec<u32> {
        self.events(holder).iter().map(|event| event.kind).collect()
    }

    /// The kind and the count of each event `holder` has not read.
    fn told(&mut self, holder: u32) -> Vec<(u32, u64)> {
        self.events(holder).iter().map(|event| (event.kind, event.count)).collect()
    }

    /// A holder that has taken the front from the log with picture 1,
    /// tagged 1, on the firmware-like card's screen. What the card was
    /// asked and what the holder was told on the way are forgotten.
    fn in_front(&mut self) -> u32 {
        let holder = self.holder(1);
        self.take_front(holder)
    }

    /// As [`Rig::in_front`], for a holder that has the attention file,
    /// as the text consoles do.
    fn console(&mut self) -> u32 {
        let holder = self.layer.holder_of(1, true).unwrap();
        self.take_front(holder)
    }

    /// Forgets what `holders` were told and the firmware-like card was
    /// asked so far.
    fn forget(&mut self, holders: &[u32]) {
        for &holder in holders {
            self.kinds(holder);
        }
        self.firmware.asked();
    }

    /// `holder` takes the front from the log, as [`Rig::in_front`] says.
    fn take_front(&mut self, holder: u32) -> u32 {
        let picture = self.frame(holder, 1);
        assert_eq!(self.change(holder, 0, 0, &[(MAIN, PLANE_PICTURE, u64::from(picture))]), DONE);
        self.firmware.asked();
        self.events(holder);
        self.layer.take_news();
        holder
    }
}

const DONE: Result<Made, Unmade> = Ok(Made::Done);

fn status(status: Status) -> Result<Made, Unmade> {
    Err(Unmade::Status(status))
}

#[test]
fn a_card_is_added_whole_with_the_log_on_its_screen() {
    let rig = Rig::firmware();
    assert_eq!(rig.firmware.asked(), [Call::LogTakes(0)]);
    assert_eq!(rig.layer.front(), None);

    let mut things = Vec::new();
    rig.layer.list(&mut |thing| things.push((thing.id, thing.kind, thing.card, thing.index)));
    let listed = [
        (CARD, KIND_CARD, CARD, 0),
        (SCREEN, KIND_SCREEN, CARD, 0),
        (PIPE, KIND_PIPE, CARD, 0),
        (MAIN, KIND_PLANE, CARD, 0),
        (POINTER, KIND_PLANE, CARD, 1),
    ];
    assert_eq!(things, listed);
    assert_eq!(rig.layer.count(), listed.len());
    assert_eq!(rig.layer.kind_of(POINTER), Some(KIND_PLANE));
    assert_eq!(rig.layer.kind_of(POINTER + 1), None);

    let card = rig.layer.card_info(CARD).unwrap();
    assert_eq!((card.kind, card.screens, card.pipes, card.planes), (CARD_FIRMWARE, 1, 1, 2));
    assert_eq!(card.name, name::<16>("firmware0"));
    assert!(rig.layer.card_info(PIPE).is_none());

    // The log's screen is fed by the card's pipe in the mode it was
    // found in, and the pipe's beat runs from when the card was added.
    let screen = rig.layer.screen_info(SCREEN).unwrap();
    assert_eq!((screen.flags, screen.pipe, screen.modes), (SCREEN_PLUGGED, PIPE, 1));
    assert_eq!(rig.layer.screen_modes(SCREEN), Some(&[mode(1024, 768)][..]));
    assert_eq!(rig.layer.monitor_data(SCREEN), Some(&[][..]));
    let pipe = rig.layer.pipe_info(PIPE).unwrap();
    assert_eq!(pipe.flags, PIPE_IS_ACTIVE);
    assert_eq!((pipe.beat_ns, pipe.beat_count, pipe.beat_time_ns), (BEAT, 1, 0));
    assert_eq!(pipe.mode, mode(1024, 768));
    let plane = rig.layer.plane_info(POINTER).unwrap();
    assert_eq!((plane.kind, plane.pipe, plane.fixed_pipe), (PLANE_POINTER, PIPE, PIPE));
    assert_eq!((plane.width_max, plane.formats[0]), (256, FORMAT_ARGB8888));
}

#[test]
fn a_card_past_what_the_layer_keeps_is_not_added() {
    let mut rig = Rig::firmware();
    let screens = [ScreenSpec { plugged: true, fixed_pipe: None, native: mode(640, 480) }];
    let spec = CardSpec {
        kind: CARD_VIRTIO,
        name: name("virtio0"),
        model: name("Virtio GPU"),
        flags: CARD_SETS_MODES,
        width_min: 64,
        height_min: 64,
        width_max: 8192,
        height_max: 8192,
        pipes: PIPES_MAX,
        screens: &screens,
        main: planes(FORMAT_XRGB8888, 8192, 8192),
        pointer: planes(FORMAT_ARGB8888, 64, 64),
    };
    // One pipe too many with the firmware card's.
    assert!(rig.layer.add_card(&spec, |_| Some(Fake::new(false))).is_err());
    // A driver that cannot be made leaves no part of its card behind.
    let fewer = CardSpec { pipes: 1, ..spec };
    assert!(rig.layer.add_card(&fewer, |_| None).is_err());
    assert_eq!(rig.layer.count(), 5);
    // The most cards are added, and no more.
    for _ in 1..CARDS_MAX {
        assert!(rig.layer.add_card(&fewer, |_| Some(Fake::new(false))).is_ok());
    }
    assert!(rig.layer.add_card(&fewer, |_| Some(Fake::new(false))).is_err());
    assert_eq!(rig.layer.count(), 5 + (CARDS_MAX - 1) * 5);
}

#[test]
fn the_first_change_takes_the_front_from_the_log() {
    let mut rig = Rig::firmware();
    rig.firmware.asked();
    let holder = rig.holder(1);
    let picture = rig.frame(holder, 1);
    // A change that only asks whether it would pass takes nothing.
    let entries = [(MAIN, PLANE_PICTURE, u64::from(picture))];
    assert_eq!(rig.change(holder, CHECK_ONLY, 0, &entries), DONE);
    assert_eq!((rig.layer.front(), rig.firmware.asked()), (None, vec![]));

    // The holder starts from what the log showed: it names its picture
    // and nothing else, and the frame is drawn at once and all over,
    // whatever the log left on the screen.
    assert_eq!(rig.change(holder, 0, 0, &entries), DONE);
    assert_eq!(rig.layer.front(), Some(holder));
    let asked = rig.firmware.asked();
    assert_eq!(asked[0], Call::LogGives);
    let drawn = Drawn {
        pipe: 0,
        active: true,
        size: (1024, 768),
        screens: 1,
        main: Some(1),
        pointer: None,
        mode: false,
        whole: true,
        pointer_new: true,
        moved: false,
        damage: vec![Rect::sized(1024, 768)],
    };
    assert_eq!(asked[1..], [Call::Show(drawn)]);
    assert_eq!(rig.kinds(holder), [EVENT_FRONT]);
    assert!(rig.layer.take_news());
    assert!(!rig.layer.take_news());
    assert_eq!(rig.layer.plane_info(MAIN).unwrap().pipe, PIPE);
}

#[test]
fn a_change_shows_at_its_pipes_next_beat() {
    let mut rig = Rig::firmware();
    let holder = rig.in_front();
    let rect = Rect { x: 10, y: 20, width: 30, height: 40 };

    // A caller that waits is given a number to wait on.
    rig.layer.set_time(3 * MS);
    let Ok(Made::Wait(seq)) = rig.change(holder, 0, 0, &[changed(MAIN, rect)]) else {
        panic!("the change was to wait for its beat");
    };
    assert_eq!(rig.firmware.asked(), [Call::Wake(BEAT)]);
    assert!(rig.layer.waits(seq));
    assert_eq!(rig.layer.next_beat(CARD), Some(BEAT));
    // Before the beat nothing is made.
    rig.layer.set_time(BEAT - 1);
    rig.layer.beat(CARD);
    assert!(rig.layer.waits(seq) && rig.firmware.asked().is_empty());
    rig.layer.set_time(BEAT);
    rig.layer.beat(CARD);
    let drawn = rig.firmware.shown();
    assert_eq!((drawn.whole, drawn.pointer_new, drawn.mode), (false, false, false));
    assert_eq!(drawn.damage, [rect]);
    assert!(!rig.layer.waits(seq) && rig.layer.take_news());
    assert_eq!((rig.layer.next_beat(CARD), rig.kinds(holder)), (None, vec![]));

    // One that does not wait is told with an event: the pipe, the
    // change's cookie, the beat it showed at and when that was, and how
    // late the next change may come and still make the beat after.
    rig.layer.set_time(BEAT + 3 * MS);
    assert_eq!(rig.change(holder, NO_WAIT, 9, &[changed(MAIN, rect)]), DONE);
    assert_eq!(rig.firmware.asked(), [Call::Wake(2 * BEAT)]);
    // The beat's thread was late by a millisecond.
    rig.layer.set_time(2 * BEAT + MS);
    rig.layer.beat(CARD);
    assert_eq!(rig.firmware.shown().damage, [rect]);
    let shown = DisplayEvent {
        kind: EVENT_SHOWN,
        thing: PIPE,
        time_ns: 2 * BEAT,
        count: 3,
        cookie: 9,
        deadline_ns: 3 * BEAT - LATCH,
    };
    assert_eq!(rig.events(holder), [shown]);
}

#[test]
fn a_pipe_takes_one_waiting_change_unless_the_newest_wins() {
    let mut rig = Rig::firmware();
    let holder = rig.in_front();
    let rects = [Rect::sized(1, 1), Rect::sized(2, 2), Rect::sized(3, 3)];
    rig.layer.set_time(MS);
    let Ok(Made::Wait(first)) = rig.change(holder, 0, 1, &[changed(MAIN, rects[0])]) else {
        panic!("the change was to wait for its beat");
    };
    assert_eq!(rig.change(holder, NO_WAIT, 2, &[changed(MAIN, rects[1])]), status(busy()));
    // The newest takes the earlier one's place and its rectangles, and
    // the caller that waited for the earlier one waits no more.
    rig.layer.take_news();
    let flags = NO_WAIT | NEWEST_WINS;
    assert_eq!(rig.change(holder, flags, 3, &[changed(MAIN, rects[2])]), DONE);
    assert!(!rig.layer.waits(first) && rig.layer.take_news());
    // And is replaced in its turn, which it is told of.
    assert_eq!(rig.change(holder, flags, 4, &[changed(MAIN, rects[1])]), DONE);
    let replaced = rig.events(holder);
    assert_eq!((replaced.len(), replaced[0].kind, replaced[0].cookie), (1, EVENT_REPLACED, 3));
    rig.firmware.asked();

    rig.layer.set_time(BEAT);
    rig.layer.beat(CARD);
    assert_eq!(rig.firmware.shown().damage, [rects[1], rects[2], rects[0]]);
    let shown = rig.events(holder);
    assert_eq!((shown.len(), shown[0].kind, shown[0].cookie), (1, EVENT_SHOWN, 4));
}

#[test]
fn a_change_that_asks_for_now_shows_at_once() {
    let mut rig = Rig::firmware();
    let holder = rig.in_front();
    let (early, late) = (Rect::sized(4, 4), Rect { x: 8, y: 8, width: 4, height: 4 });
    rig.layer.set_time(MS);
    assert_eq!(rig.change(holder, NO_WAIT, 1, &[changed(MAIN, early)]), DONE);
    rig.firmware.asked();
    // It does not wait, and what waited shows with it.
    rig.layer.set_time(2 * MS);
    let flags = NOW | NO_WAIT | NEWEST_WINS;
    assert_eq!(rig.change(holder, flags, 2, &[changed(MAIN, late)]), DONE);
    assert_eq!(rig.firmware.asked().len(), 1);
    assert_eq!(rig.layer.next_beat(CARD), None);
    let told: Vec<_> = rig.events(holder).iter().map(|event| (event.kind, event.cookie)).collect();
    assert_eq!(told, [(EVENT_REPLACED, 1), (EVENT_SHOWN, 2)]);
    // The beat finds nothing left to do.
    rig.layer.set_time(BEAT);
    rig.layer.beat(CARD);
    assert!(rig.firmware.asked().is_empty());
}

#[test]
fn rectangles_of_a_part_shown_land_where_the_part_shows() {
    let mut rig = Rig::firmware();
    let holder = rig.in_front();
    let virtio = rig.add_virtio();
    // A picture taller than the mode, of which the rows from 100 show.
    let shape = PictureShape { card: V_CARD, format: FORMAT_XRGB8888, width: 800, height: 1000 };
    let (picture, _) = rig.picture(holder, shape, 5);
    let small = u64::from(rig.mode(holder, mode(800, 600)));
    let (pipe, plane) = (V_PIPES[0], V_MAINS[0]);
    let source = Rect { x: 0, y: 100, width: 800, height: 600 };
    let entries = [
        (pipe, PIPE_MODE, small),
        (pipe, PIPE_ACTIVE, 1),
        (V_SCREENS[0], SCREEN_PIPE, u64::from(pipe)),
        (plane, PLANE_PIPE, u64::from(pipe)),
        (plane, PLANE_PICTURE, u64::from(picture)),
        (plane, abi::PLANE_SOURCE, source.pack()),
    ];
    assert_eq!(rig.change(holder, 0, 0, &entries), DONE);
    virtio.asked();
    // Rows 90 to 109 of the picture changed: the ten that show are the
    // top ten of the frame.
    let rect = Rect { x: 5, y: 90, width: 50, height: 20 };
    assert_eq!(rig.change(holder, NOW, 0, &[changed(plane, rect)]), DONE);
    assert_eq!(virtio.shown().damage, [Rect { x: 5, y: 0, width: 50, height: 10 }]);
    // Rows that do not show are no reason to draw.
    let unseen = Rect { x: 0, y: 0, width: 50, height: 50 };
    assert_eq!(rig.change(holder, NOW, 0, &[changed(plane, unseen)]), DONE);
    assert!(virtio.asked().is_empty());
    // Another part of the same picture is a new frame.
    let lower = Rect { y: 400, ..source };
    assert_eq!(rig.change(holder, NOW, 0, &[(plane, abi::PLANE_SOURCE, lower.pack())]), DONE);
    assert!(virtio.shown().whole);
}

#[test]
fn a_picture_named_again_with_no_rectangle_changed_all_over() {
    let mut rig = Rig::firmware();
    let holder = rig.in_front();
    assert_eq!(rig.change(holder, NOW, 0, &[(MAIN, PLANE_PICTURE, 1)]), DONE);
    let drawn = rig.firmware.shown();
    assert_eq!((drawn.whole, drawn.damage), (false, vec![Rect::sized(1024, 768)]));
    // Another picture is a new frame, with or without rectangles.
    let other = rig.frame(holder, 2);
    assert_eq!(rig.change(holder, NOW, 0, &[(MAIN, PLANE_PICTURE, u64::from(other))]), DONE);
    let drawn = rig.firmware.shown();
    assert_eq!((drawn.main, drawn.whole), (Some(2), true));
    // A change that changes nothing asks nothing of the card.
    assert_eq!(rig.change(holder, NOW, 0, &[(PIPE, PIPE_ACTIVE, 1)]), DONE);
    assert!(rig.firmware.asked().is_empty());
}

#[test]
fn a_refused_change_alters_nothing() {
    let mut rig = Rig::firmware();
    rig.firmware.asked();
    let holder = rig.holder(1);
    let picture = u64::from(rig.frame(holder, 1));
    // An entry that cannot be applied: which, and why.
    let entries = [(MAIN, PLANE_PICTURE, picture), (MAIN, PLANE_PICTURE, 77)];
    assert_eq!(rig.change(holder, 0, 0, &entries), Err(Unmade::Refused(1, REASON_BAD_VALUE)));
    // A state the check refuses: the last entry that set the thing.
    let shape = PictureShape { card: CARD, format: FORMAT_XRGB8888, width: 800, height: 600 };
    let (small, _) = rig.picture(holder, shape, 2);
    let entries = [(MAIN, PLANE_PICTURE, u64::from(small)), (PIPE, PIPE_ACTIVE, 1)];
    assert_eq!(rig.change(holder, 0, 0, &entries), Err(Unmade::Refused(0, REASON_SIZE)));
    // One only the card knows it cannot show: the plane the card names.
    let shape = PictureShape { card: CARD, format: FORMAT_ARGB8888, width: 32, height: 32 };
    let (arrow, _) = rig.picture(holder, shape, 3);
    let fault = PlaneFault { kind: PLANE_POINTER, reason: REASON_NOT_OFFERED };
    rig.firmware.refuses.set(Some(fault));
    let entries = [(POINTER, PLANE_PICTURE, u64::from(arrow)), (MAIN, PLANE_PICTURE, picture)];
    assert_eq!(rig.change(holder, 0, 0, &entries), Err(Unmade::Refused(0, REASON_NOT_OFFERED)));
    rig.firmware.refuses.set(None);

    // The log still has the screen, and the holder still starts from
    // what the log shows.
    assert_eq!((rig.layer.front(), rig.firmware.asked()), (None, vec![]));
    assert!(rig.kinds(holder).is_empty());
    assert_eq!(rig.change(holder, 0, 0, &[(MAIN, PLANE_PICTURE, picture)]), DONE);
    assert_eq!(rig.firmware.shown().main, Some(1));

    // In front, a refused change leaves what the holder asked for as it
    // was: its next change builds on the last that passed.
    let entries = [(MAIN, PLANE_PICTURE, u64::from(small))];
    assert_eq!(rig.change(holder, NOW, 0, &entries), Err(Unmade::Refused(0, REASON_SIZE)));
    assert_eq!(rig.change(holder, NOW, 0, &[changed(MAIN, Rect::sized(8, 8))]), DONE);
    let drawn = rig.firmware.shown();
    assert_eq!((drawn.main, drawn.damage), (Some(1), vec![Rect::sized(8, 8)]));
    // And a picture the state shows cannot be let go of.
    let held = rig.layer.holder(holder).unwrap();
    assert_eq!(held.drop_object(picture as u32), Err(busy()));
    assert_eq!(held.drop_object(small), Ok(()));
    assert_eq!(held.drop_object(small), Err(invalid()));
}

#[test]
fn the_front_is_lent_and_comes_back_with_what_its_holder_asked_for() {
    let mut rig = Rig::firmware();
    let lender = rig.in_front();
    let borrower = rig.holder(2);
    let picture = u64::from(rig.frame(borrower, 2));
    let entries = [(MAIN, PLANE_PICTURE, picture)];
    // Only the holder in front changes what shows; any may ask whether
    // a change would pass.
    assert_eq!(rig.change(borrower, NOW, 0, &entries), status(busy()));
    assert_eq!(rig.change(borrower, CHECK_ONLY, 0, &entries), DONE);
    assert_eq!(rig.layer.lend(borrower, lender), Err(busy()));
    assert_eq!(rig.layer.give_back(borrower), Err(busy()));

    // The borrower has asked for nothing yet: what shows stays until it
    // does.
    assert_eq!(rig.layer.lend(lender, borrower), Ok(()));
    assert_eq!(rig.layer.front(), Some(borrower));
    assert_eq!((rig.kinds(lender), rig.kinds(borrower)), (vec![EVENT_BACK], vec![EVENT_FRONT]));
    assert!(rig.firmware.asked().is_empty());
    assert_eq!(rig.change(lender, NOW, 0, &[changed(MAIN, Rect::sized(1, 1))]), status(busy()));
    assert_eq!(rig.layer.lend(borrower, lender), Err(invalid()));
    assert_eq!(rig.change(borrower, NOW, 0, &entries), DONE);
    assert_eq!(rig.firmware.shown().main, Some(2));

    // Given back, the lender's state is made again and it is told.
    assert_eq!(rig.layer.give_back(borrower), Ok(()));
    let drawn = rig.firmware.shown();
    assert_eq!((drawn.main, drawn.whole), (Some(1), true));
    assert_eq!((rig.kinds(borrower), rig.kinds(lender)), (vec![EVENT_BACK], vec![EVENT_FRONT]));

    // Lent again, the borrower's own state comes back, and it gives the
    // front back once more.
    assert_eq!(rig.layer.lend(lender, borrower), Ok(()));
    assert_eq!(rig.firmware.shown().main, Some(2));
    assert_eq!(rig.layer.give_back(borrower), Ok(()));
    assert_eq!(rig.firmware.shown().main, Some(1));
    assert_eq!(rig.kinds(lender), [EVENT_BACK, EVENT_FRONT]);

    // The holder at the bottom gives the front back to the log.
    assert_eq!(rig.layer.give_back(lender), Ok(()));
    assert_eq!(rig.firmware.asked(), [Call::LogTakes(0)]);
    assert_eq!((rig.layer.front(), rig.kinds(lender)), (None, vec![EVENT_BACK]));
    // And takes it again with its next change, from the state it had.
    assert_eq!(rig.change(lender, 0, 0, &[changed(MAIN, Rect::sized(1, 1))]), DONE);
    let asked = rig.firmware.asked();
    assert_eq!(asked[0], Call::LogGives);
    let Call::Show(drawn) = &asked[1] else { panic!("the frame was to be shown: {asked:?}") };
    assert_eq!((drawn.main, drawn.whole), (Some(1), true));
}

#[test]
fn the_secure_key_raises_the_attention_holder_until_it_is_done() {
    let mut rig = Rig::firmware();
    assert_eq!(rig.layer.secure_key(), None);
    // The console has the attention file and lent the front to a
    // desktop, which shows its own picture.
    let console = rig.console();
    assert_eq!(rig.layer.attention(), console);
    let desktop = rig.holder(2);
    rig.layer.lend(console, desktop).unwrap();
    let picture = u64::from(rig.frame(desktop, 2));
    assert_eq!(rig.change(desktop, NOW, 0, &[(MAIN, PLANE_PICTURE, picture)]), DONE);
    rig.forget(&[console, desktop]);

    // The key: the console's state is made again, whole, and the
    // desktop is in the back, where it changes nothing.
    assert_eq!(rig.layer.secure_key(), Some(console));
    assert_eq!((rig.layer.front(), rig.layer.raised()), (Some(console), Some(console)));
    let drawn = rig.firmware.shown();
    assert_eq!((drawn.main, drawn.whole), (Some(1), true));
    assert_eq!(rig.kinds(desktop), [EVENT_BACK]);
    // The key's event counts the front that passed, and passes back.
    assert_eq!(rig.told(console), [(EVENT_FRONT, 0), (EVENT_ATTENTION, 1)]);
    let entries = [(MAIN, PLANE_PICTURE, picture)];
    assert_eq!(rig.change(desktop, NOW, 0, &entries), status(busy()));
    // Nobody passes the front meanwhile, and only the raised holder
    // says it is done. The key again is nothing new.
    assert_eq!(rig.layer.give_back(desktop), Err(busy()));
    let third = rig.holder(3);
    assert_eq!(rig.layer.lend(console, third), Err(busy()));
    assert_eq!(rig.layer.attention_end(desktop), Err(busy()));
    assert_eq!(rig.layer.secure_key(), Some(console));
    assert!(rig.kinds(console).is_empty());

    // Done: the desktop has the front back with what it asked for.
    assert_eq!(rig.layer.attention_end(console), Ok(()));
    assert_eq!((rig.layer.front(), rig.layer.raised()), (Some(desktop), None));
    let drawn = rig.firmware.shown();
    assert_eq!((drawn.main, drawn.whole), (Some(2), true));
    assert_eq!((rig.kinds(console), rig.kinds(desktop)), (vec![EVENT_BACK], vec![EVENT_FRONT]));
    assert_eq!(rig.layer.attention_end(console), Err(busy()));
}

#[test]
fn the_secure_key_with_its_holder_in_front_already_passes_no_front() {
    let mut rig = Rig::firmware();
    let console = rig.console();
    assert_eq!(rig.layer.secure_key(), Some(console));
    assert_eq!(rig.told(console), [(EVENT_ATTENTION, 0)]);
    assert_eq!(rig.layer.attention_end(console), Ok(()));
    assert!(rig.kinds(console).is_empty());
    // Nothing was drawn anew for it.
    assert_eq!((rig.layer.front(), rig.firmware.asked()), (Some(console), vec![]));
}

#[test]
fn holders_that_end_while_the_attention_holder_is_raised() {
    let mut rig = Rig::firmware();
    let console = rig.console();
    let desktop = rig.holder(2);
    rig.layer.lend(console, desktop).unwrap();
    rig.layer.secure_key();
    rig.forget(&[console, desktop]);
    // The desktop ends behind the console: the console, its lender, is
    // told, and is simply in front once it is done.
    rig.layer.end(desktop);
    assert_eq!(rig.kinds(console), [EVENT_BORROWER_ENDED]);
    assert_eq!(rig.layer.attention_end(console), Ok(()));
    assert!(rig.kinds(console).is_empty());
    assert_eq!(rig.layer.front(), Some(console));

    // The raised console ends over a desktop: the desktop has the
    // front as if the console were done, and the attention file is free.
    let desktop = rig.holder(3);
    rig.layer.lend(console, desktop).unwrap();
    let picture = u64::from(rig.frame(desktop, 3));
    assert_eq!(rig.change(desktop, NOW, 0, &[(MAIN, PLANE_PICTURE, picture)]), DONE);
    rig.layer.secure_key();
    rig.forget(&[desktop]);
    rig.layer.end(console);
    assert_eq!((rig.layer.front(), rig.layer.raised()), (Some(desktop), None));
    assert_eq!(rig.kinds(desktop), [EVENT_FRONT]);
    assert_eq!(rig.firmware.shown().main, Some(3));
    assert_eq!((rig.layer.attention(), rig.layer.secure_key()), (0, None));
}

#[test]
fn a_holder_that_never_drew_is_raised_over_the_log_and_gives_it_back() {
    let mut rig = Rig::firmware();
    let prompt = rig.layer.holder_of(1, true).unwrap();
    rig.firmware.asked();
    assert_eq!(rig.layer.secure_key(), Some(prompt));
    assert_eq!(rig.firmware.asked(), [Call::LogGives]);
    assert_eq!(rig.told(prompt), [(EVENT_FRONT, 0), (EVENT_ATTENTION, 1)]);
    let picture = u64::from(rig.frame(prompt, 1));
    assert_eq!(rig.change(prompt, NOW, 0, &[(MAIN, PLANE_PICTURE, picture)]), DONE);
    assert_eq!(rig.firmware.shown().main, Some(1));
    assert_eq!(rig.layer.attention_end(prompt), Ok(()));
    assert_eq!(rig.layer.front(), None);
    assert_eq!(rig.firmware.asked().last(), Some(&Call::LogTakes(0)));
    assert_eq!(rig.kinds(prompt), [EVENT_BACK]);

    // Raised over the log again, it ends: its last picture stays and
    // the log waits its turn, as after any holder that ends in front.
    assert_eq!(rig.layer.secure_key(), Some(prompt));
    rig.layer.end(prompt);
    assert_eq!((rig.layer.front(), rig.layer.raised()), (None, None));
    assert!(rig.layer.keeps_last());
}

#[test]
fn what_waits_is_dropped_when_the_front_passes() {
    let mut rig = Rig::firmware();
    let lender = rig.in_front();
    let borrower = rig.holder(2);
    rig.layer.set_time(MS);
    let Ok(Made::Wait(seq)) = rig.change(lender, 0, 0, &[changed(MAIN, Rect::sized(9, 9))]) else {
        panic!("the change was to wait for its beat");
    };
    rig.layer.take_news();
    assert_eq!(rig.layer.lend(lender, borrower), Ok(()));
    assert!(!rig.layer.waits(seq) && rig.layer.take_news());
    rig.firmware.asked();
    rig.layer.set_time(BEAT);
    rig.layer.beat(CARD);
    assert!(rig.firmware.asked().is_empty());
    // The rectangles it named are part of the whole frame the lender
    // gets when the front comes back.
    assert_eq!(rig.layer.give_back(borrower), Ok(()));
    assert!(rig.firmware.shown().whole);
}

#[test]
fn a_picture_that_shows_outlives_its_holder() {
    let mut rig = Rig::firmware();
    let lender = rig.in_front();
    let borrower = rig.holder(2);
    let picture = rig.layer.holder(lender).unwrap().picture_ref(1).unwrap().clone();
    assert_eq!(rig.layer.lend(lender, borrower), Ok(()));
    // The lender ends under the borrower, which has shown nothing yet:
    // the lender's picture is still on the screen.
    rig.layer.end(lender);
    assert_eq!((rig.layer.front(), Rc::strong_count(&picture)), (Some(borrower), 2));
    assert!(rig.firmware.asked().is_empty());
    let own = u64::from(rig.frame(borrower, 2));
    assert_eq!(rig.change(borrower, NOW, 0, &[(MAIN, PLANE_PICTURE, own)]), DONE);
    assert_eq!(Rc::strong_count(&picture), 1);
    // With no holder under it, the borrower gives the front to the log.
    assert_eq!(rig.layer.give_back(borrower), Ok(()));
    assert_eq!(rig.layer.front(), None);
}

#[test]
fn the_pointer_moves_at_once_and_alone() {
    let mut rig = Rig::firmware();
    let holder = rig.in_front();
    let shape = PictureShape { card: CARD, format: FORMAT_ARGB8888, width: 32, height: 32 };
    let (arrow, _) = rig.picture(holder, shape, 7);
    // Before the pointer shows, a move is kept for when it does.
    assert_eq!(rig.layer.move_pointer(holder, POINTER, 100, 200), Ok(()));
    assert!(rig.firmware.asked().is_empty());
    let entries =
        [(POINTER, PLANE_PICTURE, u64::from(arrow)), (POINTER, PLANE_HOT_SPOT, 3 | 4 << 16)];
    assert_eq!(rig.change(holder, NOW, 0, &entries), DONE);
    let drawn = rig.firmware.shown();
    assert_eq!((drawn.pointer, drawn.pointer_new, drawn.whole), (Some((7, 100, 200)), true, false));

    // A move puts the hot spot, not the corner.
    assert_eq!(rig.layer.move_pointer(holder, POINTER, 100, 200), Ok(()));
    let drawn = rig.firmware.shown();
    assert_eq!(drawn.pointer, Some((7, 97, 196)));
    assert_eq!(
        (drawn.moved, drawn.pointer_new, drawn.whole, drawn.mode),
        (true, false, false, false)
    );
    assert!(drawn.damage.is_empty());
    // Over the edge of the frame it hangs over.
    assert_eq!(rig.layer.move_pointer(holder, POINTER, 0, 0), Ok(()));
    assert_eq!(rig.firmware.shown().pointer, Some((7, -3, -4)));
    // A change that names rectangles of the pointer's picture draws the
    // pointer again where it is.
    let again = [changed(POINTER, Rect::sized(32, 32))];
    assert_eq!(rig.change(holder, NOW, 0, &again), DONE);
    let drawn = rig.firmware.shown();
    assert_eq!((drawn.pointer, drawn.pointer_new, drawn.moved), (Some((7, -3, -4)), true, false));

    assert_eq!(rig.layer.move_pointer(holder, MAIN, 1, 1), Err(invalid()));
    let other = rig.holder(2);
    assert_eq!(rig.layer.move_pointer(other, POINTER, 1, 1), Err(busy()));
    rig.firmware.fails.set(true);
    assert_eq!(rig.layer.move_pointer(holder, POINTER, 5, 5), Err(File::Io.status(0)));
}

#[test]
fn rows_are_put_for_the_holder_in_front() {
    let mut rig = Rig::firmware();
    let holder = rig.in_front();
    let shape = PictureShape { card: CARD, format: FORMAT_XRGB8888, width: 1024, height: 768 };
    let (kept, pixels) = rig.picture(holder, shape, 4);
    let rect = Rect { x: 0, y: 16, width: 8, height: 2 };
    let rows = [0u8; 8 * 4 * 2];
    // While no plane shows it, the card is given the rows and no frame.
    assert_eq!(rig.layer.put(holder, &pixels, rect, &rows), Ok(()));
    let put = |on| Call::Put { picture: 4, rect, bytes: rows.len(), on };
    assert_eq!(rig.firmware.asked(), [put(None)]);
    assert_eq!(rig.change(holder, NOW, 0, &[(MAIN, PLANE_PICTURE, u64::from(kept))]), DONE);
    rig.firmware.asked();
    assert_eq!(rig.layer.put(holder, &pixels, rect, &rows), Ok(()));
    assert_eq!(rig.firmware.asked(), [put(Some(0))]);
    // Behind another holder, its rows go nowhere.
    let borrower = rig.holder(2);
    assert_eq!(rig.layer.lend(holder, borrower), Ok(()));
    assert_eq!(rig.layer.put(holder, &pixels, rect, &rows), Err(busy()));
    assert!(rig.firmware.asked().is_empty());
}

#[test]
fn a_holder_is_told_of_the_beat_it_asked_for() {
    let mut rig = Rig::firmware();
    let holder = rig.in_front();
    rig.layer.set_time(MS);
    assert_eq!(rig.layer.ask_beat(holder, PIPE), Ok(()));
    // Asked twice before the beat, it is told once.
    assert_eq!(rig.layer.ask_beat(holder, PIPE), Ok(()));
    assert_eq!(rig.firmware.asked(), [Call::Wake(BEAT), Call::Wake(BEAT)]);
    assert_eq!(rig.layer.next_beat(CARD), Some(BEAT));
    rig.layer.set_time(BEAT);
    rig.layer.beat(CARD);
    let beat = DisplayEvent {
        kind: EVENT_BEAT,
        thing: PIPE,
        time_ns: BEAT,
        count: 2,
        cookie: 0,
        deadline_ns: 2 * BEAT - LATCH,
    };
    assert_eq!(rig.events(holder), [beat]);
    assert!(rig.layer.take_news());
    // And of that beat alone.
    rig.layer.set_time(2 * BEAT);
    rig.layer.beat(CARD);
    assert_eq!((rig.kinds(holder), rig.layer.next_beat(CARD)), (vec![], None));

    // A holder behind another may ask too; one that ends is forgotten.
    let other = rig.holder(2);
    assert_eq!(rig.layer.ask_beat(other, PIPE), Ok(()));
    rig.layer.end(other);
    assert_eq!(rig.layer.next_beat(CARD), None);
    assert_eq!(rig.layer.ask_beat(holder, SCREEN), Err(invalid()));
    // A pipe that makes no frames has no beat.
    rig.add_virtio();
    assert_eq!(rig.layer.ask_beat(holder, V_PIPES[0]), Err(invalid()));
}

#[test]
fn the_log_has_the_screens_as_the_machine_ends() {
    let mut rig = Rig::firmware();
    let holder = rig.in_front();
    rig.layer.close();
    assert_eq!(rig.firmware.asked(), [Call::LogTakes(0)]);
    assert_eq!((rig.layer.front(), rig.kinds(holder)), (None, vec![EVENT_BACK]));
    // No holder takes them again.
    assert_eq!(rig.change(holder, 0, 0, &[changed(MAIN, Rect::sized(1, 1))]), status(busy()));
    let late = rig.holder(2);
    let picture = u64::from(rig.frame(late, 2));
    assert_eq!(rig.change(late, 0, 0, &[(MAIN, PLANE_PICTURE, picture)]), status(busy()));
    assert!(rig.firmware.asked().is_empty());
}

#[test]
fn a_holder_is_found_by_its_key_and_attention_has_one() {
    let mut rig = Rig::firmware();
    let first = rig.layer.holder_of(100, true).unwrap();
    assert_eq!(rig.layer.holder_of(100, true), Ok(first));
    assert_eq!(rig.layer.holder_by_key(100), Some(first));
    assert_eq!(rig.layer.holder_of(200, true), Err(busy()));
    assert_eq!(rig.layer.holder_by_key(200), None);
    rig.layer.end(first);
    let second = rig.layer.holder_of(200, true).unwrap();
    assert!(second != 0 && second != first);
    // As many holders as the layer keeps, and no more.
    for key in 0..HOLDERS_MAX - 1 {
        assert!(rig.layer.holder_of(key, false).is_ok());
    }
    assert_eq!(rig.layer.holder_of(HOLDERS_MAX, false), Err(Kernel::TooMany.status(0)));
    assert_eq!(rig.change(first, 0, 0, &[]), status(Kernel::Gone.status(0)));
}

#[test]
fn a_card_added_later_joins_what_the_holders_keep() {
    let mut rig = Rig::firmware();
    let holder = rig.in_front();
    rig.layer.set_time(BEAT / 2);
    let virtio = rig.add_virtio();
    assert_eq!(rig.kinds(holder), [EVENT_SCREEN]);
    assert_eq!(rig.layer.count(), 5 + 1 + 2 + 2 + 4);
    assert_eq!(rig.layer.card_info(V_CARD).unwrap().planes, 4);
    // Nothing shows on it, and it is asked for nothing, until a holder
    // says so.
    assert!(virtio.asked().is_empty());
    assert_eq!(rig.layer.pipe_info(V_PIPES[1]).unwrap().flags, 0);

    // The second screen on the second pipe, in a mode of the holder's.
    let shape = PictureShape { card: V_CARD, format: FORMAT_XRGB8888, width: 800, height: 600 };
    let (picture, _) = rig.picture(holder, shape, 5);
    let small = u64::from(rig.mode(holder, mode(800, 600)));
    let (pipe, plane) = (V_PIPES[1], V_MAINS[1]);
    let entries = [
        (pipe, PIPE_MODE, small),
        (pipe, PIPE_ACTIVE, 1),
        (V_SCREENS[1], SCREEN_PIPE, u64::from(pipe)),
        (plane, PLANE_PIPE, u64::from(pipe)),
        (plane, PLANE_PICTURE, u64::from(picture)),
    ];
    // A new mode does not wait for a beat: the pipe has none yet. Its
    // first beat is when the mode is set.
    let start = BEAT / 2;
    assert_eq!(rig.change(holder, 0, 0, &entries), DONE);
    let drawn = virtio.shown();
    assert_eq!((drawn.pipe, drawn.active, drawn.size, drawn.screens), (1, true, (800, 600), 0b10));
    assert_eq!((drawn.mode, drawn.main), (true, Some(5)));
    // The firmware card's frame did not change.
    assert!(rig.firmware.asked().is_empty());
    let info = rig.layer.pipe_info(pipe).unwrap();
    assert_eq!((info.flags, info.beat_count, info.beat_time_ns), (PIPE_IS_ACTIVE, 1, start));
    assert_eq!(rig.layer.screen_info(V_SCREENS[1]).unwrap().pipe, pipe);

    // Another mode would blank the screen for a moment, which a change
    // must allow. Beat 4 of the old mode was at the moment of the new
    // one, whose first is beat 5: a pipe's numbers only grow.
    let reset = start + 3 * BEAT;
    rig.layer.set_time(reset);
    let shape = PictureShape { card: V_CARD, format: FORMAT_XRGB8888, width: 1280, height: 800 };
    let (picture, _) = rig.picture(holder, shape, 6);
    let large = u64::from(rig.mode(holder, slow_mode(1280, 800)));
    let entries = [(pipe, PIPE_MODE, large), (plane, PLANE_PICTURE, u64::from(picture))];
    assert_eq!(rig.change(holder, 0, 0, &entries), Err(Unmade::Refused(0, REASON_WOULD_BLANK)));
    assert_eq!(rig.change(holder, MAY_BLANK | NO_WAIT, 8, &entries), DONE);
    let drawn = virtio.shown();
    assert_eq!((drawn.size, drawn.mode, drawn.main), ((1280, 800), true, Some(6)));
    let info = rig.layer.pipe_info(pipe).unwrap();
    assert_eq!((info.beat_ns, info.beat_count, info.beat_time_ns), (2 * BEAT, 5, reset));
    assert_eq!(info.mode, slow_mode(1280, 800));
    let told = rig.events(holder);
    assert_eq!(
        (told.len(), told[0].kind, told[0].thing, told[0].cookie),
        (1, EVENT_SHOWN, pipe, 8)
    );
    assert_eq!((told[0].count, told[0].time_ns), (5, reset));

    // Each card's beats are its own.
    rig.layer.set_time(reset + MS);
    let Ok(Made::Wait(_)) = rig.change(holder, 0, 0, &[changed(plane, Rect::sized(2, 2))]) else {
        panic!("the change was to wait for its beat");
    };
    assert_eq!(rig.layer.next_beat(CARD), None);
    assert_eq!(rig.layer.next_beat(V_CARD), Some(reset + 2 * BEAT));
    rig.layer.set_time(reset + 2 * BEAT);
    rig.layer.beat(CARD);
    assert!(virtio.frames().is_empty());
    rig.layer.beat(V_CARD);
    assert_eq!(virtio.shown().damage, [Rect::sized(2, 2)]);

    // Stopping the pipe blanks its screen; the count is kept for when
    // it starts again.
    let entries = [
        (plane, PLANE_PICTURE, 0),
        (plane, PLANE_PIPE, 0),
        (V_SCREENS[1], SCREEN_PIPE, 0),
        (pipe, PIPE_ACTIVE, 0),
    ];
    assert_eq!(rig.change(holder, MAY_BLANK, 0, &entries), DONE);
    let drawn = virtio.shown();
    assert_eq!((drawn.active, drawn.screens, drawn.main, drawn.mode), (false, 0, None, true));
    let info = rig.layer.pipe_info(pipe).unwrap();
    assert_eq!((info.flags, info.beat_ns, info.beat_count), (0, 0, 6));
    // A mode at the same rate as the one that showed keeps the beat it
    // has: the kernel's clock keeps it, and nothing of it changed.
    let entries = [
        (pipe, PIPE_MODE, small),
        (pipe, PIPE_ACTIVE, 1),
        (V_SCREENS[1], SCREEN_PIPE, u64::from(pipe)),
    ];
    assert_eq!(rig.change(holder, 0, 0, &entries), DONE);
    let entries = [(pipe, PIPE_MODE, u64::from(rig.mode(holder, mode(1024, 768))))];
    rig.layer.set_time(reset + 5 * BEAT);
    assert_eq!(rig.change(holder, MAY_BLANK, 0, &entries), DONE);
    let info = rig.layer.pipe_info(pipe).unwrap();
    assert_eq!((info.beat_ns, info.beat_count, info.beat_time_ns), (BEAT, 10, reset + 5 * BEAT));
}

#[test]
fn a_log_card_added_under_a_holder_waits_its_turn() {
    let mut rig = Rig::bare();
    let virtio = rig.add_virtio();
    // With no card for the log, a holder takes the front all the same.
    // The virtio-like card is the first here: its things are numbered
    // five lower than the constants say.
    let (pipe, screen, plane) = (V_PIPES[0] - 5, V_SCREENS[0] - 5, V_MAINS[0] - 5);
    let holder = rig.holder(1);
    let shape = PictureShape { card: 1, format: FORMAT_XRGB8888, width: 1280, height: 800 };
    let (picture, _) = rig.picture(holder, shape, 1);
    let native = u64::from(rig.mode(holder, mode(1280, 800)));
    let entries = [
        (pipe, PIPE_MODE, native),
        (pipe, PIPE_ACTIVE, 1),
        (screen, SCREEN_PIPE, u64::from(pipe)),
        (plane, PLANE_PIPE, u64::from(pipe)),
        (plane, PLANE_PICTURE, u64::from(picture)),
    ];
    assert_eq!(rig.change(holder, 0, 0, &entries), DONE);
    // Only the pipe that is to make frames is asked for one.
    let drawn = virtio.shown();
    assert_eq!((drawn.pipe, drawn.active, drawn.main), (0, true, Some(1)));

    // The log's card comes while the holder is in front: the log does
    // not draw on it until the holder gives the front back.
    rig.add_firmware();
    assert_eq!(rig.firmware.asked(), [Call::LogGives]);
    assert_eq!(rig.layer.give_back(holder), Ok(()));
    assert_eq!(rig.firmware.asked(), [Call::LogTakes(0)]);
    // And the pipe the holder used shows nothing.
    let drawn = virtio.shown();
    assert_eq!((drawn.pipe, drawn.active, drawn.main, drawn.mode), (0, false, None, true));
}

#[test]
fn a_card_that_does_not_answer_has_its_screens_marked_failed() {
    let mut rig = Rig::firmware();
    let holder = rig.in_front();
    let watcher = rig.holder(2);
    rig.firmware.fails.set(true);
    let other = u64::from(rig.frame(holder, 2));
    assert_eq!(rig.change(holder, NOW | NO_WAIT, 5, &[(MAIN, PLANE_PICTURE, other)]), DONE);
    // Every holder is told of the screen, and the one that asked that
    // its change did not show.
    let told: Vec<_> = rig.events(holder).iter().map(|event| (event.kind, event.thing)).collect();
    assert_eq!(told, [(EVENT_SCREEN, SCREEN), (EVENT_FAILED, PIPE)]);
    assert_eq!(rig.kinds(watcher), [EVENT_SCREEN]);
    assert_ne!(rig.layer.screen_info(SCREEN).unwrap().flags & SCREEN_FAILED, 0);
    // What shows is what showed before.
    assert_eq!(rig.layer.holder(holder).unwrap().drop_object(1), Ok(()));
    rig.firmware.fails.set(false);
    assert_eq!(rig.layer.give_back(holder), Ok(()));
    assert_eq!(rig.firmware.asked(), [Call::LogTakes(0)]);
}

#[test]
fn a_change_reads_what_it_asks_from_its_flags() {
    let bytes = change_bytes(CHECK_ONLY | NOW, 77, &[]);
    let asked = Asked::of(&ChangeRequest::read(&bytes).unwrap());
    let expected = Asked {
        check_only: true,
        may_blank: false,
        no_wait: false,
        now: true,
        newest_wins: false,
        cookie: 77,
    };
    assert_eq!(asked, expected);
    let bytes = change_bytes(MAY_BLANK | NO_WAIT | NEWEST_WINS, 0, &[]);
    let asked = Asked::of(&ChangeRequest::read(&bytes).unwrap());
    assert!(asked.may_blank && asked.no_wait && asked.newest_wins && !asked.now);
}
