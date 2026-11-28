// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The frame windowd draws around every window (windowd(1), ADR 0718):
//! the title bar with the window's name and its three buttons, and the
//! edges a window is resized by.
//!
//! The library knows nothing of Wayland or of the screens. Given the
//! size of a window's content and what the frame is to show, it says
//! where each part of the frame is and what its pixels are, so that
//! windowd and the boot tests on the host, which compare a screen with
//! what it should show pixel for pixel, draw the same frame.
//!
//! - [`theme`] is the one table of the frame's sizes and colours.
//! - [`layout`] is where the parts are, and which part a point is in.
//! - [`canvas`] is the pixels a frame is drawn in, and what is drawn:
//!   boxes with round corners, fills that change from row to row,
//!   strokes and text.
//! - [`draw`] is the frame itself.
//! - [`panel`] is the boxes windowd shows over the windows: its menu
//!   and its list.
//!
//! Places are in pixels from the top left corner of the frame's box,
//! which is what the frame's fill covers: a line and a shadow lie
//! outside it, so a place may be negative.

#![forbid(unsafe_code)]

pub mod canvas;
pub mod draw;
pub mod layout;
pub mod panel;
pub mod theme;

pub use canvas::{Canvas, Colour, Rect, shown_over};
pub use draw::Look;
pub use layout::{Bar, Button, Edge, Layout, Margins, Part, Shape};
pub use theme::Theme;
