// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The pointer's picture: one of windowd's own, the arrow or the arrows
//! that show which way an edge of a window is dragged, or what the
//! program under the pointer asked for.
//!
//! The pointer is not drawn into a screen's frame. Its picture is on
//! the pointer plane of the screen it is on, and the card puts it where
//! MOVE_POINTER says, at once (radix-display(2)): the pointer moves
//! without a frame being drawn or waited for.

use std::sync::Mutex;

use smithay::backend::allocator::Fourcc;
use smithay::backend::renderer::element::memory::{
    MemoryRenderBuffer, MemoryRenderBufferRenderElement,
};
use smithay::backend::renderer::element::surface::{
    WaylandSurfaceRenderElement, render_elements_from_surface_tree,
};
use smithay::backend::renderer::element::{Kind, render_elements};
use smithay::backend::renderer::pixman::PixmanRenderer;
use smithay::input::pointer::{CursorImageAttributes, CursorImageStatus};
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::utils::Transform;
use smithay::wayland::compositor::with_states;
use window_frame::Edge;

/// The width and the height of the pointer's picture, which every
/// pointer plane of release 1.1 takes. A program's pointer that is
/// larger is cut off there.
pub const SIDE: u32 = 64;

/// windowd's arrow, a row a line: `X` is its black edge and `.` its
/// white inside. Its tip, the hot spot, is the top left corner.
const ARROW: [&str; 19] = [
    "X           ",
    "XX          ",
    "X.X         ",
    "X..X        ",
    "X...X       ",
    "X....X      ",
    "X.....X     ",
    "X......X    ",
    "X.......X   ",
    "X........X  ",
    "X.........X ",
    "X..........X",
    "X......XXXXX",
    "X...X..X    ",
    "X..X X..X   ",
    "X.X  X..X   ",
    "XX    X..X  ",
    "X     X..X  ",
    "       XX   ",
];

/// The arrows that show an edge is dragged to the left and the right,
/// in [`ARROW`]'s marks. Its hot spot is its middle.
const ACROSS: [&str; 9] = [
    "    XX         XX    ",
    "   X.X         X.X   ",
    "  X..X         X..X  ",
    " X...XXXXXXXXXXX...X ",
    "X...................X",
    " X...XXXXXXXXXXX...X ",
    "  X..X         X..X  ",
    "   X.X         X.X   ",
    "    XX         XX    ",
];

/// The arrows that show a corner is dragged up to the left and down to
/// the right. Its hot spot is its middle.
const FALLING: [&str; 15] = [
    "XXXXXXX        ",
    "X.....X        ",
    "X....X         ",
    "X...X          ",
    "X..X.X         ",
    "X.X X.X        ",
    "XX   X.X       ",
    "      X.X      ",
    "       X.X   XX",
    "        X.X X.X",
    "         X.X..X",
    "          X...X",
    "         X....X",
    "        X.....X",
    "        XXXXXXX",
];

/// One of windowd's own pictures of the pointer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Picture {
    /// The arrow.
    Arrow,
    /// An edge is dragged to the left and the right.
    Across,
    /// An edge is dragged up and down.
    Upright,
    /// A corner is dragged up to the left and down to the right.
    Falling,
    /// A corner is dragged up to the right and down to the left.
    Rising,
}

impl Picture {
    /// The picture that shows which way `edge` is dragged.
    pub fn of(edge: Edge) -> Picture {
        match edge {
            Edge::Left | Edge::Right => Picture::Across,
            Edge::Top | Edge::Bottom => Picture::Upright,
            Edge::TopLeft | Edge::BottomRight => Picture::Falling,
            Edge::TopRight | Edge::BottomLeft => Picture::Rising,
        }
    }
}

/// windowd's own pictures of the pointer, as pixels.
#[derive(Debug)]
pub struct Arrows {
    arrow: MemoryRenderBuffer,
    across: MemoryRenderBuffer,
    upright: MemoryRenderBuffer,
    falling: MemoryRenderBuffer,
    rising: MemoryRenderBuffer,
}

impl Default for Arrows {
    fn default() -> Arrows {
        let rows = |marks: &[&str]| -> Vec<Vec<u8>> {
            marks.iter().map(|row| row.as_bytes().to_vec()).collect()
        };
        let across = rows(&ACROSS);
        let upright: Vec<Vec<u8>> =
            (0..across[0].len()).map(|x| across.iter().map(|row| row[x]).collect()).collect();
        let falling = rows(&FALLING);
        let rising: Vec<Vec<u8>> =
            falling.iter().map(|row| row.iter().rev().copied().collect()).collect();
        Arrows {
            arrow: pixels(&rows(&ARROW)),
            across: pixels(&across),
            upright: pixels(&upright),
            falling: pixels(&falling),
            rising: pixels(&rising),
        }
    }
}

impl Arrows {
    /// The pixels of `picture`, and its hot spot.
    fn of(&self, picture: Picture) -> (&MemoryRenderBuffer, (u16, u16)) {
        let middle = |marks: &[&str]| ((marks[0].len() / 2) as u16, (marks.len() / 2) as u16);
        let (across, falling) = (middle(&ACROSS), middle(&FALLING));
        match picture {
            Picture::Arrow => (&self.arrow, (0, 0)),
            Picture::Across => (&self.across, across),
            Picture::Upright => (&self.upright, (across.1, across.0)),
            Picture::Falling => (&self.falling, falling),
            Picture::Rising => (&self.rising, falling),
        }
    }
}

/// What the pointer shows.
#[derive(Debug, Clone, Copy)]
pub enum Shown<'a> {
    /// One of windowd's own pictures.
    Own(Picture),
    /// What the program under the pointer asked for.
    Programs(&'a CursorImageStatus),
}

render_elements! {
    /// What a pointer's picture is drawn from.
    pub Drawn<=PixmanRenderer>;
    /// One of windowd's own pictures.
    Arrow=MemoryRenderBufferRenderElement<PixmanRenderer>,
    /// A surface a program gave.
    Surface=WaylandSurfaceRenderElement<PixmanRenderer>,
}

/// The pixels of a picture whose rows of marks are `marks`: `X` is
/// black, `.` white and anything else see-through.
fn pixels(marks: &[Vec<u8>]) -> MemoryRenderBuffer {
    let width = marks[0].len();
    let mut pixels = Vec::with_capacity(width * marks.len() * 4);
    for mark in marks.iter().flatten() {
        pixels.extend_from_slice(match mark {
            b'X' => &[0x00, 0x00, 0x00, 0xff],
            b'.' => &[0xff, 0xff, 0xff, 0xff],
            _ => &[0x00; 4],
        });
    }
    let size = (width as i32, marks.len() as i32);
    MemoryRenderBuffer::from_slice(&pixels, Fourcc::Argb8888, size, 1, Transform::Normal, None)
}

/// What the pointer's picture is drawn from when the pointer shows
/// `shown`, and the picture's hot spot. `None` for a pointer that is
/// hidden.
pub fn drawn(
    renderer: &mut PixmanRenderer,
    shown: Shown<'_>,
    own: &Arrows,
) -> Option<(Vec<Drawn>, (u16, u16))> {
    // The programs of release 1.1 that name a pointer get the arrow
    // (docs/deferred.md).
    let picture = match shown {
        Shown::Own(picture) => picture,
        Shown::Programs(CursorImageStatus::Named(_)) => Picture::Arrow,
        Shown::Programs(CursorImageStatus::Hidden) => return None,
        Shown::Programs(CursorImageStatus::Surface(surface)) => {
            return Some(of_surface(renderer, surface));
        }
    };
    let (pixels, spot) = own.of(picture);
    let from = (0.0, 0.0);
    let drawn = MemoryRenderBufferRenderElement::from_buffer(
        renderer,
        from,
        pixels,
        None,
        None,
        None,
        Kind::Cursor,
    );
    // A picture the renderer cannot take in shows as no pointer.
    drawn.ok().map(|drawn| (vec![Drawn::Arrow(drawn)], spot))
}

/// What the pointer's picture is drawn from when it is the surface
/// `surface` of a program, and its hot spot.
fn of_surface(renderer: &mut PixmanRenderer, surface: &WlSurface) -> (Vec<Drawn>, (u16, u16)) {
    let hot = with_states(surface, |states| {
        let role = states.data_map.get::<Mutex<CursorImageAttributes>>();
        role.and_then(|role| role.lock().ok().map(|role| role.hotspot))
    })
    .unwrap_or_default();
    // A hot spot left of or above the surface: the surface is drawn
    // that far in, and the picture's corner is the spot.
    let at = ((-hot.x).max(0), (-hot.y).max(0));
    let spot = (hot.x.clamp(0, SIDE as i32 - 1) as u16, hot.y.clamp(0, SIDE as i32 - 1) as u16);
    let drawn = render_elements_from_surface_tree(renderer, surface, at, 1.0, 1.0, Kind::Cursor);
    (drawn, spot)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Checks that `picture` is a rectangle of the three marks that
    /// fits the pointer's plane, and that every white pixel has an edge
    /// or white on each side, so that it shows on any background.
    fn check(picture: &[&str]) {
        let width = picture[0].len();
        assert!(picture.iter().all(|row| row.len() == width));
        assert!(width <= SIDE as usize && picture.len() <= SIDE as usize);
        let marks = picture.iter().flat_map(|row| row.bytes());
        assert!(marks.clone().all(|mark| matches!(mark, b'X' | b'.' | b' ')));
        for (y, row) in picture.iter().enumerate() {
            for (x, _) in row.bytes().enumerate().filter(|(_, mark)| *mark == b'.') {
                for (dx, dy) in [(-1, 0), (1, 0), (0, -1), (0, 1)] {
                    let beside = picture
                        .get((y as i32 + dy) as usize)
                        .and_then(|row| row.as_bytes().get((x as i32 + dx) as usize));
                    assert!(matches!(beside, Some(b'X' | b'.')), "({x}, {y}) is open");
                }
            }
        }
    }

    #[test]
    fn the_arrow_is_a_rectangle_that_fits_the_picture_with_its_tip_at_the_corner() {
        check(&ARROW);
        assert!(ARROW[0].starts_with('X'));
    }

    #[test]
    fn the_resize_pictures_are_whole_and_alike_about_their_middles() {
        check(&ACROSS);
        check(&FALLING);
        // Each is the same turned half round, so its middle is its hot
        // spot whichever way the edge goes.
        for picture in [&ACROSS[..], &FALLING[..]] {
            let turned: Vec<String> =
                picture.iter().rev().map(|row| row.chars().rev().collect()).collect();
            assert_eq!(turned, picture);
            assert_eq!(picture.len() % 2, 1);
            assert_eq!(picture[0].len() % 2, 1);
        }
        let pictures = Arrows::default();
        assert_eq!(pictures.of(Picture::Across).1, (10, 4));
        assert_eq!(pictures.of(Picture::Upright).1, (4, 10));
        assert_eq!(pictures.of(Picture::Falling).1, (7, 7));
        assert_eq!(pictures.of(Picture::Rising).1, (7, 7));
    }

    #[test]
    fn each_edge_has_the_picture_of_its_way() {
        assert_eq!(Picture::of(Edge::Left), Picture::Across);
        assert_eq!(Picture::of(Edge::Bottom), Picture::Upright);
        assert_eq!(Picture::of(Edge::TopLeft), Picture::Falling);
        assert_eq!(Picture::of(Edge::BottomRight), Picture::Falling);
        assert_eq!(Picture::of(Edge::TopRight), Picture::Rising);
        assert_eq!(Picture::of(Edge::BottomLeft), Picture::Rising);
    }
}
