// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The display layer of the Radix kernel (release 1.1, ADR 0700): the
//! rules behind the files of _/devices/display_, apart from any card.
//!
//! A program reaches the screens through radix-display(2)'s requests. The
//! kernel (`radix::display`) holds the lock, the drivers, the memory a
//! picture's pixels are in, and the threads that keep each card's beat.
//! What it decides with is here, where the tests run on the host and the
//! proofs are checked:
//!
//! - [`wire`] reads the records a request carries (a change and its
//!   entries, a PUT and its rectangles) out of the bytes a program gave,
//!   and writes the records the kernel answers with;
//! - [`state`] is the model: what each card has ([`state::Things`]), the
//!   state a holder asks for ([`state::State`]), how one entry of a
//!   change alters it ([`state::apply`]) and whether the whole is
//!   something the cards can show ([`state::check`]);
//! - [`stack`] is who is in front: the stack of holders over the kernel's
//!   log, where the front is lent and given back and never taken;
//! - [`beat`] is the arithmetic of a pipe's beat: its count, the time of
//!   the next one, and the last moment a change still makes it;
//! - [`ring`] is the events a holder has not read, with the mark of those
//!   it lost;
//! - [`frame`] is what the layer and a card say to each other: the frame
//!   a pipe is to show, and what of it is new;
//! - [`holder`] is one holder's own: its pictures and values by their
//!   numbers, the state it last asked for, the events it has not read;
//! - [`layer`] puts them together: it keeps the cards, the holders and
//!   the state that shows, and carries each request out on the cards;
//! - [`pixels`] moves rows of a picture into device memory in the
//!   layout the firmware chose, and blends a pointer over them;
//! - [`monitor`] reads what a monitor says of itself (EDID): its name,
//!   size, modes and ranges.
//!
//! Only the layer and its holders allocate. They keep their lists in
//! vectors from the allocator the kernel gives them, reserve before they
//! change anything, and answer a failed allocation with no_memory and
//! nothing altered. Everything else is given slices.

#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]
#![feature(vec_push_within_capacity)]
// For an empty vector in a `const fn`, which the kernel's one layer is
// made with; Kani's toolchain knows it by its older name.
#![cfg_attr(not(kani), feature(allocator_ext))]
#![cfg_attr(kani, feature(allocator_api))]

extern crate alloc;

pub mod beat;
pub mod frame;
pub mod holder;
pub mod layer;
pub mod monitor;
pub mod pixels;
pub mod ring;
pub mod stack;
pub mod state;
pub mod wire;

use radix_abi::display::{FORMAT_ARGB8888, FORMAT_XRGB8888, Mode, SIDE_MAX};

/// A rectangle of a picture or of a frame, in pixels. `x` and `y` are
/// signed for a plane's place, which may hang over the frame's edge, as a
/// pointer's does.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Rect {
    /// The left column.
    pub x: i32,
    /// The top row.
    pub y: i32,
    /// The width.
    pub width: u32,
    /// The height.
    pub height: u32,
}

impl Rect {
    /// A rectangle of `width` by `height` at the top left corner.
    pub const fn sized(width: u32, height: u32) -> Rect {
        Rect { x: 0, y: 0, width, height }
    }

    /// The rectangle an entry's value packs: the column in bits 0 to 15,
    /// the row in bits 16 to 31, the width in bits 32 to 47 and the
    /// height in bits 48 to 63. `signed` reads the column and the row as
    /// signed numbers, as PLANE_PLACE's are.
    pub fn unpack(value: u64, signed: bool) -> Rect {
        let field = |shift: u32| (value >> shift) as u16;
        let position = |raw: u16| if signed { i32::from(raw as i16) } else { i32::from(raw) };
        Rect {
            x: position(field(0)),
            y: position(field(16)),
            width: u32::from(field(32)),
            height: u32::from(field(48)),
        }
    }

    /// The value that packs it, as [`Rect::unpack`] reads one. A part
    /// that does not fit its 16 bits is cut to them.
    pub fn pack(self) -> u64 {
        u64::from(self.x as u16)
            | u64::from(self.y as u16) << 16
            | u64::from(self.width as u16) << 32
            | u64::from(self.height as u16) << 48
    }

    /// Whether it holds no pixel.
    pub fn is_empty(self) -> bool {
        self.width == 0 || self.height == 0
    }

    /// Whether all of it lies in a picture of `width` by `height`.
    pub fn within(self, width: u32, height: u32) -> bool {
        let (Ok(x), Ok(y)) = (u32::try_from(self.x), u32::try_from(self.y)) else { return false };
        x.checked_add(self.width).is_some_and(|right| right <= width)
            && y.checked_add(self.height).is_some_and(|bottom| bottom <= height)
    }

    /// The part of it that `other` covers too, if they share a pixel.
    pub fn intersect(self, other: Rect) -> Option<Rect> {
        let (left, top) = (self.x.max(other.x), self.y.max(other.y));
        let width = self.right().min(other.right()) - i64::from(left);
        let height = self.bottom().min(other.bottom()) - i64::from(top);
        // Neither is more than the width or height of one of the two.
        (width > 0 && height > 0).then_some(Rect {
            x: left,
            y: top,
            width: width as u32,
            height: height as u32,
        })
    }

    /// The smallest rectangle that covers it and `other`. An empty one
    /// adds nothing.
    pub fn union(self, other: Rect) -> Rect {
        if self.is_empty() {
            return other;
        }
        if other.is_empty() {
            return self;
        }
        let (left, top) = (self.x.min(other.x), self.y.min(other.y));
        let width = self.right().max(other.right()) - i64::from(left);
        let height = self.bottom().max(other.bottom()) - i64::from(top);
        Rect {
            x: left,
            y: top,
            width: u32::try_from(width).unwrap_or(u32::MAX),
            height: u32::try_from(height).unwrap_or(u32::MAX),
        }
    }

    /// The column past its last, in a number wide enough for any
    /// rectangle.
    fn right(self) -> i64 {
        i64::from(self.x) + i64::from(self.width)
    }

    /// The row past its last.
    fn bottom(self) -> i64 {
        i64::from(self.y) + i64::from(self.height)
    }
}

/// The bytes of a pixel of `format`, if a card of release 1.1 shows it.
/// The other formats the ABI names have no card yet (ADR 0701).
pub fn pixel_bytes(format: u32) -> Option<u32> {
    matches!(format, FORMAT_XRGB8888 | FORMAT_ARGB8888).then_some(4)
}

/// The fewest bytes from one row to the next of a picture `width` pixels
/// wide in `format`, or `None` if no card shows such a picture.
pub fn row_bytes(format: u32, width: u32) -> Option<u32> {
    if width == 0 || width > SIDE_MAX {
        return None;
    }
    Some(pixel_bytes(format)? * width)
}

/// The bytes of a picture's pages that its rows reach: `height` rows
/// `row` bytes apart, the last one only as long as its pixels. `None` for
/// a picture of no size, or one whose rows are closer than its pixels are
/// wide.
pub fn picture_bytes(format: u32, width: u32, height: u32, row: u32) -> Option<u64> {
    let least = row_bytes(format, width)?;
    if height == 0 || height > SIDE_MAX || row < least {
        return None;
    }
    Some(u64::from(row) * u64::from(height - 1) + u64::from(least))
}

/// Whether two modes are the same to a screen: the same size at the same
/// rate.
pub fn same_mode(a: &Mode, b: &Mode) -> bool {
    a.width == b.width && a.height == b.height && a.refresh_mhz == b.refresh_mhz
}

#[cfg(test)]
mod tests {
    use radix_abi::display::FORMAT_NV12;

    use super::*;

    #[test]
    fn a_rectangle_survives_its_packing() {
        let rect = Rect { x: 12, y: 34, width: 1920, height: 1080 };
        assert_eq!(Rect::unpack(rect.pack(), false), rect);
        let pointer = Rect { x: -5, y: -64, width: 64, height: 64 };
        assert_eq!(Rect::unpack(pointer.pack(), true), pointer);
        // Read unsigned, a place left of the frame is far to the right.
        assert_eq!(Rect::unpack(pointer.pack(), false).x, 65531);
    }

    #[test]
    fn a_rectangle_is_within_a_picture_only_whole() {
        assert!(Rect::sized(64, 64).within(64, 64));
        assert!(!Rect { x: 1, y: 0, width: 64, height: 64 }.within(64, 64));
        assert!(!Rect { x: -1, y: 0, width: 8, height: 8 }.within(64, 64));
        assert!(!Rect { x: 0, y: 60, width: 8, height: 8 }.within(64, 64));
    }

    #[test]
    fn rectangles_meet_and_join() {
        let a = Rect { x: 0, y: 0, width: 10, height: 10 };
        let b = Rect { x: 5, y: 8, width: 10, height: 10 };
        assert_eq!(a.intersect(b), Some(Rect { x: 5, y: 8, width: 5, height: 2 }));
        assert_eq!(a.union(b), Rect { x: 0, y: 0, width: 15, height: 18 });
        let apart = Rect { x: 10, y: 0, width: 4, height: 4 };
        assert_eq!(a.intersect(apart), None);
        assert_eq!(a.union(Rect::default()), a);
        assert_eq!(Rect::default().union(b), b);
    }

    #[test]
    fn a_pointer_hanging_over_the_edge_is_cut_to_the_frame() {
        let frame = Rect::sized(1920, 1080);
        let pointer = Rect { x: -10, y: 1070, width: 64, height: 64 };
        assert_eq!(pointer.intersect(frame), Some(Rect { x: 0, y: 1070, width: 54, height: 10 }));
    }

    #[test]
    fn a_picture_needs_its_rows_and_only_the_pixels_of_the_last() {
        assert_eq!(row_bytes(FORMAT_XRGB8888, 1920), Some(7680));
        assert_eq!(picture_bytes(FORMAT_XRGB8888, 1920, 1080, 7680), Some(7680 * 1080));
        // Rows a page apart: the last row ends with its pixels.
        assert_eq!(picture_bytes(FORMAT_ARGB8888, 64, 64, 4096), Some(4096 * 63 + 256));
        assert_eq!(picture_bytes(FORMAT_XRGB8888, 1920, 1080, 7679), None);
        assert_eq!(picture_bytes(FORMAT_XRGB8888, 0, 1080, 7680), None);
        assert_eq!(picture_bytes(FORMAT_XRGB8888, 1920, SIDE_MAX + 1, 7680), None);
        assert_eq!(row_bytes(FORMAT_NV12, 1920), None);
    }

    #[test]
    fn modes_are_the_same_by_size_and_rate() {
        let mode = Mode { width: 1920, height: 1080, refresh_mhz: 60_000, ..Mode::default() };
        assert!(same_mode(&mode, &Mode { clock_khz: 148_500, ..mode }));
        assert!(!same_mode(&mode, &Mode { refresh_mhz: 59_940, ..mode }));
    }
}
