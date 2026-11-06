// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! system-release(8): checks a signed system release, such as the one on
//! an install stick's DUKOVO_LIVE partition, against the root this system
//! trusts (ADR 0308). The command is the system-release crate's, which
//! `ob` also runs on the build machine to sign releases; this is only its
//! place among the native tools.

use std::ffi::OsString;

use crate::args::Args;
use crate::util::{self, Fail};

pub fn main(args: &mut Args) -> Result<i32, Fail> {
    let words = args
        .rest()
        .into_iter()
        .map(OsString::into_string)
        .collect::<Result<Vec<String>, OsString>>()
        .map_err(|word| util::fail(format!("{}: not UTF-8", word.to_string_lossy())))?;
    Ok(system_release::cli::run(&words))
}
