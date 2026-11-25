// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! What the layer keeps (ADR 0700's bars): for a holder at the back,
//! its pictures and under 4 KiB for everything else; for a card of
//! sixteen screens, under 1 KiB for each screen, pipe and plane.
//! Measured with an allocator that counts the bytes it has out, which
//! the library's own tests cannot have: the library forbids unsafe code.
//! The second test also makes one change across the sixteen screens,
//! and says how long the check of it takes.

use std::alloc::{AllocError, Allocator, Global, Layout};
use std::cell::Cell;
use std::ptr::NonNull;
use std::rc::Rc;
use std::time::{Duration, Instant};

use radix_abi::display::{
    BLOB_MODE, CARD_FIRMWARE, CARD_HOT_PLUG, CARD_SETS_MODES, CARD_VIRTIO, CHECK_ONLY,
    Change as ChangeHeader, FORMAT_ARGB8888, FORMAT_XRGB8888, KIND_PIPE, KIND_PLANE, KIND_SCREEN,
    MAY_BLANK, Mode, NOW, PIPE_ACTIVE, PIPE_IS_ACTIVE, PIPE_MODE, PLANE_MAIN, PLANE_PICTURE,
    PLANE_PIPE, PlaneInfo, SCREEN_PIPE,
};
use radix_display::Rect;
use radix_display::frame::{
    Card, CardSpec, Change, Failed, Frame, Picture, PlaneFault, PlaneSpec, ScreenSpec, name,
};
use radix_display::holder::Holder;
use radix_display::layer::{Asked, Layer, Made};
use radix_display::state::PictureShape;
use radix_display::wire::ChangeRequest;

/// An allocator that counts the bytes it has out.
#[derive(Clone, Copy)]
struct Counted<'a>(&'a Cell<usize>);

// SAFETY: every block is the global allocator's, which each call is
// passed on to; the count kept beside them changes nothing of them.
#[allow(unsafe_code)]
unsafe impl Allocator for Counted<'_> {
    fn allocate(&self, layout: Layout) -> Result<NonNull<[u8]>, AllocError> {
        let block = Global.allocate(layout)?;
        self.0.set(self.0.get() + layout.size());
        Ok(block)
    }

    unsafe fn deallocate(&self, block: NonNull<u8>, layout: Layout) {
        self.0.set(self.0.get() - layout.size());
        // SAFETY: what the caller promises this allocator is what the
        // global one asks: `block` came from `allocate` with `layout`.
        unsafe { Global.deallocate(block, layout) };
    }
}

/// A stand-in picture: its pixels are not the layer's to keep.
struct Pixels(PictureShape);

impl Picture for Pixels {
    fn shape(&self) -> PictureShape {
        self.0
    }

    fn row_bytes(&self) -> u32 {
        self.0.width * 4
    }
}

/// A stand-in card that shows whatever it is asked.
#[derive(Clone)]
struct Blank;

impl Card<Pixels> for Blank {
    fn shows_log(&self) -> bool {
        true
    }

    fn log_takes(&self, _frame: &Frame<'_, Pixels>) {}

    fn log_gives(&self) {}

    fn check(&self, _frame: &Frame<'_, Pixels>) -> Result<(), PlaneFault> {
        Ok(())
    }

    fn show(&self, _frame: &Frame<'_, Pixels>, _change: &Change<'_>) -> Result<(), Failed> {
        Ok(())
    }

    fn put(&self, _picture: &Pixels, _rect: Rect, _rows: &[u8], _on: Option<&Frame<'_, Pixels>>) {}

    fn wake_for(&self, _at_ns: u64) {}
}

type Counting<'a> = Layer<Rc<Pixels>, Blank, Counted<'a>>;

const WIDTH: u32 = 1920;
const HEIGHT: u32 = 1080;

/// Adds a card of four screens, each fed by a pipe of its own, and
/// returns the number of its first pipe's main plane: a machine with
/// more monitors than most have.
fn add_card(layer: &mut Counting<'_>) -> u32 {
    let native = Mode { width: WIDTH, height: HEIGHT, refresh_mhz: 60_000, ..Mode::default() };
    let screens: Vec<ScreenSpec> =
        (0..4).map(|pipe| ScreenSpec { plugged: true, fixed_pipe: Some(pipe), native }).collect();
    let planes = |format: u32, side: u32| PlaneSpec {
        width_max: side,
        height_max: side,
        formats: [format, 0, 0, 0, 0, 0, 0, 0],
    };
    let spec = CardSpec {
        kind: CARD_FIRMWARE,
        name: name("card0"),
        model: name("A stand-in"),
        flags: 0,
        width_min: WIDTH,
        height_min: HEIGHT,
        width_max: WIDTH,
        height_max: HEIGHT,
        pipes: screens.len(),
        screens: &screens,
        main: planes(FORMAT_XRGB8888, 8192),
        pointer: planes(FORMAT_ARGB8888, 256),
    };
    assert!(layer.add_card(&spec, |_| Some(Blank)).is_ok());
    // The card is thing 1, its pipes 2 to 5, its screens 6 to 9, and the
    // planes follow: the first pipe's main plane, then its pointer's.
    10
}

/// The bytes of a change of `flags` with `entries`: a thing, one of its
/// properties and a value each.
fn change_bytes(flags: u32, entries: &[(u32, u32, u64)]) -> Vec<u8> {
    let mut bytes = vec![0; ChangeHeader::SIZE];
    bytes[ChangeHeader::OFFSET_FLAGS..][..4].copy_from_slice(&flags.to_le_bytes());
    let count = entries.len() as u32;
    bytes[ChangeHeader::OFFSET_ENTRIES..][..4].copy_from_slice(&count.to_le_bytes());
    for &(thing, property, value) in entries {
        bytes.extend_from_slice(&thing.to_le_bytes());
        bytes.extend_from_slice(&property.to_le_bytes());
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    bytes
}

/// Has `holder` show its picture `picture` on plane `plane`, at once.
fn show(layer: &mut Counting<'_>, holder: u32, plane: u32, picture: u32) {
    let bytes = change_bytes(NOW, &[(plane, PLANE_PICTURE, u64::from(picture))]);
    let asked = ChangeRequest::read(&bytes).unwrap();
    assert_eq!(layer.change(holder, Asked::of(&asked), &asked), Ok(Made::Done));
}

/// How many screens a virtio card can have.
const SIXTEEN: usize = 16;

/// Adds a card as a virtio one with every screen plugged in is: sixteen
/// screens, a pipe for each that can feed any of them, and modes the
/// holder sets.
fn add_sixteen(layer: &mut Counting<'_>) {
    let native = Mode { width: WIDTH, height: HEIGHT, refresh_mhz: 60_000, ..Mode::default() };
    let screens = [ScreenSpec { plugged: true, fixed_pipe: None, native }; SIXTEEN];
    let planes = |format: u32, side: u32| PlaneSpec {
        width_max: side,
        height_max: side,
        formats: [format, 0, 0, 0, 0, 0, 0, 0],
    };
    let spec = CardSpec {
        kind: CARD_VIRTIO,
        name: name("virtio0"),
        model: name("A stand-in"),
        flags: CARD_HOT_PLUG | CARD_SETS_MODES,
        width_min: 32,
        height_min: 32,
        width_max: 8192,
        height_max: 8192,
        pipes: SIXTEEN,
        screens: &screens,
        main: planes(FORMAT_XRGB8888, 8192),
        pointer: planes(FORMAT_ARGB8888, 64),
    };
    assert!(layer.add_card(&spec, |_| Some(Blank)).is_ok());
}

/// The numbers of the layer's things of `kind`, in LIST's order.
fn things(layer: &Counting<'_>, kind: u32) -> Vec<u32> {
    let mut found = Vec::new();
    layer.list(&mut |thing| {
        if thing.kind == kind {
            found.push(thing.id);
        }
    });
    found
}

#[test]
fn a_holder_at_the_back_costs_under_4_kib_and_its_pictures() {
    let out = Cell::new(0);
    let mut layer: Counting<'_> = Layer::new_in(Counted(&out));
    let main = add_card(&mut layer);
    let shape = PictureShape { card: 1, format: FORMAT_XRGB8888, width: WIDTH, height: HEIGHT };
    let picture = || Rc::new(Pixels(shape));
    // The borrower has had the front before, and gave it back to the
    // kernel's log: its own state and the layer's working room are there
    // already, so that what is counted from here is the lender's alone.
    let borrower = layer.holder_of(2, false).unwrap();
    let theirs = layer.holder(borrower).unwrap().keep_picture(picture()).unwrap();
    show(&mut layer, borrower, main, theirs);
    assert_eq!(layer.give_back(borrower), Ok(()));
    let before = out.get();

    // A holder with eight pictures and a mode takes the front, draws,
    // and lends the front on.
    let lender = layer.holder_of(1, false).unwrap();
    let holder = layer.holder(lender).unwrap();
    let own: Vec<u32> = (0..8).map(|_| holder.keep_picture(picture()).unwrap()).collect();
    let native = Mode { width: WIDTH, height: HEIGHT, refresh_mhz: 60_000, ..Mode::default() };
    holder.keep_value(BLOB_MODE, Some(native)).unwrap();
    show(&mut layer, lender, main, own[0]);
    assert_eq!(layer.lend(lender, borrower), Ok(()));
    assert_eq!(layer.front(), Some(borrower));

    // At the back it keeps the state it asked for, its numbers, the
    // events it has not read, and its place in the layer's list.
    let kept = out.get() - before + size_of::<Holder<Rc<Pixels>, Counted<'_>>>();
    assert!(kept > size_of::<Holder<Rc<Pixels>, Counted<'_>>>(), "the count counts");
    assert!(kept < 4096, "a holder at the back keeps {kept} bytes");
}

#[test]
fn sixteen_screens_cost_under_1_kib_a_thing_and_change_in_one_step() {
    let out = Cell::new(0);
    let mut layer: Counting<'_> = Layer::new_in(Counted(&out));
    let before = out.get();
    add_sixteen(&mut layer);
    let (screens, pipes) = (things(&layer, KIND_SCREEN), things(&layer, KIND_PIPE));
    let planes = things(&layer, KIND_PLANE);
    assert_eq!((screens.len(), pipes.len(), planes.len()), (SIXTEEN, SIXTEEN, 2 * SIXTEEN));
    // What it took from the allocator, and the layer itself.
    let kept = out.get() - before + size_of::<Counting<'_>>();
    let each = kept / (screens.len() + pipes.len() + planes.len());
    println!("the layer keeps {kept} bytes for sixteen screens, their pipes and planes");
    assert!(out.get() > before, "the count counts");
    assert!(each < 1024, "the layer keeps {each} bytes a screen, pipe and plane");

    // One change shows a picture of its own on every screen: the screen
    // fed by a pipe, the pipe in a mode, its main plane with a picture.
    let main_of = |pipe: u32| {
        let of_pipe = |info: PlaneInfo| info.kind == PLANE_MAIN && info.fixed_pipe == pipe;
        let found = planes.iter().find(|&&plane| layer.plane_info(plane).is_some_and(of_pipe));
        *found.expect("every pipe has a main plane")
    };
    let mains: Vec<u32> = pipes.iter().map(|&pipe| main_of(pipe)).collect();
    let shape = PictureShape { card: 1, format: FORMAT_XRGB8888, width: WIDTH, height: HEIGHT };
    let native = Mode { width: WIDTH, height: HEIGHT, refresh_mhz: 60_000, ..Mode::default() };
    let me = layer.holder_of(1, false).unwrap();
    let holder = layer.holder(me).unwrap();
    let mode = holder.keep_value(BLOB_MODE, Some(native)).unwrap();
    let mut entries = Vec::new();
    for ((&screen, &pipe), &main) in screens.iter().zip(&pipes).zip(&mains) {
        let picture = holder.keep_picture(Rc::new(Pixels(shape))).unwrap();
        entries.extend([
            (screen, SCREEN_PIPE, u64::from(pipe)),
            (pipe, PIPE_MODE, u64::from(mode)),
            (pipe, PIPE_ACTIVE, 1),
            (main, PLANE_PIPE, u64::from(pipe)),
            (main, PLANE_PICTURE, u64::from(picture)),
        ]);
    }
    let active = |layer: &Counting<'_>| {
        let runs = |&&pipe: &&u32| layer.pipe_info(pipe).unwrap().flags & PIPE_IS_ACTIVE != 0;
        pipes.iter().filter(runs).count()
    };

    // Checked only, it makes nothing: the one pipe that runs is the one
    // the kernel's log shows on. The bar is a check in under 100
    // microseconds, which only an optimized build can say anything of.
    let bytes = change_bytes(CHECK_ONLY | MAY_BLANK, &entries);
    let asked = ChangeRequest::read(&bytes).unwrap();
    let mut least = Duration::MAX;
    for _ in 0..200 {
        let start = Instant::now();
        assert_eq!(layer.change(me, Asked::of(&asked), &asked), Ok(Made::Done));
        least = least.min(start.elapsed());
    }
    assert_eq!(active(&layer), 1);
    println!("the check of a change of sixteen screens ({} entries): {least:?}", entries.len());
    if !cfg!(debug_assertions) {
        assert!(least < Duration::from_micros(100), "the check took {least:?}");
    }

    let bytes = change_bytes(NOW | MAY_BLANK, &entries);
    let asked = ChangeRequest::read(&bytes).unwrap();
    assert_eq!(layer.change(me, Asked::of(&asked), &asked), Ok(Made::Done));
    assert_eq!(active(&layer), SIXTEEN);
}
