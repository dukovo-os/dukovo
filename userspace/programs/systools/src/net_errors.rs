// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The refusals of network(8) and firewall(8) that have an ID of their own,
//! NET-176 to NET-181, each as the failure a command exits with. The ID ends
//! the line in parentheses, as the other tools' IDs do (`BOOT-102`): the web
//! console reads it from the last line of the command's error output when
//! its action names it, and the support center's page for it is found by
//! it. A command prints no `NET-1xx` of its own: it returns one of these, so
//! that the terminal, the API and the page say the same words.
//!
//! The words do not start with the command's name: `main` puts it before the
//! message.

use crate::util::{Fail, fail};

/// NET-176: `network set` while the `network` trial runs. Spark keeps one
/// trial for a file, and its undo would put back the file as it was before
/// the trial, so a change made now would be lost.
pub fn network_is_tried() -> Fail {
    fail(
        "the network settings are being tried: keep or undo that change first, with `network \
         keep` or `network undo` (NET-176)"
            .into(),
    )
}

/// NET-177: a change of the firewall's rules while the `firewall` trial
/// runs.
pub fn firewall_is_tried() -> Fail {
    fail(
        "the firewall's rules are being tried: keep or undo that change first, with `firewall \
         keep` or `firewall undo` (NET-177)"
            .into(),
    )
}

/// NET-178: `network set CARD dhcp` for a card that serves DHCP, which has
/// an address of its own and cannot ask for one.
pub fn serves_dhcp(card: &str) -> Fail {
    fail(format!(
        "{card} serves DHCP, so it needs an address of its own and cannot ask DHCP for one: \
         remove the dhcp-server settings of its section in /system/settings/network first \
         (NET-178)"
    ))
}

/// NET-178 too: `network set CARD fixed …` for a card that serves DHCP
/// from `first` to `last`, when none of the new addresses has that range
/// on its network, so that the server could not run.
pub fn range_outside(card: &str, first: std::net::Ipv4Addr, last: std::net::Ipv4Addr) -> Fail {
    fail(format!(
        "{card} serves DHCP from {first} to {last}, so one of its addresses must be on that \
         range's network: give it one, or change the dhcp-server settings of its section in \
         /system/settings/network first (NET-178)"
    ))
}

/// NET-179: `keep` or `undo` with no trial. `what` is what would have been
/// tried: "the network settings" or "the firewall's rules".
pub fn nothing_is_tried(what: &str) -> Fail {
    fail(format!(
        "{what} are not being tried: nothing waits to be kept or undone, and a change that was \
         not tried is in force already (NET-179)"
    ))
}

/// NET-180: `firewall remove` or `replace` by words that no rule has.
pub fn no_such_rule(words: &str) -> Fail {
    fail(format!("no rule has the words `{words}`; `firewall list` shows the rules (NET-180)"))
}

/// NET-181: `network show` or `network set` for a card the machine does not
/// have.
pub fn no_such_card(card: &str) -> Fail {
    fail(format!(
        "there is no network card called {card}; `network show` lists the cards (NET-181)"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_refusal_ends_with_its_id_and_exits_with_status_1() {
        let refusals = [
            (network_is_tried(), "NET-176"),
            (firewall_is_tried(), "NET-177"),
            (serves_dhcp("net1"), "NET-178"),
            (nothing_is_tried("the network settings"), "NET-179"),
            (no_such_rule("allow tcp 22"), "NET-180"),
            (no_such_card("net9"), "NET-181"),
        ];
        for (refusal, id) in refusals {
            assert!(refusal.message.ends_with(&format!(" ({id})")), "{}", refusal.message);
            assert_eq!(refusal.message.lines().count(), 1, "{id}");
            assert_eq!((refusal.code, refusal.usage), (1, false), "{id}");
        }
    }

    #[test]
    fn the_words_name_what_was_asked_for_and_what_to_run() {
        assert!(serves_dhcp("net1").message.starts_with("net1 serves DHCP"));
        assert!(no_such_rule("allow tcp 22").message.contains("`allow tcp 22`"));
        assert!(no_such_card("net9").message.starts_with("there is no network card called net9"));
        assert!(nothing_is_tried("the firewall's rules").message.starts_with("the firewall's"));
        assert!(network_is_tried().message.contains("network keep"));
        assert!(firewall_is_tried().message.contains("firewall undo"));
    }
}
