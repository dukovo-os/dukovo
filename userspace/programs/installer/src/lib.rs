// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! dukovo-install(8), Dukovo's installer (ADR 0309): it installs the
//! signed release on the install stick onto a disk, as an answers file
//! says (install-answers(5)).
//!
//! - [`answers`] reads and checks the answers file.
//! - [`install`] runs the steps, in order, from checking the release's
//!   signatures to saving the answers in the new system; start there.
//! - [`copy`] copies the release into the new slot, checking each file
//!   against the signed list as it is written.
//! - [`settings`] writes the new system's settings and accounts.
//! - [`disks`] finds the stick and names disks and partitions.
//! - [`log`] keeps the install log, `/tmp/install.log` while it runs and
//!   `/system/state/log/install.log` in the installed system: when an
//!   install fails, read it from its end.

pub mod answers;
pub mod copy;
pub mod disks;
pub mod install;
pub mod log;
pub mod settings;
