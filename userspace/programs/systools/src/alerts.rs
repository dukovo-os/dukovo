// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Alerts (ADR 0485): what the system's services and root's scripts raise
//! when something needs the owner, and alertd(8), which keeps which are
//! active and tells the owner of each change once, as a notice on the
//! screen and at sign-in, and by email and webhooks once alerts.toml(5)
//! names them.
//!
//! - [`protocol`] is what goes over alertd's socket, a line of JSON each
//!   way;
//! - [`client`] is how a service raises, clears and sets its alerts;
//! - [`command`] is `alerts(8)`, which shows them and raises one from a
//!   script;
//! - [`channels`] sets up the email and webhook channels in alerts.toml(5)
//!   and says which kinds of alert each is told of, and [`browsers`] keeps
//!   the browsers that get alerts by Web Push, for `alerts(8)`.

mod browsers;
mod channels;
pub mod client;
pub mod command;
pub mod protocol;
