// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The writer's own address on the lab network. The lab names the card
//! by its MAC address and gives the address with its prefix
//! (`dukovo.lab.mac=` and `dukovo.lab.ip=`); the writer sets it with the
//! kernel's interface ioctls, brings the card up and waits for its link.
//! There is no gateway: the lab PC is on the same network.

use std::net::{Ipv4Addr, UdpSocket};
use std::os::fd::AsRawFd;
use std::path::Path;
use std::time::{Duration, Instant};

/// Where the kernel lists network cards.
const CARDS: &str = "/sys/class/net";

/// An address with its prefix length, such as `10.77.0.2/24`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Address {
    pub ip: Ipv4Addr,
    pub prefix: u8,
}

impl Address {
    pub fn parse(text: &str) -> Result<Address, String> {
        let (ip, prefix) = text.split_once('/').ok_or_else(|| format!("{text} has no /prefix"))?;
        let ip = ip.parse().map_err(|_| format!("{text} is not an IPv4 address"))?;
        let prefix = prefix
            .parse::<u8>()
            .ok()
            .filter(|prefix| (1..=30).contains(prefix))
            .ok_or_else(|| format!("{text}'s prefix is not 1 to 30"))?;
        Ok(Address { ip, prefix })
    }

    /// The netmask of the prefix.
    pub fn netmask(self) -> Ipv4Addr {
        Ipv4Addr::from(u32::MAX << (32 - u32::from(self.prefix)))
    }
}

/// Reads a MAC address, `aa:bb:cc:dd:ee:ff` in either case, into the
/// lower-case form the kernel shows.
pub fn parse_mac(text: &str) -> Result<String, String> {
    let parts: Vec<&str> = text.split(':').collect();
    let valid = parts.len() == 6
        && parts.iter().all(|part| part.len() == 2 && part.bytes().all(|b| b.is_ascii_hexdigit()));
    if valid { Ok(text.to_ascii_lowercase()) } else { Err(format!("{text} is not a MAC address")) }
}

/// Gives the card with MAC address `mac` the address `address` and brings
/// it up, waiting `wait` for the card to show up and then for its link.
/// Returns the card's name.
pub fn configure(mac: &str, address: Address, wait: Duration) -> Result<String, String> {
    let deadline = Instant::now() + wait;
    let card = loop {
        if let Some(card) = card_with_mac(Path::new(CARDS), mac) {
            break card;
        }
        if Instant::now() >= deadline {
            return Err(format!(
                "no network card has the MAC address {mac}; the cards are: {}",
                cards(Path::new(CARDS)).join(", ")
            ));
        }
        std::thread::sleep(Duration::from_millis(250));
    };
    // Any IPv4 socket carries the interface ioctls.
    let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0))
        .map_err(|err| format!("opening a socket for the card's ioctls: {err}"))?;
    set_address(&socket, &card, libc::SIOCSIFADDR, address.ip)?;
    set_address(&socket, &card, libc::SIOCSIFNETMASK, address.netmask())?;
    bring_up(&socket, &card)?;
    let carrier = Path::new(CARDS).join(&card).join("carrier");
    let linked = Instant::now() + wait;
    while std::fs::read_to_string(&carrier).map_or(true, |text| text.trim() != "1") {
        if Instant::now() >= linked {
            return Err(format!("{card} has no link: is its cable in the lab switch?"));
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    Ok(card)
}

/// The card whose MAC address is `mac`, in the kernel's list at `cards`.
fn card_with_mac(cards: &Path, mac: &str) -> Option<String> {
    let entries = std::fs::read_dir(cards).ok()?;
    entries.filter_map(Result::ok).find_map(|entry| {
        let address = std::fs::read_to_string(entry.path().join("address")).ok()?;
        (address.trim().eq_ignore_ascii_case(mac))
            .then(|| entry.file_name().to_string_lossy().into())
    })
}

/// Each card with its MAC address, for a message.
fn cards(cards: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(cards) else { return vec!["none".into()] };
    let mut found: Vec<String> = entries
        .filter_map(Result::ok)
        .map(|entry| {
            let address = std::fs::read_to_string(entry.path().join("address")).unwrap_or_default();
            format!("{} ({})", entry.file_name().to_string_lossy(), address.trim())
        })
        .collect();
    found.sort();
    if found.is_empty() {
        found.push("none".into());
    }
    found
}

/// A zeroed `ifreq` naming `card`.
fn request(card: &str) -> Result<libc::ifreq, String> {
    // SAFETY: ifreq is plain data (a name and a union of numbers and
    // addresses), for which all zeros is a valid value.
    let mut request: libc::ifreq = unsafe { std::mem::zeroed() };
    if card.len() >= request.ifr_name.len() {
        return Err(format!("the card's name {card} is too long"));
    }
    for (to, from) in request.ifr_name.iter_mut().zip(card.bytes()) {
        *to = from as libc::c_char;
    }
    Ok(request)
}

/// Sets an IPv4 address of `card` with `ioctl`: its own address or its
/// netmask.
fn set_address(
    socket: &UdpSocket,
    card: &str,
    ioctl: libc::c_ulong,
    address: Ipv4Addr,
) -> Result<(), String> {
    let mut request = request(card)?;
    let mut data = [0 as libc::c_char; 14];
    // A sockaddr_in in a sockaddr: the port's two bytes, then the address.
    for (to, from) in data[2..6].iter_mut().zip(address.octets()) {
        *to = from as libc::c_char;
    }
    request.ifr_ifru.ifru_addr =
        libc::sockaddr { sa_family: libc::AF_INET as libc::sa_family_t, sa_data: data };
    // SAFETY: the socket is open, and the request is a whole ifreq that
    // lives through the call, as these ioctls read.
    let result = unsafe { libc::ioctl(socket.as_raw_fd(), ioctl, &raw mut request) };
    if result != 0 {
        return Err(format!(
            "setting {card}'s address {address}: {}",
            std::io::Error::last_os_error()
        ));
    }
    Ok(())
}

/// Brings `card` up.
fn bring_up(socket: &UdpSocket, card: &str) -> Result<(), String> {
    let mut request = request(card)?;
    // SAFETY: as in set_address; SIOCGIFFLAGS fills the request's flags.
    let result = unsafe { libc::ioctl(socket.as_raw_fd(), libc::SIOCGIFFLAGS, &raw mut request) };
    if result != 0 {
        return Err(format!("reading {card}'s flags: {}", std::io::Error::last_os_error()));
    }
    // SAFETY: SIOCGIFFLAGS just set the union's flags.
    let flags = unsafe { request.ifr_ifru.ifru_flags };
    request.ifr_ifru.ifru_flags = flags | libc::IFF_UP as libc::c_short;
    // SAFETY: as above.
    let result = unsafe { libc::ioctl(socket.as_raw_fd(), libc::SIOCSIFFLAGS, &raw mut request) };
    if result != 0 {
        return Err(format!("bringing {card} up: {}", std::io::Error::last_os_error()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_labs_words() {
        let address = Address::parse("10.77.0.2/24").unwrap();
        assert_eq!(address.ip, Ipv4Addr::new(10, 77, 0, 2));
        assert_eq!(address.netmask(), Ipv4Addr::new(255, 255, 255, 0));
        assert_eq!(
            Address::parse("10.0.0.9/20").unwrap().netmask(),
            Ipv4Addr::new(255, 255, 240, 0)
        );
        assert!(Address::parse("10.77.0.2").unwrap_err().contains("no /prefix"));
        assert!(Address::parse("10.77.0.2/0").unwrap_err().contains("1 to 30"));
        assert!(Address::parse("lab/24").unwrap_err().contains("IPv4"));
        assert_eq!(parse_mac("AA:bb:CC:00:11:22").unwrap(), "aa:bb:cc:00:11:22");
        assert!(parse_mac("aa:bb:cc:00:11").is_err());
        assert!(parse_mac("aa-bb-cc-00-11-22").is_err());
    }

    #[test]
    fn finds_the_card_by_its_mac() {
        let dir = std::env::temp_dir().join(format!("lab-writer-net-{}", std::process::id()));
        for (card, mac) in [("lo", "00:00:00:00:00:00"), ("enp0s31f6", "aa:bb:cc:00:11:22")] {
            std::fs::create_dir_all(dir.join(card)).unwrap();
            std::fs::write(dir.join(card).join("address"), format!("{mac}\n")).unwrap();
        }
        assert_eq!(card_with_mac(&dir, "AA:BB:CC:00:11:22").as_deref(), Some("enp0s31f6"));
        assert_eq!(card_with_mac(&dir, "aa:bb:cc:00:11:23"), None);
        assert_eq!(cards(&dir), ["enp0s31f6 (aa:bb:cc:00:11:22)", "lo (00:00:00:00:00:00)"]);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
