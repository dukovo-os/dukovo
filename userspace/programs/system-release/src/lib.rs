// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Dukovo's signed system releases (ADR 0308), in the one format the
//! install stick, the installer and the updates of Phase 15 share, as
//! the updates design's section 4 has it: a release is a list of every
//! file of `/system` with its SHA-256 ([`list`]), signed as a TUF target
//! through the four top-level roles ([`repo`]), and its files are kept
//! once each under their hash ([`store`]).
//!
//! `ob` makes a release of what it stages for a slot, signs it with the
//! build machine's keys and puts it on the stick's DUKOVO_LIVE partition;
//! the live system checks it against the root it trusts before anything
//! is installed from it. When a stick will not check, start with
//! `system-release check` (system-release(8)): it names each damaged file
//! by its path, or the signature that failed.
//!
//! The same release is Rescue on the EFI system partition ([`rescue`]):
//! its startup folder, and a boot archive of the rest, checked against
//! the signed list as it is written.

pub mod archive;
pub mod cli;
pub mod list;
pub mod repo;
pub mod rescue;
pub mod store;
pub mod updates;
