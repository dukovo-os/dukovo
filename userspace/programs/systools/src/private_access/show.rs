// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! `wireguard show`: the interface and its devices, as a list or as JSON,
//! which the web console's Security page reads.
//!
//! What the file says (its settings, and each device's name and key and
//! addresses) comes from `wg0.conf`; what only wgd knows (where a device
//! came from, its latest handshake, the bytes it sent and received) from
//! wgd's answer for the interface. Where wgd does not answer, those are
//! null and the interface is not `up`, so that a reader never takes a
//! made-up number for one the machine gave. The report holds public keys
//! and addresses only: the server's private key is turned into its public
//! one before it leaves the file.
//!
//! When a device is missing a number it should have, `wg show wg0` says
//! what wgd says of it, and `service status wgd` whether wgd runs.

use serde::Serialize;

use super::file::{self, File};
use super::{INTERFACE, path, read_file, reading};
use crate::util::{self, Fail, fail};
use crate::wireguard;

/// What `wireguard show --json` prints, in its fields' order.
#[derive(Debug, Serialize)]
pub(super) struct Report {
    /// The layout's number, 1.
    format: u32,
    /// Empty when private access is not set up, else the one interface.
    interfaces: Vec<Interface>,
}

/// The interface, `wg0`.
#[derive(Debug, Serialize)]
struct Interface {
    name: &'static str,
    /// Its address and length, as `10.8.0.1/24`.
    address: Option<String>,
    /// Its `ListenPort`.
    port: Option<u16>,
    /// What devices are told to reach.
    endpoint: Option<String>,
    /// The key devices are told the server has, in base64.
    public_key: Option<String>,
    /// Whether wgd has the interface and answers for it.
    up: bool,
    devices: Vec<Device>,
}

/// A device: a peer of the interface.
#[derive(Debug, Serialize)]
struct Device {
    name: String,
    public_key: String,
    /// Its `AllowedIPs`, as `10.8.0.2/32`.
    addresses: Vec<String>,
    /// Where it came from last; null if nowhere, or wgd does not answer.
    endpoint: Option<String>,
    /// Its latest handshake, in Unix milliseconds; null if it has had
    /// none, or wgd does not answer.
    latest_handshake_ms: Option<u64>,
    /// The bytes received from it since wgd started; null where wgd does
    /// not answer.
    received: Option<u64>,
    /// The bytes sent to it.
    sent: Option<u64>,
}

/// Unix milliseconds of a handshake wgd gives as seconds and nanoseconds
/// of the epoch, none for 0: no handshake.
fn handshake_ms((seconds, nanos): (u64, u32)) -> Option<u64> {
    (seconds != 0 || nanos != 0)
        .then(|| seconds.saturating_mul(1000).saturating_add(u64::from(nanos) / 1_000_000))
}

/// `device` as the report says it, with what wgd (`live`) says of it.
fn device(device: &file::Device, live: Option<&wireguard::Device>) -> Device {
    let seen = live.and_then(|live| live.peers.iter().find(|peer| peer.public_key == device.key));
    Device {
        name: device.name.clone(),
        public_key: device.key.base64(),
        addresses: device.addresses.iter().map(ToString::to_string).collect(),
        endpoint: seen.and_then(|peer| peer.endpoint).map(wireguard::endpoint_text),
        latest_handshake_ms: seen.and_then(|peer| handshake_ms(peer.last_handshake)),
        received: seen.map(|peer| peer.rx_bytes),
        sent: seen.map(|peer| peer.tx_bytes),
    }
}

/// The report of `file`, which is none when there is no `wg0.conf`, and of
/// what wgd says, which is none when it does not answer.
pub(super) fn report(file: Option<&File>, live: Option<&wireguard::Device>) -> Report {
    let interfaces = file
        .map(|file| Interface {
            name: INTERFACE,
            address: file.address().map(|address| address.to_string()),
            port: file.config.listen_port,
            endpoint: file.endpoint.clone(),
            public_key: file.config.private_key.map(|key| key.public().base64()),
            up: live.is_some(),
            devices: file.devices.iter().map(|each| device(each, live)).collect(),
        })
        .into_iter()
        .collect();
    Report { format: 1, interfaces }
}

/// How long ago `seconds` is, in its largest whole unit.
fn ago(seconds: u64) -> String {
    let (count, unit) = match seconds {
        0..=59 => (seconds, "second"),
        60..=3599 => (seconds / 60, "minute"),
        3600..=86399 => (seconds / 3600, "hour"),
        _ => (seconds / 86400, "day"),
    };
    format!("{count} {unit}{} ago", if count == 1 { "" } else { "s" })
}

/// `rows` as lines with their columns aligned, two spaces apart.
fn table(rows: &[Vec<String>]) -> String {
    let columns = rows.iter().map(Vec::len).max().unwrap_or(0);
    let widths: Vec<usize> = (0..columns)
        .map(|at| rows.iter().filter_map(|row| row.get(at)).map(String::len).max().unwrap_or(0))
        .collect();
    let mut shown = String::new();
    for row in rows {
        let line: Vec<String> =
            row.iter().zip(&widths).map(|(cell, width)| format!("{cell:<width$}")).collect();
        shown += line.join("  ").trim_end();
        shown.push('\n');
    }
    shown
}

/// `report` as the list a person reads; `now_ms` is when it is, for how
/// long ago a handshake was.
pub(super) fn text(report: &Report, now_ms: u64) -> String {
    let Some(interface) = report.interfaces.first() else {
        return "Private access is not set up: `wireguard serve` sets it up.\n".into();
    };
    let or_dash = |text: Option<String>| text.unwrap_or_else(|| "-".into());
    let state = if interface.up { "up" } else { "not up" };
    let rows = vec![
        vec!["interface".into(), format!("{}, {state}", interface.name)],
        vec!["address".into(), or_dash(interface.address.clone())],
        vec!["port".into(), or_dash(interface.port.map(|port| port.to_string()))],
        vec!["endpoint".into(), or_dash(interface.endpoint.clone())],
        vec!["public key".into(), or_dash(interface.public_key.clone())],
        vec!["devices".into(), interface.devices.len().to_string()],
    ];
    let mut shown = table(&rows);
    if interface.devices.is_empty() {
        return shown;
    }
    let never = if interface.up { "never" } else { "-" };
    let mut rows = vec![
        ["NAME", "ADDRESSES", "FROM", "LATEST HANDSHAKE", "RECEIVED", "SENT"]
            .map(String::from)
            .to_vec(),
    ];
    for each in &interface.devices {
        let counted =
            |bytes: Option<u64>| bytes.map_or_else(|| "-".into(), |n| util::human(n, false));
        rows.push(vec![
            each.name.clone(),
            each.addresses.join(","),
            each.endpoint.clone().unwrap_or_else(|| "-".into()),
            each.latest_handshake_ms
                .map_or_else(|| never.into(), |ms| ago(now_ms.saturating_sub(ms) / 1000)),
            counted(each.received),
            counted(each.sent),
        ]);
    }
    shown.push('\n');
    shown += &table(&rows);
    shown
}

/// What wgd says of the interface: none when it does not answer, or says
/// something this cannot read.
fn live() -> Option<wireguard::Device> {
    wireguard::parse_device(&wireguard::ask(INTERFACE, "get=1\n").ok()?).ok()
}

/// `wireguard show [--json]`.
pub(super) fn run(json: bool) -> Result<i32, Fail> {
    let path = path();
    let file = read_file(&path)?.map(|text| reading(&path, &text)).transpose()?;
    let live = file.as_ref().and_then(|_| live());
    let report = report(file.as_ref(), live.as_ref());
    if json {
        let line = serde_json::to_string(&report).map_err(|error| fail(error.to_string()))?;
        println!("{line}");
    } else {
        print!("{}", text(&report, util::now_ns() / 1_000_000));
    }
    Ok(0)
}

#[cfg(test)]
mod tests {
    use std::net::SocketAddr;

    use serde_json::{Value, json};

    use super::*;
    use crate::wireguard::Key;

    /// RFC 7748's Alice, whose public key is worked out apart from this
    /// code, as the server's key.
    const ALICE: &str = "77076d0a7318a57d3c16c17251b26645df4c2f87ebc0992ab177fba51db92c2a";
    const ALICE_PUBLIC: &str = "hSDwCYkwp1R0i33ctD73Wg2/Og0mOBr066SpjqqbTmo=";
    const PHONE: &str = "aJ7TlWZX/XND9O54UTiZPjByNwglHgoWcuhjgYX+tFk=";
    const LAPTOP: &str = "yIfVfM4ezrB6rRvCftHpkKu04B8/UvfZa4lyBz++N3s=";

    /// The file of the machine the tests build: a named device and one
    /// with no name.
    fn config() -> String {
        format!(
            "[Interface]\n# endpoint: home.example.org\nAddress = 10.8.0.1/24\n\
             ListenPort = 51820\nPrivateKey = {}\n\n# name: phone\n[Peer]\nPublicKey = {PHONE}\n\
             AllowedIPs = 10.8.0.2/32\n\n[Peer]\nPublicKey = {LAPTOP}\nAllowedIPs = 10.8.0.3/32\n",
            Key::from_hex(ALICE).unwrap().base64()
        )
    }

    /// wgd's answer: the phone has had a handshake, the laptop has not.
    fn wgd() -> wireguard::Device {
        let phone = wireguard::Peer {
            public_key: Key::from_base64(PHONE).unwrap(),
            preshared_key: None,
            endpoint: Some("198.51.100.7:40122".parse::<SocketAddr>().unwrap()),
            allowed_ips: Vec::new(),
            last_handshake: (1_763_120_000, 250_000_000),
            rx_bytes: 1234,
            tx_bytes: 5678,
            keepalive: 25,
        };
        let laptop = wireguard::Peer {
            public_key: Key::from_base64(LAPTOP).unwrap(),
            endpoint: None,
            last_handshake: (0, 0),
            rx_bytes: 0,
            tx_bytes: 0,
            ..phone.clone()
        };
        wireguard::Device {
            private_key: None,
            listen_port: 51820,
            fwmark: 0,
            peers: vec![phone, laptop],
        }
    }

    fn json_of(config: &str, live: Option<&wireguard::Device>) -> Value {
        let file = file::read(config).unwrap();
        serde_json::to_value(report(Some(&file), live)).unwrap()
    }

    #[test]
    fn the_report_is_the_golden_object_for_the_machine_the_tests_build() {
        let shown = json_of(&config(), Some(&wgd()));
        let golden: Value = serde_json::from_str(include_str!("show.golden.json")).unwrap();
        assert_eq!(shown, golden);
    }

    #[test]
    fn the_fields_come_in_the_order_the_man_page_gives() {
        let line =
            serde_json::to_string(&report(Some(&file::read(&config()).unwrap()), Some(&wgd())))
                .unwrap();
        let order = |names: &[&str]| -> Vec<usize> {
            names.iter().map(|name| line.find(&format!("\"{name}\":")).unwrap()).collect()
        };
        let top = order(&["format", "interfaces"]);
        let interface =
            order(&["name", "address", "port", "endpoint", "public_key", "up", "devices"]);
        let device = order(&["addresses", "latest_handshake_ms", "received", "sent"]);
        for found in [top, interface, device] {
            assert!(found.windows(2).all(|pair| pair[0] < pair[1]), "{line}");
        }
        assert!(!line.contains('\n'));
    }

    #[test]
    fn the_servers_key_is_given_as_its_public_one_and_no_private_key_is_shown() {
        let shown = json_of(&config(), Some(&wgd()));
        assert_eq!(shown["interfaces"][0]["public_key"], json!(ALICE_PUBLIC));
        let private = Key::from_hex(ALICE).unwrap().base64();
        let all =
            shown.to_string() + &text(&report(Some(&file::read(&config()).unwrap()), None), 0);
        assert!(!all.contains(&private), "{all}");
    }

    #[test]
    fn where_wgd_does_not_answer_what_only_it_knows_is_null() {
        let shown = json_of(&config(), None);
        let interface = &shown["interfaces"][0];
        assert_eq!(interface["up"], json!(false));
        assert_eq!(interface["port"], json!(51820));
        for device in interface["devices"].as_array().unwrap() {
            for field in ["endpoint", "latest_handshake_ms", "received", "sent"] {
                assert_eq!(device[field], Value::Null, "{field}");
            }
            assert!(device["public_key"].is_string());
        }
    }

    #[test]
    fn a_device_that_has_had_no_handshake_has_null_for_it_and_numbers_for_the_rest() {
        let shown = json_of(&config(), Some(&wgd()));
        let laptop = &shown["interfaces"][0]["devices"][1];
        assert_eq!(laptop["name"], json!("yIfVfM4e"));
        assert_eq!(laptop["latest_handshake_ms"], Value::Null);
        assert_eq!(laptop["endpoint"], Value::Null);
        assert_eq!((&laptop["received"], &laptop["sent"]), (&json!(0), &json!(0)));
        let phone = &shown["interfaces"][0]["devices"][0];
        assert_eq!(phone["latest_handshake_ms"], json!(1_763_120_000_250_u64));
    }

    #[test]
    fn a_device_that_wgd_does_not_have_yet_is_listed_without_its_numbers() {
        let mut live = wgd();
        live.peers.truncate(1);
        let shown = json_of(&config(), Some(&live));
        let laptop = &shown["interfaces"][0]["devices"][1];
        assert_eq!(shown["interfaces"][0]["up"], json!(true));
        assert_eq!((&laptop["received"], &laptop["sent"]), (&Value::Null, &Value::Null));
    }

    #[test]
    fn with_no_file_there_is_no_interface() {
        let shown = serde_json::to_value(report(None, None)).unwrap();
        assert_eq!(shown, json!({"format": 1, "interfaces": []}));
    }

    #[test]
    fn a_file_with_only_what_it_must_have_leaves_the_rest_null() {
        let shown = json_of("[Interface]\n", None);
        let interface = &shown["interfaces"][0];
        for field in ["address", "port", "endpoint", "public_key"] {
            assert_eq!(interface[field], Value::Null, "{field}");
        }
        assert_eq!(interface["devices"], json!([]));
    }

    #[test]
    fn a_handshake_of_the_epoch_is_none() {
        assert_eq!(handshake_ms((0, 0)), None);
        assert_eq!(handshake_ms((0, 5_000_000)), Some(5));
        assert_eq!(handshake_ms((u64::MAX, 0)), Some(u64::MAX));
    }

    #[test]
    fn how_long_ago_is_in_the_largest_whole_unit() {
        for (seconds, said) in [
            (0, "0 seconds ago"),
            (1, "1 second ago"),
            (59, "59 seconds ago"),
            (60, "1 minute ago"),
            (3599, "59 minutes ago"),
            (3600, "1 hour ago"),
            (86399, "23 hours ago"),
            (86400, "1 day ago"),
            (200_000, "2 days ago"),
        ] {
            assert_eq!(ago(seconds), said);
        }
    }

    #[test]
    fn the_list_says_what_there_is() {
        let file = file::read(&config()).unwrap();
        let now_ms = 1_763_120_000_250 + 180_000;
        assert_eq!(
            text(&report(Some(&file), Some(&wgd())), now_ms),
            format!(
                "interface   wg0, up\naddress     10.8.0.1/24\nport        51820\n\
                 endpoint    home.example.org\npublic key  {ALICE_PUBLIC}\ndevices     2\n\n\
                 NAME      ADDRESSES    FROM                LATEST HANDSHAKE  RECEIVED  SENT\n\
                 phone     10.8.0.2/32  198.51.100.7:40122  3 minutes ago     1.3K      5.6K\n\
                 yIfVfM4e  10.8.0.3/32  -                   never             0         0\n"
            )
        );
    }

    #[test]
    fn the_list_has_dashes_where_wgd_does_not_answer_and_a_word_for_no_interface() {
        let file = file::read(&config()).unwrap();
        let shown = text(&report(Some(&file), None), 0);
        assert!(shown.starts_with("interface   wg0, not up\n"), "{shown}");
        assert!(
            shown.contains(
                "NAME      ADDRESSES    FROM  LATEST HANDSHAKE  RECEIVED  SENT\n\
                 phone     10.8.0.2/32  -     -                 -         -\n\
                 yIfVfM4e  10.8.0.3/32  -     -                 -         -\n"
            ),
            "{shown}"
        );
        assert_eq!(
            text(&report(None, None), 0),
            "Private access is not set up: `wireguard serve` sets it up.\n"
        );
        let bare = text(&report(Some(&file::read("[Interface]\n").unwrap()), None), 0);
        assert!(bare.ends_with("devices     0\n"), "{bare}");
    }
}
