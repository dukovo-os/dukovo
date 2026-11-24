// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! linuxd: what a Linux program needs that the kernel leaves to a program
//! (ADR 0621). The program is `main.rs`; its parts are here, so that the
//! fuzz targets reach the parsers.
//!
//! `linux run` (systools) starts one linuxd beside each Linux program it
//! starts, as the person, and starts it again if it ends. linuxd finds:
//!
//! - in SLOT_LINUX, the endpoint the kernel forwards the program's calls
//!   to, with the receive right (radix-linux-call(2)): its workers answer
//!   them ([`serve`]), each through the call's own capability ([`call`]);
//! - on descriptor 3, the connection of `/devices/fuse` whose filesystem
//!   the sandbox shows at `/proc` and `/sys` ([`fs`]);
//! - on descriptor 4, that filesystem's root, where it opens the files
//!   that stand for netlink sockets ([`netlink`]).
//!
//! Run as `linuxd serve`, or `linuxd serve --again` when started again,
//! so that the filesystem takes up the connection where it was.

pub mod call;
pub mod fs;
pub mod netlink;
pub mod serve;
pub mod sys;
