// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The screens against the kernel's own display layer
//! (`radix_display::layer`), with stand-in cards whose screens are bytes
//! the tests look at. The requests reach the layer as the kernel's own
//! glue passes them on, so what consoled asks for is checked by the code
//! that checks it on a real machine, and what PUT sends lands where a
//! card would show it.

use std::alloc::Global;
use std::cell::RefCell;
use std::rc::Rc;

use display::abi::{
    ATTENTION_END, BLOB_MODE, Blob, CARD_FIRMWARE, CARD_HOT_PLUG, CARD_SETS_MODES, CARD_VIRTIO,
    CHANGE, DROP, FORMAT_ARGB8888, FORMAT_XRGB8888, GET, KIND_CARD, KIND_PIPE, KIND_PLANE,
    KIND_SCREEN, LEND, LIST, MAKE_BLOB, MAKE_PICTURE, Mode, PART_INFO, PART_MODES, PICTURE_KEPT,
    PUT, RETURN, STAND_UNDER, Thing,
};
use radix_abi::Status;
use radix_abi::errors::{File as FileError, Kernel};
use radix_display::Rect;
use radix_display::frame::{
    self, CardSpec, Change, Failed, Frame, PlaneFault, PlaneSpec, ScreenSpec, name,
};
use radix_display::layer::{Asked, Found, Layer, Made, Unmade as LayerUnmade};
use radix_display::state::PictureShape;
use radix_display::wire as kernel;

use super::*;
use crate::term::{BOLD, REVERSE};

mod key;
mod lend;

/// What consoled's description of the layer's file is known by, and
/// another holder's.
const CONSOLED: usize = 1;
const OTHER: usize = 2;

/// The byte a stand-in screen holds where nothing was ever drawn.
const BARE: u8 = 0xee;

/// A stand-in picture: one a card keeps has no pixels of its own.
struct Pixels(PictureShape);

impl frame::Picture for Pixels {
    fn shape(&self) -> PictureShape {
        self.0
    }

    fn row_bytes(&self) -> u32 {
        self.0.width * 4
    }
}

/// A stand-in card: the pixels of the screen each of its pipes feeds,
/// which PUT's rows land in while their picture shows, as on the
/// firmware's card.
struct FakeCard {
    log: bool,
    /// For each pipe, its screen's width and its pixels.
    glass: RefCell<Vec<(usize, Vec<u8>)>>,
    /// The bytes of rows each PUT rectangle carried.
    put: RefCell<Vec<usize>>,
}

impl frame::Card<Pixels> for Rc<FakeCard> {
    fn shows_log(&self) -> bool {
        self.log
    }

    fn log_takes(&self, _frame: &Frame<'_, Pixels>) {}

    fn log_gives(&self) {}

    fn check(&self, _frame: &Frame<'_, Pixels>) -> Result<(), PlaneFault> {
        Ok(())
    }

    fn show(&self, _frame: &Frame<'_, Pixels>, _change: &Change<'_>) -> Result<(), Failed> {
        Ok(())
    }

    fn put(&self, _picture: &Pixels, rect: Rect, rows: &[u8], on: Option<&Frame<'_, Pixels>>) {
        self.put.borrow_mut().push(rows.len());
        let Some(frame) = on.filter(|frame| frame.active) else { return };
        let mut glass = self.glass.borrow_mut();
        let (width, pixels) = &mut glass[frame.pipe];
        let len = rect.width as usize * 4;
        for (row, y) in rows.chunks_exact(len).zip(rect.y as usize..) {
            let at = (y * *width + rect.x as usize) * 4;
            pixels[at..at + len].copy_from_slice(row);
        }
    }

    fn wake_for(&self, _at_ns: u64) {}
}

/// A stand-in for the kernel: the layer, its cards, and the clock.
struct FakeKernel {
    layer: Layer<Rc<Pixels>, Rc<FakeCard>, Global>,
    cards: Vec<(u32, Rc<FakeCard>)>,
    now: u64,
    /// How many PUT requests consoled made.
    puts: usize,
    /// How many CHANGE requests.
    changes: usize,
    /// Whether PUT fails for want of memory.
    no_memory: bool,
    /// How many of the next GET requests fail, as one does when the
    /// thing it asks about changed since it was listed.
    changing: usize,
    /// How many descriptions of the layer's file were opened after
    /// consoled's and the other holder's.
    opened: usize,
}

fn mode(width: u32, height: u32) -> Mode {
    Mode { width, height, refresh_mhz: 60_000, ..Mode::default() }
}

fn planes(format: u32, side: u32) -> PlaneSpec {
    PlaneSpec { width_max: side, height_max: side, formats: [format, 0, 0, 0, 0, 0, 0, 0] }
}

impl FakeKernel {
    fn new() -> FakeKernel {
        FakeKernel {
            layer: Layer::new_in(Global),
            cards: Vec::new(),
            now: 0,
            puts: 0,
            changes: 0,
            no_memory: false,
            changing: 0,
            opened: 0,
        }
    }

    /// Adds a card with a screen for each of `screens`, each fed by a
    /// pipe of its own: a size for one with a monitor, `None` for one
    /// without. `log` makes it the card the kernel's log shows on, as
    /// the firmware's is.
    fn add_card(&mut self, log: bool, screens: &[Option<(u32, u32)>]) -> Rc<FakeCard> {
        let glass = screens
            .iter()
            .map(|size| {
                let (width, height) = size.unwrap_or((0, 0));
                (width as usize, vec![BARE; (width * height * 4) as usize])
            })
            .collect();
        let card =
            Rc::new(FakeCard { log, glass: RefCell::new(glass), put: RefCell::new(Vec::new()) });
        let specs: Vec<ScreenSpec> = screens
            .iter()
            .enumerate()
            .map(|(pipe, size)| ScreenSpec {
                plugged: size.is_some(),
                fixed_pipe: Some(pipe),
                native: size.map_or(Mode::default(), |(width, height)| mode(width, height)),
            })
            .collect();
        let spec = CardSpec {
            kind: if log { CARD_FIRMWARE } else { CARD_VIRTIO },
            name: name(if log { "firmware0" } else { "virtio0" }),
            model: name("A stand-in"),
            flags: if log { 0 } else { CARD_SETS_MODES | CARD_HOT_PLUG },
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
        let made = self.layer.add_card(&spec, |number| {
            id = number;
            Some(card.clone())
        });
        assert!(made.is_ok());
        self.cards.push((id, card.clone()));
        card
    }

    /// `card` finds its screen at `index` anew, as a card whose
    /// monitors come and go does: with a monitor of `size`, or with
    /// none. The stand-in's pixels are made for it.
    fn found(&mut self, card: &Rc<FakeCard>, index: usize, size: Option<(u32, u32)>) {
        let (width, height) = size.unwrap_or((0, 0));
        card.glass.borrow_mut()[index] =
            (width as usize, vec![BARE; (width * height * 4) as usize]);
        let id = self.cards.iter().find(|(_, known)| Rc::ptr_eq(known, card)).unwrap().0;
        let modes = size.map(|(width, height)| mode(width, height));
        let found = Found { plugged: size.is_some(), modes: modes.as_slice(), data: &[] };
        assert_eq!(self.layer.screen_found(id, index, &found), Ok(true));
    }

    /// Lets time pass until the change the layer numbered `seq` shows.
    fn wait_shown(&mut self, seq: u64) {
        while self.layer.waits(seq) {
            for (id, _) in &self.cards {
                if let Some(at) = self.layer.next_beat(*id) {
                    self.now = self.now.max(at);
                    self.layer.set_time(self.now);
                    self.layer.beat(*id);
                }
            }
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

    /// The holder the layer has for the description opened as `key`,
    /// which is made one now if it was not. consoled's is a description
    /// of the attention file, and any other's of the control file.
    fn holder(&mut self, key: usize) -> Result<u32, Status> {
        self.layer.holder_of(key, key == CONSOLED)
    }

    /// One request of the holder `key` knows, as the kernel's glue
    /// passes it to the layer.
    fn request(
        &mut self,
        key: usize,
        request: u32,
        argument: u64,
        buffer: &mut [u8],
    ) -> Result<u64, Status> {
        let invalid = || Kernel::InvalidArgument.status(0);
        self.now += 1_000_000;
        self.layer.set_time(self.now);
        let me = self.holder(key)?;
        match request {
            LIST => {
                let mut things: Vec<Thing> = Vec::new();
                self.layer.list(&mut |thing| things.push(thing));
                for (thing, out) in things.iter().zip(buffer.as_chunks_mut::<{ Thing::SIZE }>().0) {
                    out.copy_from_slice(&kernel::thing_bytes(thing));
                }
                Ok(things.len() as u64)
            }
            GET if self.changing > 0 => {
                self.changing -= 1;
                Err(invalid())
            }
            GET => {
                let asked = kernel::read_get(buffer).ok_or_else(invalid)?;
                let bytes = self.part(asked.thing, asked.part)?;
                buffer.get_mut(..bytes.len()).ok_or_else(invalid)?.copy_from_slice(&bytes);
                Ok(bytes.len() as u64)
            }
            MAKE_PICTURE => {
                let asked = kernel::read_picture(buffer).ok_or_else(invalid)?;
                if asked.flags != PICTURE_KEPT || self.layer.kind_of(asked.card) != Some(KIND_CARD)
                {
                    return Err(invalid());
                }
                let shape = PictureShape {
                    card: asked.card,
                    format: asked.format,
                    width: asked.width,
                    height: asked.height,
                };
                let holder = self.layer.holder(me).ok_or_else(invalid)?;
                let number = holder.keep_picture(Rc::new(Pixels(shape)))?;
                buffer[kernel::PICTURE_NUMBER_AT..][..4].copy_from_slice(&number.to_le_bytes());
                Ok(0)
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
                self.changes += 1;
                let asked = kernel::ChangeRequest::read(buffer).ok_or_else(invalid)?;
                match self.layer.change(me, Asked::of(&asked), &asked) {
                    Ok(Made::Done) => Ok(0),
                    Ok(Made::Wait(seq)) => {
                        self.wait_shown(seq);
                        Ok(0)
                    }
                    Err(LayerUnmade::Status(status)) => Err(status),
                    Err(LayerUnmade::Refused(entry, reason)) => {
                        buffer[kernel::REFUSAL_AT..][..8]
                            .copy_from_slice(&kernel::refusal(entry, reason));
                        Err(invalid())
                    }
                }
            }
            PUT if self.no_memory => Err(Kernel::NoMemory.status(0)),
            PUT => {
                self.puts += 1;
                let count = kernel::PutRequest::count(buffer).ok_or_else(invalid)?;
                let table = kernel::PutRequest::table_bytes(count).ok_or_else(invalid)?;
                let head = buffer.get(..table).ok_or_else(invalid)?;
                let asked =
                    kernel::PutRequest::read(head, buffer.len() as u64).ok_or_else(invalid)?;
                let holder = self.layer.holder(me).ok_or_else(invalid)?;
                let picture = holder.picture_ref(asked.picture).cloned().ok_or_else(invalid)?;
                let (width, height) = (picture.0.width, picture.0.height);
                // Every rectangle is checked before a row of any is
                // written.
                let parts: Option<Vec<_>> =
                    (0..asked.len()).map(|index| asked.part(index, width, height)).collect();
                for part in parts.ok_or_else(invalid)? {
                    let len = part.rect.width as usize * 4 * part.rect.height as usize;
                    let rows = &buffer[part.offset as usize..][..len];
                    self.layer.put(me, &picture, part.rect, rows)?;
                }
                Ok(0)
            }
            // A slot stands for the description opened as that key.
            LEND => {
                let borrower = self.holder(argument as usize)?;
                self.layer.lend(me, borrower).map(|()| 0)
            }
            RETURN => self.layer.give_back(me).map(|()| 0),
            ATTENTION_END => self.layer.attention_end(me).map(|()| 0),
            STAND_UNDER => self.layer.stand_under(me).map(|()| 0),
            _ => Err(Kernel::NotTerminal.status(0)),
        }
    }
}

/// A description of the stand-in kernel's layer, and what the layer
/// knows it by: consoled's, or one opened to lend the front to.
#[derive(Clone)]
struct Wire(Rc<RefCell<FakeKernel>>, usize);

impl Wire {
    /// consoled's description of `kernel`'s layer.
    fn of(kernel: FakeKernel) -> Wire {
        Wire(Rc::new(RefCell::new(kernel)), CONSOLED)
    }

    /// The holder the layer has for the description, which is made one
    /// now if it was not.
    fn holder(&self) -> u32 {
        self.0.borrow_mut().holder(self.1).unwrap()
    }

    /// Closes the description, as its program ending does.
    fn close(self) {
        let holder = self.holder();
        self.0.borrow_mut().layer.end(holder);
    }
}

/// The error the C library makes of `status`.
fn failure(status: Status) -> io::Error {
    let kernel = Kernel::from_status(status).map(Kernel::errno);
    let file = FileError::from_status(status).map(FileError::errno);
    io::Error::from_raw_os_error(kernel.or(file).unwrap_or(libc::EIO))
}

impl Control for Wire {
    fn control(&self, request: u32, argument: u64, buffer: &mut [u8]) -> io::Result<u64> {
        self.0.borrow_mut().request(self.1, request, argument, buffer).map_err(failure)
    }
}

impl Link for Wire {
    fn events(&self, into: &mut [u8]) -> io::Result<usize> {
        let mut kernel = self.0.borrow_mut();
        let me = kernel.holder(self.1).map_err(failure)?;
        let holder = kernel.layer.holder(me).ok_or_else(|| io::Error::other("no holder"))?;
        let mut done = 0;
        for out in into.as_chunks_mut::<{ DisplayEvent::SIZE }>().0 {
            let Some(event) = holder.next_event() else { break };
            out.copy_from_slice(&kernel::event_bytes(&event));
            done += DisplayEvent::SIZE;
        }
        if done == 0 { Err(io::ErrorKind::WouldBlock.into()) } else { Ok(done) }
    }

    fn another(&self) -> io::Result<Wire> {
        let mut kernel = self.0.borrow_mut();
        kernel.opened += 1;
        Ok(Wire(self.0.clone(), OTHER + kernel.opened))
    }

    fn slot(&self) -> u32 {
        self.1 as u32
    }
}

/// A stand-in kernel with one card like the firmware's, of one screen.
fn firmware(width: u32, height: u32) -> (Wire, Rc<FakeCard>) {
    let mut kernel = FakeKernel::new();
    let card = kernel.add_card(true, &[Some((width, height))]);
    (Wire::of(kernel), card)
}

/// What screen `at` of `screen` should show of `term`, with `bottom`
/// over its last row: every row of the grid painted whole, and the
/// palette's background round them.
fn expected(screen: &Screen<Wire>, at: usize, term: &Terminal, bottom: Option<&[Cell]>) -> Vec<u8> {
    let surface = &screen.surfaces[at];
    let (wide, tall) = surface.size();
    let (font, scale) = surface.font().unwrap();
    let (width, height) = (font.width * scale, font.height * scale);
    let mut pixels = paint::pixel(term.palette()[0]).repeat(wide * tall);
    let last = screen.rows - 1;
    let cursor = term.cursor().filter(|&(_, y)| bottom.is_none() || y != last);
    let reverse = term.modes().reverse_screen;
    let mut line = vec![0; screen.cols * width * 4 * height];
    for y in 0..screen.rows {
        let cell = |x: usize| match bottom {
            Some(cells) if y == last => cells.get(x).copied().unwrap_or(Cell::BLANK),
            _ if x < term.cols() && y < term.rows() => term.shown(x, y),
            _ => Cell::BLANK,
        };
        let looks: Vec<Look> = (0..screen.cols)
            .map(|x| Look::of(cell(x), term.palette(), reverse, cursor == Some((x, y))))
            .collect();
        paint::paint(&font, scale, &looks, &mut line);
        let (left, top) = (surface.fit.origin.0, surface.fit.origin.1 + y * height);
        for (row, bytes) in line.chunks_exact(screen.cols * width * 4).enumerate() {
            let to = ((top + row) * wide + left) * 4;
            pixels[to..to + bytes.len()].copy_from_slice(bytes);
        }
    }
    pixels
}

/// Checks that pipe `pipe` of `card` shows what screen `at` of `screen`
/// should show of `term`.
#[track_caller]
fn assert_shows(card: &FakeCard, pipe: usize, screen: &Screen<Wire>, at: usize, term: &Terminal) {
    let glass = card.glass.borrow();
    let want = expected(screen, at, term, None);
    let (width, shown) = (glass[pipe].0, &glass[pipe].1);
    assert_eq!(shown.len(), want.len());
    if *shown != want {
        let at = shown.iter().zip(&want).position(|(a, b)| a != b).unwrap_or(0) / 4;
        panic!("the screen differs from the console at ({}, {})", at % width, at / width);
    }
}

/// A terminal the size of `screen`'s grid.
fn terminal(screen: &Screen<Wire>) -> Terminal {
    Terminal::new(screen.cols(), screen.rows())
}

#[test]
fn the_grid_is_chosen_as_the_kernels_log_chooses_it() {
    let want = radix_fbcon::MIN_GRID;
    // 1280 by 800 is 160 by 50 cells of the smallest font: the next one
    // leaves fewer than 100 by 40.
    let (grid, fits) = arrange(&[(1280, 800)], want, Size::Normal);
    assert_eq!((grid, fits), ((160, 50), vec![Fit { step: 0, origin: (0, 0) }]));
    // A step bigger, the grid is centred.
    let (grid, fits) = arrange(&[(1280, 800)], want, Size::Larger);
    assert_eq!((grid, fits), ((106, 33), vec![Fit { step: 1, origin: (4, 4) }]));
    // The installer's screens want 100 by 30, which a larger font gives.
    let (grid, fits) = arrange(&[(1280, 800)], (100, 30), Size::Normal);
    assert_eq!((grid, fits), ((106, 33), vec![Fit { step: 1, origin: (4, 4) }]));
    // No screen: the grid the consoles keep until one comes.
    assert_eq!(arrange(&[], want, Size::Normal), (UNSEEN_GRID, vec![]));
}

#[test]
fn bigger_text_steps_through_the_fonts_then_doubles_them() {
    let step = |screen: (usize, usize), size: Size| {
        let (_, fits) = arrange(&[screen], radix_fbcon::MIN_GRID, size);
        LADDER[fits[0].step]
    };
    let hd = (1920, 1080);
    assert_eq!(step(hd, Size::Normal), ("12x24", 1));
    assert_eq!(step(hd, Size::Larger), ("16x32", 1));
    assert_eq!(step(hd, Size::Largest), ("12x24", 2));
    let uhd = (3840, 2160);
    assert_eq!(step(uhd, Size::Normal), ("16x32", 1));
    assert_eq!(step(uhd, Size::Largest), ("16x32", 2));
    // A small screen: the largest that leaves 40 by 12 cells.
    let small = (600, 400);
    assert_eq!(step(small, Size::Normal), ("8x16", 1));
    assert_eq!(step(small, Size::Larger), ("12x24", 1));
    assert_eq!(step(small, Size::Largest), ("12x24", 1));
    let (grid, fits) = arrange(&[hd], radix_fbcon::MIN_GRID, Size::Largest);
    assert_eq!((grid, fits[0].origin), ((80, 22), (0, (1080 - 22 * 48) / 2)));
}

#[test]
fn screens_of_different_sizes_show_one_grid_each_in_the_biggest_text_that_fits() {
    // Full HD beside 4K: the grid is full HD's 160 by 45, which 4K shows
    // in the same font at twice its size, edge to edge.
    let (grid, fits) = arrange(&[(1920, 1080), (3840, 2160)], radix_fbcon::MIN_GRID, Size::Normal);
    assert_eq!(grid, (160, 45));
    assert_eq!(fits, [Fit { step: 1, origin: (0, 0) }, Fit { step: 3, origin: (0, 0) }]);
    // A screen that no bigger text fits the grid on centres it.
    let (grid, fits) = arrange(&[(1920, 1200), (1920, 1080)], radix_fbcon::MIN_GRID, Size::Normal);
    assert_eq!(grid, (160, 45));
    assert_eq!(fits, [Fit { step: 1, origin: (0, 60) }, Fit { step: 1, origin: (0, 0) }]);
}

#[test]
fn consoled_takes_the_screen_from_the_log_and_draws_the_console() {
    let (wire, card) = firmware(640, 400);
    let mut screen = Screen::on(wire.clone(), radix_fbcon::MIN_GRID).unwrap();
    assert_eq!(screen.describe(), ["console of 80x25 cells with font 8x16 at (0, 0)"]);
    assert_eq!((screen.cols(), screen.rows()), (80, 25));
    // Its one change took the front.
    assert!(wire.0.borrow().layer.front().is_some());
    assert_eq!(wire.0.borrow().changes, 1);
    let mut term = terminal(&screen);
    term.feed(b"Dukovo\r\n\x1b[1;31mred\x1b[0m and \x1b[7mreversed\x1b[0m \x1b[4munder\x1b[0m");
    screen.draw(&term, None);
    assert_shows(&card, 0, &screen, 0, &term);
    // Its own front event, read later, draws nothing again.
    assert!(!screen.events());
    let puts = wire.0.borrow().puts;
    screen.draw(&term, None);
    assert_eq!(wire.0.borrow().puts, puts);
}

#[test]
fn a_draw_sends_only_the_cells_that_changed_in_one_request() {
    let (wire, card) = firmware(640, 400);
    let mut screen = Screen::on(wire.clone(), radix_fbcon::MIN_GRID).unwrap();
    let mut term = terminal(&screen);
    screen.draw(&term, None);
    let (puts, rects) = (wire.0.borrow().puts, card.put.borrow().len());
    // Three characters typed on one row, and two on another: the cells
    // typed and the one the cursor came to, in one request.
    term.feed(b"abc\x1b[10;40Hxy");
    screen.draw(&term, None);
    assert_eq!(wire.0.borrow().puts, puts + 1);
    let sent = card.put.borrow()[rects..].to_vec();
    // Row 1: the cursor's old cell and the three typed. Row 10: the two
    // typed and the cursor after them. Each cell is 8 by 16 pixels.
    assert_eq!(sent, [3 * 8 * 16 * 4, 3 * 8 * 16 * 4]);
    assert_shows(&card, 0, &screen, 0, &term);
    // Nothing changed: nothing is sent.
    screen.draw(&term, None);
    assert_eq!(wire.0.borrow().puts, puts + 1);
}

#[test]
fn the_margins_are_painted_and_follow_the_palette() {
    // 650 by 410 leaves 5 pixels round a grid of 80 by 25.
    let (wire, card) = firmware(650, 410);
    let mut screen = Screen::on(wire, radix_fbcon::MIN_GRID).unwrap();
    assert_eq!(screen.describe(), ["console of 81x25 cells with font 8x16 at (1, 5)"]);
    let mut term = terminal(&screen);
    term.feed(b"edge to edge");
    screen.draw(&term, None);
    assert_shows(&card, 0, &screen, 0, &term);
    assert!(card.glass.borrow()[0].1.iter().all(|&byte| byte != BARE));
    // The program sets colour 0 (OSC P): the margins take it.
    term.feed(b"\x1b]P0102030\x07");
    assert_ne!(term.palette()[0], default_palette()[0]);
    screen.draw(&term, None);
    assert_shows(&card, 0, &screen, 0, &term);
}

/// A step of a sequence of numbers that looks random and is the same in
/// every run.
fn next(seed: &mut u64) -> usize {
    *seed = seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
    (*seed >> 33) as usize
}

/// Something a program might write to its terminal.
fn output(seed: &mut u64) -> Vec<u8> {
    match next(seed) % 9 {
        0 => b"\r\n".repeat(1 + next(seed) % 30),
        8 => {
            format!("\x1b]4;{};#{:06x}\x07", next(seed) % 16, next(seed) % 0x100_0000).into_bytes()
        }
        1 => format!("\x1b[{};{}H", 1 + next(seed) % 30, 1 + next(seed) % 90).into_bytes(),
        2 => format!("\x1b[{};4{}m", 30 + next(seed) % 8, next(seed) % 8).into_bytes(),
        3 => b"\x1b[2J".to_vec(),
        4 => b"\x1b[?5h".to_vec(),
        5 => b"\x1b[?5l\x1b[0m".to_vec(),
        6 => b"\x1b[?25l".to_vec(),
        _ => {
            let mut text = b"\x1b[?25h".to_vec();
            text.extend((0..=next(seed) % 200).map(|_| b' ' + (next(seed) % 95) as u8));
            text
        }
    }
}

#[test]
fn whatever_is_written_the_screens_show_what_a_full_repaint_would() {
    let mut kernel = FakeKernel::new();
    let firmware = kernel.add_card(true, &[Some((650, 410))]);
    let virtio = kernel.add_card(false, &[Some((1024, 768)), None, Some((800, 600))]);
    let wire = Wire::of(kernel);
    let mut screen = Screen::on(wire, radix_fbcon::MIN_GRID).unwrap();
    assert_eq!(
        screen.describe(),
        [
            "console of 81x25 cells with font 8x16 at (1, 5)",
            "screen 2: console of 81x25 cells with font 12x24 at (26, 84)",
            "screen 3: console of 81x25 cells with font 8x16 at (76, 100)",
        ]
    );
    let mut terms = [terminal(&screen), terminal(&screen)];
    let (mut seed, mut shown) = (7, 0);
    for round in 0..150 {
        match next(&mut seed) % 20 {
            // Another console.
            0 => {
                shown = 1 - shown;
                screen.invalidate();
            }
            // Another text size, which the consoles follow.
            1 => {
                let size = Size::from_steps(next(&mut seed) % 3);
                if screen.set_size(size) {
                    for term in &mut terms {
                        term.resize(screen.cols(), screen.rows());
                    }
                }
            }
            _ => terms[shown].feed(&output(&mut seed)),
        }
        // A draw now and then, as when output comes faster than draws.
        if round % 3 != 0 {
            screen.draw(&terms[shown], None);
            assert_shows(&firmware, 0, &screen, 0, &terms[shown]);
            assert_shows(&virtio, 0, &screen, 1, &terms[shown]);
            assert_shows(&virtio, 2, &screen, 2, &terms[shown]);
        }
    }
}

#[test]
fn a_notification_covers_the_last_row_and_its_cursor() {
    let (wire, card) = firmware(640, 400);
    let mut screen = Screen::on(wire, radix_fbcon::MIN_GRID).unwrap();
    let mut term = terminal(&screen);
    term.feed(b"\x1b[25;1Hprompt> ");
    let note: Vec<Cell> =
        "A notice".chars().map(|c| Cell { c, flags: BOLD | REVERSE, ..Cell::BLANK }).collect();
    screen.draw(&term, Some(&note));
    let want = expected(&screen, 0, &term, Some(&note));
    assert!(card.glass.borrow()[0].1 == want);
    // It goes: the row and the cursor on it show again.
    screen.draw(&term, None);
    assert_shows(&card, 0, &screen, 0, &term);
}

#[test]
fn behind_another_holder_nothing_is_drawn_and_all_of_it_is_when_in_front_again() {
    let (wire, card) = firmware(640, 400);
    let mut screen = Screen::on(wire.clone(), radix_fbcon::MIN_GRID).unwrap();
    let mut term = terminal(&screen);
    term.feed(b"before");
    screen.draw(&term, None);
    // The front is lent to another holder, which draws over the screen.
    let lend = |wire: &Wire| {
        let mut kernel = wire.0.borrow_mut();
        let consoled = kernel.holder(CONSOLED).unwrap();
        let other = kernel.holder(OTHER).unwrap();
        kernel.layer.lend(consoled, other).unwrap();
        other
    };
    let other = lend(&wire);
    card.glass.borrow_mut()[0].1.fill(BARE);
    assert!(!screen.events());
    let puts = wire.0.borrow().puts;
    term.feed(b" and while behind");
    screen.draw(&term, None);
    assert_eq!(wire.0.borrow().puts, puts);
    // It gives the front back: everything is drawn, what was written
    // meanwhile too.
    wire.0.borrow_mut().layer.give_back(other).unwrap();
    assert!(!screen.events());
    screen.draw(&term, None);
    assert_shows(&card, 0, &screen, 0, &term);

    // Lent again with the event still unread: the layer refuses the
    // rows, which is no failure, and consoled draws no more until it is
    // in front.
    let other = lend(&wire);
    term.feed(b"\r\nunseen");
    screen.draw(&term, None);
    let puts = wire.0.borrow().puts;
    screen.draw(&term, None);
    assert_eq!(wire.0.borrow().puts, puts);
    card.glass.borrow_mut()[0].1.fill(BARE);
    wire.0.borrow_mut().layer.give_back(other).unwrap();
    assert!(!screen.events());
    screen.draw(&term, None);
    assert_shows(&card, 0, &screen, 0, &term);
}

#[test]
fn with_no_monitor_the_log_keeps_the_screens_and_the_consoles_wait() {
    let mut kernel = FakeKernel::new();
    kernel.add_card(false, &[None, None]);
    let wire = Wire::of(kernel);
    let mut screen = Screen::on(wire.clone(), radix_fbcon::MIN_GRID).unwrap();
    assert_eq!(screen.describe(), ["no screen is plugged in: the consoles show when one is"]);
    assert_eq!((screen.cols(), screen.rows()), UNSEEN_GRID);
    // Nothing was asked of the layer, and nothing is drawn.
    assert_eq!(wire.0.borrow().changes, 0);
    assert!(wire.0.borrow().layer.front().is_none());
    let term = terminal(&screen);
    screen.draw(&term, None);
    assert_eq!(wire.0.borrow().puts, 0);
    // With no card at all there is nothing to wait for.
    let none = Wire::of(FakeKernel::new());
    assert!(Screen::on(none, radix_fbcon::MIN_GRID).is_err());
}

#[test]
fn a_card_that_comes_later_gets_the_consoles_too() {
    let (wire, card) = firmware(640, 400);
    let mut screen = Screen::on(wire.clone(), radix_fbcon::MIN_GRID).unwrap();
    let mut term = terminal(&screen);
    term.feed(b"on every screen");
    screen.draw(&term, None);
    // A card of one smaller screen is added: the layer says the screens
    // changed, and the grid becomes what both can show.
    let virtio = wire.0.borrow_mut().add_card(false, &[Some((480, 320))]);
    assert!(screen.events());
    assert_eq!(
        screen.describe(),
        [
            "console of 60x20 cells with font 8x16 at (80, 40)",
            "screen 2: console of 60x20 cells with font 8x16 at (0, 0)",
        ]
    );
    term.resize(screen.cols(), screen.rows());
    screen.draw(&term, None);
    assert_shows(&card, 0, &screen, 0, &term);
    assert_shows(&virtio, 0, &screen, 1, &term);
    // The picture and the mode value of the arrangement before were let
    // go of: the next ones took their numbers.
    let pictures: Vec<u32> = screen.surfaces.iter().map(|surface| surface.picture).collect();
    assert_eq!(pictures.len(), 2);
    // An event that changes nothing arranges nothing.
    let changes = wire.0.borrow().changes;
    screen.event(&DisplayEvent { kind: EVENT_SCREEN, ..DisplayEvent::default() });
    assert!(!screen.events());
    assert_eq!(wire.0.borrow().changes, changes);
}

#[test]
fn a_monitor_that_comes_changes_size_and_goes_is_followed() {
    let mut kernel = FakeKernel::new();
    let card = kernel.add_card(false, &[None]);
    let wire = Wire::of(kernel);
    let mut screen = Screen::on(wire.clone(), radix_fbcon::MIN_GRID).unwrap();
    let waiting = ["no screen is plugged in: the consoles show when one is"];
    assert_eq!(screen.describe(), waiting);
    // A monitor is plugged in: the consoles show on it.
    wire.0.borrow_mut().found(&card, 0, Some((640, 400)));
    assert!(screen.events());
    assert_eq!(screen.describe(), ["console of 80x25 cells with font 8x16 at (0, 0)"]);
    let mut term = terminal(&screen);
    term.feed(b"here at last");
    screen.draw(&term, None);
    assert_shows(&card, 0, &screen, 0, &term);
    // Its size changes, as a virtual machine's window does when it is
    // dragged: the grid is laid out for the new size.
    wire.0.borrow_mut().found(&card, 0, Some((800, 600)));
    assert!(screen.events());
    assert_eq!(screen.describe(), ["console of 100x37 cells with font 8x16 at (0, 4)"]);
    term.resize(screen.cols(), screen.rows());
    screen.draw(&term, None);
    assert_shows(&card, 0, &screen, 0, &term);
    // It is unplugged: the consoles wait again, and draw nothing.
    wire.0.borrow_mut().found(&card, 0, None);
    assert!(screen.events());
    assert_eq!(screen.describe(), waiting);
    let puts = wire.0.borrow().puts;
    term.feed(b"\r\nunseen");
    screen.draw(&term, None);
    assert_eq!(wire.0.borrow().puts, puts);
}

#[test]
fn lost_events_have_everything_asked_about_and_drawn_again() {
    let (wire, card) = firmware(640, 400);
    let mut screen = Screen::on(wire.clone(), radix_fbcon::MIN_GRID).unwrap();
    let mut term = terminal(&screen);
    term.feed(b"still here");
    screen.draw(&term, None);
    card.glass.borrow_mut()[0].1.fill(BARE);
    screen.event(&DisplayEvent { kind: EVENT_OVERFLOW, ..DisplayEvent::default() });
    assert!(!screen.events());
    screen.draw(&term, None);
    assert_shows(&card, 0, &screen, 0, &term);
}

#[test]
fn a_draw_the_layer_does_not_take_is_drawn_again_whole() {
    // 650 by 410 has margins, which are drawn again too.
    let (wire, card) = firmware(650, 410);
    let mut screen = Screen::on(wire.clone(), radix_fbcon::MIN_GRID).unwrap();
    let mut term = terminal(&screen);
    term.feed(b"drawn");
    screen.draw(&term, None);
    wire.0.borrow_mut().no_memory = true;
    term.feed(b"\r\nnot drawn");
    screen.draw(&term, None);
    assert!(screen.failing);
    // The consoles go on. What a failed card may have left is drawn
    // over when the layer takes a draw again.
    card.glass.borrow_mut()[0].1.fill(BARE);
    term.feed(b"\r\nstill not");
    screen.draw(&term, None);
    wire.0.borrow_mut().no_memory = false;
    screen.draw(&term, None);
    assert!(!screen.failing);
    assert_shows(&card, 0, &screen, 0, &term);
}

#[test]
fn screens_that_change_while_consoled_starts_are_asked_about_again() {
    let (wire, card) = firmware(640, 400);
    wire.0.borrow_mut().changing = 2;
    let mut screen = Screen::on(wire.clone(), radix_fbcon::MIN_GRID).unwrap();
    let term = terminal(&screen);
    screen.draw(&term, None);
    assert_shows(&card, 0, &screen, 0, &term);
    // Screens that never settle are no screens to start on.
    let (wire, _card) = firmware(640, 400);
    wire.0.borrow_mut().changing = usize::MAX;
    assert!(Screen::on(wire, radix_fbcon::MIN_GRID).is_err());
}

#[test]
fn another_holder_in_front_keeps_the_screens_and_the_consoles_stand_under_it() {
    let (wire, _card) = firmware(640, 400);
    // Another holder's change, of nothing, took the front from the log.
    let mut nothing = display::wire::ChangeBytes::new(0, 0);
    let took = wire.0.borrow_mut().request(OTHER, CHANGE, 0, nothing.bytes_mut());
    assert!(took.is_ok());
    let changes = wire.0.borrow().changes;
    let mut screen = Screen::on(wire.clone(), radix_fbcon::MIN_GRID).unwrap();
    // It asked once, and not again, and draws nothing behind the other.
    assert!(screen.away());
    assert_eq!(screen.describe(), ["a program has the screens: the consoles show when it is done"]);
    assert!(!screen.events() && !screen.tick(Instant::now()));
    assert_eq!(wire.0.borrow().changes, changes + 1);
    let front = wire.0.borrow().layer.front();
    assert_ne!(front, Some(wire.holder()));
}
