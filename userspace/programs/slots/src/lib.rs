// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The two system slots of an installed Dukovo (ADRs 0451 to 0454).
//!
//! The system runs read-only from one slot, `@system.a` or `@system.b`,
//! subvolumes of the Dukovo root. An update builds the other slot aside,
//! checks it and seals it, then stages it in the slot state ([`state`]),
//! which GRUB reads and writes on the EFI system partitions: the new slot
//! gets three tries, each counted before it starts, and the version
//! before comes back by itself unless the new one passes its health
//! check. GRUB tells the kernel which slot it started, and whether that
//! is a fallback, on the command line ([`cmdline`]); each slot says what
//! version it is in `startup/slot.env` ([`about`]); and a fallback puts
//! the settings from before the update back ([`restore`]). When no slot
//! starts, three starts in a row that were not seen through start Rescue
//! from the EFI system partition, which counts them ([`rescue`]). A
//! whole machine restored from its backups is put in place at a start
//! the same way ([`machine`]).
//!
//! The crate is without the standard library, so that Spark uses it as
//! sysupdate(8) does. When an update will not stage or a start falls
//! back, `sysupdate status` shows the state both copies hold.

#![no_std]

extern crate alloc;

pub mod about;
pub mod cmdline;
pub mod envblock;
pub mod machine;
pub mod rescue;
pub mod restore;
pub mod state;
pub mod version;

pub use state::{Slot, State};
