// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! A canary's report as the update server receives it, checked against a
//! list with the fixed canary's key.

#![no_main]

use libfuzzer_sys::fuzz_target;
use std::sync::OnceLock;
use system_release::canary::{self, Canaries};
use update_fuzz::{CANARY, NOW, canaries};

fuzz_target!(|data: &[u8]| {
    static LIST: OnceLock<Canaries> = OnceLock::new();
    let list = LIST.get_or_init(canaries);
    if let Ok(verified) = canary::verify(data, list, NOW) {
        assert_eq!(verified.name, CANARY);
        assert_eq!(verified.report.canary, CANARY);
    }
});
