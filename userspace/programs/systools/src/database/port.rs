// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! PostgreSQL's port in the firewall's rules (ADR 0570). A server that
//! listens on the network has the inbound right for its port, so the
//! firewall lets every address reach it and only `pg_hba.conf` keeps the
//! others out, after the handshakes. `database` therefore keeps a block
//! in `/system/settings/firewall`, between two comment lines, as it keeps
//! one in `pg_hba.conf`:
//!
//! ```text
//! # database(8): who reaches PostgreSQL from other machines (ADR 0570)
//! allow tcp 5432 from 192.168.1.0/24
//! allow tcp 5432 from 10.0.0.0/8
//! deny tcp 5432
//! # database(8): end
//! ```
//!
//! The block has an `allow` for each network that a line of `pg_hba.conf`'s
//! block names, whichever database it is for, since one port serves every
//! database, and a `deny` that ends it. [`wanted`] says what the file is
//! to be, [`read`] what it says now. The block is written and taken away
//! whole by [`crate::firewall::block`], which keeps every other line as it
//! is. It is not a trial: Spark's trial holds files under
//! `/system/settings` alone, and `pg_hba.conf` is not one, so the change
//! is put back by `allow` and `deny` themselves.

use std::net::IpAddr;
use std::path::Path;

use serde::Serialize;

use super::allow::read_optional;
use super::names::Network;
use super::server::PORT;
use super::{errors, hba};
use crate::firewall::block::{self, Marks};
use crate::firewall::{Action, Protocol, Rule};
use crate::util::Fail;

/// The two lines that hold the block.
const MARKS: Marks<'static> = Marks {
    start: "# database(8): who reaches PostgreSQL from other machines (ADR 0570)",
    end: "# database(8): end",
};

/// What the firewall's rules say of the port: the document `list --json`
/// has as the server's `firewall`.
#[derive(Debug, Default, PartialEq, Eq, Serialize)]
pub struct Firewall {
    /// The networks the block opens the port to, as `192.168.1.0/24`, in
    /// the block's order; none when there is no block.
    pub networks: Vec<String>,
    /// The rule outside the block that decides first for some connections
    /// to the port, as the file writes it; none when no rule does.
    pub earlier: Option<String>,
}

/// What the firewall's file is to be.
#[derive(Debug)]
pub struct Wanted {
    /// The whole file.
    pub text: String,
    /// The rule outside the block that decides first for some connections
    /// to the port, as the file writes it; none when no rule does.
    pub earlier: Option<String>,
}

/// The firewall's rules in `before`, with the block open to the networks
/// of `rules`, the lines of `pg_hba.conf`'s block that are to stay: one
/// `allow` for each, in the order they are first seen. With none, no
/// block.
///
/// # Errors
///
/// DB-108 when the file cannot take the block: it has the block's lines
/// but not as one block, or it would hold more rules than the firewall
/// does.
pub fn wanted(before: &str, rules: &[hba::Rule]) -> Result<Wanted, Fail> {
    let networks: Vec<(IpAddr, u8)> = rules.iter().map(|rule| rule.network.parts()).collect();
    let refused = |why: String| errors::firewall_block(&why);
    let text = block::with(before, MARKS, &block::port_for(PORT, &networks)).map_err(refused)?;
    let earlier = block::decides_first(&text, MARKS, PORT).map_err(refused)?;
    Ok(Wanted { text, earlier })
}

/// Whether `rule` opens the port to the network it names: what
/// [`block::port_for`] writes for each.
fn opens(rule: &Rule) -> bool {
    rule.action == Action::Allow
        && rule.protocol == Protocol::Tcp
        && rule.ports == Some((PORT, PORT))
        && rule.on.is_none()
        && rule.program.is_none()
}

/// What the firewall's file at `path` says of the port. A file that is
/// not there has no block.
///
/// # Errors
///
/// Why the file cannot be read, or has no whole block, in words that start
/// with its path.
pub fn read(path: &Path) -> Result<Firewall, String> {
    let words = |why: &str| format!("{}: {why}", path.display());
    let Some(text) = read_optional(path).map_err(|failed| failed.message)? else {
        return Ok(Firewall::default());
    };
    let Some(rules) = block::rules_of(&text, MARKS).map_err(|why| words(&why))? else {
        return Ok(Firewall::default());
    };
    let networks = rules
        .iter()
        .filter(|rule| opens(rule))
        .filter_map(|rule| rule.from)
        .map(|(address, prefix)| Network::new(address, prefix).to_string())
        .collect();
    let earlier = block::decides_first(&text, MARKS, PORT).map_err(|why| words(&why))?;
    Ok(Firewall { networks, earlier })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(database: &str, network: &str) -> hba::Rule {
        hba::Rule {
            database: database.into(),
            role: database.into(),
            network: Network::parse(network).unwrap(),
        }
    }

    const BLOCK: &str = "# database(8): who reaches PostgreSQL from other machines (ADR 0570)
allow tcp 5432 from 192.168.1.0/24
allow tcp 5432 from 10.0.0.0/8
deny tcp 5432
# database(8): end
";

    #[test]
    fn the_block_has_each_network_of_every_database_once_in_the_order_they_come() {
        let rules = [
            rule("shop", "192.168.1.0/24"),
            rule("blog", "10.0.0.0/8"),
            rule("blog", "192.168.1.0/24"),
        ];
        let made = wanted("", &rules).unwrap();
        assert_eq!(made.text, BLOCK);
        assert_eq!(made.earlier, None);
        assert_eq!(wanted(&made.text, &rules).unwrap().text, made.text);
    }

    #[test]
    fn no_network_left_is_no_block_and_the_rest_of_the_file_is_kept() {
        let before = format!("# mine\nallow tcp 22\n\n{BLOCK}deny udp 5353\n");
        let gone = wanted(&before, &[]).unwrap();
        assert_eq!(gone.text, "# mine\nallow tcp 22\n\ndeny udp 5353\n");
        assert_eq!(wanted("", &[]).unwrap().text, "");
    }

    #[test]
    fn a_rule_of_the_administrator_s_that_comes_first_is_named() {
        let before = format!("allow tcp 5000-6000\n{BLOCK}");
        let made = wanted(&before, &[rule("shop", "10.0.0.0/8")]).unwrap();
        assert_eq!(made.earlier.as_deref(), Some("allow tcp 5000-6000"));
        assert!(made.text.starts_with("allow tcp 5000-6000\n"));
    }

    #[test]
    fn a_file_that_cannot_take_the_block_is_db_108() {
        let broken = "# database(8): end\n";
        let refused = wanted(broken, &[rule("shop", "10.0.0.0/8")]).unwrap_err();
        assert!(refused.message.ends_with("(DB-108)"), "{}", refused.message);
        assert!(refused.message.contains("line 1 ends a block that no line starts"));
        assert!(wanted(broken, &[]).is_err(), "a block to take away is as much a block");
        let full = "allow tcp 22\n".repeat(127);
        let refused = wanted(&full, &[rule("shop", "10.0.0.0/8")]).unwrap_err();
        assert!(refused.message.contains("129 rules"), "{}", refused.message);
    }

    #[test]
    fn what_the_file_says_is_read_from_the_block() {
        let dir = std::env::temp_dir().join(format!("database-port-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("firewall");
        assert_eq!(read(&path).unwrap(), Firewall::default());
        std::fs::write(&path, format!("allow tcp\n{BLOCK}")).unwrap();
        let said = read(&path).unwrap();
        assert_eq!(said.networks, ["192.168.1.0/24", "10.0.0.0/8"]);
        assert_eq!(said.earlier.as_deref(), Some("allow tcp"));
        std::fs::write(&path, "allow tcp 22\n").unwrap();
        assert_eq!(read(&path).unwrap(), Firewall::default());
        std::fs::write(&path, "# database(8): end\n").unwrap();
        let why = read(&path).unwrap_err();
        assert!(why.starts_with(&path.display().to_string()), "{why}");
        assert!(why.contains("ends a block that no line starts"), "{why}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn only_the_rules_that_open_the_port_to_a_network_are_its_networks() {
        let text = "# database(8): who reaches PostgreSQL from other machines (ADR 0570)
allow tcp 5432 from 10.0.0.0/8
allow tcp 5433 from 10.1.0.0/16
allow udp 5432 from 10.2.0.0/16
allow tcp 5432 from 10.3.0.0/16 on net1
allow tcp 5432
allow tcp 5432 from 10.0.0.9
deny tcp 5432
# database(8): end
";
        let dir = std::env::temp_dir().join(format!("database-port-rules-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("firewall");
        std::fs::write(&path, text).unwrap();
        assert_eq!(read(&path).unwrap().networks, ["10.0.0.0/8", "10.0.0.9/32"]);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
