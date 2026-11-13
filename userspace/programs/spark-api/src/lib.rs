// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! What Spark (spark(8)) and the programs that ask it about services
//! share (ADR 0470): the rules of the service graph ([`graph`]), when a
//! service listening on a port is ready ([`ready`]), the JSON of Spark's
//! `show` answer ([`show`]), which service(8), sysupdate's health check,
//! bundled(8) and the web console read, and settings tried with an undo
//! ([`trial`], ADR 0472). Three modules turn the document into words:
//! [`explain`] says why a service is in its state, for `service NAME
//! explain` and the console alike, [`dry_run`] says what putting a
//! service file to use would do, for `service check` and the console, and
//! [`words`] holds the phrases they and service(8) share.
//!
//! Nothing here reads a file, a clock or the control socket: the callers
//! pass in what they found, so every rule is tested on the host with
//! `./ob test unit`. A service that starts in the wrong order is found
//! there first; Spark's own side is the `service-graph` boot test.

#![no_std]

extern crate alloc;

pub mod dry_run;
pub mod explain;
pub mod graph;
pub mod ready;
pub mod show;
pub mod trial;
pub mod words;
