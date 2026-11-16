// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! A timestamp, checked against the fixed root's timestamp key.

#![no_main]

use libfuzzer_sys::fuzz_target;
use update_fuzz::trusted_root;

fuzz_target!(|data: &[u8]| {
    let mut trusted = trusted_root();
    let taken = trusted.update_timestamp(data).is_ok();
    // An expired timestamp is kept for rollback checks, though it errs.
    assert!(!taken || trusted.timestamp().is_some());
});
