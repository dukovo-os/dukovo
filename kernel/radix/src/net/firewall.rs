// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The firewall (ADR 0334).

use radix_net::filter::Inbound;

/// Whether to let `packet` in on interface `number` at `now_ns`.
pub fn admit(_number: usize, _packet: &Inbound, _now_ns: u64) -> bool {
    true
}
