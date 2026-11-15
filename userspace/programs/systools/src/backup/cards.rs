// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The network cards of a machine restored onto other hardware (ADR 0464,
//! backup(8)'s `--card`): which of the backup's cards had addresses of
//! their own, which card of this machine each goes to, and network(5)'s
//! file as the restored machine starts with it.
//!
//! netd sets a card up by the first section of network(5) that names it,
//! by its MAC address or by its name. A card of the old machine had the
//! addresses of the first section that named it; on this machine, that
//! section names its card still if it names the card by a name this
//! machine's card has, and no card if it names a MAC address this machine
//! lacks, which netd then sets up by DHCP. So without a `--card` the
//! restored machine is safe: no card takes an address that may be the old
//! machine's while it is still on the network, unless the section names a
//! card of this machine.
//!
//! `--card OLD=NEW` gives the section of the old card OLD to the card
//! NEW of this machine: its header names NEW's MAC address, and it moves
//! before any section that names NEW otherwise, so that it is the first
//! that names it. `--card OLD=automatic` takes the section's `address`,
//! `router`, `dns` and `dhcp` lines out, so that its card asks DHCP.
//! Every other line stays as it is, comments and order with it.
//!
//! When the restored network is not what it should be, `network show`
//! on the restored machine says how netd read the file, and the file
//! before this change is in the backup.

use std::fmt;

use super::record::{Card, Hardware};
use crate::dhcp_server::mac_of;
use crate::netd::{self, Setup};
use crate::network::set::{key_of, section_name};
use crate::util::{self, Fail};

/// The most `--card` choices a restore takes: one for each card a record
/// lists is more than any machine has.
pub const ASKED_MOST: usize = 64;

/// The lines `--card OLD=automatic` takes out of a section.
const FIXED_KEYS: [&str; 4] = ["address", "router", "dns", "dhcp"];

/// A card of the old machine that had addresses of its own.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Old {
    /// The section of network(5) that set it up, as netd reads its name:
    /// in lower case, a MAC address or a card's name.
    pub section: String,
    /// Its name, if the section or the record says it.
    pub name: Option<String>,
    /// Its MAC address, in lower case, if the section or the record says
    /// it.
    pub mac: Option<String>,
    /// Its addresses, IPv4 then IPv6, as `ADDRESS/PREFIX`.
    pub addresses: Vec<String>,
    /// Its router, IPv4 then IPv6.
    pub router: Option<String>,
}

impl Old {
    /// Whether `wanted`, a MAC address or a name, names this card.
    fn is(&self, wanted: &str) -> bool {
        let wanted = wanted.to_ascii_lowercase();
        self.section == wanted
            || self.mac.as_deref() == Some(wanted.as_str())
            || self.name.as_deref() == Some(wanted.as_str())
    }

    /// How it is named to a person: its name, else its section's.
    fn shown(&self) -> &str {
        self.name.as_deref().unwrap_or(&self.section)
    }
}

/// Where `--card` sends an old card.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    /// To the card of this machine with this MAC address, in lower case.
    Card(String),
    /// To no card: it asks DHCP.
    Automatic,
}

/// One `--card OLD=NEW`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Asked {
    /// The old card, by its MAC address or its name, in lower case.
    pub old: String,
    pub target: Target,
}

impl Asked {
    /// Reads `OLD=NEW` or `OLD=automatic`: OLD a MAC address or a card's
    /// name, NEW a MAC address.
    ///
    /// # Errors
    ///
    /// A usage error that says what the word must be.
    pub fn parse(text: &str) -> Result<Asked, Fail> {
        let wrong = || {
            util::usage(format!(
                "--card {text}: give OLD=NEW or OLD=automatic, OLD a card of the backup by its \
                 MAC address or name, NEW a card of this machine by its MAC address"
            ))
        };
        let (old, new) = text.split_once('=').ok_or_else(wrong)?;
        let (old, new) = (old.trim().to_ascii_lowercase(), new.trim().to_ascii_lowercase());
        if !crate::network::set::is_card(&old) {
            return Err(wrong());
        }
        let target = match new.as_str() {
            "automatic" => Target::Automatic,
            mac if mac_of(mac).is_some() => Target::Card(mac.to_string()),
            _ => return Err(wrong()),
        };
        Ok(Asked { old, target })
    }
}

/// How an old card's addresses are given on this machine.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Choice {
    /// Its section names `new`, which takes its addresses.
    Fixed,
    /// No card takes them: `new`, if any, asks DHCP.
    Automatic,
}

impl fmt::Display for Choice {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        out.write_str(match self {
            Choice::Fixed => "fixed",
            Choice::Automatic => "automatic",
        })
    }
}

/// An old card and where it goes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pair {
    pub old: Old,
    /// The card of this machine it goes to; none when this machine has no
    /// card left for it.
    pub new: Option<Card>,
    pub choice: Choice,
    /// Whether `--card` asked for it, so that the file is changed for it.
    pub asked: bool,
}

/// Whether `setup` gives its card addresses of its own.
fn fixed(setup: &Setup) -> bool {
    !setup.addresses.is_empty() || !setup.addresses6.is_empty()
}

/// The card `old` from the section `section` and how it set it up.
fn old_of(section: &str, setup: &Setup, card: Option<&Card>) -> Old {
    let by_mac = mac_of(section).is_some();
    let addresses = (setup.addresses.iter().map(ToString::to_string))
        .chain(setup.addresses6.iter().map(ToString::to_string))
        .collect();
    let router =
        (setup.router.map(|ip| ip.to_string())).or_else(|| setup.router6.map(|ip| ip.to_string()));
    Old {
        section: section.to_string(),
        name: card.map(|c| c.name.clone()).or_else(|| (!by_mac).then(|| section.to_string())),
        mac: card.map(|c| c.mac.to_ascii_lowercase()).or_else(|| by_mac.then(|| section.into())),
        addresses,
        router,
    }
}

/// The cards of the old machine that had addresses of their own, from the
/// text of its network(5) file and, for a record of format 2, its cards
/// (`then`): for each of its cards in order, the first section that named
/// it, when that gave addresses. Without `then`, each section that gives
/// addresses, in the file's order.
pub fn old_cards(text: &str, then: Option<&Hardware>) -> Vec<Old> {
    let (settings, _) = netd::parse_settings(text);
    let Some(then) = then else {
        return settings
            .iter()
            .filter(|(section, setup)| !section.is_empty() && fixed(setup))
            .map(|(section, setup)| old_of(section, setup, None))
            .collect();
    };
    let mut cards: Vec<Old> = Vec::new();
    for card in &then.cards {
        let Some((section, setup)) = netd::section_of(&settings, &card.name, &card.mac) else {
            continue;
        };
        if fixed(setup) && !cards.iter().any(|old| old.section == *section) {
            cards.push(old_of(section, setup, Some(card)));
        }
    }
    cards
}

/// The words that list `names` after a refusal.
fn listed<'a>(names: impl Iterator<Item = &'a str>) -> String {
    let names: Vec<&str> = names.collect();
    if names.is_empty() { "none".into() } else { names.join(", ") }
}

/// Where each card of `old` goes on this machine, whose cards are `here`,
/// with the network(5) text `text` they were read from, as `asked` says
/// and, for each card it does not name, as the file does: to the card of
/// this machine its section names, else to the next card of this machine
/// that no other takes, with its addresses taken by none.
///
/// # Errors
///
/// A usage error for an OLD that names no card of `old`, or one named
/// twice; a NEW that is not a card here, or that two take; and a NEW
/// whose MAC address another section of the file names already.
pub fn pair(text: &str, old: &[Old], here: &[Card], asked: &[Asked]) -> Result<Vec<Pair>, Fail> {
    if asked.len() > ASKED_MOST {
        return Err(util::usage(format!("--card is given at most {ASKED_MOST} times")));
    }
    let (settings, _) = netd::parse_settings(text);
    let mut chosen: Vec<Option<&Target>> = vec![None; old.len()];
    for ask in asked {
        let Some(at) = old.iter().position(|card| card.is(&ask.old)) else {
            return Err(util::usage(format!(
                "--card {}: the backup has no card with addresses of its own by that name or \
                 MAC address; it has: {}",
                ask.old,
                listed(old.iter().map(|card| card.mac.as_deref().unwrap_or(card.shown())))
            )));
        };
        if chosen[at].replace(&ask.target).is_some() {
            return Err(util::usage(format!("--card names {} twice", old[at].shown())));
        }
    }
    let mut taken: Vec<&str> = Vec::new();
    for target in chosen.iter().flatten() {
        let Target::Card(mac) = target else { continue };
        if !here.iter().any(|card| card.mac.eq_ignore_ascii_case(mac)) {
            return Err(util::usage(format!(
                "--card: this machine has no card with the MAC address {mac}; it has: {}",
                listed(here.iter().map(|card| card.mac.as_str()))
            )));
        }
        if taken.contains(&mac.as_str()) {
            return Err(util::usage(format!("--card gives two cards of the backup to {mac}")));
        }
        if let Some(other) = settings.iter().map(|(section, _)| section).find(|section| {
            *section == mac
                && !old.iter().zip(&chosen).any(|(card, to)| {
                    card.section == **section && *to == Some(&Target::Card(mac.clone()))
                })
        }) {
            return Err(util::usage(format!(
                "--card: the backup's network settings set {mac} up already, in [{other}]"
            )));
        }
        taken.push(mac);
    }
    // The card of this machine that the section of each old card names
    // as the file is, unless a card's section is given to it.
    let named: Vec<Option<&Card>> = old
        .iter()
        .zip(&chosen)
        .map(|(card, to)| {
            if matches!(to, Some(Target::Card(_))) {
                return None;
            }
            here.iter().find(|c| {
                !taken.iter().any(|mac| c.mac.eq_ignore_ascii_case(mac))
                    && netd::section_of(&settings, &c.name, &c.mac)
                        .is_some_and(|(section, _)| *section == card.section)
            })
        })
        .collect();
    taken.extend(named.iter().flatten().map(|card| card.mac.as_str()));
    let mut spare =
        here.iter().filter(|card| !taken.iter().any(|t| card.mac.eq_ignore_ascii_case(t)));
    let mut pairs = Vec::new();
    for ((card, to), named) in old.iter().zip(&chosen).zip(&named) {
        let (new, choice) = match (to, named) {
            (Some(Target::Card(mac)), _) => {
                (here.iter().find(|c| c.mac.eq_ignore_ascii_case(mac)).cloned(), Choice::Fixed)
            }
            // A card its section names keeps it, and asks DHCP if asked to.
            (Some(Target::Automatic), Some(named)) => (Some((*named).clone()), Choice::Automatic),
            (None, Some(named)) => (Some((*named).clone()), Choice::Fixed),
            (_, None) => (spare.next().cloned(), Choice::Automatic),
        };
        pairs.push(Pair { old: card.clone(), new, choice, asked: to.is_some() });
    }
    Ok(pairs)
}

/// The text's lines, each with its line end.
fn lines_of(text: &str) -> Vec<String> {
    text.split_inclusive('\n')
        .map(|line| if line.ends_with('\n') { line.to_string() } else { format!("{line}\n") })
        .collect()
}

/// The first and the line past the last of the section `section` among
/// `lines`: from its header to the next header.
fn block(lines: &[String], section: &str) -> Option<(usize, usize)> {
    let start = lines.iter().position(|line| section_name(line).as_deref() == Some(section))?;
    let end = lines[start + 1..]
        .iter()
        .position(|line| section_name(line).is_some())
        .map_or(lines.len(), |at| start + 1 + at);
    Some((start, end))
}

/// network(5)'s `text` with the changes `pairs` that `--card` asked for:
/// a section given to another card renamed to its MAC address and put
/// first among the sections that name that card; a section given to
/// DHCP without its address lines. The text is as it was when nothing was
/// asked.
pub fn rewritten(text: &str, pairs: &[Pair]) -> String {
    let mut lines = lines_of(text);
    for pair in pairs.iter().filter(|pair| pair.asked) {
        let Some((start, end)) = block(&lines, &pair.old.section) else { continue };
        match (pair.choice, &pair.new) {
            (Choice::Fixed, Some(new)) => {
                let mac = new.mac.to_ascii_lowercase();
                let mut moved: Vec<String> = lines.drain(start..end).collect();
                let shown = pair.old.shown();
                moved[0] = format!("[{mac}]\n");
                moved.insert(
                    1,
                    format!("# {shown}, given to {} by backup restore --card\n", new.name),
                );
                // Before the first section left that names the new card, if
                // one comes before it.
                let (settings, _) = netd::parse_settings(&lines.concat());
                let at = netd::section_of(&settings, &new.name, &mac)
                    .and_then(|(section, _)| block(&lines, section))
                    .map_or(start, |(first, _)| first.min(start));
                if at < start && moved.last().is_some_and(|line| !line.trim().is_empty()) {
                    moved.push("\n".into());
                }
                lines.splice(at..at, moved);
            }
            _ => {
                let kept: Vec<String> = lines[start + 1..end]
                    .iter()
                    .filter(|line| !key_of(line).is_some_and(|key| FIXED_KEYS.contains(&key)))
                    .cloned()
                    .collect();
                lines.splice(start + 1..end, kept);
            }
        }
    }
    let mut out = lines.concat();
    if !text.ends_with('\n') && out.ends_with('\n') && !text.is_empty() {
        out.pop();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn card(name: &str, mac: &str) -> Card {
        Card { name: name.into(), mac: mac.into(), model: None }
    }

    const OLD_NET0: &str = "52:54:00:00:00:01";
    const OLD_NET1: &str = "52:54:00:00:00:02";
    const NEW_NET0: &str = "52:54:00:00:00:a1";
    const NEW_NET1: &str = "52:54:00:00:00:a2";

    /// The old machine's cards.
    fn then() -> Hardware {
        Hardware {
            cards: vec![card("net0", OLD_NET0), card("net1", OLD_NET1)],
            ..Hardware::default()
        }
    }

    /// network(5) as network(8) writes it: the card by its MAC address.
    const BY_MAC: &str = "# The network's settings (network(5)).\n\
        \n\
        [52:54:00:00:00:01]\n\
        # net0, set by network(8)\n\
        address = 192.168.1.12/24\n\
        router = 192.168.1.1\n\
        dns = 192.168.1.1\n\
        metered = no\n\
        \n\
        [52:54:00:00:00:02]\n\
        # net1, set by network(8)\n\
        dhcp = yes\n";

    fn asked(text: &str) -> Asked {
        Asked::parse(text).unwrap()
    }

    #[test]
    fn a_card_choice_is_old_equals_a_mac_address_or_automatic() {
        assert_eq!(
            asked("52:54:00:00:00:01=52:54:00:00:00:A1"),
            Asked { old: OLD_NET0.into(), target: Target::Card(NEW_NET0.into()) }
        );
        assert_eq!(
            asked("Net0=automatic"),
            Asked { old: "net0".into(), target: Target::Automatic }
        );
        for bad in [
            "net0",
            "net0=",
            "=automatic",
            "net0=net1",
            "net0=52:54:00",
            "a b=automatic",
            "net0=dhcp",
        ] {
            let fail = Asked::parse(bad).unwrap_err();
            assert!(fail.usage, "{bad}");
        }
    }

    #[test]
    fn the_old_cards_are_those_their_first_section_gave_addresses() {
        let old = old_cards(BY_MAC, Some(&then()));
        assert_eq!(
            old,
            [Old {
                section: OLD_NET0.into(),
                name: Some("net0".into()),
                mac: Some(OLD_NET0.into()),
                addresses: vec!["192.168.1.12/24".into()],
                router: Some("192.168.1.1".into()),
            }]
        );
        // A section that names a card the machine did not have gave
        // nothing, and one after the first that names a card is not read.
        let text = "[net0]\ndhcp = yes\n[52:54:00:00:00:01]\naddress = 10.0.0.5/8\n\
                    [52:54:00:00:00:99]\naddress = 10.0.0.6/8\n";
        assert_eq!(old_cards(text, Some(&then())), []);
        // Without the record's cards, each section with addresses.
        let old = old_cards("[net0]\naddress = 10.0.0.5/8\naddress = 2001:db8::5/64\n", None);
        assert_eq!(old[0].name.as_deref(), Some("net0"));
        assert_eq!(old[0].mac, None);
        assert_eq!(old[0].addresses, ["10.0.0.5/8", "2001:db8::5/64"]);
        assert_eq!(old_cards(BY_MAC, None)[0].mac.as_deref(), Some(OLD_NET0));
    }

    #[test]
    fn without_a_choice_a_card_by_mac_goes_to_dhcp_and_the_plan_pairs_it_by_order() {
        let old = old_cards(BY_MAC, Some(&then()));
        let here = [card("net0", NEW_NET0)];
        let pairs = pair(BY_MAC, &old, &here, &[]).unwrap();
        assert_eq!(pairs.len(), 1);
        assert_eq!(pairs[0].new, Some(card("net0", NEW_NET0)));
        assert_eq!((pairs[0].choice, pairs[0].asked), (Choice::Automatic, false));
        // Nothing asked, nothing changed.
        assert_eq!(rewritten(BY_MAC, &pairs), BY_MAC);
        // No card is left for it.
        assert_eq!(pair(BY_MAC, &old, &[], &[]).unwrap()[0].new, None);
    }

    #[test]
    fn without_a_choice_a_card_by_name_keeps_the_card_of_that_name() {
        let text = "[net0]\naddress = 192.168.1.12/24\n";
        let old = old_cards(text, Some(&then()));
        let here = [card("net1", NEW_NET1), card("net0", NEW_NET0)];
        let pairs = pair(text, &old, &here, &[]).unwrap();
        assert_eq!(
            (pairs[0].new.as_ref().unwrap().name.as_str(), pairs[0].choice),
            ("net0", Choice::Fixed)
        );
    }

    #[test]
    fn a_card_given_to_a_new_card_is_renamed_to_its_mac_and_keeps_every_other_line() {
        let old = old_cards(BY_MAC, Some(&then()));
        let here = [card("net0", NEW_NET0)];
        let pairs = pair(BY_MAC, &old, &here, &[asked(&format!("net0={NEW_NET0}"))]).unwrap();
        assert_eq!((pairs[0].choice, pairs[0].asked), (Choice::Fixed, true));
        let text = rewritten(BY_MAC, &pairs);
        assert_eq!(
            text,
            "# The network's settings (network(5)).\n\
             \n\
             [52:54:00:00:00:a1]\n\
             # net0, given to net0 by backup restore --card\n\
             # net0, set by network(8)\n\
             address = 192.168.1.12/24\n\
             router = 192.168.1.1\n\
             dns = 192.168.1.1\n\
             metered = no\n\
             \n\
             [52:54:00:00:00:02]\n\
             # net1, set by network(8)\n\
             dhcp = yes\n"
        );
        let (settings, wrong) = netd::parse_settings(&text);
        assert!(wrong.is_empty(), "{wrong:?}");
        let setup = netd::setup_of(&settings, "net0", NEW_NET0);
        assert_eq!(setup.addresses[0].to_string(), "192.168.1.12/24");
    }

    #[test]
    fn a_card_given_to_a_new_card_goes_before_a_section_that_names_it_by_name() {
        let text =
            "[net0]\ndhcp = yes\nmetered = yes\n\n[52:54:00:00:00:02]\naddress = 10.0.0.5/8\n";
        let old = old_cards(text, Some(&then()));
        assert_eq!(old.len(), 1);
        let here = [card("net0", NEW_NET0)];
        let pairs = pair(text, &old, &here, &[asked(&format!("{OLD_NET1}={NEW_NET0}"))]).unwrap();
        let text = rewritten(text, &pairs);
        assert!(text.starts_with("[52:54:00:00:00:a1]\n# net1, given to net0"), "{text}");
        let (settings, wrong) = netd::parse_settings(&text);
        assert!(wrong.is_empty(), "{wrong:?}");
        assert_eq!(
            netd::setup_of(&settings, "net0", NEW_NET0).addresses[0].to_string(),
            "10.0.0.5/8"
        );
        assert!(text.contains("[net0]\ndhcp = yes\nmetered = yes\n"), "{text}");
    }

    #[test]
    fn a_card_given_to_dhcp_loses_its_address_lines_and_keeps_the_rest() {
        let old = old_cards(BY_MAC, Some(&then()));
        let here = [card("net0", NEW_NET0)];
        let pairs = pair(BY_MAC, &old, &here, &[asked("net0=automatic")]).unwrap();
        assert_eq!((pairs[0].choice, pairs[0].asked), (Choice::Automatic, true));
        let text = rewritten(BY_MAC, &pairs);
        assert!(
            text.contains("[52:54:00:00:00:01]\n# net0, set by network(8)\nmetered = no\n\n"),
            "{text}"
        );
        assert!(!text.contains("address") && !text.contains("router"), "{text}");
        // A section that names a card of this machine by name asks DHCP for it.
        let named = "[net0]\naddress = 192.168.1.12/24\ndhcp = no\n";
        let old = old_cards(named, Some(&then()));
        let pairs = pair(named, &old, &here, &[asked("net0=automatic")]).unwrap();
        assert_eq!(pairs[0].new.as_ref().map(|c| c.name.as_str()), Some("net0"));
        assert_eq!(rewritten(named, &pairs), "[net0]\n");
    }

    #[test]
    fn a_choice_that_cannot_be_carried_out_is_a_usage_error_that_lists_what_there_is() {
        let old = old_cards(BY_MAC, Some(&then()));
        let here = [card("net0", NEW_NET0), card("net1", NEW_NET1)];
        let refused = |asks: &[&str]| {
            let asks: Vec<Asked> = asks.iter().map(|a| asked(a)).collect();
            let fail = pair(BY_MAC, &old, &here, &asks).unwrap_err();
            assert!(fail.usage, "{}", fail.message);
            fail.message
        };
        assert!(refused(&["net1=automatic"]).ends_with(&format!("it has: {OLD_NET0}")));
        assert!(refused(&["net0=52:54:00:00:00:ff"]).contains(&format!("{NEW_NET0}, {NEW_NET1}")));
        assert!(refused(&["net0=automatic", &format!("{OLD_NET0}={NEW_NET0}")]).contains("twice"));
        let many: Vec<Asked> = (0..=ASKED_MOST).map(|_| asked("net0=automatic")).collect();
        assert!(pair(BY_MAC, &old, &here, &many).unwrap_err().usage);
        // A section of the file names the new card's MAC address already.
        let text = format!("{BY_MAC}\n[{NEW_NET1}]\nmetered = yes\n");
        let old = old_cards(&text, Some(&then()));
        let fail = pair(&text, &old, &here, &[asked(&format!("net0={NEW_NET1}"))]).unwrap_err();
        assert!(fail.message.contains("already"), "{}", fail.message);
    }

    #[test]
    fn two_old_cards_cannot_go_to_one_new_card() {
        let text = "[52:54:00:00:00:01]\naddress = 10.0.0.5/8\n[52:54:00:00:00:02]\naddress = 10.0.1.5/8\n";
        let old = old_cards(text, Some(&then()));
        let here = [card("net0", NEW_NET0)];
        let both = [asked(&format!("net0={NEW_NET0}")), asked(&format!("net1={NEW_NET0}"))];
        assert!(pair(text, &old, &here, &both).unwrap_err().message.contains("two cards"));
        // Unasked, the second has no card left.
        let pairs = pair(text, &old, &here, &[]).unwrap();
        assert_eq!(pairs[1].new, None);
    }

    #[test]
    fn a_file_without_a_last_line_end_keeps_it_so() {
        let text = "[52:54:00:00:00:01]\naddress = 10.0.0.5/8\nmetered = no";
        let old = old_cards(text, Some(&then()));
        let pairs =
            pair(text, &old, &[card("net0", NEW_NET0)], &[asked("net0=automatic")]).unwrap();
        assert_eq!(rewritten(text, &pairs), "[52:54:00:00:00:01]\nmetered = no");
    }
}
