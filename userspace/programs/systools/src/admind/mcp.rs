// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The web console's MCP server (ADR 0690): the console's actions for the
//! MCP clients an administrator lets in, which the pages call AI
//! clients.
//!
//! A client reads within the areas it was given, and changes nothing by
//! itself: what it asks to change waits for an administrator's yes on
//! the console's page, and runs as that administrator. The parts:
//!
//! - [`offer`] says what a client may be offered of the actions, and how
//!   far its scopes reach. The tools, the page and `admind --read`
//!   ([`read`](super::read)) all ask it, so that they cannot disagree.
//! - [`rpc`] reads and writes JSON-RPC, and [`protocol`] says what each
//!   method means in the revisions of MCP that are spoken (ADR 0692).

pub(crate) mod offer;
pub(crate) mod protocol;
pub(crate) mod rpc;
