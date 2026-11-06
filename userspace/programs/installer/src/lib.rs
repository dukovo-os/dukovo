// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! dukovo-install(8), Dukovo's installer (ADRs 0309 and 0310): it installs
//! the signed release on the install stick onto a disk, as its screens
//! are answered or an answers file says (install-answers(5)).
//!
//! - [`screens`] asks the questions, on the console, and shows the
//!   install's progress.
//! - [`answers`] reads and checks the answers file.
//! - [`install`] runs the steps, in order, from checking the release's
//!   signatures to saving the answers in the new system; start there.
//! - [`copy`] copies the release into the new slot, checking each file
//!   against the signed list as it is written.
//! - [`settings`] writes the new system's settings and accounts.
//! - [`plan`] works out what an install does to the disks, from the
//!   layout the answers name, before anything is written.
//! - [`disks`] finds the stick, names disks and partitions, and reads
//!   their tables.
//! - [`machine`] reads what the computer has, for the screens to offer.
//! - [`messages`] holds every message the screens show, in Fluent.
//! - [`log`] keeps the install log, `/tmp/install.log` while it runs and
//!   `/system/state/log/install.log` in the installed system: when an
//!   install fails, read it from its end.

pub mod answers;
pub mod copy;
pub mod disks;
pub mod install;
pub mod log;
pub mod machine;
pub mod messages;
pub mod plan;
pub mod screens;
pub mod settings;
