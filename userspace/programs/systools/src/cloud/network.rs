// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The cloud's network configuration, cloud-init's version 1 or 2 (ADR
//! 0328), as netd(8)'s network(5) file: a section for each card with
//! addresses of its own, named by its MAC address when the configuration
//! gives one, and else by its name, Linux's `ethN` being Dukovo's `netN`.
//! A card the configuration sets up by DHCP alone needs no section, as
//! netd asks DHCP for every card the file does not name.

use std::fmt::Write as _;
use std::net::IpAddr;

use super::yaml::Value;

/// A card's settings, as the file's section says them.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Card {
    /// The section's name: a MAC address or a card's name.
    name: String,
    /// Its addresses, each with its prefix.
    addresses: Vec<String>,
    /// Its routers, IPv4 and IPv6, one of each at most.
    routers: Vec<String>,
    /// Its DNS servers.
    dns: Vec<String>,
    /// Whether it asks DHCP too.
    dhcp: bool,
}

/// The network(5) file for `value`, a `network-config` file read, or none
/// if every card is to ask DHCP; and what of it is left out, for the log.
pub fn network_file(value: &Value) -> Result<(Option<String>, Vec<String>), String> {
    // The configuration may be under a `network` key, as in user data.
    let value = value.get("network").unwrap_or(value);
    if value.get("config").and_then(Value::text) == Some("disabled") {
        return Ok((None, Vec::new()));
    }
    let mut left_out = Vec::new();
    let cards = match value.get("version").and_then(Value::text) {
        Some("1") => version_1(value, &mut left_out)?,
        Some("2") => version_2(value, &mut left_out)?,
        Some(other) => return Err(format!("network-config: version {other} is not 1 or 2")),
        None => return Err("network-config: no version".into()),
    };
    let cards: Vec<Card> = cards.into_iter().filter(|c| !c.addresses.is_empty()).collect();
    if cards.is_empty() {
        return Ok((None, left_out));
    }
    let mut text = String::from(
        "# Written by cloud-setup(8) from the cloud's network configuration (ADR 0328).\n\
         # A card not named here asks DHCP (network(5)).\n",
    );
    for card in &cards {
        let _ = writeln!(text, "\n[{}]", card.name);
        for address in &card.addresses {
            let _ = writeln!(text, "address = {address}");
        }
        for router in &card.routers {
            let _ = writeln!(text, "router = {router}");
        }
        for dns in card.dns.iter().take(3) {
            let _ = writeln!(text, "dns = {dns}");
        }
        if card.dhcp {
            text.push_str("dhcp = yes\n");
        }
    }
    Ok((Some(text), left_out))
}

/// The section name of a card called `name` by the configuration, with
/// the MAC address `mac` if it gives one.
fn section(name: &str, mac: Option<&str>) -> String {
    if let Some(mac) = mac {
        return mac.to_ascii_lowercase();
    }
    match name.strip_prefix("eth") {
        Some(n) if !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()) => format!("net{n}"),
        _ => name.to_string(),
    }
}

/// An address with its prefix: `address` as it is if it has one, else
/// with the prefix of `netmask`, else a whole IPv4 /24 or IPv6 /64, as
/// cloud-init takes one.
fn with_prefix(address: &str, netmask: Option<&str>) -> Result<String, String> {
    if address.contains('/') {
        return Ok(address.to_string());
    }
    let ip: IpAddr =
        address.parse().map_err(|_| format!("network-config: {address} is not an address"))?;
    let prefix = match (netmask, ip) {
        (Some(mask), _) => match mask.parse::<IpAddr>() {
            Ok(IpAddr::V4(mask)) => u32::from(mask).count_ones(),
            Ok(IpAddr::V6(mask)) => u128::from(mask).count_ones(),
            Err(_) => {
                mask.parse().map_err(|_| format!("network-config: {mask} is not a netmask"))?
            }
        },
        (None, IpAddr::V4(_)) => 24,
        (None, IpAddr::V6(_)) => 64,
    };
    Ok(format!("{address}/{prefix}"))
}

/// The cards of a version 1 configuration: its `physical` entries'
/// subnets, with its `nameserver` entries' servers on each.
fn version_1(value: &Value, left_out: &mut Vec<String>) -> Result<Vec<Card>, String> {
    let mut cards = Vec::new();
    let mut servers: Vec<String> = Vec::new();
    for item in value.get("config").map_or_else(Vec::new, Value::items) {
        let kind = item.get("type").and_then(Value::text).unwrap_or("");
        match kind {
            "physical" => {
                let name = item.get("name").and_then(Value::text).unwrap_or("");
                let mac = item.get("mac_address").and_then(Value::text);
                let mut card = Card { name: section(name, mac), ..Card::default() };
                for subnet in item.get("subnets").map_or_else(Vec::new, Value::items) {
                    match subnet.get("type").and_then(Value::text).unwrap_or("") {
                        "static" | "static6" => {
                            let address = subnet
                                .get("address")
                                .and_then(Value::text)
                                .ok_or("network-config: a static subnet with no address")?;
                            let mask = subnet.get("netmask").and_then(Value::text);
                            card.addresses.push(with_prefix(address, mask)?);
                            if let Some(gateway) = subnet.get("gateway").and_then(Value::text) {
                                card.routers.push(gateway.into());
                            }
                            let dns =
                                subnet.get("dns_nameservers").map_or_else(Vec::new, Value::texts);
                            card.dns.extend(dns.into_iter().map(str::to_string));
                        }
                        "dhcp" | "dhcp4" => card.dhcp = true,
                        // netd's cards make IPv6 addresses of their own and
                        // ask DHCPv6 as their routers say.
                        "dhcp6"
                        | "ipv6_slaac"
                        | "ipv6_dhcpv6-stateless"
                        | "ipv6_dhcpv6-stateful" => {}
                        other => left_out.push(format!("network-config: a subnet of type {other}")),
                    }
                }
                cards.push(card);
            }
            "nameserver" => {
                servers.extend(
                    item.get("address")
                        .map_or_else(Vec::new, Value::texts)
                        .into_iter()
                        .map(str::to_string),
                );
            }
            other => left_out.push(format!("network-config: an entry of type {other}")),
        }
    }
    for card in &mut cards {
        for server in &servers {
            if !card.dns.contains(server) {
                card.dns.push(server.clone());
            }
        }
    }
    Ok(cards)
}

/// The cards of a version 2 configuration, netplan's: its `ethernets`.
fn version_2(value: &Value, left_out: &mut Vec<String>) -> Result<Vec<Card>, String> {
    let mut cards = Vec::new();
    for key in ["bonds", "bridges", "vlans", "wifis"] {
        if value.get(key).is_some() {
            left_out.push(format!("network-config: {key}; ip(8) makes them by hand"));
        }
    }
    let Some(Value::Map(ethernets)) = value.get("ethernets") else { return Ok(cards) };
    for (id, card_value) in ethernets {
        let mac = card_value.get("match").and_then(|m| m.get("macaddress")).and_then(Value::text);
        let name = card_value.get("set-name").and_then(Value::text).unwrap_or(id);
        let mut card = Card { name: section(name, mac), ..Card::default() };
        for address in card_value.get("addresses").map_or_else(Vec::new, Value::texts) {
            card.addresses.push(with_prefix(address, None)?);
        }
        for key in ["gateway4", "gateway6"] {
            if let Some(gateway) = card_value.get(key).and_then(Value::text) {
                card.routers.push(gateway.into());
            }
        }
        for route in card_value.get("routes").map_or_else(Vec::new, Value::items) {
            let to = route.get("to").and_then(Value::text).unwrap_or("");
            let via = route.get("via").and_then(Value::text);
            match (to, via) {
                ("default" | "0.0.0.0/0" | "::/0", Some(via)) => {
                    let version = |a: &str| a.contains(':');
                    if !card.routers.iter().any(|r| version(r) == version(via)) {
                        card.routers.push(via.into());
                    }
                }
                _ => left_out.push(format!("network-config: the route to {to} of {id}")),
            }
        }
        if let Some(servers) = card_value.get("nameservers").and_then(|n| n.get("addresses")) {
            card.dns.extend(servers.texts().into_iter().map(str::to_string));
        }
        card.dhcp = card_value.get("dhcp4").and_then(Value::truth) == Some(true);
        cards.push(card);
    }
    Ok(cards)
}
