// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Writes the seed corpus of each fuzz target into `corpus/<target>/`
//! from the fixtures of the crates' unit tests: `cargo run --bin seeds`
//! in this folder. The corpus is kept in git, and `ob test fuzz` copies
//! it into the cache the targets grow it in.

use std::path::Path;

use serde_json::json;
use slots::state::{Slot, State};
use slots::trial::Trial;
use system_release::list::{Entry, FORMAT, Kind, Release};
use update_fuzz::{canary_report, root, root_of_version, snapshot, targets, timestamp};

/// The example advisory the tests of system-release read.
const ADVISORY: &str = include_str!("../../../system-release/tests/fixtures/DKSA-2026-0001.json");

fn put(target: &str, name: &str, bytes: &[u8]) {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("corpus").join(target);
    std::fs::create_dir_all(&dir).expect("the corpus folder is made");
    std::fs::write(dir.join(name), bytes).expect("the seed is written");
}

fn entry(path: &str, kind: Kind, mode: &str) -> Entry {
    Entry {
        path: path.into(),
        kind,
        mode: mode.into(),
        size: (kind == Kind::File).then_some(9),
        sha256: (kind == Kind::File).then(|| "ab".repeat(32)),
        target: (kind == Kind::Link).then(|| "bin/dukovo".into()),
    }
}

/// A netlink request datagram: the header of `kind` and `payload`.
fn netlink_request(kind: u16, flags: u16, seq: u32, payload: &[u8]) -> Vec<u8> {
    let length = u32::try_from(16 + payload.len()).expect("the request is short");
    let mut bytes = length.to_ne_bytes().to_vec();
    bytes.extend_from_slice(&kind.to_ne_bytes());
    bytes.extend_from_slice(&flags.to_ne_bytes());
    bytes.extend_from_slice(&seq.to_ne_bytes());
    bytes.extend_from_slice(&0u32.to_ne_bytes());
    bytes.extend_from_slice(payload);
    bytes
}

fn release(version: &str, files: Vec<Entry>) -> Release {
    Release {
        format: FORMAT,
        version: version.into(),
        commit: "0123456789abcdef0123456789abcdef01234567".into(),
        toolchain: "nightly-2026-10-15".into(),
        built: "2026-11-06T00:00:00Z".into(),
        updates_from: "0.14.0".into(),
        files,
    }
}

fn main() {
    put("release-list", "empty", &release("0.15.0", Vec::new()).to_bytes());
    let files = vec![
        entry("bin", Kind::Directory, "0755"),
        entry("bin/dukovo", Kind::File, "0755"),
        entry("bin/dk", Kind::Link, "0777"),
    ];
    put("release-list", "tree", &release("0.15.1", files).to_bytes());
    put("release-list", "newer-format", br#"{"format": 2, "version": "1.0.0"}"#);

    put("tuf-root", "root", &root());
    put("tuf-root", "next-root", &root_of_version(2));
    put("tuf-timestamp", "timestamp", &timestamp());
    put("tuf-snapshot", "snapshot", &snapshot());
    put("tuf-targets", "targets", &targets());

    let withdrawal = json!({ "release": "0.15.0", "reason": "a flaw", "advice": "stay" });
    put("rollout", "withdrawal", withdrawal.to_string().as_bytes());
    let plans = json!({
        "releases": { "0.15.0": { "released": 1_700_000_000, "open": 1 } },
        "red": ["fixture"],
    });
    put("rollout", "plans", plans.to_string().as_bytes());
    put("rollout", "root", &root());

    put("advisories", "advisory", ADVISORY.as_bytes());
    let index = json!({ "advisories": [{
        "id": "DKSA-2026-0001",
        "modified": "2026-11-15T09:30:00Z",
        "severity": "high",
        "summary": "Example",
        "sha256": "cd".repeat(32),
    }] });
    put("advisories", "index", index.to_string().as_bytes());

    put("canary-report", "report", &canary_report("0.15.0"));

    let installed = State::installed();
    put("slot-state", "installed", &installed.encode());
    let staged = installed.stage(Slot::A).expect("the default stages").next();
    put("slot-state", "staged", &staged.encode());
    let trial = Trial::new(1, Slot::B, "quiet console=ttyS0,115200n8").expect("a try is valid");
    put("slot-state", "trial", &trial.encode());

    // RTM_GETLINK, RTM_GETADDR and RTM_GETROUTE as the C library's
    // getifaddrs and Go's NetlinkRIB send them, which differ in the
    // padding of the body; then one interface by name and by number, and
    // a request to change an address, which is refused.
    let dump = 0x301;
    let link = netlink_request(18, dump, 1, &[0; 4]);
    let addr = netlink_request(22, dump, 2, &[0; 4]);
    put("linuxd-netlink", "libc-links", &link);
    put("linuxd-netlink", "go-routes", &netlink_request(26, dump, 1, &[2]));
    put("linuxd-netlink", "libc-links-then-addresses", &[link, addr].concat());
    let mut by_name = vec![0; 16];
    by_name.extend_from_slice(&[7, 0, 3, 0, b'n', b'e', b't', 0]);
    put("linuxd-netlink", "link-by-name", &netlink_request(18, 5, 3, &by_name));
    let mut by_number = vec![0; 16];
    by_number[4..8].copy_from_slice(&2i32.to_ne_bytes());
    put("linuxd-netlink", "link-by-number", &netlink_request(18, 1, 4, &by_number));
    put(
        "linuxd-netlink",
        "new-address",
        &netlink_request(20, 0x105, 5, &[2, 24, 0, 0, 2, 0, 0, 0]),
    );
}
