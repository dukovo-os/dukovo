// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! A targets file, checked against the fixed root's targets key and the
//! fixed snapshot's version for it. Roles the fixed root does not
//! delegate are refused before their signatures are read.

#![no_main]

use libfuzzer_sys::fuzz_target;
use update_fuzz::trusted_snapshot;

fuzz_target!(|data: &[u8]| {
    let mut trusted = trusted_snapshot();
    if trusted.update_targets(data, "targets", "root").is_ok() {
        assert!(trusted.targets("targets").is_some());
    }
});
