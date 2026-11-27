// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! What the display boot tests' program and the host that watches it
//! share: the pictures the program draws, and the lines it says.
//!
//! The program, `display-test`, draws on the machine's screens through
//! the display layer (radix-display(2)) and says in the log what each
//! screen should show. The boot test on the host takes a picture of each
//! screen and compares it with what the lines say, pixel for pixel.
//!
//! - [`pattern`] is the pictures: the colour of each pixel is worked out
//!   from where it is and a number, so that the host can make the same
//!   picture, and so that a picture shown a pixel off, with the wrong
//!   row length or from another picture's memory does not pass.
//! - [`said`] is the lines.

#![forbid(unsafe_code)]

pub mod pattern;
pub mod said;
