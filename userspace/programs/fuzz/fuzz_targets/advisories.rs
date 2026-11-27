// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! An advisory and the advisories' index, as a machine reads them from the
//! signed targets, and as the update server reads a tree's advisory. The
//! input is each of them in turn, and an index with an advisory beside
//! it.

#![forbid(unsafe_code)]
#![no_main]

use libfuzzer_sys::fuzz_target;
use system_release::advisory::{self, Advisory, Index};

fuzz_target!(|data: &[u8]| {
    if let Ok(advisory) = Advisory::parse(data) {
        let _ = advisory::packages(&advisory);
        let _ = advisory::fixed_versions(&advisory, "dukovo");
        let _ = advisory::affects(&advisory, "dukovo", "0.15.0");
    }
    let _ = Advisory::parse_source(data, "DKSA-2026-0001.json");
    if let Ok(index) = Index::parse(data) {
        assert_eq!(Index::parse(&index.to_bytes()), Ok(index));
    }
    // The index and the advisory the input also is, as a repository
    // serves them.
    let served = [
        (advisory::INDEX_TARGET.to_string(), data.to_vec()),
        (advisory::target_path("DKSA-2026-0001"), data.to_vec()),
    ];
    let _ = advisory::check_served(&served);
});
