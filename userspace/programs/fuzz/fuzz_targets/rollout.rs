// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! What the update server's rollouts read: a withdrawal, the plans it
//! keeps, and a root that the rollout role is delegated from, and the
//! waves the schedule opens at times and for numbers taken from the
//! input's first bytes.

#![no_main]

use libfuzzer_sys::fuzz_target;
use system_release::rollout::{self, Plan, Plans, Withdrawal};

/// The `n`th eight bytes of `data` as a number, or 0 where it is shorter.
fn number(data: &[u8], n: usize) -> i64 {
    data.get(n * 8..n * 8 + 8)
        .and_then(|bytes| <[u8; 8]>::try_from(bytes).ok())
        .map_or(0, i64::from_le_bytes)
}

fuzz_target!(|data: &[u8]| {
    if let Ok(withdrawal) = Withdrawal::parse(data) {
        assert_eq!(Withdrawal::parse(&withdrawal.to_bytes()), Ok(withdrawal));
    }
    if let Ok(plans) = serde_json::from_slice::<Plans>(data) {
        let kept = serde_json::to_vec(&plans).expect("plans serialise");
        assert_eq!(serde_json::from_slice::<Plans>(&kept).ok(), Some(plans.clone()));
        for (release, plan) in &plans.releases {
            let next = rollout::next(release, plan, &plans.red, number(data, 1));
            assert!(next.wave <= rollout::LAST);
        }
    }
    let _ = rollout::delegation(data);

    let (released, now) = (number(data, 0), number(data, 1));
    let open = u8::try_from(number(data, 2).rem_euclid(256)).unwrap_or(0);
    let plan = Plan { released, open, paused: None };
    let next = rollout::next("0.15.0", &plan, &[], now);
    assert!(next.wave <= rollout::LAST);
    let machine = u16::try_from(number(data, 3).rem_euclid(1000)).unwrap_or(0);
    let _ = rollout::reaches(&next, machine, false);
    assert!(rollout::scheduled(released, now) <= rollout::LAST);
});
