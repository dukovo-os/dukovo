// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Says that `kani` is a cfg, for ob's build of the crate, where the
//! workspace's lints do not reach.

#![forbid(unsafe_code)]

fn main() {
    println!("cargo::rustc-check-cfg=cfg(kani)");
}
