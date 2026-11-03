// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Says that Radix is a target_os, for ob's build of the crate, where the
//! workspace's lints do not reach.

fn main() {
    println!("cargo::rustc-check-cfg=cfg(target_os, values(\"radix\"))");
}
