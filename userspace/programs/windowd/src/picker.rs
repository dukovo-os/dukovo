// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The picker of layouts windowd shows under a window's maximize
//! button when the pointer rests on it (ADR 0727).
//!
//! A [`Picker`] is small pictures of a screen, each divided into zones;
//! the pointer chooses a zone for the window. The picker's look and
//! the zones' places in it are the frame library's
//! ([`window_frame::picker`]).

use std::convert::Infallible;

use smithay::backend::allocator::Fourcc;
use smithay::backend::renderer::element::memory::MemoryRenderBuffer;
use smithay::utils::{Logical, Point, Rectangle, Transform};
use window_frame::picker::{self, Placed, Rows};
use window_frame::{Canvas, Rect, Theme};

use crate::windows::zones::{Level, Side, Zone};

/// A picker on the desktop.
#[derive(Debug)]
pub struct Picker {
    at: Point<i32, Logical>,
    placed: Placed,
    /// The target the pointer is on.
    hot: Option<usize>,
    pixels: MemoryRenderBuffer,
}

impl Picker {
    /// A picker under `button`, a button's rectangle on the desktop:
    /// in the middle of the button's width, or as near to that as
    /// leaves it whole on `screen`.
    pub fn new(
        theme: &Theme,
        button: Rectangle<i32, Logical>,
        screen: Rectangle<i32, Logical>,
    ) -> Picker {
        let placed = picker::place(theme);
        let pixels = MemoryRenderBuffer::new(Fourcc::Argb8888, (1, 1), 1, Transform::Normal, None);
        let (width, height) = placed.size;
        let within =
            |at: i32, size: i32, from: i32, room: i32| at.min(from + room - size).max(from);
        let at = Point::from((
            within(button.loc.x + (button.size.w - width) / 2, width, screen.loc.x, screen.size.w),
            within(button.loc.y + button.size.h, height, screen.loc.y, screen.size.h),
        ));
        let mut picker = Picker { at, placed, hot: None, pixels };
        picker.draw(theme);
        picker
    }

    fn draw(&mut self, theme: &Theme) {
        let (hot, size) = (self.hot, self.placed.size);
        let all = Rect::new(0, 0, size.0, size.1);
        let mut context = self.pixels.render();
        context.resize(size);
        let drawn = context.draw(|pixels| {
            picker::draw(theme, hot, &mut Canvas::over(all, pixels));
            Ok::<_, Infallible>(vec![Rectangle::from_size(size.into())])
        });
        let Ok(()) = drawn;
    }

    /// The picker's area of the desktop.
    pub fn area(&self) -> Rectangle<i32, Logical> {
        Rectangle::new(self.at, self.placed.size.into())
    }

    /// The picker's pixels.
    pub fn pixels(&self) -> &MemoryRenderBuffer {
        &self.pixels
    }

    /// The target at the place `at` of the desktop.
    fn target_at(&self, at: Point<f64, Logical>) -> Option<usize> {
        let within = (at - self.at.to_f64()).to_i32_floor::<i32>();
        self.placed.target_at(within.x, within.y)
    }

    /// The zone the target at the place `at` of the desktop stands
    /// for.
    pub fn zone_at(&self, at: Point<f64, Logical>) -> Option<Zone> {
        let target = self.placed.targets[self.target_at(at)?];
        let side = if target.right { Side::Right } else { Side::Left };
        Some(match target.rows {
            Rows::Both => Zone::Half(side),
            Rows::Upper => Zone::Quarter(side, Level::Upper),
            Rows::Lower => Zone::Quarter(side, Level::Lower),
        })
    }

    /// The pointer is at the place `at` of the desktop: the target
    /// there shows so. Returns whether that is another than before.
    pub fn point(&mut self, theme: &Theme, at: Point<f64, Logical>) -> bool {
        let hot = self.target_at(at);
        if hot == self.hot {
            return false;
        }
        self.hot = hot;
        self.draw(theme);
        true
    }
}
