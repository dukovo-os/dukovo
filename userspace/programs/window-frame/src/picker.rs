// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The picker of layouts windowd shows under a window's maximize
//! button: small pictures of a screen, each divided into the places a
//! window snaps to.
//!
//! There are three pictures: the two sides of the screen; the left
//! side and the two parts of the right; the four parts. Each division
//! is a target the person points at. [`place`] says how large the
//! picker is and where each target lies in it, from the picker's own
//! top left corner; [`draw`] draws it. What a window does when a
//! target is chosen is windowd's to know.

use crate::canvas::{Canvas, Rect, all_round};
use crate::theme::Theme;

/// Which of a side's rows a target stands for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Rows {
    /// The side from the top to the bottom.
    Both,
    /// The side's upper part.
    Upper,
    /// Its lower part.
    Lower,
}

/// A target: a division of a picture.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Target {
    /// Where it lies in the picker.
    pub rect: Rect,
    /// Whether it stands for the right side of a screen, not the left.
    pub right: bool,
    /// Which rows of that side.
    pub rows: Rows,
}

/// How large the picker is, and where its targets are.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Placed {
    /// The picker's width and height.
    pub size: (i32, i32),
    /// The targets, picture after picture.
    pub targets: Vec<Target>,
}

impl Placed {
    /// The target the point (`x`, `y`) is in, if any.
    pub fn target_at(&self, x: i32, y: i32) -> Option<usize> {
        self.targets.iter().position(|target| target.rect.contains(x, y))
    }
}

/// The divisions of each picture: the side, and which of its rows.
const PICTURES: [&[(bool, Rows)]; 3] = [
    &[(false, Rows::Both), (true, Rows::Both)],
    &[(false, Rows::Both), (true, Rows::Upper), (true, Rows::Lower)],
    &[(false, Rows::Upper), (true, Rows::Upper), (false, Rows::Lower), (true, Rows::Lower)],
];

/// Places the picker's targets.
pub fn place(theme: &Theme) -> Placed {
    let panel = &theme.panel;
    let (width, height) = panel.picture;
    // The line around the picker is 1.
    let edge = 1 + panel.padding;
    // A picture's left half and its upper half, with the room between
    // two divisions left out.
    let (half, upper) = ((width - panel.picture_room) / 2, (height - panel.picture_room) / 2);
    let mut targets = Vec::new();
    for (picture, divisions) in PICTURES.iter().enumerate() {
        let left = edge + picture as i32 * (width + panel.picture_gap);
        for (right, rows) in divisions.iter().copied() {
            let (x, w) = if right {
                (left + half + panel.picture_room, width - half - panel.picture_room)
            } else {
                (left, half)
            };
            let (y, h) = match rows {
                Rows::Both => (edge, height),
                Rows::Upper => (edge, upper),
                Rows::Lower => {
                    (edge + upper + panel.picture_room, height - upper - panel.picture_room)
                }
            };
            targets.push(Target { rect: Rect::new(x, y, w, h), right, rows });
        }
    }
    let pictures = PICTURES.len() as i32;
    let across = pictures * width + (pictures - 1) * panel.picture_gap;
    Placed { size: (across + 2 * edge, height + 2 * edge), targets }
}

/// Draws the picker, with the target `hot` shown as the one the pointer
/// is on, where it lies in `canvas`. What the canvas held is gone.
pub fn draw(theme: &Theme, hot: Option<usize>, canvas: &mut Canvas<'_>) {
    let panel = &theme.panel;
    let placed = place(theme);
    let around = Rect::new(0, 0, placed.size.0, placed.size.1);
    canvas.clear();
    canvas.round(around, all_round(panel.radius), |_| panel.line);
    canvas.round(around.grown(-1), all_round(panel.radius - 1), |y| {
        panel.fill.at(y - 1, around.h - 2)
    });
    for (index, target) in placed.targets.iter().enumerate() {
        let (line, fill) = if hot == Some(index) {
            (panel.chosen_line, panel.chosen)
        } else {
            (panel.line, panel.target)
        };
        canvas.round(target.rect, all_round(panel.row_radius), |_| line);
        canvas.round(target.rect.grown(-1), all_round(panel.row_radius - 1), |_| fill);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::draw::tests::at;

    fn drawn(hot: Option<usize>) -> (Rect, Vec<u8>) {
        let theme = Theme::basic();
        let placed = place(&theme);
        let rect = Rect::new(0, 0, placed.size.0, placed.size.1);
        let mut pixels = vec![0x55; Canvas::bytes_of(&rect)];
        draw(&theme, hot, &mut Canvas::over(rect, &mut pixels));
        (rect, pixels)
    }

    fn rgb(value: u32) -> [u8; 4] {
        [(value >> 16) as u8, (value >> 8) as u8, value as u8, 255]
    }

    #[test]
    fn three_pictures_stand_side_by_side_inside_the_padding() {
        let placed = place(&Theme::basic());
        assert_eq!(placed.size, (12 + 3 * 72 + 2 * 10, 12 + 48));
        assert_eq!(placed.targets.len(), 9);
        // The two sides.
        assert_eq!(placed.targets[0].rect, Rect::new(6, 6, 35, 48));
        assert_eq!(placed.targets[1].rect, Rect::new(43, 6, 35, 48));
        // The left side and the right's parts.
        assert_eq!(placed.targets[2].rect, Rect::new(88, 6, 35, 48));
        assert_eq!(placed.targets[3].rect, Rect::new(125, 6, 35, 23));
        assert_eq!(placed.targets[4].rect, Rect::new(125, 31, 35, 23));
        // The four parts.
        assert_eq!(placed.targets[5].rect, Rect::new(170, 6, 35, 23));
        assert_eq!(placed.targets[8].rect, Rect::new(207, 31, 35, 23));
        let stands_for = |index: usize| (placed.targets[index].right, placed.targets[index].rows);
        assert_eq!(stands_for(1), (true, Rows::Both));
        assert_eq!(stands_for(4), (true, Rows::Lower));
        assert_eq!(stands_for(7), (false, Rows::Lower));
    }

    #[test]
    fn a_point_is_in_one_target_or_in_none() {
        let placed = place(&Theme::basic());
        assert_eq!(placed.target_at(6, 6), Some(0));
        assert_eq!(placed.target_at(41, 30), None);
        assert_eq!(placed.target_at(43, 53), Some(1));
        assert_eq!(placed.target_at(130, 29), None);
        assert_eq!(placed.target_at(130, 31), Some(4));
        assert_eq!(placed.target_at(0, 0), None);
        for (index, target) in placed.targets.iter().enumerate() {
            let others = placed.targets.iter().skip(index + 1);
            assert!(others.into_iter().all(|other| target.rect.shared(&other.rect).is_empty()));
        }
    }

    #[test]
    fn the_target_under_the_pointer_is_lit_and_no_other_changes() {
        let (rect, plain) = drawn(None);
        let (_, hot) = drawn(Some(3));
        assert_eq!(at(&plain, rect, 0, 0), [0; 4]);
        assert_eq!(at(&plain, rect, 0, 30), rgb(0x3a_5677));
        assert_eq!(at(&plain, rect, 130, 15), rgb(0x2c_4a6e));
        assert_eq!(at(&hot, rect, 130, 15), rgb(0x2b_5c97));
        assert_eq!(at(&hot, rect, 125, 15), rgb(0x6c_b2ff));
        assert_eq!(at(&hot, rect, 130, 40), rgb(0x2c_4a6e));
        let differ = plain
            .as_chunks::<4>()
            .0
            .iter()
            .zip(hot.as_chunks::<4>().0.iter())
            .enumerate()
            .filter(|(_, (one, other))| one != other)
            .map(|(index, _)| (index as i32 % rect.w, index as i32 / rect.w));
        let lit = place(&Theme::basic()).targets[3].rect;
        assert!(differ.clone().count() > 0);
        assert!(differ.into_iter().all(|(x, y)| lit.contains(x, y)));
    }
}
