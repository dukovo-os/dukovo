// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Send and receive (ADR 0462): a read-only snapshot as a stream of
//! commands that makes it again on another filesystem, whole or as the
//! changes from a parent snapshot both sides hold, in the format of
//! Linux's `btrfs send`, version 1, so that either side can be Linux.
//!
//! - [`stream`]: the commands, written and read.
//! - [`sender`]: the commands that make a snapshot, from the items its
//!   tree and its parent's differ by ([`crate::compare`]).
//! - [`receive`]: a stream's commands applied, its paths checked first.
//! - `items`: the items of a subvolume's tree a send reads.

pub mod items;
pub mod receive;
pub mod sender;
pub mod stream;
