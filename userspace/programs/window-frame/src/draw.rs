// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! A frame's pixels.
//!
//! [`frame`] draws a whole frame on a canvas, which is cut to the
//! canvas's own rectangle: windowd calls it once for each of a
//! layout's strips, and a test on the host for a canvas of the frame's
//! bounds, and both have the same pixels. The place of the window's
//! content is left see-through, and so is the region of a shared title
//! bar that the window's program draws: the frame lies over the
//! window's surface, which shows through those two.
//!
//! What is drawn, from the back: the shadow, the black line outside the
//! box, the box with its inner line and its fill, the ring and the line
//! around the content, the icon, the title, the buttons.

use radix_fbcon::Font;

use crate::canvas::{Canvas, Colour, Line, Radii, Rect, all_round};
use crate::layout::{Bar, Button, Layout, Shape};
use crate::theme::{Glass, Theme};

/// What a frame shows beside its sizes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Look<'a> {
    /// The window's title.
    pub title: &'a str,
    /// Whether the window is the one the person works in.
    pub active: bool,
    /// The button the pointer is on.
    pub hot: Option<Button>,
    /// The button that is pressed.
    pub pressed: Option<Button>,
}

/// What stands for the end of words that were cut.
const CUT: &str = "...";

/// The side of the box the buttons' marks are given in.
const MARK_BOX: f32 = 10.0;

/// A stroke of a mark, from one point of the mark's box to another.
type Stroke = ((f32, f32), (f32, f32));

/// Minimize: a line along the bottom.
const MINIMIZE: [Stroke; 1] = [((1.0, 8.0), (9.0, 8.0))];

/// Maximize: a box. The lines across reach half a stroke further, to
/// make the corners whole.
const MAXIMIZE: [Stroke; 4] = [
    ((0.0, 1.0), (10.0, 1.0)),
    ((0.0, 8.0), (10.0, 8.0)),
    ((1.0, 1.0), (1.0, 8.0)),
    ((9.0, 1.0), (9.0, 8.0)),
];

/// Restore: a box, and the top and right of another behind it.
const RESTORE: [Stroke; 6] = [
    ((0.0, 4.0), (7.5, 4.0)),
    ((0.0, 8.5), (7.5, 8.5)),
    ((1.0, 4.0), (1.0, 8.5)),
    ((6.5, 4.0), (6.5, 8.5)),
    ((3.0, 1.0), (10.0, 1.0)),
    ((9.0, 1.0), (9.0, 6.0)),
];

/// Close: a cross.
const CLOSE: [Stroke; 2] = [((1.5, 1.5), (8.5, 8.5)), ((8.5, 1.5), (1.5, 8.5))];

/// A tick, in a box of [`MARK_BOX`].
const TICK: [Stroke; 2] = [((1.5, 5.5), (4.0, 8.0)), ((4.0, 8.0), (8.5, 2.5))];

/// The font the theme names, if the consoles have it.
pub(crate) fn font_of(theme: &Theme) -> Option<Font<'static>> {
    radix_fbcon::builtin_font(theme.font)
}

/// Draws the strokes `strokes` of a mark in the box `side` wide and
/// high whose top left corner is `at`.
fn mark(
    canvas: &mut Canvas<'_>,
    theme: &Theme,
    at: (f32, f32),
    strokes: &[Stroke],
    colour: Colour,
) {
    let scale = theme.mark_side as f32 / MARK_BOX;
    let point = |(x, y): (f32, f32)| (at.0 + x * scale, at.1 + y * scale);
    let lines: Vec<Line> =
        strokes.iter().map(|(from, to)| Line { from: point(*from), to: point(*to) }).collect();
    canvas.stroke(&lines, theme.mark_stroke_tenths as f32 / 10.0, colour);
}

/// Draws a tick in the middle of `room`.
pub(crate) fn tick(canvas: &mut Canvas<'_>, theme: &Theme, room: Rect, colour: Colour) {
    let side = theme.mark_side;
    let at = (room.x + (room.w - side) / 2, room.y + (room.h - side) / 2);
    mark(canvas, theme, (at.0 as f32, at.1 as f32), &TICK, colour);
}

/// Draws the icon of a window that has none of its own, its top left
/// corner at `at`: a small window.
pub(crate) fn icon(canvas: &mut Canvas<'_>, theme: &Theme, at: (i32, i32)) {
    let [fill, edge, bar] = theme.icon;
    let side = theme.icon_side;
    let around = Rect::new(at.0, at.1 + 1, side, side - 2);
    canvas.round(around, all_round(3), |_| edge);
    canvas.round(around.grown(-1), all_round(2), |_| fill);
    canvas.fill(Rect::new(around.x + 3, around.y + 3, side - 6, 2), bar);
}

/// Writes `words` from `at`, in no more than `room` columns: words that
/// are wider end in "...". `under` is the colour of a copy one row
/// lower, which sets the words off.
pub(crate) fn words(
    canvas: &mut Canvas<'_>,
    font: &Font<'_>,
    at: (i32, i32),
    room: i32,
    words: &str,
    colour: Colour,
    under: Option<Colour>,
) {
    let fits = (room.max(0) as usize) / font.width.max(1);
    let cut: String;
    let shown = if words.chars().count() <= fits {
        words
    } else if fits > CUT.len() {
        cut = words.chars().take(fits - CUT.len()).chain(CUT.chars()).collect();
        &cut
    } else {
        return;
    };
    if let Some(under) = under {
        canvas.text(font, (at.0, at.1 + 1), shown, under);
    }
    canvas.text(font, at, shown, colour);
}

/// The row at which words of `font` are written to stand in the middle
/// of the rows from `top` for `rows`.
pub(crate) fn middle(font: &Font<'_>, top: i32, rows: i32) -> i32 {
    top + (rows - font.height as i32) / 2
}

/// Draws the button `button` of `layout`.
fn button(
    canvas: &mut Canvas<'_>,
    theme: &Theme,
    layout: &Layout,
    look: &Look<'_>,
    button: Button,
) {
    let around = layout.button(button);
    // The dark line is at the row's two ends, between the buttons and
    // below them; the row's outer bottom corners are round.
    let radius = theme.button_radius;
    let (left, radii): (i32, Radii) = match button {
        Button::Minimize => (1, [0, 0, 0, radius]),
        Button::Maximize => (0, [0; 4]),
        Button::Close => (0, [0, 0, radius, 0]),
    };
    let inside = Rect::new(around.x + left, around.y, around.w - left - 1, around.h - 1);
    let within = |radii: Radii| radii.map(|radius| (radius - 1).max(0));

    let tone = theme.tone(look.active);
    let glass_of = |tone: &crate::theme::Tone| match button {
        Button::Close => tone.close,
        _ => tone.buttons,
    };
    // Under the pointer and pressed, a button of any window is as the
    // active window's, brighter or darker.
    let glass: Glass = if look.pressed == Some(button) {
        glass_of(&theme.active).towards(Colour::BLACK, theme.dimmed)
    } else if look.hot == Some(button) {
        glass_of(&theme.active).towards(Colour::WHITE, theme.lit)
    } else {
        glass_of(tone)
    };

    canvas.round(around, radii, |_| theme.button_line);
    canvas.round(inside, within(radii), |_| glass.ring);
    canvas.round(inside.grown(-1), within(within(radii)), |y| glass.at(y - inside.y, inside.h));

    let strokes: &[Stroke] = match (button, layout.shape) {
        (Button::Minimize, _) => &MINIMIZE,
        (Button::Maximize, Shape::Normal) => &MAXIMIZE,
        (Button::Maximize, Shape::Maximized) => &RESTORE,
        (Button::Close, _) => &CLOSE,
    };
    let side = theme.mark_side as f32;
    let at = (
        inside.x as f32 + (inside.w as f32 - side) / 2.0,
        inside.y as f32 + (inside.h as f32 - side) / 2.0,
    );
    mark(canvas, theme, at, strokes, theme.mark);
}

/// Draws the frame of `layout` as `look` has it, where it lies in
/// `canvas`. What the canvas held is gone.
pub fn frame(theme: &Theme, layout: &Layout, look: &Look<'_>, canvas: &mut Canvas<'_>) {
    canvas.clear();
    let tone = theme.tone(look.active);
    let around = layout.frame;
    match layout.shape {
        Shape::Normal => {
            let radius = theme.radius;
            let shadow = around.moved(0, tone.shadow_rows);
            canvas.round(shadow, all_round(radius), |_| tone.shadow);
            canvas.round(around.grown(1), all_round(radius + 1), |_| theme.outline);
            canvas.round(around, all_round(radius), |_| tone.line);
            canvas.round(around.grown(-1), all_round(radius - 1), |y| tone.fill.at(y, around.h));
        }
        Shape::Maximized => canvas.fill_rows(around, |y| tone.fill.at(y, around.h)),
    }
    canvas.fill(layout.content.grown(2), theme.content_ring);
    canvas.fill(layout.content.grown(1), theme.content_line);

    icon(canvas, theme, (layout.icon.x, layout.icon.y));
    if layout.bar == Bar::Full
        && let Some(font) = font_of(theme)
    {
        let at = (layout.title.x, middle(&font, layout.title.y, layout.title.h));
        let under = Some(theme.title_shadow);
        words(canvas, &font, at, layout.title.w, look.title, tone.title, under);
    }
    for each in Button::ALL {
        button(canvas, theme, layout, look, each);
    }
    canvas.open(layout.content);
    if let Some(region) = layout.region {
        canvas.open(region);
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::canvas::PIXEL;

    /// What a frame of `layout` and `look` is on a canvas of `rect`.
    pub(crate) fn drawn(layout: &Layout, look: &Look<'_>, rect: Rect) -> Vec<u8> {
        let mut pixels = vec![0xaa; Canvas::bytes_of(&rect)];
        frame(&Theme::basic(), layout, look, &mut Canvas::over(rect, &mut pixels));
        pixels
    }

    /// The pixel at (`x`, `y`) of the pixels of `rect`, as red, green,
    /// blue and cover.
    pub(crate) fn at(pixels: &[u8], rect: Rect, x: i32, y: i32) -> [u8; 4] {
        let index = ((y - rect.y) * rect.w + (x - rect.x)) as usize * PIXEL;
        let [blue, green, red, cover] = pixels[index..index + PIXEL].try_into().unwrap();
        [red, green, blue, cover]
    }

    fn rgb(value: u32) -> [u8; 4] {
        [(value >> 16) as u8, (value >> 8) as u8, value as u8, 255]
    }

    const PLAIN: Look<'static> = Look { title: "Terminal", active: true, hot: None, pressed: None };

    fn normal() -> Layout {
        Layout::of(&Theme::basic(), (320, 240), Shape::Normal, Bar::Full, None)
    }

    #[test]
    fn the_frame_is_the_approved_lines_from_the_outside_in() {
        let layout = normal();
        let bounds = layout.bounds();
        let pixels = drawn(&layout, &PLAIN, bounds);
        let at = |x, y| at(&pixels, bounds, x, y);
        // At the left, half way down: black, the inner line, two of
        // fill, the ring, black, the content's place.
        assert_eq!(at(-1, 150), rgb(0));
        assert_eq!(at(0, 150), rgb(0x5f_86b3));
        assert_eq!(at(1, 150), at(2, 150));
        assert_eq!(at(3, 150), rgb(0x2f_4566));
        assert_eq!(at(4, 150), rgb(0));
        assert_eq!(at(5, 150), [0; 4]);
        assert_eq!(at(324, 150), [0; 4]);
        assert_eq!(at(325, 150), rgb(0));
        assert_eq!(at(326, 150), rgb(0x2f_4566));
        assert_eq!(at(329, 150), rgb(0x5f_86b3));
        assert_eq!(at(330, 150), rgb(0));
        // Above the content: the ring, black, the content.
        assert_eq!(at(100, 34), rgb(0x2f_4566));
        assert_eq!(at(100, 35), rgb(0));
        assert_eq!(at(100, 36), [0; 4]);
        // Below it: black, the ring, two of fill, the inner line,
        // black, then the shadow for five rows.
        assert_eq!(at(100, 275), [0; 4]);
        assert_eq!(at(100, 276), rgb(0));
        assert_eq!(at(100, 277), rgb(0x2f_4566));
        assert_eq!(at(100, 278), at(100, 279));
        assert_eq!(at(100, 280), rgb(0x5f_86b3));
        assert_eq!(at(100, 281), rgb(0));
        for row in 282..287 {
            assert_eq!(at(100, row), [0, 0, 0, 89], "row {row}");
        }
        // The top: black, the inner line, the fill's first colours.
        assert_eq!(at(100, -1), rgb(0));
        assert_eq!(at(100, 0), rgb(0x5f_86b3));
        let fill = Theme::basic().active.fill;
        let colour = fill.at(1, 281);
        assert_eq!(at(100, 1), [colour.red, colour.green, colour.blue, 255]);
        // The tips of the corners are open.
        assert_eq!(at(-1, -1), [0; 4]);
        assert_eq!(at(330, -1), [0; 4]);
    }

    #[test]
    fn an_inactive_frame_is_paler_with_a_lower_shadow() {
        let layout = normal();
        let bounds = layout.bounds();
        let pixels = drawn(&layout, &Look { active: false, ..PLAIN }, bounds);
        let at = |x, y| at(&pixels, bounds, x, y);
        assert_eq!(at(0, 150), rgb(0x3f_5b7d));
        assert_eq!(at(100, 284), [0, 0, 0, 77]);
        assert_eq!(at(100, 285), [0; 4]);
        // The title is the paler colour somewhere in its rows.
        let pale = rgb(0xc9_d6e6);
        assert!((35..99).any(|x| (9..25).any(|y| at(x, y) == pale)));
    }

    #[test]
    fn the_strips_together_are_the_frame_drawn_whole() {
        let layout = normal();
        let bounds = layout.bounds();
        let whole = drawn(&layout, &PLAIN, bounds);
        let mut seen = 0;
        for strip in layout.strips() {
            let pixels = drawn(&layout, &PLAIN, strip);
            for y in strip.y..strip.bottom() {
                for x in strip.x..strip.right() {
                    assert_eq!(at(&pixels, strip, x, y), at(&whole, bounds, x, y), "({x}, {y})");
                    seen += 1;
                }
            }
        }
        // And nothing of the frame is outside them.
        let lit = whole.as_chunks::<PIXEL>().0.iter().filter(|pixel| pixel[3] != 0).count();
        assert!(seen >= lit);
        for y in bounds.y..bounds.bottom() {
            for x in bounds.x..bounds.right() {
                let held = layout.strips().iter().any(|strip| strip.contains(x, y));
                assert!(held || at(&whole, bounds, x, y) == [0; 4], "({x}, {y})");
            }
        }
    }

    #[test]
    fn the_title_is_white_over_a_black_copy_and_ends_before_the_buttons() {
        let layout = normal();
        let bounds = layout.bounds();
        let long = "A title that is far too long for a window as narrow as this one is";
        let pixels = drawn(&layout, &Look { title: long, ..PLAIN }, bounds);
        let at = |x, y| at(&pixels, bounds, x, y);
        let white = rgb(0xff_ffff);
        let columns: Vec<i32> =
            (0..330).filter(|x| (0..36).any(|y| at(*x, y) == white && *x < 200)).collect();
        assert!(*columns.first().unwrap() >= layout.title.x);
        assert!(*columns.last().unwrap() < layout.title.right());
        // Under a white pixel whose lower neighbour is not white, black.
        let (x, y) = (layout.title.x..layout.title.right())
            .flat_map(|x| (9..26).map(move |y| (x, y)))
            .find(|(x, y)| at(*x, *y) == white && at(*x, y + 1) != white)
            .unwrap();
        assert_eq!(at(x, y + 1), rgb(0));
        // With no room for more than the dots, nothing is written.
        let narrow = Layout::of(&Theme::basic(), (160, 100), Shape::Normal, Bar::Full, None);
        assert!(narrow.title.w < 32);
        let bounds = narrow.bounds();
        let pixels = drawn(&narrow, &Look { title: long, ..PLAIN }, bounds);
        let before_the_buttons = narrow.icon.right()..narrow.button(Button::Minimize).x;
        let mut title_bar = before_the_buttons.flat_map(|x| (0..36).map(move |y| (x, y)));
        assert!(title_bar.all(|(x, y)| super::tests::at(&pixels, bounds, x, y) != white));
    }

    #[test]
    fn the_buttons_are_glass_with_their_marks() {
        let layout = normal();
        let bounds = layout.bounds();
        let pixels = drawn(&layout, &PLAIN, bounds);
        let at = |x, y| at(&pixels, bounds, x, y);
        let (minimize, maximize, close) = (
            layout.button(Button::Minimize),
            layout.button(Button::Maximize),
            layout.button(Button::Close),
        );
        // The dark line at the row's ends, between two buttons and
        // below; the light ring inside it.
        let line = rgb(0x06_101f);
        assert_eq!(at(minimize.x, 10), line);
        assert_eq!(at(minimize.right() - 1, 10), line);
        assert_eq!(at(maximize.right() - 1, 10), line);
        assert_eq!(at(close.right() - 1, 10), line);
        assert_eq!(at(maximize.x + 5, 21), line);
        assert_eq!(at(minimize.x + 1, 10), rgb(0xc3_d3e6));
        assert_eq!(at(maximize.x, 10), rgb(0xc3_d3e6));
        assert_eq!(at(maximize.x + 5, 0), rgb(0xc3_d3e6));
        assert_eq!(at(maximize.x + 5, 20), rgb(0xc3_d3e6));
        assert_eq!(at(close.x + 5, 20), rgb(0xff_d2c3));
        // Close is red and the others blue, lighter above than below.
        let (red, blue) = (at(close.x + 5, 4), at(maximize.x + 3, 4));
        assert!(red[0] > red[2] && blue[2] > blue[0]);
        assert!(at(maximize.x + 3, 4)[0] > at(maximize.x + 3, 14)[0]);
        // The marks are white: minimize's line low, close's cross in
        // the middle.
        let white = rgb(0xff_ffff);
        assert_eq!(at(minimize.x + 16, 14), white);
        assert_eq!(at(close.x + 27, 10), white);
        assert_ne!(at(minimize.x + 16, 6), white);
        // The row's outer bottom corners are round: their tips show
        // the frame's fill.
        assert_ne!(at(minimize.x, 21), line);
        assert_ne!(at(close.right() - 1, 21), line);
        assert_eq!(at(minimize.x + 6, 21), line);
    }

    #[test]
    fn a_button_under_the_pointer_is_brighter_and_a_pressed_one_darker() {
        let layout = normal();
        let bounds = layout.bounds();
        let plain = drawn(&layout, &PLAIN, bounds);
        let hot = drawn(&layout, &Look { hot: Some(Button::Maximize), ..PLAIN }, bounds);
        let pressed =
            Look { hot: Some(Button::Maximize), pressed: Some(Button::Maximize), ..PLAIN };
        let pressed = drawn(&layout, &pressed, bounds);
        let maximize = layout.button(Button::Maximize);
        let probe = (maximize.x + 3, 4);
        let red = |pixels: &[u8]| at(pixels, bounds, probe.0, probe.1)[0];
        assert!(red(&hot) > red(&plain));
        assert!(red(&pressed) < red(&plain));
        // The others stay as they were.
        let close = layout.button(Button::Close);
        assert_eq!(at(&hot, bounds, close.x + 5, 4), at(&plain, bounds, close.x + 5, 4));
        // An inactive window's close is grey, and red under the
        // pointer.
        let inactive = Look { active: false, ..PLAIN };
        let grey = at(&drawn(&layout, &inactive, bounds), bounds, close.x + 5, 4);
        assert!(grey[2] > grey[0]);
        let hot = Look { hot: Some(Button::Close), ..inactive };
        let red = at(&drawn(&layout, &hot, bounds), bounds, close.x + 5, 4);
        assert!(red[0] > red[2]);
    }

    #[test]
    fn a_maximized_frame_is_a_square_bar_with_the_restore_mark() {
        let theme = Theme::basic();
        let layout = Layout::of(&theme, (800, 564), Shape::Maximized, Bar::Full, None);
        let bounds = layout.bounds();
        let pixels = drawn(&layout, &PLAIN, bounds);
        let at = |x, y| at(&pixels, bounds, x, y);
        // No line and no round corner: the fill from the first pixel.
        let fill = theme.active.fill.at(0, 600);
        assert_eq!(at(0, 0), [fill.red, fill.green, fill.blue, 255]);
        assert_eq!(at(799, 0), at(0, 0));
        assert_eq!(at(400, 34), rgb(0x2f_4566));
        assert_eq!(at(400, 35), rgb(0));
        assert_eq!(at(400, 36), [0; 4]);
        assert_eq!(at(0, 599), [0; 4]);
        // The mark differs from maximize's.
        let normal = Layout::of(&theme, (790, 559), Shape::Normal, Bar::Full, None);
        let other = drawn(&normal, &PLAIN, normal.bounds());
        let (here, there) = (layout.button(Button::Maximize), normal.button(Button::Maximize));
        let differs = (0..here.w).any(|across| {
            (0..here.h).any(|down| {
                at(here.x + across, down)
                    != super::tests::at(&other, normal.bounds(), there.x + across, down)
            })
        });
        assert!(differs);
    }

    #[test]
    fn a_shared_bar_has_no_title_and_is_open_where_the_program_draws() {
        let theme = Theme::basic();
        let region = Some(Rect::new(40, 4, 200, 28));
        let layout = Layout::of(&theme, (320, 276), Shape::Normal, Bar::Shared, region);
        let bounds = layout.bounds();
        let pixels = drawn(&layout, &PLAIN, bounds);
        let full = drawn(&normal(), &Look { title: "", ..PLAIN }, bounds);
        // The same frame as a full bar's with no title, but for the
        // region, which is open.
        assert_eq!(layout.frame, normal().frame);
        let region = layout.region.unwrap();
        for y in bounds.y..bounds.bottom() {
            for x in bounds.x..bounds.right() {
                let here = at(&pixels, bounds, x, y);
                if region.contains(x, y) {
                    assert_eq!(here, [0; 4], "({x}, {y})");
                } else {
                    assert_eq!(here, at(&full, bounds, x, y), "({x}, {y})");
                }
            }
        }
    }

    #[test]
    fn the_basic_look_has_its_font() {
        assert!(font_of(&Theme::basic()).is_some());
    }
}
