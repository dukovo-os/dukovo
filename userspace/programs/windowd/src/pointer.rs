// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The pointer's picture: windowd's own arrow, or what the program
//! under the pointer asked for.
//!
//! The pointer is not drawn into a screen's frame. Its picture is on
//! the pointer plane of the screen it is on, and the card puts it where
//! MOVE_POINTER says, at once (radix-display(2)): the pointer moves
//! without a frame being drawn or waited for.

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
use smithay::utils::Transform;
use smithay::wayland::compositor::with_states;
use std::sync::Mutex;

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

render_elements! {
    /// What a pointer's picture is drawn from.
    pub Drawn<=PixmanRenderer>;
    /// windowd's arrow.
    Arrow=MemoryRenderBufferRenderElement<PixmanRenderer>,
    /// A surface a program gave.
    Surface=WaylandSurfaceRenderElement<PixmanRenderer>,
}

/// windowd's arrow as pixels.
pub fn arrow() -> MemoryRenderBuffer {
    let width = ARROW[0].len();
    let mut pixels = Vec::with_capacity(width * ARROW.len() * 4);
    for mark in ARROW.iter().flat_map(|row| row.bytes()) {
        pixels.extend_from_slice(match mark {
            b'X' => &[0x00, 0x00, 0x00, 0xff],
            b'.' => &[0xff, 0xff, 0xff, 0xff],
            _ => &[0x00; 4],
        });
    }
    let size = (width as i32, ARROW.len() as i32);
    MemoryRenderBuffer::from_slice(&pixels, Fourcc::Argb8888, size, 1, Transform::Normal, None)
}

/// What the pointer's picture is drawn from when the pointer shows
/// `shown`, and the picture's hot spot. `None` for a pointer that is
/// hidden.
pub fn drawn(
    renderer: &mut PixmanRenderer,
    shown: &CursorImageStatus,
    arrow: &MemoryRenderBuffer,
) -> Option<(Vec<Drawn>, (u16, u16))> {
    match shown {
        CursorImageStatus::Hidden => None,
        CursorImageStatus::Surface(surface) => {
            let hot = with_states(surface, |states| {
                let role = states.data_map.get::<Mutex<CursorImageAttributes>>();
                role.and_then(|role| role.lock().ok().map(|role| role.hotspot))
            })
            .unwrap_or_default();
            // A hot spot left of or above the surface: the surface is
            // drawn that far in, and the picture's corner is the spot.
            let at = ((-hot.x).max(0), (-hot.y).max(0));
            let spot =
                (hot.x.clamp(0, SIDE as i32 - 1) as u16, hot.y.clamp(0, SIDE as i32 - 1) as u16);
            let drawn =
                render_elements_from_surface_tree(renderer, surface, at, 1.0, 1.0, Kind::Cursor);
            Some((drawn, spot))
        }
        // The programs of release 1.1 that name a pointer get the
        // arrow (docs/deferred.md).
        CursorImageStatus::Named(_) => {
            let arrow = MemoryRenderBufferRenderElement::from_buffer(
                renderer,
                (0.0, 0.0),
                arrow,
                None,
                None,
                None,
                Kind::Cursor,
            );
            // An arrow the renderer cannot take in shows as no pointer.
            arrow.ok().map(|arrow| (vec![Drawn::Arrow(arrow)], (0, 0)))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_arrow_is_a_rectangle_that_fits_the_picture_with_its_tip_at_the_corner() {
        let width = ARROW[0].len();
        assert!(ARROW.iter().all(|row| row.len() == width));
        assert!(width <= SIDE as usize && ARROW.len() <= SIDE as usize);
        assert!(ARROW[0].starts_with('X'));
        let marks = ARROW.iter().flat_map(|row| row.bytes());
        assert!(marks.clone().all(|mark| matches!(mark, b'X' | b'.' | b' ')));
        // Every white pixel has an edge or white on each side, so the
        // arrow shows on any background.
        for (y, row) in ARROW.iter().enumerate() {
            for (x, _) in row.bytes().enumerate().filter(|(_, mark)| *mark == b'.') {
                for (dx, dy) in [(-1, 0), (1, 0), (0, -1), (0, 1)] {
                    let beside = ARROW
                        .get((y as i32 + dy) as usize)
                        .and_then(|row| row.as_bytes().get((x as i32 + dx) as usize));
                    assert!(matches!(beside, Some(b'X' | b'.')), "({x}, {y}) is open");
                }
            }
        }
    }
}
