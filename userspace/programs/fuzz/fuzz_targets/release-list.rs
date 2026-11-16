// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! A release list, which a machine reads from the update server's files,
//! and the list written back from what was read.

#![no_main]

use libfuzzer_sys::fuzz_target;
use system_release::list::Release;

fuzz_target!(|data: &[u8]| {
    let Ok(release) = Release::parse(data) else { return };
    let _ = release.bytes();
    assert_eq!(Release::parse(&release.to_bytes()), Ok(release));
});
