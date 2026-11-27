// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! What the fuzz targets start from: a fixed root with fixed keys, the
//! signed metadata that follows from it, and a canary list with one
//! canary, so that the signature checks of a target run on mutated input
//! and not only the parsers before them.
//!
//! Every key is made from a seed written here, so that the same bytes
//! come out on every run and the seed corpus in git stays signed by the
//! keys the targets trust. `cargo run --bin seeds` writes the corpus.

#![forbid(unsafe_code)]

use std::collections::BTreeMap;
use std::sync::OnceLock;

use ring::signature::{Ed25519KeyPair, KeyPair};
use serde_json::{Value, json};
use system_release::canary::{Canaries, Envelope, Report, key_id};
use tuf::keys::Key;
use tuf::repo::{self, Ed25519, Signer};
use tuf::trusted::Trusted;

/// The time every target checks metadata at, in seconds since 1970.
pub const NOW: i64 = 1_700_000_000;

/// When the fixtures' metadata expires: long after [`NOW`].
pub const EXPIRES: &str = "2099-01-01T00:00:00Z";

/// The name of the canary of [`canaries`].
pub const CANARY: &str = "fixture";

/// The seed of the key of top-level role `n`: 1 is root, 2 timestamp, 3
/// snapshot, 4 targets, and 9 the canary.
fn seed(n: u8) -> [u8; 32] {
    [n; 32]
}

fn signer(n: u8) -> Ed25519 {
    // PKCS #8, version 1, around the 32 bytes of the seed.
    let mut der = vec![
        0x30, 0x2e, 0x02, 0x01, 0x00, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x04, 0x22, 0x04,
        0x20,
    ];
    der.extend_from_slice(&seed(n));
    Ed25519::from_pkcs8(&der).expect("a seed makes a key")
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// The root the targets trust, version 1: one key for each role.
#[must_use]
pub fn root() -> Vec<u8> {
    root_of_version(1)
}

/// The root of `version`, signed by the fixed root key, as the next root
/// after [`root`] is.
#[must_use]
pub fn root_of_version(version: u64) -> Vec<u8> {
    let keys: Vec<Ed25519> = (1..=4).map(signer).collect();
    let roles: Vec<(&str, Vec<&Key>, u64)> = ["root", "timestamp", "snapshot", "targets"]
        .into_iter()
        .zip(&keys)
        .map(|(role, key)| (role, vec![key.key()], 1))
        .collect();
    let signed = repo::root(version, EXPIRES, &roles, false);
    repo::sign(&signed, &[&keys[0]]).expect("the root signs")
}

fn signed_by(n: u8, signed: &Value) -> Vec<u8> {
    repo::sign(signed, &[&signer(n)]).expect("the metadata signs")
}

/// A timestamp of version 1 that names snapshot version 1 and no hash,
/// so that a target feeds any snapshot to the signature check.
#[must_use]
pub fn timestamp() -> Vec<u8> {
    let signed = json!({
        "_type": "timestamp",
        "spec_version": repo::SPEC_VERSION,
        "version": 1,
        "expires": EXPIRES,
        "meta": { "snapshot.json": { "version": 1 } },
    });
    signed_by(2, &signed)
}

/// A snapshot of version 1 that names `targets.json` at version 1 and no
/// hash.
#[must_use]
pub fn snapshot() -> Vec<u8> {
    let signed = json!({
        "_type": "snapshot",
        "spec_version": repo::SPEC_VERSION,
        "version": 1,
        "expires": EXPIRES,
        "meta": { "targets.json": { "version": 1 } },
    });
    signed_by(3, &signed)
}

/// A targets file of version 1 that lists one file.
#[must_use]
pub fn targets() -> Vec<u8> {
    let listed = BTreeMap::from([("system/0.15.0".to_string(), repo::file_info(b"a release"))]);
    signed_by(4, &repo::targets(1, EXPIRES, &listed))
}

fn canary_pair() -> Ed25519KeyPair {
    Ed25519KeyPair::from_seed_unchecked(&seed(9)).expect("a seed makes a key")
}

/// The canary list of the fixture: [`CANARY`], with its key.
#[must_use]
pub fn canaries() -> Canaries {
    let mut list = Canaries::default();
    list.add(CANARY, &hex(canary_pair().public_key().as_ref()), NOW).expect("the canary is valid");
    list
}

/// A report of the fixture canary, running `running`, sealed as it sends
/// it.
#[must_use]
pub fn canary_report(running: &str) -> Vec<u8> {
    let pair = canary_pair();
    let report = serde_json::to_string(&Report::new(CANARY, NOW, running, "stable"))
        .expect("a report is JSON");
    let envelope = Envelope {
        signature: hex(pair.sign(report.as_bytes()).as_ref()),
        key: key_id(pair.public_key().as_ref()),
        report,
    };
    serde_json::to_vec(&envelope).expect("an envelope is JSON")
}

/// The fixed root, timestamp and snapshot, made once: a target runs
/// thousands of times a second.
fn metadata() -> &'static (Vec<u8>, Vec<u8>, Vec<u8>) {
    static METADATA: OnceLock<(Vec<u8>, Vec<u8>, Vec<u8>)> = OnceLock::new();
    METADATA.get_or_init(|| (root(), timestamp(), snapshot()))
}

/// The metadata set of an update that has the fixed root and nothing
/// more.
///
/// # Panics
///
/// Never: the fixed root is valid.
#[must_use]
pub fn trusted_root() -> Trusted {
    Trusted::new(&metadata().0, NOW).expect("the fixed root is trusted")
}

/// The set after the fixed timestamp, which takes a snapshot next.
///
/// # Panics
///
/// Never: the fixed timestamp is valid.
#[must_use]
pub fn trusted_timestamp() -> Trusted {
    let mut trusted = trusted_root();
    trusted.update_timestamp(&metadata().1).expect("the fixed timestamp is trusted");
    trusted
}

/// The set after the fixed snapshot, which takes `targets.json` next.
///
/// # Panics
///
/// Never: the fixed snapshot is valid.
#[must_use]
pub fn trusted_snapshot() -> Trusted {
    let mut trusted = trusted_timestamp();
    trusted.update_snapshot(&metadata().2, true).expect("the fixed snapshot is trusted");
    trusted
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_fixtures_are_what_the_targets_trust() {
        let mut trusted = trusted_root();
        trusted.update_root(&root_of_version(2)).unwrap();
        let mut trusted = trusted_snapshot();
        trusted.update_targets(&targets(), "targets", "root").unwrap();
        let verified = system_release::canary::verify(&canary_report("0.15.0"), &canaries(), NOW);
        assert_eq!(verified.unwrap().name, CANARY);
    }

    #[test]
    fn a_changed_fixture_is_refused() {
        let mut bytes = targets();
        let at = bytes.len() / 2;
        bytes[at] ^= 1;
        assert!(trusted_snapshot().update_targets(&bytes, "targets", "root").is_err());
    }
}
