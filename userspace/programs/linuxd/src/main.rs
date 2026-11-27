// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! linuxd(8): `linuxd serve`, which `linux run` starts beside each Linux
//! program it starts (ADR 0621; the parts are in the library, `lib.rs`).

#![forbid(unsafe_code)]

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let again = match args.iter().map(String::as_str).collect::<Vec<_>>()[..] {
        ["serve"] => false,
        ["serve", "--again"] => true,
        _ => {
            eprintln!("usage: linuxd serve [--again]  (linux run starts it; see linuxd(8))");
            std::process::exit(2);
        }
    };
    std::process::exit(linuxd::serve::run(again));
}
