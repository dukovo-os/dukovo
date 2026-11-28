// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! A resize, paced by the window's program.
//!
//! While the person drags an edge the pointer asks for a new size with
//! every motion, far more often than a program draws. The program is
//! asked for one size at a time: the next goes out when it has drawn
//! the one before, and is the newest the person asked for, so that a
//! slow program is never sent sizes it will not show. The frame follows
//! what the program draws, with the sides that are not dragged where
//! they were when the resize began.

use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel::State;
use smithay::utils::{Logical, Point, Rectangle, Serial, Size};
use smithay::wayland::compositor::with_states;
use smithay::wayland::shell::xdg::{ToplevelSurface, XdgToplevelSurfaceData};
use window_frame::Edge;

/// The serial of the newest configure the program of `toplevel` had
/// answered when it last committed the window.
pub fn answered(toplevel: &ToplevelSurface) -> Option<Serial> {
    with_states(toplevel.wl_surface(), |states| {
        let role = states.data_map.get::<XdgToplevelSurfaceData>();
        role.and_then(|role| role.lock().ok()?.current_serial)
    })
}

/// A resize of one window.
#[derive(Debug)]
pub struct Resize {
    /// The edge or corner that is dragged.
    edge: Edge,
    /// The frame's box when the resize began.
    start: Rectangle<i32, Logical>,
    /// The size of the content the person wants now.
    wanted: Size<i32, Logical>,
    /// The size the program was asked for and has not drawn yet, as the
    /// serial of its configure.
    out: Option<Serial>,
    /// Whether the person has let go.
    over: bool,
}

impl Resize {
    /// A resize by `edge` of a window whose frame's box is `frame` and
    /// whose content is `content` wide and high.
    pub fn new(edge: Edge, frame: Rectangle<i32, Logical>, content: Size<i32, Logical>) -> Resize {
        Resize { edge, start: frame, wanted: content, out: None, over: false }
    }

    /// Where the corner of the frame's box is when the box is `frame`
    /// wide and high: the sides that are not dragged stay.
    pub fn placed(&self, frame: Size<i32, Logical>) -> Point<i32, Logical> {
        let (start, end) = (self.start.loc, self.start.loc + self.start.size.to_point());
        let x = if self.edge.left() { end.x - frame.w } else { start.x };
        let y = if self.edge.top() { end.y - frame.h } else { start.y };
        Point::from((x, y))
    }

    /// The window was moved by `by`: the sides that are not dragged
    /// stay where the move put them.
    pub fn moved(&mut self, by: Point<i32, Logical>) {
        self.start.loc += by;
    }

    /// The size a frame that began as the resize's did has when the
    /// pointer has moved by `by` since: the sides the edge names follow
    /// the pointer.
    pub fn dragged(&self, by: Point<i32, Logical>) -> Size<i32, Logical> {
        let grown = |size: i32, by: i32, low: bool, high: bool| match (low, high) {
            (true, _) => size - by,
            (_, true) => size + by,
            _ => size,
        };
        let size = self.start.size;
        Size::from((
            grown(size.w, by.x, self.edge.left(), self.edge.right()).max(1),
            grown(size.h, by.y, self.edge.top(), self.edge.bottom()).max(1),
        ))
    }

    /// The person wants the content to be `size`.
    pub fn want(&mut self, toplevel: &ToplevelSurface, size: Size<i32, Logical>) {
        self.wanted = size;
        self.ask(toplevel);
    }

    /// The window's surface is to be `rows` higher than the size that
    /// is wanted has it, for the same frame: its title bar changed
    /// hands. The program has been asked for it already.
    pub fn grow(&mut self, rows: i32) {
        self.wanted.h = (self.wanted.h + rows).max(1);
    }

    /// The person let go. Returns whether the resize is over: the
    /// program has nothing left to draw.
    pub fn end(&mut self, toplevel: &ToplevelSurface) -> bool {
        self.over = true;
        self.ask(toplevel);
        self.finished()
    }

    /// The program drew the window anew. Returns whether the resize is
    /// over.
    pub fn committed(&mut self, toplevel: &ToplevelSurface) -> bool {
        if let (Some(out), Some(drawn)) = (self.out, answered(toplevel))
            && drawn.is_no_older_than(&out)
        {
            self.out = None;
        }
        self.ask(toplevel);
        self.finished()
    }

    /// Asks the program for the size that is wanted, if it has drawn
    /// what it was last asked for.
    fn ask(&mut self, toplevel: &ToplevelSurface) {
        if self.out.is_some() {
            return;
        }
        let (wanted, over) = (self.wanted, self.over);
        toplevel.with_pending_state(|state| {
            state.size = Some(wanted);
            if over {
                state.states.unset(State::Resizing);
            } else {
                state.states.set(State::Resizing);
            }
        });
        // Nothing goes out when the program has all of that already.
        self.out = toplevel.send_pending_configure();
    }

    fn finished(&self) -> bool {
        self.over && self.out.is_none()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn resize(edge: Edge) -> Resize {
        Resize::new(edge, Rectangle::new((100, 50).into(), (330, 281).into()), (320, 240).into())
    }

    #[test]
    fn the_sides_that_are_not_dragged_stay() {
        let larger = Size::from((400, 300));
        assert_eq!(resize(Edge::BottomRight).placed(larger), Point::from((100, 50)));
        assert_eq!(resize(Edge::Right).placed(larger), Point::from((100, 50)));
        assert_eq!(resize(Edge::Left).placed(larger), Point::from((30, 50)));
        assert_eq!(resize(Edge::Top).placed(larger), Point::from((100, 31)));
        assert_eq!(resize(Edge::TopLeft).placed(larger), Point::from((30, 31)));
        assert_eq!(resize(Edge::BottomLeft).placed(larger), Point::from((30, 50)));
        assert_eq!(resize(Edge::TopRight).placed(larger), Point::from((100, 31)));
    }

    #[test]
    fn a_window_moved_before_its_program_drew_the_last_size_stays_where_it_was_put() {
        let larger = Size::from((400, 300));
        let mut right = resize(Edge::BottomRight);
        right.moved((40, -10).into());
        assert_eq!(right.placed(larger), Point::from((140, 40)));
        let mut left = resize(Edge::TopLeft);
        left.moved((40, -10).into());
        assert_eq!(left.placed(larger), Point::from((70, 21)));
    }

    #[test]
    fn the_sides_an_edge_names_follow_the_pointer() {
        let by = Point::from((10, -20));
        assert_eq!(resize(Edge::BottomRight).dragged(by), Size::from((340, 261)));
        assert_eq!(resize(Edge::TopLeft).dragged(by), Size::from((320, 301)));
        assert_eq!(resize(Edge::Right).dragged(by), Size::from((340, 281)));
        assert_eq!(resize(Edge::Top).dragged(by), Size::from((330, 301)));
        // Never to nothing.
        assert_eq!(resize(Edge::Left).dragged((1000, 0).into()), Size::from((1, 281)));
    }
}
