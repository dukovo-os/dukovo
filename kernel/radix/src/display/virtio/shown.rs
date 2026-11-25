// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! What a virtio card's screens show, and the commands that change it.
//!
//! A screen of the device shows a rectangle of a resource, or nothing.
//! [`Shown`] keeps what each was last told, so that a frame costs only
//! the commands for what differs: a screen is set when its resource or
//! the part of it shown is new, the rows that changed are copied to the
//! host and shown, and the pointer is drawn, moved or hidden.
//!
//! The pointer is the device's own: one picture of 64 pixels a side for
//! the card ([`POINTER`]), which a screen is told to show at a place. A
//! pointer's picture is copied into it when it is new. The place the
//! device is told is where the pointer points, its hot spot, which is
//! never off a screen's left or top; the device is told the hot spot's
//! place in the picture with it.

use radix_display::Rect;
use radix_display::frame::Shown as Plane;
use radix_hw::Dma;
use radix_virtio::Error;
use radix_virtio::gpu::{self, Gpu, POINTER_SIDE, Place, SCANOUTS_MAX, Shared, format};

use super::super::{Change, Frame, Picture};
use super::{SHARED, room};

/// The resource the pointer is drawn in. Pictures' count on from it.
pub(super) const POINTER: u32 = 1;

/// What a screen shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Shows {
    /// The pipe that feeds it.
    pub(super) pipe: usize,
    pub(super) resource: u32,
    /// The part of the resource that shows.
    pub(super) rect: gpu::Rect,
    /// Whether the pointer shows on it.
    pub(super) pointer: bool,
}

/// What to show on a screen: a part of a resource.
#[derive(Clone, Copy)]
pub(super) struct Source {
    pub(super) resource: u32,
    /// How its pages are laid out, for one the host shares.
    pub(super) shared: Option<Shared>,
    pub(super) rect: gpu::Rect,
}

/// Queues the command that has screen `scanout` show `source`.
fn set(gpu: &mut Gpu, scanout: u32, source: &Source) -> Result<(), Error> {
    match source.shared {
        Some(shared) => gpu.set_scanout_shared(scanout, &shared, source.rect),
        None => gpu.set_scanout(scanout, source.resource, source.rect),
    }
}

/// The resource of a picture, and how its pages are laid out if the
/// host shares them.
fn resource_of(picture: &Picture) -> (u32, Option<Shared>) {
    let resource = picture.handle as u32;
    let shared = (picture.handle & SHARED != 0).then_some(Shared {
        resource,
        format: format::BGRX,
        width: picture.shape.width,
        height: picture.shape.height,
        row_bytes: picture.row_bytes,
    });
    (resource, shared)
}

/// What the screens show, and the pointer's picture.
pub(super) struct Shown {
    /// What each screen shows, by its place.
    pub(super) screens: [Option<Shows>; SCANOUTS_MAX],
    /// How many screens the card has.
    pub(super) count: usize,
    /// The backing of the pointer's resource.
    pub(super) pointer: Dma,
}

impl Shown {
    /// Queues what makes the screens of `frame`'s pipe show it: the
    /// screens set, the rows of its picture that changed, the pointer.
    pub(super) fn frame(
        &mut self,
        gpu: &mut Gpu,
        frame: &Frame<'_>,
        change: &Change<'_>,
        wait: &mut dyn FnMut() -> bool,
    ) -> Result<(), Error> {
        let main = frame.main.as_ref().filter(|_| frame.active);
        let source = main.and_then(|main| {
            let (resource, shared) = resource_of(main.picture);
            Some(Source { resource, shared, rect: in_picture(main, main.place())? })
        });
        self.set_screens(gpu, frame.pipe, frame.screens, source, wait)?;
        if let Some(main) = main {
            send(gpu, main, change, wait)?;
        }
        self.point(gpu, frame, change, wait)
    }

    /// Has each screen of `screens`, a bit for each, show `source` for
    /// pipe `pipe`, and each other screen the pipe fed show nothing.
    pub(super) fn set_screens(
        &mut self,
        gpu: &mut Gpu,
        pipe: usize,
        screens: u32,
        source: Option<Source>,
        wait: &mut dyn FnMut() -> bool,
    ) -> Result<(), Error> {
        for index in 0..self.count {
            let was = self.screens[index];
            let place = Place { scanout: index as u32, x: 0, y: 0 };
            match (source.filter(|_| screens & 1 << index != 0), was) {
                (Some(source), _) => {
                    // Another pipe's pointer is not this one's.
                    let others = was.is_some_and(|was| was.pointer && was.pipe != pipe);
                    if others {
                        gpu.set_pointer(place, 0, (0, 0), wait)?;
                    }
                    let pointer = was.is_some_and(|was| was.pointer) && !others;
                    let now = Shows { pipe, resource: source.resource, rect: source.rect, pointer };
                    if was != Some(now) {
                        room(gpu, wait)?;
                        set(gpu, place.scanout, &source)?;
                    }
                    self.screens[index] = Some(now);
                }
                (None, Some(was)) if was.pipe == pipe => {
                    if was.pointer {
                        gpu.set_pointer(place, 0, (0, 0), wait)?;
                    }
                    room(gpu, wait)?;
                    gpu.set_scanout(place.scanout, 0, gpu::Rect::default())?;
                    self.screens[index] = None;
                }
                _ => {}
            }
        }
        Ok(())
    }

    /// Shows, moves or hides the pointer on the screens `frame`'s pipe
    /// feeds.
    pub(super) fn point(
        &mut self,
        gpu: &mut Gpu,
        frame: &Frame<'_>,
        change: &Change<'_>,
        wait: &mut dyn FnMut() -> bool,
    ) -> Result<(), Error> {
        let pointer = frame.pointer.as_ref().filter(|_| frame.active && frame.main.is_some());
        let mut drawn = false;
        for index in 0..self.count {
            let Some(mut shows) = self.screens[index].filter(|shows| shows.pipe == frame.pipe)
            else {
                continue;
            };
            let scanout = index as u32;
            match pointer {
                Some(pointer) => {
                    let hot = (u32::from(pointer.hot_spot.0), u32::from(pointer.hot_spot.1));
                    let at = |from: i32, hot: u32| from.saturating_add_unsigned(hot).max(0) as u32;
                    let place = Place { scanout, x: at(pointer.x, hot.0), y: at(pointer.y, hot.1) };
                    if change.pointer || !shows.pointer {
                        if !drawn {
                            self.draw_pointer(gpu, pointer, wait)?;
                            drawn = true;
                        }
                        gpu.set_pointer(place, POINTER, hot, wait)?;
                    } else if change.moved {
                        gpu.move_pointer(place, wait)?;
                    }
                    shows.pointer = true;
                }
                None if shows.pointer => {
                    gpu.set_pointer(Place { scanout, x: 0, y: 0 }, 0, (0, 0), wait)?;
                    shows.pointer = false;
                }
                None => {}
            }
            self.screens[index] = Some(shows);
        }
        Ok(())
    }

    /// Draws the pointer's picture in the pointer's resource: the part
    /// of it that shows at the top left, clear around it. The host
    /// reads the resource when it is told of the pointer, on the
    /// pointer's own queue, so the copy is waited for.
    fn draw_pointer(
        &self,
        gpu: &mut Gpu,
        pointer: &Plane<'_, Picture>,
        wait: &mut dyn FnMut() -> bool,
    ) -> Result<(), Error> {
        const SIDE: usize = POINTER_SIDE as usize;
        let pages = pointer.picture.pages.as_ref().ok_or(Error::BadRequest)?;
        let source = pointer.source;
        let across = (source.width as usize).min(SIDE) * 4;
        let mut row = [0u8; SIDE * 4];
        for y in 0..SIDE {
            row.fill(0);
            if y < source.height as usize {
                let from = (source.y as usize + y) * pointer.picture.row_bytes as usize;
                pages.read(from + source.x as usize * 4, &mut row[..across]);
            }
            self.pointer.write(y * SIDE * 4, &row);
        }
        room(gpu, wait)?;
        gpu.transfer(POINTER, gpu::Rect::sized(POINTER_SIDE, POINTER_SIDE), 0)?;
        gpu.finish(wait)
    }

    /// The host shows nothing and has no pictures: the card was started
    /// again.
    pub(super) fn forget_all(&mut self) {
        self.screens = [None; SCANOUTS_MAX];
    }
}

/// The rectangle of `main`'s picture that shows at `rect` of the
/// frame, if all of that is in the picture.
fn in_picture(main: &Plane<'_, Picture>, rect: Rect) -> Option<gpu::Rect> {
    let x = u32::try_from(i64::from(rect.x) - i64::from(main.x) + i64::from(main.source.x)).ok()?;
    let y = u32::try_from(i64::from(rect.y) - i64::from(main.y) + i64::from(main.source.y)).ok()?;
    let shape = main.picture.shape;
    let inside = u64::from(x) + u64::from(rect.width) <= u64::from(shape.width)
        && u64::from(y) + u64::from(rect.height) <= u64::from(shape.height);
    inside.then_some(gpu::Rect { x, y, width: rect.width, height: rect.height })
}

/// Queues the copies that bring the host's picture up to date with the
/// pages of `main`'s, and has the screens show them: all that shows
/// when the frame or its picture is new, and otherwise the rectangles
/// the change names. A picture the host shares is only shown. One the
/// card keeps has its rows by PUT, which shows them: it is shown here
/// only when it is new to its screens.
fn send(
    gpu: &mut Gpu,
    main: &Plane<'_, Picture>,
    change: &Change<'_>,
    wait: &mut dyn FnMut() -> bool,
) -> Result<(), Error> {
    let (resource, shared) = resource_of(main.picture);
    let all = change.mode || change.main;
    let copied = main.picture.pages.is_some() && shared.is_none();
    if main.picture.pages.is_none() && !all {
        return Ok(());
    }
    let whole = [main.place()];
    let rects = if all { &whole[..] } else { change.damage };
    for rect in rects {
        let Some(rect) = rect.intersect(whole[0]).and_then(|rect| in_picture(main, rect)) else {
            continue;
        };
        if copied {
            let at = u64::from(rect.y) * u64::from(main.picture.row_bytes) + u64::from(rect.x) * 4;
            room(gpu, wait)?;
            gpu.transfer(resource, rect, at)?;
        }
        room(gpu, wait)?;
        gpu.flush(resource, rect)?;
    }
    Ok(())
}
