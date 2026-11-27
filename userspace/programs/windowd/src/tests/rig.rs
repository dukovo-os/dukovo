// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! A stand-in for the kernel under the desktop: the kernel's own
//! display layer (`radix_display::layer`) with a card that keeps what
//! it was asked to show, and a clock the tests turn.
//!
//! The desktop's requests reach the layer as the kernel's glue passes
//! them on, so each change windowd asks for is checked by the code that
//! checks it on a real machine. A picture's pixels are pixman's own
//! here, where no memory file is shared with a card.

use std::alloc::Global;
use std::cell::RefCell;
use std::io;
use std::rc::Rc;

use display::abi::{
    ASK_BEAT, BLOB_MODE, Blob, CARD_HOT_PLUG, CARD_SETS_MODES, CARD_SHARED_PAGES, CARD_VIRTIO,
    CHANGE, DROP, DisplayEvent, FORMAT_ARGB8888, FORMAT_XRGB8888, GET, KIND_CARD, KIND_PIPE,
    KIND_PLANE, KIND_SCREEN, LIST, MAKE_BLOB, MOVE_POINTER, Mode, PART_INFO, PART_MODES, RETURN,
    Thing,
};
use display::wire::ChangeBytes;
use display::{Control, Holder};
use radix_abi::Status;
use radix_abi::errors::{File as FileError, Kernel};
use radix_display::Rect;
use radix_display::frame::{
    self, CardSpec, Change, Failed, Frame, PlaneFault, PlaneSpec, ScreenSpec, name,
};
use radix_display::layer::{Asked, Found, Layer, Made, Unmade};
use radix_display::state::PictureShape;
use radix_display::wire as kernel;
use smithay::reexports::pixman::{FormatCode, Image};

use crate::link::Link;

/// What the lender's description of the layer's file is known by, and
/// the desktop's.
const LENDER: usize = 1;
const DESKTOP: usize = 2;

/// A stand-in picture: what it is, and which of its holder's it is.
pub struct Pixels(PictureShape);

impl frame::Picture for Pixels {
    fn shape(&self) -> PictureShape {
        self.0
    }

    fn row_bytes(&self) -> u32 {
        self.0.width * 4
    }
}

/// One thing the card was asked to show.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Showed {
    /// The pipe, by its place on the card.
    pub pipe: usize,
    /// Whether the mode or the screens it feeds are new.
    pub mode: bool,
    /// Whether the main plane's picture is another.
    pub main: bool,
    /// Whether the pointer's picture or hot spot is new.
    pub pointer: bool,
    /// Whether the pointer moved and nothing else changed.
    pub moved: bool,
    /// The rectangles of the main plane's picture that changed.
    pub damage: Vec<Rect>,
    /// Where the pointer's picture shows, if the pointer does.
    pub pointer_at: Option<(i32, i32)>,
    /// The main plane's picture, to tell one from another.
    pub picture: usize,
}

/// A stand-in card, which keeps what it was asked to show.
#[derive(Default)]
pub struct FakeCard {
    /// Each thing, in order.
    pub showed: RefCell<Vec<Showed>>,
}

impl frame::Card<Pixels> for Rc<FakeCard> {
    fn shows_log(&self) -> bool {
        false
    }

    fn log_takes(&self, _frame: &Frame<'_, Pixels>) {}

    fn log_gives(&self) {}

    fn check(&self, _frame: &Frame<'_, Pixels>) -> Result<(), PlaneFault> {
        Ok(())
    }

    fn show(&self, frame: &Frame<'_, Pixels>, change: &Change<'_>) -> Result<(), Failed> {
        self.showed.borrow_mut().push(Showed {
            pipe: frame.pipe,
            mode: change.mode,
            main: change.main,
            pointer: change.pointer,
            moved: change.moved,
            damage: change.damage.to_vec(),
            pointer_at: frame.pointer.as_ref().map(|pointer| (pointer.x, pointer.y)),
            picture: frame
                .main
                .as_ref()
                .map_or(0, |main| core::ptr::from_ref(main.picture) as usize),
        });
        Ok(())
    }

    fn put(&self, _picture: &Pixels, _rect: Rect, _rows: &[u8], _on: Option<&Frame<'_, Pixels>>) {}

    fn wake_for(&self, _at_ns: u64) {}
}

/// The stand-in kernel: the layer, its card and the clock.
pub struct FakeKernel {
    layer: Layer<Rc<Pixels>, Rc<FakeCard>, Global>,
    card: (u32, Rc<FakeCard>),
    /// The time on the clock that stamps the layer's events.
    pub now: u64,
}

fn mode(width: u32, height: u32) -> Mode {
    Mode { width, height, refresh_mhz: 60_000, ..Mode::default() }
}

fn planes(format: u32, side: u32) -> PlaneSpec {
    PlaneSpec { width_max: side, height_max: side, formats: [format, 0, 0, 0, 0, 0, 0, 0] }
}

impl FakeKernel {
    /// A kernel with one card, with a screen for each of `screens`,
    /// each fed by a pipe of its own: a size for one with a monitor,
    /// `None` for one without. `shared` makes it a card that reads the
    /// program's pages at any moment.
    fn new(screens: &[Option<(u32, u32)>], shared: bool) -> FakeKernel {
        let mut layer = Layer::new_in(Global);
        let card = Rc::new(FakeCard::default());
        let specs: Vec<ScreenSpec> = screens
            .iter()
            .enumerate()
            .map(|(pipe, size)| ScreenSpec {
                plugged: size.is_some(),
                fixed_pipe: Some(pipe),
                native: size.map_or(Mode::default(), |(width, height)| mode(width, height)),
            })
            .collect();
        let flags = CARD_SETS_MODES | CARD_HOT_PLUG;
        let spec = CardSpec {
            kind: CARD_VIRTIO,
            name: name("virtio0"),
            model: name("A stand-in"),
            flags: if shared { flags | CARD_SHARED_PAGES } else { flags },
            width_min: 64,
            height_min: 64,
            width_max: 8192,
            height_max: 8192,
            pipes: screens.len(),
            screens: &specs,
            main: planes(FORMAT_XRGB8888, 8192),
            pointer: planes(FORMAT_ARGB8888, 256),
        };
        let mut id = 0;
        let made = layer.add_card(&spec, |number| {
            id = number;
            Some(card.clone())
        });
        assert!(made.is_ok());
        FakeKernel { layer, card: (id, card), now: 0 }
    }

    /// Lets `ns` pass, with each beat of the card that falls in it.
    pub fn pass(&mut self, ns: u64) {
        let until = self.now + ns;
        while let Some(at) = self.layer.next_beat(self.card.0).filter(|at| *at <= until) {
            self.now = self.now.max(at);
            self.layer.set_time(self.now);
            self.layer.beat(self.card.0);
        }
        self.now = until;
        self.layer.set_time(self.now);
    }

    /// The card's screen at `index` is found anew: with a monitor of
    /// `size`, or with none.
    pub fn found(&mut self, index: usize, size: Option<(u32, u32)>) {
        let modes = size.map(|(width, height)| mode(width, height));
        let found = Found { plugged: size.is_some(), modes: modes.as_slice(), data: &[] };
        assert_eq!(self.layer.screen_found(self.card.0, index, &found), Ok(true));
    }

    /// The lender takes the screens back, as when the person chooses a
    /// console, or lends them to the desktop again.
    pub fn lend(&mut self, to_desktop: bool) {
        let lender = self.layer.holder_of(LENDER, true).unwrap();
        let desktop = self.layer.holder_of(DESKTOP, false).unwrap();
        if to_desktop {
            self.layer.lend(lender, desktop).unwrap();
        } else {
            self.layer.give_back(desktop).unwrap();
        }
    }

    /// The next event the desktop's holder has not read.
    pub fn event(&mut self) -> Option<DisplayEvent> {
        let desktop = self.layer.holder_of(DESKTOP, false).ok()?;
        self.layer.holder(desktop)?.next_event()
    }

    /// Lets time pass until the change the layer numbered `seq` shows.
    fn wait_shown(&mut self, seq: u64) {
        while self.layer.waits(seq) {
            self.pass(1_000_000);
        }
    }

    /// GET's answer: `part` of `thing`.
    fn part(&self, thing: u32, part: u32) -> Result<Vec<u8>, Status> {
        let none = || FileError::NotFound.status(0);
        let layer = &self.layer;
        Ok(match (layer.kind_of(thing).ok_or_else(none)?, part) {
            (KIND_CARD, PART_INFO) => {
                kernel::card_bytes(&layer.card_info(thing).ok_or_else(none)?).to_vec()
            }
            (KIND_SCREEN, PART_INFO) => {
                kernel::screen_bytes(&layer.screen_info(thing).ok_or_else(none)?).to_vec()
            }
            (KIND_SCREEN, PART_MODES) => {
                let modes = layer.screen_modes(thing).ok_or_else(none)?;
                modes.iter().flat_map(kernel::mode_bytes).collect()
            }
            (KIND_PIPE, PART_INFO) => {
                kernel::pipe_bytes(&layer.pipe_info(thing).ok_or_else(none)?).to_vec()
            }
            (KIND_PLANE, PART_INFO) => {
                kernel::plane_bytes(&layer.plane_info(thing).ok_or_else(none)?).to_vec()
            }
            _ => return Err(Kernel::InvalidArgument.status(0)),
        })
    }

    /// One request of the desktop's holder, as the kernel's glue passes
    /// it to the layer.
    fn request(&mut self, request: u32, argument: u64, buffer: &mut [u8]) -> Result<u64, Status> {
        let invalid = || Kernel::InvalidArgument.status(0);
        let me = self.layer.holder_of(DESKTOP, false)?;
        match request {
            LIST => {
                let mut things: Vec<Thing> = Vec::new();
                self.layer.list(&mut |thing| things.push(thing));
                for (thing, out) in things.iter().zip(buffer.as_chunks_mut::<{ Thing::SIZE }>().0) {
                    out.copy_from_slice(&kernel::thing_bytes(thing));
                }
                Ok(things.len() as u64)
            }
            GET => {
                let asked = kernel::read_get(buffer).ok_or_else(invalid)?;
                let bytes = self.part(asked.thing, asked.part)?;
                buffer.get_mut(..bytes.len()).ok_or_else(invalid)?.copy_from_slice(&bytes);
                Ok(bytes.len() as u64)
            }
            MAKE_BLOB => {
                let (kind, len) = kernel::read_blob(buffer).ok_or_else(invalid)?;
                let value =
                    buffer.get(Blob::SIZE..Blob::SIZE + len as usize).ok_or_else(invalid)?;
                let mode = if kind == BLOB_MODE { kernel::read_mode(value) } else { None };
                let holder = self.layer.holder(me).ok_or_else(invalid)?;
                let number = holder.keep_value(kind, mode)?;
                buffer[kernel::BLOB_NUMBER_AT..][..4].copy_from_slice(&number.to_le_bytes());
                Ok(0)
            }
            DROP => {
                let holder = self.layer.holder(me).ok_or_else(invalid)?;
                holder.drop_object(argument as u32).map(|()| 0)
            }
            CHANGE => {
                let asked = kernel::ChangeRequest::read(buffer).ok_or_else(invalid)?;
                match self.layer.change(me, Asked::of(&asked), &asked) {
                    Ok(Made::Done) => Ok(0),
                    Ok(Made::Wait(seq)) => {
                        self.wait_shown(seq);
                        Ok(0)
                    }
                    Err(Unmade::Status(status)) => Err(status),
                    Err(Unmade::Refused(entry, reason)) => {
                        buffer[kernel::REFUSAL_AT..][..8]
                            .copy_from_slice(&kernel::refusal(entry, reason));
                        Err(invalid())
                    }
                }
            }
            MOVE_POINTER => {
                let (x, y) = ((argument >> 16) as u16 as i16, argument as u16 as i16);
                let plane = (argument >> 32) as u32;
                self.layer.move_pointer(me, plane, i32::from(x), i32::from(y)).map(|()| 0)
            }
            ASK_BEAT => self.layer.ask_beat(me, argument as u32).map(|()| 0),
            RETURN => self.layer.give_back(me).map(|()| 0),
            _ => Err(Kernel::NotTerminal.status(0)),
        }
    }
}

/// The desktop's description of the stand-in kernel's layer.
#[derive(Clone)]
pub struct Wire(pub Rc<RefCell<FakeKernel>>);

/// A stand-in kernel as [`FakeKernel::new`] makes one, whose screens
/// the lender has lent to the desktop: the desktop's holder, which is
/// in front, and the card.
pub fn lent(screens: &[Option<(u32, u32)>], shared: bool) -> (Holder<Wire>, Rc<FakeCard>) {
    let mut kernel = FakeKernel::new(screens, shared);
    let lender = kernel.layer.holder_of(LENDER, true).unwrap();
    // The lender's first change takes the screens from the log.
    let mut nothing = ChangeBytes::new(0, 0);
    let asked = kernel::ChangeRequest::read(nothing.bytes_mut()).unwrap();
    assert!(kernel.layer.change(lender, Asked::of(&asked), &asked).is_ok());
    let card = kernel.card.1.clone();
    let wire = Wire(Rc::new(RefCell::new(kernel)));
    wire.0.borrow_mut().lend(true);
    (Holder::new(wire), card)
}

/// The error the C library makes of `status`.
fn failure(status: Status) -> io::Error {
    let kernel = Kernel::from_status(status).map(Kernel::errno);
    let file = FileError::from_status(status).map(FileError::errno);
    io::Error::from_raw_os_error(kernel.or(file).unwrap_or(libc::EIO))
}

impl Control for Wire {
    fn control(&self, request: u32, argument: u64, buffer: &mut [u8]) -> io::Result<u64> {
        self.0.borrow_mut().request(request, argument, buffer).map_err(failure)
    }
}

impl Link for Wire {
    type Pixels = Image<'static, 'static>;

    fn pixels(
        holder: &Holder<Wire>,
        card: u32,
        format: u32,
        (width, height): (u32, u32),
    ) -> io::Result<(u32, Image<'static, 'static>)> {
        let mut kernel = holder.file().0.borrow_mut();
        let me = kernel.layer.holder_of(DESKTOP, false).map_err(failure)?;
        let shape = PictureShape { card, format, width, height };
        let of_holder = kernel.layer.holder(me).ok_or_else(|| io::Error::other("no holder"))?;
        let picture = of_holder.keep_picture(Rc::new(Pixels(shape))).map_err(failure)?;
        let code = match format {
            FORMAT_ARGB8888 => FormatCode::A8R8G8B8,
            _ => FormatCode::X8R8G8B8,
        };
        let pixels = Image::new(code, width as usize, height as usize, true);
        Ok((picture, pixels.map_err(|_| io::Error::other("pixman made no image"))?))
    }
}
