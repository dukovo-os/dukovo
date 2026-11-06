// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! system-release for the build machine, which `ob` builds and runs to
//! make keys and sign releases (system-release(8)). On Dukovo the same
//! command is one of the native tools.

use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let status = system_release::cli::run(&args);
    ExitCode::from(u8::try_from(status).unwrap_or(1))
}
