// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! A snapshot, checked against the fixed root's snapshot key and the
//! fixed timestamp's version, whether it is a kept one or one the
//! timestamp's hashes cover.

#![no_main]

use libfuzzer_sys::fuzz_target;
use update_fuzz::trusted_timestamp;

fuzz_target!(|data: &[u8]| {
    for kept in [true, false] {
        let mut trusted = trusted_timestamp();
        if trusted.update_snapshot(data, kept).is_ok() {
            assert!(trusted.snapshot().is_some());
        }
    }
});
