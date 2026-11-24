// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The kernel's log on a virtio card: the console's cells, drawn as
//! rows of pixels and sent to the host a row at a time.
//!
//! The console that shows on such a card keeps no picture of its screen
//! (`crate::console`). It says what changed, and what draws its cells
//! (`radix_fbcon::Painter`) makes each row of pixels in one buffer a
//! screen's width long, from where the row goes to the host through the
//! card's text band. A blank screen is one row, which the host copies
//! as often as the screen is high.
//!
//! Nothing wakes the card's thread for a line of the log, which is
//! written with any lock held: while the log is in front, the thread
//! looks fifty times a second for what changed ([`Log::draw`]). While a
//! holder is in front it does not look at all, and the host keeps no
//! picture of the log either ([`Log::release`]).
//!
//! When the kernel stops, its report is drawn with what the card has
//! already ([`Log::report`]): no memory is made, nothing sleeps and no
//! interrupt is waited for. The card is started again first, whatever
//! it was in the middle of.

use alloc::boxed::Box;
use alloc::vec::Vec;

use radix_fbcon::{Draw, Painter};
use radix_hal::Devices;
use radix_hw::Dma;
use radix_hw::pci::Address;
use radix_mm::Kmalloc;
use radix_virtio::Error;
use radix_virtio::gpu::{self, Rows, Target, put_whole};

use super::super::{Change, Frame, PAGE, kmalloc};
use super::device::Device;
use super::room;
use super::shown::Source;
use crate::{Arch, console};

/// The parts of the console drawn at most each time the card's thread
/// looks: a flood of lines does not keep the layer's lock from the
/// holders. What is left is drawn the next time.
const DRAWS_MOST: usize = 512;

/// The kernel's log on a card.
pub(super) struct Log {
    /// What draws the console's cells, from when the log first shows.
    painter: Option<Painter>,
    /// The size of the log's picture, wide and high.
    size: (u32, u32),
    /// The resource the log is drawn in, or 0 while the host has none.
    resource: u32,
    /// The backing of that resource on a card with no band. It is kept
    /// when the resource goes: the kernel's report makes no memory.
    store: Option<Dma>,
    /// Whether a screen shows the resource, so that what is drawn in it
    /// is shown as it is drawn.
    shown: bool,
    /// Whether the log draws: it is in front, and the card answers.
    draws: bool,
    /// Whether the kernel's report has the card.
    reports: bool,
    /// The cells of a row of the console.
    cells: Vec<u32, Kmalloc>,
    /// A row of pixels.
    row: Vec<u8, Kmalloc>,
}

/// The rows of a part of the console's screen, each drawn when it is
/// asked for.
struct Painted<'a> {
    painter: &'a Painter,
    draw: &'a Draw,
    cells: &'a [u32],
    row: &'a mut [u8],
}

impl Rows for Painted<'_> {
    fn row(&mut self, index: u32) -> &[u8] {
        self.painter.row(self.draw, self.cells, index as usize, self.row);
        self.row
    }
}

/// Makes `list` `len` long, the new places zero, if there is memory
/// for it.
fn resize<T: Default>(list: &mut Vec<T, Kmalloc>, len: usize) -> Result<(), Error> {
    list.try_reserve_exact(len.saturating_sub(list.len())).map_err(|_| Error::NoMemory)?;
    list.truncate(len);
    while list.len() < len {
        // Room was reserved.
        let _ = list.push_within_capacity(T::default());
    }
    Ok(())
}

impl Log {
    /// A log that shows nowhere, on the heap. Kept out of the frame of
    /// whoever makes it: what draws the cells holds a font.
    #[inline(never)]
    pub(super) fn boxed() -> Option<Box<Log, Kmalloc>> {
        let log = Log {
            painter: None,
            size: (0, 0),
            resource: 0,
            store: None,
            shown: false,
            draws: false,
            reports: false,
            cells: Vec::new_in(kmalloc()),
            row: Vec::new_in(kmalloc()),
        };
        Box::try_new_in(log, kmalloc()).ok()
    }

    /// The size of the screen the log was last laid out for.
    pub(super) fn size(&self) -> (u32, u32) {
        self.size
    }

    /// Whether the log draws, and is to be looked at for what changed.
    pub(super) fn draws(&self) -> bool {
        self.draws
    }

    /// The log is in front: has the screens of `frame` show it, at the
    /// size of the frame's mode, for which the console has been laid
    /// out. A new size, and a turn after a holder's, get a new picture.
    /// All of the console that waits is drawn in the picture before a
    /// screen shows it, and the picture there was goes only then, so
    /// that the screen never shows half a picture or none.
    pub(super) fn take(
        &mut self,
        device: &mut Device,
        frame: &Frame<'_>,
        wait: &mut dyn FnMut() -> bool,
    ) -> Result<(), Error> {
        let size = (frame.mode.width, frame.mode.height);
        self.draws = false;
        let mut old = 0;
        if self.size != size {
            old = core::mem::take(&mut self.resource);
        }
        if device.band.is_none() {
            // A store is the next picture's too, so the old one goes
            // first.
            device.forget(core::mem::take(&mut old), wait);
        }
        let shown = self.show(device, frame, size, wait);
        device.forget(old, wait);
        shown
    }

    /// [`Log::take`], once the picture there was is out of the way.
    fn show(
        &mut self,
        device: &mut Device,
        frame: &Frame<'_>,
        size: (u32, u32),
        wait: &mut dyn FnMut() -> bool,
    ) -> Result<(), Error> {
        if self.resource == 0 {
            self.resource = self.make(device, size, wait)?;
        }
        (self.draws, self.shown) = (true, false);
        let shown = self.draw(device, wait).and_then(|()| self.put_on(device, frame, wait));
        (self.draws, self.shown) = (shown.is_ok(), shown.is_ok());
        shown
    }

    /// Makes a picture of `size` for the log, with all of the console
    /// to be drawn in it, and returns its resource.
    fn make(
        &mut self,
        device: &mut Device,
        size: (u32, u32),
        wait: &mut dyn FnMut() -> bool,
    ) -> Result<u32, Error> {
        let (width, height) = (size.0 as usize, size.1 as usize);
        console::painter(&mut self.painter);
        let cols = match &self.painter {
            Some(painter) if painter.screen() == (width, height) => painter.cols(),
            // The console is another screen's.
            _ => return Err(Error::BadRequest),
        };
        resize(&mut self.cells, cols)?;
        resize(&mut self.row, width * 4)?;
        let backing = match &device.band {
            Some(band) => band.backing(),
            None => self.store_for(device.source, width * height * 4)?,
        };
        let resource = device.number()?;
        if let Err(err) = device.make(resource, size, backing, wait) {
            device.forget(resource, wait);
            return Err(err);
        }
        self.size = size;
        console::redraw();
        Ok(resource)
    }

    /// The backing of a picture of `bytes` bytes on a card with no
    /// band, as an address and a length: the store there is, if it is
    /// as large, or a new one.
    fn store_for(&mut self, source: Address, bytes: usize) -> Result<(u64, u32), Error> {
        if self.store.as_ref().is_none_or(|store| store.len() < bytes) {
            // The one there was first: no picture is left that it backs.
            self.store = None;
            self.store = <Arch as Devices>::alloc_dma(source, bytes.div_ceil(PAGE), false);
        }
        let store = self.store.as_ref().ok_or(Error::NoMemory)?;
        Ok((store.phys(), store.len() as u32))
    }

    /// Has the screens of `frame` show all of the picture, with no
    /// pointer over it, and waits for the device to say they do.
    fn put_on(
        &self,
        device: &mut Device,
        frame: &Frame<'_>,
        wait: &mut dyn FnMut() -> bool,
    ) -> Result<(), Error> {
        let whole = gpu::Rect::sized(self.size.0, self.size.1);
        let source = Source { resource: self.resource, shared: None, rect: whole };
        let Device { gpu, shown, .. } = device;
        shown.set_screens(gpu, frame.pipe, frame.screens, Some(source), wait)?;
        // The log's frame has no pointer: one a holder left is hidden.
        let all = Change { mode: true, main: true, pointer: true, moved: false, damage: &[] };
        shown.point(gpu, frame, &all, wait)?;
        room(gpu, wait)?;
        gpu.flush(self.resource, whole)?;
        gpu.finish(wait)
    }

    /// A holder is in front: nothing more is drawn. The picture stays
    /// until the holder's frames are on its screens ([`Log::release`]).
    pub(super) fn give(&mut self) {
        self.draws = false;
    }

    /// Has the host let go of the log's picture, once a holder is in
    /// front and no screen shows the picture any more: a log that does
    /// not show costs the host nothing.
    pub(super) fn release(&mut self, device: &mut Device, wait: &mut dyn FnMut() -> bool) {
        let resource = self.resource;
        let on_screen = device.shown.screens.iter().flatten().any(|on| on.resource == resource);
        if self.draws || resource == 0 || on_screen {
            return;
        }
        (self.resource, self.shown) = (0, false);
        // Nobody waits for it: the host frees it when it comes to it.
        let _ = device.gpu.loosely(|gpu| room(gpu, wait).and_then(|()| gpu.unref(resource)));
    }

    /// Sends the host what waits to be drawn of the console, at most
    /// [`DRAWS_MOST`] parts of it: the startup screen, a blank screen,
    /// or the cells of a row that changed. Nothing while the log does
    /// not draw.
    pub(super) fn draw(
        &mut self,
        device: &mut Device,
        wait: &mut dyn FnMut() -> bool,
    ) -> Result<(), Error> {
        let Some(painter) = self.painter.as_ref().filter(|_| self.draws && self.resource != 0)
        else {
            return Ok(());
        };
        let to = Target { resource: self.resource, width: self.size.0, shown: self.shown };
        for _ in 0..DRAWS_MOST {
            let Some(draw) = console::next_draw(&mut self.cells) else { break };
            let (x, y, width, height) = painter.area(&draw);
            let rect =
                gpu::Rect { x: x as u32, y: y as u32, width: width as u32, height: height as u32 };
            let Some(row) = self.row.get_mut(..width * 4).filter(|_| width * height > 0) else {
                continue;
            };
            let Device { gpu, band, .. } = device;
            match (band, &self.store) {
                (Some(band), _) if draw == Draw::Blank => {
                    painter.row(&draw, &[], 0, row);
                    band.fill(gpu, to, rect, row, wait)?;
                }
                (Some(band), _) => {
                    let mut rows = Painted { painter, draw: &draw, cells: &self.cells, row };
                    band.put(gpu, to, rect, &mut rows, wait)?;
                }
                (None, Some(store)) => {
                    let mut rows = Painted { painter, draw: &draw, cells: &self.cells, row };
                    put_whole(gpu, store, to, rect, &mut rows, wait)?;
                }
                (None, None) => return Err(Error::NoMemory),
            }
        }
        Ok(())
    }

    /// The kernel has stopped with its report on the console: draws the
    /// console on the screens the card last found a monitor on, at the
    /// size the log last had. The first call starts the card again, and
    /// draws all of the console; a later one draws what changed since.
    ///
    /// A card that has never shown the log shows no report: it would
    /// have to make the memory to draw it with.
    pub(super) fn report(&mut self, device: &mut Device, wait: &mut dyn FnMut() -> bool) {
        if self.painter.is_none() {
            return;
        }
        let drawn = if core::mem::replace(&mut self.reports, true) {
            self.draw(device, wait).and_then(|()| device.gpu.finish(wait))
        } else {
            self.start_report(device, wait)
        };
        // A card that fails is asked nothing more.
        self.draws = drawn.is_ok();
    }

    /// The first call of [`Log::report`]: the card is started again, so
    /// the host has no picture and shows nothing, and the log gets a
    /// picture anew with all of the console in it.
    fn start_report(
        &mut self,
        device: &mut Device,
        wait: &mut dyn FnMut() -> bool,
    ) -> Result<(), Error> {
        (self.resource, self.shown, self.draws) = (0, false, false);
        if !device.gpu.restart() {
            return Err(Error::DeviceFailed);
        }
        device.shown.forget_all();
        let (width, height) = (self.size.0 as usize, self.size.1 as usize);
        console::painter(&mut self.painter);
        let fits = self.painter.as_ref().is_some_and(|painter| {
            painter.screen() == (width, height) && painter.cols() <= self.cells.len()
        });
        let backing = match (&device.band, &self.store) {
            (Some(band), _) => band.backing(),
            (None, Some(store)) => (store.phys(), store.len() as u32),
            (None, None) => return Err(Error::NoMemory),
        };
        if !fits || self.row.len() < width * 4 {
            return Err(Error::BadRequest);
        }
        let resource = device.number()?;
        device.make(resource, self.size, backing, wait)?;
        (self.resource, self.draws) = (resource, true);
        console::redraw();
        self.draw(device, wait)?;
        let whole = gpu::Rect::sized(self.size.0, self.size.1);
        let none = !device.found.iter().any(|screen| screen.enabled);
        let Device { gpu, shown, found, .. } = device;
        for (index, screen) in found.iter().enumerate().take(shown.count) {
            // On the first screen if no monitor was found: a host may
            // show it all the same.
            if screen.enabled || (none && index == 0) {
                room(gpu, wait)?;
                gpu.set_scanout(index as u32, resource, whole)?;
            }
        }
        room(gpu, wait)?;
        gpu.flush(resource, whole)?;
        gpu.finish(wait)?;
        self.shown = true;
        Ok(())
    }
}
