// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Where the parts of a frame are, and which part a point is in.
//!
//! A [`Layout`] is reckoned from the size of a window's own surface,
//! the shape of the window and whose the title bar is. The frame's box
//! has its top left corner at (0, 0); the surface lies in it, and what
//! is drawn outside it, a line and a shadow, is at negative places and
//! past the box's size.
//!
//! The title bar is windowd's whole ([`Bar::Full`]), with the surface
//! under it, or shared with the window's program ([`Bar::Shared`],
//! ADR 0719): then the surface reaches to the top of the box, and its
//! rows in the bar show only in the region the program draws, which is
//! cut so that the icon, the buttons and a strip to move the window by
//! stay windowd's.

use crate::canvas::Rect;
use crate::theme::Theme;

/// How a window lies on its screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shape {
    /// With a frame all round, where the person put it.
    Normal,
    /// Over the whole of its screen, with a title bar alone.
    Maximized,
}

/// Whose the title bar is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Bar {
    /// windowd's.
    Full,
    /// windowd's, with a region the window's program draws.
    Shared,
}

/// A button of the title bar.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Button {
    /// Takes the window off the screen, into the list.
    Minimize,
    /// Fills the screen with the window, or puts it back.
    Maximize,
    /// Asks the window's program to close it.
    Close,
}

impl Button {
    /// The three, from left to right.
    pub const ALL: [Button; 3] = [Button::Minimize, Button::Maximize, Button::Close];
}

/// An edge or a corner of a frame, which a window is resized by.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Edge {
    /// The top edge.
    Top,
    /// The bottom edge.
    Bottom,
    /// The left edge.
    Left,
    /// The right edge.
    Right,
    /// The top left corner.
    TopLeft,
    /// The top right corner.
    TopRight,
    /// The bottom left corner.
    BottomLeft,
    /// The bottom right corner.
    BottomRight,
}

impl Edge {
    /// Whether the window's left side moves with it.
    pub const fn left(self) -> bool {
        matches!(self, Edge::Left | Edge::TopLeft | Edge::BottomLeft)
    }

    /// Whether the window's right side moves with it.
    pub const fn right(self) -> bool {
        matches!(self, Edge::Right | Edge::TopRight | Edge::BottomRight)
    }

    /// Whether the window's top moves with it.
    pub const fn top(self) -> bool {
        matches!(self, Edge::Top | Edge::TopLeft | Edge::TopRight)
    }

    /// Whether the window's bottom moves with it.
    pub const fn bottom(self) -> bool {
        matches!(self, Edge::Bottom | Edge::BottomLeft | Edge::BottomRight)
    }
}

/// What of a window a point is in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Part {
    /// The title bar, where the window is moved by.
    Title,
    /// A button.
    Button(Button),
    /// An edge or a corner, or the room around it that takes hold of
    /// it.
    Edge(Edge),
    /// What the window's program draws and hears the pointer in.
    Content,
    /// Nothing of the window.
    Outside,
}

/// How much frame there is on each side of a window's surface.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Margins {
    /// Left of it.
    pub left: i32,
    /// Above it.
    pub top: i32,
    /// Right of it.
    pub right: i32,
    /// Below it.
    pub bottom: i32,
}

impl Margins {
    /// The frame around the surface of a window of `shape` whose title
    /// bar is `bar`.
    pub const fn of(theme: &Theme, shape: Shape, bar: Bar) -> Margins {
        let side = match shape {
            Shape::Normal => theme.border,
            Shape::Maximized => 0,
        };
        let top = match bar {
            Bar::Full => theme.bar,
            Bar::Shared => 0,
        };
        Margins { left: side, top, right: side, bottom: side }
    }

    /// The size of the frame's box around a surface of `surface`.
    pub const fn around(&self, surface: (i32, i32)) -> (i32, i32) {
        (surface.0 + self.left + self.right, surface.1 + self.top + self.bottom)
    }

    /// The size of the surface in a frame whose box is `frame`.
    pub const fn within(&self, frame: (i32, i32)) -> (i32, i32) {
        (frame.0 - self.left - self.right, frame.1 - self.top - self.bottom)
    }
}

/// Where the parts of one frame are.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Layout {
    /// The window's shape.
    pub shape: Shape,
    /// Whose the title bar is.
    pub bar: Bar,
    /// The frame's box, whose corner is (0, 0).
    pub frame: Rect,
    /// Where the window's surface lies.
    pub surface: Rect,
    /// The part of the surface below the title bar, all of which shows.
    pub content: Rect,
    /// The part of the title bar the window's program draws, if the
    /// bar is shared and the program named one.
    pub region: Option<Rect>,
    /// Where the icon is.
    pub icon: Rect,
    /// Where the title may be written.
    pub title: Rect,
    /// The buttons, in [`Button::ALL`]'s order.
    pub buttons: [Rect; 3],
    grab: i32,
    corner: i32,
    top_edge: i32,
    border: i32,
    radius: i32,
    shadow_rows: i32,
}

impl Layout {
    /// How far the frame's own parts are drawn in from the box's sides
    /// in a window of `shape`: a maximized window has no frame at its
    /// sides for them to stand off from.
    const fn pulled_in(theme: &Theme, shape: Shape) -> i32 {
        match shape {
            Shape::Normal => 0,
            Shape::Maximized => theme.border,
        }
    }

    /// The columns of a shared title bar that stay windowd's, counted
    /// from the left and from the right edge of the window's surface:
    /// the icon's, and the buttons' with the strip to move the window
    /// by.
    pub fn insets(theme: &Theme, shape: Shape) -> (i32, i32) {
        let margins = Margins::of(theme, shape, Bar::Shared);
        let pulled = Layout::pulled_in(theme, shape);
        let left = theme.icon_at.0 - pulled + theme.icon_side + theme.title_gap - margins.left;
        let buttons: i32 = theme.button_widths.iter().sum();
        let right = theme.buttons_right - pulled + buttons + theme.title_gap + theme.drag_strip
            - margins.right;
        (left, right)
    }

    /// The least width and height of a window's content: what leaves
    /// the icon and the buttons room in the title bar.
    pub fn least(theme: &Theme) -> (i32, i32) {
        let buttons: i32 = theme.button_widths.iter().sum();
        let bar =
            theme.icon_at.0 + theme.icon_side + theme.title_gap + buttons + theme.buttons_right;
        (bar - 2 * theme.border, 1)
    }

    /// The layout of the frame of a window of `shape` whose surface is
    /// `surface` wide and high. `region` is what the window's program
    /// asked to draw of a shared title bar, from the surface's corner.
    pub fn of(
        theme: &Theme,
        surface: (i32, i32),
        shape: Shape,
        bar: Bar,
        region: Option<Rect>,
    ) -> Layout {
        let margins = Margins::of(theme, shape, bar);
        let (width, height) = margins.around(surface);
        let surface = Rect::new(margins.left, margins.top, surface.0, surface.1);
        let content = match bar {
            Bar::Full => surface,
            Bar::Shared => {
                Rect::new(surface.x, theme.bar, surface.w, (surface.h - theme.bar).max(0))
            }
        };
        let pulled = Layout::pulled_in(theme, shape);
        let mut buttons = [Rect::default(); 3];
        let mut right = width - (theme.buttons_right - pulled);
        for (button, button_width) in buttons.iter_mut().zip(theme.button_widths).rev() {
            right -= button_width;
            *button = Rect::new(right, 0, button_width, theme.button_height);
        }
        let icon =
            Rect::new(theme.icon_at.0 - pulled, theme.icon_at.1, theme.icon_side, theme.icon_side);
        let title_from = icon.right() + theme.title_gap;
        let title_to = buttons[0].x - theme.title_gap;
        let title = Rect::new(title_from, 0, (title_to - title_from).max(0), theme.bar - 1);
        let region = match (bar, region) {
            (Bar::Shared, Some(asked)) => {
                let (left, right) = Layout::insets(theme, shape);
                let allowed = Rect::new(
                    surface.x + left,
                    1,
                    (surface.w - left - right).max(0),
                    theme.bar - 1,
                );
                let cut = asked.moved(surface.x, surface.y).shared(&allowed);
                (!cut.is_empty()).then_some(cut)
            }
            _ => None,
        };
        Layout {
            shape,
            bar,
            frame: Rect::new(0, 0, width, height),
            surface,
            content,
            region,
            icon,
            title,
            buttons,
            grab: theme.grab,
            corner: theme.corner,
            top_edge: theme.top_edge,
            border: theme.border,
            radius: theme.radius,
            shadow_rows: theme.active.shadow_rows.max(theme.inactive.shadow_rows),
        }
    }

    /// The rectangle of `button`.
    pub fn button(&self, button: Button) -> Rect {
        self.buttons[button as usize]
    }

    /// The rows of the frame that are the title bar.
    pub fn bar_rows(&self) -> i32 {
        self.title.h + 1
    }

    /// The rectangles the frame is drawn in, which together hold every
    /// pixel of it and leave out the middle of the window: the strip
    /// with the title bar, and for a window that is not maximized the
    /// two sides and the bottom with its corners and the shadow.
    pub fn strips(&self) -> Vec<Rect> {
        let (width, height, bar) = (self.frame.w, self.frame.h, self.bar_rows());
        if self.shape == Shape::Maximized {
            return vec![Rect::new(0, 0, width, bar)];
        }
        // The line outside the box is 1 wide.
        let bottom_from = (height - self.radius).max(bar);
        let side = self.border + 1;
        let strips = [
            Rect::new(-1, -1, width + 2, bar + 1),
            Rect::new(-1, bar, side, bottom_from - bar),
            Rect::new(width - self.border, bar, side, bottom_from - bar),
            Rect::new(-1, bottom_from, width + 2, height + 1 + self.shadow_rows - bottom_from),
        ];
        strips.into_iter().filter(|strip| !strip.is_empty()).collect()
    }

    /// The rectangle around everything the frame draws and the window's
    /// surface.
    pub fn bounds(&self) -> Rect {
        match self.shape {
            Shape::Maximized => self.frame,
            Shape::Normal => {
                Rect::new(-1, -1, self.frame.w + 2, self.frame.h + 2 + self.shadow_rows)
            }
        }
    }

    /// Which part of the window the point (`x`, `y`) is in.
    pub fn part_at(&self, x: i32, y: i32) -> Part {
        let inside = self.frame.contains(x, y);
        if inside
            && let Some(button) = Button::ALL.into_iter().find(|b| self.button(*b).contains(x, y))
        {
            return Part::Button(button);
        }
        if self.shape == Shape::Normal
            && self.frame.grown(self.grab).contains(x, y)
            && let Some(edge) = self.edge_at(x, y)
        {
            return Part::Edge(edge);
        }
        if !inside {
            return Part::Outside;
        }
        if self.region.is_some_and(|region| region.contains(x, y)) || self.content.contains(x, y) {
            return Part::Content;
        }
        Part::Title
    }

    /// The edge or corner the point (`x`, `y`), which is in the frame
    /// or the room around it, takes hold of, if any.
    fn edge_at(&self, x: i32, y: i32) -> Option<Edge> {
        let (width, height) = (self.frame.w, self.frame.h);
        let (left, right) = (x < self.border, x >= width - self.border);
        let (top, bottom) = (y < self.top_edge, y >= height - self.border);
        let (near_left, near_right) = (x < self.corner, x >= width - self.corner);
        let (near_top, near_bottom) = (y < self.corner, y >= height - self.corner);
        let corner = |first: bool, second: bool, both: [Edge; 3]| {
            if first {
                both[0]
            } else if second {
                both[1]
            } else {
                both[2]
            }
        };
        if top {
            Some(corner(near_left, near_right, [Edge::TopLeft, Edge::TopRight, Edge::Top]))
        } else if bottom {
            Some(corner(near_left, near_right, [Edge::BottomLeft, Edge::BottomRight, Edge::Bottom]))
        } else if left {
            Some(corner(near_top, near_bottom, [Edge::TopLeft, Edge::BottomLeft, Edge::Left]))
        } else if right {
            Some(corner(near_top, near_bottom, [Edge::TopRight, Edge::BottomRight, Edge::Right]))
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn normal(surface: (i32, i32)) -> Layout {
        Layout::of(&Theme::basic(), surface, Shape::Normal, Bar::Full, None)
    }

    #[test]
    fn the_frame_is_five_round_the_content_and_thirty_six_above() {
        let layout = normal((320, 240));
        assert_eq!(layout.frame, Rect::new(0, 0, 330, 281));
        assert_eq!(layout.surface, Rect::new(5, 36, 320, 240));
        assert_eq!(layout.content, layout.surface);
        assert_eq!(layout.region, None);
        assert_eq!(layout.bar_rows(), 36);
    }

    #[test]
    fn the_least_content_leaves_the_icon_and_the_buttons_room() {
        let theme = Theme::basic();
        assert_eq!(Layout::least(&theme), (152, 1));
        let layout = Layout::of(&theme, (152, 1), Shape::Normal, Bar::Full, None);
        assert_eq!(layout.button(Button::Minimize).x, layout.icon.right() + theme.title_gap);
        assert_eq!(layout.title.w, 0);
    }

    #[test]
    fn the_buttons_end_seven_from_the_right_edge() {
        let layout = normal((320, 240));
        assert_eq!(layout.button(Button::Close), Rect::new(330 - 7 - 56, 0, 56, 22));
        assert_eq!(layout.button(Button::Maximize), Rect::new(330 - 7 - 56 - 31, 0, 31, 22));
        assert_eq!(layout.button(Button::Minimize), Rect::new(330 - 7 - 120, 0, 33, 22));
        assert_eq!(layout.icon, Rect::new(8, 8, 19, 19));
        assert_eq!(layout.title, Rect::new(35, 0, 330 - 7 - 120 - 8 - 35, 35));
    }

    #[test]
    fn a_maximized_window_has_the_title_bar_alone() {
        let theme = Theme::basic();
        let layout = Layout::of(&theme, (1024, 732), Shape::Maximized, Bar::Full, None);
        assert_eq!(layout.frame, Rect::new(0, 0, 1024, 768));
        assert_eq!(layout.surface, Rect::new(0, 36, 1024, 732));
        assert_eq!(layout.strips(), vec![Rect::new(0, 0, 1024, 36)]);
        assert_eq!(layout.bounds(), layout.frame);
        assert_eq!(layout.button(Button::Close).right(), 1024 - 2);
        assert_eq!(layout.icon.x, 3);
        // No edge takes hold of it, in it or around it.
        assert_eq!(layout.part_at(0, 100), Part::Content);
        assert_eq!(layout.part_at(-1, 100), Part::Outside);
        assert_eq!(layout.part_at(500, 0), Part::Title);
    }

    #[test]
    fn the_margins_turn_a_frame_into_its_surface_and_back() {
        let theme = Theme::basic();
        for shape in [Shape::Normal, Shape::Maximized] {
            for bar in [Bar::Full, Bar::Shared] {
                let margins = Margins::of(&theme, shape, bar);
                assert_eq!(margins.within(margins.around((640, 480))), (640, 480));
            }
        }
        let maximized = Margins::of(&theme, Shape::Maximized, Bar::Full);
        assert_eq!(maximized.within((1024, 768)), (1024, 732));
        let shared = Margins::of(&theme, Shape::Normal, Bar::Shared);
        assert_eq!(shared.around((320, 276)), (330, 281));
    }

    #[test]
    fn the_strips_hold_the_frame_and_leave_the_middle() {
        let layout = normal((320, 240));
        let strips = layout.strips();
        assert_eq!(
            strips,
            vec![
                Rect::new(-1, -1, 332, 37),
                Rect::new(-1, 36, 6, 237),
                Rect::new(325, 36, 6, 237),
                Rect::new(-1, 273, 332, 15),
            ]
        );
        let bounds = layout.bounds();
        assert_eq!(bounds, Rect::new(-1, -1, 332, 289));
        // No two overlap, and all are in the bounds.
        for (at, strip) in strips.iter().enumerate() {
            assert_eq!(strip.shared(&bounds), *strip);
            assert!(strips[at + 1..].iter().all(|other| strip.shared(other).is_empty()));
        }
        // Every pixel of the bounds outside the content's middle is in
        // one of them.
        let middle = Rect::new(5, 36, 320, 237);
        for y in bounds.y..bounds.bottom() {
            for x in bounds.x..bounds.right() {
                let held = strips.iter().any(|strip| strip.contains(x, y));
                assert_eq!(held, !middle.contains(x, y), "({x}, {y})");
            }
        }
    }

    #[test]
    fn a_window_too_low_for_sides_has_none() {
        let layout = normal((200, 2));
        let strips = layout.strips();
        assert_eq!(strips.len(), 2);
        assert_eq!(strips[1].y, 36);
        assert_eq!(strips[1].bottom(), layout.bounds().bottom());
    }

    #[test]
    fn a_point_is_in_the_part_it_looks_to_be_in() {
        let layout = normal((320, 240));
        assert_eq!(layout.part_at(100, 20), Part::Title);
        assert_eq!(layout.part_at(100, 35), Part::Title);
        assert_eq!(layout.part_at(100, 36), Part::Content);
        assert_eq!(layout.part_at(5, 36), Part::Content);
        assert_eq!(layout.part_at(324, 275), Part::Content);
        assert_eq!(layout.part_at(300, 10), Part::Button(Button::Close));
        assert_eq!(layout.part_at(250, 21), Part::Button(Button::Maximize));
        assert_eq!(layout.part_at(203, 0), Part::Button(Button::Minimize));
        assert_eq!(layout.part_at(202, 10), Part::Title);
        assert_eq!(layout.part_at(250, 22), Part::Title);
    }

    #[test]
    fn the_edges_take_hold_in_the_frame_and_eight_outside_it() {
        let layout = normal((320, 240));
        assert_eq!(layout.part_at(100, 3), Part::Edge(Edge::Top));
        assert_eq!(layout.part_at(100, 4), Part::Title);
        assert_eq!(layout.part_at(100, -8), Part::Edge(Edge::Top));
        assert_eq!(layout.part_at(100, -9), Part::Outside);
        assert_eq!(layout.part_at(4, 100), Part::Edge(Edge::Left));
        assert_eq!(layout.part_at(-8, 100), Part::Edge(Edge::Left));
        assert_eq!(layout.part_at(-9, 100), Part::Outside);
        assert_eq!(layout.part_at(325, 100), Part::Edge(Edge::Right));
        assert_eq!(layout.part_at(337, 100), Part::Edge(Edge::Right));
        assert_eq!(layout.part_at(338, 100), Part::Outside);
        assert_eq!(layout.part_at(100, 276), Part::Edge(Edge::Bottom));
        assert_eq!(layout.part_at(100, 288), Part::Edge(Edge::Bottom));
        assert_eq!(layout.part_at(100, 289), Part::Outside);
        // A button is a button to its top row; above the frame, the
        // edge.
        assert_eq!(layout.part_at(300, 0), Part::Button(Button::Close));
        assert_eq!(layout.part_at(300, -1), Part::Edge(Edge::Top));
    }

    #[test]
    fn a_corner_reaches_twenty_along_each_edge() {
        let layout = normal((320, 240));
        assert_eq!(layout.part_at(19, 2), Part::Edge(Edge::TopLeft));
        assert_eq!(layout.part_at(20, 2), Part::Edge(Edge::Top));
        assert_eq!(layout.part_at(2, 19), Part::Edge(Edge::TopLeft));
        assert_eq!(layout.part_at(2, 20), Part::Edge(Edge::Left));
        assert_eq!(layout.part_at(-5, -5), Part::Edge(Edge::TopLeft));
        assert_eq!(layout.part_at(334, -5), Part::Edge(Edge::TopRight));
        assert_eq!(layout.part_at(-5, 285), Part::Edge(Edge::BottomLeft));
        assert_eq!(layout.part_at(334, 285), Part::Edge(Edge::BottomRight));
        assert_eq!(layout.part_at(310, 278), Part::Edge(Edge::BottomRight));
        assert_eq!(layout.part_at(309, 278), Part::Edge(Edge::Bottom));
        assert_eq!(layout.part_at(327, 261), Part::Edge(Edge::BottomRight));
        assert_eq!(layout.part_at(327, 260), Part::Edge(Edge::Right));
    }

    #[test]
    fn each_edge_moves_the_sides_it_names() {
        assert!(Edge::TopLeft.top() && Edge::TopLeft.left());
        assert!(!Edge::TopLeft.bottom() && !Edge::TopLeft.right());
        assert!(Edge::Right.right() && !Edge::Right.top() && !Edge::Right.bottom());
        assert!(Edge::BottomRight.bottom() && Edge::BottomRight.right());
        assert!(Edge::Bottom.bottom() && !Edge::Bottom.left() && !Edge::Bottom.right());
    }

    #[test]
    fn a_shared_bar_puts_the_surfaces_top_rows_in_the_bar() {
        let theme = Theme::basic();
        let layout = Layout::of(&theme, (600, 436), Shape::Normal, Bar::Shared, None);
        assert_eq!(layout.frame, Rect::new(0, 0, 610, 441));
        assert_eq!(layout.surface, Rect::new(5, 0, 600, 436));
        assert_eq!(layout.content, Rect::new(5, 36, 600, 400));
        // With no region the whole bar is windowd's.
        assert_eq!(layout.part_at(200, 20), Part::Title);
        assert_eq!(Layout::insets(&theme, Shape::Normal), (30, 226));
        assert_eq!(Layout::insets(&theme, Shape::Maximized), (30, 226));
    }

    #[test]
    fn a_region_is_cut_to_what_windowd_leaves_the_program() {
        let theme = Theme::basic();
        let shared = |region| Layout::of(&theme, (600, 436), Shape::Normal, Bar::Shared, region);
        // All of the bar asked for: the icon, the strip and the buttons
        // stay, and so does the frame's top line.
        let all = shared(Some(Rect::new(0, 0, 600, 36))).region.unwrap();
        assert_eq!(all, Rect::new(35, 1, 600 - 30 - 226, 35));
        assert_eq!(all.right(), 610 - 7 - 120 - 8 - 96);
        // A region inside what is allowed is as asked.
        let some = shared(Some(Rect::new(40, 6, 200, 30))).region.unwrap();
        assert_eq!(some, Rect::new(45, 6, 200, 30));
        // One below the bar, or of no size, is none.
        assert_eq!(shared(Some(Rect::new(40, 36, 200, 30))).region, None);
        assert_eq!(shared(Some(Rect::new(40, 6, 0, 30))).region, None);
        // The pointer is the program's in the region and windowd's
        // beside it.
        let layout = shared(Some(Rect::new(40, 6, 200, 30)));
        assert_eq!(layout.part_at(45, 6), Part::Content);
        assert_eq!(layout.part_at(244, 35), Part::Content);
        assert_eq!(layout.part_at(245, 20), Part::Title);
        assert_eq!(layout.part_at(45, 5), Part::Title);
        assert_eq!(layout.part_at(100, 2), Part::Edge(Edge::Top));
        // A bar that is windowd's whole has no region, whatever is
        // asked.
        let full = Layout::of(&theme, (600, 400), Shape::Normal, Bar::Full, Some(all));
        assert_eq!(full.region, None);
    }
}
