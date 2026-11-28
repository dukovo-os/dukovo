// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The Wayland protocols that are Dukovo's own, as the window server
//! and a program speak them.
//!
//! A protocol is written in XML, in `protocols/`, which is what a
//! toolkit's own tools read. The window server and the test program
//! speak it from the tables here: every interface's requests and
//! events with their arguments, written out by hand and held to the
//! XML by this crate's tests. The Wayland crates' tool that writes
//! such tables from the XML writes `unsafe` code with them, for the C
//! library neither side uses, and the desktop's programs have none
//! (docs/coding-standard.md, ADR 0720).
//!
//! Requests and events are numbered in the order the XML has them.

#![forbid(unsafe_code)]

use wayland_backend::protocol::Interface;

pub mod session;
pub mod title_bar;
#[cfg(test)]
mod xml;

/// The interface a request of these protocols names a window by. An
/// argument's interface is held to its name alone, so the name is all
/// there is of it here: the interface itself is the xdg-shell
/// protocol's.
pub static XDG_TOPLEVEL: Interface =
    Interface { name: "xdg_toplevel", version: 1, requests: &[], events: &[], c_ptr: None };
