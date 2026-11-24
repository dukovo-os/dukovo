// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! What a holder at the back costs the kernel: its pictures, and under
//! 4 KiB for everything else the layer keeps of it. Measured with an
//! allocator that counts the bytes it has out, which the library's own
//! tests cannot have: the library forbids unsafe code.

use std::alloc::{AllocError, Allocator, Global, Layout};
use std::cell::Cell;
use std::ptr::NonNull;
use std::rc::Rc;

use radix_abi::display::{
    BLOB_MODE, CARD_FIRMWARE, Change as ChangeHeader, FORMAT_ARGB8888, FORMAT_XRGB8888, Mode, NOW,
    PLANE_PICTURE,
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

/// Has `holder` show its picture `picture` on plane `plane`, at once.
fn show(layer: &mut Counting<'_>, holder: u32, plane: u32, picture: u32) {
    let mut bytes = vec![0; ChangeHeader::SIZE];
    bytes[ChangeHeader::OFFSET_FLAGS..][..4].copy_from_slice(&NOW.to_le_bytes());
    bytes[ChangeHeader::OFFSET_ENTRIES..][..4].copy_from_slice(&1u32.to_le_bytes());
    bytes.extend_from_slice(&plane.to_le_bytes());
    bytes.extend_from_slice(&PLANE_PICTURE.to_le_bytes());
    bytes.extend_from_slice(&u64::from(picture).to_le_bytes());
    let asked = ChangeRequest::read(&bytes).unwrap();
    assert_eq!(layer.change(holder, Asked::of(&asked), &asked), Ok(Made::Done));
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
