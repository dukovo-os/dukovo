// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! What Spark (spark(8)) and the programs that ask it about services
//! share (ADR 0470): the rules of the service graph ([`graph`]), when a
//! service listening on a port is ready ([`ready`]), and the JSON of
//! Spark's `show` answer ([`show`]), which service(8), sysupdate's health
//! check, bundled(8) and the web console read.
//!
//! Nothing here reads a file, a clock or the control socket: the callers
//! pass in what they found, so every rule is tested on the host with
//! `./ob test unit`. A service that starts in the wrong order is found
//! there first; Spark's own side is the `service-graph` boot test.

#![no_std]

extern crate alloc;

pub mod graph;
pub mod ready;
pub mod show;
