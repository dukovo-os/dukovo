// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The one table of a frame's sizes and colours (ADR 0719).
//!
//! Nothing else in the library or in windowd holds a size or a colour
//! of the frame, so that the theme file of the looks (docs/deferred.md)
//! has one thing to fill. [`Theme::basic`] is the Basic look in the
//! dark theme and the Twilight colour: the frame is solid, with no
//! blur and no soft shadow, for a machine whose processor draws.

use crate::canvas::Colour;

/// A fill that changes from the top of a box to its bottom.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Fill {
    /// The colour of the first row.
    pub top: Colour,
    /// A row at which the fill has reached a colour of its own, if it
    /// does not go straight from top to bottom.
    pub knee: Option<(i32, Colour)>,
    /// The colour of the last row.
    pub bottom: Colour,
}

impl Fill {
    /// The colour of row `row` of a box of `rows` rows.
    pub fn at(&self, row: i32, rows: i32) -> Colour {
        let between = |from: Colour, to: Colour, row: i32, rows: i32| {
            let rows = rows.max(1);
            from.towards(to, (row.clamp(0, rows) * 255 / rows) as u8)
        };
        match self.knee {
            Some((knee, colour)) if row < knee => between(self.top, colour, row, knee),
            Some((knee, colour)) => between(colour, self.bottom, row - knee, rows - 1 - knee),
            None => between(self.top, self.bottom, row, rows - 1),
        }
    }
}

/// A button's fill: glass that is lit from above, in two halves.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Glass {
    /// The colours at the top, just above the middle, just below it and
    /// at the bottom.
    pub stops: [Colour; 4],
    /// The line just inside the button's edge.
    pub ring: Colour,
}

impl Glass {
    /// Where the upper half ends and the lower begins, in 100ths of
    /// the button's height.
    const HALVES: (i32, i32) = (48, 52);

    /// The colour of row `row` of a button of `rows` rows.
    pub fn at(&self, row: i32, rows: i32) -> Colour {
        let [top, above, below, bottom] = self.stops;
        // A row is where its middle is, in 200ths of the height.
        let at = (row * 200 + 100) / rows.max(1);
        let (upper, lower) = (Glass::HALVES.0 * 2, Glass::HALVES.1 * 2);
        let share = |from: i32, to: i32| ((at - from) * 255 / (to - from)).clamp(0, 255) as u8;
        if at < upper {
            top.towards(above, share(0, upper))
        } else if at < lower {
            above.towards(below, share(upper, lower))
        } else {
            below.towards(bottom, share(lower, 200))
        }
    }

    /// The glass with every colour `share` of 255 of the way to
    /// `colour`: lit under the pointer, or dimmed while pressed.
    #[must_use]
    pub fn towards(&self, colour: Colour, share: u8) -> Glass {
        Glass {
            stops: self.stops.map(|stop| stop.towards(colour, share)),
            ring: self.ring.towards(colour, share),
        }
    }
}

/// What differs between the frame of the window the person works in and
/// the frames of the others.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Tone {
    /// The frame's fill.
    pub fill: Fill,
    /// The line just inside the frame's edge.
    pub line: Colour,
    /// The title.
    pub title: Colour,
    /// How many rows under the frame its shadow shows.
    pub shadow_rows: i32,
    /// The shadow.
    pub shadow: Colour,
    /// Minimize and maximize.
    pub buttons: Glass,
    /// Close.
    pub close: Glass,
}

/// The colours of the boxes windowd shows over the windows: the window
/// menu and the list of windows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Panel {
    /// The box's fill.
    pub fill: Fill,
    /// The line around it, and between groups of lines.
    pub line: Colour,
    /// The width of the window menu.
    pub menu_width: i32,
    /// The width of the list of windows.
    pub list_width: i32,
    /// The radius of its corners.
    pub radius: i32,
    /// The room between its edge and its lines.
    pub padding: i32,
    /// The height of a line.
    pub row: i32,
    /// The radius of the corners of the line that is chosen.
    pub row_radius: i32,
    /// The height of the room a line between groups takes.
    pub gap: i32,
    /// How far the line between groups ends from the lines' sides.
    pub gap_inset: i32,
    /// The room left of a line's words, where a tick shows.
    pub indent: i32,
    /// The room right of a line's words.
    pub margin: i32,
    /// A line's words.
    pub words: Colour,
    /// The words of a line that cannot be chosen.
    pub words_off: Colour,
    /// The words that name a line's key.
    pub key: Colour,
    /// The fill of the line the pointer or the keys are on.
    pub chosen: Colour,
    /// The line around that line.
    pub chosen_line: Colour,
}

/// Every size and colour of a frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Theme {
    /// The frame at the sides and the bottom of a window's content.
    pub border: i32,
    /// The frame above it: the title bar.
    pub bar: i32,
    /// The radius of the frame's corners.
    pub radius: i32,
    /// The line outside the frame.
    pub outline: Colour,
    /// The ring just outside the content.
    pub content_ring: Colour,
    /// The line between that ring and the content.
    pub content_line: Colour,
    /// The frame of the window the person works in.
    pub active: Tone,
    /// The frames of the other windows.
    pub inactive: Tone,
    /// Where the icon's top left corner is in the frame.
    pub icon_at: (i32, i32),
    /// The icon's side.
    pub icon_side: i32,
    /// The icon's fill, its edge and its mark.
    pub icon: [Colour; 3],
    /// The room between the icon and the title, and between the title
    /// and the buttons.
    pub title_gap: i32,
    /// The copy of the title a row lower, which sets it off.
    pub title_shadow: Colour,
    /// The name of the title's font among the consoles' own.
    pub font: &'static str,
    /// The buttons' height.
    pub button_height: i32,
    /// The widths of minimize, maximize and close.
    pub button_widths: [i32; 3],
    /// The room between close and the frame's right edge.
    pub buttons_right: i32,
    /// The radius of the outer bottom corners of the row of buttons.
    pub button_radius: i32,
    /// The line at the buttons' sides and bottom.
    pub button_line: Colour,
    /// The side of the box a button's mark is drawn in.
    pub mark_side: i32,
    /// The thickness of the mark's strokes, in 10ths of a pixel.
    pub mark_stroke_tenths: i32,
    /// The mark.
    pub mark: Colour,
    /// How far a button under the pointer goes towards white, of 255.
    pub lit: u8,
    /// How far a pressed button goes towards black, of 255.
    pub dimmed: u8,
    /// How far outside the frame an edge is still taken hold of.
    pub grab: i32,
    /// How far along an edge from a corner both edges are taken hold
    /// of.
    pub corner: i32,
    /// How much of the title bar's top is its top edge.
    pub top_edge: i32,
    /// The width of the title bar a program's region always leaves, to
    /// move the window by.
    pub drag_strip: i32,
    /// The menu and the list.
    pub panel: Panel,
}

impl Theme {
    /// The Basic look, dark, in Twilight.
    pub const fn basic() -> Theme {
        let rgb = Colour::rgb;
        Theme {
            border: 5,
            bar: 36,
            radius: 8,
            outline: Colour::BLACK,
            content_ring: rgb(0x2f_4566),
            content_line: Colour::BLACK,
            active: Tone {
                fill: Fill {
                    top: rgb(0x3e_5c7e),
                    knee: Some((41, rgb(0x1b_3558))),
                    bottom: rgb(0x12_2640),
                },
                line: rgb(0x5f_86b3),
                title: Colour::WHITE,
                shadow_rows: 6,
                shadow: Colour::BLACK.covering(89),
                buttons: Glass {
                    stops: [rgb(0xa9_bdd4), rgb(0x6c_87a6), rgb(0x3f_5b7d), rgb(0x5d_7fa6)],
                    ring: rgb(0xc3_d3e6),
                },
                close: Glass {
                    stops: [rgb(0xef_a996), rgb(0xde_7056), rgb(0xc4_2e16), rgb(0xe2_6848)],
                    ring: rgb(0xff_d2c3),
                },
            },
            inactive: Tone {
                fill: Fill { top: rgb(0x2c_4463), knee: None, bottom: rgb(0x17_283f) },
                line: rgb(0x3f_5b7d),
                title: rgb(0xc9_d6e6),
                shadow_rows: 4,
                shadow: Colour::BLACK.covering(77),
                buttons: Glass {
                    stops: [rgb(0x6f_8299), rgb(0x4b_617c), rgb(0x33_475f), rgb(0x44_597a)],
                    ring: rgb(0x84_97ae),
                },
                close: Glass {
                    stops: [rgb(0x6f_8299), rgb(0x4b_617c), rgb(0x33_475f), rgb(0x44_597a)],
                    ring: rgb(0x84_97ae),
                },
            },
            icon_at: (8, 8),
            icon_side: 19,
            icon: [rgb(0x17_2e4a), rgb(0x6c_b2ff), rgb(0xe8_f0fa)],
            title_gap: 8,
            title_shadow: Colour::BLACK,
            font: "8x16",
            button_height: 22,
            button_widths: [33, 31, 56],
            buttons_right: 7,
            button_radius: 5,
            button_line: rgb(0x06_101f),
            mark_side: 13,
            mark_stroke_tenths: 26,
            mark: Colour::WHITE,
            lit: 64,
            dimmed: 38,
            grab: 8,
            corner: 20,
            top_edge: 4,
            drag_strip: 96,
            panel: Panel {
                fill: Fill { top: rgb(0x1e_3a5b), knee: None, bottom: rgb(0x17_2e4a) },
                line: rgb(0x3a_5677),
                menu_width: 330,
                list_width: 480,
                radius: 5,
                padding: 5,
                row: 30,
                row_radius: 3,
                gap: 11,
                gap_inset: 4,
                indent: 34,
                margin: 10,
                words: rgb(0xe8_f0fa),
                words_off: rgb(0x82_96ae),
                key: rgb(0xa2_b5cc),
                chosen: rgb(0x2b_5c97),
                chosen_line: rgb(0x6c_b2ff),
            },
        }
    }

    /// The tone of a window's frame: the active one's or the others'.
    pub fn tone(&self, active: bool) -> &Tone {
        if active { &self.active } else { &self.inactive }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fill_with_a_knee_meets_its_three_colours() {
        let fill = Theme::basic().active.fill;
        assert_eq!(fill.at(0, 400), Colour::rgb(0x3e_5c7e));
        assert_eq!(fill.at(41, 400), Colour::rgb(0x1b_3558));
        assert_eq!(fill.at(399, 400), Colour::rgb(0x12_2640));
        // Between two of them, a colour between.
        let between = fill.at(20, 400);
        assert!(between.red < 0x3e && between.red > 0x1b);
        // A box shorter than the knee is cut off, not squeezed.
        assert_eq!(fill.at(20, 30), between);
    }

    #[test]
    fn a_fill_without_a_knee_goes_straight() {
        let fill = Theme::basic().inactive.fill;
        assert_eq!(fill.at(0, 100), Colour::rgb(0x2c_4463));
        assert_eq!(fill.at(99, 100), Colour::rgb(0x17_283f));
        assert_eq!(fill.at(0, 1), Colour::rgb(0x2c_4463));
    }

    #[test]
    fn glass_is_two_halves_with_a_step_between() {
        let glass = Theme::basic().active.buttons;
        let rows = 21;
        let (top, bottom) = (glass.at(0, rows), glass.at(rows - 1, rows));
        // The first and last rows are half a row from the ends.
        assert!(top.red <= 0xa9 && top.red > 0xa0, "{top:?}");
        assert!(bottom.red <= 0x5d && bottom.red > 0x58, "{bottom:?}");
        // Row 9 ends above the middle and row 11 starts below it.
        assert!(glass.at(9, rows).red > 0x6c - 8);
        assert!(glass.at(11, rows).red < 0x3f + 8);
        // Lit, every colour is nearer white.
        let lit = glass.towards(Colour::WHITE, 64);
        assert!(lit.at(5, rows).red > glass.at(5, rows).red);
        assert!(lit.ring.red > glass.ring.red);
    }

    #[test]
    fn the_basic_look_is_the_approved_sizes() {
        let theme = Theme::basic();
        assert_eq!((theme.border, theme.bar, theme.radius), (5, 36, 8));
        assert_eq!(theme.button_widths.iter().sum::<i32>(), 120);
        assert_eq!(theme.tone(true).shadow_rows, 6);
        assert_eq!(theme.tone(false).shadow_rows, 4);
        // 35% and 30% of black.
        assert_eq!(theme.active.shadow.cover, 89);
        assert_eq!(theme.inactive.shadow.cover, 77);
    }
}
