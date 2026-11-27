// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! windowd: Dukovo's window server (windowd(1), ADR 0712).

#![forbid(unsafe_code)]

mod clients;
mod curve;
mod desktop;
mod devices;
mod keys;
mod link;
mod pointer;
mod screens;
mod shell;

#[cfg(test)]
mod tests;

fn main() {}
