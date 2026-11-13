// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

use std::net::{Ipv4Addr, Ipv6Addr};

use serde_json::{Value, json};

use super::*;
use crate::ip::{Address6, Cidr, Cidr6, Interface, Ipv6, Kind, Origin};

/// A machine named `vm100` with one card, as a test's.
struct Machine;

impl Facts for Machine {
    fn host_name(&self) -> String {
        "vm100".into()
    }

    fn time_ns(&self) -> u64 {
        1_794_000_000_123_456_789
    }

    fn os(&self) -> Os {
        Os {
            kernel_release: "0.14.0".into(),
            kernel_version: "Radix 0.14.0".into(),
            machine: "x86_64".into(),
            name: "Dukovo Server".into(),
            version: "0.14.0".into(),
        }
    }

    fn interfaces(&self) -> Vec<Interface> {
        let six = |ip: &str, origin, tentative| Address6 {
            cidr: Cidr6 { ip: ip.parse::<Ipv6Addr>().unwrap(), prefix: 64 },
            origin,
            preferred: None,
            valid: None,
            tentative,
        };
        vec![Interface {
            name: "net0".into(),
            index: 2,
            mtu: 1500,
            mac: "52:54:00:12:34:56".into(),
            loopback: false,
            carrier: true,
            operstate: "up".into(),
            addresses: vec![Cidr { ip: Ipv4Addr::new(10, 0, 2, 15), prefix: 24 }],
            router: Some(Ipv4Addr::new(10, 0, 2, 2)),
            ipv6: Ipv6 {
                addresses: vec![
                    six("fe80::5054:ff:fe12:3456", Origin::LinkLocal, false),
                    // Not yet its own: duplicate address detection runs.
                    six("fec0::1", Origin::Stable, true),
                ],
                ..Ipv6::default()
            },
            kind: Kind::Card,
            master: None,
            received: [1000, 10, 0, 1],
            sent: [2000, 20, 0, 0],
        }]
    }
}

/// The answer to `text`, read back as JSON, and the end it asks for.
fn ask(text: &str) -> (Option<Value>, Option<&'static str>) {
    let (answer, end) = answer(text.as_bytes(), &Machine);
    let answer = answer.map(|bytes| {
        assert_eq!(bytes.last(), Some(&b'\n'), "an answer is a line");
        serde_json::from_slice(&bytes).expect("an answer is JSON")
    });
    (answer, end)
}

#[test]
fn requests_are_cut_out_of_the_stream_by_their_braces() {
    let mut splitter = Splitter::default();
    let request = |text: &str| Piece::Request(text.as_bytes().to_vec());
    // Two at once, then one in three reads, a brace inside a string.
    assert_eq!(
        splitter.feed(b"{\"execute\":\"guest-ping\"}\n{\"execute\": \"a\"}"),
        [request("{\"execute\":\"guest-ping\"}"), request("{\"execute\": \"a\"}")]
    );
    assert!(splitter.feed(b"  {\"execute\": \"x}\\\"\", ").is_empty());
    assert!(splitter.feed(b"\"arguments\": {\"id\": [1, 2]}").is_empty());
    assert_eq!(
        splitter.feed(b"}\r\n"),
        [request("{\"execute\": \"x}\\\"\", \"arguments\": {\"id\": [1, 2]}}")]
    );
    // 0xFF throws away a request begun, as guest-sync-delimited's clients
    // send it to begin afresh.
    assert!(splitter.feed(b"{\"execute\": \"guest-").is_empty());
    assert_eq!(
        splitter.feed(b"\xff{\"execute\":\"guest-ping\"}"),
        [request("{\"execute\":\"guest-ping\"}")]
    );
    // A stray word ends at a space, and is answered as not JSON.
    assert_eq!(splitter.feed(b"hello {}"), [request("hello "), request("{}")]);
    // A request too long is thrown away whole.
    let mut long = b"{\"execute\": \"".to_vec();
    long.extend(std::iter::repeat_n(b'a', MOST + 10));
    long.extend_from_slice(b"\"}{\"execute\":\"guest-ping\"}");
    assert_eq!(splitter.feed(&long), [Piece::TooLong, request("{\"execute\":\"guest-ping\"}")]);
}

#[test]
fn ping_info_and_sync_answer_as_qemu_ga_does() {
    assert_eq!(ask("{\"execute\": \"guest-ping\"}"), (Some(json!({ "return": {} })), None));
    // The request's id comes back with its answer.
    assert_eq!(
        ask("{\"execute\": \"guest-ping\", \"id\": \"abc\"}").0,
        Some(json!({ "return": {}, "id": "abc" }))
    );
    assert_eq!(
        ask("{\"execute\": \"guest-sync\", \"arguments\": {\"id\": 1234}}").0,
        Some(json!({ "return": 1234 }))
    );
    // The delimited one starts with 0xFF, which is not JSON.
    let (delimited, _) =
        answer(b"{\"execute\":\"guest-sync-delimited\",\"arguments\":{\"id\":7}}", &Machine);
    assert_eq!(delimited.unwrap(), b"\xff{\"return\":7}\n");
    let info = ask("{\"execute\": \"guest-info\"}").0.unwrap();
    let commands = info["return"]["supported_commands"].as_array().unwrap();
    assert_eq!(commands.len(), COMMANDS.len());
    assert!(commands.contains(
        &json!({ "name": "guest-shutdown", "enabled": true, "success-response": false })
    ));
    assert_eq!(info["return"]["version"], VERSION);
}

#[test]
fn the_machine_is_described_as_the_host_asks() {
    assert_eq!(
        ask("{\"execute\": \"guest-get-host-name\"}").0,
        Some(json!({ "return": { "host-name": "vm100" } }))
    );
    assert_eq!(
        ask("{\"execute\": \"guest-get-time\"}").0,
        Some(json!({ "return": 1_794_000_000_123_456_789_u64 }))
    );
    let os = ask("{\"execute\": \"guest-get-osinfo\"}").0.unwrap();
    assert_eq!(
        os["return"],
        json!({
            "id": "dukovo", "name": "Dukovo Server", "pretty-name": "Dukovo Server 0.14.0",
            "version": "0.14.0", "version-id": "0.14.0",
            "kernel-release": "0.14.0", "kernel-version": "Radix 0.14.0", "machine": "x86_64",
        })
    );
    let cards = ask("{\"execute\": \"guest-network-get-interfaces\"}").0.unwrap();
    assert_eq!(
        cards["return"],
        json!([{
            "name": "net0",
            "hardware-address": "52:54:00:12:34:56",
            "ip-addresses": [
                { "ip-address-type": "ipv4", "ip-address": "10.0.2.15", "prefix": 24 },
                { "ip-address-type": "ipv6", "ip-address": "fe80::5054:ff:fe12:3456", "prefix": 64 },
            ],
            "statistics": {
                "rx-bytes": 1000, "rx-packets": 10, "rx-errs": 0, "rx-dropped": 1,
                "tx-bytes": 2000, "tx-packets": 20, "tx-errs": 0, "tx-dropped": 0,
            },
        }])
    );
}

#[test]
fn shutdown_answers_nothing_and_ends_the_machine() {
    assert_eq!(ask("{\"execute\": \"guest-shutdown\"}"), (None, Some("poweroff")));
    let reboot = "{\"execute\": \"guest-shutdown\", \"arguments\": {\"mode\": \"reboot\"}}";
    assert_eq!(ask(reboot), (None, Some("reboot")));
    let (answer, end) =
        ask("{\"execute\": \"guest-shutdown\", \"arguments\": {\"mode\": \"sleep\"}}");
    assert_eq!(end, None);
    assert_eq!(answer.unwrap()["error"]["class"], "GenericError");
}

#[test]
fn mistakes_are_answered_with_errors() {
    let class = |text: &str| ask(text).0.unwrap()["error"]["class"].as_str().unwrap().to_string();
    assert_eq!(class("{\"execute\": \"guest-exec\"}"), "CommandNotFound");
    assert_eq!(class("hello "), "GenericError");
    assert_eq!(class("[1, 2]"), "GenericError");
    assert_eq!(class("{\"arguments\": {}}"), "GenericError");
    assert_eq!(class("{\"execute\": \"guest-sync\"}"), "GenericError");
    // An error keeps the id too.
    let (answer, _) = ask("{\"execute\": \"guest-nothing\", \"id\": 5}");
    assert_eq!(answer.unwrap()["id"], 5);
}

#[test]
fn a_port_named_after_the_start_is_waited_for_and_one_never_named_is_not() {
    let dir = std::env::temp_dir().join(format!("guest-agent-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let port = dir.join("org.qemu.guest_agent.0");
    let named = port.clone();
    let host = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(200));
        std::fs::write(named, b"").unwrap();
    });
    assert!(appears(&port, Duration::from_secs(5)));
    host.join().unwrap();
    let started = Instant::now();
    assert!(!appears(&dir.join("other"), Duration::from_millis(300)));
    assert!(started.elapsed() >= Duration::from_millis(300));
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn the_ports_the_host_did_name_are_said() {
    let dir = std::env::temp_dir().join(format!("guest-agent-named-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(named_ports(&dir), "it named none");
    std::fs::create_dir_all(&dir).unwrap();
    assert_eq!(named_ports(&dir), "it named none");
    std::fs::write(dir.join("org.qemu.guest_agent.1"), b"").unwrap();
    assert_eq!(named_ports(&dir), "it named org.qemu.guest_agent.1");
    for name in ["com.redhat.spice.0", "org.libguestfs.channel.0"] {
        std::fs::write(dir.join(name), b"").unwrap();
    }
    assert_eq!(
        named_ports(&dir),
        "it named com.redhat.spice.0, org.libguestfs.channel.0 and org.qemu.guest_agent.1"
    );
    std::fs::remove_dir_all(&dir).unwrap();
}
