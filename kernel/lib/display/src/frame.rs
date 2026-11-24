// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! What the layer and a card say to each other.
//!
//! A card's driver describes the card once ([`CardSpec`]) and from then
//! on does what the layer asks through [`Card`]. The layer asks in whole
//! frames: a [`Frame`] is everything one pipe is to show, and a
//! [`Change`] says what of it differs from the frame the pipe shows now,
//! so that a driver redraws or sends no more than that. A driver keeps
//! no state of its own about who is in front or what waits: each call
//! carries all it needs.

use alloc::sync::Arc;
use core::alloc::Allocator;

use radix_abi::display::Mode;

use crate::Rect;
use crate::state::PictureShape;

/// A picture, as far as the layer looks at one. What its pixels are kept
/// in is its card's and the kernel's to know.
pub trait Picture {
    /// What it is: its card, its format and its size.
    fn shape(&self) -> PictureShape;
    /// The bytes from one of its rows to the next.
    fn row_bytes(&self) -> u32;
}

/// A card did not do what it was asked: its device stopped answering.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Failed;

/// Why a card cannot show a frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PlaneFault {
    /// The kind of the plane at fault, a PLANE_MAIN or PLANE_POINTER.
    pub kind: u32,
    /// A REASON_*.
    pub reason: u32,
}

/// A plane's picture in a frame.
pub struct Shown<'a, P> {
    /// The picture.
    pub picture: &'a P,
    /// The part of the picture that shows.
    pub source: Rect,
    /// Where in the frame its left column is; before the frame for a
    /// pointer that hangs over the edge.
    pub x: i32,
    /// Where its top row is.
    pub y: i32,
    /// The point of a pointer's picture its place puts.
    pub hot_spot: (u16, u16),
}

impl<P: Picture> Shown<'_, P> {
    /// The rectangle of the frame the picture covers.
    pub fn place(&self) -> Rect {
        Rect { x: self.x, y: self.y, width: self.source.width, height: self.source.height }
    }

    /// Where the pixel that shows at (`x`, `y`) of the frame is in the
    /// picture's bytes, if the picture has one there. For a picture of
    /// four bytes to the pixel, as every format of release 1.1 is.
    pub fn byte_at(&self, x: i32, y: i32) -> Option<usize> {
        let shape = self.picture.shape();
        let column = i64::from(x) - i64::from(self.x) + i64::from(self.source.x);
        let row = i64::from(y) - i64::from(self.y) + i64::from(self.source.y);
        let column = u32::try_from(column).ok().filter(|&column| column < shape.width)?;
        let row = u32::try_from(row).ok().filter(|&row| row < shape.height)?;
        Some(row as usize * self.picture.row_bytes() as usize + column as usize * 4)
    }
}

/// What one pipe is to show.
pub struct Frame<'a, P> {
    /// The pipe's place among those of its card.
    pub pipe: usize,
    /// Whether it makes frames.
    pub active: bool,
    /// Its mode, while it makes frames.
    pub mode: Mode,
    /// The screens it feeds: a bit for each by its place on the card.
    pub screens: u32,
    /// Its main plane's picture.
    pub main: Option<Shown<'a, P>>,
    /// Its pointer plane's picture.
    pub pointer: Option<Shown<'a, P>>,
}

/// What of a frame differs from the one its pipe shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Change<'d> {
    /// Whether it makes frames, its mode, or the screens it feeds.
    pub mode: bool,
    /// The main plane's picture, the part of it shown, or where: all of
    /// the frame is to be drawn.
    pub main: bool,
    /// The pointer's picture or its hot spot.
    pub pointer: bool,
    /// Where the pointer is, and nothing else of it.
    pub moved: bool,
    /// Rectangles of the frame whose pixels changed in the main plane's
    /// picture.
    pub damage: &'d [Rect],
}

/// What the layer asks of a card. The layer is called with its lock
/// held, so a card is asked one thing at a time; a driver may sleep in a
/// call, as one that waits for its device to answer does, and bounds how
/// long.
pub trait Card<P> {
    /// Whether it can show the kernel's log while no holder is in front.
    fn shows_log(&self) -> bool;
    /// The kernel's log is in front: `frame`, of the pipe that feeds the
    /// card's first plugged screen, is the log's to draw in.
    fn log_takes(&self, frame: &Frame<'_, P>);
    /// A holder is in front: the log draws no more.
    fn log_gives(&self);
    /// Whether it can show `frame`, as far as only it knows: the layer
    /// has checked the rest.
    ///
    /// # Errors
    ///
    /// The plane it cannot show as asked, and why.
    fn check(&self, frame: &Frame<'_, P>) -> Result<(), PlaneFault>;
    /// Makes a pipe show `frame`, which passed `check`; `change` says
    /// what of it is new.
    ///
    /// # Errors
    ///
    /// [`Failed`] if its device did not confirm it.
    fn show(&self, frame: &Frame<'_, P>, change: &Change<'_>) -> Result<(), Failed>;
    /// Writes `rows` into `rect` of `picture`, which it keeps: each row
    /// is the rectangle's width in pixels of FORMAT_XRGB8888. `on` is
    /// the frame that shows the picture on its main plane, if one does,
    /// whose screens are to show the rows at once.
    fn put(&self, picture: &P, rect: Rect, rows: &[u8], on: Option<&Frame<'_, P>>);
    /// How long before a beat it needs a change to have it ready.
    fn latch_ns(&self) -> u64 {
        0
    }
    /// Something now waits for the beat at `at_ns`: whatever calls
    /// [`crate::layer::Layer::beat`] for the card is to do so by then.
    fn wake_for(&self, at_ns: u64);
}

impl<P, T: Card<P> + ?Sized, A: Allocator> Card<P> for Arc<T, A> {
    fn shows_log(&self) -> bool {
        (**self).shows_log()
    }

    fn log_takes(&self, frame: &Frame<'_, P>) {
        (**self).log_takes(frame);
    }

    fn log_gives(&self) {
        (**self).log_gives();
    }

    fn check(&self, frame: &Frame<'_, P>) -> Result<(), PlaneFault> {
        (**self).check(frame)
    }

    fn show(&self, frame: &Frame<'_, P>, change: &Change<'_>) -> Result<(), Failed> {
        (**self).show(frame, change)
    }

    fn put(&self, picture: &P, rect: Rect, rows: &[u8], on: Option<&Frame<'_, P>>) {
        (**self).put(picture, rect, rows, on);
    }

    fn latch_ns(&self) -> u64 {
        (**self).latch_ns()
    }

    fn wake_for(&self, at_ns: u64) {
        (**self).wake_for(at_ns);
    }
}

/// What a card's planes of one kind take.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PlaneSpec {
    /// The widest picture.
    pub width_max: u32,
    /// The tallest.
    pub height_max: u32,
    /// FORMAT_* numbers, padded with zeros.
    pub formats: [u32; 8],
}

/// A screen of a card being added.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ScreenSpec {
    /// Whether a monitor is plugged in.
    pub plugged: bool,
    /// The one pipe of the card that can feed it, by its place, if only
    /// one can.
    pub fixed_pipe: Option<usize>,
    /// The mode it was found in.
    pub native: Mode,
}

/// A card being added: every pipe gets a main plane and a pointer plane
/// that are its alone.
#[derive(Clone, Copy, Debug)]
pub struct CardSpec<'a> {
    /// A CARD_*.
    pub kind: u32,
    /// Its name, such as `virtio0`, padded with zeros.
    pub name: [u8; 16],
    /// What it is, for people, padded with zeros.
    pub model: [u8; 32],
    /// CARD_* flag bits.
    pub flags: u32,
    /// The narrowest mode it sets, where it sets modes.
    pub width_min: u32,
    /// The shortest.
    pub height_min: u32,
    /// The widest mode it sets and picture it shows.
    pub width_max: u32,
    /// The tallest.
    pub height_max: u32,
    /// How many pipes it has.
    pub pipes: usize,
    /// Its screens.
    pub screens: &'a [ScreenSpec],
    /// What each pipe's main plane takes.
    pub main: PlaneSpec,
    /// What each pipe's pointer plane takes.
    pub pointer: PlaneSpec,
}

/// `text` as a record's name: padded with zeros, cut to leave one.
pub fn name<const N: usize>(text: &str) -> [u8; N] {
    let mut out = [0; N];
    let len = text.len().min(N.saturating_sub(1));
    out[..len].copy_from_slice(&text.as_bytes()[..len]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Plain(PictureShape, u32);

    impl Picture for Plain {
        fn shape(&self) -> PictureShape {
            self.0
        }

        fn row_bytes(&self) -> u32 {
            self.1
        }
    }

    #[test]
    fn a_pixel_of_the_frame_is_found_in_the_picture() {
        // A picture of 8 by 4 with rows 40 bytes apart, whose part from
        // (2, 1) shows at (10, 20) of the frame.
        let picture = Plain(PictureShape { card: 1, format: 1, width: 8, height: 4 }, 40);
        let source = Rect { x: 2, y: 1, width: 6, height: 3 };
        let shown = Shown { picture: &picture, source, x: 10, y: 20, hot_spot: (0, 0) };
        assert_eq!(shown.place(), Rect { x: 10, y: 20, width: 6, height: 3 });
        assert_eq!(shown.byte_at(10, 20), Some(40 + 2 * 4));
        assert_eq!(shown.byte_at(15, 22), Some(3 * 40 + 7 * 4));
        // Left of and above the picture, and past its last column and row.
        assert_eq!(shown.byte_at(7, 20), None);
        assert_eq!(shown.byte_at(10, 18), None);
        assert_eq!(shown.byte_at(16, 20), None);
        assert_eq!(shown.byte_at(10, 23), None);
    }

    #[test]
    fn a_pointer_that_hangs_over_the_edge_keeps_its_place() {
        let picture = Plain(PictureShape { card: 1, format: 2, width: 16, height: 16 }, 64);
        let source = Rect::sized(16, 16);
        let shown = Shown { picture: &picture, source, x: -5, y: -3, hot_spot: (5, 3) };
        assert_eq!(shown.place(), Rect { x: -5, y: -3, width: 16, height: 16 });
        // The first pixel of the frame shows the pointer's (5, 3).
        assert_eq!(shown.byte_at(0, 0), Some(3 * 64 + 5 * 4));
    }

    #[test]
    fn a_name_is_padded_and_cut_to_leave_a_zero() {
        assert_eq!(name::<8>("fw0"), *b"fw0\0\0\0\0\0");
        assert_eq!(name::<4>("firmware"), *b"fir\0");
    }
}
