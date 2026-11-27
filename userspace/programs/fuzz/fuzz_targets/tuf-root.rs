// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! A root as the root this computer trusts, and as the next root of the
//! fixed one.

#![forbid(unsafe_code)]
#![no_main]

use libfuzzer_sys::fuzz_target;
use tuf::trusted::Trusted;
use update_fuzz::{NOW, trusted_root};

fuzz_target!(|data: &[u8]| {
    let _ = Trusted::new(data, NOW);
    let _ = Trusted::offline(data);
    let mut trusted = trusted_root();
    if trusted.update_root(data).is_ok() {
        assert_eq!(trusted.root().version, 2, "a root that is taken follows the fixed one");
    }
});
