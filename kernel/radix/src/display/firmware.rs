// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The firmware's framebuffer as a card (ADR 0705): `firmware0`, one
//! screen in the one mode the boot loader left it in, where the kernel's
//! log shows while no holder is in front.
//!
//! The card has no memory but the screen's own. A frame is drawn by
//! copying the main plane's picture into the framebuffer: all of it when
//! the picture is new, and otherwise the rectangles a change names. The
//! pointer is blended in on the way (`radix_display::pixels`), a row at
//! a time, so the picture's own pixels are never altered and the
//! framebuffer, which is slow to read, never read: what was under the
//! pointer comes back from the picture when the pointer moves on. A
//! framebuffer in the pictures' own layout takes a row with no pointer
//! on it straight from the picture's pages.
//!
//! A picture the card keeps has no pixels here either: PUT's rows go
//! straight to the screen while the picture shows, and are dropped while
//! it does not. Its holder sends them again when it is in front, as
//! radix-display(2) says it must. With nothing to restore from under a
//! pointer, a pointer over such a picture is refused.
//!
//! The framebuffer is the kernel console's (`crate::console`), which
//! draws the log on it and, in a panic, the report. Every write here
//! goes through [`console::with_pixels`], with the console's lock held
//! for a row, or for a few short ones ([`HOLD_BYTES`]), and writes
//! nothing once the log or the panic report has the screen: the report
//! shows whatever a holder was doing.
//!
//! The firmware says nothing of the monitor's rate, so the card's beat
//! is the kernel clock's at 60 Hz. Nothing here waits for the monitor:
//! a change that asks for now is on the screen when the call returns.
//!
//! The framebuffer is memory of some device, which the firmware set up
//! to show it. When a driver takes that device the card steps aside
//! ([`step_aside`]): the driver's card has the screen from then on, and
//! what the firmware set up is the driver's to change.

use alloc::boxed::Box;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, Ordering};

use radix_abi::Status;
use radix_abi::display::{
    CARD_FIRMWARE, FORMAT_ARGB8888, FORMAT_XRGB8888, MODE_PREFERRED, Mode, PLANE_POINTER,
    REASON_NOT_OFFERED, SIDE_MAX,
};
use radix_display::Rect;
use radix_display::pixels::{self, Channel, Layout};
use radix_display::state::PictureShape;
use radix_hw::pci::{Bar, Function};
use radix_klog::{info, warn};
use radix_mm::Kmalloc;
use radix_mm::bytes::zeroed;
use radix_sync::Mutex;

use super::{
    CardSpec, Change, Driver, Failed, Frame, Pages, Picture, PlaneFault, PlaneSpec, ScreenSpec,
    Shown, add_card, kmalloc, name,
};
use crate::console;

/// The rate the card's beat is kept at, in frames in 1000 seconds.
const REFRESH_MHZ: u32 = 60_000;
/// The longest side of a pointer's picture.
const POINTER_SIDE: u32 = 256;
/// The most bytes of rows copied to the screen in one hold of the
/// console's lock, which masks interrupts: one row at least, whatever
/// its length, and as many shorter rows as come to this. A row of a
/// screen 3840 pixels wide is 15 KiB.
const HOLD_BYTES: usize = 16 << 10;

/// The card's number in the layer and where its framebuffer starts in
/// physical memory, while the layer has the card.
static CARD: Mutex<Option<(u32, u64)>> = Mutex::new(None);

/// The firmware's framebuffer.
struct Firmware {
    width: u32,
    height: u32,
    /// The bytes from one row of the framebuffer to the next.
    pitch: usize,
    /// How a pixel of the framebuffer is laid out.
    layout: Layout,
    /// Whether a holder draws on the screen, and the log does not.
    handed: AtomicBool,
    drawn: Mutex<Drawn>,
}

/// What the card keeps between frames.
struct Drawn {
    /// Where the pointer shows on the screen, cut to it.
    pointer: Option<Rect>,
    /// A row of the frame, for the pointer to be blended over.
    row: Vec<u8, Kmalloc>,
    /// A row of the pointer's picture.
    over: Vec<u8, Kmalloc>,
}

/// Adds the firmware's framebuffer to the layer, if there is one and
/// pictures can be written in its layout.
pub(super) fn add() {
    let Some((width, height, pitch, format)) = console::screen() else { return };
    let channel = |from: radix_hal::Channel| Channel { shift: from.shift, bits: from.bits };
    let layout = Layout {
        red: channel(format.red),
        green: channel(format.green),
        blue: channel(format.blue),
        bytes: format.bytes_per_pixel,
    };
    let side =
        |pixels: usize| u32::try_from(pixels).ok().filter(|side| (1..=SIDE_MAX).contains(side));
    let (Some(width), Some(height)) = (side(width), side(height)) else { return };
    if !layout.is_usable() || pitch < width as usize * usize::from(layout.bytes) {
        info!(
            "Display: the firmware's framebuffer shows the log alone: no picture fits its layout"
        );
        return;
    }
    let mode = Mode {
        width,
        height,
        refresh_mhz: REFRESH_MHZ,
        flags: MODE_PREFERRED,
        clock_khz: 0,
        h_sync_start: 0,
        h_sync_end: 0,
        h_total: 0,
        v_sync_start: 0,
        v_sync_end: 0,
        v_total: 0,
    };
    let formats = |format: u32| [format, 0, 0, 0, 0, 0, 0, 0];
    let spec = CardSpec {
        kind: CARD_FIRMWARE,
        name: name("firmware0"),
        model: name("Firmware framebuffer"),
        // It sets no mode, mirrors nothing, and its one screen stays.
        flags: 0,
        width_min: width,
        height_min: height,
        width_max: width,
        height_max: height,
        pipes: 1,
        screens: &[ScreenSpec { plugged: true, fixed_pipe: Some(0), native: mode }],
        main: PlaneSpec { width_max: width, height_max: height, formats: formats(FORMAT_XRGB8888) },
        pointer: PlaneSpec {
            width_max: POINTER_SIDE,
            height_max: POINTER_SIDE,
            formats: formats(FORMAT_ARGB8888),
        },
    };
    let made = add_card(&spec, |_| {
        let drawn = Drawn {
            pointer: None,
            row: zeroed(width as usize * 4, kmalloc()).ok()?,
            over: zeroed(POINTER_SIDE as usize * 4, kmalloc()).ok()?,
        };
        let card = Firmware {
            width,
            height,
            pitch,
            layout,
            handed: AtomicBool::new(false),
            drawn: Mutex::new(drawn),
        };
        let driver: Box<dyn Driver, Kmalloc> = Box::try_new_in(card, kmalloc()).ok()?;
        Some(driver)
    });
    match made {
        Ok(card) => {
            *CARD.lock() = console::framebuffer_start().map(|start| (card.id, start));
            info!("Display: firmware0 is the firmware's framebuffer, {width}x{height}");
        }
        Err(why) => warn!("Display: the firmware's framebuffer shows the log alone: {why}"),
    }
}

/// Takes the firmware's card away if its framebuffer is memory of
/// `function`, a device whose driver has just added a card of its own
/// (ADR 0705). The kernel's log passes to that card, and holders are
/// told the screens changed. Returns whether the card went.
pub(super) fn step_aside(function: &Function) -> bool {
    let mut card = CARD.lock();
    let Some((id, start)) = *card else { return false };
    let holds = |bar| match function.bar(bar) {
        Some(Bar::Memory { address, size, .. }) => start >= address && start - address < size,
        _ => false,
    };
    if !(0..6).any(holds) {
        return false;
    }
    *card = None;
    drop(card);
    super::remove_card(id)
}

/// What a row of a frame is drawn from.
struct Source<'a> {
    /// The main plane's picture and its pages: black with none.
    main: Option<(&'a Shown<'a>, &'a Pages)>,
    /// The pointer's picture, where one shows.
    pointer: Option<&'a Shown<'a>>,
}

impl Firmware {
    /// Draws `rect` of `frame` on the screen: the main plane's picture,
    /// or black where the pipe shows none, with the pointer over it. A
    /// main picture the card keeps is left as the screen has it.
    ///
    /// Each hold of the console's lock, which masks interrupts, copies
    /// one row, or as many short ones as come to [`HOLD_BYTES`].
    fn draw(&self, drawn: &mut Drawn, frame: &Frame<'_>, rect: Rect) {
        let Some(rect) = rect.intersect(Rect::sized(self.width, self.height)) else { return };
        let main = frame.main.as_ref().filter(|_| frame.active);
        let main = match main.map(|main| (main, main.picture.pages.as_ref())) {
            Some((main, Some(pages))) => Some((main, pages)),
            // Its rows come by PUT.
            Some((_, None)) => return,
            None => None,
        };
        let source = Source { main, pointer: frame.pointer.as_ref().filter(|_| frame.active) };
        let row_bytes = rect.width as usize * usize::from(self.layout.bytes);
        let at_once = (HOLD_BYTES / row_bytes.max(1)).max(1);
        static FBDIAG_LOGGED: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
        let fbdiag_whole = rect.width == self.width && rect.height == self.height;
        let fbdiag_started = <crate::Arch as radix_hal::Timer>::now_ns();
        let mut rows = (rect.y..).take(rect.height as usize).peekable();
        while rows.peek().is_some() {
            let held = console::with_pixels(|screen| {
                for y in rows.by_ref().take(at_once) {
                    self.draw_row(screen, drawn, &source, Rect { y, height: 1, ..rect });
                }
            });
            // The log has the screen, or the kernel's report.
            if held.is_none() {
                return;
            }
        }
        if fbdiag_whole && FBDIAG_LOGGED.fetch_add(1, Ordering::Relaxed) % 64 < 4 {
            let took = <crate::Arch as radix_hal::Timer>::now_ns() - fbdiag_started;
            let start = source.main.map(|(_, pages)| pages.start);
            let over = source.pointer.is_some();
            radix_klog::info!(
                "FBDIAG draw: {took} ns, picture starts at {start:?} in its page, pointer {over}, \
                 layout XRGB8888 {}, rows a hold {at_once}",
                self.layout == Layout::XRGB8888
            );
        }
    }

    /// Draws `rect`, one row of the frame that is on the screen, into
    /// `screen`, the framebuffer's bytes.
    fn draw_row(&self, screen: &mut [u8], drawn: &mut Drawn, source: &Source<'_>, rect: Rect) {
        let (width, pixel) = (rect.width as usize, usize::from(self.layout.bytes));
        let at = rect.y as usize * self.pitch + rect.x as usize * pixel;
        let Some(target) = screen.get_mut(at..at + width * pixel) else { return };
        // The part of the row the pointer is over, and where that part's
        // pixels are in the pointer's picture.
        let over = source.pointer.and_then(|pointer| {
            let part = rect.intersect(pointer.place())?;
            Some((part, pointer.byte_at(part.x, rect.y)?, pointer.picture.pages.as_ref()?))
        });
        let from =
            source.main.and_then(|(main, pages)| Some((main.byte_at(rect.x, rect.y)?, pages)));
        let Some(row) = drawn.row.get_mut(..width * 4) else { return };
        match from {
            // One copy, from the picture's pages to the screen.
            Some((from, pages)) if over.is_none() && self.layout == Layout::XRGB8888 => {
                return pages.read(from, target);
            }
            Some((from, pages)) => pages.read(from, row),
            None => row.fill(0),
        }
        if let Some((part, from, pages)) = over {
            let skip = part.x.abs_diff(rect.x) as usize * 4;
            let len = part.width as usize * 4;
            if let (Some(under), Some(over)) =
                (row.get_mut(skip..skip + len), drawn.over.get_mut(..len))
            {
                pages.read(from, over);
                pixels::blend(under, over);
            }
        }
        pixels::store(self.layout, row, target);
    }

    /// Writes `row`, the pixels PUT sends for `rect`, one row of the
    /// picture on the main plane `main`, to `screen`, the framebuffer's
    /// bytes: where the row shows in the frame, cut to the screen.
    fn put_row(&self, screen: &mut [u8], main: &Shown<'_>, rect: Rect, row: &[u8]) {
        let whole =
            Rect { x: rect.x - main.source.x + main.x, y: rect.y - main.source.y + main.y, ..rect };
        let Some(part) = whole.intersect(Rect::sized(self.width, self.height)) else { return };
        let skip = part.x.abs_diff(whole.x) as usize * 4;
        let Some(row) = row.get(skip..skip + part.width as usize * 4) else { return };
        let pixel = usize::from(self.layout.bytes);
        let at = part.y as usize * self.pitch + part.x as usize * pixel;
        if let Some(target) = screen.get_mut(at..at + part.width as usize * pixel) {
            pixels::store(self.layout, row, target);
        }
    }
}

impl Driver for Firmware {
    fn shows_log(&self) -> bool {
        true
    }

    fn log_takes(&self, _frame: &Frame<'_>) {
        self.drawn.lock().pointer = None;
        // Not at boot, when the log has the screen already and the
        // startup screen may show on it.
        if self.handed.swap(false, Ordering::AcqRel) {
            console::give_back();
        }
    }

    fn log_gives(&self) {
        self.handed.store(true, Ordering::Release);
        console::hand_over();
    }

    fn check(&self, frame: &Frame<'_>) -> Result<(), PlaneFault> {
        // The pointer is blended over the picture's own pixels, and one
        // the card keeps has none here.
        let kept = frame.main.as_ref().is_some_and(|main| main.picture.pages.is_none());
        if kept && frame.pointer.is_some() {
            return Err(PlaneFault { kind: PLANE_POINTER, reason: REASON_NOT_OFFERED });
        }
        Ok(())
    }

    fn make_picture(
        &self,
        _shape: &PictureShape,
        _row_bytes: u32,
        _pages: Option<&Pages>,
    ) -> Result<u64, Status> {
        // Nothing to make: a picture's pages are read as they are.
        Ok(0)
    }

    fn drop_picture(&self, _picture: &Picture) {}

    fn show(&self, frame: &Frame<'_>, change: &Change<'_>) -> Result<(), Failed> {
        let mut drawn = self.drawn.lock();
        let screen = Rect::sized(self.width, self.height);
        let pointer = frame.pointer.as_ref().filter(|_| frame.active);
        let pointer = pointer.and_then(|pointer| pointer.place().intersect(screen));
        let was = core::mem::replace(&mut drawn.pointer, pointer);
        if change.mode || change.main {
            self.draw(&mut drawn, frame, screen);
            return Ok(());
        }
        for &rect in change.damage {
            self.draw(&mut drawn, frame, rect);
        }
        if change.pointer || change.moved {
            // Where the pointer was the picture shows again, and where
            // it is now the pointer over it.
            for rect in [was, pointer].into_iter().flatten() {
                self.draw(&mut drawn, frame, rect);
            }
        }
        Ok(())
    }

    fn put(&self, _picture: &Picture, rect: Rect, rows: &[u8], on: Option<&Frame<'_>>) {
        let main = on.filter(|frame| frame.active).and_then(|frame| frame.main.as_ref());
        let Some(main) = main else { return };
        let len = rect.width as usize * 4;
        if len == 0 {
            return;
        }
        let at_once = (HOLD_BYTES / len).max(1);
        let mut rows = rows.chunks_exact(len).take(rect.height as usize).zip(rect.y..).peekable();
        while rows.peek().is_some() {
            let held = console::with_pixels(|screen| {
                for (row, y) in rows.by_ref().take(at_once) {
                    self.put_row(screen, main, Rect { y, height: 1, ..rect }, row);
                }
            });
            // The log has the screen, or the kernel's report.
            if held.is_none() {
                return;
            }
        }
    }
}
