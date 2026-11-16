// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! What `database` accepts as a name, a text sorting, a site and a
//! network, so that nothing it sends PostgreSQL or writes in
//! `pg_hba.conf` is anything else.
//!
//! A database and a role are called by the one rule: a lowercase letter,
//! then lowercase letters, digits and `_`, up to 63 characters in all,
//! and never a name PostgreSQL keeps for itself. The rule is stricter
//! than PostgreSQL's, which takes nearly anything between double quotes;
//! the names are still quoted as identifiers wherever a statement holds
//! one.

use std::fmt;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use super::server::{DUMPS_IN_SHARED, SHARED, SNAPSHOTS_OF_SHARED};
use crate::cert::acme;

/// The longest name PostgreSQL takes: 63 bytes.
pub const NAME_MAX: usize = 63;

/// Names PostgreSQL keeps for its own databases and role.
const RESERVED: [&str; 3] = ["postgres", "template0", "template1"];

/// The sorting a new database may have: the C library's English (US),
/// the default, and the builtin provider's `C.UTF-8`, which sorts by
/// character code and is the same on every machine.
pub const LOCALES: [&str; 2] = ["en_US.UTF-8", "C.UTF-8"];

/// The sorting a database has when none is asked for.
pub const DEFAULT_LOCALE: &str = LOCALES[0];

/// The most networks one `allow` takes.
pub const NETWORKS_MAX: usize = 20;

/// Checks `name` as the name of a database or a role.
///
/// # Errors
///
/// What is wrong with it, as a sentence that follows the name.
pub fn check_name(name: &str) -> Result<(), String> {
    let mut chars = name.chars();
    let fits = name.len() <= NAME_MAX
        && chars.next().is_some_and(|first| first.is_ascii_lowercase())
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_');
    if !fits {
        return Err(format!(
            "`{}` is not a name for a database or a role: it starts with a lowercase letter and \
             has lowercase letters, digits and _, up to {NAME_MAX} characters",
            name.escape_debug()
        ));
    }
    if name.starts_with("pg_") || RESERVED.contains(&name) {
        return Err(format!(
            "`{name}` is a name PostgreSQL keeps for itself: choose another, not one that \
             starts with pg_, and not postgres, template0 or template1"
        ));
    }
    Ok(())
}

/// Checks `locale` as one of the sortings a new database may have.
///
/// # Errors
///
/// That it is none of [`LOCALES`], with the two.
pub fn check_locale(locale: &str) -> Result<(), String> {
    if LOCALES.contains(&locale) {
        Ok(())
    } else {
        Err(format!(
            "`{}` is not a sorting database makes: use {} or {}",
            locale.escape_debug(),
            LOCALES[0],
            LOCALES[1]
        ))
    }
}

/// Checks `site` as the name of a website, by the rule `cert` names a
/// certificate's names by.
///
/// # Errors
///
/// What is wrong with it.
pub fn check_site(site: &str) -> Result<(), String> {
    acme::check_dns_name_to_check(site, false)
}

/// The most bytes of a snapshot's name.
const SNAPSHOT_MAX: usize = 255;

/// The most bytes of a copy's file name, before `.dump`.
const COPY_NAME_MAX: usize = 100;

/// Whether `name` can be a snapshot's name in a copy's path: 1 to 255
/// printable ASCII characters, which are not `/` and do not start with a
/// dot.
#[must_use]
pub fn snapshot_name_ok(name: &str) -> bool {
    (1..=SNAPSHOT_MAX).contains(&name.len())
        && !name.starts_with('.')
        && name.bytes().all(|byte| (b' '..=b'~').contains(&byte) && byte != b'/')
}

/// Checks `path` as a copy that the web console may put back: a file
/// `NAME.dump` in the folder of the copies the backup hook makes, or in
/// the same folder of a snapshot of `/shared`.
///
/// The file name has letters, digits, `_`, `.` and `-`, and does not start
/// with a dot or a hyphen, which is what the hook gives a copy.
///
/// # Errors
///
/// What the path must be.
pub fn check_copy_path(path: &str) -> Result<(), String> {
    let wrong = || {
        format!(
            "`{}` is not a copy the backup hook made: it is a file NAME.dump in \
             {SHARED}/{DUMPS_IN_SHARED}, or in the same folder of a snapshot, \
             {SNAPSHOTS_OF_SHARED}/SNAPSHOT/{DUMPS_IN_SHARED}",
            path.escape_debug()
        )
    };
    let inside = if let Some(rest) = path.strip_prefix(&format!("{SHARED}/")) {
        rest
    } else if let Some(rest) = path.strip_prefix(&format!("{SNAPSHOTS_OF_SHARED}/")) {
        let (snapshot, rest) = rest.split_once('/').ok_or_else(wrong)?;
        if !snapshot_name_ok(snapshot) {
            return Err(wrong());
        }
        rest
    } else {
        return Err(wrong());
    };
    let file = inside
        .strip_prefix(DUMPS_IN_SHARED)
        .and_then(|rest| rest.strip_prefix('/'))
        .and_then(|rest| rest.strip_suffix(".dump"))
        .ok_or_else(wrong)?;
    let fits = (1..=COPY_NAME_MAX).contains(&file.len())
        && file.starts_with(|c: char| c.is_ascii_alphanumeric() || c == '_')
        && file.chars().all(|c| c.is_ascii_alphanumeric() || "_.-".contains(c));
    if fits { Ok(()) } else { Err(wrong()) }
}

/// A network a database is opened to: an address and the length of the
/// prefix that says how much of it counts. The bits after the prefix are
/// zero, as PostgreSQL reads them whatever they are written as.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Network {
    /// The first address of the network.
    address: IpAddr,
    /// How many of the address's leading bits name the network.
    prefix: u8,
}

impl Network {
    /// The network `text` writes, `192.168.1.0/24` or `2001:db8::/64`.
    ///
    /// # Errors
    ///
    /// When it is not an address with a prefix length: no length, a
    /// length past the address's bits, or a name in place of an address.
    pub fn parse(text: &str) -> Result<Network, String> {
        let wrong = || {
            format!(
                "`{}` is not an address with its prefix length, such as 192.168.1.0/24 or \
                 2001:db8::/64",
                text.escape_debug()
            )
        };
        let (address, prefix) = text.split_once('/').ok_or_else(wrong)?;
        let address: IpAddr = address.parse().map_err(|_| wrong())?;
        if prefix.is_empty() || !prefix.bytes().all(|b| b.is_ascii_digit()) {
            return Err(wrong());
        }
        let prefix: u8 = prefix.parse().map_err(|_| wrong())?;
        let bits = if address.is_ipv4() { 32 } else { 128 };
        if prefix > bits {
            return Err(format!("`{text}` has a prefix length past the address's {bits} bits"));
        }
        Ok(Network::new(address, prefix))
    }

    /// The network of `address`'s first `prefix` bits.
    fn new(address: IpAddr, prefix: u8) -> Network {
        let address = match address {
            IpAddr::V4(v4) => {
                let mask = u32::MAX.checked_shl(32 - u32::from(prefix)).unwrap_or(0);
                IpAddr::V4(Ipv4Addr::from(u32::from(v4) & mask))
            }
            IpAddr::V6(v6) => {
                let mask = u128::MAX.checked_shl(128 - u32::from(prefix)).unwrap_or(0);
                IpAddr::V6(Ipv6Addr::from(u128::from(v6) & mask))
            }
        };
        Network { address, prefix }
    }

    /// The network that `address` and `netmask` say, as PostgreSQL's
    /// `pg_hba_file_rules` gives them. None when they are not an
    /// address and a netmask of its kind that is a prefix.
    #[must_use]
    pub fn of_netmask(address: &str, netmask: &str) -> Option<Network> {
        let address: IpAddr = address.parse().ok()?;
        let mask: IpAddr = netmask.parse().ok()?;
        let (ones, bits) = match (address, mask) {
            (IpAddr::V4(_), IpAddr::V4(mask)) => {
                let mask = u32::from(mask);
                (mask.leading_ones(), mask.count_ones())
            }
            (IpAddr::V6(_), IpAddr::V6(mask)) => {
                let mask = u128::from(mask);
                (mask.leading_ones(), mask.count_ones())
            }
            _ => return None,
        };
        // A mask whose ones are not all in front is no prefix.
        if ones != bits {
            return None;
        }
        Some(Network::new(address, u8::try_from(ones).ok()?))
    }
}

impl fmt::Display for Network {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}", self.address, self.prefix)
    }
}

/// Checks the networks of one `allow` or `deny`: at most
/// [`NETWORKS_MAX`], none twice.
///
/// # Errors
///
/// What is wrong with the list.
pub fn check_networks(networks: &[Network]) -> Result<(), String> {
    if networks.len() > NETWORKS_MAX {
        return Err(format!(
            "there are {} networks, and at most {NETWORKS_MAX} are taken",
            networks.len()
        ));
    }
    for (at, network) in networks.iter().enumerate() {
        if networks[..at].contains(network) {
            return Err(format!("{network} is named twice"));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_name_is_a_lowercase_letter_then_lowercase_letters_digits_and_underscores() {
        for good in ["shop", "my_blog", "a", "shop_2", &"a".repeat(63)] {
            assert_eq!(check_name(good), Ok(()), "{good}");
        }
        for bad in [
            "",
            "Shop",
            "2shop",
            "_shop",
            "my-blog",
            "a b",
            "a;b",
            "a\"b",
            "it's",
            "shop.example",
            "caf\u{e9}",
            "a\0b",
            &"a".repeat(64),
        ] {
            assert!(check_name(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn the_names_postgresql_keeps_are_refused() {
        for kept in ["postgres", "template0", "template1", "pg_x", "pg_database_owner", "pg_"] {
            let why = check_name(kept).unwrap_err();
            assert!(why.contains("keeps for itself"), "{kept}: {why}");
        }
        // Only the name itself, and the prefix, are kept.
        for fine in ["postgres2", "mypg_x", "template", "pg", "pgsql"] {
            assert_eq!(check_name(fine), Ok(()), "{fine}");
        }
    }

    #[test]
    fn a_copy_is_a_dump_in_the_folder_of_the_hook_or_in_a_snapshot_of_it() {
        let live = "/shared/services/org.postgresql.postgresql/postgres/dumps";
        let snap =
            "/snapshots/shared/2026-11-16-0200/services/org.postgresql.postgresql/postgres/dumps";
        for good in [
            format!("{live}/shop.dump"),
            format!("{live}/Shop_2.x-y.dump"),
            format!("{snap}/shop.dump"),
            "/snapshots/shared/backup 1/services/org.postgresql.postgresql/postgres/dumps/a.dump"
                .to_string(),
            format!("{live}/_x.dump"),
        ] {
            assert_eq!(check_copy_path(&good), Ok(()), "{good}");
        }
        for bad in [
            String::new(),
            "shop.dump".to_string(),
            format!("{live}/shop.sql"),
            format!("{live}/.dump"),
            format!("{live}/.hidden.dump"),
            format!("{live}/-x.dump"),
            format!("{live}/a/b.dump"),
            format!("{live}/../data/x.dump"),
            format!("{live}/a b.dump"),
            format!("{live}/{}.dump", "a".repeat(101)),
            format!("{live}x/shop.dump"),
            format!("{live}//shop.dump"),
            "/shared/services/org.postgresql.postgresql/postgres/data/shop.dump".to_string(),
            "/shared/services/org.postgresql.postgresql/postgres/dumps".to_string(),
            "/users/ann/shop.dump".to_string(),
            "/shared/../etc/shop.dump".to_string(),
            "/snapshots/shared/services/org.postgresql.postgresql/postgres/dumps/shop.dump"
                .to_string(),
            "/snapshots/shared/.hidden/services/org.postgresql.postgresql/postgres/dumps/a.dump"
                .to_string(),
            "/snapshots/shared/caf\u{e9}/services/org.postgresql.postgresql/postgres/dumps/a.dump"
                .to_string(),
            "/snapshots/shared//services/org.postgresql.postgresql/postgres/dumps/a.dump"
                .to_string(),
            "/snapshots/users/x/services/org.postgresql.postgresql/postgres/dumps/a.dump"
                .to_string(),
            format!("{live}/shop.dump\n"),
            format!("{live}/shop\0.dump"),
        ] {
            assert!(check_copy_path(&bad).is_err(), "{bad:?}");
        }
        assert!(snapshot_name_ok("2026-11-16-0200.2"));
        assert!(snapshot_name_ok("before the update"));
        for bad in ["", ".x", "a/b", "a\nb", "caf\u{e9}", "a\0b", &"a".repeat(256)] {
            assert!(!snapshot_name_ok(bad), "{bad:?}");
        }
        assert!(snapshot_name_ok(&"a".repeat(255)));
    }

    #[test]
    fn the_sortings_are_the_two_the_page_offers() {
        assert_eq!(check_locale("en_US.UTF-8"), Ok(()));
        assert_eq!(check_locale("C.UTF-8"), Ok(()));
        for bad in ["de_DE.UTF-8", "c.utf-8", "", "C", "en_US.UTF-8; DROP"] {
            assert!(check_locale(bad).is_err(), "{bad:?}");
        }
        assert_eq!(DEFAULT_LOCALE, "en_US.UTF-8");
    }

    #[test]
    fn a_site_is_a_name_as_cert_takes_them() {
        for good in ["shop.example.org", "shop", "check", "my-blog.example.org"] {
            assert_eq!(check_site(good), Ok(()), "{good}");
        }
        for bad in ["", "Shop.example.org", "*.example.org", "a..b", "-x", "a b", "it's.example"] {
            assert!(check_site(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn a_network_is_an_address_and_its_prefix_length() {
        for (text, written) in [
            ("192.168.1.0/24", "192.168.1.0/24"),
            ("10.0.2.0/24", "10.0.2.0/24"),
            ("2001:db8::/64", "2001:db8::/64"),
            ("2001:0db8:0000::/48", "2001:db8::/48"),
            ("127.0.0.1/32", "127.0.0.1/32"),
            ("0.0.0.0/0", "0.0.0.0/0"),
            ("::/0", "::/0"),
        ] {
            assert_eq!(Network::parse(text).unwrap().to_string(), written, "{text}");
        }
        for bad in [
            "10.0.2.0",
            "anywhere",
            "",
            "/24",
            "10.0.2.0/",
            "10.0.2.0/33",
            "::/129",
            "10.0.2.0/+24",
            "10.0.2.0/-1",
            "10.0.2.0/ 24",
            " 10.0.2.0/24",
            "10.0.2.0/24 ",
            "10.0.2/24",
            "example.org/24",
            "10.0.2.0/24/8",
            "10.0.2.0/256",
            "fe80::1%eth0/64",
        ] {
            assert!(Network::parse(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn the_bits_after_the_prefix_are_zero() {
        assert_eq!(Network::parse("192.168.1.77/24").unwrap().to_string(), "192.168.1.0/24");
        assert_eq!(Network::parse("10.255.255.255/9").unwrap().to_string(), "10.128.0.0/9");
        assert_eq!(Network::parse("2001:db8::1/64").unwrap().to_string(), "2001:db8::/64");
        assert_eq!(Network::parse("1.2.3.4/0").unwrap().to_string(), "0.0.0.0/0");
        assert_eq!(
            Network::parse("192.168.1.77/24").unwrap(),
            Network::parse("192.168.1.0/24").unwrap()
        );
    }

    #[test]
    fn a_netmask_of_the_servers_gives_the_prefix() {
        let net = |a, m| Network::of_netmask(a, m).map(|n| n.to_string());
        assert_eq!(net("192.0.2.0", "255.255.255.0").as_deref(), Some("192.0.2.0/24"));
        assert_eq!(net("127.0.0.1", "255.255.255.255").as_deref(), Some("127.0.0.1/32"));
        assert_eq!(net("0.0.0.0", "0.0.0.0").as_deref(), Some("0.0.0.0/0"));
        assert_eq!(
            net("::1", "ffff:ffff:ffff:ffff:ffff:ffff:ffff:ffff").as_deref(),
            Some("::1/128")
        );
        assert_eq!(net("2001:db8::", "ffff:ffff:ffff:ffff::").as_deref(), Some("2001:db8::/64"));
        // Not a prefix, not of one kind, not an address.
        assert_eq!(net("192.0.2.0", "255.0.255.0"), None);
        assert_eq!(net("192.0.2.0", "ffff::"), None);
        assert_eq!(net("example.org", "255.255.255.0"), None);
        assert_eq!(net("all", ""), None);
    }

    #[test]
    fn the_networks_of_one_command_are_at_most_twenty_and_none_twice() {
        let many: Vec<Network> =
            (0..21).map(|n| Network::parse(&format!("10.0.{n}.0/24")).unwrap()).collect();
        assert_eq!(check_networks(&many[..20]), Ok(()));
        assert!(check_networks(&many).unwrap_err().contains("at most 20"));
        let twice = [many[0], many[1], many[0]];
        assert!(check_networks(&twice).unwrap_err().contains("10.0.0.0/24 is named twice"));
        assert_eq!(check_networks(&[]), Ok(()));
    }
}
