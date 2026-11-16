// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The text of `wg0.conf` as wireguard(8) reads and edits it: the two
//! remarks it keeps in the file, where each device's lines are, and what
//! `serve` and `remove` make of the text.
//!
//! The file is wg-quick's (wireguard.conf(5)), and `wireguard::parse_config`
//! passes over its remarks, so the two that mean something are read here
//! from the lines: `# endpoint:` in the `[Interface]` section, and `# name:`
//! on the line before a `[Peer]`. Every edit is a function of the text, and
//! leaves every other line, remark and blank line as it was, byte for byte.
//! A device owns its `[Peer]` and the lines after it, its remark, and the
//! one blank line above them, so that lines added for a device and then
//! removed leave the file as it was. [`read`] is the one place that finds
//! the devices, which `remove` and `add` share.
//!
//! When a device is not found by the name `wireguard show` gives it, look at
//! its `# name:` line: it must be the line right before its `[Peer]`.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::ops::Range;

use crate::wireguard::{Config, Key, Prefix, parse_config};

/// The remark that says what devices are told to reach.
const ENDPOINT: &str = "endpoint";

/// The remark that names the device whose `[Peer]` follows.
const NAME: &str = "name";

/// How many characters of its key name a device that has no remark.
const KEY_NAME: usize = 8;

/// Which section a header opens.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Section {
    Interface,
    Peer,
    Other,
}

/// A section's header line, and where its own lines start.
struct Header {
    /// The header's line, counted from 0.
    at: usize,
    section: Section,
    /// The first line that is the section's own: the remark before a
    /// `[Peer]` when it has one, else the header.
    start: usize,
}

/// The lines of `text`, each with what ends it.
fn lines(text: &str) -> Vec<&str> {
    text.split_inclusive('\n').collect()
}

/// The section `line` opens, if it is a header.
fn section_of(line: &str) -> Option<Section> {
    let code = line.split('#').next().unwrap_or_default().trim();
    if !code.starts_with('[') {
        return None;
    }
    Some(match code.to_ascii_lowercase().as_str() {
        "[interface]" => Section::Interface,
        "[peer]" => Section::Peer,
        _ => Section::Other,
    })
}

/// What the remark `# key: value` says, if `line` is one with a value.
fn remark<'a>(line: &'a str, key: &str) -> Option<&'a str> {
    let (word, value) = line.trim().strip_prefix('#')?.split_once(':')?;
    if !word.trim().eq_ignore_ascii_case(key) {
        return None;
    }
    let value = value.trim();
    (!value.is_empty()).then_some(value)
}

/// The key and the value of a `Key = value` line, with a remark after the
/// value left off and the key in lower case.
fn setting(line: &str) -> Option<(String, &str)> {
    let code = line.split('#').next()?.trim();
    let (key, value) = code.split_once('=')?;
    Some((key.trim().to_ascii_lowercase(), value.trim()))
}

/// The headers of `all`, in order.
fn headers(all: &[&str]) -> Vec<Header> {
    all.iter()
        .enumerate()
        .filter_map(|(at, line)| {
            let section = section_of(line)?;
            let named = section == Section::Peer
                && at.checked_sub(1).is_some_and(|before| remark(all[before], NAME).is_some());
            Some(Header { at, section, start: if named { at - 1 } else { at } })
        })
        .collect()
}

/// The first line a section that starts at `start` owns: the blank line
/// above it comes with it.
fn owned_from(all: &[&str], start: usize) -> usize {
    match start.checked_sub(1) {
        Some(before) if all[before].trim().is_empty() => before,
        _ => start,
    }
}

/// A device of the interface: a `[Peer]` and what is its own.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct Device {
    /// What `wireguard show` calls it: its remark's name, or the start of
    /// its key.
    pub name: String,
    /// Its public key.
    pub key: Key,
    /// Its `AllowedIPs`: the addresses it has on the tunnel.
    pub addresses: Vec<Prefix>,
    /// Its lines, counted from 0: the remark, the `[Peer]`, its settings,
    /// and the blank line above.
    pub lines: Range<usize>,
}

/// What `wg0.conf` says.
#[derive(Debug)]
pub(super) struct File {
    /// The settings and peers `wgd` reads.
    pub config: Config,
    /// The `# endpoint:` remark of the `[Interface]` section.
    pub endpoint: Option<String>,
    /// The devices, in the order of their `[Peer]`s.
    pub devices: Vec<Device>,
}

impl File {
    /// The devices that `name` names: none, one, or, when the file was
    /// edited by hand, more.
    pub fn called(&self, name: &str) -> Vec<&Device> {
        self.devices.iter().filter(|device| device.name == name).collect()
    }

    /// The server's address on the tunnel, which the network of the
    /// devices' addresses is taken from: the first IPv4 one, or the first.
    pub fn address(&self) -> Option<Prefix> {
        let addresses = &self.config.addresses;
        addresses.iter().find(|address| address.ip.is_ipv4()).or_else(|| addresses.first()).copied()
    }
}

/// Reads the file `text`.
///
/// # Errors
///
/// What `parse_config` says is wrong with it, and on which line.
pub(super) fn read(text: &str) -> Result<File, String> {
    let config = parse_config(text, &mut |_| {})?;
    let all = lines(text);
    let headers = headers(&all);
    let mut endpoint = None;
    let mut placed = Vec::new();
    for (at, header) in headers.iter().enumerate() {
        let next = headers.get(at + 1);
        match header.section {
            Section::Interface => {
                let end = next.map_or(all.len(), |next| next.start);
                endpoint = endpoint.or_else(|| {
                    all[header.at + 1..end]
                        .iter()
                        .find_map(|line| remark(line, ENDPOINT))
                        .map(str::to_string)
                });
            }
            Section::Peer => {
                let name = all.get(header.start).filter(|_| header.start < header.at);
                let from = owned_from(&all, header.start);
                let to = next.map_or(all.len(), |next| owned_from(&all, next.start));
                placed.push((name.and_then(|line| remark(line, NAME)), from..to));
            }
            Section::Other => {}
        }
    }
    let devices = placed
        .into_iter()
        .zip(&config.peers)
        .map(|((name, lines), peer)| Device {
            name: name.map_or_else(
                || peer.public_key.base64().chars().take(KEY_NAME).collect(),
                str::to_string,
            ),
            key: peer.public_key,
            addresses: peer.allowed_ips.clone(),
            lines,
        })
        .collect();
    Ok(File { config, endpoint, devices })
}

/// `text` without the lines in `place`, which is a device's.
pub(super) fn without(text: &str, place: &Range<usize>) -> String {
    lines(text)
        .into_iter()
        .enumerate()
        .filter(|(at, _)| !place.contains(at))
        .map(|(_, line)| line)
        .collect()
}

/// How many addresses of the network [`free_address`] looks at, at most:
/// a network of more is not filled one address at a time.
const SEARCH_MOST: u128 = 1 << 16;

/// `ip` alone, as AllowedIPs and Address give one address: a /32 or a
/// /128.
pub(super) fn host(ip: IpAddr) -> Prefix {
    Prefix { ip, len: if ip.is_ipv4() { 32 } else { 128 } }
}

/// The first address of the network of `server`, the server's address on
/// the tunnel, that `file` gives no one: not the network's own, nor an
/// IPv4 network's last, which is its broadcast; not one of the server's;
/// and in no device's addresses. None when the first [`SEARCH_MOST`] are
/// all taken.
pub(super) fn free_address(file: &File, server: Prefix) -> Option<IpAddr> {
    let network = server.network();
    let (first, bits) = match network.ip {
        IpAddr::V4(ip) => (u128::from(u32::from(ip)), 32 - u32::from(network.len)),
        IpAddr::V6(ip) => (u128::from(ip), 128 - u32::from(network.len)),
    };
    let size = 1u128.checked_shl(bits).unwrap_or(u128::MAX);
    // The last address of an IPv4 network is its broadcast.
    let end = if network.ip.is_ipv4() { size - 1 } else { size };
    let taken = |ip: IpAddr| {
        file.config.addresses.iter().any(|own| own.ip == ip)
            || file.devices.iter().any(|device| device.addresses.iter().any(|of| of.contains(ip)))
    };
    (1..end.min(SEARCH_MOST))
        .map(|offset| address_at(network.ip, first + offset))
        .find(|ip| !taken(*ip))
}

/// The address of the family of `like` whose number is `number`.
fn address_at(like: IpAddr, number: u128) -> IpAddr {
    match like {
        IpAddr::V4(_) => IpAddr::V4(Ipv4Addr::from(u32::try_from(number).unwrap_or(u32::MAX))),
        IpAddr::V6(_) => IpAddr::V6(Ipv6Addr::from(number)),
    }
}

/// `text` with a device added at its end: a blank line, its remark, and
/// its `[Peer]` with `key`, its public key, and `address`, its one address
/// on the tunnel. The blank line is the device's, so that removing the
/// device gives back `text`.
pub(super) fn with_device(text: &str, name: &str, key: &Key, address: IpAddr) -> String {
    // The file's first line says which end its lines have.
    let eol = if text.split_inclusive('\n').next().is_some_and(|line| line.ends_with("\r\n")) {
        "\r\n"
    } else {
        "\n"
    };
    let mut text = text.to_string();
    if !text.is_empty() && !text.ends_with('\n') {
        text.push_str(eol);
    }
    text.push_str(&format!(
        "{eol}# {NAME}: {name}{eol}[Peer]{eol}PublicKey = {}{eol}AllowedIPs = {}{eol}",
        key.base64(),
        host(address)
    ));
    text
}

/// What a device reaches the server at: `endpoint`, a host's name or an
/// address, and `port`, with an IPv6 address in brackets.
pub(super) fn endpoint_of(endpoint: &str, port: u16) -> String {
    if endpoint.parse::<Ipv6Addr>().is_ok() {
        format!("[{endpoint}]:{port}")
    } else {
        format!("{endpoint}:{port}")
    }
}

/// What the server tells a device, for the device to set itself up with.
#[derive(Debug)]
pub(super) struct ToDevice<'a> {
    /// The device's private key, which is written nowhere on the server.
    pub private_key: &'a Key,
    /// The device's address on the tunnel.
    pub address: IpAddr,
    /// The server's public key.
    pub server_key: &'a Key,
    /// What the device reaches the server at.
    pub endpoint: &'a str,
    /// The server's UDP port.
    pub port: u16,
    /// The server's address on the tunnel, which the device reaches.
    pub server_address: IpAddr,
}

/// The device's file, as the WireGuard app reads it (wg-quick's): its key
/// and address, and the server, which the device keeps in touch with every
/// 25 seconds so that a router between them keeps its way open.
pub(super) fn device_file(to: &ToDevice<'_>) -> String {
    format!(
        "[Interface]\nPrivateKey = {}\nAddress = {}\n\n[Peer]\nPublicKey = {}\nEndpoint = {}\n\
         AllowedIPs = {}\nPersistentKeepalive = 25\n",
        to.private_key.base64(),
        host(to.address),
        to.server_key.base64(),
        endpoint_of(to.endpoint, to.port),
        host(to.server_address)
    )
}

/// What `serve` is asked to set; what is `None` stays as the file has it.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(super) struct Settings {
    /// What devices are told to reach.
    pub endpoint: Option<String>,
    /// The UDP port.
    pub port: Option<u16>,
    /// The server's address on the tunnel, with its length.
    pub address: Option<Prefix>,
}

/// The file `serve` makes when there is none.
pub(super) fn first_file(private_key: &Key, endpoint: &str, port: u16, address: Prefix) -> String {
    format!(
        "[Interface]\n# {ENDPOINT}: {endpoint}\nAddress = {address}\nListenPort = {port}\n\
         PrivateKey = {}\n",
        private_key.base64()
    )
}

/// `text` with `settings` made, and nothing else changed.
///
/// # Errors
///
/// When the file does not read, has no `[Interface]`, or has a device
/// that the new address would leave outside its network.
pub(super) fn serve(text: &str, settings: &Settings) -> Result<String, String> {
    let file = read(text)?;
    if let Some(address) = settings.address
        && file.config.addresses != [address]
    {
        fits(&file, address)?;
    }
    let mut text = text.to_string();
    if let Some(address) = settings.address {
        text = put(
            &text,
            |line| setting(line).is_some_and(|(key, _)| key == "address"),
            |line| {
                setting(line).is_some_and(|(_, value)| {
                    value.split(',').map(Prefix::parse).eq(std::iter::once(Some(address)))
                })
            },
            &format!("Address = {address}"),
        )?;
    }
    if let Some(port) = settings.port {
        text = put(
            &text,
            |line| setting(line).is_some_and(|(key, _)| key == "listenport"),
            |line| setting(line).is_some_and(|(_, value)| value.parse::<u16>().ok() == Some(port)),
            &format!("ListenPort = {port}"),
        )?;
    }
    if let Some(endpoint) = &settings.endpoint {
        text = put(
            &text,
            |line| remark(line, ENDPOINT).is_some(),
            |line| remark(line, ENDPOINT) == Some(endpoint.as_str()),
            &format!("# {ENDPOINT}: {endpoint}"),
        )?;
    }
    read(&text)?;
    Ok(text)
}

/// Whether every device's addresses are in the network of `address`, and
/// none is `address` itself.
fn fits(file: &File, address: Prefix) -> Result<(), String> {
    let network = address.network();
    for device in &file.devices {
        for own in &device.addresses {
            if !network.contains(own.ip) || own.ip == address.ip {
                return Err(format!(
                    "{address} does not fit the device {}, which has {own}: remove the device \
                     first",
                    device.name
                ));
            }
        }
    }
    Ok(())
}

/// Makes the `[Interface]` section have one line of a kind, `line`: the
/// first line that `is` of the kind takes its place, unless it `says` it
/// already and is the only one; the others of the kind go; and with none,
/// `line` is added after the last line of the section that is not blank.
fn put(
    text: &str,
    is: impl Fn(&str) -> bool,
    says: impl Fn(&str) -> bool,
    line: &str,
) -> Result<String, String> {
    let all = lines(text);
    let headers = headers(&all);
    let (at, interface) = headers
        .iter()
        .enumerate()
        .find(|(_, header)| header.section == Section::Interface)
        .ok_or("it has no [Interface] section")?;
    let end = headers.get(at + 1).map_or(all.len(), |next| next.start);
    let of_the_kind: Vec<usize> = (interface.at + 1..end).filter(|&at| is(all[at])).collect();
    let mut out: Vec<String> = all.iter().map(ToString::to_string).collect();
    match of_the_kind.split_first() {
        None => {
            let last = (interface.at + 1..end)
                .rev()
                .find(|&at| !all[at].trim().is_empty())
                .unwrap_or(interface.at);
            // The header line says which end the file uses.
            let eol = if all[interface.at].ends_with("\r\n") { "\r\n" } else { "\n" };
            if !out[last].ends_with('\n') {
                out[last].push_str(eol);
            }
            out.insert(last + 1, format!("{line}{eol}"));
        }
        Some((&first, [])) if says(all[first]) => return Ok(text.to_string()),
        Some((&first, others)) => {
            let old = all[first];
            let indent = &old[..old.len() - old.trim_start().len()];
            let eol = if old.ends_with("\r\n") {
                "\r\n"
            } else if old.ends_with('\n') {
                "\n"
            } else {
                ""
            };
            out[first] = format!("{indent}{line}{eol}");
            for &other in others.iter().rev() {
                out.remove(other);
            }
        }
    }
    Ok(out.concat())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Keys as `wg` writes them. The server's is a private key, the others
    /// public.
    const SERVER: &str = "QP+8TVHq83qe2CMjbz/y1jrTl+4P1MHDsqbZ3w1aoVI=";
    const PHONE: &str = "aJ7TlWZX/XND9O54UTiZPjByNwglHgoWcuhjgYX+tFk=";
    const LAPTOP: &str = "yIfVfM4ezrB6rRvCftHpkKu04B8/UvfZa4lyBz++N3s=";
    const LAB: &str = "aI7N0wXX5LsneBALsH/asGXvwP+jtK0er5WND6MgMlk=";
    const TABLET: &str = "MFWK2I9PLdDL6QSmqI9GGF88NCU+XDl46alsmXsAdmk=";

    fn prefix(text: &str) -> Prefix {
        Prefix::parse(text).unwrap()
    }

    /// A file with a named device, one with no name, and a named one
    /// again, each after a blank line.
    fn file() -> String {
        format!(
            "[Interface]\n# endpoint: home.example.org\nAddress = 10.8.0.1/24\n\
             ListenPort = 51820\nPrivateKey = {SERVER}\n\n# name: phone\n[Peer]\n\
             PublicKey = {PHONE}\nAllowedIPs = 10.8.0.2/32\n\n[Peer]\nPublicKey = {LAPTOP}\n\
             AllowedIPs = 10.8.0.3/32\n\n# name: lab\n[Peer]\nPublicKey = {LAB}\n\
             AllowedIPs = 10.8.0.4/32\n"
        )
    }

    fn settings(endpoint: Option<&str>, port: Option<u16>, network: Option<&str>) -> Settings {
        Settings { endpoint: endpoint.map(str::to_string), port, address: network.map(prefix) }
    }

    #[test]
    fn the_interface_and_each_device_are_found_by_their_remarks() {
        let file = read(&file()).unwrap();
        assert_eq!(file.endpoint.as_deref(), Some("home.example.org"));
        assert_eq!(file.address(), Some(prefix("10.8.0.1/24")));
        assert_eq!(file.config.listen_port, Some(51820));
        let names: Vec<&str> = file.devices.iter().map(|device| device.name.as_str()).collect();
        assert_eq!(names, ["phone", "yIfVfM4e", "lab"]);
        assert_eq!(file.devices[0].addresses, [prefix("10.8.0.2/32")]);
        assert_eq!(file.devices[2].key, Key::from_base64(LAB).unwrap());
    }

    #[test]
    fn a_device_owns_its_remark_its_lines_and_the_blank_line_above() {
        // Lines: 0 [Interface], 1 endpoint, 2 Address, 3 ListenPort,
        // 4 PrivateKey, 5 blank, 6 name, 7 [Peer], 8 PublicKey,
        // 9 AllowedIPs, 10 blank, 11 [Peer], 12 and 13 its settings,
        // 14 blank, 15 name, 16 [Peer], 17 and 18 its settings.
        let places: Vec<Range<usize>> =
            read(&file()).unwrap().devices.into_iter().map(|device| device.lines).collect();
        assert_eq!(places, [5..10, 10..14, 14..19]);
    }

    #[test]
    fn a_remark_that_is_not_right_before_its_peer_names_nothing() {
        let text = format!(
            "[Interface]\nPrivateKey = {SERVER}\n# name: phone\n\n[Peer]\nPublicKey = {PHONE}\n\
             AllowedIPs = 10.8.0.2/32\n"
        );
        assert_eq!(read(&text).unwrap().devices[0].name, "aJ7TlWZX");
        // A remark in the interface's lines or in a device's own names no
        // device.
        let text = format!(
            "[Interface]\n# name: server\nPrivateKey = {SERVER}\n[Peer]\n# name: self\n\
             PublicKey = {PHONE}\nAllowedIPs = 10.8.0.2/32\n"
        );
        assert_eq!(read(&text).unwrap().devices[0].name, "aJ7TlWZX");
    }

    #[test]
    fn remarks_are_read_loosely_in_their_spaces_and_case() {
        for line in ["#name:phone", "#  Name :  phone  ", "\t# NAME: phone\r\n"] {
            assert_eq!(remark(line, NAME), Some("phone"), "{line:?}");
        }
        for line in ["# name:", "# names: phone", "name: phone", "# the name: phone", "#"] {
            assert_eq!(remark(line, NAME), None, "{line:?}");
        }
    }

    #[test]
    fn a_name_is_found_exactly_and_may_belong_to_more_than_one_device() {
        let text = file().replace("# name: lab", "# name: phone");
        let file = read(&text).unwrap();
        assert_eq!(file.called("phone").len(), 2);
        assert_eq!(file.called("yIfVfM4e").len(), 1);
        assert!(file.called("Phone").is_empty());
        assert!(file.called("lab").is_empty());
        assert!(file.called("").is_empty());
    }

    #[test]
    fn a_file_that_does_not_read_says_what_and_where() {
        let why = read("[Interface]\nPrivateKey = nonsense\n").unwrap_err();
        assert!(why.starts_with("line 2: "), "{why}");
        assert!(read("[Interface]\n[Peer]\nAllowedIPs = 10.8.0.2/32\n").is_err());
    }

    #[test]
    fn a_file_with_no_device_and_no_remark_reads() {
        let file = read(&format!("[Interface]\nPrivateKey = {SERVER}\n")).unwrap();
        assert_eq!((file.devices.len(), file.address(), file.endpoint), (0, None, None));
    }

    #[test]
    fn the_address_is_the_first_ipv4_one() {
        let text = format!(
            "[Interface]\nAddress = fd00::1/64\nAddress = 10.8.0.1/24, 10.9.0.1/24\n\
             PrivateKey = {SERVER}\n"
        );
        assert_eq!(read(&text).unwrap().address(), Some(prefix("10.8.0.1/24")));
        let text = format!("[Interface]\nAddress = fd00::1/64\nPrivateKey = {SERVER}\n");
        assert_eq!(read(&text).unwrap().address(), Some(prefix("fd00::1/64")));
    }

    #[test]
    fn removing_a_device_leaves_every_other_line_as_it_was() {
        let text = file();
        let file = read(&text).unwrap();
        assert_eq!(
            without(&text, &file.devices[0].lines),
            format!(
                "[Interface]\n# endpoint: home.example.org\nAddress = 10.8.0.1/24\n\
                 ListenPort = 51820\nPrivateKey = {SERVER}\n\n[Peer]\nPublicKey = {LAPTOP}\n\
                 AllowedIPs = 10.8.0.3/32\n\n# name: lab\n[Peer]\nPublicKey = {LAB}\n\
                 AllowedIPs = 10.8.0.4/32\n"
            )
        );
        let last = without(&text, &file.devices[2].lines);
        assert!(last.ends_with("AllowedIPs = 10.8.0.3/32\n"), "{last}");
        assert_eq!(read(&last).unwrap().devices.len(), 2);
    }

    #[test]
    fn lines_added_for_a_device_and_removed_again_leave_the_file_as_it_was() {
        let before = file();
        let added = format!(
            "{before}\n# name: tablet\n[Peer]\nPublicKey = {TABLET}\nAllowedIPs = 10.8.0.9/32\n"
        );
        let file = read(&added).unwrap();
        assert_eq!(without(&added, &file.called("tablet")[0].lines), before);
    }

    #[test]
    fn a_file_with_windows_line_ends_is_read_and_cut_the_same() {
        let text = file().replace('\n', "\r\n");
        let file = read(&text).unwrap();
        assert_eq!(file.devices[0].name, "phone");
        assert_eq!(file.endpoint.as_deref(), Some("home.example.org"));
        let after = without(&text, &file.devices[0].lines);
        assert!(after.contains("\r\n\r\n[Peer]\r\nPublicKey = yIfV"), "{after:?}");
        assert!(!after.replace("\r\n", "").contains('\n'));
    }

    #[test]
    fn the_first_file_reads_back_as_it_was_made() {
        let key = Key([7; 32]);
        let text = first_file(&key, "home.example.org", 51820, prefix("10.8.0.1/24"));
        assert_eq!(
            text,
            format!(
                "[Interface]\n# endpoint: home.example.org\nAddress = 10.8.0.1/24\n\
                 ListenPort = 51820\nPrivateKey = {}\n",
                key.base64()
            )
        );
        let file = read(&text).unwrap();
        assert_eq!(file.endpoint.as_deref(), Some("home.example.org"));
        assert_eq!(file.config.private_key, Some(key));
        assert_eq!(file.config.listen_port, Some(51820));
        assert_eq!(file.address(), Some(prefix("10.8.0.1/24")));
        assert!(file.devices.is_empty());
    }

    #[test]
    fn serving_with_what_the_file_says_changes_nothing() {
        let text = file();
        let same = settings(Some("home.example.org"), Some(51820), Some("10.8.0.1/24"));
        assert_eq!(serve(&text, &same).unwrap(), text);
        assert_eq!(serve(&text, &Settings::default()).unwrap(), text);
        // Said in other words, it is still what it says.
        let loose = text.replace("ListenPort = 51820", "listenport=51820 # the port");
        assert_eq!(serve(&loose, &same).unwrap(), loose);
    }

    #[test]
    fn serving_changes_the_lines_given_and_no_others() {
        let text = file();
        let new = serve(&text, &settings(Some("vpn.example.net"), Some(40000), None)).unwrap();
        assert_eq!(
            new,
            text.replace("home.example.org", "vpn.example.net")
                .replace("ListenPort = 51820", "ListenPort = 40000")
        );
        assert_eq!(read(&new).unwrap().devices.len(), 3);
    }

    #[test]
    fn a_line_that_is_changed_keeps_its_indent_and_loses_its_remark() {
        let text = file().replace("ListenPort = 51820", "  ListenPort=51820 # the port");
        let new = serve(&text, &settings(None, Some(40000), None)).unwrap();
        assert!(new.contains("\n  ListenPort = 40000\n"), "{new}");
        assert!(!new.contains("the port"));
    }

    #[test]
    fn lines_the_file_lacks_are_added_at_the_end_of_the_interface() {
        let text = format!(
            "[Interface]\nPrivateKey = {SERVER}\n\n# name: phone\n[Peer]\nPublicKey = {PHONE}\n\
             AllowedIPs = 10.8.0.2/32\n"
        );
        let all = settings(Some("home.example.org"), Some(51820), Some("10.8.0.1/24"));
        let new = serve(&text, &all).unwrap();
        assert_eq!(
            new,
            format!(
                "[Interface]\nPrivateKey = {SERVER}\nAddress = 10.8.0.1/24\nListenPort = 51820\n\
                 # endpoint: home.example.org\n\n# name: phone\n[Peer]\nPublicKey = {PHONE}\n\
                 AllowedIPs = 10.8.0.2/32\n"
            )
        );
        let file = read(&new).unwrap();
        assert_eq!(file.devices[0].name, "phone");
        assert_eq!(file.endpoint.as_deref(), Some("home.example.org"));
    }

    #[test]
    fn a_file_that_ends_without_a_newline_gets_one_and_keeps_its_kind_of_line_end() {
        let port = settings(None, Some(51820), None);
        let text = format!("[Interface]\r\nPrivateKey = {SERVER}");
        assert_eq!(
            serve(&text, &port).unwrap(),
            format!("[Interface]\r\nPrivateKey = {SERVER}\r\nListenPort = 51820\r\n")
        );
        let text = format!("[Interface]\nPrivateKey = {SERVER}");
        assert_eq!(
            serve(&text, &port).unwrap(),
            format!("[Interface]\nPrivateKey = {SERVER}\nListenPort = 51820\n")
        );
    }

    #[test]
    fn an_address_becomes_the_only_address_line() {
        let text = format!(
            "[Interface]\nAddress = 10.8.0.1/24\nAddress = fd00::1/64\nPrivateKey = {SERVER}\n\
             Address = 10.8.0.1/24\n"
        );
        let new = serve(&text, &settings(None, None, Some("10.8.0.1/24"))).unwrap();
        assert_eq!(new, format!("[Interface]\nAddress = 10.8.0.1/24\nPrivateKey = {SERVER}\n"));
        assert_eq!(read(&new).unwrap().config.addresses, [prefix("10.8.0.1/24")]);
    }

    #[test]
    fn an_address_that_leaves_a_device_outside_its_network_is_refused() {
        let text = file();
        let why = serve(&text, &settings(None, None, Some("10.9.0.1/24"))).unwrap_err();
        assert!(why.contains("the device phone, which has 10.8.0.2/32"), "{why}");
        assert!(why.ends_with("remove the device first"), "{why}");
        // A wider network keeps them; the server's own address may not be
        // a device's; and a device of the other kind of address is outside.
        assert!(serve(&text, &settings(None, None, Some("10.8.1.1/16"))).is_ok());
        assert!(serve(&text, &settings(None, None, Some("10.8.0.3/24"))).is_err());
        assert!(serve(&text, &settings(None, None, Some("fd00::1/64"))).is_err());
    }

    #[test]
    fn a_file_with_no_interface_or_that_does_not_read_is_refused() {
        let none = format!("[Peer]\nPublicKey = {PHONE}\nAllowedIPs = 10.8.0.2/32\n");
        assert_eq!(
            serve(&none, &settings(None, Some(51820), None)).unwrap_err(),
            "it has no [Interface] section"
        );
        assert!(serve("[Interface]\nListenPort = x\n", &settings(None, Some(1024), None)).is_err());
    }

    #[test]
    fn the_endpoint_of_a_peer_is_not_the_servers() {
        let text = format!(
            "[Interface]\nPrivateKey = {SERVER}\n\n# name: phone\n[Peer]\n# endpoint: elsewhere\n\
             PublicKey = {PHONE}\nAllowedIPs = 10.8.0.2/32\n"
        );
        assert_eq!(read(&text).unwrap().endpoint, None);
        let new = serve(&text, &settings(Some("home.example.org"), None, None)).unwrap();
        assert!(new.contains("# endpoint: elsewhere\n"));
        assert_eq!(read(&new).unwrap().endpoint.as_deref(), Some("home.example.org"));
    }

    #[test]
    fn a_new_device_gets_the_first_address_no_one_has() {
        let parsed = read(&file()).unwrap();
        let server = parsed.address().unwrap();
        assert_eq!(free_address(&parsed, server), Some("10.8.0.5".parse().unwrap()));
        // A gap a removed device left is filled first.
        let text = file().replace("AllowedIPs = 10.8.0.3/32", "AllowedIPs = 10.8.0.9/32");
        assert_eq!(free_address(&read(&text).unwrap(), server), Some("10.8.0.3".parse().unwrap()));
    }

    #[test]
    fn a_device_that_holds_a_network_holds_each_of_its_addresses() {
        let text = file().replace("AllowedIPs = 10.8.0.4/32", "AllowedIPs = 10.8.0.4/30");
        let file = read(&text).unwrap();
        assert_eq!(free_address(&file, file.address().unwrap()), Some("10.8.0.8".parse().unwrap()));
    }

    #[test]
    fn a_full_network_has_no_free_address_and_its_ends_are_never_given() {
        // 10.8.0.0/30: .0 is the network's, .1 the server's, .3 the
        // broadcast; the one device has .2.
        let text = format!(
            "[Interface]\nAddress = 10.8.0.1/30\nPrivateKey = {SERVER}\n\n[Peer]\n\
             PublicKey = {PHONE}\nAllowedIPs = 10.8.0.2/32\n"
        );
        let file = read(&text).unwrap();
        assert_eq!(free_address(&file, file.address().unwrap()), None);
        // A server that is not the network's first leaves the first free.
        let text = format!("[Interface]\nAddress = 10.8.0.2/30\nPrivateKey = {SERVER}\n");
        let file = read(&text).unwrap();
        assert_eq!(free_address(&file, file.address().unwrap()), Some("10.8.0.1".parse().unwrap()));
    }

    #[test]
    fn an_ipv6_tunnel_gives_out_its_last_address_too() {
        let text = format!("[Interface]\nAddress = fd00:8::1/126\nPrivateKey = {SERVER}\n");
        let file = read(&text).unwrap();
        let server = file.address().unwrap();
        assert_eq!(free_address(&file, server), Some("fd00:8::2".parse().unwrap()));
        let text = format!("{text}\n[Peer]\nPublicKey = {PHONE}\nAllowedIPs = fd00:8::2/128\n");
        assert_eq!(free_address(&read(&text).unwrap(), server), Some("fd00:8::3".parse().unwrap()));
    }

    #[test]
    fn a_device_added_reads_back_with_its_name_key_and_address_and_removing_it_gives_the_text_back()
    {
        let tablet = Key::from_base64(TABLET).unwrap();
        for text in [file(), file().replace('\n', "\r\n"), file().trim_end().to_string()] {
            let new = with_device(&text, "tablet", &tablet, "10.8.0.5".parse().unwrap());
            let read_back = read(&new).unwrap();
            let [device] = read_back.called("tablet")[..] else { panic!("{new:?}") };
            assert_eq!(
                (device.key, device.addresses.as_slice()),
                (tablet, &[prefix("10.8.0.5/32")][..])
            );
            assert_eq!(read_back.devices.len(), 4);
            // The text ends with a line end; one it lacked is the device's
            // to add, and stays.
            let back = without(&new, &device.lines);
            let whole = if text.ends_with('\n') { text.clone() } else { format!("{text}\n") };
            assert_eq!(back, whole, "{new:?}");
            if text.contains("\r\n") {
                assert!(!new.replace("\r\n", "").contains('\n'), "{new:?}");
            }
        }
    }

    #[test]
    fn the_device_file_has_its_key_and_address_and_reaches_the_server_alone() {
        let device = Key::from_base64(PHONE).unwrap();
        let server = Key::from_base64(LAB).unwrap();
        let to = ToDevice {
            private_key: &device,
            address: "10.8.0.5".parse().unwrap(),
            server_key: &server,
            endpoint: "home.example.org",
            port: 51820,
            server_address: "10.8.0.1".parse().unwrap(),
        };
        assert_eq!(
            device_file(&to),
            format!(
                "[Interface]\nPrivateKey = {PHONE}\nAddress = 10.8.0.5/32\n\n[Peer]\n\
                 PublicKey = {LAB}\nEndpoint = home.example.org:51820\nAllowedIPs = 10.8.0.1/32\n\
                 PersistentKeepalive = 25\n"
            )
        );
        // What prints the struct leaves its keys out.
        let shown = format!("{to:?}");
        assert!(!shown.contains(PHONE) && !shown.contains(LAB), "{shown}");
    }

    #[test]
    fn an_ipv6_endpoint_is_in_brackets() {
        assert_eq!(endpoint_of("2001:db8::7", 51820), "[2001:db8::7]:51820");
        assert_eq!(endpoint_of("203.0.113.7", 51820), "203.0.113.7:51820");
        assert_eq!(endpoint_of("home.example.org", 443), "home.example.org:443");
    }
}
