// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The `systools` program: every tool of the library of that name as one
//! binary (ADR 0096). The library has the tools, and the code that picks
//! the one the program was called as.

fn main() -> std::process::ExitCode {
    systools::main()
}
